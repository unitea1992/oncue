import { $, browser, expect } from '@wdio/globals';
import { loginThroughUi, mockCommands } from './mocks.js';

const E2E_USER = {
  id: 'u1',
  username: 'e2e',
  displayName: 'E2E',
  twoFactorAuthEnabled: false,
};

async function fitsViewport(): Promise<boolean> {
  return browser.execute(
    () => document.documentElement.scrollWidth <= window.innerWidth + 1,
  );
}

async function loginAsUser(): Promise<void> {
  await browser.url('/');
  await mockCommands({
    login: { success: true, user: E2E_USER },
    get_config: { theme: 'light', selectedPresetId: null },
    get_presets: [],
  });
  await loginThroughUi();
}

describe('layout (Layer A)', () => {
  it('900x600で主要画面が横スクロールしない', async () => {
    await browser.setWindowSize(900, 600);
    await browser.url('/');
    await mockCommands({});
    expect(await fitsViewport()).toBe(true);
    await loginAsUser();
    expect(await fitsViewport()).toBe(true);
    await $('button[aria-label="イベント"]').click();
    expect(await fitsViewport()).toBe(true);
    await $('button[aria-label="設定"]').click();
    expect(await fitsViewport()).toBe(true);
  });
  it('200%相当でもログインと監視が操作可能', async () => {
    await browser.setWindowSize(450, 300);
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
    await expect($('#login-username')).toBeDisplayed();
    await loginThroughUi();
    await expect($('[data-testid="cue-panel"]')).toBeDisplayed();
    expect(await fitsViewport()).toBe(true);
  });
});
