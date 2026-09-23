//! run_llm_loop 原样搬迁（P2 解剖对象：轮循环/重试环/工具批/steer/estop）。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

impl AgentLoop {
    /// Core LLM loop shared by `run_with_trace()` and `resume_execution()`.
    ///
    /// `turn_budget` (T3/U12): per-turn tool-round override. When set (>0) it
    /// REPLACES `config.max_turns` for this turn — the per-fire budget of a
    /// cron continuation. Exhaustion reuses the grace-round semantics (one
    /// finalize round, then a resumable stop with reason `budget_exhausted`).
    pub(crate) async fn run_llm_loop(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        voice_playback: bool,
        cancel_token: &tokio_util::sync::CancellationToken,
        turn_budget: Option<u32>,
    ) -> Vec<AgentEvent> {
        let mut events = Vec::new();

        // max_tokens: per-model `max_output_tokens` from config if declared
        // (each model's real output ceiling, not a blanket 8192 — large files
        // write in one shot instead of truncating); else 8192. temperature 0.7.
        let chat_opts = crate::types::ChatOptions {
            max_tokens: Some(self.current_max_tokens().unwrap_or(8192)),
            temperature: Some(0.7),
            // H4 (U16 half): per-model reasoning effort from config.json.
            reasoning_effort: self.current_reasoning_effort(),
            ..Default::default()
        };

        // I3 (U9): durable turn boundary markers. Heartbeat sessions are
        // exempt — they run periodically and would grow heartbeat.jsonl
        // without bound (3+ boundary lines per beat, forever).
        // 4th-pass fix: the heartbeat exemption keys on the CONTEXT user
        // marker (set by process_heartbeat), not the session_key — a real
        // user session could legitimately be named "agent:heartbeat" and
        // would have been silently exempted from boundary logging.
        // Round-5 fix: cron-originated turns are exempt too (user=="cron",
        // set by run_agent_loop_internal when cron_job_id metadata is
        // present) — a recurring cron on a persistent session grows the
        // boundary sidecar unboundedly otherwise.
        let log_boundaries = context.user != "heartbeat"
            && context.user != "cron"
            && !is_internal_channel(&context.channel);
        if log_boundaries {
            crate::chat_log::append_boundary_event(&context.session_key, "turn_start", "");
        }
        let mut turns_used = 0u32;
        // Phase 2 (small-model-tool-robustness): per-request consecutive
        // validation-failure counter. Reset on any successful (valid or
        // auto-fixed) tool call; incremented on each schema violation. When it
        // reaches the tier budget the loop stops, preventing a struggling model
        // from burning max_turns on the same malformed call.
        let mut validation_failures = 0u32;
        // Continue-generation budget for max_tokens truncation: when output
        // hits the token cap it's cut mid-way (often mid tool-call JSON).
        // Instead of routing the broken call through the validation budget
        // (which force-stops with a misleading "args invalid" error — Big tier
        // = 0 retries), append partial content + a "continue" prompt and
        // re-loop.
        let mut length_continuations = 0u32;
        const MAX_LENGTH_CONTINUATIONS: u32 = 5;
        // ② Grace-round latch. When the tool-call budget is exhausted we grant
        // one extra round (with GRACE_ROUND_NUDGE injected) so the model can
        // synthesize a final answer from completed work; a second hit stops
        // resumably instead of hard-crashing with "Max iterations reached".
        let mut grace_round = false;
        // Turn-scoped guards (⑥ alternating loop, ⑦ degenerate output). Fresh
        // per request — no state crosses requests.
        let mut turn_guard = crate::turn_guard::TurnGuard::new();
        // ⑦ Degenerate-answer nudge awaiting re-injection. Transient — kept out
        // of instance history / session_log; re-applied after each build_messages
        // until the model gives a visible answer or the retry budget runs out.
        let mut degenerate_nudge_pending: Option<String> = None;
        // ⑧ Pending cross-round prose-repetition nudge (same transient pattern:
        // re-applied after each build_messages, never persisted to history).
        let mut repetition_nudge_pending: Option<String> = None;
        // I1 (U7): one-shot escape-hatch latch (see the Accept branch).
        let mut steer_escape_used = false;
        // L2 (full review): terminal reason recorded AT the break site
        // instead of post-hoc string sniffing (a model reply containing
        // the paused-after wording would have been misclassified).
        let mut terminal_reason: Option<&'static str> = None;
        // K2 (U14): turn-end hook (dialect Stop) continue budget. Each
        // `Continue` demand injects the hook feedback as a user message and
        // grants one more round; exhausted → stop anyway (fail-open, same
        // discipline as MAX_LLM_HOOK_RETRIES).
        let mut turn_end_continues: u32 = 0;

        // K1b (U14): labeled so the LLM post-hook retry loop (deep inside,
        // around the guarded re-call) can abort the turn with `break 'turn`.
        // Bare `break`s elsewhere keep targeting their nearest loop — this
        // label only ADDS a way to name the turn loop, changing nothing else.
        'turn: loop {
            // Auto-reload MCP tools if config file changed.
            self.check_mcp_reload();
            // Phase 4a: re-resolve capability tier if config.json changed on
            // disk (dashboard model add, CLI `model set-tier` while running).
            self.check_config_reload();

            // Check cancellation at the top of each iteration.
            if cancel_token.is_cancelled() {
                info!(
                    "[AgentLoop] LLM loop cancelled at top of iteration, turns_used={}",
                    turns_used
                );
                events.push(AgentEvent::Done("已取消".to_string()));
                break;
            }

            // 全局急停检查：触发则立刻结束当前轮。未接线（None）时整块跳过。
            let estop_engaged = self
                .estop
                .read()
                .as_ref()
                .map(|e| e.is_engaged())
                .unwrap_or(false);
            if estop_engaged {
                info!(
                    "[AgentLoop] E-stop engaged at top of iteration, turns_used={}",
                    turns_used
                );
                events.push(AgentEvent::Done(
                    "⛔ 已急停 (E-STOP) — 已停止当前任务。发送 `nemesisbot estop --release` 恢复。"
                        .to_string(),
                ));
                break;
            }

            // ①/② max_turns cap + grace round. max_turns == 0 means unlimited
            // (opt-in). T3 (U12): when a per-turn budget override is set
            // (cron continuation's max_rounds), it REPLACES the global cap for
            // this turn. On the first hit we grant one grace round (with
            // GRACE_ROUND_NUDGE injected below) so the model can finalize from
            // completed work; a second hit stops resumably — no work is lost.
            let effective_max_turns = turn_budget.unwrap_or(self.config.max_turns);
            if effective_max_turns > 0 && turns_used >= effective_max_turns {
                if !grace_round {
                    grace_round = true;
                    info!(
                        "[AgentLoop] max_turns ({}) reached after {} turns; granting one grace round to finalize",
                        effective_max_turns, turns_used
                    );
                    // Fall through: this iteration runs as the grace round.
                } else if turn_budget.is_some() {
                    warn!(
                        "[AgentLoop] paused after {} tool-call rounds (per-turn budget exhausted, grace round spent)",
                        effective_max_turns
                    );
                    // T3 (U12): budget-driven stop. The job that fired this
                    // turn is NOT deleted — the next fire re-budgets, so the
                    // message says so instead of suggesting a config change.
                    terminal_reason = Some("budget_exhausted");
                    events.push(AgentEvent::Done(format!(
                        "已在定时任务预算 {} 轮工具调用后暂停，已完成的工作已保存。定时任务未被删除，下次触发时会重新获得预算。",
                        effective_max_turns
                    )));
                    break;
                } else {
                    warn!(
                        "[AgentLoop] paused after {} tool-call rounds (grace round exhausted)",
                        effective_max_turns
                    );
                    terminal_reason = Some("max_turns");
                    events.push(AgentEvent::Done(format!(
                        "已在 {} 轮工具调用后暂停，已完成的工作已保存。发送下一条消息可继续，或调大 max_tool_iterations（设为 0 表示不限）。",
                        effective_max_turns
                    )));
                    break;
                }
            }

            // I1 (U7): inbox claim — before EVERY LLM call of this turn, take
            // all pending steer messages (next-step) into history as real user
            // messages (persisted: they ARE genuine user input). Placement
            // after the existing history = same position as the time/env
            // injection's protected prefix zone (appended user turn), so the
            // provider prefix stays stable.
            //
            // ROUND-5 EFFICIENCY FIX: claim BEFORE build_messages (it used to
            // run after, so every steered round built the full message list
            // twice — skills catalog scan + instruction-chain file IO + 2
            // sha256s — and threw the first build away). One build, always.
            let steer_batch = self.inbox.claim_next_step(&context.session_key);
            if !steer_batch.is_empty() {
                for m in &steer_batch {
                    // L4 (full review) + round-5: strip the marker via the
                    // SINGLE shared rule (inbox::strip_steer_marker) — it is a
                    // ROUTING signal, not content, and the same message must
                    // arrive marker-free whether injected in-turn (here) or
                    // replayed post-turn (drain path).
                    let content = crate::inbox::strip_steer_marker(&m.msg.content).to_string();
                    // I2：steer 消息与首轮同源（B1 原则延伸）——@文件引用同样
                    // 展开（同基准/同安全闸），不因注入时点而异。
                    let at_base = self
                        .workspace_root
                        .read()
                        .clone()
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    let content = crate::message_preprocess::expand_at_files(
                        &content,
                        &at_base,
                        &m.msg.channel,
                        #[cfg(feature = "security")]
                        self.security_plugin.as_deref(),
                        #[cfg(not(feature = "security"))]
                        None,
                    );
                    // B1（2026-09-03 二次回归）：steer 消息与首轮同源——同样可能
                    // 携带图片（media 引用 + 文本点名路径）。走与 process_admitted
                    // 同一附加链（URL 预取 + 统一附加 + 诚实注记 + image_refs），
                    // 不静默丢图。
                    let ws_for_uploads = self.workspace_root.read().clone();
                    let uploads_base = ws_for_uploads
                        .unwrap_or_else(|| nemesis_path::default_path_manager().workspace());
                    let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(&uploads_base);
                    let (media_for_attach, url_notes) = crate::image_attach::fetch_url_media(
                        &m.msg.media,
                        &uploads_dir,
                        #[cfg(feature = "security")]
                        self.security_plugin.as_deref().and_then(|p| p.ssrf_guard()),
                        #[cfg(not(feature = "security"))]
                        None,
                    )
                    .await;
                    // J6：同 process_admitted——降采样开关 fresh-read，产物落 uploads。
                    let downscale_dir = self.current_image_downscale().then(|| uploads_dir.clone());
                    let attach = crate::image_attach::attach_turn_images(
                        &content,
                        &media_for_attach,
                        self.workspace_root.read().as_deref(),
                        downscale_dir.as_deref(),
                        &m.msg.channel,
                        #[cfg(feature = "security")]
                        self.security_plugin.as_deref(),
                        #[cfg(not(feature = "security"))]
                        None,
                    );
                    let content = attach.merge_into_text(content);
                    let content = crate::image_attach::AttachOutcome {
                        attached: Vec::new(),
                        notes: url_notes,
                    }
                    .merge_into_text(content);
                    instance.add_user_message_with_images(&content, &attach.ref_strings());
                    // L3（2026-09-04 四轮盲审）：steer 的 chat_log 行也要带图片
                    // 路径引用——首轮路径用 append_chat_log_full_with_images，
                    // steer 路径却用 3 参变体丢掉 images → 会话浏览器/self-heal
                    // 重建/fork 时 steer 轮的图片凭空消失（instance 里有图、
                    // 落盘行无图，两套存储分叉）。
                    crate::chat_log::append_chat_log_full_with_images(
                        &context.session_key,
                        "user",
                        &format!("[steer] {}", content),
                        None,
                        None,
                        None,
                        &attach.ref_strings(),
                    );
                    info!(
                        "[AgentLoop] steer message injected before LLM call: session_key={}, len={}",
                        context.session_key,
                        m.msg.content.len()
                    );
                    if log_boundaries {
                        crate::chat_log::append_boundary_event(
                            &context.session_key,
                            "steer_injected",
                            &format!("len={}", m.msg.content.len()),
                        );
                    }
                }
            }

            // P3.1 (sixth batch): auto-inject memory prefetch — async search
            // against the CURRENT (latest) user message, done OUTSIDE
            // build_messages (which is sync; search is async). Per round: the
            // latest user message changes when steer messages land, so
            // re-prefetching per LLM round keeps the section in sync with
            // what the model is about to see. Off (default) ⇒ None ⇒ the
            // build is byte-identical to pre-P3.1.
            let memory_hits: Option<Vec<String>> = self.prefetch_memory_context(instance).await;

            // Build the message list from instance history (AFTER the steer
            // claim so injected turns are already included).
            //
            // T8 (U9 ②): the annotated build + the injection records below
            // form this round's projection ledger — everything a later
            // byte-exact replay needs beyond the session store (the
            // transient injections are never persisted). See `crate::replay`.
            let (mut messages, build_annotation) =
                self.build_messages_with_memory_annotated(instance, memory_hits.as_deref());
            let mut replay_injections: Vec<crate::replay::InjectionRecord> = Vec::new();
            if let Some(idx) = build_annotation.digest_index {
                replay_injections.push(crate::replay::InjectionRecord {
                    index: idx,
                    role: messages[idx].role.clone(),
                    source: crate::replay::INJECTION_CONTEXT_DIGEST.to_string(),
                    content: messages[idx].content.clone(),
                });
            }
            let mut replay_voice: Option<crate::replay::VoiceAppend> = None;

            // Voice playback prompt injection: append to last user message (not stored in history).
            if voice_playback && let Some(pos) = messages.iter().rposition(|m| m.role == "user") {
                messages[pos].content.push_str(VOICE_PLAYBACK_SUFFIX);
                replay_voice = Some(crate::replay::VoiceAppend {
                    index: pos,
                    suffix: VOICE_PLAYBACK_SUFFIX.to_string(),
                });
            }

            // ② Grace-round nudge. Transient — NOT persisted to instance history
            // or session_log; only this turn's message list carries it.
            if grace_round {
                messages.push(LlmMessage {
                    role: "system".to_string(),
                    content: GRACE_ROUND_NUDGE.to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                    images: Vec::new(),
                });
                replay_injections.push(crate::replay::InjectionRecord {
                    index: messages.len() - 1,
                    role: "system".to_string(),
                    source: crate::replay::INJECTION_GRACE_NUDGE.to_string(),
                    content: GRACE_ROUND_NUDGE.to_string(),
                });
            }

            // ⑦ Re-inject a pending degenerate-answer nudge (transient, like the
            // grace nudge — never persisted to instance history / session_log).
            if let Some(nudge) = &degenerate_nudge_pending {
                messages.push(LlmMessage {
                    role: "user".to_string(),
                    content: nudge.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                    images: Vec::new(),
                });
                replay_injections.push(crate::replay::InjectionRecord {
                    index: messages.len() - 1,
                    role: "user".to_string(),
                    source: crate::replay::INJECTION_DEGENERATE_NUDGE.to_string(),
                    content: nudge.clone(),
                });
            }

            // ⑧ Re-inject a pending prose-repetition nudge (transient).
            if let Some(nudge) = &repetition_nudge_pending {
                messages.push(LlmMessage {
                    role: "system".to_string(),
                    content: nudge.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                    images: Vec::new(),
                });
                replay_injections.push(crate::replay::InjectionRecord {
                    index: messages.len() - 1,
                    role: "system".to_string(),
                    source: crate::replay::INJECTION_REPETITION_NUDGE.to_string(),
                    content: nudge.clone(),
                });
            }

            debug!("[AgentLoop] Sending {} messages to LLM", messages.len());

            // K1b (U14): LLM-call-level pre hooks. Runs AFTER messages are
            // built (nudges included) and BEFORE the LlmRequest observer
            // event — appended messages land in request_log and in the T8
            // replay ledger (byte-exact replay keeps holding).
            {
                let llm_hooks = self.llm_hooks.read().snapshot();
                if !llm_hooks.is_empty() {
                    let hook_call = crate::hooks::HookLlmCall {
                        model: self.active_model.read().clone(),
                        session_key: context.session_key.clone(),
                        round: turns_used as usize + 1,
                    };
                    match crate::hooks::run_llm_pre_hooks(&llm_hooks, &hook_call, &messages).await {
                        Ok(appended) => {
                            for m in appended {
                                replay_injections.push(crate::replay::InjectionRecord {
                                    index: messages.len(),
                                    role: m.role.clone(),
                                    source: crate::replay::INJECTION_LLM_HOOK.to_string(),
                                    content: m.content.clone(),
                                });
                                messages.push(m);
                            }
                        }
                        Err(reason) => {
                            warn!(
                                "[AgentLoop] LLM hook blocked the call, turns_used={}: {}",
                                turns_used, reason
                            );
                            events.push(AgentEvent::Done(format!(
                                "⛔ HOOK BLOCKED [layer:hook|policy:llm_hook] {} — A registered LLM hook denied this round. Do NOT retry unless the user changes the hook policy.",
                                reason
                            )));
                            break;
                        }
                    }
                }
            }

            // Build tool definitions from registered tools for LLM function calling.
            // Mirrors Go's ToolRegistry.ToProviderDefs() which calls tool.Description() and tool.Parameters().
            // Sort by name so the order is stable across runs — a deterministic
            // tool order gives reproducible behaviour and avoids unnecessary prompt
            // variation between requests.
            // Y1 (Phase4-a): fold AFTER the tier filter — description text only,
            // byte-identical passthrough whenever folding is off/degrades.
            let tool_defs: Vec<crate::types::ToolDefinition> = self.effective_tool_defs(instance);
            debug!(
                "[AgentLoop] Sending {} tool definitions to LLM",
                tool_defs.len()
            );

            // Emit LLM request observer event.
            // F-J：经 observer_msg_values 剥掉图片 base64（见其 doc）。
            let msg_values: Vec<serde_json::Value> = observer_msg_values(&messages);
            let tool_values: Vec<serde_json::Value> = tool_defs
                .iter()
                .filter_map(|t| serde_json::to_value(t).ok())
                .collect();
            // Extract model string before emit so RwLockReadGuard doesn't span the await.
            let active_model = self.active_model.read().clone();
            self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmRequest {
                trace_id: trace_id.to_string(),
                round: turns_used + 1,
                model: active_model.clone(),
                messages_count: messages.len(),
                tools_count: tool_defs.len(),
                messages: msg_values,
                tools: tool_values,
                provider_name: String::new(),
                api_key: String::new(),
                api_base: String::new(),
            })
            .await;

            // Call LLM.
            instance.set_state(crate::types::AgentState::Thinking);
            let round_start = std::time::Instant::now();
            // Clone provider Arc so RwLock guard is dropped before .await.
            let active_provider = self.provider.read().clone();

            // 订阅急停状态——LLM 调用进行中若触发急停，能即时打断：select 命中
            // estop arm 后，chat future 被 drop → reqwest 取消在途 HTTP 请求。
            // `None`（未接线）时该 arm 永不 resolve（pending），等价于没这条 arm。
            // 注意：subscribe() 返回的是 owned Receiver（不借用 guard），所以这里
            // 拿完就能放掉 estop 的读锁。
            let mut estop_rx = self.estop.read().as_ref().map(|e| e.subscribe());

            // I3 (U9): durable llm_request marker (model + size estimate,
            // no bodies). Heartbeat/internal-channel exemption (see
            // turn_start).
            //
            // T8 (U9 ②): projection-ledger sidecar for this round — the
            // durable record of every non-persisted injection (full bodies),
            // enabling byte-exact replay from the session store. Same
            // cron/heartbeat/internal exemption as the marker above: those
            // turns recur forever and would grow the ledger unboundedly.
            if log_boundaries {
                crate::chat_log::append_boundary_event(
                    &context.session_key,
                    "llm_request",
                    &format!(
                        "model={} messages={} turns_used={}",
                        self.active_model.read(),
                        messages.len(),
                        turns_used
                    ),
                );
                crate::replay::append_projection_record(&crate::replay::RequestProjectionRecord {
                    trace_id: trace_id.to_string(),
                    session_key: context.session_key.clone(),
                    round: turns_used as usize + 1,
                    ts: crate::replay::now_rfc3339(),
                    messages_count: messages.len(),
                    roles: messages.iter().map(|m| m.role.clone()).collect(),
                    history_len_at_build: build_annotation.history_len,
                    injections: replay_injections,
                    voice_append: replay_voice,
                    summary_as_of: build_annotation.summary_as_of.clone(),
                    vision_projected: build_annotation.vision_projected,
                });
            }

            // T10（多模态 D4 ③）：provider 兜底提示的判定输入——最终请求里
            // 是否真的带了图片字节（vision=yes/默认放行时图才会进来；nudge/
            // hook 注入消息恒无图）。
            let request_had_images = messages.iter().any(|m| !m.images.is_empty());

            // Use tokio::select! to allow cancellation / e-stop during the LLM call.
            // P3B 撞墙检测起点：首次调用的失败时长供 transient 重试链比对。
            let first_call_start = std::time::Instant::now();
            // ②A：首捕同样包超时——挂死连接不再无限等（cancel/estop 臂
            // 仍然可打断超时等待，语义只增不减）。
            let first_call_timeout = self.current_provider_call_timeout_secs();
            let chat_result = tokio::select! {
                result = Self::chat_call_bounded(
                    first_call_timeout,
                    &active_provider,
                    &active_model,
                    messages,
                    Some(chat_opts.clone()),
                    tool_defs,
                ) => result,
                _ = cancel_token.cancelled() => {
                    info!("[AgentLoop] LLM call cancelled while waiting for response, turns_used={}", turns_used);
                    events.push(AgentEvent::Done("已取消".to_string()));
                    break;
                }
                // 急停：watch 翻成 engaged 才 resolve。（K1b 起该等待逻辑抽成
                // wait_estop_engaged 共享——hook 重呼路径用同一段，防漂移。）
                _ = Self::wait_estop_engaged(estop_rx.as_mut()) => {
                    info!(
                        "[AgentLoop] E-stop engaged during LLM call, turns_used={}",
                        turns_used
                    );
                    events.push(AgentEvent::Done(
                        "⛔ 已急停 (E-STOP) — LLM 调用已中断。发送 `nemesisbot estop --release` 恢复。"
                            .to_string(),
                    ));
                    break;
                }
            };

            let mut response = match chat_result {
                Ok(resp) => resp,
                Err(err) => {
                    // 首次调用从发起到失败的时长（Err 臂入口即失败点）。
                    let first_call_failed_after = first_call_start.elapsed();
                    let err_lower = err.to_lowercase();
                    let is_context_error = ["token", "context", "length", "invalid"]
                        .iter()
                        .any(|keyword| err_lower.contains(keyword));

                    if is_context_error {
                        // Mirrors Go's retry-with-compression logic (loop_executor.go).
                        // Attempt up to 2 retries with progressive history compression.
                        let mut retry_count = 0u32;
                        let max_retries = 2u32;
                        let mut retry_err = err.clone();
                        let mut got_response = None;

                        // Notify user about compression.
                        info!(
                            "[AgentLoop] LLM context error, attempting compression and retry: {}",
                            retry_err
                        );

                        while retry_count < max_retries {
                            retry_count += 1;

                            // Force-compress: advance the summary cache (tail → SMALL_K_FORCE,
                            // then → 0 on a second pass). History is not mutated.
                            self.force_compression(instance).await;

                            // Rebuild messages from compressed history.
                            let mut compressed_messages = self.build_messages(instance);

                            // Re-apply voice playback prompt after compression.
                            if voice_playback
                                && let Some(last_user) = compressed_messages
                                    .iter_mut()
                                    .rev()
                                    .find(|m| m.role == "user")
                            {
                                last_user.content.push_str("（语音播报模式已开启，请用简洁、便于口语播报的方式回复，避免使用代码块、表格等不适合语音的内容。）");
                            }
                            debug!(
                                "[AgentLoop] Retry {}: sending {} messages after compression",
                                retry_count,
                                compressed_messages.len()
                            );

                            let retry_tool_defs: Vec<crate::types::ToolDefinition> = self
                                .tools
                                .read()
                                .iter()
                                .map(|(name, tool)| crate::types::ToolDefinition {
                                    tool_type: "function".to_string(),
                                    function: crate::types::ToolFunctionDef {
                                        name: name.clone(),
                                        description: tool.description(),
                                        parameters: tool.parameters(),
                                    },
                                })
                                .collect();

                            // ②A：压缩重试环内调用同样包超时（挂死连接有限
                            // 重试后终局，不无限挂）。
                            match Self::chat_call_bounded(
                                self.current_provider_call_timeout_secs(),
                                &active_provider,
                                &active_model,
                                compressed_messages,
                                Some(chat_opts.clone()),
                                retry_tool_defs,
                            )
                            .await
                            {
                                Ok(resp) => {
                                    got_response = Some(resp);
                                    break;
                                }
                                Err(e) => {
                                    retry_err = e;
                                    warn!(
                                        "[AgentLoop] LLM retry {} failed: {}",
                                        retry_count, retry_err
                                    );
                                }
                            }
                        }

                        match got_response {
                            Some(resp) => resp,
                            None => {
                                warn!("[AgentLoop] All LLM retries exhausted: {}", retry_err);
                                let error_round = turns_used + 1;
                                let error_duration = round_start.elapsed();
                                self.emit_observer_sync(
                                    crate::loop_executor::ObserverEvent::LlmResponse {
                                        trace_id: trace_id.to_string(),
                                        round: error_round,
                                        duration_ms: error_duration.as_millis() as u64,
                                        has_tool_calls: false,
                                        content: format!("Error: {}", retry_err),
                                        tool_calls: vec![],
                                        tool_calls_count: 0,
                                        finish_reason: Some("error".to_string()),
                                        usage: None,
                                        raw_request_body: None,
                                        raw_response_body: None,
                                    },
                                )
                                .await;
                                instance.add_assistant_message(
                                    &format!("Error: {}", retry_err),
                                    Vec::new(),
                                    None,
                                );
                                // [capture] LLM retries exhausted (context-error
                                // retry path). Flush the full retry_err — the raw
                                // provider error, likely source of the user-visible
                                // "tools" wording — plus trace_id to correlate with
                                // request_logs/' now-complete failed round (组1).
                                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                                    sink.flush(
                                        &context.session_key,
                                        "llm_retry_exhausted",
                                        Some(trace_id),
                                        Some(retry_err.as_str()),
                                    );
                                }
                                // T10（多模态 D4 ③）：兜底提示只进用户可见
                                // 文案；retry_err 原文继续走 history /
                                // observer / capture（诊断不掺提示）。
                                let formatted =
                                    context.format_rpc_message(&append_vision_fallback_hint(
                                        format!("Error: {}", retry_err),
                                        request_had_images,
                                    ));
                                events.push(AgentEvent::Error(formatted));
                                break;
                            }
                        }
                    } else {
                        // ③ Transient-error retry (network / stream / 5xx). Retries
                        // do NOT consume turns_used — the per-iteration increment
                        // below happens once regardless of how many retries it took
                        // to get a successful response. Messages + tool_defs are
                        // rebuilt fresh because the first-attempt values were moved
                        // into the failed call.
                        let is_transient_error = [
                            "timeout",
                            "timed out",
                            "connection reset",
                            "broken pipe",
                            "connect error",
                            "connection refused",
                            "temporarily unavailable",
                            "reset by peer",
                            "502",
                            "503",
                            "504",
                            "service unavailable",
                        ]
                        .iter()
                        .any(|k| err_lower.contains(k));

                        // ③a 429 限流重试环（2026-09-17 BUG 文档裁决④⑧）：
                        // 分类词表对齐 provider 侧。与 context-compression 环
                        // 互斥（限流文案不含 token/context/length/invalid），
                        // 与 transient 词表不重叠——三环不互吞。
                        let is_rate_limit_error = RATE_LIMIT_ERROR_KEYWORDS
                            .iter()
                            .any(|k| err_lower.contains(k));

                        let mut last_err = err.clone();
                        let mut maybe_resp: Option<LlmResponse> = None;

                        if is_rate_limit_error {
                            let max_retries = self.current_rate_limit_retries().max(0) as u32;
                            let mut retries = 0u32;
                            // BUG 2026-09-21 ②B：总预算（等待 + 调用累计）——
                            // 0 = 不限；②A：单次调用超时——0 = 不设。
                            let budget_secs = self.current_rate_limit_budget_secs();
                            let call_timeout = self.current_provider_call_timeout_secs();
                            // tokio 时钟（非 std Instant）：sleep/timeout 都走
                            // tokio 定时器，预算必须同源——否则 start_paused
                            // 测试下预算永不推进，真实场景语义不变。
                            let budget_started = tokio::time::Instant::now();
                            let mut budget_exhausted = false;
                            while retries < max_retries {
                                // ②B：进入下一轮等待+调用前先看预算——预算是
                                // 硬上限，超了直接终局（ sleep 也要算进去，
                                // 所以检查放循环头顶部）。
                                if budget_secs > 0
                                    && budget_started.elapsed()
                                        >= std::time::Duration::from_secs(budget_secs)
                                {
                                    budget_exhausted = true;
                                    warn!(
                                        "[AgentLoop] rate-limit retry budget exhausted ({}s), giving up",
                                        budget_secs
                                    );
                                    break;
                                }
                                retries += 1;
                                let wait = rate_limit_wait_secs(&last_err, retries);
                                // 裁决③「显式进度」：每次重试对用户可见；B 端
                                // outbound_tx 未装配 → 静默重试。
                                self.send_retry_progress(
                                    context,
                                    format!(
                                        "⏳ 上游限流（{}），第 {retries}/{max_retries} 次重试，等待 {wait} 秒…",
                                        self.current_display_model()
                                    ),
                                )
                                .await;
                                info!(
                                    "[AgentLoop] LLM rate limited, retry {retries}/{max_retries} in {wait}s: {}",
                                    last_err
                                );
                                // BUG 2026-09-21 ①：实时快照（WSAPI
                                // agent.retry_status）——切走切回后前端占位区
                                // 可查「第 N/M 次重试」，不再是哑转圈。
                                self.set_rate_limit_status(
                                    &context.session_key,
                                    RateLimitStatus {
                                        retry: retries,
                                        max_retries,
                                        wait_secs: wait,
                                        model: self.current_display_model(),
                                        updated_at: std::time::Instant::now(),
                                    },
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                                // 消息 + tool_defs 重建（首捕值已随失败调用
                                // move，同 transient 环纪律）。
                                let r_msgs = self.build_messages(instance);
                                let r_tools: Vec<crate::types::ToolDefinition> = self
                                    .tools
                                    .read()
                                    .iter()
                                    .map(|(name, tool)| crate::types::ToolDefinition {
                                        tool_type: "function".to_string(),
                                        function: crate::types::ToolFunctionDef {
                                            name: name.clone(),
                                            description: tool.description(),
                                            parameters: tool.parameters(),
                                        },
                                    })
                                    .collect();
                                // BUG 2026-09-21 ②A：单次调用包超时（统一走
                                // chat_call_bounded）——上游 429 后连接挂起
                                // （不回响应体）时不再无限等待（实测挂死 27
                                // 分钟）；超时按该次重试失败计，走既有环
                                // （次数/预算双重有界）。
                                let call_result = Self::chat_call_bounded(
                                    call_timeout,
                                    &active_provider,
                                    &active_model,
                                    r_msgs,
                                    Some(chat_opts.clone()),
                                    r_tools,
                                )
                                .await;
                                match call_result {
                                    Ok(resp) => {
                                        maybe_resp = Some(resp);
                                        break;
                                    }
                                    Err(e) => {
                                        last_err = e;
                                        warn!(
                                            "[AgentLoop] rate-limit retry {retries}/{max_retries} failed: {}",
                                            last_err
                                        );
                                    }
                                }
                            }
                            // BUG 2026-09-21 ①：环出口统一清快照（成功 break /
                            // 次数耗尽 / 预算终局三路都过这里；E-STOP abort
                            // 路径由读侧过期兜底）。
                            self.clear_rate_limit_status(&context.session_key);
                            if maybe_resp.is_none() {
                                // 裁决④「终局诚实」：结构化标注，不再裸抛
                                // 原始错误。
                                last_err = if budget_exhausted {
                                    format!(
                                        "上游限流，重试总时长超过 {budget_secs} 秒预算（已重试 {retries} 次），停止重试"
                                    )
                                } else {
                                    format!("上游限流，已重试 {retries} 次：{}", last_err)
                                };
                            }
                        } else if is_transient_error {
                            info!(
                                "[AgentLoop] LLM transient error, retrying up to {} times: {}",
                                MAX_TRANSIENT_RETRIES, last_err
                            );
                            // P3B 撞墙检测（2026-09-12）：prev 带入首次调用的失败
                            // 时长——相邻两次失败几乎同时长（±1s）= 大概率固定
                            // 超时阈值拦截（provider timeout / 上游网关 / CC
                            // Switch 类代理的固定超时），继续盲重试只是重复烧墙。
                            // 双端真机 S2 实证：评审 LLM 连续 4 次精确 120.01s
                            // 超时——根因 anthropic lane 默认 120s（已另行根修
                            // 为全 lane 600s + per-model timeout_secs 覆盖）。
                            let mut prev_fail_after = Some(first_call_failed_after);
                            let mut retries = 0u32;
                            // ②A：transient 环内调用同样包超时——挂死的
                            // 「真网络等待」形态不再把有限重试环变成无限挂。
                            let call_timeout = self.current_provider_call_timeout_secs();
                            // 退避资格（2026-09-18 CI 实调）：失败耗时 <1s =
                            // 上游**立即**拒绝（连接拒绝 / DNS / mock 类）——
                            // sleep 无意义（门已关上，等待解决不了），立即重试；
                            // ≥1s = 真网络等待（上游在挣扎），按 1s/2s/4s 退避
                            // 才有效。不设门槛的副作用实锤：integration
                            // gateway/concurrent 5 会话共享 legacy session 串行
                            // 处理，mock 立即失败链从 40ms 膨胀到 8s，5×8s 推出
                            // 30s 窗口（首挂实录；历史 3 连 PASS）。与 P3B 撞墙
                            // 检测同哲学：立即失败的重试无效信号更早暴露。
                            let mut backoff_eligible =
                                first_call_failed_after >= std::time::Duration::from_secs(1);
                            while retries < MAX_TRANSIENT_RETRIES {
                                retries += 1;
                                // 429 文档关联隐患 1（可重试类统一设计）：transient
                                // 环对真网络等待加 1s/2s/4s 小退避——此前无 sleep
                                // 立即重发，对上游抖动基本无效。立即拒绝（见
                                // backoff_eligible）不退避。测试经 tokio
                                // start_paused 自动推进，零等待。
                                if backoff_eligible {
                                    tokio::time::sleep(std::time::Duration::from_secs(
                                        1u64 << (retries - 1).min(2),
                                    ))
                                    .await;
                                }
                                let r_msgs = self.build_messages(instance);
                                let r_tools: Vec<crate::types::ToolDefinition> = self
                                    .tools
                                    .read()
                                    .iter()
                                    .map(|(name, tool)| crate::types::ToolDefinition {
                                        tool_type: "function".to_string(),
                                        function: crate::types::ToolFunctionDef {
                                            name: name.clone(),
                                            description: tool.description(),
                                            parameters: tool.parameters(),
                                        },
                                    })
                                    .collect();
                                let attempt_start = std::time::Instant::now();
                                match Self::chat_call_bounded(
                                    call_timeout,
                                    &active_provider,
                                    &active_model,
                                    r_msgs,
                                    Some(chat_opts.clone()),
                                    r_tools,
                                )
                                .await
                                {
                                    Ok(resp) => {
                                        maybe_resp = Some(resp);
                                        break;
                                    }
                                    Err(e) => {
                                        last_err = e;
                                        let failed_after = attempt_start.elapsed();
                                        // 下一轮退避资格按本次实际耗时重判
                                        // （立即拒绝 ↔ 真等待可能在重试间切换）。
                                        backoff_eligible =
                                            failed_after >= std::time::Duration::from_secs(1);
                                        if let Some(prev) = prev_fail_after
                                            && prev.abs_diff(failed_after)
                                                <= std::time::Duration::from_secs(1)
                                        {
                                            warn!(
                                                "[AgentLoop] ⚠ 疑似固定超时阈值拦截：相邻两次失败时长几乎相同（前次 {:.1}s / 本次 {:.1}s），重试大概率无效——检查模型条目 timeout_secs / provider 超时与上游可用性，而不是继续重试。最后错误: {}",
                                                prev.as_secs_f32(),
                                                failed_after.as_secs_f32(),
                                                last_err
                                            );
                                        }
                                        prev_fail_after = Some(failed_after);
                                        warn!(
                                            "[AgentLoop] transient retry {}/{} failed: {}",
                                            retries, MAX_TRANSIENT_RETRIES, last_err
                                        );
                                    }
                                }
                            }
                        }

                        if let Some(resp) = maybe_resp {
                            resp
                        } else {
                            // Non-transient error, or transient retries exhausted.
                            warn!("[AgentLoop] LLM call failed: {}", last_err);
                            let error_round = turns_used + 1;
                            let error_duration = round_start.elapsed();
                            self.emit_observer_sync(
                                crate::loop_executor::ObserverEvent::LlmResponse {
                                    trace_id: trace_id.to_string(),
                                    round: error_round,
                                    duration_ms: error_duration.as_millis() as u64,
                                    has_tool_calls: false,
                                    content: format!("Error: {}", last_err),
                                    tool_calls: vec![],
                                    tool_calls_count: 0,
                                    finish_reason: Some("error".to_string()),
                                    usage: None,
                                    raw_request_body: None,
                                    raw_response_body: None,
                                },
                            )
                            .await;
                            instance.add_assistant_message(
                                &format!("Error: {}", last_err),
                                Vec::new(),
                                None,
                            );
                            // [capture] Non-transient error or transient retries
                            // exhausted. Flush full last_err + trace_id.
                            if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                                sink.flush(
                                    &context.session_key,
                                    "llm_call_failed",
                                    Some(trace_id),
                                    Some(last_err.as_str()),
                                );
                            }
                            // T10（多模态 D4 ③）：同上——提示只进用户可见
                            // 文案，last_err 原文继续走 history / observer /
                            // capture。
                            let formatted =
                                context.format_rpc_message(&append_vision_fallback_hint(
                                    format!("Error: {}", last_err),
                                    request_had_images,
                                ));
                            events.push(AgentEvent::Error(formatted));
                            break;
                        }
                    }
                }
            };

            // K1b (U14): LLM-call-level post hooks — the「拦思考」layer. Runs
            // AFTER the built-in error recovery, BEFORE the LlmResponse
            // observer event / turns_used increment, so every downstream
            // consumer (observer, usage, tool execution) sees the final
            // decision. Retry re-calls carry the same cancel/e-stop select
            // guard and emit their own LlmRequest observer event (visible in
            // request_log; no extra T8 ledger record — same shape as the
            // built-in transient retries).
            {
                let llm_hooks = self.llm_hooks.read().snapshot();
                if !llm_hooks.is_empty() {
                    let mut hook_retries: u32 = 0;
                    loop {
                        let hook_call = crate::hooks::HookLlmCall {
                            model: active_model.clone(),
                            session_key: context.session_key.clone(),
                            round: turns_used as usize + 1,
                        };
                        // Pass a clone: on fail-open paths (budget exhausted /
                        // retry call failed) `response` still holds the
                        // original and needs no restore.
                        match crate::hooks::run_llm_post_hooks(
                            &llm_hooks,
                            &hook_call,
                            response.clone(),
                        )
                        .await
                        {
                            crate::hooks::PostLlmOutcome::Allow(final_resp) => {
                                response = final_resp;
                                break;
                            }
                            crate::hooks::PostLlmOutcome::Block { reason } => {
                                warn!(
                                    "[AgentLoop] LLM hook blocked the response, turns_used={}: {}",
                                    turns_used, reason
                                );
                                events.push(AgentEvent::Done(format!(
                                    "⛔ HOOK BLOCKED [layer:hook|policy:llm_hook] {} — A registered LLM hook terminated this round. Inform the user.",
                                    reason
                                )));
                                break 'turn;
                            }
                            crate::hooks::PostLlmOutcome::Retry { reason } => {
                                if hook_retries >= crate::hooks::MAX_LLM_HOOK_RETRIES {
                                    warn!(
                                        "[AgentLoop] LLM hook retry budget exhausted ({}), allowing the previous response",
                                        crate::hooks::MAX_LLM_HOOK_RETRIES
                                    );
                                    break;
                                }
                                hook_retries += 1;
                                // Regenerate: rebuild messages (the first
                                // attempt's were moved into the call), append
                                // the hook feedback, re-call under the same
                                // cancel/e-stop guard.
                                let mut r_msgs = self.build_messages(instance);
                                r_msgs.push(LlmMessage {
                                    role: "system".to_string(),
                                    content: format!("# Hook feedback: {}", reason),
                                    tool_calls: None,
                                    tool_call_id: None,
                                    reasoning_content: None,
                                    images: Vec::new(),
                                });
                                // Y1 (Phase4-a): fold the retry call's defs with
                                // the same gates/rendering as the main call —
                                // same query ⇒ same fold bytes.
                                let r_tools = self.effective_tool_defs(instance);
                                self.emit_observer_sync(
                                    crate::loop_executor::ObserverEvent::LlmRequest {
                                        trace_id: trace_id.to_string(),
                                        round: turns_used + 1,
                                        model: active_model.clone(),
                                        messages_count: r_msgs.len(),
                                        tools_count: r_tools.len(),
                                        messages: observer_msg_values(&r_msgs),
                                        tools: r_tools
                                            .iter()
                                            .filter_map(|t| serde_json::to_value(t).ok())
                                            .collect(),
                                        provider_name: String::new(),
                                        api_key: String::new(),
                                        api_base: String::new(),
                                    },
                                )
                                .await;
                                let mut r_estop_rx =
                                    self.estop.read().as_ref().map(|e| e.subscribe());
                                // ②A：hook 重呼同样包超时（cancel/estop 臂不
                                // 变，仍可打断超时等待）。
                                let r = tokio::select! {
                                    res = Self::chat_call_bounded(
                                        self.current_provider_call_timeout_secs(),
                                        &active_provider,
                                        &active_model,
                                        r_msgs,
                                        Some(chat_opts.clone()),
                                        r_tools,
                                    ) => res,
                                    _ = cancel_token.cancelled() => {
                                        info!("[AgentLoop] Hook retry call cancelled, turns_used={}", turns_used);
                                        events.push(AgentEvent::Done("已取消".to_string()));
                                        break 'turn;
                                    }
                                    _ = Self::wait_estop_engaged(r_estop_rx.as_mut()) => {
                                        info!(
                                            "[AgentLoop] E-stop engaged during hook retry call, turns_used={}",
                                            turns_used
                                        );
                                        events.push(AgentEvent::Done(
                                            "⛔ 已急停 (E-STOP) — LLM 调用已中断。发送 `nemesisbot estop --release` 恢复。"
                                                .to_string(),
                                        ));
                                        break 'turn;
                                    }
                                };
                                match r {
                                    Ok(resp) => {
                                        info!(
                                            "[AgentLoop] Hook retry {} succeeded, re-checking response",
                                            hook_retries
                                        );
                                        response = resp;
                                        continue;
                                    }
                                    Err(e) => {
                                        // Fail-open: keep the response the hook
                                        // rejected — a failed re-call must not
                                        // lose the round's only answer.
                                        warn!(
                                            "[AgentLoop] Hook retry call failed, keeping previous response: {}",
                                            e
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            turns_used += 1;

            // Emit LLM response observer event.
            let round_duration = round_start.elapsed();
            let tc_values: Vec<serde_json::Value> = response
                .tool_calls
                .iter()
                .filter_map(|tc| serde_json::to_value(tc).ok())
                .collect();
            let tc_count = response.tool_calls.len();
            self.emit_observer_sync(crate::loop_executor::ObserverEvent::LlmResponse {
                trace_id: trace_id.to_string(),
                round: turns_used,
                duration_ms: round_duration.as_millis() as u64,
                has_tool_calls: !response.tool_calls.is_empty(),
                content: response.content.clone(),
                tool_calls: tc_values,
                tool_calls_count: tc_count,
                finish_reason: if response.finished {
                    Some("stop".to_string())
                } else {
                    None
                },
                usage: response.usage.clone(),
                raw_request_body: response.raw_request_body.take(),
                raw_response_body: response.raw_response_body.take(),
            })
            .await;

            // Record usage statistics if data store is available.
            if let Some(ref ds) = self.data_store
                && let Some(ref usage) = response.usage
            {
                let model_name = self.active_model.read().clone();
                let cache_creation = usage.cache_creation_tokens.unwrap_or(0);
                let cache_read = usage.cache_read_tokens.or(usage.cached_tokens).unwrap_or(0);
                // A3：分项计价 + 实际命中条目名（未命中 → 空名 + 全 0，
                // 明细行可区分「未命中」与「命中免费条目」）。
                let breakdown = ds.compute_cost_breakdown(
                    &model_name,
                    usage.prompt_tokens,
                    usage.completion_tokens,
                    cache_creation,
                    cache_read,
                );
                let empty = nemesis_data::CostBreakdown::default();
                let bd = breakdown.as_ref().unwrap_or(&empty);
                let log = nemesis_data::RequestLog {
                    id: 0,
                    trace_id: trace_id.to_string(),
                    model: model_name.clone(),
                    provider_type: String::new(),
                    input_tokens: usage.prompt_tokens,
                    output_tokens: usage.completion_tokens,
                    cache_creation_tokens: cache_creation,
                    cache_read_tokens: cache_read,
                    total_cost_usd: bd.total_cost_usd,
                    latency_ms: round_duration.as_millis() as i64,
                    status_code: if response.content.starts_with("Error:") {
                        500
                    } else {
                        200
                    },
                    error_message: None,
                    is_streaming: false,
                    created_at: chrono::Local::now().timestamp(),
                    pricing_model: bd.pricing_model.clone(),
                    input_cost_usd: bd.input_cost_usd,
                    output_cost_usd: bd.output_cost_usd,
                    cache_creation_cost_usd: bd.cache_creation_cost_usd,
                    cache_read_cost_usd: bd.cache_read_cost_usd,
                    // provider trait 无流式通路，TTFT 无从测量（列留 NULL）。
                    first_token_ms: None,
                    session_key: context.session_key.clone(),
                };
                if let Err(e) = ds.insert_request_log(&log) {
                    tracing::warn!("[AgentLoop] Failed to record usage: {e}");
                }
            }

            // Continue-generation on max_tokens truncation.
            // When completion hits the cap, output is cut mid-way —
            // often mid tool-call JSON, which args_validator would report as
            // "Arguments are not valid JSON" and burn the validation budget
            // (Big tier = 0 retries → instant force-stop with a misleading
            // error). Detect it here: drop the truncated tool calls (executing
            // them would write a partial file / run half-formed args), keep
            // partial content, append a "continue" prompt, re-loop. Bounded so
            // a genuinely too-large file surfaces a clear error, not a loop.
            // NOTE: detection assumes chat_opts.max_tokens is Some — the agent
            // always sets Some(8192) when building chat_opts. If that ever
            // becomes None (provider's own default cap), gate on is_some():
            // the 8192 fallback could false-positive against a higher cap.
            let token_cap = chat_opts.max_tokens.unwrap_or(8192) as u64;
            let hit_cap = response
                .usage
                .as_ref()
                .map(|u| (u.completion_tokens as u64) >= token_cap)
                .unwrap_or(false);
            if hit_cap {
                if length_continuations < MAX_LENGTH_CONTINUATIONS {
                    length_continuations += 1;
                    warn!(
                        "[AgentLoop] response truncated at max_tokens cap ({token_cap}); \
                         continue-generation {length_continuations}/{MAX_LENGTH_CONTINUATIONS}"
                    );
                    instance.add_assistant_message(
                        &response.content,
                        Vec::new(),
                        response.reasoning_content.clone(),
                    );
                    instance.add_user_message(
                        "Output limit reached. Continue exactly where you left off — \
                         no recap, no apology. If you were writing a large file, \
                         break it into smaller writes.",
                    );
                    continue;
                }
                // Budget exhausted: clear, non-misleading error.
                warn!(
                    "[AgentLoop] length-continuation budget exhausted; \
                     output keeps exceeding max_tokens ({token_cap})"
                );
                let notice = format!(
                    "输出反复超过 max_tokens 上限（{token_cap}）被截断，文件可能太大。\
                     请调大 max_tokens，或让我分段写入。"
                );
                instance.add_assistant_message(&notice, Vec::new(), None);
                events.push(AgentEvent::Error(context.format_rpc_message(&notice)));
                break;
            }
            // Complete (non-truncated) response — reset the counter.
            length_continuations = 0;

            // ⑧ Cross-round prose repetition: if the model's content is
            // near-identical to the previous round's, queue a transient nudge
            // for the next build. Catches "saying the same thing while churning
            // tools" — a loop ⑥ cannot see (it watches tool results, not prose).
            if let Some(nudge) = turn_guard.check_text_repetition(&response.content) {
                info!("[AgentLoop] loop guard: response content repeating across rounds; nudging");
                repetition_nudge_pending = Some(nudge);
            } else {
                repetition_nudge_pending = None;
            }

            if response.tool_calls.is_empty() || response.finished {
                // No tool calls: candidate final response. ⑦ Check for degenerate
                // (empty / whitespace-only / reasoning-only) content and nudge
                // the model to retry before accepting. Skipped for heartbeat —
                // an empty heartbeat response means "nothing to do", a valid
                // outcome, not a broken answer.
                let content = response.content.clone();
                if context.user == "heartbeat" {
                    instance.add_assistant_message(
                        &content,
                        Vec::new(),
                        response.reasoning_content.clone(),
                    );
                    let formatted = context.format_rpc_message(&content);
                    events.push(AgentEvent::Done(formatted));
                    break;
                }
                match turn_guard.check_final_answer(&content) {
                    crate::turn_guard::FinalAnswerVerdict::Accept => {
                        instance.add_assistant_message(
                            &content,
                            Vec::new(),
                            response.reasoning_content.clone(),
                        );
                        // I1 (U7) turn escape hatch: the model is about to
                        // finish, but an unclaimed steer message arrived in
                        // the last moments — hand it to the model for one
                        // more round instead of answering past it — pending
                        // next-step input keeps the turn open. At most once
                        // per turn
                        // (steer_escape_used) so `!`-spam cannot loop the
                        // turn forever.
                        if !steer_escape_used
                            && self.inbox.has_next_step(&context.session_key)
                            && self.concurrent_mode == ConcurrentMode::Steer
                        {
                            steer_escape_used = true;
                            info!(
                                "[AgentLoop] escape hatch: pending steer at turn end, one more round"
                            );
                            // Loop again — the claim at the top of the next
                            // iteration injects the steer message(s).
                            continue;
                        }
                        // K2 (U14): turn-end lifecycle hooks (Stop
                        // dialect event). Runs after the assistant message
                        // is recorded, before the Done event. `Continue`
                        // injects the hook feedback as a user message and
                        // grants one more round, bounded by
                        // MAX_TURN_END_CONTINUES (exhausted → stop anyway,
                        // fail-open). Only the normal Accept path fires —
                        // heartbeat (its own branch above), GiveUp and
                        // error/stop paths do not.
                        {
                            let lifecycle = self.lifecycle_hooks.read().snapshot();
                            if !lifecycle.is_empty() {
                                let end = crate::hooks::HookTurnEnd {
                                    session_key: context.session_key.clone(),
                                    channel: context.channel.clone(),
                                    chat_id: context.chat_id.clone(),
                                    final_content: content.clone(),
                                    stop_hook_active: turn_end_continues > 0,
                                };
                                if let crate::hooks::TurnEndDecision::Continue { feedback } =
                                    crate::hooks::run_turn_end_hooks(&lifecycle, &end).await
                                {
                                    if turn_end_continues < crate::hooks::MAX_TURN_END_CONTINUES {
                                        turn_end_continues += 1;
                                        info!(
                                            "[AgentLoop] turn-end hook blocked stopping \
                                             ({}/{}, session '{}') — one more round",
                                            turn_end_continues,
                                            crate::hooks::MAX_TURN_END_CONTINUES,
                                            context.session_key
                                        );
                                        instance.add_user_message(&feedback);
                                        continue 'turn;
                                    }
                                    warn!(
                                        "[AgentLoop] turn-end hook keeps blocking stop after {} \
                                         continues; stopping anyway (fail-open), session '{}'",
                                        crate::hooks::MAX_TURN_END_CONTINUES,
                                        context.session_key
                                    );
                                }
                            }
                        }
                        let formatted = context.format_rpc_message(&content);
                        events.push(AgentEvent::Done(formatted));
                        break;
                    }
                    crate::turn_guard::FinalAnswerVerdict::RetryWithNudge(nudge) => {
                        warn!(
                            "[AgentLoop] degenerate final answer (empty/no visible text); nudging retry"
                        );
                        // Record the empty attempt in history, then queue the
                        // nudge for transient re-injection on the next build.
                        instance.add_assistant_message(
                            &content,
                            Vec::new(),
                            response.reasoning_content.clone(),
                        );
                        degenerate_nudge_pending = Some(nudge);
                        continue;
                    }
                    crate::turn_guard::FinalAnswerVerdict::GiveUp(notice) => {
                        warn!(
                            "[AgentLoop] degenerate final answer retry budget exhausted; giving up"
                        );
                        instance.add_assistant_message(&notice, Vec::new(), None);
                        let formatted = context.format_rpc_message(&notice);
                        events.push(AgentEvent::Done(formatted));
                        break;
                    }
                }
            }

            // Model produced tool calls → it is making progress. Clear any
            // pending degenerate-answer nudge (⑦) so it stops nagging while the
            // model works — tool work is the opposite of a degenerate empty
            // final answer.
            degenerate_nudge_pending = None;

            // Record the assistant's response with tool calls.
            let tool_calls = response.tool_calls.clone();
            let assistant_content = response.content.clone();
            instance.add_assistant_message(
                &assistant_content,
                tool_calls.clone(),
                response.reasoning_content.clone(),
            );
            // R1（2026-09-21）：中间轮正文非空才发布——模型多步执行时每轮的
            // 过程叙述（「我先看下 X 再改 Y」）此前只进 history，前端看不见；
            // 空正文轮（纯工具调用）无可读内容，不发。
            if !assistant_content.trim().is_empty() {
                self.emit_round_text(&context.session_key, &context.chat_id, &assistant_content);
            }
            events.push(AgentEvent::ToolCall(tool_calls.clone()));

            // Execute each tool call.
            instance.set_state(crate::types::AgentState::ExecutingTool);
            let mut hit_async = false;
            // Outer-scope turn-stop latch. Set inside the tool-call for-loop
            // (where `break` can only exit the batch, not the outer LLM loop) by
            // ⑥ escalation OR validation-budget exhaustion. Checked after the
            // for-loop to actually end the turn. Without this two-step, those
            // `break`s only stopped the current batch and the model was called
            // again — escalation fired every round without stopping (observed
            // 43× in a deployed test), and "validation stopping loop" was a lie.
            let mut force_stop: Option<AgentEvent> = None;
            // U5 (sixth batch): precompute execution for an ALL-parallel-safe
            // batch (≥2 calls, every tool read-only OR explicitly opted in —
            // G3: spawn). The for-loop then replays the serial guards on the
            // precomputed results in source order — the audit chain stays
            // ordered = model source order (roadmap risk 3). cluster_rpc/exec
            // /writers are never parallel-safe → this stays None for those
            // batches → the loop below runs byte-identical to pre-U5.
            // `None` also when a cancel/estop is already engaged at batch
            // start (the for-loop's per-item check handles that case
            // unchanged).
            let precomputed: Option<Vec<PrecomputedTool>> = if tool_calls.len() >= 2
                && !cancel_token.is_cancelled()
                && !self
                    .estop
                    .read()
                    .as_ref()
                    .map(|e| e.is_engaged())
                    .unwrap_or(false)
                && tool_calls
                    .iter()
                    .all(|tc| self.tool_is_parallel_safe(&tc.name))
            {
                let pc = self
                    .precompute_parallel_batch(&tool_calls, context, instance.detached_depth())
                    .await;
                Some(pc)
            } else {
                None
            };
            // U5: in the parallel path, cancel/estop are NOT re-checked per item
            // — the batch was checkpointed non-cancelled above and runs to
            // completion (goal §四 documented semantic: a cancel arriving during
            // the parallel window takes effect on the NEXT turn, not mid-batch).
            // The serial path (precomputed.is_none()) keeps the per-item checks
            // byte-identical.
            let skip_cancel_estop = precomputed.is_some();
            for (batch_idx, tc) in tool_calls.iter().enumerate() {
                // Check cancellation before each tool execution.
                if !skip_cancel_estop && cancel_token.is_cancelled() {
                    info!(
                        "[AgentLoop] LLM loop cancelled before tool execution: {}, turns_used={}",
                        tc.name, turns_used
                    );
                    events.push(AgentEvent::Done("已取消".to_string()));
                    break;
                }

                // 全局急停检查：触发则拒绝后续工具调用并结束当前轮。
                let estop_engaged = self
                    .estop
                    .read()
                    .as_ref()
                    .map(|e| e.is_engaged())
                    .unwrap_or(false);
                if estop_engaged {
                    info!(
                        "[AgentLoop] E-stop engaged before tool execution: {}, turns_used={}",
                        tc.name, turns_used
                    );
                    events.push(AgentEvent::Done(
                        "⛔ 已急停 (E-STOP) — 工具调用已拒绝。发送 `nemesisbot estop --release` 恢复。"
                            .to_string(),
                    ));
                    break;
                }

                let tool_start = std::time::Instant::now();
                // Phase 2 (small-model-tool-robustness): validate args against
                // the tool's schema before dispatch. Catches B-class failures;
                // auto-fixes high-confidence field-name typos (edit distance ≤2);
                // otherwise bounces a structured error back to the model so it
                // can self-correct on the next round.
                //
                // U5 (sixth batch): when `precomputed` is Some, the execution
                // already ran concurrently (above) — replay its result + the
                // `validation_failures` counter increment here, then fall
                // through to the SAME serial guards (observer/capture/
                // turn_guard/spill/escalation). Guards run in source order
                // because join_all preserves iteration order. `tool_duration`
                // carries the REAL per-task wall time (measured in the pool),
                // not this near-zero clone.
                let (result, tool_duration_ms) = if let Some(ref pc) = precomputed {
                    let p = &pc[batch_idx];
                    if p.validation_failed {
                        validation_failures += 1;
                        self.record_tool_validation_stats(true);
                    } else {
                        validation_failures = 0;
                        self.record_tool_validation_stats(false);
                    }
                    (p.result.clone(), p.duration_ms)
                } else {
                    let r = match self.check_tool_args(tc) {
                        crate::args_validator::Outcome::Valid => {
                            validation_failures = 0;
                            self.record_tool_validation_stats(false);
                            // G2: dispatch at this instance's sub-agent depth so
                            // depth-aware tools (spawn) enforce max_depth.
                            self.handle_tool_call_at_depth(tc, context, instance.detached_depth())
                                .await
                        }
                        crate::args_validator::Outcome::Fixed(fixed_args) => {
                            validation_failures = 0;
                            self.record_tool_validation_stats(false);
                            info!(
                                "[AgentLoop] Auto-fixed args for tool '{}' (id={})",
                                tc.name, tc.id
                            );
                            let mut fixed = tc.clone();
                            fixed.arguments = fixed_args;
                            self.handle_tool_call_at_depth(
                                &fixed,
                                context,
                                instance.detached_depth(),
                            )
                            .await
                        }
                        crate::args_validator::Outcome::Invalid { message, class } => {
                            validation_failures += 1;
                            self.record_tool_validation_stats(true);
                            warn!(
                                "[AgentLoop] Arg validation failed for tool '{}' (id={}, class={}): {}",
                                tc.name, tc.id, class, message
                            );
                            format!("Tool error: {}", message)
                        }
                    };
                    (r, tool_start.elapsed().as_millis() as u64)
                };
                let tool_duration = std::time::Duration::from_millis(tool_duration_ms);
                let tool_success =
                    !result.starts_with("Error:") && !result.starts_with("Tool error:");

                // Emit tool call observer event.
                self.emit_observer_sync(crate::loop_executor::ObserverEvent::ToolCall {
                    trace_id: trace_id.to_string(),
                    tool_name: tc.name.clone(),
                    success: tool_success,
                    duration_ms: tool_duration.as_millis() as u64,
                    round: turns_used,
                    arguments: tc.arguments.clone(),
                    result: result.clone(),
                })
                .await;

                // [capture] Record the full pre-truncation tool result. loop.rs
                // does NOT truncate tool results before they enter the context,
                // so this is what catches a bloated output blowing out the
                // context window (the suspected bug trigger). No-op unless
                // capture is enabled; flushed only on a later failure signal.
                if let Some(sink) = crate::capture_sink::CaptureSink::global() {
                    sink.record_tool(
                        &context.session_key,
                        crate::capture_sink::ToolCapture {
                            tool_name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                            result: result.clone(),
                            success: tool_success,
                            duration_ms: tool_duration.as_millis() as u64,
                            error: if tool_success {
                                String::new()
                            } else {
                                result.clone()
                            },
                            llm_round: turns_used as usize,
                            ts: String::new(),
                        },
                    );
                }

                // Check for async cluster_rpc result — save continuation snapshot.
                //
                // Plan C (template-based UX): the cluster_rpc tool encodes the
                // peer's display name as the 4th part of the marker so we can
                // render a human-friendly "waiting" message here without an
                // extra cluster lookup (this crate can't depend on
                // nemesis-cluster). The full LLM-generated persona response
                // was deferred — it would double cross-node latency and
                // complicate the continuation snapshot. See loop_tools.rs
                // for the encoding site.
                //
                // Format: `__ASYNC__:{task_id}:{target_id}:{target_name}`
                // Older senders may omit the name part (3-segment format),
                // in which case we fall back to the bare target_id.
                if result.starts_with("__ASYNC__:") {
                    let parts: Vec<String> = result.splitn(4, ':').map(|s| s.to_string()).collect();
                    if parts.len() >= 3 {
                        let task_id = parts[1].clone();
                        let target_id = parts[2].clone();
                        let target_name = parts
                            .get(3)
                            .cloned()
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| target_id.clone());
                        if let Some(ref mgr) = self.continuation_manager {
                            // Get messages up to this point (including the assistant's tool_call).
                            // We use build_messages() to convert history → LlmMessage format.
                            let messages = self.build_messages(instance);
                            let channel = context.channel.clone();
                            let chat_id = context.chat_id.clone();
                            let session_key = context.session_key.clone();
                            // T6（多模态）：快照只落图片路径引用（磁盘侧剥离
                            // base64 字节）。引用取自 instance 历史最后一条
                            // user turn（即本 turn 的 image_refs，:3468 写入）。
                            let image_refs: Vec<String> = instance
                                .get_history()
                                .iter()
                                .rev()
                                .find(|t| t.role == "user")
                                .map(|t| t.image_refs.clone())
                                .unwrap_or_default();

                            // Save continuation snapshot (spawns a tokio task for disk write)
                            let mgr = mgr.clone();
                            let tc_id = tc.id.clone();
                            let msgs = messages.clone();
                            let task_id_spawn = task_id.clone();
                            // G5: 对端 ID 随快照落盘 —— A 侧重启后 first_start
                            // 恢复 pending 任务时才知道 poll 该问谁。
                            let peer_id_spawn = target_id.clone();
                            tokio::spawn(async move {
                                mgr.save_continuation_with_images(
                                    &task_id_spawn,
                                    msgs,
                                    &tc_id,
                                    &channel,
                                    &chat_id,
                                    &session_key,
                                    &peer_id_spawn,
                                    &image_refs,
                                )
                                .await;
                            });

                            info!(
                                "[AgentLoop] Continuation saved for async cluster_rpc: task_id={}, tool_call_id={}",
                                task_id, tc.id
                            );
                        }

                        // Return an intermediate message to the user and stop processing.
                        // The continuation will resume when the callback arrives.
                        //
                        // NOTE: `is_async_done` in nemesisbot/src/cluster_agent.rs detects
                        // this async path via the `__CLUSTER_ASYNC__` marker in conversation
                        // history, NOT by matching this message text. So the wording here
                        // is free to change without breaking multi-hop (A→B→C) detection.
                        //
                        // The template is deliberately persona-agnostic — this code has
                        // no knowledge of which AI identity is currently loaded (IDENTITY.md
                        // is applied at the LLM layer, not here). Address terms like "老爷"
                        // belong in the persona file, not in hardcoded system messages.
                        // The task_id is omitted from user-visible copy — it's an internal
                        // correlation ID with no meaning to the user.
                        let intermediate = format!("已经联系 {} 了，稍等~", target_name);
                        instance.add_tool_result(&tc.id, &format!(
                            "Request accepted by {}. Task ID: {} | __CLUSTER_ASYNC__{{\"task_id\":\"{}\",\"target\":\"{}\"}}",
                            target_id, task_id, task_id, target_id
                        ));

                        let formatted = context.format_rpc_message(&intermediate);
                        events.push(AgentEvent::Done(formatted));
                        hit_async = true;
                        break;
                    }
                }

                // G4 (devtool-upgrade 阶段 3)：后台 subagent —— spawn 闭包
                // （background=true）已在闭包侧把任务转入后台 tokio 任务并
                // 立即返回此 marker。这里与 __ASYNC__（集群）同构：存续行
                // 快照 + 中间消息收尾本回合；任务完成时闭包侧向 bus 发布
                // `subagent_continuation:{task_id}`，gate_inbound 拦截后走
                // dispatch_continuation → handle_cluster_continuation 全复用
                // （单飞闸认领 + 快照加载 + 续行 + 持久化 + finish_handling
                // 收口自清——磁盘快照保留到最终回复持久化后，发现 F 2026-09-11）。
                //
                // 快照保存必须 **inline await**（不能像 __ASYNC__ 那样
                // spawn）：后台子代理毫秒级即可完成并回灌，spawn 式保存要
                // 等下一个 await 点才落内存，load 端扑空即静默且回复
                // （handle_cluster_continuation 的 debug-skip 分支）。inline
                // 消除该竞态；一次小盘写的延迟可忽略。
                //
                // 格式：`__BG_SPAWN__:{task_id}`（编码端 = agent_factory 注入
                // 的 spawn 闭包）。
                if let Some(bg_task_id) = result.strip_prefix("__BG_SPAWN__:") {
                    let bg_task_id = bg_task_id.trim().to_string();
                    if let Some(ref mgr) = self.continuation_manager {
                        let messages = self.build_messages(instance);
                        let channel = context.channel.clone();
                        let chat_id = context.chat_id.clone();
                        let session_key = context.session_key.clone();
                        // T6（多模态）：快照只落图片路径引用。引用取自
                        // instance 历史最后一条 user turn（同 __ASYNC__ 路径）。
                        let image_refs: Vec<String> = instance
                            .get_history()
                            .iter()
                            .rev()
                            .find(|t| t.role == "user")
                            .map(|t| t.image_refs.clone())
                            .unwrap_or_default();
                        // peer_id 留空：后台任务是进程内 tokio 任务，无对端
                        // 可 poll —— cluster first_start 恢复循环对空 peer_id
                        // 快照本就跳过（留给 TTL 清扫）；重启丢失场景由适配器
                        // 启动期的诚实丢失注入兜底（见 nemesisbot adapters）。
                        mgr.save_continuation_with_images(
                            &bg_task_id,
                            messages,
                            &tc.id,
                            &channel,
                            &chat_id,
                            &session_key,
                            "",
                            &image_refs,
                        )
                        .await;
                        info!(
                            "[AgentLoop] Continuation saved for background subagent: task_id={}, tool_call_id={}",
                            bg_task_id, tc.id
                        );
                    }

                    // 工具结果 marker：与 __CLUSTER_ASYNC__ 刻意不同名，两套
                    // 异步路径在会话历史里互不误判。
                    instance.add_tool_result(&tc.id, &format!(
                        "Background sub-agent accepted. Task ID: {} | __BG_ASYNC__{{\"task_id\":\"{}\"}}",
                        bg_task_id, bg_task_id
                    ));

                    let intermediate =
                        "已派后台子代理任务，完成后结果会自动带回本会话~".to_string();
                    let formatted = context.format_rpc_message(&intermediate);
                    events.push(AgentEvent::Done(formatted));
                    hit_async = true;
                    break;
                }

                let tool_result = ToolCallResult {
                    tool_name: tc.name.clone(),
                    result: result.clone(),
                    is_error: false,
                };
                events.push(AgentEvent::ToolResult(tool_result));

                // ⑤/⑥ Loop guards — mutually exclusive per call (success vs error).
                // Use the shared helper so ExecTool's `Ok("Exit code: N")` for
                // non-zero exits is detected as a failure — otherwise build
                // loops look like success and the guards never fire.
                let tool_succeeded = !crate::turn_guard::tool_result_indicates_error(&result);

                // ⑤ Repeat-success guard: a write-like tool succeeding with
                // identical args is a no-op / write loop → append a nudge.
                //
                // NOTE: keys on `tc.arguments` (the model's ORIGINAL args), not
                // the validator's auto-fixed args. Intentional — if the model
                // keeps re-sending the same (typo'd) args, that IS the repeat
                // we want to catch, regardless of the per-call auto-fix.
                // Detection stays consistent; the signature is the pre-fix form.
                // X1 (U3 projection prune): the pure tool output, kept for
                // history. The ⑤/⑤′/⑥ guard nudges below decorate only the
                // MODEL-facing text; `guard_nudged` marks that a decoration
                // fired (the decorated form cannot be recomputed from the
                // original later, so it must be recorded as the projection
                // override — see the gate block below).
                let original_result = result.clone();
                let mut guard_nudged = false;
                let result = if tool_succeeded {
                    match turn_guard.record_write_success(&tc.name, &tc.arguments) {
                        Some(nudge) => {
                            info!(
                                "[AgentLoop] loop guard: '{}' repeated an identical write; nudging",
                                tc.name
                            );
                            guard_nudged = true;
                            format!("{}\n{}", result, nudge)
                        }
                        None => result,
                    }
                } else {
                    result
                };

                // ⑤′ Read-side repeat guard: a non-write tool succeeding with
                // identical args repeatedly is a re-query loop — the model is
                // not consuming the result it already has. Advisory nudge only
                // (never blocks). Same NOTE as ⑤ above: keys on the model's
                // ORIGINAL args, not the auto-fixed form.
                let result = if tool_succeeded {
                    match turn_guard.record_read_success(&tc.name, &tc.arguments) {
                        Some(nudge) => {
                            info!(
                                "[AgentLoop] loop guard: '{}' repeated an identical read; nudging",
                                tc.name
                            );
                            guard_nudged = true;
                            format!("{}\n{}", result, nudge)
                        }
                        None => result,
                    }
                } else {
                    result
                };

                // C3 (devtool-upgrade 阶段 2) 编辑后诊断回灌：write_file /
                // edit_file 成功后，若该路径语言有已安装 LSP 且
                // `agents.defaults.diagnostics_loop.enabled`，同步文档 →
                // 等 ERROR → 把 ≤max_errors 条追加到工具结果尾部（"please
                // fix"），让模型同轮自纠——修复闭环。插在 ⑤′ 与 spill/gate
                // 之间：反馈与工具结果同走一条模型可见管线（gate/spill/
                // projection 都作用于装饰后的文本）。失败路径全部静默
                // （开关关 / 非 write|edit / 无 manager / 无服务器 /
                // 同步失败 / 无 ERROR）——永不拖垮工具调用。
                let result = if tool_succeeded
                    && matches!(tc.name.as_str(), "write_file" | "edit_file")
                    && let Ok(args_val) = serde_json::from_str::<serde_json::Value>(&tc.arguments)
                    && let Some(path_str) = args_val.get("path").and_then(|v| v.as_str())
                {
                    self.diagnostics_feedback(&tc.name, path_str, &result).await
                } else {
                    result
                };

                // G3 (U3) + G4 (U4) + X1 (U3 projection prune): model-free
                // size gates, computed here but applied at the PROJECTION
                // (build_messages), not at history-write time. History keeps
                // `original_result` (the mid-section stays recoverable for
                // history replay / session branching); `gate_text` is the
                // bounded model-facing form the OLD code stored in history —
                // byte-identical to pre-X1 behavior:
                //   >= SPILL_THRESHOLD_CHARS  → spill whole text to disk, keep
                //                               preview + locator (readable back
                //                               via read_file offset/limit).
                //   > MAX_TOOL_RESULT_INLINE_CHARS (and no spill) → head +
                //                               marker + tail prune.
                // Spill is best-effort: a storage failure falls through to the
                // prune tier (a spill failure must never lose a successful
                // tool call's content outright).
                //
                // B3 (devtool-upgrade 阶段 3): when the registry HAS a spawn
                // tool, both gate texts append the sub-agent hint (the elided
                // content is unreadable inline — a spawned sub-agent can read
                // it and return a digest). Hinted prune text is registry-
                // state-dependent and NOT recomputable by the pure projection
                // path, so `prune_hinted` forces the projection override
                // (same ledger as spill/nudges); replay recomputes without
                // hint only for unhinted turns, which are byte-stable.
                let hint_subagent = self.tools.read().contains_key("spawn");
                let mut spill_applied = false;
                let mut prune_hinted = false;
                let gate_text: String = {
                    let spill_root = self.spill_root.read().clone();
                    let spilled = spill_root.as_ref().and_then(|root| {
                        let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S%3f").to_string();
                        match crate::spill::spill_tool_result(
                            &result,
                            &tc.name,
                            root,
                            &context.session_key,
                            &stamp,
                            &tc.id,
                            hint_subagent,
                        ) {
                            crate::spill::SpillOutcome::Spilled(text) => Some(text),
                            crate::spill::SpillOutcome::SpillFailed => {
                                warn!(
                                    "[AgentLoop] tool result spill failed for '{}' — falling back to prune tier",
                                    tc.name
                                );
                                None
                            }
                            crate::spill::SpillOutcome::BelowThreshold => None,
                        }
                    });
                    match spilled {
                        Some(text) => {
                            info!(
                                "[AgentLoop] tool result spilled to disk: '{}' result >= {} chars",
                                tc.name,
                                crate::spill::SPILL_THRESHOLD_CHARS
                            );
                            spill_applied = true;
                            text
                        }
                        None => {
                            match crate::prune::prune_tool_result(&result, &tc.name, hint_subagent)
                            {
                                Some(pruned) => {
                                    info!(
                                        "[AgentLoop] tool result pruned: '{}' result exceeded {} chars",
                                        tc.name,
                                        crate::prune::MAX_TOOL_RESULT_INLINE_CHARS
                                    );
                                    prune_hinted = hint_subagent;
                                    pruned
                                }
                                None => result,
                            }
                        }
                    }
                };

                // ⑥ Alternating-loop guard: per-turn (tool, error) failure
                // frequency, NOT reset by intervening successes (also handles ④
                // storm — consecutive identical — internally). On a repeated
                // failure, append a nudge so the model sees it in the error.
                // Signature input is the post-gate text — same as pre-X1.
                let error_for_guard: Option<&str> = if tool_succeeded {
                    None
                } else {
                    Some(&gate_text)
                };
                let nudge6 = turn_guard
                    .record_tool_outcome(&tc.name, error_for_guard)
                    .inspect(|_nudge| {
                        info!(
                            "[AgentLoop] loop guard: '{}' repeating the same failure within this turn; nudging",
                            tc.name
                        );
                    });

                // X1: the recorded projection override — only when the final
                // model-facing text cannot be recomputed from the original
                // later: the spill tier (locator path embeds a wall-clock
                // stamp), any guard-nudge decoration (⑤/⑤′/⑥ — dynamic
                // per-turn state), or B3's spawn-hinted prune (the hint flag
                // is registry state, not turn data). Otherwise None and
                // build_messages recomputes the pure prune (deterministic,
                // ledger-free).
                let projection: Option<String> =
                    if spill_applied || guard_nudged || nudge6.is_some() || prune_hinted {
                        Some(match &nudge6 {
                            Some(nudge) => format!("{}\n{}", gate_text, nudge),
                            None => gate_text.clone(),
                        })
                    } else {
                        None
                    };
                instance.add_tool_result_projected(&tc.id, &original_result, &tc.name, projection);

                // H5 (U18): touch-driven instruction-chain invalidation. A
                // successful read_file/write_file/edit_file may have touched
                // a file on the workspace instruction chain — invalidate the
                // context digests so the next build re-reads the chain.
                // (File-level check only: the re-read happens at injection
                // time. Rare + cheap: only fires for these three tools and
                // only when the path matches a chain file name.)
                if tool_succeeded
                    && matches!(tc.name.as_str(), "read_file" | "write_file" | "edit_file")
                    && let Ok(args_val) = serde_json::from_str::<serde_json::Value>(&tc.arguments)
                    && let Some(path_str) = args_val.get("path").and_then(|v| v.as_str())
                {
                    let touched = std::path::PathBuf::from(path_str);
                    // I1 (devtool-upgrade 阶段 3): record agent-authored
                    // writes so the fs watcher's event for the same path is
                    // dropped inside the self-write window (the agent knows
                    // what it just wrote — must not surface as "外部修改").
                    if matches!(tc.name.as_str(), "write_file" | "edit_file") {
                        self.note_self_write(path_str);
                    }
                    // I3 (devtool-upgrade 阶段 3): lazy sub-directory
                    // instruction discovery on successful reads — the file's
                    // directory may carry AGENTS.md/CLAUDE.md that was never
                    // injected (root chain only, by default). Queued here,
                    // injected one-shot at the next build.
                    if tc.name == "read_file" {
                        self.note_read_for_instructions(instance, path_str);
                    }
                    let ws_root = self.workspace_root.read().clone();
                    if let Some(ref root) = ws_root {
                        // Chain files are <dir>/AGENTS.md or CLAUDE.md
                        // under the workspace — check by file name to
                        // avoid re-reading the whole chain on every
                        // file op (the full path_is_on_chain check
                        // happens against the loaded chain at
                        // injection; here the name match is the
                        // conservative trigger).
                        let name = touched
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if name == "AGENTS.md" || name == "CLAUDE.md" {
                            let chain =
                                crate::workspace_instructions::load_instruction_chain(root, root);
                            if crate::workspace_instructions::path_is_on_chain(&chain, &touched) {
                                info!(
                                    "[AgentLoop] instruction-chain file touched: {} — context digest invalidated",
                                    touched.display()
                                );
                                self.invalidate_context_digests();
                            }
                        }
                    }
                }

                // ⑥ Escalation: same (tool, error) failed past the hard-stop
                // threshold → nudges are being ignored. Latch a stop event and
                // break the tool batch; the outer-scope check after this for-loop
                // ends the turn (a bare `break` here only exits the batch, not
                // the LLM loop).
                //
                // J5 (devtool-upgrade 阶段 6)：`agents.doom_loop_approval` 开且
                // question asker 已装配时，先发提问卡问用户「继续吗？」——
                // approve = 清签名计数继续；deny / 超时 / 通路缺失 / 开关关 =
                // 现行为（停轮）。turn_guard 现行为是安全底座，开关默认关。
                if let Some((sig, count)) = turn_guard.escalating_signature() {
                    let approved = self.current_doom_loop_approval()
                        && self
                            .ask_doom_loop_approval(&sig, count, context)
                            .await
                            .unwrap_or(false);
                    if approved {
                        warn!(
                            "[AgentLoop] loop guard escalation on '{}' (x{}) — user approved, clearing signature count and continuing",
                            sig.split('\x00').next().unwrap_or("tool"),
                            count
                        );
                        turn_guard.clear_signature(&sig);
                    } else {
                        warn!(
                            "[AgentLoop] loop guard escalation: stopping turn to avoid burning max_turns on a stuck loop"
                        );
                        // P1（2026-09-11 真机日志分析）：升级停轮也是失败终止——
                        // 记终端原因，turn_end 边界标记不再谎报 "done"。
                        terminal_reason = Some("escalation");
                        force_stop = Some(AgentEvent::Done(context.format_rpc_message(
                            &crate::turn_guard::TurnGuard::escalation_message(&sig, count),
                        )));
                        break;
                    }
                }

                // Phase 2: bound consecutive validation failures so a struggling
                // model cannot burn the whole max_turns budget on the same
                // malformed arguments. Same latch pattern as escalation — a bare
                // `break` here used to only exit the batch while the outer LLM
                // loop kept calling the model (the "stopping loop" log was a lie,
                // observed in a deployed cluster test). Now it actually ends the
                // turn, giving the model exactly `validation_retry_budget` retries.
                if validation_failures >= self.validation_retry_budget() {
                    warn!(
                        "[AgentLoop] Validation retry budget exhausted ({}); stopping turn.",
                        validation_failures
                    );
                    // P1（2026-09-11 真机日志分析）：校验预算耗尽的 turn 是失败
                    // 终止（B 端曾把它包装成 success 回调 → 空交付结构性缺陷）。
                    // 记终端原因 + 末事件为 Error——cluster_agent 据此发 error 回调。
                    terminal_reason = Some("validation_exhausted");
                    force_stop = Some(AgentEvent::Error(format!(
                        "工具参数校验连续失败 {} 次，已停止重试。最近工具：'{}'。\
                         建议：换用更强的模型（model set-tier / 模型管理页）或把任务拆得更具体后重试。",
                        validation_failures, tc.name
                    )));
                    break;
                }
            }

            if hit_async {
                break;
            }

            // Outer-scope turn stop latched from inside the tool-call for-loop
            // (⑥ escalation OR validation-budget exhaustion). A bare `break` in
            // that for-loop only exits the batch; this actually ends the turn,
            // emitting a single terminal event.
            if let Some(ev) = force_stop {
                events.push(ev);
                break;
            }
        }

        instance.set_state(crate::types::AgentState::Idle);

        // I3 (U9): durable turn_end marker with the terminal reason.
        // L2 (full review): the reason comes from the break-site latch
        // (terminal_reason), not from sniffing the Done text — a model
        // reply containing the paused-after wording can no longer be
        // misclassified as max_turns.
        let end_reason = if cancel_token.is_cancelled() {
            "cancelled"
        } else {
            terminal_reason.unwrap_or("done")
        };
        if log_boundaries {
            crate::chat_log::append_boundary_event(&context.session_key, "turn_end", end_reason);
        } else if terminal_reason == Some("budget_exhausted") {
            // T3 (U12): cron turns are exempt from per-turn boundary events
            // (a recurring job would grow the sidecar unboundedly), but a
            // budget-exhausted stop is a rare, one-shot terminal fact worth
            // exactly one marker — the budget's observability requirement.
            crate::chat_log::append_boundary_event(&context.session_key, "turn_end", end_reason);
        }

        events
    }

    // -----------------------------------------------------------------------
    // Tool handling
    // -----------------------------------------------------------------------
}
