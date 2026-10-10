use crate::domain::models::{ApiBudgetEstimate, AppError, Candidate};
use crate::infra::http_client::HttpClient;
use crate::services::{base_poll_interval_secs, count_base_polls};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

/// 1pollあたりのGET数。group-specific user instancesの1GETのみ。
pub const REQUESTS_PER_POLL: u32 = 1;
/// カレンダーのページ取得サイズと上限。ページングの無限追跡を防ぐ。
const CALENDAR_PAGE_SIZE: i32 = 50;
const CALENDAR_MAX_ITEMS: usize = 200;
/// 取得ページ数の上限。変換に失敗した項目やoffsetを無視する応答があっても、
/// 要求回数がこれを超えないようにする（50件×4ページ=200件）。
const CALENDAR_MAX_PAGES: i32 = CALENDAR_MAX_ITEMS as i32 / CALENDAR_PAGE_SIZE;
/// 見積の警告・抑止閾値。現行窓(Full約40GET)では警告に出ない水準。
/// スケジューラと同一根拠(count_base_polls×1)のため、scheduler変更時は要再較正。
pub const BUDGET_WARN_THRESHOLD: u32 = 72;
pub const BUDGET_BLOCK_THRESHOLD: u32 = 120;

pub struct VrchatApiService {
    http_client: Arc<Mutex<HttpClient>>,
}

impl VrchatApiService {
    pub fn new(http_client: Arc<Mutex<HttpClient>>) -> Self {
        Self { http_client }
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(&self, endpoint: &str) -> Result<T, AppError> {
        let client = self.http_client.lock().await;
        let data: T = client.get(endpoint).await.map_err(AppError::from)?;
        Ok(data)
    }

    /// 監視poll・手動確認用の1GET。group-specific user instancesは
    /// location/instanceId/worldId/world/displayName/full/hasCapacityForYou/
    /// queueEnabled/queueSize/userCount/capacity/calendarEntryIdを1応答で返すため、
    /// 旧2GET（group instances + user instances）のlocation突合は不要。
    /// SDK 1.20.9の `get_user_group_instances_for_group` と同一エンドポイント。
    pub async fn get_group_instances_for_group(
        &self,
        user_id: &str,
        group_id: &str,
    ) -> Result<UserGroupInstancesResponse, AppError> {
        ensure_path_segment(group_id)?;
        let endpoint = format!("/users/{}/instances/groups/{}", user_id, group_id);
        let raw: serde_json::Value = self.get_json(&endpoint).await?;
        let instance_keys = instance_field_names(&raw);
        let mut response: UserGroupInstancesResponse =
            serde_json::from_value(raw).map_err(AppError::from)?;
        response.instance_keys = instance_keys;
        Ok(response)
    }

    /// 1会場の詳細から名前だけを取る。一覧で名前が空の会場を補うために使う。
    pub async fn get_instance_name(
        &self,
        world_id: &str,
        instance_id: &str,
    ) -> Result<InstanceNameProbe, AppError> {
        if world_id.is_empty() || instance_id.is_empty() {
            return Err(AppError::InvalidInput(
                "会場のIDが空のため詳細を取得できません".to_string(),
            ));
        }
        ensure_path_segment(world_id)?;
        let endpoint = format!(
            "/instances/{}:{}",
            urlencoding::encode(world_id),
            urlencoding::encode(instance_id)
        );
        self.get_json(&endpoint).await
    }

    /// 秘密を含めない診断用の試し取得。1GET + カレンダー取得を雛形エンドポイントの証跡にまとめる。
    /// 取得自体は通常認証で行い、出力に秘密を含めない。
    /// カレンダー取得失敗時は候補証跡を優先し、calendarAvailable=falseで返す。
    pub async fn probe_secret_free_diagnostic(
        &self,
        user_id: &str,
        group_id: &str,
    ) -> Result<serde_json::Value, AppError> {
        let instances = self
            .get_group_instances_for_group(user_id, group_id)
            .await?;
        let calendar = self.get_group_calendar(group_id, None).await;
        let calendar_available = calendar.is_ok();
        let events = calendar.unwrap_or_default();
        build_secret_free_diagnostic(group_id, &instances.instances, &events, calendar_available)
    }

    pub async fn get_group_summary(&self, group_id: &str) -> Result<GroupSummary, AppError> {
        ensure_path_segment(group_id)?;
        let endpoint = format!("/groups/{}", group_id);
        self.get_json(&endpoint).await
    }

    /// グループカレンダーをページング取得する。n/offsetで辿り、
    /// 短いページで終了する。欠落・不正エントリは1件ずつ捨て、頁全体を壊さない。
    pub async fn get_group_calendar(
        &self,
        group_id: &str,
        date: Option<DateTime<Utc>>,
    ) -> Result<Vec<CalendarEvent>, AppError> {
        ensure_path_segment(group_id)?;
        let date_query = match date {
            Some(d) => {
                let date_str = d.format("%Y-%m-%dT%H:%M:%SZ").to_string();
                format!("&date={}", urlencoding::encode(&date_str))
            }
            None => String::new(),
        };

        let mut events = Vec::new();
        for page_index in 0..CALENDAR_MAX_PAGES {
            let offset = page_index * CALENDAR_PAGE_SIZE;
            let endpoint = format!(
                "/calendar/{}?n={}&offset={}{}",
                group_id, CALENDAR_PAGE_SIZE, offset, date_query
            );
            let page: CalendarPage = self.get_json(&endpoint).await?;
            let items = page.results.unwrap_or_default();
            let short_page = items.len() < CALENDAR_PAGE_SIZE as usize;
            for raw in items {
                if let Some(event) = CalendarEvent::from_raw(raw) {
                    events.push(event);
                    if events.len() >= CALENDAR_MAX_ITEMS {
                        return Ok(events);
                    }
                }
            }
            if short_page {
                break;
            }
        }
        Ok(events)
    }

    pub async fn invite_myself(&self, world_id: &str, instance_id: &str) -> Result<(), AppError> {
        let endpoint = format!("/invite/myself/to/{}:{}", world_id, instance_id);
        let client = self.http_client.lock().await;

        #[derive(Deserialize)]
        struct EmptyResponse {}

        let _: EmptyResponse = client
            .post(&endpoint, &serde_json::json!({}))
            .await
            .map_err(AppError::from)?;

        Ok(())
    }

    /// API見積。スケジューラと同一根拠(count_base_polls×1GET)で計算する。
    /// 3値はresolve_monitor_windowの解決結果をそのまま渡す。
    /// 推定は基本間隔で計算し、実測はjitterで±20%乖離しうる。
    pub fn estimate_api_budget(
        &self,
        event_start: DateTime<Utc>,
        monitor_start: DateTime<Utc>,
        monitor_end: DateTime<Utc>,
    ) -> ApiBudgetEstimate {
        let polls = count_base_polls(event_start, monitor_start, monitor_end, Utc::now());
        let estimated_requests = polls * REQUESTS_PER_POLL;

        ApiBudgetEstimate {
            profile_name: format!(
                "Current({}s/{}s)",
                crate::services::POLL_INTERVAL_RELAXED_SECS,
                crate::services::POLL_INTERVAL_FAST_SECS
            ),
            estimated_requests,
            warn_threshold: BUDGET_WARN_THRESHOLD,
            block_threshold: BUDGET_BLOCK_THRESHOLD,
            should_warn: estimated_requests >= BUDGET_WARN_THRESHOLD,
            should_block: estimated_requests >= BUDGET_BLOCK_THRESHOLD,
        }
    }

    /// 現在のpoll間隔(秒)。UI表示用。スケジューラと同一関数を使う。
    pub fn current_poll_interval_secs(&self, event_start: DateTime<Utc>) -> u64 {
        base_poll_interval_secs(Utc::now(), event_start)
    }
}

/// 診断出力のエンドポイント雛形。実際のIDは証跡へ出さない。
pub const DIAG_ENDPOINT_TEMPLATE: &str = "/users/{userId}/instances/groups/{groupId}";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSummary {
    pub id: String,
    pub name: String,
    pub short_code: Option<String>,
    pub member_count: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorldInfo {
    pub id: String,
    pub name: String,
    pub capacity: i32,
    #[serde(rename = "recommendedCapacity")]
    pub recommended_capacity: i32,
}

#[derive(Debug, Deserialize)]
pub struct UserGroupInstancesResponse {
    // SDKでは任意。欠落・nullでもpollを壊さない。
    #[serde(rename = "fetchedAt", default)]
    pub fetched_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub instances: Vec<DetailedInstance>,
    /// 応答の各会場が持つフィールド名（値は含めない）。名前の所在を調べる診断用。
    #[serde(skip)]
    pub instance_keys: Vec<String>,
}

/// 応答の各会場が持つフィールド名の和集合。値は読まない。
fn instance_field_names(raw: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = raw
        .get("instances")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|i| i.as_object())
        .flat_map(|o| o.keys().cloned())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// `/instances/{worldId}:{instanceId}` の名前部分だけ。ほかのフィールドは読まない。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct InstanceNameProbe {
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// 会場の説明文。名前が空のとき、主催者が名前をここへ書いていないか確かめる診断用。
    #[serde(default)]
    pub description: Option<String>,
}

/// group-specific user instancesの1件分。SDK 1.20.9のInstanceと同一shape。
/// full/queueは必須、表示・容量・calendarEntryIdは任意として受容する。
#[derive(Debug, Clone, Deserialize)]
pub struct DetailedInstance {
    pub id: String,
    #[serde(rename = "instanceId")]
    pub instance_id: String,
    pub location: String,
    #[serde(rename = "worldId")]
    pub world_id: String,
    #[serde(default)]
    pub world: Option<WorldInfo>,
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    pub full: bool,
    #[serde(rename = "hasCapacityForYou", default)]
    pub has_capacity_for_you: Option<bool>,
    #[serde(rename = "queueEnabled", default)]
    pub queue_enabled: Option<bool>,
    #[serde(rename = "queueSize", default)]
    pub queue_size: Option<i32>,
    #[serde(rename = "userCount")]
    pub user_count: i32,
    // SDKでは任意。欠落してもpollを壊さない。
    #[serde(default)]
    pub capacity: Option<i32>,
    // SDKでは二重Option（欠落・null・文字列）。意味の推測はしない。
    #[serde(rename = "calendarEntryId", default)]
    pub calendar_entry_id: Option<String>,
}

impl DetailedInstance {
    /// 1GET応答を候補へ写す。旧2GET突合はしない。
    /// world_id欠落時はworld.idで補う。member数はuserCountを使う。
    pub fn to_candidate(&self) -> Candidate {
        Candidate {
            location: self.location.clone(),
            instance_id: self.instance_id.clone(),
            world_id: if self.world_id.trim().is_empty() {
                self.world
                    .as_ref()
                    .map(|w| w.id.clone())
                    .unwrap_or_default()
            } else {
                self.world_id.clone()
            },
            // 会場名が空ならワールド名で代える。会場ごとに別ワールドを立てる運用では
            // ワールド名が会場の見分けになる。同じワールドなら名前が重なり、自動では選ばない。
            // 空文字は名前なしとして扱う（名前条件の判定を誤らせない）。
            display_name: self
                .display_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| self.world.as_ref().map(|w| w.name.clone()))
                .filter(|name| !name.trim().is_empty()),
            member_count: self.user_count,
            has_capacity_for_you: self.has_capacity_for_you,
            is_full: Some(self.full),
            queue_enabled: self.queue_enabled,
            queue_size: self.queue_size,
            calendar_entry_id: self.calendar_entry_id.clone(),
        }
    }
}

/// 1GET応答の一覧を候補へ写す。
pub fn candidates_from_detailed(instances: &[DetailedInstance]) -> Vec<Candidate> {
    instances
        .iter()
        .map(DetailedInstance::to_candidate)
        .collect()
}

#[derive(Debug, Deserialize)]
struct CalendarPage {
    #[serde(default)]
    results: Option<Vec<RawCalendarEvent>>,
}

/// 実APIの任意フィールドに耐える中間形。欠落・nullを受理し、必須欠けは1件捨て。
#[derive(Debug, Deserialize)]
struct RawCalendarEvent {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "startsAt", default)]
    starts_at: Option<String>,
    #[serde(rename = "endsAt", default)]
    ends_at: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "usesInstanceOverflow", default)]
    uses_instance_overflow: Option<bool>,
    // occurrenceKind（single/series/occurrence等）。意味の推測はせず保持する。
    #[serde(rename = "occurrenceKind", default)]
    occurrence_kind: Option<String>,
    #[serde(default)]
    recurrence: Option<RawCalendarRecurrence>,
}

/// recurrence（frequency/interval/daysOfWeek/timezone等）。意味の推測はせず保持する。
#[derive(Debug, Deserialize)]
struct RawCalendarRecurrence {
    #[serde(default)]
    frequency: Option<String>,
    #[serde(default)]
    interval: Option<i32>,
    #[serde(rename = "daysOfWeek", default)]
    days_of_week: Vec<String>,
    #[serde(default)]
    timezone: Option<String>,
}

/// recurrence（frequency/interval/daysOfWeek/timezone等）。意味の推測はせず保持する。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarRecurrence {
    #[serde(default)]
    pub frequency: Option<String>,
    #[serde(default)]
    pub interval: Option<i32>,
    #[serde(rename = "daysOfWeek", default)]
    pub days_of_week: Vec<String>,
    #[serde(default)]
    pub timezone: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub description: Option<String>,
    pub uses_instance_overflow: Option<bool>,
    pub occurrence_kind: Option<String>,
    pub recurrence: Option<CalendarRecurrence>,
}

impl CalendarEvent {
    fn from_raw(raw: RawCalendarEvent) -> Option<Self> {
        let id = raw.id.filter(|s| !s.trim().is_empty())?;
        let title = raw.title.filter(|s| !s.trim().is_empty())?;
        let starts_at = raw.starts_at.as_deref()?.parse::<DateTime<Utc>>().ok()?;
        let ends_at = raw
            .ends_at
            .as_deref()
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or(starts_at);
        Some(Self {
            id,
            title,
            starts_at,
            ends_at,
            description: raw.description.filter(|s| !s.trim().is_empty()),
            uses_instance_overflow: raw.uses_instance_overflow,
            occurrence_kind: raw.occurrence_kind.filter(|s| !s.trim().is_empty()),
            recurrence: raw.recurrence.map(|r| CalendarRecurrence {
                frequency: r.frequency.filter(|s| !s.trim().is_empty()),
                interval: r.interval,
                days_of_week: r.days_of_week,
                timezone: r.timezone.filter(|s| !s.trim().is_empty()),
            }),
        })
    }
}

/// location/instanceIdの `~...` 接尾辞（nonce/hidden等）を落とす。秘密情報を証跡へ出さない。
pub fn redact_location_tag(value: &str) -> String {
    match value.split_once('~') {
        Some((head, _)) => head.to_string(),
        None => value.to_string(),
    }
}

/// secret-free診断の候補1件分。人間比較用のevidenceのみ。秘密は含めない。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCandidate {
    pub location: String,
    pub display_name: Option<String>,
    pub full: bool,
    pub has_capacity_for_you: Option<bool>,
    pub queue_enabled: Option<bool>,
    pub queue_size: Option<i32>,
    pub user_count: i32,
    pub capacity: Option<i32>,
    pub world_id: String,
    pub calendar_entry_id: Option<String>,
}

/// 診断JSONに秘密情報が混ざっていないか検査する。deny-list該当でfailさせる。
/// userId/cookie/Authorization/nonce等は証跡へ含めない。
fn deny_list_hit(serialized: &str) -> Option<&'static str> {
    const KEY_TOKENS: &[&str] = &[
        "\"password\"",
        "\"cookie\"",
        "\"authorization\"",
        "\"set-cookie\"",
        "\"token\"",
        "\"session\"",
        "\"nonce\"",
        "\"auth\"",
        "\"secret\"",
        "\"apikey\"",
        "\"api_key\"",
        "\"userid\"",
        "\"user_id\"",
    ];
    const VALUE_TOKENS: &[&str] = &["auth=", "nonce(", "~hidden(", "~nonce(", "usr_"];
    let lowered = serialized.to_lowercase();
    for token in KEY_TOKENS {
        if lowered.contains(token) {
            return Some(token);
        }
    }
    if let Some(token) = VALUE_TOKENS
        .iter()
        .copied()
        .find(|token| lowered.contains(token))
    {
        return Some(token);
    }
    None
}

/// 秘密を含めない診断出力を組み立てる。禁止リストの検出で失敗させる。
/// `userId` / `cookie` / `Authorization` / `nonce` は含めない。エンドポイントは雛形のみ。
pub fn build_secret_free_diagnostic(
    group_id: &str,
    instances: &[DetailedInstance],
    events: &[CalendarEvent],
    calendar_available: bool,
) -> Result<serde_json::Value, AppError> {
    let candidates: Vec<DiagnosticCandidate> = instances
        .iter()
        .map(|d| DiagnosticCandidate {
            location: redact_location_tag(&d.location),
            display_name: d.display_name.clone(),
            full: d.full,
            has_capacity_for_you: d.has_capacity_for_you,
            queue_enabled: d.queue_enabled,
            queue_size: d.queue_size,
            user_count: d.user_count,
            capacity: d.capacity,
            world_id: d.world_id.clone(),
            calendar_entry_id: d.calendar_entry_id.clone(),
        })
        .collect();
    let calendar_events: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "occurrenceKind": e.occurrence_kind,
                "recurrence": e.recurrence,
            })
        })
        .collect();
    let export = serde_json::json!({
        "endpointTemplate": DIAG_ENDPOINT_TEMPLATE,
        "groupId": group_id,
        "candidateCount": candidates.len(),
        "candidates": candidates,
        "calendarEventCount": calendar_events.len(),
        "calendarEvents": calendar_events,
        "calendarAvailable": calendar_available,
    });
    let serialized = serde_json::to_string(&export).map_err(AppError::from)?;
    if let Some(token) = deny_list_hit(&serialized) {
        return Err(AppError::Operation(format!(
            "診断出力に秘密情報の疑いがあるため失敗させます（検出: {}）。値を送らず管理者に連絡してください。",
            token
        )));
    }
    Ok(export)
}

/// URLパスへ埋め込むIDの検査。`/` `?` `#` `.` などでAPIの別エンドポイントへ
/// 化けないよう、英数字・`_`・`-` だけを許可する（`grp_...` 形式はこれで足りる）。
fn ensure_path_segment(id: &str) -> Result<(), AppError> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if valid {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "グループIDの形式が正しくありません（英数字・_・- のみ使用できます）".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn path_segment_rejects_url_structure_chars() {
        assert!(super::ensure_path_segment("grp_12345678-1234-1234-1234-123456789012").is_ok());
        assert!(super::ensure_path_segment("grp_old").is_ok());
        for bad in [
            "",
            "grp_x/../..",
            "grp_x?n=1",
            "grp_x#f",
            "grp x",
            "grp_x%2F",
            "grp_..",
        ] {
            assert!(super::ensure_path_segment(bad).is_err(), "{bad}");
        }
    }

    use super::*;
    use chrono::Duration;

    fn window_from_now() -> (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>) {
        let now = Utc::now();
        let event = now + Duration::minutes(10);
        (
            event,
            event - Duration::minutes(3),
            event + Duration::minutes(2),
        )
    }

    #[test]
    fn budget_estimate_for_window() {
        let client = Arc::new(Mutex::new(HttpClient::new().unwrap()));
        let api = VrchatApiService::new(client);
        let (event, start, end) = window_from_now();
        let estimate = api.estimate_api_budget(event, start, end);
        // 緩和150秒/15 + 高速150秒/5 = 10 + 30 = 40poll、1poll=1GETで40要求
        assert_eq!(estimate.estimated_requests, 40);
        assert!(!estimate.should_warn);
        assert!(!estimate.should_block);
    }

    #[test]
    fn budget_is_zero_after_window() {
        let client = Arc::new(Mutex::new(HttpClient::new().unwrap()));
        let api = VrchatApiService::new(client);
        let now = Utc::now();
        let event = now - Duration::minutes(10);
        let estimate = api.estimate_api_budget(
            event,
            event - Duration::minutes(3),
            event + Duration::minutes(2),
        );
        assert_eq!(estimate.estimated_requests, 0);
        assert!(!estimate.should_warn);
    }

    #[test]
    fn deserialize_for_group_response_full_shape() {
        // group-specific 1GETのshape。SDK 1.20.9のInstanceと同一項目。
        let json = serde_json::json!({
            "fetchedAt": "2025-03-30T12:00:00Z",
            "instances": [
                {
                    "id": "inst_1",
                    "instanceId": "12345~group(grp_test)",
                    "location": "wrld_test:12345~group(grp_test)",
                    "worldId": "wrld_test",
                    "world": {
                        "id": "wrld_test",
                        "name": "Test World",
                        "capacity": 40,
                        "recommendedCapacity": 32
                    },
                    "displayName": "Event Instance",
                    "full": false,
                    "hasCapacityForYou": true,
                    "queueEnabled": false,
                    "queueSize": 0,
                    "userCount": 12,
                    "capacity": 40,
                    "calendarEntryId": "cal_1"
                }
            ]
        });

        let parsed: UserGroupInstancesResponse = serde_json::from_value(json).unwrap();

        assert_eq!(parsed.instances.len(), 1);
        let instance = &parsed.instances[0];
        assert_eq!(instance.display_name.as_deref(), Some("Event Instance"));
        assert_eq!(instance.has_capacity_for_you, Some(true));
        assert_eq!(instance.calendar_entry_id.as_deref(), Some("cal_1"));
        assert_eq!(instance.world.as_ref().unwrap().name, "Test World");
        assert_eq!(instance.capacity, Some(40));
    }

    #[test]
    fn deserialize_for_group_response_tolerates_missing_optionals() {
        // fetchedAt・表示・容量・calendarEntryIdの欠落・nullでもpollを壊さない。
        let json = serde_json::json!({
            "instances": [
                {
                    "id": "inst_2",
                    "instanceId": "67890",
                    "location": "wrld_test:67890",
                    "worldId": "wrld_test",
                    "displayName": null,
                    "full": true,
                    "userCount": 40,
                    "calendarEntryId": null
                }
            ]
        });

        let parsed: UserGroupInstancesResponse = serde_json::from_value(json).unwrap();

        assert_eq!(parsed.fetched_at, None);
        assert_eq!(parsed.instances.len(), 1);
        assert_eq!(parsed.instances[0].display_name, None);
        assert_eq!(parsed.instances[0].has_capacity_for_you, None);
        assert_eq!(parsed.instances[0].calendar_entry_id, None);
        assert_eq!(parsed.instances[0].capacity, None);
        // 欠落時は空扱いでpollを壊さない。
        let empty: UserGroupInstancesResponse =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(empty.instances.is_empty());
    }

    #[test]
    fn to_candidate_maps_single_get_fields() {
        let instance: DetailedInstance = serde_json::from_value(serde_json::json!({
            "id": "inst_1",
            "instanceId": "12345~group(grp_test)",
            "location": "wrld_test:12345~group(grp_test)",
            "worldId": "",
            "world": {
                "id": "wrld_test",
                "name": "Test World",
                "capacity": 40,
                "recommendedCapacity": 32
            },
            "displayName": "Event Instance",
            "full": false,
            "hasCapacityForYou": true,
            "queueEnabled": false,
            "queueSize": 0,
            "userCount": 12,
            "capacity": 40,
            "calendarEntryId": "cal_1"
        }))
        .unwrap();

        let candidate = instance.to_candidate();
        // member数はuserCount、world_idはworld.idで補う。
        assert_eq!(candidate.member_count, 12);
        assert_eq!(candidate.world_id, "wrld_test");
        assert_eq!(candidate.calendar_entry_id.as_deref(), Some("cal_1"));
        assert_eq!(candidate.is_full, Some(false));
        assert!(candidate.is_joinable());
        assert_eq!(candidates_from_detailed(&[instance]).len(), 1);
    }

    #[test]
    fn blank_display_name_becomes_none_and_keys_are_listed() {
        let raw = serde_json::json!({
            "instances": [{
                "id": "inst_1",
                "instanceId": "1~group(grp_test)",
                "location": "wrld_test:1~group(grp_test)",
                "worldId": "wrld_test",
                "displayName": "",
                "full": false,
                "userCount": 1
            }, {
                "id": "inst_2",
                "instanceId": "2~group(grp_test)",
                "location": "wrld_test:2~group(grp_test)",
                "worldId": "wrld_test",
                "displayName": null,
                "full": true,
                "userCount": 40,
                "extra": 1
            }]
        });
        let keys = instance_field_names(&raw);
        assert!(keys.contains(&"extra".to_string()));
        assert_eq!(keys.iter().filter(|k| *k == "displayName").count(), 1);
        let response: UserGroupInstancesResponse = serde_json::from_value(raw).unwrap();
        let candidates = candidates_from_detailed(&response.instances);
        assert!(candidates.iter().all(|c| c.display_name.is_none()));
    }

    #[test]
    fn missing_display_name_falls_back_to_world_name() {
        let instance = |display_name: serde_json::Value| -> DetailedInstance {
            serde_json::from_value(serde_json::json!({
                "id": "inst_1",
                "instanceId": "1~group(grp_test)",
                "location": "wrld_test:1~group(grp_test)",
                "worldId": "wrld_test",
                "world": {
                    "id": "wrld_test",
                    "name": "Event_第2インスタンス",
                    "capacity": 40,
                    "recommendedCapacity": 32
                },
                "displayName": display_name,
                "full": false,
                "userCount": 1
            }))
            .unwrap()
        };
        assert_eq!(
            instance(serde_json::Value::Null)
                .to_candidate()
                .display_name
                .as_deref(),
            Some("Event_第2インスタンス")
        );
        assert_eq!(
            instance(serde_json::json!(""))
                .to_candidate()
                .display_name
                .as_deref(),
            Some("Event_第2インスタンス")
        );
        assert_eq!(
            instance(serde_json::json!("Test Event"))
                .to_candidate()
                .display_name
                .as_deref(),
            Some("Test Event")
        );
    }

    #[test]
    fn calendar_request_count_is_bounded_by_pages() {
        // 変換できない項目ばかりでも、要求は最大でこのページ数で止まる。
        assert_eq!(super::CALENDAR_MAX_PAGES, 4);
        assert_eq!(
            super::CALENDAR_MAX_PAGES * super::CALENDAR_PAGE_SIZE,
            super::CALENDAR_MAX_ITEMS as i32
        );
    }

    #[test]
    fn calendar_page_tolerates_optional_fields() {
        let json = serde_json::json!({
            "hasNext": true,
            "totalCount": 3,
            "results": [
                {
                    "id": "evt_1",
                    "title": "Group Event",
                    "startsAt": "2025-03-30T12:00:00Z",
                    "endsAt": "2025-03-30T13:00:00Z",
                    "description": "desc",
                    "usesInstanceOverflow": false
                },
                {
                    "id": "evt_2",
                    "title": "No Optional Fields",
                    "startsAt": "2025-03-30T14:00:00Z"
                },
                {
                    "id": "",
                    "title": "Missing Id Is Dropped",
                    "startsAt": "2025-03-30T15:00:00Z"
                },
                {
                    "id": "evt_4",
                    "title": "Bad Date Is Dropped",
                    "startsAt": "not-a-date"
                }
            ]
        });

        let page: CalendarPage = serde_json::from_value(json).unwrap();
        let events: Vec<CalendarEvent> = page
            .results
            .unwrap()
            .into_iter()
            .filter_map(CalendarEvent::from_raw)
            .collect();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "evt_1");
        assert_eq!(events[0].uses_instance_overflow, Some(false));
        // endsAt欠落はstartsAtへ退化し、頁全体は壊さない
        assert_eq!(events[1].id, "evt_2");
        assert_eq!(events[1].ends_at, events[1].starts_at);
        assert_eq!(events[1].description, None);
    }

    #[test]
    fn calendar_event_keeps_occurrence_and_recurrence() {
        let json = serde_json::json!({
            "results": [
                {
                    "id": "evt_series",
                    "title": "Weekly Event",
                    "startsAt": "2026-09-06T12:00:00Z",
                    "endsAt": "2026-09-06T13:00:00Z",
                    "occurrenceKind": "series",
                    "recurrence": {
                        "frequency": "weekly",
                        "interval": 1,
                        "daysOfWeek": ["sunday"],
                        "timezone": "Asia/Tokyo"
                    }
                },
                {
                    "id": "evt_single",
                    "title": "Single Event",
                    "startsAt": "2026-09-07T12:00:00Z"
                }
            ]
        });

        let page: CalendarPage = serde_json::from_value(json).unwrap();
        let events: Vec<CalendarEvent> = page
            .results
            .unwrap()
            .into_iter()
            .filter_map(CalendarEvent::from_raw)
            .collect();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].occurrence_kind.as_deref(), Some("series"));
        let recurrence = events[0].recurrence.as_ref().expect("recurrenceを捨てない");
        assert_eq!(recurrence.frequency.as_deref(), Some("weekly"));
        assert_eq!(recurrence.interval, Some(1));
        assert_eq!(recurrence.days_of_week, vec!["sunday".to_string()]);
        assert_eq!(recurrence.timezone.as_deref(), Some("Asia/Tokyo"));
        assert_eq!(events[1].occurrence_kind, None);
        assert_eq!(events[1].recurrence, None);
    }

    fn diagnostic_instance(location: &str) -> DetailedInstance {
        DetailedInstance {
            id: "inst_1".to_string(),
            instance_id: "12345~group(grp_test)".to_string(),
            location: location.to_string(),
            world_id: "wrld_test".to_string(),
            world: None,
            display_name: Some("Event Instance".to_string()),
            full: false,
            has_capacity_for_you: Some(true),
            queue_enabled: Some(false),
            queue_size: Some(0),
            user_count: 12,
            capacity: Some(40),
            calendar_entry_id: Some("cal_1".to_string()),
        }
    }

    #[test]
    fn diagnostic_export_normal_shape() {
        // 正常系の証拠保持と人間比較用IDの露出を1本で確認する。
        // 失敗系（deny-list漏えい）は別に独立維持する。
        let instances = vec![diagnostic_instance(
            "wrld_test:12345~hidden(usr_c1644b5b)~region(eu)~nonce(27e8414a)",
        )];
        let export = build_secret_free_diagnostic("grp_test", &instances, &[], true).unwrap();
        assert_eq!(export["endpointTemplate"], DIAG_ENDPOINT_TEMPLATE);
        assert_eq!(export["groupId"], "grp_test");
        assert_eq!(export["candidateCount"], 1);
        assert_eq!(export["candidates"][0]["location"], "wrld_test:12345");
        assert_eq!(export["candidates"][0]["calendarEntryId"], "cal_1");
        assert_eq!(export["candidates"][0]["hasCapacityForYou"], true);
        // 実ID・nonceは出さない。
        let serialized = serde_json::to_string(&export).unwrap();
        assert!(!serialized.contains("nonce"));
        assert!(!serialized.contains("usr_"));
        assert!(!serialized.contains("27e8414a"));
        // カレンダー側のIDも人間比較可能。
        let events = vec![CalendarEvent {
            id: "evt_1".to_string(),
            title: "Weekly Event".to_string(),
            starts_at: "2026-09-06T12:00:00Z".parse().unwrap(),
            ends_at: "2026-09-06T13:00:00Z".parse().unwrap(),
            description: None,
            uses_instance_overflow: None,
            occurrence_kind: Some("series".to_string()),
            recurrence: Some(CalendarRecurrence {
                frequency: Some("weekly".to_string()),
                interval: Some(1),
                days_of_week: vec!["sunday".to_string()],
                timezone: Some("Asia/Tokyo".to_string()),
            }),
        }];
        let export = build_secret_free_diagnostic("grp_test", &instances, &events, true).unwrap();
        assert_eq!(export["candidateCount"], 1);
        assert_eq!(export["calendarEventCount"], 1);
        assert_eq!(export["calendarEvents"][0]["id"], "evt_1");
        assert_eq!(
            export["calendarEvents"][0]["recurrence"]["frequency"],
            "weekly"
        );
    }

    #[test]
    fn diagnostic_export_fails_on_deny_list_hit() {
        let mut leaked = diagnostic_instance("wrld_test:12345");
        leaked.display_name = Some("auth=stolen-cookie-value".to_string());
        assert!(build_secret_free_diagnostic("grp_test", &[leaked], &[], true).is_err());
    }
}
