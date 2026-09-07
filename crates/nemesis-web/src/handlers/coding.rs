//! Coding handler — P2-1 (2026-08-24 UI entry gap) 「代码开发」页后端。
//!
//! Read-only status commands for the three delegation/semantic-code tool
//! configs; writes deliberately go through the existing generic
//! `config.set_field` WSAPI (dot-path into config.json via ConfigStore) —
//! this handler only adds what config files cannot express: runtime PATH
//! probing of the five LSP language servers (§九.6 状态显示原则：能力状态
//! 必须问后端，禁止前端硬编码)，以及 C6 的 `lsp_install` 一键安装（唯一
//! 带副作用的命令：经 agent loop 的 exec dispatch 走安全 8 层 + 审批卡，
//! 不在本 handler 内裸 spawn）。
//!
//! All three toggles are read at AgentLoop build time (agent_factory PATH
//! probe registration), so the UI card tells the user to restart the Agent
//! (`agent.stop` → `agent.start`) after saving.

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct CodingHandler;

#[async_trait::async_trait]
impl ModuleHandler for CodingHandler {
    fn module_name(&self) -> &str {
        "coding"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["lsp_status", "lsp_install", "config"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "lsp_status" => self.lsp_status(),
            // C6: 一键安装缺失的语言服务器（走 agent 的 exec dispatch——
            // 安全 8 层 + 审批卡 + executor 隔离全链路）。
            "lsp_install" => self.lsp_install(data, ctx).await,
            "config" => self.config(ctx),
            _ => Err(format!("unknown command: coding.{}", cmd)),
        }
    }
}

impl CodingHandler {
    /// Live PATH probe of every language server in the nemesis-lsp registry
    /// table (`SERVERS`). Availability is machine-dependent — this command is
    /// exactly why the page does not hard-code anything.
    fn lsp_status(&self) -> Result<Option<serde_json::Value>, String> {
        let languages: Vec<serde_json::Value> = nemesis_lsp::registry::SERVERS
            .iter()
            .map(|spec| {
                // C6: 每语言附带平台相关安装命令（一键执行或复制）。
                let install = nemesis_lsp::install::install_command(spec.lang);
                serde_json::json!({
                    "lang": format!("{:?}", spec.lang),
                    "label": spec.lang.label(),
                    "command": spec.command,
                    "available": nemesis_lsp::registry::server_available(spec.lang),
                    "install_command": install.display,
                    "needs_interactive": install.needs_interactive,
                })
            })
            .collect();
        let available_count = nemesis_lsp::registry::probe_available().len();
        Ok(Some(serde_json::json!({
            "languages": languages,
            "available_count": available_count,
            // The lsp tool only registers when at least one server exists
            // (probe_available empty ⇒ tool not registered at all).
            "tool_would_register": available_count > 0,
        })))
    }

    /// C6（devtool-upgrade 阶段 6）：一键安装缺失的语言服务器。
    ///
    /// 执行走 **AgentLoop 的 exec dispatch**（`handle_tool_call`）——estop /
    /// hidden / Plan 闸 / 安全 8 层 / guardian / 审批卡（M7）/ executor 隔离
    /// 全链路，与模型调用 exec 完全同权同责；审批弹卡复用 dashboard 审批卡
    /// （process_exec 是 CRITICAL → RequireApproval → WebApprovalManager）。
    /// 交互命令（sudo 等）诚实拒绝只给复制。Agent 未运行时诚实报错。
    async fn lsp_install(
        &self,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let data = data.ok_or("missing data")?;
        let lang_str = data
            .get("lang")
            .and_then(|v| v.as_str())
            .ok_or("missing lang")?;
        // 接受 lsp_status 回显的 `{:?}` 形态（"Rust"），大小写宽容。
        let lang = nemesis_lsp::registry::SERVERS
            .iter()
            .map(|s| s.lang)
            .find(|l| format!("{:?}", l).eq_ignore_ascii_case(lang_str))
            .ok_or_else(|| {
                format!(
                    "unknown lang: {lang_str} (expected one of: {})",
                    nemesis_lsp::registry::SERVERS
                        .iter()
                        .map(|s| format!("{:?}", s.lang))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let cmd = nemesis_lsp::install::install_command(lang);
        if cmd.needs_interactive {
            return Err(format!(
                "「{}」的安装命令需要交互终端（sudo 密码等），无法一键执行——请复制命令手动运行：{}",
                cmd.display, cmd.display
            ));
        }
        // 槽读取窗口窄：clone Arc 后立即放锁（M7 approval.rs 同款纪律）。
        let agent_loop = {
            let guard = ctx.state.agent_loop.read();
            guard.clone()
        }
        .ok_or(
            "agent loop not running — 启动 Agent 后再试（安装经 agent 的 exec 通路走安全 8 层 + 审批）",
        )?;
        // Plan 模式预检：dispatch 闸会拒 exec（写类工具），这里给出针对性
        // 提示而非通用的 plan 拒绝文案（dashboard 用户没在聊天上下文里）。
        if agent_loop.mode() == nemesis_agent::types::AgentMode::Plan {
            return Err(
                "Agent 处于 Plan（规划）模式，一键安装被拒——在聊天里输入 /build 切回执行模式后重试"
                    .to_string(),
            );
        }
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let tool_call = nemesis_agent::types::ToolCallInfo {
            id: format!("lsp-install-{millis}"),
            // exec 走平台 shell（display 就是 shell 形态）；10 分钟上限——
            // npm -g 慢网络场景，ExecTool 自带超时+部分输出注记。
            name: "exec".to_string(),
            arguments: serde_json::json!({"command": cmd.display, "timeout": 600}).to_string(),
        };
        // 独立 RequestContext：不挂任何会话（安装不是聊天轮次——D3 文件映射
        // /checkpoint/A6 都按工具类天然不触发 exec）。
        let agent_ctx = nemesis_agent::context::RequestContext::new(
            "dashboard:lsp_install",
            &ctx.chat_id,
            "dashboard",
            "",
        );
        let result = agent_loop.handle_tool_call(&tool_call, &agent_ctx).await;
        Ok(Some(serde_json::json!({
            "lang": lang_str,
            "command": cmd.display,
            "result": result,
        })))
    }

    /// The three tool config sections for the page (read side; writes go via
    /// `config.set_field` with paths like `agents.lsp_tool.enabled`).
    fn config(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let home = crate::handlers::require_home(ctx)?;
        let cfg = load_config(home)?;
        Ok(Some(serde_json::json!({
            "lsp": {
                "enabled": cfg.agents.lsp_tool.enabled,
                "timeout_secs": cfg.agents.lsp_tool.timeout_secs,
                "idle_secs": cfg.agents.lsp_tool.idle_secs,
                // C6: 静默自举缺失服务器（网关启动期后台安装）。
                "auto_install": cfg.agents.lsp_tool.auto_install,
            },
            "claude_code": {
                "enabled": cfg.agents.claude_code_tool.enabled,
                "timeout_secs": cfg.agents.claude_code_tool.timeout_secs,
                // Valid: default | accept_edits | plan | bypass_permissions
                // (empty → accept_edits fail-safe at spawn; NOT model-selectable).
                "permission_mode": cfg.agents.claude_code_tool.permission_mode,
            },
            "codex": {
                "enabled": cfg.agents.codex_tool.enabled,
                "timeout_secs": cfg.agents.codex_tool.timeout_secs,
                // Valid: read_only | workspace_write | danger_full_access
                // (empty → read_only fail-safe at spawn; NOT model-selectable).
                "sandbox": cfg.agents.codex_tool.sandbox,
            },
            // C4: edit→diagnostics feedback loop. Independent of `lsp` above
            // (loop on + lsp tool off is a legal, recommended default combo).
            "diagnostics": {
                "enabled": cfg.agents.defaults.diagnostics_loop.enabled,
                "max_errors": cfg.agents.defaults.diagnostics_loop.max_errors,
                "wait_max_ms": cfg.agents.defaults.diagnostics_loop.wait_max_ms,
            },
            // E2 (devtool-upgrade 阶段 3): concurrent request mode card.
            // Valid modes: reject | queue | steer (queue = E1 default).
            // Read at AgentLoop build time → save then restart Agent.
            "concurrent": {
                "mode": cfg.agents.defaults.concurrent_request_mode,
                "queue_size": cfg.agents.defaults.queue_size,
            },
            // N2 (devtool-upgrade 阶段 4): small-model chore lane. Sole
            // consumer is manual /compact summarization (auto compression
            // stays on the main model). Read at AgentLoop build time →
            // save then restart Agent; unresolvable refs fall back to the
            // main model at startup (warn in gateway log).
            "small_model": {
                "configured": cfg
                    .agents
                    .small_model
                    .as_deref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false),
                "model": cfg.agents.small_model,
                "model_names": cfg
                    .model_list
                    .iter()
                    .filter(|m| !m.model_name.is_empty())
                    .map(|m| m.model_name.clone())
                    .collect::<Vec<_>>(),
            },
        })))
    }
}

fn config_path(home: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(home).join("config.json")
}

/// Same load strategy as handlers/config.rs: prefer the runtime ConfigStore,
/// fall back to disk. Keeps the read consistent with what set_field writes.
fn load_config(home: &str) -> Result<nemesis_config::Config, String> {
    if let Some(cfg) = nemesis_config::load_live() {
        return Ok(cfg);
    }
    nemesis_config::load_config(&config_path(home))
        .map_err(|e| format!("failed to load config: {}", e))
}

#[cfg(test)]
mod tests;
