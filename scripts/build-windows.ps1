param([switch]$SkipChecks, [switch]$NoBundle)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows.' }
$repo = Split-Path $PSScriptRoot -Parent
Push-Location $repo
try {
    if (-not $SkipChecks) { & "$PSScriptRoot/check-windows.ps1" }
    & cargo build --locked --release -p paneferry-windows
    if ($LASTEXITCODE -ne 0) { throw 'Windows publisher build failed.' }
    $resources = Join-Path $repo 'apps/launcher/src-tauri/resources'
    New-Item -ItemType Directory -Force $resources | Out-Null
    Copy-Item -LiteralPath (Join-Path $repo 'target/release/paneferry-windows.exe') -Destination $resources
    Push-Location (Join-Path $repo 'apps/launcher')
    try {
        & npm.cmd ci --no-audit --no-fund
        if ($LASTEXITCODE -ne 0) { throw 'Frontend dependency installation failed.' }
        & npm.cmd test
        if ($LASTEXITCODE -ne 0) { throw 'Frontend tests failed.' }
        if ($NoBundle) { & npm.cmd run tauri -- build --no-bundle }
        else { & npm.cmd run tauri -- build }
        if ($LASTEXITCODE -ne 0) { throw 'Windows launcher build failed.' }
    } finally { Pop-Location }
} finally { Pop-Location }
