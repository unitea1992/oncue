# OnCue

VRChatのグループイベントを対象にした非公式ツールです。イベントが始まる時刻にインスタンスを監視し、条件に合う会場が開いたらVRChatを起動して入室させるWindows用アプリです。

> [!IMPORTANT]
> - **非公式ツールです。** VRChat Inc. とは関係がなく、同社の承認や保証も受けていません。
> - VRChatのユーザー名・パスワード・2FAコードをこのアプリに入力して使います。VRChatはOAuthを提供しておらず、第三者ツールへ認証情報を渡さないよう利用者に求めています。入力した情報は VRChat API（`api.vrchat.cloud`）へのログインにだけ使います。パスワードは保存しません。「ログイン状態を保存する」を選んだ場合に限り、ログイン状態（セッションCookie）を Windows DPAPI で暗号化し、実行ファイルと同じフォルダの `data/` に保存します。
> - 非公式APIの利用や自動化は、VRChatの利用規約やコミュニティガイドラインに抵触するおそれがあります。アカウントへの影響も含めて、利用は自己責任です。本ソフトウェアは無保証です（[LICENSE](LICENSE)）。

## このリポジトリについて

作者が自分で使うために作ったツールのソースコードを公開しています。

- 実行ファイルの配布はしていません。使う場合は、下の手順で自分でビルドしてください。
- 質問への回答、Issue や Pull Request への対応は約束できません。必要な変更はフォークして自由に行ってください（MIT License）。
- 脆弱性の報告は [SECURITY.md](SECURITY.md) を参照してください。

## できること

- 登録したイベントの開始3分前から開始2分後まで、グループのインスタンスを監視します。確認間隔は通常15秒、開始30秒前からは5秒です（どちらも少し揺らぎを入れています）。
- 条件に合う会場が見つかると、`vrchat://` の起動リンクでVRChatにインスタンスを開かせます。その後、最長90秒間、本人の現在地が対象インスタンスに変わったかを確認します。移動中やキュー待ちの間は成功扱いにしません。
- 「優先する会場名」を指定すると、名前にその語を含むインスタンスだけを対象にします。大文字と小文字、全角と半角の違いは区別しません。指定がなく候補が複数あるときは、誤った会場へ入らないよう自動参加せずに止まります。満員の場合は、監視期限まで空きを待ち続けます。
- セルフ招待は自動では送りません。デバッグモードで手動送信だけできます。
- 2FAは認証アプリ（TOTP）とメールOTPに対応しています。
- イベントは単発・毎週・隔週で登録でき、時刻は日本時間で扱います。グループIDを入れるとグループ名を確認でき、グループのイベントカレンダーから開始時刻を取り込めます。

## ビルドして使う

### 必要なもの

ビルドは Windows 10/11 で行います。

- [Rust](https://rustup.rs/)（stable。最低バージョンは 1.90）
- [Node.js](https://nodejs.org/) 22 以上
- Visual Studio Build Tools の「C++ によるデスクトップ開発」ワークロード
- WebView2 Runtime（Windows 10/11 には通常入っています）

Tauri CLI は `npm ci` で一緒に入るので、別途インストールする必要はありません。

### ビルド手順

```powershell
git clone <このリポジトリのURL>
cd <クローンしたフォルダ>
npm ci
npm run tauri build -- --no-bundle
```

`src-tauri/target/release/oncue.exe` ができます。初回はRustの依存関係をコンパイルするため、数分から十数分かかります。

C ランタイムを静的リンクしているので（`src-tauri/.cargo/config.toml`）、実行に Visual C++ 再頒布可能パッケージは要りません。

### 配置

`oncue.exe` だけを、デスクトップやドキュメントなど書き込みできる空のフォルダへコピーして起動します。Program Files のような書き込みできない場所は避けてください。設定やログは exe と同じフォルダに作られます。

```
oncue/
├── oncue.exe
├── data/    ← ログイン状態・イベント設定（初回起動で作成）
├── cache/
└── logs/    ← JSON Lines 形式のログ
```

`data/` にはログイン状態が入っているため、他人に渡したり公開したりしないでください。

## 使い方

1. 起動してVRChatアカウントでログインします。2FAを有効にしている場合は、認証アプリかメールのコードを入力します。「ログイン状態を保存する」を選ぶと、次回から入力を省けます。
2. 「イベント」画面の「イベントを追加」で、グループID（`grp_` で始まるID）・イベント名・開催日時を登録します。単発のイベントは「グループのカレンダーを読み込む」から選ぶと、名前と日時が入ります。会場が複数立つイベントなら「優先する会場名」も入れておきます。
3. 「イベント」画面で「これを監視する」を押します。アプリを開いたままにしておくと、開始3分前に自動で監視が動き出します。進み具合は「監視」画面のランプと秒読みで分かります。会場が複数あるときは、画面で入る会場を選びます。

設定画面でデバッグモードを有効にすると、詳細ログの表示と、イベントごとの読み取り専用APIテスト・起動リンク送信・セルフ招待送信を個別に試せます。

## うまくいかないとき

- **設定やログイン状態が保存されない** → exe を書き込みできるフォルダへ移します。書き込めない場所でも起動はしますが、`data/` に保存できず、ログも最小限になります。
- **「セッションが期限切れです」と出る** → 保存したログイン状態は30日で無効になります。もう一度ログインしてください。
- **「VRChat APIの制限に達しました」と出る** → APIの呼び出し回数制限です。数分から数十分おいてから再開します。
- **入室できない** → VRChat がインストールされ `vrchat://` リンクを開ける状態か、グループIDと開催時刻が正しいかを確認します。会場が複数あって自動で決められない場合は「優先する会場名」を指定します。

ログは `logs/` にあります。PowerShell で次のように確認できます。

```powershell
Get-Content .\logs\*.jsonl -Tail 80                            # 最新のログ
Get-Content .\logs\*.jsonl | Select-String '"level":"ERROR"'   # エラーだけ
```

## 開発

### テストと静的検査

```powershell
npm test                    # renderer-domain テスト
npm run test:ui:typecheck   # UIテストの型検査
npm run test:rust           # Rust テスト（Windows 向けコードを含むため Windows で実行）
cd src-tauri; cargo fmt -- --check; cargo clippy --all-targets --all-features --locked -- -D warnings
```

UIテスト（Layer A）は、Rust側を使わずブラウザ上で画面だけを動かすテストです。`npm run test:ui` で開発サーバーの起動からテストまでを実行します。

### CI

GitHub Actions はすべて GitHub ホストランナーで動き、secrets は使いません。

| ワークフロー | 起動条件 | 内容 |
|---|---|---|
| `CI (hosted quality gate)` | PR / main への push / 手動 | Ubuntu で型検査、renderer-domain テスト、フロントエンドビルド、Layer A、`cargo fmt` |
| `Validate Windows (QA scaffold)` | PR / main への push / 手動 | Windows で `cargo fmt`・`clippy`・`test`、Layer A、版の整合検査、リリース相当のビルド、ZIP と SHA256 の検査 |
| `Build Portable Windows App` | `v*` タグの push / 手動 | Windows で検証から配布用 ZIP の作成まで。ZIP は作者の動作確認用に1日だけ Artifacts に残します。タグのときは下書きの Release を作ります |
| `Native E2E (hosted, Layer B)` | Build Portable の完了後 / 手動 | Build Portable が作った exe を起動し、ログイン画面が出るところまでを確認します |
| `Check Rust MSRV` / `Lint GitHub Actions workflows` | 関連ファイルの変更時 | 依存クレートが要求するRustバージョンと、ワークフローの構文を検査 |

### バージョンとリリース

バージョン番号の正本は `package.json` の `version` だけです。変更したら `npm run version:sync` で Cargo.toml・Cargo.lock・package-lock.json に反映し、`npm run version:check` で確認します。

`vX.Y.Z`（RC は `vX.Y.Z-rc.N`）のタグを push すると、ZIP と SHA256 を添付した下書きの Release ができます。下書きは公開しない運用です。タグとバージョンが食い違うとビルドが失敗します。

## ライセンス

[MIT License](LICENSE)

VRChat は VRChat Inc. の商標です。本プロジェクトは VRChat Inc. と提携しておらず、承認も受けていません。
