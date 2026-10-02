pub mod join_service;
pub mod monitor_service;
pub mod sdk_auth_client;
pub mod vrchat_api;
pub mod websocket_service;

pub use join_service::JoinService;
pub use monitor_service::MonitorService;
pub use sdk_auth_client::{SdkAuthClient, SdkLoginResult};
pub use vrchat_api::{
    CalendarEvent, DetailedInstance, UserGroupInstancesResponse, VrchatApiService,
};
pub use websocket_service::WebsocketService;

pub use crate::infra::http_client::APP_USER_AGENT;
use chrono::{DateTime, Duration, Utc};

/// 監視窓: イベント開始の3分前〜2分後。見積とスケジューラの共通根拠。
pub const MONITOR_LEAD_MINUTES: i64 = 3;
pub const MONITOR_TRAIL_MINUTES: i64 = 2;
/// 基本poll間隔。イベント30秒より前は15秒、30秒以内は5秒。
pub const POLL_INTERVAL_RELAXED_SECS: u64 = 15;
pub const POLL_INTERVAL_FAST_SECS: u64 = 5;
pub const FAST_PHASE_THRESHOLD_SECS: i64 = 30;
/// launch送信後の有限確認期限（秒）。WSの新鮮な本人location一致だけが成功。
pub const CONFIRM_TIMEOUT_SECS: u64 = 90;
/// 一過性失敗（ネットワーク/5xx）がこの回数連続したら停止する。
pub const MAX_CONSECUTIVE_TRANSIENT_ERRORS: u32 = 5;
/// rate-limit検知がこの回数連続したら停止する。
pub const MAX_CONSECUTIVE_RATE_LIMITS: u32 = 3;
/// Retry-Afterヘッダ無し時の指数backoff初項（秒）。明示値には上限を付けない。
pub const RATE_LIMIT_BACKOFF_BASE_SECS: u64 = 10;
/// 同backoffの上限（秒）。明示Retry-Afterには適用しない。
pub const RATE_LIMIT_BACKOFF_CAP_SECS: u64 = 60;
/// 終了間際に候補活動がある場合の延長（分）。複数インスタンスのずれ対策。
pub const MONITOR_GRACE_MINUTES: i64 = 5;
/// 延長の上限回数。無制限の延長でAPIを消費しない。
pub const MAX_MONITOR_EXTENSIONS: u32 = 2;

/// 解決済みの監視窓。予定解決の正本はこの関数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorWindow {
    pub event_start: DateTime<Utc>,
    pub monitor_start: DateTime<Utc>,
    pub monitor_end: DateTime<Utc>,
}

/// scheduleを監視窓へ解決する。単発の期限切れは翌日へ繰り越さずNone。
/// weekly/biweeklyは次回発生へ解決する（不正入力のみNone）。
/// 当日再開で発生時刻を過ぎている場合は、再開時点から1猶予分を確保する。
pub fn resolve_monitor_window(
    schedule: &crate::domain::models::EventSchedule,
    now: DateTime<Utc>,
) -> Option<MonitorWindow> {
    let event_start = match schedule {
        crate::domain::models::EventSchedule::Once { starts_at } => {
            if *starts_at + Duration::minutes(MONITOR_TRAIL_MINUTES) <= now {
                return None;
            }
            *starts_at
        }
        crate::domain::models::EventSchedule::Weekly { weekday, time } => {
            let naive = crate::domain::models::parse_daily_time_str(time).ok()?;
            crate::domain::models::resolve_weekly_occurrence_utc(*weekday, naive, 1, None, now)
                .ok()?
        }
        crate::domain::models::EventSchedule::Biweekly {
            weekday,
            time,
            anchor_date,
        } => {
            let naive = crate::domain::models::parse_daily_time_str(time).ok()?;
            let anchor = crate::domain::models::parse_anchor_date(anchor_date).ok()?;
            crate::domain::models::resolve_weekly_occurrence_utc(
                *weekday,
                naive,
                2,
                Some(anchor),
                now,
            )
            .ok()?
        }
    };
    let late_restart = event_start + Duration::minutes(MONITOR_TRAIL_MINUTES) <= now;
    Some(MonitorWindow {
        event_start,
        monitor_start: event_start - Duration::minutes(MONITOR_LEAD_MINUTES),
        monitor_end: if late_restart {
            now + Duration::minutes(MONITOR_GRACE_MINUTES)
        } else {
            event_start + Duration::minutes(MONITOR_TRAIL_MINUTES)
        },
    })
}

/// 基本poll間隔（秒）。見積とスケジューラで同一関数を使う。
pub fn base_poll_interval_secs(now: DateTime<Utc>, event_start: DateTime<Utc>) -> u64 {
    if (event_start - now).num_seconds() > FAST_PHASE_THRESHOLD_SECS {
        POLL_INTERVAL_RELAXED_SECS
    } else {
        POLL_INTERVAL_FAST_SECS
    }
}

/// 固定時計への同期を避けるjitter（×0.8〜×1.2）。乱数crateなしで時刻ナノ秒由来。
pub fn jittered_poll_interval_secs(base_secs: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let factor = 80 + (nanos % 41);
    (base_secs * factor as u64 / 100).max(2)
}

/// 基本間隔での窓内poll回数。1poll=1GET。見積の根拠（実測はjitterで±20%変動）。
pub fn count_base_polls(
    event_start: DateTime<Utc>,
    monitor_start: DateTime<Utc>,
    monitor_end: DateTime<Utc>,
    now: DateTime<Utc>,
) -> u32 {
    if now >= monitor_end {
        return 0;
    }
    let start = now.max(monitor_start);
    let fast_start = event_start - Duration::seconds(FAST_PHASE_THRESHOLD_SECS);
    let relaxed_secs = (fast_start - start).num_seconds().max(0) as u64;
    let fast_secs = (monitor_end - fast_start.max(start)).num_seconds().max(0) as u64;
    (relaxed_secs / POLL_INTERVAL_RELAXED_SECS + fast_secs / POLL_INTERVAL_FAST_SECS) as u32
}

#[cfg(test)]
mod schedule_tests {
    use super::*;
    use crate::domain::models::EventSchedule;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
            .and_utc()
    }

    #[test]
    fn test_resolve_once_future_returns_window() {
        let now = utc(2026, 9, 6, 11, 55, 0);
        let schedule = EventSchedule::Once {
            starts_at: utc(2026, 9, 6, 12, 0, 0),
        };
        let window = resolve_monitor_window(&schedule, now).unwrap();
        assert_eq!(window.event_start, utc(2026, 9, 6, 12, 0, 0));
        assert_eq!(window.monitor_start, utc(2026, 9, 6, 11, 57, 0));
        assert_eq!(window.monitor_end, utc(2026, 9, 6, 12, 2, 0));
    }

    #[test]
    fn test_resolve_once_expired_returns_none() {
        // T+2分の窓を過ぎた単発は翌日へ繰り越さない。
        let now = utc(2026, 9, 6, 12, 3, 0);
        let schedule = EventSchedule::Once {
            starts_at: utc(2026, 9, 6, 12, 0, 0),
        };
        assert!(resolve_monitor_window(&schedule, now).is_none());
    }

    #[test]
    fn test_resolve_weekly_late_restart_keeps_today_with_fresh_end() {
        // 日曜22:00 JST（13:00 UTC）の回。22:05 JSTの再開は当日を対象にする。
        let schedule = EventSchedule::Weekly {
            weekday: 0,
            time: "22:00".to_string(),
        };
        let now = utc(2026, 9, 6, 13, 5, 0);
        let window = resolve_monitor_window(&schedule, now).unwrap();
        assert_eq!(window.event_start, utc(2026, 9, 6, 13, 0, 0));
        assert_eq!(
            window.monitor_end,
            now + Duration::minutes(MONITOR_GRACE_MINUTES)
        );
    }

    #[test]
    fn test_base_poll_interval_boundary() {
        let event = utc(2026, 9, 6, 12, 0, 0);
        assert_eq!(
            base_poll_interval_secs(utc(2026, 9, 6, 11, 59, 29), event),
            POLL_INTERVAL_RELAXED_SECS
        );
        assert_eq!(
            base_poll_interval_secs(utc(2026, 9, 6, 11, 59, 30), event),
            POLL_INTERVAL_FAST_SECS
        );
    }

    // test_jitter_stays_within_band は時刻由来の非決定値を回して実装定数を
    // 再確認するだけのため削除。

    #[test]
    fn test_count_base_polls_full_window() {
        let event = utc(2026, 9, 6, 12, 0, 0);
        let window = MonitorWindow {
            event_start: event,
            monitor_start: event - Duration::minutes(3),
            monitor_end: event + Duration::minutes(2),
        };
        // 緩和150秒/15 + 高速150秒/5 = 10 + 30 = 40poll（=40GET）。
        assert_eq!(
            count_base_polls(
                event,
                window.monitor_start,
                window.monitor_end,
                window.monitor_start
            ),
            40
        );
    }

    #[test]
    fn test_resolve_weekly_returns_window() {
        // JST 21:25（日曜）→ 当日21:30 JST = 12:30 UTC。
        let now = utc(2026, 9, 6, 12, 25, 0);
        let schedule = EventSchedule::Weekly {
            weekday: 0,
            time: "21:30".to_string(),
        };
        let window = resolve_monitor_window(&schedule, now).unwrap();
        assert_eq!(window.event_start, utc(2026, 9, 6, 12, 30, 0));
        assert_eq!(window.monitor_start, utc(2026, 9, 6, 12, 27, 0));
        assert_eq!(window.monitor_end, utc(2026, 9, 6, 12, 32, 0));
    }

    #[test]
    fn test_resolve_biweekly_skips_off_week() {
        // anchor週（09-06）→ 当日。非該当週（09-10時点）は09-20。
        let on_week = EventSchedule::Biweekly {
            weekday: 0,
            time: "21:30".to_string(),
            anchor_date: "2026-09-06".to_string(),
        };
        let window = resolve_monitor_window(&on_week, utc(2026, 9, 6, 12, 25, 0)).unwrap();
        assert_eq!(window.event_start, utc(2026, 9, 6, 12, 30, 0));
        let window = resolve_monitor_window(&on_week, utc(2026, 9, 10, 11, 0, 0)).unwrap();
        assert_eq!(window.event_start, utc(2026, 9, 20, 12, 30, 0));
    }

    // test_resolve_biweekly_without_anchor_returns_none は domain recurrence 正本と
    // 重複するため削除。不正anchorの拒否は domain が担う。
    // test_resolve_biweekly_skips_off_week は biweekly の wrapper 代表として維持する。
}
