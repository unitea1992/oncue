/** 非公式ツールであることと、認証情報の扱い。ログイン画面と設定画面に常に出す。 */
export function UnofficialNotice({ className }: { className?: string }) {
  return (
    <div className={className}>
      <p>
        <strong>VRChat 非公式のツールです。</strong>
        <br />
        VRChat Inc. の承認は受けていません。
      </p>
      <p>
        入力した情報は、
        <br />
        VRChat へのログインにだけ使います。
        <br />
        パスワードは保存しません。
      </p>
      <p>
        利用規約に触れるおそれがあります。
        <br />
        自己責任で使ってください。
      </p>
    </div>
  );
}
