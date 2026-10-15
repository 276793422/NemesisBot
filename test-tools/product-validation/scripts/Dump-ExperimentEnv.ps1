# Dump-ExperimentEnv.ps1 - environment fingerprint (shared-device contention evidence)
# Usage: powershell -NoProfile -File Dump-ExperimentEnv.ps1 -OutFile <path.json>
param([Parameter(Mandatory=$true)][string]$OutFile)
$ErrorActionPreference = 'Continue'
$procs = @(Get-Process | Where-Object { $_.ProcessName -match '^(cargo|rustc|rustdoc|nemesisbot|node|npm|link|lld)' } |
    ForEach-Object { [PSCustomObject]@{ id=$_.Id; name=$_.ProcessName; cpu_sec=[math]::Round($_.CPU,1); ws_mb=[math]::Round($_.WorkingSet64/1MB,0) } })
$cpuLoad = $null
try { $cpuLoad = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average } catch {}
$freeMemGb = $null
try { $freeMemGb = [math]::Round((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory/1MB, 2) } catch {}
# Windows excluded port ranges: known source of bind-failure flakes (infra attribution)
$udpExcl = @(netsh interface ipv4 show excludedportrange protocol=udp 2>$null)
$tcpExcl = @(netsh interface ipv4 show excludedportrange protocol=tcp 2>$null)
[PSCustomObject]@{
    utc                = (Get-Date).ToUniversalTime().ToString('o')
    cpu_load_pct       = $cpuLoad
    free_mem_gb        = $freeMemGb
    relevant_processes = $procs
    udp_excluded_ranges = ($udpExcl -join "`n")
    tcp_excluded_ranges = ($tcpExcl -join "`n")
} | ConvertTo-Json -Depth 3 | Set-Content -Encoding UTF8 -Path $OutFile
Write-Output "env-dump: $OutFile (procs=$($procs.Count) cpu=$cpuLoad)"
