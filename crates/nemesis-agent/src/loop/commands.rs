//! slash 命令：handle_command(_with_context)、rewrite_custom_command/skill_fallback、process_system_message、历史请求与发布、record_last_channel/chat_id、get_startup_info、set_chat_seq_lookup。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

impl AgentLoop {
    /// Process a system message.
    /// Mirrors Go's `processSystemMessage()`.
    pub(crate) async fn process_system_message(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) -> (String, Option<String>) {
        if msg.channel != "system" {
            return (
                String::new(),
                Some(format!(
                    "processSystemMessage called with non-system channel: {}",
                    msg.channel
                )),
            );
        }

        info!(
            "[AgentLoop] Processing system message: sender_id={}, chat_id={}",
            msg.sender_id, msg.chat_id
        );

        // Parse origin channel from chat_id (format: "channel:chat_id").
        let (origin_channel, origin_chat_id) = if let Some(idx) = msg.chat_id.find(':') {
            (&msg.chat_id[..idx], msg.chat_id[idx + 1..].to_string())
        } else {
            ("cli", msg.chat_id.clone())
        };

        // Skip internal channels.
        if is_internal_channel(origin_channel) {
            info!(
                "[AgentLoop] Subagent completed (internal channel): content_len={}",
                msg.content.len()
            );
            return (String::new(), None);
        }

        // Use default agent session key.
        let session_key = build_agent_main_session_key("main");

        // Extract subagent result from message content.
        // Format: "Task 'label' completed.\n\nResult:\n<actual content>"
        // Mirrors Go's: if idx := strings.Index(content, "Result:\n"); idx >= 0 { content = content[idx+8:] }
        let content = if let Some(idx) = msg.content.find("Result:\n") {
            &msg.content[idx + 8..]
        } else {
            &msg.content
        };

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cron_job_id = msg.metadata.get("cron_job_id").map(|s| s.as_str());
        let cron_job_name = msg.metadata.get("cron_job_name").map(|s| s.as_str());
        // B1（2026-09-22）：user 行落盘职责上移（原在 run_agent_loop_internal
        // 开头）——system 路径内容 / 时点 / checkpoint 缺省保持原样，仅位置
        // 随职责移动到调用方。
        let sys_user_row = format!("[System: {}] {}", msg.sender_id, content);
        {
            let log_existed = Self::session_log_exists_before_append(&session_key);
            crate::chat_log::append_chat_log_meta(
                &session_key,
                "user",
                &sys_user_row,
                &crate::chat_log::ChatLogMeta {
                    model: None,
                    cron_job_id,
                    cron_job_name,
                    images: &[],
                    file_changes: &[],
                    checkpoint_turn: None, // E3：system 直调不经 gate/preamble，无 checkpoint 标记
                },
            );
            if !log_existed {
                self.emit_session_created(&session_key);
            }
        }
        let result = self
            .run_agent_loop_internal(
                &session_key,
                &sys_user_row,
                origin_channel,
                &origin_chat_id,
                false,
                &cancel_token,
                cron_job_id,
                cron_job_name,
                None,
                &[],
            )
            .await;

        match result {
            Ok(response) => (response, None),
            Err(e) => (String::new(), Some(e)),
        }
    }

    // -----------------------------------------------------------------------
    // History request handling
    // -----------------------------------------------------------------------

    /// Handle a history request by reading from session and publishing response.
    /// Mirrors Go's `handleHistoryRequest()`.
    pub(crate) async fn handle_history_request(
        &self,
        msg: &nemesis_types::channel::InboundMessage,
    ) {
        // 解析/组装走 crate::history 共享件（BUG 2026-09-23 入站过滤链：
        // web 咽喉点 HistoryFilter 与本路径共用同一组装逻辑，防漂移）。
        let req = match crate::history::HistoryRequest::parse(&msg.content) {
            Ok(r) => r,
            Err(e) => {
                error!("[AgentLoop] Failed to parse history request: {}", e);
                self.publish_history_response(
                    &msg.chat_id,
                    "",
                    &Vec::<serde_json::Value>::new(),
                    false,
                    0,
                    0,
                    None,
                    0,
                )
                .await;
                return;
            }
        };

        let limit = req.effective_limit();
        // HD（2026-09-17）：session_id 原样回显——前端收到后与当前选中会话
        // 比对，快速切换会话时迟到响应按归属丢弃（防串台）。
        let req_session_id = msg
            .metadata
            .get("session_id")
            .map(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        let agent_id = self
            .registry
            .as_ref()
            .and_then(|r| r.default_agent_id())
            .unwrap_or_else(|| "main".to_string());
        // Multi-session: if the client sent a session_id, derive
        // `agent:main:session:{sid}` (agent: prefix → adopted by loop.rs:1623,
        // bypasses routing). Otherwise fall back to the default "legacy"
        // conversation — MUST match server.rs process_messages' fallback so
        // history-read and chat-write share the same key (else the default
        // conversation's history wouldn't reload).
        let session_key = match msg.metadata.get("session_id") {
            Some(sid) if !sid.is_empty() => format!(
                "agent:main:session:{}",
                crate::session::SessionStore::sanitize_session_id(sid)
            ),
            _ => format!("agent:{}:session:legacy", agent_id),
        };

        // Read history from chat log (separate from session store).
        // _async（BUG 2026-09-22）：阻塞读移出 tokio worker——大日志秒级
        // 读曾占住 worker 并把本响应拖到前端 10s 失败围栏之后。
        let (page, total_count, has_more, oldest_index) =
            crate::chat_log::read_chat_log_async(&session_key, limit, req.before_index).await;

        // A1（2026-09-22 聊天切会话竞态）：历史读取**之后**采样环尾 seq——
        // 「seq ≤ last_seq 的 assistant 实时帧必已含于本批次」的推断前提是
        // assistant 入环点在 chat_log 落盘之后（user 行入环早于落盘，前端
        // 剔除规则只作用于 assistant 帧）。未注入回调 → 0 → 响应字段缺省，
        // 前端跳过剔除（A2 同文兜底）。
        let last_seq = self
            .chat_seq_lookup
            .read()
            .as_ref()
            .map(|f| f(&session_key))
            .unwrap_or(0);

        self.publish_history_response(
            &msg.chat_id,
            &req.request_id,
            &page,
            has_more,
            oldest_index,
            total_count,
            Some(&req_session_id),
            last_seq,
        )
        .await;
    }

    /// Publish a history response via the outbound channel.
    /// Mirrors Go's `publishHistoryResponse()`.
    async fn publish_history_response(
        &self,
        chat_id: &str,
        request_id: &str,
        messages: &[serde_json::Value],
        has_more: bool,
        oldest_index: usize,
        total_count: usize,
        // HD（2026-09-17）：session_id 回显（前端会话归属校验）；None = 解析
        // 失败路径（前端围栏按无归属放行，不因错误响应丢帧）。
        session_id: Option<&str>,
        // A1（2026-09-22）：历史读取时刻的环尾 seq（0 = 未注入回调/环空，
        // 序列化时省略——前端按无 last_seq 走同文兜底）。
        last_seq: u64,
    ) {
        // 组装唯一真相源 crate::history::history_response_json（HD 回显 /
        // A1 last_seq 省略规则只写一处）；本函数只保留 transport。
        let content = match crate::history::history_response_json(
            request_id,
            messages,
            has_more,
            oldest_index,
            total_count,
            session_id,
            last_seq,
        ) {
            Some(c) => c,
            None => {
                error!("[AgentLoop] Failed to marshal history response");
                return;
            }
        };

        if let Some(ref tx) = self.outbound_tx {
            let outbound = nemesis_types::channel::OutboundMessage {
                channel: "web".to_string(),
                chat_id: chat_id.to_string(),
                content,
                message_type: "history".to_string(),
                meta: Default::default(),
            };
            if let Err(e) = tx.send(outbound).await {
                warn!("[AgentLoop] Failed to send history response: {}", e);
            }
        } else {
            warn!("[AgentLoop] publish_history_response: no outbound_tx available");
        }

        debug!(
            "[AgentLoop] History response published: chat_id={}, request_id={}, total_count={}, has_more={}",
            chat_id, request_id, total_count, has_more
        );
    }

    // -----------------------------------------------------------------------
    // State recording
    // -----------------------------------------------------------------------

    /// Record the last active channel for crash recovery.
    /// Mirrors Go's `state.Manager.SetLastChannel()`.
    pub fn record_last_channel(&self, channel: &str) {
        if let Some(ref mgr) = self.state_manager
            && let Err(e) = mgr.set_last_channel(channel)
        {
            tracing::warn!("[AgentLoop] Failed to persist last channel: {}", e);
        }
    }

    /// Record the last active chat ID for crash recovery.
    /// Mirrors Go's `state.Manager.SetLastChatID()`.
    pub fn record_last_chat_id(&self, chat_id: &str) {
        if let Some(ref mgr) = self.state_manager
            && let Err(e) = mgr.set_last_chat_id(chat_id)
        {
            tracing::warn!("[AgentLoop] Failed to persist last chat ID: {}", e);
        }
    }

    // -----------------------------------------------------------------------
    // Session busy state management
    // -----------------------------------------------------------------------

    /// A1（2026-09-22 聊天切会话竞态）：注入环尾 seq 查询回调
    /// （nemesis-web `chat_event_log::latest_seq`，gateway 装配期调用）。
    /// `handle_history_request` 采样进历史响应 `last_seq`，前端据此剔除
    /// 「先于历史快照渲染的 assistant 实时帧」。未注入（standalone）→
    /// 响应不带 last_seq，前端走尾部同文兜底。
    pub fn set_chat_seq_lookup(&self, f: std::sync::Arc<dyn Fn(&str) -> u64 + Send + Sync>) {
        *self.chat_seq_lookup.write() = Some(f);
    }

    /// 自定义 slash 命令改写（改写型，区别于内置命令的短路型）。K3 补齐后
    /// 三段解析链（每段只在上一段未命中时生效）：
    ///
    /// 1. **自定义命令**：`/name args` → 命令表模板中的 `$ARGUMENTS` 替换为
    ///    `args`（模板无占位符且带参数 → 追加为独立段）。
    /// 2. **`` !`cmd` `` 注入**：对展开后的模板文本 regex 扫 `` !`cmd` `` →
    ///    同步执行（复用 C8 exec 内核 `run_one_stage`，3s 超时，workspace 为
    ///    cwd）→ stdout（≤4KB，字符边界截断）替换进模板；失败注记
    ///    `[command failed: ...]`。相同命令一段模板内只执行一次（结果复用）。
    ///    信任边界（诚实声明）：这是用户自己配置的模板/参数 = 用户本机终端
    ///    同级信任，**不**过 9 层安全管线（管线管的是 LLM 工具调用）；且只在
    ///    命令模板路径生效——普通消息不做注入（不放大攻击面）。
    /// 3. **技能回落**：`/name` 未命中命令表时查已装 skills（workspace →
    ///    global → builtin），命中则改写为 `Use the {name} skill to handle:
    ///    {args}`（无参数则 `Use the {name} skill.`）——技能斜杠化；
    ///    内置名/自定义命令优先级恒高于技能名。
    ///
    /// 未命中/内置名/非 slash 一律不动。每消息做一次 mtime 检查（一次 stat，
    /// 可忽略）。async 的原因只有 `` !`cmd` `` 执行；调用点 `process_inbound_
    /// message` 本就是 async，无锁跨 await（命令表读锁在 exec 前释放）。
    pub async fn rewrite_custom_command(&self, msg: &mut nemesis_types::channel::InboundMessage) {
        let content = msg.content.trim();
        if !content.starts_with('/') {
            return;
        }
        let rest = &content[1..]; // '/' 为 1 字节 ASCII，字节切片安全
        // 拷贝为 owned：name/args 借用自 msg.content，而技能回落要可变借
        // msg（E0502）——slash 消息本就低频，两次小分配无所谓。
        let (name, args) = match rest.split_once(char::is_whitespace) {
            Some((n, a)) => (n.to_string(), a.trim().to_string()),
            None => (rest.to_string(), String::new()),
        };
        if name.is_empty() || Self::BUILTIN_SLASH_COMMANDS.contains(&name.as_str()) {
            return;
        }
        // 命令表读锁 scope 严格限制在查表——技能回落走 fs 扫描，不得持锁
        // （return 走出本块时 guard 自动释放）。
        let expanded = {
            let hot_guard = self.commands_hot.read();
            let Some(hot) = hot_guard.as_ref() else {
                return self.rewrite_skill_fallback(&name, &args, msg);
            };
            hot.check();
            let commands = hot.get();
            let Some(cmd) = commands.commands.iter().find(|c| c.name == name) else {
                return self.rewrite_skill_fallback(&name, &args, msg);
            };
            // $ARGUMENTS 占位替换；模板无占位符且带参数 → 追加为独立段（对
            // 用户更友好：模板忘写占位符时参数不至于被吞）。
            if cmd.prompt.contains("$ARGUMENTS") {
                cmd.prompt.replace("$ARGUMENTS", &args)
            } else if !args.is_empty() {
                format!("{}\n\n{}", cmd.prompt, args)
            } else {
                cmd.prompt.clone()
            }
        };
        // K3：`` !`cmd` `` 注入（展开后的模板文本；模板与参数都可能携带）。
        let expanded = self.expand_shell_injections(expanded).await;
        info!(
            "[AgentLoop] custom command /{} expanded (prompt {} chars)",
            name,
            expanded.len()
        );
        msg.content = expanded;
    }

    /// K3：技能回落（只在命令表未命中时被 [`Self::rewrite_custom_command`]
    /// 调用）。命中已装技能 → 改写为技能驱动提示词；否则不动。
    fn rewrite_skill_fallback(
        &self,
        name: &str,
        args: &str,
        msg: &mut nemesis_types::channel::InboundMessage,
    ) {
        let Some(loader) = self.skills_loader.read().clone() else {
            return;
        };
        let known = loader.list_skills().iter().any(|s| s.name == name);
        if !known {
            return;
        }
        msg.content = if args.is_empty() {
            format!("Use the {name} skill.")
        } else {
            format!("Use the {name} skill to handle: {args}")
        };
        info!("[AgentLoop] /{name} resolved to installed skill");
    }

    /// Handle slash commands embedded in message content (standalone, no context).
    pub fn handle_command(&self, content: &str) -> Option<String> {
        self.handle_command_with_context(content, "")
    }

    /// Handle slash commands with optional channel context.
    /// Mirrors Go's `handleCommand()`.
    pub(crate) fn handle_command_with_context(
        &self,
        content: &str,
        current_channel: &str,
    ) -> Option<String> {
        let content = content.trim();
        if !content.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        match parts[0] {
            "/help" => Some(
                "Commands: /show [model|channel|agents], /list [tools|models], /model <alias>, /plan (计划模式: 停用文件修改), /build (切回构建模式), /compact (压缩会话上下文), /clear (清空会话历史), /help".to_string(),
            ),
            "/model" => {
                if parts.len() < 2 {
                    let current = self.active_model.read().clone();
                    let aliases = self.model_aliases();
                    Some(format!(
                        "Current model: {}\nAliases: {} (or pass any model id)",
                        current,
                        if aliases.is_empty() {
                            "(none configured)".to_string()
                        } else {
                            aliases.join(", ")
                        }
                    ))
                } else {
                    let new_model = self.set_active_model(parts[1]);
                    Some(format!("✓ Model switched to: {}", new_model))
                }
            }
            "/show" => {
                if parts.len() < 2 {
                    return Some("Usage: /show [model|channel|agents]".to_string());
                }
                match parts[1] {
                    "model" => Some(format!("Current model: {}", self.active_model.read())),
                    "channel" => Some(format!("Current channel: {}", current_channel)),
                    "agents" => {
                        let agent_ids = self
                            .registry
                            .as_ref()
                            .map(|r| r.list_agent_ids())
                            .unwrap_or_default();
                        if agent_ids.is_empty() {
                            let guard = self.tools.read();
                            let tool_names: Vec<&str> =
                                guard.keys().map(|s| s.as_str()).collect();
                            Some(format!("Registered agents (tools): {}", tool_names.join(", ")))
                        } else {
                            Some(format!("Registered agents: {}", agent_ids.join(", ")))
                        }
                    }
                    _ => Some(format!("Unknown show target: {}", parts[1])),
                }
            }
            "/list" => {
                if parts.len() < 2 {
                    return Some("Usage: /list [models|channels|agents|tools]".to_string());
                }
                match parts[1] {
                    "tools" => {
                        let guard = self.tools.read();
                        let tool_names: Vec<&str> =
                            guard.keys().map(|s| s.as_str()).collect();
                        Some(format!("Available tools: {}", tool_names.join(", ")))
                    }
                    "model" | "models" => Some(format!(
                        "Current model: {} (configured in config.json)",
                        self.active_model.read()
                    )),
                    "channels" => {
                        let channels = self.channel_manager_channels.lock();
                        if channels.is_empty() {
                            Some("No channels enabled".to_string())
                        } else {
                            Some(format!("Enabled channels: {}", channels.join(", ")))
                        }
                    }
                    "agents" => {
                        let agent_ids = self
                            .registry
                            .as_ref()
                            .map(|r| r.list_agent_ids())
                            .unwrap_or_default();
                        if agent_ids.is_empty() {
                            let guard = self.tools.read();
                            let tool_names: Vec<&str> =
                                guard.keys().map(|s| s.as_str()).collect();
                            Some(format!("Registered agents: {}", tool_names.join(", ")))
                        } else {
                            Some(format!("Registered agents: {}", agent_ids.join(", ")))
                        }
                    }
                    _ => Some(format!("Unknown list target: {}", parts[1])),
                }
            }
            "/switch" => {
                if parts.len() < 4 || parts[2] != "to" {
                    return Some("Usage: /switch [model|channel] to <name>".to_string());
                }
                let target = parts[1];
                let value = parts[3];

                match target {
                    "model" => {
                        let old_model = self.active_model.read().clone();
                        Some(format!(
                            "Model switch requested: {} -> {} (restart required for persistent change)",
                            old_model, value
                        ))
                    }
                    "channel" => Some(format!("Target channel switched to: {}", value)),
                    _ => Some(format!("Unknown switch target: {}", target)),
                }
            }
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Startup info
    // -----------------------------------------------------------------------

    /// Get startup information about the agent loop for logging.
    /// Mirrors Go's `GetStartupInfo()`.
    pub fn get_startup_info(&self) -> serde_json::Value {
        let guard = self.tools.read();
        let tool_names: Vec<&str> = guard.keys().map(|s| s.as_str()).collect();

        let agent_ids = self
            .registry
            .as_ref()
            .map(|r| r.list_agent_ids())
            .unwrap_or_default();

        serde_json::json!({
            "tools": {
                "count": tool_names.len(),
                "names": tool_names,
            },
            "agents": {
                "count": agent_ids.len(),
                "ids": agent_ids,
            },
            "model": self.active_model.read().to_string(),
            "max_turns": self.config.max_turns,
            "system_prompt_configured": self.config.system_prompt.is_some(),
        })
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------
}

// ---------------------------------------------------------------------------
// 自由函数归位（P1-c 自 loop.rs 根搬迁；仅增 pub(crate) 可见性标注）
// ---------------------------------------------------------------------------

/// Build an agent-scoped main session key.
///
/// Format: `agent:{agent_id}:main`
pub fn build_agent_main_session_key(agent_id: &str) -> String {
    format!("agent:{}:main", agent_id)
}

// ---------------------------------------------------------------------------
// Message formatting utilities
// ---------------------------------------------------------------------------
