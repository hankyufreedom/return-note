param([Parameter(Mandatory=$true)][string]$Exe)
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
$destination=Join-Path $env:LOCALAPPDATA 'ReturnNote'
New-Item -ItemType Directory -Force $destination|Out-Null
$target=Join-Path $destination 'return-note.exe'
if(Get-Process -Name return-note -ErrorAction SilentlyContinue | Where-Object {$_.Path -eq $target}) {throw '回來正在執行，請從設定按「結束程式」後再更新。'}
Copy-Item -LiteralPath $Exe -Destination $target -Force
if((Get-FileHash -Algorithm SHA256 $Exe).Hash -ne (Get-FileHash -Algorithm SHA256 $target).Hash){throw 'Executable copy checksum mismatch'}
$desktop=[Environment]::GetFolderPath('DesktopDirectory')
$shell=New-Object -ComObject WScript.Shell
$shortcut=$shell.CreateShortcut((Join-Path $desktop '回來.lnk'))
$shortcut.TargetPath=$target
$shortcut.WorkingDirectory=$destination
$shortcut.IconLocation="$target,0"
$shortcut.Description='隨手記下待辦，回來時接著做。Ctrl+Alt+N 叫出。'
$shortcut.Save()
Write-Output "INSTALLED=$target"
Write-Output "SHORTCUT=$(Join-Path $desktop '回來.lnk')"
Write-Output "SHA256=$((Get-FileHash -Algorithm SHA256 $target).Hash)"
exit 0
