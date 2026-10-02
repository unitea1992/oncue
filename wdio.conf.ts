// Layer A (renderer-only) の WebdriverIO 設定。
// plain Chrome + Vite dev サーバで実行し、Rust ビルドは不要。
// 同一 `npm run test:ui` で Ubuntu/Windows 双方を実行する。
//
// 注意: test-only の service/capability (tauri-plugin-wdio 相当) を
// 通常 Release の tauri.conf.json / capabilities に混入させないこと。
// wdio 用の capability はこのファイル内に閉じ込める。
import { spawn, type ChildProcess } from 'node:child_process';
import { chmod, stat } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Browser, detectBrowserPlatform, install, resolveBuildId } from '@puppeteer/browsers';

// wdio-chromedriver-service は wdio v9 系と解決しないため使わない。
// 代わりに chromedriver binary を直接立てて port で繋ぐ。
let chromedriver: ChildProcess | undefined;
let driverStderr = '';

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

async function waitForDriver(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
    } catch {
      /* 起動途中 */
    }
    if (Date.now() > deadline) {
      throw new Error(`chromedriver が ${timeoutMs}ms で起動しませんでした: ${driverStderr.slice(-500)}`);
    }
    await delay(200);
  }
}

/** system Chrome の探索。Ubuntu/Windows の通常配置 + CHROME_BINARY を見る。 */
function systemChromeBinary(): string | undefined {
  const envBin = process.env.CHROME_BINARY;
  if (envBin && existsSync(envBin)) return envBin;
  const candidates: string[] =
    process.platform === 'win32'
      ? [
          join(
            process.env.ProgramFiles ?? 'C:\\Program Files',
            'Google',
            'Chrome',
            'Application',
            'chrome.exe',
          ),
          join(
            process.env['ProgramFiles(x86)'] ?? 'C:\\Program Files (x86)',
            'Google',
            'Chrome',
            'Application',
            'chrome.exe',
          ),
          join(
            process.env.LOCALAPPDATA ?? '',
            'Google',
            'Chrome',
            'Application',
            'chrome.exe',
          ),
        ]
      : [
          '/usr/bin/google-chrome',
          '/usr/bin/chromium',
          '/usr/bin/chromium-browser',
          '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
        ];
  return candidates.find((p) => p !== '' && existsSync(p));
}

/**
 * system Chrome が無い環境 (Windows runner 等) 用。
 * chrome-headless-shell と対応 chromedriver を取得する (test-only、production不変)。
 * WDIO_FORCE_BROWSER_DOWNLOAD=1 で強制的にこちらを通す (動作確認用)。
 */
async function downloadBrowserPair(cacheDir: string): Promise<{ binary: string; driver: string }> {
  const platform = detectBrowserPlatform();
  if (!platform) throw new Error('browser platformを判定できません');
  const shellBuildId = await resolveBuildId(Browser.CHROMEHEADLESSSHELL, platform, 'stable');
  const shell = await install({ browser: Browser.CHROMEHEADLESSSHELL, buildId: shellBuildId, cacheDir });
  const driverBuildId = await resolveBuildId(Browser.CHROMEDRIVER, platform, 'stable');
  const driver = await install({ browser: Browser.CHROMEDRIVER, buildId: driverBuildId, cacheDir });
  if (process.platform !== 'win32') {
    // ダウンロード物の実行bit欠落に備える (失敗してもgateを止めない)。
    for (const p of [shell.executablePath, driver.executablePath]) {
      try {
        await stat(p).then((st) => (st.mode & 0o111 ? null : chmod(p, 0o755)));
      } catch {
        /* 起動時に判定する */
      }
    }
  }
  return { binary: shell.executablePath, driver: driver.executablePath };
}

export const config = {
  runner: 'local',
  specs: ['./tests/ui/**/*.spec.ts'],
  maxInstances: 1,
  // headless Chrome のため仮想ディスプレイは不要。wdio は DISPLAY 不在の Linux で
  // ワーカーを xvfb-run 経由で起動するが、Ubuntu 26.04 の xvfb-run は fd 3 を閉じて
  // コマンドを実行するため、fd 3 を使う wdio の IPC が壊れ全ワーカーが `write EINVAL` で落ちる。
  autoXvfb: false,
  capabilities: [
    {
      // browser mode の前提は browserName: 'tauri'。service が chrome へ書換える。
      browserName: 'tauri',
      port: 9515,
      'wdio:tauriServiceOptions': {
        mode: 'browser',
        devServerUrl: 'http://localhost:1420',
      },
      // hosted ubuntu-latest 実機で Layer A が全滅した実績あり:
      // `session not created: Chrome instance exited` (chromedriver /session)。
      // hosted sandbox 下では sandbox/shared-memory/GPU なしで起動する必要がある。
      // ローカル実行でも無害な flags のみ。headless は継続。
      'goog:chromeOptions': {
        args: ['--headless=new', '--no-sandbox', '--disable-dev-shm-usage', '--disable-gpu'],
      },
    },
  ],
  logLevel: 'error',
  baseUrl: 'http://localhost:1420',
  waitforTimeout: 10000,
  connectionRetryTimeout: 120000,
  framework: 'mocha',
  reporters: ['spec'],
  services: [
    // browser mode: invoke 境界の差替えと mock API を提供 (driver は提供しない)。
    '@wdio/tauri-service',
  ],
  async onPrepare(_config: unknown, capabilities: unknown) {
    const caps = Array.isArray(capabilities) ? (capabilities[0] as Record<string, unknown>) : undefined;
    if (!caps) throw new Error('wdio capabilities が配列ではありません');
    let driverBin = join(process.cwd(), 'node_modules', '.bin', 'chromedriver');
    if (!process.env.WDIO_FORCE_BROWSER_DOWNLOAD) {
      const sys = systemChromeBinary();
      if (sys) {
        // system Chrome があれば従来通り (driver は npm package の chromedriver)。
      } else {
        const cacheDir =
          process.env.WDIO_BROWSER_CACHE ?? join(tmpdir(), 'wdio-browsers');
        const pair = await downloadBrowserPair(cacheDir);
        driverBin = pair.driver;
        // static capabilities の args (headless + sandbox対策) を維持し binary のみ差替え。
        const prevArgs =
          (caps['goog:chromeOptions'] as { args?: string[] } | undefined)?.args ?? [];
        const mergedArgs = [...prevArgs];
        for (const flag of ['--no-sandbox', '--disable-dev-shm-usage', '--disable-gpu']) {
          if (!mergedArgs.includes(flag)) mergedArgs.push(flag);
        }
        caps['goog:chromeOptions'] = { binary: pair.binary, args: mergedArgs };
      }
    } else {
      const cacheDir = process.env.WDIO_BROWSER_CACHE ?? join(tmpdir(), 'wdio-browsers');
      const pair = await downloadBrowserPair(cacheDir);
      driverBin = pair.driver;
      // static capabilities の args (headless + sandbox対策) を維持し binary のみ差替え。
      const forcedPrevArgs =
        (caps['goog:chromeOptions'] as { args?: string[] } | undefined)?.args ?? [];
      const forcedArgs = [...forcedPrevArgs];
      for (const flag of ['--no-sandbox', '--disable-dev-shm-usage', '--disable-gpu']) {
        if (!forcedArgs.includes(flag)) forcedArgs.push(flag);
      }
      caps['goog:chromeOptions'] = { binary: pair.binary, args: forcedArgs };
    }
    chromedriver = spawn(driverBin, ['--port=9515'], {
      stdio: ['ignore', 'ignore', 'pipe'],
      shell: process.platform === 'win32',
    });
    chromedriver.stderr?.on('data', (chunk: Buffer) => {
      driverStderr += chunk.toString();
    });
    chromedriver.on('error', (err) => {
      driverStderr += String(err);
    });
    await waitForDriver('http://127.0.0.1:9515/status', 30000);
  },
  onComplete() {
    chromedriver?.kill();
    chromedriver = undefined;
  },
  mochaOpts: {
    ui: 'bdd',
    timeout: 60000,
  },
  autoCompileOpts: {
    autoCompile: true,
    tsNodeOpts: { transpileOnly: true },
  },
};
