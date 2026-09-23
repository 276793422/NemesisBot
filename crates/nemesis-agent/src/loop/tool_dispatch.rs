//! 工具派发全守卫链：handle_tool_call(_at_depth)、is_lsp_write_call、plan_mode_write_allowed、rewrite_tool_paths_for_base、ask_doom_loop_approval。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

impl AgentLoop {
    /// J5：doom-loop 审批卡。经 [`Self::question_asker`]（F7 同源 broker 的
    /// ask 端）发结构化提问「继续吗？」，阻塞等用户作答。
    ///
    /// 返回：`Some(true)` = 用户选「继续」（调用方清签名计数继续）；
    /// `Some(false)` = 拒绝或超时（调用方走现行为停轮）；`None` = asker
    /// 未装配或 ask 通路异常（同现行为，fail-closed）。
    pub(crate) async fn ask_doom_loop_approval(
        &self,
        sig: &str,
        count: u32,
        context: &RequestContext,
    ) -> Option<bool> {
        let asker = self.question_asker()?;
        let tool = sig.split('\x00').next().unwrap_or("tool").to_string();
        let request = nemesis_types::agent::QuestionRequest {
            question_id: crate::loop_tools::next_question_id(),
            question: format!(
                "循环守卫拦截：{} 已连续 {} 次报相同错误且未响应纠正提示，继续执行吗？",
                tool, count
            ),
            options: vec!["继续执行".to_string(), "停止".to_string()],
            multi: false,
            chat_id: context.chat_id.clone(),
            session_key: context.session_key.clone(),
            timeout_secs: crate::loop_tools::QUESTION_DEFAULT_TIMEOUT_SECS,
        };
        // ask 是同步阻塞（最长 120s）——与 question 工具同形：spawn_blocking
        // 出 worker 线程，broker 内部按上下文决定 block_in_place/直等。
        let outcome = tokio::task::spawn_blocking(move || asker.ask(request))
            .await
            .ok()
            .and_then(|r| r.ok());
        match outcome {
            Some(nemesis_types::agent::QuestionOutcome::Answered(selected)) => {
                Some(selected.iter().any(|s| s == "继续执行"))
            }
            _ => Some(false),
        }
    }

    /// Execute a single tool call at the top-level depth (0).
    ///
    /// G2 thin wrapper: external callers (hooks/cc_hooks/loop tests, any
    /// non-subagent dispatch) mean "top-level agent invoked this" — they
    /// stay untouched and get depth 0. The production serial dispatch in
    /// `run_llm_loop` calls [`Self::handle_tool_call_at_depth`] with the
    /// instance's detached depth instead.
    pub async fn handle_tool_call(
        &self,
        tool_call: &ToolCallInfo,
        context: &RequestContext,
    ) -> String {
        self.handle_tool_call_at_depth(tool_call, context, 0).await
    }

    /// G2: depth-aware variant of [`Self::handle_tool_call`]. `depth` is the
    /// invoking instance's sub-agent nesting depth (instance.detached_depth());
    /// it is injected into depth-aware tools (spawn) before execution so the
    /// `agents.subagent.max_depth` limit can reject over-deep spawns.
    pub async fn handle_tool_call_at_depth(
        &self,
        tool_call: &ToolCallInfo,
        context: &RequestContext,
        depth: usize,
    ) -> String {
        info!(
            "[AgentLoop] Executing tool: {} (id={})",
            tool_call.name, tool_call.id
        );

        // 全局急停：触发时拒绝所有工具分发。这是 handle_tool_call 公开入口级
        // 的防御深度——与 run_llm_loop 里的批次检查点互补，任何调用方都吃到。
        let estop_engaged = self
            .estop
            .read()
            .as_ref()
            .map(|e| e.is_engaged())
            .unwrap_or(false);
        if estop_engaged {
            warn!(
                "[AgentLoop] E-stop engaged — tool {} refused.",
                tool_call.name
            );
            return "⛔ ESTOP: 已急停 — 工具调用已被拒绝 (e-stop engaged). 不要重试；告知用户当前处于急停状态，等待释放。"
                .to_string();
        }

        // F-U3-2（UAT U3 实证）：档案管线任务的工作副本路径基准重写。位置在
        // 所有闸门之前——下游（Plan 闸/安全管线/审计链）看到的都是重写后的
        // 真实落点路径。
        let rewritten;
        let tool_call: &ToolCallInfo = match Self::rewrite_tool_paths_for_base(tool_call, context) {
            Some(fixed) => {
                rewritten = fixed;
                &rewritten
            }
            None => tool_call,
        };

        // F8 (devtool-upgrade 阶段 3): dispatch-side hidden gate — the second
        // half of the `agents.hidden_tools` double gate. Supply-side filtering
        // (build_tool_defs) keeps hidden tools out of the defs, but a stale
        // prompt cache / in-flight request can still carry an old defs snapshot;
        // re-checking the same list here (same matcher, same fresh read) makes
        // the hidden state authoritative at execution time too. Runs BEFORE the
        // security pipeline so a hidden tool costs no judge/scanner work.
        {
            let hidden = self.current_hidden_tools();
            if tool_name_matches_hidden(&hidden, &tool_call.name) {
                warn!(
                    "[AgentLoop] Hidden tool {} refused (agents.hidden_tools).",
                    tool_call.name
                );
                return format!(
                    "Error: Tool '{}' is hidden by configuration (agents.hidden_tools) and cannot be executed. Inform the user; do NOT retry unless they un-hide it.",
                    tool_call.name
                );
            }
        }

        // F1 (devtool-upgrade 阶段 4): Plan 模式分发闸——供给侧
        // （build_tool_defs 白名单过滤）之外的第二道闸，与 F8 同一防御模型：
        // 陈旧 prompt cache / 子代理 full 档（其 defs 也被 loop 级过滤，但
        // 未知 MCP 写工具不在过滤范围）都从这里兜底。深度无关：子代理
        // （depth ≥ 1）同样被拦——Plan 语义是「这个 loop 现在不许改文件」，
        // 委托子代理不能成为旁路。位置在 security 之前：模式级拒绝不必
        // 白付 judge/scanner 成本（F8 同理由）。唯一写放行 = write_file
        // 且路径落在 `<workspace>/plans/`（计划产物落盘）。C7：lsp 的
        // rename op 是跨文件改内容——同受 Plan 拦截（其余 lsp op 只读）。
        if *self.mode.read() == crate::types::AgentMode::Plan
            && ((Self::PLAN_MODE_WRITE_TOOLS.contains(&tool_call.name.as_str())
                && !self.plan_mode_write_allowed(&tool_call.name, &tool_call.arguments))
                || Self::is_lsp_write_call(&tool_call.name, &tool_call.arguments))
        {
            warn!(
                "[AgentLoop] Plan mode: write-class tool {} refused.",
                tool_call.name
            );
            return "Plan mode: file modifications are denied. Present your plan as text, or ask the user to switch with /build. (Exception: write_file under <workspace>/plans/ is allowed for saving the plan itself.)"
                .to_string();
        }

        // Pre-execution security check (mirrors Go's PluginableTool.Execute → PluginManager → SecurityPlugin).
        #[cfg(feature = "security")]
        {
            if let Some(ref security) = self.security_plugin {
                // P0 vault（D2/D3，2026-09-22 计划 §4）：声明式量化风险限
                // 制。位置：estop 之后、安全管线之前（限额不是内容安全，
                // 不付 judge/scanner 成本）。类别由工具声明、规则由
                // security.limits 配置（装配层注入；缺省全关）。超限不静默
                // 拒绝——走 auditor 审批直通车升级（Web/Channel 管理器零
                // 改动）：批准 = 计数 + 放行；拒绝/超时/无通道 = fail-closed。
                {
                    let limit_categories: Vec<String> = self
                        .tools
                        .read()
                        .get(&tool_call.name)
                        .map(|t| t.limit_categories().iter().map(|s| s.to_string()).collect())
                        .unwrap_or_default();
                    if let Some(over) = limits::check_and_record(&limit_categories) {
                        let approval_ctx = nemesis_security::auditor::ApprovalContext {
                            channel: context.channel.clone(),
                            chat_id: context.chat_id.clone(),
                            sender_id: context.user.clone(),
                        };
                        match security.auditor().request_limit_approval(
                            &tool_call.name,
                            &over.denial_message(),
                            Some(&approval_ctx),
                        ) {
                            Ok(av) if av.approved => {
                                // 人工放行：这次调用计入配额，放行执行。
                                limits::record(&limit_categories);
                                info!(
                                    "[AgentLoop] Rate limit on {} (category {}) overridden by user approval",
                                    tool_call.name, over.category
                                );
                            }
                            Ok(av) => {
                                let note = av
                                    .note
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|n| !n.is_empty())
                                    .map(|n| format!("用户备注: {n}"))
                                    .unwrap_or_default();
                                warn!(
                                    "[AgentLoop] Rate limit on {}: user rejected override",
                                    tool_call.name
                                );
                                return format!(
                                    "⛔ RATE LIMIT — USER REJECTED [layer:limits|category:{}] {}。用户拒绝了本次超限放行。Do NOT retry. Inform the user.{}",
                                    over.category,
                                    over.denial_message(),
                                    note
                                );
                            }
                            Err(e) => {
                                // 无审批通道/审批器未运行：fail-closed（与
                                // guardian_failure 无通道同语义）。
                                warn!(
                                    "[AgentLoop] Rate limit on {}: no approval channel ({})",
                                    tool_call.name, e
                                );
                                return format!(
                                    "⛔ RATE LIMIT — NO APPROVAL CHANNEL [layer:limits|category:{}] {}。无可用审批通道，fail-closed 拒绝。Do NOT retry. Inform the user.",
                                    over.category,
                                    over.denial_message()
                                );
                            }
                        }
                    }
                }
                let args_value = serde_json::from_str::<serde_json::Value>(&tool_call.arguments)
                    .unwrap_or(serde_json::Value::Null);
                let invocation = nemesis_security::types::ToolInvocation {
                    tool_name: tool_call.name.clone(),
                    args: args_value,
                    user: String::new(),
                    source: context.channel.clone(),
                    // K4 (b)（devtool-upgrade 阶段 7）：审批来源上下文随行——
                    // pipeline 层 3 提取后传给审批管理器，IM 通道的审批卡
                    // 路由回发起对话（web/dashboard 走原路径不受影响）。
                    metadata: {
                        let mut m = std::collections::HashMap::new();
                        m.insert("approval_chat_id".to_string(), context.chat_id.clone());
                        m.insert("approval_sender_id".to_string(), context.user.clone());
                        m
                    },
                };
                let (allowed, deny) = security.execute(&invocation);
                if !allowed {
                    // F5: 结构化 deny 反馈——回灌带 layer/policy/suggestion
                    // 三要素（summary 是各层原文），并明示模型不要原样重试。
                    let info = deny.unwrap_or_else(|| nemesis_security::types::DenyInfo {
                        layer: "unknown",
                        policy: "security_pipeline".to_string(),
                        summary: "operation denied by security policy".to_string(),
                        suggestion: None,
                    });
                    warn!(
                        "[AgentLoop] Security blocked tool {}: [{}:{}] {}",
                        tool_call.name, info.layer, info.policy, info.summary
                    );
                    // Use a very explicit prefix so the LLM cannot misinterpret this
                    // as a generic error (e.g. "file not found"). The LLM must
                    // understand that the USER or SECURITY POLICY blocked the action.
                    let suggestion_line = info
                        .suggestion
                        .as_deref()
                        .map(|s| format!("\n建议：{s}"))
                        .unwrap_or_default();
                    return format!(
                        "⛔ SECURITY BLOCKED [layer:{}|policy:{}] {}\nDo NOT retry the same call unchanged. Inform the user that the operation was rejected.{}",
                        info.layer, info.policy, info.summary, suggestion_line
                    );
                }
                // P5: guardian (LLM safety judge) semantic review. Runs only
                // after the rule layers allow; coverage is governed by
                // `guardian_mode`（2026-09-16 无上下文 LLM 命令审计，默认
                // off 不审）via `guardian_should_review` — the single
                // decision point on SecurityPlugin. Request is deliberately
                // CONTEXT-FREE（宪法）：judge 只见命令本身 + 危级元数据，
                // 零任务/对话信息；rubric = intent/matches_rules 四元组。
                // D2（2026-09-16 横扫存量加固）：judge **Err 不再静默落空**
                // （fail-open 曾是裸奔面——guardian 挂了 CRITICAL 操作全放行），
                // 按 `guardian_failure_policy` 分支：allow=显式放行 / ask=审批
                // 直通车（fail-closed：无审批管理器=拒）/ 未配置=旧行为放行。
                if security.guardian_should_review(&tool_call.name, &tool_call.arguments)
                    && let Some(judge) = security.judge()
                {
                    let req = nemesis_security::guardian::JudgeRequest {
                        action: tool_call.name.clone(),
                        risk_level: security.tool_danger_level(&tool_call.name),
                        command: tool_call.arguments.clone(),
                    };
                    match judge.judge(&req).await {
                        Ok(v) => {
                            // verdict 全量落审计（放行也记）——事后可回答
                            // 「LLM 为什么放行/拦下了这条命令」。
                            let guardian_mode = security.guardian_mode();
                            security.auditor().log_guardian_verdict(
                                &tool_call.name,
                                &guardian_mode,
                                &v,
                            );
                            if v.is_allow() {
                                info!(
                                    "[AgentLoop] Guardian allowed {} (rec={}, rules_match={})",
                                    tool_call.name, v.recommendation, v.matches_rules
                                );
                            } else {
                                // 只升格（2026-09-16 拍板）：LLM ask/deny 不
                                // 无声硬拦——转人工审批卡（误报留人工通道）；
                                // 无审批管理器/用户拒绝/超时 = fail-closed 拒绝。
                                let approval_ctx = nemesis_security::auditor::ApprovalContext {
                                    channel: context.channel.clone(),
                                    chat_id: context.chat_id.clone(),
                                    ..Default::default()
                                };
                                match security.auditor().request_guardian_verdict_approval(
                                    &tool_call.name,
                                    &v.risk_level,
                                    &guardian_mode,
                                    &v,
                                    Some(&approval_ctx),
                                ) {
                                    Ok(av) if av.approved => {
                                        info!(
                                            "[AgentLoop] Guardian flagged {} (rec={}) but user approved",
                                            tool_call.name, v.recommendation
                                        );
                                    }
                                    Ok(av) => {
                                        let note = av
                                            .note
                                            .as_deref()
                                            .map(str::trim)
                                            .filter(|n| !n.is_empty())
                                            .map(|n| format!(": {}", n))
                                            .unwrap_or_default();
                                        return format!(
                                            "⛔ GUARDIAN FLAGGED — USER REJECTED [layer:guardian|policy:guardian_mode={}] The safety judge flagged this operation ({}: {}) and the user declined to approve. Do NOT retry. Inform the user.{}",
                                            guardian_mode, v.recommendation, v.rationale, note
                                        );
                                    }
                                    Err(e) => {
                                        return format!(
                                            "⛔ GUARDIAN FLAGGED [layer:guardian|policy:guardian_mode={}] The safety judge flagged this operation ({}: {}) and no approval channel is available ({}). Fail-closed: operation denied. Do NOT retry. Inform the user.",
                                            guardian_mode, v.recommendation, v.rationale, e
                                        );
                                    }
                                }
                            }
                        }
                        Err(guardian_err) => {
                            match security.guardian_failure_policy().as_str() {
                                "allow" => {
                                    warn!(
                                        "[AgentLoop] Guardian failed on critical tool {} (guardian_failure_policy=allow, proceeding): {}",
                                        tool_call.name, guardian_err
                                    );
                                }
                                "ask" => {
                                    let approval_ctx = nemesis_security::auditor::ApprovalContext {
                                        channel: context.channel.clone(),
                                        chat_id: context.chat_id.clone(),
                                        ..Default::default()
                                    };
                                    match security.auditor().request_guardian_failure_approval(
                                        &tool_call.name,
                                        &format!(
                                            "Guardian (LLM safety judge) failed: {}",
                                            guardian_err
                                        ),
                                        Some(&approval_ctx),
                                    ) {
                                        Ok(v) if v.approved => {
                                            info!(
                                                "[AgentLoop] Guardian failed on critical tool {}: user approved",
                                                tool_call.name
                                            );
                                        }
                                        Ok(v) => {
                                            let note = v
                                                .note
                                                .as_deref()
                                                .map(str::trim)
                                                .filter(|n| !n.is_empty())
                                                .map(|n| format!(": {}", n))
                                                .unwrap_or_default();
                                            return format!(
                                                "⛔ GUARDIAN UNAVAILABLE — USER REJECTED [layer:guardian|policy:guardian_failure_policy=ask] Safety judge failed ({}) and the user declined to approve. Do NOT retry. Inform the user.{}",
                                                guardian_err, note
                                            );
                                        }
                                        Err(e) => {
                                            // fail-closed：无审批管理器/未运行/调用失败 = 拒绝
                                            return format!(
                                                "⛔ GUARDIAN UNAVAILABLE [layer:guardian|policy:guardian_failure_policy=ask] Safety judge failed ({}) and no approval channel is available ({}). Fail-closed: operation denied. Do NOT retry. Inform the user.",
                                                guardian_err, e
                                            );
                                        }
                                    }
                                }
                                "deny" => {
                                    return format!(
                                        "⛔ GUARDIAN UNAVAILABLE — DENIED BY POLICY [layer:guardian|policy:guardian_failure_policy=deny] Safety judge failed ({}). Fail-closed: operation denied by policy. Do NOT retry. Inform the user.",
                                        guardian_err
                                    );
                                }
                                // 未知值：fail-closed 优先于 fail-open（复核
                                // 2026-09-16：此前未知值落 `_` 臂静默放行——
                                // 用户显式选择 deny 得到与未配置相同的裸奔
                                // 行为，D2 最严档完全失效）。
                                other if !other.is_empty() => {
                                    warn!(
                                        "[AgentLoop] Unknown guardian_failure_policy {:?} (critical tool {}), treating as deny (fail-closed): {}",
                                        other, tool_call.name, guardian_err
                                    );
                                    return format!(
                                        "⛔ GUARDIAN UNAVAILABLE — DENIED BY POLICY [layer:guardian|policy:guardian_failure_policy={}] Safety judge failed ({}). Fail-closed: operation denied. Do NOT retry. Inform the user.",
                                        other, guardian_err
                                    );
                                }
                                // 未配置（空串）：旧行为——Err 落空放行
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        // K1a (U14): user tool hooks. Pre hooks run here — AFTER the fixed
        // security gate above (which stays inline as the de-facto pre[0]; see
        // crate::hooks module doc for why it wasn't converted to a trait
        // object) and BEFORE context injection / checkpoint / execute.
        // Ordered, first Block wins. Fires on every dispatch attempt,
        // including unknown tool names (a hook may deny what the model
        // *tried* to call — mirrors the dialect's PreToolUse).
        let hook_call = crate::hooks::HookToolCall {
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.clone(),
            channel: context.channel.clone(),
            chat_id: context.chat_id.clone(),
            session_key: context.session_key.clone(),
        };
        {
            let hooks = self.hooks.tool_hooks.read().snapshot();
            if let Some(reason) = crate::hooks::run_pre_hooks(&hooks, &hook_call).await {
                warn!(
                    "[AgentLoop] Hook blocked tool {}: {}",
                    tool_call.name, reason
                );
                return format!(
                    "⛔ HOOK BLOCKED [layer:hook|policy:tool_hook] {} — A registered hook denied this operation. Do NOT retry unless the user changes the hook policy. Inform the user if this keeps blocking.",
                    reason
                );
            }
        }

        // Inject channel/chat_id into context-aware tools before execution.
        // Mirrors loop_executor.rs:1634 which calls set_context for AgentLoopExecutor.
        // G2: also inject the invocation depth for depth-aware tools (spawn) —
        // the accepted set-then-read race on a shared SpawnTool only marginally
        // misattributes depth between concurrent dispatches (same class as the
        // pre-existing set_context race).
        {
            let guard = self.tools.read();
            if let Some(tool) = guard.get(&tool_call.name) {
                tool.set_context(&context.channel, &context.chat_id);
                tool.set_invocation_depth(depth);
            }
        }

        #[cfg(feature = "forge")]
        let tool_start = std::time::Instant::now();
        // K1a 三段化（2026-08-29）：around 链——scoped
        // hooks 逆序包装真实执行，Err 分支走 post_tool_use_failure 变体
        // （PostToolUseFailure 语义挂点）。作用域过滤：主 agent 只接 None。
        let scoped_hooks: Vec<std::sync::Arc<dyn crate::hooks::ToolHook>> = {
            let hooks = self.hooks.tool_hooks.read().snapshot();
            hooks.into_iter().filter(|h| h.scope().is_none()).collect()
        };
        let ctx_arc = std::sync::Arc::new(context.clone());
        let tools_snapshot = std::sync::Arc::new(self.tools.read().clone());
        let checkpoint_arc = std::sync::Arc::new(self.checkpoint_store.read().as_ref().cloned());
        let hooks_arc = std::sync::Arc::new(scoped_hooks.clone());
        // A6：format-on-save 的 config.json 路径快照（每次 dispatch 新鲜读，
        // 同 C3 current_diagnostics_loop 模式——运行中可翻转开关）。
        let format_cfg_path = self.config_path.read().clone();
        // D3：本 turn 声明式文件变更的收集桶（Arc 捕获进 'static Fn 闭包）。
        let turn_fc = Arc::clone(&self.turn_file_changes);
        type NextExec = std::sync::Arc<
            dyn Fn(
                    HookToolCall,
                )
                    -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
                + Send
                + Sync,
        >;
        let mut chain: NextExec = {
            let ctx = ctx_arc.clone();
            let tools = tools_snapshot.clone();
            let cp = checkpoint_arc.clone();
            let hooks = hooks_arc.clone();
            let format_cfg_path = format_cfg_path.clone();
            let turn_fc = turn_fc.clone();
            std::sync::Arc::new(move |call: HookToolCall| {
                // Fn 闭包：捕获的 Arc 每次调用克隆一份（不能 move 出 Fn）。
                let ctx = ctx.clone();
                let tools = tools.clone();
                let cp = cp.clone();
                let hooks = hooks.clone();
                let format_cfg_path = format_cfg_path.clone();
                let turn_fc = turn_fc.clone();
                Box::pin(async move {
                    let tool_opt = tools.get(&call.name).cloned();
                    let tool_was_registered = tool_opt.is_some();
                    // A7：preview_all = checkpoint 预检多点版（multiedit
                    // 一次 dispatch 快照全部待改文件；单文件工具恰好一条，
                    // 与旧 preview 行为逐字节等价）。
                    // D3：预检清单同时喂消息级收集桶（session_key 分桶）——
                    // 收集独立于 checkpoint 是否挂载（消息↔文件变更映射不
                    // 依赖安全网开关）；快照仍只在挂载时发生。
                    let previewed = tool_opt
                        .as_ref()
                        .map(|tool| tool.preview_all(&call.arguments))
                        .unwrap_or_default();
                    for change in previewed {
                        turn_fc
                            .lock()
                            .entry(ctx.session_key.clone())
                            .or_default()
                            .push(change.clone());
                        if let Some(cp) = cp.as_ref() {
                            cp.snapshot(&change).await;
                        }
                    }
                    match tool_opt {
                        // P0 vault（C2，2026-09-22 计划 §3）：凭据别名最后
                        // 一刻注入——副本改写，上游四个表面（pre-hooks/
                        // args_preview/会话历史/observer）全部只见别名。
                        // 注入失败（别名不存在/vault 锁定）→ 错误串即工具
                        // 结果，工具不执行（fail loud）。
                        Some(tool) => {
                            let exec_args = match credential_injection::inject_credential_aliases(
                                tool.as_ref(),
                                &call.arguments,
                            ) {
                                Ok(a) => a,
                                Err(errmsg) => return errmsg,
                            };
                            match tool.execute(&exec_args, &ctx).await {
                                Ok(result) => {
                                    debug!(
                                        "[AgentLoop] Tool {} returned: {} bytes",
                                        call.name,
                                        result.len()
                                    );
                                    // A6（devtool-upgrade 阶段 5）format-on-save：
                                    // write_file / edit_file / multiedit（A7 批量
                                    // 版，逐文件去重依次格式化）成功后、PostToolUse
                                    // hooks **之前**跑——用户自定义 hook 看到/
                                    // 拿到的是格式化后的文件（计划明文的正交语
                                    // 义）；外层 C3 诊断带在瀑布之后，诊断同样作
                                    // 用于格式化后文件。失败/超时/开关关/无匹配
                                    // 格式化器全部静默——永不拖垮工具调用。
                                    let format_paths: Vec<String> =
                                        if !crate::turn_guard::tool_result_indicates_error(&result)
                                            && let Ok(args_val) =
                                                serde_json::from_str::<serde_json::Value>(
                                                    &call.arguments,
                                                )
                                        {
                                            match call.name.as_str() {
                                                "write_file" | "edit_file" => args_val
                                                    .get("path")
                                                    .and_then(|v| v.as_str())
                                                    .map(|p| vec![p.to_string()])
                                                    .unwrap_or_default(),
                                                "multiedit" => {
                                                    // 去重保序：同文件多条编辑只
                                                    // 格式化一次（重复跑也是 no-op，
                                                    // 省子进程）。
                                                    let mut seen = std::collections::HashSet::new();
                                                    args_val
                                                        .get("edits")
                                                        .and_then(|v| v.as_array())
                                                        .map(|arr| {
                                                            arr.iter()
                                                                .filter_map(|e| {
                                                                    e.get("path")
                                                                        .and_then(|p| p.as_str())
                                                                })
                                                                .filter(|p| {
                                                                    seen.insert((*p).to_string())
                                                                })
                                                                .map(|p| p.to_string())
                                                                .collect()
                                                        })
                                                        .unwrap_or_default()
                                                }
                                                _ => Vec::new(),
                                            }
                                        } else {
                                            Vec::new()
                                        };
                                    let mut result = result;
                                    for path_str in format_paths {
                                        result = crate::formatter::format_on_save(
                                            format_cfg_path.clone(),
                                            &path_str,
                                            &result,
                                        )
                                        .await;
                                    }
                                    if tool_was_registered {
                                        crate::hooks::run_post_hooks(&hooks, &call, result).await
                                    } else {
                                        result
                                    }
                                }
                                Err(err) => {
                                    warn!("[AgentLoop] Tool {} error: {}", call.name, err);
                                    if tool_was_registered {
                                        crate::hooks::run_post_failure_hooks(&hooks, &call, &err)
                                            .await
                                    } else {
                                        format!("Tool error: {err}")
                                    }
                                }
                            }
                        }
                        None => {
                            warn!("[AgentLoop] Unknown tool: {}", call.name);
                            format!("Error: Unknown tool '{}'", call.name)
                        }
                    }
                })
            })
        };
        for hook in scoped_hooks.iter().rev() {
            let hook = std::sync::Arc::clone(hook);
            let next = std::sync::Arc::clone(&chain);
            chain = std::sync::Arc::new(move |call: HookToolCall| {
                // async move 捕获 owned Arc → future 'static（摆脱 &self 借用）。
                let hook = std::sync::Arc::clone(&hook);
                let next = std::sync::Arc::clone(&next);
                Box::pin(async move { hook.around_tool_use(call, next).await })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
            }) as NextExec;
        }
        let hook_call_owned = hook_call.clone();
        let result = chain(hook_call_owned).await;

        // Record experience for Forge self-learning (non-blocking).
        #[cfg(feature = "forge")]
        {
            if let Some(ref forge) = self.forge {
                // Gate on the runtime master switch — without this, experiences
                // are recorded on every tool call even when forge.enabled=false.
                if forge.is_enabled() {
                    // Truncate payloads: despite the "summary" field name the
                    // previous code stored the FULL args/result (read_file/exec
                    // could be megabytes). 500 chars is enough for reflection
                    // stats. Note: the dedup hash below still uses the full
                    // parsed `args`, so hashing is unaffected.
                    let trunc = |s: &str| -> String { s.chars().take(500).collect() };
                    let exp = nemesis_types::forge::Experience {
                        id: uuid::Uuid::new_v4().to_string(),
                        tool_name: tool_call.name.clone(),
                        input_summary: trunc(&tool_call.arguments),
                        output_summary: trunc(&result),
                        success: !result.contains("SECURITY BLOCKED")
                            && !result.contains("Tool error:"),
                        duration_ms: tool_start.elapsed().as_millis() as u64,
                        timestamp: chrono::Local::now().to_rfc3339(),
                        session_key: format!("{}:{}", context.channel, context.chat_id),
                    };
                    let args = serde_json::from_str(&tool_call.arguments)
                        .unwrap_or(serde_json::Value::Null);
                    let _ = forge.collector().record_with_args(exp, &args).await;
                }
            }
        }

        result
    }

    /// C7：lsp 工具的 `rename` op 是写类（跨文件改内容）——plan 模式必须
    /// 拦。lsp 不在 [`Self::PLAN_MODE_WRITE_TOOLS`] 里（其余 op 只读，整表
    /// 拦截会误伤），故按 args 探测 op 字段特判。解析失败从严当写拦
    /// （plan 模式拒绝成本不对称：误拒可让用户切 /build，误放行改了文件）。
    pub(crate) fn is_lsp_write_call(tool_name: &str, arguments: &str) -> bool {
        if tool_name != "lsp" {
            return false;
        }
        serde_json::from_str::<serde_json::Value>(arguments)
            .ok()
            .and_then(|v| v.get("op").and_then(|o| o.as_str()).map(String::from))
            .map(|op| op == "rename")
            .unwrap_or(true)
    }

    /// F1：Plan 模式分发端唯一写放行——`write_file` 且目标路径落在
    /// `<workspace>/plans/` 前缀内。路径解析与 [`crate::loop_tools::validate_workspace_path`]
    /// 同构（相对路径 join 工作区根 + 双侧 `canonicalize_for_compare` 防
    /// 8.3 短名/大小写失配；目标不存在按最长存在祖先解析）。workspace_root
    /// 未注入（standalone）→ 无放行锚点，一律拦截（诚实从严）。
    pub(crate) fn plan_mode_write_allowed(&self, tool_name: &str, arguments: &str) -> bool {
        if tool_name != "write_file" {
            return false;
        }
        let Some(root) = self.workspace_root.read().clone() else {
            return false;
        };
        let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments) else {
            return false;
        };
        let Some(raw_path) = args.get("path").and_then(|v| v.as_str()) else {
            return false;
        };
        let target = std::path::Path::new(raw_path);
        let candidate = if target.is_absolute() {
            target.to_path_buf()
        } else {
            root.join(target)
        };
        let plans_dir = root.join("plans");
        let resolved = nemesis_path::paths::canonicalize_for_compare(&candidate);
        let plans_canon = nemesis_path::paths::canonicalize_for_compare(&plans_dir);
        resolved.starts_with(&plans_canon)
    }

    /// F-U3-2（UAT U3 实证）：档案管线任务的工作副本路径基准重写。
    ///
    /// worker prompt 宣称「所有文件读写必须在工作副本目录内进行」，但文件
    /// 工具的相对 `path` 以节点 workspace 根为基准、`exec` 缺省 cwd 也是
    /// workspace 根——worker 一旦用相对路径（glm-5.3-flash 实测发生），文件
    /// 就静默落在工作副本之外：变更集扫描只认 exec 目录，成果丢失 + 工作区
    /// 被污染。这里按 `context.tool_path_base` 把相对路径重写进工作副本
    /// （`exec`/`async_shell` 缺省 cwd 一并注入——缺省执行根=工作副本）；
    /// 普通会话 base 为 None，零行为变化。仅改写参数 JSON，不触碰工具语义
    /// 与安全管线（安全层看到的就是重写后的真实落点）。
    pub(crate) fn rewrite_tool_paths_for_base(
        call: &ToolCallInfo,
        context: &RequestContext,
    ) -> Option<ToolCallInfo> {
        let base = context.tool_path_base.as_ref()?;
        // 单 `path` 参数的文件面工具（相对 path → base.join）。
        const PATH_ARG_TOOLS: &[&str] = &[
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "list_dir",
            "file_exists",
            "create_dir",
            "delete_file",
            "delete_dir",
            "grep",
            "multiedit",
        ];
        // 执行类工具：`cwd`（exec）/ `working_dir`（async_shell），缺省时注入。
        const CWD_ARG_TOOLS: &[&str] = &["exec", "async_shell"];
        let name = call.name.as_str();
        let is_path = PATH_ARG_TOOLS.contains(&name);
        let is_cwd = CWD_ARG_TOOLS.contains(&name);
        if !is_path && !is_cwd {
            return None;
        }
        // 参数不是合法 JSON（后续 args_validator 会报）——保持原样透传。
        let Ok(mut args) = serde_json::from_str::<serde_json::Value>(&call.arguments) else {
            return None;
        };
        let join = |p: &str| base.join(p).to_string_lossy().into_owned();
        let mut changed = false;
        let rewrite_rel = |slot: &mut serde_json::Value, changed: &mut bool| {
            if let Some(p) = slot.as_str() {
                let t = p.trim();
                if !t.is_empty() && !std::path::Path::new(t).is_absolute() {
                    *slot = serde_json::Value::String(join(t));
                    *changed = true;
                }
            }
        };
        if is_path {
            if let Some(slot) = args.get_mut("path") {
                rewrite_rel(slot, &mut changed);
            }
            // multiedit 的批量形态：edits[].path 逐条重写。
            if name == "multiedit"
                && let Some(edits) = args.get_mut("edits").and_then(|v| v.as_array_mut())
            {
                for e in edits.iter_mut() {
                    if let Some(slot) = e.get_mut("path") {
                        rewrite_rel(slot, &mut changed);
                    }
                }
            }
        }
        if is_cwd {
            let key = if name == "exec" { "cwd" } else { "working_dir" };
            match args.get_mut(key) {
                Some(slot) => {
                    // 显式传了空串 = 视同缺省（工具侧同样回退默认目录）。
                    if slot.as_str().map(str::trim).unwrap_or("").is_empty() {
                        *slot = serde_json::Value::String(base.to_string_lossy().into_owned());
                        changed = true;
                    } else {
                        rewrite_rel(slot, &mut changed);
                    }
                }
                None => {
                    args[key] = serde_json::Value::String(base.to_string_lossy().into_owned());
                    changed = true;
                }
            }
        }
        if !changed {
            return None;
        }
        let mut out = call.clone();
        out.arguments = args.to_string();
        Some(out)
    }
}
