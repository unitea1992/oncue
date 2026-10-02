import { useState, type ReactNode } from 'react';
import { Eye, EyeOff } from 'lucide-react';
import { useAuth } from '../features/auth/AuthContext';
import { type TwoFactorMethod } from '../lib/tauri';
import { BrandMark } from '../components/BrandMark';
import { UnofficialNotice } from '../components/UnofficialNotice';

const METHOD_LABEL: Record<TwoFactorMethod, string> = {
  totp: '認証アプリ',
  emailOtp: 'メール',
};

const METHOD_HINT: Record<TwoFactorMethod, string> = {
  totp: '認証アプリに表示されている6桁の数字を入力してください。',
  emailOtp: 'VRChat から届いたメールに書かれている6桁の数字を入力してください。',
};

/** 左に舞台（ブランドと注意書き）、右に入力欄を置く共通の枠。 */
function LoginShell({ brandAsHeading, children }: { brandAsHeading: boolean; children: ReactNode }) {
  const brand = (
    <>
      <BrandMark className="login-mark" />
      OnCue
    </>
  );
  return (
    <div className="login">
      <div className="login-stage">
        {brandAsHeading ? <h1 className="brand login-brand">{brand}</h1> : <p className="brand login-brand">{brand}</p>}
        <p className="login-tagline">
          イベント開始の合図で、
          <br />
          開いた会場へすぐ入ります。
        </p>
        <UnofficialNotice className="login-notice" />
      </div>
      <div className="login-panel">{children}</div>
    </div>
  );
}

export function LoginPage() {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [code, setCode] = useState('');
  const [method, setMethod] = useState<TwoFactorMethod>('totp');
  // 既定はOFF。保存は利用者が明示的に選んだときだけ行う。
  const [rememberSession, setRememberSession] = useState(false);
  const {
    login,
    verifyTwoFactor,
    cancelTwoFactor,
    loading,
    error,
    clearError,
    requiresTwoFactor,
    pendingUsername,
    supportedMethods,
  } = useAuth();

  const handleLogin = async (e: React.FormEvent) => {
    e.preventDefault();
    clearError();
    // 失敗時もusername/passwordは消さない（入力保持）
    await login(username, password, rememberSession);
  };

  const handleTwoFactor = async (e: React.FormEvent) => {
    e.preventDefault();
    clearError();
    await verifyTwoFactor(code.trim(), method, rememberSession);
  };

  if (requiresTwoFactor) {
    const effectiveMethod = supportedMethods.includes(method) ? method : supportedMethods[0];
    return (
      <LoginShell brandAsHeading={false}>
        <div className="login-form" aria-busy={loading}>
          <h1 className="login-title">二要素認証</h1>
          {supportedMethods.length === 0 ? (
            <>
              <p id="login-error" data-testid="login-error" role="alert" className="error-message">
                このアカウントの二要素認証の方式には、まだ対応していません。
                {'\n'}認証アプリかメールの方式を有効にしてから、もう一度ログインしてください。
              </p>
              <button type="button" onClick={cancelTwoFactor}>
                ログイン画面に戻る
              </button>
            </>
          ) : (
            <form onSubmit={handleTwoFactor} className="login-fields">
              <p className="login-lead">
                {pendingUsername ? `${pendingUsername} さんのアカウントは、` : 'このアカウントは、'}
                <br />
                二要素認証が有効になっています。
              </p>
              {supportedMethods.length > 1 && (
                <div className="field">
                  <span className="field-label" id="two-factor-method-label">
                    コードの受け取り方
                  </span>
                  <div className="segmented" role="radiogroup" aria-labelledby="two-factor-method-label">
                    {supportedMethods.map((m) => (
                      <label key={m}>
                        <input
                          type="radio"
                          name="two-factor-method"
                          value={m}
                          checked={effectiveMethod === m}
                          onChange={() => setMethod(m)}
                          disabled={loading}
                        />
                        {METHOD_LABEL[m]}
                      </label>
                    ))}
                  </div>
                </div>
              )}
              <div className="field">
                <label className="field-label" htmlFor="two-factor-code">
                  確認コード
                </label>
                <input
                  id="two-factor-code"
                  className="code-input num"
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                  autoComplete="one-time-code"
                  inputMode="numeric"
                  aria-describedby="two-factor-hint"
                  disabled={loading}
                  required
                />
                <p id="two-factor-hint" className="hint">
                  {effectiveMethod ? METHOD_HINT[effectiveMethod] : ''}
                </p>
              </div>
              {error && (
                <p id="login-error" data-testid="login-error" role="alert" className="error-message">
                  {error}
                </p>
              )}
              <button type="submit" className="primary large" disabled={loading || !effectiveMethod}>
                {loading ? '確認しています…' : '確認する'}
              </button>
              <button type="button" className="quiet" onClick={cancelTwoFactor} disabled={loading}>
                ログイン画面に戻る
              </button>
            </form>
          )}
        </div>
      </LoginShell>
    );
  }

  return (
    <LoginShell brandAsHeading>
      <form className="login-form login-fields" onSubmit={handleLogin} aria-busy={loading}>
        <h2 className="login-title">VRChat アカウントでログイン</h2>
        <div className="field">
          <label className="field-label" htmlFor="login-username">
            ユーザー名またはメールアドレス
          </label>
          <input
            id="login-username"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoComplete="username"
            disabled={loading}
            required
          />
        </div>
        <div className="field">
          <label className="field-label" htmlFor="login-password">
            パスワード
          </label>
          <div className="password-row">
            <input
              id="login-password"
              type={showPassword ? 'text' : 'password'}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password"
              disabled={loading}
              required
            />
            <button
              type="button"
              className="quiet icon"
              onClick={() => setShowPassword((v) => !v)}
              aria-label={showPassword ? 'パスワードを隠す' : 'パスワードを表示'}
              aria-pressed={showPassword}
              aria-controls="login-password"
            >
              {showPassword ? <EyeOff size={18} aria-hidden /> : <Eye size={18} aria-hidden />}
            </button>
          </div>
        </div>
        <label className="check">
          <input
            type="checkbox"
            checked={rememberSession}
            onChange={(e) => setRememberSession(e.target.checked)}
            disabled={loading}
            aria-describedby="remember-hint"
          />
          <span>
            ログイン状態を保存する
            <span id="remember-hint" className="hint check-hint">
              この PC に暗号化して保存します。
              <br />
              30日間は入力を省けます。
            </span>
          </span>
        </label>
        {error && (
          <p id="login-error" data-testid="login-error" role="alert" className="error-message">
            {error}
          </p>
        )}
        <button type="submit" className="primary large" disabled={loading}>
          {loading ? 'ログインしています…' : 'ログイン'}
        </button>
      </form>
    </LoginShell>
  );
}
