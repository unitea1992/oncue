import { useEffect, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { AlertTriangle, LogOut, RefreshCw } from 'lucide-react';
import { useAuth } from '../features/auth/AuthContext';
import { api, formatInvokeError } from '../lib/tauri';
import { applyTheme, type Theme } from '../lib/theme';
import { UnofficialNotice } from '../components/UnofficialNotice';

const CREDITS = [
  ['Tauri', 'https://tauri.app/'],
  ['Rust', 'https://www.rust-lang.org/'],
  ['React', 'https://react.dev/'],
  ['TypeScript', 'https://www.typescriptlang.org/'],
  ['Vite', 'https://vite.dev/'],
  ['WebView2', 'https://developer.microsoft.com/en-us/microsoft-edge/webview2/'],
  ['VRChat API Documentation', 'https://vrchat.community/'],
  ['vrchatapi', 'https://crates.io/crates/vrchatapi'],
  ['Lucide', 'https://lucide.dev/'],
  ['Big Shoulders', 'https://github.com/xotypeco/big_shoulders'],
] as const;

const THEMES = [
  ['light', 'ライト'],
  ['dark', 'ダーク'],
  ['system', 'Windows に合わせる'],
] as const satisfies readonly (readonly [Theme, string])[];

export function SettingsPage() {
  const { user, logout, error: authError } = useAuth();
  const [debugMode, setDebugMode] = useState(false);
  const [theme, setTheme] = useState<Theme>('light');
  const [version, setVersion] = useState<string | null>(null);
  const [versionError, setVersionError] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [loggingOut, setLoggingOut] = useState(false);

  useEffect(() => {
    loadConfig();
    loadVersion();
  }, []);

  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  const loadConfig = async () => {
    try {
      const config = await api.config.getConfig();
      setDebugMode(!!config.debugMode);
      // 新規既定はlight、明示済みは保持する
      setTheme(config.theme ?? 'light');
      setSettingsError(null);
    } catch (err) {
      setSettingsError(formatInvokeError(err, '設定を読み込めませんでした。\nもう一度試してください。'));
    } finally {
      setLoaded(true);
    }
  };

  const loadVersion = async () => {
    setVersionError(false);
    try {
      setVersion(await getVersion());
    } catch {
      setVersionError(true);
    }
  };

  const handleThemeChange = async (newTheme: Theme) => {
    if (saving || newTheme === theme) return;
    const prev = theme;
    setTheme(newTheme);
    setSaving(true);
    try {
      await api.config.saveConfigPatch({ theme: newTheme });
    } catch (err) {
      setTheme(prev);
      setSettingsError(formatInvokeError(err, 'テーマを保存できませんでした。\nもう一度選んでください。'));
    } finally {
      setSaving(false);
    }
  };

  const handleToggleDebugMode = async () => {
    if (saving) return;
    const next = !debugMode;
    setSaving(true);
    try {
      await api.config.saveConfigPatch({ debugMode: next });
      setDebugMode(next);
    } catch (err) {
      setSettingsError(formatInvokeError(err, 'デバッグモードを切り替えられませんでした。\nもう一度試してください。'));
    } finally {
      setSaving(false);
    }
  };

  const handleLogout = async () => {
    if (loggingOut) return;
    setLoggingOut(true);
    try {
      await logout();
    } finally {
      setLoggingOut(false);
    }
  };

  const handleRetry = async () => {
    setSettingsError(null);
    setLoaded(false);
    await loadConfig();
  };

  return (
    <div className="page settings">
      <h1 className="sr-only">設定</h1>

      {settingsError && (
        <div className="callout error" role="alert">
          <AlertTriangle size={16} aria-hidden />
          <div className="callout-body">
            <p className="error-message">{settingsError}</p>
            <button type="button" onClick={handleRetry}>
              <RefreshCw size={14} aria-hidden />
              読み込み直す
            </button>
          </div>
        </div>
      )}

      <section className="settings-section" aria-labelledby="settings-account">
        <h2 id="settings-account">アカウント</h2>
        <div className="settings-box">
          <div className="settings-row">
            {user ? (
              <>
                {user.profileIconUrl ? (
                  <img src={user.profileIconUrl} alt="" className="avatar" />
                ) : (
                  <span className="avatar" aria-hidden>
                    {(user.displayName || user.username).slice(0, 1)}
                  </span>
                )}
                <div className="settings-key">
                  <span className="user-name">{user.displayName}</span>
                  <span className="hint">@{user.username}</span>
                </div>
                <button type="button" onClick={handleLogout} disabled={loggingOut}>
                  <LogOut size={16} aria-hidden />
                  {loggingOut ? 'ログアウトしています…' : 'ログアウト'}
                </button>
              </>
            ) : (
              <p className="hint">ログイン中のアカウント情報を表示できません。</p>
            )}
          </div>
          {authError && (
            <p className="error-message settings-row" role="alert">
              {authError}
            </p>
          )}
        </div>
      </section>

      <section className="settings-section" aria-labelledby="settings-display">
        <h2 id="settings-display">表示と動作</h2>
        <div className="settings-box" aria-busy={!loaded}>
          <div className="settings-row">
            <span className="settings-key" id="settings-theme-label">
              テーマ
            </span>
            <div className="segmented" role="radiogroup" aria-labelledby="settings-theme-label">
              {THEMES.map(([t, label]) => (
                <label key={t}>
                  <input
                    type="radio"
                    name="theme"
                    value={t}
                    checked={theme === t}
                    onChange={() => handleThemeChange(t)}
                    disabled={saving || !loaded}
                  />
                  {label}
                </label>
              ))}
            </div>
          </div>
          <div className="settings-row">
            <span className="settings-key">
              <span id="settings-debug-label">デバッグモード</span>
              <span className="hint" id="settings-debug-hint">
                詳しいログと、イベントごとの動作確認ボタンを表示します。
              </span>
            </span>
            <button
              type="button"
              role="switch"
              aria-checked={debugMode}
              aria-label="デバッグモード"
              aria-describedby="settings-debug-hint"
              className="switch"
              data-testid="debug-switch"
              onClick={handleToggleDebugMode}
              disabled={saving || !loaded}
            />
          </div>
        </div>
      </section>

      <section className="settings-section" aria-labelledby="settings-about" data-testid="settings-about">
        <h2 id="settings-about">このアプリについて</h2>
        <div className="settings-box about">
          <p className="about-title">
            <span className="about-name">OnCue</span>
            {version ? (
              <span className="num about-version">{version}</span>
            ) : versionError ? (
              <button type="button" className="link" onClick={loadVersion}>
                バージョンを読み込み直す
              </button>
            ) : null}
            <span className="hint">MIT ライセンス</span>
          </p>
          <UnofficialNotice className="about-notice" />
          <div className="about-credits">
            <p className="hint">使っているソフトウェアと素材</p>
            <ul>
              {CREDITS.map(([name, url]) => (
                <li key={name}>
                  <a href={url} target="_blank" rel="noreferrer">
                    {name}
                  </a>
                </li>
              ))}
            </ul>
          </div>
        </div>
      </section>
    </div>
  );
}
