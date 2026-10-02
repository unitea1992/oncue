use crate::domain::models::AppError;
use crate::AppState;
use tauri::State;
use tracing::{error, info, warn};

async fn connect_websocket_from_saved_session(state: &AppState) {
    if let Some(auth_cookie) = state.sdk_auth_client.current_auth_cookie() {
        let _ = state.websocket_service.connect(&auth_cookie).await;
    } else {
        warn!("[command:websocket] WebSocket接続用の認証Cookieがありません");
    }
}

/// メモリ認証を正本としてHttpClientへCookie伝搬する。保存の再投入はしない。
/// 直列化と最新読取はSDK側へ委譲し、logout完了後の旧値復活を防ぐ。
pub(crate) async fn propagate_auth_cookie(state: &AppState) -> Result<(), AppError> {
    state
        .sdk_auth_client
        .propagate_cookie_to(&state.http_client)
        .await
}

#[tauri::command]
pub async fn check_session(
    state: State<'_, AppState>,
) -> Result<Option<crate::infra::http_client::CurrentUser>, AppError> {
    // 共通command境界。SDK操作→HTTP/WS伝搬完了まで保持する。
    let _lifecycle = state.auth_lifecycle.lock().await;
    let result = state.sdk_auth_client.check_existing_session().await;
    match &result {
        Ok(Some(user)) => {
            info!(
                "[command:check_session] Session valid for: {}",
                user.display_name
            );
            // 復元セッションを現行HTTP/WSへ一度だけ伝搬する。以降の呼び出しは冪等。
            if let Err(e) = propagate_auth_cookie(&state).await {
                warn!("[command:check_session] Cookie伝搬に失敗: {}", e);
            }
            connect_websocket_from_saved_session(&state).await;
        }
        Ok(None) => info!("[command:check_session] No valid session"),
        Err(e) => error!("[command:check_session] Session check failed: {}", e),
    }
    result
}

#[tauri::command]
pub async fn login(
    state: State<'_, AppState>,
    username: String,
    password: String,
    remember_session: bool,
) -> Result<serde_json::Value, AppError> {
    // 共通command境界。開始時のWS切断と旧正本破棄から伝搬完了まで保持する。
    let _lifecycle = state.auth_lifecycle.lock().await;
    // 別userへの取り違え防止: 旧WSを切り、旧Cookieとメモリ正本を捨てる(保存は残す)。
    state.websocket_service.disconnect().await;
    state.sdk_auth_client.reset_for_new_login().await;
    match state
        .sdk_auth_client
        .login(&username, &password, remember_session)
        .await?
    {
        crate::services::SdkLoginResult::Success { user } => {
            info!(
                "[command:login] Login successful for: {}",
                user.display_name
            );
            propagate_auth_cookie(&state).await?;
            connect_websocket_from_saved_session(&state).await;
            Ok(serde_json::json!({
                "success": true,
                "requiresTwoFactor": [],
                "user": {
                    "id": user.id,
                    "username": user.username,
                    "displayName": user.display_name,
                    "twoFactorAuthEnabled": user.two_factor_auth_enabled,
                    "profileIconUrl": user.profile_icon_url
                }
            }))
        }
        crate::services::SdkLoginResult::RequiresTwoFactor { username, methods } => {
            info!("[command:login] 2FA required for: {}", username);
            Ok(serde_json::json!({
                "success": true,
                "requiresTwoFactor": methods,
                "username": username
            }))
        }
    }
}

fn user_json(user: &crate::infra::http_client::CurrentUser) -> serde_json::Value {
    serde_json::json!({
        "success": true,
        "user": {
            "id": user.id,
            "username": user.username,
            "displayName": user.display_name,
            "twoFactorAuthEnabled": user.two_factor_auth_enabled,
            "profileIconUrl": user.profile_icon_url
        }
    })
}

#[tauri::command]
pub async fn verify_two_factor(
    state: State<'_, AppState>,
    code: String,
    method: String,
    remember_session: bool,
) -> Result<serde_json::Value, AppError> {
    // 共通command境界。SDK操作→HTTP/WS伝搬完了まで保持する。
    let _lifecycle = state.auth_lifecycle.lock().await;
    let user = state
        .sdk_auth_client
        .verify_two_factor(&code, &method, remember_session)
        .await?;

    propagate_auth_cookie(&state).await?;
    connect_websocket_from_saved_session(&state).await;

    Ok(user_json(&user))
}

#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> Result<(), AppError> {
    // 共通command境界。監視停止→WS切断→SDK→HTTP clear全体を保持する。
    // stop_monitoring側は取得しない合意のためデッドロックしない。
    let _lifecycle = state.auth_lifecycle.lock().await;
    // 順序: 監視停止→WS切断→SDKログアウト→HTTP認証クリア
    state.monitor_service.stop_monitoring().await;
    state.websocket_service.disconnect().await;

    let result = state.sdk_auth_client.logout().await;
    // SDK側の成否にかかわらずHTTP側を直列化クリアし、旧値残留を断つ。
    state
        .sdk_auth_client
        .clear_http_cookie(&state.http_client)
        .await;
    result
}
