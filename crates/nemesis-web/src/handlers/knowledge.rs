//! 知识库重定向（天工系统 v1，2026-11-26）handler — `knowledge.get` /
//! `knowledge.set`。
//!
//! 程序不理解「知识库」：用户配置声明（enabled + code_root/info_root 配
//! 对），程序机械执行（TIANGONG 存在性声明注入 + read/write/edit/multiedit
//! 四工具路径重定向）；LLM 从声明与 TIANGONG 内容获得全部语义。
//!
//! 配置文件按**实体路由**（与 `resolve_session_loop` 三分同款判定，但此处
//! 不需要 loop Arc——只定位 config 路径）：
//! - 主 loop：`<workspace>/config/config.knowledge.json`
//! - selfbuilt agent（session_key 形如 `agents:{id}:session:{sid}`）：
//!   `<workspace>/agents/{id}/home/config/config.knowledge.json`（正规落点）
//! - 项目会话：v1 不支持（诚实报错；BUG 在案）
//!
//! 写盘即热生效（`KnowledgeHandle` mtime 缓存——下一次工具调用 / 下一轮
//! build_messages 自动重读，无需重启）。围栏放行根（boundary.extra_roots）
//! 注册期烧入 = **重启网关生效**（前端设置窗注明）。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_config::knowledge::{KnowledgeConfig, load_knowledge_config, save_knowledge_config};
use std::path::PathBuf;

/// 单一裁决：session_id 归属 + 对应 config 路径。三分与
/// `resolve_session_loop` 同款顺序（agents 在先——纯字符串解析键空间不
/// 相交；projects bridge 次之；主槽兜底）。返回 (scope 标签, config 路径)，
/// scope ∈ {"main","agent"}——项目归属在函数内诚实报错，不产生 scope。
/// workspace 缺失 = headless 早期/测试形态，诚实报错。
fn resolve_scope(
    ctx: &RequestContext,
    session_id: Option<&str>,
) -> Result<(&'static str, PathBuf), String> {
    let workspace = ctx
        .workspace
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or("workspace not available")?;
    let ws = std::path::Path::new(workspace);

    // 1. selfbuilt agent 臂（agents::agent_owner_of 纯字符串解析）。
    if let Some(sid) = session_id
        && let Some(agent_id) = crate::handlers::agents::agent_owner_of(sid)
    {
        let path = nemesis_path::resolve_agent_knowledge_config_path_in_workspace(ws, &agent_id);
        return Ok(("agent", path));
    }

    // 2. 项目会话臂：归属命中 = v1 不支持（诚实报错，不静默落主槽——
    //    写错配置文件比报错更糟）。
    if let Some(sid) = session_id
        && let Some(bridge) = crate::handlers::projects::projects_bridge()
        && bridge.owner_of(sid).is_some()
    {
        return Err("项目会话暂不支持知识库配置（v1 覆盖面 = 主对话与自建 agent）".to_string());
    }

    // 3. 主 loop 臂。
    Ok((
        "main",
        nemesis_path::resolve_workspace_knowledge_config_path_in_workspace(ws),
    ))
}

pub struct KnowledgeHandler;

#[async_trait::async_trait]
impl ModuleHandler for KnowledgeHandler {
    fn module_name(&self) -> &str {
        "knowledge"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["get", "set"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "get" => {
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str());
                let (scope_label, config_path) = resolve_scope(ctx, session_id)?;
                let config = load_knowledge_config(&config_path);
                Ok(Some(serde_json::json!({
                    "scope": scope_label,
                    "config_path": config_path.to_string_lossy(),
                    "config": config,
                })))
            }
            "set" => {
                let data = data.ok_or("missing request data")?;
                let session_id = data.get("session_id").and_then(|v| v.as_str());
                let config: KnowledgeConfig = serde_json::from_value(
                    data.get("config")
                        .cloned()
                        .ok_or("missing 'config' field")?,
                )
                .map_err(|e| format!("config 形状非法: {e}"))?;
                let (_scope, config_path) = resolve_scope(ctx, session_id)?;
                // save 内部先 validate（绝对路径/枚举/重叠），非法 loud 拒绝
                // 不落盘；写盘原子（write_file_atomic）。热生效由
                // KnowledgeHandle mtime 缓存承接——写盘后下一次消费点自动重读。
                save_knowledge_config(&config_path, &config)?;
                tracing::info!(
                    "[Web/WSAPI] knowledge config saved: {} (enabled={}, pairs={})",
                    config_path.display(),
                    config.enabled,
                    config.pairs.len()
                );
                Ok(Some(serde_json::json!({
                    "saved": true,
                    "config_path": config_path.to_string_lossy(),
                })))
            }
            _ => Err(format!("unknown command: knowledge.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
