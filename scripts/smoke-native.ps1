<#
.SYNOPSIS
  Layer Bのネイティブスモークテスト。リリース互換の実行ファイルを実起動し、UIAutomationで描画・操作性を実測する。
.DESCRIPTION
  .NET UIAutomation (UIAutomationClient/UIAutomationTypes) のみ使用。新規依存なし。
  SendKeys (System.Windows.Forms, Windows同梱) はTab移動の実測にのみ使う。
  手動実行の正本は `pwsh -File .\smoke-native.ps1 -ExePath ".\oncue.exe"`。
  Windows PowerShell 5.1 では日本語解析で失敗し得るため、実行には pwsh (PowerShell 7+) を使うこと。
  製品相当のバイナリに対し以下を実測する:
  プロセス起動・メインウィンドウ生成・ウィンドウ寸法 (tauri.conf.json準拠)・
  ユーザー名・パスワード入力欄へのフォーカス・Tab移動・ボタン到達・正常終了。
  ウィンドウ名の空 (WINDOW_TITLE_EMPTY)・ドキュメント検出 (WEBVIEW2_DOCUMENT_HINT) は診断の手掛かりであり、合否条件ではない。
  タイトル空・ドキュメント不在でもセマンティックコントロール (WEB_CONTENT_UIA 以降) が通れば PASSED になる。
  非対話デスクトップ・WebView2欠落はUNVERIFIEDにせず失敗させる。RCの合否判定を成功にしない。
  フォーム送信はしない (ボタン到達まで。認証は実行しない)。
.NOTES
  診断出力は無害化した項目 (ControlType名・AutomationId・Name・IsOffscreen・IsEnabled) のみ出す。
  ValuePatternの値・編集内容・資格情報 (ユーザー名・パスワード値)・location等の秘密の可能性がある値は絶対に出さない。
  失敗時限定の診断出力以外は、測定結果の真偽値と寸法のみ出す。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$ExePath,
  [int]$StartupTimeoutSeconds = 30,
  [int]$CloseTimeoutSeconds = 10
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $ExePath)) { throw "EXE not found: $ExePath" }

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Windows.Forms

# 1. 実測の前提。欠けたら失敗させる (UNVERIFIED成功にしない)。
$interactive = [Environment]::UserInteractive -and ((Get-Process -Id $PID).SessionId -ne 0)
if (-not $interactive) { throw 'NO_INTERACTIVE_DESKTOP: 対話desktopなしのため実測不可' }
$wv2present = (Test-Path 'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}') -or (Test-Path 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}')
if (-not $wv2present) { throw 'WEBVIEW2_MISSING: WebView2なしのため実測不可' }

$AE = [System.Windows.Automation.AutomationElement]
$scopeDesc = [System.Windows.Automation.TreeScope]::Descendants

function Find-Descendant($root, $condition) {
  $root.FindFirst($scopeDesc, $condition)
}

function Find-Control($root, $controlType) {
  Find-Descendant $root (New-Object System.Windows.Automation.PropertyCondition(
    $AE::ControlTypeProperty, $controlType))
}

# 失敗時限定の無害化UIA下位ツリー出力。通常PASSED時は呼ばない。
# メインウィンドウ配下のDescendantsを最大30要素まで列挙する。
# 出力は ControlType名・AutomationId・Name・IsOffscreen・IsEnabled の5項目のみ。
# ValuePattern値・編集内容・資格情報 (ユーザー名・パスワード値)・location等の秘密の可能性がある値は絶対に出さない。
# 診断であり合否条件にしない (PASSED/FAILED判定に使わない)。
function Write-SanitizedUiaDump($window) {
  try {
    $all = $window.FindAll($scopeDesc, [System.Windows.Automation.Condition]::TrueCondition)
  } catch {
    Write-Output 'UIA_DUMP_UNAVAILABLE=true'
    return
  }
  Write-Output ('UIA_DUMP_COUNT=' + $all.Count)
  $n = [Math]::Min(30, $all.Count)
  for ($i = 0; $i -lt $n; $i++) {
    try {
      $c = $all[$i].Current
      $typeName = $c.ControlType.ProgrammaticName
      Write-Output ('UIA_DUMP=[' + $i + '] Type=' + $typeName + ' AutomationId=' + $c.AutomationId + ' Name=' + $c.Name + ' IsOffscreen=' + $c.IsOffscreen + ' IsEnabled=' + $c.IsEnabled)
    } catch {
      Write-Output ('UIA_DUMP=[' + $i + '] UNAVAILABLE=true')
    }
  }
}

$p = $null
try {
  # 2. プロセス起動
  $p = Start-Process -FilePath $ExePath -PassThru
  Write-Output 'PROCESS_STARTED=true'

  # 3. メインウィンドウ生成待ち。早期終了は隠さず失敗させる。
  $deadline = (Get-Date).AddSeconds($StartupTimeoutSeconds)
  $hwnd = [IntPtr]::Zero
  while ((Get-Date) -lt $deadline) {
    $p.Refresh()
    if ($p.HasExited) { throw ('APP_EXITED_EARLY EXIT_CODE=' + $p.ExitCode) }
    if ($p.MainWindowHandle -ne [IntPtr]::Zero) { $hwnd = $p.MainWindowHandle; break }
    Start-Sleep -Milliseconds 500
  }
  if ($hwnd -eq [IntPtr]::Zero) { throw 'MAIN_WINDOW_TIMEOUT: main windowが生成されなかった' }
  Write-Output 'MAIN_WINDOW=true'

  $window = $AE::FromHandle($hwnd)
  if ($null -eq $window) { throw 'UIA_FROM_HANDLE_FAILED' }

  # 4. ウィンドウ名 (tauri.conf.json: "OnCue")。診断のみ。タイトル空でも継続し後段のセマンティックコントロールが判定する。
  if ([string]::IsNullOrWhiteSpace($window.Current.Name)) { Write-Warning 'WINDOW_TITLE_EMPTY: 空titleのため診断のみで継続'; Write-Output 'WINDOW_TITLE_EMPTY=true' } else { Write-Output 'WINDOW_TITLE=true' }

  # 5. ウィンドウ寸法 (tauri.conf.jsonのwidth/heightを正本とする。外枠込みのため±80px許容)
  $tauriConf = Join-Path $PSScriptRoot '..' 'src-tauri' 'tauri.conf.json'
  if (-not (Test-Path -LiteralPath $tauriConf)) { throw "TAURI_CONF_MISSING: $tauriConf" }
  $conf = Get-Content -LiteralPath $tauriConf -Raw | ConvertFrom-Json
  $expW = [int]$conf.app.windows[0].width
  $expH = [int]$conf.app.windows[0].height
  if ($expW -le 0 -or $expH -le 0) { throw 'TAURI_WINDOW_SIZE_INVALID' }
  $rect = $window.Current.BoundingRectangle
  $w = [int]$rect.Width
  $h = [int]$rect.Height
  Write-Output ('WINDOW_SIZE=' + $w + 'x' + $h)
  if ([Math]::Abs($w - $expW) -gt 80 -or [Math]::Abs($h - $expH) -gt 80) {
    throw ('WINDOW_SIZE_UNEXPECTED: ' + $w + 'x' + $h + ' expected ' + $expW + 'x' + $expH + '+-80')
  }
  Write-Output 'WINDOW_SIZE_OK=true'

  # 6. WebView2描画の手掛かり: Document descendant (Chromium page) の有無を出すだけ。失敗させない。
  # 合否条件ではない。ドキュメント不在でも後段のセマンティックコントロールが通ればPASSEDになる。
  $doc = Find-Control $window ([System.Windows.Automation.ControlType]::Document)
  Write-Output ('WEBVIEW2_DOCUMENT_HINT=' + ($null -ne $doc).ToString().ToLowerInvariant())

  # 7. ユーザー名・パスワード入力欄の bounded poll (timeout 15秒・interval 300ms)。
  # AutomationId `login-username` / `login-password` 優先、なければEdit列挙先頭2件。
  # Web描画待ちのため繰り返し確認する。各反復でプロセス早期終了は隠さず失敗させる。
  $CT = [System.Windows.Automation.ControlType]
  $editCond = New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $CT::Edit)
  $userEdit = $null
  $passEdit = $null
  $pollDeadline = (Get-Date).AddSeconds(15)
  while ((Get-Date) -lt $pollDeadline) {
    $p.Refresh()
    if ($p.HasExited) { throw 'APP_EXITED_BEFORE_WEB_CONTENT' }
    $userEdit = Find-Descendant $window (New-Object System.Windows.Automation.AndCondition(
      $editCond,
      (New-Object System.Windows.Automation.PropertyCondition($AE::AutomationIdProperty, 'login-username'))))
    $passEdit = Find-Descendant $window (New-Object System.Windows.Automation.AndCondition(
      $editCond,
      (New-Object System.Windows.Automation.PropertyCondition($AE::AutomationIdProperty, 'login-password'))))
    if ($null -eq $userEdit -or $null -eq $passEdit) {
      $edits = $window.FindAll($scopeDesc, $editCond)
      if ($edits.Count -ge 2) {
        if ($null -eq $userEdit) { $userEdit = $edits[0] }
        if ($null -eq $passEdit) { $passEdit = $edits[1] }
      }
    }
    if ($null -ne $userEdit -and $null -ne $passEdit) { break }
    Start-Sleep -Milliseconds 300
  }
  if ($null -eq $userEdit -or $null -eq $passEdit) {
    # 失敗時のみ無害化出力を出してから失敗させる。
    Write-SanitizedUiaDump $window
    throw 'WEB_CONTENT_UIA_MISSING'
  }
  Write-Output 'WEB_CONTENT_UIA=true'
  Write-Output 'LOGIN_INPUTS=true'

  # 8. 入力欄フォーカス (各々SetFocusしHasKeyboardFocusを実測)
  $userEdit.SetFocus()
  Start-Sleep -Milliseconds 300
  if (-not $userEdit.Current.HasKeyboardFocus) { throw 'USERNAME_FOCUS_FAILED' }
  Write-Output 'USERNAME_FOCUS=true'
  $passEdit.SetFocus()
  Start-Sleep -Milliseconds 300
  if (-not $passEdit.Current.HasKeyboardFocus) { throw 'PASSWORD_FOCUS_FAILED' }
  Write-Output 'PASSWORD_FOCUS=true'

  # 9. Tab移動: ユーザー名にフォーカス→Tab→パスワードへ移動を実測
  $userEdit.SetFocus()
  Start-Sleep -Milliseconds 300
  [System.Windows.Forms.SendKeys]::SendWait('{TAB}')
  Start-Sleep -Milliseconds 500
  if (-not $passEdit.Current.HasKeyboardFocus) { throw 'TAB_NAVIGATION_FAILED' }
  Write-Output 'TAB_NAVIGATION=true'

  # 10. ボタン到達 (送信はしない): Name「ログイン」優先、なければ先頭の有効Button
  $btnCond = New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $CT::Button)
  $submit = Find-Descendant $window (New-Object System.Windows.Automation.AndCondition(
    $btnCond,
    (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, 'ログイン'))))
  if ($null -eq $submit) {
    $enabled = @($window.FindAll($scopeDesc, $btnCond) | Where-Object { $_.Current.IsEnabled })
    $submit = $enabled | Select-Object -First 1
  }
  if ($null -eq $submit) { throw 'LOGIN_BUTTON_MISSING' }
  if (-not $submit.Current.IsEnabled) { throw 'LOGIN_BUTTON_DISABLED' }
  if ($submit.Current.IsOffscreen) { throw 'LOGIN_BUTTON_OFFSCREEN' }
  Write-Output 'LOGIN_BUTTON=true'

  # 11. 正常終了。閉じなければ失敗させる (強制終了の隠蔽なし)。
  $p.CloseMainWindow() | Out-Null
  if (-not $p.WaitForExit($CloseTimeoutSeconds * 1000)) { throw 'CLOSE_TIMEOUT: 正常終了しなかった' }
  Write-Output ('EXIT_CODE=' + $p.ExitCode)
  if ($p.ExitCode -ne 0) { throw ('EXIT_CODE_NONZERO: ' + $p.ExitCode) }
  Write-Output 'GRACEFUL_EXIT=true'
  Write-Output 'SMOKE=PASSED'
} finally {
  if ($null -ne $p -and -not $p.HasExited) {
    try { Stop-Process -Id $p.Id -Force } catch { }
  }
}
