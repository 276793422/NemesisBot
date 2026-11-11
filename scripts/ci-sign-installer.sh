#!/usr/bin/env bash
# ============================================================================
# ci-sign-installer.sh — CI 侧 NBSETUP1 安装包打包 + 两层签名
# （daily-release.yml 的 Package installer step 调用）
#
# 用法:   ci-sign-installer.sh <payload_dir> <skeleton> <out_file> <version> <platform>
#   payload_dir   载荷目录（CI = staging/，二进制已被 ci-sign-artifacts.sh 签过；
#                 pack 只读不写，压缩包打包不受影响）
#   skeleton      安装器骨架（installer/target/release/nbsetup-win.exe|nbsetup-linux，
#                 编译期锚经 NEMESIS_BUILD_ROOT_ANCHOR 已固化——与 bot 二进制同机制同值）
#   out_file      输出安装包（nightly-windows-setup.exe / nightly-linux-setup.run）
#   version       manifest.bot_version（git describe --tags --abbrev=0，同本地脚本）
#   platform      windows | linux
#
# 环境变量:
#   NMB_ISSUING_SK / NMB_ISSUING_CERT   Secrets 注入（与 ci-sign-artifacts.sh 同源——
#                                       同一签发链 → 安装包与 bot 二进制同一信任根）。
#   GITHUB_SHA                          叶证书 CN 标识（Actions 默认 env 自带）
#   NMB_ROOT_CERT                       根证书路径（缺省 = 仓库 certs/root_cert.der；
#                                       测试/演练缝——本地用自生成体系走全链时指到
#                                       演练根，CI 不设 = 生产行为不变）
#
# 签名两层（与本地 installer/build-*.bat|.sh --sign 同链）:
#   pack --sign <keys>   manifest blob 签名（防载荷被换；install 引擎释放前
#                        对 manifest 分级消费）
#   pack sign <file>     整包外壳签名（安装器启动自检的绿级来源）
#
# 降级语义（镜像 ci-sign-artifacts.sh，全部诚实、不静默）:
#   1. Secrets 未配置                              → WARN + 未签安装包照常产出
#                                                    （fork 零门槛，nightly 不缺席）
#   2. 签名工具链不就绪（exe-sign-tool / mint-leaf / root_cert.der 缺）→ 同上
#   3. pack 二进制缺席 / 载荷缺必需文件             → exit 1（fatal——安装包是
#                                                    nightly 承诺产物，构建逻辑错不静默）
#   4. 签后自检失败（外壳验签非 Valid）             → exit 1（fail-the-build，
#                                                    防止静默发坏链产物）
# ============================================================================
set -euo pipefail

PAYLOAD="${1:?usage: ci-sign-installer.sh <payload_dir> <skeleton> <out> <version> <platform>}"
SKELETON="${2:?缺少骨架路径}"
OUT="${3:?缺少输出路径}"
VERSION="${4:?缺少版本号}"
PLATFORM="${5:?缺少平台}"

[ -d "$PAYLOAD" ] || { echo "❌ 载荷目录不存在: $PAYLOAD"; exit 1; }
[ -f "$SKELETON" ] || { echo "❌ 骨架不存在: $SKELETON（先构建 installer workspace）"; exit 1; }
case "$PLATFORM" in
    windows|linux) ;;
    *) echo "❌ platform 非法: $PLATFORM（windows|linux）"; exit 1 ;;
esac

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PY="$(command -v python3 || command -v python)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# --- pack 二进制（installer workspace 的 [[bin]]；缺席 = fatal，非降级）-------
PACK=""
for cand in "$REPO_ROOT/installer/target/release/pack.exe" \
            "$REPO_ROOT/installer/target/release/pack"; do
    [ -x "$cand" ] && PACK="$cand" && break
done
[ -n "$PACK" ] || {
    echo "❌ pack 二进制不存在（workflow 需先构建 installer workspace："
    echo "   cargo build --release -p setup-windows -p setup-core / -p setup-linux -p setup-core）"
    exit 1
}

# --- 签名材料（与 ci-sign-artifacts.sh 同源同链；任一缺席 = 未签降级，非 fatal）---
TOOL=""
KEYS=""
ROOT_CERT="${NMB_ROOT_CERT:-$REPO_ROOT/certs/root_cert.der}"
if [ -z "${NMB_ISSUING_SK:-}" ] || [ -z "${NMB_ISSUING_CERT:-}" ]; then
    echo "⚠️ 未配置 NMB_ISSUING_SK_HEX / NMB_ISSUING_CERT_HEX Secrets——安装包不签名（黄色提示形态）"
elif [ ! -f "$ROOT_CERT" ]; then
    echo "⚠️ 根证书不存在: $ROOT_CERT（信任根未提交进仓库）——安装包不签名"
else
    for cand in "$REPO_ROOT/target/release/exe-sign-tool.exe" \
                "$REPO_ROOT/target/release/exe-sign-tool"; do
        [ -x "$cand" ] && TOOL="$cand" && break
    done
    if [ -z "$TOOL" ] || ! "$TOOL" mint-leaf --help >/dev/null 2>&1; then
        echo "⚠️ exe-sign-tool / mint-leaf 不就绪（workflow 需先 cargo build --release -p exe-sign-tool）——安装包不签名"
        TOOL=""
    fi
fi

if [ -n "$TOOL" ]; then
    # 组装 issuing 材料（KeyHierarchyJson v2，空串字段 = sign-only 容忍语义）
    # → 铸当日叶（与 ci-sign-artifacts.sh 逐行同源：同一签发链 = 同一信任根）。
    echo "==> 组装 issuing 材料（Secrets 值不回显）"
    NMB_ISSUING_SK="$NMB_ISSUING_SK" NMB_ISSUING_CERT="$NMB_ISSUING_CERT" \
    "$PY" - "$ROOT_CERT" "$WORK/issuing.ci.json" <<'PYEOF'
import json, binascii, os, sys
root_hex = binascii.hexlify(open(sys.argv[1], 'rb').read()).decode()
json.dump({
    "version": 2,
    "root_sk": "",
    "root_cert": root_hex,
    "issuing_sk": os.environ["NMB_ISSUING_SK"],
    "issuing_cert": os.environ["NMB_ISSUING_CERT"],
    "leaf_sk": "", "leaf_cert": "",
}, open(sys.argv[2], 'w', encoding='utf-8'))
PYEOF
    SHORT_SHA="${GITHUB_SHA:-unknown}"; SHORT_SHA="${SHORT_SHA:0:8}"
    echo "==> 铸叶证书（CN=NemesisBot CI $SHORT_SHA, 365d）"
    "$TOOL" mint-leaf --issuing "$WORK/issuing.ci.json" --days 365 \
        --cn "NemesisBot CI $SHORT_SHA" --out "$WORK/ci-keys.json"
    KEYS="$WORK/ci-keys.json"
fi

# --- 打包（payload 只读；排除规则对 staging 天然干净）-------------------------
echo "==> 打包 NBSETUP1（版本 $VERSION，平台 $PLATFORM，载荷 $PAYLOAD）"
PACK_ARGS=(pack
    --payload "$PAYLOAD"
    --skeleton "$SKELETON"
    --out "$OUT"
    --version "$VERSION"
    --platform "$PLATFORM")
if [ -n "$KEYS" ]; then
    PACK_ARGS+=(--sign "$KEYS")
fi
"$PACK" "${PACK_ARGS[@]}"

# --- 整包外壳签名（两层信任链第二层；fresh pack 未签过 → 无追签冲突）----------
if [ -n "$KEYS" ]; then
    echo "==> 整包外壳签名（启动自检绿级来源）"
    "$PACK" sign "$OUT" --keys "$KEYS"
fi

# --- 检视（信息性；布局/分级/文件数留 CI 日志）---------------------------------
echo "==> 检视成品"
"$PACK" inspect "$OUT"

# --- 签后自检（fail-the-build 语义）：外壳签名必须 Valid ----------------------
if [ -n "$KEYS" ]; then
    echo "==> 签后自检（外壳签名必须 Valid）"
    GOT="$("$TOOL" verify --root-cert "$ROOT_CERT" --target "$OUT" 2>&1 | head -1)" || GOT="(verify 失败)"
    if [ "${GOT#Valid}" != "$GOT" ]; then
        echo "    [Valid] $(basename "$OUT") 外壳签名"
    else
        echo "    [FAIL]  $(basename "$OUT") 外壳签名 → $GOT"
        echo "❌ 签后自检失败——按 fail-the-build 语义退出"
        exit 1
    fi
fi

echo "==> ✅ 安装包产出: $OUT（$([ -n "$KEYS" ] && echo '已签（两层，同 bot 信任根）' || echo '未签降级')）"
