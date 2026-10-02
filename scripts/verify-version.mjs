#!/usr/bin/env node
// 版一元性ゲート。package.json.version を唯一の正本とし、
// SemVer・Cargo同期・Tauri解決版・タグ・User-Agent・成果物名を検査する。
// 既定は検査のみ (ソースを書き換えない)。--sync で Cargo.toml / Cargo.lock /
// package-lock.json を正本へ寄せ、再読込→再検証で全一致なら終了コード0。
// 正規版が不正などの同期不能時は書き換えず失敗のまま。
// --root <dir> で検証起点を差し替え (検証用仮データ用)、
// --print-metadata でリリースのメタデータを JSON 一行で出す (ワークフローは読むだけ)。
// 標準Nodeのみ (依存なし)。終了コード: 0=成功、1=失敗 (表示はOK/NG)。タグ不一致は即時失敗。
import { readFileSync, writeFileSync, appendFileSync } from "node:fs";
import { resolve, dirname, basename } from "node:path";
import { fileURLToPath } from "node:url";

let ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const env = process.env;
const cli = {};
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
  const m = argv[i].match(/^--([^=]+)(?:=(.*))?$/);
  if (!m) continue;
  const next = argv[i + 1];
  if (m[2] !== undefined) cli[m[1]] = m[2];
  else if (next !== undefined && !next.startsWith("--")) { cli[m[1]] = next; i++; }
  else cli[m[1]] = true;
}
if (typeof cli.root === "string" && cli.root) ROOT = resolve(cli.root);
// テスト用差し替え (--root <dir> / --package-json <検証用ファイル> 等で不正系を判定できる)
const P = (name, def) => resolve(ROOT, (cli[name] === true ? def : cli[name] || def) + "");
const F = {
  packageJson: P("package-json", "package.json"),
  cargoToml: P("cargo-toml", "src-tauri/Cargo.toml"),
  cargoLock: P("cargo-lock", "src-tauri/Cargo.lock"),
  tauriConf: P("tauri-conf", "src-tauri/tauri.conf.json"),
  httpClient: P("http-client", "src-tauri/src/infra/http_client.rs"),
  packageLock: P("package-lock", "package-lock.json"),
};
const read = (p) => readFileSync(p, "utf8");

const metaMode = cli["print-metadata"] !== undefined;
let failed = false;
let nonSyncFailed = false;
let syncFailed = false;
const check = (name, ok, detail = "") => {
  if (!metaMode) console.log(`${ok ? "OK  " : "NG  "}${name}${detail ? `: ${detail}` : ""}`);
  if (!ok) { failed = true; nonSyncFailed = true; }
};
const checkSync = (name, ok, detail = "") => {
  if (!metaMode) console.log(`${ok ? "OK  " : "NG  "}${name}${detail ? `: ${detail}` : ""}`);
  if (!ok) { failed = true; syncFailed = true; }
};

// SemVer 2.0 (公式正規表現の要点: 先行ゼロ禁止・pre/build受理)
const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;

// target triple/通称 -> 利用者向け表記 (tripleをファイル名へ出さない)。
// RUNNER_ARCH (X64等、大文字)・PROCESSOR_ARCHITECTURE (AMD64等) も吸収するため小文字化して引く。
const ARCH_MAP = {
  "x86_64-pc-windows-msvc": "x64",
  x86_64: "x64",
  x64: "x64",
  amd64: "x64",
  "aarch64-pc-windows-msvc": "arm64",
  aarch64: "arm64",
  arm64: "arm64",
};

const canonical = JSON.parse(read(F.packageJson)).version;
check("package.jsonはvalid SemVer", SEMVER.test(canonical ?? ""), String(canonical));
check("rc系列の受理", !/-rc\./.test(canonical ?? "") || SEMVER.test(canonical ?? ""), String(canonical));

// タグ不一致は即時失敗 (タグ先行で版を決めない。--print-metadata時のみJSON返却のため継続)
const refTag = (env.GITHUB_REF ?? "").match(/^refs\/tags\/v(.+)$/)?.[1];
const tag = cli.tag === true ? "" : cli.tag || env.TAG_VERSION || refTag || "";
const strippedTag = String(tag).replace(/^v/, "");
if (tag) {
  if (strippedTag !== canonical) {
    check("tagと正本が一致", false, `tag=${strippedTag} canonical=${canonical}`);
    if (!metaMode) process.exit(1);
  } else {
    check("tagと正本が一致", true, `tag=${strippedTag}`);
  }
} else if (!metaMode) {
  console.log("SKIP tag未指定 (tag build時はGITHUB_REF/--tagで検査)");
}

// [package]節直下のversionのみ見る (依存版に引っ張られない)
const cargoToml = read(F.cargoToml).replace(/\r\n/g, "\n");
const cargoVer = cargoToml
  .split(/^\[(?!package\])/m)[0]
  .match(/^version\s*=\s*"([^"]+)"/m)?.[1];
checkSync("Cargo.toml同期", cargoVer === canonical, `Cargo.toml=${cargoVer}`);

const lockBlock = read(F.cargoLock)
  .replace(/\r\n/g, "\n")
  .match(/\[\[package\]\]\nname = "oncue"\nversion = "([^"]+)"/);
checkSync("Cargo.lock同期", lockBlock?.[1] === canonical, `Cargo.lock=${lockBlock?.[1]}`);

const pl = JSON.parse(read(F.packageLock));
checkSync(
  "package-lock.json同期",
  pl.version === canonical && pl.packages?.[""]?.version === canonical,
  `package-lock=${pl.version}`,
);

// tauri.conf.jsonのversionがパス参照なら解決して比較 (Tauri v2仕様)
const tauriVer = JSON.parse(read(F.tauriConf)).version;
const resolved =
  typeof tauriVer === "string" && tauriVer.endsWith("package.json")
    ? JSON.parse(read(resolve(dirname(F.tauriConf), tauriVer))).version
    : tauriVer;
check("Tauri解決版の同期", resolved === canonical, `tauri=${tauriVer} 解決=${resolved}`);

// User-Agentは固定版を持たずCargo版へ追随する
const ua = read(F.httpClient);
check("User-AgentがCARGO_PKG_VERSIONへ追随", ua.includes("CARGO_PKG_VERSION"));
check("User-Agentの固定版が残っていない", !/OnCue\/\d+\.\d+\.\d+/.test(ua));
check("User-Agentの連絡先がCargo.tomlのrepositoryへ追随", ua.includes("CARGO_PKG_REPOSITORY") && !/[\w.+-]+@[\w-]+\.[\w.]+/.test(ua));

// 成果物名: oncue-<version>-<arch>.zip (+.sha256)。portable/windowsを含めない。
const archRaw = cli.arch || env.ARCH_LABEL || env.RUNNER_ARCH || env.PROCESSOR_ARCHITECTURE || "x64";
const arch = ARCH_MAP[String(archRaw).toLowerCase()] ?? String(archRaw);
check("arch表記が利用者向け", arch === "x64" || arch === "arm64", `${archRaw} -> ${arch}`);
const stem = `oncue-${canonical}-${arch}`;
check("成果物stemにportable/windowsを含まない", !/portable|windows/i.test(stem), stem);
if (!metaMode) {
  console.log(`期待ZIP: ${stem}.zip`);
  console.log(`期待SHA : ${stem}.zip.sha256`);
}
if (cli.expect) {
  check("指定成果物名の一致", basename(String(cli.expect)) === `${stem}.zip`, String(cli.expect));
}

// リリースのメタデータは本スクリプトが唯一の正本 (YAML側に版解析を持たせない)。
// プレリリース判定はタグではなく正本versionで行う (タグ不一致は失敗のため一致時は同値)。
const prerelease = /alpha|beta|rc/i.test(canonical ?? "");
const stable = SEMVER.test(canonical ?? "") && !prerelease;
const meta = {
  version: canonical,
  tag: tag ? strippedTag : "",
  tagMatch: !tag || strippedTag === canonical,
  prerelease,
  stable,
  arch,
  stem,
  zip: `${stem}.zip`,
  sha: `${stem}.zip.sha256`,
  files: [`${stem}.zip`, `${stem}.zip.sha256`],
};

if (cli.sync && !metaMode) {
  // 機械同期: 人手での複数ファイル手入力を廃止する。
  // 正規版が不正などの同期不能時は書き換えず失敗のまま。
  if (SEMVER.test(canonical ?? "") && syncFailed) {
    const toml = cargoToml.replace(
      /(\[package\][^\[]*?^version\s*=\s*")[^"]+(")/m,
      `$1${canonical}$2`,
    );
    const lock = read(F.cargoLock).replace(
      /(\[\[package\]\]\nname = "oncue"\nversion = ")[^"]+(")/,
      `$1${canonical}$2`,
    );
    pl.version = canonical;
    if (pl.packages?.[""]) pl.packages[""].version = canonical;
    writeFileSync(F.cargoToml, toml);
    writeFileSync(F.cargoLock, lock);
    writeFileSync(F.packageLock, JSON.stringify(pl, null, 2) + "\n");
    console.log("SYNC Cargo.toml / Cargo.lock / package-lock.json を正本へ寄せた");
    // 再読込→再検証し、全一致なら失敗を戻す (終了コード0)。同期不能以外が残れば失敗を維持。
    const cargoVer2 = read(F.cargoToml)
      .replace(/\r\n/g, "\n")
      .split(/^\[(?!package\])/m)[0]
      .match(/^version\s*=\s*"([^"]+)"/m)?.[1];
    const lockVer2 = read(F.cargoLock)
      .replace(/\r\n/g, "\n")
      .match(/\[\[package\]\]\nname = "oncue"\nversion = "([^"]+)"/)?.[1];
    const pl2 = JSON.parse(read(F.packageLock));
    const reok =
      cargoVer2 === canonical &&
      lockVer2 === canonical &&
      pl2.version === canonical &&
      pl2.packages?.[""]?.version === canonical;
    console.log(`${reok ? "OK  " : "NG  "}同期後の再検証`);
    failed = nonSyncFailed || !reok;
  }
} else if (env.GITHUB_ENV && !failed && !metaMode) {
  // YAML側に版解析ロジックを持たせない (ワークフローは受け取るだけ)
  appendFileSync(
    env.GITHUB_ENV,
    `version=${canonical}\narch=${arch}\nstem=${stem}\nzip=${meta.zip}\nsha=${meta.sha}\nprerelease=${prerelease}\nstable=${stable}\n`,
  );
}

if (metaMode) {
  console.log(JSON.stringify(meta));
}

process.exit(failed ? 1 : 0);
