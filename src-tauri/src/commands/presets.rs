use crate::domain::models::AppError;
use crate::services::vrchat_api::{CalendarEvent, GroupSummary};
use crate::storage::{AppConfig, Preset, SessionError};
use crate::AppState;
use chrono::Utc;
use serde::Serialize;
use tauri::State;

/// メモリ認証を正本とする。保存Cookieの復元も/auth検証もここでは行わず、
/// Cookie伝搬だけ行う。未ログインはエラーを返し、UIに再ログインを促す。
async fn prepare_authenticated_api(state: &AppState) -> Result<(), AppError> {
    if state.sdk_auth_client.current_auth_cookie().is_none() {
        return Err(AppError::Auth("ログインが必要です".to_string()));
    }

    super::auth::propagate_auth_cookie(state).await
}

fn storage_error(e: SessionError) -> AppError {
    match e {
        SessionError::InvalidInput(message) => AppError::InvalidInput(message),
        other => AppError::Storage(format!("イベント設定を保存できませんでした: {}", other)),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventSummary {
    pub id: String,
    pub title: String,
    pub starts_at: chrono::DateTime<Utc>,
    pub ends_at: chrono::DateTime<Utc>,
    pub description: Option<String>,
}

#[tauri::command]
pub async fn get_presets(state: State<'_, AppState>) -> Result<Vec<Preset>, AppError> {
    state.config_store.load_presets().map_err(storage_error)
}

#[tauri::command]
pub async fn save_preset(state: State<'_, AppState>, preset: Preset) -> Result<(), AppError> {
    let mut owned = preset;
    owned.normalize();
    owned.validate().map_err(storage_error)?;
    let preset_id = owned.id.clone();
    state
        .config_store
        .update_presets(|presets| {
            if let Some(existing) = presets.iter_mut().find(|p| p.id == preset_id) {
                *existing = owned.clone();
            } else {
                presets.push(owned.clone());
            }
            Ok(())
        })
        .map_err(storage_error)
}

#[tauri::command]
pub async fn delete_preset(state: State<'_, AppState>, preset_id: String) -> Result<(), AppError> {
    state
        .config_store
        .update_presets(|presets| {
            presets.retain(|p| p.id != preset_id);
            Ok(())
        })
        .map_err(storage_error)
}

#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<AppConfig, AppError> {
    state.config_store.load_config().map_err(storage_error)
}

#[tauri::command]
pub async fn lookup_group_summary(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<GroupSummary, AppError> {
    prepare_authenticated_api(&state).await?;

    state.vrchat_api.get_group_summary(&group_id).await
}

/// 設定の部分更新パッチ。存在するキーだけ読み替えて書き戻し、他は保持する。
/// `selectedPresetId` は3値(欠落=維持/null=解除/文字列=設定)を区別する。
/// `presets` キーは受け付けない(プリセット系コマンドが正本)。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigPatch {
    #[serde(default)]
    pub selected_preset_id: Option<Option<String>>,
    #[serde(default)]
    pub debug_mode: Option<bool>,
    #[serde(default)]
    pub theme: Option<String>,
}

#[tauri::command]
pub async fn save_config(
    state: State<'_, AppState>,
    patch: AppConfigPatch,
) -> Result<(), AppError> {
    if patch.selected_preset_id.is_none() && patch.debug_mode.is_none() && patch.theme.is_none() {
        return Ok(());
    }
    state
        .config_store
        .update_config(|current| {
            if let Some(selected) = patch.selected_preset_id.clone() {
                current.selected_preset_id = selected;
            }
            if let Some(debug) = patch.debug_mode {
                current.debug_mode = debug;
            }
            if let Some(theme) = patch.theme.clone() {
                current.theme = theme;
            }
            Ok(())
        })
        .map_err(storage_error)
}

#[tauri::command]
pub async fn get_group_calendar_events(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Vec<CalendarEventSummary>, AppError> {
    prepare_authenticated_api(&state).await?;

    let now = Utc::now();
    let mut events: Vec<CalendarEvent> = state
        .vrchat_api
        .get_group_calendar(&group_id, Some(now))
        .await?
        .into_iter()
        .filter(|event| event.starts_at >= now)
        .collect();

    events.sort_by_key(|a| a.starts_at);

    Ok(events
        .into_iter()
        .take(5)
        .map(|event| CalendarEventSummary {
            id: event.id,
            title: event.title,
            starts_at: event.starts_at,
            ends_at: event.ends_at,
            description: event.description,
        })
        .collect())
}
