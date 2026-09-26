//! P23（能力扩展 WS10）：`nemesisbot mcp-serve` —— stdio MCP server。
//!
//! 把 NemesisBot 的能力作为 MCP（Model Context Protocol）server 通过 stdio
//! 暴露，Claude Code / Cursor 等 MCP 客户端零改造接入。协议层复用
//! `nemesis-mcp` 的 server 侧实现（[`nemesis_mcp::server::McpServer`]：
//! initialize / tools/list / tools/call / resources / ping 全套），本模块
//! 只补 stdio ndjson 帧循环与 K1 式装配。
//!
//! 装配与 ACP（`crate::acp.rs`）同构：进程启动时**一次完整 K1 式装配**
//! （`build_security_plugin` + `SharedResources` + `build_agent_loop`，与
//! `commands/run.rs` 逐行同源）——安全 8 层 + guardian + tier 过滤 +
//! turn_guard 与 gateway **同源生效**，MCP 出口不是安全旁路。`run` 工具
//! 走装配出的同一个 agent loop 的 `run_detached_events`（与 headless run
//! / spawn 子代理同一条全治理链路）。
//!
//! v1 工具面（四个）：
//! - `run`：headless 单任务（LLM + 工具全链，可能耗时数分钟）。
//! - `sessions_list`：列出会话（与 Dashboard `sessions.list` 同源扫描
//!   `workspace/logs/session_logs/*.jsonl`）。
//! - `memory_search`：记忆检索（enhanced memory 开 = MemoryToolExecutor
//!   同源路径；否则回落 `workspace/memory/` 轻量文本检索）。
//! - `board_issue_query`：看板 issue 查询（board feature 裁剪时诚实报错）。
//!
//! 协议边界（v1 诚实边界）：
//! - stdout 是协议帧专用通道，任何日志一律 stderr（ACP 同纪律）。
//! - JSON-RPC 通知（无 id 帧，如 `notifications/initialized`）按 JSON-RPC
//!   语义不回帧（[`is_notification`] 判定）。
//! - 无 resources/prompts 实际内容、无增量流式（`run` 整段返回）；MCP
//!   客户端自带的超时对长任务生效。
//! - sync ToolHandler → async 桥用 `block_in_place` + `Handle::block_on`
//!   （AcpApprovalManager 同款手法），要求 multi-thread runtime——
//!   `nemesisbot mcp-serve` 经 `#[tokio::main]` / macOS 手动 multi-thread
//!   runtime 满足。

use std::path::Path;
use std::sync::Arc;

use nemesis_mcp::server::{McpServer, ToolHandler};
use nemesis_mcp::types::{McpTool, ToolCallResult};
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;

/// MCP server 身份名（initialize 的 serverInfo.name）。
pub const SERVER_NAME: &str = "nemesisbot";

// ---------------------------------------------------------------------------
// K1 式装配（ACP / run 同源）
// ---------------------------------------------------------------------------

/// mcp-serve 的装配产物。`security_active` 是「安全 8 层是否真实在位」的
/// 结构断言面（build_security_plugin 返回 Some = true；config 关闭或
/// security feature 裁剪 = false，诚实呈现绝不谎报）。
pub struct McpServeAssembly {
    pub home: std::path::PathBuf,
    pub workspace: std::path::PathBuf,
    pub agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    pub security_active: bool,
    /// enhanced memory 开（config.json `memory.enabled=true` 且 memory
    /// feature 编译）时的同源记忆执行器（gateway ctx.rs 同款构造）。
    #[cfg(feature = "memory")]
    pub memory_executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

/// 手写 Debug（AgentLoop / MemoryToolExecutor 不 impl Debug；测试里
/// `unwrap_err` 需要 Ok 侧 Debug）。
impl std::fmt::Debug for McpServeAssembly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServeAssembly")
            .field("home", &self.home)
            .field("workspace", &self.workspace)
            .field("security_active", &self.security_active)
            .finish_non_exhaustive()
    }
}

/// 一次完整 K1 式装配（`commands/run.rs` 同源：config 加载 → credentials/
/// vault（调用方 run_command 已做）→ security plugin → SharedResources →
/// build_agent_loop）。装配失败 = Err（含用户可读 remedy）。
pub async fn assemble(home: &Path) -> Result<McpServeAssembly, String> {
    // 配置必须存在（headless 不隐式 onboard，与 run/acp 同纪律）。
    let config_path = crate::common::config_path(home);
    if !config_path.exists() {
        return Err(format!(
            "Configuration not found: {}. Run 'nemesisbot onboard default' first.",
            config_path.display()
        ));
    }
    let cfg = nemesis_config::load_config(&config_path)
        .map_err(|e| format!("failed to load config: {e}"))?;
    let workspace = crate::common::workspace_path(home);

    // SecurityPlugin——与 gateway 完全同一构造（K1 提取的单一真相源）。
    let security_enabled = cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true);
    let security_plugin =
        crate::security_setup::build_security_plugin(home, security_enabled).await;
    let security_active = security_plugin.is_some();

    let config_store = Arc::new(nemesis_config::ConfigStore::from_config(
        cfg.clone(),
        config_path,
    ));
    let shared = Arc::new(crate::agent_factory::SharedResources {
        home: home.to_path_buf(),
        workspace: workspace.clone(),
        config_store,
        security_plugin,
        mcp_enabled: cfg.mcp.as_ref().map(|m| m.enabled).unwrap_or(false),
        mcp_config_path: crate::common::mcp_config_path(home),
        // v1 不透工具事件（无流式契约）；需要时挂 broadcast + 通知即可。
        agent_event_tx: None,
        ..Default::default()
    });
    let agent_loop = crate::agent_factory::build_agent_loop(&shared)
        .map_err(|e| format!("failed to build agent loop: {e}"))?;

    // enhanced memory：gateway ctx.rs 同款构造（with_config_dir 自动探测
    // ONNX 插件，缺插件自动降级 basic，绝不 panic）。
    #[cfg(feature = "memory")]
    let memory_executor = {
        if cfg.memory.as_ref().map(|m| m.enabled).unwrap_or(false) {
            let memory_data_dir = home.join("workspace").join("memory_vector");
            let config_dir = home.join("workspace").join("config");
            let mgr = Arc::new(nemesis_memory::manager::MemoryManager::with_config_dir(
                &memory_data_dir,
                &config_dir,
            ));
            Some(Arc::new(
                nemesis_memory::memory_tools::MemoryToolExecutor::new(mgr),
            ))
        } else {
            None
        }
    };

    Ok(McpServeAssembly {
        home: home.to_path_buf(),
        workspace,
        agent_loop,
        security_active,
        #[cfg(feature = "memory")]
        memory_executor,
    })
}

// ---------------------------------------------------------------------------
// 工具面（四个）
// ---------------------------------------------------------------------------

/// v1 工具清单（静态定义；handler 由 [`make_handler`] 按 name 装配）。
pub fn tool_definitions() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "run".into(),
            description: Some(
                "Run a NemesisBot agent task (headless, full security pipeline). \
                 Executes an autonomous LLM + tools loop and returns the final reply. \
                 May take minutes; subject to the MCP client's own timeout."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Task prompt (required)"},
                    "max_turns": {"type": "integer", "description": "Tool-turn budget (0 = config default)"}
                },
                "required": ["task"]
            }),
        },
        McpTool {
            name: "sessions_list".into(),
            description: Some(
                "List NemesisBot conversations (session logs under workspace/logs/session_logs). \
                 Read-only; same source as the Dashboard session list."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "description": "Max sessions to return (default 50, cap 200)"}
                }
            }),
        },
        McpTool {
            name: "memory_search".into(),
            description: Some(
                "Search NemesisBot memory. Uses the enhanced memory stack when enabled \
                 (TF-IDF / vector / episodic / graph); always also searches the \
                 workspace memory markdown files as a lightweight fallback."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (required)"},
                    "limit": {"type": "integer", "description": "Max hits (default 10, cap 50)"}
                },
                "required": ["query"]
            }),
        },
        McpTool {
            name: "board_issue_query".into(),
            description: Some(
                "Query NemesisBot board issues (read-only). Without 'number' returns a \
                 filtered list; with 'number' (e.g. NB-1) returns issue detail + comments. \
                 Errors honestly when the board subsystem is not compiled in."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "number": {"type": "string", "description": "Issue number for detail view, e.g. NB-1"},
                    "status": {"type": "string", "description": "Status filter: backlog/todo/in_progress/in_review/done/blocked/cancelled"},
                    "query": {"type": "string", "description": "Substring filter on number/title"},
                    "limit": {"type": "integer", "description": "Max issues in list view (default 50, cap 200)"}
                }
            }),
        },
    ]
}

/// sync ToolHandler → async 桥：`block_in_place` 把当前 worker 线程转为
/// blocking 线程后 `block_on` 驱动内层 future（AcpApprovalManager 同款
/// 手法；要求 multi-thread runtime，见模块头）。**内层 future 必须
/// 'static**（按值持有 `Arc`/`Value` 捕获——泛型签名无法把 `Fut` 的
/// 生命周期绑到 `F` 的借用上，借用捕获过不了编译）。
fn bridge<F, Fut>(make_future: F) -> ToolCallResult
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ToolCallResult> + 'static,
{
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(make_future()))
}

/// 按工具名装配 handler（[`tool_definitions`] 的运行时半边）。
fn make_handler(name: &str, asm: &Arc<McpServeAssembly>) -> Option<ToolHandler> {
    match name {
        "run" => {
            let asm = Arc::clone(asm);
            Some(Arc::new(move |args| {
                let asm = Arc::clone(&asm);
                bridge(move || tool_run(asm, args))
            }))
        }
        "sessions_list" => {
            let asm = Arc::clone(asm);
            Some(Arc::new(move |args| {
                let asm = Arc::clone(&asm);
                bridge(move || tool_sessions_list(asm, args))
            }))
        }
        "memory_search" => {
            let asm = Arc::clone(asm);
            Some(Arc::new(move |args| {
                let asm = Arc::clone(&asm);
                bridge(move || tool_memory_search(asm, args))
            }))
        }
        "board_issue_query" => {
            let asm = Arc::clone(asm);
            // SQLite 本地只读查询，同步执行即可（不占异步桥）。
            Some(Arc::new(move |args| tool_board_issue_query(&asm, &args)))
        }
        _ => None,
    }
}

/// `run` 工具：headless 单任务，走装配出的同一个 agent loop
/// （`run_detached_events`——与 `nemesisbot run` / spawn 子代理同一条全治理
/// 链路；安全 8 层在 dispatch 前照常全跑）。
async fn tool_run(asm: Arc<McpServeAssembly>, args: Value) -> ToolCallResult {
    let Some(task) = args.get("task").and_then(|v| v.as_str()) else {
        return ToolCallResult::err("missing required argument: task (string)");
    };
    if task.trim().is_empty() {
        return ToolCallResult::err("task must not be empty");
    }
    let max_turns = args
        .get("max_turns")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32;
    let events = asm
        .agent_loop
        .run_detached_events(
            task,
            nemesis_agent::r#loop::DetachedOpts {
                depth: 0,
                max_turns,
                ..Default::default()
            },
        )
        .await;
    // 折叠语义与 `nemesisbot run` 同源（commands::run::fold_text：Done 优先）。
    match crate::commands::run::fold_text(&events) {
        (Some(m), _) => ToolCallResult::ok(m),
        (None, Some(e)) => ToolCallResult::err(format!("agent error: {e}")),
        (None, None) => ToolCallResult::err("agent produced no output"),
    }
}

/// `sessions_list` 工具：Dashboard `sessions.list` 同源扫描
/// （`nemesis_web::handlers::logs::scan_session_logs`），裁剪成紧凑字段。
async fn tool_sessions_list(asm: Arc<McpServeAssembly>, args: Value) -> ToolCallResult {
    let workspace = asm.workspace.to_string_lossy().to_string();
    let all = nemesis_web::handlers::logs::scan_session_logs(&workspace);
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .clamp(1, 200) as usize;
    let sessions: Vec<Value> = all
        .iter()
        .take(limit)
        .map(|s| {
            json!({
                "id": s.get("id"),
                "session_key": s.get("session_key"),
                "title": s.get("title"),
                "startTime": s.get("startTime"),
                "lastTime": s.get("lastTime"),
                "messageCount": s.get("messageCount"),
            })
        })
        .collect();
    let payload = json!({
        "total": all.len(),
        "shown": sessions.len(),
        "sessions": sessions,
    });
    ToolCallResult::ok(serde_json::to_string_pretty(&payload).unwrap_or_default())
}

/// `memory_search` 工具：enhanced memory 开 = MemoryToolExecutor 同源路径
/// （agent 的 memory_search 工具同一实现）；否则/之外恒并做
/// `workspace/memory/` 轻量文本检索（MEMORY.md 等静态面不在向量库里，
/// 两路信息互补）。
async fn tool_memory_search(asm: Arc<McpServeAssembly>, args: Value) -> ToolCallResult {
    let Some(query) = args.get("query").and_then(|v| v.as_str()) else {
        return ToolCallResult::err("missing required argument: query (string)");
    };
    if query.trim().is_empty() {
        return ToolCallResult::err("query must not be empty");
    }
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .clamp(1, 50) as usize;

    let mut sections: Vec<String> = Vec::new();

    // 同源 enhanced 路径（feature 裁剪或 config 关时跳过）。
    #[cfg(feature = "memory")]
    if let Some(exec) = &asm.memory_executor {
        let res = exec
            .execute("memory_search", &json!({"query": query, "limit": limit}))
            .await;
        if res.success {
            sections.push(res.content);
        } else {
            // 检索失败不静默：把错误如实带出，文件检索结果照常附后。
            sections.push(format!("[enhanced memory error] {}", res.content));
        }
    }

    // 轻量文本检索（恒做；静态 memory 面与向量库互补）。
    let hits = search_memory_files(&asm.workspace, query, limit);
    if hits.is_empty() {
        sections.push(format!(
            "No matches in workspace memory files (memory/*.md) for: {query}"
        ));
    } else {
        let body = hits
            .iter()
            .map(|h| format!("{}:{}: {}", h.file, h.line_no, h.line))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Workspace memory file hits:\n{body}"));
    }

    ToolCallResult::ok(sections.join("\n\n"))
}

/// 轻量文本检索单条命中（纯数据，便于单测）。
pub struct MemoryFileHit {
    /// 相对 workspace 的文件路径（`memory/MEMORY.md`）。
    pub file: String,
    /// 1-based 行号。
    pub line_no: usize,
    /// 行文本（截断到 300 字符）。
    pub line: String,
}

/// `workspace/memory/` 下 *.md 的大小写不敏感子串检索（递归，≤200 文件 /
/// 单文件 ≤1MB 防护）。目录不存在 = 空结果（不是错误——记忆面可以为空）。
pub fn search_memory_files(workspace: &Path, query: &str, limit: usize) -> Vec<MemoryFileHit> {
    let mut hits = Vec::new();
    let needle = query.to_lowercase();
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect_markdown_files(&workspace.join("memory"), 0, &mut files);
    for path in files {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(workspace)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().to_string());
        for (idx, line) in content.lines().enumerate() {
            if line.to_lowercase().contains(&needle) {
                hits.push(MemoryFileHit {
                    file: rel.clone(),
                    line_no: idx + 1,
                    line: line.trim().chars().take(300).collect(),
                });
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
    }
    hits
}

/// 递归收集 .md 文件（深度 ≤4、≤200 个——防劫持遍历）。
fn collect_markdown_files(dir: &Path, depth: usize, out: &mut Vec<std::path::PathBuf>) {
    if depth > 4 || out.len() >= 200 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if path.is_dir() {
            collect_markdown_files(&path, depth + 1, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            // 单文件 ≤1MB 防护；超限跳过（诚实边界：超大记忆文件不走轻量检索）。
            if ent
                .metadata()
                .map(|m| m.len() <= 1024 * 1024)
                .unwrap_or(false)
            {
                out.push(path);
            }
            if out.len() >= 200 {
                return;
            }
        }
    }
}

/// `board_issue_query` 工具（同步实现：本地 SQLite 只读查询）。
/// board feature 裁剪构建下诚实报错。
#[cfg(feature = "board")]
fn tool_board_issue_query(asm: &McpServeAssembly, args: &Value) -> ToolCallResult {
    use nemesis_board::models::{IssueFilter, IssueStatus};

    let db_path = asm.workspace.join("board").join("board.db");
    if !db_path.exists() {
        // 只读语义：库不存在 ≠ 错误——返回空集 + 注记，绝不顺手建库。
        return ToolCallResult::ok(
            serde_json::to_string_pretty(&json!({
                "issues": [],
                "note": "看板库不存在（尚无 issue——看板未初始化或未建过单）",
            }))
            .unwrap_or_default(),
        );
    }
    let store = match nemesis_board::BoardStore::open(&db_path, "NB") {
        Ok(s) => s,
        Err(e) => return ToolCallResult::err(format!("failed to open board store: {e}")),
    };

    // 单卡详情优先：number（NB-1 / 数字 id）→ issue + 评论。
    if let Some(number) = args
        .get("number")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let issue = match store.get_issue_by_number(number) {
            Ok(i) => i,
            Err(_) => {
                return ToolCallResult::err(format!(
                    "issue not found: {number}（支持编号 NB-1 或数字 id）"
                ));
            }
        };
        let comments: Vec<Value> = store
            .list_comments(issue.id)
            .unwrap_or_default()
            .iter()
            .map(|c| {
                json!({
                    "author": format!("{}/{}", c.author.kind, c.author.id),
                    "content": c.content,
                })
            })
            .collect();
        let payload = json!({
            "issue": issue_to_json(&issue),
            "description": issue.description,
            "comments": comments,
        });
        return ToolCallResult::ok(serde_json::to_string_pretty(&payload).unwrap_or_default());
    }

    // 列表视图：status / query 过滤 + limit 截断。
    let status = match args.get("status").and_then(|v| v.as_str()) {
        None => None,
        Some(s) if s.trim().is_empty() => None,
        Some(s) => match IssueStatus::from_str(s) {
            Some(st) => Some(st),
            None => {
                return ToolCallResult::err(format!(
                    "unknown status: {s}（可选 backlog/todo/in_progress/in_review/done/blocked/cancelled）"
                ));
            }
        },
    };
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .clamp(1, 200) as usize;
    let filter = IssueFilter {
        status,
        query: args
            .get("query")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        ..Default::default()
    };
    let issues = match store.list_issues(&filter) {
        Ok(v) => v,
        Err(e) => return ToolCallResult::err(format!("failed to list issues: {e}")),
    };
    let total = issues.len();
    let payload = json!({
        "total": total,
        "shown": total.min(limit),
        "issues": issues.iter().take(limit).map(issue_to_json).collect::<Vec<_>>(),
    });
    ToolCallResult::ok(serde_json::to_string_pretty(&payload).unwrap_or_default())
}

/// Issue → 紧凑 JSON（面向外部 MCP 客户端，不漏内部字段）。
#[cfg(feature = "board")]
fn issue_to_json(issue: &nemesis_board::Issue) -> Value {
    let assignee = match (&issue.assignee, &issue.assignee_id) {
        (Some(a), Some(id)) => json!(format!("{a}/{id}")),
        _ => Value::Null,
    };
    json!({
        "number": issue.number,
        "title": issue.title,
        "status": issue.status.to_string(),
        "priority": issue.priority,
        "assignee": assignee,
        "parent_issue_id": issue.parent_issue_id,
        "project_id": issue.project_id,
    })
}

/// board feature 裁剪构建：诚实报错（IoT / minimal 构建的既定语义）。
#[cfg(not(feature = "board"))]
fn tool_board_issue_query(_asm: &McpServeAssembly, _args: &Value) -> ToolCallResult {
    ToolCallResult::err("board subsystem is not compiled in this build (feature=board 裁剪)")
}

// ---------------------------------------------------------------------------
// 协议循环
// ---------------------------------------------------------------------------

/// 行是否为 JSON-RPC 通知（无 id 帧）。通知按 JSON-RPC 语义不回帧——
/// 不能把 `handle_raw` 对通知产生的 id=null 响应写上 stdout（协议噪声）。
/// 解析失败按「要回帧」处理（handle_raw 会回 -32700 parse error）。
fn is_notification(line: &str) -> bool {
    match serde_json::from_str::<Value>(line) {
        Ok(v) => v.is_object() && v.get("id").is_none(),
        Err(_) => false,
    }
}

/// `nemesisbot mcp-serve`：stdio 上跑 MCP server 直到 stdin EOF。
/// stdout 只承载协议帧；日志/装配横幅一律 stderr（ACP 同纪律）。
pub async fn run_server(home: std::path::PathBuf) -> Result<(), String> {
    // 会话日志平化迁移（SAN-01/D4）与 gateway / acp 同源（幂等 best-effort）
    // ——sessions_list 扫 session_logs，旧嵌套目录历史先平化再服务。
    nemesis_agent::chat_log::migrate_nested_session_logs();

    let asm = Arc::new(assemble(&home).await?);
    eprintln!(
        "mcp-serve: workspace={} security_pipeline={} (same-source K1 assembly)",
        asm.workspace.display(),
        asm.security_active
    );

    let mut server = McpServer::new(SERVER_NAME, crate::common::format_version());
    for def in tool_definitions() {
        let Some(handler) = make_handler(&def.name, &asm) else {
            return Err(format!("no handler for tool {}", def.name));
        };
        server
            .register_tool(def, handler)
            .map_err(|e| format!("register tool failed: {e}"))?;
    }

    let stdin = tokio::io::stdin();
    let mut lines = tokio::io::BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| format!("stdin read failed: {e}"))?
    {
        if line.trim().is_empty() {
            continue;
        }
        let notification = is_notification(&line);
        let resp = server.handle_raw(&line).await;
        if notification {
            continue;
        }
        // stdout 断开 = 客户端退出：安静收尾。
        if tokio::io::AsyncWriteExt::write_all(&mut stdout, format!("{resp}\n").as_bytes())
            .await
            .is_err()
        {
            break;
        }
        if tokio::io::AsyncWriteExt::flush(&mut stdout).await.is_err() {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
