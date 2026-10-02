import { useEffect, useSyncExternalStore } from 'react';
import { api, formatInvokeError } from './tauri';

/**
 * 時間になったら自動で監視を始める。
 * イベントを選んでいてアプリが開いていれば、監視開始の少し前に start_monitoring を呼ぶ。
 * バックエンドは開始を受けると監視開始時刻まで待つため、少し早めに呼んでよい。
 *
 * 一度扱った回（自動で始めた回、手動で始めた回、止めた回）は、同じ回のうちは再び自動では始めない。
 * 記録はアプリを開いている間だけ持つ。
 */
const LEAD_MS = 2 * 60 * 1000;
const TICK_MS = 10 * 1000;
const RETRY_MS = 30 * 1000;

const handled = new Set<string>();
const lastAttempt = new Map<string, number>();
const listeners = new Set<() => void>();
let snapshot: { version: number; error: string | null } = { version: 0, error: null };

function emit(error: string | null) {
  snapshot = { version: snapshot.version + 1, error };
  listeners.forEach((listener) => listener());
}

export function occurrenceKey(presetId: string, eventStartIso: string): string {
  const time = new Date(eventStartIso).getTime();
  return `${presetId}@${Number.isNaN(time) ? eventStartIso : time}`;
}

/** この回を、もう自動では始めない回として扱う */
export function markOccurrenceHandled(presetId: string, eventStartIso: string) {
  handled.add(occurrenceKey(presetId, eventStartIso));
}

export function isOccurrenceHandled(presetId: string, eventStartIso: string): boolean {
  return handled.has(occurrenceKey(presetId, eventStartIso));
}

/** 自動開始の結果。version は自動で始めたときや失敗したときに進む */
export function useAutoStartState() {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => snapshot,
  );
}

async function tick() {
  let config;
  let status;
  try {
    [config, status] = await Promise.all([api.config.getConfig(), api.monitor.getStatus()]);
  } catch {
    return;
  }
  // 手動で始めた回や、終わった・止めた回は、自動で始め直さない
  if (status?.config?.presetId && status.config.eventStart) {
    markOccurrenceHandled(status.config.presetId, status.config.eventStart);
  }
  const state = status?.state;
  const running = typeof state === 'string' && state !== 'Idle';
  const presetId = config?.selectedPresetId;
  if (running || !presetId) {
    if (snapshot.error) emit(null);
    return;
  }

  let budget;
  try {
    budget = await api.monitor.getApiBudgetEstimate(presetId);
  } catch {
    return; // 単発で終わった回など。次の回がない
  }
  if (!budget?.eventStart || !budget.monitorStart || !budget.monitorEnd) return;

  const key = occurrenceKey(presetId, budget.eventStart);
  const now = Date.now();
  const start = new Date(budget.monitorStart).getTime();
  const end = new Date(budget.monitorEnd).getTime();
  if (handled.has(key) || now < start - LEAD_MS || now >= end) {
    if (snapshot.error) emit(null);
    return;
  }
  if (now - (lastAttempt.get(key) ?? 0) < RETRY_MS) return;

  lastAttempt.set(key, now);
  try {
    await api.monitor.start(presetId);
    handled.add(key);
    emit(null);
  } catch (err) {
    // 失敗した回は印を付けず、少し待ってからもう一度試す（VRChat を起動すれば始まる）
    emit(formatInvokeError(err, '自動で監視を始められませんでした。'));
  }
}

/** ログイン中だけ動かす */
export function useAutoStart(enabled: boolean) {
  useEffect(() => {
    if (!enabled) return;
    void tick();
    const id = window.setInterval(() => void tick(), TICK_MS);
    return () => window.clearInterval(id);
  }, [enabled]);
}
