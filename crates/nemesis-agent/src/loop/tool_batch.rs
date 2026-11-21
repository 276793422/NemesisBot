//! 工具批次执行（器官 8 子器官自 `loop/run_loop.rs` 物理搬迁；docs/PLAN/
//! 2026-09-23_.md /8a-8e）。
//! 语义零变化：`__ASYNC__` 集群续行快照（8a）、`__BG_SPAWN__` 后台子代理
//! park 等待（8b，2026-11-19 park 化——不再伪造 Done 收口）、⑤/⑤′/⑥
//! 环守卫 + 诊断回灌 + spill/prune 门 + projection（8c）、// 指令链触碰
//! （8d）、escalation 与校验预算两终局判定（8e）。
//!
//! 器官出口约定（对照表）：8a/8b 命中即 `Some(Done(中间消息))`/`Done`
//! 返回，骨架统一 `push + break`；8e 命中即 `Some(终局事件)` 返回，骨架置
//! `terminal_reason` 后 push + break——事件向量与文案逐字节不变，golden
//! transcript 把关。：批循环本体（中间消息落账 + 并行预计算 + 批次
//! for）亦收编为 [`AgentLoop::execute_tool_batch`]，返回 `TurnFlow`；原
//! `force_stop`/`hit_async` 两补偿闩随方法化退役。
#[cfg(not(target_os = "linux"))]
use super::prelude::*;
use super::*;

/// 器官 8b park 等待的结论（organ 8b 据此取回灌文本）。
pub(crate) enum BgParkOutcome {
    /// 真实完成（成功结果或 `Error:` 失败文本——都作为工具真实结果
    /// 走统一守卫链：ToolResult 事件 + projection/历史落账 + 8d/8e）。
    Completed(String),
    /// estop/cancel 打断（⛔ 文案回灌，模型当轮自停；文本与
    /// tool_dispatch chokepoint 两臂逐字节一致）。
    Interrupted(String),
}

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

        // 子代理可见性：集群 delegate 任务并入注册表
        //（「delegate 并入同一列表」）。A 端生命周期视图——begin= 本处
        //（`__ASYNC__` marker 落地），finish= 续行回灌（dispatch_continuation）；
        // B 端 transcript 留在对端节点不拉取（诚实边界：本行 events 环恒空）。
        // id/bg_task_id 直接用集群 task_id（gateway 侧跨节点状态面同键）。
        // 任务文本取 cluster_rpc args 的 message 字段（解析失败 = 空，诚实
        // 留白，不编造）。
        let delegate_task = serde_json::from_str::<serde_json::Value>(&tc.arguments)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        crate::subagent_registry::global().begin(crate::subagent_registry::SubagentBegin {
            id: task_id.clone(),
            parent_session_key: context.session_key.clone(),
            parent_chat_id: context.chat_id.clone(),
            origin: "delegate".to_string(),
            agent_id: target_id.clone(),
            task: delegate_task,
            role: String::new(),
            tools_profile: String::new(),
            depth: 0,
            background: true,
            loop_origin: self.loop_origin(),
            bg_task_id: Some(task_id.clone()),
        });

        if let Some(ref mgr) = self.continuation_manager {
            // Get messages up to this point (including the assistant's tool_call).
            // We use build_messages() to convert history → LlmMessage format.
            let messages = self.build_messages(instance);
            let channel = context.channel.clone();
            let chat_id = context.chat_id.clone();
            let session_key = context.session_key.clone();
            // （多模态）：快照只落图片路径引用（磁盘侧剥离
            // base64 字节）。引用取自 instance 历史最后一条
            // user turn（即本 turn 的 image_refs，3468 写入）。
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
            // 对端 ID 随快照落盘 —— A 侧重启后 first_start
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

    /// 器官 8b：`__BG_SPAWN__:{task_id}` 后台子代理 **park 等待**
    /// （2026-11-19 spawn park 化，替代旧 save_bg_spawn_continuation 的
    /// 伪造 Done 收口）。spawn 闭包（background=true）把任务转入后台
    /// tokio 任务并返回 marker；本 turn **不再收口**，而是 park 在本
    /// 工具调用上等真实完成——一个逻辑任务=一个 turn=一块连续过程
    /// 时间线=一次 stop（未来 stop hook 挂回合边界的语义前提）。
    ///
    /// 流程：
    /// 1. 续行快照 inline 保存（语义不变——park 是活体等待，快照降格
    ///    为崩溃恢复保险；estop 打断后的释放恢复也走它）。inline await
    ///    消除「毫秒级完成先于保存」竞态的语义保持原样。
    /// 2. marker 落史（`add_tool_result`）——等待期间历史完整、时间线
    ///    不收口；真实完成后由 `apply_tool_guards` 的 replace 分支原地
    ///    覆写本条（APPEND 会同 call_id 双 tool message，LLM API 400）。
    /// 3. park 等待：rx 取自 `bg_park` 注册表（slot 由 spawn 闭包在
    ///    `tokio::spawn` **前**注册，先于任何可能完成，无竞态窗口）。
    ///    select 三臂（biased，estop 优先——chokepoint 同序）：
    ///    - 完成 parcel → 真实结果（失败 = `Error:` 前缀，与
    ///      `tool_result_indicates_error`/WAL ok 判定同刻度——
    ///      run_continuation_turn legacy 臂同形）；
    ///    - estop → slot 留 Waiting + 快照保留，⛔ 文案回灌（完成回灌
    ///      撞 WaiterGone 落回 legacy 快照续行 = estop 释放恢复旧世界
    ///      语义）；
    ///    - session cancel → 先尽力取消子代理执行体
    ///      （`SubagentRegistry::cancel_by_bg_task_id`——不能只唤醒主
    ///      turn 留下无人收养的子代理继续烧 token）+ slot 置 Cancelled
    ///      + 快照回收，⛔ 文案回灌。
    ///
    /// park 期间本 turn 持 session busy guard——新消息走 queue/steer
    /// （旧世界假空闲会开并行轮，是界面乱象根源之一，占用反而正确）。
    ///
    /// 并行批同享本器官：spawn parallel-safe，`precompute_parallel_batch`
    /// 并发跑全部 spawn 闭包后，重放循环按源序逐个 park——一批多个
    /// 后台子代理天然 wait-all（总等待 = max，非 sum）。
    ///
    /// 格式：`__BG_SPAWN__:{task_id}`（编码端 = agent_factory 注入的
    /// spawn 闭包，主/项目 loop 两处注入点同契约）。
    pub(crate) async fn park_bg_spawn(
        &self,
        instance: &AgentInstance,
        context: &RequestContext,
        tc: &crate::types::ToolCallInfo,
        bg_task_id: &str,
    ) -> BgParkOutcome {
        let bg_task_id = bg_task_id.trim().to_string();
        // 快照（恢复保险）：peer_id 留空——后台任务是进程内 tokio 任务，
        // 无对端可 poll；cluster first_start 恢复循环对空 peer_id 快照
        // 本就跳过（留给 TTL 清扫）；重启丢失场景由适配器启动期的诚实
        // 丢失注入兜底（见 nemesisbot adapters）。
        if let Some(ref mgr) = self.continuation_manager {
            let messages = self.build_messages(instance);
            let channel = context.channel.clone();
            let chat_id = context.chat_id.clone();
            let session_key = context.session_key.clone();
            // （多模态）：快照只落图片路径引用。引用取自
            // instance 历史最后一条 user turn（同 __ASYNC__ 路径）。
            let image_refs: Vec<String> = instance
                .get_history()
                .iter()
                .rev()
                .find(|t| t.role == "user")
                .map(|t| t.image_refs.clone())
                .unwrap_or_default();
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
                "[AgentLoop] Continuation saved for background subagent (park insurance): task_id={}, tool_call_id={}",
                bg_task_id, tc.id
            );
        }

        // marker 落史：与 __CLUSTER_ASYNC__ 刻意不同名，两套异步路径
        // 在会话历史里互不误判。等待期间本条作为占位；真实完成由守卫
        // 链 replace 分支原地覆写。
        instance.add_tool_result(
            &tc.id,
            &format!(
                "Background sub-agent accepted. Task ID: {} | __BG_ASYNC__{{\"task_id\":\"{}\"}}",
                bg_task_id, bg_task_id
            ),
        );

        // park 等待 rx。None = slot 缺席/已消费/已取消（异常装配：闭包
        // 未注册、等待已发生过）——落回旧世界 marker 即返形态，完成走
        // legacy 快照续行（诚实降级，不悬挂）。
        let Some(rx) = crate::bg_park::global().wait(&bg_task_id) else {
            warn!(
                "[AgentLoop] bg spawn park slot missing for task {} — falling back to marker-return (legacy async delivery)",
                bg_task_id
            );
            return BgParkOutcome::Completed(format!(
                "Background sub-agent accepted (park unavailable; result will arrive asynchronously). Task ID: {} | __BG_ASYNC__{{\"task_id\":\"{}\"}}",
                bg_task_id, bg_task_id
            ));
        };

        let interrupted_at = std::time::Instant::now();
        let mut estop_rx = self.security.subscribe();
        let session_cancel_token = self
            .cancel_tokens
            .get(&context.session_key)
            .map(|e| e.value().clone());
        let parcel = tokio::select! {
            biased;
            _ = Self::wait_estop_engaged(estop_rx.as_mut()) => {
                warn!(
                    "[AgentLoop] E-stop interrupted bg-spawn park (task {}); snapshot kept for release-resume.",
                    bg_task_id
                );
                self.emit_tool_interrupted(
                    &context.session_key,
                    &context.chat_id,
                    &tc.id,
                    &tc.name,
                    interrupted_at,
                    "⛔ interrupted by e-stop (estop engaged mid-park)",
                );
                return BgParkOutcome::Interrupted(
                    "⛔ ESTOP: 已急停 — 工具调用已被打断 (e-stop engaged). 不要重试；告知用户当前处于急停状态，等待释放。"
                        .to_string(),
                );
            }
            _ = async {
                match session_cancel_token.as_ref() {
                    Some(tok) => tok.cancelled().await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                info!(
                    "[AgentLoop] Session cancel interrupted bg-spawn park (task {}) — cancelling sub-agent executor.",
                    bg_task_id
                );
                // 尽力取消子代理执行体（结果丢弃——不留无人收养的子代理
                // 继续烧 token）+ slot 置 Cancelled + 快照回收（此后完成
                // 回灌 deliver=CancelledNoResume 幂等兜底）。
                crate::subagent_registry::global().cancel_by_bg_task_id(&bg_task_id);
                crate::bg_park::global().mark_cancelled(&bg_task_id);
                if let Some(ref mgr) = self.continuation_manager {
                    mgr.remove_continuation(&bg_task_id).await;
                }
                self.emit_tool_interrupted(
                    &context.session_key,
                    &context.chat_id,
                    &tc.id,
                    &tc.name,
                    interrupted_at,
                    "⛔ interrupted by session cancel (stop requested mid-park)",
                );
                return BgParkOutcome::Interrupted(
                    "⛔ CANCELLED: 已取消 — 工具调用已被打断（用户请求停止）。不要重试该工具；停止当前工作，简短确认已停止即可。"
                        .to_string(),
                );
            }
            parcel = rx => match parcel {
                Ok(p) => p,
                // rx 死亡（防御臂：tx 侧意外弃发）。落回 marker 即返，
                // 完成走 legacy 快照续行。
                Err(_) => {
                    return BgParkOutcome::Completed(format!(
                        "Background sub-agent accepted (park link lost; result will arrive asynchronously). Task ID: {} | __BG_ASYNC__{{\"task_id\":\"{}\"}}",
                        bg_task_id, bg_task_id
                    ));
                }
            },
        };
        // 真实完成：parcel → 结果文本。失败 = `Error:` 前缀（与
        // `tool_result_indicates_error`/WAL ok 判定/守卫同刻度）。
        let real = if parcel.failed {
            format!(
                "Error: {}",
                parcel
                    .error
                    .filter(|e| !e.trim().is_empty())
                    .unwrap_or_else(|| parcel.content.clone())
            )
        } else {
            parcel.content
        };
        info!(
            "[AgentLoop] bg-spawn park delivered (task {}) — real result fed back to LLM.",
            bg_task_id
        );
        BgParkOutcome::Completed(real)
    }

    /// 器官 8c：⑤/⑤′/⑥ 环守卫 + 诊断回灌 + spill/prune 门 +
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
        replace_existing: bool,
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
        // (projection prune): the pure tool output, kept for
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

        // (devtool-upgrade) 编辑后诊断回灌：写工具成功后，若
        // `agents.defaults.diagnostics_loop.enabled` 且该路径语言有已安装
        // LSP，同步文档 → 等 ERROR → 把诊断追加到工具结果尾部（"please
        // fix"），让模型同轮自纠——修复闭环。插在 ⑤′ 与 spill/gate 之间：
        // 反馈与工具结果同走一条模型可见管线（gate/spill/projection 都作
        // 用于装饰后的文本）。失败路径全部静默（开关关 / 非触发写工具 /
        // 无 manager / 无服务器 / 同步失败 / 无 ERROR）——永不拖垮工具调用。
        // 触发臂扩到 multiedit（枚举表
        // DIAGNOSTICS_WRITE_TOOLS 单一真相源；2026-11-06 append_file 并入
        // edit_file（action=append），触发由 edit_file 条目一并承载）；路径
        // 提取按 args 形态分派（multiedit = edits[].path）。跨文件聚合
        // + stale 过滤（状态按 session_key 隔离在 diagnostics_touched）。
        #[cfg(feature = "lsp")]
        let result = if tool_succeeded
            && Self::DIAGNOSTICS_WRITE_TOOLS.contains(&tc.name.as_str())
            && let Ok(args_val) = serde_json::from_str::<serde_json::Value>(&tc.arguments)
        {
            let paths = extract_diag_feedback_paths(&tc.name, &args_val);
            // 与 format-on-save 同源——钩子路径经
            // resolve_tool_path 重算（相对路径锚定工作区根），不吃原始 args
            // 的 cwd 歧义。解析失败诚实跳过该路径。
            let paths: Vec<String> = paths
                .into_iter()
                .filter_map(|p| {
                    crate::loop_tools::resolve_tool_path(
                        &p,
                        self.workspace_boundary.read().as_deref(),
                        crate::loop_tools::FenceOpFamily::Read,
                    )
                    .ok()
                    .map(|pb| pb.to_string_lossy().into_owned())
                })
                .collect();
            if paths.is_empty() {
                result
            } else {
                self.diagnostics_feedback(&context.session_key, &tc.name, &paths, &result)
                    .await
            }
        } else {
            result
        };

        // + + (projection prune): model-free
        // size gates, computed here but applied at the PROJECTION
        // (build_messages), not at history-write time. History keeps
        // `original_result` (the mid-section stays recoverable for
        // history replay / session branching); `gate_text` is the
        // bounded model-facing form the OLD code stored in history —
        // byte-identical to pre behavior:
        //   >= SPILL_THRESHOLD_CHARS → spill whole text to disk, keep
        //                               preview + locator (readable back
        //                               via read_file offset/limit).
        //   > MAX_TOOL_RESULT_INLINE_CHARS (and no spill) → head +
        //                               marker + tail prune.
        // Spill is best-effort: a storage failure falls through to the
        // prune tier (a spill failure must never lose a successful
        // tool call's content outright).
        //
        // (devtool-upgrade): when the registry HAS a spawn
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
        // B30（2026-11-06 三方对比整改）：cap 世代快照读（fresh-read，改键
        // 下轮生效）。0 = 关：prune 闸不裁；spill 层不受影响——那是可恢复
        // 性机制（≥65536 落盘 + locator），不是封顶闸。
        let tool_result_cap = self.current_tool_result_cap();
        let mut prune_applied = false;
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
                None => match crate::prune::prune_tool_result_with_cap(
                    &result,
                    &tc.name,
                    hint_subagent,
                    tool_result_cap,
                ) {
                    Some(pruned) => {
                        info!(
                            "[AgentLoop] tool result pruned: '{}' result exceeded {} chars (agents.tool_result_cap)",
                            tc.name, tool_result_cap
                        );
                        prune_hinted = hint_subagent;
                        // B30/X1：unhinted cap 裁剪与 const 纯重算**不同构**
                        // 时才记投影（缺省 cap=8000 下 >8192 深超限与 legacy
                        // 逐字节同形 → 纯重算照常，ledger 零漂移；灰区
                        // ≤8192 或头尾预算收缩 → 必须记，否则重算 None/异形
                        // 会把不一致文本回进上下文）。
                        prune_applied = !hint_subagent
                            && !crate::prune::cap_prune_matches_recompute(
                                tool_result_cap,
                                result.chars().count(),
                            );
                        pruned
                    }
                    None => {
                        // BUG-20261107-027（B30/X1 补洞）：gate 全文放行
                        // （cap=0 关闭 / total ≤ cap 放大档）也可能与 const
                        // 纯重算不同构——total > 8192 时下一轮请求/replay
                        // 的 model_facing_content 会把它重算成有界裁剪形态，
                        // 与本轮 dispatch 实际回灌的全文静默不一致（会话内
                        // 缩水）。同走同构谓词（未裁方向）：重算会裁 → 记
                        // 投影=全文钉死所见；total ≤ 8192 两边都不裁 → 照旧
                        // 不记（纯重算语义零漂移）。
                        prune_applied = !crate::prune::cap_prune_matches_recompute(
                            tool_result_cap,
                            result.chars().count(),
                        );
                        result
                    }
                },
            }
        };

        // ⑥ Alternating-loop guard: per-turn (tool, error) failure
        // frequency, NOT reset by intervening successes (also handles ④
        // storm — consecutive identical — internally). On a repeated
        // failure, append a nudge so the model sees it in the error.
        // Signature input is the post-gate text — same as pre-.
        let error_for_guard: Option<&str> = if tool_succeeded {
            None
        } else {
            Some(&gate_text)
        };
        // （扩展计划）：真实执行收据——本方法的 result 参数是
        // registry 真实产物（dispatch 闸后的唯一成功面入口），对每个结果
        // 以实例密钥生成执行证明（绑定 tool+args+result+ts）入本轮收据
        // 环。结果自称成功但无收据（绕过执行点注入）会被
        // record_tool_outcome_verified 以合成失败签名喂 escalation。
        let receipt_ts = crate::tool_receipts::now_ms();
        let receipt = crate::tool_receipts::generate_receipt(
            &self.receipt_key,
            &tc.name,
            &tc.arguments,
            &original_result,
            receipt_ts,
        );
        let nudge6 = turn_guard
                    .record_tool_outcome_verified(&tc.name, error_for_guard, Some((receipt.as_str(), receipt_ts)))
                    .inspect(|_nudge| {
                        info!(
                            "[AgentLoop] loop guard: '{}' repeating the same failure within this turn; nudging",
                            tc.name
                        );
                    });

        // the recorded projection override — only when the final
        // model-facing text cannot be recomputed from the original
        // later: the spill tier (locator path embeds a wall-clock
        // stamp), any guard-nudge decoration (⑤/⑤′/⑥ — dynamic
        // per-turn state), B3's spawn-hinted prune (the hint flag
        // is registry state, not turn data), or a B30 cap-pruned
        // form (`agents.tool_result_cap` is dispatch-time state — a
        // cap-pruned text under the const recompute threshold would
        // otherwise recompute to None and leak the full original).
        // Otherwise None and build_messages recomputes the pure prune
        // at the const default (deterministic, ledger-free).
        let projection: Option<String> =
            if spill_applied || guard_nudged || nudge6.is_some() || prune_hinted || prune_applied {
                Some(match &nudge6 {
                    Some(nudge) => format!("{}\n{}", gate_text, nudge),
                    None => gate_text.clone(),
                })
            } else {
                None
            };
        // (Phase 5 图像工具层) 工具产出的图片路径随 tool 轮落史——串行
        // dispatch 主路径的唯一取图点（dispatch 前 clear_tool_images、
        // 工具 push、此处 take）。并行批回放走同一入口但槽恒空（图片工具
        // 非 parallel-safe，进不了并行批）。异步占位臂 __ASYNC__ 在守卫
        // 之前 return，不经过此处；__BG_SPAWN__ park 臂经此处但走
        // replace 分支（marker 已在史）。
        let tool_images = context.take_tool_images();
        if replace_existing {
            // spawn park 落地（2026-11-19）：marker 已在史，真实结果
            // 原地覆写（同 call_id 唯一 tool message）。
            instance.replace_tool_result_projected(
                &tc.id,
                &original_result,
                &tc.name,
                projection,
                tool_images,
            );
        } else {
            instance.add_tool_result_projected_with_images(
                &tc.id,
                &original_result,
                &tc.name,
                projection,
                tool_images,
            );
        }
        tool_succeeded
    }

    /// 器官 8d：// 指令链触碰（自写窗口登记 / 懒发现队列 /
    /// touch-driven instruction-chain invalidation. A
    /// successful read_file/write_file/edit_file/multiedit may have touched
    /// a file on the workspace instruction chain — invalidate the
    /// context digests so the next build re-reads the chain.
    /// (File-level check only: the re-read happens at injection
    /// time. Rare + cheap: only fires for these read/write-family tools and
    /// only when the path matches a chain file name.)
    pub(crate) fn touch_instruction_chain(
        &self,
        instance: &AgentInstance,
        tc: &crate::types::ToolCallInfo,
        tool_succeeded: bool,
    ) {
        if tool_succeeded
            && matches!(
                tc.name.as_str(),
                "read_file" | "write_file" | "edit_file" | "multiedit"
            )
            && let Ok(args_val) = serde_json::from_str::<serde_json::Value>(&tc.arguments)
        {
            // multiedit 顶层无 path——批量目标在 edits[].path（提取形态与
            // format-on-save / rewrite_tool_paths_for_base 的 PATH_ARG_TOOLS
            // 同源）；其余工具单 path。
            let paths: Vec<String> = if tc.name == "multiedit" {
                args_val
                    .get("edits")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| e.get("path").and_then(|p| p.as_str()))
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                args_val
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .into_iter()
                    .collect()
            };
            let ws_root = self.workspace_root.read().clone();
            for path_str in paths {
                // (devtool-upgrade): record agent-authored
                // writes so the fs watcher's event for the same path is
                // dropped inside the self-write window (the agent knows
                // what it just wrote — must not surface as "外部修改").
                // multiedit 同为写族工具：单次调用批量编辑 N 个文件，
                // 每个 edits[].path 都要登记（BUG-20261118-091：漏登记
                // 会让 agent 刚批量编辑的文件在下轮上下文被谎报为
                // 「外部修改（非本会话操作）」）。
                if matches!(tc.name.as_str(), "write_file" | "edit_file" | "multiedit") {
                    self.note_self_write(&path_str);
                }
                // (devtool-upgrade): lazy sub-directory
                // instruction discovery on successful reads — the file's
                // directory may carry AGENTS.md/CLAUDE.md that was never
                // injected (root chain only, by default). Queued here,
                // injected one-shot at the next build.
                if tc.name == "read_file" {
                    self.note_read_for_instructions(instance, &path_str);
                }
                if let Some(ref root) = ws_root {
                    // Chain files are <dir>/AGENTS.md or CLAUDE.md
                    // under the workspace — check by file name to
                    // avoid re-reading the whole chain on every
                    // file op (the full path_is_on_chain check
                    // happens against the loaded chain at
                    // injection; here the name match is the
                    // conservative trigger).
                    let touched = std::path::PathBuf::from(&path_str);
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
        }
    }

    /// 器官 8e-1：escalation 终局判定——(tool, error) 签名越 hard-stop
    /// ⑥ Escalation: same (tool, error) failed past the hard-stop
    /// threshold (24) — every level of the intervention ladder
    /// （通用 → 换方法 → 换路径/分解 → 绕过+记录缺口，
    /// `turn_guard::alternating_nudge` 计次分级）has been ignored. Latch a
    /// stop event and break the tool batch; the outer-scope check after this
    /// for-loop ends the turn (a bare `break` here only exits the batch, not
    /// the LLM loop).
    ///
    /// 循环干预阶梯终级（取代 审批化消费）：发**无超时严重
    /// 卡**问用户（`ask_doom_loop_terminal`：timeout_secs=0 + 免打扰直批
    /// 分支绕过——任何自动化开关都压不掉终级卡，烟雾报警器必须响）。
    /// 批准 = 清签名计数继续；用户选停止 / 急停 cancelled / asker 缺席 /
    /// 通路异常 = 一律停轮（fail-closed）。
    /// `Some(Done(升级停轮文案))`；批准继续 = `None`。
    pub(crate) async fn check_escalation(
        &self,
        turn_guard: &mut crate::turn_guard::TurnGuard,
        context: &RequestContext,
    ) -> Option<AgentEvent> {
        if let Some((sig, count)) = turn_guard.escalating_signature() {
            let approved = self
                .ask_doom_loop_terminal(&sig, count, context)
                .await
                .unwrap_or(false);
            if approved {
                warn!(
                    "[AgentLoop] loop guard escalation on '{}' (x{}) — user approved continuing, clearing signature count",
                    sig.split('\x00').next().unwrap_or("tool"),
                    count,
                );
                turn_guard.clear_signature(&sig);
            } else {
                warn!(
                    "[AgentLoop] loop guard escalation: stopping turn to avoid burning max_turns on a stuck loop"
                );
                // （真机日志分析）：升级停轮也是失败终止——
                // 记终端原因，turn_end 边界标记不再谎报 "done"。
                // ESCALATION_MARKER 前缀逐字节保留（cluster_agent 跨 crate
                // 识别依赖， 阶梯化不改编排文案）。
                return Some(AgentEvent::Done(context.format_rpc_message(
                    &crate::turn_guard::TurnGuard::escalation_message(&sig, count),
                )));
            }
        }
        None
    }

    /// 器官 8e-2：校验重试预算终局判定——连续畸形参数烧完预算即
    /// bound consecutive validation failures so a struggling
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
            // （真机日志分析）：校验预算耗尽的 turn 是失败
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
    /// 器官 8：中间消息落账 + 并行预计算+ 批次循环，
    /// 自 run_loop.rs 骨架收编（与 8a-8e 同居本文件）。出口归一：
    /// 批内 cancel/estop = `Stop(ev)` + `terminal_reason` 照记（
    /// 双发遗留修正—— 重构时「① 不顺手改」的 push Done + Continue
    /// 旧形态退役：它会让下一轮器官 1 顶检重发一次 Done = 聊天里两条
    /// 「已取消」；现与 8a/8b/8e 终局同一出口，骨架 push + break 一次到位）；
    /// 8a/8b 快照与 8e escalation/validation 两终局 = `Stop(ev)`
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
        // 中间轮输出发布——模型多步执行时每轮的过程叙述
        // （「我先看下 X 再改 Y」）此前只进 history，前端看不见；
        // 叙述与思考链同发，皆空才不发（guard 在
        // emit_round_output 内）——thinking-only 轮（叙述空、思考有值）照发。
        self.emit_round_output(
            &context.session_key,
            &context.chat_id,
            &assistant_content,
            response.reasoning_content.as_deref(),
        );
        events.push(AgentEvent::ToolCall(tool_calls.clone()));

        // Execute each tool call.
        instance.set_state(crate::types::AgentState::ExecutingTool);
        // (sixth batch): precompute execution for an ALL-parallel-safe
        // batch (≥2 calls, every tool read-only OR explicitly opted in —
        // spawn). The for-loop then replays the serial guards on the
        // precomputed results in source order — the audit chain stays
        // ordered = model source order (roadmap risk 3). cluster_rpc/exec
        // /writers are never parallel-safe → this stays None for those
        // batches → the loop below runs byte-identical to pre-.
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
                .precompute_parallel_batch(
                    &tool_calls,
                    context,
                    instance.detached_depth(),
                    instance.detached_no_tools(),
                    instance.detached_allowed_tools(),
                    // 职能工具围栏随行（serial 路径同款裁决；空白名单 =
                    // 未围，parallel batch 内恒放行）。
                    instance.config().tool_fence_profession.clone(),
                    instance.config().tools.clone(),
                )
                .await;
            Some(pc)
        } else {
            None
        };
        // in the parallel path, cancel/estop are NOT re-checked per item
        // — the batch was checkpointed non-cancelled above and runs to
        // completion (goal §四 documented semantic: a cancel arriving during
        // the parallel window takes effect on the NEXT turn, not mid-batch).
        // The serial path (precomputed.is_none()) keeps the per-item checks
        // byte-identical.
        let skip_cancel_estop = precomputed.is_some();
        // 批后急停检查点——并行批窗口内到达的急停
        // （precompute 内 biased select 已中断在飞收集并补发中断面
        // ToolFinished，见 tool_defs.rs 同款注释）在此收口：拒绝整批回灌、
        // 终局本轮（旧语义 = 批照跑完且回灌照常、下一轮边界才停）。per-item
        // estop 臂同款单发 Done + terminal_reason 纪律（turn_end 不谎报 done）。
        if precomputed.is_some() && self.security.is_engaged() {
            info!(
                "[AgentLoop] E-stop engaged during/after parallel batch ({} calls) — stopping turn.",
                tool_calls.len()
            );
            st.terminal_reason = Some("estop");
            return TurnFlow::Stop(AgentEvent::Done(
                "⛔ 已急停 (E-STOP) — 工具调用已拒绝。发送 `nemesisbot estop --release` 恢复。"
                    .to_string(),
            ));
        }
        // 批后取消检查点（Phase 5 A2）——并行批窗口内到达的会话取消
        // （chokepoint cancel 臂已中断在飞执行并补发中断面 ToolFinished）
        // 在此收口，与上方批后急停检查点同形：拒绝整批回灌、终局本轮。
        // （并行路径 skip_cancel_estop=true 跳过下方 per-item 检查——若
        // 无此闸，取消结果照回灌、模型下一轮读到 ⛔ 才自停 = 多烧一轮
        // LLM 且「停止」变两跳。）串行路径 per-item 检查已覆盖，不受影响。
        if precomputed.is_some() && cancel_token.is_cancelled() {
            info!(
                "[AgentLoop] Session cancel arrived during/after parallel batch ({} calls) — stopping turn.",
                tool_calls.len()
            );
            st.terminal_reason = Some("cancelled");
            return TurnFlow::Stop(AgentEvent::Done("已取消".to_string()));
        }
        for (batch_idx, tc) in tool_calls.iter().enumerate() {
            // Check cancellation before each tool execution.
            if !skip_cancel_estop && cancel_token.is_cancelled() {
                info!(
                    "[AgentLoop] LLM loop cancelled before tool execution: {}, turns_used={}",
                    tc.name, st.turns_used
                );
                // 单发 Done（双发遗留修正）：原形态 push Done +
                // Continue 落回骨架 → 下一轮器官 1 顶检再发一次并停轮 =
                // 聊天里出现两条「已取消」。改走 Stop 出口与 8a/8b/8e 终局
                // 归一（骨架 push + break + 收尾一次到位）。
                st.terminal_reason = Some("cancelled");
                return TurnFlow::Stop(AgentEvent::Done("已取消".to_string()));
            }

            // 全局急停检查：触发则拒绝后续工具调用并结束当前轮。
            let estop_engaged = self.security.is_engaged();
            if estop_engaged {
                info!(
                    "[AgentLoop] E-stop engaged before tool execution: {}, turns_used={}",
                    tc.name, st.turns_used
                );
                // 单发 Done（同上双发遗留修正）；estop 臂 cancel_token 未
                // 触发，finalize_turn 靠 terminal_reason 记 "estop"——
                // turn_end 边界标记不再谎报 "done"（599 行同款纪律）。
                st.terminal_reason = Some("estop");
                return TurnFlow::Stop(AgentEvent::Done(
                    "⛔ 已急停 (E-STOP) — 工具调用已拒绝。发送 `nemesisbot estop --release` 恢复。"
                        .to_string(),
                ));
            }

            let tool_start = std::time::Instant::now();
            // (small-model-tool-robustness): validate args against
            // the tool's schema before dispatch. Catches B-class failures;
            // auto-fixes high-confidence field-name typos (edit distance ≤2);
            // otherwise bounces a structured error back to the model so it
            // can self-correct on the next round.
            //
            // (sixth batch): when `precomputed` is Some, the execution
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
                    // 校验失败回灌计入会话统计。
                    self.bump_session_stats(
                        &context.session_key,
                        crate::session_stats::StatCounter::ValidationFailures,
                    );
                    self.record_tool_validation_stats(true);
                } else {
                    st.validation_failures = 0;
                    self.record_tool_validation_stats(false);
                }
                (p.result.clone(), p.duration_ms)
            } else {
                // 环境约束预拒统一注册点——detached +
                // 职能围栏两约束一处声明，串行/并行同源（并行预检消费
                // 值形态 [`super::tool_dispatch:
                // environment_constraint_refusal_for`]）。环境性拒绝在
                // 安全段之前权威拒绝：不弹审批卡、不进风险管线（
                // 阻塞链的派发侧护栏之一）。
                let r = match super::tool_dispatch::environment_constraint_refusal(
                    instance, &tc.name,
                )
                .or_else(|| {
                    // (Phase 5 图像工具层) vision 前置闸：产出图片的工具
                    // 在 vision=no 的模型面前零 IO 拒绝（注册点
                    // vision_gated_image_tool_refusal；并行批不含图片工具，
                    // 无需并行臂）。
                    super::tool_dispatch::vision_gated_image_tool_refusal(
                        self.current_vision().supported,
                        &tc.name,
                    )
                }) {
                    // 压测①a：detached 工具闸——no_tools/白名单外
                    // 在派发现场权威拒绝（供给层只管 defs 可见性，上游端点
                    // 注入的调用不经供给层）。拒绝串回灌当工具结果，下游
                    // observer/turn_guard/spill 守卫照常按序跑。
                    Some(refusal) => {
                        warn!(
                            "[AgentLoop] Environment constraint refused '{}': {}",
                            tc.name,
                            refusal.trim_start_matches("Error: ")
                        );
                        refusal
                    }
                    None => match self.check_tool_args(tc).await {
                        crate::args_validator::Outcome::Valid => {
                            st.validation_failures = 0;
                            self.record_tool_validation_stats(false);
                            // (Phase 5 图像工具层) 每次串行分派前清图片槽：
                            // 上一次调用的残留（工具 push 后守卫前中断的边角）
                            // 不得泄进本次 tool 轮。取图在 apply_tool_guards。
                            context.clear_tool_images();
                            // dispatch at this instance's sub-agent depth so
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
                            context.clear_tool_images();
                            self.handle_tool_call_at_depth(
                                &fixed,
                                context,
                                instance.detached_depth(),
                            )
                            .await
                        }
                        crate::args_validator::Outcome::Invalid { message, class } => {
                            st.validation_failures += 1;
                            // 校验失败回灌计入会话统计（串行臂）。
                            self.bump_session_stats(
                                &context.session_key,
                                crate::session_stats::StatCounter::ValidationFailures,
                            );
                            self.record_tool_validation_stats(true);
                            warn!(
                                "[AgentLoop] Arg validation failed for tool '{}' (id={}, class={}): {}",
                                tc.name, tc.id, class, message
                            );
                            // 建设：回灌单一真相源（与并行预计算臂同源——
                            // 惰性供给时附 tools_schema_get 取回指引）。
                            super::tool_defs::validation_error_feedback(
                                &message,
                                self.current_lazy_tool_schema(),
                                &tc.name,
                            )
                        }
                    },
                };
                (r, tool_start.elapsed().as_millis() as u64)
            };
            let tool_duration = std::time::Duration::from_millis(tool_duration_ms);
            // 同刻度判定：observer/capture 的成败分类与
            // turn_guard/WAL 共用同一 helper——ExecTool 的 `Ok("Exit code: N")`
            // 与 `Ok("Command timed out …")` 都是失败，不共用会把失败循环记成
            // 成功流。
            let tool_success = !crate::turn_guard::tool_result_indicates_error(&result);

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
            // The `enabled()` gate must come FIRST: the ToolCapture literal
            // below clones the full result twice, and a bare `global()` check
            // would do that cloning even when capture is disabled (only to
            // drop it inside record_tool) — every tool call on every session
            // would pay it.
            if crate::capture_sink::CaptureSink::enabled()
                && let Some(sink) = crate::capture_sink::CaptureSink::global()
            {
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

            // 器官 8a（tool_batch.rs）：__ASYNC__ 集群续行快照 +
            // 中间消息；Some = 终局 Done（return Stop，骨架 push + break）。
            if result.starts_with("__ASYNC__:")
                && let Some(ev) = self
                    .save_async_continuation(instance, context, tc, &result)
                    .await
            {
                return TurnFlow::Stop(ev);
            }

            // 器官 8b（tool_batch.rs）：__BG_SPAWN__ 后台子代理 park
            // 等待（2026-11-19 park 化：废除伪造 Done 收口——turn 停在
            // 本工具调用上等真实完成；完成 parcel / estop/cancel ⛔
            // 文案都作为本调用的最终结果落回统一守卫链）。等待后不再
            // return，走 8c 历史落账；bg_park_replace 告知 apply_tool_
            // guards 落账走 replace 语义（marker 已在史——park 前落账
            // ——APPEND 会同 call_id 双 tool message，LLM API 400）。
            let (result, bg_park_replace) =
                if let Some(bg_task_id) = result.strip_prefix("__BG_SPAWN__:") {
                    let parked = self.park_bg_spawn(instance, context, tc, bg_task_id).await;
                    (
                        match parked {
                            BgParkOutcome::Completed(real) => real,
                            BgParkOutcome::Interrupted(text) => text,
                        },
                        true,
                    )
                } else {
                    (result, false)
                };

            let tool_result = ToolCallResult {
                tool_name: tc.name.clone(),
                result: result.clone(),
                is_error: false,
            };
            events.push(AgentEvent::ToolResult(tool_result));

            // 器官 8c（tool_batch.rs）：⑤/⑤′/⑥ 守卫 + 诊断
            // 回灌 + spill/prune 门 + projection + 历史落账；返回
            // tool_succeeded 供 8d 与边界判定。
            let tool_succeeded = self
                .apply_tool_guards(
                    instance,
                    context,
                    tc,
                    result,
                    &mut st.turn_guard,
                    bg_park_replace,
                )
                .await;

            // 器官 8d：// 指令链触碰。
            self.touch_instruction_chain(instance, tc, tool_succeeded);

            // 器官 8e（tool_batch.rs）：两终局判定（Some = 终局
            // 事件 → terminal_reason 照记 + return Stop（骨架 push +
            // break）； 批准继续 = None 落回批次）。
            if let Some(ev) = self.check_escalation(&mut st.turn_guard, context).await {
                st.terminal_reason = Some("escalation");
                return TurnFlow::Stop(ev);
            }
            if let Some(ev) = self.check_validation_budget(st.validation_failures, &tc.name) {
                st.terminal_reason = Some("validation_exhausted");
                return TurnFlow::Stop(ev);
            }
        }

        // present_plan verdict 消费（批次边界，单一消费点）。
        // 按 session_key 取走本会话的裁决（跨会话 turn 并发不串台）。
        // 驳回/超时 = 本轮结构性终止（模型不再获得下一轮，工具结果文本已落
        // 历史，下条用户消息可）；批准 = 模式已在工具内翻转，落回正常轮。
        if let Some(handle) = self.plan_present.read().clone()
            && let Some(verdict) = handle.verdicts.lock().remove(&context.session_key)
        {
            match verdict {
                crate::loop_tools::PlanPresentVerdict::Approved => {}
                crate::loop_tools::PlanPresentVerdict::Rejected => {
                    st.terminal_reason = Some("plan_rejected");
                    return TurnFlow::Stop(AgentEvent::Done(
                        "❌ 计划已被用户驳回 — 本轮到此结束（仍处于 plan 模式）。请根据用户反馈修改计划后再次调用 present_plan。"
                            .to_string(),
                    ));
                }
                crate::loop_tools::PlanPresentVerdict::Timeout => {
                    st.terminal_reason = Some("plan_timeout");
                    return TurnFlow::Stop(AgentEvent::Done(
                        "⏱ 计划审批超时未获响应 — 本轮到此结束（仍处于 plan 模式）。".to_string(),
                    ));
                }
            }
        }

        TurnFlow::Continue
    }
}
