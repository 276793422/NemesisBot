#!/usr/bin/env bash
# ============================================================================
# P1 UAT：REL-002 关键配置原子写入 + SEC-001 凭据引导状态机（2026-09-23）
#
# 用法:   bash scripts/uat_p1_rel002_sec001.sh
# 前提:   target/release/nemesisbot.exe 为当前源码的最新 release 构建；
#         node 可用（WSAPI 运行时写用例用 web/node_modules/ws）。
#
# 路径语义：NEMESISBOT_HOME 指父目录，真实 home = $NEMESISBOT_HOME/.nemesisbot
# （nemesis-path paths.rs:214）。脚本里 $home = 父目录（临时根），$nb = 真实 home。
#
# 隔离纪律：
#   - 每用例独立 NEMESISBOT_HOME（系统临时目录），全程零接触默认 ~/.nemesisbot
#   - 全程共用一份独立命名的 exe 副本（nbuat_uat.exe），启动前/收尾统一按
#     镜像名清理，绝不误杀用户自己的 nemesisbot 进程
#   - 每用例独立端口，规避 Windows ghost socket TIME_WAIT
#
# 退出码：失败用例数（0 = 全绿）
# ============================================================================
set -u
cd "$(dirname "$0")/.." || exit 1

REL_BIN="target/release/nemesisbot.exe"
# Windows 原生混合路径（C:/...）：attrib/node/exe 都认，bash 工具也认。
TMPROOT="$(cygpath -m "${LOCALAPPDATA:-C:/Users/$USERNAME/AppData/Local}")/Temp"
PASS=0; FAIL=0; SKIPPED=0
declare -a FAILED_CASES
UAT_TOKEN="276793422"   # 引导值（与 common::BOOTSTRAP_WEB_TOKEN 同源，仅作判定参照）

# ---------------------------------------------------------------------------
# 基础设施
# ---------------------------------------------------------------------------

new_home() {  # $1=case 名 → 输出临时父目录（已创建；真实 home = 其下 .nemesisbot）
    local h="$TMPROOT/nb_uat_$1_$$_$RANDOM"
    mkdir -p "$h" && echo "$h"
}

nbdir() { echo "$1/.nemesisbot"; }

bin_for() {  # $1=case 名（忽略）→ 全程共用一份 exe 副本：
    # 串行执行下同镜像名进程 ≤1，末尾统一 taskkill //IM 清理；
    # 固定路径只让 Defender 扫一次（每用例换新文件名会反复触发全量扫描，
    # 76MB exe 冷启动可拖数十秒）。
    echo "${UAT_BIN:-$TMPROOT/nbuat_uat.exe}"
}

patch_cfg() {  # $1=home(父)  $2=JPointer  $3=JSON值（中间节点缺失自动创建）
    # 去头斜杠：MSYS 会把 "/x/y" 形参当 POSIX 路径转成 "C:/Program Files/Git/x/y"
    # （实测 2026-09-23），补丁全落垃圾键。相对形态不触发转换。
    local ptr="${2#/}"
    node -e '
      const fs=require("fs");
      const [h,p,v]=process.argv.slice(1);
      const f=h+"/.nemesisbot/config.json";
      const c=JSON.parse(fs.readFileSync(f,"utf8"));
      let t=c; const segs=p.split("/");
      for(let i=0;i<segs.length-1;i++){
        if(typeof t[segs[i]]!=="object"||t[segs[i]]===null) t[segs[i]]={};
        t=t[segs[i]];
      }
      t[segs[segs.length-1]]=JSON.parse(v);
      fs.writeFileSync(f, JSON.stringify(c,null,2));
    ' "$1" "$ptr" "$3"
}

read_cfg() {  # $1=home(父)  $2=JPointer → 输出值（解析失败输出空 + 退码1）
    local ptr="${2#/}"
    node -e '
      const fs=require("fs");
      const [h,p]=process.argv.slice(1);
      try{
        const c=JSON.parse(fs.readFileSync(h+"/.nemesisbot/config.json","utf8"));
        const v=p.split("/").reduce((t,k)=>t&&t[k],c);
        if(v===undefined) process.exit(3);
        console.log(typeof v==="object"?JSON.stringify(v):String(v));
      }catch(e){process.exit(1);}
    ' "$1" "$ptr"
}

cfg_ok() { read_cfg "$1" "/channels" >/dev/null 2>&1; }

enable_cluster() {  # $1=home(父) → 让控制面如实绑定 0.0.0.0（web_bind_and_display_hosts
    # 只在 cluster_should_start=true 时保留 0.0.0.0，否则保守翻译成回环；
    # cluster_should_start = 主配置 cluster.enabled && workspace/config/config.cluster.json
    # enabled（gateway.rs:2461-2465；config.cluster.json 真身 <home>/workspace/config/，
    # 新装 onboard 不落此文件，serde 默认 enabled=false）。
    patch_cfg "$1" "/cluster/enabled" 'true'
    mkdir -p "$(nbdir "$1")/workspace/config"
    printf '{\n  "enabled": true\n}\n' > "$(nbdir "$1")/workspace/config/config.cluster.json"
}

no_tmp_left() {  # $1=home(父) → 0=无 .tmp-* 残留（含 .nemesisbot 子树）
    [ -z "$(find "$1" -name '.tmp-*' 2>/dev/null)" ]
}

start_gw() {  # $1=case  $2=home(父)  $3=bin  其余=gateway 额外参数
    # exec 替换 + 重定向挂子 shell 外层：若 gateway 命令在捕获 $( ) 里后台化，
    # 子 shell 会继承捕获管道写端 → $() 永等 EOF 卡死（2026-09-23 实测）。
    local case="$1" home="$2" bin="$3"; shift 3
    ( cd "$home" && NEMESISBOT_HOME="$home" exec "$bin" gateway "$@" ) >"$home/gw.log" 2>&1 &
    echo $!
}

stop_gw() {  # $1=start_gw 返回的 bash job pid（//T 树杀连子进程 gateway.exe）
    local winpid
    winpid=$(ps -W | awk -v p="$1" '$1==p {print $4; exit}')
    if [ -n "$winpid" ]; then taskkill //F //T //PID "$winpid" >/dev/null 2>&1; fi
    wait "$1" 2>/dev/null
}

gw_alive() { kill -0 "$1" 2>/dev/null; }

probe_http() {  # $1=port → 0=HTTP 可达（任何状态码都算活）
    curl -s -o /dev/null --max-time 2 "http://127.0.0.1:$1/" 2>/dev/null
}

# 等待网关「拒启」：进程退出 且 日志含 SEC-001。$1=jobpid $2=log $3=超时秒
wait_refused() {
    local deadline=$((SECONDS+$3))
    while [ $SECONDS -lt $deadline ]; do
        if ! gw_alive "$1"; then
            grep -q "SEC-001" "$2" 2>/dev/null && return 0
            return 1
        fi
        sleep 1
    done
    gw_alive "$1" && stop_gw "$1" >/dev/null 2>&1
    return 2
}

wait_alive() {  # $1=port $2=超时秒
    local deadline=$((SECONDS+$2))
    while [ $SECONDS -lt $deadline ]; do
        probe_http "$1" && return 0
        sleep 1
    done
    return 1
}

verdict() {  # $1=用例号  $2=PASS|FAIL  $3=说明
    if [ "$2" = "PASS" ]; then PASS=$((PASS+1)); printf '  [%s] %s — %s\n' "$1" "PASS" "$3";
    else FAIL=$((FAIL+1)); FAILED_CASES+=("$1"); printf '  [%s] %s — %s\n' "$1" "FAIL" "$3"; fi
}

cleanup_home() { rm -rf "$1" 2>/dev/null; }

onboard_into() {  # $1=home(父) $2=bin（stdin 关死避免交互挂起）
    ( cd "$1" && NEMESISBOT_HOME="$1" "$2" onboard default </dev/null >/dev/null 2>&1 )
}

# ===========================================================================
echo "==== UAT 开始：$(date '+%F %T') ===="
[ -x "$REL_BIN" ] || { echo "FATAL: $REL_BIN 不存在或不可执行"; exit 99; }

# 预清理：上一轮异常退出可能残留 nbuat_* 进程/exe（占用副本导致 cp 失败）
for img in $(tasklist 2>/dev/null | grep nbuat_ | awk '{print $1}' | sort -u); do
    taskkill //F //IM "$img" >/dev/null 2>&1
done
sleep 1
rm -f "$TMPROOT"/nbuat_*.exe 2>/dev/null
UAT_BIN="$TMPROOT/nbuat_uat.exe"
cp "$REL_BIN" "$UAT_BIN" || { echo "FATAL: exe 副本创建失败"; exit 99; }

# ---------------------------------------------------------------------------
echo "---- SEC 组：凭据引导状态机（gateway 启动闸）----"

# SEC1 新装 + 回环 + 引导值 → 正常启动
home=$(new_home sec1); bin=$(bin_for sec1)
if onboard_into "$home" "$bin"; then
    patch_cfg "$home" "/channels/web/port" '49010'
    jpid=$(start_gw sec1 "$home" "$bin")
    if wait_alive 49010 40 && gw_alive "$jpid"; then
        verdict SEC1 PASS "新装回环正常启动"
    else verdict SEC1 FAIL "回环+引导值未启动（日志: $(tail -3 "$home/gw.log" 2>/dev/null | tr '\n' ' ')）"; fi
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC1 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC2 0.0.0.0 + 空 token + 集群开启（真 bind-all）→ 拒启
#      （单机模式产品把 0.0.0.0 保守翻译成回环绑定——无暴露面、闸放行是对的；
#        真实全网卡绑定只发生在集群启动形态，本用例 enable_cluster 走该路径）
home=$(new_home sec2); bin=$(bin_for sec2)
if onboard_into "$home" "$bin"; then
    enable_cluster "$home" \
        && patch_cfg "$home" "/channels/web/host" '"0.0.0.0"' \
        && patch_cfg "$home" "/channels/web/auth_token" '""'
    jpid=$(start_gw sec2 "$home" "$bin")
    wait_refused "$jpid" "$home/gw.log" 40; rc=$?
    if [ "$rc" = 0 ]; then
        msg=$(grep -m1 -A2 "SEC-001" "$home/gw.log" | head -2 | tr '\n' ' ')
        verdict SEC2 PASS "0.0.0.0+空token 拒启（$msg）"
    else verdict SEC2 FAIL "wait_refused rc=$rc"; fi
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC2 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC3 0.0.0.0 + 默认令牌（276793422）+ 集群开启 → 拒启
home=$(new_home sec3); bin=$(bin_for sec3)
if onboard_into "$home" "$bin"; then
    enable_cluster "$home" && patch_cfg "$home" "/channels/web/host" '"0.0.0.0"'
    jpid=$(start_gw sec3 "$home" "$bin")
    [ "$(wait_refused "$jpid" "$home/gw.log" 40; echo $?)" = 0 ] \
        && verdict SEC3 PASS "0.0.0.0+默认令牌 拒启" \
        || verdict SEC3 FAIL "默认令牌未被闸拦截"
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC3 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC4 升级形态：0.0.0.0 + CLI 已设真实令牌 → 正常启动
home=$(new_home sec4); bin=$(bin_for sec4)
if onboard_into "$home" "$bin"; then
    NEMESISBOT_HOME="$home" "$bin" channel web auth-set uat-real-token-4 </dev/null >/dev/null 2>&1
    patch_cfg "$home" "/channels/web/host" '"0.0.0.0"'
    patch_cfg "$home" "/channels/web/port" '49013'
    jpid=$(start_gw sec4 "$home" "$bin")
    if wait_alive 49013 40 && gw_alive "$jpid"; then
        verdict SEC4 PASS "真实令牌 0.0.0.0 正常启动"
    else verdict SEC4 FAIL "真实令牌被误拦（日志: $(tail -3 "$home/gw.log" 2>/dev/null | tr '\n' ' ')）"; fi
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC4 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC5 恢复备份形态：真实令牌被旧备份回退成引导值 + 0.0.0.0（集群开启）→ 拒启
#      （隐式状态机：判定只看 config 原始值，不依赖任何持久化标记）
home=$(new_home sec5); bin=$(bin_for sec5)
if onboard_into "$home" "$bin"; then
    NEMESISBOT_HOME="$home" "$bin" channel web auth-set uat-real-token-5 </dev/null >/dev/null 2>&1
    enable_cluster "$home" && patch_cfg "$home" "/channels/web/host" '"0.0.0.0"'
    patch_cfg "$home" "/channels/web/auth_token" "\"$UAT_TOKEN\""   # 模拟恢复旧备份
    jpid=$(start_gw sec5 "$home" "$bin")
    [ "$(wait_refused "$jpid" "$home/gw.log" 40; echo $?)" = 0 ] \
        && verdict SEC5 PASS "恢复引导值备份后拒启（值判定，无标记失步）" \
        || verdict SEC5 FAIL "引导值备份未被闸拦截"
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC5 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC6 websocket 独立闸：web 回环正常 + ws 0.0.0.0 + ws 引导值 → 拒启，
#      且报错指认 channels.websocket.auth_token（不误指 web）
home=$(new_home sec6); bin=$(bin_for sec6)
if onboard_into "$home" "$bin"; then
    patch_cfg "$home" "/channels/websocket/host" '"0.0.0.0"'
    patch_cfg "$home" "/channels/websocket/auth_token" '""'
    jpid=$(start_gw sec6 "$home" "$bin")
    if [ "$(wait_refused "$jpid" "$home/gw.log" 40; echo $?)" = 0 ] \
        && grep -q "channels\.websocket\.auth_token" "$home/gw.log" \
        && ! grep -q "channels\.web\.auth_token" "$home/gw.log"; then
        verdict SEC6 PASS "websocket 独立闸触发且字段指认正确"
    else verdict SEC6 FAIL "ws 闸未触发或字段指认错误（日志: $(grep -m1 'SEC-001' "$home/gw.log" 2>/dev/null)）"; fi
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC6 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC7 websocket 独立闸 + 真实令牌 → 正常启动
home=$(new_home sec7); bin=$(bin_for sec7)
if onboard_into "$home" "$bin"; then
    patch_cfg "$home" "/channels/websocket/host" '"0.0.0.0"'
    patch_cfg "$home" "/channels/websocket/auth_token" '"uat-ws-token"'
    patch_cfg "$home" "/channels/web/port" '49016'
    jpid=$(start_gw sec7 "$home" "$bin")
    if wait_alive 49016 40 && gw_alive "$jpid"; then
        verdict SEC7 PASS "ws 真实令牌 0.0.0.0 正常启动"
    else verdict SEC7 FAIL "ws 真实令牌被误拦"; fi
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC7 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC8 --relay 纯中继豁免：控制面闸无对象 → 不拒启
home=$(new_home sec8); bin=$(bin_for sec8)
if onboard_into "$home" "$bin"; then
    patch_cfg "$home" "/channels/web/host" '"0.0.0.0"'
    patch_cfg "$home" "/bridge/server/token" '"uat-bridge-token"'
    jpid=$(start_gw sec8 "$home" "$bin" --relay)
    if gw_alive "$jpid" && ! grep -q "SEC-001" "$home/gw.log" 2>/dev/null; then
        verdict SEC8 PASS "--relay 不经过控制面闸（结构性豁免）"
    else verdict SEC8 FAIL "relay 被闸误拦或未启动（日志: $(tail -3 "$home/gw.log" 2>/dev/null | tr '\n' ' ')）"; fi
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC8 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC9 vault:/env: 引用 = 已初始化凭据 → 0.0.0.0 放行
home=$(new_home sec9); bin=$(bin_for sec9)
if onboard_into "$home" "$bin"; then
    patch_cfg "$home" "/channels/web/host" '"0.0.0.0"'
    patch_cfg "$home" "/channels/web/port" '49019'
    patch_cfg "$home" "/channels/web/auth_token" '"env:NB_UAT_SEC9_TOKEN"'
    jpid=$(start_gw sec9 "$home" "$bin")
    if wait_alive 49019 40 && gw_alive "$jpid"; then
        verdict SEC9 PASS "env: 引用视为已初始化，放行"
    else verdict SEC9 FAIL "env: 引用被误拦"; fi
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC9 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC10 重复 onboard：已有真实令牌时 re-onboard（选 N 保留）→ 令牌不被重置回引导值
home=$(new_home sec10); bin=$(bin_for sec10)
if onboard_into "$home" "$bin"; then
    NEMESISBOT_HOME="$home" "$bin" channel web auth-set uat-real-token-10 </dev/null >/dev/null 2>&1
    printf 'n\n' | ( cd "$home" && NEMESISBOT_HOME="$home" "$bin" onboard default >/dev/null 2>&1 )
    got=$(read_cfg "$home" "/channels/web/auth_token")
    if [ "$got" = "uat-real-token-10" ]; then
        verdict SEC10 PASS "重复 onboard 保留用户令牌（不回退引导值）"
    else verdict SEC10 FAIL "re-onboard 后令牌=$got"; fi
else verdict SEC10 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC11 显式 LAN IP + 引导值 → 拒启（闸对具体 IP 同样生效）
home=$(new_home sec11); bin=$(bin_for sec11)
if onboard_into "$home" "$bin"; then
    patch_cfg "$home" "/channels/web/host" '"192.0.2.55"'
    jpid=$(start_gw sec11 "$home" "$bin")
    [ "$(wait_refused "$jpid" "$home/gw.log" 40; echo $?)" = 0 ] \
        && verdict SEC11 PASS "显式 LAN IP + 引导值 拒启" \
        || verdict SEC11 FAIL "LAN IP 未被闸拦截"
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC11 FAIL "onboard 失败"; fi
cleanup_home "$home"

# SEC12 单机保守翻译语义（G9）：0.0.0.0 + 引导值 + 集群关闭 → 正常启动，
#      且 netstat 实证 web 端口只绑 127.0.0.1（web_bind_and_display_hosts
#      翻译，无局域网暴露面——放行是安全决策而非漏闸）
home=$(new_home sec12); bin=$(bin_for sec12)
if onboard_into "$home" "$bin"; then
    patch_cfg "$home" "/channels/web/host" '"0.0.0.0"'
    patch_cfg "$home" "/channels/web/port" '49028'
    jpid=$(start_gw sec12 "$home" "$bin")
    if wait_alive 49028 40 && gw_alive "$jpid"; then
        sleep 1
        lo=$(netstat -ano | grep "LISTENING" | grep -c "127\.0\.0\.1:49028" || true)
        any=$(netstat -ano | grep "LISTENING" | grep -c "0\.0\.0\.0:49028" || true)
        if [ "$lo" -ge 1 ] && [ "$any" = "0" ]; then
            verdict SEC12 PASS "单机 0.0.0.0 翻译为回环绑定（netstat: 仅 127.0.0.1，无 0.0.0.0 监听）"
        else verdict SEC12 FAIL "绑定面异常（127.0.0.1:$lo 0.0.0.0:$any）"; fi
    else verdict SEC12 FAIL "单机 0.0.0.0 未启动（日志: $(tail -3 "$home/gw.log" 2>/dev/null | tr '\n' ' ')）"; fi
    stop_gw "$jpid" >/dev/null 2>&1
else verdict SEC12 FAIL "onboard 失败"; fi
cleanup_home "$home"

# ---------------------------------------------------------------------------
echo "---- REL 组：关键配置原子写入（CLI / 运行时 / 失败注入 / 并发）----"

# REL1 新装落盘：config.json 可解析 + 无 .tmp-* 残留
home=$(new_home rel1); bin=$(bin_for rel1)
if onboard_into "$home" "$bin"; then
    if cfg_ok "$home" && no_tmp_left "$home"; then
        verdict REL1 PASS "onboard 落盘可解析且无临时残留"
    else verdict REL1 FAIL "config 不可解析或有 tmp 残留"; fi
else verdict REL1 FAIL "onboard 失败"; fi

# REL2 model add（原子 chokepoint）→ 可解析 + 条目在 + 无残留
# （CLI 会剥厂商前缀：--model test/testai-1.1 → model_name="testai-1.1"）
if NEMESISBOT_HOME="$home" "$bin" model add --model test/testai-1.1 \
        --base http://127.0.0.1:8080/v1 --key uat-key-2 </dev/null >/dev/null 2>&1; then
    if cfg_ok "$home" && [ "$(read_cfg "$home" "/model_list/0/model_name")" = "testai-1.1" ] \
        && no_tmp_left "$home"; then
        verdict REL2 PASS "model add 原子落盘"
    else verdict REL2 FAIL "add 后 config 异常或有残留"; fi
else verdict REL2 FAIL "model add 退码非 0"; fi

# REL3 channel web auth-set → 值生效 + 无残留
if NEMESISBOT_HOME="$home" "$bin" channel web auth-set uat-token-3 </dev/null >/dev/null 2>&1; then
    if [ "$(read_cfg "$home" "/channels/web/auth_token")" = "uat-token-3" ] \
        && cfg_ok "$home" && no_tmp_left "$home"; then
        verdict REL3 PASS "auth-set 原子落盘"
    else verdict REL3 FAIL "token 未生效或 config 异常"; fi
else verdict REL3 FAIL "auth-set 退码非 0"; fi

# REL4 model set-tier / set-size（另一族写入口）→ 可解析 + 字段生效
NEMESISBOT_HOME="$home" "$bin" model set-tier test/testai-1.1 mini </dev/null >/dev/null 2>&1
rc1=$?
NEMESISBOT_HOME="$home" "$bin" model set-size test/testai-1.1 30B </dev/null >/dev/null 2>&1
rc2=$?
if [ "$rc1" = 0 ] && [ "$rc2" = 0 ] && cfg_ok "$home" && no_tmp_left "$home"; then
    verdict REL4 PASS "set-tier/set-size 原子落盘"
else verdict REL4 FAIL "rc=($rc1,$rc2) 或 config 异常"; fi

# REL5 失败注入：config.json 只读属性 → 写失败诚实报错 + 旧值完好 + 无残留；恢复属性后写成功
cfg_win=$(cygpath -w "$(nbdir "$home")/config.json")
attrib +R "$cfg_win" >/dev/null
err_out=$(NEMESISBOT_HOME="$home" "$bin" channel web auth-set uat-should-fail </dev/null 2>&1)
rc_inject=$?
inject_ok=0
if [ "$rc_inject" != 0 ] && echo "$err_out" | grep -q "atomic write" \
    && [ "$(read_cfg "$home" "/channels/web/auth_token")" = "uat-token-3" ] \
    && cfg_ok "$home" && no_tmp_left "$home"; then
    inject_ok=1
fi
attrib -R "$cfg_win" >/dev/null
if [ "$inject_ok" = 1 ]; then
    if NEMESISBOT_HOME="$home" "$bin" channel web auth-set uat-token-5-fixed </dev/null >/dev/null 2>&1 \
        && [ "$(read_cfg "$home" "/channels/web/auth_token")" = "uat-token-5-fixed" ]; then
        verdict REL5 PASS "注入失败诚实报错+旧值完好，恢复后写入成功"
    else verdict REL5 FAIL "恢复属性后写入仍失败"; fi
else verdict REL5 FAIL "注入未诚实失败（rc=$rc_inject err=$(echo "$err_out" | tail -1)）"; fi

# REL6 并发写：两个 CLI 写进程并行 → 双双成功 + 最终可解析 + 无 tmp 互踩残留
NEMESISBOT_HOME="$home" "$bin" model add --model test/testai-2.0 \
    --base http://127.0.0.1:8080/v1 --key uat-key-6 </dev/null >/dev/null 2>&1
NEMESISBOT_HOME="$home" "$bin" model set-size test/testai-1.1 7B </dev/null >/dev/null 2>&1 &
p1=$!
NEMESISBOT_HOME="$home" "$bin" model set-size test/testai-2.0 9B </dev/null >/dev/null 2>&1 &
p2=$!
wait $p1; rcA=$?
wait $p2; rcB=$?
if [ "$rcA" = 0 ] && [ "$rcB" = 0 ] && cfg_ok "$home" && no_tmp_left "$home"; then
    s1=$(read_cfg "$home" "/model_list/0/model_size_b")
    s2=$(read_cfg "$home" "/model_list/1/model_size_b")
    verdict REL6 PASS "并发双写均成功，最终 config 可解析（size 实况: [$s1, $s2]；跨进程 last-writer-wins 为已文档化语义）"
else verdict REL6 FAIL "并发写 rc=($rcA,$rcB) 或 config 损坏/有残留"; fi

# REL7 运行时写（WSAPI models.add → dashboard 同路径）→ 响应成功 + config 落盘生效 + 无残留
patch_cfg "$home" "/channels/web/host" '"127.0.0.1"'
patch_cfg "$home" "/channels/web/port" '49027'
jpid=$(start_gw rel7 "$home" "$bin")
if wait_alive 49027 25; then
    WS_PORT=49027 WS_TOKEN=uat-token-5-fixed \
    node scripts/uat_p1_ws_models_add.mjs "$(cygpath -m "$(nbdir "$home")")" 2>&1 | tail -1 > "$home/ws.out"
    ws_rc=${PIPESTATUS[0]}
    if [ "$ws_rc" = 0 ] && read_cfg "$home" "/model_list" | grep -q '"model_name":"test/uat-ws-model"' \
        && cfg_ok "$home" && no_tmp_left "$home"; then
        verdict REL7 PASS "运行时 WSAPI 写入原子落盘（$(cat "$home/ws.out")）"
    else verdict REL7 FAIL "WS 写后 config 异常（$(cat "$home/ws.out" 2>/dev/null)）"; fi
else
    verdict REL7 FAIL "网关 49027 未起（$(tail -2 "$home/gw.log" 2>/dev/null | tr '\n' ' ')）"
    SKIPPED=$((SKIPPED+1))
fi
stop_gw "$jpid" >/dev/null 2>&1
cleanup_home "$home"

# ---------------------------------------------------------------------------
echo "---- 收尾核查 ----"
stray=$(tasklist 2>/dev/null | grep -c 'nbuat_' || true)
if [ "$stray" = "0" ]; then echo "  无残留 nbuat_* 进程"; else
    echo "  WARN: 残留 nbuat_* 进程 $stray 个，强制清理"
    for img in $(tasklist 2>/dev/null | grep nbuat_ | awk '{print $1}' | sort -u); do
        taskkill //F //IM "$img" >/dev/null 2>&1
    done
fi
rm -f "$TMPROOT"/nbuat_*.exe 2>/dev/null
leftovers=$(find "$TMPROOT" -maxdepth 1 -name 'nb_uat_*' 2>/dev/null | wc -l)
echo "  遗留临时 home: $leftovers 个"

echo "==== UAT 结束：PASS=$PASS FAIL=$FAIL ===="
[ "$FAIL" -gt 0 ] && { printf '  失败用例: %s\n' "${FAILED_CASES[*]}"; exit "$FAIL"; }
exit 0
