//! 单轮驱动：CHANGES_SUMMARY_CAP/render_turn_changes_summary、ProcessOptions、DetachedOpts、spawn_session_title_job、get_or_create_instance、run(_with_trace/_detached/_detached_events)、resume_execution(_with_token)、drain_turn_file_changes。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// K4 (c): B 端 peer_chat 任务完成回报的变化摘要段。
///
/// B 端执行节点处理 `cluster_rpc:` 会话的最后一轮时，把本轮声明式文件
/// 工具变更以紧凑列表追加到最终回复（chat_log 落盘与出站发布共用同一
/// 字符串，日志与通道看到的完全一致）。上限 [`CHANGES_SUMMARY_CAP`] 条，
/// 溢出诚实注记（不静默截断）；顺序 = drain 顺序（确定性，重放字节稳定）。
/// 诚实边界：只覆盖**最终轮**——B 端多轮续行的历史轮变更不聚合（M3 会话
/// 级聚合是远期项）。
const CHANGES_SUMMARY_CAP: usize = 20;

pub(crate) fn render_turn_changes_summary(changes: &[FileChange]) -> Option<String> {
    if changes.is_empty() {
        return None;
    }
    let kind_str = |k: FileChangeKind| match k {
        FileChangeKind::Create => "create",
        FileChangeKind::Modify => "modify",
        FileChangeKind::Delete => "delete",
    };
    let mut s = format!("\n\n---\n📁 变更文件（本轮，共 {} 个）：", changes.len());
    for c in changes.iter().take(CHANGES_SUMMARY_CAP) {
        s.push_str(&format!("\n- {} ({})", c.path, kind_str(c.kind)));
    }
    if changes.len() > CHANGES_SUMMARY_CAP {
        s.push_str(&format!(
            "\n- …另有 {} 个文件未列出",
            changes.len() - CHANGES_SUMMARY_CAP
        ));
    }
    Some(s)
}
/// Configuration for how a message is processed through the agent loop.
#[derive(Debug, Clone)]
pub struct ProcessOptions {
    /// Session identifier for history/context.
    pub session_key: String,
    /// Target channel for tool execution.
    pub channel: String,
    /// Target chat ID for tool execution.
    pub chat_id: String,
    /// User message content.
    pub user_message: String,
    /// Response when LLM returns empty.
    pub default_response: String,
    /// Whether to trigger summarization.
    pub enable_summary: bool,
    /// Whether to send response via bus.
    pub send_response: bool,
    /// If true, don't load session history (for heartbeat).
    pub no_history: bool,
    /// Trace ID for observer events.
    pub trace_id: String,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            session_key: String::new(),
            channel: String::new(),
            chat_id: String::new(),
            user_message: String::new(),
            default_response: "I've completed processing but have no response to give.".to_string(),
            enable_summary: true,
            send_response: false,
            no_history: false,
            trace_id: String::new(),
        }
    }
}
/// G0 (devtool-upgrade 阶段 3)：`run_detached` 选项。
///
/// 构造用 `Default`（全空 = 全继承主 loop）。语义详见
/// [`AgentLoop::run_detached`]。
#[derive(Default)]
pub struct DetachedOpts<'a> {
    /// 子代理可用工具白名单（工具名集合）。`None` / 空 = 继承主 loop
    /// 全量供给（仍经 tier 过滤）。由 `effective_tool_defs` 消费。
    pub allowed_tools: Option<&'a [&'a str]>,
    /// 嵌套深度（0 = 顶层 spawn）。G2 消费两处：`run_detached` 把它写到
    /// instance（`detached_depth`），SpawnTool 层执行
    /// `agents.subagent.max_depth` 深度限制（`parent+1 > max_depth` 拒绝）；
    /// G4 后台化续行时同值透传。
    pub depth: usize,
    /// 模型覆盖。**v1 边界：暂未实现切换**——子代理沿用主 loop 当前
    /// active model（provider 选择在 run_llm_loop 深处，覆盖需独立
    /// 通道；字段先占位，实现留 G4/后续）。
    pub model: Option<String>,
    /// 工具轮预算（>0 时作为 `turn_budget` 传入，REPLACE
    /// `config.max_turns`；0 = 用主配置默认）。
    pub max_turns: u32,
    /// Swarm M1（裸提示词模式）：替换主 loop 人格 system_prompt（本回合
    /// 专用）。None = 继承主 loop 人格。供 board planner / 验收 / 主持人等
    /// 结构化子系统使用——它们需要无人格污染的纯净调用，不是"另一个
    /// 人格的 agent"。
    pub system_prompt: Option<&'a str>,
    /// Swarm M1：零工具供给。true = 本回合不向模型暴露任何工具定义
    /// （`effective_tool_defs` 直返空）；false = 正常供给链
    /// （tier → 白名单 → hidden）。与 max_turns=1 组合即纯文本单轮调用。
    pub no_tools: bool,
    /// Swarm M1：会话标签，注入 session_key（`subagent:{label}:{uuid}`）与
    /// trace_id，供日志检索与可观测（G13）。None = 维持 `subagent:{uuid}`。
    pub label: Option<&'a str>,
}

impl AgentLoop {
    /// E7 (devtool-upgrade 阶段 5)：会话标题自动生成——assistant 回复落盘
    /// 后触发；后台任务取首条 user 消息（≤500 字）→ 小模型生成 ≤24 字标题
    /// → 写 sidecar meta（只填「无标题/仅占位符」的会话，手动改名永不覆盖，
    /// 见 `chat_log::write_session_meta_auto_title`）。未配置
    /// `agents.small_model` / 无资格 / 首条消息缺失 → 不 spawn（返回 None）。
    /// 返回 JoinHandle 供测试 await；生产调用点丢弃句柄。
    pub(crate) fn spawn_session_title_job(
        &self,
        session_key: &str,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let (provider, model) = self.small_model_slot()?;
        if !crate::chat_log::auto_title_eligible(session_key) {
            return None;
        }
        let first = crate::chat_log::first_user_message(session_key, E7_TITLE_INPUT_MAX_CHARS)?;
        let key = session_key.to_string();
        Some(tokio::spawn(async move {
            if let Some(title) =
                generate_title_from_first_message(provider.as_ref(), &model, &first).await
            {
                crate::chat_log::write_session_meta_auto_title(&key, &title);
            }
        }))
    }

    /// D3：drain 本 session 的 turn 文件变更缓冲（取走即清 + 按 path 去重，
    /// kind 取最后声明——投影规则见 `chat_log::dedup_file_changes`）。
    /// assistant 最终回复落盘前调用。
    pub(crate) fn drain_turn_file_changes(&self, session_key: &str) -> Vec<FileChange> {
        let drained = self
            .turn_file_changes
            .lock()
            .remove(session_key)
            .unwrap_or_default();
        crate::chat_log::dedup_file_changes(drained)
    }

    /// Get or create an AgentInstance for the given session key.
    pub(crate) fn get_or_create_instance(&self, session_key: &str) -> AgentInstance {
        let config = AgentConfig {
            model: self.active_model.read().clone(),
            system_prompt: self.config.system_prompt.clone(),
            max_turns: self.config.max_turns,
            tools: self.config.tools.clone(),
            models: self.config.models.clone(),
        };
        let instance = AgentInstance::new(config);

        // Restore history + summary cache from session store if available.
        // Mirrors Go's `agent.Sessions.Get(sessionKey)` in `getOrCreateInstance`.
        if let Some(ref store) = self.session_store {
            let stored = store.get_or_create(session_key);
            let existing_summary = store.get_summary(session_key);
            let covers = store.get_summary_covers_up_to(session_key);
            if !stored.messages.is_empty() {
                let history: Vec<crate::types::ConversationTurn> =
                    stored.messages.into_iter().map(|m| m.into()).collect();
                instance.set_history(history);
            }
            // Restore the summary cache. covers_up_to indexes the full history
            // (system prompt at index 0). For new-format files it is stored
            // explicitly; for legacy files (pre-refactor, field absent → None)
            // we map it so build_messages sends ALL loaded messages verbatim
            // while injecting the summary as floating context for older content
            // that was truncated out under the old regime.
            if !existing_summary.is_empty() {
                let history = instance.get_history();
                if !history.is_empty() {
                    let c = match covers {
                        Some(c) => c.clamp(1, history.len()),
                        None => history
                            .iter()
                            .take_while(|t| t.role == "system")
                            .count()
                            .max(1),
                    };
                    instance.set_summary_cache(Some(crate::instance::SummaryCache {
                        covers_up_to: c,
                        text: existing_summary,
                    }));
                }
            }
        }

        instance
    }

    /// Run the agent loop for a specific session.
    /// Mirrors Go's `runAgentLoop()`.
    ///
    /// T5（多模态）：`image_refs` 是本 turn 已通过安全闸的图片路径引用
    /// （process_admitted 统一附加产出），随 user turn 进
    /// `ConversationTurn.image_refs`；无图调用方传 `&[]`。
    pub(crate) async fn run_agent_loop_internal(
        &self,
        session_key: &str,
        user_message: &str,
        channel: &str,
        chat_id: &str,
        voice_playback: bool,
        cancel_token: &tokio_util::sync::CancellationToken,
        cron_job_id: Option<&str>,
        cron_job_name: Option<&str>,
        turn_budget: Option<u32>,
        image_refs: &[String],
    ) -> Result<String, String> {
        // Round-5 fix: cron-originated turns are exempt from boundary events,
        // same as heartbeat. A recurring cron job targeting a persistent
        // session would grow its boundary sidecar (3+ rows per fire) without
        // bound — the exact unbounded-growth failure the heartbeat exemption
        // exists to prevent. The gateway cron handler (gateway.rs) delivers
        // via the bus with metadata cron_job_id, which flows here through
        // the caller; the CronTool path passes user="cron" (detected below).
        let is_cron_turn = cron_job_id.is_some();
        // Generate trace ID and emit conversation_start event.
        let trace_id = format!(
            "{}-{}",
            session_key,
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let start_time = std::time::Instant::now();

        // D3：本 turn 文件变更缓冲从空开始（正常路径上轮 drain 已清；这里
        // 兜底上轮异常短路未走到 assistant 落盘的残留）。
        self.turn_file_changes.lock().remove(session_key);

        // B1（2026-09-22 聊天切会话竞态）：user 行落盘职责已上移到调用方——
        // process_admitted 在预处理链之前落原始文本（「接纳即落盘」，消除
        // 早切回空视图），process_system_message 落 [System: ...] 行（内容
        // 与时点保持原样）。本函数不再写 user 行；assistant 行仍在函数末尾
        // 落盘，HD「一轮 = jsonl 恰好 +2 行」不变量由两处合计维持。
        // 取舍（2026-09-25 修订）：store 条目由 process_admitted 在本函数之前
        // 物化（get_or_create 先于 user 行落盘）——物化先行是硬契约：否则新
        // 会话双缺失时 get_or_create_instance 走 rebuild_from_chat_log，把
        // B1 已落盘的本轮 user 行回放进模型上下文，下方 add_user_message
        // 再加一次 → 首轮请求用户消息重复（agent-bench context_integrity
        // 实证）。turn 末 store 仍以 instance 全量 set_history 为单一真相源。

        // Emit conversation_start observer event.
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "agent".to_string(),
            content: user_message.to_string(),
        })
        .await;

        // Record last channel (skip internal channels).
        if !channel.is_empty() && !chat_id.is_empty() && !is_internal_channel(channel) {
            let channel_key = format!("{}:{}", channel, chat_id);
            self.record_last_channel(&channel_key);
        }

        let instance = self.get_or_create_instance(session_key);
        let mut context = RequestContext::new(channel, chat_id, "agent", session_key);
        if is_cron_turn {
            // Round-5 fix: propagate cron origin so run_llm_loop's boundary
            // gating can exempt it (see log_boundaries).
            context.user = "cron".to_string();
        }

        let events = self
            .run_with_trace(
                &instance,
                user_message,
                &context,
                &trace_id,
                voice_playback,
                cancel_token,
                turn_budget,
                image_refs,
            )
            .await;

        // Maybe trigger summarization.
        self.maybe_update_summary(&instance, session_key, channel, chat_id)
            .await;

        // Persist to session store. Post-refactor (inline-summarization): the
        // store holds the FULL instance history (system + user + assistant +
        // tool calls/results), not just a user/assistant log, so the summary
        // cache's covers_up_to index stays coherent across turns (the instance
        // is rebuilt from the store each turn). The user-facing conversation
        // log is written separately to chat_log below.
        //
        // (Pre-refactor this appended only user + final assistant, mirroring
        // Go's runAgentLoop. That left tool context out of the session file
        // and made a persistent summary cache incoherent — the root cause of
        // the "summary not injected / silent amnesia" bug this refactor fixes.)

        // Extract final response once (shared by session store, chat log, and observer).
        // K4 (c)：`mut`——`cluster_rpc:` 会话（B 端执行节点）本轮有文件变更
        // 时，完成回报会在下方追加变更摘要段（落盘与出站共用同一字符串）。
        let mut final_response = events
            .iter()
            .rev()
            .find_map(|e| {
                if let AgentEvent::Done(msg) = e {
                    Some(msg.clone())
                } else if let AgentEvent::Error(msg) = e {
                    Some(msg.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        if let Some(ref store) = self.session_store {
            // Ensure session exists in store.
            store.get_or_create(session_key);

            // Persist the summary cache (text + covers_up_to) BEFORE the
            // history: set_history's trim_to_limit reads covers_up_to to decide
            // which oldest messages are safe to drop (only from the covered
            // prefix) and adjusts it downward by the number dropped. Setting
            // covers first means the trim operates on this turn's correct value
            // and leaves a coherent store (messages + covers aligned). Setting
            // it AFTER would clobber the trim's adjustment and, for long
            // conversations (>MAX_STORED_MESSAGES), leave covers_up_to too large
            // so build_messages drops the verbatim tail.
            match instance.get_summary_cache() {
                Some(cache) => {
                    store.set_summary(session_key, &cache.text);
                    store.set_summary_covers_up_to(session_key, Some(cache.covers_up_to));
                }
                None => {
                    store.set_summary(session_key, "");
                    store.set_summary_covers_up_to(session_key, None);
                }
            }

            // Persist the full instance history (the store is the single source
            // of truth the next turn's get_or_create_instance reloads). trim runs
            // inside, bounding to MAX_STORED_MESSAGES and adjusting covers_up_to.
            let stored: Vec<crate::session::StoredMessage> = instance
                .get_history()
                .iter()
                .map(crate::session::StoredMessage::from)
                .collect();
            store.set_history(session_key, stored);

            if let Err(e) = store.save(session_key) {
                warn!(
                    "[AgentLoop] Failed to persist session history for {}: {}",
                    session_key, e
                );
            }
        }

        // Append to chat log (independent of session store).
        // user 行已在函数开头（LLM 循环前）落盘（切页恢复，见上方注释）——
        // 这里只补 assistant 行，HD「一轮 = jsonl 恰好 +2 行」不变量由
        // 两处合计维持。
        // D3：本 turn 声明式文件工具变更随 assistant 行落盘（消息↔文件
        // 变更映射；M3 会话级 diff 查看器的数据源）。drain 即清（下 turn
        // 从空开始）；去重规则见 `chat_log::dedup_file_changes`。
        let turn_changes = self.drain_turn_file_changes(session_key);
        // K4 (c) (devtool-upgrade 阶段 7): B 端 peer_chat 任务完成回报带
        // 变更摘要——`cluster_rpc:` 会话（B 端执行节点）本轮有声明式文件
        // 变更时，把紧凑列表追加进最终回复。落盘与出站共用同一字符串：
        // chat_log、channel 回执（含 rpc correlation 前缀包裹）看到的完全
        // 一致，A 端用户在原通道直接看到执行节点改了哪些文件。渲染规则
        // （上限/溢出注记/确定性顺序）见 `render_turn_changes_summary`。
        if session_key.starts_with("cluster_rpc:")
            && let Some(summary) = render_turn_changes_summary(&turn_changes)
        {
            final_response.push_str(&summary);
        }
        crate::chat_log::append_chat_log_meta(
            session_key,
            "assistant",
            &final_response,
            &crate::chat_log::ChatLogMeta {
                model: Some(&self.current_display_model()),
                cron_job_id,
                cron_job_name,
                images: &[],
                file_changes: &turn_changes,
                checkpoint_turn: None, // E3：标记只在 user 行（turn 开始锚）
            },
        );

        // E7：会话标题自动生成（assistant 回复落盘后；无资格/未配置小模型
        // 内部诚实跳过。通常首轮触发——meta 已有标题后不再重复）。
        let _ = self.spawn_session_title_job(session_key);

        // Emit conversation_end observer event.
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: session_key.to_string(),
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
        })
        .await;

        // Extract final response.
        for event in events.iter().rev() {
            if let AgentEvent::Done(msg) = event {
                return Ok(msg.clone());
            }
        }
        for event in events.iter().rev() {
            if let AgentEvent::Error(msg) = event {
                return Err(msg.clone());
            }
        }

        Ok("I've completed processing but have no response to give.".to_string())
    }

    // -----------------------------------------------------------------------
    // Standalone run loop
    // -----------------------------------------------------------------------

    /// Run the agent loop to process a user message (standalone mode).
    ///
    /// Returns a vector of events produced during execution.
    pub async fn run(
        &self,
        instance: &AgentInstance,
        user_message: &str,
        context: &RequestContext,
    ) -> Vec<AgentEvent> {
        let trace_id = format!(
            "run-{}",
            chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let token = tokio_util::sync::CancellationToken::new();
        self.run_with_trace(
            instance,
            user_message,
            context,
            &trace_id,
            false,
            &token,
            None,
            &[],
        )
        .await
    }

    /// Run the agent loop with a specific trace ID for observer event correlation.
    ///
    /// This is the actual implementation that emits observer events for:
    /// - LLM request (before calling the provider)
    /// - LLM response (after receiving the response)
    /// - Tool call (after each tool execution)
    pub async fn run_with_trace(
        &self,
        instance: &AgentInstance,
        user_message: &str,
        context: &RequestContext,
        trace_id: &str,
        voice_playback: bool,
        cancel_token: &tokio_util::sync::CancellationToken,
        turn_budget: Option<u32>,
        image_refs: &[String],
    ) -> Vec<AgentEvent> {
        // K2 (U14): prompt-level lifecycle hooks (dialect SessionStart +
        // UserPromptSubmit dialect events). Runs BEFORE `add_user_message`
        // so a blocked prompt NEVER enters history — the model doesn't see
        // it, matching the dialect's block semantics. `resume_execution` does not
        // pass through here (no new user prompt → no event), by design.
        {
            let lifecycle = self.hooks.lifecycle_hooks.read().snapshot();
            if !lifecycle.is_empty() {
                let prompt = crate::hooks::HookPrompt {
                    session_key: context.session_key.clone(),
                    channel: context.channel.clone(),
                    chat_id: context.chat_id.clone(),
                    prompt: user_message.to_string(),
                };
                if let Some(reason) = crate::hooks::run_user_prompt_hooks(&lifecycle, &prompt).await
                {
                    warn!(
                        "[AgentLoop] prompt hook blocked the message for session '{}': {}",
                        context.session_key, reason
                    );
                    return vec![AgentEvent::Done(format!(
                        "⛔ HOOK BLOCKED [layer:hook|policy:prompt_hook] {} — A registered hook denied this prompt. Adjust the hook policy and resend.",
                        reason
                    ))];
                }
            }
        }

        // Add user message to instance history.
        // T5（多模态）：带图片路径引用的变体（引用进 ConversationTurn.image_refs，
        // build_messages 每轮水合；无图时与 add_user_message 等价）。
        instance.add_user_message_with_images(user_message, image_refs);
        instance.set_state(crate::types::AgentState::Thinking);

        self.run_llm_loop(
            instance,
            context,
            trace_id,
            voice_playback,
            cancel_token,
            turn_budget,
        )
        .await
    }

    /// G0 (devtool-upgrade 阶段 3)：派生一个**分离执行**的子代理回合。
    ///
    /// 与主路径的本质关系：不走 bus、不写 session store / chat_log（会话
    /// 只存在于本次派生的临时 instance，跑完即弃），但**复用
    /// `run_with_trace` 全链路**——工具调度、安全 8 层、guardian、hook、
    /// tier 过滤、args 校验、spill、turn_guard、C3 诊断回灌全部同源生效
    /// （这是与 legacy `nemesis-tools` 异步版的本质差异：子代理拥有与主
    /// loop 相同的全套治理）。最终 assistant 文本从 `Done` 事件提取。
    ///
    /// 由 `SpawnTool` 的 spawn_slot 闭包（agent_factory 组装后注入，持
    /// `Weak<AgentLoop>` 防 Arc 环）调用。
    pub async fn run_detached(&self, task: &str, opts: DetachedOpts<'_>) -> Result<String, String> {
        let events = self.run_detached_events(task, opts).await;
        // Done 优先；Error 次之；两者皆无 = 诚实报错（不编造输出）。
        let mut done: Option<String> = None;
        let mut error: Option<String> = None;
        for e in events {
            match e {
                AgentEvent::Done(m) => done = Some(m),
                AgentEvent::Error(e) => error = Some(e),
                _ => {}
            }
        }
        match (done, error) {
            (Some(m), _) => Ok(m),
            (None, Some(e)) => Err(e),
            (None, None) => Err("sub-agent produced no output".to_string()),
        }
    }

    /// Swarm M1：detached 会话键构造。`label` 注入段（`subagent:{label}:{uuid}`）
    /// 供日志检索与可观测（G13）；None 维持 `subagent:{uuid}` 现状。
    pub(crate) fn detached_session_key(label: Option<&str>) -> String {
        match label {
            Some(l) => format!("subagent:{}:{}", l, uuid::Uuid::new_v4()),
            None => format!("subagent:{}", uuid::Uuid::new_v4()),
        }
    }

    /// K1/K2（devtool-upgrade 阶段 4）：[`AgentLoop::run_detached`] 的**事件
    /// 全集**变体。同一条 `run_with_trace` 链路（工具调度、安全 8 层、
    /// guardian、tier、spill、turn_guard 全部同源），但把事件 `Vec` 原样
    /// 交还调用方——headless `run` 文本模式折 Done/Error，NDJSON 模式（K2）
    /// 逐事件序列化。会话语义同 run_detached：临时 instance 跑完即弃。
    pub async fn run_detached_events(&self, task: &str, opts: DetachedOpts<'_>) -> Vec<AgentEvent> {
        // Swarm M1（裸提示词模式）：system_prompt 覆盖须在 AgentInstance::new
        // 之前——构造函数把它注入为 history 首轮 system turn。
        let mut cfg = self.config.clone();
        if let Some(sp) = opts.system_prompt {
            cfg.system_prompt = Some(sp.to_string());
        }
        let session_key = Self::detached_session_key(opts.label);
        let instance = AgentInstance::new(cfg);
        if let Some(allowed) = opts.allowed_tools
            && !allowed.is_empty()
        {
            instance
                .set_detached_allowed_tools(Some(allowed.iter().map(|s| s.to_string()).collect()));
        }
        if opts.no_tools {
            instance.set_detached_no_tools(true);
        }
        // G2: record the sub-agent nesting depth on the instance — the serial
        // dispatch reads it back (instance.detached_depth()) and injects it
        // into depth-aware tools so `agents.subagent.max_depth` is enforced.
        instance.set_detached_depth(opts.depth);
        // 注：workspace 继承由 agent_factory 的 SpawnConfig 提供（与主
        // instance 同目录）；standalone loop（无 workspace 概念）留空。
        let context = RequestContext {
            channel: "subagent".to_string(),
            chat_id: session_key.clone(),
            user: "subagent".to_string(),
            session_key: session_key.clone(),
            correlation_id: None,
            async_callback: None,
            tool_path_base: None,
        };
        let trace_id = format!("subagent-{}", uuid::Uuid::new_v4().simple());
        // Swarm G13：detached 路径补合成 ConversationStart/End。run_llm_loop
        // 只发 LlmRequest/LlmResponse，而请求日志观察者（RequestLoggerObserver
        // / ClusterRequestLoggerObserver）的 active 表以 start 事件注册
        // trace_id——缺 start 则全部事件被静默丢弃，评审/子代理/无头任务的
        // LLM 调用不可回放。与 cluster_agent worker 任务同一处置。
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: session_key.clone(),
            channel: "subagent".to_string(),
            chat_id: session_key.clone(),
            sender_id: "subagent".to_string(),
            content: task.to_string(),
        })
        .await;
        let start_time = std::time::Instant::now();
        let events = self
            .run_with_trace(
                &instance,
                task,
                &context,
                &trace_id,
                false,
                &tokio_util::sync::CancellationToken::new(),
                (opts.max_turns > 0).then_some(opts.max_turns),
                &[],
            )
            .await;
        let duration_ms = start_time.elapsed().as_millis() as u64;
        let rounds = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall(_)))
            .count() as u32
            + 1;
        // Done 优先、Error 次之、皆无为空串——与 run_detached 的提取一致。
        let final_response = events
            .iter()
            .rev()
            .find_map(|e| match e {
                AgentEvent::Done(m) => Some(m.clone()),
                _ => None,
            })
            .or_else(|| {
                events.iter().rev().find_map(|e| match e {
                    AgentEvent::Error(e) => Some(e.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();
        self.emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id,
            session_key,
            total_rounds: rounds,
            duration_ms,
            content: final_response,
            channel: "subagent".to_string(),
            chat_id: context.chat_id,
        })
        .await;
        events
    }

    /// Resume execution from a previously saved conversation state.
    ///
    /// Unlike `run_with_trace()`, this does NOT inject a user message.
    /// The instance should already have history loaded (via `set_history()`)
    /// and a tool result injected (via `add_tool_result()`).
    pub async fn resume_execution(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
    ) -> Vec<AgentEvent> {
        // 委托带 token 版本（自造一次性 token，行为与旧实现一致）。
        self.resume_execution_with_token(
            instance,
            context,
            trace_id,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    /// `resume_execution` 的带取消令牌变体（W2 P4 per-task cancel）：集群续行
    /// 任务执行前由 cluster_agent 注册 token，cancel_task 下行时可中断 LLM 循环。
    /// 语义与 [`Self::resume_execution`] 相同，仅 token 透传。
    pub async fn resume_execution_with_token(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> Vec<AgentEvent> {
        instance.set_state(crate::types::AgentState::Thinking);
        self.run_llm_loop(instance, context, trace_id, false, cancel_token, None)
            .await
    }
}
