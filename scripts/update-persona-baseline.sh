#!/usr/bin/env bash
# 人格超市基线快照刷新（拍板 B1：纯手动——想刷的时候自己跑，无自动流程触发）。
#
# 做什么：拉取 agency-agents 上游 tarball → 机械过滤全量 *.md → 覆盖
# plugins/plugin-resources/resources/persona-catalog/agency-agents/md/ 基线
# 目录 + 重写 manifest.json（基线溯源记录） → git diff --stat 报告变更规模。
#
# 边界（设计拍板 2026-11-12）：
# - 机械过滤（*.md）在本脚本与插件 build.rs 之间共享口径
#   （plugin-resources 的 resource_filter：只留 .md）；语义过滤
#   （SKIP_DIRS/SKIP_FILENAMES，即 is_agent_file）留消费侧——基线存全量
#   *.md，README/策略文件等由 web/CLI 的 is_agent_file 在运行期滤掉。
# - 语义上非人格的 .md（如某些目录下的杂务文档）会进基线——无害：消费侧
#   过滤兜住；基线保持「忠实上游镜像」形态，与在线臂同一枚举口径。
# - 提交与否由用户自己决定（本脚本只改工作区文件，不 git commit）。
#
# 用法：bash scripts/update-persona-baseline.sh
# 依赖：curl（或 wget）、tar；网络可达 github.com。

set -euo pipefail

REPO_URL="https://github.com/msitarzewski/agency-agents"
BRANCH="main"
PLUGIN_DIR="$(cd "$(dirname "$0")/.." && pwd)/plugins/plugin-resources"
BASELINE_META="$PLUGIN_DIR/resources/persona-catalog/agency-agents/manifest.json"

echo "== 人格超市基线快照刷新 =="
echo "上游: $REPO_URL ($BRANCH)"

# 1. 取上游 commit（指纹；ls-remote 失败 = 诚实退出，不瞎刷）
COMMIT="$(git ls-remote "$REPO_URL.git" "$BRANCH" 2>/dev/null | awk '{print $1; exit}')"
if [ -z "$COMMIT" ]; then
    echo "ERROR: 无法探测上游 commit（离线？仓库被删？）——基线保持不动" >&2
    exit 1
fi
echo "上游 commit: $COMMIT"

# 2. 检查上游是否真的变了（与现基线同 commit = 无事可做，幂等早退）
if [ -f "$BASELINE_META" ]; then
    CURRENT_COMMIT="$(node -e "console.log(JSON.parse(require('fs').readFileSync(process.argv[1],'utf8')).commit)" "$BASELINE_META" 2>/dev/null \
        || grep -o '"commit": *"[^"]*"' "$BASELINE_META" | head -1 | sed 's/.*"commit": *"//;s/"$//')"
    if [ "$CURRENT_COMMIT" = "$COMMIT" ]; then
        echo "基线已是最新（$COMMIT）——无事可做"
        exit 0
    fi
    echo "基线更新: $CURRENT_COMMIT -> $COMMIT"
fi

# 3. 下载 codeload tarball（走独立临时目录；上传目录内容不落仓库）
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
TARBALL="$WORK/snap.tar.gz"
echo "下载 tarball..."
if ! curl -fsSL --connect-timeout 10 --max-time 300 -o "$TARBALL" \
        "https://codeload.github.com/msitarzewski/agency-agents/tar.gz/$COMMIT"; then
    echo "ERROR: tarball 下载失败——基线保持不动" >&2
    exit 1
fi

# 4. 解包 + 机械过滤 *.md 覆盖基线 md/ 目录（整目录换血——上游删除的文件
#    同步消失，不做增量）
EXTRACTED="$WORK/extracted"
mkdir -p "$EXTRACTED"
tar -xzf "$TARBALL" -C "$EXTRACTED"
SRC_ROOT="$(find "$EXTRACTED" -mindepth 1 -maxdepth 1 -type d | head -1)"
if [ -z "$SRC_ROOT" ]; then
    echo "ERROR: tarball 解包后无内容" >&2
    exit 1
fi

MD_DIR="$PLUGIN_DIR/resources/persona-catalog/agency-agents/md"
# 先计数后换血（0 个 md = 上游结构剧变：此时不能动 md/——先 rm 再发现空
# 会留下空基线目录，下次构建 fail-loud，且报错文案「保持不动」变谎话）。
EXPECTED_COUNT="$(find "$SRC_ROOT" -type f -name '*.md' | wc -l)"
if [ "$EXPECTED_COUNT" -eq 0 ]; then
    echo "ERROR: 上游未过滤出任何 *.md——疑似仓库内容结构剧变，基线保持不动" >&2
    exit 1
fi
rm -rf "$MD_DIR"
mkdir -p "$MD_DIR"
COUNT=0
while IFS= read -r -d '' f; do
    rel="${f#"$SRC_ROOT"/}"
    dest="$MD_DIR/$rel"
    mkdir -p "$(dirname "$dest")"
    cp "$f" "$dest"
    COUNT=$((COUNT + 1))
done < <(find "$SRC_ROOT" -type f -name '*.md' -print0)

# 5. 重写 manifest.json（基线溯源记录）（真实 UTC 时刻——与 build.rs fetched/FILE 臂
#    的 utc_now_string 同粒度；label 只展示日期段）
TODAY="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
cat > "$BASELINE_META" <<EOF
{
  "repo": "msitarzewski/agency-agents",
  "branch": "main",
  "commit": "$COMMIT",
  "fetched_at_utc": "$TODAY",
  "file_count": $COUNT,
  "license": "MIT"
}
EOF

# 6. 同步 LICENSE-upstream（上游 LICENSE 文件若存在则跟随更新）
UPSTREAM_LICENSE="$(find "$SRC_ROOT" -maxdepth 1 -iname 'LICENSE*' | head -1)"
if [ -n "$UPSTREAM_LICENSE" ]; then
    cp "$UPSTREAM_LICENSE" "$PLUGIN_DIR/LICENSE-upstream"
fi

echo ""
echo "== 完成：$COUNT 个 *.md 已入基线 =="
echo "变更规模："
git -C "$(dirname "$0")/.." diff --stat -- plugins/plugin-resources/resources/ | tail -5 || true
echo ""
echo "提示：基线刷新只改工作区，commit 与否由你决定；下一次 plugin-resources"
echo "重编译（含 build.rs rerun）即携带新基线。官方构建脚本仍会按指纹直拉更新。"
