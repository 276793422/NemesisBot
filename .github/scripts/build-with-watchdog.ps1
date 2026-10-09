# CI 编译看门狗（2026-10-08；2026-10-09 R8 去 WMI + 空转熔断）：Windows hosted
# runner 在 nemesisbot 冷构建 bin 步反复楔死（R6/R7 四轮实录：libs 步健康速度
# 跑完 → bin 步 40-44min 零进展 → MaxMinutes 上限从未触发 → runner ~50min 被
# 回收）。两个根因级教训固化进本脚本：
#   1. Get-CimInstance（WMI）在编译/链接高载下会爬行甚至挂死——资源监视循环
#      卡死 = MaxMinutes 永不触发 = 软着陆永不发生。R8 起内存读数改 P/Invoke
#      GlobalMemoryStatusEx（内核 syscall，微秒级，与负载无关），磁盘走
#      Get-PSDrive（.NET PSDrive 提供器，非 WMI）。
#   2. 楔死唯一可观测症状 = 重定向输出零增长（bin 步单 crate codegen+link
#      期间 stdout/stderr 两文件字节数纹丝不动）。空转熔断：N 分钟无增长即
#      树杀软着陆（exit 3）——软着陆后 Post 步骤照常执行，rust-cache 照常
#      保存，下一轮热缓存快路径（ET 实证 bin 2.8min），打破「冷→楔死→runner
#      死→无缓存→再冷」死循环。
# 用法（workflow step 内）：
#   shell: powershell
#   run: powershell -NoProfile -ExecutionPolicy Bypass -File ".github\scripts\build-with-watchdog.ps1" -Tag build -BuildCommand "cargo build ..."
param(
    [Parameter(Mandatory = $true)][string]$BuildCommand,
    [string]$Tag = "build",
    [int]$PollSecs = 15,
    [int]$MaxMinutes = 90,
    # 空转熔断阈值：stdout+stderr 合计字节数连续 N 分钟不增长即判楔死。
    # 健康冷链最后一段（单 crate link）静默 <5min，10min 给足裕量。
    [int]$IdleMinutes = 10,
    # 地板默认值：commit 余量 2GB / 物理 RAM 余量 1.5GB / C 盘余量 2GB。
    # 跌破任一 = 再编译下去就是 paging 风暴 → runner 心跳饿死失联，提前软着陆。
    [long]$FreeCommitFloorMb = 2048,
    [long]$FreeRamFloorMb = 1536,
    [long]$FreeDiskFloorMb = 2048,
    [int]$TailOut = 25,
    [int]$TailErr = 40
)
$ErrorActionPreference = 'Stop'
$sw = [System.Diagnostics.Stopwatch]::StartNew()
# RUNNER_TEMP 仅存在于 runner；本地冒烟回落系统 TEMP
$base = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
$out = Join-Path $base "$Tag-build.out.log"
$err = Join-Path $base "$Tag-build.err.log"
# cmd /c 承载任意命令行（规避 PowerShell 参数再解析歧义）；-WindowStyle Hidden
# 是本项目纪律（严禁任何会创建可见窗口的启动方式）。
$p = Start-Process -FilePath $env:ComSpec -ArgumentList '/c', $BuildCommand `
    -RedirectStandardOutput $out -RedirectStandardError $err -PassThru -WindowStyle Hidden
# 不先触碰 .Handle 的话 .ExitCode 退出后仍是 null（PS 经典坑，exit 0 假绿）
$null = $p.Handle
"[$Tag] started pid=$($p.Id) cmd=$BuildCommand"
# 中止/退出后都要打印输出尾部——否则被杀时无法知道命令当时进行到哪
function Show-Tails {
    "[$Tag] ---- stderr tail ----"
    Get-Content $err -Tail $TailErr | ForEach-Object { "[$Tag][err] $_" }
    "[$Tag] ---- stdout tail ----"
    Get-Content $out -Tail $TailOut | ForEach-Object { "[$Tag] $_" }
}
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class NbMemStatus {
    [StructLayout(LayoutKind.Sequential)]
    public class MEMORYSTATUSEX {
        public uint dwLength;
        public uint dwMemoryLoad;
        public ulong ullTotalPhys;
        public ulong ullAvailPhys;
        public ulong ullTotalPageFile;
        public ulong ullAvailPageFile;
        public ulong ullTotalVirtual;
        public ulong ullAvailVirtual;
        public ulong ullAvailExtendedVirtual;
    }
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool GlobalMemoryStatusEx([In, Out] MEMORYSTATUSEX buf);
}
"@
function Get-MemSnapshot {
    $ms = New-Object NbMemStatus+MEMORYSTATUSEX
    $ms.dwLength = [System.Runtime.InteropServices.Marshal]::SizeOf($ms)
    [void][NbMemStatus]::GlobalMemoryStatusEx($ms)
    [pscustomobject]@{
        FreeRamMb     = [math]::Round($ms.ullAvailPhys / 1MB)
        FreeCommitMb  = [math]::Round($ms.ullAvailPageFile / 1MB)
        CommitLimitMb = [math]::Round($ms.ullTotalPageFile / 1MB)
    }
}
$lastBytes = -1L
$lastProgressMin = 0.0
while (-not $p.HasExited) {
    Start-Sleep -Seconds $PollSecs
    $mem = Get-MemSnapshot
    $disk = Get-PSDrive C
    $freeDiskMb = [math]::Round($disk.Free / 1MB)
    $elMin = $sw.Elapsed.TotalMinutes
    $bytes = (Get-Item $out).Length + (Get-Item $err).Length
    if ($bytes -ne $lastBytes) { $lastBytes = $bytes; $lastProgressMin = $elMin }
    $idleMin = [math]::Round($elMin - $lastProgressMin, 1)
    $el = [int]$elMin
    "[$Tag] t=${el}min idle=${idleMin}min free_ram=$($mem.FreeRamMb)MB free_commit=$($mem.FreeCommitMb)/$($mem.CommitLimitMb)MB free_disk=${freeDiskMb}MB"
    if ($el -ge $MaxMinutes) {
        taskkill /T /F /PID $p.Id | Out-Null
        "[$Tag] ABORT max-time ${MaxMinutes}min exceeded"
        Show-Tails
        exit 1
    }
    if ($idleMin -ge $IdleMinutes) {
        taskkill /T /F /PID $p.Id | Out-Null
        "[$Tag] ABORT no-output idle ${IdleMinutes}min (last progress t=$([math]::Round($lastProgressMin,1))min)"
        Show-Tails
        exit 3
    }
    if ($mem.FreeCommitMb -lt $FreeCommitFloorMb -or $mem.FreeRamMb -lt $FreeRamFloorMb -or $freeDiskMb -lt $FreeDiskFloorMb) {
        taskkill /T /F /PID $p.Id | Out-Null
        "[$Tag] ABORT resource floor breach: free_commit=$($mem.FreeCommitMb)MB free_ram=$($mem.FreeRamMb)MB free_disk=${freeDiskMb}MB (floors ${FreeCommitFloorMb}/${FreeRamFloorMb}/${FreeDiskFloorMb}MB)"
        Show-Tails
        exit 2
    }
}
"[$Tag] exited code=$($p.ExitCode) after $([int]$sw.Elapsed.TotalMinutes)min"
Show-Tails
exit $p.ExitCode
