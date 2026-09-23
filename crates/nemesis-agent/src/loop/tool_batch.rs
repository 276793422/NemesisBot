//! 工具批次执行（P2-2 器官 8 子器官自 `loop/run_loop.rs` 物理搬迁；docs/PLAN/
//! 2026-09-23_agentloop-god-object-decomposition.md §4.2 表行 8/8a-8e）。
//! 语义零变化：`__ASYNC__` 集群续行快照（8a）、`__BG_SPAWN__` 后台子代理
//! 快照（8b，inline await 语义保持）、⑤/⑤′/⑥ 环守卫 + C3 诊断回灌 +
//! spill/prune 门 + X1 projection（8c）、H5/I1/I3 指令链触碰（8d）、
//! escalation 与校验预算两终局判定（8e）。
//!
//! 器官出口约定（§4.3 对照表）：8a/8b 命中即 `Some(Done(中间消息))`/`Done`
//! 返回，骨架统一 `push + break`；8e 命中即 `Some(终局事件)` 返回，骨架置
//! `terminal_reason` 后 push + break——事件向量与文案逐字节不变，golden
//! transcript 把关。P2-5：批循环本体（中间消息落账 + 并行预计算 + 批次
//! for）亦收编为 [`AgentLoop::execute_tool_batch`]，返回 `TurnFlow`；原
//! `force_stop`/`hit_async` 两补偿闩随方法化退役。
use super::prelude::*;
use super::*;

impl AgentLoop {
    /// 器官 8a：`__ASYNC__:{task_id}:{target_id}[:{target_name}]` 集群续行
    /// Check for async cluster_rpc result — save continuation snapshot.
    ///
    /// Plan C (template-based UX): the cluster_rpc tool encodes the
    /// peer's display name as the 4th part of the marker so we can
    /// render a human-friendly "waiting" message here without an
    /// extra cluster lookup (this crate can't depend on
    /// nemesis-cluster). The full LLM-generated persona response
    /// was deferred — it would double cross-node latency and
    /// complicate the continuation snapshot. See loop_tools.rs
    /// for the encoding site.
    ///
    /// Format: `__ASYNC__:{task_id}:{target_id}:{target_name}`
    /// Older senders may omit the name part (3-segment format),
    /// in which case we fall back to the bare target_id.
    /// 快照 + 中间消息（原批内 `events.push + hit_async + break` 整段）。
    /// 命中（marker 合法）= `Some(Done(中间消息))`；marker 畸形 = `None`
    /// （落回批次继续，与原 fall-through 一致）。
    pub(crate) async fn save_async_continuation(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        tc: &crate::types::ToolCallInfo,
        result: &str,
    ) -> Option<AgentEvent> {
        let parts: Vec<String> = result.splitn(4, ':').map(|s| s.to_string()).collect();
        if parts.len() < 3 {
            return None;
        }
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
        Some(AgentEvent::Done(formatted))
    }

    /// 器官 8b：`__BG_SPAWN__:{task_id}` 后台子代理续行快照 + 中间消息。
    /// G4 (devtool-upgrade 阶段 3)：后台 subagent —— spawn 闭包
    /// （background=true）已在闭包侧把任务转入后台 tokio 任务并
    /// 立即返回此 marker。这里与 __ASYNC__（集群）同构：存续行
    /// 快照 + 中间消息收尾本回合；任务完成时闭包侧向 bus 发布
    /// `subagent_continuation:{task_id}`，gate_inbound 拦截后走
    /// dispatch_continuation → handle_cluster_continuation 全复用
    /// （单飞闸认领 + 快照加载 + 续行 + 持久化 + finish_handling
    /// 收口自清——磁盘快照保留到最终回复持久化后，发现 F 2026-09-11）。
    ///
    /// 快照保存必须 **inline await**（不能像 __ASYNC__ 那样
    /// spawn）：后台子代理毫秒级即可完成并回灌，spawn 式保存要
    /// 等下一个 await 点才落内存，load 端扑空即静默且回复
    /// （handle_cluster_continuation 的 debug-skip 分支）。inline
    /// 消除该竞态；一次小盘写的延迟可忽略。
    ///
    /// 格式：`__BG_SPAWN__:{task_id}`（编码端 = agent_factory 注入
    /// 的 spawn 闭包）。
    /// 调用方（strip_prefix 分类留在骨架）拿到返回值后统一
    /// `push + hit_async + break`。快照保存 **inline await**（原注释所述
    /// 竞态消除语义保持）。
    pub(crate) async fn save_bg_spawn_continuation(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        tc: &crate::types::ToolCallInfo,
        bg_task_id: &str,
    ) -> AgentEvent {
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
        instance.add_tool_result(
            &tc.id,
            &format!(
                "Background sub-agent accepted. Task ID: {} | __BG_ASYNC__{{\"task_id\":\"{}\"}}",
                bg_task_id, bg_task_id
            ),
        );

        let intermediate = "已派后台子代理任务，完成后结果会自动带回本会话~".to_string();
        let formatted = context.format_rpc_message(&intermediate);
        AgentEvent::Done(formatted)
    }

    /// 器官 8c：⑤/⑤′/⑥ 环守卫 + C3 诊断回灌 + spill/prune 门 + X1
    /// ⑤/⑥ Loop guards — mutually exclusive per call (success vs error).
    /// Use the shared helper so ExecTool's `Ok("Exit code: N")` for
    /// non-zero exits is detected as a failure — otherwise build
    /// loops look like success and the guards never fire.
    /// projection + 历史落账（add_tool_result_projected）。返回
    /// `tool_succeeded` 供 8d 指令链触碰与批内后续判定。
    pub(crate) async fn apply_tool_guards(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        tc: &crate::types::ToolCallInfo,
        result: String,
        turn_guard: &mut crate::turn_guard::TurnGuard,
    ) -> bool {
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
                None => match crate::prune::prune_tool_result(&result, &tc.name, hint_subagent) {
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
                },
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
        tool_succeeded
    }

    /// 器官 8d：H5/I1/I3 指令链触碰（自写窗口登记 / 懒发现队列 /
    /// H5 (U18): touch-driven instruction-chain invalidation. A
    /// successful read_file/write_file/edit_file may have touched
    /// a file on the workspace instruction chain — invalidate the
    /// context digests so the next build re-reads the chain.
    /// (File-level check only: the re-read happens at injection
    /// time. Rare + cheap: only fires for these three tools and
    /// only when the path matches a chain file name.)
    pub(crate) fn touch_instruction_chain(
        &self,
        instance: &AgentInstance,
        tc: &crate::types::ToolCallInfo,
        tool_succeeded: bool,
    ) {
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
                    let chain = crate::workspace_instructions::load_instruction_chain(root, root);
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
    }

    /// 器官 8e-1：escalation 终局判定——(tool, error) 签名越 hard-stop
    /// ⑥ Escalation: same (tool, error) failed past the hard-stop
    /// threshold → nudges are being ignored. Latch a stop event and
    /// break the tool batch; the outer-scope check after this for-loop
    /// ends the turn (a bare `break` here only exits the batch, not
    /// the LLM loop).
    ///
    /// J5 (devtool-upgrade 阶段 6)：`agents.doom_loop_approval` 开且
    /// question asker 已装配时，先发提问卡问用户「继续吗？」——
    /// approve = 清签名计数继续；deny / 超时 / 通路缺失 / 开关关 =
    /// 现行为（停轮）。turn_guard 现行为是安全底座，开关默认关。
    /// `Some(Done(升级停轮文案))`；批准继续 = `None`。
    pub(crate) async fn check_escalation(
        &self,
        turn_guard: &mut crate::turn_guard::TurnGuard,
        context: &RequestContext,
    ) -> Option<AgentEvent> {
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
                return Some(AgentEvent::Done(context.format_rpc_message(
                    &crate::turn_guard::TurnGuard::escalation_message(&sig, count),
                )));
            }
        }
        None
    }

    /// 器官 8e-2：校验重试预算终局判定——连续畸形参数烧完预算即
    /// Phase 2: bound consecutive validation failures so a struggling
    /// model cannot burn the whole max_turns budget on the same
    /// malformed arguments. Same latch pattern as escalation — a bare
    /// `break` here used to only exit the batch while the outer LLM
    /// loop kept calling the model (the "stopping loop" log was a lie,
    /// observed in a deployed cluster test). Now it actually ends the
    /// turn, giving the model exactly `validation_retry_budget` retries.
    /// `Some(Error(终止文案))`（cluster_agent 据此发 error 回调）。
    pub(crate) fn check_validation_budget(
        &self,
        validation_failures: u32,
        tc_name: &str,
    ) -> Option<AgentEvent> {
        if validation_failures >= self.validation_retry_budget() {
            warn!(
                "[AgentLoop] Validation retry budget exhausted ({}); stopping turn.",
                validation_failures
            );
            // P1（2026-09-11 真机日志分析）：校验预算耗尽的 turn 是失败
            // 终止（B 端曾把它包装成 success 回调 → 空交付结构性缺陷）。
            // 记终端原因 + 末事件为 Error——cluster_agent 据此发 error 回调。
            return Some(AgentEvent::Error(format!(
                "工具参数校验连续失败 {} 次，已停止重试。最近工具：'{}'。\
                         建议：换用更强的模型（model set-tier / 模型管理页）或把任务拆得更具体后重试。",
                validation_failures, tc_name
            )));
        }
        None
    }
    /// 器官 8（§4.2 表行 8）：中间消息落账 + 并行预计算（U5）+ 批次循环，
    /// 自 run_loop.rs 骨架收编（P2-5，与 8a-8e 同居本文件）。出口归一：
    /// 批内 cancel/estop = push Done + `Continue`（骨架 continue → 下一
    /// 轮器官 1 顶检再发一次并停轮——双发 Done 现状语义原样，§9① 不顺手
    /// 改）；8a/8b 快照与 8e escalation/validation 两终局 = `Stop(ev)`
    /// （骨架 push + break，terminal_reason 照记）；批次正常完毕 =
    /// `Continue` 落回下一轮。原 force_stop/hit_async 两补偿闩随方法化
    /// 退役（闩的存在意义就是批次内 break 出不了外层循环）。
    pub(crate) async fn execute_tool_batch(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        trace_id: &str,
        response: &LlmResponse,
        cancel_token: &tokio_util::sync::CancellationToken,
        st: &mut TurnState,
        events: &mut Vec<AgentEvent>,
    ) -> TurnFlow {
        // Model produced tool calls → it is making progress. Clear any
        // pending degenerate-answer nudge (⑦) so it stops nagging while the
        // model works — tool work is the opposite of a degenerate empty
        // final answer.
        st.degenerate_nudge_pending = None;

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
            && !self.security.is_engaged()
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
                    tc.name, st.turns_used
                );
                events.push(AgentEvent::Done("已取消".to_string()));
                // 双发 Done 语义原样（§9①）：批内已发一次，落回
                // 骨架 Continue → 下一轮器官 1 顶检再发一次并停轮。
                return TurnFlow::Continue;
            }

            // 全局急停检查：触发则拒绝后续工具调用并结束当前轮。
            let estop_engaged = self.security.is_engaged();
            if estop_engaged {
                info!(
                    "[AgentLoop] E-stop engaged before tool execution: {}, turns_used={}",
                    tc.name, st.turns_used
                );
                events.push(AgentEvent::Done(
                    "⛔ 已急停 (E-STOP) — 工具调用已拒绝。发送 `nemesisbot estop --release` 恢复。"
                        .to_string(),
                ));
                // 同上：双发 Done 语义原样（§9①）。
                return TurnFlow::Continue;
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
                    st.validation_failures += 1;
                    self.record_tool_validation_stats(true);
                } else {
                    st.validation_failures = 0;
                    self.record_tool_validation_stats(false);
                }
                (p.result.clone(), p.duration_ms)
            } else {
                let r = match self.check_tool_args(tc) {
                    crate::args_validator::Outcome::Valid => {
                        st.validation_failures = 0;
                        self.record_tool_validation_stats(false);
                        // G2: dispatch at this instance's sub-agent depth so
                        // depth-aware tools (spawn) enforce max_depth.
                        self.handle_tool_call_at_depth(tc, context, instance.detached_depth())
                            .await
                    }
                    crate::args_validator::Outcome::Fixed(fixed_args) => {
                        st.validation_failures = 0;
                        self.record_tool_validation_stats(false);
                        info!(
                            "[AgentLoop] Auto-fixed args for tool '{}' (id={})",
                            tc.name, tc.id
                        );
                        let mut fixed = tc.clone();
                        fixed.arguments = fixed_args;
                        self.handle_tool_call_at_depth(&fixed, context, instance.detached_depth())
                            .await
                    }
                    crate::args_validator::Outcome::Invalid { message, class } => {
                        st.validation_failures += 1;
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
            let tool_success = !result.starts_with("Error:") && !result.starts_with("Tool error:");

            // Emit tool call observer event.
            self.emit_observer_sync(crate::loop_executor::ObserverEvent::ToolCall {
                trace_id: trace_id.to_string(),
                tool_name: tc.name.clone(),
                success: tool_success,
                duration_ms: tool_duration.as_millis() as u64,
                round: st.turns_used,
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
                        llm_round: st.turns_used as usize,
                        ts: String::new(),
                    },
                );
            }

            // 器官 8a（§4.2 → tool_batch.rs）：__ASYNC__ 集群续行快照 +
            // 中间消息；Some = 终局 Done（return Stop，骨架 push + break）。
            if result.starts_with("__ASYNC__:")
                && let Some(ev) = self
                    .save_async_continuation(instance, context, tc, &result)
                    .await
            {
                return TurnFlow::Stop(ev);
            }

            // 器官 8b（§4.2 → tool_batch.rs）：__BG_SPAWN__ 后台子代理
            // 快照（inline await 语义保持）。
            if let Some(bg_task_id) = result.strip_prefix("__BG_SPAWN__:") {
                let ev = self
                    .save_bg_spawn_continuation(instance, context, tc, bg_task_id)
                    .await;
                return TurnFlow::Stop(ev);
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
                .apply_tool_guards(instance, context, tc, result, &mut st.turn_guard)
                .await;

            // 器官 8d：H5/I1/I3 指令链触碰。
            self.touch_instruction_chain(instance, tc, tool_succeeded);

            // 器官 8e（§4.2 → tool_batch.rs）：两终局判定（Some = 终局
            // 事件 → terminal_reason 照记 + return Stop（骨架 push +
            // break）；J5 批准继续 = None 落回批次）。
            if let Some(ev) = self.check_escalation(&mut st.turn_guard, context).await {
                st.terminal_reason = Some("escalation");
                return TurnFlow::Stop(ev);
            }
            if let Some(ev) = self.check_validation_budget(st.validation_failures, &tc.name) {
                st.terminal_reason = Some("validation_exhausted");
                return TurnFlow::Stop(ev);
            }
        }

        TurnFlow::Continue
    }
}
