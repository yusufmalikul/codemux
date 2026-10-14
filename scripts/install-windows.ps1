# Build Codemux and install it for the current user,
# so it opens from the Start menu (press Win, type "Codemux").
# Run from PowerShell: .\scripts\install-windows.ps1
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

cargo build --release -p codemux
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$dir = Join-Path $env:LOCALAPPDATA "Programs\Codemux"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$exe = Join-Path $dir "Codemux.exe"
Copy-Item -Force "target\release\Codemux.exe" $exe

$lnk = Join-Path ([Environment]::GetFolderPath("Programs")) "Codemux.lnk"
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
$shortcut.TargetPath = $exe
$shortcut.WorkingDirectory = $env:USERPROFILE
$shortcut.Description = "Session manager for Claude Code"
$shortcut.Save()

Write-Host "Installed $exe and a Start menu shortcut"
