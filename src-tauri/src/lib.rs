pub mod commands;
pub mod domain;
pub mod infra;
pub mod logging;
pub mod services;
pub mod storage;

use std::sync::Arc;
use tauri::Manager;
use tokio::sync::Mutex;

use infra::http_client::HttpClient;
use logging::LogWriter;
use services::{JoinService, MonitorService, SdkAuthClient, VrchatApiService, WebsocketService};
use storage::{ConfigStore, PortablePaths, SessionStore, StorageError};

pub struct AppState {
    pub storage_paths: PortablePaths,
    pub storage_writable: bool,
    pub session_store: Arc<SessionStore>,
    pub config_store: Arc<ConfigStore>,
    pub http_client: Arc<Mutex<HttpClient>>,
    pub sdk_auth_client: Arc<SdkAuthClient>,
    pub vrchat_api: Arc<VrchatApiService>,
    pub join_service: Arc<JoinService>,
    pub monitor_service: Arc<MonitorService>,
    pub websocket_service: Arc<WebsocketService>,
    pub log_writer: Arc<Mutex<LogWriter>>,
    /// 認証ライフサイクルの単一境界。login delayed connect→logout→late connected復活を遮断する。
    /// 取得順は auth_lifecycle→sdk.auth_op→http のみ（逆順禁止）。
    /// 開始・手動副作用の入口で保持し、認証準備→WS接続→副作用完了まで離さない。
    /// stop_monitoringはlogout保持中に呼ばれるため取得しない。
    pub auth_lifecycle: Mutex<()>,
}

impl AppState {
    pub fn new() -> Result<Self, StorageError> {
        let storage_paths = PortablePaths::new()?;
        let storage_writable = storage_paths.verify_writable().is_ok();

        let session_store = Arc::new(SessionStore::new(storage_paths.clone()));
        let config_store = Arc::new(ConfigStore::new(storage_paths.clone()));
        let http_client = Arc::new(Mutex::new(HttpClient::new().map_err(|e| {
            StorageError::Io(std::io::Error::other(format!(
                "Failed to create HTTP client: {}",
                e
            )))
        })?));

        let sdk_auth_client = Arc::new(SdkAuthClient::new(session_store.clone()).map_err(|e| {
            StorageError::Io(std::io::Error::other(format!(
                "Failed to create SDK auth client: {}",
                e
            )))
        })?);

        let vrchat_api = Arc::new(VrchatApiService::new(http_client.clone()));
        // 書込不可配置でも起動を止めない。診断はpreflightのlogFileWritableで通知する。
        storage_paths.prune_old_logs();
        let log_writer = Arc::new(Mutex::new(LogWriter::new(storage_paths.log_file())));

        let join_service = Arc::new(JoinService::new());
        let websocket_service =
            Arc::new(WebsocketService::with_log_writer(Some(log_writer.clone())));

        let monitor_service = Arc::new(MonitorService::new(
            vrchat_api.clone(),
            join_service.clone(),
            log_writer.clone(),
            websocket_service.clone(),
        ));

        Ok(Self {
            storage_paths,
            storage_writable,
            session_store,
            config_store,
            http_client,
            sdk_auth_client,
            vrchat_api,
            join_service,
            monitor_service,
            websocket_service,
            log_writer,
            auth_lifecycle: Mutex::new(()),
        })
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    match tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            match AppState::new() {
                Ok(state) => {
                    app.manage(state);
                }
                Err(e) => {
                    eprintln!("Failed to initialize app state: {}", e);
                    return Err(Box::new(std::io::Error::other(format!(
                        "アプリの初期化に失敗しました。書き込み可能なフォルダに配置し直してから起動してください（ZIPはProgram Filesなどへ置かず、ダウンロード直下などで解凍してください）。詳細: {}",
                        e
                    ))));
                }
            };
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::auth::check_session,
            commands::auth::login,
            commands::auth::verify_two_factor,
            commands::auth::logout,
            commands::presets::get_presets,
            commands::presets::save_preset,
            commands::presets::delete_preset,
            commands::presets::get_config,
            commands::presets::save_config,
            commands::presets::lookup_group_summary,
            commands::presets::get_group_calendar_events,
            commands::system::check_preflight,
            commands::system::get_portable_paths_info,
            commands::monitor::get_monitor_status,
            commands::monitor::start_monitoring,
            commands::monitor::stop_monitoring,
            commands::monitor::get_api_budget_estimate,
            commands::monitor::test_preset_api,
            commands::monitor::test_group_info,
            commands::monitor::test_instance_check,
            commands::monitor::run_preset_api_action,
            commands::monitor::select_monitor_candidate,
            commands::monitor::export_monitor_diagnostics,
            commands::logs::get_recent_logs,
        ])
        .run(tauri::generate_context!())
    {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Tauriアプリケーションの実行中にエラーが発生しました: {}", e);
            std::process::exit(1);
        }
    }
}
