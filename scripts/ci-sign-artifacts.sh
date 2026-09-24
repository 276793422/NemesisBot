#!/usr/bin/env bash
# ============================================================================
# ci-sign-artifacts.sh — CI 侧 v4 签名（daily-release.yml 的 Sign artifacts step 调用）
#
# 计划: docs/PLAN/2026-09-23_ci-v4-signing-integration.md §5（P1.1）
#
# 用法:   ci-sign-artifacts.sh <staging_dir>
# 环境变量:
#   NMB_ISSUING_SK / NMB_ISSUING_CERT   Secrets 注入（= issuing.ci.json 的
#                                       issuing_sk / issuing_cert 字段 hex 原文）。
#                                       GitHub Actions 自动掩盖 Secrets 值的日志回显。
#   GITHUB_SHA                          构建 commit（叶证书 CN 标识用）
#
# 降级语义（三层，全部诚实、不静默）:
#   1. Secrets 未配置      → WARN + exit 0（产物无签名，构建绿——fork/新仓库零门槛，
#                            对齐 daily-release.yml Android job 的缺省兜底语义）
#   2. mint-leaf 子命令缺失（P0 代码未合入）→ WARN + exit 0（同上；YAML 可先于代码合入）
#   3. 签后自检任一非 Valid → exit 1（构建红——防止静默发出坏链产物）
#
# 各分发 staging（主 / 锁定版 / signserver / sign-tools）各调用一次本脚本——
# 锁定包内不允许出现未签名的锁定版二进制（接入计划 §2.2）。
# ============================================================================
set -euo pipefail

STAGING="${1:?usage: ci-sign-artifacts.sh <staging_dir>}"
[ -d "$STAGING" ] || { echo "❌ staging 目录不存在: $STAGING"; exit 1; }

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PY="$(command -v python3 || command -v python)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# --- 降级 1: Secrets 未配置 -------------------------------------------------
if [ -z "${NMB_ISSUING_SK:-}" ] || [ -z "${NMB_ISSUING_CERT:-}" ]; then
    echo "⚠️ 未配置 NMB_ISSUING_SK_HEX / NMB_ISSUING_CERT_HEX Secrets——产物跳过签名"
    echo "   （配置方法见 docs/PLAN/2026-09-23_ci-v4-signing-integration.md §6）"
    exit 0
fi

# --- 降级 2: 工具链未就绪 ----------------------------------------------------
TOOL=""
for cand in "$REPO_ROOT/target/release/exe-sign-tool.exe" \
            "$REPO_ROOT/target/release/exe-sign-tool"; do
    [ -x "$cand" ] && TOOL="$cand" && break
done
if [ -z "$TOOL" ]; then
    echo "⚠️ exe-sign-tool 二进制不存在（workflow 需先 cargo build --release -p exe-sign-tool）——跳过签名"
    exit 0
fi
if ! "$TOOL" mint-leaf --help >/dev/null 2>&1; then
    echo "⚠️ exe-sign-tool 缺 mint-leaf 子命令（P0 代码未合入）——跳过签名"
    exit 0
fi
[ -f "$REPO_ROOT/certs/root_cert.der" ] || {
    echo "⚠️ certs/root_cert.der 不存在（信任根未提交进仓库）——跳过签名"
    exit 0
}

# --- 组装 issuing 材料（KeyHierarchyJson 形态，空串字段 = P0.1 的 sign-only 容忍语义）---
echo "==> 组装 issuing 材料（Secrets 值不回显）"
NMB_ISSUING_SK="$NMB_ISSUING_SK" NMB_ISSUING_CERT="$NMB_ISSUING_CERT" \
"$PY" - "$REPO_ROOT/certs/root_cert.der" "$WORK/issuing.ci.json" <<'PYEOF'
import json, binascii, os, sys
root_hex = binascii.hexlify(open(sys.argv[1], 'rb').read()).decode()
json.dump({
    "version": 2,
    "root_sk": "",                                   # 空 = 私钥不在场（IssuingOnly 形态，mint-leaf 材料）
    "root_cert": root_hex,
    "issuing_sk": os.environ["NMB_ISSUING_SK"],      # Secrets hex 原文
    "issuing_cert": os.environ["NMB_ISSUING_CERT"],
    "leaf_sk": "", "leaf_cert": "",                  # 空 = 不在场（mint-leaf 现铸回填）
}, open(sys.argv[2], 'w', encoding='utf-8'))
PYEOF

# --- 铸当日叶（1 年期，CN 带构建标识）---------------------------------------
SHORT_SHA="${GITHUB_SHA:-unknown}"; SHORT_SHA="${SHORT_SHA:0:8}"
echo "==> 铸叶证书（CN=NemesisBot CI $SHORT_SHA, 365d）"
"$TOOL" mint-leaf --issuing "$WORK/issuing.ci.json" --days 365 \
    --cn "NemesisBot CI $SHORT_SHA" --out "$WORK/ci-keys.json"

# --- 签名: staging 内全部本方二进制（主程序/revoke-server/exe-sign-tool/plugins）----
echo "==> 签名 staging 产物"
# 枚举真实目录项再按名过滤——不能写 "$STAGING/nemesisbot" 这类无扩展名候选：
# MSYS2/git-bash 对 -f 测试有 .exe 自动回退（nemesisbot 不存在也判真），
# 原生 exe-sign-tool 拿到无扩展路径会 os error 2（真 CI windows-latest 必踩）。
TARGETS=()
for f in "$STAGING"/*; do
    [ -f "$f" ] || continue
    case "$(basename "$f")" in
        nemesisbot | nemesisbot.exe | revoke-server | revoke-server.exe | exe-sign-tool | exe-sign-tool.exe)
            TARGETS+=("$f") ;;
    esac
done
for f in "$STAGING/plugins/"*; do
    [ -f "$f" ] && TARGETS+=("$f")
done
[ ${#TARGETS[@]} -gt 0 ] || { echo "⚠️ staging 下无可签二进制——跳过签名"; exit 0; }

for t in "${TARGETS[@]}"; do
    echo "    sign: $(basename "$t")"
    # --out "$t" 原地签：CLI 缺省写 {target}.signed 新文件，不传会留原文件未签
    "$TOOL" sign --keys "$WORK/ci-keys.json" --target "$t" --out "$t"
done

# --- 根证书公开部分随包分发（先拷入，signatures.json 要引用它的指纹）---------
mkdir -p "$STAGING/certs"
cp "$REPO_ROOT/certs/root_cert.der" "$STAGING/certs/root_cert.der"

# --- 签名清单 ---------------------------------------------------------------
echo "==> 生成 signatures.json"
"$PY" - "$STAGING" "${TARGETS[@]}" <<'PYEOF'
import hashlib, json, sys, time, datetime
staging = sys.argv[1]
targets = sys.argv[2:]
root_fp = hashlib.sha256(open(f"{staging}/certs/root_cert.der", "rb").read()).hexdigest()
entries = []
for t in targets:
    data = open(t, "rb").read()
    entries.append({
        "file": str(t).replace(staging + "/", "", 1).replace("\\", "/"),
        "sha256": hashlib.sha256(data).hexdigest(),
        "signed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
    })
json.dump({
    "schema": 1,
    "tool": "nemesis-verify v4 (Authenticode 格式)",
    "chain_root_fp": root_fp,
    "leaf_cn": f"NemesisBot CI {__import__('os').environ.get('GITHUB_SHA', 'unknown')[:8]}",
    "leaf_validity_days": 365,
    "signatures": entries,
}, open(f"{staging}/signatures.json", "w", encoding="utf-8"), indent=2, ensure_ascii=False)
PYEOF

# --- 签后自检（fail the build 语义）------------------------------------------
echo "==> 签后自检（每个产物必须 Valid）"
FAIL=0
for t in "${TARGETS[@]}"; do
    GOT="$("$TOOL" verify --root-cert "$REPO_ROOT/certs/root_cert.der" --target "$t" 2>&1 | head -1)" || GOT="(verify 失败)"
    if [ "${GOT#Valid}" != "$GOT" ]; then
        echo "    [Valid] $(basename "$t")"
    else
        echo "    [FAIL]  $(basename "$t") → $GOT"
        FAIL=1
    fi
done
[ "$FAIL" -eq 0 ] || { echo "❌ 签后自检失败——按 fail-the-build 语义退出"; exit 1; }

echo "==> ✅ 签名完成: ${#TARGETS[@]} 个产物 + signatures.json + certs/root_cert.der"
