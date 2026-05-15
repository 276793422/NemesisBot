#
# setup-env.ps1 - NemesisBot Rust 项目测试环境准备脚本 (PowerShell)
#
# 功能：
#   1. 编译 testaiserver.exe → test-tools/autotest/（Go 项目）
#   2. 编译 nemesisbot.exe → test-tools/autotest/（Rust 项目）
#   3. 编译 websocket_chat_client.exe → test-tools/autotest/（Rust 项目）
#   4. 启动 testaiserver（后台）
#   5. 保存 testaiserver PID
#
# 使用方法：
#   .\Skills\automated-testing\scripts\setup-env.ps1
#
# 输出格式：
#   SETUP_START
#   SETUP_SUCCESS/SETUP_FAILURE
#   TESTAI_PID=<pid>
#   TESTAI_PORT=8080
#

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

Write-Host "SETUP_START"

# 获取项目根目录
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $ScriptDir))

Set-Location $ProjectRoot

# === 1. 检查环境 ===

# 检查 Rust/Cargo
try {
    $null = cargo --version
} catch {
    Write-Host "ERROR: Cargo not installed (Rust toolchain required)"
    Write-Host "SETUP_FAILURE"
    exit 1
}

# 检查 Go（TestAIServer 仍为 Go 项目）
try {
    $null = go version
} catch {
    Write-Host "ERROR: Go not installed (required for TestAIServer)"
    Write-Host "SETUP_FAILURE"
    exit 1
}

# 检查必要目录
if (-not (Test-Path "test-tools\TestAIServer")) {
    Write-Host "ERROR: test-tools\TestAIServer not found"
    Write-Host "SETUP_FAILURE"
    exit 1
}

if (-not (Test-Path "test-tools\websocket-client")) {
    Write-Host "ERROR: test-tools\websocket-client not found"
    Write-Host "SETUP_FAILURE"
    exit 1
}

# === 2. 创建测试目录 ===

Write-Host "Creating test-tools\autotest directory..."
New-Item -ItemType Directory -Force -Path "test-tools\autotest" | Out-Null

# === 3. 编译组件 ===

Write-Host "Compiling test components..."

# 编译 TestAIServer（Go 项目）
Write-Host "[1/3] Compiling TestAIServer..."
Set-Location "test-tools\TestAIServer"
go build -o ..\autotest\testaiserver.exe .
if ($LASTEXITCODE -ne 0) {
    Write-Host "ERROR: Failed to compile TestAIServer"
    Set-Location $ProjectRoot
    Write-Host "SETUP_FAILURE"
    exit 1
}
Set-Location $ProjectRoot

# 编译 NemesisBot（Rust 项目）
Write-Host "[2/3] Compiling NemesisBot..."
cargo build --release -p nemesisbot
if ($LASTEXITCODE -ne 0) {
    Write-Host "ERROR: Failed to compile NemesisBot"
    Write-Host "SETUP_FAILURE"
    exit 1
}
Copy-Item "target\release\nemesisbot.exe" "test-tools\autotest\"

# 编译 WebSocket 客户端（Rust 项目）
Write-Host "[3/3] Compiling WebSocket client..."
Set-Location "test-tools\websocket-client"
cargo build --release
if ($LASTEXITCODE -ne 0) {
    Write-Host "ERROR: Failed to compile WebSocket client"
    Set-Location $ProjectRoot
    Write-Host "SETUP_FAILURE"
    exit 1
}
# 复制编译产物
$wsBins = @("websocket_chat_client", "websocket-client", "ws_client")
foreach ($binName in $wsBins) {
    $binPath = "target\release\${binName}.exe"
    if (Test-Path $binPath) {
        Copy-Item $binPath "..\autotest\websocket_chat_client.exe"
        break
    }
}
Set-Location $ProjectRoot

# 验证编译产物
if (-not (Test-Path "test-tools\autotest\testaiserver.exe") -or
    -not (Test-Path "test-tools\autotest\nemesisbot.exe")) {
    Write-Host "ERROR: Compilation artifacts missing"
    Write-Host "SETUP_FAILURE"
    exit 1
}

Write-Host "Compilation successful"

# === 4. 启动 TestAIServer ===

Write-Host "Starting TestAIServer..."

Set-Location "test-tools\autotest"

# 停止可能存在的旧进程
Stop-Process -Name "testaiserver" -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

# 启动 TestAIServer
$testaiProcess = Start-Process -FilePath ".\testaiserver.exe" -PassThru
$TESTAI_PID = $testaiProcess.Id

# 保存 PID
$TESTAI_PID | Out-File -FilePath "testaiserver.pid"

# 等待 TestAIServer 就绪
Write-Host "Waiting for TestAIServer to be ready..."
$ready = $false
for ($i = 1; $i -le 30; $i++) {
    try {
        $response = Invoke-WebRequest -Uri "http://127.0.0.1:8080/v1/models" -UseBasicParsing -TimeoutSec 1
        if ($response.StatusCode -eq 200) {
            $ready = $true
            break
        }
    } catch {
        # 继续等待
    }
    Start-Sleep -Seconds 1
}

if (-not $ready) {
    Write-Host "ERROR: TestAIServer failed to start"
    Write-Host "SETUP_FAILURE"
    exit 1
}

# === 5. 输出结果 ===

Write-Host "SETUP_SUCCESS"
Write-Host "TESTAI_PID=$TESTAI_PID"
Write-Host "TESTAI_PORT=8080"
Write-Host "WORK_DIR=$PWD"

Write-Host ""
Write-Host "Environment setup completed successfully!"
Write-Host "TestAIServer is running with PID: $TESTAI_PID"
Write-Host "TestAIServer endpoint: http://127.0.0.1:8080/v1"

exit 0
