# cleanup-env.ps1 - Memory UAT 环境清理脚本 (PowerShell)
#
# 使用方法：
#   .\Skills\memory-uat\scripts\cleanup-env.ps1

$ErrorActionPreference = "Stop"

$ProjectRoot = Resolve-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent)
$WorkDir = Join-Path $ProjectRoot "test-tools\memory-uat-workdir"

Write-Host "=========================================" -ForegroundColor Cyan
Write-Host " Memory UAT 环境清理 (PowerShell)" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan

# === 1. 停止 TestAIServer ===

$pidFile = Join-Path $WorkDir "testaiserver.pid"
if (Test-Path $pidFile) {
    $pid = Get-Content $pidFile
    $process = Get-Process -Id $pid -ErrorAction SilentlyContinue
    if ($process) {
        Stop-Process -Id $pid -Force
        Write-Host "TestAIServer (PID: $pid) stopped"
    } else {
        Write-Host "TestAIServer (PID: $pid) already stopped"
    }
    Remove-Item $pidFile -Force
}

# === 2. 杀死占用端口的进程 ===

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

# === 3. 删除工作目录 ===

if (Test-Path $WorkDir) {
    Remove-Item -Recurse -Force $WorkDir
    Write-Host "Work directory removed: $WorkDir"
} else {
    Write-Host "Work directory not found (already cleaned)"
}

Write-Host ""
Write-Host "=========================================" -ForegroundColor Cyan
Write-Host " 环境清理完成" -ForegroundColor Cyan
Write-Host "=========================================" -ForegroundColor Cyan
