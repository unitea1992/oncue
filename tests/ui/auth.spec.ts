import { $, browser, expect } from '@wdio/globals';
import { mockCommands, setReactInput } from './mocks.js';

const E2E_USER = {
  id: 'u1',
  username: 'e2e',
  displayName: 'E2E',
  twoFactorAuthEnabled: false,
};

async function fillLogin(): Promise<void> {
  await setReactInput('login-username', 'e2e');
  await setReactInput('login-password', 'secret');
  await $('button[type="submit"]').click();
}

describe('auth (Layer A)', () => {
  it('ログイン失敗でエラーを表示する', async () => {
    await browser.url('/');
    await mockCommands({ login: { success: false } });
    await fillLogin();
    const loginError = await $('#login-error').getText();
    expect(loginError).toContain('ログインに失敗しました');
    // 現UI仕様: ログイン失敗時も入力は保持される (LoginPageは再マウントしない)。実UI正として固定する。
    await expect($('#login-username')).toHaveValue('e2e');
  });

  it('TOTP二要素認証を完遂できる', async () => {
    await browser.url('/');
    await mockCommands({
      login: { success: true, requiresTwoFactor: ['totp'], username: 'e2e' },
      verify_two_factor: { success: true, user: E2E_USER },
    });
    await fillLogin();
    await expect($('h1')).toHaveText('二要素認証');
    await $('#two-factor-code').setValue('123456');
    await $('button[type="submit"]').click();
    await expect($('[data-testid="cue-title"]')).toHaveText('監視するイベントがありません');
  });

  it('2FAを取り消してログインに戻れる', async () => {
    await browser.url('/');
    await mockCommands({
      login: { success: true, requiresTwoFactor: ['totp'], username: 'e2e' },
      logout: null,
    });
    await fillLogin();
    await expect($('h1')).toHaveText('二要素認証');
    await $('//button[normalize-space()="ログイン画面に戻る"]').click();
    await expect($('#login-username')).toBeDisplayed();
  });
});
