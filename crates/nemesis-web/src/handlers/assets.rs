//! 资产盘点 + AI-BOM（W1 安全图谱批 #11+#6–10，2026-11-19）。
//!
//! gateway 全局资产清单（对照评估 #6–#10 + #11）：**实时聚合视图**——
//! 每次调用现场收集各既有真相源（工具注册表 / MCP 配置 / skills 目录 +
//! lockfile / WASM 插件三源合流 / 集群节点注册表 / subagent 注册表 /
//! model_list / memory 统计 / WSAPI 命令注册表 / 会话数），组装单一
//! inventory 文档；BOM（`bom_view`）是同一文档的投影（五类组件：框架 /
//! 模型 / 技能 / 工具插件 / MCP，各带 version/source/hash/signature，
//! 不可得字段诚实 null——不编造）。零新增存储真相源：快照
//! `<workspace>/logs/assets/inventory.json` 只是投影落盘（gateway 周期
//! 刷新 + WSAPI 调用即刷新 + skills 安装成功即刷新），供离线消费
//! （`nemesisbot assets bom` CLI）与审计留痕。
//!
//! 诚实边界：
//! - 缺省态（无插件 / 无 MCP / 集群未装配 / agent 未运行）= 诚实空清单
//!   + note 注记，不谎报不炸（门禁要求）。
//! - **无秘密**：模型行只带 api_base（origin），api_key 永不出现在
//!   inventory；MCP 行复用 `mcp.servers` 的掩码投影（headers/env 已脱敏）。
//! - WASM 插件安装面无 ctx（全局静态 installer），装后快照由 gateway
//!   周期刷新收编（默认 300s）；skills 安装（有 ctx）成功即刷。
//! - 全局聚合非 per-会话（会话级资源盘点归 `session.resources`）。

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use serde_json::{Value, json};
use std::path::PathBuf;

pub struct AssetsHandler;

/// inventory 快照落盘路径（单一拼接点）。
pub(crate) fn snapshot_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace)
        .join("logs")
        .join("assets")
        .join("inventory.json")
}

/// 构建完整 inventory 文档（同步聚合——全部数据源都是本地读，
/// gateway 周期刷新侧走 spawn_blocking 调本函数）。
pub fn build_inventory(
    state: &crate::api_handlers::AppState,
    workspace: &str,
    home: Option<&str>,
) -> Value {
    let generated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let models = home
        .and_then(|h| crate::handlers::config::load_config(h).ok())
        .map(|cfg| {
            // 默认模型权威 = agents.defaults.llm（models.list 同源口径；
            // 空 = 无显式默认，全部 false——不沿用位置默认假装有）。
            let default_llm = cfg.agents.defaults.llm.clone();
            cfg.model_list
                .iter()
                .map(|m| {
                    // api_key 永不出现在 inventory（秘密面红线）。
                    json!({
                        "model": m.model,
                        "provider_name": m.model_name,
                        "api_base": m.api_base,
                        "protocol": m.protocol,
                        "role": m.role,
                        "reasoning_effort": m.reasoning_effort,
                        "default": !default_llm.is_empty()
                            && (m.model_name == default_llm || m.model == default_llm),
                        "model_tier": m.extra.get("model_tier").cloned().unwrap_or(Value::Null),
                        "vision": m.extra.get("vision").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let model_count = models.len();

    // 工具注册表（agent 运行中才有；缺席 = 诚实空 + note）。
    let tools = match state.agent_loop.read().clone() {
        Some(al) => {
            let registry = al.tools();
            let rows: Vec<Value> = registry
                .iter()
                .map(|(name, tool)| {
                    json!({
                        "name": name,
                        "kind": if name.starts_with("plugin.") { "plugin" } else { "builtin" },
                        "min_tier": tool.min_tier(),
                    })
                })
                .collect();
            json!({ "count": rows.len(), "rows": rows, "note": Value::Null })
        }
        None => json!({
            "count": 0, "rows": [],
            "note": "agent 未运行（工具注册表不可见）",
        }),
    };

    // MCP 配置面（复用 mcp handler 的掩码投影——headers/env 已脱敏；
    // 这里只取身份字段，双重不带秘密）。
    let mcp = match crate::handlers::mcp::McpHandler::new().servers(workspace) {
        Ok(Some(v)) => {
            let servers = v
                .get("servers")
                .and_then(|s| s.as_array())
                .cloned()
                .unwrap_or_default();
            let rows: Vec<Value> = servers
                .iter()
                .map(|s| {
                    json!({
                        "name": s.get("name"),
                        "transport_type": s.get("transport_type"),
                        "url": s.get("url"),
                        "description": s.get("description"),
                    })
                })
                .collect();
            json!({ "count": rows.len(), "servers": rows, "note": Value::Null })
        }
        Ok(None) => json!({ "count": 0, "servers": [], "note": Value::Null }),
        Err(e) => json!({ "count": 0, "servers": [], "note": e }),
    };

    // skills：workspace 目录扫描 ∪ lockfile 记账（来源/信任态只认
    // lockfile；目录有而 lockfile 无 = workspace-local 手工放置）。
    let skills = build_skills_section(workspace);

    // WASM 插件（feature 关 / installer 未装配 = 诚实空 + note）。
    #[cfg(feature = "plugins-wasm")]
    let plugins_wasm = match crate::handlers::plugins_wasm::cmd_list() {
        Ok(v) => {
            let plugins = v
                .get("plugins")
                .and_then(|p| p.as_array())
                .cloned()
                .unwrap_or_default();
            json!({ "count": plugins.len(), "plugins": plugins, "note": Value::Null })
        }
        Err(e) => json!({ "count": 0, "plugins": [], "note": e }),
    };
    #[cfg(not(feature = "plugins-wasm"))]
    let plugins_wasm = json!({
        "count": 0, "plugins": [],
        "note": "plugins-wasm feature 未编译",
    });

    // 集群节点注册表（feature 关 / 未装配 = 诚实空 + note）。
    #[cfg(feature = "cluster")]
    let cluster = match state.cluster.as_ref() {
        Some(c) => {
            let nodes = c.list_nodes();
            let rows: Vec<Value> = nodes
                .iter()
                .map(|n| {
                    json!({
                        "id": n.base.id,
                        "name": n.base.name,
                        "role": n.base.role,
                        "category": n.base.category,
                        "status": format!("{:?}", n.status),
                        "tier": n.tier,
                        "professions": n.professions,
                    })
                })
                .collect();
            json!({ "count": rows.len(), "nodes": rows, "note": Value::Null })
        }
        None => json!({ "count": 0, "nodes": [], "note": "集群未装配" }),
    };
    #[cfg(not(feature = "cluster"))]
    let cluster = json!({ "count": 0, "nodes": [], "note": "cluster feature 未编译" });

    // subagent 注册表（全局静态槽，恒可用）。
    let sub_tasks = nemesis_agent::subagent_registry::global().list();
    let sub_rows: Vec<Value> = sub_tasks
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "status": t.status,
                "running": !t.status.is_terminal(),
            })
        })
        .collect();
    let sub_running = sub_rows
        .iter()
        .filter(|r| r.get("running").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();

    // 会话数（session_logs jsonl 文件数——目录缺席 = 0，诚实缺省）。
    let sessions_count =
        std::fs::read_dir(PathBuf::from(workspace).join("logs").join("session_logs"))
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
                    .count()
            })
            .unwrap_or(0);

    // memory 统计（feature 门控；复用 memory handler 的 stats 投影）。
    // config_dir 与 memory.rs WSAPI 主路径同源（nemesis_path::
    // workspace_config_dir —— <workspace>/config 单一真相源）。此前手搓
    // home.join("config") 是错层路径（<home>/config ≠ <home>/workspace/config，
    // BUG-20261122-042）：生产 home 恒 Some（web_init.rs 装配），每次资产页
    // 查询 / skills 安装刷新都会在 <home>/config 下 load-or-create 默认
    // embedding 配置 = 错位置写盘 + 统计读错文件；home=None 臂更落
    // Path::new("")（相对进程 cwd 落盘——crate 源码树被测试进程污染的实锤
    // 现场）。workspace 参数就在手边，直接同源，home 不参与本段。
    #[cfg(feature = "memory")]
    let memory = {
        let config_dir = nemesis_path::workspace_config_dir(std::path::Path::new(workspace));
        match crate::handlers::memory::MemoryHandler.stats(&config_dir, workspace) {
            Ok(Some(mut v)) => {
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("enabled".into(), json!(state.memory_manager.is_some()));
                }
                v
            }
            _ => json!({ "enabled": state.memory_manager.is_some(), "note": "统计不可用" }),
        }
    };
    #[cfg(not(feature = "memory"))]
    let memory = json!({ "enabled": false, "note": "memory feature 未编译" });

    // WSAPI 命令注册表（L1 单一数据源；router 未装配 = 空 + note）。
    let wsapi = match state.ws_router.as_ref() {
        Some(router) => {
            let reg = router.commands_registry();
            let command_total: usize = reg.iter().map(|(_, cmds)| cmds.len()).sum();
            json!({
                "modules": reg.len(),
                "commands": command_total,
                "registry": reg.iter().map(|(m, c)| json!({ "module": m, "commands": c })).collect::<Vec<_>>(),
                "note": Value::Null,
            })
        }
        None => json!({ "modules": 0, "commands": 0, "registry": [], "note": "router 未装配" }),
    };

    json!({
        "generated_at": generated_at,
        "bot_version": state.version,
        "workspace": workspace,
        "framework": [{ "name": "nemesisbot", "version": state.version, "source": "builtin" }],
        "models": { "count": model_count, "rows": models },
        "tools": tools,
        "mcp": mcp,
        "skills": skills,
        "plugins_wasm": plugins_wasm,
        "cluster": cluster,
        "subagents": { "count": sub_rows.len(), "running": sub_running, "rows": sub_rows },
        "sessions": { "count": sessions_count },
        "memory": memory,
        "wsapi": wsapi,
    })
}

/// skills 段：目录扫描 ∪ lockfile 记账合并。read_dir 结果显式排序
/// （NTFS/ext4 顺序是平台实现细节）。
fn build_skills_section(workspace: &str) -> Value {
    let skills_dir = PathBuf::from(workspace).join("skills");
    let lockfile = nemesis_skills::lockfile::SkillsLockfile::load(std::path::Path::new(workspace));

    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if skills_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&skills_dir)
    {
        for entry in entries.flatten() {
            if entry.path().is_dir()
                && let Some(n) = entry.path().file_name().and_then(|n| n.to_str())
            {
                names.insert(n.to_string());
            }
        }
    }
    for slug in lockfile.skills.keys() {
        // lockfile 记账可能先于目录落位（载荷被手工删除的可见性兜底）——
        // 同 wasm.list 三源合流纪律。
        names.insert(slug.clone());
    }

    let rows: Vec<Value> = names
        .iter()
        .map(|name| {
            let desc = {
                let md = skills_dir.join(name).join("SKILL.md");
                if md.is_file() {
                    nemesis_skills::loader::get_skill_metadata(&md)
                        .map(|m| m.description)
                        .unwrap_or_default()
                } else {
                    String::new()
                }
            };
            match lockfile.skills.get(name) {
                Some(entry) => json!({
                    "name": name,
                    "description": desc,
                    "source": entry.source,
                    "commit": entry.commit,
                    "trust_state": entry.verified_state,
                    "files_hashed": entry.files.len(),
                    "installed_at": entry.installed_at,
                }),
                None => json!({
                    "name": name,
                    "description": desc,
                    "source": "workspace-local",
                    "commit": Value::Null,
                    "trust_state": Value::Null,
                    "files_hashed": 0,
                    "installed_at": Value::Null,
                }),
            }
        })
        .collect();

    json!({ "count": rows.len(), "rows": rows, "note": Value::Null })
}

/// AI-BOM 投影（#11）：inventory 的五类组件视图。各组件带
/// version/source/hash/signature；**不可得字段诚实 null**（模型无文件
/// hash、MCP 无签名面——不编造字段值）。
pub fn bom_view(inventory: &Value) -> Value {
    let models_rows: Vec<Value> = inventory
        .pointer("/models/rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            json!({
                "name": m.get("model"),
                "version": Value::Null, // 模型 id 本身即身份；无独立版本字段
                "source": m.get("api_base"),
                "hash": Value::Null,
                "signature": Value::Null,
            })
        })
        .collect();
    let framework_rows: Vec<Value> = inventory
        .get("framework")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|f| {
            json!({
                "name": f.get("name"),
                "version": f.get("version"),
                "source": f.get("source"),
                "hash": Value::Null,
                "signature": Value::Null,
            })
        })
        .collect();
    let skill_rows: Vec<Value> = inventory
        .pointer("/skills/rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| {
            // hash = 锁定的 commit SHA（供应链 pin；workspace-local 无 = null）。
            json!({
                "name": s.get("name"),
                "version": Value::Null,
                "source": s.get("source"),
                "hash": s.get("commit"),
                "signature": s.get("trust_state"),
            })
        })
        .collect();
    let plugin_rows: Vec<Value> = inventory
        .pointer("/plugins_wasm/plugins")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|p| {
            json!({
                "name": p.get("slug"),
                "version": p.get("version"),
                "source": "plugins_wasm",
                "hash": Value::Null,
                "signature": p.get("trust"),
            })
        })
        .collect();
    let mcp_rows: Vec<Value> = inventory
        .pointer("/mcp/servers")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| {
            json!({
                "name": s.get("name"),
                "version": Value::Null,
                "source": format!(
                    "{} {}",
                    s.get("transport_type").and_then(|v| v.as_str()).unwrap_or("stdio"),
                    s.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                )
                .trim()
                .to_string(),
                "hash": Value::Null,
                "signature": Value::Null,
            })
        })
        .collect();

    json!({
        "generated_at": inventory.get("generated_at").cloned().unwrap_or(Value::Null),
        "bot_version": inventory.get("bot_version").cloned().unwrap_or(Value::Null),
        "components": [
            { "category": "framework", "count": framework_rows.len(), "rows": framework_rows },
            { "category": "model", "count": models_rows.len(), "rows": models_rows },
            { "category": "skill", "count": skill_rows.len(), "rows": skill_rows },
            { "category": "tool_plugin", "count": plugin_rows.len(), "rows": plugin_rows },
            { "category": "mcp", "count": mcp_rows.len(), "rows": mcp_rows },
        ],
    })
}

/// 快照落盘（原子写；目录惰性创建）。返回落盘路径。
pub fn write_snapshot(workspace: &str, inventory: &Value) -> Result<String, String> {
    let path = snapshot_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 assets 目录失败: {}", e))?;
    }
    let data = serde_json::to_string_pretty(inventory)
        .map_err(|e| format!("inventory 序列化失败: {}", e))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), data.as_bytes(), 0o644)
        .map_err(|e| format!("inventory 快照写入失败: {}", e))?;
    Ok(path.to_string_lossy().to_string())
}

/// 构建落盘一体化（gateway 周期刷新 / WSAPI 共用入口）。返回快照路径。
pub fn build_and_write_snapshot(
    state: &crate::api_handlers::AppState,
    workspace: &str,
    home: Option<&str>,
) -> Result<String, String> {
    let inventory = build_inventory(state, workspace, home);
    write_snapshot(workspace, &inventory)
}

/// 安装事件刷新钩（fire-and-forget）：skills 安装成功后调用——后台
/// 重建 + 落盘快照，失败只 WARN（快照是投影，非真相源，不阻塞主路径）。
pub(crate) fn spawn_snapshot_refresh(ctx: &RequestContext) {
    let Some(workspace) = ctx.workspace.clone() else {
        return;
    };
    let state = ctx.state.clone();
    let home = ctx.home.clone();
    tokio::spawn(async move {
        let ws = workspace.clone();
        let result = tokio::task::spawn_blocking(move || {
            build_and_write_snapshot(&state, &ws, home.as_deref())
        })
        .await;
        match result {
            Ok(Ok(path)) => {
                tracing::debug!("[Assets] inventory snapshot refreshed: {path}");
            }
            Ok(Err(e)) => {
                tracing::warn!("[Assets] inventory snapshot refresh failed: {e}");
            }
            Err(e) => {
                tracing::warn!("[Assets] inventory snapshot refresh task failed: {e}");
            }
        }
    });
}

#[async_trait::async_trait]
impl ModuleHandler for AssetsHandler {
    fn module_name(&self) -> &str {
        "assets"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["inventory", "bom"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        _data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "inventory" => {
                let doc = build_inventory(&ctx.state, workspace, ctx.home.as_deref());
                // 调用即刷新快照（best-effort；失败注记不阻断返回）。
                let snapshot_note = write_snapshot(workspace, &doc).err();
                let mut out = doc;
                if let Some(note) = snapshot_note
                    && let Some(obj) = out.as_object_mut()
                {
                    obj.insert("snapshot_note".into(), json!(note));
                }
                Ok(Some(out))
            }
            "bom" => {
                let doc = build_inventory(&ctx.state, workspace, ctx.home.as_deref());
                let snapshot_note = write_snapshot(workspace, &doc).err();
                let mut out = bom_view(&doc);
                if let Some(note) = snapshot_note
                    && let Some(obj) = out.as_object_mut()
                {
                    obj.insert("snapshot_note".into(), json!(note));
                }
                Ok(Some(out))
            }
            _ => Err(format!("unknown command: assets.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
