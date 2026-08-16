# Super-Instruct Release Build Script (PowerShell)
# 用法: .\build-release.ps1
# 产物: src-tauri\target\release\bundle\nsis\*.exe

Write-Host "=== Super-Instruct Release Build ===" -ForegroundColor Cyan

# 1. 安装前端依赖
Write-Host "[1/3] Installing frontend dependencies..." -ForegroundColor Yellow
npm install
if ($LASTEXITCODE -ne 0) { Write-Host "npm install failed" -ForegroundColor Red; exit 1 }

# 2. 构建 Tauri release bundle
Write-Host "[2/3] Building Tauri release bundle..." -ForegroundColor Yellow
npx tauri build --bundles nsis
if ($LASTEXITCODE -ne 0) { Write-Host "tauri build failed" -ForegroundColor Red; exit 1 }

# 3. 列出产物
Write-Host "[3/3] Build artifacts:" -ForegroundColor Yellow
$bundleDir = "src-tauri\target\release\bundle"
if (Test-Path $bundleDir) {
    Get-ChildItem -Path $bundleDir -Recurse -File | ForEach-Object {
        Write-Host "  $($_.FullName)" -ForegroundColor Green
    }
} else {
    Write-Host "  Bundle directory not found: $bundleDir" -ForegroundColor Red
}

Write-Host "=== Build complete ===" -ForegroundColor Cyan