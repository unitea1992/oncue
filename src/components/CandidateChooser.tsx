import { useId } from 'react';
import { AlertTriangle, RefreshCw } from 'lucide-react';
import { canCandidateQueue, isCandidateFull, isCandidateJoinable, type Candidate } from '../lib/tauri';

interface CandidateChooserProps {
  candidates: Candidate[];
  /** 単発でカレンダーから登録したイベントのID。一致する会場に印を付ける */
  calendarEventId: string | null;
  selectingLocation: string | null;
  notice: string | null;
  error: string | null;
  onSelect: (candidate: Candidate) => void;
  onRefresh: () => void;
}

function CandidateRow({
  candidate,
  calendarMatch,
  selecting,
  disabled,
  onSelect,
}: {
  candidate: Candidate;
  calendarMatch: boolean;
  selecting: boolean;
  disabled: boolean;
  onSelect: () => void;
}) {
  const nameId = useId();
  const name = candidate.displayName || '名前を取得できなかった会場';
  const joinable = isCandidateJoinable(candidate);
  const queue = canCandidateQueue(candidate);
  const full = isCandidateFull(candidate);

  let tag: { text: string; tone: string } | null = null;
  let note: string | null = null;
  if (joinable) {
    note = calendarMatch ? 'カレンダーのイベントと一致する会場です。' : '空きがあります。';
  } else if (queue) {
    tag = { text: '満員・キューあり', tone: 'queue' };
    note = candidate.queueSize != null ? `いまキューに ${candidate.queueSize} 人並んでいます。` : 'キューに並ぶと、順番が来たら入れます。';
  } else if (full) {
    tag = { text: '満員', tone: 'full' };
    note = '満員のため入れません。';
  } else {
    tag = { text: '状態不明', tone: 'full' };
    note = '入れるかどうか確認できないため、選べません。';
  }

  return (
    <li className={`candidate${joinable || queue ? '' : ' is-off'}`}>
      <div className="candidate-main">
        <span className="candidate-name" id={nameId}>
          {name}
          {tag && <span className={`tag tag-${tag.tone}`}>{tag.text}</span>}
        </span>
        <span className="hint">{note}</span>
      </div>
      <span className="candidate-count">
        <span className="num">{candidate.memberCount}</span>
        <span className="candidate-count-unit">人</span>
      </span>
      <button
        type="button"
        className={joinable ? 'primary' : ''}
        onClick={onSelect}
        disabled={disabled || !(joinable || queue)}
        aria-describedby={nameId}
      >
        {selecting ? '選んでいます…' : joinable ? 'この会場に入る' : queue ? 'キューに並ぶ' : '入れません'}
      </button>
    </li>
  );
}

export function CandidateChooser({
  candidates,
  calendarEventId,
  selectingLocation,
  notice,
  error,
  onSelect,
  onRefresh,
}: CandidateChooserProps) {
  if (candidates.length === 0) {
    return (
      <p className="hint" role="status">
        会場の一覧を確認しています…
      </p>
    );
  }
  return (
    <section className="candidates" data-testid="candidate-chooser" aria-label="入る会場の選択">
      {notice && (
        <p className="hint" role="status">
          {notice}
        </p>
      )}
      {error && (
        <div className="callout error" role="alert">
          <AlertTriangle size={16} aria-hidden />
          <div className="callout-body">
            <p className="error-message">{error}</p>
            <button type="button" onClick={onRefresh} disabled={selectingLocation !== null}>
              <RefreshCw size={14} aria-hidden />
              一覧を更新する
            </button>
          </div>
        </div>
      )}
      <ul className="candidate-list">
        {candidates.map((candidate) => (
          <CandidateRow
            key={candidate.location}
            candidate={candidate}
            calendarMatch={!!calendarEventId && candidate.calendarEntryId === calendarEventId}
            selecting={selectingLocation === candidate.location}
            disabled={selectingLocation !== null}
            onSelect={() => onSelect(candidate)}
          />
        ))}
      </ul>
    </section>
  );
}
