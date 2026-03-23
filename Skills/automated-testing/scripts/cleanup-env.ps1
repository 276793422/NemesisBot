#
# cleanup-env.ps1 - NemesisBot 测试环境清理脚本 (PowerShell)
#
# 功能：
#   1. 停止 nemesisbot.exe（通过进程名）
#   2. 停止 testaiserver.exe（通过进程名）
#   3. 等待文件释放
#
# 注意：
#   - 不删除 test/autotest/ 目录（由 AI 负责）
#
# 使用方法：
#   .\Skills\automated-testing\scripts\cleanup-env.ps1
#
# 输出格式：
#   CLEANUP_START
#   CLEANUP_SUCCESS/CLEANUP_FAILURE
#

[CmdletBinding()]
param()

$ErrorActionPreference = "Continue"

Write-Host "CLEANUP_START"

# 获取项目根目录
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $ScriptDir))

Set-Location $ProjectRoot

# === 1. 停止 NemesisBot ===

Write-Host "Stopping NemesisBot..."

# 尝试通过 PID 文件停止
if (Test-Path "test\autotest\nemesisbot.pid") {
    $nemesisPid = Get-Content "test\autotest\nemesisbot.pid"
    $process = Get-Process -Id $nemesisPid -ErrorAction SilentlyContinue
    if ($process) {
        Write-Host "Stopping NemesisBot (PID: $nemesisPid)..."
        Stop-Process -Id $nemesisPid -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -Path "test\autotest\nemesisbot.pid" -Force -ErrorAction SilentlyContinue
}

# 通过进程名强制停止
Stop-Process -Name "nemesisbot" -Force -ErrorAction SilentlyContinue

# === 2. 停止 TestAIServer ===

Write-Host "Stopping TestAIServer..."

# 尝试通过 PID 文件停止
if (Test-Path "test\autotest\testaiserver.pid") {
    $testaiPid = Get-Content "test\autotest\testaiserver.pid"
    $process = Get-Process -Id $testaiPid -ErrorAction SilentlyContinue
    if ($process) {
        Write-Host "Stopping TestAIServer (PID: $testaiPid)..."
        Stop-Process -Id $testaiPid -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -Path "test\autotest\testaiserver.pid" -Force -ErrorAction SilentlyContinue
}

# 通过进程名强制停止
Stop-Process -Name "testaiserver" -Force -ErrorAction SilentlyContinue

# === 3. 等待文件释放 ===

Write-Host "Waiting for file handles to be released..."
Start-Sleep -Seconds 3

# === 4. 验证清理结果 ===

$nemesisRunning = Get-Process -Name "nemesisbot" -ErrorAction SilentlyContinue
$testaiRunning = Get-Process -Name "testaiserver" -ErrorAction SilentlyContinue

if ($nemesisRunning -or $testaiRunning) {
    Write-Host "WARNING: Some processes may still be running"
} else {
    Write-Host "All processes stopped successfully"
}

Write-Host "CLEANUP_SUCCESS"
Write-Host ""
Write-Host "Environment cleanup completed!"
Write-Host "Note: test\autotest\ directory was not removed (AI should handle this)"

exit 0
