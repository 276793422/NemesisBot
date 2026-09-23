//! Tool trait 与 FileChange(Kind)、工具注册/移除/枚举、MCP reload/refresh/snapshot 族、校验统计与重试预算。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// A previewable file change, used by the checkpoint (edit safety net) to
/// snapshot a file's pre-edit state so a `/rewind` can restore it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileChange {
    /// Path the tool will modify (as given in args; resolved against workspace
    /// root at snapshot/restore time).
    pub path: String,
    /// Kind of change — determines how a rewind restores it.
    pub kind: FileChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileChangeKind {
    /// File did not exist before the edit; rewind deletes it.
    Create,
    /// File existed and is being modified; rewind restores old content.
    Modify,
    /// File existed and is being deleted; rewind restores old content.
    Delete,
}

/// Trait for tools that can be executed by the agent loop.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Execute the tool with the given arguments, returning a string result.
    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String>;

    /// Set the execution context (channel + chat_id) for context-aware tools.
    ///
    /// This is called before each LLM iteration to inject the current channel
    /// and chat_id into tools that need them for routing (e.g., message, spawn,
    /// cluster_rpc). The default implementation is a no-op; tools that need
    /// context should override this method.
    fn set_context(&self, _channel: &str, _chat_id: &str) {}

    /// G2 (devtool-upgrade 阶段 3): notify the tool of the invocation's
    /// sub-agent nesting depth (0 = top-level agent, N = N levels deep).
    /// Called by `handle_tool_call_at_depth` right before execute, same
    /// injection pattern as `set_context`. Default no-op; depth-aware tools
    /// (spawn — `agents.subagent.max_depth` enforcement) override.
    fn set_invocation_depth(&self, _depth: usize) {}

    /// Return a human-readable description of this tool for the LLM.
    /// Mirrors Go's Tool.Description() string.
    fn description(&self) -> String {
        String::new()
    }

    /// Return the JSON schema for this tool's parameters.
    /// Mirrors Go's Tool.Parameters() map[string]interface{}.
    /// Should return a serde_json::Value representing an OpenAI-compatible
    /// JSON Schema object (e.g., {"type": "object", "properties": {...}}).
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    /// Preview the file change this call would make, for checkpointing (the edit
    /// safety net). Synchronous — parse `args` (the same JSON string passed to
    /// `execute`) to determine the target path and change kind only; the
    /// checkpoint store reads the file's current content separately (async).
    /// Returns `None` for read-only tools or non-file tools (default), so only
    /// writer tools opt in. Never panic on malformed args — return `None`.
    fn preview(&self, _args: &str) -> Option<FileChange> {
        None
    }

    /// A7（2026-09-06）：checkpoint 预检多点版——默认与 [`Tool::preview`]
    /// 等价（单文件）；multiedit 覆盖为逐文件清单，让检查点安全网在一次
    /// dispatch 内快照全部待改文件。调用点（K1a 瀑布）只消费本方法。
    fn preview_all(&self, args: &str) -> Vec<FileChange> {
        self.preview(args).into_iter().collect()
    }

    /// U5 (sixth batch): whether this tool is a pure read with no side effects
    /// (filesystem read, list, search, web fetch — safe to run concurrently
    /// with other read-only calls in the same tool batch). Default `false`
    /// — FAIL-CLOSED: a tool that has not declared itself read-only never
    /// joins the parallel pool, so a latent writer can't slip in. Writer
    /// tools and `exec` (even `cat`) stay `false`.
    fn is_read_only(&self) -> bool {
        false
    }

    /// G3 (devtool-upgrade 阶段 6): whether this tool may join the U5
    /// parallel pre-execution pool. Default = [`Self::is_read_only`] —
    /// pure reads stay the only automatic joiners. Override to `true` only
    /// for tools whose concurrent execution is safe by construction: the
    /// spawn tool opts in because (a) every pool dispatch injects the
    /// instance's sub-agent depth via `handle_tool_call_at_depth` (no-op
    /// for tools that don't override `set_invocation_depth`), (b) G0's own
    /// semaphore caps real concurrent sub-agents (`agents.subagent.
    /// max_concurrent`) on top of the pool's 4-permit limiter, (c) the
    /// full dispatch waterfall (estop/hidden/Plan/security/hooks) still
    /// runs inside each `handle_tool_call_at_depth` call, and (d) detached
    /// sub-agents own isolated instances/sessions — no shared mutable
    /// state with sibling spawns. Writers must NOT opt in: a `false` here
    /// keeps the whole batch serial (fail-closed, same as U5).
    fn is_parallel_safe(&self) -> bool {
        self.is_read_only()
    }

    /// P0 vault（C1，2026-09-22 计划 §3）：声明本工具参数中承载凭据**别名**
    /// 的槽位（顶层字段名，如 `"credential"`）。声明了槽位的工具，其参数
    /// 里这些字段的 `vault:<alias>` 引用会在 dispatch 最内层、execute 前
    /// 一刻被改写为真值（见 loop/credential_injection 模块）——工具执行的
    /// 是真值，全部日志/历史/预览表面只见别名。默认无槽位（机制零侵入）；
    /// 业务知识（哪个字段是凭据）只住在工具自己的声明里。
    fn credential_arg_keys(&self) -> &[&str] {
        &[]
    }

    /// P0 vault（D2，2026-09-22 计划 §4）：声明本工具归属的量化风险限制
    /// 类别（与 `security.limits` 配置键对应；空 = 不参与限额）。机制不
    /// 认识业务名词——类别语义（"exec"、"mass_message"…）由工具声明、
    /// 由配置绑定，本 crate 只提供滑动窗口计数与超限升级。
    fn limit_categories(&self) -> &[&str] {
        &[]
    }
}

impl AgentLoop {
    /// Register a tool with the agent loop (standalone mode).
    pub fn register_tool(&mut self, name: String, tool: Box<dyn Tool>) {
        debug!("[AgentLoop] Registered tool: {}", name);
        self.tools.write().insert(name, Arc::from(tool));
    }

    /// Register a tool across all agents in the registry (bus mode).
    /// Mirrors Go's `AgentLoop.RegisterTool()`.
    pub fn register_tool_shared(&mut self, name: String, tool: Box<dyn Tool>) {
        debug!("[AgentLoop] Registered shared tool: {}", name);
        self.tools.write().insert(name, Arc::from(tool));
    }

    /// J3 (devtool-upgrade 阶段 4)：注册 MCP 工具——同名不再静默覆盖。
    /// 旧路径直接 HashMap insert：sanitize 撞名的两个工具（同 server 内
    /// `search-file`/`search_file`，或两 server 名 sanitize 后相同）后者
    /// 无声顶掉前者，无任何痕迹。已存在时 warn 并依序改注册为
    /// `{name}_2`、`{name}_3`…——build_tool_defs 用注册键做 LLM 可见名，
    /// 改名后两个工具都诚实可见、可被调用（bridge 内部原始名只用于
    /// description/parameters，不受影响）。
    pub(crate) fn register_mcp_tool(&self, tool: Box<dyn nemesis_mcp::adapter::Tool>) {
        let base = tool.definition().name.clone();
        let mut tools = self.tools.write();
        let name = if tools.contains_key(&base) {
            let mut n = 2;
            while tools.contains_key(&format!("{base}_{n}")) {
                n += 1;
            }
            let renamed = format!("{base}_{n}");
            warn!(
                "[AgentLoop] MCP tool name collision: '{}' already registered; \
                 duplicate registered as '{renamed}'",
                base
            );
            renamed
        } else {
            base
        };
        tools.insert(
            name,
            Arc::from(Box::new(crate::mcp_bridge::McpToolBridge::new(tool)) as Box<dyn Tool>),
        );
    }

    // [ClusterService-Full] 完整方案预留：动态移除工具
    // 当前未启用，原因：避免影响 LLM 提示词缓存命中率
    // 启用条件：当 LLM 提供商支持按工具分组缓存或工具定义独立缓存时
    /// Remove a tool by name from the registry.
    /// Returns true if the tool was found and removed.
    pub fn remove_tool_shared(&mut self, name: &str) -> bool {
        if self.tools.write().remove(name).is_some() {
            debug!("[AgentLoop] Removed shared tool: {}", name);
            true
        } else {
            debug!("[AgentLoop] Tool '{}' not found, nothing to remove", name);
            false
        }
    }

    /// Return the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.read().len()
    }

    /// Return the names of all registered tools.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.read().keys().cloned().collect()
    }

    /// L6++（2026-09-08）：按名取注册工具的 `parameters()` JSON。项目 loop
    /// 工厂测试用它对两 loop 的共有工具逐同名比对 schema 字节一致（prompt
    /// cache 契约——同名工具 schema 漂移会让切会话组的请求缓存全失效）。
    /// None = 未注册。
    pub fn tool_parameters(&self, name: &str) -> Option<serde_json::Value> {
        self.tools.read().get(name).map(|t| t.parameters())
    }

    /// Enable automatic MCP tool reload via mtime-based change detection.
    ///
    /// Creates an `McpManager` for the given config path, discovers tools from
    /// all currently configured servers, and registers them. On each LLM round,
    /// the manager checks if the config file changed and loads new servers.
    pub fn enable_mcp_reload(&mut self, config_path: std::path::PathBuf) {
        let mgr = nemesis_mcp::manager::McpManager::new(config_path);
        if mgr.is_enabled() {
            for server in mgr.list_servers().to_vec() {
                let server_name = server.name.clone();
                match tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(mgr.discover_tools(&server))
                }) {
                    Ok(tools) => {
                        let count = tools.len();
                        for tool in tools {
                            self.register_mcp_tool(tool);
                        }
                        info!(
                            "[AgentLoop] MCP: registered {} tools from '{}'",
                            count, server_name
                        );
                    }
                    Err(e) => {
                        warn!(
                            "[AgentLoop] MCP: server '{}' discovery failed: {}",
                            server_name, e
                        );
                    }
                }
            }
            self.mcp_manager = Some(std::sync::Mutex::new(mgr));
            info!("[AgentLoop] MCP dynamic reload enabled (mtime-based)");
        } else {
            // Store manager even when disabled so we can detect future enable via config change
            self.mcp_manager = Some(std::sync::Mutex::new(mgr));
            info!("[AgentLoop] MCP config disabled; reload watcher active for future changes");
        }
        self.refresh_mcp_snapshot();
    }

    /// Check MCP config for changes and register tools from new servers.
    /// Uses interior mutability since the run loop borrows `&self`.
    pub(crate) fn check_mcp_reload(&self) {
        let mgr = match self.mcp_manager.as_ref() {
            Some(m) => m,
            None => return,
        };

        // 热重载收编登记（2026-08-29）：本处是 manager 重建 + 工具注册副作用
        // （重语义），非纯数据加载——保留独立实现并登记（同 tier 注记）。
        let changed = {
            match mgr.lock() {
                Ok(mut m) => m.check_config_changed(),
                Err(_) => return,
            }
        };

        if !changed {
            return;
        }

        // Collect existing MCP server prefixes to detect what's new.
        // J3：先取 mgr（锁序 mgr → tools 与下方发现循环一致），配置 server
        // 名 + 工具键快照后交给纯函数 [`registered_server_prefixes`] 正推。
        let (configured_servers, tool_keys) = {
            let configured: Vec<String> = match mgr.lock() {
                Ok(m) => m.list_servers().iter().map(|s| s.name.clone()).collect(),
                Err(_) => return,
            };
            let keys: Vec<String> = self.tools.read().keys().cloned().collect();
            (configured, keys)
        };
        let registered = registered_server_prefixes(&configured_servers, &tool_keys);

        let new_servers: Vec<_> = {
            match mgr.lock() {
                Ok(m) => m
                    .find_new_servers(&registered)
                    .into_iter()
                    .cloned()
                    .collect(),
                Err(_) => return,
            }
        };

        for server in new_servers {
            let server_name = server.name.clone();
            let tools = match mgr.lock() {
                Ok(m) => tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(m.discover_tools(&server))
                }),
                Err(_) => continue,
            };

            match tools {
                Ok(tools) => {
                    let count = tools.len();
                    for tool in tools {
                        self.register_mcp_tool(tool);
                    }
                    info!(
                        "[AgentLoop] MCP reload: registered {} tools from '{}'",
                        count, server_name
                    );
                }
                Err(e) => {
                    warn!(
                        "[AgentLoop] MCP reload: server '{}' failed: {}",
                        server_name, e
                    );
                }
            }
        }
        self.refresh_mcp_snapshot();
    }

    /// Refresh the MCP tool snapshot from the tool registry.
    fn refresh_mcp_snapshot(&self) {
        let snapshot: Vec<(String, String)> = self
            .tools
            .read()
            .iter()
            .filter(|(name, _)| name.starts_with("mcp_"))
            .map(|(name, tool)| (name.clone(), tool.description()))
            .collect();
        *self.mcp_tool_snapshot.write() = snapshot;
    }

    /// Return a shared reference to the MCP tool snapshot.
    /// Used to wire up McpListTool.
    pub fn mcp_tool_snapshot(&self) -> Arc<parking_lot::RwLock<Vec<(String, String)>>> {
        self.mcp_tool_snapshot.clone()
    }

    /// P2B（2026-09-12 NB-15 根修配套）：工具参数校验结果落账（DataStore
    /// 天×模型 upsert，`models.health` 数据源——worker 端 validation_budget
    /// 失败沉淀为可观测的长期趋势，供 tier 校准决策）。无 data_store 静默
    /// 跳过；写失败只 warn——审计是增值动作，不反压轮次。
    pub(crate) fn record_tool_validation_stats(&self, failed: bool) {
        if let Some(ref ds) = self.data_store {
            let model = self.active_model.read().clone();
            if let Err(e) = ds.record_tool_validation(&model, failed) {
                tracing::warn!("[AgentLoop] record_tool_validation failed: {e}");
            }
        }
    }

    /// Phase 2: check a tool call's arguments against the registered tool's
    /// schema. Returns Valid / Fixed / Invalid. Unknown tools return Valid so
    /// the existing unknown-tool path in `handle_tool_call` reports them
    /// (class C, not a schema failure).
    pub(crate) fn check_tool_args(
        &self,
        tool_call: &ToolCallInfo,
    ) -> crate::args_validator::Outcome {
        let schema_opt = self
            .tools
            .read()
            .get(&tool_call.name)
            .map(|t| t.parameters());
        match schema_opt {
            Some(schema) => crate::args_validator::check(&schema, &tool_call.arguments),
            None => crate::args_validator::Outcome::Valid,
        }
    }

    /// Phase 2: per-request consecutive-validation-failure budget, tier-aware.
    /// Mini models get 3, Normal/Big 2（P2C：Big 1→2，并行工具批首个坏调用
    /// 即烧光 budget=1，模型没机会自纠；见 capability.rs 注释）.
    pub(crate) fn validation_retry_budget(&self) -> u32 {
        (*self.tier.read()).validation_retry_budget()
    }
}
