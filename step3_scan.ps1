
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
    Start-Sleep -Milliseconds 80
    [WinX]::mouse_event([WinX]::MOUSEEVENTF_LEFTDOWN, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 50
    [WinX]::mouse_event([WinX]::MOUSEEVENTF_LEFTUP, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 100
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
    return $txts
}

$sd = 'd:\work\Trae-sync\screenshots_20260822_222500'
$logPath = Join-Path $sd 'scan_log.txt'
"" | Out-File $logPath -Encoding utf8

$proc = Get-Process -Id 13044
$hw = $proc.MainWindowHandle
[WinX]::ShowWindow($hw, 9) | Out-Null
Start-Sleep -Milliseconds 300
[WinX]::SetForegroundWindow($hw) | Out-Null
Start-Sleep -Milliseconds 500

$r = New-Object WinX+RECT
[WinX]::GetWindowRect($hw, [ref]$r) | Out-Null
$navX = $r.L + 90
"NAV_X=$navX WIN=$($r.R-$r.L)x$($r.B-$r.T)" | Out-File $logPath -Append -Encoding utf8

$positions = @(170, 220, 270, 320, 370, 420, 500, 600, 700, 780)

foreach ($yo in $positions) {
    $screenY = $r.T + $yo
    Click-At $navX $screenY
    Start-Sleep -Milliseconds 1000
    $arr = Get-PageTexts $hw
    $shotP = Join-Path $sd ("scan_y$yo.png")
    Take-Shot $hw $shotP
    $line = "Y=$yo CNT=$($arr.Count) SHOT=$shotP :: " + ($arr -join " | ")
    if ($line.Length -gt 4000) { $line = $line.Substring(0,4000) }
    $line | Out-File $logPath -Append -Encoding utf8
}
"DONE" | Out-File $logPath -Append -Encoding utf8
Write-Host "LOG=$logPath"
