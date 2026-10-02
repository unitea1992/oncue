<#
.SYNOPSIS
  リリース配布物 (ZIP + .sha256) の包装正本。Build Portable 実績の抽出。
.DESCRIPTION
  Build Portable (#134実績) のワークフローに直書きされていた包装処理をそのまま抽出したもの。
  Compress-Archive / Get-FileHash (OS標準) を使い、自前ZIP codecは持たない。
  版名は組み立てない。入力は scripts/verify-version.mjs の出力
  (stem/zip/sha) をそのまま受け取る。SemVer解析・stem生成はしない
  (版の唯一の正本は verify-version.mjs)。
  責務: 出力ディレクトリ準備 / README+LICENSE同梱 / 混入除外 /
  正規名のそのまま使用 / SHA256生成 / 要約追記 / 安全側で失敗。
.PARAMETER Stem
  verify-version.mjs の stem (例 oncue-0.1.0-x64)。stage dir名にも使う。
.PARAMETER Zip
  verify-version.mjs の zip (例 oncue-0.1.0-x64.zip)。"$Stem.zip" と一致必須。
.PARAMETER Sha
  verify-version.mjs の sha (例 oncue-0.1.0-x64.zip.sha256)。"$Zip.sha256" と一致必須。
.PARAMETER ExePath
  リリース実行ファイル。既定は <repo>/src-tauri/target/release/oncue.exe。
.PARAMETER OutDir
  ZIP/.sha256 の出力先。既定はリポジトリルート。
  validate-windows は workdir を渡す。
.PARAMETER Summary
  指定時のみ `zip=` / `sha256=` を追記するファイル。
.NOTES
  案A/B敵対比較の結論は案B (本スクリプト)。
  - 案A (Node自前ZIP codec約130行の旧包装スクリプト。削除済み): codec保守が残る上、
    zipFileName() が旧portable/windows名を吐き版ゲートの正規命名と既に乖離。
    二重の版解析 (独自VERSION_RE) もdriftの原因。CI包装は Windows ランナー固定のため
    クロスプラットフォーム対応の実利なし。
  - 案B (本スクリプト): OS標準cmdletのみでcodec保守ゼロ。命名は版ゲートの
    出力をそのまま使い、portable/windows混入は検査で失敗させる。
    以前ワークフローに直書きされていた処理と同一動作の抽出であり振る舞い変更なし。
  反証 (例: Windows外のランナーでの包装要求) は現行のruns-onに存在しないため案Bを採用する。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$Stem,
  [Parameter(Mandatory = $true)][string]$Zip,
  [Parameter(Mandatory = $true)][string]$Sha,
  [string]$ExePath = '',
  [string]$OutDir = '',
  [string]$Summary = ''
)
$ErrorActionPreference = 'Stop'

# 1. 版ゲート出力の受領 (空の場合は安全側で失敗させる。組み立て直さない)
if (-not $Stem) { throw 'version metadata missing: Stem is empty (Check version unity must run first)' }
if (-not $Zip) { throw 'version metadata missing: Zip is empty (Check version unity must run first)' }
if (-not $Sha) { throw 'version metadata missing: Sha is empty (Check version unity must run first)' }

# 2. 正規名称の整合確認 (SemVer解析なしの文字照合のみ)
if ($Zip -ne "$Stem.zip") { throw "Zip must equal Stem + '.zip': Zip=$Zip Stem=$Stem" }
if ($Sha -ne "$Zip.sha256") { throw "Sha must equal Zip + '.sha256': Sha=$Sha Zip=$Zip" }
foreach ($name in @($Stem, $Zip, $Sha)) {
  if ($name -match 'portable|windows') { throw "Non-canonical artifact name (portable/windows forbidden): $name" }
}

# 3. パス解決 (既定はリポジトリルート起点)
$repoRoot = Split-Path $PSScriptRoot -Parent
if (-not $ExePath) { $ExePath = Join-Path $repoRoot 'src-tauri\target\release\oncue.exe' }
if (-not $OutDir) { $OutDir = $repoRoot }
if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) { throw "Release exe not found: $ExePath" }
$licensePath = Join-Path $repoRoot 'LICENSE'
if (-not (Test-Path -LiteralPath $licensePath -PathType Leaf)) { throw "LICENSE not found: $licensePath" }
$readmePath = Join-Path $repoRoot 'README.md'

$stageDir = Join-Path $OutDir $Stem
if ($stageDir -eq $OutDir) { throw "stageDir must differ from outDir: $stageDir" }
if ($stageDir -eq $repoRoot) { throw "stageDir must not be the repo root: $stageDir" }

# 4. stage作り直し (自身のstageのみ。作業ディレクトリ掃除はしない)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if (Test-Path -LiteralPath $stageDir) { Remove-Item -LiteralPath $stageDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stageDir | Out-Null
Copy-Item -LiteralPath $ExePath -Destination (Join-Path $stageDir 'oncue.exe')
Copy-Item -LiteralPath $licensePath -Destination (Join-Path $stageDir 'LICENSE')
if (Test-Path -LiteralPath $readmePath -PathType Leaf) {
  Copy-Item -LiteralPath $readmePath -Destination (Join-Path $stageDir 'README.md')
}

# 5. 混入防止: 実行時生成物が紛れ込んだら失敗させる (#134実績と同一の4件)
foreach ($leak in @('data', 'cache', 'logs', 'session')) {
  if (Test-Path (Join-Path $stageDir $leak)) {
    throw "Forbidden directory '$leak' found in portable package"
  }
}

# 6. ZIP圧縮 (名は版ゲートの出力そのまま)
$zipPath = Join-Path $OutDir $Zip
Compress-Archive -Path $stageDir -DestinationPath $zipPath -Force

# 7. ZIP内容物検査 (名前のみ。data/cache/logs/session混入禁止)
# 注意: $Zip は版名パラメータ (string) のため衝突回避で別名を使う (PowerShellはcase-insensitive)。
Add-Type -AssemblyName System.IO.Compression.FileSystem
if (-not (Test-Path -LiteralPath $zipPath -PathType Leaf)) { throw "ZIP not created: $zipPath" }
$zipArchive = $null
try {
  $zipArchive = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
  $entries = @($zipArchive.Entries | ForEach-Object { $_.FullName })
} finally {
  if ($null -ne $zipArchive) { $zipArchive.Dispose() }
}
$bad = @($entries | Where-Object { $_ -match '(^|/)data(/|$)|(^|/)cache(/|$)|(^|/)logs(/|$)|(^|/)session(/|$)' })
if ($bad.Count -gt 0) { throw ('Forbidden entries in ZIP: ' + ($bad -join ', ')) }

# 8. SHA256 (小文字hex + 二空白 + zip名。ascii・改行なし)
$shaPath = Join-Path $OutDir $Sha
(Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLower() + '  ' + $Zip |
  Out-File -Encoding ascii -NoNewline -LiteralPath $shaPath

# 9. 要約追記 (指定時のみ)
$shaLine = Get-Content -LiteralPath $shaPath -Raw -Encoding ascii
if ($Summary) {
  $summaryDir = Split-Path $Summary -Parent
  if ($summaryDir -and -not (Test-Path -LiteralPath $summaryDir)) {
    New-Item -ItemType Directory -Force -Path $summaryDir | Out-Null
  }
  "zip=$Zip`nsha256=$shaLine`n" | Out-File -Append -Encoding utf8 -LiteralPath $Summary
}

Write-Output "stem=$Stem"
Write-Output "stage=$stageDir"
Write-Output "zip=$Zip"
Write-Output "sha256=$shaLine"
