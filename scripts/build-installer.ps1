# Builds the Windows NSIS installer and copies it to ./installer/
$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot

Set-Location $Root
Write-Host "Compilando CreatorFloww y generando instalador NSIS..." -ForegroundColor Cyan
npm run tauri:build
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$setup = Get-ChildItem -Path (Join-Path $Root "src-tauri\target\release\bundle\nsis") -Filter "*-setup.exe" -ErrorAction SilentlyContinue
if (-not $setup) {
    $cargoTarget = $env:CARGO_TARGET_DIR
    if ($cargoTarget) {
        $setup = Get-ChildItem -Path (Join-Path $cargoTarget "release\bundle\nsis") -Filter "*-setup.exe" -ErrorAction SilentlyContinue
    }
}
if (-not $setup) {
    Write-Error "No se encontro el .exe del instalador. Revisa src-tauri\target\release\bundle\nsis\"
    exit 1
}

$outDir = Join-Path $Root "installer"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$dest = Join-Path $outDir $setup.Name
Copy-Item -Path $setup.FullName -Destination $dest -Force
Write-Host ""
Write-Host "Instalador listo:" -ForegroundColor Green
Write-Host "  $dest"
Write-Host "  $([math]::Round((Get-Item $dest).Length / 1MB, 2)) MiB"