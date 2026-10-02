import { $, browser } from '@wdio/globals';
// 型のみ: transpileOnlyで消去されruntime不変。browser.tauri.mock()の実型。
import type { TauriMock } from '@wdio/native-types';
// test-only: Tauri invoke() の mock。tests/ui 内の graph に閉じ込めること。
// production entry (src/main.tsx) および src/ 以下から参照させない。
// production bundle への混入禁止。
// 実 credential/session は扱わない。ここに password/cookie/token を置かない。
// 仕組みは @wdio/tauri-service browser mode の mock API (browser.tauri.mock)。
// service が navigation 毎に注入し直すため、手書きの invoke 上書きはしない。
export const mockResponses: Record<string, unknown> = {
  get_config: { presets: [], theme: 'light' },
  get_presets: [],
  check_session: null,
  login: { success: false },
  check_preflight: {
    portableWritable: true,
    protocolHandlerAvailable: false,
    vrchatProcessDetected: false,
    savedSessionValid: false,
    websocketConnected: false,
    websocketConnectionState: 'disconnected',
    allClear: true,
    blockers: [],
  },
  get_monitor_status: { state: 'Idle', stopReason: null, config: null },
  get_recent_logs: [],
  get_api_budget_estimate: { estimatedRequests: 0, shouldWarn: false },
  start_monitoring: undefined,
  stop_monitoring: undefined,
};

/** browser.tauri.mock() でコマンド表を登録する。未登録コマンドは throw される。 */
const handles: Record<string, TauriMock> = {};
export async function mockCommands(extra: Record<string, unknown> = {}): Promise<void> {
  const table = { ...mockResponses, ...extra };
  for (const [cmd, value] of Object.entries(table)) {
    // navigation は browser 側登録を消すため、既存 handle は restore して作り直す。
    const prev = handles[cmd];
    if (prev) {
      try {
        await prev.mockRestore();
      } catch {
        /* worker 側に残っていない場合は作り直すだけ */
      }
    }
    const m = await browser.tauri.mock(cmd);
    await m.mockResolvedValue(value ?? null);
    handles[cmd] = m;
  }
}

/** 呼出回数などの検証用に handle を取り出す。未登録なら undefined。 */
export function getMockCommand(cmd: string): TauriMock | undefined {
  return handles[cmd];
}

/** React controlled inputへ確実に届ける。click+keysはfocus競合で落とすため使わない。 */
export async function setReactInput(id: string, text: string): Promise<void> {
  await browser.execute(
    (targetId: string, value: string) => {
      const el = document.getElementById(targetId);
      if (!(el instanceof HTMLInputElement)) throw new Error(`input not found: ${targetId}`);
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
      setter?.call(el, value);
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
    },
    id,
    text,
  );
}

/** idを持たないinput (time/datetime-local等) 用。aria-label等のセレクタで指定する。 */
export async function setNativeInputValue(selector: string, value: string): Promise<void> {
  await browser.execute(
    (sel: string, val: string) => {
      const el = document.querySelector(sel);
      if (!(el instanceof HTMLInputElement)) throw new Error(`input not found: ${sel}`);
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
      setter?.call(el, val);
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
    },
    selector,
    value,
  );
}

/** ログイン画面からUI操作で認証する。呼び出し前に login mockを登録すること。 */
export async function loginThroughUi(username = 'e2e', password = 'secret'): Promise<void> {
  await setReactInput('login-username', username);
  await setReactInput('login-password', password);
  await $('button[type="submit"]').click();
}
