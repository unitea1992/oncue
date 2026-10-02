//! VRChat SDK直接使用認証クライアント
//!
//! メモリ認証を正本とする。保存Cookieの復元は起動時の初回のみで、
//! 通常コマンドでは保存Cookieを再投入しない。認証の更新は直列化し、
//! 通信awaitは状態ロックの外で行う。Cookie確立後はBasic資格情報を解放する。

use crate::domain::models::AppError;
use crate::infra::http_client::{CurrentUser, HttpClient, FORBIDDEN_MESSAGE};
use crate::services::APP_USER_AGENT;
use crate::storage::{SessionData, SessionStore};
use reqwest::cookie::{CookieStore, Jar};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use vrchatapi::apis::{authentication_api, configuration::Configuration};
use vrchatapi::models::{TwoFactorAuthCode, TwoFactorAuthType, TwoFactorEmailCode};

/// verify_two_factorのmethod名。frontendのTwoFactorMethodと一致させる。
pub const TWO_FACTOR_TOTP: &str = "totp";
/// 2FAコード不一致の表示文。
const INVALID_TWO_FACTOR_CODE: &str =
    "認証コードが正しくないか、有効期限が切れています。\n新しいコードを入力してください。";
pub const TWO_FACTOR_EMAIL_OTP: &str = "emailOtp";

/// method名を正規化する。SDKの"otp"はTOTP扱い。未知はNoneで対応不可を明示する。
pub fn normalize_two_factor_method(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "totp" | "otp" => Some(TWO_FACTOR_TOTP),
        "emailotp" => Some(TWO_FACTOR_EMAIL_OTP),
        _ => None,
    }
}

fn normalize_sdk_methods(types: &[TwoFactorAuthType]) -> Vec<String> {
    let mut methods: Vec<String> = types
        .iter()
        .map(|t| match t {
            TwoFactorAuthType::EmailOtp => TWO_FACTOR_EMAIL_OTP.to_string(),
            _ => TWO_FACTOR_TOTP.to_string(),
        })
        .collect();
    methods.sort();
    methods.dedup();
    if methods.is_empty() {
        methods.push(TWO_FACTOR_TOTP.to_string());
    }
    methods
}

/// ログイン結果
#[derive(Debug, Clone)]
pub enum SdkLoginResult {
    Success {
        user: CurrentUser,
    },
    RequiresTwoFactor {
        username: String,
        methods: Vec<String>,
    },
}

/// VRChat SDK直接使用認証クライアント
/// SDK準拠: Configuration::default() を使用しシンプルに実装
pub struct SdkAuthClient {
    config: Mutex<Configuration>,
    session_store: Arc<SessionStore>,
    cookie_jar: Arc<Jar>, // SDKのCookieJarにアクセスするため保持
    /// 認証の参照・更新(login/verify/logout/復元/伝搬)を直列化する。
    /// 取得順はauth_op→HTTPで固定し、逆順取得はしない。
    auth_op: Mutex<()>,
    /// 保存Cookieの復元は起動時の初回だけ
    restored: AtomicBool,
    /// メモリ正本の利用者情報
    cached_user: Mutex<Option<CurrentUser>>,
}

impl SdkAuthClient {
    /// 新しいSdkAuthClientインスタンスを作成
    pub fn new(session_store: Arc<SessionStore>) -> Result<Self, AppError> {
        // CookieJarを作成し、クライアントに設定
        let cookie_jar = Arc::new(Jar::default());
        let client = reqwest::Client::builder()
            .cookie_provider(cookie_jar.clone())
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| {
                AppError::Network(format!("HTTPクライアントの作成に失敗しました: {}", e))
            })?;

        // SDK準拠: Configuration::default() を使用しつつ必要なフィールドを設定
        // Configurationは#[non_exhaustive]のため、既定値から必要な項目だけ上書きする。
        let mut config = Configuration::default();
        config.user_agent = Some(APP_USER_AGENT.to_string());
        config.client = client.into();

        Ok(Self {
            config: Mutex::new(config),
            session_store,
            cookie_jar,
            auth_op: Mutex::new(()),
            restored: AtomicBool::new(false),
            cached_user: Mutex::new(None),
        })
    }

    /// CookieJarからauth cookieを抽出
    pub fn current_auth_cookie(&self) -> Option<String> {
        let url = "https://api.vrchat.cloud/api/1".parse().ok()?;
        let cookies = self.cookie_jar.cookies(&url)?;
        let cookie_str = cookies.to_str().ok()?;

        // "auth=xxx;..." からauth cookieを抽出
        cookie_str.split(';').find_map(|part| {
            let part = part.trim();
            part.strip_prefix("auth=").map(|s| s.to_string())
        })
    }

    /// CookieJarにauth cookieを設定
    fn set_auth_cookie(&self, cookie_value: &str) {
        let url = "https://api.vrchat.cloud/api/1".parse().unwrap();
        let cookie = format!("auth={}; Domain=api.vrchat.cloud; Path=/", cookie_value);
        self.cookie_jar.add_cookie_str(&cookie, &url);
    }

    fn clear_auth_cookie(&self) {
        let url = "https://api.vrchat.cloud/api/1".parse().unwrap();
        let cookie =
            "auth=; Domain=api.vrchat.cloud; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT";
        self.cookie_jar.add_cookie_str(cookie, &url);
    }

    /// 新規login開始時の取り違え防止。旧Cookieとメモリ正本を捨てる。
    /// 保存セッションは残し、確立・失敗後の永続化方針に従う。
    pub async fn reset_for_new_login(&self) {
        let _op = self.auth_op.lock().await;
        self.clear_auth_cookie();
        *self.cached_user.lock().await = None;
    }

    /// メモリ正本の利用者情報を返す
    pub async fn cached_user(&self) -> Option<CurrentUser> {
        self.cached_user.lock().await.clone()
    }

    /// セッションを保存（実際のauth cookieを抽出して保存）
    fn save_session(&self, user: &vrchatapi::models::CurrentUser) -> Result<(), AppError> {
        // SDKのCookieJarからauth cookieを抽出
        let auth_cookie = self.current_auth_cookie().ok_or_else(|| {
            error!("認証Cookieの抽出に失敗");
            AppError::Auth(
                "セッションの作成に失敗しました。認証Cookieを取得できませんでした。".to_string(),
            )
        })?;

        let session = SessionData {
            auth_cookie,
            user_id: user.id.clone(),
            username: user
                .username
                .clone()
                .unwrap_or_else(|| user.display_name.clone()),
            profile_icon_url: select_profile_icon(user),
            created_at: chrono::Utc::now(),
        };

        self.session_store.save(&session).map_err(|e| {
            error!("セッション保存に失敗: {}", e);
            AppError::Storage(format!(
                "認証は成功しましたが、セッションの保存に失敗しました: {}",
                e
            ))
        })?;

        info!("セッションを保存しました: user_id={}", user.id);
        Ok(())
    }

    fn persist_session_if_requested(
        &self,
        user: &vrchatapi::models::CurrentUser,
        remember_session: bool,
    ) -> Result<(), AppError> {
        if remember_session {
            self.save_session(user)
        } else {
            self.session_store.clear().map_err(|e| {
                AppError::Storage(format!("保存済みセッションの削除に失敗しました: {}", e))
            })
        }
    }

    /// 認証確立の後始末。平文資格情報は `login` 内のスナップショットに閉じて捨て、
    /// 稼働中の設定には置かない。保存方針に従って永続化、メモリ正本を更新する。
    /// 呼び出し元は `auth_op` を保持していること。
    async fn finish_authenticated_user(
        &self,
        user: vrchatapi::models::CurrentUser,
        remember_session: bool,
    ) -> Result<CurrentUser, AppError> {
        // 保存に失敗したらJarに残ったCookieも捨て、中途半端な確立を残さない。
        if let Err(e) = self.persist_session_if_requested(&user, remember_session) {
            self.clear_auth_cookie();
            return Err(e);
        }
        let current = CurrentUser::from(user);
        *self.cached_user.lock().await = Some(current.clone());
        Ok(current)
    }

    /// 2FA検証のSDKエラー変換。VRChatはコード不一致を400/401（`{"verified":false}`）で
    /// 返すため、これを入力ミスとして案内する。それ以外は通常の変換に任せる。
    fn convert_two_factor_error<T>(&self, e: vrchatapi::apis::Error<T>) -> AppError {
        if let vrchatapi::apis::Error::ResponseError(resp) = &e {
            if matches!(resp.status.as_u16(), 400 | 401) {
                return AppError::Auth(INVALID_TWO_FACTOR_CODE.to_string());
            }
        }
        self.convert_sdk_error(e)
    }

    /// SDKエラーをAppErrorに変換
    fn convert_sdk_error<T>(&self, e: vrchatapi::apis::Error<T>) -> AppError {
        match e {
            vrchatapi::apis::Error::ResponseError(resp) => {
                // 応答本文（英語のJSON）は画面へ出さず、HTTPステータスだけを手掛かりに残す。
                let status = resp.status.as_u16();
                warn!("VRChat APIエラー: status={} body={}", status, resp.content);

                match status {
                    401 => AppError::Auth(
                        "ユーザー名またはパスワードが正しくありません。".to_string(),
                    ),
                    403 => AppError::Forbidden(FORBIDDEN_MESSAGE.to_string()),
                    429 => AppError::rate_limited(None),
                    code if code >= 500 => AppError::Api(format!(
                        "VRChatサーバーでエラーが発生しました（HTTP {}）。しばらく待ってから再試行してください。",
                        code
                    )),
                    _ => AppError::Api(format!(
                        "VRChat APIへの要求が受け付けられませんでした（HTTP {}）。",
                        status
                    )),
                }
            }
            vrchatapi::apis::Error::Reqwest(req_err) => {
                if req_err.is_timeout() {
                    AppError::Network(
                        "接続がタイムアウトしました。インターネット接続を確認してください。"
                            .to_string(),
                    )
                } else if req_err.is_connect() {
                    AppError::Network(
                        "サーバーに接続できません。インターネット接続を確認してください。"
                            .to_string(),
                    )
                } else {
                    warn!("ネットワークエラー: {}", req_err);
                    AppError::Network(
                        "VRChatに接続できませんでした。インターネット接続を確認してください。"
                            .to_string(),
                    )
                }
            }
            vrchatapi::apis::Error::ReqwestMiddleware(middleware_err) => {
                if middleware_err.is_timeout() {
                    AppError::Network(
                        "接続がタイムアウトしました。インターネット接続を確認してください。"
                            .to_string(),
                    )
                } else if middleware_err.is_connect() {
                    AppError::Network(
                        "サーバーに接続できません。インターネット接続を確認してください。"
                            .to_string(),
                    )
                } else {
                    warn!("ネットワークエラー: {}", middleware_err);
                    AppError::Network(
                        "VRChatに接続できませんでした。インターネット接続を確認してください。"
                            .to_string(),
                    )
                }
            }
            vrchatapi::apis::Error::Serde(serde_err) => {
                warn!("応答の解析に失敗: {}", serde_err);
                AppError::Api(
                    "VRChatからの応答を読み取れませんでした。VRChat側の仕様変更の可能性があります。"
                        .to_string(),
                )
            }
            _ => AppError::Api("予期しないエラーが発生しました".to_string()),
        }
    }

    /// ユーザー名とパスワードでログイン
    ///
    /// GET /auth/user を使用してBasic認証でログイン (SDK準拠の実装)。
    /// 通信awaitはconfigロックの外で行い、認証の更新だけ直列化する。
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        remember_session: bool,
    ) -> Result<SdkLoginResult, AppError> {
        info!("SDKログインを開始: username={}", username);

        let _op = self.auth_op.lock().await;

        // ロック保持は設定の複写だけに留め、通信は外で行う
        let attempt = {
            let live = self.config.lock().await;
            let mut snapshot = live.clone();
            snapshot.basic_auth = Some((username.to_string(), Some(password.to_string())));
            snapshot
        };

        // GET /auth/user を使用してログイン
        let result = authentication_api::get_current_user(&attempt).await;

        match result {
            Ok(vrchatapi::models::RegisterUserAccount200Response::RequiresTwoFactorAuth(
                pending,
            )) => {
                // 検証呼び出しはCookieJarの保留Cookieで行う。平文資格情報は
                // 稼働中の設定へ書き戻さず、このスナップショットと共に捨てる。
                info!("2FAが必要です");
                Ok(SdkLoginResult::RequiresTwoFactor {
                    username: username.to_string(),
                    methods: normalize_sdk_methods(&pending.requires_two_factor_auth),
                })
            }
            Ok(vrchatapi::models::RegisterUserAccount200Response::CurrentUser(user)) => {
                info!("ログイン成功: {}", user.display_name);
                let current = self
                    .finish_authenticated_user(user, remember_session)
                    .await?;
                Ok(SdkLoginResult::Success { user: current })
            }
            Err(e) => {
                error!("SDKログインエラー: {:?}", e);
                // 失敗時の保留Cookieを残さず、次の試行・放棄時の中途半端な復活を断つ。
                self.clear_auth_cookie();
                Err(self.convert_sdk_error(e))
            }
        }
    }

    /// 2FAコードを検証する。methodは"totp"または"emailOtp"(未知は対応不可)。
    pub async fn verify_two_factor(
        &self,
        code: &str,
        method: &str,
        remember_session: bool,
    ) -> Result<CurrentUser, AppError> {
        let normalized = normalize_two_factor_method(method).ok_or_else(|| {
            AppError::InvalidInput(format!("対応していない2FA方式です: {}", method))
        })?;
        info!("2FA検証を開始: method={}", normalized);

        let _op = self.auth_op.lock().await;
        // 通信はロックの外。設定の複写だけ保持する
        let snapshot = self.config.lock().await.clone();
        let code = code.trim().to_string();

        if normalized == TWO_FACTOR_EMAIL_OTP {
            let verify_result =
                authentication_api::verify2_fa_email_code(&snapshot, TwoFactorEmailCode::new(code))
                    .await
                    .map_err(|e| self.convert_two_factor_error(e))?;
            if !verify_result.verified {
                return Err(AppError::Auth(INVALID_TWO_FACTOR_CODE.to_string()));
            }
        } else {
            let two_factor_code = TwoFactorAuthCode { code };
            let verify_result = authentication_api::verify2_fa(&snapshot, two_factor_code)
                .await
                .map_err(|e| self.convert_two_factor_error(e))?;
            if !verify_result.verified {
                return Err(AppError::Auth(INVALID_TWO_FACTOR_CODE.to_string()));
            }
        }

        info!("2FA検証成功");

        // SDK準拠: 再度get_current_userを呼んでユーザー情報を取得
        let user_result = authentication_api::get_current_user(&snapshot)
            .await
            .map_err(|e| self.convert_sdk_error(e))?;

        match user_result {
            vrchatapi::models::RegisterUserAccount200Response::CurrentUser(user) => {
                info!("2FAフロー完了: {}", user.display_name);
                self.finish_authenticated_user(user, remember_session).await
            }
            vrchatapi::models::RegisterUserAccount200Response::RequiresTwoFactorAuth(_) => Err(
                AppError::Auth("予期しない認証状態です。再度ログインしてください。".to_string()),
            ),
        }
    }

    /// セッション状態を返す。保存Cookieの復元は起動時の初回だけ行い、
    /// 以降はメモリ正本だけを見る。通常コマンドで保存Cookieを再投入しない。
    /// 参照・復元はauth_op配下で直列化し、logoutとの干渉を断つ。
    pub async fn check_existing_session(&self) -> Result<Option<CurrentUser>, AppError> {
        let _op = self.auth_op.lock().await;

        // メモリ正本があればそれだけを見る
        if self.current_auth_cookie().is_some() {
            return Ok(self.cached_user.lock().await.clone());
        }

        // 復元済みなら保存を見に行かない(one-shot)
        if self.restored.swap(true, Ordering::SeqCst) {
            return Ok(self.cached_user.lock().await.clone());
        }

        info!("セッション復元を開始");

        if !self.session_store.exists() {
            info!("セッションファイルが存在しません");
            return Ok(None);
        }

        let session = match self.session_store.load() {
            Ok(s) => {
                info!("セッションを復元しました: user_id={}", s.user_id);
                s
            }
            Err(e) => {
                let error_msg = match &e {
                    crate::storage::SessionError::NotFound => {
                        "セッションファイルが見つかりません".to_string()
                    }
                    crate::storage::SessionError::InvalidData => {
                        "セッションデータが破損しています".to_string()
                    }
                    crate::storage::SessionError::Crypto(_) => {
                        "セッションの復号化に失敗しました（別PCでの実行など）".to_string()
                    }
                    _ => format!("セッション読み込みエラー: {}", e),
                };

                info!("{}", error_msg);

                if let Err(clear_err) = self.session_store.clear() {
                    return Err(AppError::Storage(format!(
                        "{} (セッションクリアも失敗: {})",
                        error_msg, clear_err
                    )));
                }

                return Ok(None);
            }
        };

        // セッション有効期限チェック（30日）
        let session_age = chrono::Utc::now() - session.created_at;
        if session_age.num_days() > 30 {
            info!(
                "セッションが期限切れです（{}日経過）",
                session_age.num_days()
            );
            if let Err(e) = self.session_store.clear() {
                return Err(AppError::Storage(format!(
                    "期限切れセッションのクリアに失敗: {}",
                    e
                )));
            }
            return Ok(None);
        }

        // auth_op配下のためlogoutとの干渉はなく、世代guardは不要。
        // CookieJarに復元。起動時は過剰な /auth 検証を避け、保存済みメタデータでUIを復元する。
        self.set_auth_cookie(&session.auth_cookie);
        let user = CurrentUser {
            id: session.user_id,
            username: session.username.clone(),
            display_name: session.username,
            two_factor_auth_enabled: false,
            profile_icon_url: session.profile_icon_url.clone(),
            presence: None,
        };
        *self.cached_user.lock().await = Some(user.clone());
        Ok(Some(user))
    }

    /// メモリCookieで/auth/userを1回解決する。Jar副作用のためauth_opで統一する。
    pub async fn get_current_user_from_memory(&self) -> Result<Option<CurrentUser>, AppError> {
        let _op = self.auth_op.lock().await;
        if self.current_auth_cookie().is_none() {
            return Ok(None);
        }

        let snapshot = self.config.lock().await.clone();
        match authentication_api::get_current_user(&snapshot).await {
            Ok(vrchatapi::models::RegisterUserAccount200Response::CurrentUser(user)) => {
                let current = CurrentUser::from(user);
                *self.cached_user.lock().await = Some(current.clone());
                Ok(Some(current))
            }
            Ok(vrchatapi::models::RegisterUserAccount200Response::RequiresTwoFactorAuth(_)) => {
                Ok(None)
            }
            Err(e) => Err(self.convert_sdk_error(e)),
        }
    }

    /// ログアウト処理 (SDK準拠)。監視停止は呼出側(commands::auth::logout)が先に行う。
    /// auth_op配下でJar・正本・保存を一括クリアし、遅い応答の復活を断つ。
    pub async fn logout(&self) -> Result<(), AppError> {
        info!("ログアウト処理を開始");

        let _op = self.auth_op.lock().await;

        let snapshot = self.config.lock().await.clone();
        let _ = authentication_api::logout(&snapshot).await;

        // 稼働中の設定に平文資格情報は置かない方針のため、消す対象はCookieと正本のみ。
        self.clear_auth_cookie();
        *self.cached_user.lock().await = None;

        // セッションをクリア
        self.session_store.clear().map_err(|e| {
            error!("セッションクリアに失敗: {}", e);
            AppError::Storage(format!("セッションの削除に失敗しました: {}", e))
        })?;

        info!("ログアウト完了");
        Ok(())
    }

    /// HTTP jarをSDK jarの現状へ同期する。取得順はauth_op→HTTPで固定。
    /// SDK側にCookieがなければHTTP側もクリアし、旧値の再投入を防ぐ。
    pub async fn propagate_cookie_to(
        &self,
        http_client: &Arc<tokio::sync::Mutex<HttpClient>>,
    ) -> Result<(), AppError> {
        let _op = self.auth_op.lock().await;
        let cookie = self.current_auth_cookie();
        let client = http_client.lock().await;
        client.clear_auth();
        match cookie {
            Some(value) => {
                client.with_auth_cookie(&value);
                Ok(())
            }
            None => Err(AppError::Auth("認証Cookieが見つかりません".to_string())),
        }
    }

    /// HTTP jarのクリアを認証更新と直列化する(logout後の旧値復活を防ぐ)。
    pub async fn clear_http_cookie(&self, http_client: &Arc<tokio::sync::Mutex<HttpClient>>) {
        let _op = self.auth_op.lock().await;
        http_client.lock().await.clear_auth();
    }
}

/// SDK 1.21以降、プロフィール画像は `iconUrl` と `presence` 側へ移ったため、
/// 新しい項目を優先し、最後にアバター画像へフォールバックする。
fn select_profile_icon(user: &vrchatapi::models::CurrentUser) -> Option<String> {
    let presence = user.presence.as_ref();
    [
        user.icon_url.as_deref(),
        presence.and_then(|p| p.profile_pic_override.as_ref()?.as_deref()),
        presence.and_then(|p| p.user_icon.as_ref()?.as_deref()),
        Some(user.current_avatar_thumbnail_image_url.as_str()),
        Some(user.current_avatar_image_url.as_str()),
    ]
    .into_iter()
    .flatten()
    .find(|s| !s.trim().is_empty())
    .map(str::to_string)
}

impl From<vrchatapi::models::CurrentUser> for CurrentUser {
    fn from(user: vrchatapi::models::CurrentUser) -> Self {
        // 構造体リテラルでの部分moveより先に借用を済ませる。
        let profile_icon_url = select_profile_icon(&user);
        Self {
            id: user.id,
            // usernameはOption<String>なので、なければdisplay_nameを使用
            username: user.username.unwrap_or_else(|| user.display_name.clone()),
            display_name: user.display_name,
            two_factor_auth_enabled: user.two_factor_auth_enabled,
            profile_icon_url,
            presence: user.presence.map(|p| crate::infra::http_client::Presence {
                world: p.world.unwrap_or_default(),
                instance: p.instance.flatten().unwrap_or_default(),
                traveling_to_world: p.traveling_to_world,
                traveling_to_instance: p.traveling_to_instance.flatten(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_client() -> (SdkAuthClient, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let session_store = Arc::new(SessionStore::with_custom_path(temp_dir.path()));
        let client = SdkAuthClient::new(session_store).expect("テスト用クライアントの作成に失敗");
        (client, temp_dir)
    }

    #[test]
    fn two_factor_method_contract_table() {
        // method正規化とSDK enum→契約値の対応を1群で確認する。
        let cases: Vec<(&str, Option<&str>)> = vec![
            ("totp", Some("totp")),
            ("otp", Some("totp")),
            ("emailOtp", Some("emailOtp")),
            ("EMAILOTP", Some("emailOtp")),
            ("sms", None),
            ("", None),
        ];
        for (raw, expected) in cases {
            assert_eq!(normalize_two_factor_method(raw), expected, "raw={:?}", raw);
        }
        assert_eq!(
            normalize_sdk_methods(&[TwoFactorAuthType::Totp]),
            vec!["totp".to_string()]
        );
        assert_eq!(
            normalize_sdk_methods(&[TwoFactorAuthType::EmailOtp]),
            vec!["emailOtp".to_string()]
        );
        // SDKのotpはtotpへ正規化し、重複は除く
        assert_eq!(
            normalize_sdk_methods(&[TwoFactorAuthType::Totp, TwoFactorAuthType::Otp]),
            vec!["totp".to_string()]
        );
        assert_eq!(
            normalize_sdk_methods(&[TwoFactorAuthType::EmailOtp, TwoFactorAuthType::Totp]),
            vec!["emailOtp".to_string(), "totp".to_string()]
        );
    }
    #[test]
    fn profile_icon_prefers_icon_url_then_presence_then_avatar() {
        let presence = vrchatapi::models::CurrentUserPresence {
            profile_pic_override: Some(Some("https://example.com/override.png".to_string())),
            user_icon: Some(Some("https://example.com/user-icon.png".to_string())),
            ..Default::default()
        };
        let mut user = vrchatapi::models::CurrentUser {
            icon_url: Some("https://example.com/icon.png".to_string()),
            presence: Some(presence),
            current_avatar_thumbnail_image_url: "https://example.com/avatar.png".to_string(),
            ..Default::default()
        };
        assert_eq!(
            select_profile_icon(&user).as_deref(),
            Some("https://example.com/icon.png")
        );
        user.icon_url = Some("  ".to_string());
        assert_eq!(
            select_profile_icon(&user).as_deref(),
            Some("https://example.com/override.png")
        );
        user.presence.as_mut().unwrap().profile_pic_override = Some(None);
        assert_eq!(
            select_profile_icon(&user).as_deref(),
            Some("https://example.com/user-icon.png")
        );
        user.presence = None;
        assert_eq!(
            select_profile_icon(&user).as_deref(),
            Some("https://example.com/avatar.png")
        );
        user.current_avatar_thumbnail_image_url.clear();
        user.current_avatar_image_url.clear();
        assert_eq!(select_profile_icon(&user), None);
        let converted = CurrentUser::from(user);
        assert_eq!(converted.profile_icon_url, None);
    }

    fn response_error(status: u16, body: &str) -> vrchatapi::apis::Error<()> {
        vrchatapi::apis::Error::ResponseError(vrchatapi::apis::ResponseContent {
            status: reqwest::StatusCode::from_u16(status).unwrap(),
            content: body.to_string(),
            entity: None,
        })
    }

    #[test]
    fn two_factor_rejection_is_shown_as_invalid_code_without_raw_body() {
        let (client, _temp) = create_test_client();
        for status in [400, 401] {
            let message = client
                .convert_two_factor_error(response_error(status, r#"{"verified":false}"#))
                .to_string();
            assert_eq!(message, INVALID_TWO_FACTOR_CODE);
        }
    }

    #[test]
    fn sdk_errors_do_not_expose_response_body() {
        let (client, _temp) = create_test_client();
        for status in [400, 401, 403, 404, 500, 503] {
            let message = client
                .convert_sdk_error(response_error(
                    status,
                    r#"{"error":{"message":"raw-body"}}"#,
                ))
                .to_string();
            assert!(!message.contains("raw-body"), "{status}: {message}");
            assert!(!message.contains("error"), "{status}: {message}");
        }
    }

    // test_cookie_extraction は setter→getter の自己検証のため削除。
    // test_unknown_two_factor_method_is_rejected は正規化表と verify 冒頭分岐の
    // 重複のため削除（sms/空の拒否は上記表が担う）。

    #[tokio::test]
    #[cfg(target_os = "windows")]
    async fn test_session_save_load() {
        let (client, _temp) = create_test_client();

        // ダミーユーザーとセッション（最小限のフィールドのみ設定）
        let user = vrchatapi::models::CurrentUser {
            id: "usr_test".to_string(),
            username: Some("testuser".to_string()),
            display_name: "Test User".to_string(),
            two_factor_auth_enabled: false,
            accepted_tos_version: 0,
            age_verification_status: vrchatapi::models::AgeVerificationStatus::hidden,
            age_verified: false,
            allow_avatar_copying: false,
            current_avatar: "".to_string(),
            current_avatar_image_url: "".to_string(),
            current_avatar_tags: vec![],
            current_avatar_thumbnail_image_url: "".to_string(),
            icon_url: Some("https://example.com/icon.png".to_string()),
            date_joined: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            developer_type: vrchatapi::models::DeveloperType::None,
            email_verified: false,
            friend_group_names: vec![],
            friend_key: "".to_string(),
            friends: vec![],
            has_birthday: false,
            has_email: false,
            has_logged_in_from_client: false,
            has_pending_email: false,
            home_location: "".to_string(),
            is_adult: false,
            is_friend: false,
            state: vrchatapi::models::UserState::Offline,
            status: vrchatapi::models::UserStatus::Offline,
            tags: vec![],
            ..Default::default()
        };

        // セッション保存
        client.set_auth_cookie("test_cookie_value");
        client.save_session(&user).unwrap();

        // セッションが保存されたことを確認
        assert!(client.session_store.exists());

        // 読み込み（画像URLも復元される）
        let loaded = client.session_store.load().unwrap();
        assert_eq!(loaded.auth_cookie, "test_cookie_value");
        assert_eq!(loaded.user_id, "usr_test");
        assert_eq!(loaded.username, "testuser");
        assert_eq!(
            loaded.profile_icon_url.as_deref(),
            Some("https://example.com/icon.png")
        );
        // 画像URLなしの旧セッションはNoneとして読める。
        let legacy: SessionData = serde_json::from_str(
            r#"{"auth_cookie":"c","user_id":"u","username":"n","created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(legacy.profile_icon_url, None);
    }

    #[tokio::test]
    async fn test_check_existing_session_not_found() {
        let (client, _temp) = create_test_client();

        let result = client.check_existing_session().await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_propagate_without_cookie_is_auth_error() {
        // Cookie不在の伝搬は旧値復活ではなく明示エラーにする(実構成の競合防衛)。
        let (client, _temp) = create_test_client();
        let http = Arc::new(tokio::sync::Mutex::new(
            HttpClient::new().expect("HTTPクライアントの作成に失敗"),
        ));
        let result = client.propagate_cookie_to(&http).await;
        assert!(matches!(result, Err(AppError::Auth(_))));
    }

    #[tokio::test]
    async fn test_restored_session_comes_from_memory() {
        let (client, _temp) = create_test_client();

        // 初回は保存が無いのでNone。以降は保存を見に行かずメモリだけを見る。
        assert!(client.check_existing_session().await.unwrap().is_none());
        client.set_auth_cookie("memory-cookie");
        *client.cached_user.lock().await = Some(CurrentUser {
            id: "usr_mem".to_string(),
            username: "mem".to_string(),
            display_name: "Mem".to_string(),
            two_factor_auth_enabled: false,
            profile_icon_url: None,
            presence: None,
        });
        let user = client.check_existing_session().await.unwrap().unwrap();
        assert_eq!(user.id, "usr_mem");
    }

    #[tokio::test]
    async fn test_logout_clears_session() {
        let (client, _temp) = create_test_client();
        client.set_auth_cookie("test_session_123");
        *client.cached_user.lock().await = Some(CurrentUser {
            id: "usr_test".to_string(),
            username: "test".to_string(),
            display_name: "Test".to_string(),
            two_factor_auth_enabled: false,
            profile_icon_url: None,
            presence: None,
        });
        assert_eq!(
            client.current_auth_cookie(),
            Some("test_session_123".to_string())
        );
        assert!(client.cached_user().await.is_some());
        let result = client.logout().await;
        assert!(result.is_ok());
        assert_eq!(client.current_auth_cookie(), None);
        assert!(client.cached_user().await.is_none());
        assert!(!client.session_store.exists());
    }

    // test_real_login_flow / test_real_totp_verification は常時ignore・資格情報・
    // 外部API依存のため通常テスト群から外す（手動診断手順へ移す前提で削除）。
}
