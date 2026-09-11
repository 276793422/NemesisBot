//! WebSocket API handlers for all dashboard modules.
//!
//! Each module handler implements [`ModuleHandler`](crate::ws_router::ModuleHandler)
//! and is registered via [`register_all`]. Handlers are pure business logic and
//! transport-agnostic — they read/write configuration files and workspace data.

pub mod agent;
// M7（devtool-upgrade 阶段 5）：dashboard 审批卡 WSAPI（approval.respond /
// approval.pending）。无 feature 闸——responder 槽在 nemesis-types，未装配时
// handler 诚实报「未装配」。
pub mod approval;
pub mod board;
// Swarm M3（§5.4/D6）：看板资产下载端点（HTTP，非 WSAPI；token 即凭据）。
pub mod board_asset;
pub mod channels;
pub mod chat;
#[cfg(feature = "cluster")]
pub mod cluster;
#[cfg(feature = "cluster")]
pub mod cluster_persona_gen;
pub mod coding;
pub mod commands;
pub mod config;
pub mod estop;
#[cfg(feature = "forge")]
pub mod forge;
pub mod fs;
pub mod hooks;
pub mod identity;
pub mod logs;
pub mod mcp;
#[cfg(feature = "memory")]
pub mod memory;
pub mod models;
pub mod persona;
pub mod plugins;
// L6++ G4（2026-09-08）：项目注册表 WSAPI（projects.list/create/remove/
// rename）+ resolve_session_loop 会话归属解析。无 feature 闸——bridge 槽
// 未装配时 handler 诚实报「未装配」。
pub mod projects;
// F7（devtool-upgrade 阶段 5）：dashboard 提问卡 WSAPI（question.respond /
// question.pending）。无 feature 闸——responder 槽在 nemesis-types，未装配时
// handler 诚实报「未装配」。
pub mod question;
#[cfg(feature = "sandbox")]
pub mod sandbox;
#[cfg(feature = "security")]
pub mod scanner;
#[cfg(feature = "security")]
pub mod security;
pub mod sessions;
pub mod skills;
pub mod system;
pub mod tasks;
pub mod tools;
pub mod upload;
#[cfg(feature = "voice")]
pub mod voice;
#[cfg(feature = "workflow")]
pub mod workflow;

// Phase 3 覆盖率（2026-08-25）：sessions 各命令缺参 bail 臂。
// create/rename happy path 不测——chat_log::write_session_meta 走
// default_path_manager() 进程级 OnceLock 单例 home，会写真实 ~/.nemesisbot。
#[cfg(test)]
mod sessions_extra_tests;

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): sessions
// success arms (list prefix-strip, clear/delete/export) — create/rename
// happy paths stay excluded per the note above.
#[cfg(test)]
mod sessions_s10b_tests;

// L4 会话分享（2026-09-07）：share_create/share_list/share_revoke 三命令
// 的 handler 层测试（存储语义在 crate::share 单元测试里）。
#[cfg(test)]
mod sessions_share_tests;

// L6++ G4（2026-09-08）：projects WSAPI 全命令臂（含未装配诚实报错）+
// resolve_session_loop 两分支 + 项目根切换。
#[cfg(test)]
mod projects_tests;

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): shared path/file
// utility arms (absolute/traversal rejection, canonicalize fallback, atomic
// write fallback) + ConfigHandler error arms and CORS stubs.
#[cfg(test)]
mod config_s10b_tests;
#[cfg(test)]
mod s10b_tests;

// M5 (2026-09-05, devtool-upgrade 阶段 3): 会话级用量 WSAPI——
// logs.session_usage / backfill_session_usage / chat.context_status。
#[cfg(test)]
mod m5_session_usage_tests;

// P2B (2026-09-12, NB-15 根修配套): models.health 工具健康视图——
// 阈值建议 / 窗口夹取 / 账本未装配诚实空。
#[cfg(test)]
mod models_health_tests;

// L1 (2026-09-06, devtool-upgrade 阶段 6): WSAPI 命令注册表——结构不变量 +
// system.commands dispatch 链路 + docs/INFO/wsapi-commands.md 文档生成 +
// 安全子集 dispatch 冒烟。
#[cfg(test)]
mod l1_tests;

// L2 (2026-09-06, devtool-upgrade 阶段 6): chat.sync 断线补拉 dispatch 测试
// （record → sync 补拉 → 窗口/gap 语义；不依赖 AgentLoop）。
#[cfg(test)]
mod chat_sync_tests;

use std::path::PathBuf;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// L1（devtool-upgrade 阶段 6）：WSAPI 命令注册表快照——`register_all`
/// 末尾一次性发布，`system.commands` 与文档生成读取。
///
/// 设计偏差（vs 计划原文「AppState commands_registry 字段」）：AppState
/// 字面量散布 69 个测试文件 105 处（M7 已核实不可动），新增字段是全库
/// 爆破——OnceLock 模块级快照保有同一「单一真相源」性质（register_all
/// 是唯一写点）且零爆破半径。
static COMMANDS_REGISTRY: std::sync::OnceLock<Vec<(String, Vec<&'static str>)>> =
    std::sync::OnceLock::new();

/// L1：已发布的命令注册表（module → 命令清单，注册顺序）。`register_all`
/// 尚未跑过时返回空切片（headless 早期诚实空表）。
pub fn commands_registry() -> &'static [(String, Vec<&'static str>)] {
    COMMANDS_REGISTRY.get().map(|v| v.as_slice()).unwrap_or(&[])
}

/// Register all module handlers with the given router.
pub fn register_all(router: &mut crate::ws_router::WsRouter) {
    router.register(Arc::new(system::SystemHandler));
    router.register(Arc::new(estop::EstopHandler));
    // M7：审批响应端点（dashboard 审批卡 → WebApprovalManager）。
    router.register(Arc::new(approval::ApprovalHandler));
    // F7：提问响应端点（dashboard 提问卡 → WebQuestionBroker）。
    router.register(Arc::new(question::QuestionHandler));
    router.register(Arc::new(chat::ChatHandler));
    router.register(Arc::new(config::ConfigHandler::new()));
    router.register(Arc::new(models::ModelsHandler::new()));
    router.register(Arc::new(channels::ChannelsHandler::new()));
    router.register(Arc::new(identity::IdentityHandler));
    router.register(Arc::new(tools::ToolsHandler));
    #[cfg(feature = "security")]
    {
        router.register(Arc::new(scanner::ScannerHandler::new()));
    }
    #[cfg(feature = "memory")]
    {
        router.register(Arc::new(memory::MemoryHandler));
    }
    router.register(Arc::new(skills::SkillsHandler::new()));
    router.register(Arc::new(mcp::McpHandler::new()));
    #[cfg(feature = "security")]
    {
        router.register(Arc::new(security::SecurityHandler::new()));
    }
    #[cfg(feature = "sandbox")]
    {
        router.register(Arc::new(sandbox::SandboxHandler::new()));
    }
    #[cfg(feature = "forge")]
    {
        router.register(Arc::new(forge::ForgeHandler::new()));
    }
    router.register(Arc::new(tasks::TasksHandler));
    // P2-1 (2026-08-24 UI entry gap): 「代码开发」页 read-only status commands
    // (LSP PATH probe + tool config sections). No feature gate — nemesis-lsp
    // is an unconditional agent dependency (lsp tool itself is config-gated).
    router.register(Arc::new(coding::CodingHandler));
    // P4 (2026-08-24 UI entry gap): 设置页「Hooks」Tab — hooks.json 读写
    // (hooks.json 方言, nemesis-agent cc_hooks)。No feature gate — cc_hooks 无条件编译。
    router.register(Arc::new(hooks::HooksHandler));
    // 2026-08-29: 自定义 slash 命令表（快捷提示词发送器）— CommandsView CRUD。
    // AgentLoop 侧 mtime 热重载同一文件，无需重启。No feature gate。
    router.register(Arc::new(commands::CommandsHandler));
    // I2 (2026-09-05, devtool-upgrade 阶段 3): 聊天 `@` 文件引用的路径补全
    // 后端（fs.complete_path）。No feature gate — 遍历只读 + 有界（deadline）。
    router.register(Arc::new(fs::FsHandler::new()));
    // 2026-08-29: 插件状态总览（只读）— PluginsView 数据源。No feature gate
    // （探测逻辑无条件编译；onnx 能力状态节内含 memory cfg 门控）。
    router.register(Arc::new(plugins::PluginsHandler));
    // W2 P1 (2026-08-31): managed-agent 看板。No feature gate — nemesis-board
    // 是 nemesis-web 无条件依赖（cron 先例）；store 未注入时命令统一报
    // "board service not available"（gateway 仅在 board feature 开启时注入）。
    router.register(Arc::new(board::BoardHandler));
    #[cfg(feature = "cluster")]
    {
        router.register(Arc::new(cluster::ClusterHandler::new()));
    }
    router.register(Arc::new(logs::LogsHandler));
    router.register(Arc::new(agent::AgentHandler));
    #[cfg(feature = "voice")]
    {
        router.register(Arc::new(voice::VoiceHandler::new()));
    }
    router.register(Arc::new(persona::PersonaHandler::new()));
    router.register(Arc::new(sessions::SessionsHandler));
    // L6++ G4（2026-09-08）：项目注册表 WSAPI（bridge 未装配时诚实报错）。
    router.register(Arc::new(projects::ProjectsHandler));
    #[cfg(feature = "workflow")]
    {
        router.register(Arc::new(workflow::WorkflowHandler));
    }

    // L1：注册完成后发布命令注册表快照（OnceLock 首写胜出——重复注册
    // 内容一致，静默忽略后续 set 失败）。
    let _ = COMMANDS_REGISTRY.set(router.commands_registry());
}

// ---------------------------------------------------------------------------
// Shared utilities
// ---------------------------------------------------------------------------

/// Mask a sensitive string, showing only the first 4 and last 4 characters.
pub fn mask_sensitive(value: &str) -> String {
    if value.len() <= 8 {
        return "****".to_string();
    }
    // floor/ceil to char boundary — passwords may contain multibyte chars.
    let end = nemesis_types::utils::floor_char_boundary(value, 4);
    let start = nemesis_types::utils::ceil_char_boundary(value, value.len() - 4);
    format!("{}****{}", &value[..end], &value[start..])
}

/// Check whether a field name is considered sensitive and should be masked.
pub fn is_sensitive_field(field_name: &str) -> bool {
    matches!(
        field_name.to_lowercase().as_str(),
        "api_key"
            | "token"
            | "secret"
            | "password"
            | "auth_token"
            | "app_secret"
            | "encrypt_key"
            | "access_token"
            | "bot_token"
            | "app_token"
            | "client_secret"
    )
}

/// Resolve a path relative to the workspace, preventing path traversal.
pub fn resolve_path(workspace: &str, relative: &str) -> Result<PathBuf, String> {
    // Reject paths that look absolute (drive letter or leading slash)
    let rel_path = PathBuf::from(relative);
    if rel_path.is_absolute() || relative.starts_with('/') || relative.starts_with('\\') {
        return Err("absolute paths not allowed".to_string());
    }

    let base = PathBuf::from(workspace);
    let resolved = base.join(relative);

    // Quick string check: resolved should start with base (catches drive-root escapes on Windows)
    let base_str = base.to_string_lossy();
    let resolved_str = resolved.to_string_lossy();
    if !resolved_str.starts_with(base_str.as_ref()) {
        return Err("path traversal denied".to_string());
    }

    // Canonicalize both paths for accurate comparison (handles .., symlinks, etc.)
    let canonical_base = base.canonicalize().unwrap_or_else(|_| base.clone());
    let canonical_resolved = match resolved.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            // File doesn't exist yet — the string check above already caught most traversal
            if relative.contains("..") {
                return Err("path traversal denied".to_string());
            }
            return Ok(resolved);
        }
    };

    if !canonical_resolved.starts_with(&canonical_base) {
        return Err("path traversal denied".to_string());
    }
    Ok(resolved)
}

/// Extract a required string field from a JSON value.
pub fn get_str(data: &serde_json::Value, field: &str) -> Result<String, String> {
    data.get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing field: {}", field))
}

/// Extract an optional string field from a JSON value.
pub fn get_opt_str(data: &serde_json::Value, field: &str) -> Option<String> {
    data.get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract an optional bool field, loud on wrong types: key absent or null →
/// None (leave unchanged); present but not a bool → Err. Project convention
/// (cf. sandbox `set_config`): a present-but-mistyped key must fail visibly
/// instead of being silently ignored — the frontend bug surfaces in the same
/// turn rather than as "the toggle does nothing".
pub fn get_opt_bool_loud(data: &serde_json::Value, field: &str) -> Result<Option<bool>, String> {
    match data.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("field '{}' must be a bool (got {v})", field)),
    }
}

/// Same loud contract as [`get_opt_bool_loud`] for unsigned integers.
pub fn get_opt_u64_loud(data: &serde_json::Value, field: &str) -> Result<Option<u64>, String> {
    match data.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("field '{}' must be a non-negative integer (got {v})", field)),
    }
}

/// Same loud contract as [`get_opt_bool_loud`] for strings.
pub fn get_opt_str_loud(data: &serde_json::Value, field: &str) -> Result<Option<String>, String> {
    match data.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v
            .as_str()
            .map(|s| s.to_string())
            .map(Some)
            .ok_or_else(|| format!("field '{}' must be a string (got {v})", field)),
    }
}

/// Read a text file from the workspace.
pub fn read_workspace_file(workspace: &str, relative: &str) -> Result<String, String> {
    let path = resolve_path(workspace, relative)?;
    std::fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {}", relative, e))
}

/// Write a text file to the workspace (atomic write via tmp + rename).
pub fn write_workspace_file(workspace: &str, relative: &str, content: &str) -> Result<(), String> {
    let path = resolve_path(workspace, relative)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create dir: {}", e))?;
    }
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, content).map_err(|e| format!("failed to write tmp: {}", e))?;
    if let Err(e) = std::fs::rename(&tmp_path, &path) {
        let _ = std::fs::remove_file(&tmp_path);
        std::fs::write(&path, content).map_err(|e| format!("failed to write file: {}", e))?;
        tracing::warn!(error = %e, "[WebServer] Atomic rename failed, fell back to direct write");
    }
    Ok(())
}

/// List files in a workspace directory, returning relative paths.
pub fn list_workspace_dir(workspace: &str, relative: &str) -> Result<Vec<String>, String> {
    let dir = resolve_path(workspace, relative)?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(&dir).map_err(|e| format!("failed to read dir: {}", e))?;
    for entry in read_dir {
        let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
        if let Some(name) = entry.file_name().to_str() {
            entries.push(name.to_string());
        }
    }
    entries.sort();
    Ok(entries)
}

/// Get workspace path from context or return error.
pub fn require_workspace(ctx: &crate::ws_router::RequestContext) -> Result<&str, String> {
    ctx.workspace
        .as_deref()
        .ok_or_else(|| "workspace not configured".to_string())
}

/// Get home directory from context or return error.
pub fn require_home(ctx: &crate::ws_router::RequestContext) -> Result<&str, String> {
    ctx.home
        .as_deref()
        .ok_or_else(|| "home not configured".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "workflow", feature = "security"))]
mod tests;

// Previously-written extra/more coverage modules were never `mod`-declared, so
// they silently did not compile or run. Wire them in here.
// （workflow 也在门控集里：测试体引用 nemesis_workflow / handlers::workflow，
// 只开 cluster 不开 workflow 编译时 E0433——2026-09-02 本地 --features cluster
// 单独验证时实录；nemesisbot 透传两者恒同开所以从未暴露。）
#[cfg(all(test, feature = "cluster", feature = "workflow"))]
mod cluster_extra_tests;
#[cfg(all(test, feature = "cluster"))]
mod cluster_more_tests;
// P3-web3 (2026-08-25): cluster.rs deep coverage — runtime metrics, real TCP
// ping probes, nodes.refresh full arms, tasks log enrichment, topology real
// connections, config fallbacks, firewall AddrInUse, persona_generate/apply.
// H1/H2 (2026-09-05): chat.todo_get 读路径同构性测试。
#[cfg(test)]
mod chat_todo_tests;
// F1 (2026-09-05): chat.set_mode / chat.get_mode（plan/build 双模式）测试。
#[cfg(test)]
mod chat_mode_tests;
#[cfg(all(test, feature = "cluster"))]
mod cluster_deep_tests;
#[cfg(all(test, feature = "forge"))]
mod forge_extra_tests;
#[cfg(all(test, feature = "memory"))]
mod memory_extra_tests;
#[cfg(all(test, feature = "workflow"))]
mod persona_extra_tests;
#[cfg(all(test, feature = "workflow"))]
mod persona_more_tests;
#[cfg(all(test, feature = "security"))]
mod scanner_extra_tests;
#[cfg(all(test, feature = "security"))]
mod scanner_more_tests;
#[cfg(all(test, feature = "workflow"))]
mod skills_extra_tests;
#[cfg(all(test, feature = "workflow"))]
mod skills_more_tests;
#[cfg(all(test, feature = "voice"))]
mod voice_extra_tests;
