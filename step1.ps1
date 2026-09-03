
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$code3 = @'
using System;
using System.Runtime.InteropServices;
public class WinX {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern void SetCursorPos(int X, int Y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, int dx, int dy, uint cButtons, UIntPtr dwExtraInfo);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
    public const uint MOUSEEVENTF_LEFTDOWN = 0x02;
    public const uint MOUSEEVENTF_LEFTUP = 0x04;
}
'@
Add-Type -TypeDefinition $code3 -PassThru | Out-Null

function Click-At([int]$x, [int]$y) {
    [WinX]::SetCursorPos($x, $y)
    Start-Sleep -Milliseconds 60
    [WinX]::mouse_event([WinX]::MOUSEEVENTF_LEFTDOWN, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 40
    [WinX]::mouse_event([WinX]::MOUSEEVENTF_LEFTUP, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 60
}

function Take-Shot([IntPtr]$hw, [string]$path) {
    $r = New-Object WinX+RECT
    [WinX]::GetWindowRect($hw, [ref]$r) | Out-Null
    $w = $r.R - $r.L; $h = $r.B - $r.T
    $bmp = New-Object System.Drawing.Bitmap($w, $h)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

function Get-PageTexts([IntPtr]$hw) {
    $root = [Windows.Automation.AutomationElement]::FromHandle($hw)
    $txtCond = New-Object Windows.Automation.PropertyCondition(
        [Windows.Automation.AutomationElement]::ControlTypeProperty,
        [Windows.Automation.ControlType]::Text)
    $allTxts = $root.FindAll([Windows.Automation.TreeScope]::Descendants, $txtCond)
    $txts = @()
    foreach ($t in $allTxts) { if ($t.Current.Name) { $txts += $t.Current.Name } }
    return ($txts -join ' | ')
}

$sd = 'd:\work\Trae-sync\screenshots_20260822_222500'
if (-not (Test-Path $sd)) { New-Item -ItemType Directory -Path $sd -Force | Out-Null }

$proc = Get-Process -Id 13044
$hw = $proc.MainWindowHandle
[WinX]::ShowWindow($hw, 9) | Out-Null
Start-Sleep -Milliseconds 200
[WinX]::SetForegroundWindow($hw) | Out-Null
Start-Sleep -Milliseconds 400

$r = New-Object WinX+RECT
[WinX]::GetWindowRect($hw, [ref]$r) | Out-Null
Write-Host "WIN_RECT: L=$($r.L) T=$($r.T) R=$($r.R) B=$($r.B) SIZE=$($r.R-$r.L)x$($r.B-$r.T)"

$navX = $r.L + 90
Write-Host "NAV_X_ABS=$navX"

# Step 1: 首页截图
$p01 = Join-Path $sd '01_home.png'
Take-Shot $hw $p01
Write-Host "SHOT1_HOME=$p01"
$homeTxt = Get-PageTexts $hw
if ($homeTxt.Length -gt 3000) { $homeTxt = $homeTxt.Substring(0, 3000) }
Write-Host "HOME_TEXTS=$homeTxt"

# Step 2: 找到签到导航
$yOffsets = @(120, 170, 220, 270, 320, 370, 420, 470)
$checkinFound = $false
$checkinShot = ""
$checkinPageText = ""

foreach ($yo in $yOffsets) {
    $screenY = $r.T + $yo
    Write-Host "CHECKIN_TRY_Y=$yo"
    Click-At $navX $screenY
    Start-Sleep -Milliseconds 900
    $shotPath = Join-Path $sd ("02_checkin_try_y" + $yo + ".png")
    Take-Shot $hw $shotPath
    $txt = Get-PageTexts $hw
    Write-Host "  LEN=$($txt.Length)"
    if ($txt -match '签到|今日已签|未签|全部签到') {
        Write-Host "  MATCHED_CHECKIN"
        $checkinFound = $true
        $checkinPageText = $txt
        $checkinShot = Join-Path $sd '02_checkin.png'
        Take-Shot $hw $checkinShot
        break
    }
}

if (-not $checkinFound) {
    $checkinShot = Join-Path $sd '02_checkin_fallback.png'
    Take-Shot $hw $checkinShot
    $checkinPageText = Get-PageTexts $hw
    Write-Host "CHECKIN_FALLBACK"
}
if ($checkinPageText.Length -gt 3500) { $checkinPageText = $checkinPageText.Substring(0, 3500) }
Write-Host "FINAL_CHECKIN_SHOT=$checkinShot"
Write-Host "CHECKIN_FOUND=$checkinFound"
Write-Host "CHECKIN_PAGE_TEXTS=$checkinPageText"
