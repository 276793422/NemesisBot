# setup-env.ps1 - Memory UAT 环境准备脚本 (PowerShell)
#
# 使用方法：
#   .\Skills\memory-uat\scripts\setup-env.ps1

$ErrorActionPreference = "Stop"

$ProjectRoot = Resolve-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent)
$WorkDir = Join-Path $ProjectRoot "test-tools\memory-uat-workdir"

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host " Memory UAT 环境准备 (PowerShell)" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan

# === 1. 检查二进制文件 ===

$NemesisbotBin = $null
if (Test-Path "$ProjectRoot\target\release\nemesisbot.exe") {
    $NemesisbotBin = "$ProjectRoot\target\release\nemesisbot.exe"
    Write-Host "[OK] nemesisbot.exe (release)" -ForegroundColor Green
} elseif (Test-Path "$ProjectRoot\target\debug\nemesisbot.exe") {
    $NemesisbotBin = "$ProjectRoot\target\debug\nemesisbot.exe"
    Write-Host "[WARN] nemesisbot.exe (debug)" -ForegroundColor Yellow
} else {
    Write-Host "[MISSING] nemesisbot.exe. Run: cargo build --release -p nemesisbot" -ForegroundColor Red
    exit 1
}

$AiBin = $null
if (Test-Path "$ProjectRoot\test-tools\TestAIServer\testaiserver.exe") {
    $AiBin = "$ProjectRoot\test-tools\TestAIServer\testaiserver.exe"
    Write-Host "[OK] testaiserver.exe" -ForegroundColor Green
} else {
    Write-Host "[BUILD] Building testaiserver.exe..." -ForegroundColor Yellow
    Push-Location "$ProjectRoot\test-tools\TestAIServer"
    go build -o testaiserver.exe
    Pop-Location
    $AiBin = "$ProjectRoot\test-tools\TestAIServer\testaiserver.exe"
    Write-Host "[OK] testaiserver.exe (built)" -ForegroundColor Green
}

# === 2. 检查 plugin DLL ===

$PluginDll = $null
if (Test-Path "$ProjectRoot\target\release\plugins\plugin_onnx.dll") {
    $PluginDll = "$ProjectRoot\target\release\plugins\plugin_onnx.dll"
    Write-Host "[OK] plugin_onnx.dll" -ForegroundColor Green
} else {
    Write-Host "[SKIP] plugin_onnx.dll not found" -ForegroundColor Yellow
}

# === 3. 创建工作目录 ===

Write-Host ""
Write-Host "Creating work directory: $WorkDir"
if (Test-Path $WorkDir) { Remove-Item -Recurse -Force $WorkDir }
New-Item -ItemType Directory -Path "$WorkDir\plugins" -Force | Out-Null

Copy-Item $NemesisbotBin $WorkDir
Copy-Item $AiBin $WorkDir
if ($PluginDll) { Copy-Item $PluginDll "$WorkDir\plugins\" }

Write-Host "[OK] Work directory created" -ForegroundColor Green

# === 4. 杀死占用端口的进程 ===

Write-Host ""
Write-Host "Cleaning up ports..."
foreach ($port in @(8080, 49000, 18790)) {
    $connections = netstat -ano | Select-String ":$port " | Select-String "LISTENING"
    foreach ($conn in $connections) {
        $pid = ($conn -split '\s+')[-1]
        if ($pid -match '^\d+$') {
            Stop-Process -Id ([int]$pid) -Force -ErrorAction SilentlyContinue
            Write-Host "  Killed PID $pid (port $port)"
        }
    }
}

# === 5. 启动 TestAIServer ===

Write-Host ""
Write-Host "Starting TestAIServer..."
$aiProcess = Start-Process -FilePath "$WorkDir\testaiserver.exe" -WorkingDirectory $WorkDir -WindowStyle Hidden -PassThru
Start-Sleep -Seconds 2

# 验证 AI server
try {
    $response = Invoke-WebRequest -Uri "http://127.0.0.1:8080/v1/models" -TimeoutSec 5 -ErrorAction Stop
    Write-Host "[OK] TestAIServer started (PID: $($aiProcess.Id), port: 8080)" -ForegroundColor Green
} catch {
    Write-Host "[FAIL] TestAIServer failed to start" -ForegroundColor Red
    exit 1
}

# 保存 PID
$aiProcess.Id | Out-File "$WorkDir\testaiserver.pid"

Write-Host ""
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host " 环境准备完成" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host " 工作目录: $WorkDir"
Write-Host " AI Server PID: $($aiProcess.Id)"
Write-Host " Plugin DLL: $(if ($PluginDll) { 'available' } else { 'NOT AVAILABLE' })"
Write-Host ""
Write-Host " 下一步：按照 SKILL.md Stage 3-6 执行 UAT 测试"
Write-Host "=========================================" -ForegroundColor Cyan
