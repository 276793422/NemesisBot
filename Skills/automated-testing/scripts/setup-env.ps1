#
# setup-env.ps1 - NemesisBot 测试环境准备脚本 (PowerShell)
#
# 功能：
#   1. 编译 testaiserver.exe → test/autotest/
#   2. 编译 nemesisbot.exe → test/autotest/
#   3. 编译 websocket_chat_client.exe → test/autotest/
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

# 检查 Go
try {
    $null = go version
} catch {
    Write-Host "ERROR: Go not installed"
    Write-Host "SETUP_FAILURE"
    exit 1
}

# 检查必要目录
if (-not (Test-Path "test\TestAIServer")) {
    Write-Host "ERROR: test\TestAIServer not found"
    Write-Host "SETUP_FAILURE"
    exit 1
}

if (-not (Test-Path "test\websocket_chat_client.go")) {
    Write-Host "ERROR: test\websocket_chat_client.go not found"
    Write-Host "SETUP_FAILURE"
    exit 1
}

# === 2. 创建测试目录 ===

Write-Host "Creating test\autotest directory..."
New-Item -ItemType Directory -Force -Path "test\autotest" | Out-Null

# === 3. 编译组件 ===

Write-Host "Compiling test components..."

# 编译 TestAIServer
Write-Host "[1/3] Compiling TestAIServer..."
Set-Location "test\TestAIServer"
go build -o ..\autotest\testaiserver.exe .
if ($LASTEXITCODE -ne 0) {
    Write-Host "ERROR: Failed to compile TestAIServer"
    Set-Location $ProjectRoot
    Write-Host "SETUP_FAILURE"
    exit 1
}
Set-Location $ProjectRoot

# 编译 NemesisBot
Write-Host "[2/3] Compiling NemesisBot..."
# IMPORTANT: 必须使用 production build tag 才能编译 Wails UI
go build -tags production -o test\autotest\nemesisbot.exe .\nemesisbot
if ($LASTEXITCODE -ne 0) {
    Write-Host "ERROR: Failed to compile NemesisBot"
    Write-Host "SETUP_FAILURE"
    exit 1
}

# 编译 WebSocket 客户端
Write-Host "[3/3] Compiling WebSocket client..."
go build -o test\autotest\websocket_chat_client.exe test\websocket_chat_client.go
if ($LASTEXITCODE -ne 0) {
    Write-Host "ERROR: Failed to compile WebSocket client"
    Write-Host "SETUP_FAILURE"
    exit 1
}

# 验证编译产物
if (-not (Test-Path "test\autotest\testaiserver.exe") -or
    -not (Test-Path "test\autotest\nemesisbot.exe") -or
    -not (Test-Path "test\autotest\websocket_chat_client.exe")) {
    Write-Host "ERROR: Compilation artifacts missing"
    Write-Host "SETUP_FAILURE"
    exit 1
}

Write-Host "Compilation successful"

# === 4. 启动 TestAIServer ===

Write-Host "Starting TestAIServer..."

Set-Location "test\autotest"

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
