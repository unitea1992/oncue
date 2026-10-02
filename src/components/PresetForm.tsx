import { useEffect, useState } from 'react';
import { CalendarDays, Search } from 'lucide-react';
import {
  api,
  formatInvokeError,
  type EventSchedule,
  type GroupCalendarEventSummary,
  type Preset,
} from '../lib/tauri';
import {
  anchorMatchesWeekday,
  createBiweeklySchedule,
  createWeeklySchedule,
  dailyToTimeInput,
  isExpiredOnce,
  nextWeeklyOccurrenceUtc,
  normalizeDailyTime,
  onceFromLocalInput,
  onceToLocalInput,
  parseAnchorDate,
} from '../lib/schedule';
import { formatMonthDayHm, formatSchedule } from '../lib/format';
import { NativeDialog } from './ConfirmDialog';

interface PresetFormProps {
  preset?: Preset | null;
  /** trueで保存成功。false/例外は入力を保持したまま開き続ける。 */
  onSave: (preset: Omit<Preset, 'id'>) => Promise<boolean>;
  onCancel: () => void;
}

const GROUP_ID_RE = /^grp_[a-f0-9-]+$/i;

const WEEKDAY_OPTIONS: Array<{ value: number; label: string }> = [
  { value: 0, label: '日曜' },
  { value: 1, label: '月曜' },
  { value: 2, label: '火曜' },
  { value: 3, label: '水曜' },
  { value: 4, label: '木曜' },
  { value: 5, label: '金曜' },
  { value: 6, label: '土曜' },
];

type ScheduleKind = 'once' | 'weekly' | 'biweekly';
type LoadState = 'idle' | 'loading' | 'success' | 'error';

function initialKind(preset: Preset | null | undefined): ScheduleKind {
  const kind = preset?.schedule.kind;
  if (kind === 'weekly' || kind === 'biweekly') return kind;
  return 'once';
}

/** aria-describedby 用。空の要素を除いて結合する */
function describedBy(...ids: Array<string | false | null | undefined>): string | undefined {
  const joined = ids.filter(Boolean).join(' ');
  return joined || undefined;
}

export function PresetForm({ preset, onSave, onCancel }: PresetFormProps) {
  const [label, setLabel] = useState(preset?.label || '');
  const [groupId, setGroupId] = useState(preset?.groupId || '');
  const [groupName, setGroupName] = useState(preset?.groupName || '');
  const [kind, setKind] = useState<ScheduleKind>(() => initialKind(preset));
  const [onceLocal, setOnceLocal] = useState(
    preset?.schedule.kind === 'once' ? onceToLocalInput(preset.schedule.startsAt) : '',
  );
  const [weeklyTime, setWeeklyTime] = useState(() => {
    const schedule = preset?.schedule;
    if (schedule?.kind === 'weekly' || schedule?.kind === 'biweekly') {
      return dailyToTimeInput(schedule.time);
    }
    return '';
  });
  const [weekday, setWeekday] = useState<number | null>(() => {
    const schedule = preset?.schedule;
    return schedule?.kind === 'weekly' || schedule?.kind === 'biweekly' ? schedule.weekday : null;
  });
  const [anchorDate, setAnchorDate] = useState(
    preset?.schedule.kind === 'biweekly' ? preset.schedule.anchorDate : '',
  );
  const [sourceEventId, setSourceEventId] = useState(preset?.sourceEventId || '');
  const [preferredName, setPreferredName] = useState(preset?.preferredInstanceName || '');
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [lookupState, setLookupState] = useState<LoadState>('idle');
  const [lookupMessage, setLookupMessage] = useState('');
  const [calendarEvents, setCalendarEvents] = useState<GroupCalendarEventSummary[]>([]);
  const [calendarState, setCalendarState] = useState<LoadState>('idle');
  const [calendarMessage, setCalendarMessage] = useState('');
  const groupIdValid = GROUP_ID_RE.test(groupId.trim());

  useEffect(() => {
    if (!GROUP_ID_RE.test(groupId.trim())) {
      setLookupState('idle');
      setLookupMessage('');
      setCalendarEvents([]);
      setCalendarState('idle');
      setCalendarMessage('');
      if (!groupId.trim()) setGroupName('');
    }
  }, [groupId]);

  const previewSchedule: EventSchedule | null =
    kind === 'once'
      ? (() => {
          const startsAt = onceFromLocalInput(onceLocal);
          return startsAt ? { kind: 'once' as const, startsAt } : null;
        })()
      : kind === 'weekly'
        ? (weekday === null ? null : createWeeklySchedule(weekday, weeklyTime))
        : (weekday === null || !anchorDate ? null : createBiweeklySchedule(weekday, weeklyTime, anchorDate));

  const previewNext =
    previewSchedule && (previewSchedule.kind === 'weekly' || previewSchedule.kind === 'biweekly')
      ? nextWeeklyOccurrenceUtc(
          previewSchedule.weekday,
          previewSchedule.time,
          previewSchedule.kind === 'weekly' ? 1 : 2,
          previewSchedule.kind === 'biweekly' ? previewSchedule.anchorDate : null,
        )
      : null;
  const previewExpired = previewSchedule?.kind === 'once' && isExpiredOnce(previewSchedule.startsAt);

  const handleLookupGroup = async () => {
    const trimmed = groupId.trim();
    if (!GROUP_ID_RE.test(trimmed)) return;
    try {
      setLookupState('loading');
      setLookupMessage('グループを確認しています…');
      const summary = await api.presets.lookupGroupSummary(trimmed);
      setGroupName(summary.name);
      setLookupState('success');
      setLookupMessage(`「${summary.name}」が見つかりました。`);
      if (!label.trim()) setLabel(summary.name);
    } catch (err) {
      setLookupState('error');
      setLookupMessage(formatInvokeError(err, 'グループを確認できませんでした。\nID が正しいか確かめてください。'));
    }
  };

  const handleLoadCalendarEvents = async () => {
    const trimmed = groupId.trim();
    if (!GROUP_ID_RE.test(trimmed)) return;
    try {
      setCalendarState('loading');
      setCalendarMessage('カレンダーを読み込んでいます…');
      const events = await api.presets.getGroupCalendarEvents(trimmed);
      setCalendarEvents(events);
      setCalendarState('success');
      setCalendarMessage(
        events.length === 0
          ? '近いうちのイベントは見つかりませんでした。\n日時を下に直接入力してください。'
          : '選ぶと、イベント名と日時が入ります。',
      );
    } catch (err) {
      setCalendarState('error');
      setCalendarEvents([]);
      setCalendarMessage(formatInvokeError(err, 'カレンダーを読み込めませんでした。\n日時を下に直接入力してください。'));
    }
  };

  const applyCalendarEvent = (event: GroupCalendarEventSummary) => {
    setOnceLocal(onceToLocalInput(event.startsAt));
    setKind('once');
    setSourceEventId(event.id);
    if (!label.trim() || label === groupName) setLabel(event.title);
  };

  const validate = () => {
    const next: Record<string, string> = {};
    if (!label.trim()) next.label = 'イベント名を入力してください。';
    if (!groupId.trim()) {
      next.groupId = 'グループ ID を入力してください。';
    } else if (!GROUP_ID_RE.test(groupId.trim())) {
      next.groupId = 'グループ ID の形式が違います。\n「grp_」から始まる ID を入れてください。';
    }
    if (kind === 'once') {
      if (!onceLocal) {
        next.schedule = '日時を入力してください。';
      } else if (!onceFromLocalInput(onceLocal)) {
        next.schedule = '存在する日時を入力してください。';
      }
    } else {
      if (weekday === null) next.weekday = '曜日を選んでください。';
      if (!normalizeDailyTime(weeklyTime)) {
        next.schedule = '時刻を入力してください。';
      } else if (kind === 'biweekly') {
        if (!anchorDate) {
          next.schedule = '次回の開催日を入力してください。';
        } else if (!parseAnchorDate(anchorDate)) {
          next.schedule = '存在する日付を入力してください。';
        } else if (weekday !== null && !anchorMatchesWeekday(anchorDate, weekday)) {
          next.schedule = '次回の開催日は、選んだ曜日と同じ曜日にしてください。';
        }
      }
    }
    setErrors(next);
    return Object.keys(next).length === 0;
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!validate()) return;
    // validate済みのため生成は成功する。
    const schedule: EventSchedule =
      kind === 'once'
        ? { kind: 'once', startsAt: onceFromLocalInput(onceLocal)! }
        : kind === 'weekly'
          ? createWeeklySchedule(weekday ?? -1, weeklyTime)!
          : createBiweeklySchedule(weekday ?? -1, weeklyTime, anchorDate)!;
    setSaving(true);
    setSaveError(null);
    try {
      // 失敗時は閉じず入力を保持する。閉じる判断は親の戻り値で行う。
      const ok = await onSave({
        label: label.trim(),
        groupId: groupId.trim(),
        groupName: groupName || null,
        schedule,
        sourceEventId: sourceEventId.trim() || null,
        preferredInstanceName: preferredName.trim() || null,
      });
      if (!ok) setSaveError('保存できませんでした。\n入力内容はそのまま残っています。もう一度保存してください。');
    } catch (err) {
      setSaveError(formatInvokeError(err, '保存できませんでした。\n入力内容はそのまま残っています。もう一度保存してください。'));
    } finally {
      setSaving(false);
    }
  };

  return (
    <NativeDialog open title={preset ? 'イベントを編集' : 'イベントを追加'} onClose={saving ? () => {} : onCancel}>
      <form onSubmit={handleSubmit} noValidate>
        <div className="dialog-body">
          <div className="field">
            <label className="field-label" htmlFor="event-group">
              グループ ID
            </label>
            <div className="field-row">
              <input
                id="event-group"
                value={groupId}
                onChange={(e) => setGroupId(e.target.value)}
                placeholder="grp_xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
                spellCheck={false}
                aria-invalid={!!errors.groupId}
                aria-describedby={describedBy('event-group-hint', errors.groupId && 'event-group-error', lookupMessage && 'event-group-status')}
              />
              <button type="button" onClick={handleLookupGroup} disabled={!groupIdValid || lookupState === 'loading'}>
                <Search size={16} aria-hidden />
                {lookupState === 'loading' ? '確認しています…' : 'グループ名を確認'}
              </button>
            </div>
            <p id="event-group-hint" className="hint">
              VRChat のグループページの URL にある「grp_」から始まる文字列です。
            </p>
            {errors.groupId && (
              <p id="event-group-error" role="alert" className="error-message">
                {errors.groupId}
              </p>
            )}
            {lookupMessage && (
              <p
                id="event-group-status"
                role="status"
                className={lookupState === 'error' ? 'error-message' : 'hint field-ok'}
              >
                {lookupMessage}
              </p>
            )}
          </div>

          <div className="field">
            <span className="field-label" id="event-calendar-label">
              カレンダーから選ぶ<span className="optional">任意</span>
            </span>
            <div className="field-row">
              <button
                type="button"
                onClick={handleLoadCalendarEvents}
                disabled={!groupIdValid || calendarState === 'loading'}
                aria-describedby="event-calendar-status"
              >
                <CalendarDays size={16} aria-hidden />
                {calendarState === 'loading' ? '読み込んでいます…' : 'グループのカレンダーを読み込む'}
              </button>
            </div>
            <p
              id="event-calendar-status"
              role="status"
              className={calendarState === 'error' ? 'error-message' : 'hint'}
            >
              {calendarMessage || (groupIdValid ? '単発のイベントなら、ここから選ぶのが確実です。' : 'グループ ID を入れると使えます。')}
            </p>
            {calendarEvents.length > 0 && (
              <div className="calendar-options" role="radiogroup" aria-labelledby="event-calendar-label">
                {calendarEvents.map((event) => {
                  const startsAt = new Date(event.startsAt);
                  return (
                    <label key={event.id} className="calendar-option">
                      <input
                        type="radio"
                        name="calendar-event"
                        checked={sourceEventId === event.id}
                        onChange={() => applyCalendarEvent(event)}
                      />
                      <span className="calendar-option-title">{event.title}</span>
                      <span className="calendar-option-date num">
                        {Number.isNaN(startsAt.getTime()) ? '日時未定' : formatMonthDayHm(startsAt)}
                      </span>
                    </label>
                  );
                })}
              </div>
            )}
          </div>

          <div className="field">
            <label className="field-label" htmlFor="event-label">
              イベント名
            </label>
            <input
              id="event-label"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="例: 金曜のクラブイベント"
              aria-invalid={!!errors.label}
              aria-describedby={errors.label ? 'event-label-error' : undefined}
            />
            {errors.label && (
              <p id="event-label-error" role="alert" className="error-message">
                {errors.label}
              </p>
            )}
          </div>

          <fieldset className="field schedule-field">
            <legend className="field-label">開催日時（日本時間）</legend>
            <div className="segmented" role="group" aria-label="開催の頻度" data-testid="schedule-type">
              {(
                [
                  ['once', '単発'],
                  ['weekly', '毎週'],
                  ['biweekly', '隔週'],
                ] as const
              ).map(([value, text]) => (
                <button key={value} type="button" aria-pressed={kind === value} onClick={() => setKind(value)}>
                  {text}
                </button>
              ))}
            </div>
            {kind === 'once' ? (
              <input
                type="datetime-local"
                value={onceLocal}
                onChange={(e) => setOnceLocal(e.target.value)}
                aria-label="開催日時 (日本時間)"
                aria-invalid={!!errors.schedule}
                aria-describedby={describedBy(errors.schedule && 'event-schedule-error', 'event-schedule-preview')}
              />
            ) : (
              <div className="schedule-row">
                <div className="field">
                  <label className="field-label" htmlFor="event-weekday">
                    曜日
                  </label>
                  <select
                    id="event-weekday"
                    value={weekday === null ? '' : weekday}
                    onChange={(e) => setWeekday(e.target.value === '' ? null : Number(e.target.value))}
                    aria-invalid={!!errors.weekday}
                    aria-describedby={errors.weekday ? 'event-weekday-error' : undefined}
                  >
                    <option value="">選んでください</option>
                    {WEEKDAY_OPTIONS.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="field">
                  <label className="field-label" htmlFor="event-time">
                    時刻
                  </label>
                  <input
                    id="event-time"
                    type="time"
                    value={weeklyTime}
                    onChange={(e) => setWeeklyTime(e.target.value)}
                    aria-invalid={!!errors.schedule}
                    aria-describedby={describedBy(errors.schedule && 'event-schedule-error', 'event-schedule-preview')}
                  />
                </div>
                {kind === 'biweekly' && (
                  <div className="field">
                    <label className="field-label" htmlFor="event-next-date">
                      次回の開催日
                    </label>
                    <input
                      id="event-next-date"
                      type="date"
                      value={anchorDate}
                      onChange={(e) => setAnchorDate(e.target.value)}
                      aria-invalid={!!errors.schedule}
                      aria-describedby={describedBy('event-next-date-hint', errors.schedule && 'event-schedule-error')}
                    />
                  </div>
                )}
              </div>
            )}
            {kind === 'biweekly' && (
              <p id="event-next-date-hint" className="hint">
                隔週の数え始めになる日です。選んだ曜日と同じ曜日にしてください。
              </p>
            )}
            {errors.weekday && (
              <p id="event-weekday-error" role="alert" className="error-message">
                {errors.weekday}
              </p>
            )}
            {errors.schedule && (
              <p id="event-schedule-error" role="alert" className="error-message">
                {errors.schedule}
              </p>
            )}
            {previewSchedule && (
              <p id="event-schedule-preview" role="status" className={previewExpired ? 'error-message' : 'hint field-ok'}>
                {formatSchedule(previewSchedule)}
                {previewNext ? `（次回 ${formatMonthDayHm(previewNext)}）` : ''}
                {previewExpired ? '\nこの日時はもう過ぎています。' : ''}
              </p>
            )}
          </fieldset>

          <div className="field">
            <label className="field-label" htmlFor="event-instance">
              優先する会場名<span className="optional">任意</span>
            </label>
            <input
              id="event-instance"
              value={preferredName}
              onChange={(e) => setPreferredName(e.target.value)}
              placeholder="例: Event Hall"
              aria-describedby="event-instance-hint"
            />
            <p id="event-instance-hint" className="hint">
              入れると、名前にこの語を含む会場だけに入ります。
              <br />
              大文字と小文字、全角と半角は区別しません。
            </p>
          </div>

          {saveError && (
            <p role="alert" className="error-message">
              {saveError}
            </p>
          )}
        </div>

        <div className="dialog-footer">
          <button type="button" onClick={onCancel} disabled={saving}>
            キャンセル
          </button>
          <button type="submit" className="primary" disabled={saving}>
            {saving ? '保存しています…' : '保存'}
          </button>
        </div>
      </form>
    </NativeDialog>
  );
}
