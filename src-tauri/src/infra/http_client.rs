use crate::domain::models::AppError;
/// User-Agentの正本。版はCargo版へ追随し、連絡先はCargo.tomlの`repository`を使う
/// (VRChat APIはアプリ名・版・連絡先を含むUser-Agentを求める)。
pub const APP_USER_AGENT: &str = concat!(
    "OnCue/",
    env!("CARGO_PKG_VERSION"),
    " (Windows; contact: ",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);
use reqwest::{cookie::Jar, header::HeaderMap, Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Invalid JSON response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Rate limit exceeded")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Access forbidden: {0}")]
    Forbidden(String),
    #[error("Not found")]
    NotFound,
    #[error("Server error: {0}")]
    ServerError(u16),
}

impl HttpError {
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            HttpError::RateLimited { retry_after_secs } => *retry_after_secs,
            _ => None,
        }
    }
}

impl From<HttpError> for AppError {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::RateLimited { retry_after_secs } => AppError::rate_limited(retry_after_secs),
            // VRChatの応答本文は英語の生データのため画面へ出さない。
            HttpError::Unauthorized(_) => AppError::Auth(
                "VRChatのログイン状態が無効になりました。ログインし直してください。".to_string(),
            ),
            HttpError::Forbidden(_) => AppError::Forbidden(FORBIDDEN_MESSAGE.to_string()),
            HttpError::NotFound => AppError::NotFound(
                "VRChat上で対象が見つかりませんでした。グループIDなどを確認してください。"
                    .to_string(),
            ),
            HttpError::ServerError(code) if code >= 500 => AppError::Api(format!(
                "VRChatサーバーでエラーが発生しました（HTTP {}）。しばらく待ってから再試行してください。",
                code
            )),
            HttpError::ServerError(code) => AppError::Api(format!(
                "VRChat APIへの要求が受け付けられませんでした（HTTP {}）。",
                code
            )),
            HttpError::Request(err) if err.is_timeout() => AppError::Network(
                "接続がタイムアウトしました。インターネット接続を確認してください。".to_string(),
            ),
            HttpError::Request(_) => AppError::Network(
                "VRChatに接続できませんでした。インターネット接続を確認してください。".to_string(),
            ),
            HttpError::Json(_) => AppError::Api(
                "VRChatからの応答を読み取れませんでした。VRChat側の仕様変更の可能性があります。"
                    .to_string(),
            ),
        }
    }
}

/// 403の表示文。グループ関連の取得で起きることが大半のため、確認先を添える。
pub const FORBIDDEN_MESSAGE: &str =
    "VRChatにアクセスを拒否されました。対象グループのメンバーか、閲覧権限があるかを確認してください。";

#[derive(Debug, serde::Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorDetail>,
}

#[derive(Debug, serde::Deserialize)]
struct ErrorDetail {
    message: Option<String>,
}

fn extract_error_message(body: &str) -> Option<String> {
    serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .and_then(|envelope| envelope.error)
        .and_then(|error| error.message)
}

/// Retry-Afterヘッダを秒数へ変換する。秒数またはHTTP-dateを受理し、
/// 解析不能・過去時刻はNone。明示値は切り詰めない(尊重はscheduler側の窓判定)。
fn parse_retry_after(headers: &HeaderMap) -> Option<u64> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(secs);
    }
    if let Ok(date) = chrono::DateTime::parse_from_rfc2822(value) {
        let delta = date.timestamp() - chrono::Utc::now().timestamp();
        if delta > 0 {
            return Some(delta as u64);
        }
    }
    None
}

fn map_status(status: u16, headers: &HeaderMap, body: Option<&str>) -> Result<(), HttpError> {
    match status {
        429 => Err(HttpError::RateLimited {
            retry_after_secs: parse_retry_after(headers),
        }),
        401 => Err(HttpError::Unauthorized(
            body.and_then(extract_error_message)
                .unwrap_or_else(|| "Unauthorized".to_string()),
        )),
        403 => Err(HttpError::Forbidden(
            body.and_then(extract_error_message)
                .unwrap_or_else(|| "Forbidden".to_string()),
        )),
        404 => Err(HttpError::NotFound),
        code if code >= 400 => Err(HttpError::ServerError(code)),
        _ => Ok(()),
    }
}

/// VRChat API用HTTPクライアント。接続再利用のため要求ごとに作り直さない。
/// reqwestのClientは内部でコネクションプールを持ちCloneは安価だが、
/// ここではAppStateが単一インスタンスを保持し、各メソッドが&selfで並行利用する。
pub struct HttpClient {
    client: Client,
    base_url: String,
    cookie_jar: Arc<Jar>,
}

impl HttpClient {
    pub fn new() -> Result<Self, HttpError> {
        let cookie_jar = Arc::new(Jar::default());

        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(30))
            .cookie_provider(cookie_jar.clone())
            .user_agent(APP_USER_AGENT)
            .build()?;

        Ok(Self {
            client,
            base_url: "https://api.vrchat.cloud/api/1".to_string(),
            cookie_jar,
        })
    }

    /// メモリ認証を正本とし、保存Cookieの再投入は行わない。
    /// 呼び出し側(SDK認証クライアント由来のCookie伝搬)が直列化する。
    pub fn with_auth_cookie(&self, cookie_value: &str) {
        let url = self.base_url.parse().unwrap();
        let cookie = format!("auth={}; Domain=api.vrchat.cloud; Path=/", cookie_value);
        self.cookie_jar.add_cookie_str(&cookie, &url);
    }

    pub fn clear_auth(&self) {
        let url = self.base_url.parse().unwrap();
        let cookie =
            "auth=; Domain=api.vrchat.cloud; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT";
        self.cookie_jar.add_cookie_str(cookie, &url);
    }

    pub async fn get<T: for<'de> Deserialize<'de>>(&self, endpoint: &str) -> Result<T, HttpError> {
        let url = format!("{}{}", self.base_url, endpoint);

        let response = self.client.get(&url).send().await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.bytes().await?;

        if status >= 400 {
            let error_body = String::from_utf8_lossy(&body);
            map_status(status, &headers, Some(&error_body))?;
        }

        let data = serde_json::from_slice::<T>(&body)?;
        Ok(data)
    }

    pub async fn post<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        endpoint: &str,
        body: &B,
    ) -> Result<T, HttpError> {
        let url = format!("{}{}", self.base_url, endpoint);

        let response = self.client.post(&url).json(body).send().await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.bytes().await?;

        if status >= 400 {
            let error_body = String::from_utf8_lossy(&body);
            map_status(status, &headers, Some(&error_body))?;
        }

        let data = serde_json::from_slice::<T>(&body)?;
        Ok(data)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentUser {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub two_factor_auth_enabled: bool,
    /// VRChatのユーザー画像URL。取得不可のときはNone。
    #[serde(default)]
    pub profile_icon_url: Option<String>,
    pub presence: Option<Presence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    pub world: String,
    pub instance: String,
    pub traveling_to_world: Option<String>,
    pub traveling_to_instance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderValue, RETRY_AFTER};

    fn headers_with_retry_after(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn retry_after_parser_table() {
        // 秒数 / 不正 / 欠落 / HTTP-date未来 / 過去 の parser 分類を1本で確認する。
        let cases: Vec<(&str, Option<u64>)> = vec![
            ("20", Some(20)),
            ("3600", Some(3600)),
            ("soon", None),
            ("", None),
        ];
        for (value, expected) in cases {
            let headers = if value.is_empty() {
                HeaderMap::new()
            } else {
                headers_with_retry_after(value)
            };
            assert_eq!(parse_retry_after(&headers), expected, "value={:?}", value);
        }
        assert_eq!(parse_retry_after(&HeaderMap::new()), None);
        let future = (chrono::Utc::now() + chrono::Duration::seconds(45))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let parsed = parse_retry_after(&headers_with_retry_after(&future)).unwrap();
        assert!((40..=50).contains(&parsed), "got {}", parsed);
        let past = (chrono::Utc::now() - chrono::Duration::seconds(60))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        assert_eq!(
            parse_retry_after(&headers_with_retry_after(&past)),
            None,
            "past HTTP-date"
        );
    }

    #[test]
    fn rate_limit_retry_after_propagates_unmodified() {
        // Retry-After が HttpError → AppError まで改変されないことを1本で確認する。
        // 明示値は切り詰めない。尊重と窓外停止はscheduler側の判定。
        let headers = headers_with_retry_after("3600");
        match map_status(429, &headers, None) {
            Err(HttpError::RateLimited { retry_after_secs }) => {
                assert_eq!(retry_after_secs, Some(3600))
            }
            other => panic!("unexpected: {:?}", other),
        }
        let app: AppError = HttpError::RateLimited {
            retry_after_secs: Some(3600),
        }
        .into();
        assert_eq!(app.retry_after_secs(), Some(3600));
        // ヘッダなし429はNoneのまま伝搬する。
        match map_status(429, &HeaderMap::new(), None) {
            Err(HttpError::RateLimited { retry_after_secs }) => {
                assert_eq!(retry_after_secs, None)
            }
            other => panic!("unexpected: {:?}", other),
        }
        let app: AppError = HttpError::RateLimited {
            retry_after_secs: None,
        }
        .into();
        assert_eq!(app.retry_after_secs(), None);
        // 遠い未来のHTTP-dateも端まで通す（実API不要の防衛線）。
        let far_future = (chrono::Utc::now() + chrono::Duration::seconds(7200))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let parsed = parse_retry_after(&headers_with_retry_after(&far_future)).unwrap();
        assert!((7100..=7300).contains(&parsed), "got {}", parsed);
    }

    #[test]
    fn map_status_classifies_expected_errors() {
        let headers = HeaderMap::new();
        assert!(matches!(
            map_status(401, &headers, None),
            Err(HttpError::Unauthorized(_))
        ));
        assert!(matches!(
            map_status(403, &headers, None),
            Err(HttpError::Forbidden(_))
        ));
        assert!(matches!(
            map_status(404, &headers, None),
            Err(HttpError::NotFound)
        ));
        assert!(matches!(
            map_status(503, &headers, None),
            Err(HttpError::ServerError(503))
        ));
        assert!(map_status(200, &headers, None).is_ok());
    }

    #[test]
    fn forbidden_maps_to_forbidden_not_auth() {
        let app: AppError = HttpError::Forbidden("private group".to_string()).into();
        assert!(matches!(app, AppError::Forbidden(_)));
        assert!(!matches!(app, AppError::Auth(_)));
    }

    // map_status_429_carries_retry_after / rate_limited_maps_with_retry_after /
    // explicit_retry_after_is_never_truncated は上記2本へ集約したため削除。

    #[test]
    fn deserialize_current_user_minimal_shape() {
        let json = serde_json::json!({
            "id": "usr_123",
            "username": "test-user",
            "displayName": "Test User",
            "twoFactorAuthEnabled": true,
            "presence": {
                "world": "wrld_123",
                "instance": "12345",
                "travelingToWorld": null,
                "travelingToInstance": null
            }
        });

        let parsed: CurrentUser = serde_json::from_value(json).unwrap();

        assert_eq!(parsed.display_name, "Test User");
        assert!(parsed.two_factor_auth_enabled);
    }
}
