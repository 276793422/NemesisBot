#!/usr/bin/env bash
# Gitea 真实链路 e2e 生命周期管理（收官批 A 段，PLAN §3.2）。
#
# 职责：下载钉版 Gitea 单二进制（dl.gitea.com 优先，GitHub releases 兜底）
# → 非交互初始化（app.ini INSTALL_LOCK=true + migrate + admin user create）
# → 启动 web → API 生成 PAT → 建 demo repo + 目标 issue → 跑
#   `cargo test -p nemesisbot gitea_real_chain_e2e`（env 驱动的真实链路集成测试）
# → 清理（kill gitea；GITEA_E2E_KEEP=1 保留现场调试）。
#
# 环境变量：
#   GITEA_E2E_VERSION  钉版（默认 1.27.3）
#   GITEA_E2E_PORT     web 端口（默认 3210）
#   GITEA_E2E_DIR      工作目录（默认 <repo>/target/gitea-e2e）
#   GITEA_E2E_KEEP     =1 保留实例不清理（调试用）
#
# 无网络/端口占用/xz 缺失（Linux）= 诚实报错退出（exit 2），不假红。
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GITEA_VERSION="${GITEA_E2E_VERSION:-1.27.3}"
PORT="${GITEA_E2E_PORT:-3210}"
DIR="${GITEA_E2E_DIR:-$REPO_ROOT/target/gitea-e2e}"
BASE_URL="http://127.0.0.1:$PORT"
ADMIN_PASS="e2e-Root-Pass-2026"
REPO="root/demo"

OS="$(uname -s)"
case "$OS" in
    MINGW*|MSYS*|CYGWIN*)
        PLATFORM=windows
        ASSET="gitea-$GITEA_VERSION-windows-4.0-amd64.exe"
        BIN_NAME="gitea.exe"
        ;;
    Linux)
        PLATFORM=linux
        ASSET="gitea-$GITEA_VERSION-linux-amd64.xz"
        BIN_NAME="gitea"
        ;;
    Darwin)
        PLATFORM=darwin
        ASSET="gitea-$GITEA_VERSION-darwin-10.12-amd64.xz"
        BIN_NAME="gitea"
        ;;
    *)
        echo "SKIP: 不支持的平台 $OS"; exit 2 ;;
esac

log() { echo "[gitea-e2e] $*"; }
fail() { echo "[gitea-e2e] FAIL: $*" >&2; exit 2; }

mkdir -p "$DIR"
cd "$DIR"

# ---- 1. 下载（dl.gitea.com 优先；GitHub releases 兜底；已存在则复用） ----
if [ ! -f "$BIN_NAME" ]; then
    DOWNLOADED=0
    for HOST in "https://dl.gitea.com/gitea/$GITEA_VERSION" \
                "https://github.com/go-gitea/gitea/releases/download/v$GITEA_VERSION"; do
        log "下载 $HOST/$ASSET"
        if curl -fSL --max-time 600 --retry 2 -o "$ASSET" "$HOST/$ASSET" 2>/dev/null; then
            DOWNLOADED=1
            break
        fi
    done
    [ "$DOWNLOADED" = "1" ] || fail "Gitea $GITEA_VERSION 下载失败（无网络？）——真实链路层诚实缺席"

    case "$PLATFORM" in
        windows) cp "$ASSET" "$BIN_NAME" ;;
        *)
            command -v xz >/dev/null 2>&1 || fail "Linux/Darwin 需要 xz 解压——未安装"
            xz -d -k "$ASSET"
            mv "gitea-$GITEA_VERSION-linux-amd64" "$BIN_NAME" 2>/dev/null \
                || mv "gitea-$GITEA_VERSION-darwin-10.12-amd64" "$BIN_NAME"
            chmod +x "$BIN_NAME"
            ;;
    esac
fi
"./$BIN_NAME" --version >/dev/null 2>&1 || fail "二进制不可执行"
log "Gitea $(./"$BIN_NAME" --version 2>/dev/null | head -1)"

# ---- 2. 端口预检（占用 = 环境冲突，诚实退出） ----
if curl -s --max-time 3 "$BASE_URL/api/healthz" >/dev/null 2>&1; then
    fail "端口 $PORT 已有服务（可能是上次残留实例）——清理或改 GITEA_E2E_PORT"
fi

# ---- 3. 非交互初始化（app.ini INSTALL_LOCK=true + migrate + admin user） ----
# 每轮全 fresh（二进制复用，状态清零——重复跑不受上轮 DB/用户残留影响）
rm -rf custom data log
WORK_PATH_ABS="$(pwd -W 2>/dev/null || pwd)"
mkdir -p custom/conf data
cat > custom/conf/app.ini <<EOF
APP_NAME = NemesisBot e2e
RUN_MODE = prod
WORK_PATH = $WORK_PATH_ABS

[server]
PROTOCOL = http
HTTP_ADDR = 127.0.0.1
HTTP_PORT = $PORT
ROOT_URL = $BASE_URL/
DISABLE_SSH = true
OFFLINE_MODE = true

[database]
DB_TYPE = sqlite3
PATH = $WORK_PATH_ABS/data/gitea.db

[security]
INSTALL_LOCK = true
SECRET_KEY = e2e-secret-not-production
INTERNAL_TOKEN = eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.e2e-internal-token-placeholder
PASSWORD_COMPLEXITY = off
MIN_PASSWORD_LENGTH = 8

[service]
DISABLE_REGISTRATION = true
REQUIRE_SIGNIN_VIEW = false

[log]
MODE = file
LEVEL = Info

[mailer]
ENABLED = false
EOF

export GITEA_WORK_DIR="$WORK_PATH_ABS"
"./$BIN_NAME" migrate --config custom/conf/app.ini >/dev/null 2>&1 \
    || fail "gitea migrate 失败（log: $DIR/log）"
"./$BIN_NAME" admin user create --config custom/conf/app.ini \
    --username root --password "$ADMIN_PASS" --email root@example.com --admin \
    >/dev/null 2>&1 || fail "admin user create 失败（log: $DIR/log）"

# ---- 4. 启动 web 并等 healthz ----
GITEA_PID=""
cleanup() {
    if [ -n "$GITEA_PID" ] && [ "${GITEA_E2E_KEEP:-0}" != "1" ]; then
        log "清理：kill gitea (pid $GITEA_PID)"
        case "$PLATFORM" in
            windows) taskkill //F //PID "$GITEA_PID" >/dev/null 2>&1 ;;
            *) kill "$GITEA_PID" >/dev/null 2>&1 ;;
        esac
    elif [ -n "$GITEA_PID" ]; then
        log "GITEA_E2E_KEEP=1：保留实例于 $BASE_URL"
    fi
}
trap cleanup EXIT

if [ "$PLATFORM" = "windows" ]; then
    # 禁 start /B；Start-Process -WindowStyle Hidden + -PassThru 取真实 Windows PID
    GITEA_PID=$(powershell -NoProfile -Command "(Start-Process -FilePath '.\\$BIN_NAME' -ArgumentList 'web','--config','custom/conf/app.ini' -WorkingDirectory '$WORK_PATH_ABS' -WindowStyle Hidden -PassThru).Id")
else
    nohup "./$BIN_NAME" web --config custom/conf/app.ini > web.log 2>&1 &
    GITEA_PID=$!
fi
[ -n "$GITEA_PID" ] || fail "gitea web 启动失败"

HEALTH_OK=0
for _ in $(seq 1 60); do
    if curl -s --max-time 3 "$BASE_URL/api/healthz" | grep -q '"pass"'; then
        HEALTH_OK=1
        break
    fi
    sleep 1
done
[ "$HEALTH_OK" = "1" ] || fail "healthz 60s 未就绪（log: $DIR/log）"
log "healthz OK"

# ---- 5. PAT + demo repo + 目标 issue（均为幂等 upsert 形态） ----
PAT_JSON=$(curl -sf -u "root:$ADMIN_PASS" -X POST "$BASE_URL/api/v1/users/root/tokens" \
    -H 'Content-Type: application/json' \
    -d "{\"name\":\"e2e-$(date +%s)\",\"scopes\":[\"all\"]}") \
    || fail "PAT 生成失败"
PAT=$(printf '%s' "$PAT_JSON" | sed -n 's/.*"sha1":"\([0-9a-f]*\)".*/\1/p')
[ -n "$PAT" ] || fail "PAT 解析失败: $PAT_JSON"

curl -sf -u "root:$ADMIN_PASS" -X POST "$BASE_URL/api/v1/user/repos" \
    -H 'Content-Type: application/json' \
    -d '{"name":"demo","auto_init":true,"private":false}' >/dev/null \
    || { curl -s "$BASE_URL/api/v1/repos/root/demo" -u "root:$ADMIN_PASS" >/dev/null \
         || fail "repo 创建失败"; }

curl -sf -X POST "$BASE_URL/api/v1/repos/$REPO/issues" \
    -H "Authorization: token $PAT" -H 'Content-Type: application/json' \
    -d '{"title":"e2e 验收目标 issue","body":"由收官批 A 段验收关闭"}' >/dev/null \
    || fail "目标 issue 创建失败"
log "PAT + repo($REPO) + issue#1 就绪"

# ---- 6. 跑真实链路集成测试 ----
export GITEA_E2E_URL="$BASE_URL"
export GITEA_E2E_TOKEN="$PAT"
export GITEA_E2E_REPO="$REPO"

cd "$REPO_ROOT"
TEST_EXIT=0
cargo test -p nemesisbot gitea_real_chain_e2e -- --nocapture || TEST_EXIT=$?

if [ "$TEST_EXIT" = "0" ]; then
    log "PASS: Gitea 真实链路 e2e 全绿"
else
    log "FAIL: 集成测试退出码 $TEST_EXIT（实例日志: $DIR/log；GITEA_E2E_KEEP=1 可保留现场）"
fi
exit "$TEST_EXIT"
