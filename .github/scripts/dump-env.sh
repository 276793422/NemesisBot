#!/usr/bin/env bash
# Platform environment fingerprint (2026-10-12 device-attribution discipline).
# full mode (default): complete hardware/OS/network/pressure/toolchain identity
# of the assigned hosted runner -> step log + GITHUB_STEP_SUMMARY, so
# slow-machine / freeze / lost-communication incidents can be attributed to
# the assigned device with evidence instead of guesswork. Ten domains:
#   runner identity+image wave / OS+kernel / virtualization+cgroup quota /
#   CPU(+contention probe) / memory / disk(+write probe) / network(+RTT probe)
#   / limits+pressure / toolchain baseline.
# brief mode: identity + live pressure snapshot right before test execution.
# All probes failure-guarded; step never reds because a probe is unavailable.

set +e
MODE="${1:-full}"

if [ "$MODE" = "brief" ]; then
  echo "NowUTC=$(date -u +%Y-%m-%dT%H:%M:%S%z) (pre-test pressure snapshot)"
  echo "RUNNER_NAME=${RUNNER_NAME:-} ImageVersion=${ImageVersion:-}"
  echo "loadavg=$(cat /proc/loadavg 2>/dev/null)"
  free -m 2>/dev/null | grep -E '^(Mem|Swap)'
  df -m / 2>/dev/null | tail -1
  exit 0
fi

tmp="$(mktemp)"
{
  echo "NowUTC=$(date -u +%Y-%m-%dT%H:%M:%S%z) TZ=$(date +%Z 2>/dev/null)"
  echo "=== RUNNER IDENTITY ==="
  echo "RUNNER_NAME=${RUNNER_NAME:-}"
  echo "RUNNER_ENVIRONMENT=${RUNNER_ENVIRONMENT:-} RUNNER_OS=${RUNNER_OS:-} RUNNER_ARCH=${RUNNER_ARCH:-}"
  echo "ImageOS=${ImageOS:-} ImageVersion=${ImageVersion:-}"
  echo "=== OS ==="
  grep -E '^(PRETTY_NAME|VERSION)=' /etc/os-release 2>/dev/null
  echo "Kernel=$(uname -r) Arch=$(uname -m)"
  echo "Boot=$(uptime -s 2>/dev/null)"
  echo "=== VIRTUALIZATION / CGROUP ==="
  echo "virt=$(systemd-detect-virt 2>/dev/null || echo unknown)"
  # Resource caps the container/host actually enforces: nproc can say 4 while a
  # cgroup quota throttles to less — this is where throttling-style slowness hides.
  if [ -f /sys/fs/cgroup/cpu.max ]; then
    echo "cgroup_cpu_max=$(cat /sys/fs/cgroup/cpu.max 2>/dev/null)"
    echo "cgroup_memory_max=$(cat /sys/fs/cgroup/memory.max 2>/dev/null)"
  elif [ -f /sys/fs/cgroup/cpu/cpu.cfs_quota_us ]; then
    echo "cgroup_cpu_quota=$(cat /sys/fs/cgroup/cpu/cpu.cfs_quota_us 2>/dev/null)/$(cat /sys/fs/cgroup/cpu/cpu.cfs_period_us 2>/dev/null)"
    echo "cgroup_memory_max=$(cat /sys/fs/cgroup/memory/memory.limit_in_bytes 2>/dev/null)"
  fi
  echo "=== CPU ==="
  echo "nproc=$(nproc)"
  lscpu 2>/dev/null | grep -E '^(Model name|CPU family|Model:|CPU\(s\)|Socket|Core|Thread|NUMA node\(s\)|CPU max|CPU min|Virtualization|Hypervisor vendor|L[1-3][di]? cache)'
  echo "cpuinfo_MHz=$(grep -m1 'cpu MHz' /proc/cpuinfo 2>/dev/null | awk '{print $4}')"
  # Bash int loop as a contention probe: same image/bash on every runner =>
  # cross-VM comparable. Slow number with matching cpu model = host steal.
  t0=$(date +%s%N)
  i=0; x=0
  while [ "$i" -lt 1000000 ]; do x=$((x + i % 7)); i=$((i + 1)); done
  t1=$(date +%s%N)
  echo "CpuProbe1M=$(( (t1 - t0) / 1000000 ))ms (bash int loop, x=$x; slower=host contention)"
  echo "=== MEMORY ==="
  free -h 2>/dev/null
  echo "=== DISK ==="
  echo "TMPDIR=${TMPDIR:-/tmp}"
  df -hT / 2>/dev/null || df -h / 2>/dev/null
  # Sequential-write probe: 256MB incl. fdatasync, same method on every runner
  # => cross-VM comparable (slow-disk family attribution).
  dd_res=$(dd if=/dev/zero of="${TMPDIR:-/tmp}/envbench.tmp" bs=1M count=256 conv=fdatasync 2>&1 | tail -1)
  echo "SeqWrite256MB: $dd_res"
  rm -f "${TMPDIR:-/tmp}/envbench.tmp"
  echo "=== NETWORK ==="
  for n in $(ls /sys/class/net 2>/dev/null); do
    s=$(cat "/sys/class/net/$n/speed" 2>/dev/null)
    echo "NIC=$n speed=${s:-?}Mbps"
  done
  echo "Ping github.com: $(ping -c 1 -W 2 github.com 2>&1 | grep -E 'time=|100% packet|unknown host|Name or service' | head -1)"
  echo "=== LIMITS / PRESSURE ==="
  echo "ulimit_n_soft=$(ulimit -Sn) hard=$(ulimit -Hn)"
  echo "processes=$(ls -d /proc/[0-9]* 2>/dev/null | wc -l)"
  echo "loadavg=$(cat /proc/loadavg 2>/dev/null)"
  echo "=== TOOLCHAINS (image baseline; setup steps may install newer) ==="
  for t in rustc cargo node npm go git gcc clang cargo-nextest; do
    if [ "$t" = "go" ]; then
      v="$(go version 2>/dev/null | head -1)"
    else
      v="$("$t" --version 2>/dev/null | head -1)"
    fi
    [ -z "$v" ] && v="(absent)"
    echo "$t = $v"
  done
  echo "rustc_$(rustc -vV 2>/dev/null | grep '^host:')"
} | tee "$tmp"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  { echo '```text'; cat "$tmp"; echo '```'; } >> "$GITHUB_STEP_SUMMARY"
fi
rm -f "$tmp"
exit 0
