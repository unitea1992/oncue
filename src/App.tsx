import { useState, useEffect } from 'react';
import { useAuth } from './features/auth/AuthContext';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { AppHeader, type AppPage } from './components/AppHeader';
import { StatusBar } from './components/StatusBar';
import { MonitorPage } from './pages/MonitorPage';
import { PresetsPage } from './pages/PresetsPage';
import { SettingsPage } from './pages/SettingsPage';
import { LoginPage } from './pages/LoginPage';
import { applyTheme, type Theme } from './lib/theme';
import { useAutoStart } from './lib/autoStart';
import './styles/base.css';
import './styles/shell.css';
import './styles/login.css';
import './styles/settings.css';
import './styles/monitor.css';
import './styles/events.css';

function App() {
  const { isAuthenticated, loading } = useAuth();
  const [booted, setBooted] = useState(false);
  const [activePage, setActivePage] = useState<AppPage>('monitor');
  const [vrchatRunning, setVrchatRunning] = useState(false);
  useAutoStart(isAuthenticated);

  useEffect(() => {
    if (!isAuthenticated) return;
    let cancelled = false;
    const loadPreflight = () =>
      invoke<{ vrchatProcessDetected: boolean }>('check_preflight')
        .then((result) => {
          if (!cancelled) setVrchatRunning(result.vrchatProcessDetected);
        })
        .catch(() => {
          /* 状態表示は最後の確定値を保つ */
        });

    loadPreflight();
    const intervalId = window.setInterval(loadPreflight, 15000);
    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
  }, [isAuthenticated]);

  // タイトルバーにバージョンを出す（Windows 標準のタイトルバーはそのまま使う）
  useEffect(() => {
    getVersion()
      .then((version) => getCurrentWindow().setTitle(`OnCue ${version}`))
      .catch(() => {
        /* Tauri 外では何もしない */
      });
  }, []);

  useEffect(() => {
    let isMounted = true;
    const loadTheme = async () => {
      try {
        const config = await invoke<{ theme?: Theme }>('get_config');
        if (!isMounted) return;
        // 新規既定はlight、明示済みテーマは保持する
        applyTheme(config.theme ?? 'light');
      } catch {
        if (isMounted) applyTheme('light');
      }
    };
    loadTheme();
    return () => {
      isMounted = false;
    };
  }, []);

  // 初回セッション確認だけ全画面待機にする。以後のloading (ログイン送信等) で
  // 画面を置換すると form state が unmount で失われるため維持する。
  useEffect(() => {
    if (!loading) setBooted(true);
  }, [loading]);

  if (!booted) {
    return (
      <div className="boot" role="status" aria-label="読み込み中">
        <div className="boot-lamp" aria-hidden></div>
      </div>
    );
  }

  if (!isAuthenticated) {
    return <LoginPage />;
  }

  return (
    <div className="app">
      <AppHeader
        activePage={activePage}
        onPageChange={setActivePage}
        status={<StatusBar vrchatRunning={vrchatRunning} />}
      />
      <main className="main-content">
        {activePage === 'monitor' && <MonitorPage onOpenEvents={() => setActivePage('events')} />}
        {activePage === 'events' && <PresetsPage />}
        {activePage === 'settings' && <SettingsPage />}
      </main>
    </div>
  );
}

export default App;
