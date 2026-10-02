import type { EventSchedule, Preset } from './tauri';
import { JST_OFFSET_MS, MONITOR_END_GRACE_MS, nextWeeklyOccurrenceUtc, normalizeDailyTime } from './schedule';

const WEEKDAY = ['日', '月', '火', '水', '木', '金', '土'];
const pad = (n: number) => String(n).padStart(2, '0');

function jst(date: Date) {
  return new Date(date.getTime() + JST_OFFSET_MS);
}

/** JST の「21:00」 */
export function formatHm(date: Date): string {
  const d = jst(date);
  return `${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}`;
}

/** JST の「21:00:06」 */
export function formatHms(date: Date): string {
  const d = jst(date);
  return `${formatHm(date)}:${pad(d.getUTCSeconds())}`;
}

/** JST の「10/9(金)」 */
export function formatMonthDay(date: Date): string {
  const d = jst(date);
  return `${d.getUTCMonth() + 1}/${d.getUTCDate()}(${WEEKDAY[d.getUTCDay()]})`;
}

/** JST の「10/9(金) 21:00」 */
export function formatMonthDayHm(date: Date): string {
  return `${formatMonthDay(date)} ${formatHm(date)}`;
}

/** 予定の短い表示。「毎週金曜 21:00」「隔週日曜 22:30」「10/5(月) 22:00」 */
export function formatSchedule(schedule: EventSchedule): string {
  if (schedule.kind === 'weekly' || schedule.kind === 'biweekly') {
    const prefix = schedule.kind === 'weekly' ? '毎週' : '隔週';
    return `${prefix}${WEEKDAY[schedule.weekday]}曜 ${normalizeDailyTime(schedule.time) ?? schedule.time}`;
  }
  const date = new Date(schedule.startsAt);
  return Number.isNaN(date.getTime()) ? '日時未定' : formatMonthDayHm(date);
}

/** 曜日の名前。「金曜」 */
export function weekdayName(weekday: number): string {
  return `${WEEKDAY[weekday] ?? '?'}曜`;
}

/** 次に来る開催日時（UTC瞬間）。単発で終了済みなら null。計算の正本は Rust 側。 */
export function nextOccurrence(preset: Preset, now: Date = new Date()): Date | null {
  const s = preset.schedule;
  if (s.kind === 'once') {
    const date = new Date(s.startsAt);
    if (Number.isNaN(date.getTime())) return null;
    return date.getTime() + MONITOR_END_GRACE_MS > now.getTime() ? date : null;
  }
  return nextWeeklyOccurrenceUtc(
    s.weekday,
    s.time,
    s.kind === 'weekly' ? 1 : 2,
    s.kind === 'biweekly' ? s.anchorDate : null,
    now,
  );
}

/**
 * 残り時間の表示。数字部分は時刻書体で大きく出す。
 * 1時間未満は「02:41」、1日未満は「19:06:42」、それ以上は「3日」。
 */
export function formatCountdown(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const days = Math.floor(total / 86400);
  if (days >= 1) return `${days}日`;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

/** 経過時間の表示。「+00:12」 */
export function formatElapsed(ms: number): string {
  return `+${formatCountdown(ms)}`;
}

/** 読み上げ用の残り時間。「2分41秒」 */
export function spokenDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const days = Math.floor(total / 86400);
  if (days >= 1) return `${days}日`;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}時間${m}分`;
  if (m > 0) return `${m}分${s}秒`;
  return `${s}秒`;
}
