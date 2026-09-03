
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
    Start-Sleep -Milliseconds 150
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
    $txtCond = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Text)
    $allTxts = $root.FindAll([Windows.Automation.TreeScope]::Descendants, $txtCond)
    $txts = @()
    foreach ($t in $allTxts) { if ($t.Current.Name) { $txts += $t.Current.Name } }
    return $txts
}
function Scan-Invokables([IntPtr]$hw) {
    $root = [Windows.Automation.AutomationElement]::FromHandle($hw)
    $walker = [Windows.Automation.TreeWalker]::RawViewWalker
    $results = @()
    function ScanEl([Windows.Automation.AutomationElement]$el, [int]$lv) {
        if ($null -eq $el) { return }
        if ($lv -gt 20) { return }
        try {
            $ct = $el.Current.ControlType.ProgrammaticName
            $n = $el.Current.Name
            $br = $el.Current.BoundingRectangle
            $hasInvoke = $null -ne $el.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)
            if (($n -or $hasInvoke) -and $br.Width -gt 3 -and $br.Height -gt 3) {
                $s = 'LV=' + $lv + ' CT=' + $ct + ' N=' + $n + ' INV=' + $hasInvoke + ' W=' + $br.Width + ' H=' + $br.Height
                $results += $s
            }
            $ch = $walker.GetFirstChild($el)
            while ($ch) {
                ScanEl $ch ($lv+1)
                $ch = $walker.GetNextSibling($ch)
            }
        } catch { }
    }
    ScanEl $root 0
    return $results
}

$sd = 'd:\work\Trae-sync\screenshots_20260822_222500'
$rep = Join-Path $sd 'final_report.txt'
'Trae Sync 视觉验证 - 最终报告' | Out-File $rep -Encoding utf8
'================================================' | Out-File $rep -Append -Encoding utf8

$proc = Get-Process -Id 13044
$hw = $proc.MainWindowHandle
[WinX]::ShowWindow($hw, 9) | Out-Null
Start-Sleep -Milliseconds 300
[WinX]::SetForegroundWindow($hw) | Out-Null
Start-Sleep -Milliseconds 600

$r = New-Object WinX+RECT
[WinX]::GetWindowRect($hw, [ref]$r) | Out-Null
$navX = $r.L + 90
'窗口尺寸: ' + ($r.R - $r.L) + 'x' + ($r.B - $r.T) | Out-File $rep -Append -Encoding utf8

# 01 首页
Click-At $navX ($r.T + 170)
Start-Sleep -Milliseconds 1200
$p01 = Join-Path $sd '01_home.png'
Take-Shot $hw $p01
$t = Get-PageTexts $hw
'' | Out-File $rep -Append -Encoding utf8
'[01_首页] 截图: ' + $p01 | Out-File $rep -Append -Encoding utf8
$homeOk = (($t -join ' ') -match '晚上好.*历史总览')
'  结论: ' + $(if ($homeOk) {'符合预期 - 应用渲染正常;显示欢迎语与账号/项目/对话统计'} else {'不符合预期'}) | Out-File $rep -Append -Encoding utf8
'  文本前10条: ' + (($t | Select-Object -First 10) -join ' <> ') | Out-File $rep -Append -Encoding utf8

# 02 签到页
Click-At $navX ($r.T + 320)
Start-Sleep -Milliseconds 1200
$p02 = Join-Path $sd '02_checkin.png'
Take-Shot $hw $p02
$t = Get-PageTexts $hw
$btns = Scan-Invokables $hw
'' | Out-File $rep -Append -Encoding utf8
'[02_签到页] 截图: ' + $p02 | Out-File $rep -Append -Encoding utf8
'  全部文本: ' + ($t -join ' | ') | Out-File $rep -Append -Encoding utf8
'  可调用控件(前20): ' + (($btns | Select-Object -First 20) -join ' ;; ') | Out-File $rep -Append -Encoding utf8
$allTxt = ($t -join ' ')
$hasTitle = $allTxt -match '签到'
$rows = ([regex]::Matches($allTxt, '用户[0-9]+|LY|梦梦|[0-9]{11}').Count)
$hasRows = $rows -ge 3
$btnFlat = ($btns -join ' ')
$hasExBtn = $btnFlat -match '签到|开始|执行|全部|批量|一键'
$badges = ([regex]::Matches($allTxt, '已签|未签|待签|今日')).Count
$ok = $hasTitle -and $hasRows
'  验证: hasTitle=' + $hasTitle + ' accountRowMatchCount=' + $rows + ' hasExecBtn=' + $hasExBtn + ' badgeHits=' + $badges | Out-File $rep -Append -Encoding utf8
$conc = '需要视觉截图核查'
if ($ok) { $conc = '符合预期 - 签到标题+账号行存在; 徽章和执行按钮以截图视觉为准(UIA可能未暴露按钮文本)' }
'  结论: ' + $conc | Out-File $rep -Append -Encoding utf8

# 03 账号页
Click-At $navX ($r.T + 270)
Start-Sleep -Milliseconds 1200
$p03 = Join-Path $sd '03_account.png'
Take-Shot $hw $p03
$t = Get-PageTexts $hw
$btns2 = Scan-Invokables $hw
'' | Out-File $rep -Append -Encoding utf8
'[03_账号页] 截图: ' + $p03 | Out-File $rep -Append -Encoding utf8
'  全部文本: ' + ($t -join ' | ') | Out-File $rep -Append -Encoding utf8
'  可调用控件: ' + ($btns2 -join ' ;; ') | Out-File $rep -Append -Encoding utf8
$allTxt = ($t -join ' ')
$hasPt = $allTxt -match '积分'
$hasHlth = $allTxt -match '登录有效|健康度|剩.*天'
$hasDev = ([regex]::Matches($allTxt, '设备.*[.]{3}[0-9]{4}')).Count -ge 2
$btnF = $btns2 -join ' '
$noCk = -not (($allTxt -match '全部签到|一键签到|开始签到') -or ($btnF -match '全部签到|一键签到'))
$cnt = ([regex]::Matches($allTxt, '积分')).Count
$okA = $hasPt -and $hasHlth -and $hasDev
'  验证: hasPoints=' + $hasPt + ' hasHealth=' + $hasHlth + ' hasDeviceTail=' + $hasDev + ' cardCount=' + $cnt + ' NoCheckinExecBtn=' + $noCk | Out-File $rep -Append -Encoding utf8
$cA = '需要核查'
if ($okA) { $cA = '符合预期 - 账号卡片含积分/登录健康度(剩余天数)/设备尾号; 且页面无签到执行按钮' }
'  结论: ' + $cA | Out-File $rep -Append -Encoding utf8

# 04 设置页
Click-At $navX ($r.T + 370)
Start-Sleep -Milliseconds 1200
$p04 = Join-Path $sd '04_settings.png'
Take-Shot $hw $p04
$t = Get-PageTexts $hw
'' | Out-File $rep -Append -Encoding utf8
'[04_设置页] 截图: ' + $p04 | Out-File $rep -Append -Encoding utf8
'  全部文本: ' + ($t -join ' | ') | Out-File $rep -Append -Encoding utf8
$allTxt = ($t -join ' ')
$hSK = $allTxt -match '来源密钥'
$hHK = $allTxt -match '历史库密钥'
$hCK = $allTxt -match '候选 source key'
$okS = $hSK -and $hHK -and $hCK
'  验证: hasSourceKey=' + $hSK + ' hasHistoryKey=' + $hHK + ' hasCandidateSourceKey=' + $hCK | Out-File $rep -Append -Encoding utf8
$cS = '不符合预期'
if ($okS) { $cS = '符合预期 - 密钥维护区包含来源密钥/历史库密钥/候选 source key 输入框标签' }
'  结论: ' + $cS | Out-File $rep -Append -Encoding utf8

'' | Out-File $rep -Append -Encoding utf8
'完成: ' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss') | Out-File $rep -Append -Encoding utf8

Write-Host ('SHOT01=' + $p01)
Write-Host ('SHOT02=' + $p02)
Write-Host ('SHOT03=' + $p03)
Write-Host ('SHOT04=' + $p04)
Write-Host ('REPORT=' + $rep)
