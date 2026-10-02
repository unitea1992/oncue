use crate::domain::models::AppError;
use crate::AppState;
use tauri::State;

#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub source: String,
}

#[tauri::command]
pub async fn get_recent_logs(
    state: State<'_, AppState>,
    count: Option<usize>,
) -> Result<Vec<LogEntry>, AppError> {
    let count = count.unwrap_or(50).min(200);
    let log_file = state.storage_paths.log_file();

    if !log_file.exists() {
        return Ok(vec![]);
    }

    let content = tokio::fs::read_to_string(&log_file)
        .await
        .map_err(|e| AppError::Storage(format!("ログファイルを読み込めませんでした: {}", e)))?;

    let mut logs: Vec<LogEntry> = content
        .lines()
        .filter_map(|line| {
            if line.trim().is_empty() {
                return None;
            }

            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| {
                    let timestamp = v.get("timestamp")?.as_str()?.to_string();
                    let level = v.get("level")?.as_str()?.to_string().to_uppercase();
                    let category = v.get("category")?.as_str()?.to_string();
                    let message = v.get("message")?.as_str()?.to_string();

                    Some(LogEntry {
                        timestamp,
                        level,
                        message,
                        source: category,
                    })
                })
        })
        .collect();

    // Get the most recent logs
    let start = logs.len().saturating_sub(count);
    logs = logs[start..].to_vec();

    Ok(logs)
}
