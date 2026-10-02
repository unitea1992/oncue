import type { ReactNode } from 'react';
import { CUE_STEPS, isPhaseActive, type CueView } from '../lib/cue';
import { formatCountdown, formatElapsed, formatHm, spokenDuration } from '../lib/format';

interface CueWindow {
  monitorStart: Date;
  eventStart: Date;
  monitorEnd: Date;
}

interface CuePanelProps {
  view: CueView;
  now: Date;
  window: CueWindow | null;
  compact?: boolean;
  /** タイトル行の下に差し込む補足（イベント名など） */
  caption?: ReactNode;
}

interface Count {
  value: string;
  label: string;
  spoken: string;
}

function countFor(view: CueView, now: Date, w: CueWindow | null): Count | null {
  if (!w) return null;
  const t = now.getTime();
  const toStart = w.monitorStart.getTime() - t;
  const toEvent = w.eventStart.getTime() - t;
  switch (view.phase) {
    case 'done':
      return { value: formatHm(w.eventStart), label: 'の回に入室', spoken: `${formatHm(w.eventStart)} の回に入室` };
    case 'stopped':
      return null;
    case 'idle':
      if (view.ended) return null;
      if (toStart > 0) return { value: formatCountdown(toStart), label: '監視開始まで', spoken: `監視開始まで ${spokenDuration(toStart)}` };
      break;
    case 'waiting':
      return { value: formatCountdown(toStart), label: '監視開始まで', spoken: `監視開始まで ${spokenDuration(toStart)}` };
  }
  if (toEvent > 0) return { value: formatCountdown(toEvent), label: 'イベント開始まで', spoken: `イベント開始まで ${spokenDuration(toEvent)}` };
  return { value: formatElapsed(-toEvent), label: '開始から', spoken: `開始から ${spokenDuration(-toEvent)}` };
}

function percent(w: CueWindow, at: Date): number {
  const span = w.monitorEnd.getTime() - w.monitorStart.getTime();
  if (span <= 0) return 0;
  return Math.min(100, Math.max(0, ((at.getTime() - w.monitorStart.getTime()) / span) * 100));
}

export function CuePanel({ view, now, window: w, compact = false, caption }: CuePanelProps) {
  const count = countFor(view, now, w);
  const inWindow = !!w && now >= w.monitorStart && now <= w.monitorEnd;
  const showTrack = !!w && !compact && (isPhaseActive(view.phase) || (view.phase === 'idle' && inWindow));
  const golit = view.lamp.startsWith('go');
  const tone = golit ? ' is-go' : view.phase === 'idle' ? ' is-idle' : '';
  const nowPct = w ? percent(w, now) : 0;
  const eventPct = w ? percent(w, w.eventStart) : 0;

  return (
    <section
      className={`cue${compact ? ' cue-compact' : ''}`}
      aria-labelledby="cue-title"
      data-testid="cue-panel"
      data-phase={view.phase}
      data-tone={view.tone}
    >
      <div className={`lamp lamp-${view.lamp}`} aria-hidden />
      <div className="cue-text">
        <h2 id="cue-title" className="cue-title" data-testid="cue-title">
          {view.title}
        </h2>
        {caption}
        <p className="cue-detail">{view.detail}</p>
      </div>
      {count && (
        <div className={`cue-count${golit ? ' is-go' : ''}${view.phase === 'idle' ? ' is-idle' : ''}`}>
          <span className="cue-count-value num" aria-hidden>
            {count.value}
          </span>
          <span className="cue-count-label" aria-hidden>
            {count.label}
          </span>
          {/* 毎秒の変化は読み上げない。読み上げソフトでは移動したときに現在値を読める */}
          <span className="sr-only">{count.spoken}</span>
        </div>
      )}
      {showTrack && w && (
        <div className="cue-track" aria-hidden>
          <div className="cue-track-bar" />
          <div className={`cue-track-fill${tone}`} style={{ width: `${nowPct}%` }} />
          <div className="cue-tick" style={{ left: 0 }} />
          <div className="cue-tick is-event" style={{ left: `${eventPct}%` }} />
          <div className="cue-tick" style={{ left: '100%' }} />
          <div className={`cue-now${tone}`} style={{ left: `${nowPct}%` }} />
          <span className="cue-mark is-start">
            <b className="num">{formatHm(w.monitorStart)}</b> 監視開始
          </span>
          <span className="cue-mark" style={{ left: `${eventPct}%` }}>
            <b className="num">{formatHm(w.eventStart)}</b> イベント開始
          </span>
          <span className="cue-mark is-end">
            <b className="num">{formatHm(w.monitorEnd)}</b> 監視終了
          </span>
        </div>
      )}
      <p className="sr-only" role="status">
        {view.title}
      </p>
    </section>
  );
}

/** 舞台の進行表にならった段階表示 Q1〜Q5 */
export function CueSteps({ step, done }: { step: number; done: boolean }) {
  return (
    <ol className={`cue-steps${done ? ' is-done' : ''}`} aria-label="進み具合">
      {CUE_STEPS.map((label, i) => {
        const state = done || i < step ? 'past' : i === step ? 'current' : 'future';
        return (
          <li key={label} className={`is-${state}`} aria-current={state === 'current' && !done ? 'step' : undefined}>
            <span className="cue-step-no num" aria-hidden>
              Q{i + 1}
            </span>
            {label}
          </li>
        );
      })}
    </ol>
  );
}
