param([string]$Mode = 'full')

# Platform environment fingerprint (2026-10-12 device-attribution discipline).
# full mode: complete hardware/OS/network/security/pressure/toolchain identity
# of the assigned hosted runner -> step log + GITHUB_STEP_SUMMARY, so
# slow-machine / freeze / lost-communication incidents can be attributed to
# the assigned device with evidence instead of guesswork. Ten domains:
#   runner identity+image wave / OS build / virtualization / CPU / memory /
#   disk(+write probe) / network(+RTT probe) / limits+pressure / security+power
#   / toolchain baseline.
# brief mode (Windows test battlegrounds only): identity + live pressure
# snapshot right before test execution (free RAM / commit / load / disk).
# All probes failure-guarded; step never reds because a probe is unavailable.
# ASCII-only on purpose: PowerShell 5.1 reads BOM-less files as ANSI.

$ErrorActionPreference = 'SilentlyContinue'
$lines = New-Object System.Collections.Generic.List[string]
function Add-Line { param([string]$s) $script:lines.Add($s) | Out-Null }

if ($Mode -eq 'brief') {
  Add-Line "NowUTC=$([DateTime]::UtcNow.ToString('o')) (pre-test pressure snapshot)"
  Add-Line "RUNNER_NAME=$env:RUNNER_NAME ImageVersion=$env:ImageVersion"
  $os  = Get-CimInstance Win32_OperatingSystem
  $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
  $cd  = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'"
  Add-Line "CPU_loadPct=$($cpu.LoadPercentage) processes=$((Get-Process).Count)"
  Add-Line "FreeRAM_GB=$([math]::Round(($os.FreePhysicalMemory*1KB)/1GB,2))"
  Add-Line "CommitFree_GB=$([math]::Round(($os.FreeVirtualMemory*1KB)/1GB,2))"
  Add-Line "C_free_GB=$([math]::Round($cd.FreeSpace/1GB,1))"
  Write-Output ($lines -join "`n")
  exit 0
}

Add-Line "NowUTC=$([DateTime]::UtcNow.ToString('o')) TZ=$((Get-TimeZone).Id)"
Add-Line "=== RUNNER IDENTITY ==="
Add-Line "RUNNER_NAME=$env:RUNNER_NAME"
Add-Line "RUNNER_ENVIRONMENT=$env:RUNNER_ENVIRONMENT RUNNER_OS=$env:RUNNER_OS RUNNER_ARCH=$env:RUNNER_ARCH"
Add-Line "ImageOS=$env:ImageOS ImageVersion=$env:ImageVersion"

$cs   = Get-CimInstance Win32_ComputerSystem
$os   = Get-CimInstance Win32_OperatingSystem
$cpus = @(Get-CimInstance Win32_Processor)

Add-Line "=== OS ==="
$cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
Add-Line "OS=$($os.Caption) Arch=$($os.OSArchitecture)"
Add-Line "Version=$($os.Version) Build=$($cv.CurrentBuildNumber).$($cv.UBR) DisplayVersion=$($cv.DisplayVersion)"
Add-Line "System=$($cs.Manufacturer) $($cs.Model)"
Add-Line "InstallDate=$($os.InstallDate) LastBoot=$($os.LastBootUpTime)"
Add-Line "CodePage=$((chcp.com)) Locale=$($os.Locale)"

Add-Line "=== CPU ==="
Add-Line "Sockets=$($cs.NumberOfProcessors)"
$i = 0
foreach ($c in $cpus) {
  $i++
  Add-Line "CPU[$i] model=$($c.Name)"
  Add-Line "CPU[$i] caption=$($c.Caption)"
  Add-Line "CPU[$i] cores=$($c.NumberOfCores) logical=$($c.NumberOfLogicalProcessors)"
  Add-Line "CPU[$i] baseClockMHz=$($c.MaxClockSpeed) currentClockMHz=$($c.CurrentClockSpeed) loadPct=$($c.LoadPercentage)"
  Add-Line "CPU[$i] L2KB=$($c.L2CacheSize) L3KB=$($c.L3CacheSize)"
}
# Interpreted int loop as a contention probe: same image+PS version on every
# runner => cross-VM comparable. Slow number with matching cpu model = host
# steal / noisy neighbor (the slow-machine family signature).
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$x = 0
for ($k = 0; $k -lt 2000000; $k++) { $x += $k % 7 }
$sw.Stop()
Add-Line "CpuProbe2M=$([math]::Round($sw.Elapsed.TotalMilliseconds,0))ms (PS int loop; slower=host contention)"
Add-Line "CpuProbeX=$x (checksum, prevents loop elision)"

Add-Line "=== MEMORY ==="
Add-Line "TotalRAM_GB=$([math]::Round($cs.TotalPhysicalMemory/1GB,2))"
Add-Line "FreeRAM_GB=$([math]::Round(($os.FreePhysicalMemory*1KB)/1GB,2))"
Add-Line "CommitLimit_GB=$([math]::Round(($os.TotalVirtualMemorySize*1KB)/1GB,2)) CommitFree_GB=$([math]::Round(($os.FreeVirtualMemory*1KB)/1GB,2))"
$dims = @(Get-CimInstance Win32_PhysicalMemory)
Add-Line "DIMMs=$($dims.Count)"
$j = 0
foreach ($m in $dims) {
  $j++
  Add-Line "DIMM[$j] capacityGB=$([math]::Round($m.Capacity/1GB,0)) speedMT_s=$($m.Speed) configuredMT_s=$($m.ConfiguredClockSpeed) manufacturer=$($m.Manufacturer)"
}
foreach ($pf in @(Get-CimInstance Win32_PageFileUsage)) {
  Add-Line "PageFile=$($pf.Name) allocatedMB=$($pf.AllocatedBaseSize) peakUsageMB=$($pf.PeakUsage)"
}

Add-Line "=== DISK ==="
Add-Line "TEMP=$env:TEMP"
foreach ($d in @(Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3')) {
  Add-Line "$($d.DeviceID) total_GB=$([math]::Round($d.Size/1GB,1)) free_GB=$([math]::Round($d.FreeSpace/1GB,1)) fs=$($d.FileSystem)"
}
# Sequential-write probe: 256MB single stream incl. flush-to-disk, same
# method on every runner => cross-VM comparable (slow-disk family attribution;
# also indirectly measures Defender pre-exclusion scan overhead).
try {
  $bench = Join-Path $env:TEMP 'envbench.tmp'
  $buf = New-Object 'byte[]' (1MB)
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $fs = [System.IO.File]::Create($bench)
  for ($k = 0; $k -lt 256; $k++) { $fs.Write($buf, 0, $buf.Length) }
  $fs.Flush($true)
  $fs.Close()
  $sw.Stop()
  Add-Line "SeqWrite256MB=$([math]::Round(256/$sw.Elapsed.TotalSeconds,1))MB/s flush-incl ($([math]::Round($sw.Elapsed.TotalSeconds,2))s)"
  Remove-Item $bench -Force
} catch {
  Add-Line "SeqWrite256MB=ERROR $($_.Exception.Message)"
}

Add-Line "=== NETWORK ==="
foreach ($a in @(Get-NetAdapter | Where-Object { $_.NetEnabled })) {
  Add-Line "NIC=$($a.Name) speed=$($a.LinkSpeed) desc=$($a.InterfaceDescription)"
}
$pg = (ping.exe -n 1 github.com 2>&1 | Select-String -Pattern 'Reply|timed out|could not find').Line
Add-Line "Ping github.com: $pg"

Add-Line "=== LIMITS / PRESSURE ==="
Add-Line "Processes=$((Get-Process).Count)"

Add-Line "=== SECURITY / POWER ==="
$mp = Get-MpComputerStatus
Add-Line "Defender enabled=$($mp.AntivirusEnabled) realTime=$($mp.RealTimeProtectionEnabled) sigAgeHrs=$($mp.AntivirusSignatureAge)"
$pp = (powercfg.exe /getactivescheme 2>&1) -join ' '
Add-Line "PowerPlan=$pp"
$lic = Get-CimInstance SoftwareLicensingProduct -Filter "ApplicationID='55c92734-d682-4d71-983e-d6ec3f16059f' AND PartialProductKey IS NOT NULL" | Select-Object -First 1
Add-Line "WindowsLicenseStatus=$($lic.LicenseStatus) (1=Licensed)"

Add-Line "=== TOOLCHAINS (image baseline; setup steps may install newer) ==="
Add-Line "PowerShell=$($PSVersionTable.PSVersion)"
foreach ($t in @('rustc','cargo','node','npm','go','git','cargo-nextest')) {
  $v = $null
  try {
    if ($t -eq 'go') { $v = (& go version 2>&1 | Select-Object -First 1) }
    else             { $v = (& $t --version 2>&1 | Select-Object -First 1) }
  } catch {}
  if ([string]::IsNullOrWhiteSpace("$v")) { $v = '(absent)' }
  Add-Line "$t = $v"
}
$hh = (rustc -vV 2>&1 | Select-String '^host:').Line
Add-Line "rustc_$hh"

$out = ($lines -join "`n")
Write-Output $out
if ($env:GITHUB_STEP_SUMMARY) {
  Add-Content -Path $env:GITHUB_STEP_SUMMARY -Value @('```text', $out, '```')
}
exit 0
