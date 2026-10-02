import type { ReactNode } from 'react';
import { BrandMark } from './BrandMark';

export type AppPage = 'monitor' | 'events' | 'settings';

interface AppHeaderProps {
  activePage: AppPage;
  onPageChange: (page: AppPage) => void;
  status?: ReactNode;
}

const ITEMS: Array<{ id: AppPage; label: string }> = [
  { id: 'monitor', label: '監視' },
  { id: 'events', label: 'イベント' },
  { id: 'settings', label: '設定' },
];

export function AppHeader({ activePage, onPageChange, status }: AppHeaderProps) {
  return (
    <header className="app-header">
      <span className="brand">
        <BrandMark />
        <span className="brand-text">OnCue</span>
      </span>
      <nav className="app-nav" aria-label="画面の切り替え">
        {ITEMS.map((item) => (
          <button
            key={item.id}
            type="button"
            aria-label={item.label}
            aria-current={activePage === item.id ? 'page' : undefined}
            onClick={() => onPageChange(item.id)}
          >
            {item.label}
          </button>
        ))}
      </nav>
      {status}
    </header>
  );
}
