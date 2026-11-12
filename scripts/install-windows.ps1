# Install Prism for the current user: no admin rights needed.
#
#   pwsh scripts/install-windows.ps1              # build (if needed), install, add to Start menu, start it
#   pwsh scripts/install-windows.ps1 -Uninstall   # remove it again (your settings stay unless -Purge)
#
# Installs to %LOCALAPPDATA%\Programs\Prism and adds a Start menu shortcut.
param([switch]$Uninstall, [switch]$Purge, [switch]$NoStart)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$name = 'Prism'
$dir = Join-Path $env:LOCALAPPDATA "Programs\$name"
$exe = Join-Path $dir 'prism.exe'
$lnk = Join-Path ([Environment]::GetFolderPath('Programs')) "$name.lnk"

function Stop-Running {
    if (Test-Path $exe) {
        & $exe --quit 2>$null
        Start-Sleep -Milliseconds 500
    }
    Get-Process prism -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe } | Stop-Process -Force
}

if ($Uninstall) {
    Stop-Running
    # Give Print Screen and the Win+S keys (Win+Shift+S among them) back if this app had taken them.
    $settings = Join-Path $env:APPDATA "$name\settings.json"
    if ((Test-Path $settings) -and ((Get-Content $settings -Raw | ConvertFrom-Json).replace_snipping)) {
        Set-ItemProperty 'HKCU:\Control Panel\Keyboard' PrintScreenKeyForSnippingEnabled 1 -Type DWord
        $advanced = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced'
        $letters = (Get-ItemProperty $advanced -Name DisabledHotkeys -ErrorAction SilentlyContinue).DisabledHotkeys
        if ($null -ne $letters) {
            $letters = $letters -replace '[Ss]', ''
            if ($letters) { Set-ItemProperty $advanced DisabledHotkeys $letters -Type String }
            else { Remove-ItemProperty $advanced DisabledHotkeys }
        }
    }
    Remove-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' $name -ErrorAction SilentlyContinue
    Remove-Item $lnk, $dir -Recurse -Force -ErrorAction SilentlyContinue
    if ($Purge) { Remove-Item (Join-Path $env:APPDATA $name) -Recurse -Force -ErrorAction SilentlyContinue }
    Write-Host "Removed $name."
    return
}

$built = Join-Path $root 'target\release\prism.exe'
if (-not (Test-Path $built)) {
    cargo build --release -p prism-windows --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw 'build failed' }
}

Stop-Running
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item $built $exe -Force

$shell = New-Object -ComObject WScript.Shell
$s = $shell.CreateShortcut($lnk)
$s.TargetPath = $exe
$s.WorkingDirectory = $dir
$s.Description = 'Screenshot and annotate'
$s.Save()

Write-Host "Installed to $dir"
Write-Host "Start menu: $name"
# First start: the tray icon plus a short welcome card. Later starts open a capture.
if (-not $NoStart) { Start-Process $exe }
