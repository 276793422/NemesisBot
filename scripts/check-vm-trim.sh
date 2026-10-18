#!/usr/bin/env bash
# ============================================================================
# P5 vm feature 裁剪双态检查（check-vm-trim）——「编译开关三线」回归门
#
# 验证 vm feature 的两态编译（全量开 / 收窄关）+ 三线同步（features.toml 条目
# / nemesisbot Cargo feature / IoT 预设钉 false）。vm 关 = 整个
# nemesis-sandbox::vm 模块不参与编译（executor.vm 配置缺消费者属预期）。
#
# 用法（仓库根目录）：
#   bash scripts/check-vm-trim.sh            # 两态 cargo check + 三线 grep
#
# 退出码：0 = 全绿；非 0 = 有红灯，先修。
# ============================================================================
set -u
cd "$(dirname "$0")/.." || exit 1

FAIL=0
step() { printf '\n===== %s =====\n' "$*"; }

step "三线同步 grep（features.toml 条目 / Cargo feature / IoT 预设）"
grep -q '^id = "vm"' scripts/customize/features.toml \
    || { echo "!! features.toml 缺 vm 条目"; FAIL=1; }
grep -q '^vm = \["dep:nemesis-sandbox", "nemesis-sandbox/vm", "nemesis-web/vm"\]' \
    nemesisbot/Cargo.toml \
    || { echo "!! nemesisbot/Cargo.toml 缺 vm feature（穿透三连）"; FAIL=1; }
for p in minimal-iot minimal-agent; do
    grep -q '^vm = false' "scripts/customize/profiles/$p.config" \
        || { echo "!! 预设 $p 未钉 vm = false"; FAIL=1; }
done
grep -q '^vm = \[\]' crates/nemesis-web/Cargo.toml \
    || { echo "!! nemesis-web/Cargo.toml 缺 vm feature"; FAIL=1; }

step "全量态 cargo check（vm 开，default features）"
cargo check -p nemesisbot -p nemesis-sandbox -p nemesis-config -p nemesis-exec-protocol 2>&1 | tail -3
[ "${PIPESTATUS[0]}" -ne 0 ] && FAIL=1

step "收窄态 cargo check（vm 关，sandbox 开——vm 模块必须退出编译）"
cargo check -p nemesisbot --no-default-features \
    --features "channels-web,channels-rpc,security,sandbox" 2>&1 | tail -3
[ "${PIPESTATUS[0]}" -ne 0 ] && FAIL=1

step "vm 模块开态单测（nemesis-sandbox --features vm）"
cargo test -p nemesis-sandbox --features vm --quiet 2>&1 | grep -E "^test result|^error"
[ "${PIPESTATUS[0]}" -ne 0 ] && FAIL=1

printf '\n'
if [ "$FAIL" -eq 0 ]; then
    echo "check-vm-trim: ALL GREEN"
else
    echo "check-vm-trim: FAIL（见上方 !! 行）"
fi
exit "$FAIL"
