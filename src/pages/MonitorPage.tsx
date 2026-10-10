import { useCallback, useEffect, useMemo, useState } from 'react';
import { AlertTriangle, RefreshCw } from 'lucide-react';
import {
  api,
  canCandidateQueue,
  stopReasonKind,
  formatInvokeError,
  isCandidateJoinable,
  type ApiBudgetEstimate,
  type AppConfig,
  type Candidate,
  type MonitorConfig,
  type MonitorSnapshot,
  type MonitorState,
  type PendingTarget,
  type PreflightResult,
  type Preset,
  type StopReason,
} from '../lib/tauri';
import { describeCue, isPhaseActive } from '../lib/cue';
import { isOccurrenceHandled, markOccurrenceHandled, useAutoStartState } from '../lib/autoStart';
import { formatHms, formatMonthDayHm, formatSchedule, nextOccurrence } from '../lib/format';
import { CandidateChooser } from '../components/CandidateChooser';
import { CuePanel, CueSteps } from '../components/CuePanel';

interface LogEntry {
  timestamp: string;
  level: 'debug' | 'info' | 'warn' | 'error';
  message: string;
}

const LEVEL_LABEL: Record<LogEntry['level'], string> = {
  debug: '詳細',
  info: '情報',
  warn: '注意',
  error: 'エラー',
};

/** 監視の時間帯。Rust 側の解決値（監視中の設定、なければ見積り）を正とし、なければ開始3分前〜2分後で推定する */
const FALLBACK_LEAD_MS = 3 * 60 * 1000;
const FALLBACK_TAIL_MS = 2 * 60 * 1000;

function toDate(iso: string | null | undefined): Date | null {
  if (!iso) return null;
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? null : date;
}

function useNow(intervalMs = 1000): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = window.setInterval(() => setNow(new Date()), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
  return now;
}

interface MonitorPageProps {
  onOpenEvents: () => void;
}

export function MonitorPage({ onOpenEvents }: MonitorPageProps) {
  const now = useNow();
  const autoStart = useAutoStartState();
  const [preflight, setPreflight] = useState<PreflightResult | null>(null);
  const [initializing, setInitializing] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [presets, setPresets] = useState<Preset[]>([]);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [monitorState, setMonitorState] = useState<MonitorState | undefined>(undefined);
  const [stopReason, setStopReason] = useState<StopReason | null>(null);
  const [pendingTarget, setPendingTarget] = useState<PendingTarget | null>(null);
  const [monitorConfig, setMonitorConfig] = useState<MonitorConfig | null>(null);
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [generation, setGeneration] = useState(0);
  const [budget, setBudget] = useState<ApiBudgetEstimate | null>(null);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [starting, setStarting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [selectingLocation, setSelectingLocation] = useState<string | null>(null);
  const [selectError, setSelectError] = useState<string | null>(null);
  const [selectNotice, setSelectNotice] = useState<string | null>(null);

  const applyStatus = (status: MonitorSnapshot) => {
    setMonitorState(status.state);
    setStopReason(status.stopReason ?? null);
    setMonitorConfig(status.config ?? null);
    setPendingTarget(status.pendingTarget ?? null);
    setCandidates(status.candidates ?? []);
    setGeneration(status.generation ?? 0);
  };

  const loadBudget = useCallback(async (presetId: string | undefined) => {
    if (!presetId) {
      setBudget(null);
      return;
    }
    try {
      setBudget(await api.monitor.getApiBudgetEstimate(presetId));
    } catch {
      setBudget(null);
    }
  }, []);

  const loadLogs = useCallback(async () => {
    try {
      const result = await api.logs.getRecent(80);
      setLogs(
        result.map((log) => ({
          timestamp: log.timestamp,
          level: log.level.toLowerCase() as LogEntry['level'],
          message: log.message,
        })),
      );
    } catch {
      /* ログ取得の失敗は次回に委ねる */
    }
  }, []);

  const loadAll = useCallback(async () => {
    setLoadError(null);
    try {
      const [preflightResult, configResult, presetResult, status] = await Promise.all([
        api.system.checkPreflight(),
        api.config.getConfig(),
        api.presets.getPresets(),
        api.monitor.getStatus(),
      ]);
      setPreflight(preflightResult);
      setConfig(configResult);
      setPresets(presetResult);
      applyStatus(status);
      await Promise.all([loadBudget(configResult.selectedPresetId), loadLogs()]);
    } catch (err) {
      // 中途半端な状態を作らず、失敗は明示して再試行させる
      setLoadError(formatInvokeError(err, '監視の状態を読み込めませんでした。\nもう一度試してください。'));
    } finally {
      setInitializing(false);
    }
  }, [loadBudget, loadLogs]);

  useEffect(() => {
    loadAll();
  }, [loadAll]);

  const selectedPreset = presets.find((p) => p.id === config?.selectedPresetId) ?? null;
  const running = !!monitorState && typeof monitorState === 'string' && monitorState !== 'Idle';

  // 監視中は状態を細かく追い、それ以外はゆっくり確認する。get_monitor_status は手元の状態を読むだけで通信しない
  useEffect(() => {
    const id = window.setInterval(async () => {
      try {
        applyStatus(await api.monitor.getStatus());
      } catch {
        /* ポーリング失敗は次回に委ね、表示中の確定状態を壊さない */
      }
    }, running ? 2000 : 10000);
    return () => window.clearInterval(id);
  }, [running]);

  // 準備状況・時間帯・ログはゆっくり更新する（いずれも手元の計算・読み取り）
  useEffect(() => {
    const id = window.setInterval(() => {
      api.system.checkPreflight().then(setPreflight).catch(() => {});
      void loadBudget(config?.selectedPresetId);
      void loadLogs();
    }, running ? 5000 : 30000);
    return () => window.clearInterval(id);
  }, [running, config?.selectedPresetId, loadBudget, loadLogs]);

  // 自動で監視を始めたら、すぐに状態を取り直す
  useEffect(() => {
    if (autoStart.version === 0) return;
    api.monitor.getStatus().then(applyStatus).catch(() => {});
  }, [autoStart.version]);

  // 状態が切り替わったら、次の回の時間帯とログをすぐ取り直す
  const stateKey = typeof monitorState === 'object' ? 'Stopped' : (monitorState ?? 'none');
  useEffect(() => {
    if (initializing) return;
    void loadBudget(config?.selectedPresetId);
    void loadLogs();
  }, [stateKey]);

  const handleStart = async (presetId: string) => {
    setStarting(true);
    setActionError(null);
    try {
      await api.monitor.start(presetId);
      applyStatus(await api.monitor.getStatus());
    } catch (err) {
      setActionError(formatInvokeError(err, '監視を始められませんでした。\nもう一度試してください。'));
    } finally {
      setStarting(false);
    }
  };

  const handleStop = async () => {
    setStopping(true);
    setActionError(null);
    // 止めた回は、同じ回のうちに自動で始め直さない
    if (monitorConfig?.presetId && monitorConfig.eventStart) {
      markOccurrenceHandled(monitorConfig.presetId, monitorConfig.eventStart);
    }
    try {
      await api.monitor.stop();
      applyStatus(await api.monitor.getStatus());
    } catch (err) {
      setActionError(formatInvokeError(err, '監視を止められませんでした。\nもう一度試してください。'));
    } finally {
      setStopping(false);
    }
  };

  const handleSelectCandidate = async (candidate: Candidate) => {
    // 消失・満員化した古い選択は実行せず、一覧を更新して短く理由を示す
    const current = candidates.find((c) => c.location === candidate.location);
    if (!current || (!isCandidateJoinable(current) && !canCandidateQueue(current))) {
      setSelectNotice('会場の一覧が変わりました。もう一度選んでください。');
      try {
        applyStatus(await api.monitor.getStatus());
      } catch {
        /* 更新失敗は次回pollに委ねる */
      }
      return;
    }
    setSelectingLocation(candidate.location);
    setSelectError(null);
    setSelectNotice(null);
    try {
      await api.monitor.selectCandidate(generation, candidate.location);
      applyStatus(await api.monitor.getStatus());
    } catch (err) {
      setSelectError(formatInvokeError(err, '会場を選べませんでした。\n一覧を更新して、もう一度選んでください。'));
      try {
        applyStatus(await api.monitor.getStatus());
      } catch {
        /* 更新失敗は次回pollに委ねる */
      }
    } finally {
      setSelectingLocation(null);
    }
  };

  const refreshCandidates = async () => {
    setSelectError(null);
    try {
      applyStatus(await api.monitor.getStatus());
    } catch (err) {
      setSelectError(formatInvokeError(err, '会場の一覧を更新できませんでした。\nもう一度試してください。'));
    }
  };

  // 監視中の設定が選択中イベントのものか
  const monitoringSelected = !!selectedPreset && (!monitorConfig || monitorConfig.presetId === selectedPreset.id);
  // 監視中と、入室成功・失敗の結果を出している間は、その回の設定を時間帯の正とする。
  // 止めたあとも config は残るため、それ以外は次の回の見積りを使う
  const showingResult = !running && !!stopReason && stopReasonKind(stopReason) !== 'ManualStop';
  const useRunConfig = monitoringSelected && !!monitorConfig && (running || showingResult);

  const upcomingWindow = useMemo(() => {
    const fromBudget = budget
      ? { s: toDate(budget.monitorStart), e: toDate(budget.eventStart), x: toDate(budget.monitorEnd) }
      : null;
    if (fromBudget?.s && fromBudget.e && fromBudget.x) {
      return { monitorStart: fromBudget.s, eventStart: fromBudget.e, monitorEnd: fromBudget.x };
    }
    const next = selectedPreset ? nextOccurrence(selectedPreset) : null;
    if (!next) return null;
    return {
      monitorStart: new Date(next.getTime() - FALLBACK_LEAD_MS),
      eventStart: next,
      monitorEnd: new Date(next.getTime() + FALLBACK_TAIL_MS),
    };
  }, [budget, selectedPreset]);

  const cueWindow = useMemo(() => {
    if (useRunConfig && monitorConfig) {
      const s = toDate(monitorConfig.monitorStart);
      const e = toDate(monitorConfig.eventStart);
      const x = toDate(monitorConfig.monitorEnd);
      if (s && e && x) return { monitorStart: s, eventStart: e, monitorEnd: x };
    }
    return upcomingWindow;
  }, [useRunConfig, monitorConfig, upcomingWindow]);

  if (initializing) {
    return (
      <div className="page" data-testid="monitor-page">
        <p className="page-status" role="status">
          監視の状態を確認しています…
        </p>
      </div>
    );
  }

  if (loadError && !preflight) {
    return (
      <div className="page" data-testid="monitor-page">
        <div className="callout error" role="alert">
          <AlertTriangle size={16} aria-hidden />
          <div className="callout-body">
            <strong>監視の状態を読み込めませんでした</strong>
            <p className="error-message">{loadError}</p>
            <button type="button" onClick={loadAll}>
              <RefreshCw size={14} aria-hidden />
              読み込み直す
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (!selectedPreset) {
    return (
      <div className="page" data-testid="monitor-page">
        <h1 className="sr-only">監視</h1>
        <section className="cue cue-empty" aria-labelledby="cue-title" data-testid="cue-panel" data-phase="empty">
          <div className="lamp lamp-off" aria-hidden />
          <div className="cue-text">
            <h2 id="cue-title" className="cue-title" data-testid="cue-title">
              監視するイベントがありません
            </h2>
            <p className="cue-detail">
              {presets.length === 0
                ? 'まずイベント画面で、入りたいイベントを登録してください。'
                : 'イベント画面で、監視するイベントを1件選んでください。'}
            </p>
          </div>
        </section>
        {running && (
          <div className="callout warning" role="status">
            <AlertTriangle size={16} aria-hidden />
            <div className="callout-body">
              <strong>選んでいないイベントの監視が続いています。</strong>
              <p>不要なら止めてください。</p>
              <button type="button" onClick={handleStop} disabled={stopping}>
                {stopping ? '止めています…' : '監視を止める'}
              </button>
            </div>
          </div>
        )}
        {actionError && (
          <div className="callout error" role="alert">
            <AlertTriangle size={16} aria-hidden />
            <div className="callout-body">
              <p className="error-message">{actionError}</p>
            </div>
          </div>
        )}
        <div className="cue-actions">
          <button type="button" className="primary large" onClick={onOpenEvents}>
            {presets.length === 0 ? 'イベントを登録する' : 'イベントを選ぶ'}
          </button>
        </div>
      </div>
    );
  }

  const expired = !monitoringSelected || !running ? nextOccurrence(selectedPreset, now) === null : false;
  const view = describeCue({
    state: monitoringSelected ? monitorState : 'Idle',
    stopReason: monitoringSelected ? stopReason : null,
    pendingTarget,
    monitorStart: cueWindow?.monitorStart ?? null,
    eventStart: cueWindow?.eventStart ?? null,
    expired,
    skipAuto: !!upcomingWindow && isOccurrenceHandled(selectedPreset.id, upcomingWindow.eventStart.toISOString()),
    seenCandidates: monitoringSelected && candidates.length > 0,
  });
  // 会場は見えているが条件に合わず待っている間も、一覧から手動で選べるようにする
  const showChooser = view.phase === 'choose' || (view.phase === 'watching' && candidates.length > 0);
  const active = isPhaseActive(view.phase);
  const otherRunning = running && !monitoringSelected;
  const visibleLogs = logs.filter((log) => config?.debugMode || log.level !== 'debug');
  const latestLog = visibleLogs[visibleLogs.length - 1];
  // 今の回がもう始まっているなら「次の回」とは呼ばない
  const nextLabel = upcomingWindow && !expired && upcomingWindow.eventStart > now ? formatMonthDayHm(upcomingWindow.eventStart) : null;

  return (
    <div className="page monitor" data-testid="monitor-page">
      <div className="event-line">
        <h1 className="event-line-name">{selectedPreset.label}</h1>
        <span className="event-line-meta">
          <b>{formatSchedule(selectedPreset.schedule)}</b>
          <span>{selectedPreset.groupName || 'グループ名は未確認'}</span>
        </span>
        {!active && (
          <button type="button" className="link event-line-switch" onClick={onOpenEvents}>
            別のイベントにする
          </button>
        )}
      </div>

      <CuePanel
        view={view}
        now={now}
        window={cueWindow}
        compact={view.phase === 'choose'}
      />

      {showChooser && (
        <CandidateChooser
          candidates={candidates}
          calendarEventId={selectedPreset.schedule.kind === 'once' ? selectedPreset.sourceEventId ?? null : null}
          selectingLocation={selectingLocation}
          notice={selectNotice}
          error={selectError}
          onSelect={handleSelectCandidate}
          onRefresh={refreshCandidates}
        />
      )}

      {view.step !== null && <CueSteps step={view.step} done={view.phase === 'done'} />}

      {otherRunning && (
        <div className="callout warning" role="status">
          <AlertTriangle size={16} aria-hidden />
          <div className="callout-body">
            <strong>別のイベントを監視しています。</strong>
            <p>このイベントを監視するには、先にそちらを止めてください。</p>
          </div>
        </div>
      )}

      {!active && preflight && !preflight.allClear &&
        preflight.blockers.map((blocker, idx) => (
          <div key={idx} className="callout warning" role="alert">
            <AlertTriangle size={16} aria-hidden />
            <div className="callout-body">
              <p>{blocker}</p>
            </div>
          </div>
        ))}

      {budget?.shouldWarn && (
        <div className="callout warning" role="status">
          <AlertTriangle size={16} aria-hidden />
          <div className="callout-body">
            <p>
              VRChat への問い合わせが多めになる見込みです（約 {budget.estimatedRequests} 回）。
              <br />
              制限を受けると、途中で監視が止まることがあります。
            </p>
          </div>
        </div>
      )}

      {actionError && (
        <div className="callout error" role="alert">
          <AlertTriangle size={16} aria-hidden />
          <div className="callout-body">
            <p className="error-message">{actionError}</p>
          </div>
        </div>
      )}

      {autoStart.error && !running && (
        <div className="callout warning" role="alert">
          <AlertTriangle size={16} aria-hidden />
          <div className="callout-body">
            <strong>自動で監視を始められませんでした。</strong>
            <p className="pre-line">{autoStart.error}</p>
            <p>30秒ごとに、もう一度試します。</p>
          </div>
        </div>
      )}

      <div className="cue-actions">
        {running ? (
          <button type="button" onClick={handleStop} disabled={stopping}>
            {stopping ? '止めています…' : '監視を止める'}
          </button>
        ) : (
          <>
            <button
              type="button"
              className="primary"
              onClick={() => handleStart(selectedPreset.id)}
              disabled={starting || expired}
            >
              {starting ? '始めています…' : '今すぐ監視を始める'}
            </button>
            {expired ? (
              <span className="hint">単発のイベントで、この回はもう終わっています。</span>
            ) : (
              view.phase !== 'idle' && nextLabel && <span className="hint">次の回は {nextLabel} です。</span>
            )}
          </>
        )}
      </div>

      <details className="log-panel">
        <summary>
          <span className="log-summary-title">ログ</span>
          {latestLog ? (
            <span className="log-summary-latest">
              <span className="num">{formatHms(new Date(latestLog.timestamp))}</span>
              <span className="log-summary-text">{latestLog.message}</span>
            </span>
          ) : (
            <span className="hint">まだありません</span>
          )}
        </summary>
        {visibleLogs.length > 0 && (
          <ol className="log-list">
            {[...visibleLogs].reverse().map((log, index) => (
              <li key={`${log.timestamp}-${index}`} className={`log-entry is-${log.level}`}>
                <span className="log-time num">{formatHms(new Date(log.timestamp))}</span>
                <span className="log-level">{LEVEL_LABEL[log.level] ?? log.level}</span>
                <span className="log-message">{log.message}</span>
              </li>
            ))}
          </ol>
        )}
      </details>
    </div>
  );
}
