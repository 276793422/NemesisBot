#!/usr/bin/env bash
# ============================================================================
# init-signing.sh — v4 签名体系一次性初始化（纯操作脚本，不改任何仓库代码）
#
# 对应计划: docs/PLAN/2026-09-23_ci-v4-signing-integration.md §6（P2 操作单）
# 原则: 原计划中所有网页 UI 操作全部命令行化（gh CLI）；唯一保留的手动步骤是
#       「root 私钥入密码管理器/冷备」——依赖用户所选密码管理器产品，无法通用
#       cmd 化。脚本把该步骤的待办文件整理到 Ceremony 目录并打印清单。
#
# 用法:
#   bash scripts/init-signing.sh <stage>
#
# stages:
#   build     编译 exe-sign-tool（release）
#   keygen    密钥仪式: keygen → split-keys → 提取 root_cert.der
#             （全量私钥落 Ceremony 目录 = 仓库外的 ~/nemesis-keyceremony/）
#   secrets   上传中间 CA 材料到 GitHub Secrets（gh secret set；缺 gh 时打印安装指引）
#   cert      root_cert.der 放入 certs/ 并 git add（提交由用户执行——提交纪律）
#   dispatch  触发 daily-release workflow_dispatch 并等待构建完成
#   verify    下载构建产物，跑正例 + 三负例验签（Valid/Tampered/Untrusted/NoSignature）
#   clean     销毁 Ceremony 目录明文私钥（必须 --yes；执行前确认 root 已入保险库！）
#   all       build → keygen → secrets → cert → dispatch → verify（clean 永远单独跑）
#
# 密钥红线（docs/PLAN/2026-09-23_ci-v4-signing-integration.md §10）:
#   本脚本产生的任何私钥文件绝不放进仓库目录；Ceremony 目录默认在仓库外。
# ============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CEREMONY_DIR="${NMB_KEYCEREMONY_DIR:-$HOME/nemesis-keyceremony}"
STAGE="${1:-}"
EXTRA="${2:-}"

# ---------------------------------------------------------------------------
# 工具发现: 优先 EXE_SIGN_TOOL 环境变量，其次 target/release 下的两种命名
# ---------------------------------------------------------------------------
find_tool() {
    if [ -n "${EXE_SIGN_TOOL:-}" ] && [ -x "${EXE_SIGN_TOOL}" ]; then
        echo "${EXE_SIGN_TOOL}"
        return 0
    fi
    for cand in "$REPO_ROOT/target/release/exe-sign-tool.exe" \
                "$REPO_ROOT/target/release/exe-sign-tool"; do
        [ -x "$cand" ] && { echo "$cand"; return 0; }
    done
    return 1
}

PY="$(command -v python3 || command -v python || true)"
[ -n "$PY" ] || { echo "❌ 需要 python（提取证书/篡改字节用），未找到"; exit 1; }

msg()  { echo ""; echo "==> $*"; }
die()  { echo "❌ $*" >&2; exit 1; }

need_gh() {
    if ! command -v gh >/dev/null 2>&1; then
        echo "❌ 未找到 gh CLI（GitHub 官方命令行）。安装方式二选一："
        echo "   1) winget install GitHub.cli        # MSI 安装，可能弹 UAC 确认框"
        echo "   2) portable（无弹窗）: 到 https://github.com/cli/cli/releases"
        echo "      下载 windows amd64 zip，解压后把 gh.exe 所在目录加入 PATH"
        echo "   装好后执行一次登录（device flow，终端给码→浏览器贴码，无表单）:"
        echo "      gh auth login"
        exit 1
    fi
    gh auth status >/dev/null 2>&1 || die "gh 未登录，先执行: gh auth login"
}

# ---------------------------------------------------------------------------
stage_build() {
    msg "编译 exe-sign-tool（release）"
    (cd "$REPO_ROOT" && cargo build --release -p exe-sign-tool)
    TOOL="$(find_tool)" || die "编译后仍未找到 exe-sign-tool 二进制"
    msg "工具就绪: $TOOL"
}

# ---------------------------------------------------------------------------
stage_keygen() {
    local TOOL
    TOOL="$(find_tool)" || die "先跑 build stage（或设置 EXE_SIGN_TOOL）"
    "$TOOL" split-keys --help >/dev/null 2>&1 \
        || die "当前 exe-sign-tool 没有 split-keys 子命令——P0 代码尚未实施（见计划 §4），先完成代码部分"

    if [ -f "$CEREMONY_DIR/keys.json" ]; then
        die "已存在 $CEREMONY_DIR/keys.json —— 拒绝覆盖已有密钥仪式产物；确要重来请先手动删除该目录"
    fi
    mkdir -p "$CEREMONY_DIR"

    msg "生成三级链（root → 发行锚 CA → leaf）到 $CEREMONY_DIR"
    "$TOOL" keygen --out "$CEREMONY_DIR/keys.json"

    msg "拆分: 根（离线）/ 中间 CA（进 Secrets）"
    # --root-cert-out 若当前工具版本不支持则回退 python 提取
    if "$TOOL" split-keys \
        --in "$CEREMONY_DIR/keys.json" \
        --root-out "$CEREMONY_DIR/root.offline.json" \
        --issuing-out "$CEREMONY_DIR/issuing.ci.json" \
        --root-cert-out "$CEREMONY_DIR/root_cert.der"; then
        :
    else
        echo "（split-keys 不支持 --root-cert-out，用 python 从 keys.json 提取根证书）"
        "$PY" - "$CEREMONY_DIR/keys.json" "$CEREMONY_DIR/root_cert.der" <<'PYEOF'
import json, binascii, sys
d = json.load(open(sys.argv[1]))
open(sys.argv[2], 'wb').write(binascii.unhexlify(d['root_cert']))
PYEOF
    fi

    chmod 600 "$CEREMONY_DIR"/*.json 2>/dev/null || true   # git-bash 下 best-effort

    msg "信任锚指纹（记录下来，验证端锚定用）"
    sha256sum "$CEREMONY_DIR/root_cert.der"

    cat <<EOF

============================================================
✅ 密钥仪式完成。Ceremony 目录: $CEREMONY_DIR
   keys.json          全量三级私钥（等价 root+issuing+leaf 全权）
   root.offline.json  根私钥 + 根证书   ← 要保管的
   issuing.ci.json    中间 CA 私钥+证书  ← 下一 stage 传 Secrets
   root_cert.der      根证书公开部分      ← 要提交进仓库

⚠️ 唯一保留的手动步骤（无通用 cmd 替代）:
   把 root.offline.json 和 keys.json 存入密码管理器附件（Bitwarden/
   1Password/KeePass）+ 一份离线冷备（U 盘）。若用 KeePass 类本地库，
   等效操作是把这两个文件复制进库目录。
   【确认已入库之前，不要跑 clean stage】
============================================================
EOF
}

# ---------------------------------------------------------------------------
stage_secrets() {
    local TOOL DIR
    DIR="$CEREMONY_DIR"
    [ -f "$DIR/issuing.ci.json" ] || die "缺少 $DIR/issuing.ci.json —— 先跑 keygen stage"
    need_gh

    msg "提取中间 CA 材料（值不回显，直接进 Secrets）"
    "$PY" - "$DIR/issuing.ci.json" "$DIR/.issuing_sk.tmp" "$DIR/.issuing_cert.tmp" <<'PYEOF'
import json, sys
d = json.load(open(sys.argv[1]))
open(sys.argv[2], 'w').write(d['issuing_sk'])
open(sys.argv[3], 'w').write(d['issuing_cert'])
PYEOF

    msg "上传 NMB_ISSUING_SK_HEX / NMB_ISSUING_CERT_HEX（gh 自动加密）"
    gh secret set NMB_ISSUING_SK_HEX   < "$DIR/.issuing_sk.tmp"
    gh secret set NMB_ISSUING_CERT_HEX < "$DIR/.issuing_cert.tmp"
    rm -f "$DIR/.issuing_sk.tmp" "$DIR/.issuing_cert.tmp"

    msg "✅ Secrets 配置完成（目标仓库 = 本目录 git remote 所指）"
    echo "   确认页: https://github.com/$(git -C "$REPO_ROOT" remote get-url origin \
        | sed -E 's#.*[:/]([^/]+/[^/]+)(\.git)?$#\1#')/settings/secrets/actions"
}

# ---------------------------------------------------------------------------
stage_cert() {
    local SRC="$CEREMONY_DIR/root_cert.der"
    [ -f "$SRC" ] || die "缺少 $SRC —— 先跑 keygen stage"

    msg "根证书公开部分入仓（certs/root_cert.der）"
    mkdir -p "$REPO_ROOT/certs"
    cp "$SRC" "$REPO_ROOT/certs/root_cert.der"
    msg "信任锚指纹（与 keygen stage 打印的一致性自查）"
    sha256sum "$REPO_ROOT/certs/root_cert.der"

    (cd "$REPO_ROOT" && git add certs/root_cert.der)
    msg "已 git add（按提交纪律，commit 由你执行），示例:"
    echo "   git commit -m \"ADD v4签名信任根公开证书certs/root_cert.der(anchor见docs计划)\""
}

# ---------------------------------------------------------------------------
stage_dispatch() {
    need_gh
    msg "触发 Daily Nightly Release（workflow_dispatch）"
    gh workflow run daily-release.yml
    sleep 8
    local RID
    RID="$(gh run list --workflow=daily-release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
    echo "    run id: $RID"
    msg "等待构建完成（nightly 构建约 10~30 分钟，Ctrl+C 退出不影响构建本身）"
    gh run watch "$RID" --exit-status
    echo "    构建完成。跑 verify stage 做四例验签: bash scripts/init-signing.sh verify $RID"
}

# ---------------------------------------------------------------------------
stage_verify() {
    local TOOL RID DL BIN DIRNAME
    TOOL="$(find_tool)" || die "先跑 build stage"
    "$TOOL" verify --help 2>&1 | grep -q -- "--root-cert" \
        || die "当前 exe-sign-tool 的 verify 不支持 --root-cert——P0.4 代码未实施"
    need_gh

    if [ -n "$EXTRA" ]; then RID="$EXTRA"; else
        RID="$(gh run list --workflow=daily-release.yml --limit 5 \
            --json databaseId,conclusion --jq '[.[]|select(.conclusion=="success")][0].databaseId')" \
            || die "未找到最近成功的 daily-release run，可显式传入: verify <run-id>"
    fi
    DL="$(mktemp -d)"
    msg "下载 run $RID 的产物 → $DL"
    gh run download "$RID" -D "$DL"

    BIN="$(find "$DL" \( -name 'nemesisbot.exe' -o -name 'nemesisbot' \) -type f | head -1)"
    [ -n "$BIN" ] || die "产物里没找到 nemesisbot 二进制"
    DIRNAME="$(dirname "$BIN")"

    local PASS=0 FAIL=0
    check() { # check <名称> <期望状态前缀> <verify参数...>
        local NAME="$1" WANT="$2"; shift 2
        local GOT
        GOT="$("$TOOL" verify "$@" 2>&1 | head -1)" || GOT="(verify 命令失败)"
        if [ "${GOT#${WANT}}" != "$GOT" ]; then
            echo "  [PASS] $NAME → $GOT"; PASS=$((PASS+1))
        else
            echo "  [FAIL] $NAME → 期望 ${WANT}*, 实得: $GOT"; FAIL=$((FAIL+1))
        fi
    }

    msg "正例: 签名产物 + 仓库根证书 → 期望 Valid"
    check "正例 Valid" "Valid" --root-cert "$REPO_ROOT/certs/root_cert.der" --target "$BIN"

    msg "负例1: 篡改一个字节 → 期望 Tampered"
    "$PY" - "$BIN" "$DIRNAME/tampered.exe" <<'PYEOF'
import sys
d = bytearray(open(sys.argv[1], 'rb').read())
d[len(d)//2] ^= 0xFF
open(sys.argv[2], 'wb').write(bytes(d))
PYEOF
    check "负例1 Tampered" "Tampered" --root-cert "$REPO_ROOT/certs/root_cert.der" --target "$DIRNAME/tampered.exe"

    msg "负例2: 换一根（临时新造密钥体系的根）→ 期望 Untrusted"
    WRONG_ANCHOR_DIR="$(mktemp -d)"
    "$TOOL" keygen --out "$WRONG_ANCHOR_DIR/wrong.json" >/dev/null
    "$PY" - "$WRONG_ANCHOR_DIR/wrong.json" "$WRONG_ANCHOR_DIR/wrong_root.der" <<'PYEOF'
import json, binascii, sys
d = json.load(open(sys.argv[1]))
open(sys.argv[2], 'wb').write(binascii.unhexlify(d['root_cert']))
PYEOF
    check "负例2 Untrusted" "Untrusted" --root-cert "$WRONG_ANCHOR_DIR/wrong_root.der" --target "$BIN"

    msg "负例3: 从未签名的文件 → 期望 NoSignature"
    # 用 certs/root_cert.der 自身当材料：永远存在、永远未被签，比在产物里找未签文件更稳
    check "负例3 NoSignature" "NoSignature" --root-cert "$REPO_ROOT/certs/root_cert.der" --target "$REPO_ROOT/certs/root_cert.der"

    rm -rf "$DL" "$WRONG_ANCHOR_DIR"
    echo ""
    if [ "$FAIL" -eq 0 ]; then
        msg "✅ 四例验签全部符合预期（$PASS 通过）"
    else
        die "四例验签有 $FAIL 例不符合预期（$PASS 通过）——排查后再继续"
    fi
}

# ---------------------------------------------------------------------------
stage_clean() {
    [ "$EXTRA" = "--yes" ] || die "clean 会删除 Ceremony 目录全部明文私钥（不可逆）。\
确认 root.offline.json 与 keys.json 已入密码管理器/冷备后，执行: \
bash scripts/init-signing.sh clean --yes"
    [ -d "$CEREMONY_DIR" ] || die "$CEREMONY_DIR 不存在，无可清理"
    echo "将删除: $(ls -A "$CEREMONY_DIR")"
    rm -rf "$CEREMONY_DIR"
    msg "✅ 本机明文私钥已销毁"
}

# ---------------------------------------------------------------------------
case "$STAGE" in
    build)    stage_build ;;
    keygen)   stage_keygen ;;
    secrets)  stage_secrets ;;
    cert)     stage_cert ;;
    dispatch) stage_dispatch ;;
    verify)   stage_verify ;;
    clean)    stage_clean ;;
    all)
        stage_build
        stage_keygen
        stage_secrets
        stage_cert
        stage_dispatch
        stage_verify
        echo ""
        echo "全部完成。最后一步（等你确认 root 已入保险库后）手动执行:"
        echo "  bash scripts/init-signing.sh clean --yes"
        ;;
    ""|help|-h|--help)
        sed -n '2,30p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) die "未知 stage: $STAGE（见 bash scripts/init-signing.sh help）" ;;
esac
