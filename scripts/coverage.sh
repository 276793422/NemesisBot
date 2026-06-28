#!/usr/bin/env bash
# NemesisBot coverage baseline (cargo-llvm-cov).
#
# Prerequisites (one-time):
#   rustup component add llvm-tools-preview
#   cargo install cargo-llvm-cov    # already 0.8.7 on this machine
#
# Usage:
#   scripts/coverage.sh                   # core crates, summary to stdout
#   scripts/coverage.sh --html            # + HTML report under target/coverage/html
#   scripts/coverage.sh nemesis-agent     # a single crate
#   scripts/coverage.sh --html nemesis-memory nemesis-security
#
# Note: the first run compiles instrumented binaries for every dependency — slow.
# Subsequent runs are incremental. See docs/INFO/2026-06-27_coverage-baseline.md.
set -euo pipefail

CORE_CRATES=(nemesis-agent nemesis-security nemesis-memory nemesis-web)

ARGS=()
HTML=0
for arg in "$@"; do
    case "$arg" in
        --html) HTML=1 ;;
        -*) echo "unknown flag: $arg" >&2; exit 2 ;;
        *) ARGS+=("-p" "$arg") ;;
    esac
done
if [ ${#ARGS[@]} -eq 0 ]; then
    for c in "${CORE_CRATES[@]}"; do
        ARGS+=("-p" "$c")
    done
fi

echo "[coverage] cargo llvm-cov ${ARGS[*]} --summary-only  (compiles instrumented binaries — slow on first run)"

if [ "$HTML" -eq 1 ]; then
    cargo llvm-cov "${ARGS[@]}" --html --output-dir target/coverage/html 2>&1 | tail -80
    echo "[coverage] HTML report: target/coverage/html/index.html"
else
    cargo llvm-cov "${ARGS[@]}" --summary-only 2>&1 | tail -80
fi
