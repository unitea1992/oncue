import { $, browser, expect } from '@wdio/globals';
import { getMockCommand, loginThroughUi, mockCommands, setNativeInputValue, setReactInput } from './mocks.js';

const E2E_USER = {
  id: 'u1',
  username: 'e2e',
  displayName: 'E2E',
  twoFactorAuthEnabled: false,
};

const GROUP_ID = 'grp_12345678-1234-1234-1234-123456789012';

async function loginWithPresets(presets: unknown[], selectedPresetId: string | null): Promise<void> {
  await browser.url('/');
  await mockCommands({
    login: { success: true, user: E2E_USER },
    get_config: { theme: 'light', selectedPresetId },
    get_presets: presets,
    save_preset: null,
  });
  await loginThroughUi();
  await $('button[aria-label="イベント"]').click();
}

async function fillScheduleWeekly(weekdayVisible: string, time: string): Promise<void> {
  // 現UIの予定種別は 単発/毎週/隔週 のsegmented button。実UI正として毎週を選択する。
  await $('//button[normalize-space()="毎週"]').click();
  await $('#event-weekday').selectByVisibleText(weekdayVisible);
  // timeはwdio setValueがReact controlledへ届かないためnative setter経由。
  await setNativeInputValue('#event-time', time);
}

async function fillScheduleOnce(local: string): Promise<void> {
  await $('//button[normalize-space()="単発"]').click();
  await setNativeInputValue('input[aria-label="開催日時 (日本時間)"]', local);
}

describe('events form (Layer A)', () => {
  it('毎週イベントを追加保存できる', async () => {
    await loginWithPresets([], null);
    await $('[data-testid="events-empty"] button').click();
    await expect($('#event-label')).toBeDisplayed();
    await setReactInput('event-label', '金曜イベント');
    await setReactInput('event-group', GROUP_ID);
    await fillScheduleWeekly('金曜', '22:00');
    await $('dialog button[type="submit"]').click();
    const saveMock = getMockCommand('save_preset');
    await saveMock?.update();
    expect(saveMock?.mock.calls[0]?.[0]).toEqual({
      preset: expect.objectContaining({
        label: '金曜イベント',
        groupId: GROUP_ID,
        schedule: { kind: 'weekly', weekday: 5, time: '22:00' },
      }),
    });
    await expect($('#event-label')).not.toBeDisplayed();
  });

  it('単発は指定日時を保存する', async () => {
    await loginWithPresets([], null);
    await $('[data-testid="events-empty"] button').click();
    await setReactInput('event-label', '単発イベント');
    await setReactInput('event-group', GROUP_ID);
    await fillScheduleOnce('2026-09-18T22:00');
    await $('dialog button[type="submit"]').click();
    const saveMock = getMockCommand('save_preset');
    await saveMock?.update();
    expect(saveMock?.mock.calls[0]?.[0]).toEqual({
      preset: expect.objectContaining({
        schedule: { kind: 'once', startsAt: '2026-09-18T13:00:00.000Z' },
      }),
    });
  });
});
