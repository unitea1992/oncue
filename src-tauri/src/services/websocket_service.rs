use crate::domain::models::{AppError, WebsocketConnectionState};
use crate::logging::{LogEvent, LogWriter};
use crate::services::APP_USER_AGENT;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::USER_AGENT;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tracing::{error, info, warn};

/// 位置・キュー通知の鮮度管理。成功判定は「現世代の接続で
/// 起動要求の送出以後に受信した本人の `location` が対象と一致」のみ。
/// 古い接続・古い受信の値は成功に使わない。
#[derive(Debug, Clone)]
pub struct LocationTracker {
    generation: u64,
    location: Option<LocationSample>,
    queue: Option<QueueSample>,
}

#[derive(Debug, Clone)]
struct LocationSample {
    location: String,
    received_at: DateTime<Utc>,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueKind {
    Joined,
    Ready,
}

#[derive(Debug, Clone)]
struct QueueSample {
    kind: QueueKind,
    received_at: DateTime<Utc>,
    generation: u64,
    /// 通知内容から拾えた対象ヒント（location系文字列）。
    location_hint: Option<String>,
}

impl LocationTracker {
    pub fn new() -> Self {
        Self {
            generation: 0,
            location: None,
            queue: None,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 接続・切断で世代を進め、旧世代の受信値を無効化する。
    pub fn next_generation(&mut self) {
        self.generation += 1;
        self.location = None;
        self.queue = None;
    }

    pub fn push_location(&mut self, location: String, received_at: DateTime<Utc>) {
        self.location = Some(LocationSample {
            location,
            received_at,
            generation: self.generation,
        });
    }

    pub fn push_queue(
        &mut self,
        kind: QueueKind,
        location_hint: Option<String>,
        received_at: DateTime<Utc>,
    ) {
        self.queue = Some(QueueSample {
            kind,
            received_at,
            generation: self.generation,
            location_hint,
        });
    }

    pub fn latest_location(&self) -> Option<String> {
        self.location.as_ref().map(|s| s.location.clone())
    }

    fn fresh_location_since(&self, since: DateTime<Utc>) -> Option<&str> {
        let sample = self.location.as_ref()?;
        if sample.generation == self.generation && sample.received_at >= since {
            Some(&sample.location)
        } else {
            None
        }
    }

    /// 成功条件：現世代・起動要求の送出以後・対象と完全一致。移動中の証拠は一致でも成功にしない。
    pub fn matches_target(&self, target: &str, since: DateTime<Utc>) -> bool {
        match self.fresh_location_since(since) {
            Some(location) => location == target && !location.to_lowercase().contains("traveling"),
            None => false,
        }
    }

    /// 移動中の証拠（成功ではない）。表示用。
    pub fn is_travelling_since(&self, since: DateTime<Utc>) -> bool {
        match self.fresh_location_since(since) {
            Some(location) => location.to_lowercase().contains("traveling"),
            None => false,
        }
    }

    /// 対象に一致するqueue通知のみ扱う。ヒント付きで不一致なら無視する。
    pub fn queue_for_target_since(&self, target: &str, since: DateTime<Utc>) -> Option<QueueKind> {
        let sample = self.queue.as_ref()?;
        if sample.generation != self.generation || sample.received_at < since {
            return None;
        }
        if let Some(hint) = &sample.location_hint {
            if hint != target && !target.contains(hint.as_str()) && !hint.contains(target) {
                return None;
            }
        }
        Some(sample.kind)
    }
}

impl Default for LocationTracker {
    fn default() -> Self {
        Self::new()
    }
}

pub struct WebsocketService {
    connection_state: Arc<RwLock<WebsocketConnectionState>>,
    connected: Arc<RwLock<bool>>,
    tracker: Arc<RwLock<LocationTracker>>,
    last_error: Arc<RwLock<Option<String>>>,
    log_writer: Option<Arc<Mutex<LogWriter>>>,
    task_handle: Arc<tokio::sync::Mutex<Option<JoinHandle<()>>>>,
}

impl Default for WebsocketService {
    fn default() -> Self {
        Self::with_log_writer(None)
    }
}

impl WebsocketService {
    pub fn new() -> Self {
        Self::with_log_writer(None)
    }

    pub fn with_log_writer(log_writer: Option<Arc<Mutex<LogWriter>>>) -> Self {
        Self {
            connection_state: Arc::new(RwLock::new(WebsocketConnectionState::Disconnected)),
            connected: Arc::new(RwLock::new(false)),
            tracker: Arc::new(RwLock::new(LocationTracker::new())),
            last_error: Arc::new(RwLock::new(None)),
            log_writer,
            task_handle: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    async fn log(&self, event: LogEvent) {
        if let Some(log_writer) = &self.log_writer {
            if let Ok(mut writer) = log_writer.try_lock() {
                let _ = writer.log(event);
            }
        }
    }

    pub async fn get_connection_state(&self) -> WebsocketConnectionState {
        self.connection_state.read().await.clone()
    }

    pub async fn is_connected(&self) -> bool {
        *self.connected.read().await
    }

    pub async fn get_last_location(&self) -> Option<String> {
        self.tracker.read().await.latest_location()
    }

    pub async fn get_last_error(&self) -> Option<String> {
        self.last_error.read().await.clone()
    }

    pub async fn current_generation(&self) -> u64 {
        self.tracker.read().await.generation()
    }

    /// 成功判定：現世代の接続で起動要求の送出以後に受信した本人の `location` が対象と一致。
    pub async fn fresh_location_matches(&self, target: &str, since: DateTime<Utc>) -> bool {
        self.tracker.read().await.matches_target(target, since)
    }

    pub async fn is_travelling_since(&self, since: DateTime<Utc>) -> bool {
        self.tracker.read().await.is_travelling_since(since)
    }

    pub async fn queue_for_target_since(
        &self,
        target: &str,
        since: DateTime<Utc>,
    ) -> Option<QueueKind> {
        self.tracker
            .read()
            .await
            .queue_for_target_since(target, since)
    }

    pub async fn connect(&self, auth_cookie: &str) -> Result<(), AppError> {
        if self.is_connected().await {
            self.log(LogEvent::debug("websocket", "WebSocketは接続済みです。"))
                .await;
            return Ok(());
        }

        *self.connection_state.write().await = WebsocketConnectionState::Connecting;
        *self.last_error.write().await = None;
        self.tracker.write().await.next_generation();
        if let Some(handle) = self.task_handle.lock().await.take() {
            handle.abort();
        }

        let url = format!(
            "wss://pipeline.vrchat.cloud/?authToken={}",
            urlencoding::encode(auth_cookie)
        );

        self.log(LogEvent::info("websocket", "WebSocket接続を開始します。"))
            .await;

        let mut request = url.as_str().into_client_request().map_err(|e| {
            AppError::Network(format!(
                "WebSocket接続リクエストの作成に失敗しました: {}",
                e
            ))
        })?;
        request
            .headers_mut()
            .insert(USER_AGENT, HeaderValue::from_static(APP_USER_AGENT));

        let (ws_stream, _) = match connect_async(request).await {
            Ok(result) => result,
            // 接続URLに認証トークンを含むため、エラーの詳細は画面にもログにも出さない。
            Err(_) => {
                let message =
                    "VRChatのリアルタイム通知（WebSocket）に接続できませんでした。".to_string();
                *self.connection_state.write().await = WebsocketConnectionState::Error;
                *self.connected.write().await = false;
                self.tracker.write().await.next_generation();
                *self.last_error.write().await = Some(message.clone());
                self.log(LogEvent::error("websocket", &message)).await;
                return Err(AppError::Network(message));
            }
        };

        *self.connection_state.write().await = WebsocketConnectionState::Connected;
        *self.connected.write().await = true;
        *self.last_error.write().await = None;
        info!("WebSocket connected");
        self.log(LogEvent::info("websocket", "WebSocketに接続しました。"))
            .await;

        let connected = self.connected.clone();
        let tracker = self.tracker.clone();
        let last_error = self.last_error.clone();
        let connection_state = self.connection_state.clone();
        let log_writer = self.log_writer.clone();
        let task_handle = self.task_handle.clone();

        let handle = tokio::spawn(async move {
            let (_, mut read) = ws_stream.split();

            let write_log = |event: LogEvent, log_writer: Option<Arc<Mutex<LogWriter>>>| async move {
                if let Some(log_writer) = log_writer {
                    if let Ok(mut writer) = log_writer.try_lock() {
                        let _ = writer.log(event);
                    }
                }
            };

            while let Some(msg) = read.next().await {
                match msg {
                    Ok(tokio_tungstenite::tungstenite::protocol::Message::Text(text)) => {
                        let now = Utc::now();
                        if let Some(location) = parse_user_location(&text) {
                            tracker.write().await.push_location(location.clone(), now);
                            info!("User location update: {}", location);
                            write_log(
                                LogEvent::debug(
                                    "websocket",
                                    &format!("現在位置を受信しました: {}", location),
                                ),
                                log_writer.clone(),
                            )
                            .await;
                        }
                        if let Some((kind, hint)) = parse_queue_event(&text) {
                            tracker.write().await.push_queue(kind, hint.clone(), now);
                            info!("Queue event: {:?} hint={:?}", kind, hint);
                            write_log(
                                LogEvent::info(
                                    "websocket",
                                    match kind {
                                        QueueKind::Joined => "キューに参加しました。",
                                        QueueKind::Ready => {
                                            "キューの順番が来ました。入室確認を続けます。"
                                        }
                                    },
                                ),
                                log_writer.clone(),
                            )
                            .await;
                        }
                    }
                    Ok(tokio_tungstenite::tungstenite::protocol::Message::Close(_)) => {
                        warn!("WebSocket closed");
                        let message = "WebSocket接続が切断されました。".to_string();
                        *last_error.write().await = Some(message.clone());
                        *connection_state.write().await = WebsocketConnectionState::Error;
                        *connected.write().await = false;
                        tracker.write().await.next_generation();
                        write_log(LogEvent::warn("websocket", &message), log_writer.clone()).await;
                        break;
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        let message =
                            "VRChatのリアルタイム通知（WebSocket）が切断されました。".to_string();
                        *last_error.write().await = Some(message.clone());
                        *connection_state.write().await = WebsocketConnectionState::Error;
                        *connected.write().await = false;
                        tracker.write().await.next_generation();
                        write_log(LogEvent::error("websocket", &message), log_writer.clone()).await;
                        break;
                    }
                    _ => {}
                }
            }

            *connected.write().await = false;
            if last_error.read().await.is_none() {
                *connection_state.write().await = WebsocketConnectionState::Disconnected;
            }
            tracker.write().await.next_generation();
            *task_handle.lock().await = None;
        });

        *self.task_handle.lock().await = Some(handle);

        Ok(())
    }

    pub async fn disconnect(&self) {
        *self.connection_state.write().await = WebsocketConnectionState::Disconnected;
        *self.connected.write().await = false;
        self.tracker.write().await.next_generation();
        *self.last_error.write().await = None;

        if let Some(handle) = self.task_handle.lock().await.take() {
            handle.abort();
        }

        info!("WebSocket disconnected");
        self.log(LogEvent::info("websocket", "WebSocket接続を切断しました。"))
            .await;
    }

    /// テスト用に受信サンプルを注入する。生成・鮮度ロジック自体は本番と同一。
    #[cfg(test)]
    pub(crate) async fn push_test_location(&self, location: &str, at: DateTime<Utc>) {
        self.tracker
            .write()
            .await
            .push_location(location.to_string(), at);
    }

    /// テスト用にqueue通知を注入する。
    #[cfg(test)]
    pub(crate) async fn push_test_queue(
        &self,
        kind: QueueKind,
        hint: Option<String>,
        at: DateTime<Utc>,
    ) {
        self.tracker.write().await.push_queue(kind, hint, at);
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum WsEvent {
    #[serde(rename = "user-location")]
    UserLocation {
        content: WsContent<UserLocationContent>,
    },
    #[serde(rename = "notification")]
    Notification,
    #[serde(rename = "friend-active")]
    FriendActive,
    #[serde(rename = "friend-location")]
    FriendLocation,
    #[serde(rename = "friend-offline")]
    FriendOffline,
    #[serde(rename = "friend-add")]
    FriendAdd,
    #[serde(rename = "friend-delete")]
    FriendDelete,
    #[serde(rename = "instance-queue-joined")]
    InstanceQueueJoined {
        #[serde(default)]
        content: Option<serde_json::Value>,
    },
    #[serde(rename = "instance-queue-ready")]
    InstanceQueueReady {
        #[serde(default)]
        content: Option<serde_json::Value>,
    },
}

#[derive(Debug, Deserialize)]
struct UserLocationContent {
    location: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WsContent<T> {
    Json(T),
    String(String),
}

fn parse_user_location(text: &str) -> Option<String> {
    match serde_json::from_str::<WsEvent>(text).ok()? {
        WsEvent::UserLocation { content } => match content {
            WsContent::Json(content) => Some(content.location),
            WsContent::String(content) => serde_json::from_str::<UserLocationContent>(&content)
                .ok()
                .map(|content| content.location),
        },
        _ => None,
    }
}

/// queue通知の種別と対象ヒントを抽出する。対象に一致する通知のみ扱う。
fn parse_queue_event(text: &str) -> Option<(QueueKind, Option<String>)> {
    match serde_json::from_str::<WsEvent>(text).ok()? {
        WsEvent::InstanceQueueJoined { content } => Some((
            QueueKind::Joined,
            content.as_ref().and_then(extract_location_hint),
        )),
        WsEvent::InstanceQueueReady { content } => Some((
            QueueKind::Ready,
            content.as_ref().and_then(extract_location_hint),
        )),
        _ => None,
    }
}

/// 通知content（文字列化JSONの場合あり）からlocation系の文字列を拾う。
fn extract_location_hint(content: &serde_json::Value) -> Option<String> {
    let value = match content {
        serde_json::Value::String(s) => {
            serde_json::from_str::<serde_json::Value>(s).unwrap_or(serde_json::Value::Null)
        }
        other => other.clone(),
    };
    let obj = value.as_object()?;
    for key in [
        "location",
        "instanceLocation",
        "instanceId",
        "instanceID",
        "worldId",
    ] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn parses_double_encoded_user_location_content() {
        let text = serde_json::json!({
            "type": "user-location",
            "content": "{\"location\":\"wrld_test:12345~group(grp_test)\"}"
        })
        .to_string();

        assert_eq!(
            parse_user_location(&text).as_deref(),
            Some("wrld_test:12345~group(grp_test)")
        );
    }

    #[test]
    fn queue_parser_table() {
        // object / contentなし / 文字列化 / 非queue の parser 分類を1本で確認する。
        let object = serde_json::json!({
            "type": "instance-queue-joined",
            "content": {"location": "wrld_a:1~group(g)", "position": 3}
        })
        .to_string();
        let ready = serde_json::json!({"type": "instance-queue-ready"}).to_string();
        let stringified = serde_json::json!({
            "type": "instance-queue-joined",
            "content": "{\"instanceId\": \"12345~group(grp_x)\"}"
        })
        .to_string();
        let non_queue = serde_json::json!({
            "type": "user-location",
            "content": {"location": "wrld_a:1"}
        })
        .to_string();
        let cases = vec![
            (
                "object",
                &object,
                Some((QueueKind::Joined, Some("wrld_a:1~group(g)".to_string()))),
            ),
            ("contentなし", &ready, Some((QueueKind::Ready, None))),
            (
                "文字列化",
                &stringified,
                Some((QueueKind::Joined, Some("12345~group(grp_x)".to_string()))),
            ),
            ("非queue", &non_queue, None),
        ];
        for (name, text, expected) in cases {
            assert_eq!(parse_queue_event(text), expected, "case: {}", name);
        }
    }

    fn tracker_with_location(location: &str, age_secs: i64) -> (LocationTracker, DateTime<Utc>) {
        let mut tracker = LocationTracker::new();
        let now = Utc::now();
        tracker.push_location(location.to_string(), now - Duration::seconds(age_secs));
        (tracker, now)
    }

    #[test]
    fn fresh_match_requires_current_generation_and_dispatch_time() {
        let (tracker, now) = tracker_with_location("wrld_a:1~group(g)", 5);
        assert!(tracker.matches_target("wrld_a:1~group(g)", now - Duration::seconds(10)));
        // 起動要求の送出より前の受信は成功に使わない。
        assert!(!tracker.matches_target("wrld_a:1~group(g)", now));
        // 別対象は不一致。
        assert!(!tracker.matches_target("wrld_b:2~group(g)", now - Duration::seconds(10)));
    }

    #[test]
    fn generation_bump_invalidates_old_samples() {
        let (mut tracker, now) = tracker_with_location("wrld_a:1~group(g)", 0);
        tracker.next_generation();
        assert!(!tracker.matches_target("wrld_a:1~group(g)", now - Duration::seconds(60)));
        assert_eq!(tracker.latest_location(), None);
    }

    #[test]
    fn travelling_is_not_a_match_but_detectable() {
        let (tracker, now) = tracker_with_location("traveling:wrld_a:1", 1);
        assert!(!tracker.matches_target("traveling:wrld_a:1", now - Duration::seconds(5)));
        assert!(tracker.is_travelling_since(now - Duration::seconds(5)));
        let (other, now) = tracker_with_location("wrld_a:1~group(g)", 1);
        assert!(!other.is_travelling_since(now - Duration::seconds(5)));
    }

    #[test]
    fn queue_hint_mismatch_is_ignored() {
        let mut tracker = LocationTracker::new();
        let now = Utc::now();
        tracker.push_queue(QueueKind::Joined, Some("wrld_other:9".to_string()), now);
        assert_eq!(
            tracker.queue_for_target_since("wrld_a:1~group(g)", now - Duration::seconds(5)),
            None
        );
        tracker.push_queue(QueueKind::Ready, None, now);
        assert_eq!(
            tracker.queue_for_target_since("wrld_a:1~group(g)", now - Duration::seconds(5)),
            Some(QueueKind::Ready)
        );
    }
}
