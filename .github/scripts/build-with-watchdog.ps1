# CI 编译看门狗（2026-10-08）：Windows hosted runner 在 nemesisbot 编译期反复
# "hosted runner lost communication"（六连死，51-55min 恒定挂点），runner 级死亡
# 无日志可查，step 状态是唯一现场。本脚本把编译包进资源监视循环：
#   - 每 PollSecs 流式输出一行 free RAM / commit 余量 / 磁盘余量到步骤日志
#     （runner 若仍死亡，死前已流出的曲线就是事后定位的证据）；
#   - 任一资源跌破地板 → taskkill 树杀编译 → exit 2（软着陆，runner 不死，
#     完整日志可上传，下一步直接看数字定位元凶）；
#   - 超过 MaxMinutes → 树杀 → exit 1（防呆）；
#   - 编译正常退出 → 透传退出码 + 打尾部日志。
# 用法（workflow step 内）：
#   shell: powershell
#   run: & .github/scripts/build-with-watchdog.ps1 -Tag etbuild -BuildCommand "cargo build -p nemesisbot"
param(
    [Parameter(Mandatory = $true)][string]$BuildCommand,
    [string]$Tag = "build",
    [int]$PollSecs = 15,
    [int]$MaxMinutes = 90,
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
while (-not $p.HasExited) {
    Start-Sleep -Seconds $PollSecs
    $os = Get-CimInstance Win32_OperatingSystem
    $disk = Get-PSDrive C
    $freeRamMb = [math]::Round($os.FreePhysicalMemory / 1024)
    $freeCommitMb = [math]::Round($os.FreeVirtualMemory / 1024)
    $commitLimitMb = [math]::Round($os.TotalVirtualMemorySize / 1024)
    $freeDiskMb = [math]::Round($disk.Free / 1MB)
    $el = [int]$sw.Elapsed.TotalMinutes
    "[$Tag] t=${el}min free_ram=${freeRamMb}MB free_commit=${freeCommitMb}/${commitLimitMb}MB free_disk=${freeDiskMb}MB"
    if ($el -ge $MaxMinutes) {
        taskkill /T /F /PID $p.Id | Out-Null
        "[$Tag] ABORT max-time ${MaxMinutes}min exceeded"
        Show-Tails
        exit 1
    }
    if ($freeCommitMb -lt $FreeCommitFloorMb -or $freeRamMb -lt $FreeRamFloorMb -or $freeDiskMb -lt $FreeDiskFloorMb) {
        taskkill /T /F /PID $p.Id | Out-Null
        "[$Tag] ABORT resource floor breach: free_commit=${freeCommitMb}MB free_ram=${freeRamMb}MB free_disk=${freeDiskMb}MB (floors ${FreeCommitFloorMb}/${FreeRamFloorMb}/${FreeDiskFloorMb}MB)"
        Show-Tails
        exit 2
    }
}
"[$Tag] exited code=$($p.ExitCode) after $([int]$sw.Elapsed.TotalMinutes)min"
Show-Tails
exit $p.ExitCode
