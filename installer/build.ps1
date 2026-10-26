# Builds the release exe and the installer locally.
# Needs Inno Setup 6:  winget install JRSoftware.InnoSetup
# Output: installer\Output\SpeedMeter-Setup-<version>.exe

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Building SpeedMeter $version"

cargo test --release
if ($LASTEXITCODE -ne 0) { throw "Tests failed" }
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "Build failed" }

$iscc = @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup" }

& $iscc "/DAppVersion=$version" "installer\speedmeter.iss"
if ($LASTEXITCODE -ne 0) { throw "Installer build failed" }
Write-Host "Done: installer\Output\SpeedMeter-Setup-$version.exe"
