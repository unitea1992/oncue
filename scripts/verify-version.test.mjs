// verify-version の behavior test。fixture/temp ベース。
// 実行: node --test scripts/verify-version.test.mjs (標準Nodeのみ)。
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "verify-version.mjs");

// 本物と同形の最小 UA (固定版なし・CARGO_PKG_VERSION 追随)。形式自体は変えない。
const UA = `pub const APP_USER_AGENT: &str = concat!(
    "OnCue/",
    env!("CARGO_PKG_VERSION"),
    " (Windows; contact: ",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);
`;

// package.json のみ bump した状態を再現できるよう各版を個別指定できる。
const fixture = (version, { cargo = version, lock = version, plock = version } = {}) => {
  const dir = mkdtempSync(join(tmpdir(), "vrc-ver-"));
  mkdirSync(join(dir, "src-tauri", "src", "infra"), { recursive: true });
  writeFileSync(join(dir, "package.json"), JSON.stringify({ name: "x", version }, null, 2) + "\n");
  writeFileSync(join(dir, "src-tauri", "Cargo.toml"), `[package]\nname = "oncue"\nversion = "${cargo}"\n`);
  writeFileSync(join(dir, "src-tauri", "Cargo.lock"), `[[package]]\nname = "oncue"\nversion = "${lock}"\n`);
  writeFileSync(join(dir, "src-tauri", "tauri.conf.json"), JSON.stringify({ version: "../package.json" }));
  writeFileSync(join(dir, "src-tauri", "src", "infra", "http_client.rs"), UA);
  writeFileSync(
    join(dir, "package-lock.json"),
    JSON.stringify({ version: plock, packages: { "": { version: plock } } }, null, 2) + "\n",
  );
  return dir;
};

// CI 環境変数の干渉を断つ (GITHUB_ENV への追記も抑止)。
const run = (dir, args = [], extra = {}) => {
  const env = {
    ...process.env,
    GITHUB_REF: "",
    TAG_VERSION: "",
    ARCH_LABEL: "",
    RUNNER_ARCH: "",
    PROCESSOR_ARCHITECTURE: "",
    GITHUB_ENV: "",
    ...extra,
  };
  try {
    const stdout = execFileSync(process.execPath, [SCRIPT, "--root", dir, ...args], { env, encoding: "utf8" });
    return { code: 0, stdout };
  } catch (e) {
    return { code: e.status ?? 1, stdout: String(e.stdout ?? "") };
  }
};
const metaOf = (dir, args = [], extra = {}) => ({
  res: run(dir, ["--print-metadata", ...args], extra),
  get meta() {
    return JSON.parse(this.res.stdout);
  },
});

test("SemVer channel判定 (table)", () => {
  const cases = [
    ["0.3.0", false, true],
    ["0.2.0-rc.1", true, false],
  ];
  for (const [version, prerelease, stable] of cases) {
    const dir = fixture(version);
    assert.equal(run(dir).code, 0, version);
    const { res, meta } = metaOf(dir);
    assert.equal(res.code, 0, version);
    assert.equal(meta.prerelease, prerelease, version);
    assert.equal(meta.stable, stable, version);
  }
  const { meta } = metaOf(fixture("0.3.0"));
  assert.equal(meta.version, "0.3.0");
  assert.equal(meta.stem, "oncue-0.3.0-x64");
  assert.deepEqual(meta.files, ["oncue-0.3.0-x64.zip", "oncue-0.3.0-x64.zip.sha256"]);
});

test("invalid SemVer は fail・--sync でも fail 維持", () => {
  const dir = fixture("01.2.3");
  assert.equal(run(dir).code, 1);
  assert.equal(run(dir, ["--sync"]).code, 1);
});

test("tag 一致は exit 0・不一致は fail", () => {
  const dir = fixture("0.2.0-rc.1");
  assert.equal(run(dir, [], { TAG_VERSION: "v0.2.0-rc.1" }).code, 0);
  assert.equal(run(dir, [], { TAG_VERSION: "v9.9.9" }).code, 1);
  const { res, meta } = metaOf(dir, [], { TAG_VERSION: "v9.9.9" });
  assert.equal(res.code, 1);
  assert.equal(meta.tagMatch, false);
});

test("package.json のみ bump→--sync で exit 0＋全同期", () => {
  const dir = fixture("9.9.9-rc.2", { cargo: "0.0.0", lock: "0.0.0", plock: "0.0.0" });
  assert.equal(run(dir).code, 1);
  const synced = run(dir, ["--sync"]);
  assert.equal(synced.code, 0);
  assert.match(readFileSync(join(dir, "src-tauri", "Cargo.toml"), "utf8"), /version = "9\.9\.9-rc\.2"/);
  assert.match(readFileSync(join(dir, "src-tauri", "Cargo.lock"), "utf8"), /version = "9\.9\.9-rc\.2"/);
  const plock = JSON.parse(readFileSync(join(dir, "package-lock.json"), "utf8"));
  assert.equal(plock.version, "9.9.9-rc.2");
  assert.equal(plock.packages[""].version, "9.9.9-rc.2");
  assert.equal(run(dir).code, 0);
});

test("arch mapping (x64/X64/AMD64/arm64)", () => {
  const dir = fixture("0.3.0");
  for (const [input, arch] of [["x64", "x64"], ["X64", "x64"], ["AMD64", "x64"], ["arm64", "arm64"]]) {
    const { res, meta } = metaOf(dir, [], { ARCH_LABEL: input });
    assert.equal(res.code, 0);
    assert.equal(meta.arch, arch);
  }
  const { meta } = metaOf(dir, ["--arch", "x86_64-pc-windows-msvc"]);
  assert.equal(meta.arch, "x64");
});

// 期待ZIP名の一致/不一致は上記stableのfiles代表テストへ統合したため削除。
