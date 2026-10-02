import { invoke } from '@tauri-apps/api/core';

export interface CurrentUser {
  id: string;
  username: string;
  displayName: string;
  twoFactorAuthEnabled: boolean;
  /** VRChatのユーザー画像URL。取得不可のときはnull。 */
  profileIconUrl?: string | null;
  presence?: {
    world: string;
    instance: string;
    travelingToWorld?: string;
    travelingToInstance?: string;
  };
}

/** イベント予定。onceはUTCのRFC3339、weekly/biweeklyの時刻はJSTのHH:MM(:SS)。 */
export type EventSchedule =
  | { kind: 'once'; startsAt: string }
  | WeeklySchedule
  | BiweeklySchedule;

/** 毎週。weekdayは0=日曜〜6=土曜 (JS Date.getDayと同一)。 */
export interface WeeklySchedule {
  kind: 'weekly';
  weekday: number;
  time: string;
}

/** 隔週。anchorDate (JST暦日YYYY-MM-DD) が周期の基準。 */
export interface BiweeklySchedule {
  kind: 'biweekly';
  weekday: number;
  time: string;
  anchorDate: string;
}

export interface Preset {
  id: string;
  label: string;
  groupId: string;
  groupName?: string | null;
  schedule: EventSchedule;
  sourceEventId?: string | null;
  preferredInstanceName?: string | null;
}

export interface AppConfig {
  presets: Preset[];
  selectedPresetId?: string;
  debugMode?: boolean;
  theme?: 'light' | 'dark' | 'system';
}

/** `save_config` の差分形式。欠落キーは維持、`selectedPresetId` のnullは解除。 */
export interface ConfigPatch {
  selectedPresetId?: string | null;
  debugMode?: boolean;
  theme?: 'light' | 'dark' | 'system';
}

export interface PreflightResult {
  portableWritable: boolean;
  protocolHandlerAvailable: boolean;
  vrchatProcessDetected: boolean;
  savedSessionValid: boolean;
  websocketConnected: boolean;
  websocketConnectionState: 'disconnected' | 'connecting' | 'connected' | 'error';
  websocketLastError?: string;
  allClear: boolean;
  blockers: string[];
}

export type TwoFactorMethod = 'totp' | 'emailOtp';
export const TWO_FACTOR_METHODS: readonly TwoFactorMethod[] = ['totp', 'emailOtp'];

export function twoFactorMethodLabel(method: string): string {
  if (method === 'totp') return '認証アプリ (TOTP)';
  if (method === 'emailOtp') return 'メールワンタイムパスワード';
  return method;
}

export interface LoginResponse {
  success: boolean;
  requiresTwoFactor?: string[];
  username?: string;
  user?: CurrentUser;
}

export interface MonitorConfig {
  presetId: string;
  groupId: string;
  eventStart: string;
  monitorStart: string;
  monitorEnd: string;
  preferredInstanceName?: string | null;
}

/** 送信済み・確認不能の追跡対象。confirmUntilまで入室確認を待つ。 */
export interface PendingTarget {
  location: string;
  displayName?: string | null;
  dispatchedAt: string;
  confirmUntil: string;
}

/** 実行時に確定するMonitorState (外部タグserde)。停止時は {Stopped: StopReason}。 */
export type MonitorPhase =
  | 'Idle'
  | 'WaitingForWindow'
  | 'Watching'
  | 'AwaitingInstanceSelection'
  | 'CandidateDetected'
  | 'LaunchDispatchRequested'
  | 'JoinPendingObservation'
  | 'Travelling'
  | 'QueueWaiting';

/** 実行時に確定するStopReason。成功はJoinConfirmedのみ。 */
export type StopReason =
  | 'JoinConfirmed'
  | 'LaunchSentUnconfirmed'
  | 'InstanceFull'
  | 'AuthInvalid'
  | 'RateLimit'
  | 'ManualStop'
  | 'MonitorExpired'
  | 'PreferredNameUnverifiable'
  | { AmbiguousTarget: string }
  | { LaunchFailed: string }
  | { ApiError: string }
  | { UnrecoverableFailure: string };

export type MonitorState = MonitorPhase | { Stopped: StopReason };

export function stopReasonKind(reason: StopReason): string {
  return typeof reason === 'string' ? reason : Object.keys(reason)[0];
}

export function stopReasonDetail(reason: StopReason): string | undefined {
  if (typeof reason === 'string') return undefined;
  const value = Object.values(reason)[0];
  return typeof value === 'string' ? value : undefined;
}

export function isMonitorActive(state: MonitorState | undefined): boolean {
  if (!state) return false;
  if (typeof state === 'string') return state !== 'Idle';
  return false;
}
/** 監視ポーリングの候補1件。Rust側 `Candidate` (camelCase) と同一形状。 */
export interface Candidate {
  location: string;
  instanceId: string;
  worldId: string;
  displayName?: string | null;
  memberCount: number;
  hasCapacityForYou?: boolean | null;
  isFull?: boolean | null;
  queueEnabled?: boolean | null;
  queueSize?: number | null;
  /** group-specific由来。単発の対象ID照合にのみ使う。 */
  calendarEntryId?: string | null;
}

/** 直接入室できるか。両方不明を参加可能扱いしない。キューは含めない。 */
export function isCandidateJoinable(candidate: Candidate): boolean {
  if (candidate.hasCapacityForYou === false) return false;
  if (candidate.hasCapacityForYou === true) return true;
  return candidate.isFull === false;
}

/** 満員が確定しているか。 */
export function isCandidateFull(candidate: Candidate): boolean {
  return candidate.isFull === true || candidate.hasCapacityForYou === false;
}

/** キュー参加できるか（満員時の別経路）。 */
export function canCandidateQueue(candidate: Candidate): boolean {
  return (
    candidate.queueEnabled === true && isCandidateFull(candidate) && !isCandidateJoinable(candidate)
  );
}

/** `get_monitor_status` の応答。`candidates` は選択待ち中の現在一覧、`generation` は現在の実行の世代。 */
export interface MonitorSnapshot {
  state: MonitorState;
  stopReason?: StopReason | null;
  config?: MonitorConfig | null;
  pendingTarget?: PendingTarget | null;
  candidates: Candidate[];
  generation: number;
}

export interface ApiBudgetEstimate {
  profileName: string;
  estimatedRequests: number;
  warnThreshold: number;
  blockThreshold: number;
  shouldWarn: boolean;
  shouldBlock: boolean;
  /** UTC RFC3339。UIは同じ値を表示する。 */
  eventStart: string;
  monitorStart: string;
  monitorEnd: string;
}

export interface GroupSummary {
  id: string;
  name: string;
  shortCode?: string;
  memberCount?: number;
}

export interface GroupCalendarEventSummary {
  id: string;
  title: string;
  startsAt: string;
  endsAt: string;
  description?: string;
}

export interface PresetApiTestResult {
  groupName: string;
  instanceCount: number;
  estimatedRequests: number;
  shouldWarn: boolean;
}

export interface GroupInfoTestResult {
  groupName: string;
  shortCode?: string;
  memberCount?: number;
}

export type SelectionStatus = 'ready' | 'waiting' | 'ambiguous' | 'unverifiable';

export function selectionStatusLabel(status: SelectionStatus): string {
  switch (status) {
    case 'ready': return '参加可能';
    case 'waiting': return '待機中';
    case 'ambiguous': return '対象が曖昧';
    case 'unverifiable': return '確認不能';
  }
}

export interface InstanceCheckTestResult {
  groupName: string;
  instanceCount: number;
  estimatedRequests: number;
  shouldWarn: boolean;
  targetLocation?: string;
  targetDisplayName?: string;
  canDispatch: boolean;
  selectionStatus: SelectionStatus;
  message: string;
  /** 現在候補一覧。複数時はUI選択用（非終端）。 */
  candidates: Candidate[];
}

/** 秘密を含めない診断の書き出しの候補1件。ID類は含めない。 */
export interface DiagnosticCandidate {
  location: string;
  displayName?: string | null;
  full: boolean;
  hasCapacityForYou?: boolean | null;
  queueEnabled?: boolean | null;
  queueSize?: number | null;
  userCount: number;
  capacity?: number | null;
  worldId: string;
  calendarEntryId?: string | null;
}

/** `export_monitor_diagnostics` の応答。秘密情報は含めない。 */
export interface MonitorDiagnostics {
  endpointTemplate: string;
  groupId: string;
  candidateCount: number;
  candidates: DiagnosticCandidate[];
  calendarAvailable: boolean;
}

export interface ApiActionTestResult {
  action: string;
  targetLocation: string;
  targetDisplayName?: string;
  message: string;
}

export function formatInvokeError(err: unknown, fallback: string): string {
  const message =
    typeof err === 'string'
      ? err
      : err instanceof Error
        ? err.message
        : err && typeof err === 'object' && 'message' in err && typeof err.message === 'string'
          ? err.message
          : '';
  // Tauri runtime不在 (素のブラウザ等) では invoke 自体が存在しない。raw TypeErrorを出さない。
  if (/__TAURI_INTERNALS__|reading 'invoke'|invoke is not a function/.test(message)) {
    return 'アプリのバックエンドに接続できませんでした。アプリを再起動してください。';
  }
  // バックエンドの表示用エラーは日本語で返す。日本語を含まないもの（Tauriの引数エラーや
  // JSの例外などの生メッセージ）は利用者向けでないため、呼び出し側の文言に置き換える。
  if (/[ぁ-んァ-ヶ一-龠]/.test(message)) return message.trim();
  return fallback;
}

export const api = {
  auth: {
    checkSession: () => invoke<CurrentUser | null>('check_session'),
    login: (username: string, password: string, rememberSession: boolean) =>
      invoke<LoginResponse>('login', { username, password, rememberSession }),
    verifyTwoFactor: (code: string, method: TwoFactorMethod, rememberSession: boolean) =>
      invoke<{ success: boolean; user?: CurrentUser }>('verify_two_factor', {
        code,
        method,
        rememberSession,
      }),
    logout: () => invoke<void>('logout'),
  },

  presets: {
    getPresets: () => invoke<Preset[]>('get_presets'),
    savePreset: (preset: Preset) => invoke<void>('save_preset', { preset }),
    deletePreset: (presetId: string) => invoke<void>('delete_preset', { presetId }),
    lookupGroupSummary: (groupId: string) =>
      invoke<GroupSummary>('lookup_group_summary', { groupId }),
    getGroupCalendarEvents: (groupId: string) =>
      invoke<GroupCalendarEventSummary[]>('get_group_calendar_events', { groupId }),
  },

  config: {
    getConfig: () => invoke<AppConfig>('get_config'),
    /** 差分のみ。欠落=維持、`selectedPresetId` のnull=解除。`presets` は送らない。 */
    saveConfigPatch: (patch: ConfigPatch) => invoke<void>('save_config', { patch }),
  },

  system: {
    checkPreflight: () => invoke<PreflightResult>('check_preflight'),
    getPortablePathsInfo: () =>
      invoke<{
        dataDir: string;
        cacheDir: string;
        logsDir: string;
        sessionFile: string;
        presetsFile: string;
      }>('get_portable_paths_info'),
  },

  monitor: {
    getStatus: () => invoke<MonitorSnapshot>('get_monitor_status'),
    start: (presetId: string) => invoke<void>('start_monitoring', { presetId }),
    stop: () => invoke<void>('stop_monitoring'),
    /** 現在の実行に対する候補選択。スナップショットの `generation` と `location` を渡す。 */
    selectCandidate: (generation: number, location: string) =>
      invoke<void>('select_monitor_candidate', { generation, location }),
    /** 秘密を含めない診断の書き出し。候補件数・容量・キュー・`calendarEntryId` のみ返す。 */
    exportDiagnostics: (presetId: string) =>
      invoke<MonitorDiagnostics>('export_monitor_diagnostics', { presetId }),
    getApiBudgetEstimate: (presetId: string) =>
      invoke<ApiBudgetEstimate>('get_api_budget_estimate', { presetId }),
    testPresetApi: (presetId: string) =>
      invoke<PresetApiTestResult>('test_preset_api', { presetId }),
    testGroupInfo: (presetId: string) =>
      invoke<GroupInfoTestResult>('test_group_info', { presetId }),
    testInstanceCheck: (presetId: string) =>
      invoke<InstanceCheckTestResult>('test_instance_check', { presetId }),
    runPresetApiAction: (presetId: string, action: 'launch' | 'selfInvite') =>
      invoke<ApiActionTestResult>('run_preset_api_action', { presetId, action }),
  },

  logs: {
    getRecent: (count?: number) =>
      invoke<
        Array<{
          timestamp: string;
          level: string;
          message: string;
          source: string;
        }>
      >('get_recent_logs', { count }),
  },
};
