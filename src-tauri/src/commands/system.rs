use crate::domain::models::AppError;
use crate::domain::models::WebsocketConnectionState;
use crate::services::JoinService;
use crate::AppState;
use tauri::State;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightResult {
    pub portable_writable: bool,
    pub protocol_handler_available: bool,
    pub vrchat_process_detected: bool,
    pub saved_session_valid: bool,
    pub websocket_connected: bool,
    pub websocket_connection_state: WebsocketConnectionState,
    pub websocket_last_error: Option<String>,
    pub log_file_writable: bool,
    pub all_clear: bool,
    pub blockers: Vec<String>,
}

#[tauri::command]
pub async fn check_preflight(state: State<'_, AppState>) -> Result<PreflightResult, AppError> {
    let portable_writable = state.storage_writable;
    let protocol_handler_available = JoinService::check_protocol_handler();
    let vrchat_process_detected = JoinService::check_vrchat_process();

    let saved_session_valid = state.session_store.exists() && state.session_store.load().is_ok();

    let websocket_connected = state.websocket_service.is_connected().await;
    let websocket_connection_state = state.websocket_service.get_connection_state().await;
    let websocket_last_error = state.websocket_service.get_last_error().await;
    let log_file_writable = state
        .log_writer
        .try_lock()
        .map(|writer| writer.file_available())
        .unwrap_or(false);

    let mut blockers = Vec::new();

    if !portable_writable {
        blockers.push(
            "アプリフォルダに書き込みできません。書き込み可能な場所へ移動してください。"
                .to_string(),
        );
    }
    if !protocol_handler_available {
        blockers.push(
            "VRChatの起動プロトコルが見つかりません。VRChatのインストール状態を確認してください。"
                .to_string(),
        );
    }
    if !vrchat_process_detected {
        blockers.push("VRChatが起動していません。先にVRChatを起動してください。".to_string());
    }

    let all_clear = blockers.is_empty();

    Ok(PreflightResult {
        portable_writable,
        protocol_handler_available,
        vrchat_process_detected,
        saved_session_valid,
        websocket_connected,
        websocket_connection_state,
        websocket_last_error,
        log_file_writable,
        all_clear,
        blockers,
    })
}

#[tauri::command]
pub async fn get_portable_paths_info(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, AppError> {
    let data_dir = state.storage_paths.data_dir();
    let cache_dir = state.storage_paths.cache_dir();
    let logs_dir = state.storage_paths.logs_dir();

    Ok(serde_json::json!({
        "dataDir": data_dir.to_string_lossy(),
        "cacheDir": cache_dir.to_string_lossy(),
        "logsDir": logs_dir.to_string_lossy(),
        "sessionFile": state.storage_paths.session_file().to_string_lossy(),
        "presetsFile": state.storage_paths.presets_file().to_string_lossy(),
    }))
}
