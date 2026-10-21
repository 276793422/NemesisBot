//! P5-R1：guest 内浏览器（headless chromium + CDP）。
//!
//! 宿主 `browser` 工具的**执行体**：在 VM 内拉起/复用一个常驻 headless
//! chromium，经 Chrome DevTools Protocol（CDP）执行导航/点击/输入/取文/
//! 截图等动作。设计锚点（plan P5-R1）：
//!
//! - **常驻 + 登录态**：chromium 以 `--user-data-dir=<workspace>/.browser-
//!   profile` 启动，profile 落 VM 盘（overlay 承载）——跨 savevm 睡眠/唤醒
//!   与冷重启都持久，「登录 → 睡眠 → 唤醒 → 登录态仍在」验收门的实现锚。
//!   `.browser-profile/` 在宿主 vm_sync `DEFAULT_EXCLUDE_PREFIXES` 表内
//!   （cookies/缓存不进项目工作副本——体积与正确性双考虑）。
//! - **懒拉起**：每次调用先探 CDP 端口（`/json/version`），活着就复用；
//!   死了才 spawn。spawn 是**游离进程**（不 kill_on_drop）——浏览器生命
//!   周期是 agent 级不是调用级，工具调用结束后 chromium 必须活着。
//! - **出网面**：chromium 经 QEMU slirp NAT 出网，宿主零新暴露面；CDP
//!   端口钉 `127.0.0.1`（VM 内回环，宿主摸不到）。
//! - **安全分层**：guest 是哑执行体，无安全判断；宿主 `browser` 工具
//!   走标准 dispatch（8 层管线 + 审计链），这里是第 9 层 VM 隔离兜底。
//!
//! CDP 客户端刻意**手写最小面**（无 chromiumoxide/headless_chrome 依赖）：
//! - HTTP 发现端点（/json/version、/json/list、/json/new）：回环裸 TCP
//!   HTTP/1.1，按响应头框架精确读（Content-Length 在场读正好 N 字节，
//!   整体超时兜底）。两版教训都在这里：①HTTP/1.0 会被 chromium 155 的
//!   DevTools server 明拒（「Cannot handle request with protocol:
//!   HTTP/1.0」，cdp_alive 永久 false，chromium 3s 就绪却白烧满 120s 预算
//!   报 not ready）；②HTTP/1.1 默认 keep-alive，该 server 实测**不因请求
//!   头 Connection: close 关连**，「读到 EOF」形态直接挂死（forensic8
//!   实证：宿主 300s 超时打出 CALL_ERR，guest 内无任何进度）。所以只认
//!   Content-Length 框架，缺席诚实报错；
//! - WS 命令通道：`tokio-tungstenite`（workspace 既有依赖）连 page
//!   target 的 webSocketDebuggerUrl，单命令单连接（无状态，防会话泄漏）。
//!
//! 截图默认落 `<workspace>/uploads/browser-<ts>.png`——uploads 子树经
//! vm_sync 回灌宿主，vision 评审 M5 同源（与 nemesis-guest-screenshot
//! desktop 版同一条回传链）。

use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use futures::{SinkExt, StreamExt};

/// CDP 调试端口（`--remote-debugging-port`；VM 内回环，不进宿主网络面）。
const CDP_PORT: u16 = 9222;
/// chromium 可执行名候选（Debian 系 `chromium` 优先；构建脚本装的就是它）。
const BROWSER_CANDIDATES: [&str; 4] = [
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
];
/// 冷 spawn 后等 CDP 就绪的墙钟预算（实测全新实例 chromium 首跑 ~3s 就绪；
/// 预算是上限不是延迟——慢盘/重载宿主兜底，活着即提前返回。历史注记：
/// 曾有三跑撞满 120s 报 not ready，根因不是冷启动慢，是下方 cdp_request
/// 用 HTTP/1.0 被 chromium 155 DevTools server 明拒——探测永远失败）。
const SPAWN_READY_SECS: u64 = 120;
/// 单条 CDP 命令墙钟预算（navigate 另有载入等待，见下）。
const CDP_CMD_TIMEOUT: Duration = Duration::from_secs(30);
/// 单条 HTTP 发现端点请求整体预算（回环小响应正常毫秒级；10s 是上限不是
/// 延迟——框架读形态下唯一会撞它的是服务端异常，挂死形态已被框架读封死）。
const CDP_HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// navigate 后等 document.readyState==complete 的缺省预算。
const NAV_WAIT_DEFAULT_MS: u64 = 30_000;
/// wait_for 缺省/上限预算。
const WAIT_FOR_DEFAULT_MS: u64 = 10_000;
const WAIT_FOR_MAX_MS: u64 = 60_000;
/// 点击/输入后的稳定等待（让 JS 框架消化事件）。
const SETTLE_MS: u64 = 300;
/// 单条 CDP 命令的 WS 响应id（单命令单连接，id 恒 1）。
const CDP_CMD_ID: u64 = 1;

/// spawn 互斥（防并发调用双双判「未运行」各拉一个 chromium——后者因
/// 调试端口被占会自己退出，仍无害；互斥只是省一次无谓 spawn）。
fn spawn_gate() -> &'static tokio::sync::Mutex<()> {
    static GATE: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    GATE.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// 工具入口：解析 `action` 并分派（宿主 VmBrowserTool 传来的 args 原样）。
/// 动作合法性先校验（未知 action 在任何 CDP 接触前诚实拒绝），再统一懒拉
/// 起 chromium（探活 → 死了才 spawn），最后分派到具体动作实现。
pub(crate) async fn handle(args: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing 'action' argument (navigate/click/type/fill_form/extract_text/screenshot/wait_for)".to_string())?;
    if !matches!(
        action,
        "navigate" | "click" | "type" | "fill_form" | "extract_text" | "screenshot" | "wait_for"
    ) {
        return Err(format!(
            "unknown browser action: {action} (allowed: navigate, click, type, fill_form, \
             extract_text, screenshot, wait_for)"
        ));
    }
    ensure_chromium(workspace).await?;
    match action {
        "navigate" => navigate(args, workspace).await,
        "click" => click(args).await,
        "type" => type_text(args).await,
        "fill_form" => fill_form(args).await,
        "extract_text" => extract_text(args).await,
        "screenshot" => screenshot(args, workspace).await,
        "wait_for" => wait_for(args).await,
        _ => unreachable!("validated above"),
    }
}

// ---------------------------------------------------------------------------
// chromium 生命周期
// ---------------------------------------------------------------------------

/// CDP 端口活性探测（GET /json/version 200 = 活着）。
async fn cdp_alive() -> bool {
    matches!(cdp_http("GET", "/json/version").await, Ok(body) if body.contains("Browser"))
}

/// 确保常驻 chromium 在跑（探活 → 死了才 spawn + 等就绪）。
async fn ensure_chromium(workspace: &Path) -> Result<(), String> {
    if cdp_alive().await {
        return Ok(());
    }
    let _guard = spawn_gate().lock().await;
    // 双检（拿锁期间可能已被并发调用拉起）。
    if cdp_alive().await {
        return Ok(());
    }

    // 可执行名探测（PATH 上第一个存在的候选；`sh -c command -v` 借 shell
    // 做 PATH 查找，避免自己解析 PATH）。
    let mut bin: Option<String> = None;
    for cand in BROWSER_CANDIDATES {
        let probe = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {cand} >/dev/null 2>&1"))
            .status()
            .await;
        if matches!(probe, Ok(s) if s.success()) {
            bin = Some(cand.to_string());
            break;
        }
    }
    let bin = bin.ok_or_else(|| {
        format!(
            "chromium not found in VM (tried: {}); install it via build-vm-image.sh (default variant 预装) or `apt install chromium`",
            BROWSER_CANDIDATES.join(", ")
        )
    })?;

    let profile_dir = workspace.join(".browser-profile");
    std::fs::create_dir_all(&profile_dir)
        .map_err(|e| format!("create browser profile dir failed: {e}"))?;

    // 游离 spawn（无 kill_on_drop）：浏览器生命周期 = agent 级，工具调用
    // 结束必须活着（登录态常驻的锚）。后台收割任务接管 Child——tokio 不再
    // 持有它（drop 不会 kill），chromium 退出时被 wait() 收割不留僵尸。
    // stdout 丢弃（DOM 无用）；stderr 落 /tmp/nemesis-chromium.log——首跑
    // 初始化慢 / 崩溃的可诊断面（dbus 噪音 KB 级，/tmp 重启即清），打开
    // 失败 best-effort 回落丢弃。
    let stderr_log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/nemesis-chromium.log")
        .ok();
    let child = tokio::process::Command::new(&bin)
        .args([
            "--headless=new",
            &format!("--remote-debugging-port={CDP_PORT}"),
            "--remote-debugging-address=127.0.0.1",
            &format!("--user-data-dir={}", profile_dir.to_string_lossy()),
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            // guest agent 以 root 运行（appliance 形态，unit 无 User=），
            // chromium 对 root 硬拒启（"Running as root without --no-sandbox
            // is not supported"，秒退→CDP 永不就绪）。关掉的是 chromium 自带
            // 沙盒——本形态的隔离边界是第 9 层 VM 本身（见模块级注释），不
            // 依赖浏览器内沙盒。
            "--no-sandbox",
            "--window-size=1280,720",
            "about:blank",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(match stderr_log {
            Some(f) => std::process::Stdio::from(f),
            None => std::process::Stdio::null(),
        })
        .spawn()
        .map_err(|e| format!("spawn {bin} failed: {e}"))?;
    tokio::spawn(async move {
        let mut child = child; // wait() 需要 &mut
        let _ = child.wait().await; // 只收割，不杀
    });

    // 就绪轮询（spawn 成功 ≠ CDP 可用；≤SPAWN_READY_SECS 内 /json/version 应答）。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(SPAWN_READY_SECS);
    while tokio::time::Instant::now() < deadline {
        if cdp_alive().await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(format!(
        "chromium spawned but CDP port {CDP_PORT} not ready within {SPAWN_READY_SECS}s"
    ))
}

// ---------------------------------------------------------------------------
// CDP 传输原语（回环 HTTP 发现端点 + WS 命令通道）
// ---------------------------------------------------------------------------

/// 回环 CDP 发现端点请求构造（纯函数，回归锁钉死 HTTP/1.1——chromium
/// 155 DevTools server 明拒 HTTP/1.0，见模块级注释）。`Connection: close`
/// 仍带上：服务端若尊重它就早关连，不尊重也不影响框架读。
fn cdp_request(method: &str, path: &str) -> String {
    format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{CDP_PORT}\r\nConnection: close\r\n\r\n")
}

/// 响应头部解析（纯函数，回归锁面）：状态码 + Content-Length。CL 缺席返回
/// None——调用方诚实报错（keep-alive 形态下无框架不可读，不做无限等待）。
fn extract_status_and_content_length(head: &str) -> Result<(u16, Option<usize>), String> {
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| "CDP endpoint: malformed HTTP response (status line)".to_string())?;
    let content_length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok());
    Ok((status, content_length))
}

async fn cdp_http(method: &str, path: &str) -> Result<String, String> {
    let req = cdp_request(method, path);
    let io = async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", CDP_PORT))
            .await
            .map_err(|e| format!("CDP endpoint connect failed: {e}"))?;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| format!("CDP endpoint write failed: {e}"))?;
        // 头部块读到 \r\n\r\n（回环小响应通常一包全来，仍按增量处理——
        // 不赌单包形态；64KiB 上限防异常服务端刷爆内存）。
        let mut head: Vec<u8> = Vec::with_capacity(512);
        let mut byte = [0u8; 1];
        loop {
            let n = stream
                .read(&mut byte)
                .await
                .map_err(|e| format!("CDP endpoint read failed: {e}"))?;
            if n == 0 {
                return Err("CDP endpoint: connection closed before headers complete".to_string());
            }
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
            if head.len() > 64 * 1024 {
                return Err("CDP endpoint: response head exceeds 64KiB".to_string());
            }
        }
        let head_str = String::from_utf8_lossy(&head);
        let (status, content_length) = extract_status_and_content_length(&head_str)?;
        if !(200..300).contains(&status) {
            return Err(format!("CDP endpoint {path} returned HTTP {status}"));
        }
        let len = content_length.ok_or_else(|| {
            format!("CDP endpoint {path}: no Content-Length (keep-alive framing unreadable)")
        })?;
        let mut body = vec![0u8; len];
        stream
            .read_exact(&mut body)
            .await
            .map_err(|e| format!("CDP endpoint body read failed: {e}"))?;
        Ok::<String, String>(String::from_utf8_lossy(&body).into_owned())
    };
    // 整体预算兜底：框架读封死了「无框架挂死」形态，这个 timeout 只防
    // 服务端半死（accept 后不吐字节的极端态）。
    tokio::time::timeout(CDP_HTTP_TIMEOUT, io)
        .await
        .map_err(|_| {
            format!(
                "CDP endpoint {path}: not settled within {}s",
                CDP_HTTP_TIMEOUT.as_secs()
            )
        })?
}

/// 解析 page target id（/json/list 第一个 type==page；没有则 PUT /json/new
/// 开一个——chromium ≥111 要求 PUT，老版本 GET 已弃用形态不迁就）。
async fn page_target() -> Result<String, String> {
    let body = cdp_http("GET", "/json/list").await?;
    let list: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("/json/list not JSON: {e}"))?;
    if let Some(pages) = list.as_array() {
        for t in pages {
            if t.get("type").and_then(|v| v.as_str()) == Some("page")
                && let Some(id) = t.get("id").and_then(|v| v.as_str())
            {
                return Ok(id.to_string());
            }
        }
    }
    let body = cdp_http("PUT", "/json/new?about:blank").await?;
    let created: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("/json/new not JSON: {e}"))?;
    created
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "/json/new: no target id in response".to_string())
}

/// 单命令 CDP 调用（page target WS：单命令单连接，响应带同 id 才收）。
/// CDP error 响应（`"error"` 键）转 Err（协议层失败，如未知方法/非法参数）。
async fn cdp_command(
    target_id: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    use tokio_tungstenite::tungstenite::Message;

    let url = format!("ws://127.0.0.1:{CDP_PORT}/devtools/page/{target_id}");
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| format!("CDP ws connect failed: {e}"))?;
    let (mut sink, mut stream) = ws.split();

    let req = serde_json::json!({ "id": CDP_CMD_ID, "method": method, "params": params });
    let payload = serde_json::to_string(&req).map_err(|e| format!("cdp req serialize: {e}"))?;
    sink.send(Message::Text(payload.into()))
        .await
        .map_err(|e| format!("CDP ws send failed: {e}"))?;

    let wait = async {
        while let Some(msg) = stream.next().await {
            let msg = msg.map_err(|e| format!("CDP ws read failed: {e}"))?;
            let Message::Text(text) = msg else { continue };
            let val: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if val.get("id").and_then(|v| v.as_u64()) == Some(CDP_CMD_ID) {
                if let Some(err) = val.get("error") {
                    return Err(format!("CDP error from {method}: {err}"));
                }
                return Ok(val
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null));
            }
            // 事件帧（id 缺失）跳过继续读。
        }
        Err("CDP ws closed before response".to_string())
    };
    tokio::time::timeout(CDP_CMD_TIMEOUT, wait)
        .await
        .map_err(|_| format!("CDP command {method} timed out ({CDP_CMD_TIMEOUT:?})"))?
}

/// Runtime.evaluate 包装（returnByValue 恒开；返回 result.result.value）。
async fn evaluate(target_id: &str, expression: &str) -> Result<serde_json::Value, String> {
    let res = cdp_command(
        target_id,
        "Runtime.evaluate",
        serde_json::json!({ "expression": expression, "returnByValue": true }),
    )
    .await?;
    // 异常形态：{exceptionDetails: {...}} —— 诚实回传异常摘要。
    if let Some(exc) = res.get("exceptionDetails") {
        return Err(format!("page JS exception: {exc}"));
    }
    Ok(res
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

// ---------------------------------------------------------------------------
// JS 片段构造（纯函数，测试钉形态——selector/text 经 JSON 字符串字面量
// 嵌入 = 注入安全：JSON 转义吃掉引号/反斜杠/换行，无法逃出字符串字面量）
// ---------------------------------------------------------------------------

/// JSON 字符串字面量嵌入（JS 字符串字面量的安全子集——JSON 转义在 JS 里
/// 语义相同；U+2028/2029 这两个 JSON 合法但裸 JS 字符串非法的码点，
/// serde_json 会转义为   形态，安全）。
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn js_click(selector: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({}); \
         if (!el) return 'ERROR:no-element'; \
         el.click(); return 'ok'; }})()",
        js_str(selector)
    )
}

fn js_type(selector: &str, text: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector({}); \
         if (!el) return 'ERROR:no-element'; \
         const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; \
         const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set; \
         if (setter) setter.call(el, {}); else el.value = {}; \
         el.dispatchEvent(new Event('input', {{ bubbles: true }})); \
         el.dispatchEvent(new Event('change', {{ bubbles: true }})); \
         return 'ok'; }})()",
        js_str(selector),
        js_str(text),
        js_str(text)
    )
}

fn js_extract(selector: Option<&str>) -> String {
    match selector {
        Some(sel) => format!(
            "(() => {{ const el = document.querySelector({}); \
             return el ? (el.innerText || el.textContent || '') : 'ERROR:no-element'; }})()",
            js_str(sel)
        ),
        None => "document.body ? document.body.innerText : ''".to_string(),
    }
}

fn js_exists(selector: &str) -> String {
    format!("document.querySelector({}) !== null", js_str(selector))
}

const JS_READY_COMPLETE: &str =
    "document.readyState === 'complete' || document.readyState === 'interactive'";

// ---------------------------------------------------------------------------
// 动作实现
// ---------------------------------------------------------------------------

async fn navigate(args: &serde_json::Value, _workspace: &Path) -> Result<String, String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "navigate: missing 'url'".to_string())?;
    let wait_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(NAV_WAIT_DEFAULT_MS)
        .min(WAIT_FOR_MAX_MS * 4);

    let target = page_target().await?;
    cdp_command(&target, "Page.navigate", serde_json::json!({ "url": url })).await?;

    // 载入等待：轮询 readyState（Page.loadEventFired 事件需订阅生命周期，
    // 单命令无状态形态下轮询是同等可信的最小实现）。
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms);
    loop {
        let ready = evaluate(&target, JS_READY_COMPLETE)
            .await
            .map(|v| v == serde_json::Value::Bool(true))
            .unwrap_or(false);
        if ready {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "navigate: page load not complete within {wait_ms}ms"
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let title = evaluate(&target, "document.title")
        .await
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    Ok(serde_json::json!({ "url": url, "title": title }).to_string())
}

async fn click(args: &serde_json::Value) -> Result<String, String> {
    let selector = args
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "click: missing 'selector'".to_string())?;
    let target = page_target().await?;
    let val = evaluate(&target, &js_click(selector)).await?;
    check_element_result(&val)?;
    tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;
    Ok(serde_json::json!({ "clicked": selector }).to_string())
}

async fn type_text(args: &serde_json::Value) -> Result<String, String> {
    let selector = args
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "type: missing 'selector'".to_string())?;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "type: missing 'text'".to_string())?;
    let target = page_target().await?;
    let val = evaluate(&target, &js_type(selector, text)).await?;
    check_element_result(&val)?;
    tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;
    Ok(serde_json::json!({ "typed": selector, "chars": text.chars().count() }).to_string())
}

async fn fill_form(args: &serde_json::Value) -> Result<String, String> {
    let fields = args
        .get("fields")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            "fill_form: missing 'fields' array ({selector, text} entries)".to_string()
        })?;
    // 先全量校验（先于 page_target 的 CDP 接触——坏字段 fail-fast，
    // 不留半填状态）。
    let pairs: Vec<(&str, &str)> = fields
        .iter()
        .map(|f| {
            let (sel, text) = (
                f.get("selector").and_then(|v| v.as_str()),
                f.get("text").and_then(|v| v.as_str()),
            );
            match (sel, text) {
                (Some(s), Some(t)) => Ok((s, t)),
                _ => Err("fill_form: each field needs 'selector' and 'text'".to_string()),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let target = page_target().await?;
    let mut filled = 0usize;
    for (sel, text) in &pairs {
        let val = evaluate(&target, &js_type(sel, text)).await?;
        check_element_result(&val)?;
        filled += 1;
    }
    tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;
    Ok(serde_json::json!({ "filled": filled }).to_string())
}

async fn extract_text(args: &serde_json::Value) -> Result<String, String> {
    let selector = args.get("selector").and_then(|v| v.as_str());
    let target = page_target().await?;
    let val = evaluate(&target, &js_extract(selector)).await?;
    let text = check_element_result(&val)?;
    // 截断纪律同 guest 其他文本出口（head+tail 防 TCP 帧撑爆）。
    Ok(truncate_head_tail(&text))
}

async fn screenshot(args: &serde_json::Value, workspace: &Path) -> Result<String, String> {
    let rel = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            format!("uploads/browser-{ts}.png")
        });
    let out = crate::resolve_in_workspace(workspace, &rel)?;

    let target = page_target().await?;
    let res = cdp_command(
        &target,
        "Page.captureScreenshot",
        serde_json::json!({ "format": "png" }),
    )
    .await?;
    let b64 = res
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "captureScreenshot: no data".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("screenshot b64 decode failed: {e}"))?;

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create dir failed: {e}"))?;
    }
    std::fs::write(&out, &bytes).map_err(|e| format!("write screenshot failed: {e}"))?;
    Ok(serde_json::json!({
        "path": out.to_string_lossy(),
        "bytes": bytes.len(),
    })
    .to_string())
}

async fn wait_for(args: &serde_json::Value) -> Result<String, String> {
    let selector = args
        .get("selector")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "wait_for: missing 'selector'".to_string())?;
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(WAIT_FOR_DEFAULT_MS)
        .min(WAIT_FOR_MAX_MS);
    let target = page_target().await?;
    let expr = js_exists(selector);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let found = evaluate(&target, &expr)
            .await
            .map(|v| v == serde_json::Value::Bool(true))
            .unwrap_or(false);
        if found {
            return Ok(serde_json::json!({ "found": selector }).to_string());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "wait_for: '{selector}' not found within {timeout_ms}ms"
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

// ---------------------------------------------------------------------------
// 共享小件
// ---------------------------------------------------------------------------

/// evaluate 返回 'ERROR:no-element' 哨兵 → 诚实 Err；其余返回文本
/// （哨兵与真实内容同通道是 JS 求值形态的固有约束——页面正常文本撞上
/// 这个前缀的概率由更长的哨兵形态压低，且误报面 = 诚实失败面，不静默）。
fn check_element_result(val: &serde_json::Value) -> Result<String, String> {
    let s = match val {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s == "ERROR:no-element" {
        return Err("element not found (selector matched nothing)".to_string());
    }
    Ok(s)
}

/// head+tail 截断（同 tools.rs 的 TRUNCATE 纪律：8k 字符上限，中段省略
/// 标记——extract_text 的 body.innerText 可能是整页文本）。
fn truncate_head_tail(text: &str) -> String {
    const HEAD: usize = 4000;
    const TAIL: usize = 4000;
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= HEAD + TAIL {
        return text.to_string();
    }
    let head: String = chars[..HEAD].iter().collect();
    let tail: String = chars[chars.len() - TAIL..].iter().collect();
    format!(
        "{head}\n...[truncated {} chars]...\n{tail}",
        chars.len() - HEAD - TAIL
    )
}

#[cfg(test)]
mod tests;
