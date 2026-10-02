use crate::domain::models::{
    select_candidate, target_calendar_entry_id_for_schedule, AppError, Candidate,
    CandidateSelection, MonitorSnapshot,
};
use crate::infra::http_client::CurrentUser;
use crate::services::vrchat_api::candidates_from_detailed;
use crate::services::{resolve_monitor_window, CONFIRM_TIMEOUT_SECS};
use crate::storage::Preset;
use crate::AppState;
use chrono::Utc;
use serde_json::json;
use tauri::State;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetApiTestResult {
    pub group_name: String,
    pub instance_count: usize,
    pub estimated_requests: u32,
    pub should_warn: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfoTestResult {
    pub group_name: String,
    pub short_code: Option<String>,
    pub member_count: Option<i32>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceCheckTestResult {
    pub group_name: String,
    pub instance_count: usize,
    pub estimated_requests: u32,
    pub should_warn: bool,
    pub target_location: Option<String>,
    pub target_display_name: Option<String>,
    pub can_dispatch: bool,
    pub selection_status: String,
    pub message: String,
    /// 現在候補一覧。複数時はUI選択用（非終端）。
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiActionTestResult {
    pub action: String,
    pub target_location: String,
    pub target_display_name: Option<String>,
    pub message: String,
}

fn load_preset(state: &AppState, preset_id: &str) -> Result<Preset, AppError> {
    let presets = state
        .config_store
        .load_presets()
        .map_err(|e| AppError::Storage(format!("イベント設定を読み込めませんでした: {}", e)))?;

    presets
        .into_iter()
        .find(|p| p.id == preset_id)
        .ok_or_else(|| {
            AppError::NotFound(
                "選択したイベントが見つかりません。イベント一覧を確認してください。".to_string(),
            )
        })
}

/// メモリ認証を正本とする。保存セッションの復元は起動時のみで、
/// 通常コマンドで保存Cookieを再投入しない。
/// Cookie伝搬はSDKの認証更新境界へ一任し、旧認証の再投入を防ぐ。
async fn prepare_authenticated_client(state: &AppState) -> Result<CurrentUser, AppError> {
    let current_user = state
        .sdk_auth_client
        .get_current_user_from_memory()
        .await?
        .ok_or_else(|| {
            AppError::Auth("ログインが必要です。ログインし直してください。".to_string())
        })?;

    state
        .sdk_auth_client
        .propagate_cookie_to(&state.http_client)
        .await?;

    Ok(current_user)
}

/// 監視と共通の選択規則。calendarEntryIdは自動選択に使わず、曖昧な対象は選ばない。
fn select_action_candidate(preset: &Preset, candidates: &[Candidate]) -> CandidateSelection {
    let target =
        target_calendar_entry_id_for_schedule(&preset.schedule, preset.source_event_id.as_deref());
    select_candidate(
        target.as_deref(),
        preset.preferred_instance_name.as_deref(),
        candidates,
    )
}

#[tauri::command]
pub async fn get_monitor_status(state: State<'_, AppState>) -> Result<MonitorSnapshot, AppError> {
    Ok(state.monitor_service.get_snapshot().await)
}

#[tauri::command]
pub async fn start_monitoring(
    state: State<'_, AppState>,
    preset_id: String,
) -> Result<(), AppError> {
    let presets = state
        .config_store
        .load_presets()
        .map_err(|e| AppError::Storage(format!("イベント設定を読み込めませんでした: {}", e)))?;

    let preset = presets.iter().find(|p| p.id == preset_id).ok_or_else(|| {
        AppError::NotFound(
            "選択したイベントが見つかりません。イベント一覧を確認してください。".to_string(),
        )
    })?;

    // R2境界: 認証準備→WS接続→監視開始まで保持し、logoutとの競合を遮断する。
    // stop_monitoring側では取得しない（logout保持中の再取得はデッドロック）。
    let _auth_guard = state.auth_lifecycle.lock().await;
    let current_user = prepare_authenticated_client(&state).await?;

    // 開始前に現行CookieでWS接続を復活させる（最低2回試行）。run中の無限reconnectはしない。
    // 未接続のまま開始した場合、確認は送信済み・確認不能で終える。
    if !state.websocket_service.is_connected().await {
        let mut connected = false;
        if let Some(cookie) = state.sdk_auth_client.current_auth_cookie() {
            for attempt in 0..2 {
                if state.websocket_service.connect(&cookie).await.is_ok() {
                    connected = true;
                    break;
                }
                if attempt == 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
        if !connected {
            if let Ok(mut writer) = state.log_writer.try_lock() {
                let _ = writer.log(crate::logging::LogEvent::warn(
                    "monitor",
                    "WebSocketに接続できないまま監視を開始します。入室確認は送信済み・確認不能になります。",
                ));
            }
        }
    }

    // Load config to get debug_mode setting
    let app_config = state
        .config_store
        .load_config()
        .map_err(|e| AppError::Storage(format!("イベント設定を読み込めませんでした: {}", e)))?;

    {
        let mut log_writer = state.log_writer.lock().await;
        log_writer.set_level(if app_config.debug_mode {
            crate::logging::LogLevel::Debug
        } else {
            crate::logging::LogLevel::Info
        });
    }

    state
        .monitor_service
        .start_monitoring(preset, &current_user.id, app_config.debug_mode)
        .await?;

    Ok(())
}

#[tauri::command]
pub async fn stop_monitoring(state: State<'_, AppState>) -> Result<(), AppError> {
    state.monitor_service.stop_monitoring().await;
    Ok(())
}

#[tauri::command]
pub async fn get_api_budget_estimate(
    state: State<'_, AppState>,
    preset_id: String,
) -> Result<serde_json::Value, AppError> {
    let presets = state
        .config_store
        .load_presets()
        .map_err(|e| AppError::Storage(format!("イベント設定を読み込めませんでした: {}", e)))?;

    let preset = presets.iter().find(|p| p.id == preset_id).ok_or_else(|| {
        AppError::NotFound(
            "選択したイベントが見つかりません。イベント一覧を確認してください。".to_string(),
        )
    })?;

    let window = resolve_monitor_window(&preset.schedule, Utc::now()).ok_or_else(|| {
        AppError::InvalidInput(
            "単発イベントの開始時刻を過ぎています。新しい日時で作り直してください。".to_string(),
        )
    })?;

    let estimate = state.vrchat_api.estimate_api_budget(
        window.event_start,
        window.monitor_start,
        window.monitor_end,
    );

    Ok(json!({
        "profileName": estimate.profile_name,
        "estimatedRequests": estimate.estimated_requests,
        "warnThreshold": estimate.warn_threshold,
        "blockThreshold": estimate.block_threshold,
        "shouldWarn": estimate.should_warn,
        "shouldBlock": estimate.should_block,
        "eventStart": window.event_start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "monitorStart": window.monitor_start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "monitorEnd": window.monitor_end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }))
}

#[tauri::command]
pub async fn test_preset_api(
    state: State<'_, AppState>,
    preset_id: String,
) -> Result<PresetApiTestResult, AppError> {
    let preset = load_preset(&state, &preset_id)?;
    let current_user = prepare_authenticated_client(&state).await?;

    let group = state.vrchat_api.get_group_summary(&preset.group_id).await?;
    // 件数も1GETで取る。旧group instancesは使わない。
    let instances = state
        .vrchat_api
        .get_group_instances_for_group(&current_user.id, &preset.group_id)
        .await?;
    let window = resolve_monitor_window(&preset.schedule, Utc::now()).ok_or_else(|| {
        AppError::InvalidInput(
            "単発イベントの開始時刻を過ぎています。新しい日時で作り直してください。".to_string(),
        )
    })?;
    let estimate = state.vrchat_api.estimate_api_budget(
        window.event_start,
        window.monitor_start,
        window.monitor_end,
    );

    Ok(PresetApiTestResult {
        group_name: group.name,
        instance_count: instances.instances.len(),
        estimated_requests: estimate.estimated_requests,
        should_warn: estimate.should_warn,
    })
}

#[tauri::command]
pub async fn test_group_info(
    state: State<'_, AppState>,
    preset_id: String,
) -> Result<GroupInfoTestResult, AppError> {
    let preset = load_preset(&state, &preset_id)?;
    prepare_authenticated_client(&state).await?;

    let group = state.vrchat_api.get_group_summary(&preset.group_id).await?;

    Ok(GroupInfoTestResult {
        group_name: group.name,
        short_code: group.short_code,
        member_count: group.member_count,
    })
}

#[tauri::command]
pub async fn test_instance_check(
    state: State<'_, AppState>,
    preset_id: String,
) -> Result<InstanceCheckTestResult, AppError> {
    let preset = load_preset(&state, &preset_id)?;
    let current_user = prepare_authenticated_client(&state).await?;

    let group = state.vrchat_api.get_group_summary(&preset.group_id).await?;
    // group-specific 1GET。旧2GET突合はしない。
    let response = state
        .vrchat_api
        .get_group_instances_for_group(&current_user.id, &preset.group_id)
        .await?;
    let candidates = candidates_from_detailed(&response.instances);
    let selection = select_action_candidate(&preset, &candidates);
    let target = selection.clone().target();
    let window = resolve_monitor_window(&preset.schedule, Utc::now()).ok_or_else(|| {
        AppError::InvalidInput(
            "単発イベントの開始時刻を過ぎています。新しい日時で作り直してください。".to_string(),
        )
    })?;
    let estimate = state.vrchat_api.estimate_api_budget(
        window.event_start,
        window.monitor_start,
        window.monitor_end,
    );

    Ok(InstanceCheckTestResult {
        group_name: group.name,
        instance_count: response.instances.len(),
        estimated_requests: estimate.estimated_requests,
        should_warn: estimate.should_warn,
        target_location: target.as_ref().map(|candidate| candidate.location.clone()),
        target_display_name: target
            .as_ref()
            .and_then(|candidate| candidate.display_name.clone()),
        can_dispatch: target.is_some(),
        selection_status: selection.status_key().to_string(),
        message: selection.message(),
        candidates,
    })
}

#[tauri::command]
pub async fn run_preset_api_action(
    state: State<'_, AppState>,
    preset_id: String,
    action: String,
) -> Result<ApiActionTestResult, AppError> {
    // R2境界: 認証準備→手動副作用まで保持し、logoutとの競合を遮断する。
    let _auth_guard = state.auth_lifecycle.lock().await;
    let preset = load_preset(&state, &preset_id)?;
    let current_user = prepare_authenticated_client(&state).await?;

    // 起動可否の判定は `join_service` の共通送出処理に集約（監視と同一）。プロトコル未登録は即エラー。
    if action == "selfInvite" && !crate::services::JoinService::check_vrchat_process() {
        return Err(AppError::Operation(
            "VRChatが起動していないため、セルフ招待の確認ができません。先にVRChatを起動してログイン状態にしてください。".to_string(),
        ));
    }

    if action == "selfInvite" && !crate::services::JoinService::check_vrchat_process() {
        return Err(AppError::Operation(
            "VRChatが起動していないため、セルフ招待の確認ができません。先にVRChatを起動してログイン状態にしてください。".to_string(),
        ));
    }

    // group-specific 1GET。旧2GET突合はしない。
    let response = state
        .vrchat_api
        .get_group_instances_for_group(&current_user.id, &preset.group_id)
        .await?;
    let candidates = candidates_from_detailed(&response.instances);
    // 監視と共通の選択規則。曖昧な対象は手動でも起動しない。
    let candidate = match select_action_candidate(&preset, &candidates) {
        CandidateSelection::Target(candidate) => candidate,
        selection => {
            return Err(AppError::Operation(selection.message()));
        }
    };

    match action.as_str() {
        "launch" => {
            state
                .join_service
                .dispatch_launch_checked(&candidate.location)
                .await?;

            Ok(ApiActionTestResult {
                action,
                target_location: candidate.location.clone(),
                target_display_name: candidate.display_name.clone(),
                message: format!(
                    "vrchat:// 起動リンクを送信しました。VRChat側で遷移を確認してください（確認期限の目安は{}秒です）。",
                    CONFIRM_TIMEOUT_SECS
                ),
            })
        }
        "selfInvite" => {
            state
                .vrchat_api
                .invite_myself(&candidate.world_id, &candidate.instance_id)
                .await?;

            Ok(ApiActionTestResult {
                action,
                target_location: candidate.location.clone(),
                target_display_name: candidate.display_name.clone(),
                message: "セルフ招待を送信しました。VRChat内の通知を確認してください。自動での招待送信は行いません（手動の救済操作です）。"
                    .to_string(),
            })
        }
        _ => Err(AppError::InvalidInput(
            "Unknown API test action".to_string(),
        )),
    }
}

/// 現在の実行に対する候補選択。スナップショットの `generation` と `location` を渡す。
/// 世代一致・候補現存・参加/キュー待ち可否を再検証し、実行内に固定する。
/// `preferred` への自動保存はしない。`lib.rs` への登録は統合時に行う。
#[tauri::command]
pub async fn select_monitor_candidate(
    state: State<'_, AppState>,
    generation: u64,
    location: String,
) -> Result<(), AppError> {
    state
        .monitor_service
        .select_pinned_location(generation, &location)
        .await
}

/// 秘密情報を含まない診断の書き出し。候補件数・容量・キュー・`calendarEntryId` のみ返す。
/// `userId` / `cookie` / `Authorization` / `nonce` は含めない。禁止リストの検出で失敗させる。
/// `lib.rs` への登録は統合時に行う。
#[tauri::command]
pub async fn export_monitor_diagnostics(
    state: State<'_, AppState>,
    preset_id: String,
) -> Result<serde_json::Value, AppError> {
    let preset = load_preset(&state, &preset_id)?;
    let current_user = prepare_authenticated_client(&state).await?;
    state
        .vrchat_api
        .probe_secret_free_diagnostic(&current_user.id, &preset.group_id)
        .await
}
