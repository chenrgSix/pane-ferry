$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows.' }
Set-Location (Split-Path $PSScriptRoot -Parent)
Get-Command cargo -ErrorAction Stop | Out-Null

function Run-Cargo {
    & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "Cargo failed: $args" }
}

Run-Cargo fmt --all -- --check
Run-Cargo test --locked -p paneferry-audio -p paneferry-platform -p paneferry-backend-windows -p paneferry-windows
Run-Cargo test --locked -p paneferry-core node::tests
# Existing Unix socket integration tests are covered on macOS/Linux. The Windows
# publisher integration test above exercises real Windows pipes and paired QUIC.
Run-Cargo test --locked -p paneferry-network --lib
Run-Cargo check --locked -p paneferry-transport
Run-Cargo build --locked --release -p paneferry-windows -p paneferry-network
Write-Host 'PASS: Windows build and synthetic publisher/QUIC tests. Real desktop capture and input still require manual validation.'
