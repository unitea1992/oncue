<#
.SYNOPSIS
  ポータブルZIPの安全なスモークテスト。実行ファイル+READMEだけを一時書き込み可能パスへ展開し、
  起動→生存→終了を検証する。生成data/cache/logsは存在有無のみ見て
  内容は取得しない。
.NOTES
  UNVERIFIEDは事前判定できる既知条件(非対話デスクトップ/WebView2なし)のみ。
  それ以外の起動例外は例外を送出し、起動不良を隠さない。強制終了した場合は
  GRACEFUL_SHUTDOWN=falseを出し、正常終了とは主張しない。
  診断・資格情報・環境出力は禁止のため、本スクリプトは以下を出さない:
  ファイル内容、環境変数、資格情報、トークン。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$ZipPath,
  [int]$SurviveSeconds = 5
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $ZipPath)) { throw "ZIP not found: $ZipPath" }

$stage = Join-Path ([System.IO.Path]::GetTempPath()) ('vrc-smoke-' + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $stage | Out-Null
try {
  # 1. ZIP内容物検査 (名前のみ。data/cache/logs混入禁止)
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $zip = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
  try { $entries = @($zip.Entries | ForEach-Object { $_.FullName }) }
  finally { $zip.Dispose() }
  Write-Output ('entries: ' + ($entries -join ', '))
  $bad = @($entries | Where-Object { $_ -match '(^|/)data(/|$)|(^|/)cache(/|$)|(^|/)logs(/|$)' })
  if ($bad.Count -gt 0) { throw ('Forbidden entries in ZIP: ' + ($bad -join ', ')) }
  $exeEntries = @($entries | Where-Object { $_ -like '*.exe' })
  if ($exeEntries.Count -ne 1) { throw 'ZIP must contain exactly one exe' }

  # 2. 書き込み可能な一時領域へ展開 (ポータブル運用の模擬)
  Expand-Archive -LiteralPath $ZipPath -DestinationPath $stage -Force
  $exe = (Get-ChildItem -Path $stage -Filter 'oncue.exe' -Recurse | Select-Object -First 1).FullName
  if (-not $exe) { throw 'exe not found after expand' }
  $work = Split-Path $exe -Parent

  # 3. UNVERIFIEDの事前判定 (既知条件のみ。ここ以外は例外送出)
  $nonInteractive = -not ([Environment]::UserInteractive -and ((Get-Process -Id $PID).SessionId -ne 0))
  if ($nonInteractive) {
    Write-Warning 'UNVERIFIED (NO_INTERACTIVE_DESKTOP): 対話desktopなしのため起動実測未確認。本結果は参考記録とする'
    Write-Output 'SMOKE=UNVERIFIED'
    return
  }
  $wv2present = $true
  try {
    $wv2present = (Test-Path 'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}') -or (Test-Path 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}')
  } catch { $wv2present = $true }
  if (-not $wv2present) {
    Write-Warning 'UNVERIFIED (WEBVIEW2_MISSING): WebView2なしのため起動実測未確認。本結果は参考記録とする'
    Write-Output 'SMOKE=UNVERIFIED'
    return
  }

  # 4. 起動 (例外は送出して隠さない)
  $p = Start-Process -FilePath $exe -WorkingDirectory $work -PassThru

  # 5. 生存検証
  Start-Sleep -Seconds $SurviveSeconds
  if ($p.HasExited) {
    Write-Output ('ALIVE=false EXIT_CODE=' + $p.ExitCode)
    Write-Output 'GRACEFUL_SHUTDOWN=na'
    if ($p.ExitCode -ne 0) { throw ('exe exited abnormally. EXIT_CODE=' + $p.ExitCode) }
    Write-Output 'SMOKE=PASSED'
  } else {
    Write-Output 'ALIVE=true'
    # 6. 終了検証 (正常に閉じなければ強制し正直に記録)
    $p.CloseMainWindow() | Out-Null
    Start-Sleep -Seconds 3
    if ($p.HasExited) {
      Write-Output 'GRACEFUL_SHUTDOWN=true'
    } else {
      Stop-Process -Id $p.Id -Force
      Write-Output 'GRACEFUL_SHUTDOWN=false'
    }
    Write-Output 'SMOKE=PASSED'
  }

  # 7. 実行時生成物の存在確認のみ (内容を読まない・同梱しない)
  foreach ($d in @('data', 'cache', 'logs')) {
    Write-Output ($d + '-generated=' + (Test-Path (Join-Path $work $d)))
  }
} finally {
  Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}
