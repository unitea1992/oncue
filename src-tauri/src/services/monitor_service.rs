use crate::domain::models::{
    select_candidate, target_calendar_entry_id_for_schedule, AppError, Candidate,
    CandidateSelection, MonitorConfig, MonitorSnapshot, MonitorState, PendingTarget, StopReason,
};
use crate::logging::{LogEvent, LogLevel, LogWriter};
use crate::services::join_service::JoinService;
use crate::services::vrchat_api::{candidates_from_detailed, VrchatApiService};
use crate::services::websocket_service::QueueKind;
use crate::services::{
    base_poll_interval_secs, jittered_poll_interval_secs, resolve_monitor_window, WebsocketService,
    CONFIRM_TIMEOUT_SECS, MAX_CONSECUTIVE_RATE_LIMITS, MAX_CONSECUTIVE_TRANSIENT_ERRORS,
    MAX_MONITOR_EXTENSIONS, MONITOR_GRACE_MINUTES, RATE_LIMIT_BACKOFF_BASE_SECS,
    RATE_LIMIT_BACKOFF_CAP_SECS,
};
use crate::storage::Preset;
use chrono::{DateTime, Datelike, Duration, FixedOffset, Timelike, Utc};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;

struct Lifecycle {
    generation: u64,
    task: Option<JoinHandle<()>>,
}

/// 実行単位の状態。共有せずspawnへmoveし、旧実行が新実行へ干渉しない。
struct RunContext {
    generation: u64,
    config: MonitorConfig,
    user_id: String,
    debug_mode: bool,
    /// 自動優先しない（常にNone）。ID自体は診断の人間比較用にモデル保持。
    target_calendar_entry_id: Option<String>,
    attempted: HashSet<String>,
    transient_errors: u32,
    rate_limit_hits: u32,
    last_poll_all_full: bool,
    last_poll_unverifiable: bool,
    /// 直近pollで候補が1件でも存在したか。終了間際の延長判定に使う。
    last_poll_had_candidates: bool,
    /// 延長した回数。上限で打ち切る。
    extensions_used: u32,
    notified_selection: bool,
    /// 会場は見えているが待機している理由を最後に通常ログへ出した文。同じ内容は繰り返さない。
    last_wait_note: Option<String>,
}

/// 選択IPCと監視ループで共有するrun-local状態。pinは恒久preferredへ保存しない。
struct RunSelectState {
    generation: u64,
    group_id: String,
    user_id: String,
    pinned_location: Option<String>,
}

enum PollAction {
    Waiting,
    Finished(StopReason),
    Stale,
}

enum RunEnd {
    Finished(StopReason),
    Stale,
}

/// Retry-Afterヘッダ無し時の上限付き指数backoff。明示値には使わない。
fn rate_limit_backoff_secs(hits: u32) -> u64 {
    let shift = hits.saturating_sub(1).min(6);
    RATE_LIMIT_BACKOFF_BASE_SECS
        .saturating_mul(1 << shift)
        .min(RATE_LIMIT_BACKOFF_CAP_SECS)
}

enum RateLimitDecision {
    Wait(u64),
    /// 待機が監視期限を超えるため送信せず終了する。
    StopWindowExceeded,
}

/// 明示Retry-Afterは切り詰めず尊重する。待機が監視期限を超えるなら送信せず終了。
/// 上限付き指数backoffはヘッダ無しの場合のみ。
fn decide_rate_limit_wait(
    retry_after_secs: Option<u64>,
    hits: u32,
    now: DateTime<Utc>,
    monitor_end: DateTime<Utc>,
) -> RateLimitDecision {
    let wait_secs = match retry_after_secs {
        Some(explicit) => explicit,
        None => rate_limit_backoff_secs(hits),
    };
    let wait = Duration::seconds(wait_secs.min(i64::MAX as u64) as i64);
    if now + wait > monitor_end {
        RateLimitDecision::StopWindowExceeded
    } else {
        RateLimitDecision::Wait(wait_secs)
    }
}

/// pinの再検証。候補の現存とjoin/queue可能を確認する。消失・full化は選び直しにする。
fn revalidate_pin(fresh: &[Candidate], location: &str) -> Result<Candidate, AppError> {
    match fresh.iter().find(|c| c.location == location) {
        None => Err(AppError::InvalidInput(
            "選択した候補は既に存在しません。一覧から選び直してください。".to_string(),
        )),
        Some(candidate) if !candidate.is_actionable() => Err(AppError::InvalidInput(
            "選択した候補は現在参加できません。一覧から選び直してください。".to_string(),
        )),
        Some(candidate) => Ok(candidate.clone()),
    }
}

/// 監視窓終了時の理由。名前検証不可・満員を期限末に正しく区別する（N回heuristicなし）。
fn deadline_reason(last_unverifiable: bool, last_all_full: bool) -> StopReason {
    if last_unverifiable {
        StopReason::PreferredNameUnverifiable
    } else if last_all_full {
        StopReason::InstanceFull
    } else {
        StopReason::MonitorExpired
    }
}

/// 終了間際の延長判定。候補活動があり、上限未満のときだけ延長する。
fn should_extend_monitor(extensions_used: u32, had_candidates: bool) -> bool {
    had_candidates && extensions_used < MAX_MONITOR_EXTENSIONS
}

/// デバッグ用のpoll計測1行。追加のAPI取得はしない。通常ログには出さない。
/// rttは送信から受信まで、fetched_ageはサーバー側の取得時刻から受信までの経過時間。
/// fetched_ageがrttより大きければ、律速要因は監視間隔ではなくサーバー側の反映にある。
fn format_poll_debug(
    poll_start: DateTime<Utc>,
    poll_received: DateTime<Utc>,
    fetched_at: Option<DateTime<Utc>>,
    candidates: &[Candidate],
    preferred: Option<&str>,
    selection: &CandidateSelection,
) -> String {
    let rtt_ms = (poll_received - poll_start).num_milliseconds();
    let fetched_age = fetched_at
        .map(|f| format!("{}ms", (poll_received - f).num_milliseconds()))
        .unwrap_or_else(|| "unknown".to_string());
    let actionable = candidates.iter().filter(|c| c.is_actionable()).count();
    let matched = preferred
        .map(|name| {
            candidates
                .iter()
                .filter(|c| c.matches_preferred_name(name))
                .count()
                .to_string()
        })
        .unwrap_or_else(|| "-".to_string());
    format!(
        "poll rtt={}ms fetched_age={} candidates={} actionable={} matched={} selection={}",
        rtt_ms,
        fetched_age,
        candidates.len(),
        actionable,
        matched,
        selection.status_key(),
    )
}

/// 見えている会場の名前と状態の一覧。人数は毎回変わるため含めない（同じ内容の重複出力を避ける）。
fn describe_seen_candidates(candidates: &[Candidate]) -> String {
    candidates
        .iter()
        .map(|c| {
            let state = if c.is_joinable() {
                "空きあり"
            } else if c.can_queue() {
                "満員・キューあり"
            } else if c.is_full_now() {
                "満員"
            } else {
                "状態不明"
            };
            format!(
                "{}（{}）",
                c.display_name.as_deref().unwrap_or("名前なし"),
                state
            )
        })
        .collect::<Vec<_>>()
        .join("、")
}

/// 確認tickの観測結果。判定順序（一致→移動中→queue→切断→継続）を一箇所に固定する。
#[derive(Debug, PartialEq, Eq)]
enum ConfirmSignal {
    Confirmed,
    Travelling,
    Queue(QueueKind),
    Disconnected,
    Pending,
}
#[derive(Clone)]
pub struct MonitorService {
    vrchat_api: Arc<VrchatApiService>,
    join_service: Arc<JoinService>,
    websocket_service: Arc<WebsocketService>,
    log_writer: Arc<Mutex<LogWriter>>,
    lifecycle: Arc<Mutex<Lifecycle>>,
    snapshot: Arc<RwLock<MonitorSnapshot>>,
    run_select: Arc<Mutex<Option<RunSelectState>>>,
}

impl MonitorService {
    pub fn new(
        vrchat_api: Arc<VrchatApiService>,
        join_service: Arc<JoinService>,
        log_writer: Arc<Mutex<LogWriter>>,
        websocket_service: Arc<WebsocketService>,
    ) -> Self {
        Self {
            vrchat_api,
            join_service,
            websocket_service,
            log_writer,
            lifecycle: Arc::new(Mutex::new(Lifecycle {
                generation: 0,
                task: None,
            })),
            snapshot: Arc::new(RwLock::new(MonitorSnapshot::default())),
            run_select: Arc::new(Mutex::new(None)),
        }
    }

    async fn log_info(&self, message: &str) {
        let event = LogEvent::info("monitor", message);
        if let Ok(mut writer) = self.log_writer.try_lock() {
            let _ = writer.log(event);
        }
    }

    async fn log_error(&self, message: &str) {
        let event = LogEvent::error("monitor", message);
        if let Ok(mut writer) = self.log_writer.try_lock() {
            let _ = writer.log(event);
        }
    }

    async fn log_warn(&self, message: &str) {
        let event = LogEvent::warn("monitor", message);
        if let Ok(mut writer) = self.log_writer.try_lock() {
            let _ = writer.log(event);
        }
    }

    async fn log_debug(&self, message: &str) {
        let event = LogEvent {
            timestamp: chrono::Utc::now(),
            level: LogLevel::Debug,
            category: "monitor".to_string(),
            message: message.to_string(),
            details: None,
        };
        if let Ok(mut writer) = self.log_writer.try_lock() {
            let _ = writer.log(event);
        }
    }

    fn format_jst(dt: DateTime<Utc>) -> String {
        let jst =
            FixedOffset::east_opt(9 * 3600).expect("JST offset (9 hours) should always be valid");
        dt.with_timezone(&jst)
            .format("%Y-%m-%d %H:%M:%S JST")
            .to_string()
    }

    fn format_event_time_for_display(dt: DateTime<Utc>, reference_now: DateTime<Utc>) -> String {
        let jst =
            FixedOffset::east_opt(9 * 3600).expect("JST offset (9 hours) should always be valid");
        let dt_jst = dt.with_timezone(&jst);
        let reference_jst = reference_now.with_timezone(&jst);

        let mut result = if dt_jst.year() != reference_jst.year() {
            format!("{}年{}月{}日 ", dt_jst.year(), dt_jst.month(), dt_jst.day())
        } else {
            format!("{}月{}日 ", dt_jst.month(), dt_jst.day())
        };

        let time_part = match (dt_jst.minute(), dt_jst.second()) {
            (0, 0) => format!("{}時", dt_jst.hour()),
            (_, 0) => format!("{}時{:02}分", dt_jst.hour(), dt_jst.minute()),
            _ => format!(
                "{}時{:02}分{:02}秒",
                dt_jst.hour(),
                dt_jst.minute(),
                dt_jst.second()
            ),
        };

        result.push_str(&time_part);
        result
    }

    fn format_event_time_jst(dt: DateTime<Utc>) -> String {
        Self::format_event_time_for_display(dt, Utc::now())
    }

    pub async fn get_state(&self) -> MonitorState {
        self.snapshot.read().await.state.clone()
    }

    pub async fn get_stop_reason(&self) -> Option<StopReason> {
        self.snapshot.read().await.stop_reason.clone()
    }

    pub async fn get_config(&self) -> Option<MonitorConfig> {
        self.snapshot.read().await.config.clone()
    }

    /// 整合したスナップショットを単一ロックで返す。
    pub async fn get_snapshot(&self) -> MonitorSnapshot {
        self.snapshot.read().await.clone()
    }

    pub async fn get_pending_target(&self) -> Option<PendingTarget> {
        self.snapshot.read().await.pending_target.clone()
    }

    async fn is_current(&self, generation: u64) -> bool {
        self.lifecycle.lock().await.generation == generation
    }

    /// 中間phaseの遷移。旧世代・停止済みなら何もしない。
    async fn set_phase(&self, generation: u64, state: MonitorState) -> bool {
        let lifecycle = self.lifecycle.lock().await;
        if lifecycle.generation != generation {
            return false;
        }
        let mut snapshot = self.snapshot.write().await;
        if matches!(snapshot.state, MonitorState::Stopped(_)) {
            return false;
        }
        snapshot.state = state;
        true
    }

    async fn set_pending_target(&self, generation: u64, pending: Option<PendingTarget>) -> bool {
        let lifecycle = self.lifecycle.lock().await;
        if lifecycle.generation != generation {
            return false;
        }
        let mut snapshot = self.snapshot.write().await;
        if matches!(snapshot.state, MonitorState::Stopped(_)) {
            return false;
        }
        snapshot.pending_target = pending;
        true
    }

    /// 現在候補一覧の更新。選択待ちと、会場が見えているのに待機している間だけ中身を持つ。旧世代・停止済みなら何もしない。
    async fn set_candidates(&self, generation: u64, candidates: Vec<Candidate>) -> bool {
        let lifecycle = self.lifecycle.lock().await;
        if lifecycle.generation != generation {
            return false;
        }
        let mut snapshot = self.snapshot.write().await;
        if matches!(snapshot.state, MonitorState::Stopped(_)) {
            return false;
        }
        snapshot.candidates = candidates;
        true
    }

    /// 現在runのpinを読む。世代不一致・未設定はNone。
    async fn pinned_for(&self, generation: u64) -> Option<String> {
        let run_select = self.run_select.lock().await;
        match run_select.as_ref() {
            Some(state) if state.generation == generation => state.pinned_location.clone(),
            _ => None,
        }
    }

    /// pinを外す。世代不一致なら何もしない。
    async fn clear_pinned(&self, generation: u64) {
        let mut run_select = self.run_select.lock().await;
        if let Some(state) = run_select.as_mut() {
            if state.generation == generation {
                state.pinned_location = None;
            }
        }
    }

    /// 実行の確定終了。旧世代・既停止なら副作用なし。
    async fn finish_run(&self, generation: u64, reason: StopReason) {
        let lifecycle = self.lifecycle.lock().await;
        if lifecycle.generation != generation {
            return;
        }
        *self.run_select.lock().await = None;
        let mut snapshot = self.snapshot.write().await;
        if matches!(snapshot.state, MonitorState::Stopped(_)) {
            return;
        }
        snapshot.state = MonitorState::Stopped(reason.clone());
        snapshot.stop_reason = Some(reason);
        snapshot.pending_target = None;
        snapshot.candidates = Vec::new();
    }

    pub async fn start_monitoring(
        &self,
        preset: &Preset,
        user_id: &str,
        debug_mode: bool,
    ) -> Result<(), AppError> {
        let mut lifecycle = self.lifecycle.lock().await;
        if let Some(handle) = lifecycle.task.as_ref() {
            if !handle.is_finished() {
                return Err(AppError::Operation(
                    "監視は既に実行中です。先に停止してください。".to_string(),
                ));
            }
        }

        let now = Utc::now();
        let window = resolve_monitor_window(&preset.schedule, now).ok_or_else(|| {
            AppError::InvalidInput(
                "単発イベントの開始時刻を過ぎています。翌日への繰り越しはしません。新しい日時で作り直してください。"
                    .to_string(),
            )
        })?;

        let config = MonitorConfig {
            preset_id: preset.id.clone(),
            group_id: preset.group_id.clone(),
            event_start: window.event_start,
            monitor_start: window.monitor_start,
            monitor_end: window.monitor_end,
            preferred_instance_name: preset.preferred_instance_name.clone(),
        };
        let target_calendar_entry_id = target_calendar_entry_id_for_schedule(
            &preset.schedule,
            preset.source_event_id.as_deref(),
        );

        lifecycle.generation += 1;
        let generation = lifecycle.generation;
        *self.run_select.lock().await = Some(RunSelectState {
            generation,
            group_id: preset.group_id.clone(),
            user_id: user_id.to_string(),
            pinned_location: None,
        });

        {
            let mut snapshot = self.snapshot.write().await;
            let initial = if now < window.monitor_start {
                MonitorState::WaitingForWindow
            } else {
                MonitorState::Watching
            };
            *snapshot = MonitorSnapshot {
                state: initial,
                stop_reason: None,
                config: Some(config.clone()),
                pending_target: None,
                candidates: Vec::new(),
                generation,
            };
        }

        let runner = self.clone();
        let ctx = RunContext {
            generation,
            config,
            user_id: user_id.to_string(),
            debug_mode,
            target_calendar_entry_id,
            attempted: HashSet::new(),
            transient_errors: 0,
            rate_limit_hits: 0,
            last_poll_all_full: false,
            last_poll_unverifiable: false,
            last_poll_had_candidates: false,
            extensions_used: 0,
            notified_selection: false,
            last_wait_note: None,
        };
        lifecycle.task = Some(tokio::spawn(async move {
            let end = runner.monitor_main(ctx).await;
            match end {
                RunEnd::Finished(reason) => runner.finish_run(generation, reason).await,
                RunEnd::Stale => {}
            }
            let mut lifecycle = runner.lifecycle.lock().await;
            if lifecycle.generation == generation {
                lifecycle.task = None;
            }
        }));

        Ok(())
    }

    pub async fn stop_monitoring(&self) {
        self.stop_with_reason(StopReason::ManualStop).await;
    }

    /// 停止：世代を進め（旧実行の副作用を封じる）、終了を待つ。
    async fn stop_with_reason(&self, reason: StopReason) {
        let handle = {
            let mut lifecycle = self.lifecycle.lock().await;
            lifecycle.generation += 1;
            *self.run_select.lock().await = None;
            {
                let mut snapshot = self.snapshot.write().await;
                if !matches!(
                    snapshot.state,
                    MonitorState::Idle | MonitorState::Stopped(_)
                ) {
                    snapshot.state = MonitorState::Stopped(reason.clone());
                    snapshot.stop_reason = Some(reason);
                    snapshot.pending_target = None;
                    snapshot.candidates = Vec::new();
                }
            }
            lifecycle.task.take()
        };
        if let Some(handle) = handle {
            handle.abort();
            let _ = handle.await;
        }
    }

    async fn monitor_main(&self, mut ctx: RunContext) -> RunEnd {
        let generation = ctx.generation;
        let monitor_start = ctx.config.monitor_start;
        let mut monitor_end = ctx.config.monitor_end;
        let event_start = ctx.config.event_start;

        self.log_info(&format!(
            "監視を開始しました。イベント時刻は {} です（監視窓 {}〜{}）。",
            Self::format_event_time_jst(event_start),
            Self::format_jst(monitor_start),
            Self::format_jst(monitor_end)
        ))
        .await;

        if ctx.debug_mode {
            self.log_debug(&format!(
                "monitor_start={}, monitor_end={}, now={}",
                Self::format_jst(monitor_start),
                Self::format_jst(monitor_end),
                Self::format_jst(Utc::now())
            ))
            .await;
        }

        if Utc::now() < monitor_start {
            if !self
                .set_phase(generation, MonitorState::WaitingForWindow)
                .await
            {
                return RunEnd::Stale;
            }
            self.log_info(&format!(
                "監視開始まで待機しています。開始予定: {}",
                Self::format_event_time_jst(monitor_start)
            ))
            .await;
            loop {
                if !self.is_current(generation).await {
                    return RunEnd::Stale;
                }
                if Utc::now() >= monitor_start {
                    break;
                }
                sleep(std::time::Duration::from_secs(1)).await;
            }
            if !self.set_phase(generation, MonitorState::Watching).await {
                return RunEnd::Stale;
            }
        }

        // 初回は即poll（検出後の不要待機を作らない）。
        let mut last_poll: Option<std::time::Instant> = None;
        let mut current_interval =
            jittered_poll_interval_secs(base_poll_interval_secs(Utc::now(), event_start));

        loop {
            if !self.is_current(generation).await {
                return RunEnd::Stale;
            }

            let now = Utc::now();
            if now >= monitor_end {
                // 複数インスタンスのずれ対策。候補活動があれば上限まで延長する。
                if should_extend_monitor(ctx.extensions_used, ctx.last_poll_had_candidates) {
                    ctx.extensions_used += 1;
                    monitor_end += Duration::minutes(MONITOR_GRACE_MINUTES);
                    ctx.config.monitor_end = monitor_end;
                    {
                        let mut snapshot = self.snapshot.write().await;
                        snapshot.config = Some(ctx.config.clone());
                    }
                    self.log_info(&format!(
                        "候補があるため監視を{}分延長します（{}回目、〜{}）。",
                        MONITOR_GRACE_MINUTES,
                        ctx.extensions_used,
                        Self::format_jst(monitor_end)
                    ))
                    .await;
                } else {
                    let reason =
                        deadline_reason(ctx.last_poll_unverifiable, ctx.last_poll_all_full);
                    match &reason {
                        StopReason::PreferredNameUnverifiable => {
                            self.log_info(
                                "監視窓の終了時点でインスタンス名を確認できなかったため、名前条件では参加しませんでした。",
                            )
                            .await;
                        }
                        StopReason::InstanceFull => {
                            self.log_info(
                                "監視窓の終了時点で対象が満員のままでした。キュー通知を確認するか、次回に参加してください。",
                            )
                            .await;
                        }
                        _ => {
                            self.log_info("監視可能時間を過ぎたため停止しました。")
                                .await;
                        }
                    }
                    return RunEnd::Finished(reason);
                }
            }

            let due = last_poll
                .map(|t| t.elapsed().as_secs() >= current_interval)
                .unwrap_or(true);
            if due {
                match self.poll_and_process(&mut ctx).await {
                    PollAction::Waiting => {
                        last_poll = Some(std::time::Instant::now());
                        current_interval = jittered_poll_interval_secs(base_poll_interval_secs(
                            Utc::now(),
                            event_start,
                        ));
                    }
                    PollAction::Finished(reason) => return RunEnd::Finished(reason),
                    PollAction::Stale => return RunEnd::Stale,
                }
            }

            sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    async fn handle_poll_error(&self, ctx: &mut RunContext, error: AppError) -> PollAction {
        match error {
            AppError::Auth(message) => {
                self.log_error(&format!(
                    "認証が無効です。再ログインしてください: {}",
                    message
                ))
                .await;
                PollAction::Finished(StopReason::AuthInvalid)
            }
            AppError::Forbidden(message) => {
                self.log_error(&format!(
                    "グループへのアクセスが拒否されました（権限なし）: {}",
                    message
                ))
                .await;
                PollAction::Finished(StopReason::UnrecoverableFailure(format!(
                    "グループへのアクセスが拒否されました: {}",
                    message
                )))
            }
            // 入力不正は再試行しても直らないため、即停止して理由を示す。
            AppError::InvalidInput(message) => {
                self.log_error(&format!("監視対象の設定が不正です: {}", message))
                    .await;
                PollAction::Finished(StopReason::UnrecoverableFailure(message))
            }
            AppError::RateLimit { retry_after_secs } => {
                ctx.rate_limit_hits += 1;
                if ctx.rate_limit_hits >= MAX_CONSECUTIVE_RATE_LIMITS {
                    self.log_error(
                        "API制限が続いたため停止しました。時間を置いて再試行してください。",
                    )
                    .await;
                    return PollAction::Finished(StopReason::RateLimit);
                }
                match decide_rate_limit_wait(
                    retry_after_secs,
                    ctx.rate_limit_hits,
                    Utc::now(),
                    ctx.config.monitor_end,
                ) {
                    RateLimitDecision::StopWindowExceeded => {
                        self.log_warn(
                            "サーバー指定の待機が監視期限を超えるため、これ以上送信せず終了します。",
                        )
                        .await;
                        PollAction::Finished(StopReason::RateLimit)
                    }
                    RateLimitDecision::Wait(wait_secs) => {
                        let basis = if retry_after_secs.is_some() {
                            "サーバー指定"
                        } else {
                            "backoff"
                        };
                        self.log_warn(&format!(
                            "API制限を検知しました。{}秒待機します（{}・{}回目）。",
                            wait_secs, basis, ctx.rate_limit_hits
                        ))
                        .await;
                        if self
                            .sleep_cancellable(ctx.generation, wait_secs)
                            .await
                            .is_err()
                        {
                            return PollAction::Stale;
                        }
                        PollAction::Waiting
                    }
                }
            }
            other => {
                ctx.transient_errors += 1;
                self.log_error(&format!("監視中のAPI取得に失敗しました: {}", other))
                    .await;
                if ctx.transient_errors >= MAX_CONSECUTIVE_TRANSIENT_ERRORS {
                    return PollAction::Finished(StopReason::UnrecoverableFailure(format!(
                        "API取得が{}回連続で失敗しました",
                        ctx.transient_errors
                    )));
                }
                PollAction::Waiting
            }
        }
    }
    async fn sleep_cancellable(&self, generation: u64, secs: u64) -> Result<(), ()> {
        for _ in 0..secs {
            if !self.is_current(generation).await {
                return Err(());
            }
            sleep(std::time::Duration::from_secs(1)).await;
        }
        Ok(())
    }

    async fn poll_and_process(&self, ctx: &mut RunContext) -> PollAction {
        let generation = ctx.generation;

        // group-specific 1GET。旧2GETのlocation突合はしない。
        let poll_start = Utc::now();
        let response = match self
            .vrchat_api
            .get_group_instances_for_group(&ctx.user_id, &ctx.config.group_id)
            .await
        {
            Ok(response) => response,
            Err(e) => return self.handle_poll_error(ctx, e).await,
        };
        let poll_received = Utc::now();

        ctx.transient_errors = 0;
        ctx.rate_limit_hits = 0;
        let candidates = candidates_from_detailed(&response.instances);
        ctx.last_poll_had_candidates = !candidates.is_empty();
        ctx.last_poll_all_full =
            !candidates.is_empty() && candidates.iter().all(|c| c.is_full_now());

        // run-local pinを優先する。恒久preferredへ自動保存しない。
        if let Some(pinned) = self.pinned_for(generation).await {
            match revalidate_pin(&candidates, &pinned) {
                Ok(candidate) => {
                    self.set_candidates(generation, Vec::new()).await;
                    return self.launch_candidate(ctx, candidate).await;
                }
                Err(message) => {
                    self.clear_pinned(generation).await;
                    self.log_warn(&message.to_string()).await;
                    // 下の通常選択へ落とす。
                }
            }
        }

        let selection = select_candidate(
            ctx.target_calendar_entry_id.as_deref(),
            ctx.config.preferred_instance_name.as_deref(),
            &candidates,
        );
        // 名前検証不可も窓内は待機し、期限末に正しい理由で終了する。
        ctx.last_poll_unverifiable = matches!(selection, CandidateSelection::Unverifiable(_));
        if ctx.debug_mode {
            self.log_debug(&format_poll_debug(
                poll_start,
                poll_received,
                response.fetched_at,
                &candidates,
                ctx.config.preferred_instance_name.as_deref(),
                &selection,
            ))
            .await;
        }
        match selection {
            // 会場が見えているのに待つ理由は通常ログへ出し、一覧も画面へ渡して手動で選べるようにする。
            CandidateSelection::Waiting(message) | CandidateSelection::Unverifiable(message) => {
                ctx.notified_selection = false;
                if candidates.is_empty() {
                    ctx.last_wait_note = None;
                    self.log_debug(&message).await;
                } else {
                    let note = format!(
                        "{} 見えている会場: {}",
                        message,
                        describe_seen_candidates(&candidates)
                    );
                    if ctx.last_wait_note.as_deref() != Some(note.as_str()) {
                        self.log_info(&note).await;
                        ctx.last_wait_note = Some(note);
                    } else {
                        self.log_debug(&message).await;
                    }
                }
                self.set_candidates(generation, candidates).await;
                PollAction::Waiting
            }
            // 複数候補は非終端の選択待ちへ。窓内は通常間隔で更新し、
            // 選択IPCでpinして再開する。5秒固定pollへの張り付きはしない。
            CandidateSelection::Ambiguous(message) => {
                ctx.last_wait_note = None;
                self.set_candidates(generation, candidates).await;
                if self
                    .set_phase(generation, MonitorState::AwaitingInstanceSelection)
                    .await
                {
                    if !ctx.notified_selection {
                        self.log_warn(&message).await;
                        ctx.notified_selection = true;
                    } else {
                        self.log_debug(&message).await;
                    }
                }
                PollAction::Waiting
            }
            CandidateSelection::Target(candidate) => {
                self.set_candidates(generation, Vec::new()).await;
                ctx.notified_selection = false;
                self.launch_candidate(ctx, candidate).await
            }
        }
    }

    /// 対象決定後の共通処理。送信済みlocationの再送はしない。
    async fn launch_candidate(&self, ctx: &mut RunContext, candidate: Candidate) -> PollAction {
        let generation = ctx.generation;
        if ctx.attempted.contains(&candidate.location) {
            return PollAction::Waiting;
        }
        if !self.is_current(generation).await {
            return PollAction::Stale;
        }
        ctx.attempted.insert(candidate.location.clone());
        self.dispatch_and_confirm(ctx, &candidate).await
    }

    /// 現在の実行に対する候補選択。登録前に必ず再検証する:
    /// `generation` 一致・候補の現存・参加/キュー待ち可否。固定は実行内のみ。
    /// ロック順序は `lifecycle` → `run_select` で固定する。
    pub async fn select_pinned_location(
        &self,
        generation: u64,
        location: &str,
    ) -> Result<(), AppError> {
        let (group_id, user_id) = {
            let lifecycle = self.lifecycle.lock().await;
            if lifecycle.generation != generation {
                return Err(AppError::InvalidInput(
                    "選択が古い実行のものです。最新の候補一覧から選び直してください。".to_string(),
                ));
            }
            let run_select = self.run_select.lock().await;
            match run_select.as_ref() {
                Some(state) if state.generation == generation => {
                    (state.group_id.clone(), state.user_id.clone())
                }
                _ => {
                    return Err(AppError::InvalidInput(
                        "選択が古い実行のものです。最新の候補一覧から選び直してください。"
                            .to_string(),
                    ))
                }
            }
        };
        // 最新pollで候補の現存とjoin/queue可能を再検証する（fresh 1GET）。
        let response = self
            .vrchat_api
            .get_group_instances_for_group(&user_id, &group_id)
            .await?;
        let candidates = candidates_from_detailed(&response.instances);
        revalidate_pin(&candidates, location)?;
        {
            let mut run_select = self.run_select.lock().await;
            match run_select.as_mut() {
                Some(state) if state.generation == generation => {
                    state.pinned_location = Some(location.to_string());
                }
                _ => {
                    return Err(AppError::InvalidInput(
                        "選択が古い実行のものです。最新の候補一覧から選び直してください。"
                            .to_string(),
                    ))
                }
            }
        }
        self.set_phase(generation, MonitorState::Watching).await;
        self.log_info("候補を選択しました。選択した会場を監視します。")
            .await;
        Ok(())
    }

    /// 確認tickの単一観測。成功は新鮮な本人location一致のみ。
    /// 未接続のままならDisconnected（呼出側は送信済み・確認不能で終える）。
    async fn confirm_signal(&self, target: &str, since: DateTime<Utc>) -> ConfirmSignal {
        if self
            .websocket_service
            .fresh_location_matches(target, since)
            .await
        {
            ConfirmSignal::Confirmed
        } else if self.websocket_service.is_travelling_since(since).await {
            ConfirmSignal::Travelling
        } else if let Some(kind) = self
            .websocket_service
            .queue_for_target_since(target, since)
            .await
        {
            ConfirmSignal::Queue(kind)
        } else if !self.websocket_service.is_connected().await {
            ConfirmSignal::Disconnected
        } else {
            ConfirmSignal::Pending
        }
    }

    /// 検出から起動要求までの間に固定の待機を入れない。自動セルフ招待はしない。
    async fn dispatch_and_confirm(&self, ctx: &RunContext, candidate: &Candidate) -> PollAction {
        let generation = ctx.generation;

        if !self
            .set_phase(generation, MonitorState::CandidateDetected)
            .await
        {
            return PollAction::Stale;
        }
        self.log_info("参加可能なインスタンスを検出しました。")
            .await;

        if !self
            .set_phase(generation, MonitorState::LaunchDispatchRequested)
            .await
        {
            return PollAction::Stale;
        }
        if let Err(e) = self
            .join_service
            .dispatch_launch_checked(&candidate.location)
            .await
        {
            self.log_warn(&format!("VRChatの起動に失敗しました: {}", e))
                .await;
            // 起動POSTの不明結果を自動再送しない。手動の救済操作へ案内する。
            return PollAction::Finished(StopReason::LaunchFailed(format!(
                "{}。手動テストから再送できます。",
                e
            )));
        }

        let dispatch_at = Utc::now();
        let confirm_until = dispatch_at + Duration::seconds(CONFIRM_TIMEOUT_SECS as i64);
        let pending = PendingTarget {
            location: candidate.location.clone(),
            display_name: candidate.display_name.clone(),
            dispatched_at: dispatch_at,
            confirm_until,
        };
        if !self.set_pending_target(generation, Some(pending)).await {
            return PollAction::Stale;
        }
        if !self
            .set_phase(generation, MonitorState::JoinPendingObservation)
            .await
        {
            return PollAction::Stale;
        }
        self.log_info(&format!(
            "起動リンクを送信しました。{}秒以内にVRChat側の入室を確認します。",
            CONFIRM_TIMEOUT_SECS
        ))
        .await;

        let mut announced_travelling = false;
        let mut announced_queue = false;

        while Utc::now() < confirm_until {
            if !self.is_current(generation).await {
                return PollAction::Stale;
            }

            match self.confirm_signal(&candidate.location, dispatch_at).await {
                ConfirmSignal::Confirmed => {
                    self.log_info("入室を確認しました。監視を終了します。")
                        .await;
                    return PollAction::Finished(StopReason::JoinConfirmed);
                }
                ConfirmSignal::Travelling => {
                    self.set_phase(generation, MonitorState::Travelling).await;
                    if !announced_travelling {
                        self.log_info("VRChat側で移動中です。入室の確定ではありません。")
                            .await;
                        announced_travelling = true;
                    }
                }
                ConfirmSignal::Queue(kind) => {
                    self.set_phase(generation, MonitorState::QueueWaiting).await;
                    if !announced_queue {
                        match kind {
                            QueueKind::Joined => {
                                self.log_info(
                                    "キューに参加しています。順番が来るまで確認を続けます。",
                                )
                                .await;
                            }
                            QueueKind::Ready => {
                                self.log_info("キューの順番が来ました。入室確認を続けます。")
                                    .await;
                            }
                        }
                        announced_queue = true;
                    }
                }
                // WS未接続のまま確認に入ったら必ず送信済み・確認不能で終える。
                ConfirmSignal::Disconnected => {
                    self.log_warn(
                        "WebSocketが切断されたため入室を確認できません。起動リンクは送信済みです。",
                    )
                    .await;
                    return PollAction::Finished(StopReason::LaunchSentUnconfirmed);
                }
                ConfirmSignal::Pending => {}
            }

            sleep(std::time::Duration::from_secs(1)).await;
        }

        self.log_warn(&format!(
            "確認期限（{}秒）を過ぎました。起動リンクは送信済みのため、VRChat側の状態を確認してください。",
            CONFIRM_TIMEOUT_SECS
        ))
        .await;
        PollAction::Finished(StopReason::LaunchSentUnconfirmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::EventSchedule;
    use crate::infra::http_client::HttpClient;
    use tempfile::TempDir;
    fn test_window_preset(id: &str, starts_at: DateTime<Utc>) -> Preset {
        Preset {
            id: id.to_string(),
            label: "test".to_string(),
            group_id: "grp_test".to_string(),
            group_name: None,
            schedule: EventSchedule::Once { starts_at },
            source_event_id: None,
            preferred_instance_name: None,
        }
    }

    fn log_writer_for_test() -> Arc<Mutex<LogWriter>> {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("test.log");
        std::mem::forget(dir);
        Arc::new(Mutex::new(LogWriter::new(path)))
    }

    fn create_test_service() -> MonitorService {
        let http_client = Arc::new(Mutex::new(HttpClient::new().expect("http client")));
        let vrchat_api = Arc::new(VrchatApiService::new(http_client));
        let join_service = Arc::new(JoinService::new());
        let websocket_service = Arc::new(WebsocketService::new());
        MonitorService::new(
            vrchat_api,
            join_service,
            log_writer_for_test(),
            websocket_service,
        )
    }

    // test_second_start_while_running_is_rejected はより強い
    // test_concurrent_starts_allow_exactly_one に包含されるため削除。

    #[tokio::test]
    async fn test_concurrent_starts_allow_exactly_one() {
        let service = create_test_service();
        let mut handles = Vec::new();
        for i in 0..8 {
            let service = service.clone();
            handles.push(tokio::spawn(async move {
                let preset =
                    test_window_preset(&format!("p{}", i), Utc::now() + Duration::minutes(60));
                service
                    .start_monitoring(&preset, "usr_test", false)
                    .await
                    .is_ok()
            }));
        }
        let mut ok_count = 0;
        for handle in handles {
            if handle.await.expect("task") {
                ok_count += 1;
            }
        }
        assert_eq!(ok_count, 1, "exactly one start must win the race");
        service.stop_monitoring().await;
    }

    #[tokio::test]
    async fn test_stop_waits_and_reports_manual_stop() {
        let service = create_test_service();
        let preset = test_window_preset("a", Utc::now() + Duration::minutes(60));
        service
            .start_monitoring(&preset, "usr_test", false)
            .await
            .expect("start");
        assert_eq!(service.get_state().await, MonitorState::WaitingForWindow);
        service.stop_monitoring().await;
        let snapshot = service.get_snapshot().await;
        assert_eq!(
            snapshot.state,
            MonitorState::Stopped(StopReason::ManualStop)
        );
        assert_eq!(snapshot.stop_reason, Some(StopReason::ManualStop));
        assert_eq!(snapshot.config.as_ref().unwrap().preset_id, "a");
    }

    #[tokio::test]
    async fn test_stale_run_does_not_clobber_new_run() {
        let service = create_test_service();
        let preset_a = test_window_preset("a", Utc::now() + Duration::minutes(60));
        service
            .start_monitoring(&preset_a, "usr_test", false)
            .await
            .expect("start A");
        service.stop_monitoring().await;
        // 旧実行の終了を待ってから新実行を開始する（stopはjoin済み）。
        let preset_b = test_window_preset("b", Utc::now() + Duration::minutes(60));
        service
            .start_monitoring(&preset_b, "usr_test", false)
            .await
            .expect("start B");
        // 旧タスクの残骸があっても `snapshot` はBのまま。
        sleep(std::time::Duration::from_millis(200)).await;
        let snapshot = service.get_snapshot().await;
        assert_eq!(snapshot.config.as_ref().unwrap().preset_id, "b");
        assert!(!matches!(snapshot.state, MonitorState::Stopped(_)));
        service.stop_monitoring().await;
    }

    #[tokio::test]
    async fn test_expired_once_is_rejected_without_side_effects() {
        let service = create_test_service();
        let preset = test_window_preset("old", Utc::now() - Duration::minutes(30));
        let result = service.start_monitoring(&preset, "usr_test", false).await;
        assert!(matches!(result, Err(AppError::InvalidInput(_))));
        assert_eq!(service.get_state().await, MonitorState::Idle);
    }

    #[tokio::test]
    async fn test_snapshot_json_uses_camel_case() {
        let service = create_test_service();
        let preset = test_window_preset("a", Utc::now() + Duration::minutes(60));
        service
            .start_monitoring(&preset, "usr_test", false)
            .await
            .expect("start");
        let snapshot = service.get_snapshot().await;
        let json = serde_json::to_value(&snapshot).unwrap();
        // camelCase破綻の再発検出に絞る。配線assertは量産しない。
        assert!(
            json.get("stopReason").is_some(),
            "missing stopReason: {}",
            json
        );
        assert!(json["config"].get("monitorStart").is_some());
        service.stop_monitoring().await;
        let stopped = service.get_snapshot().await;
        let stopped_json = serde_json::to_value(&stopped).unwrap();
        assert_eq!(
            stopped_json["state"],
            serde_json::json!({"Stopped": "ManualStop"})
        );
        assert_eq!(stopped_json["stopReason"], serde_json::json!("ManualStop"));
    }
    #[test]
    fn rate_limit_decision_table() {
        // 明示値の尊重・窓外停止・指数バックオフ上限を1本で確認する。
        let now = Utc::now();
        let far_end = now + Duration::seconds(7200);
        match decide_rate_limit_wait(Some(3600), 1, now, far_end) {
            RateLimitDecision::Wait(secs) => assert_eq!(secs, 3600),
            RateLimitDecision::StopWindowExceeded => panic!("explicit value must be respected"),
        }
        let near_end = now + Duration::seconds(60);
        assert!(matches!(
            decide_rate_limit_wait(Some(3600), 1, now, near_end),
            RateLimitDecision::StopWindowExceeded
        ));
        let wait = |hits| match decide_rate_limit_wait(None, hits, now, far_end) {
            RateLimitDecision::Wait(secs) => secs,
            RateLimitDecision::StopWindowExceeded => panic!("window is far"),
        };
        assert_eq!(wait(1), 10);
        assert_eq!(wait(2), 20);
        assert_eq!(wait(10), RATE_LIMIT_BACKOFF_CAP_SECS);
    }
    #[test]
    fn test_deadline_reason_prefers_unverifiable_then_full() {
        assert_eq!(
            deadline_reason(true, true),
            StopReason::PreferredNameUnverifiable
        );
        assert_eq!(
            deadline_reason(true, false),
            StopReason::PreferredNameUnverifiable
        );
        assert_eq!(deadline_reason(false, true), StopReason::InstanceFull);
        assert_eq!(deadline_reason(false, false), StopReason::MonitorExpired);
    }

    #[test]
    fn test_grace_extension_only_with_activity_below_cap() {
        assert!(should_extend_monitor(0, true));
        assert!(should_extend_monitor(1, true));
        assert!(!should_extend_monitor(0, false));
        assert!(!should_extend_monitor(MAX_MONITOR_EXTENSIONS, true));
        assert!(!should_extend_monitor(MAX_MONITOR_EXTENSIONS + 1, true));
    }

    #[tokio::test]
    async fn test_start_does_not_require_websocket() {
        // WS未接続のままでも開始できる。確認は送信済み・確認不能で終える。
        let service = create_test_service();
        assert!(!service.websocket_service.is_connected().await);
        let preset = test_window_preset("a", Utc::now() + Duration::minutes(60));
        service
            .start_monitoring(&preset, "usr_test", false)
            .await
            .expect("start must not demand websocket");
        assert_eq!(service.get_state().await, MonitorState::WaitingForWindow);
        service.stop_monitoring().await;
    }

    #[tokio::test]
    async fn test_confirm_signal_orders_match_before_travelling() {
        use crate::services::websocket_service::QueueKind as QK;

        let service = create_test_service();
        let since = Utc::now();
        let target = "wrld_a:1~group(g)";

        // 未接続・無通知は必ずDisconnected（成功にも継続にもしない）。
        assert_eq!(
            service.confirm_signal(target, since).await,
            ConfirmSignal::Disconnected
        );

        // 対象一致は成功。
        service
            .websocket_service
            .push_test_location(target, since)
            .await;
        assert_eq!(
            service.confirm_signal(target, since).await,
            ConfirmSignal::Confirmed
        );

        // 移動中は一致文字列でも成功にしない。
        let travelling = "traveling:wrld_a:1";
        service
            .websocket_service
            .push_test_location(travelling, since)
            .await;
        assert_eq!(
            service.confirm_signal(travelling, since).await,
            ConfirmSignal::Travelling
        );

        // 対象のqueue通知のみ扱う。
        service
            .websocket_service
            .push_test_queue(QK::Joined, Some(target.to_string()), since)
            .await;
        // locationは移動中のままなのでTravellingが優先される。
        assert_eq!(
            service.confirm_signal(target, since).await,
            ConfirmSignal::Travelling
        );
        service
            .websocket_service
            .push_test_location("offline", since)
            .await;
        assert_eq!(
            service.confirm_signal(target, since).await,
            ConfirmSignal::Queue(QK::Joined)
        );
        service
            .websocket_service
            .push_test_queue(QK::Joined, Some("wrld_other:9".to_string()), since)
            .await;
        // ヒント不一致のqueueは無視され、未接続のためDisconnected。
        assert_eq!(
            service.confirm_signal(target, since).await,
            ConfirmSignal::Disconnected
        );
    }

    fn pin_candidate(location: &str, joinable: bool) -> Candidate {
        Candidate {
            location: location.to_string(),
            instance_id: "inst~id".to_string(),
            world_id: "wrld_test".to_string(),
            display_name: Some(location.to_string()),
            member_count: 10,
            has_capacity_for_you: Some(joinable),
            is_full: Some(!joinable),
            queue_enabled: Some(false),
            queue_size: Some(0),
            calendar_entry_id: None,
        }
    }

    #[test]
    fn revalidate_pin_table() {
        // 受理・消失・満員の判定を1本で確認する。選択後にfull化したら選び直しにする。
        let fresh = vec![pin_candidate("loc-a", true), pin_candidate("loc-b", true)];
        let candidate = revalidate_pin(&fresh, "loc-a").expect("actionable pin");
        assert_eq!(candidate.location, "loc-a");
        let disappeared = vec![pin_candidate("loc-b", true)];
        assert!(revalidate_pin(&disappeared, "loc-a").is_err());
        assert!(revalidate_pin(&[], "loc-a").is_err());
        let full = vec![pin_candidate("loc-a", false)];
        assert!(revalidate_pin(&full, "loc-a").is_err());
    }

    #[test]
    fn seen_candidates_list_names_and_states_without_counts() {
        let mut nameless = pin_candidate("loc-c", true);
        nameless.display_name = None;
        let line = describe_seen_candidates(&[
            pin_candidate("HookahHolic_第1インスタンス", false),
            nameless,
        ]);
        assert_eq!(
            line,
            "HookahHolic_第1インスタンス（満員）、名前なし（空きあり）"
        );
    }

    #[test]
    fn poll_debug_line_reports_timing_and_counts() {
        // rttとfetched_ageで間隔律速と反映遅延を切り分ける。値は代表1本で固定する。
        let now = Utc::now();
        let start = now - Duration::milliseconds(120);
        let fetched = now - Duration::milliseconds(800);
        let candidates = vec![
            pin_candidate("Event_第1", true),
            pin_candidate("Other", true),
        ];
        let selection = select_candidate(None, Some("第1"), &candidates);
        let line = format_poll_debug(
            start,
            now,
            Some(fetched),
            &candidates,
            Some("第1"),
            &selection,
        );
        assert!(line.contains("rtt=120ms"), "got {line}");
        assert!(line.contains("fetched_age=800ms"), "got {line}");
        assert!(line.contains("candidates=2"), "got {line}");
        assert!(line.contains("actionable=2"), "got {line}");
        assert!(line.contains("matched=1"), "got {line}");
        assert!(line.contains("selection="), "got {line}");
        let unknown = format_poll_debug(
            start,
            now,
            None,
            &[],
            None,
            &CandidateSelection::Waiting("w".to_string()),
        );
        assert!(unknown.contains("fetched_age=unknown"), "got {unknown}");
        assert!(unknown.contains("matched=-"), "got {unknown}");
    }

    #[tokio::test]
    async fn test_select_with_stale_generation_is_rejected() {
        let service = create_test_service();
        // runなし・世代不一致はネットワークに出る前に拒否する。
        assert!(service.select_pinned_location(1, "loc-a").await.is_err());
        *service.run_select.lock().await = Some(RunSelectState {
            generation: 5,
            group_id: "grp_test".to_string(),
            user_id: "usr_test".to_string(),
            pinned_location: None,
        });
        assert!(service.select_pinned_location(3, "loc-a").await.is_err());
        assert!(service.select_pinned_location(6, "loc-a").await.is_err());
        assert!(service.pinned_for(5).await.is_none());
    }
}
