#!/usr/bin/env bash
# README Rust 版本与 rust-toolchain.toml 一致性校验（防文档漂移）。
# 复评整改批次 A 加固项（复评整改计划，docs 归档于 REPORT/2026-10-15）：
# 此前 README badge/环境要求停在 1.85+，toolchain 已钉 1.95.0，公开文档与事实脱节。
# CI 挂载于 ci.yml rust-check job；本地可直接跑：bash scripts/check-doc-version.sh
set -euo pipefail
cd "$(dirname "$0")/.."

toolchain=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml | head -1)
if [ -z "$toolchain" ]; then
    echo "FAIL: rust-toolchain.toml 未找到 channel = \"x.y.z\" 版本行"
    exit 1
fi

fail() {
    echo "FAIL: $1（rust-toolchain.toml = ${toolchain}）"
    exit 1
}

# badge 形态：img.shields.io/badge/Rust-<version>-
grep -q "badge/Rust-${toolchain}-" README.md ||
    fail "README badge 的 Rust 版本与 toolchain 不一致"

# 环境要求行形态：Rust <version>（
grep -qE "Rust ${toolchain}（" README.md ||
    fail "README 环境要求的 Rust 版本与 toolchain 不一致"

echo "OK: README Rust 版本 = ${toolchain} = rust-toolchain.toml"
