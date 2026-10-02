interface StatusBarProps {
  vrchatRunning: boolean;
}

/**
 * 上部メニュー右端の状態表示。良し悪しを色だけでなく文字と印の形でも示す。
 * ログイン済みかどうかはこの画面が出ている時点で自明なため載せない
 * (savedSessionValid は「ログイン状態を保存したか」であり、ログイン中かどうかではない)。
 * WebSocket は自動接続で利用者が操作できず、異常時はログと結果表示に出るため載せない。
 */
export function StatusBar({ vrchatRunning }: StatusBarProps) {
  return (
    <div className="header-status" role="status" data-testid="header-status">
      <span className="header-status-item">
        <span className={`pip${vrchatRunning ? '' : ' warn'}`} aria-hidden />
        {vrchatRunning ? 'VRChat 起動中' : 'VRChat 未起動'}
      </span>
    </div>
  );
}
