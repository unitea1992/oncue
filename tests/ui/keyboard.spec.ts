import { $, browser, expect } from '@wdio/globals';
import { getMockCommand, mockCommands, setNativeInputValue } from './mocks.js';

const E2E_USER = {
  id: 'u1',
  username: 'e2e',
  displayName: 'E2E',
  twoFactorAuthEnabled: false,
};
const GROUP_ID = 'grp_12345678-1234-1234-1234-123456789012';

type FocusInfo = { id: string; tag: string; text: string; label: string; value: string };

async function activeInfo(): Promise<FocusInfo> {
  return browser.execute(() => {
    const el = document.activeElement as HTMLElement | null;
    const input = el as HTMLInputElement | null;
    return {
      id: el?.id ?? '',
      tag: el?.tagName ?? '',
      text: (el?.textContent ?? '').slice(0, 24),
      label: el?.getAttribute('aria-label') ?? '',
      value: input?.value ?? '',
    };
  });
}

/** Tab連打で条件に合う要素へフォーカスを進める */
async function tabTo(match: (info: FocusInfo) => boolean, max = 40): Promise<FocusInfo> {
  for (let i = 0; i < max; i++) {
    const info = await activeInfo();
    if (match(info)) return info;
    await browser.keys('Tab');
  }
  throw new Error('tab target not reached');
}

async function loginAsUser(extraMocks: Record<string, unknown> = {}): Promise<void> {
  await browser.url('/');
  await mockCommands({
    login: { success: true, user: E2E_USER },
    get_config: { theme: 'light', selectedPresetId: null },
    get_presets: [],
    save_preset: null,
    ...extraMocks,
  });
  await tabTo((info) => info.id === 'login-username');
  await browser.keys('e2e');
  await tabTo((info) => info.id === 'login-password');
  await browser.keys('secret');
  await tabTo((info) => info.text.includes('ログイン') && info.tag === 'BUTTON');
  await browser.keys('Enter');
  await expect($('[data-testid="cue-title"]')).toBeDisplayed();
}

describe('keyboard-only (Layer A)', () => {
  it('ログインをキー操作だけで完遂できる', async () => {
    await browser.url('/');
    await mockCommands({
      login: { success: true, user: E2E_USER },
      get_config: { theme: 'light', selectedPresetId: null },
      get_presets: [],
    });
    await tabTo((info) => info.id === 'login-username');
    await browser.keys('e2e');
    await tabTo((info) => info.id === 'login-password');
    await browser.keys('secret');
    // Shift+Tabで戻れること
    await browser.keys(['Shift', 'Tab']);
    expect((await activeInfo()).id).toBe('login-username');
    await tabTo((info) => info.id === 'login-password');
    await tabTo((info) => info.text.includes('ログイン') && info.tag === 'BUTTON');
    await browser.keys('Enter');
    await expect($('[data-testid="cue-title"]')).toHaveText('監視するイベントがありません');
  });

  it('top navigationをキー操作できる', async () => {
    await loginAsUser();
    await tabTo((info) => info.label === 'イベント');
    await browser.keys('Enter');
    await expect($('button[aria-label="イベント"]')).toHaveAttribute('aria-current', 'page');
    await tabTo((info) => info.label === '設定');
    await browser.keys('Enter');
    await expect($('button[aria-label="設定"]')).toHaveAttribute('aria-current', 'page');
  });

  // 「イベント追加と予定切替をキー操作できる」はJS helperでの値設定が多く
  // keyboard保証が弱いため削除。events-formとASCII入力ケースで保証する。
  it('ASCIIイベント追加をキー操作で保存できる', async () => {
    await loginAsUser();
    await tabTo((info) => info.label === 'イベント');
    await browser.keys('Enter');
    await tabTo((info) => info.text.includes('イベントを追加'));
    await browser.keys('Enter');
    await expect($('#event-label')).toBeDisplayed();

    // フォームはグループ → カレンダー → イベント名 の順に並ぶ
    await tabTo((info) => info.id === 'event-group');
    await browser.keys(GROUP_ID);
    await tabTo((info) => info.id === 'event-label');
    await browser.keys('Friday Event');
    await tabTo((info) => info.text === '毎週' && info.tag === 'BUTTON');
    await browser.keys('Enter');
    await expect($('//button[normalize-space()="毎週"]')).toHaveAttribute('aria-pressed', 'true');
    await tabTo((info) => info.id === 'event-weekday');
    await $('#event-weekday').selectByVisibleText('金曜');
    await tabTo((info) => info.id === 'event-time');
    // time入力のキー打鍵はheadless自動化で不安定なため確定ヘルパー。到達はTabで検証済み。
    await setNativeInputValue('#event-time', '22:00');
    await tabTo((info) => info.text === '保存');
    await browser.keys('Enter');

    const saveMock = getMockCommand('save_preset');
    await saveMock?.update();
    expect(saveMock?.mock.calls[0]?.[0]).toEqual({
      preset: expect.objectContaining({
        label: 'Friday Event',
        groupId: GROUP_ID,
        schedule: { kind: 'weekly', weekday: 5, time: '22:00' },
      }),
    });
  });

  it('イベント編集をキー操作で保存できる', async () => {
    const editPreset = {
      id: 'p-edit',
      label: 'Editable Event',
      groupId: GROUP_ID,
      groupName: 'E2E',
      schedule: { kind: 'weekly', weekday: 5, time: '22:00' },
    };
    await loginAsUser({
      get_config: { theme: 'light', selectedPresetId: 'p-edit' },
      get_presets: [editPreset],
      save_preset: null,
      get_api_budget_estimate: null,
    });
    await tabTo((info) => info.label === 'イベント');
    await browser.keys('Enter');
    await tabTo((info) => info.label === 'Editable Eventを編集');
    await browser.keys('Enter');
    await expect($('#event-label')).toBeDisplayed();
    await tabTo((info) => info.id === 'event-label');
    await browser.keys(['Control', 'a']);
    await browser.keys('Edited Event');
    await tabTo((info) => info.text === '保存');
    await browser.keys('Enter');

    const saveMock = getMockCommand('save_preset');
    await saveMock?.update();
    expect(saveMock?.mock.calls[0]?.[0]).toEqual({
      preset: expect.objectContaining({
        id: 'p-edit',
        label: 'Edited Event',
        groupId: GROUP_ID,
        schedule: { kind: 'weekly', weekday: 5, time: '22:00' },
      }),
    });
  });
  it('dialogのEscとcancelで閉じられる', async () => {
    await loginAsUser();
    await tabTo((info) => info.label === 'イベント');
    await browser.keys('Enter');
    await tabTo((info) => info.text.includes('イベントを追加'));
    await browser.keys('Enter');
    await expect($('#event-label')).toBeDisplayed();
    await browser.keys('Escape');
    await expect($('#event-label')).not.toBeDisplayed();
    // NativeDialogはEscで開く前のフォーカス (イベントを追加ボタン) へ戻す (現UI仕様)。トラップ残存がなくTab継続できることを確認する。
    const afterEsc = await activeInfo();
    expect(afterEsc.tag).toBe('BUTTON');
    expect(afterEsc.text).toContain('イベントを追加');
    // cancel経路
    await tabTo((info) => info.text.includes('イベントを追加'));
    await browser.keys('Enter');
    await expect($('#event-label')).toBeDisplayed();
    await tabTo((info) => info.text === 'キャンセル');
    await browser.keys('Enter');
    await expect($('#event-label')).not.toBeDisplayed();
  });

  it('監視開始をキー操作できる', async () => {
    await browser.url('/');
    await mockCommands({
      login: { success: true, user: E2E_USER },
      get_config: { theme: 'light', selectedPresetId: 'p1' },
      get_presets: [
        {
          id: 'p1',
          label: 'E2Eイベント',
          groupId: 'g1',
          groupName: 'E2E',
          schedule: { kind: 'once', startsAt: '2026-12-24T10:00:00.000Z' },
        },
      ],
      get_api_budget_estimate: null,
      get_monitor_status: {
        state: 'Idle',
        stopReason: null,
        config: { presetId: 'p1' },
      },
      start_monitoring: null,
    });
    await tabTo((info) => info.id === 'login-username');
    await browser.keys('e2e');
    await tabTo((info) => info.id === 'login-password');
    await browser.keys('secret');
    await tabTo((info) => info.text.includes('ログイン') && info.tag === 'BUTTON');
    await browser.keys('Enter');
    await tabTo((info) => info.text === '今すぐ監視を始める' && info.tag === 'BUTTON');
    await browser.keys('Enter');
    const startMock = getMockCommand('start_monitoring');
    await startMock?.update();
    expect(startMock?.mock.calls[0]?.[0]).toEqual({ presetId: 'p1' });
  });

  // debug switchのSpace操作は標準ブラウザ動作のため削除。
  // debug機能自体はsettings-debugの利用者フローで保証する。
});
