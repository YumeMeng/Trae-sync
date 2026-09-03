
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
    return $txts
}

$sd = 'd:\work\Trae-sync\screenshots_20260822_222500'
$proc = Get-Process -Id 13044
$hw = $proc.MainWindowHandle
[WinX]::ShowWindow($hw, 9) | Out-Null
Start-Sleep -Milliseconds 200
[WinX]::SetForegroundWindow($hw) | Out-Null
Start-Sleep -Milliseconds 500

$r = New-Object WinX+RECT
[WinX]::GetWindowRect($hw, [ref]$r) | Out-Null
$navX = $r.L + 90
Write-Host "NAV_X=$navX"

# 更密集地采样每个导航位置，保存详细文本，输出编号列表
# 先用更小的步长从上到下扫描
$allY = @(90, 120, 145, 170, 195, 220, 245, 270, 295, 320, 345, 370, 395, 420, 445, 470, 500, 530, 560, 600, 650, 700, 750, 800)

foreach ($yo in $allY) {
    $screenY = $r.T + $yo
    Click-At $navX $screenY
    Start-Sleep -Milliseconds 900
    $arr = Get-PageTexts $hw
    $joined = $arr -join ' <<<>> '
    Write-Host "========= Y=$yo  COUNT=$($arr.Count) ========="
    if ($joined.Length -gt 5000) { $joined = $joined.Substring(0,5000) }
    Write-Host $joined
    # 对 220、270、320、370 存截图
    if ($yo -eq 220) { Take-Shot $hw (Join-Path $sd 'detailed_y220.png') }
    if ($yo -eq 270) { Take-Shot $hw (Join-Path $sd 'detailed_y270.png') }
    if ($yo -eq 320) { Take-Shot $hw (Join-Path $sd 'detailed_y320.png') }
    if ($yo -eq 370) { Take-Shot $hw (Join-Path $sd 'detailed_y370.png') }
    if ($yo -eq 170) { Take-Shot $hw (Join-Path $sd 'detailed_y170.png') }
}
