import { $, browser, expect } from '@wdio/globals';
import { loginThroughUi, mockCommands } from './mocks.js';

const E2E_USER = {
  id: 'u1',
  username: 'e2e',
  displayName: 'E2E',
  twoFactorAuthEnabled: false,
};

async function loginWithConfig(configExtra: Record<string, unknown>): Promise<void> {
  await browser.url('/');
  await mockCommands({
    login: { success: true, user: E2E_USER },
    get_config: { theme: 'light', ...configExtra },
    get_presets: [
      {
        id: 'p1',
        label: 'E2Eイベント',
        groupId: 'g1',
        groupName: 'E2E',
        schedule: { kind: 'once', startsAt: '2026-12-24T10:00:00.000Z' },
      },
    ],
    save_config: null,
    'plugin:app|version': '9.9.9-e2e',
  });
  await loginThroughUi();
}

describe('settings and debug (Layer A)', () => {
  it('debug OFF→ONの利用者フローを完遂できる', async () => {
    await loginWithConfig({});
    await $('button[aria-label="イベント"]').click();
    await expect($("//button[normalize-space()='グループ情報']")).not.toExist();
    await $('button[aria-label="設定"]').click();
    const debugSwitch = await $('button[aria-label="デバッグモード"]');
    await expect(debugSwitch).toHaveAttribute('aria-checked', 'false');
    await debugSwitch.click();
    await expect(debugSwitch).toHaveAttribute('aria-checked', 'true');
    // ON設定での再読込後は診断を表示する。
    await loginWithConfig({ debugMode: true });
    await $('button[aria-label="イベント"]').click();
    await expect($("//button[normalize-space()='グループ情報']")).toExist();
  });

  it('バージョンを表示できる', async () => {
    await loginWithConfig({});
    await $('button[aria-label="設定"]').click();
    const settingsText = await $('[data-testid="settings-about"]').getText();
    expect(settingsText).toContain('9.9.9-e2e');
  });
});
