#!/usr/bin/env bash
# ============================================================================
# 推送前双端验证（pre-push verify）——规避「本地绿 CI 红」的系统性防线
#
# 背景：2026-09-04 用户要求「规避 CI 红灯，而不是红灯后分析」。CI 的实质
# 门禁分两侧：Windows 侧本地可跑；Linux 侧（clippy 编译 cfg(not(windows))
# 代码 + Linux 行为全量测试）本地 Windows 永远看不见。本脚本把 Linux 门禁
# 搬到推送前：bundle 增量同步 → 远程 Linux worktree（不碰远程工作区）→
# 跑与 CI 完全相同的命令。
#
# 用法（仓库根目录）：
#   bash scripts/pre-push-verify.sh          # 快速层：fmt+内联+本地 clippy+远程 clippy
#   bash scripts/pre-push-verify.sh --full   # 完整层：+ 两端全量 workspace 测试
#                                            #   （远程带 ulimit -n 8192，等同 CI nightly job）
#
# 退出码：0 = 双端全绿，可提交/推送；非 0 = 有红灯，先修。
# 可配置环境变量：REMOTE（默认 zoo@192.168.137.237）
# ============================================================================
set -u
cd "$(dirname "$0")/.." || exit 1

REMOTE="${REMOTE:-zoo@192.168.137.237}"
REPO_DIR="/home/zoo/桌面/NemesisBot/nemesisbot_rust"
WORKTREE="/home/zoo/nb-verify"
FULL=0
[ "${1:-}" = "--full" ] && FULL=1
# 跟随当前分支（goal 期间工作分支可能是 devtool-upgrade，不硬编码 main）
BRANCH=$(git rev-parse --abbrev-ref HEAD)

FAIL=0
step() { printf '\n===== %s =====\n' "$*"; }

# ssh/scp 链路间歇丢包：失败自动重试一次
net_retry() {
    "$@" && return 0
    echo "!! 网络操作失败，5s 后重试一次: $*"
    sleep 5
    "$@"
}

# ---------------------------------------------------------------------------
# 本地（Windows）门禁
# ---------------------------------------------------------------------------
step "本地 fmt --check"
cargo fmt --all -- --check; [ $? -ne 0 ] && FAIL=1

step "本地内联测试门禁"
bash scripts/check-inline-tests.sh || FAIL=1

step "本地 clippy --workspace --all-targets -D warnings"
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
[ "${PIPESTATUS[0]}" -ne 0 ] && FAIL=1

if [ "$FULL" -eq 1 ]; then
    step "本地全量 cargo test --workspace（数十分钟）"
    cargo test --workspace 2>&1 | tail -3
    [ "${PIPESTATUS[0]}" -ne 0 ] && FAIL=1
fi

# ---------------------------------------------------------------------------
# 远程同步：bundle 增量 + worktree（不碰远程工作区，那边可能有未提交改动）
# ---------------------------------------------------------------------------
step "远程同步（bundle 增量 → ${WORKTREE}）"

# bundle 基线 = worktree 当前 HEAD（复用时）或远程主仓 HEAD（首次）
BASE=$(ssh -o ConnectTimeout=10 "$REMOTE" \
    "git -C '${WORKTREE}' rev-parse HEAD 2>/dev/null || git -C '${REPO_DIR}' rev-parse HEAD")
if [ -z "$BASE" ]; then echo "!! 无法取得远程基线"; FAIL=1; else
echo "bundle 基线 = ${BASE}"
fi

BUNDLE=/tmp/nb-pre-push.bundle
PATCH=/tmp/nb-pre-push.patch
WANT=/tmp/nb-pre-push.want
if [ -n "${BASE:-}" ]; then
    AHEAD=$(git rev-list "${BASE}..${BRANCH}" --count)

    # --- 提交增量（bundle）：AHEAD=0 = 无新提交，跳过 ---
    if [ "${AHEAD}" -eq 0 ]; then
        echo "无新提交，跳过 bundle 同步"
    else
        echo "领先 ${AHEAD} 提交，bundle 增量同步"
        git bundle create "$BUNDLE" "${BASE}..${BRANCH}" || FAIL=1
        net_retry scp -o ConnectTimeout=10 "$BUNDLE" "$REMOTE:/home/zoo/nb-pre-push.bundle" || FAIL=1
        git rev-parse "$BRANCH" > "$WANT"
        net_retry scp -o ConnectTimeout=10 "$WANT" "$REMOTE:/home/zoo/nb-pre-push.want" || FAIL=1
        # pipefail 防「fetch 失败被管道吞」；校验 ref == 本地 HEAD（防旧 bundle 谎报绿）
        net_retry ssh -o ConnectTimeout=10 "$REMOTE" "set -eo pipefail
        cd '${REPO_DIR}'
        out=\$(git fetch /home/zoo/nb-pre-push.bundle refs/heads/${BRANCH}:refs/remotes/bundle/pre-push 2>&1) || { echo \"\$out\"; exit 1; }
        GOT=\$(git rev-parse bundle/pre-push)
        WANT=\$(cat /home/zoo/nb-pre-push.want)
        if [ \"\$GOT\" != \"\$WANT\" ]; then echo \"!! ref 失配 GOT=\$GOT WANT=\$WANT\"; exit 1; fi
        echo 远程 ref 已对齐 \$GOT" || FAIL=1
    fi

    # --- 未提交改动（patch）：独立于是否有新提交；无则清远程旧残留 ---
    if git diff --quiet "${BRANCH}" 2>/dev/null; then
        echo "本地工作区干净，无 patch"
        net_retry ssh -o ConnectTimeout=10 "$REMOTE" "rm -f /home/zoo/nb-pre-push.patch" || FAIL=1
    else
        git diff "${BRANCH}" > "$PATCH"
        echo "同步未提交改动（$(wc -l < "$PATCH") 行 diff）"
        net_retry scp -o ConnectTimeout=10 "$PATCH" "$REMOTE:/home/zoo/nb-pre-push.patch" || FAIL=1
    fi

    # --- worktree 对齐 + apply patch ---
    # checkout --force 清上次 apply 的脏改动：worktree 是验证替身，以本地真实状态为准
    net_retry ssh -o ConnectTimeout=10 "$REMOTE" "set -e
    if [ -d '${WORKTREE}' ]; then
        git -C '${WORKTREE}' checkout --detach --force $( [ "${AHEAD}" -eq 0 ] && echo HEAD || echo bundle/pre-push ) 2>&1 | tail -1
    else
        git -C '${REPO_DIR}' worktree add '${WORKTREE}' bundle/pre-push 2>&1 | tail -1
    fi
    if [ -s /home/zoo/nb-pre-push.patch ]; then
        git -C '${WORKTREE}' apply --whitespace=nowarn /home/zoo/nb-pre-push.patch && echo '未提交改动已应用'
    fi" || FAIL=1
fi

# ---------------------------------------------------------------------------
# 前端构建（include_dir! 前置，等同 ci.yml "Build Vue frontend" 步骤）
# static/ 是 gitignored 构建产物，worktree 首次要建；之后沿用（clippy/测试
# 不读其内容）。build 失败或 npm 缺失则落占位并 WARN——lint/测试不受影响，
# 但此时远程端验证不覆盖前端，诚实标注。
# ---------------------------------------------------------------------------
step "远程前端构建（include_dir! 前置）"
net_retry ssh -o ConnectTimeout=10 "$REMOTE" "bash -lic '
  if [ -f ${WORKTREE}/crates/nemesis-web/static/index.html ]; then
    echo 前端产物已存在，跳过构建
  elif command -v npm >/dev/null 2>&1; then
    cd ${WORKTREE}/web \
      && npm ci --no-audit --no-fund >/dev/null 2>&1 \
      && rm -rf ../crates/nemesis-web/static/assets \
      && npm run build >/dev/null 2>&1 \
      && echo FRONTEND_BUILT
  fi'"
if ! net_retry ssh -o ConnectTimeout=10 "$REMOTE" "test -f ${WORKTREE}/crates/nemesis-web/static/index.html"; then
    echo "!! 前端构建失败/npm 不可用 → static 落占位（lint/测试不受影响，前端质量门以 CI frontend job 为准）"
    net_retry ssh -o ConnectTimeout=10 "$REMOTE" \
        "mkdir -p ${WORKTREE}/crates/nemesis-web/static && echo placeholder > ${WORKTREE}/crates/nemesis-web/static/index.html" || FAIL=1
fi

# ---------------------------------------------------------------------------
# 远程（Linux）门禁——与 CI 完全相同的命令
# ---------------------------------------------------------------------------
step "远程 clippy --workspace --all-targets -D warnings（= ci.yml rust-check 核心）"
net_retry ssh -o ConnectTimeout=10 "$REMOTE" \
    "bash -lic 'cd ${WORKTREE} && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3; exit \${PIPESTATUS[0]}'" \
    || FAIL=1

if [ "$FULL" -eq 1 ]; then
    step "远程全量 cargo test --workspace + ulimit -n 8192（= CI nightly job，可达 1-2 小时）"
    net_retry ssh -o ConnectTimeout=10 "$REMOTE" \
        "bash -lic 'cd ${WORKTREE} && ulimit -n 8192 && cargo test --workspace --no-fail-fast 2>&1 | tail -5; exit \${PIPESTATUS[0]}'" \
        || FAIL=1
fi

# ---------------------------------------------------------------------------
# 汇总
# ---------------------------------------------------------------------------
step "结论"
if [ "$FAIL" -eq 0 ]; then
    echo "✅ 双端全绿——与 CI 实质门禁等效，可以提交/推送"
else
    echo "❌ 有红灯（见上），修复后重跑；不要带着红灯推送"
fi
exit "$FAIL"
