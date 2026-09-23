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

            // 器官 4（§4.2）：LLM 调用 + 上下文/429/transient 三恢复环 +
            // post-hooks——P2-1 搬入 loop/recovery.rs。终局出口经 Err 返回
            // 事件（§4.3 约定），骨架在此统一 push + break。
            let mut response = match self
                .call_llm_with_recovery(
                    instance,
                    context,
                    trace_id,
                    messages,
                    tool_defs,
                    &active_model,
                    &chat_opts,
                    cancel_token,
                    voice_playback,
                    request_had_images,
                    turns_used,
                    round_start,
                )
                .await
            {
                Ok(resp) => resp,
                Err(ev) => {
                    events.push(ev);
                    break;
                }
            };

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

                // 器官 8a（§4.2 → tool_batch.rs）：__ASYNC__ 集群续行快照 +
                // 中间消息；Some = 终局 Done（骨架 push + hit_async + break）。
                if result.starts_with("__ASYNC__:")
                    && let Some(ev) = self
                        .save_async_continuation(instance, context, tc, &result)
                        .await
                {
                    events.push(ev);
                    hit_async = true;
                    break;
                }

                // 器官 8b（§4.2 → tool_batch.rs）：__BG_SPAWN__ 后台子代理
                // 快照（inline await 语义保持）。
                if let Some(bg_task_id) = result.strip_prefix("__BG_SPAWN__:") {
                    let ev = self
                        .save_bg_spawn_continuation(instance, context, tc, bg_task_id)
                        .await;
                    events.push(ev);
                    hit_async = true;
                    break;
                }

                let tool_result = ToolCallResult {
                    tool_name: tc.name.clone(),
                    result: result.clone(),
                    is_error: false,
                };
                events.push(AgentEvent::ToolResult(tool_result));

                // 器官 8c（§4.2 → tool_batch.rs）：⑤/⑤′/⑥ 守卫 + C3 诊断
                // 回灌 + spill/prune 门 + X1 projection + 历史落账；返回
                // tool_succeeded 供 8d 与边界判定。
                let tool_succeeded = self
                    .apply_tool_guards(instance, context, tc, result, &mut turn_guard)
                    .await;

                // 器官 8d：H5/I1/I3 指令链触碰。
                self.touch_instruction_chain(instance, tc, tool_succeeded);

                // 器官 8e（§4.2 → tool_batch.rs）：两终局判定（Some = 终局
                // 事件 → terminal_reason + force_stop latch + break 批次；
                // J5 批准继续 = None 落回批次）。
                if let Some(ev) = self.check_escalation(&mut turn_guard, context).await {
                    terminal_reason = Some("escalation");
                    force_stop = Some(ev);
                    break;
                }
                if let Some(ev) = self.check_validation_budget(validation_failures, &tc.name) {
                    terminal_reason = Some("validation_exhausted");
                    force_stop = Some(ev);
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
