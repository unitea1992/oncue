import { $, browser, expect } from '@wdio/globals';
// Layer A スモーク骨格 (3件のみ)。業務フロー再現は膨らませない。
// browser mode では起動時 invoke を横取りできないため (service の timing caveat)、
// mock 登録後に UI 操作で導線を進める。初期読込の失敗は error/retry UI として扱う。
import { mockCommands, setReactInput } from './mocks.js';

const E2E_USER = {
  id: 'u1',
  username: 'e2e',
  displayName: 'E2E',
  twoFactorAuthEnabled: false,
};
async function loginThroughUi(): Promise<void> {
  await setReactInput('login-username', 'e2e');
  await setReactInput('login-password', 'secret');
  await $('button[type="submit"]').click();
}
describe('smoke (Layer A 骨格)', () => {
  it('ログイン画面が表示される', async () => {
    await browser.url('/');
    await mockCommands();
    await expect($('h1')).toHaveText('OnCue');
    await expect($('#login-username')).toBeDisplayed();
  });

  it('監視開始で開始待ちになる', async () => {
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
        state: 'WaitingForWindow',
        stopReason: null,
        config: { presetId: 'p1' },
      },
    });
    await loginThroughUi();
    await expect($('[data-testid="cue-title"]')).toHaveText('監視の開始を待っています');
  });

  it('イベント未選択では空状態になる', async () => {
    await browser.url('/');
    await mockCommands({
      login: { success: true, user: E2E_USER },
      get_config: { theme: 'light', selectedPresetId: null },
      get_presets: [],
    });
    await loginThroughUi();
    await expect($('[data-testid="cue-title"]')).toHaveText('監視するイベントがありません');
  });
});
