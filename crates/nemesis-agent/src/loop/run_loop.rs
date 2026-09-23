//! run_llm_loop 原样搬迁（P2 解剖对象：轮循环/重试环/工具批/steer/estop）。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// 一轮（round）的结局（§4.1）：继续下一轮，或携带终局事件结束本 turn。
/// crate 内私有（P2 白名单：不进 pub 面）——P2-5 起器官 7（run_loop.rs）
/// 与器官 8（tool_batch.rs）的统一返回面。
pub(crate) enum TurnFlow {
    Continue,
    Stop(AgentEvent),
}

/// 本 turn 的可变状态（§4.1：原 run_llm_loop 的 12 个跨轮局部变量收拢；
/// P2-4）。crate 内私有——器官 4（recovery.rs）签名按 §4.2 终态持
/// `&TurnState`，故字段需跨模块可见；不进 pub 面。
///
/// `force_stop` / `hit_async` **不进** TurnState（P2-5 退役）：它们是
/// "for 循环 break 出不去外层循环"的补偿机制；工具批次改为方法返回
/// `TurnFlow` 后即无存在必要。per-round 暂存（replay_injections /
/// replay_voice / request_had_images）由 [`AgentLoop::build_round_messages`]
/// 返回、当轮边界落盘即弃，不收拢——混进 TurnState 而忘记每轮清空 =
/// 台账跨轮累积（重构期典型自坑点，特此钉死）。
pub(crate) struct TurnState {
    /// 已消耗的 LLM 轮数（organ 5 观测轮号 / 预算判定 / 日志轮号）。
    pub(crate) turns_used: u32,
    /// 每请求连续参数校验失败计数（成功即清零；烧穿 tier 预算停轮，
    /// 防小模型同一次畸形参数烧穿 max_turns）。
    pub(crate) validation_failures: u32,
    /// max_tokens 截断续写预算（organ 6；[`MAX_LENGTH_CONTINUATIONS`] 封顶）。
    pub(crate) length_continuations: u32,
    /// ② grace-round 闩：工具轮预算耗尽后的一次终稿机会（organ 1 授予，
    /// organ 3 注入 [`GRACE_ROUND_NUDGE`]）；二次命中可恢复停轮。
    pub(crate) grace_round: bool,
    /// turn 域守卫（⑥ 交替循环 / ⑦ 退化输出 / ⑤ 写环 / ⑤′ 读环）。
    /// 每请求新建——无状态跨请求。
    pub(crate) turn_guard: crate::turn_guard::TurnGuard,
    /// ⑦ 待再注入的退化答案 nudge（瞬时——不进 history / session_log，
    /// organ 3 每次 build 后重挂，模型给出可见答案或预算耗尽为止）。
    pub(crate) degenerate_nudge_pending: Option<String>,
    /// ⑧ 待再注入的跨轮行文重复 nudge（同上瞬时模式）。
    pub(crate) repetition_nudge_pending: Option<String>,
    /// I1 (U7)：一次性 escape-hatch 闩（Accept 分支：终答在即但有未认领
    /// steer——多给一轮，防 `!` 刷屏无限续 turn）。
    pub(crate) steer_escape_used: bool,
    /// L2：break 点记录的终端原因（turn_end 边界标记不再靠文案嗅探）。
    pub(crate) terminal_reason: Option<&'static str>,
    /// K2 (U14)：turn-end 钩子（Stop 方言）续命预算；耗尽 → 仍停
    /// （fail-open，与 MAX_LLM_HOOK_RETRIES 同纪律）。
    pub(crate) turn_end_continues: u32,
    /// I3 (U9)：turn 边界标记开关（heartbeat/cron/内部通道豁免）。
    pub(crate) log_boundaries: bool,
    /// 本 turn 的 LLM 调用选项（max_tokens/temperature/reasoning_effort）。
    pub(crate) chat_opts: crate::types::ChatOptions,
}

/// max_tokens 截断续写预算上限（organ 6）——真超限的大文件要给出清晰
/// 报错，而不是无限续写。
const MAX_LENGTH_CONTINUATIONS: u32 = 5;

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

        // 本 turn 可变状态收拢（§4.1 TurnState：12 跨轮局部变量；P2-4）。
        // 字段语义见类型定义处注释。
        let mut st = TurnState {
            turns_used: 0,
            validation_failures: 0,
            length_continuations: 0,
            grace_round: false,
            turn_guard: crate::turn_guard::TurnGuard::new(),
            degenerate_nudge_pending: None,
            repetition_nudge_pending: None,
            steer_escape_used: false,
            terminal_reason: None,
            turn_end_continues: 0,
            log_boundaries,
            chat_opts: crate::types::ChatOptions {
                // max_tokens: per-model `max_output_tokens` from config if
                // declared (each model's real output ceiling, not a blanket
                // 8192 — large files write in one shot instead of
                // truncating); else 8192. temperature 0.7.
                max_tokens: Some(self.current_max_tokens().unwrap_or(8192)),
                temperature: Some(0.7),
                // H4 (U16 half): per-model reasoning effort from config.json.
                reasoning_effort: self.current_reasoning_effort(),
                ..Default::default()
            },
        };

        // K1b (U14) 沿革：此处曾有 `'turn:` 标签，供 LLM post-hook 重呼环
        // `break 'turn` 直停本 turn；P2 器官化后各出口改经器官返回值
        // （§4.3 Err/Stop 约定）由骨架统一 push + break，标签闲置遂除
        // （P2-5）。裸 `break` 仍只跳出最近层循环，语义不变。
        loop {
            // 器官 1（§4.2）：MCP/config 热重载 + cancel/estop 顶检 +
            // max_turns/grace/cron 预算判定。Some = (终局事件, 终端原因)。
            if let Some((ev, reason)) = self.prepare_round(turn_budget, cancel_token, &mut st) {
                if let Some(r) = reason {
                    st.terminal_reason = Some(r);
                }
                events.push(ev);
                break;
            }

            // 器官 2（§4.2）：inbox claim → @file 展开 → URL 媒体预取 →
            // 附加链 → chat_log 落行。
            self.claim_steer_messages(instance, context, &st).await;

            // 器官 3（§4.2）：memory 预取 + annotated build + 瞬时注入族 +
            // LLM pre-hooks + tool_defs + LlmRequest observer + boundary
            // marker/T8 台账。Err = hook 拦截终局。
            let (messages, tool_defs, active_model, request_had_images, round_start) = match self
                .build_round_messages(instance, context, trace_id, voice_playback, &st)
                .await
            {
                Ok(v) => v,
                Err(ev) => {
                    events.push(ev);
                    break;
                }
            };

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
                    cancel_token,
                    voice_playback,
                    request_had_images,
                    round_start,
                    &st,
                )
                .await
            {
                Ok(resp) => resp,
                Err(ev) => {
                    events.push(ev);
                    break;
                }
            };

            st.turns_used += 1;

            // 器官 5（§4.2 → observer.rs）：LlmResponse observer 事件 +
            // data store 计价明细。
            let round_duration = round_start.elapsed();
            self.record_round_usage(
                context,
                trace_id,
                &mut response,
                round_duration,
                st.turns_used,
            )
            .await;

            // 器官 6（§4.2）：max_tokens 截断续写判定。Some(Continue) =
            // 截断已追加续写提示（continue 下一轮）；Some(Stop) = 续写
            // 预算耗尽终局；None = 未截断（计数清零）落回终答判定。
            match self.handle_length_continuation(instance, context, &response, &mut st) {
                Some(TurnFlow::Continue) => continue,
                Some(TurnFlow::Stop(ev)) => {
                    events.push(ev);
                    break;
                }
                None => {}
            }

            // ⑧ Cross-round prose repetition: if the model's content is
            // near-identical to the previous round's, queue a transient nudge
            // for the next build. Catches "saying the same thing while churning
            // tools" — a loop ⑥ cannot see (it watches tool results, not prose).
            if let Some(nudge) = st.turn_guard.check_text_repetition(&response.content) {
                info!("[AgentLoop] loop guard: response content repeating across rounds; nudging");
                st.repetition_nudge_pending = Some(nudge);
            } else {
                st.repetition_nudge_pending = None;
            }

            // 器官 7（§4.2）：终答判定。Some(Continue) = 续轮（steer
            // escape / turn-end hook / 退化 nudge 三出口归一）；Some(Stop)
            // = 终局事件（push + break）；None = 非终答落器官 8。
            match self
                .judge_final_answer(instance, context, &response, &mut st)
                .await
            {
                Some(TurnFlow::Continue) => continue,
                Some(TurnFlow::Stop(ev)) => {
                    events.push(ev);
                    break;
                }
                None => {}
            }

            // 器官 8（§4.2 → tool_batch.rs）：中间消息落账 + 批次执行。
            // Continue = 批次毕（或批内 cancel/estop 双发 Done 语义——顶检
            // 重发，§9①）；Stop = 终局事件 push + break。
            match self
                .execute_tool_batch(
                    instance,
                    context,
                    trace_id,
                    &response,
                    cancel_token,
                    &mut st,
                    &mut events,
                )
                .await
            {
                TurnFlow::Continue => continue,
                TurnFlow::Stop(ev) => {
                    events.push(ev);
                    break;
                }
            }
        }

        // 器官 9（§4.2）：turn 收尾——state Idle + turn_end 边界标记。
        self.finalize_turn(instance, context, cancel_token, &st);

        events
    }

    /// 器官 6（§4.2）：max_tokens 截断续写判定。返回 `Option<TurnFlow>`：
    /// `Some(Continue)` = 命中截断且续写预算未耗尽（已追加续写提示，骨架
    /// `continue` 下一轮）；`Some(Stop)` = 续写预算耗尽终局（骨架 push +
    /// break）；`None` = 未截断（计数清零，落回终答判定）。三态塞不进
    /// 「双臂 TurnFlow / None」出口约定——如实偏离（计划只给两态）。
    fn handle_length_continuation(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        response: &LlmResponse,
        st: &mut TurnState,
    ) -> Option<TurnFlow> {
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
        let token_cap = st.chat_opts.max_tokens.unwrap_or(8192) as u64;
        let hit_cap = response
            .usage
            .as_ref()
            .map(|u| (u.completion_tokens as u64) >= token_cap)
            .unwrap_or(false);
        if hit_cap {
            if st.length_continuations < MAX_LENGTH_CONTINUATIONS {
                st.length_continuations += 1;
                warn!(
                    "[AgentLoop] response truncated at max_tokens cap ({}); \
                     continue-generation {}/{}",
                    token_cap, st.length_continuations, MAX_LENGTH_CONTINUATIONS
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
                return Some(TurnFlow::Continue);
            }
            // Budget exhausted: clear, non-misleading error.
            warn!(
                "[AgentLoop] length-continuation budget exhausted; \
                 output keeps exceeding max_tokens ({token_cap})"
            );
            let notice = format!(
                "输出反复超过 max_tokens 上限（{}）被截断，文件可能太大。\
                 请调大 max_tokens，或让我分段写入。",
                token_cap
            );
            instance.add_assistant_message(&notice, Vec::new(), None);
            return Some(TurnFlow::Stop(AgentEvent::Error(
                context.format_rpc_message(&notice),
            )));
        }
        // Complete (non-truncated) response — reset the counter.
        st.length_continuations = 0;
        None
    }

    /// 器官 7（§4.2）：终答判定——heartbeat 特例 → ⑦ 退化三判 → I1
    /// steer escape → K2 turn-end hooks → Accept 落地。`Some(Continue)`
    /// = 续轮（escape/hook/nudge 三 `continue` 出口归一）；`Some(Stop)`
    /// = 终局事件（heartbeat/Accept/GiveUp 三 push+break 出口归一）；
    /// `None` = 非终答（有未完工具调用），落器官 8 工具批次。
    async fn judge_final_answer(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        response: &LlmResponse,
        st: &mut TurnState,
    ) -> Option<TurnFlow> {
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
                return Some(TurnFlow::Stop(AgentEvent::Done(formatted)));
            }
            match st.turn_guard.check_final_answer(&content) {
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
                    if !st.steer_escape_used
                        && self.inbox.has_next_step(&context.session_key)
                        && self.concurrent_mode == ConcurrentMode::Steer
                    {
                        st.steer_escape_used = true;
                        info!(
                            "[AgentLoop] escape hatch: pending steer at turn end, one more round"
                        );
                        // Loop again — the claim at the top of the next
                        // iteration injects the steer message(s).
                        return Some(TurnFlow::Continue);
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
                        let lifecycle = self.hooks.lifecycle_hooks.read().snapshot();
                        if !lifecycle.is_empty() {
                            let end = crate::hooks::HookTurnEnd {
                                session_key: context.session_key.clone(),
                                channel: context.channel.clone(),
                                chat_id: context.chat_id.clone(),
                                final_content: content.clone(),
                                stop_hook_active: st.turn_end_continues > 0,
                            };
                            if let crate::hooks::TurnEndDecision::Continue { feedback } =
                                crate::hooks::run_turn_end_hooks(&lifecycle, &end).await
                            {
                                if st.turn_end_continues < crate::hooks::MAX_TURN_END_CONTINUES {
                                    st.turn_end_continues += 1;
                                    info!(
                                        "[AgentLoop] turn-end hook blocked stopping \
                                             ({}/{}, session '{}') — one more round",
                                        st.turn_end_continues,
                                        crate::hooks::MAX_TURN_END_CONTINUES,
                                        context.session_key
                                    );
                                    instance.add_user_message(&feedback);
                                    return Some(TurnFlow::Continue);
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
                    return Some(TurnFlow::Stop(AgentEvent::Done(formatted)));
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
                    st.degenerate_nudge_pending = Some(nudge);
                    return Some(TurnFlow::Continue);
                }
                crate::turn_guard::FinalAnswerVerdict::GiveUp(notice) => {
                    warn!("[AgentLoop] degenerate final answer retry budget exhausted; giving up");
                    instance.add_assistant_message(&notice, Vec::new(), None);
                    let formatted = context.format_rpc_message(&notice);
                    return Some(TurnFlow::Stop(AgentEvent::Done(formatted)));
                }
            }
        }
        None
    }

    /// 器官 9（§4.2）：turn 收尾——state Idle + turn_end 边界标记。
    /// I3 终端原因取 break-site latch（`st.terminal_reason`），不从 Done
    /// 文案嗅探；`cancel_token` 入参为 cancelled 判定所需（计划签名漏列，
    /// 如实补上）。
    fn finalize_turn(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        cancel_token: &tokio_util::sync::CancellationToken,
        st: &TurnState,
    ) {
        instance.set_state(crate::types::AgentState::Idle);

        // I3 (U9): durable turn_end marker with the terminal reason.
        // L2 (full review): the reason comes from the break-site latch
        // (terminal_reason), not from sniffing the Done text — a model
        // reply containing the paused-after wording can no longer be
        // misclassified as max_turns.
        let end_reason = if cancel_token.is_cancelled() {
            "cancelled"
        } else {
            st.terminal_reason.unwrap_or("done")
        };
        if st.log_boundaries {
            crate::chat_log::append_boundary_event(&context.session_key, "turn_end", end_reason);
        } else if st.terminal_reason == Some("budget_exhausted") {
            // T3 (U12): cron turns are exempt from per-turn boundary events
            // (a recurring job would grow the sidecar unboundedly), but a
            // budget-exhausted stop is a rare, one-shot terminal fact worth
            // exactly one marker — the budget's observability requirement.
            crate::chat_log::append_boundary_event(&context.session_key, "turn_end", end_reason);
        }
    }

    // -----------------------------------------------------------------------
    // Round organs（P2-3/P2-4：§4.2 器官 1-9 自 run_llm_loop 内联块收编为
    // 方法，1-4/6/9 留本文件、5 留 observer.rs、4 系 recovery.rs；跨轮
    // 可变状态经 `&TurnState`/`&mut TurnState` 传递）
    // -----------------------------------------------------------------------

    /// 器官 1：MCP/config 热重载 + cancel/estop 顶检 + ①/② max_turns cap +
    /// grace round + T3 cron 预算判定。`Some((终局事件, 终端原因))` = 结束
    /// 本 turn（骨架 push + 记 terminal_reason + break）；grace 授予写回
    /// `st.grace_round`；其余 = `None` 继续。
    fn prepare_round(
        &self,
        turn_budget: Option<u32>,
        cancel_token: &tokio_util::sync::CancellationToken,
        st: &mut TurnState,
    ) -> Option<(AgentEvent, Option<&'static str>)> {
        let turns_used = st.turns_used;
        let grace_round = &mut st.grace_round;
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
            return Some((AgentEvent::Done("已取消".to_string()), None));
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
            return Some((
                AgentEvent::Done(
                    "⛔ 已急停 (E-STOP) — 已停止当前任务。发送 `nemesisbot estop --release` 恢复。"
                        .to_string(),
                ),
                None,
            ));
        }

        // ①/② max_turns cap + grace round. max_turns == 0 means unlimited
        // (opt-in). T3 (U12): when a per-turn budget override is set
        // (cron continuation's max_rounds), it REPLACES the global cap for
        // this turn. On the first hit we grant one grace round (with
        // GRACE_ROUND_NUDGE injected below) so the model can finalize from
        // completed work; a second hit stops resumably — no work is lost.
        let effective_max_turns = turn_budget.unwrap_or(self.config.max_turns);
        if effective_max_turns > 0 && turns_used >= effective_max_turns {
            if !*grace_round {
                *grace_round = true;
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
                return Some((
                    AgentEvent::Done(format!(
                        "已在定时任务预算 {} 轮工具调用后暂停，已完成的工作已保存。定时任务未被删除，下次触发时会重新获得预算。",
                        effective_max_turns
                    )),
                    Some("budget_exhausted"),
                ));
            } else {
                warn!(
                    "[AgentLoop] paused after {} tool-call rounds (grace round exhausted)",
                    effective_max_turns
                );
                return Some((
                    AgentEvent::Done(format!(
                        "已在 {} 轮工具调用后暂停，已完成的工作已保存。发送下一条消息可继续，或调大 max_tool_iterations（设为 0 表示不限）。",
                        effective_max_turns
                    )),
                    Some("max_turns"),
                ));
            }
        }
        None
    }

    /// 器官 2：inbox claim → @file 展开 → URL 媒体预取 → 统一附加链 →
    /// chat_log 落行 + steer_injected 边界事件。无出口（原内联块原样）。
    async fn claim_steer_messages(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        st: &TurnState,
    ) {
        let log_boundaries = st.log_boundaries;
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
    }

    /// 器官 3：memory 预取 + annotated build + 瞬时注入族（voice/grace/
    /// degenerate/repetition）+ K1b LLM pre-hooks + tool_defs 折叠 +
    /// LlmRequest observer + durable llm_request marker + T8 projection
    /// 台账（P2-3 连同 boundary marker 块一并收入——台账消费 organ 内
    /// 产物 replay_injections/replay_voice/build_annotation）。返回
    /// `(messages, tool_defs, active_model, request_had_images, round_start)`
    /// 五件套供器官 4；`Err` = hook 拦截终局事件（骨架 push + break，§4.3）。
    async fn build_round_messages(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        voice_playback: bool,
        st: &TurnState,
    ) -> Result<
        (
            Vec<LlmMessage>,
            Vec<crate::types::ToolDefinition>,
            String,
            bool,
            std::time::Instant,
        ),
        AgentEvent,
    > {
        let turns_used = st.turns_used;
        let grace_round = st.grace_round;
        let degenerate_nudge_pending = &st.degenerate_nudge_pending;
        let repetition_nudge_pending = &st.repetition_nudge_pending;
        let log_boundaries = st.log_boundaries;
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
        if let Some(nudge) = degenerate_nudge_pending {
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
        if let Some(nudge) = repetition_nudge_pending {
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
            let llm_hooks = self.hooks.llm_hooks.read().snapshot();
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
                        return Err(AgentEvent::Done(format!(
                            "⛔ HOOK BLOCKED [layer:hook|policy:llm_hook] {} — A registered LLM hook denied this round. Do NOT retry unless the user changes the hook policy.",
                            reason
                        )));
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
        Ok((
            messages,
            tool_defs,
            active_model,
            request_had_images,
            round_start,
        ))
    }
    // -----------------------------------------------------------------------
    // Tool handling
    // -----------------------------------------------------------------------
}
