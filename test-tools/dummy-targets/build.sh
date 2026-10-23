#!/usr/bin/env bash
# dummy-targets 构建脚本（紫微系统 T0.8）：x64 + x86 双架构 → bin/{x64,x86}/
# 用法：bash test-tools/dummy-targets/build.sh
set -euo pipefail
cd "$(dirname "$0")"

have() { command -v "$1" >/dev/null 2>&1; }

build_one() {
    local target="$1" out="$2"
    if ! rustup target list --installed | grep -q "^${target}\$"; then
        echo "[dummy-targets] rustup target 未安装：${target}（rustup target add ${target}）——跳过该架构" >&2
        return 1
    fi
    for proj in dummy-draw dummy-monitor; do
        (cd "$proj" && cargo build --release --target "$target")
        mkdir -p "bin/${out}"
        cp "${proj}/target/${target}/release/${proj}.exe" "bin/${out}/${proj}.exe"
        echo "[dummy-targets] ${out}/${proj}.exe $(stat -c%s "bin/${out}/${proj}.exe" 2>/dev/null || wc -c < "bin/${out}/${proj}.exe") bytes"
    done
}

rc=0
build_one x86_64-pc-windows-msvc x64 || rc=1
build_one i686-pc-windows-msvc   x86 || rc=1
exit "$rc"
