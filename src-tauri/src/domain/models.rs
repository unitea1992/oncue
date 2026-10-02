use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

/// 名前照合用の正規化。NFKCで全角・半角などの表記ゆれを吸収してから小文字化する。
/// 例: 全角の１と半角の1、半角カナと全角カナは同じものとして扱う。
fn normalize_name_for_match(value: &str) -> String {
    value.nfkc().collect::<String>().to_lowercase()
}

/// 画面へそのまま出すエラー。表示文は各variantの文字列だけで、英語の接頭辞や
/// VRChatの応答本文などの生データは含めない（生データが必要ならログへ出す）。
#[derive(Debug, Clone, Error)]
pub enum AppError {
    #[error("{0}")]
    Auth(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Api(String),
    #[error("{0}")]
    Storage(String),
    #[error("{0}")]
    Network(String),
    #[error("VRChat APIの制限に達しました。しばらく時間をおいてから再試行してください。")]
    RateLimit { retry_after_secs: Option<u64> },
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Operation(String),
    #[error("操作を取り消しました。")]
    Cancelled,
}

impl AppError {
    /// 429相当のエラーを作る。Retry-After秒数が分かれば尊重される。
    pub fn rate_limited(retry_after_secs: Option<u64>) -> Self {
        AppError::RateLimit { retry_after_secs }
    }

    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            AppError::RateLimit { retry_after_secs } => *retry_after_secs,
            _ => None,
        }
    }
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let text = match self {
            AppError::RateLimit {
                retry_after_secs: Some(secs),
            } => format!("{}（{}秒後に再試行できます）", self, secs),
            _ => self.to_string(),
        };
        serializer.serialize_str(&text)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Storage(format!("ファイルの読み書きに失敗しました: {}", e))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Storage(format!(
            "保存データの形式が正しくありません（{}行目）。",
            e.line()
        ))
    }
}

/// イベントの開催予定。利用者には「イベント」と表示する。
/// serde表現はTypeScriptの閉じたunionに対応する:
/// `{ "kind": "once", "startsAt": "<UTC RFC3339>" }` /
/// `{ "kind": "weekly", "weekday": 0-6, "time": "HH:MM[:SS]" }` (JST) /
/// `{ "kind": "biweekly", "weekday": 0-6, "time": "HH:MM[:SS]", "anchorDate": "YYYY-MM-DD" }` (JST)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EventSchedule {
    Once {
        #[serde(rename = "startsAt")]
        starts_at: DateTime<Utc>,
    },
    /// 毎週。weekdayは0=日曜〜6=土曜（JS Date.getDayと同一）。時刻はJST。
    Weekly {
        weekday: u8,
        #[serde(rename = "time")]
        time: String,
    },
    /// 隔週。anchor_date（JST暦日YYYY-MM-DD）が周期の基準。必須。
    Biweekly {
        weekday: u8,
        #[serde(rename = "time")]
        time: String,
        #[serde(rename = "anchorDate")]
        anchor_date: String,
    },
}

/// JSTのUTCに対する秒オフセット。Asia/TokyoはDSTなしの固定値。
pub const JST_OFFSET_SECS: i32 = 9 * 3600;
/// 発生時刻の猶予。発生+2分までは当回を有効とする。
pub const SCHEDULE_GRACE_MINUTES: i64 = 2;
/// 当日再開の猶予（時間）。当回+2分を過ぎても、この時間内かつ同日なら当日の回を対象にする。
pub const SAME_DAY_RESTART_HOURS: i64 = 6;

fn jst_offset() -> FixedOffset {
    FixedOffset::east_opt(JST_OFFSET_SECS).expect("JST offset (9 hours) should always be valid")
}

fn weekday_ja(weekday: u8) -> &'static str {
    match weekday {
        0 => "日",
        1 => "月",
        2 => "火",
        3 => "水",
        4 => "木",
        5 => "金",
        _ => "土",
    }
}

/// weekdayの検証。0=日曜〜6=土曜。
pub fn validate_weekday(weekday: u8) -> Result<(), AppError> {
    if weekday <= 6 {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "曜日は0（日曜）〜6（土曜）で指定してください: {}",
            weekday
        )))
    }
}

/// anchor日付（YYYY-MM-DD）の検証。存在しない日付の繰り上がりを認めない。
pub fn parse_anchor_date(anchor: &str) -> Result<NaiveDate, AppError> {
    NaiveDate::parse_from_str(anchor.trim(), "%Y-%m-%d").map_err(|_| {
        AppError::InvalidInput(format!(
            "基準日の形式が正しくありません（YYYY-MM-DD）: {}",
            anchor
        ))
    })
}

/// weekly/biweeklyの次回発生をUTC瞬間で解決する。日付・曜日計算の正本はRust側。
/// interval_weeksは1（毎週）か2（隔週）。隔週はanchor必須。
/// JSTの暦で曜日を合わせ、当回+2分猶予を過ぎていれば次回へ進める。
pub fn resolve_weekly_occurrence_utc(
    weekday: u8,
    time: NaiveTime,
    interval_weeks: u8,
    anchor_date: Option<NaiveDate>,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, AppError> {
    validate_weekday(weekday)?;
    if interval_weeks != 1 && interval_weeks != 2 {
        return Err(AppError::InvalidInput(format!(
            "周期は1（毎週）か2（隔週）で指定してください: {}",
            interval_weeks
        )));
    }
    let jst = jst_offset();
    let today = now.with_timezone(&jst).date_naive();
    let mut day = if interval_weeks == 1 {
        let delta = (weekday as i64 - today.weekday().num_days_from_sunday() as i64 + 7) % 7;
        today + Duration::days(delta)
    } else {
        let anchor = anchor_date.ok_or_else(|| {
            AppError::InvalidInput("隔週は基準日（anchorDate）が必須です".to_string())
        })?;
        if anchor.weekday().num_days_from_sunday() as u8 != weekday {
            return Err(AppError::InvalidInput(format!(
                "基準日{}は{}曜ではありません。指定曜日と一致する日を選んでください。",
                anchor.format("%Y-%m-%d"),
                weekday_ja(weekday)
            )));
        }
        let diff = today.signed_duration_since(anchor).num_days();
        let step = if diff <= 0 { 0 } else { (diff + 13) / 14 };
        anchor + Duration::days(step * 14)
    };
    loop {
        let occ_jst = day
            .and_time(time)
            .and_local_timezone(jst)
            .single()
            .expect("JST datetime should always be valid (no DST)");
        let occ_utc = occ_jst.with_timezone(&Utc);
        if occ_utc + Duration::minutes(SCHEDULE_GRACE_MINUTES) > now {
            return Ok(occ_utc);
        }
        // 当日中の再開始は当日の回に留める（トラブル遅延の取りこぼし対策）。
        if day == today && now < occ_utc + Duration::hours(SAME_DAY_RESTART_HOURS) {
            return Ok(occ_utc);
        }
        day += Duration::days(if interval_weeks == 1 { 7 } else { 14 });
    }
}

/// 候補由来の対象カレンダーID。IDは保持するが診断用の証跡にのみ使い、自動選択には使わない。
/// once/weekly/biweekly共通。稼働中の証跡取得後に自動選択への利用を再検討する。
/// 将来の証跡検証に備えて関数形状と `sourceEventId` 引数を残す。
pub fn target_calendar_entry_id_for_schedule(
    _schedule: &EventSchedule,
    _source_event_id: Option<&str>,
) -> Option<String> {
    None
}

pub fn parse_daily_time_str(time: &str) -> Result<NaiveTime, AppError> {
    let trimmed = time.trim();
    NaiveTime::parse_from_str(trimmed, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(trimmed, "%H:%M"))
        .map_err(|_| {
            AppError::InvalidInput(format!(
                "時刻の形式が正しくありません（HH:MM または HH:MM:SS）: {}",
                time
            ))
        })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StopReason {
    /// 本人の新鮮な現在の `location` が対象と一致した（唯一の成功）。
    JoinConfirmed,
    /// 起動要求の送出済みだが確認不能（WS切断・確認期限到達）。
    LaunchSentUnconfirmed,
    /// 候補が複数あり自動起動しなかった。`preferredInstanceName` が必要。
    AmbiguousTarget(String),
    /// 監視窓の終了時点で対象が満員のままだった。
    InstanceFull,
    AuthInvalid,
    RateLimit,
    ApiError(String),
    LaunchFailed(String),
    UnrecoverableFailure(String),
    ManualStop,
    MonitorExpired,
    PreferredNameUnverifiable,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::JoinConfirmed => write!(f, "入室を確認しました"),
            StopReason::LaunchSentUnconfirmed => {
                write!(f, "起動要求は送信しましたが、入室を確認できませんでした")
            }
            StopReason::AmbiguousTarget(msg) => write!(f, "対象が曖昧です: {}", msg),
            StopReason::InstanceFull => write!(f, "対象が満員です"),
            StopReason::AuthInvalid => write!(f, "VRChatの認証が無効です"),
            StopReason::RateLimit => write!(f, "VRChat APIの制限に達しました"),
            StopReason::ApiError(msg) => write!(f, "VRChat APIでエラーが発生しました: {}", msg),
            StopReason::LaunchFailed(msg) => write!(f, "VRChatの起動に失敗しました: {}", msg),
            StopReason::UnrecoverableFailure(msg) => {
                write!(f, "監視を継続できないエラーが発生しました: {}", msg)
            }
            StopReason::ManualStop => write!(f, "停止操作で終了しました"),
            StopReason::MonitorExpired => write!(f, "監視終了時刻を過ぎました"),
            StopReason::PreferredNameUnverifiable => {
                write!(f, "優先インスタンス名を確認できませんでした")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MonitorState {
    Idle,
    /// 監視窓（T-3分）より前の待機。
    WaitingForWindow,
    Watching,
    /// 複数候補の非終端選択待ち。`snapshot.candidates` に一覧を返す。
    /// 選択IPCで実行内に固定して再開する。終端への遷移には使わない。
    AwaitingInstanceSelection,
    CandidateDetected,
    LaunchDispatchRequested,
    /// 起動要求の送出後の有限確認（移動中・キュー・未確認を含む）。
    JoinPendingObservation,
    /// 移動中を検出（成功ではない）。
    Travelling,
    /// キュー待機を検出（入室ではない）。
    QueueWaiting,
    Stopped(StopReason),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum WebsocketConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub location: String,
    pub instance_id: String,
    pub world_id: String,
    pub display_name: Option<String>,
    pub member_count: i32,
    pub has_capacity_for_you: Option<bool>,
    pub is_full: Option<bool>,
    pub queue_enabled: Option<bool>,
    pub queue_size: Option<i32>,
    /// group-specific instances由来のcalendarEntryId。欠落・nullあり得る。
    /// IDは保持するが診断用evidenceにのみ使い、自動選択には使わない。
    /// live evidence取得後に自動選択への利用を再検討する。
    pub calendar_entry_id: Option<String>,
}

impl Candidate {
    /// 直接入室できるか。hasCapacityForYou==falseはfull表示に関わらず不可とし、
    /// 両方unknownをjoinable扱いしない。queueは含めない（別途can_queue）。
    pub fn is_joinable(&self) -> bool {
        if self.has_capacity_for_you == Some(false) {
            return false;
        }
        if self.has_capacity_for_you == Some(true) {
            return true;
        }
        self.is_full == Some(false)
    }

    /// キュー参加できるか（満員時の別経路）。通常空きとは別に表現する。
    pub fn can_queue(&self) -> bool {
        self.queue_enabled == Some(true) && self.is_full_now() && !self.is_joinable()
    }

    /// 自動対象にできるか（直接入室またはキュー経路）。
    pub fn is_actionable(&self) -> bool {
        self.is_joinable() || self.can_queue()
    }

    /// 満員が確定しているか。
    pub fn is_full_now(&self) -> bool {
        self.is_full == Some(true) || self.has_capacity_for_you == Some(false)
    }

    pub fn matches_preferred_name(&self, preferred: &str) -> bool {
        if let Some(display_name) = &self.display_name {
            normalize_name_for_match(display_name).contains(&normalize_name_for_match(preferred))
        } else {
            false
        }
    }
}

/// 候補選択の結果。監視ループと手動テストで共通利用し、
/// 曖昧な対象への自動起動をどちらの経路でも起こさない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateSelection {
    Target(Candidate),
    /// 対象なし。待機を継続する。
    Waiting(String),
    /// 複数候補。自動起動せず条件指定を促す。
    Ambiguous(String),
    /// 表示名がなく名前条件を検証できない。
    Unverifiable(String),
}

impl CandidateSelection {
    pub fn target(self) -> Option<Candidate> {
        match self {
            CandidateSelection::Target(candidate) => Some(candidate),
            _ => None,
        }
    }

    /// 手動テスト用の状態区分。
    pub fn status_key(&self) -> &'static str {
        match self {
            CandidateSelection::Target(_) => "ready",
            CandidateSelection::Waiting(_) => "waiting",
            CandidateSelection::Ambiguous(_) => "ambiguous",
            CandidateSelection::Unverifiable(_) => "unverifiable",
        }
    }

    pub fn message(&self) -> String {
        match self {
            CandidateSelection::Target(candidate) => format!(
                "参加対象: {}",
                candidate
                    .display_name
                    .clone()
                    .unwrap_or(candidate.location.clone())
            ),
            CandidateSelection::Waiting(msg)
            | CandidateSelection::Ambiguous(msg)
            | CandidateSelection::Unverifiable(msg) => msg.clone(),
        }
    }
}

/// 監視・手動テスト共通の選択規則。
/// 優先: preferred名一意一致 > actionable1件自動 >
/// 複数=Ambiguous（非終端・UI選択待ち） > 0件=Waiting（継続）。
/// calendarEntryIdは診断用evidenceにのみ使い、自動選択には使わない。
/// live evidence取得後に自動選択への利用を再検討する。
/// 単一会場でもカレンダーイベントとの同一性は保証しない（UIで明示）。
pub fn select_candidate(
    _target_calendar_entry_id: Option<&str>,
    preferred: Option<&str>,
    candidates: &[Candidate],
) -> CandidateSelection {
    if candidates.is_empty() {
        return CandidateSelection::Waiting(
            "グループインスタンスが見つかりません。監視を継続します。".to_string(),
        );
    }

    if let Some(name) = preferred {
        if candidates.iter().all(|c| c.display_name.is_none()) {
            return CandidateSelection::Unverifiable(
                "インスタンス名を取得できないため、名前条件を確認できません。名前条件を外すか、時間を置いて再試行してください。"
                    .to_string(),
            );
        }
        let matched: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| c.matches_preferred_name(name))
            .collect();
        match matched.len() {
            0 => CandidateSelection::Waiting(format!(
                "「{}」に一致するインスタンスがありません。監視を継続します。",
                name
            )),
            1 => {
                let candidate = matched[0].clone();
                if candidate.is_actionable() {
                    CandidateSelection::Target(candidate)
                } else {
                    CandidateSelection::Waiting(format!(
                        "「{}」は現在参加できないため待機します。",
                        candidate.display_name.unwrap_or(candidate.location)
                    ))
                }
            }
            _ => CandidateSelection::Ambiguous(format!(
                "「{}」に一致する候補が{}件あるため自動参加しません。名前条件を絞ってください。",
                name,
                matched.len()
            )),
        }
    } else {
        let actionable: Vec<&Candidate> = candidates.iter().filter(|c| c.is_actionable()).collect();
        match actionable.len() {
            0 => CandidateSelection::Waiting(
                "参加可能なインスタンスがありません。満員の可能性があります。監視を継続します。"
                    .to_string(),
            ),
            1 => CandidateSelection::Target(actionable[0].clone()),
            _ => CandidateSelection::Ambiguous(format!(
                "参加可能な候補が{}件あります。参加する会場を選択してください。",
                actionable.len()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_candidate(
        display_name: Option<String>,
        has_capacity: Option<bool>,
        is_full: Option<bool>,
    ) -> Candidate {
        Candidate {
            location: "test".to_string(),
            instance_id: "test~id".to_string(),
            world_id: "wrld_123".to_string(),
            display_name,
            member_count: 10,
            has_capacity_for_you: has_capacity,
            is_full,
            queue_enabled: None,
            queue_size: None,
            calendar_entry_id: None,
        }
    }

    fn create_entry_candidate(location: &str, entry_id: Option<&str>) -> Candidate {
        Candidate {
            location: location.to_string(),
            instance_id: format!("{}~id", location),
            world_id: "wrld_123".to_string(),
            display_name: Some(location.to_string()),
            member_count: 10,
            has_capacity_for_you: Some(true),
            is_full: Some(false),
            queue_enabled: Some(false),
            queue_size: Some(0),
            calendar_entry_id: entry_id.map(str::to_string),
        }
    }

    #[test]
    fn test_matches_preferred_name_table() {
        let cases: Vec<(Option<&str>, &str, bool)> = vec![
            (Some("Main Event"), "main", true),
            (Some("Japanese Event"), "JAPANESE", true),
            (Some("Night Session"), "day", false),
            (None, "test", false),
            (Some("HookahHolic_第1インスタンス"), "第1", true),
            (Some("HookahHolic_第１インスタンス"), "第1", true),
            (Some("HookahHolic_第1インスタンス"), "第１", true),
            (Some("Ｍａｉｎ Event"), "main", true),
            (Some("ホロライブ"), "ﾎﾛﾗｲﾌﾞ", true),
        ];
        for (display_name, preferred, expected) in cases {
            let candidate = create_test_candidate(display_name.map(str::to_string), None, None);
            assert_eq!(
                candidate.matches_preferred_name(preferred),
                expected,
                "display={:?} preferred={:?}",
                display_name,
                preferred
            );
        }
    }

    #[test]
    fn test_select_without_preference_table() {
        let joinable = || create_test_candidate(Some("B".to_string()), Some(true), Some(false));
        let full =
            |name: &str| create_test_candidate(Some(name.to_string()), Some(false), Some(true));
        let cases: Vec<(Vec<Candidate>, &'static str)> = vec![
            (vec![full("A"), joinable()], "ready"),
            (
                vec![
                    create_test_candidate(Some("A".to_string()), Some(true), Some(false)),
                    joinable(),
                ],
                "ambiguous",
            ),
            (vec![full("A"), full("B")], "waiting"),
        ];
        for (candidates, expected) in cases {
            let selection = select_candidate(None, None, &candidates);
            assert_eq!(
                selection.status_key(),
                expected,
                "candidates={:?}",
                candidates
                    .iter()
                    .map(|c| &c.display_name)
                    .collect::<Vec<_>>()
            );
            assert_eq!(selection.target().is_some(), expected == "ready");
        }
    }

    #[test]
    fn test_select_preferred_table() {
        let full_main = || create_test_candidate(Some("Main".to_string()), Some(false), Some(true));
        let joinable_main =
            |name: &str| create_test_candidate(Some(name.to_string()), Some(true), Some(false));
        // (candidates, preferred, expected status)
        let cases: Vec<(Vec<Candidate>, &'static str, &'static str)> = vec![
            (vec![full_main()], "main", "waiting"),
            (
                vec![joinable_main("Main A"), joinable_main("Main B")],
                "main",
                "ambiguous",
            ),
            (
                vec![create_test_candidate(None, Some(true), Some(false))],
                "main",
                "unverifiable",
            ),
        ];
        for (candidates, preferred, expected) in cases {
            let selection = select_candidate(None, Some(preferred), &candidates);
            assert_eq!(
                selection.status_key(),
                expected,
                "preferred={:?} candidates={:?}",
                preferred,
                candidates
                    .iter()
                    .map(|c| &c.display_name)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn test_event_schedule_ipc_contract_shapes() {
        // IPC契約 once / weekly / biweekly を1本で確認する。
        let once = EventSchedule::Once {
            starts_at: DateTime::parse_from_rfc3339("2026-09-07T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let json = serde_json::to_value(&once).unwrap();
        assert_eq!(json["kind"], "once");
        assert_eq!(json["startsAt"], "2026-09-07T12:00:00Z");
        assert_eq!(serde_json::from_value::<EventSchedule>(json).unwrap(), once);
        let weekly = EventSchedule::Weekly {
            weekday: 0,
            time: "21:30".to_string(),
        };
        let json = serde_json::to_value(&weekly).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "weekly", "weekday": 0, "time": "21:30"})
        );
        assert_eq!(
            serde_json::from_value::<EventSchedule>(json).unwrap(),
            weekly
        );
        let biweekly = EventSchedule::Biweekly {
            weekday: 3,
            time: "07:05:09".to_string(),
            anchor_date: "2026-09-09".to_string(),
        };
        let json = serde_json::to_value(&biweekly).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "biweekly", "weekday": 3, "time": "07:05:09", "anchorDate": "2026-09-09"})
        );
        assert_eq!(
            serde_json::from_value::<EventSchedule>(json).unwrap(),
            biweekly
        );
    }

    #[test]
    fn test_parse_daily_time_accepts_hh_mm_and_hh_mm_ss() {
        assert!(parse_daily_time_str("21:30").is_ok());
        assert!(parse_daily_time_str("07:05:09").is_ok());
        assert!(parse_daily_time_str("25:00").is_err());
        assert!(parse_daily_time_str("event").is_err());
    }

    /// 削除した is_joinable 単体5本 + queue分離の保証をこの表で維持する。
    /// (hasCapacity, full, queue) と (joinable, full_now, can_queue, actionable) の組。
    type CapacityCase = (
        Option<bool>,
        Option<bool>,
        Option<bool>,
        (bool, bool, bool, bool),
    );
    #[test]
    fn test_capacity_combinations_have_no_joinable_full_contradiction() {
        let cases: Vec<CapacityCase> = vec![
            (None, None, None, (false, false, false, false)),
            (Some(true), None, None, (true, false, false, true)),
            (Some(true), Some(false), None, (true, false, false, true)),
            (Some(true), Some(true), None, (true, true, false, true)),
            (Some(false), Some(false), None, (false, true, false, false)),
            (Some(false), Some(true), None, (false, true, false, false)),
            (
                Some(false),
                Some(false),
                Some(true),
                (false, true, true, true),
            ),
            (
                Some(false),
                Some(true),
                Some(true),
                (false, true, true, true),
            ),
            (None, Some(false), None, (true, false, false, true)),
            (None, Some(false), Some(true), (true, false, false, true)),
            (None, Some(true), None, (false, true, false, false)),
            (None, Some(true), Some(true), (false, true, true, true)),
        ];
        for (capacity, full, queue, expected) in cases {
            let mut candidate = create_test_candidate(None, capacity, full);
            candidate.queue_enabled = queue;
            assert_eq!(
                (
                    candidate.is_joinable(),
                    candidate.is_full_now(),
                    candidate.can_queue(),
                    candidate.is_actionable()
                ),
                expected,
                "capacity={:?} full={:?} queue={:?}",
                capacity,
                full,
                queue
            );
            // joinableとfullの同時trueを禁止する（hasCapacity==trueの予約枠を除く）。
            if capacity != Some(true) {
                assert!(
                    !(candidate.is_joinable() && candidate.is_full_now()),
                    "contradiction at capacity={:?} full={:?}",
                    capacity,
                    full
                );
            }
        }
    }

    #[test]
    fn test_select_calendar_entry_is_not_prioritized() {
        // ID一致でも自動優先しない。actionable複数はchooserへ。
        // calendarEntryIdで勝手に自動優先しない安全不変条件の代表ケース。
        let candidates = vec![
            create_entry_candidate("loc-a", Some("cal_1")),
            create_entry_candidate("loc-b", Some("cal_2")),
        ];
        let selection = select_candidate(Some("cal_1"), None, &candidates);
        assert!(matches!(selection, CandidateSelection::Ambiguous(_)));
        // sourceEventIdがあっても全schedule種別で自動優先しない。
        let once = EventSchedule::Once {
            starts_at: DateTime::parse_from_rfc3339("2026-09-07T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        assert_eq!(
            target_calendar_entry_id_for_schedule(&once, Some("cal_1")),
            None
        );
        let weekly = EventSchedule::Weekly {
            weekday: 0,
            time: "21:30".to_string(),
        };
        assert_eq!(
            target_calendar_entry_id_for_schedule(&weekly, Some("cal_1")),
            None
        );
        let biweekly = EventSchedule::Biweekly {
            weekday: 0,
            time: "21:30".to_string(),
            anchor_date: "2026-09-06".to_string(),
        };
        assert_eq!(
            target_calendar_entry_id_for_schedule(&biweekly, Some("cal_1")),
            None
        );
        let target = target_calendar_entry_id_for_schedule(&once, Some("cal_1"));
        let selection = select_candidate(target.as_deref(), None, &candidates);
        assert!(matches!(selection, CandidateSelection::Ambiguous(_)));
    }

    #[test]
    fn test_select_calendar_entry_falls_back_when_unknown() {
        let candidates = vec![
            create_entry_candidate("loc-a", None),
            create_entry_candidate("loc-b", None),
        ];
        // ID不明時は件数規則へ落とす。2件は曖昧（強制選択しない）。
        let selection = select_candidate(Some("cal_9"), None, &candidates);
        assert!(matches!(selection, CandidateSelection::Ambiguous(_)));
        // preferredがあれば名前規則へ落とす。
        let selection = select_candidate(Some("cal_9"), Some("loc-b"), &candidates);
        match selection {
            CandidateSelection::Target(candidate) => assert_eq!(candidate.location, "loc-b"),
            other => panic!("preferred fallback must target, got {:?}", other),
        }
    }

    // test_select_calendar_entry_unique_but_full_waits /
    // test_select_calendar_entry_duplicate_is_not_forced /
    // test_target_entry_gate_always_none は上記代表ケースと候補選択表へ吸収したため削除。

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
            .and_utc()
    }

    fn iso(dt: DateTime<Utc>) -> String {
        dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    #[test]
    fn test_weekly_next_occurrence_and_grace_boundary() {
        // 2026-09-06は日曜。21:30 JST == 12:30 UTC。
        let time = parse_daily_time_str("21:30").unwrap();
        assert_eq!(
            iso(
                resolve_weekly_occurrence_utc(0, time, 1, None, utc(2026, 9, 6, 11, 0, 0)).unwrap()
            ),
            "2026-09-06T12:30:00Z"
        );
        // 猶予2分以内は当回。
        assert_eq!(
            iso(
                resolve_weekly_occurrence_utc(0, time, 1, None, utc(2026, 9, 6, 12, 31, 0))
                    .unwrap()
            ),
            "2026-09-06T12:30:00Z"
        );
        // 猶予超過でも6時間以内・同日なら当日（当日再開の取りこぼし対策）。
        assert_eq!(
            iso(
                resolve_weekly_occurrence_utc(0, time, 1, None, utc(2026, 9, 6, 12, 33, 0))
                    .unwrap()
            ),
            "2026-09-06T12:30:00Z"
        );
        // 6時間超過は翌週。
        assert_eq!(
            iso(
                resolve_weekly_occurrence_utc(0, time, 1, None, utc(2026, 9, 6, 18, 31, 0))
                    .unwrap()
            ),
            "2026-09-13T12:30:00Z"
        );
    }

    #[test]
    fn test_weekly_same_day_restart_stays_today() {
        // 日曜00:30 JSTの回。当日23:00 JST（14:00 UTC）の再開は6時間超過のため翌週。
        let early = parse_daily_time_str("00:30").unwrap();
        assert_eq!(
            iso(
                resolve_weekly_occurrence_utc(0, early, 1, None, utc(2026, 9, 6, 14, 0, 0))
                    .unwrap()
            ),
            "2026-09-12T15:30:00Z"
        );
        // 同じ回でも2時間後（02:30 JST）の再開は当日。
        assert_eq!(
            iso(
                resolve_weekly_occurrence_utc(0, early, 1, None, utc(2026, 9, 5, 17, 30, 0))
                    .unwrap()
            ),
            "2026-09-05T15:30:00Z"
        );
    }

    #[test]
    fn test_weekly_uses_jst_calendar_across_utc_midnight() {
        // 2026-09-05 15:30 UTC == 09-06 00:30 JST（日曜）。UTC日付は土曜だがJSTでは日曜。
        let time = parse_daily_time_str("21:30").unwrap();
        assert_eq!(
            iso(
                resolve_weekly_occurrence_utc(0, time, 1, None, utc(2026, 9, 5, 15, 30, 0))
                    .unwrap()
            ),
            "2026-09-06T12:30:00Z"
        );
    }

    #[test]
    fn test_biweekly_anchor_boundaries() {
        // anchor 2026-09-06（日曜）。該当週は09-06、次は09-20。
        let time = parse_daily_time_str("21:30").unwrap();
        let anchor = NaiveDate::from_ymd_opt(2026, 9, 6).unwrap();
        assert_eq!(
            iso(
                resolve_weekly_occurrence_utc(0, time, 2, Some(anchor), utc(2026, 9, 6, 11, 0, 0))
                    .unwrap()
            ),
            "2026-09-06T12:30:00Z"
        );
        // 非該当週（09-13）は飛ばして09-20。
        assert_eq!(
            iso(resolve_weekly_occurrence_utc(
                0,
                time,
                2,
                Some(anchor),
                utc(2026, 9, 10, 11, 0, 0)
            )
            .unwrap()),
            "2026-09-20T12:30:00Z"
        );
        // 該当週の猶予内・6時間以内は当回、6時間超過は+14日。
        assert_eq!(
            iso(resolve_weekly_occurrence_utc(
                0,
                time,
                2,
                Some(anchor),
                utc(2026, 9, 20, 12, 31, 0)
            )
            .unwrap()),
            "2026-09-20T12:30:00Z"
        );
        assert_eq!(
            iso(resolve_weekly_occurrence_utc(
                0,
                time,
                2,
                Some(anchor),
                utc(2026, 9, 20, 12, 33, 0)
            )
            .unwrap()),
            "2026-09-20T12:30:00Z"
        );
        assert_eq!(
            iso(resolve_weekly_occurrence_utc(
                0,
                time,
                2,
                Some(anchor),
                utc(2026, 9, 20, 18, 31, 0)
            )
            .unwrap()),
            "2026-10-04T12:30:00Z"
        );
        // 未来anchorはanchor自体が初回。
        let future = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        assert_eq!(
            iso(
                resolve_weekly_occurrence_utc(0, time, 2, Some(future), utc(2026, 9, 6, 11, 0, 0))
                    .unwrap()
            ),
            "2026-10-04T12:30:00Z"
        );
    }

    #[test]
    fn test_weekly_rejects_bad_input_without_guessing() {
        let time = parse_daily_time_str("21:30").unwrap();
        assert!(
            resolve_weekly_occurrence_utc(7, time, 1, None, utc(2026, 9, 6, 11, 0, 0)).is_err()
        );
        assert!(
            resolve_weekly_occurrence_utc(0, time, 3, None, utc(2026, 9, 6, 11, 0, 0)).is_err()
        );
        assert!(
            resolve_weekly_occurrence_utc(0, time, 2, None, utc(2026, 9, 6, 11, 0, 0)).is_err()
        );
        // anchorが月曜なのに日曜指定は推測せず却下。
        let monday = NaiveDate::from_ymd_opt(2026, 9, 7).unwrap();
        assert!(
            resolve_weekly_occurrence_utc(0, time, 2, Some(monday), utc(2026, 9, 6, 11, 0, 0))
                .is_err()
        );
    }

    // test_event_schedule_weekly_biweekly_serde_shapes は上記IPC契約へ統合したため削除。

    #[test]
    fn test_snapshot_carries_candidates_and_generation() {
        let snapshot = MonitorSnapshot {
            state: MonitorState::AwaitingInstanceSelection,
            stop_reason: None,
            config: None,
            pending_target: None,
            candidates: vec![create_entry_candidate("loc-a", Some("cal_1"))],
            generation: 7,
        };
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["state"], "AwaitingInstanceSelection");
        assert_eq!(json["generation"], 7);
        assert_eq!(json["candidates"].as_array().unwrap().len(), 1);
        assert_eq!(json["candidates"][0]["calendarEntryId"], "cal_1");
    }

    // test_forbidden_is_not_auth は型の自明な確認のため削除。
    // 403境界の実保証は infra::http_client::forbidden_maps_to_forbidden_not_auth が担う。
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorConfig {
    pub preset_id: String,
    pub group_id: String,
    pub event_start: chrono::DateTime<chrono::Utc>,
    pub monitor_start: chrono::DateTime<chrono::Utc>,
    pub monitor_end: chrono::DateTime<chrono::Utc>,
    pub preferred_instance_name: Option<String>,
}

/// 起動要求の送出後の確認対象。UIのカウントダウン表示に使う。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTarget {
    pub location: String,
    pub display_name: Option<String>,
    pub dispatched_at: chrono::DateTime<chrono::Utc>,
    pub confirm_until: chrono::DateTime<chrono::Utc>,
}

/// `get_monitor_status` の応答。単一ロックで取得した整合スナップショット。
/// `candidates` は `AwaitingInstanceSelection` 中の現在候補一覧（それ以外は空）。
/// `generation` は現在の実行の世代。選択IPCは一致を要求し、古いものを拒否する。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    pub state: MonitorState,
    pub stop_reason: Option<StopReason>,
    pub config: Option<MonitorConfig>,
    pub pending_target: Option<PendingTarget>,
    pub candidates: Vec<Candidate>,
    pub generation: u64,
}

impl Default for MonitorSnapshot {
    fn default() -> Self {
        Self {
            state: MonitorState::Idle,
            stop_reason: None,
            config: None,
            pending_target: None,
            candidates: Vec::new(),
            generation: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiBudgetEstimate {
    pub profile_name: String,
    pub estimated_requests: u32,
    pub warn_threshold: u32,
    pub block_threshold: u32,
    pub should_warn: bool,
    pub should_block: bool,
}
