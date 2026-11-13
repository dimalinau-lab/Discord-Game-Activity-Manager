# ==============================================================================
# Скрипт сборки нативного Windows MSI-установщика (Discord Game Activity Manager)
# ==============================================================================

$ErrorActionPreference = "Stop"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $scriptDir

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "  Сборка MSI-установщика Discord Game Activity Manager" -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

# 1. Проверяем наличие WiX Toolset
$wixDir = Join-Path $scriptDir "tools\wix"
$candleExe = Join-Path $wixDir "candle.exe"
$lightExe = Join-Path $wixDir "light.exe"

if (-not (Test-Path $candleExe) -or -not (Test-Path $lightExe)) {
    Write-Host "[1/3] Загрузка WiX Toolset..." -ForegroundColor Yellow
    $zipPath = Join-Path $scriptDir "tools\wix.zip"
    New-Item -ItemType Directory -Force -Path $wixDir | Out-Null
    Invoke-WebRequest -Uri "https://github.com/wixtoolset/wix3/releases/download/wix3112rtm/wix311-binaries.zip" -OutFile $zipPath
    Expand-Archive -Path $zipPath -DestinationPath $wixDir -Force
    Remove-Item -Path $zipPath -Force
}

# 2. Сборка релизных бинарников Rust
Write-Host "[2/3] Компиляция релизных бинарников Rust..." -ForegroundColor Green
cargo build --release --manifest-path (Join-Path $scriptDir "Cargo.toml") --bin discord-game-farmer --bin dummy_game
if ($LASTEXITCODE -ne 0) {
    throw "Ошибка компиляции Cargo!"
}

# 3. Сборка MSI установщика через WiX
Write-Host "[3/3] Упаковка в MSI через WiX Toolset..." -ForegroundColor Green
$Version = "0.2.10"
$targetDir = Join-Path $scriptDir "target"
$wixObj = Join-Path $targetDir "main.wixobj"
$msiOut = Join-Path $targetDir "DiscordGameFarmer-v$Version-x64.msi"
$wxsFile = Join-Path $scriptDir "wix\main.wxs"

& $candleExe -nologo -arch x64 -out $wixObj $wxsFile
if ($LASTEXITCODE -ne 0) {
    throw "Ошибка компиляции WiX candle!"
}

& $lightExe -nologo -ext WixUIExtension -cultures:ru-RU -sice:ICE64 -sice:ICE91 -sice:ICE61 -sice:ICE38 -sice:ICE69 -out $msiOut $wixObj
if ($LASTEXITCODE -ne 0) {
    throw "Ошибка линковки WiX light!"
}

# Копируем установщик в корень проекта для удобного доступа
$rootMsi = Join-Path $scriptDir "DiscordGameFarmer-v$Version-x64.msi"
Copy-Item $msiOut $rootMsi -Force

$fileInfo = Get-Item $rootMsi
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "Готово! MSI-установщик успешно собран:" -ForegroundColor Green
Write-Host "Путь: $($fileInfo.FullName)" -ForegroundColor White
Write-Host "Размер: $([math]::Round($fileInfo.Length / 1MB, 2)) МБ" -ForegroundColor White
Write-Host "==========================================================" -ForegroundColor Cyan

