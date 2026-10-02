import React, { createContext, useCallback, useContext, useEffect, useState } from 'react';
import {
  api,
  formatInvokeError,
  TWO_FACTOR_METHODS,
  type CurrentUser,
  type TwoFactorMethod,
} from '../../lib/tauri';

interface AuthContextType {
  user: CurrentUser | null;
  isAuthenticated: boolean;
  loading: boolean;
  error: string | null;
  /** 未確定の2FA方式一覧。nullなら2FA待ちではない。 */
  requiresTwoFactor: string[] | null;
  pendingUsername: string;
  supportedMethods: TwoFactorMethod[];
  unsupportedMethods: string[];
  login: (username: string, password: string, rememberSession: boolean) => Promise<boolean>;
  verifyTwoFactor: (code: string, method: TwoFactorMethod, rememberSession: boolean) => Promise<boolean>;
  logout: () => Promise<void>;
  /** 2FA待ちの取消。IPCなしで2FA状態だけ戻し、ログイン入力は保持する。 */
  cancelTwoFactor: () => void;
  clearError: () => void;
}

const TWO_FACTOR_FAILED = '確認コードが違うか、有効期限が切れています。\n新しいコードを入力してください。';

const AuthContext = createContext<AuthContextType | undefined>(undefined);

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [user, setUser] = useState<CurrentUser | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [requiresTwoFactor, setRequiresTwoFactor] = useState<string[] | null>(null);
  const [pendingUsername, setPendingUsername] = useState('');

  const checkSession = useCallback(async () => {
    try {
      setLoading(true);
      setUser(await api.auth.checkSession());
    } catch (err) {
      setError(formatInvokeError(err, '保存したログイン状態を確認できませんでした。\nもう一度ログインしてください。'));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    checkSession();
  }, [checkSession]);

  const login = useCallback(async (username: string, password: string, rememberSession: boolean) => {
    try {
      setLoading(true);
      setError(null);
      setRequiresTwoFactor(null);
      setPendingUsername('');

      const result = await api.auth.login(username, password, rememberSession);
      if (result.user) {
        setUser(result.user);
        return true;
      }
      if (result.requiresTwoFactor && result.requiresTwoFactor.length > 0) {
        setRequiresTwoFactor(result.requiresTwoFactor);
        setPendingUsername(result.username || username);
        return true;
      }
      setError('ログインに失敗しました。\n時間をおいて、もう一度試してください。');
      return false;
    } catch (err) {
      setError(formatInvokeError(err, 'ログインに失敗しました。\n時間をおいて、もう一度試してください。'));
      return false;
    } finally {
      setLoading(false);
    }
  }, []);

  const verifyTwoFactor = useCallback(
    async (code: string, method: TwoFactorMethod, rememberSession: boolean) => {
      try {
        setLoading(true);
        setError(null);
        const result = await api.auth.verifyTwoFactor(code, method, rememberSession);
        if (result.success && result.user) {
          setUser(result.user);
          setRequiresTwoFactor(null);
          setPendingUsername('');
          return true;
        }
        setError(TWO_FACTOR_FAILED);
        return false;
      } catch (err) {
        setError(formatInvokeError(err, TWO_FACTOR_FAILED));
        return false;
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  const logout = useCallback(async () => {
    try {
      setLoading(true);
      await api.auth.logout();
      setUser(null);
      setError(null);
      setRequiresTwoFactor(null);
      setPendingUsername('');
    } catch (err) {
      setError(formatInvokeError(err, 'ログアウトできませんでした。\nもう一度試してください。'));
    } finally {
      setLoading(false);
    }
  }, []);

  const cancelTwoFactor = useCallback(() => {
    setRequiresTwoFactor(null);
    setPendingUsername('');
    setError(null);
  }, []);

  const clearError = useCallback(() => {
    setError(null);
  }, []);

  const supportedMethods = (requiresTwoFactor ?? []).filter((m): m is TwoFactorMethod =>
    (TWO_FACTOR_METHODS as readonly string[]).includes(m),
  );
  const unsupportedMethods = (requiresTwoFactor ?? []).filter(
    (m) => !(TWO_FACTOR_METHODS as readonly string[]).includes(m),
  );

  const value: AuthContextType = {
    user,
    isAuthenticated: !!user,
    loading,
    error,
    requiresTwoFactor,
    pendingUsername,
    supportedMethods,
    unsupportedMethods,
    login,
    verifyTwoFactor,
    logout,
    cancelTwoFactor,
    clearError,
  };

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  const context = useContext(AuthContext);
  if (!context) throw new Error('useAuth must be used within AuthProvider');
  return context;
}
