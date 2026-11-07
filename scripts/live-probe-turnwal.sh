#!/usr/bin/env bash
# 活体探针：kill-9 形态 WAL 播种 → 真 gateway 启动 → 验证恢复分流
#   - cluster_rpc（不可逆清单命中）→ unknown 收尾、不续轮（WARN 可见）
#   - read_file（幂等）→ interrupted + 正常续轮
# 用法：bash scripts/live-probe-turnwal.sh   (需先 cargo build --release -p nemesisbot)
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOME_DIR="$ROOT/target/liveprobe-home"
BIN="$ROOT/target/release/nemesisbot.exe"
AI="$ROOT/test-tools/TestAIServer/testaiserver.exe"
WEB_PORT=42333
HEALTH_PORT=42334
AI_PORT=18099
export NEMESISBOT_HOME="$HOME_DIR"
# nemesis-path 语义：NEMESISBOT_HOME 指向父目录，真实 home = <env>/.nemesisbot
REAL_HOME="$HOME_DIR/.nemesisbot"

echo "== 1. onboard（提取模板 + 默认配置）="
"$BIN" onboard default > "$ROOT/target/liveprobe-onboard.log" 2>&1 || echo "onboard exit=$? (继续)"

echo "== 2. 写 bench 形态 config.json ="
python - "$REAL_HOME" "$AI_PORT" "$WEB_PORT" "$HEALTH_PORT" <<'PYEOF'
import json, sys
home, ai_port, web_port, health_port = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
model = lambda alias: {
    "model": f"test/{alias}", "model_name": alias,
    "api_base": f"http://127.0.0.1:{ai_port}/v1", "api_key": "test-key",
    "provider": "test", "enabled": True,
}
config = {
    "version": "1.0", "default_model": "test/testai-1.1",
    "model_list": [model("testai-1.1")],
    "channels": {"web": {"enabled": True, "host": "127.0.0.1", "port": web_port,
                          "auth_token": "probe-token"},
                  "websocket": {"enabled": False}},
    "gateway": {"host": "127.0.0.1", "port": health_port},
    "agents": {"defaults": {"workspace": "", "restrict_to_workspace": True,
                             "llm": "test/testai-1.1", "max_tokens": 8192,
                             "temperature": 0.7, "max_tool_iterations": 20,
                             "concurrent_request_mode": "reject", "queue_size": 8}},
    "security": {"enabled": True},
    "forge": {"enabled": False},
    "logging": {"llm": {"enabled": False}},
    "cluster": {"enabled": False},
}
open(home + "/config.json", "w", encoding="utf-8").write(json.dumps(config, indent=2))
print("config written")
PYEOF

echo "== 3. 播种 kill-9 形态 WAL ="
python - "$REAL_HOME" <<'PYEOF'
import json, time, os, sys
home = sys.argv[1]
now = int(time.time() * 1000)
def rec(call_id, tool, args):
    return {"ts_ms": now, "session_key": "agent:main:main", "channel": "web",
            "chat_id": "probe-chat", "user": "probe-user", "call_id": call_id,
            "tool": tool, "args_preview": args, "state": "started", "ok": None}
d = home + "/workspace/logs/turn_wal"
os.makedirs(d, exist_ok=True)
recs = [rec("probe-irr-1", "cluster_rpc", '{"target":"node-b","task":"x"}'),
        rec("probe-idem-1", "read_file", '{"path":"x.txt"}')]
with open(d + "/wal.jsonl", "w", encoding="utf-8") as f:
    for r in recs:
        f.write(json.dumps(r) + "\n")
print("seeded:", *[r["call_id"] + "/" + r["tool"] for r in recs])
PYEOF

echo "== 4. 起 TestAIServer（:$AI_PORT）+ gateway（:$WEB_PORT）="
"$AI" --port "$AI_PORT" > "$ROOT/target/liveprobe-ai.log" 2>&1 &
AI_PID=$!
sleep 1
"$BIN" gateway > "$ROOT/target/liveprobe-gw.log" 2>&1 &
GW_PID=$!
echo "AI_PID=$AI_PID GW_PID=$GW_PID"

echo "== 5. 等 gateway 就绪（health 轮询，最长 30s）="
for i in $(seq 1 30); do
  if curl -s "http://127.0.0.1:$HEALTH_PORT/health" > /dev/null 2>&1; then
    echo "gateway ready after ${i}s"; break
  fi
  sleep 1
done

echo "== 6. 恢复验证证据 ="
echo "--- gateway 日志：TurnWal/恢复/续轮 相关行 ---"
grep -E "TurnWal|Turn WAL|续轮" "$ROOT/target/liveprobe-gw.log" || echo "(无匹配行)"
echo ""
echo "--- 恢复后 wal.jsonl 内容 ---"
cat "$REAL_HOME/workspace/logs/turn_wal/wal.jsonl"
echo ""
echo "--- 断言 ---"
python - "$REAL_HOME" <<'PYEOF'
import json, sys
home = sys.argv[1]
lines = [json.loads(l) for l in open(home + "/workspace/logs/turn_wal/wal.jsonl", encoding="utf-8") if l.strip()]
by_call = {}
for r in lines:
    by_call.setdefault(r["call_id"], []).append(r)
ok = True
# probe-irr-1：须有 unknown 收尾、无 interrupted
irr_states = [r["state"] for r in by_call.get("probe-irr-1", [])]
if "unknown" in irr_states and "interrupted" not in irr_states:
    print("PASS 不可逆 cluster_rpc → unknown 收尾（不续轮不丢账）:", irr_states)
else:
    print("FAIL cluster_rpc 状态序列异常:", irr_states); ok = False
# probe-idem-1：须有 interrupted 收尾（续轮已发生）
idem_states = [r["state"] for r in by_call.get("probe-idem-1", [])]
if "interrupted" in idem_states:
    print("PASS 幂等 read_file → interrupted + 续轮:", idem_states)
else:
    print("FAIL read_file 未按旧行为续轮:", idem_states); ok = False
sys.exit(0 if ok else 1)
PYEOF
ASSERT_EXIT=$?

echo "== 7. 清理 ="
# git-bash 的 $! 是 MSYS 伪 PID，taskkill 杀不到——按 exe 路径/端口精准杀真实进程。
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='nemesisbot.exe'\" | Where-Object { \$_.ExecutablePath -like '*target\\release*' } | ForEach-Object { Stop-Process -Id \$_.ProcessId -Force }" > /dev/null 2>&1
powershell -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='testaiserver.exe'\" | Where-Object { \$_.CommandLine -like '*--port $AI_PORT*' } | ForEach-Object { Stop-Process -Id \$_.ProcessId -Force }" > /dev/null 2>&1
echo "ASSERT_EXIT=$ASSERT_EXIT（0=两臂全过）"
