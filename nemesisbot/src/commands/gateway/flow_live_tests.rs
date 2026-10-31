//! P4 bugfix-flow 样板工程全链真机验收（面板 + flow 剧本 + connector 三件套）。
//!
//! 单节点 gateway（cluster 关 / security 开但四层放行 / 无 MockAi——本链零 LLM，
//! fixloop 只观察不生成）+ 手搓 mock Jira（TcpListener 按路由回 JSON）：
//!
//! 1. **装配**：样板载荷拷进 %TEMP% 插件目录（`NEMESIS_WASM_PLUGINS_DIR` 进程级
//!    注入，CLI 与 gateway 子进程继承同一解析）→ CLI `plugin install --yes
//!    --allow-unsigned`（九步漏斗，lockfile trust=review-required → 启动装载豁免
//!    无签名）→ CLI `plugin enable`（盘上开关；安装≠加载）→ mock connector 写进
//!    `<workspace>/connectors/jira/`（v1 插件装配不自动注册 connector——样板 README
//!    诚实边界，测试手工照做）→ spawn gateway（load_all 装载 enabled 插件 +
//!    flow 引擎桥装配）。
//! 2. **声明面锚点**：`ui.registry` → slug/route/payload/7 视图全中（P4 钉死）。
//! 3. **六步剧本全链**（ui.invoke 全串透传宿主引擎）：手动 fetch（幂等）→
//!    board_land 落单（board_issue 产物）→ **窗口期**：看板单 in_progress + 面板
//!    confirm 人工闸（gate 未确认时 tick 跳过 fixloop——in_progress 先行无竞态）
//!    → fixloop 观察态流转（in_review → Completed(delivered)）→ regress（done →
//!    accepted）→ confirm 发布闸 → publish 单节点诚实缺口（无派发记录，
//!    on_fail=continue 落 gap 账）→ report（dossier 导出 + `{{token}}` 回帖
//!    mock Jira）→ 实例 done。
//! 4. **guest 血肉面**：`jira_get` 走 wasm32-wasip2 guest → host tool-invoke →
//!    dispatch 瀑布 → connector → mock Jira（字段投影断言）。
//!
//! 纪律：live_gate 互斥闸（与 r9/scenario 组同闸）；home 隔离 `--local`+tempdir；
//! web 端口 0；宿主壳 DLL / 样板载荷缺席 = 诚实 SKIP（前置检查，不假红）。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use test_harness::{
    TestWorkspace, WsStream, graceful_shutdown_gateway, resolve_nemesisbot_bin, ws_connect,
    ws_recv_matching, ws_send_json,
};

use super::r9_live_tests::{
    install_home_config, live_gate, live_gateway_config, spawn_gateway, wait_for_web_port,
};

/// 样板插件 slug（plugin.toml 单一真相源，此处仅作路径/寻址键）。
const PLUGIN_SLUG: &str = "bugfix-flow";

/// 剧本模板 id（flow:// 绑定的 template 段）。
const TEMPLATE: &str = "bugfix-pipeline";

/// 外部条目键（mock Jira 固定回这一条）。
const EXT_KEY: &str = "PROJ-1";

/// 安全配置（connector_flow.rs e2e 先例同款姿势）：SSRF 层**开着**带
/// allowed_hosts 白名单放行回环 mock——层关 = guard=None = connector http
/// 绑定 fail-closed 全拒（首跑实录）；放行内网端点走白名单，不 gut 整层。
/// guardian 关（零 LLM 链）；default_action=allow 直通；其余层保持默认开启
/// ——connector op 照常过真实闸门。
const SECURITY_CFG: &str = r#"{
  "default_action": "allow",
  "guardian_mode": "off",
  "layers": {
    "ssrf": { "enabled": true, "allowed_hosts": ["127.0.0.1"] }
  }
}"#;

// ---------------------------------------------------------------------------
// mock Jira（std TcpListener 阻塞线程；按 (method, path) 路由回固定 JSON）
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MockJiraLog {
    /// "METHOD path"（query 剥掉）。
    requests: Vec<String>,
    /// issue_comment 的原始请求 body。
    comment_bodies: Vec<String>,
}

struct MockJira {
    port: u16,
    log: Arc<StdMutex<MockJiraLog>>,
}

fn jira_search_body() -> String {
    // parse_entries 解包 "issues" 数组；title 取 fields.summary。
    json!({
        "startAt": 0,
        "maxResults": 20,
        "total": 1,
        "issues": [{
            "key": EXT_KEY,
            "fields": {
                "summary": "流水线验收样例缺陷",
                "status": { "name": "Open" },
                "assignee": { "displayName": "flow-tester" },
            },
        }],
    })
    .to_string()
}

fn jira_detail_body() -> String {
    json!({
        "key": EXT_KEY,
        "fields": {
            "summary": "流水线验收样例缺陷",
            "status": { "name": "Open" },
            "assignee": { "displayName": "flow-tester" },
        },
    })
    .to_string()
}

fn start_mock_jira() -> MockJira {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock jira");
    let port = listener.local_addr().expect("local addr").port();
    let log = Arc::new(StdMutex::new(MockJiraLog::default()));
    let log_t = log.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let log_t = log_t.clone();
            std::thread::spawn(move || handle_jira_conn(&mut stream, &log_t));
        }
    });
    MockJira { port, log }
}

/// 一次 HTTP 请求的读取-路由-应答（Connection: close，一连接一请求）。
fn handle_jira_conn(stream: &mut std::net::TcpStream, log: &Arc<StdMutex<MockJiraLog>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        // header 末尾 + content-length 满足即停（防读超时挂死在 keep-alive 等待）。
        if let Some(pos) = find_sub(&buf, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            let cl: usize = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            let total = pos + 4 + cl;
            if buf.len() >= total {
                break pos;
            }
        }
        match stream.read(&mut tmp) {
            Ok(0) => break find_sub(&buf, b"\r\n\r\n").unwrap_or(buf.len().saturating_sub(4)),
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(_) => return,
        }
    };
    let text = String::from_utf8_lossy(&buf).to_string();
    let first_line = text.lines().next().unwrap_or_default();
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path_q = parts.next().unwrap_or_default().to_string();
    let path = path_q.split('?').next().unwrap_or_default().to_string();
    let body = text.get(header_end + 4..).unwrap_or_default().to_string();

    log.lock()
        .expect("mock jira log")
        .requests
        .push(format!("{method} {path}"));

    let (status, reason, resp): (u16, &str, String) = match (method.as_str(), path.as_str()) {
        ("GET", "/rest/api/2/search") => (200, "OK", jira_search_body()),
        ("GET", "/rest/api/2/issue/PROJ-1") => (200, "OK", jira_detail_body()),
        ("POST", "/rest/api/2/issue/PROJ-1/comment") => {
            log.lock().expect("mock jira log").comment_bodies.push(body);
            (200, "OK", r#"{"id":"10001"}"#.to_string())
        }
        _ => (404, "Not Found", "{}".to_string()),
    };
    let out = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        resp.len(),
        resp,
    );
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.flush();
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// 装配小组件
// ---------------------------------------------------------------------------

/// 仓库根（CARGO_MANIFEST_DIR = nemesisbot/）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// 诚实 SKIP 前置：样板载荷（plugin.toml + plugin.wasm）与宿主壳 DLL 任一缺席
/// → None（e2e 无法成立，不假红）。
fn flow_prereqs() -> Option<PathBuf> {
    let payload = repo_root().join("plugins_wasm").join(PLUGIN_SLUG);
    if !payload.join("plugin.toml").is_file() || !payload.join("plugin.wasm").is_file() {
        return None;
    }
    let host_dll_dev = repo_root()
        .join("plugins")
        .join("plugin-wasm")
        .join("target")
        .join("release")
        .join("plugin_wasm.dll");
    let host_dll_bin = repo_root()
        .join("target")
        .join("release")
        .join("plugins")
        .join("plugin_wasm.dll");
    if !host_dll_dev.is_file() && !host_dll_bin.is_file() {
        return None;
    }
    Some(payload)
}

/// 载荷拷进 %TEMP% 独立插件目录 + 进程级 `NEMESIS_WASM_PLUGINS_DIR` 一次性注入。
///
/// 目录按 pid+毫秒唯一命名且**永不删除**：env 权威指向在进程生命周期内必须
/// 有效（TestWorkspace Drop 会使 tempdir 消失，殃及后续测试 spawn 的 gateway
/// 子进程继承解析）；%TEMP% 残留 KB 级测试产物可接受。
fn stage_plugins_dir(payload: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis();
    let dir =
        std::env::temp_dir().join(format!("nb_flow_plugins_{}_{stamp:x}", std::process::id()));
    copy_dir_recursive(payload, &dir.join(PLUGIN_SLUG)).expect("stage bugfix-flow payload");
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // SAFETY（edition2024 set_var 为 unsafe）：本文件唯一一次写入，发生在
        // 本套件任何子进程 spawn 之前；flow 组在 live_gate 下串行，其他并发
        // 测试不读该变量（其 gateway 子进程继承后仅影响 wasm 载荷目录解析——
        // 无 wasm 开关即零装载，惰性无害）。进程内 std env 读写走 std 同步路径。
        unsafe { std::env::set_var("NEMESIS_WASM_PLUGINS_DIR", &dir) };
    });
    dir
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// 手工 connector 装配（v1 诚实边界：插件装配不自动注册 connector——样板
/// README 同款步骤）。**不含 `[connector.credentials]`**：缺凭据会触发 B4
/// 阻塞审批卡，mock Jira 无需鉴权。base 指向本测试 mock 端口。
fn write_test_connector(ws: &TestWorkspace, jira_port: u16) {
    let dir = ws.home().join("workspace").join("connectors").join("jira");
    std::fs::create_dir_all(&dir).expect("mkdir connectors/jira");
    let toml = format!(
        r#"[connector]
id = "jira"
name = "Jira"
version = "0.1.0"
kind = "rest"
description = "flow_live_tests mock Jira（无凭据形态）"

[connector.http]
base = "http://127.0.0.1:{jira_port}"
[connector.http.headers]
Content-Type = "application/json"

[[op]]
id = "issues_list"
intent = ["列出 Jira issue"]
risk = "low"
class = "network"
http = {{ method = "GET", path = "/rest/api/2/search" }}
args_schema = {{ type = "object", properties = {{ jql = {{ type = "string" }}, maxResults = {{ type = "integer" }} }} }}

[[op]]
id = "issue_get"
intent = ["查看 Jira issue"]
risk = "low"
class = "network"
http = {{ method = "GET", path = "/rest/api/2/issue/{{key}}" }}
args_schema = {{ type = "object", required = ["key"], properties = {{ key = {{ type = "string" }} }} }}

[[op]]
id = "issue_comment"
intent = ["评论 Jira issue"]
risk = "medium"
class = "write"
receipt = "id"
http = {{ method = "POST", path = "/rest/api/2/issue/{{key}}/comment" }}
args_schema = {{ type = "object", required = ["key", "body"], properties = {{ key = {{ type = "string" }}, body = {{ type = "string" }} }} }}
"#
    );
    std::fs::write(dir.join("connector.toml"), toml).expect("write connector.toml");
}

/// CLI 子命令直跑（继承测试进程 env → NEMESIS_WASM_PLUGINS_DIR 同源解析；
/// cwd = ws 根配合 --local 隔离 home）。
fn run_cli(bin: &Path, ws: &TestWorkspace, args: &[&str], what: &str) {
    let out = std::process::Command::new(bin)
        .args(args)
        .current_dir(ws.path())
        .output()
        .unwrap_or_else(|e| panic!("{what}: spawn cli failed: {e}"));
    assert!(
        out.status.success(),
        "{what} failed (exit={:?})\n--- stdout tail ---\n{}\n--- stderr tail ---\n{}",
        out.status.code(),
        tail_str(&String::from_utf8_lossy(&out.stdout), 800),
        tail_str(&String::from_utf8_lossy(&out.stderr), 800),
    );
}

fn tail_str(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut start = s.len() - n;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

// ---------------------------------------------------------------------------
// WS 驱动（scenario_tests 同款帧形态）
// ---------------------------------------------------------------------------

async fn wsapi(
    stream: &mut WsStream,
    module: &str,
    cmd: &str,
    data: Value,
) -> Result<Value, String> {
    static N: AtomicU64 = AtomicU64::new(0);
    let rid = format!("flw-req-{}", N.fetch_add(1, Ordering::Relaxed));
    let claimed = rid.clone();
    ws_send_json(
        stream,
        &json!({
            "type": "request", "module": module, "cmd": cmd,
            "reqId": rid, "data": data,
        }),
    )
    .await
    .map_err(|e| e.to_string())?;
    let frame = ws_recv_matching(
        stream,
        Duration::from_secs(15),
        &format!("{module}.{cmd} response"),
        move |v| {
            v.get("type").and_then(|t| t.as_str()) == Some("response")
                && v.get("reqId").and_then(|r| r.as_str()) == Some(claimed.as_str())
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    if let Some(err) = frame.get("error").and_then(|e| e.as_str()) {
        return Err(format!("{module}.{cmd} error: {err}"));
    }
    Ok(frame.get("data").cloned().unwrap_or(Value::Null))
}

/// ui.invoke 快捷形态（面板数据/动作通道；flow:// 全串透传宿主引擎）。
async fn ui_invoke(stream: &mut WsStream, command: &str, args: Value) -> Result<Value, String> {
    wsapi(
        stream,
        "plugins",
        "ui.invoke",
        json!({ "slug": PLUGIN_SLUG, "command": command, "args": args }),
    )
    .await
}

/// detail 步骤状态（snake_case wire：pending/running/ok/failed）。
fn step_status(detail: &Value, idx: usize) -> String {
    detail["steps"][idx]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// 轮询 detail 直到谓词成立（600ms 步进；引擎 tick 5s 节奏，步进远细于 tick）。
async fn poll_detail(
    stream: &mut WsStream,
    what: &str,
    timeout: Duration,
    pred: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + timeout;
    loop {
        let d = ui_invoke(
            stream,
            "flow://bugfix-pipeline/detail",
            json!({ "instance": EXT_KEY }),
        )
        .await
        .unwrap_or_else(|e| panic!("detail invoke failed ({what}): {e}"));
        if pred(&d) {
            return d;
        }
        assert!(
            Instant::now() < deadline,
            "poll_detail({what}): not reached within {timeout:?}; last detail={d}"
        );
        tokio::time::sleep(Duration::from_millis(600)).await;
    }
}

// ---------------------------------------------------------------------------
// 全链测试
// ---------------------------------------------------------------------------

/// P4 全链：装配 → 声明面锚点 → fetch 幂等 → 六步剧本（双人工闸 + 诚实缺口）
/// → report 回帖 → dossier → guest 血肉面。
#[tokio::test]
async fn flow_live_full_chain() {
    let _gate = live_gate().await;
    let Some(payload) = flow_prereqs() else {
        eprintln!("SKIP flow_live_full_chain: 样板载荷或宿主壳 DLL 缺席（诚实 SKIP，非失败）");
        return;
    };
    let bin = resolve_nemesisbot_bin().expect("resolve nemesisbot binary");
    let ws = TestWorkspace::new().expect("temp workspace");
    let jira = start_mock_jira();
    write_test_connector(&ws, jira.port);
    let plugins_dir = stage_plugins_dir(&payload);

    let token = format!("flw-{}", std::process::id());
    let cfg = live_gateway_config(
        &ws.home().join("workspace"),
        // 零 LLM 链：模型指到死端口（任何意外 LLM 调用都会快速连接失败暴露）。
        "http://127.0.0.1:9",
        "flow-model",
        "big",
        &token,
        false, // heartbeat 关
        false, // cluster 关（单节点；publish 缺口即由此而来——诚实记账非假红）
        true,  // security 开（connector op 过真实 8 层闸；SSRF 白名单见 SECURITY_CFG）
    );
    install_home_config(&ws.home(), &cfg);
    std::fs::write(
        ws.home()
            .join("workspace")
            .join("config")
            .join("config.security.json"),
        SECURITY_CFG,
    )
    .expect("write config.security.json");

    // CLI 装配漏斗：install（--allow-unsigned → lockfile trust=review-required，
    // 启动装载豁免无签名）+ enable（盘上开关；安装≠加载）。
    let payload_dir = plugins_dir.join(PLUGIN_SLUG);
    run_cli(
        &bin,
        &ws,
        &[
            "--local",
            "plugin",
            "install",
            &payload_dir.to_string_lossy(),
            "--yes",
            "--allow-unsigned",
        ],
        "plugin install",
    );
    run_cli(
        &bin,
        &ws,
        &["--local", "plugin", "enable", PLUGIN_SLUG],
        "plugin enable",
    );
    // 盘上真相回读：开关记录必须已在 workspace config（CLI 诚实写盘验证）。
    let switch_txt = std::fs::read_to_string(
        ws.home()
            .join("workspace")
            .join("config")
            .join("config.plugin.wasm.json"),
    )
    .expect("config.plugin.wasm.json must exist after enable");
    assert!(
        switch_txt.contains(PLUGIN_SLUG),
        "switch file must record {PLUGIN_SLUG}: {switch_txt}"
    );

    let mut proc = spawn_gateway("flw_gateway", &bin, &ws);
    let home = ws.home();
    let web_port = wait_for_web_port(&home).await;
    assert!(
        proc.is_running().await,
        "gateway died during boot (web_port={web_port})"
    );

    let mut ws_stream = ws_connect(web_port, &token).await.expect("ws connect");

    // ---- A1 声明面锚点：ui.registry 投影（P4 钉死形态）----
    let reg = wsapi(&mut ws_stream, "plugins", "ui.registry", json!({}))
        .await
        .expect("ui.registry");
    let panels = reg["panels"].as_array().cloned().unwrap_or_default();
    let panel = panels
        .iter()
        .find(|p| p["slug"].as_str() == Some(PLUGIN_SLUG))
        .unwrap_or_else(|| panic!("registry missing {PLUGIN_SLUG} panel: {reg}"))
        .clone();
    assert_eq!(
        panel["payload"].as_bool(),
        Some(true),
        "payload hash anchor: {panel}"
    );
    assert_eq!(
        panel["pages"][0]["route"].as_str(),
        Some("bugfix"),
        "route anchor: {panel}"
    );
    let views = panel["pages"][0]["views"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        views.len(),
        7,
        "seven-view page (P4 pin): {:?}",
        views.len()
    );

    // ---- A2 手动 fetch + 幂等 ----
    let f1 = ui_invoke(&mut ws_stream, "flow://bugfix-pipeline/fetch", json!({}))
        .await
        .expect("fetch #1");
    assert_eq!(f1["template"].as_str(), Some(TEMPLATE), "fetch reply: {f1}");
    assert_eq!(f1["entries"].as_i64(), Some(1), "fetch entries: {f1}");
    assert_eq!(f1["created"].as_i64(), Some(1), "fetch created: {f1}");
    let f2 = ui_invoke(&mut ws_stream, "flow://bugfix-pipeline/fetch", json!({}))
        .await
        .expect("fetch #2");
    assert_eq!(f2["entries"].as_i64(), Some(1), "refetch entries: {f2}");
    assert_eq!(
        f2["created"].as_i64(),
        Some(0),
        "refetch must be idempotent: {f2}"
    );

    // ---- A3 list 投影：实例已物化 ----
    let list = ui_invoke(&mut ws_stream, "flow://bugfix-pipeline/list", json!({}))
        .await
        .expect("list");
    let rows = list["rows"].as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1, "one instance row: {list}");
    assert_eq!(rows[0]["instance"].as_str(), Some(EXT_KEY));
    assert_eq!(
        rows[0]["title"].as_str(),
        Some("流水线验收样例缺陷"),
        "title 投影: {list}"
    );

    // ---- A4 board_land 落单（首 tick ~20s 启动延迟 + tick 5s）----
    let d = poll_detail(
        &mut ws_stream,
        "land completes (board_issue artifact)",
        Duration::from_secs(90),
        |d| !d["board_issue"].as_str().unwrap_or_default().is_empty(),
    )
    .await;
    let board_issue = d["board_issue"]
        .as_str()
        .expect("board_issue str")
        .to_string();
    assert!(
        board_issue.starts_with("NB-"),
        "board issue number: {board_issue}"
    );
    assert_eq!(step_status(&d, 0), "ok", "fetch step done");
    assert_eq!(step_status(&d, 1), "ok", "land step done: {d}");

    // ---- A5 看板单核验（engine 经 WSAPI 同库可见）----
    let got = wsapi(
        &mut ws_stream,
        "board",
        "issue.get",
        json!({ "number": board_issue }),
    )
    .await
    .expect("board issue.get");
    let issue_id = got["issue"]["id"]
        .as_i64()
        .unwrap_or_else(|| panic!("issue.id missing: {got}"));
    let st = got["issue"]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(st == "todo" || st == "backlog", "land initial status: {st}");

    // ---- A6/A7 窗口期：in_progress 先行 + confirm 人工闸 ----
    // gate 未确认时 tick 跳过 fixloop（awaiting_gate）——两写之间无竞态：
    // in_progress 落定前任何 tick 都不会触发 fixloop 派发（单节点派发必失败
    // 会停车），confirm 后 start_fixloop 读到 InProgress 走观察臂。
    wsapi(
        &mut ws_stream,
        "board",
        "issue.status",
        json!({ "id": issue_id, "status": "in_progress" }),
    )
    .await
    .expect("issue.status in_progress");
    let c1 = ui_invoke(
        &mut ws_stream,
        "flow://bugfix-pipeline/confirm",
        json!({ "instance": EXT_KEY }),
    )
    .await
    .expect("confirm fixloop gate");
    assert_eq!(c1["ok"].as_bool(), Some(true), "confirm reply: {c1}");

    // ---- A8 fixloop 观察态：running 后置 in_review → Completed(delivered) ----
    poll_detail(
        &mut ws_stream,
        "fixloop running",
        Duration::from_secs(60),
        |d| step_status(d, 2) == "running",
    )
    .await;
    wsapi(
        &mut ws_stream,
        "board",
        "issue.status",
        json!({ "id": issue_id, "status": "in_review" }),
    )
    .await
    .expect("issue.status in_review");

    // ---- A9 fixloop 完成 → regress 前置：置 done ----
    poll_detail(&mut ws_stream, "fixloop ok", Duration::from_secs(60), |d| {
        step_status(d, 2) == "ok"
    })
    .await;
    wsapi(
        &mut ws_stream,
        "board",
        "issue.status",
        json!({ "id": issue_id, "status": "done" }),
    )
    .await
    .expect("issue.status done");

    // ---- A10 regress（check_regress：done → accepted）----
    poll_detail(&mut ws_stream, "regress ok", Duration::from_secs(60), |d| {
        step_status(d, 3) == "ok"
    })
    .await;

    // ---- A11/A12 publish 闸确认 → 单节点诚实缺口（on_fail=continue 落 gap 账）----
    let c2 = ui_invoke(
        &mut ws_stream,
        "flow://bugfix-pipeline/confirm",
        json!({ "instance": EXT_KEY }),
    )
    .await
    .expect("confirm publish gate");
    assert_eq!(
        c2["ok"].as_bool(),
        Some(true),
        "publish confirm reply: {c2}"
    );
    let d = poll_detail(
        &mut ws_stream,
        "publish failed honestly",
        Duration::from_secs(60),
        |d| step_status(d, 4) == "failed",
    )
    .await;
    let pub_err = d["steps"][4]["error"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        pub_err.contains("无派发记录"),
        "publish honest error: {pub_err}"
    );
    let gaps = d["gaps"].as_array().cloned().unwrap_or_default();
    assert!(
        !gaps.is_empty(),
        "gap ledger must record publish failure: {d}"
    );
    assert_eq!(
        gaps[0]["step"].as_str(),
        Some("publish"),
        "gap step: {gaps:?}"
    );
    assert!(
        gaps[0]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("无派发记录"),
        "gap error: {gaps:?}"
    );

    // ---- A13 report：dossier 导出 + token 回帖 → 实例 done ----
    poll_detail(
        &mut ws_stream,
        "report ok (instance done)",
        Duration::from_secs(60),
        |d| step_status(d, 5) == "ok",
    )
    .await;
    let list = ui_invoke(&mut ws_stream, "flow://bugfix-pipeline/list", json!({}))
        .await
        .expect("final list");
    assert_eq!(
        list["rows"][0]["state"].as_str(),
        Some("done"),
        "final row: {list}"
    );
    assert_eq!(
        list["rows"][0]["board_issue"].as_str(),
        Some(board_issue.as_str()),
        "board_issue projection: {list}"
    );

    // ---- A14 mock Jira 收到回帖：token 全替换（零 {{ 残留）----
    {
        let log = jira.log.lock().expect("mock jira log");
        assert!(
            log.requests
                .iter()
                .any(|r| r.contains("/rest/api/2/search")),
            "search GET never reached mock: {:?}",
            log.requests
        );
        let comment = log
            .comment_bodies
            .last()
            .unwrap_or_else(|| panic!("no comment POST recorded: {:?}", log.requests));
        assert!(
            comment.contains(&board_issue),
            "board_issue token: {comment}"
        );
        assert!(
            comment.contains("流水线验收样例缺陷"),
            "title token: {comment}"
        );
        assert!(
            comment.contains("报告："),
            "report_path token prefix: {comment}"
        );
        assert!(
            comment.contains("dossiers"),
            "report path points at dossier dir: {comment}"
        );
        assert!(
            !comment.contains("{{"),
            "unknown-token residue is verbatim ban: {comment}"
        );
    }

    // ---- A15 dossier 目录（report 步骤导出产物；wait_for_dossier 内部断言）----
    let _dossier = wait_for_dossier(&home, Duration::from_secs(30)).await;

    // ---- A16 看板单终态 done（外部系统侧状态由测试驱动，引擎只观察）----
    let got2 = wsapi(
        &mut ws_stream,
        "board",
        "issue.get",
        json!({ "number": board_issue }),
    )
    .await
    .expect("board issue.get final");
    assert_eq!(
        got2["issue"]["status"].as_str(),
        Some("done"),
        "final issue: {got2}"
    );

    // ---- A17 guest 血肉面：wasm guest → host tool-invoke → connector ----
    let g = ui_invoke(&mut ws_stream, "jira_get", json!({ "key": EXT_KEY }))
        .await
        .expect("guest jira_get");
    assert!(
        g.get("error").is_none(),
        "guest reply must not be business-error: {g}"
    );
    assert_eq!(g["key"].as_str(), Some(EXT_KEY), "guest reply: {g}");
    assert_eq!(
        g["summary"].as_str(),
        Some("流水线验收样例缺陷"),
        "guest reply: {g}"
    );
    assert_eq!(
        g["assignee"].as_str(),
        Some("flow-tester"),
        "guest reply: {g}"
    );

    // ---- A18 timeline 投影：步骤事件 + publish 缺口事件 ----
    let tl = ui_invoke(&mut ws_stream, "flow://bugfix-pipeline/timeline", json!({}))
        .await
        .expect("timeline");
    let events = tl["events"].as_array().cloned().unwrap_or_default();
    assert!(events.len() >= 6, "timeline events: {tl}");
    assert!(
        events.iter().any(|e| e["state"].as_str() == Some("error")
            && e["title"]
                .as_str()
                .unwrap_or_default()
                .contains("publish 缺口")),
        "publish gap event missing: {tl}"
    );
    assert!(
        events.iter().any(|e| e["state"].as_str() == Some("ok")
            && e["title"].as_str().unwrap_or_default().contains("report")),
        "report ok event missing: {tl}"
    );

    // ---- 收尾：优雅停机（盘上产物已断言完毕；dossier 路径入消息）----
    graceful_shutdown_gateway(web_port, &token)
        .await
        .expect("graceful shutdown request");
    proc.wait_for_exit(Duration::from_secs(40))
        .await
        .expect("gateway exited after shutdown");
}

/// dossier 导出产物轮询：`<workspace>/logs/dossiers/<dir>/README.md`（断言
/// 内容级产物而非目录存在性——Windows 删除挂起幻影防回归纪律同源）。
async fn wait_for_dossier(home: &Path, timeout: Duration) -> PathBuf {
    let root = home.join("workspace").join("logs").join("dossiers");
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(rd) = std::fs::read_dir(&root) {
            let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            dirs.sort(); // read_dir 顺序是平台实现细节：显式排序
            for d in dirs {
                if d.join("README.md").is_file() {
                    return d;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "dossier dir with README.md not produced within {timeout:?} (root={:?})",
            root
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
