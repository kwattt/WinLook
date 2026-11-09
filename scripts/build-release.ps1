$ErrorActionPreference = "Stop"

Write-Host "==> Building WinLook release binary (x86_64-pc-windows-msvc)..."
Stop-Process -Name "winlook" -Force -ErrorAction SilentlyContinue
cargo build --release

$exePath = Join-Path $PSScriptRoot "..\target\release\winlook.exe"
if (-not (Test-Path $exePath)) {
    throw "Release binary not found at $exePath"
}

$sizeMb = [math]::Round((Get-Item $exePath).Length / 1MB, 2)
Write-Host "==> Built target\release\winlook.exe ($sizeMb MB)"

# Locate Inno Setup 6 (ISCC.exe) across PATH, per-user %LOCALAPPDATA%, and Program Files
$isccCandidates = @(
    (Get-Command "iscc.exe" -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source),
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
)
$iscc = $isccCandidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1

if ($iscc) {
    Write-Host "==> Found Inno Setup at: $iscc"
    Write-Host "==> Packaging Windows installer with Inno Setup..."
    & $iscc (Join-Path $PSScriptRoot "..\installer\winlook.iss")
    if ($LASTEXITCODE -ne 0) {
        throw "ISCC.exe failed with exit code $LASTEXITCODE"
    }
    $installerOut = Join-Path $PSScriptRoot "..\target\installer\WinLook-Setup-0.1.0-x64.exe"
    if (Test-Path $installerOut) {
        $instMb = [math]::Round((Get-Item $installerOut).Length / 1MB, 2)
        Write-Host "==> Success! Installer created at: target\installer\WinLook-Setup-0.1.0-x64.exe ($instMb MB)"
    }
} else {
    Write-Host "==> Note: Install Inno Setup 6 (winget install JRSoftware.InnoSetup) to build WinLook-Setup.exe from installer\winlook.iss"
}
