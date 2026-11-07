# Build the release exe and zip it with the readme: dist/prism-<version>-windows-x64.zip
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    cargo build --release -p prism-windows
    $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
    $stage = Join-Path $root "dist\stage"
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item target\release\prism.exe $stage
    Copy-Item README.md $stage
    $zip = Join-Path $root "dist\prism-$version-windows-x64.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Compress-Archive -Path "$stage\*" -DestinationPath $zip
    Remove-Item $stage -Recurse -Force
    (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower() + "  " + (Split-Path $zip -Leaf) | Set-Content "$zip.sha256"
    Write-Host "wrote $zip"
} finally { Pop-Location }
