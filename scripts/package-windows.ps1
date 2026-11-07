# Build a release: the setup wizard and a zip of the exe, with checksums, in dist/.
#
#   pwsh scripts/package-windows.ps1
#
#   dist/Prism-Setup-<version>.exe        the installer most people want
#   dist/prism-<version>-windows-x64.zip  the exe on its own, to run without installing
#   dist/SHA256SUMS.txt
#
# The installer needs Inno Setup 6: winget install JRSoftware.InnoSetup
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    cargo build --release -p prism-windows
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
    $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
    $dist = Join-Path $root "dist"
    New-Item -ItemType Directory -Force $dist | Out-Null

    # The zip: the exe and the readme.
    $stage = Join-Path $dist "stage"
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item target\release\prism.exe, README.md $stage
    $zip = Join-Path $dist "prism-$version-windows-x64.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Compress-Archive -Path "$stage\*" -DestinationPath $zip
    Remove-Item $stage -Recurse -Force

    # The installer.
    $iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $iscc) { throw "Inno Setup 6 isn't installed: winget install JRSoftware.InnoSetup" }
    & $iscc /Q "/DAppVersion=$version" "/DSourceExe=$root\target\release\prism.exe" installer\prism.iss
    if ($LASTEXITCODE -ne 0) { throw "installer build failed" }
    $setup = Join-Path $dist "Prism-Setup-$version.exe"

    $sums = foreach ($file in $setup, $zip) { (Get-FileHash $file -Algorithm SHA256).Hash.ToLower() + "  " + (Split-Path $file -Leaf) }
    $sums | Set-Content (Join-Path $dist "SHA256SUMS.txt")
    Write-Host "wrote $setup"
    Write-Host "wrote $zip"
} finally { Pop-Location }
