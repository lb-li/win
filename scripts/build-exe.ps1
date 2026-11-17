$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    Write-Host "[1/3] building release binary ..."
    cargo build --release -p fsearch-ui
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

    Write-Host "[2/3] staging dist ..."
    $dist = Join-Path $root "dist"
    New-Item -ItemType Directory -Force -Path $dist | Out-Null

    $exe = Join-Path $root "target\release\fsearch.exe"
    if (-not (Test-Path $exe)) { throw "missing $exe" }

    $target = Join-Path $dist "fsearch.exe"
    Copy-Item $exe $target -Force

    Write-Host "[3/3] done"
    Write-Host "OK: $target"
    Write-Host ("size: {0:N1} MB" -f ((Get-Item $target).Length / 1MB))
} finally {
    Pop-Location
}