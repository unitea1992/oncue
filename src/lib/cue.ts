import {
  stopReasonDetail,
  stopReasonKind,
  type MonitorState,
  type PendingTarget,
  type StopReason,
} from './tauri';
import { formatHm, formatHms, formatMonthDayHm } from './format';

/**
 * 監視画面の見せ方。バックエンドの MonitorState を、舞台の合図になぞらえた段階へまとめる。
 * - idle: 監視していない（ランプ消灯）
 * - waiting / watching / choose: 待機・監視中（琥珀）
 * - go / confirm: 会場が決まり、参加と入室確認の最中（緑）
 * - done: 入室を確認した（緑で点灯したまま）
 * - stopped: 入室せずに止まった（消灯）
 */
export type CuePhase = 'idle' | 'waiting' | 'watching' | 'choose' | 'go' | 'confirm' | 'done' | 'stopped';

export type LampState = 'off' | 'standby' | 'standby-live' | 'standby-blink' | 'go' | 'go-blink';

export const CUE_STEPS = ['待機', '会場を監視', '会場を決める', 'VRChat で参加', '入室を確認'] as const;

export interface CueView {
  phase: CuePhase;
  lamp: LampState;
  title: string;
  /** 改行区切り。画面では行ごとに出す */
  detail: string;
  /** 段階表示で今いる位置 (0〜4)。表示しない場合は null */
  step: number | null;
  /** 入室せずに止まったときの重さ。警告として目立たせるか */
  tone: 'neutral' | 'warning' | 'success';
  /** 単発でこの回が終わっている（残り時間を出さない） */
  ended?: boolean;
}

/** バックエンドの詳細文は日本語のときだけ出す（英語の生メッセージや内部値を出さない）。 */
function readableDetail(reason: StopReason): string | null {
  const detail = stopReasonDetail(reason)?.trim();
  if (!detail || !/[ぁ-んァ-ヶ一-龠]/.test(detail)) return null;
  // 日本語の案内に、HTTP の状態や JSON、英文がそのまま混ざっているものは出さない
  if (/HTTP\s*\d{3}|[{}<>]|\b[A-Za-z]+(?:\s+[A-Za-z]+){2,}\b/.test(detail)) return null;
  return detail;
}

function withDetail(reason: StopReason, ...lines: string[]): string {
  const detail = readableDetail(reason);
  return (detail ? [detail, ...lines] : lines).join('\n');
}

function stoppedView(reason: StopReason): CueView {
  const base = { phase: 'stopped' as const, lamp: 'off' as const, step: null, tone: 'warning' as const };
  switch (stopReasonKind(reason)) {
    case 'JoinConfirmed':
      return { phase: 'done', lamp: 'go', step: 4, tone: 'success', title: '入室しました', detail: '入室を確認して、監視を終えました。' };
    case 'LaunchSentUnconfirmed':
      return {
        ...base,
        title: '入室を確認できませんでした',
        detail:
          'VRChat に参加を要求しましたが、期限内に入室を確認できませんでした。\nVRChat の画面で、会場に入れているか確かめてください。\n（はじめからその会場にいた場合も、こう表示されます）',
      };
    case 'MonitorExpired':
      return { ...base, tone: 'neutral', title: '監視の時間が終わりました', detail: '時間内に入れる会場は見つかりませんでした。' };
    case 'InstanceFull':
      return { ...base, title: '会場が満員のままでした', detail: '監視の時間内に空きが出ませんでした。' };
    case 'AmbiguousTarget':
      return {
        ...base,
        title: '入る会場を決められませんでした',
        detail: withDetail(reason, 'イベントの設定で「優先する会場名」を指定すると、自動で選べます。'),
      };
    case 'AuthInvalid':
      return {
        ...base,
        title: 'ログインの有効期限が切れました',
        detail: '設定画面でログアウトし、もう一度ログインしてください。',
      };
    case 'RateLimit':
      return {
        ...base,
        title: 'VRChat への問い合わせが制限されました',
        detail: '問い合わせが多すぎたため、監視を止めました。\nしばらく待ってから、もう一度始めてください。',
      };
    case 'PreferredNameUnverifiable':
      return {
        ...base,
        title: '優先する会場名を確認できませんでした',
        detail: '会場名を取得できなかったため、監視を止めました。\nイベントの設定で会場名を見直してください。',
      };
    case 'LaunchFailed':
      return {
        ...base,
        title: 'VRChat を起動できませんでした',
        detail: withDetail(reason, 'VRChat がこの PC にインストールされているか確かめてください。'),
      };
    case 'ApiError':
      return {
        ...base,
        title: 'VRChat との通信に失敗しました',
        detail: withDetail(reason, '時間をおいて、もう一度始めてください。'),
      };
    case 'UnrecoverableFailure':
      return {
        ...base,
        title: '監視を続けられなくなりました',
        detail: withDetail(reason, 'ログを確認して、もう一度始めてください。'),
      };
    default:
      return { ...base, title: '監視が止まりました', detail: 'ログを確認してください。' };
  }
}

export interface CueInput {
  state: MonitorState | undefined;
  stopReason: StopReason | null;
  pendingTarget: PendingTarget | null;
  monitorStart: Date | null;
  eventStart: Date | null;
  /** 単発で、この回がもう終わっているか */
  expired: boolean;
  /** この回は止めたなどの理由で、自動では始めない */
  skipAuto: boolean;
  /** 監視中に会場は見えているが、条件に合わないなどで待っている */
  seenCandidates?: boolean;
}

export function describeCue({ state, stopReason, pendingTarget, monitorStart, expired, skipAuto, seenCandidates }: CueInput): CueView {
  const reason = stopReason ?? (state && typeof state === 'object' ? state.Stopped : null);

  if (!state || state === 'Idle' || typeof state === 'object') {
    // 手動停止は自明なので結果として出さず、未監視として扱う
    if (reason && stopReasonKind(reason) !== 'ManualStop') return stoppedView(reason);
    if (expired) {
      return {
        phase: 'idle',
        lamp: 'off',
        step: null,
        tone: 'neutral',
        ended: true,
        title: 'この回は終わっています',
        detail: '単発のイベントです。\nイベント画面で、次の日時に直してください。',
      };
    }
    if (skipAuto) {
      return {
        phase: 'idle',
        lamp: 'off',
        step: null,
        tone: 'neutral',
        title: 'この回は監視しません',
        detail: '監視を止めたため、この回は自動では始めません。\n「今すぐ監視を始める」で再開できます。',
      };
    }
    return {
      phase: 'idle',
      lamp: 'off',
      step: null,
      tone: 'neutral',
      title: '次の回を待っています',
      detail: monitorStart
        ? `${formatMonthDayHm(monitorStart)} になったら、自動で監視を始めます。\nアプリは開いたままにしてください。`
        : '時間になったら、自動で監視を始めます。\nアプリは開いたままにしてください。',
    };
  }

  switch (state) {
    case 'WaitingForWindow':
      return {
        phase: 'waiting',
        lamp: 'standby',
        step: 0,
        tone: 'neutral',
        title: '監視の開始を待っています',
        detail: monitorStart
          ? `${formatHm(monitorStart)} になったら、自動で監視を始めます。\nアプリは開いたままにしてください。`
          : '時間になったら、自動で監視を始めます。\nアプリは開いたままにしてください。',
      };
    case 'Watching':
      return {
        phase: 'watching',
        lamp: 'standby-live',
        step: 1,
        tone: 'neutral',
        title: '監視しています',
        detail: seenCandidates
          ? '条件に合う会場を待っています。\n下の一覧から選んで入ることもできます。'
          : '会場が開くのを待っています。\nこのままお待ちください。',
      };
    case 'AwaitingInstanceSelection':
      return {
        phase: 'choose',
        lamp: 'standby-blink',
        step: 2,
        tone: 'neutral',
        title: '入る会場を選んでください',
        detail: '会場が複数あります。\n選ぶまで、自動では参加しません。',
      };
    case 'CandidateDetected':
      return {
        phase: 'go',
        lamp: 'go',
        step: 2,
        tone: 'neutral',
        title: '会場が見つかりました',
        detail: 'VRChat で参加する準備をしています。',
      };
    case 'LaunchDispatchRequested':
      return {
        phase: 'go',
        lamp: 'go',
        step: 3,
        tone: 'neutral',
        title: 'VRChat に参加を要求しています',
        detail: 'まもなく VRChat が会場を開きます。',
      };
    case 'JoinPendingObservation': {
      const name = pendingTarget?.displayName || '会場';
      const until = pendingTarget ? new Date(pendingTarget.confirmUntil) : null;
      return {
        phase: 'confirm',
        lamp: 'go-blink',
        step: 4,
        tone: 'neutral',
        title: '入室を確認しています',
        detail:
          until && !Number.isNaN(until.getTime())
            ? `${name}への参加を要求しました。\n${formatHms(until)} まで入室を確認します。`
            : `${name}への参加を要求しました。`,
      };
    }
    case 'Travelling':
      return {
        phase: 'confirm',
        lamp: 'go-blink',
        step: 4,
        tone: 'neutral',
        title: '会場へ移動しています',
        detail: 'VRChat が会場へ移動しています。',
      };
    case 'QueueWaiting':
      return {
        phase: 'confirm',
        lamp: 'go-blink',
        step: 4,
        tone: 'neutral',
        title: 'キューに並んでいます',
        detail: '順番が来るまで待っています。',
      };
  }
}

/** 監視中（止める操作が必要な状態）か */
export function isPhaseActive(phase: CuePhase): boolean {
  return phase === 'waiting' || phase === 'watching' || phase === 'choose' || phase === 'go' || phase === 'confirm';
}
