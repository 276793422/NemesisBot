//! R9 补测批 — gateway 活动场景组（live scenarios）。
//!
//! 目标区间（gateway.rs 高价值 miss 区段，全部要求长时间运行的 gateway 子进程）：
//!
//! | # | 区间                              | 覆盖手法 |
//! |---|-----------------------------------|----------|
//! | 1 | cron on_job 双分支 (1348-1397)     | 预种子 jobs.json（过期 "at" 任务）→ gateway arm 后秒级触发；分支 A 发 InboundMessage（经 LLM 落 request_log），分支 B 空消息直接 Ok("No message to deliver")（落 store 历史）|
//! | 2 | cluster 双节点 peer_chat 全链      | A/B 两 gateway 子进程（独立 temp home、完全独立端口组），共享 AEAD token，互为静态 peers；A 由预种子 cron 驱动（无 WS）发 cluster_rpc → B 的 MockAi 回 canned 文本 → callback 三路由的 Route2(bus cluster_continuation)+Route3(TaskManager complete) 完成 A 侧续行 |
//! | 3 | cluster_rpc 工具选型 (2099-2126)   | 同上：A 模型脚本第一条即 ToolCall(cluster_rpc)，peers_fn 注入 + call_fn 异步 ack 全链路真跑（本测试与 #2 合并在同一场景）|
//! | 4 | heartbeat 多臂 (3146-3252)         | 实例 a：BOOTSTRAP.md 在 → 零 LLM 命中；实例 b：HEARTBEAT.md 有任务行 + interval=1min → 第一拍(+1s)喂 passthrough 文本、第二拍(+60s)喂 "  HEARTBEAT_OK\n"（trim 归一命中）|
//! | 5 | workflow message/event 触发驱动    | definitions/ 放两条 YAML：wf-r9-msg(message 触发, transform 节点零依赖) + wf-r9-event(event 触发, 匹配 workflow.completed 且 workflow_name==wf-r9-msg 防自递归)；由同一条 cron InboundMessage 广播级联点亮 2908-3078 两个订阅任务；断言 workspace/workflow/executions/*.jsonl |
//! | 6 | approval ask 规则链 (196-274)      | config.security.json file_rules.write=[{pattern:"*",action:"ask"}] + MockAi 首条 write_file 工具调用 → dll 缺席早拒臂 Ok(false)（弹窗永不出现）；工具结果回灌后第二轮出终文本 |
//! | 7 | open_dashboard / shutdown 内部命令 | POST /api/internal {"cmd":"open_dashboard"}（dll 缺席守卫下无窗口风险）拿 {"status":"ok"} ack；随后每实例的优雅停机本身走 Shutdown 臂 (3601-3608) |
//!
//! 无 WS 说明：nemesisbot dev-deps 只有 tempfile + test-harness（tokio-tungstenite
//! 只在 test-harness 内部传递可用，测试体无法 `use`）。因此所有入站驱动一律用
//! 「预种子 cron 过期任务」替代 WS chat.send——cron arm 后 on_job 闭包向总线发布
//! InboundMessage，与 WS 入站在 loop.rs/工作流订阅者处汇合点相同。HTTP 断言走
//! test_harness 公开 API（http_client/graceful_shutdown_gateway 返回值方法调用
//! 不需要命名 reqwest 类型）。
//!
//! 端口纪律：web/health 绑 127.0.0.1:0（OS 分配）；集群 UDP/RPC 探针取空闲并显式
//! 避开禁占端口（8080/18790-18793/49000/49001）；rpc_port 严格遵守「UDP+10000」
//! 静态 peer 反推约定。
//!
//! 诚实边界（放弃项见各测试注释 + 交付报告）：
//! - Route1（嵌套 ClusterAgent 任务注入，1986-1996）：需在 B 侧先挂起一个
//!   ClusterAgent 子任务再让回调命中 find_by_child_task_id —— 夹具成本远超本批，
//!   放弃（Route2/Route3 已覆盖回调分发主路径）。
//! - heartbeat Empty 臂（Ok(response) if response.is_empty() → None，3218）：
//!   interval 下限 60s（分钟粒度 i64），三拍=180s 超出「最短周期×2+30s」预算，
//!   本批只实跑两拍（passthrough + HEARTBEAT_OK 归一），Empty 臂留待后续。
//! - approval 超时臂（272-275）：需要真实弹出 popup 进程且等满 timeout+15s，
//!   违反无窗口纪律；只在 dll 缺席时测早拒臂，超时臂如实上报未测。
//! - heartbeat Err 臂（3233-3239）：`AgentLoop::process_heartbeat`（loop.rs
//!   1929-2006）全部返回路径都是 Ok（Done 提取失败也返回 Ok(fallback 字符串)），
//!   经网关 handler 无法构造 Err —— 判定为该 caller 形态下的不可达 seam，按纪律
//!   上报而非改生产代码。

use std::net::{TcpListener, UdpSocket};
use std::path::Path;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use test_harness::mock_ai::{MockAiReply, MockAiServer};
use test_harness::{
    graceful_shutdown_gateway, http_client, resolve_nemesisbot_bin, ManagedProcess, TestWorkspace,
};

// ---------------------------------------------------------------------------
// 常量与小组件
// ---------------------------------------------------------------------------

/// 子进程 gateway 启动预算（web bind 就绪轮询上限，对齐 tests.rs 全装配冒烟）。
const BOOT_TIMEOUT_SECS: u64 = 120;

/// 优雅停机后等待子进程自行退出预算（profraw flush 需要 exit 链走完）。
const EXIT_TIMEOUT_SECS: u64 = 40;

/// 本项目生产端口禁区（绝不占用）。
const FORBIDDEN_PORTS: &[u16] = &[8080, 18790, 18791, 18792, 18793, 49000, 49001];

/// 本进程内已认领的集群 UDP 基端口（防同进程多实例探到同一对端口后竞速绑定）。
static CLAIMED_CLUSTER_PORTS: StdMutex<Vec<u16>> = StdMutex::new(Vec::new());

/// 毫秒时间戳（种子 cron 用）。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis() as i64
}

/// 测试内唯一标记（跨并行测试防串扰）。
fn unique_tag(prefix: &str) -> String {
    format!(
        "{}_p{}_{:x}",
        prefix,
        std::process::id(),
        now_ms() as u64 & 0xFFFF_FFFF
    )
}

/// TCP 端口此刻是否可绑（探测即释）。
fn tcp_bindable(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// 取一对满足「rpc = udp + 10000」约定的空闲端口（静态 peer 反推协议要求；
/// gateway.rs 1746 直接 `udp + 10000`，因此 udp 必须 ≤ 55535 防 u16 回绕）。
///
/// 不能用 `:0` 探测：Windows ephemeral 端口范围是 49152-65535，几乎全部
/// > 55535，`:0` 会把 256 次尝试全部烧光。改为在 20000-45000 范围内以
/// > 进程 id + 毫秒时钟为起点确定性步进探测。
fn probe_cluster_port_pair() -> (u16, u16) {
    let mut claimed = CLAIMED_CLUSTER_PORTS.lock().unwrap();
    // 起点混合 pid 与时间：同进程内多个测试串行调用时步进序列错开，
    // 时间项保证跨二进制运行不固定撞同一个起点。
    let start = 20000u32 + ((std::process::id() as u32).wrapping_mul(2654435761)
        ^ (now_ms() as u32))
        % 25000;
    for i in 0..25000 {
        let udp = (start + i) % 25000 + 20000; // 始终落在 [20000, 45000)
        let udp = udp as u16;
        let rpc = udp + 10000;
        if FORBIDDEN_PORTS.contains(&udp) || FORBIDDEN_PORTS.contains(&rpc) {
            continue;
        }
        if claimed.contains(&udp) || claimed.contains(&rpc) {
            continue;
        }
        // UDP 与 TCP 双向验证（探测即释——存在与其他进程的微小竞争窗口，
        // gateway 子进程随后真绑失败会走端口冲突降级路径，测试断言不依赖
        // 端口本身，只依赖节点互通，可接受）。
        if UdpSocket::bind(("127.0.0.1", udp)).is_err() {
            continue;
        }
        if !tcp_bindable(rpc) {
            continue;
        }
        claimed.push(udp);
        claimed.push(rpc);
        return (udp, rpc);
    }
    panic!("probe_cluster_port_pair: cannot find a free pair");
}

/// 子进程 exe 旁 plugins/plugin_ui.* 是否存在（镜像 gateway.rs:56
/// plugin_ui_library_exists 对子进程 current_exe 的判定——两者都在同一个 exe 目录下解析）。
fn plugin_ui_dll_next_to_bin(bin: &Path) -> bool {
    let Some(exe_dir) = bin.parent() else {
        return false;
    };
    let plugins = exe_dir.join("plugins");
    for name in [
        "plugin_ui.dll",
        "plugin-ui.dll",
        "libplugin_ui.so",
        "libplugin_ui.dylib",
    ] {
        if plugins.join(name).exists() {
            return true;
        }
    }
    false
}

/// 基于 CONFIG_DEFAULT 构造一份活动网关配置（模板里 web 默认 0.0.0.0:8080 /
/// gateway 18790，必须全改写；heartbeat 默认 30 分钟开着会烧 mock 脚本，默认关）。
fn live_gateway_config(
    workspace_abs: &Path,
    mock_base_url: &str,
    alias: &str,
    tier: &str,
    auth_token: &str,
    heartbeat_on: bool,
    cluster_master_on: bool,
    security_on: bool,
) -> Value {
    let mut cfg: Value =
        serde_json::from_str(crate::CONFIG_DEFAULT).expect("parse CONFIG_DEFAULT");
    cfg["channels"]["web"]["host"] = json!("127.0.0.1");
    cfg["channels"]["web"]["port"] = json!(0);
    cfg["channels"]["web"]["auth_token"] = json!(auth_token);
    // websocket 通道默认关，双保险显式关（避免任何隐式启用占口）。
    cfg["channels"]["websocket"]["enabled"] = json!(false);
    cfg["gateway"]["host"] = json!("127.0.0.1");
    cfg["gateway"]["port"] = json!(0);
    cfg["agents"]["defaults"]["llm"] = json!(alias);
    cfg["agents"]["defaults"]["workspace"] =
        json!(workspace_abs.to_string_lossy().to_string());
    // 模型必须带 provider 前缀（裸名会被解析成 openai → Codex /responses）。
    cfg["model_list"] = json!([{
        "model_name": alias,
        "model": format!("r9prov/{alias}"),
        "api_key": "r9-key",
        "api_base": format!("{mock_base_url}/v1"),
        "model_tier": tier,
    }]);
    cfg["heartbeat"]["enabled"] = json!(heartbeat_on);
    cfg["heartbeat"]["interval"] = json!(1); // 分钟；仅 heartbeat_on 时生效（下限 60s）
    cfg["security"]["enabled"] = json!(security_on);
    cfg["cluster"]["enabled"] = json!(cluster_master_on);
    // LLM 请求/响应信封落盘（断言锚点主干）：full + save_raw。
    cfg["logging"]["llm"]["enabled"] = json!(true);
    cfg["logging"]["llm"]["detail_level"] = json!("full");
    cfg["logging"]["llm"]["save_raw"] = json!(true);
    cfg["logging"]["llm"]["log_dir"] = json!("logs/request_logs");
    cfg
}

/// 写 config.json + 最小 workspace 种子（skills/forge 空对象避免告警分支噪声）。
fn install_home_config(home: &Path, cfg: &Value) {
    std::fs::create_dir_all(home.join("workspace").join("config")).expect("mkdir ws/config");
    std::fs::write(home.join("config.json"), cfg.to_string()).expect("write config.json");
    std::fs::create_dir_all(home.join("config")).expect("mkdir home/config");
    std::fs::write(
        home.join("workspace").join("config").join("config.skills.json"),
        "{}",
    )
    .expect("seed skills cfg");
    std::fs::write(
        home.join("workspace").join("config").join("config.forge.json"),
        "{}",
    )
    .expect("seed forge cfg");
}

/// 组装一条预种子 cron 任务（复刻 nemesis-cron CronStoreData/CronJob 序列化形态；
/// kind="at" + 过期 at_ms + delete_after_run=false：arm 后单次触发 → enabled=false 收尾）。
fn seeded_at_job(id: &str, name: &str, message: &str, due_ms: i64, channel: Option<&str>) -> Value {
    json!({
        "id": id,
        "name": name,
        "enabled": true,
        "schedule": {
            "kind": "at",
            "at_ms": due_ms,
            "every_ms": null,
            "expr": null,
            "tz": null,
        },
        "payload": {
            "kind": "agent_turn",
            "message": message,
            "command": null,
            "deliver": true,
            "channel": channel,
            "to": null,
            "session_key": null,
            "max_rounds": null,
        },
        "state": {
            "next_run_at_ms": due_ms,
            "last_run_at_ms": null,
            "last_status": null,
            "last_error": null,
            "history": [],
        },
        "created_at_ms": due_ms - 1000,
        "updated_at_ms": due_ms - 1000,
        "delete_after_run": false,
    })
}

/// 把种子任务写进 cron store（CronService::new 会在启动时加载）。
fn seed_cron_store(home: &Path, jobs: Vec<Value>) {
    let store = json!({"version": 1, "jobs": jobs});
    let dir = home.join("workspace").join("cron");
    std::fs::create_dir_all(&dir).expect("mkdir cron dir");
    std::fs::write(
        dir.join("jobs.json"),
        serde_json::to_string_pretty(&store).expect("ser cron store"),
    )
    .expect("write cron store");
}

/// 读回 cron store（触发完成状态断言锚点）。
fn read_cron_store(home: &Path) -> Option<Value> {
    let txt = std::fs::read_to_string(home.join("workspace").join("cron").join("jobs.json")).ok()?;
    serde_json::from_str(&txt).ok()
}

/// 启动一个 `--local gateway` 子进程（coverage env 由 ManagedProcess 注入）。
fn spawn_gateway(name: &'static str, bin: &Path, ws: &TestWorkspace) -> ManagedProcess {
    ManagedProcess::spawn(name, bin, &["--local", "gateway"], ws.path())
        .expect("spawn gateway child")
}

/// 轮询 {home}/workspace/state/gateway.json 直到 web_port != 0（真实 bind 后写入）。
async fn wait_for_web_port(home: &Path) -> u16 {
    let state = home.join("workspace").join("state").join("gateway.json");
    let deadline = Instant::now() + Duration::from_secs(BOOT_TIMEOUT_SECS);
    loop {
        if let Ok(txt) = std::fs::read_to_string(&state)
            && let Ok(v) = serde_json::from_str::<Value>(&txt)
                && let Some(p) = v.get("web_port").and_then(|x| x.as_u64())
                    && p > 0 && p <= u16::MAX as u64 {
                        return p as u16;
                    }
        assert!(
            Instant::now() < deadline,
            "gateway did not bind web within {}s (state={:?})",
            BOOT_TIMEOUT_SECS,
            std::fs::read_to_string(&state).ok(),
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 每 250ms 轮询一次同步条件直到成立或超时（断言锚点通用等待器）。
async fn wait_until(timeout_secs: u64, what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if cond() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "wait_until({what}): condition not met within {timeout_secs}s",
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// wait_until 的带诊断变体：超时 panic 时**现场**收集证据（此刻 temp home
/// 尚在 Drop 清理之前），把「多进程编排卡在哪一跳」直接写进失败输出，
/// 替代事后盲猜（2026-08-27 workspace 两次 live 编排超时无证据可用之教训）。
async fn wait_until_diag(
    timeout_secs: u64,
    what: &str,
    mut cond: impl FnMut() -> bool,
    diag: impl FnOnce() -> String,
) {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if cond() {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "wait_until({what}): condition not met within {timeout_secs}s\n==== DIAG ====\n{}",
                diag()
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 单文件尾部诊断（lossy 读最后 max_chars 字符）。
fn diag_file_tail(path: &Path, max_chars: usize) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let start = text.len().saturating_sub(max_chars);
            // 避开多字节字符边界截断 panic。
            let start = (0..=start)
                .rev()
                .find(|i| text.is_char_boundary(*i))
                .unwrap_or(0);
            format!("---- tail {} ----\n{}", path.display(), &text[start..])
        }
        Err(e) => format!("---- tail {} ---- <unreadable: {e}>", path.display()),
    }
}

/// 收集一个 temp home 的诊断快照：logs 目录清单 + 最新修改文件的尾部 +
/// cluster/state.toml 全文 + rpc_cache 清单。全部容错（缺失=占位行）。
fn diag_home(home: &Path) -> String {
    let ws = home.join("workspace");
    let logs = ws.join("logs");
    let mut out = format!("[home] {}\n", home.display());

    match std::fs::read_dir(&logs) {
        Ok(entries) => {
            let names: Vec<String> = entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            out.push_str(&format!("[logs entries] {names:?}\n"));
            let mut files: Vec<(SystemTime, std::path::PathBuf)> = std::fs::read_dir(&logs)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    p.is_file().then_some(())?;
                    let m = e.metadata().ok()?;
                    let t = m.modified().ok()?;
                    Some((t, p))
                })
                .collect();
            files.sort();
            if let Some((_, newest)) = files.last() {
                out.push_str(&diag_file_tail(newest, 1800));
                out.push('\n');
            }
        }
        Err(e) => out.push_str(&format!("[logs entries] <unreadable: {e}>\n")),
    }

    let state = ws.join("cluster").join("state.toml");
    out.push_str(&match std::fs::metadata(&state) {
        Ok(_) => diag_file_tail(&state, 800),
        Err(_) => "[cluster/state.toml] <absent>".to_string(),
    });
    out.push('\n');

    let cache = ws.join("cluster").join("rpc_cache");
    match std::fs::read_dir(&cache) {
        Ok(entries) => {
            let names: Vec<String> = entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            out.push_str(&format!("[rpc_cache] {names:?}\n"));
        }
        Err(_) => out.push_str("[rpc_cache] <absent>\n"),
    }
    out
}

/// 递归扫描目录下任意文本文件是否包含 needle（二进制/不可读文件跳过）。
fn tree_contains(dir: &Path, needle: &str) -> bool {    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if tree_contains(&path, needle) {
                return true;
            }
            continue;
        }
        // 信封/jsonl/log 都是 UTF-8 文本； lossy 读保证不因个别字节炸掉。
        if let Ok(bytes) = std::fs::read(&path) {
            let text = String::from_utf8_lossy(&bytes);
            if text.contains(needle) {
                return true;
            }
        }
    }
    false
}

/// 优雅停机：POST /api/internal shutdown（同时就是内部命令 Shutdown 臂的真跑）
/// + wait_for_exit 让 profraw 走 atexit 落盘；超时则兜底 kill 并如实说明。
async fn graceful_teardown(mut gw: ManagedProcess, web_port: u16, token: &str, label: &str) {
    match graceful_shutdown_gateway(web_port, token).await {
        Ok(()) => {
            if let Err(e) = gw.wait_for_exit(Duration::from_secs(EXIT_TIMEOUT_SECS)).await {
                eprintln!("[r9-live] {label}: graceful exit timeout ({e}); killing (profraw lost)");
                gw.kill().await;
            }
        }
        Err(e) => {
            eprintln!("[r9-live] {label}: graceful shutdown POST failed ({e}); killing");
            gw.kill().await;
        }
    }
}

// ===========================================================================
// #1 cron on_job 双分支（1348-1397）
// ===========================================================================

/// 预种子两个过期 "at" 任务：
/// - A：有 message + channel="web" → on_job 发布 InboundMessage（agent 真跑一轮，
///   request_logs 应同时出现任务消息原文和 mock 回复原文）；
/// - B：空 message → on_job 直接 Ok("No message to deliver")，不发总线
///   （mock 命中数不应因 B 增加；store 里仍记 "ok" + history 一条）。
///
/// 断言锚点：workspace/cron/jobs.json（last_status/history/enabled=false 收尾态）
/// + workspace/logs/request_logs 树内标记扫描 + MockAi 命中计数。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r9_live_cron_on_job_both_branches_fire_and_mark_store() {
    let ws = TestWorkspace::new().expect("temp workspace");
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");
    let home = ws.home();
    let token = "r9cron-token";

    let tag = unique_tag("cron");
    let reply = format!("R9-CRON-REPLY {tag}");
    let mock = MockAiServer::start(vec![MockAiReply::Text(reply.clone())]).expect("mock ai");

    let msg_a = format!("R9-CRON-MESSAGE-A {tag}");
    let due = now_ms() - 4000;
    let cfg = live_gateway_config(
        &home.join("workspace"),
        &mock.base_url(),
        "r9-cron-model",
        "mini",
        token,
        false, // heartbeat 关：mock 脚本只能被 cron 入站消费
        false, // cluster 关
        false, // security 关：减少无关拦截面
    );
    install_home_config(&home, &cfg);
    seed_cron_store(
        &home,
        vec![
            seeded_at_job("r9cronjja", "job-with-message", &msg_a, due, Some("web")),
            seeded_at_job("r9cronjjb", "job-empty-message", "", due, None),
        ],
    );

    let gw = spawn_gateway("r9_cron_gw", &bin, &ws);
    let web_port = wait_for_web_port(&home).await;
    // arm() 在 web bind 之后（3336-3350），给足触发余量。
    tokio::time::sleep(Duration::from_secs(5)).await;

    // 两个 job 都被 on_job 执行并标记（1 秒 tick，窗口放宽到 90s）。
    wait_until(90, "both cron jobs marked executed", || {
        let Some(store) = read_cron_store(&home) else {
            return false;
        };
        let Some(jobs) = store.get("jobs").and_then(|v| v.as_array()) else {
            return false;
        };
        let st = |id: &str| {
            jobs.iter()
                .find(|j| j.get("id").and_then(|v| v.as_str()) == Some(id))
                .and_then(|j| j.pointer("/state/last_status").cloned())
        };
        st("r9cronjja").and_then(|v| v.as_str().map(str::to_owned))
            == Some("ok".into())
            && st("r9cronjjb").and_then(|v| v.as_str().map(str::to_owned)) == Some("ok".into())
    })
    .await;

    // --- 分支 A 断言：入站真的到达了 agent 总线扇出（LLM 信封里有消息 + 回复）---
    let logs = home.join("workspace").join("logs").join("request_logs");
    // 180s：envelope 落盘等待在 workspace 满载（IO 竞态）下 60s 偶发不足，
    // 隔离复跑 5.67s 绿证明纯余量问题，按纪律加负载余量不降断言强度。
    wait_until(180, "cron branch A LLM envelope persisted", || {
        tree_contains(&logs, &msg_a) && tree_contains(&logs, &reply)
    })
    .await;
    assert!(
        mock.hits() >= 1,
        "branch A must have driven exactly-one-plus LLM calls, got {}",
        mock.hits()
    );

    // --- 分支 B 断言：store 状态机收尾（enabled=false + next_run=None + history）---
    let store = read_cron_store(&home).expect("cron store readable after fire");
    let jobs = store.get("jobs").and_then(|v| v.as_array()).expect("jobs arr");
    let jb = jobs
        .iter()
        .find(|j| j.get("id").and_then(|v| v.as_str()) == Some("r9cronjjb"))
        .expect("job B persists (delete_after_run=false)");
    assert_eq!(jb.get("enabled").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(
        jb.pointer("/state/next_run_at_ms").and_then(|v| v.as_i64()),
        None,
        "'at' job fired once must have next_run cleared"
    );
    let hist = jb
        .pointer("/state/history")
        .and_then(|v| v.as_array())
        .expect("history");
    assert_eq!(hist.len(), 1, "exactly one run record for job B");
    assert_eq!(
        hist[0].get("status").and_then(|v| v.as_str()),
        Some("ok"),
        "empty-message branch returns Ok('No message to deliver') → recorded ok"
    );

    // B 不应有任何额外 LLM 消耗（脚本恰好只备了 A 的一条）。
    assert_eq!(
        mock.remaining(),
        0,
        "script fully consumed by branch A alone proves B made no LLM calls"
    );

    graceful_teardown(gw, web_port, token, "cron_gw").await;
}

// ===========================================================================
// #2/#3/#7 cluster 双节点 peer_chat 全链 + cluster_rpc 选型 + 内部命令臂
// ===========================================================================

/// 写节点侧 peers.toml：[node] 身份 + [peers.<对端>] 静态表（地址=对端 UDP 口，
/// 网关加载循环按 udp+10000 反推对端 RPC 口）。
fn install_peers_toml(home: &Path, self_id: &str, peer_id: &str, peer_udp: u16) {
    let dir = home.join("workspace").join("cluster");
    std::fs::create_dir_all(&dir).expect("mkdir workspace/cluster");
    let body = format!(
        "[node]\n\
         id = \"{self_id}\"\n\
         name = \"{self_id}\"\n\
         role = \"worker\"\n\
         category = \"development\"\n\
         tags = []\n\
         \n\
         [peers.{peer_id}]\n\
         address = \"127.0.0.1:{peer_udp}\"\n\
         name = \"{peer_id}\"\n\
         role = \"worker\"\n\
         category = \"development\"\n"
    );
    std::fs::write(dir.join("peers.toml"), body).expect("write peers.toml");
}

/// 写 workspace/config/config.cluster.json（AppConfig 层：开关 + 端口 + 共享 token）。
fn install_cluster_app_config(home: &Path, udp: u16, rpc: u16, token: &str) {
    let cfg = json!({
        "enabled": true,
        "port": udp,
        "rpc_port": rpc,
        "broadcast_interval": 2,
        "llm_timeout_secs": 120,
        "token": token,
    });
    std::fs::write(
        home.join("workspace").join("config").join("config.cluster.json"),
        serde_json::to_string_pretty(&cfg).expect("ser cluster cfg"),
    )
    .expect("write config.cluster.json");
}

/// 双节点 peer_chat 端到端：
/// 1. 先起 B（模型=mini，MockAi canned 文本回复），就绪后再起 A（模型=big 以解锁
///    cluster_rpc 全量工具集——mini/normal 的 tier_allowed_tools 不含 cluster_rpc）；
/// 2. A 由预种子 cron 入站驱动：首条脚本 ToolCall(cluster_rpc, target=B, message=M)
///    → call_fn(2101-2114) 异步 ack → 快照 → （可选中间轮）→ 收到 B 回调后经
///    Route2 总线 cluster_continuation 续行，把 B 的 canned 回复当工具结果喂回 LLM；
/// 3. 断言：A 侧 request_logs 出现 B_REPLY（全链闭环铁证）、B 侧 request_logs 出现
///    PEER_MESSAGE（B 的 PeerChatHandler 真跑了 LLM）、两侧 store/cron ok；
/// 4. open_dashboard 臂（POST /api/internal，dll 缺席守卫下无窗口风险）拿 ack；
/// 5. 双节点各自走 shutdown 内部命令臂优雅停机（3601-3608）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r9_live_dual_node_peer_chat_full_chain_with_cluster_rpc_tool() {
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");

    // —— 端口组：A/B 完全独立且互相不同 ——
    let (udp_a, rpc_a) = probe_cluster_port_pair();
    let (udp_b, rpc_b) = probe_cluster_port_pair();
    assert_ne!(udp_a, udp_b, "port groups must be fully disjoint");

    let token_a = "r9tokA";
    let token_b = "r9tokB";
    let cluster_token = format!("r9-shared-{}", std::process::id());
    let peer_msg = format!("R9-PEER-MESSAGE-relay-{}", unique_tag("msg"));
    let b_reply = format!("R9-B-REPLY-{}", unique_tag("brep"));

    // —— B 先起（避免 A 的 rpc 拨号撞上 B 未监听）——
    let wb = TestWorkspace::new().expect("temp workspace B");
    let home_b = wb.home();
    let mock_b = MockAiServer::start(vec![MockAiReply::Text(b_reply.clone())]).expect("mock B");
    let cfg_b = live_gateway_config(
        &home_b.join("workspace"),
        &mock_b.base_url(),
        "r9-b-model",
        "mini",
        token_b,
        false,
        true,
        false,
    );
    install_home_config(&home_b, &cfg_b);
    install_peers_toml(&home_b, "R9NODEB", "R9NODEA", udp_a);
    install_cluster_app_config(&home_b, udp_b, rpc_b, &cluster_token);

    let gw_b = spawn_gateway("r9_node_b", &bin, &wb);
    let web_port_b = wait_for_web_port(&home_b).await;
    tokio::time::sleep(Duration::from_secs(8)).await; // 等 B 的 RPC 监听稳定

    // —— A 后起：cluster_rpc ToolCall → 中间轮 ack 文本 → 续行轮 final 文本 ——
    let wa = TestWorkspace::new().expect("temp workspace A");
    let home_a = wa.home();
    let mock_a = MockAiServer::start(vec![
        MockAiReply::ToolCall {
            name: "cluster_rpc".to_string(),
            arguments: json!({"target": "R9NODEB", "message": peer_msg}).to_string(),
        },
        MockAiReply::Text("R9-A-ACK-SENTINEL".to_string()),
        MockAiReply::Text("R9-A-FINAL-DONE".to_string()),
    ])
    .expect("mock A");
    let cfg_a = live_gateway_config(
        &home_a.join("workspace"),
        &mock_a.base_url(),
        "r9-a-model",
        "big", // 全量工具集：tier_allowed_tools(Big/Auto) = 空 slice = 不过滤
        token_a,
        false,
        true,
        false,
    );
    install_home_config(&home_a, &cfg_a);
    install_peers_toml(&home_a, "R9NODEA", "R9NODEB", udp_b);
    install_cluster_app_config(&home_a, udp_a, rpc_a, &cluster_token);
    seed_cron_store(
        &home_a,
        vec![seeded_at_job(
            "r9clustertrigger",
            "peer-chat-driver",
            &format!("please relay via cluster: {peer_msg}"),
            now_ms() - 4000,
            Some("web"),
        )],
    );

    let gw_a = spawn_gateway("r9_node_a", &bin, &wa);
    let web_port_a = wait_for_web_port(&home_a).await;

    // —— 全链断言：A 侧续行请求的信封里嵌着 B 的 canned 回复 ——
    let logs_a = home_a.join("workspace").join("logs");
    let logs_b = home_b.join("workspace").join("logs");
    wait_until_diag(150, "round-trip marker back on A", || {
        tree_contains(&logs_a, &b_reply)
    }, || {
        format!("home_a={}\nhome_b={}", diag_home(&home_a), diag_home(&home_b))
    })
    .await;
    wait_until(30, "B side processed the peer message", || {
        tree_contains(&logs_b, &peer_msg)
    })
    .await;
    assert!(
        mock_a.hits() >= 2,
        "A must consume ≥2 script entries (initial tool-call round + resumed round), got {}",
        mock_a.hits()
    );
    assert!(
        mock_b.hits() >= 1,
        "B must have served its canned reply over HTTP, got {}",
        mock_b.hits()
    );
    // cron 驱动任务自身也应记账 ok（fire → on_job → on_job 分支 A 走到 agent）。
    wait_until(20, "driver cron job marked ok", || {
        read_cron_store(&home_a)
            .and_then(|s| {
                s.get("jobs")
                    .and_then(|v| v.as_array())?
                    .iter()
                    .find(|j| j.get("id").and_then(|x| x.as_str()) == Some("r9clustertrigger"))
                    .and_then(|j| j.pointer("/state/last_status").cloned())
            })
            .and_then(|v| v.as_str().map(str::to_owned))
            == Some("ok".into())
    })
    .await;

    // —— #7 open_dashboard：ack 是 mpsc fire-and-forget，无论下游 spawn 成败都回 ok。
    // 若 exe 旁真有 plugin_ui.dll 则会拉起真实 WebView 窗口（禁），跳过该 POST。
    if plugin_ui_dll_next_to_bin(&bin) {
        eprintln!(
            "[r9-live] plugin_ui library found beside binary; skipping open_dashboard POST \
             to honor the no-popup discipline (ack arm remains covered by code reading)"
        );
    } else {
        let url = format!("http://127.0.0.1:{web_port_a}/api/internal");
        let resp = http_client()
            .post(&url)
            .header("X-Auth-Token", token_a)
            .json(&json!({"cmd": "open_dashboard"}))
            .send()
            .await
            .expect("POST /api/internal open_dashboard");
        assert!(
            resp.status().is_success(),
            "open_dashboard should be acked OK, got {}",
            resp.status()
        );
        let body: Value = resp.json().await.expect("ack json");
        assert_eq!(body.get("status").and_then(|v| v.as_str()), Some("ok"));
    }

    // —— 双节点优雅停机（每个实例都真跑一次 InternalCommand::Shutdown 臂）——
    graceful_teardown(gw_a, web_port_a, token_a, "node_a").await;
    graceful_teardown(gw_b, web_port_b, token_b, "node_b").await;
}

// ===========================================================================
// #5 workflow message/event 触发驱动（2908-3078）
// ===========================================================================

/// 级联设计：
/// - 预种子 cron 任务 → InboundMessage(channel=web, content 含 *R9WFBT* 标记)
///   同时抵达两个订阅者：AgentLoop（消耗 mock 一条 Text，顺带证明广播扇出）和
///   workflow 消息触发订阅任务（2929-2998）；
/// - wf-r9-msg：message 触发（channel=web + content glob），单个 transform
///   identity 节点（零外部依赖，不耗 LLM），完成后引擎在 dispatcher 上发
///   workflow.completed 事件（engine.rs:1777-1788，data 带 workflow_name/status）；
/// - wf-r9-event：event 触发匹配 event_type=workflow.completed 且
///   workflow_name=wf-r9-msg（这条 data 键过滤同时封死自递归：它自己完成时
///   workflow_name=wf-r9-event 不匹配）→ event 触发订阅任务（3002-3078）启动它。
///
/// 断言锚点：workspace/workflow/executions/{name}_{execution_id}.jsonl 存在且含
/// 各自节点输出文本。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r9_live_workflow_message_trigger_cascades_to_event_trigger() {
    let ws = TestWorkspace::new().expect("temp workspace");
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");
    let home = ws.home();
    let token = "r9wftoken";

    let tag = unique_tag("wf");
    let trigger_msg = format!("hello R9WFBT{tag} cascade please");

    let mock = MockAiServer::start(vec![MockAiReply::Text(format!(
        "R9-WF-AGENT-ECHO {tag}"
    ))])
    .expect("mock ai");

    let cfg = live_gateway_config(
        &home.join("workspace"),
        &mock.base_url(),
        "r9-wf-model",
        "mini",
        token,
        false,
        false,
        false,
    );
    install_home_config(&home, &cfg);

    // 定义目录：{home}/workspace/workflow/definitions/
    let defs = home.join("workspace").join("workflow").join("definitions");
    std::fs::create_dir_all(&defs).expect("mkdir workflow defs");
    std::fs::write(
        defs.join("wf-r9-msg.yaml"),
        "name: wf-r9-msg\n\
         version: \"1.0.0\"\n\
         triggers:\n\
         \x20 - trigger_type: message\n\
         \x20   config:\n\
         \x20     channel: \"web\"\n\
         \x20     content: \"*R9WFBT*\"\n\
         nodes:\n\
         \x20 - id: t1\n\
         \x20   node_type: transform\n\
         \x20   config:\n\
         \x20     expression: identity\n\
         \x20     input: \"saw:r9-transform-ok\"\n\
         \x20   is_terminal: true\n",
    )
    .expect("write wf-r9-msg.yaml");
    std::fs::write(
        defs.join("wf-r9-event.yaml"),
        "name: wf-r9-event\n\
         version: \"1.0.0\"\n\
         triggers:\n\
         \x20 - trigger_type: event\n\
         \x20   config:\n\
         \x20     event_type: \"workflow.completed\"\n\
         \x20     workflow_name: \"wf-r9-msg\"\n\
         nodes:\n\
         \x20 - id: t2\n\
         \x20   node_type: transform\n\
         \x20   config:\n\
         \x20     expression: identity\n\
         \x20     input: \"evt-cascade\"\n\
         \x20   is_terminal: true\n",
    )
    .expect("write wf-r9-event.yaml");

    seed_cron_store(
        &home,
        vec![seeded_at_job(
            "r9wftrigger",
            "workflow-message-driver",
            &trigger_msg,
            now_ms() - 4000,
            Some("web"),
        )],
    );

    let gw = spawn_gateway("r9_wf_gw", &bin, &ws);
    let web_port = wait_for_web_port(&home).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // 执行持久化目录：{workflow_name}_{execution_id}.jsonl
    let exec_dir = home.join("workspace").join("workflow").join("executions");

    // 消息触发的执行及其节点输出。
    wait_until(60, "wf-r9-msg execution jsonl with node output", || {
        let Ok(entries) = std::fs::read_dir(&exec_dir) else {
            return false;
        };
        entries.flatten().any(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("wf-r9-msg_") && name.ends_with(".jsonl")
                && std::fs::read_to_string(e.path())
                    .map(|t| t.contains("saw:"))
                    .unwrap_or(false)
        })
    })
    .await;

    // 级联事件触发的执行（证明 event 订阅任务真跑）。
    wait_until(60, "wf-r9-event execution jsonl cascaded", || {
        let Ok(entries) = std::fs::read_dir(&exec_dir) else {
            return false;
        };
        entries.flatten().any(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("wf-r9-event_") && name.ends_with(".jsonl")
                && std::fs::read_to_string(e.path())
                    .map(|t| t.contains("evt-cascade"))
                    .unwrap_or(false)
        })
    })
    .await;

    // 广播扇出补证：同一条入站也被 agent 消费（echo 信封落盘）。用 wait_until
    // 而非硬断言——并行多网关负载下 agent LLM 轮可能滞后于 workflow 执行落盘
    // （workflow 两个 wait_until 消耗完 ≠ agent 轮完成；echo 本身就是异步事件）。
    let logs = home.join("workspace").join("logs");
    let echo_tag = format!("R9-WF-AGENT-ECHO {tag}");
    wait_until(60, "agent echo envelope (bus fan-out)", || {
        tree_contains(&logs, &echo_tag)
    })
    .await;

    graceful_teardown(gw, web_port, token, "wf_gw").await;
}

// ===========================================================================
// #4 heartbeat 多臂（3146-3252）
// ===========================================================================

/// 实例 a：BOOTSTRAP.md 存在 → 服务级 should_skip + handler 早退双双短路，
/// 心跳整拍不做任何 LLM 调用（skip_file 设置臂 3246-3249 + handler skip 臂
/// 3180-3190 都在第一拍就执行完）。
///
/// 断言：boot 完成后观察 20s（第一拍固定 +1s，若未跳过必然有命中），mock 命中
/// 必须为 0。诚实边界：只实证第一拍的跳过决策；后续 60s 节拍不再等待（预算纪律）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r9_live_heartbeat_bootstrap_skip_makes_zero_llm_calls() {
    let ws = TestWorkspace::new().expect("temp workspace");
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");
    let home = ws.home();
    let token = "r9hbskip";

    // 空 mock 脚本：任何命中都会得到 500 并让 hits>0 → 测试响亮失败。
    let mock = MockAiServer::start(Vec::new()).expect("mock ai");

    let cfg = live_gateway_config(
        &home.join("workspace"),
        &mock.base_url(),
        "r9-hb-skip-model",
        "mini",
        token,
        true, // heartbeat 开，间隔压到最小 1 分钟（第一拍 +1s）
        false,
        false,
    );
    install_home_config(&home, &cfg);
    std::fs::write(
        home.join("workspace").join("BOOTSTRAP.md"),
        "# bootstrap marker\n",
    )
    .expect("write BOOTSTRAP.md");

    let gw = spawn_gateway("r9_hb_skip_gw", &bin, &ws);
    let web_port = wait_for_web_port(&home).await;

    tokio::time::sleep(Duration::from_secs(20)).await;
    assert_eq!(
        mock.hits(),
        0,
        "BOOTSTRAP.md present must suppress every heartbeat LLM call"
    );

    graceful_teardown(gw, web_port, token, "hb_skip_gw").await;
}

/// 实例 b：无 BOOTSTRAP.md + HEARTBEAT.md 有任务行 → 心跳真跑。
/// 脚本两拍：
/// - 第 1 拍（+1s）：任意文本 → `trim() != "HEARTBEAT_OK"` passthrough 臂执行
///   （for_llm=原文；silent=true 落 Return SilentResult，无 disk 痕迹属预期，
///   以 LLM 信封里的响应原文作为「这一拍真的发生了且内容原样」的证据）；
/// - 第 2 拍（~61s）：缩进+换行包裹的 "  HEARTBEAT_OK\n" → `response.trim() ==
///   "HEARTBEAT_OK"` 归一命中臂执行（同样 silent，证据=信封响应原文）。
///
/// 两臂覆盖度的如实说明：silent 结果不落盘是生产行为；llvm-cov 行命中来自代码
/// 真跑（不同内容 ⇒ else/if 两个臂都被求值），测试断言锚定的是信封层面的输入/
/// 输出证据，而不是推测 silent 分支内部状态。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r9_live_heartbeat_two_ticks_passthrough_then_heartbeat_ok_match() {
    let ws = TestWorkspace::new().expect("temp workspace");
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");
    let home = ws.home();
    let token = "r9hblive";

    let tag = unique_tag("hb");
    let raw_reply = format!("R9-HB-RAW {tag}");

    let mock = MockAiServer::start(vec![
        MockAiReply::Text(raw_reply.clone()),
        MockAiReply::Text("  HEARTBEAT_OK\n".to_string()),
    ])
    .expect("mock ai");

    let cfg = live_gateway_config(
        &home.join("workspace"),
        &mock.base_url(),
        "r9-hb-model",
        "mini",
        token,
        true,
        false,
        false,
    );
    install_home_config(&home, &cfg);
    std::fs::write(
        home.join("workspace").join("HEARTBEAT.md"),
        "- report one synthetic heartbeat fact\n",
    )
    .expect("write HEARTBEAT.md");

    let gw = spawn_gateway("r9_hb_live_gw", &bin, &ws);
    let web_port = wait_for_web_port(&home).await;

    // 第 1 拍证据：原始 passthrough 文本进了响应信封。
    let logs = home.join("workspace").join("logs");
    wait_until(90, "tick1 raw passthrough envelope", || {
        tree_contains(&logs, &raw_reply)
    })
    .await;
    // 信封落盘与第 2 拍 LLM 请求之间存在竞态窗口（tick2 可能已发），只断下界。
    assert!(mock.hits() >= 1, "at least one tick served so far");

    // 第 2 拍证据：HEARTBEAT_OK（连同前后空白出现在 wire 上，trim 命中归一臂）。
    wait_until(150, "tick2 HEARTBEAT_OK envelope", || {
        mock.hits() >= 2 && tree_contains(&logs, "HEARTBEAT_OK")
    })
    .await;
    assert_eq!(
        mock.remaining(),
        0,
        "script fully consumed = exactly two heartbeat LLM rounds happened"
    );

    graceful_teardown(gw, web_port, token, "hb_live_gw").await;
}

// ===========================================================================
// #6 approval ask 规则链（194-274 + load_security_rules 371-506）
// ===========================================================================

/// 预种子 cron 驱动一轮 agent：第一轮 LLM 脚本发出 write_file 工具调用，命中
/// config.security.json 的 file_rules.write[{pattern:"*",action:"ask"}] →
/// auditor RequireApproval → ApprovalPopupAdapter.request_approval_sync：
/// - dll 缺席（常态：target/*/deps 旁没有 plugins/）→ 早拒臂 Ok(false)（196-207），
///   不弹任何窗口；工具结果以失败回灌 → 第二轮 LLM 出终文本。
///
/// 判别关键：若管道错放行了，write_file 会真的在工作区创建文件 —— 因此
/// 「目标文件不存在」+「第二轮发生」二者共同锁定走的是 deny 分支。
/// 保底软检查（打印不断言）：logs 下审计痕迹。
///
/// 诚实边界：超时臂（272-275 recv_timeout → Ok(false)）需要真 popup 进程，
/// 本批不改测；若检测到 exe 旁确有 plugin_ui 库，整个测试按纪律提前让步。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r9_live_approval_ask_rule_denies_via_plugin_ui_early_exit() {
    let ws = TestWorkspace::new().expect("temp workspace");
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");
    if plugin_ui_dll_next_to_bin(&bin) {
        eprintln!(
            "[r9-live] plugin_ui library found beside binary; skipping approval scenario \
             entirely (real popup window would appear — banned by the no-window discipline)"
        );
        return;
    }
    let home = ws.home();
    let token = "r9appr";

    let tag = unique_tag("appr");
    let final_text = format!("R9-APPROVAL-FINAL {tag}");
    let probe_rel = format!("approval_probe_{tag}.txt");

    let mock = MockAiServer::start(vec![
        MockAiReply::ToolCall {
            name: "write_file".to_string(),
            arguments: json!({"path": probe_rel, "content": "should never be written"})
                .to_string(),
        },
        MockAiReply::Text(final_text.clone()),
    ])
    .expect("mock ai");

    let cfg = live_gateway_config(
        &home.join("workspace"),
        &mock.base_url(),
        "r9-appr-model",
        "mini",
        token,
        false,
        false,
        true, // security 开：load_security_rules + 审批管理器装配才会生效
    );
    install_home_config(&home, &cfg);
    std::fs::write(
        home.join("workspace").join("config").join("config.security.json"),
        r#"{
            "default_action": "allow",
            "layers": {
                "injection": {"enabled": false},
                "command_guard": {"enabled": false},
                "credential": {"enabled": false},
                "ssrf": {"enabled": false}
            },
            "file_rules": {
                "write": [{"pattern": "*", "action": "ask", "comment": "r9 ask-everything probe"}]
            }
        }"#,
    )
    .expect("write config.security.json");

    seed_cron_store(
        &home,
        vec![seeded_at_job(
            "r9apprgo",
            "approval-driver",
            &format!("write the probe file {tag}"),
            now_ms() - 4000,
            Some("web"),
        )],
    );

    let gw = spawn_gateway("r9_appr_gw", &bin, &ws);
    let web_port = wait_for_web_port(&home).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // 第二轮发生（deny 工具结果回灌后的续轮）。
    let logs = home.join("workspace").join("logs");
    // 240s：deny 回灌后第二轮在 workspace 满载下已三次超时（B1 第三轮 +
    // workspace 两轮复跑），隔离绿但窗口加大无收敛 → 改带 DIAG 定责。
    wait_until_diag(
        240,
        "second round final text after deny",
        || mock.hits() >= 2 && tree_contains(&logs, &final_text),
        || format!("mock.hits={}\n{}", mock.hits(), diag_home(&home)),
    )
    .await;

    // deny 判别：目标文件绝不能存在（放行路径会在工作区根写盘）。
    let written = home.join("workspace").join(&probe_rel).exists();
    assert!(
        !written,
        "ask-rule tool call was denied by the adapter early-exit arm; the file must NOT exist"
    );

    // 诊断性软输出（不作硬断言）：安全审计痕迹的确切格式随配置变化。
    let audit_dir = logs.join("security");
    if audit_dir.exists() {
        println!(
            "[r9-live] security audit dir contents: {:?}",
            std::fs::read_dir(&audit_dir)
                .map(|it| it.flatten().count())
                .unwrap_or(0)
        );
    }

    graceful_teardown(gw, web_port, token, "appr_gw").await;
}

// ===========================================================================
// R10 live 批（2026-08-27 MERGED miss 快照 A 类收口 · 需要真实网关/双节点/
// 心跳节拍的剩余 miss）。复用本文件既有的探测/安装/spawn/wait/teardown 助手；
// 不触碰上方任何既有测试与其 wait_until 超时值。
// ===========================================================================

/// 写「只有本节点身份、没有静态 peers」的 peers.toml（discovery-lag 用：
/// 对端只能经 RPC payload 的 _rpc.from 未知节点注册分支进入本节点视野）。
fn r10_install_peers_toml_self_only(home: &Path, self_id: &str) {
    let dir = home.join("workspace").join("cluster");
    std::fs::create_dir_all(&dir).expect("mkdir workspace/cluster");
    let body = format!(
        "[node]\n\
         id = \"{self_id}\"\n\
         name = \"{self_id}\"\n\
         role = \"worker\"\n\
         category = \"development\"\n\
         tags = []\n"
    );
    std::fs::write(dir.join("peers.toml"), body).expect("write peers.toml");
}

/// config.cluster.json 变体写手：broadcast_interval 可调（lag 场景拉到 3600，
/// 让「广播发现在本窗口内不起作用」变成显式前提而非隐式巧合）。
fn r10_install_cluster_app_config_lagged(
    home: &Path,
    udp: u16,
    rpc: u16,
    token: &str,
    broadcast_interval_secs: i64,
) {
    let cfg = json!({
        "enabled": true,
        "port": udp,
        "rpc_port": rpc,
        "broadcast_interval": broadcast_interval_secs,
        "llm_timeout_secs": 120,
        "token": token,
    });
    std::fs::write(
        home.join("workspace").join("config").join("config.cluster.json"),
        serde_json::to_string_pretty(&cfg).expect("ser cluster cfg"),
    )
    .expect("write config.cluster.json");
}

/// 手搓最小 RFC6455 客户端：连 /ws?token=<token>，升级后发一条掩码文本帧
/// {"type":"request","module":<module>,"cmd":<cmd>,"reqId":<req_id>,"data":...}，
/// 然后读回**一帧**文本响应并解析为 JSON 返回（信封 type=response，data/error 见 protocol.rs）。
/// 为本批 heartbeat-d（agent.stop 无 HTTP 路由）与 dual-node 伪造错误回调批
/// （cluster.tasks.submit / cluster.tasks.list）服务。
/// 握手侧自造 Sec-WebSocket-Key（16 字节的固定熵 + 标准 base64 即满足协议
/// 格式校验；我们不做也不需要校验服务端 Accept 值）。
fn r10_wsapi_request(
    web_port: u16,
    token: &str,
    module: &str,
    cmd: &str,
    data: Option<serde_json::Value>,
    req_id: &str,
) -> Result<serde_json::Value, String> {
    use std::io::{Read as _, Write as _};

    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    fn b64(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(B64[(n >> 18) as usize & 63] as char);
            out.push(B64[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                B64[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                B64[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    let mut s = std::net::TcpStream::connect(("127.0.0.1", web_port))
        .map_err(|e| format!("tcp connect: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;

    let key_bytes: [u8; 16] = core::array::from_fn(|i| (i as u8).wrapping_mul(31).wrapping_add(7));
    let req = format!(
        "GET /ws?token={token} HTTP/1.1\r\n\
         Host: 127.0.0.1:{web_port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n",
        b64(&key_bytes)
    );
    s.write_all(req.as_bytes()).map_err(|e| format!("handshake write: {e}"))?;

    // 读到响应头结束为止（拒绝则拿到非 101 状态行）。
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match s.read(&mut byte) {
            Ok(1) => {
                head.push(byte[0]);
                if head.ends_with(b"\r\n\r\n") {
                    break;
                }
                if head.len() > 16 * 1024 {
                    return Err("handshake header too large".into());
                }
            }
            Ok(_) => continue,
            Err(e) => return Err(format!("handshake read: {e}")),
        }
    }
    let head_text = String::from_utf8_lossy(&head).to_string();
    if !head_text.starts_with("HTTP/1.1 101") {
        return Err(format!("upgrade rejected: {}", head_text.lines().next().unwrap_or("")));
    }

    // 掩码文本帧（客户端→服务端必须掩码；请求 payload 远小于 125，无需扩展长度）。
    let payload = match data {
        Some(d) => serde_json::json!({
            "type": "request", "module": module, "cmd": cmd,
            "reqId": req_id, "data": d,
        })
        .to_string(),
        None => serde_json::json!({
            "type": "request", "module": module, "cmd": cmd, "reqId": req_id,
        })
        .to_string(),
    };
    let mask = [0x5au8, 0xa5, 0x37, 0xc3];
    let plen_bytes = payload.as_bytes();
    let mut frame = Vec::with_capacity(plen_bytes.len() + 16);
    frame.push(0x81u8);
    match plen_bytes.len() {
        n if n < 126 => frame.push(0x80u8 | n as u8),
        n if n < 65536 => {
            frame.push(0x80u8 | 126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            frame.push(0x80u8 | 127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .as_bytes()
            .iter()
            .zip(mask.iter().cycle())
            .map(|(b, m)| b ^ m),
    );
    s.write_all(&frame).map_err(|e| format!("frame write: {e}"))?;

    // 读一帧服务端响应（server→client 不掩码；支持 7/16/64-bit 三档长度）。
    fn read_exact_n(s: &mut std::net::TcpStream, n: usize) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; n];
        s.read_exact(&mut buf).map_err(|e| format!("ws read: {e}"))?;
        Ok(buf)
    }
    let hdr = read_exact_n(&mut s, 2)?;
    let opcode = hdr[0] & 0x0f;
    if opcode != 0x1 && opcode != 0x8 {
        return Err(format!("unexpected ws opcode {opcode:#04x}"));
    }
    let len7 = (hdr[1] & 0x7f) as usize;
    let plen = match len7 {
        126 => {
            let ext = read_exact_n(&mut s, 2)?;
            u16::from_be_bytes([ext[0], ext[1]]) as usize
        }
        127 => {
            let ext = read_exact_n(&mut s, 8)?;
            u64::from_be_bytes(ext.try_into().unwrap()) as usize
        }
        n => n,
    };
    if opcode == 0x8 {
        return Err("connection closed by server".into());
    }
    if plen > 1024 * 1024 {
        return Err(format!("response payload too large: {plen}"));
    }
    let raw = read_exact_n(&mut s, plen)?;
    serde_json::from_slice(&raw).map_err(|e| format!("response json: {e}: {}", String::from_utf8_lossy(&raw)))
}

/// agent.stop 薄封装（无返回数据需求；失败即整串错误消息）。
fn r10_wsapi_agent_stop(web_port: u16, token: &str, req_id: &str) -> Result<(), String> {
    r10_wsapi_request(web_port, token, "agent", "stop", None, req_id).map(|_| ())
}

/// 单节点：预占住 cluster RPC 目标端口 → rpc_server_ref.start() bind 失败 →
/// gateway.rs:1822-1827 的 error! 臂（非致命），网关继续装配到就绪。
/// 断言收敛为「照常就绪 + 照常优雅停机」——error!/info 双臂按执行顺序必然
/// 都被走过；端口全程由测试进程持有（wildcard 尽力 + specific 兜底双占位）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r10_live_cluster_rpc_port_busy_bind_error_stays_nonfatal() {
    let ws = TestWorkspace::new().expect("temp workspace");
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");
    let home = ws.home();
    let token = "r10rpcbusy";

    let (udp, rpc) = probe_cluster_port_pair();
    // 双占位：先抢 wildcard；specific 若成功也一并持有（服务器绑哪个都撞）。
    let hold_wild =
        TcpListener::bind(("0.0.0.0", rpc)).expect("hold wildcard rpc port");
    let hold_specific = TcpListener::bind(("127.0.0.1", rpc)); // 可能因 wild 已占而失败——无妨

    let cfg = live_gateway_config(
        &home.join("workspace"),
        "http://127.0.0.1:9", // 死端点即可：无人调用 LLM
        "r10-rpcbusy-model",
        "mini",
        token,
        false,
        true, // cluster master 开 → 才会走 RPC server 启动路径
        false,
    );
    install_home_config(&home, &cfg);
    install_peers_toml(&home, "R10BUSYNODE", "R10NOPE", udp);
    install_cluster_app_config(&home, udp, rpc, token);

    let gw = spawn_gateway("r10_rpcbusy_gw", &bin, &ws);
    // 关键断言：RPC bind 失败绝不阻断后续装配（web 照常 bind 出 state 端口）。
    let web_port = wait_for_web_port(&home).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    graceful_teardown(gw, web_port, token, "rpcbusy_gw").await;
    drop(hold_wild);
    drop(hold_specific);
}

/// 双节点回调路由 + 伪造 error 回调：
///
/// Phase 1（真实 round trip）：B 的 mock 脚本为空 → peer_chat 处理期 LLM 必 500。
/// 实测行为（与初版假设不同，已按实况修正）：B 端 work 队列里 loop 内部把 LLM
/// 失败**优雅降级**（loop.rs:4082 warn）后仍回 status=success 回调（cluster_agent
/// 只在 execute/resume 硬 Err 才发 "error"），A 端照样走完整回调链：gateway.rs
/// 1983 收到 → Route2 metadata{status,source_node} → bus 发布 cluster_continuation
/// → 续行轮消费脚本条目产出终文本信封落盘。另注意：异步 cluster_rpc 后第一轮的
/// assistant 回复由 loop.rs:4756 固定话术「已经联系 X 了，稍等~」顶替，不消耗脚本，
/// 故 A 脚本只需 [ToolCall, Text(final)] 两格。
/// 断言锚点：B 真的被打过 LLM（hits≥1）＋ A 终文本落盘 ＋ driver cron ok。
///
/// Phase 2（error 路线）：仓库内不存在能产出 status="error" 回调的可注入生产路径
/// （唯一产源 cluster_agent::handle_task_error 只挂在 execute/resume 的 serde/
/// extract 硬 Err 上），故测试以**合法传输层身份**伪造：用共享 token 派生 AEAD 密钥
/// （nemesis_cluster::transport::frame::derive_key + encrypt_frame 全公开 API），
/// 直连 A 的 RPC 口送一条 action=peer_chat_callback、status="error"、task_id=
/// dashboard 任务 T 的 WireMessage——模拟一个行为异常的对端节点。断言锚点：
/// tasks.submit 先建 pending 任务 T（目标指向不存在的 peer，永不收到真回调），
/// 伪造回调后 tasks.list 里 T 变 **failed**（Route3 fail_task 臂 2034-2035）；
/// 同载荷也吃过 Route2 的 error 键插入（2008-2010）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r10_live_dual_node_callback_roundtrip_and_forged_error_route() {
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");

    let (udp_a, rpc_a) = probe_cluster_port_pair();
    let (udp_b, rpc_b) = probe_cluster_port_pair();
    assert_ne!(udp_a, udp_b);

    let token_a = "r10tokEA";
    let token_b = "r10tokEB";
    let cluster_token = format!("r10-shared-err-{}", std::process::id());
    let peer_msg = format!("R10-ERR-PEER-MESSAGE-{}", unique_tag("emsg"));
    let a_final = format!("R10-ERR-FINAL-DONE-{}", unique_tag("efin"));

    // —— B 先起；脚本为空 = 每次 LLM 必 500 ——
    let wb = TestWorkspace::new().expect("temp workspace B");
    let home_b = wb.home();
    let mock_b = MockAiServer::start(Vec::new()).expect("empty mock B");
    let cfg_b = live_gateway_config(
        &home_b.join("workspace"),
        &mock_b.base_url(),
        "r10-err-b-model",
        "mini",
        token_b,
        false,
        true,
        false,
    );
    install_home_config(&home_b, &cfg_b);
    install_peers_toml(&home_b, "R10ERRB", "R10ERRA", udp_a);
    install_cluster_app_config(&home_b, udp_b, rpc_b, &cluster_token);

    let gw_b = spawn_gateway("r10_err_node_b", &bin, &wb);
    let web_port_b = wait_for_web_port(&home_b).await;
    tokio::time::sleep(Duration::from_secs(8)).await;

    // —— A 后起：首条 ToolCall(cluster_rpc) → 固定话术 ack → 错误工具结果续行轮 ——
    let wa = TestWorkspace::new().expect("temp workspace A");
    let home_a = wa.home();
    let mock_a = MockAiServer::start(vec![
        MockAiReply::ToolCall {
            name: "cluster_rpc".to_string(),
            arguments: json!({"target": "R10ERRB", "message": peer_msg}).to_string(),
        },
        MockAiReply::Text(a_final.clone()),
    ])
    .expect("mock A");
    let cfg_a = live_gateway_config(
        &home_a.join("workspace"),
        &mock_a.base_url(),
        "r10-err-a-model",
        "big",
        token_a,
        false,
        true,
        false,
    );
    install_home_config(&home_a, &cfg_a);
    install_peers_toml(&home_a, "R10ERRA", "R10ERRB", udp_b);
    install_cluster_app_config(&home_a, udp_a, rpc_a, &cluster_token);
    seed_cron_store(
        &home_a,
        vec![seeded_at_job(
            "r10errtrigger",
            "error-chain-driver",
            &format!("relay with expected failure: {peer_msg}"),
            now_ms() - 4000,
            Some("web"),
        )],
    );

    let gw_a = spawn_gateway("r10_err_node_a", &bin, &wa);
    let web_port_a = wait_for_web_port(&home_a).await;

    // B 至少真的尝试过一次 LLM（500 也计请求；启动探针也可能占 hit，故用 ≥）。
    // 预算对齐 r9 同形测试的 150s 主窗（多进程编排：B/A 起动+cron 摄取+
    // mock_a+rpc 拨号）；超时自带 DIAG 现场（hits+双 home 快照）定责卡点。
    wait_until_diag(
        150,
        "B received peer chat and tried its LLM",
        || mock_b.hits() >= 1,
        || {
            format!(
                "mock_a.hits={} mock_b.hits={}\n{}\n{}",
                mock_a.hits(),
                mock_b.hits(),
                diag_home(&home_a),
                diag_home(&home_b)
            )
        },
    )
    .await;

    // 回调把工具结果灌回 → A 续行轮消费脚本第 2 格产出终文本信封（回调链铁证）。
    let logs_a = home_a.join("workspace").join("logs");
    wait_until(150, "resumed final text after B-side degradation", || {
        mock_a.hits() >= 2 && tree_contains(&logs_a, &a_final)
    })
    .await;
    wait_until(20, "driver cron job marked ok", || {
        read_cron_store(&home_a)
            .and_then(|s| {
                s.get("jobs")
                    .and_then(|v| v.as_array())?
                    .iter()
                    .find(|j| j.get("id").and_then(|x| x.as_str()) == Some("r10errtrigger"))
                    .and_then(|j| j.pointer("/state/last_status").cloned())
            })
            .and_then(|v| v.as_str().map(str::to_owned))
            == Some("ok".into())
    })
    .await;

    // —— Phase 2：伪造 status="error" 回调 ——
    // 经 WSAPI 建 dashboard 任务 T（目标指向不存在的 peer → 永无真回调干扰），
    // 再以共享 token 派生 AES-GCM 密钥直连 A 的 RPC 口送 error 载荷。
    let submit = r10_wsapi_request(
        web_port_a,
        token_a,
        "cluster",
        "tasks.submit",
        Some(json!({
            "content": format!("r10 forged-error driver {peer_msg}"),
            "target_node_id": "R10GHOST",
        })),
        "r10sub1",
    )
    .expect("wsapi tasks.submit");
    let t10task = submit
        .pointer("/data/task_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    assert!(!t10task.is_empty(), "tasks.submit 未返回 task_id: {submit}");

    let key = nemesis_cluster::transport::frame::derive_key(&cluster_token);
    let wire = nemesis_cluster::transport::WireMessage::new_request(
        "R10ERRA",
        "R10ERRA",
        "peer_chat_callback",
        json!({
            "task_id": t10task,
            "status": "error",
            "response": "r10-forged-error-payload",
            "source_node": "R10FORGE",
        }),
    );
    let cipher = nemesis_cluster::transport::frame::encrypt_frame(&wire.to_bytes().expect("serialize wire"), &key)
        .expect("encrypt forged callback");
    {
        use std::io::{Read as _, Write as _};
        let mut rpc =
            std::net::TcpStream::connect(("127.0.0.1", rpc_a)).expect("connect A rpc port");
        rpc.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        rpc.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut framed = Vec::with_capacity(4 + cipher.len());
        framed.extend_from_slice(&(cipher.len() as u32).to_be_bytes());
        framed.extend_from_slice(&cipher);
        rpc.write_all(&framed).expect("write forged callback");
        // 尽力读响应头 4 字节（handler 先执行后回包；读不到不影响断言）。
        let mut ack_head = [0u8; 4];
        let _ = rpc.read_exact(&mut ack_head);
    }

    // Route3 fail 臂落地的外部观测：T 从 pending 翻成 failed。
    wait_until(20, "forged error marked dashboard task failed via Route3", || {
        match r10_wsapi_request(
            web_port_a,
            token_a,
            "cluster",
            "tasks.list",
            None,
            "r10list",
        ) {
            Ok(v) => v
                .pointer("/data/tasks")
                .and_then(|t| t.as_array())
                .is_some_and(|arr| {
                    arr.iter().any(|e| {
                        e.get("id").and_then(|x| x.as_str()) == Some(t10task.as_str())
                            && e.get("status").and_then(|x| x.as_str()) == Some("failed")
                    })
                }),
            Err(_) => false,
        }
    })
    .await;

    graceful_teardown(gw_a, web_port_a, token_a, "err_node_a").await;
    graceful_teardown(gw_b, web_port_b, token_b, "err_node_b").await;
}

/// ⭐ discovery-lag 未知节点注册：B 没有 A 的静态表，且两侧广播间隔拉到
/// 3600s。同主机双节点的 UDP 广播物理上不可达——announce 的目标端口是
/// **发送者自己的** UDP 口（discovery.rs:662-667 `SocketAddrV4::new(addr, port)`，
/// 本机探得的两对端口必然不同），所以「未知对端进入视野」的唯一通道是
/// gateway.rs:1907-1931 的注册分支：peer_chat 载荷 `_rpc.from` 提取源节点 +
/// `_source_rpc_port`（ClusterRpcTool 注入 A 的真实 RPC 口，loop_tools.rs:2367）
/// 注册 127.0.0.1:<真实端口>。
///
/// 判别力说明：若注册分支没跑，B 只能落到 unwrap_or(21949) 回退地址上回叫
/// ——本场景探针端口不可能是 21949，回调必然迷失，round trip 完不成。
/// 因此「A 收到 B 的 canned 回复」同时证明：注册分支真跑 + 回调寻址成功。
/// （RpcMeta{from:Some} 主臂与 fallback None 臂由同一段提取代码产出的
/// source_node_id 决定，非空在先，两臂结构上是同一出口。）⭐
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r10_live_discovery_lag_unknown_peer_registers_via_rpc_meta() {
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");

    let (udp_a, rpc_a) = probe_cluster_port_pair();
    let (udp_b, rpc_b) = probe_cluster_port_pair();
    assert_ne!(udp_a, udp_b);

    let token_a = "r10tokLA";
    let token_b = "r10tokLB";
    let cluster_token = format!("r10-shared-lag-{}", std::process::id());
    let peer_msg = format!("R10-LAG-PEER-MESSAGE-{}", unique_tag("lmsg"));
    let b_reply = format!("R10-LAG-B-REPLY-{}", unique_tag("lrep"));

    // —— B 先起：身份必须有（接受被点名 target），但对端静态表必须缺席 ——
    let wb = TestWorkspace::new().expect("temp workspace B");
    let home_b = wb.home();
    let mock_b = MockAiServer::start(vec![MockAiReply::Text(b_reply.clone())]).expect("mock B");
    let cfg_b = live_gateway_config(
        &home_b.join("workspace"),
        &mock_b.base_url(),
        "r10-lag-b-model",
        "mini",
        token_b,
        false,
        true,
        false,
    );
    install_home_config(&home_b, &cfg_b);
    r10_install_peers_toml_self_only(&home_b, "R10LAGB"); // ← 无 [peers.*]
    r10_install_cluster_app_config_lagged(&home_b, udp_b, rpc_b, &cluster_token, 3600);

    let gw_b = spawn_gateway("r10_lag_node_b", &bin, &wb);
    let web_port_b = wait_for_web_port(&home_b).await;
    tokio::time::sleep(Duration::from_secs(8)).await;

    // —— A：静态知道 B；由 cron 驱动 cluster_rpc 首呼 ——
    let wa = TestWorkspace::new().expect("temp workspace A");
    let home_a = wa.home();
    let mock_a = MockAiServer::start(vec![
        MockAiReply::ToolCall {
            name: "cluster_rpc".to_string(),
            arguments: json!({"target": "R10LAGB", "message": peer_msg}).to_string(),
        },
        // 异步 cluster_rpc 后第一轮回复被 loop.rs:4756 固定话术顶替、不消耗脚本，
        // 续行轮才消费这一格——与 dual-node 绿测同构，只留两格。
        MockAiReply::Text("R10-LAG-FINAL-DONE".to_string()),
    ])
    .expect("mock A");
    let cfg_a = live_gateway_config(
        &home_a.join("workspace"),
        &mock_a.base_url(),
        "r10-lag-a-model",
        "big",
        token_a,
        false,
        true,
        false,
    );
    install_home_config(&home_a, &cfg_a);
    install_peers_toml(&home_a, "R10LAGA", "R10LAGB", udp_b); // 只有 A 认识 B
    r10_install_cluster_app_config_lagged(&home_a, udp_a, rpc_a, &cluster_token, 3600);
    seed_cron_store(
        &home_a,
        vec![seeded_at_job(
            "r10lagtrigger",
            "lag-driver",
            &format!("cold-contact relay: {peer_msg}"),
            now_ms() - 4000,
            Some("web"),
        )],
    );

    let gw_a = spawn_gateway("r10_lag_node_a", &bin, &wa);
    let web_port_a = wait_for_web_port(&home_a).await;

    // B 侧处理过消息（LLM 信封含原文）。
    let logs_b = home_b.join("workspace").join("logs");
    wait_until(150, "B processed cold peer message", || {
        tree_contains(&logs_b, &peer_msg)
    })
    .await;

    // A 侧续行轮产出自己的终文本 = B 的回调真的沿「刚注册的 127.0.0.1:<真实
    // rpc 口>」回来了（若注册分支没跑、落到 unwrap_or(21949) 回退地址，回调必
    // 迷失，A 永远到不了终文本）——判别力不变。
    let logs_a = home_a.join("workspace").join("logs");
    wait_until(150, "final text after callback along freshly-registered route", || {
        tree_contains(&logs_a, "R10-LAG-FINAL-DONE")
    })
    .await;
    wait_until(20, "lag driver cron job marked ok", || {
        read_cron_store(&home_a)
            .and_then(|s| {
                s.get("jobs")
                    .and_then(|v| v.as_array())?
                    .iter()
                    .find(|j| j.get("id").and_then(|x| x.as_str()) == Some("r10lagtrigger"))
                    .and_then(|j| j.pointer("/state/last_status").cloned())
            })
            .and_then(|v| v.as_str().map(str::to_owned))
            == Some("ok".into())
    })
    .await;

    graceful_teardown(gw_a, web_port_a, token_a, "lag_node_a").await;
    graceful_teardown(gw_b, web_port_b, token_b, "lag_node_b").await;
}

/// heartbeat 变体 c：BOOTSTRAP.md 在启动**之后**才写入。
/// 启动期 HEARTBEAT.md 缺席 → 早期各拍 build_prompt 空 → 早退（零 LLM）；
/// 就绪后同时写 HEARTBEAT.md（任务行）+ BOOTSTRAP.md → 下一拍起 handler 进入
/// 「bootstrap 存在早退 HEARTBEAT_OK」臂（gateway.rs:3195-3204），依旧零 LLM。
/// 观察 ≥80s 保证至少一拍落在写文件之后；mock 空脚本下任何命中都会响亮失败。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r10_live_heartbeat_bootstrap_written_after_boot_suppresses_all_llm() {
    let ws = TestWorkspace::new().expect("temp workspace");
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");
    let home = ws.home();
    let token = "r10hbc";

    let mock = MockAiServer::start(Vec::new()).expect("empty mock");

    let cfg = live_gateway_config(
        &home.join("workspace"),
        &mock.base_url(),
        "r10-hbc-model",
        "mini",
        token,
        true, // heartbeat 开；第一拍 +1s、之后每 60s
        false,
        false,
    );
    install_home_config(&home, &cfg);
    // 有意不种 HEARTBEAT.md / BOOTSTRAP.md。

    let gw = spawn_gateway("r10_hbc_gw", &bin, &ws);
    let web_port = wait_for_web_port(&home).await;

    // 就绪后才布置心跳任务 + 引导标记：早期拍已被 prompt 空守卫吃掉。
    std::fs::write(
        home.join("workspace").join("HEARTBEAT.md"),
        "- r10 synthetic heartbeat task\n",
    )
    .expect("write HEARTBEAT.md late");
    std::fs::write(
        home.join("workspace").join("BOOTSTRAP.md"),
        "# r10 bootstrap marker\n",
    )
    .expect("write BOOTSTRAP.md late");

    // ≥80s：保证至少一个完整间隔拍发生在文件就位之后。
    tokio::time::sleep(Duration::from_secs(85)).await;
    assert_eq!(
        mock.hits(),
        0,
        "post-boot bootstrap marker must zero out every heartbeat LLM call"
    );
    assert_eq!(mock.remaining(), 0, "empty script stays untouched");

    graceful_teardown(gw, web_port, token, "hbc_gw").await;
}

/// heartbeat 变体 d：WSAPI agent.stop 之后再来的拍进「agent 不在」臂。
/// HEARTBEAT.md 启动即在（bootstrap 缺席）→ 前两拍（arm+~1s、interval 首
/// tick 即时）agent 存活正常消耗脚本两条；web 就绪后经手搓 WS 客户端发
/// {"module":"agent","cmd":"stop"} 停掉 AgentLoop → 下一个 +60s 拍命中
/// adapter.current()==None 早退臂（gateway.rs:3207-3218），不再耗任何 LLM。
/// 断言：恰好 2 次命中、脚本余量 0。诚实的失败模式：若机器极慢导致第三拍
/// 抢在 stop 之前，脚本第 3 次请求拿到 500 → hits==3 → 断言响亮失败（按
/// problem-analysis 纪律视为环境慢而非静默放过）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r10_live_heartbeat_agent_none_after_wsapi_stop_sends_zero_llm() {
    let ws = TestWorkspace::new().expect("temp workspace");
    let bin = resolve_nemesisbot_bin().expect("nemesisbot binary");
    let home = ws.home();
    let token = "r10hbd";

    let t1 = format!("R10-HBD-TICK1-{}", unique_tag("hbd"));
    let t2 = format!("R10-HBD-TICK2-{}", unique_tag("hbd"));
    let mock = MockAiServer::start(vec![
        MockAiReply::Text(t1),
        MockAiReply::Text(t2),
    ])
    .expect("two-entry mock");

    let cfg = live_gateway_config(
        &home.join("workspace"),
        &mock.base_url(),
        "r10-hbd-model",
        "mini",
        token,
        true,
        false,
        false,
    );
    install_home_config(&home, &cfg);
    std::fs::write(
        home.join("workspace").join("HEARTBEAT.md"),
        "- report one synthetic heartbeat fact\n",
    )
    .expect("write HEARTBEAT.md");

    let gw = spawn_gateway("r10_hbd_gw", &bin, &ws);
    let web_port = wait_for_web_port(&home).await;

    // 前两拍（agent 存活期）落地余量。
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 手搓 RFC6455 停掉主 AgentLoop（无 HTTP 等价路由）。
    r10_wsapi_agent_stop(web_port, token, "r10hbstop")
        .unwrap_or_else(|e| panic!("wsapi agent.stop upgrade/frame failed: {e}"));

    wait_until(60, "both alive-agent heartbeat ticks consumed", || {
        mock.hits() >= 2
    })
    .await;

    // 观察跨越下一个 60s 间隔拍：此后任何拍都必须止步于 agent-none 臂。
    tokio::time::sleep(Duration::from_secs(70)).await;
    assert_eq!(
        mock.hits(),
        2,
        "after WSAPI agent.stop every later heartbeat tick must bypass the LLM entirely"
    );
    assert_eq!(
        mock.remaining(),
        0,
        "script fully consumed = exactly two pre-stop heartbeat rounds happened"
    );

    graceful_teardown(gw, web_port, token, "hbd_gw").await;
}
