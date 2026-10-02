import { getCurrentWindow } from '@tauri-apps/api/window';

export type Theme = 'light' | 'dark' | 'system';

const darkQuery = () => window.matchMedia('(prefers-color-scheme: dark)');
let unsubscribeSystem: (() => void) | null = null;

function setResolved(resolved: 'light' | 'dark') {
  document.documentElement.setAttribute('data-theme', resolved);
}

/** Windows のタイトルバーの明暗をアプリに合わせる。system は Windows 側の設定に任せる */
function syncTitleBar(theme: Theme) {
  try {
    getCurrentWindow()
      .setTheme(theme === 'system' ? null : theme)
      .catch(() => {
        /* Tauri 外（ブラウザでの画面確認・テスト）では何もしない */
      });
  } catch {
    /* 同上 */
  }
}

/** テーマを適用する。system の間は Windows 側の切り替えにも追従する。 */
export function applyTheme(theme: Theme) {
  unsubscribeSystem?.();
  unsubscribeSystem = null;
  syncTitleBar(theme);
  if (theme !== 'system') {
    setResolved(theme);
    return;
  }
  const query = darkQuery();
  setResolved(query.matches ? 'dark' : 'light');
  const onChange = (event: MediaQueryListEvent) => setResolved(event.matches ? 'dark' : 'light');
  query.addEventListener('change', onChange);
  unsubscribeSystem = () => query.removeEventListener('change', onChange);
}
