// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 nodes.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

// 跨子文件引用（机械拆分）：helpers 定义在 llm.rs / tool.rs
use super::llm::{failed_node_result, resolve_runtime_string, resp_status_reason};
use super::tool::resolve_template_value;

/// HTTP 节点 SSRF 闸槽（BUG-20261102-324）：`SecurityPlugin::ssrf_guard()` 的
/// 克隆（Guard Clone 共享 inner，闸配置热更新对节点执行同效）。gateway 在
/// 安全插件构建后注入；None（security feature 关 / security.enabled=false /
/// ssrf 层关）= 直通——但重定向纪律（手动循环 + 跳数上限）不受槽影响。
/// release_fetch::SSRF_GUARD_SLOT 同构形态（init 顺序解耦 + 静态槽）。
#[cfg(feature = "security")]
static HTTP_SSRF_GUARD_SLOT: parking_lot::RwLock<Option<nemesis_security::ssrf::Guard>> =
    parking_lot::RwLock::new(None);

/// gateway 装配点：注入 HTTP 节点 SSRF 闸（与装配顺序无关）。
#[cfg(feature = "security")]
pub fn set_http_ssrf_guard(guard: Option<nemesis_security::ssrf::Guard>) {
    *HTTP_SSRF_GUARD_SLOT.write() = guard;
}

/// 重定向跨源判定（剥敏感头的开关，BUG-20261103-352 提取为纯函数单测）：
/// host、端口、scheme 任一变化即跨源，对齐 reqwest
/// `remove_sensitive_headers` 的判定形态（端口比
/// `port_or_known_default()`、scheme 独立比较）。修复前只比
/// host+`port()`：`reqwest::Url` 解析时把显式默认端口归一化掉（
/// `http://host:80` 的 `port()` 已是 None），于是 `https://host →
/// http://host` 两侧 `port()` 均 None + host 相同 → 不剥
/// Authorization/cookie/proxy-authorization，凭据明文跟随降级进 http；
/// 显式端口形态（`https://host:8443 → http://host:8443`）同理漏判。scheme
/// 比较兜住两族跨源降级/升级。
pub(super) fn redirect_cross_origin(base: &reqwest::Url, next: &reqwest::Url) -> bool {
    let cross_host = next.host_str() != base.host_str()
        || next.port_or_known_default() != base.port_or_known_default();
    cross_host || next.scheme() != base.scheme()
}

/// BUG-20261102-324：为当前跳构建 no-redirect client（重定向由调用方手动
/// 循环，每跳重建 = 每跳重新过闸）。security 构建下闸槽在场时先过闸
/// （Err = 拦，调用方转 failed_node_result）；闸返回解析 IP 集时钉死 DNS
///（防 rebinding TOCTOU——闸解析过 ≠ reqwest 连接用的 IP；TLS SNI/证书
/// 校验仍按原 host）。闸缺席（feature 关 / security.enabled=false / ssrf
/// 层关 / 白名单直通）= 无闸直通，重定向纪律由调用方循环承担。
fn http_hop_client(url: &str) -> Result<reqwest::Client, String> {
    #[cfg(feature = "security")]
    {
        let guard_opt = HTTP_SSRF_GUARD_SLOT.read().clone();
        if let Some(guard) = guard_opt {
            return match guard.resolve_and_validate_collect(url) {
                Ok(ips) if !ips.is_empty() => {
                    let parsed = reqwest::Url::parse(url)
                        .map_err(|e| format!("invalid URL '{url}': {e}"))?;
                    let host = parsed
                        .host_str()
                        .ok_or_else(|| format!("URL without host: {url}"))?
                        .to_string();
                    let port = parsed
                        .port_or_known_default()
                        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
                    let mut builder =
                        reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
                    for ip in ips {
                        builder = builder.resolve(&host, std::net::SocketAddr::new(ip, port));
                    }
                    builder
                        .build()
                        .map_err(|e| format!("HTTP client build failed: {e}"))
                }
                Ok(_) => no_redirect_http_client(),
                Err(e) => Err(format!("SSRF blocked: {e}")),
            };
        }
    }
    let _ = url; // security 关时无闸可查（loop_tools 同款钳制）
    no_redirect_http_client()
}

/// 不跟随重定向的 plain client（闸缺席 / 白名单直通路径共用）。
fn no_redirect_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("HTTP client build failed: {e}"))
}

/// Built-in HTTP node executor.
///
/// Makes an actual HTTP request using reqwest and returns the response.
pub struct HTTPNodeExecutor;

#[async_trait]
impl NodeExecutor for HTTPNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        let url_raw = node
            .config
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Resolve {{var}} / whole-value credential references in URL.
        let url = match resolve_runtime_string(url_raw, context, "http.url") {
            Ok(v) => v,
            Err(e) => return Ok(failed_node_result(&node.id, now, &e)),
        };

        let method = node
            .config
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_uppercase();

        if url.is_empty() {
            return Ok(NodeResult {
                node_id: node.id.clone(),
                output: serde_json::Value::Null,
                error: Some("http node requires 'url' config".to_string()),
                state: ExecutionState::Failed,
                started_at: now,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            });
        }

        let body_raw = node
            .config
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Resolve {{var}} / whole-value credential references in body.
        let body = match resolve_runtime_string(body_raw, context, "http.body") {
            Ok(v) => v,
            Err(e) => return Ok(failed_node_result(&node.id, now, &e)),
        };

        // Optional per-request timeout (default 30s). Without this, a slow
        // or unresponsive endpoint would hang the entire workflow indefinitely.
        // `timeout_secs: 0` is treated as "use default" since 0 isn't a useful
        // timeout for an actual HTTP round-trip. 手动循环后按跳计（web_fetch /
        // release_fetch 同语义），节点总墙钟由引擎层节点超时兜底。
        let timeout_secs = node
            .config
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .filter(|&n| n > 0)
            .unwrap_or(30);

        // Resolve {{var}} in headers (values only; keys are static).
        let headers_raw = node
            .config
            .get("headers")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let resolved_headers = match resolve_template_value(
            &serde_json::Value::Object(headers_raw),
            context,
            "http.headers",
        ) {
            Ok(v) => v,
            Err(e) => return Ok(failed_node_result(&node.id, now, &e)),
        };

        // BUG-20261102-324：手动重定向循环替代 `Client::new()` 默认自动跟随
        //（≤10 跳）——「先校验再发请求」的闸只覆盖显式 URL，对端 302 指内网
        // /元数据端点即整条绕闸（web_fetch J2a / http-pool R36 同判例；
        // workflow http 节点的 url 支持 {{var}} 运行时注入，威胁面与
        // web_fetch 同级）。语义对齐 reqwest 默认跟随：301/302/303 把非
        // GET/HEAD 转为 GET 并剥 body；307/308 保持 method+body；跨源
        //（host/端口/scheme 任一变化，BUG-20261103-352）时剥敏感头；跳数
        // 上限 5。
        const HTTP_MAX_HOPS: usize = 5;
        const SENSITIVE_HEADERS: [&str; 3] = ["authorization", "cookie", "proxy-authorization"];

        let mut current_url = url;
        let mut current_method = method;
        let mut current_body = body;
        let mut current_headers = resolved_headers.as_object().cloned().unwrap_or_default();

        let mut hops: usize = 0;
        let mut resp;
        loop {
            let client = match http_hop_client(&current_url) {
                Ok(c) => c,
                Err(e) => return Ok(failed_node_result(&node.id, now, &e)),
            };
            let mut req_builder = match current_method.as_str() {
                "POST" => client.post(&current_url),
                "PUT" => client.put(&current_url),
                "PATCH" => client.patch(&current_url),
                "DELETE" => client.delete(&current_url),
                "HEAD" => client.head(&current_url),
                _ => client.get(&current_url),
            };
            for (k, v) in &current_headers {
                if let Some(s) = v.as_str() {
                    req_builder = req_builder.header(k.as_str(), s);
                }
            }
            if !current_body.is_empty() {
                req_builder = req_builder.body(current_body.to_string());
            }
            let hop_resp = tokio::time::timeout(
                std::time::Duration::from_secs(timeout_secs),
                req_builder.send(),
            )
            .await
            .map_err(|_| format!("HTTP request timed out after {timeout_secs}s"))?
            .map_err(|e| format!("HTTP request failed: {}", e))?;

            let status = hop_resp.status();
            if status.is_redirection() {
                hops += 1;
                if hops > HTTP_MAX_HOPS {
                    return Ok(failed_node_result(
                        &node.id,
                        now,
                        &format!(
                            "too many redirects (limit {HTTP_MAX_HOPS}), stopped at {current_url}"
                        ),
                    ));
                }
                let location = hop_resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        format!("HTTP {status} redirect without Location header at {current_url}")
                    })?
                    .to_string();
                let base = reqwest::Url::parse(&current_url)
                    .map_err(|e| format!("invalid URL '{}': {}", current_url, e))?;
                let next = base
                    .join(&location)
                    .map_err(|e| format!("invalid redirect target '{}': {}", location, e))?;
                if matches!(status.as_u16(), 301..=303)
                    && current_method != "GET"
                    && current_method != "HEAD"
                {
                    current_method = "GET".to_string();
                    current_body.clear();
                }
                if redirect_cross_origin(&base, &next) {
                    current_headers
                        .retain(|k, _| !SENSITIVE_HEADERS.contains(&k.to_lowercase().as_str()));
                }
                current_url = next.to_string();
                continue;
            }

            resp = hop_resp;
            break;
        }

        let status_code = resp.status().as_u16();

        // Collect response headers
        let mut resp_headers = serde_json::Map::new();
        for (key, value) in resp.headers() {
            if let Ok(v) = value.to_str() {
                resp_headers.insert(key.as_str().to_string(), serde_json::json!(v));
            }
        }

        // Read response body with a real 1MB cap（BUG-20261025-88：此前
        // 注释声称 "limit to 1MB" 但 `resp.bytes()` 实际无上限全量缓冲——
        // URL 支持 {{var}} 从不可信输入解析，指向大端点 = 内存耗尽面，
        // 且整包 ×2 拷贝（bytes→String）进 output 落 checkpoint/日志）。
        // 分块累计到上限即停（chunked 无 Content-Length 也被盖住）；
        // 截断不是失败——output 带 truncated 诚实标记供下游分支。
        const MAX_RESPONSE_BODY_BYTES: usize = 1024 * 1024;
        let mut body_bytes: Vec<u8> = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| format!("failed to read response body: {}", e))?
        {
            if body_bytes.len() + chunk.len() > MAX_RESPONSE_BODY_BYTES {
                let take = MAX_RESPONSE_BODY_BYTES - body_bytes.len();
                body_bytes.extend_from_slice(&chunk[..take]);
                truncated = true;
                break;
            }
            body_bytes.extend_from_slice(&chunk);
        }
        let body_str = String::from_utf8_lossy(&body_bytes).to_string();

        // 诚实失败契约（2026-09-23 计划 A2）：≥400 是应用层失败，默认节点
        // Failed——全引擎不能只有 http 谎报成功（script 非零退出、tool 错误
        // 都 Failed）。完整响应保留在 output 供诊断；需要按状态码自行分支
        // 的工作流显式 `fail_on_http_error: false` 退出。
        let fail_on_http_error = node
            .config
            .get("fail_on_http_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let (state, error) = if fail_on_http_error && status_code >= 400 {
            let reason = resp_status_reason(status_code);
            (
                ExecutionState::Failed,
                Some(format!(
                    "HTTP {status_code}{reason} — fail_on_http_error 缺省 true，\
                     需按状态码分支请显式配 false"
                )),
            )
        } else {
            (ExecutionState::Completed, None)
        };

        Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "status_code": status_code,
                "headers": serde_json::Value::Object(resp_headers),
                "body": body_str,
                "truncated": truncated,
            }),
            error,
            state,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Script Node
// ---------------------------------------------------------------------------

/// Pick the interpreter binary, file extension, and argv flag for a given
/// `language` config value. Pure function so tests can verify language→binary
/// mapping without spawning processes (BUG #4 + BUG #13 regression tests).
///
/// "bat" / "cmd" route to Windows `cmd.exe /C` — anything else would run
/// batch syntax under bash and fail immediately on Windows hosts.
///
/// "powershell" / "pwsh" route to `powershell.exe` on Windows (always
/// pre-installed) and `pwsh` elsewhere (user-installed PowerShell Core).
/// Mapping both to `pwsh` silently broke Windows users who only had
/// built-in Windows PowerShell 5.1.
pub(super) fn select_script_interpreter(
    language: &str,
) -> (&'static str, &'static str, &'static str) {
    match language {
        "python" | "python3" => ("python3", ".py", "-c"),
        "python2" => ("python2", ".py", "-c"),
        "node" | "javascript" | "js" => ("node", ".js", "-e"),
        "powershell" | "pwsh" => {
            #[cfg(windows)]
            {
                ("powershell", ".ps1", "-Command")
            }
            #[cfg(not(windows))]
            {
                ("pwsh", ".ps1", "-Command")
            }
        }
        "sh" => ("sh", ".sh", "-c"),
        "bat" | "cmd" => ("cmd", ".bat", "/C"),
        _ => ("bash", ".sh", "-c"), // default to bash
    }
}

/// Built-in script node executor.
///
/// Executes a script using the system shell. The script content is written
/// to a temporary file and executed using the configured language interpreter.
/// Supports bash, python, and other scripting languages.
pub struct ScriptNodeExecutor {
    /// When `Some`, delegate to the agent `run_script` tool (a MOVE_TOOL, so
    /// executor separation / Sandboxie contains the script identically to
    /// `exec`). `None` in unit tests / stub paths → direct spawn fallback.
    tools: Option<Arc<nemesis_tools::registry::ToolRegistry>>,
    /// U10 统一执行世界。优先级：registry 工具车道 → world 工具车道（CLI/
    /// 无 registry 装配）→ 受守卫 Spawn 车道（per-node `sandbox: false`
    /// 显式 opt-out）→ 裸 spawn（单测，无任何装配）。见模块文档
    /// `crates/nemesis-sandbox/src/exec_world.rs`。
    world: Option<Arc<dyn nemesis_sandbox::exec_world::ExecutionWorld>>,
}

impl ScriptNodeExecutor {
    /// No tool registry → direct spawn (unit tests, stub executors,
    /// `execute_inline_node`). Preserves the pre-sandbox behaviour.
    pub fn new() -> Self {
        Self {
            tools: None,
            world: None,
        }
    }

    /// With the agent tool registry bridged in by the gateway → scripts run via
    /// the `run_script` MOVE_TOOL (sandbox-aware, security-pipelined).
    pub fn with_tools(tools: Arc<nemesis_tools::registry::ToolRegistry>) -> Self {
        Self {
            tools: Some(tools),
            world: None,
        }
    }

    /// With a U10 execution world but no registry (CLI / bare engines):
    /// scripts route through the world's tool lane (executor child / box).
    pub fn with_world(world: Arc<dyn nemesis_sandbox::exec_world::ExecutionWorld>) -> Self {
        Self {
            tools: None,
            world: Some(world),
        }
    }

    /// Registry + world (gateway integrated engine after
    /// `set_execution_world`): registry lane first, world covers the
    /// opt-out/Spawn lanes.
    pub fn with_tools_and_world(
        tools: Arc<nemesis_tools::registry::ToolRegistry>,
        world: Arc<dyn nemesis_sandbox::exec_world::ExecutionWorld>,
    ) -> Self {
        Self {
            tools: Some(tools),
            world: Some(world),
        }
    }
}

impl Default for ScriptNodeExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl NodeExecutor for ScriptNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        let now = Local::now();

        let script = node
            .config
            .get("script")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let language = node
            .config
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("bash");

        if script.is_empty() {
            return Ok(NodeResult {
                node_id: node.id.clone(),
                output: serde_json::Value::Null,
                error: Some("script node requires 'script' config".to_string()),
                state: ExecutionState::Failed,
                started_at: now,
                ended_at: Local::now(),
                metadata: HashMap::new(),
            });
        }

        // Resolve template variables from context
        let resolved_script = resolve_template(script, context);

        // Determine the interpreter and file extension based on language.
        // "bat"/"cmd" map to cmd.exe so Windows users actually get batch
        // semantics — falling through to bash would run bat syntax under
        // bash and immediately fail.
        let (interpreter, _ext, flag) = select_script_interpreter(language);
        // Windows：裸 "bash"/"sh" 经 PATH 恒命中 System32 的 WSL launcher
        // （feature 启用但无发行版 = exit 1 空壳，2026-09-02 CI 实证）→
        // 解析到 Git/MSYS2 的真 bash。下方四条车道（registry 工具 args /
        // world SpawnOp / world 工具 args / 裸 spawn）消费同一解析结果；
        // 单一真相源 nemesis-tools::shell。
        let interpreter = nemesis_tools::shell::resolve_posix_shell_path(interpreter);

        // Sandbox-aware path: delegate to the `run_script` agent tool (a
        // MOVE_TOOL). When executor separation is on, RemoteExecutorTool runs
        // it in the subprocess / Sandboxie box — the SAME switch the agent's
        // `exec` respects, so a workflow script follows the global sandbox
        // state automatically. AgentToolAdapter also runs the security rule
        // pipeline per call (closing the script node's historic "no security
        // checks" gap). The tool returns structured {stdout,stderr,exit_code},
        // preserving this node's output contract.
        if let Some(tools) = &self.tools
            && tools.has("run_script")
        {
            let args = serde_json::json!({
                "interpreter": interpreter,
                "flag": flag,
                "script": resolved_script,
            });
            let tool_result = tools.execute("run_script", &args).await;
            return Ok(script_tool_result_to_node_result(
                &node.id,
                now,
                language,
                tool_result,
            ));
        }
        // run_script not registered (minimal/test registry) → fall through
        // to the world / direct-spawn paths below.

        // U10 per-node `sandbox` 开关（config key，bool）：
        // - 缺省 / `true`：跟随全局（registry → world 工具车道 → 受守卫
        //   Spawn 车道），与 agent `exec` 同一开关链。
        // - `false`：**显式 opt-out**——脚本在本进程内直跑（受守卫：
        //   cwd 根 + env 清洗 + CREATE_NO_WINDOW + 超时），不进子进程/盒。
        //   语义边界：per-node 只能降级不能升级（executor.sandbox=false 时
        //   没有通道/盒可进，`sandbox: true` 不凭空造出盒）。
        let sandbox_opt = node.config.get("sandbox").and_then(|v| v.as_bool());

        if sandbox_opt == Some(false)
            && let Some(world) = &self.world
        {
            let op =
                nemesis_sandbox::exec_world::ExecOp::Spawn(nemesis_sandbox::exec_world::SpawnOp {
                    program: interpreter.to_string(),
                    args: vec![flag.to_string(), resolved_script.clone()],
                    cwd: None,
                    stdin: None,
                    timeout_secs: None,
                });
            match world.run(op).await {
                Ok(outcome) => {
                    return Ok(script_output_node_result(
                        &node.id,
                        now,
                        language,
                        outcome.stdout,
                        outcome.stderr,
                        outcome.exit_code.unwrap_or(-1) as i64,
                    ));
                }
                Err(e) => {
                    return Ok(script_output_node_result(
                        &node.id,
                        now,
                        language,
                        String::new(),
                        format!("execution-world spawn failed: {e}"),
                        -1,
                    ));
                }
            }
        }
        // 无 world（单测/裸装配）→ 落到下方裸 spawn（保持旧行为）。

        // U10 world 工具车道：无 registry 的装配（CLI `workflow run`、裸
        // engine）经执行世界调 `run_script` —— 与 agent `exec` 同一个
        // executor 子进程 / Sandboxie 盒开关，不再有「CLI 直 spawn 绕过
        // 沙盒」的逃逸路径。
        if let Some(world) = &self.world
            && world.supports_tool_calls()
        {
            let args = serde_json::json!({
                "interpreter": interpreter,
                "flag": flag,
                "script": resolved_script,
            })
            .to_string();
            let op =
                nemesis_sandbox::exec_world::ExecOp::Tool(nemesis_sandbox::exec_world::ToolOp {
                    tool: "run_script".to_string(),
                    args,
                });
            match world.run(op).await {
                Ok(outcome) => {
                    return Ok(script_output_node_result(
                        &node.id,
                        now,
                        language,
                        outcome.stdout,
                        outcome.stderr,
                        outcome.exit_code.unwrap_or(-1) as i64,
                    ));
                }
                Err(e) => {
                    return Ok(script_output_node_result(
                        &node.id,
                        now,
                        language,
                        String::new(),
                        format!("execution-world tool lane failed: {e}"),
                        -1,
                    ));
                }
            }
        }

        // Fallback (no registry / no world — unit tests / stub paths):
        // direct spawn.
        let mut cmd = tokio::process::Command::new(interpreter);
        cmd.arg(flag).arg(&resolved_script);
        // 黑窗根修（2026-10-19）：直跑臂与生产 execution-world 车道
        // （guarded_direct_spawn）同纪律压 CREATE_NO_WINDOW。
        // （tokio Command 在 Windows 原生自带 creation_flags，无需 std CommandExt）
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let output = cmd
            .output()
            .await
            .map_err(|e| format!("failed to execute script: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1) as i64;

        Ok(script_output_node_result(
            &node.id, now, language, stdout, stderr, exit_code,
        ))
    }
}

/// Build a `NodeResult` from raw script output (shared by the direct-spawn
/// fallback and the `run_script` tool path). State is `Completed` iff
/// `exit_code == 0`; output preserves the `{stdout,stderr,exit_code,language}`
/// contract.
fn script_output_node_result(
    node_id: &str,
    started: chrono::DateTime<Local>,
    language: &str,
    stdout: String,
    stderr: String,
    exit_code: i64,
) -> NodeResult {
    let state = if exit_code == 0 {
        ExecutionState::Completed
    } else {
        ExecutionState::Failed
    };
    let error = if exit_code != 0 {
        Some(if stderr.trim().is_empty() {
            format!("Script exited with code {}", exit_code)
        } else {
            format!("Script error (exit {}): {}", exit_code, stderr.trim())
        })
    } else {
        None
    };
    NodeResult {
        node_id: node_id.to_string(),
        output: serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
            "language": language,
        }),
        error,
        state,
        started_at: started,
        ended_at: Local::now(),
        metadata: HashMap::new(),
    }
}

/// Convert a `run_script` tool `ToolResult` into a `NodeResult`. The tool
/// returns structured JSON `{stdout,stderr,exit_code}` on success (parsed back
/// here) and an error string on spawn/timeout failure.
pub(super) fn script_tool_result_to_node_result(
    node_id: &str,
    started: chrono::DateTime<Local>,
    language: &str,
    result: nemesis_tools::types::ToolResult,
) -> NodeResult {
    if result.is_error {
        return NodeResult {
            node_id: node_id.to_string(),
            output: serde_json::Value::Null,
            error: Some(format!("run_script: {}", result.for_llm)),
            state: ExecutionState::Failed,
            started_at: started,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        };
    }
    // The tool always returns JSON on a successful spawn; parse defensively.
    let parsed: serde_json::Value =
        serde_json::from_str(&result.for_llm).unwrap_or(serde_json::Value::Null);
    let stdout = parsed
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let stderr = parsed
        .get("stderr")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let exit_code = parsed
        .get("exit_code")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    script_output_node_result(node_id, started, language, stdout, stderr, exit_code)
}

/// Resolve simple template variables in a string from the context.
///
/// Replaces `{{variable_name}}` patterns with the corresponding values
/// from the context map.
pub(super) fn resolve_template(
    template: &str,
    context: &HashMap<String, serde_json::Value>,
) -> String {
    let mut result = template.to_string();
    for (key, value) in context {
        let pattern = format!("{{{{{}}}}}", key);
        let replacement = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        result = result.replace(&pattern, &replacement);
    }
    result
}

// ---------------------------------------------------------------------------
// HumanReview Node
// ---------------------------------------------------------------------------
