
$sd = 'd:\work\Trae-sync\screenshots_20260822_222500'
if (Test-Path (Join-Path $sd 'scan_y170.png')) { Copy-Item (Join-Path $sd 'scan_y170.png') (Join-Path $sd '01_home.png') -Force; Write-Host 'CP_01_HOME_OK' } else { Write-Host 'MISSING scan_y170' }
if (Test-Path (Join-Path $sd 'scan_y320.png')) { Copy-Item (Join-Path $sd 'scan_y320.png') (Join-Path $sd '02_checkin.png') -Force; Write-Host 'CP_02_CHECKIN_OK' } else { Write-Host 'MISSING scan_y320' }
if (Test-Path (Join-Path $sd 'scan_y270.png')) { Copy-Item (Join-Path $sd 'scan_y270.png') (Join-Path $sd '03_account.png') -Force; Write-Host 'CP_03_ACCOUNT_OK' } else { Write-Host 'MISSING scan_y270' }
if (Test-Path (Join-Path $sd 'scan_y370.png')) { Copy-Item (Join-Path $sd 'scan_y370.png') (Join-Path $sd '04_settings.png') -Force; Write-Host 'CP_04_SETTINGS_OK' } else { Write-Host 'MISSING scan_y370' }
Write-Host '--- Directory listing ---'
Get-ChildItem $sd -Filter '*.png' | Select-Object Name, Length, LastWriteTime | Format-Table -AutoSize | Out-String | Write-Host
