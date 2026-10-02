import type { EventSchedule } from './tauri';

/** JSTはDSTなしの固定+9:00。標準ライブラリのみで wall-clock 変換する。 */
export const JST_OFFSET_MS = 9 * 60 * 60 * 1000;

const DAILY_TIME_RE = /^([01]\d|2[0-3]):([0-5]\d)(?::([0-5]\d))?$/;

export function isDailyTime(value: string): boolean {
  return DAILY_TIME_RE.test(value.trim());
}

/** "HH:MM(:SS)" を "HH:MM" に正規化する。不正なら null。 */
export function normalizeDailyTime(value: string): string | null {
  const m = DAILY_TIME_RE.exec(value.trim());
  if (!m) return null;
  return `${m[1]}:${m[2]}`;
}

function jstParts(date: Date): {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
} {
  const jst = new Date(date.getTime() + JST_OFFSET_MS);
  return {
    year: jst.getUTCFullYear(),
    month: jst.getUTCMonth() + 1,
    day: jst.getUTCDate(),
    hour: jst.getUTCHours(),
    minute: jst.getUTCMinutes(),
  };
}


const DATE_FMT = new Intl.DateTimeFormat('ja-JP', {
  timeZone: 'Asia/Tokyo',
  month: 'numeric',
  day: 'numeric',
  hour: 'numeric',
  minute: '2-digit',
});

const DATE_FMT_WITH_YEAR = new Intl.DateTimeFormat('ja-JP', {
  timeZone: 'Asia/Tokyo',
  year: 'numeric',
  month: 'numeric',
  day: 'numeric',
  hour: 'numeric',
  minute: '2-digit',
});

/** UTC瞬間をJSTの「M月d日 H時mm分」形式にする。 */
export function formatJstDateTime(date: Date, reference: Date = new Date()): string {
  const sameYear =
    jstParts(date).year === jstParts(reference).year;
  return (sameYear ? DATE_FMT : DATE_FMT_WITH_YEAR).format(date);
}

function formatDailyTimeJa(time: string): string {
  const [h, m] = time.split(':').map(Number);
  return m === 0 ? `${h}時` : `${h}時${String(m).padStart(2, '0')}分`;
}

/** 毎週。weekdayは0=日曜〜6=土曜 (JS Date.getDayと同一)。時刻はJSTのHH:MM(:SS)。 */
export interface WeeklySchedule {
  kind: 'weekly';
  weekday: number;
  time: string;
}

/** 隔週。anchorDate (JST暦日YYYY-MM-DD) が周期の基準。必須。 */
export interface BiweeklySchedule {
  kind: 'biweekly';
  weekday: number;
  time: string;
  anchorDate: string;
}

/** 新規作成の予定。once/weekly/biweekly。 */
export type NewSchedule = { kind: 'once'; startsAt: string } | WeeklySchedule | BiweeklySchedule;

const WEEKDAY_JA = ['日', '月', '火', '水', '木', '金', '土'];

/** 毎週・隔週の利用者向け短縮表示。「毎週日曜 21時」「隔週日曜 21時」。 */
export function formatWeeklyScheduleJa(schedule: WeeklySchedule | BiweeklySchedule): string {
  const prefix = schedule.kind === 'weekly' ? '毎週' : '隔週';
  const normalized = normalizeDailyTime(schedule.time);
  return `${prefix}${WEEKDAY_JA[schedule.weekday]}曜 ${formatDailyTimeJa(normalized ?? schedule.time)}`;
}

/** 予定の利用者向け短縮表示。「毎週日曜 21時」「12月24日 19時」。 */
export function formatScheduleJa(
  schedule: EventSchedule | WeeklySchedule | BiweeklySchedule,
): string {
  if (schedule.kind === 'weekly' || schedule.kind === 'biweekly') {
    return formatWeeklyScheduleJa(schedule);
  }
  const date = new Date(schedule.startsAt);
  if (Number.isNaN(date.getTime())) return '日時未定';
  return formatJstDateTime(date);
}

/** Rust正本の監視終了境界 (開始+2分)。単発の開始可否はこの到達で判定し、翌日に繰り越さない。 */
export const MONITOR_END_GRACE_MS = 2 * 60 * 1000;

/** 監視終了(開始+2分)到達で期限切れ。IPC解決値がある側はそちらを優先する。 */
export function isExpiredOnce(startsAt: string, now: Date = new Date()): boolean {
  const date = new Date(startsAt);
  if (Number.isNaN(date.getTime())) return false;
  return date.getTime() + MONITOR_END_GRACE_MS <= now.getTime();
}

/** IPC解決の監視終了時刻による期限切れ判定。不正値は期限切れ扱いしない。 */
export function isExpiredByMonitorEnd(monitorEnd: string, now: Date = new Date()): boolean {
  const end = new Date(monitorEnd);
  if (Number.isNaN(end.getTime())) return false;
  return end.getTime() <= now.getTime();
}

function pad(n: number): string {
  return String(n).padStart(2, '0');
}

/** UTC RFC3339 → datetime-local 入力値 (JST wall-clock)。秒以下は切り捨て。 */
export function onceToLocalInput(startsAt: string): string {
  const date = new Date(startsAt);
  if (Number.isNaN(date.getTime())) return '';
  const jst = new Date(date.getTime() + JST_OFFSET_MS);
  return `${jst.getUTCFullYear()}-${pad(jst.getUTCMonth() + 1)}-${pad(jst.getUTCDate())}T${pad(jst.getUTCHours())}:${pad(jst.getUTCMinutes())}`;
}

const LOCAL_INPUT_RE = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/;

/** datetime-local 入力値 (JST) → UTC RFC3339。不正なら null。日付を推測して補わない。 */
export function onceFromLocalInput(local: string): string | null {
  const m = LOCAL_INPUT_RE.exec(local.trim());
  if (!m) return null;
  const [, y, mo, d, h, mi] = m.map(Number);
  if (mo < 1 || mo > 12 || d < 1 || d > 31 || h > 23 || mi > 59) return null;
  const ms = Date.UTC(y, mo - 1, d, h, mi, 0, 0) - JST_OFFSET_MS;
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return null;
  // 存在しない日付 (例: 2月30日) の繰り上がりを検出する
  const back = new Date(date.getTime() + JST_OFFSET_MS);
  if (
    back.getUTCFullYear() !== y ||
    back.getUTCMonth() + 1 !== mo ||
    back.getUTCDate() !== d
  ) {
    return null;
  }
  return date.toISOString();
}

/** daily "HH:MM" → time 入力値。不正はそのまま返さず空にする。 */
export function dailyToTimeInput(time: string): string {
  return normalizeDailyTime(time) ?? '';
}

/** 曜日 (0=日曜〜6=土曜) の検証。 */
export function isWeekday(value: unknown): value is number {
  return typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 6;
}

const ANCHOR_DATE_RE = /^(\d{4})-(\d{2})-(\d{2})$/;

/** anchor日付 (YYYY-MM-DD) の検証。存在しない日付の繰り上がりを認めない。 */
export function parseAnchorDate(value: string): { y: number; mo: number; d: number } | null {
  const m = ANCHOR_DATE_RE.exec(value.trim());
  if (!m) return null;
  const y = Number(m[1]);
  const mo = Number(m[2]);
  const d = Number(m[3]);
  if (mo < 1 || mo > 12 || d < 1 || d > 31) return null;
  const ms = Date.UTC(y, mo - 1, d);
  const back = new Date(ms);
  if (back.getUTCFullYear() !== y || back.getUTCMonth() + 1 !== mo || back.getUTCDate() !== d) {
    return null;
  }
  return { y, mo, d };
}

/** anchor日付の曜日が指定曜日と一致するか。 */
export function anchorMatchesWeekday(anchorDate: string, weekday: number): boolean {
  const parsed = parseAnchorDate(anchorDate);
  if (!parsed || !isWeekday(weekday)) return false;
  return new Date(Date.UTC(parsed.y, parsed.mo - 1, parsed.d)).getUTCDay() === weekday;
}

/** weekly/biweeklyの次回JST発生をUTC瞬間として返す。計算正本はRust側で、TSは表示・入力変換用。
 * intervalWeeksは1 (毎週) か2 (隔週)。隔週はanchorDate必須で、曜日不一致は推測せずnull。
 * 当回+2分猶予 (MONITOR_END_GRACE_MS) を過ぎていれば次回へ進める。 */
export function nextWeeklyOccurrenceUtc(
  weekday: number,
  time: string,
  intervalWeeks: 1 | 2,
  anchorDate: string | null,
  now: Date = new Date(),
): Date | null {
  if (!isWeekday(weekday)) return null;
  const normalized = normalizeDailyTime(time);
  if (!normalized) return null;
  if (intervalWeeks !== 1 && intervalWeeks !== 2) return null;
  const [hour, minute] = normalized.split(':').map(Number);
  const nowJst = new Date(now.getTime() + JST_OFFSET_MS);
  const todayJstMs = Date.UTC(nowJst.getUTCFullYear(), nowJst.getUTCMonth(), nowJst.getUTCDate());
  const dayMs = 24 * 60 * 60 * 1000;
  let occurrenceDayMs: number;
  if (intervalWeeks === 1) {
    const delta = (weekday - new Date(todayJstMs).getUTCDay() + 7) % 7;
    occurrenceDayMs = todayJstMs + delta * dayMs;
  } else {
    if (!anchorDate) return null;
    const parsed = parseAnchorDate(anchorDate);
    if (!parsed) return null;
    const anchorMs = Date.UTC(parsed.y, parsed.mo - 1, parsed.d);
    if (new Date(anchorMs).getUTCDay() !== weekday) return null;
    const diffDays = Math.round((todayJstMs - anchorMs) / dayMs);
    const step = diffDays <= 0 ? 0 : Math.ceil(diffDays / 14);
    occurrenceDayMs = anchorMs + step * 14 * dayMs;
  }
  const stepDays = intervalWeeks === 1 ? 7 : 14;
  for (;;) {
    const occurrenceMs = occurrenceDayMs + (hour * 60 + minute) * 60 * 1000 - JST_OFFSET_MS;
    if (occurrenceMs + MONITOR_END_GRACE_MS > now.getTime()) return new Date(occurrenceMs);
    occurrenceDayMs += stepDays * dayMs;
  }
}

/** 毎週予定の生成。不正ならnull。 */
export function createWeeklySchedule(weekday: number, time: string): WeeklySchedule | null {
  const normalized = normalizeDailyTime(time);
  if (!isWeekday(weekday) || !normalized) return null;
  return { kind: 'weekly', weekday, time: normalized };
}

/** 隔週予定の生成。anchor必須・曜日一致必須。不正ならnull。 */
export function createBiweeklySchedule(
  weekday: number,
  time: string,
  anchorDate: string,
): BiweeklySchedule | null {
  const normalized = normalizeDailyTime(time);
  if (!isWeekday(weekday) || !normalized || !anchorMatchesWeekday(anchorDate, weekday)) return null;
  return { kind: 'biweekly', weekday, time: normalized, anchorDate: anchorDate.trim() };
}

