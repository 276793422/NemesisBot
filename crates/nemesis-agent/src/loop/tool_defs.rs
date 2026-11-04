//! 工具表构建（build_tool_defs/effective_tool_defs）、文档折叠、并行安全预计算、PrecomputedTool、current_tool_doc_folding/hidden_tools。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
#[cfg(not(target_os = "linux"))]
use super::prelude::*;
use super::*;

/// The core agent execution loop.
///
/// In standalone mode, this wraps a single LLM provider, tool registry,
/// and agent config. In bus-integrated mode, it additionally owns a
/// registry of agent instances, a message bus adapter, summarizer,
/// and session busy tracker.
///
/// U5 (sixth batch): one parallel-executed read-only call's result, captured
/// for serial guard replay in source order. `validation_failed` lets the
/// serial loop replay the `validation_failures` counter exactly as the serial
/// path would (Invalid → +1; Valid/Fixed → reset to 0).
pub(crate) struct PrecomputedTool {
    pub(crate) result: String,
    pub(crate) validation_failed: bool,
    pub(crate) duration_ms: u64,
}

// L19（改进项三期 2026-11-03）：并行批双上限（手法3 背压的 U5 形态）。
// 取宽值防回归：128 调用 / 8 MiB 覆盖任何现实批（模型输出级别的 args），
// 只有病态大包才被诚实拒绝。
pub(crate) const PARALLEL_BATCH_MAX_CALLS: usize = 128;
pub(crate) const PARALLEL_BATCH_MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;

/// L19 手法1+3：并行批专属准入池（OnceLock 静态单池——跨批/跨 agent loop
/// 共享同一 CPU 准入信号量与双上限账本；借用形态：执行体借用 `&self`/
/// `&context` 无法 `'static` spawn，经 [`WorkPool::try_reserve`] 拿账本
/// 预留 + [`WorkPool::permits`] 拿准入，由调用方 future 自持到执行结束）。
///
/// pending 上限 = 批调用数上限（**预检=池容量连贯**：预检放行的单批恒可
/// 整体预留，不会中段撞池；跨 loop 并发大批共享同一账本，超过账本容量的
/// 并发总量才会中段拒绝并回滚整批——这就是手法3 要的跨批背压）。准入
/// 许可仍走手法1 的 `max(1, N-1)`。
fn parallel_batch_pool() -> &'static std::sync::Arc<nemesis_utils::workpool::WorkPool> {
    use std::sync::OnceLock;
    static POOL: OnceLock<std::sync::Arc<nemesis_utils::workpool::WorkPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        std::sync::Arc::new(nemesis_utils::workpool::WorkPool::new(
            nemesis_utils::workpool::PoolLimits {
                permits: nemesis_utils::workpool::cpu_admission_permits(),
                max_pending: PARALLEL_BATCH_MAX_CALLS,
                max_input_bytes: PARALLEL_BATCH_MAX_INPUT_BYTES,
            },
        ))
    })
}

impl AgentLoop {
    /// U5 (sixth batch), G3-extended: is `name` eligible for the parallel
    /// pool? Looks up the agent-side registry and asks the tool's
    /// `is_parallel_safe()` (= `is_read_only()` by default; spawn opts in
    /// explicitly). Fail-closed: unknown tools and writer tools return
    /// false → never join the parallel pool. When executor separation is
    /// ON, the MOVE_TOOLS (incl. read_file/list_dir/grep) are
    /// `RemoteExecutorTool` instances whose `is_read_only()` is the default
    /// `false` — so an executor-separated batch naturally falls back to
    /// serial here, no separate check needed.
    pub(crate) fn tool_is_parallel_safe(&self, name: &str) -> bool {
        match self.tools.read().get(name) {
            Some(t) => t.is_parallel_safe(),
            None => false,
        }
    }

    /// U5: the result of one parallel-executed call, captured for serial
    /// guard replay in source order. `validation_failed` lets the serial
    /// loop replay the `validation_failures` counter exactly as the serial
    /// path would (Invalid → +1; Valid/Fixed → reset to 0).
    ///
    /// U5/G3: concurrently execute an ALL-parallel-safe batch (validated
    /// read-only-or-opted-in by the caller). Each task runs the SAME
    /// execution path as the serial loop's match (`check_tool_args` →
    /// `handle_tool_call_at_depth` or synthesize an Invalid error), gated
    /// by the L19 shared admission pool ([`parallel_batch_pool`]:
    /// `max(1, N-1)` CPU permits + dual-cap backlog). Every dispatch goes
    /// through the depth-aware variant with the invoking instance's
    /// `detached_depth` — behaviorally identical to the old depth-0 dispatch
    /// for read-only tools (their `set_invocation_depth` is the trait's
    /// default no-op) and exactly what spawn needs for `max_depth`
    /// enforcement (G2/G3). Returns results in SOURCE ORDER (`join_all`
    /// preserves iteration order) so the serial guard-replay keeps the
    /// audit chain ordered = model source order (roadmap risk 3 hard
    /// constraint). cluster_rpc/exec/writers are never here (not
    /// parallel-safe) → the `__ASYNC__` continuation and executor paths are
    /// structurally excluded; spawn's own `__BG_SPAWN__` marker is handled
    /// by the unchanged inline-await replay in the for-loop.
    ///
    /// L19 手法3：批级预检（调用数 / 累计 args 字节超 [`PARALLEL_BATCH_MAX_CALLS`]
    /// / [`PARALLEL_BATCH_MAX_INPUT_BYTES`]）→ 全批类型化拒绝（零执行零挂起，
    /// 每调用回同一条 "pool overloaded (rejected, not queued)" 消息）；批内
    /// 逐调用 `try_reserve` 失败 → 回滚整批预留 → 全批同拒绝（不部分执行）。
    pub(crate) async fn precompute_parallel_batch(
        &self,
        tool_calls: &[ToolCallInfo],
        context: &RequestContext,
        depth: usize,
        // 压测①a（2026-09-30）：detached 工具闸随行（串行路径同款裁决，见
        // [`super::tool_dispatch::environment_constraint_refusal_for`]）——
        // 并行安全批（list_dir 等只读工具）同样不得绕过 no_tools / allowed_tools。
        no_tools: bool,
        allowed_tools: Option<Vec<String>>,
        // B2（profession-capability-evolution 2026-10-14）：职能工具围栏随
        // 行（串行路径同款裁决，同一注册点 [`super::tool_dispatch::
        // environment_constraint_refusal_for`]）——只读工具同样不得绕过围栏。
        // 逐 future clone，调用方从 instance config 取值（None/空 = 未围，
        // 恒 `None` 放行）。
        fence_profession: Option<String>,
        fence_tools: Vec<String>,
    ) -> Vec<PrecomputedTool> {
        // L19 手法3：批级预检 + 逐调用账本预留（共享池——跨批/跨 loop 统一
        // CPU 准入与双上限背压）。超限 = 全批类型化拒绝（零执行零挂起）。
        let pool = parallel_batch_pool();
        let overload_msg = |why: &str| -> String {
            format!(
                "Error: parallel tool batch rejected — {why}. Split the batch into smaller groups and retry. Do NOT retry the same oversized batch."
            )
        };
        if tool_calls.len() > PARALLEL_BATCH_MAX_CALLS {
            warn!(
                "[AgentLoop] Parallel batch rejected: {} calls > cap {}",
                tool_calls.len(),
                PARALLEL_BATCH_MAX_CALLS
            );
            let msg = overload_msg(&format!(
                "{} calls exceeds the batch cap of {}",
                tool_calls.len(),
                PARALLEL_BATCH_MAX_CALLS
            ));
            return tool_calls
                .iter()
                .map(|_| PrecomputedTool {
                    result: msg.clone(),
                    validation_failed: false,
                    duration_ms: 0,
                })
                .collect();
        }
        let total_bytes: usize = tool_calls.iter().map(|tc| tc.arguments.len()).sum();
        if total_bytes > PARALLEL_BATCH_MAX_INPUT_BYTES {
            warn!(
                "[AgentLoop] Parallel batch rejected: {} args bytes > cap {}",
                total_bytes, PARALLEL_BATCH_MAX_INPUT_BYTES
            );
            let msg = overload_msg(&format!(
                "combined tool arguments of {} bytes exceed the batch cap of {} bytes",
                total_bytes, PARALLEL_BATCH_MAX_INPUT_BYTES
            ));
            return tool_calls
                .iter()
                .map(|_| PrecomputedTool {
                    result: msg.clone(),
                    validation_failed: false,
                    duration_ms: 0,
                })
                .collect();
        }
        // 逐调用账本预留：任一失败 → 回滚整批（drop 全部预留）→ 全批同拒绝
        // （不部分执行——批级原子性）。
        let mut reservations = Vec::with_capacity(tool_calls.len());
        let mut over: Option<nemesis_utils::workpool::PoolOverloaded> = None;
        for tc in tool_calls {
            match pool.try_reserve(tc.arguments.len()) {
                Ok(r) => reservations.push(r),
                Err(e) => {
                    over = Some(e);
                    break;
                }
            }
        }
        if let Some(err) = over {
            drop(reservations);
            warn!(
                "[AgentLoop] Parallel batch rejected by shared pool: {}",
                err
            );
            let msg = overload_msg(&err.to_string());
            return tool_calls
                .iter()
                .map(|_| PrecomputedTool {
                    result: msg.clone(),
                    validation_failed: false,
                    duration_ms: 0,
                })
                .collect();
        }
        let permits = pool.permits();
        // 并行批审批聚合（BUG-A 第三层，2026-11-05）：本批共享一个批号，
        // 批内任一 ask 卡经 metadata 随行批号 → 前端聚成一张卡
        // （N 项 + 全部允许/全部拒绝，一次裁决全批）。批号每批新生成，
        // 串行路径不生成、字节不变。
        let batch_id = format!("batch-{}", uuid::Uuid::new_v4());
        // 修④（BUG-C 2026-11-04）：逐 future 完成旗标 + 批起点墙钟（中断面
        // ToolFinished 的 duration 取批起点——池排队导致的起点偏差可接受，
        // 中断行本就是近似时刻）。
        let batch_started = std::time::Instant::now();
        let completed_flags: Vec<std::sync::Arc<std::sync::atomic::AtomicBool>> = (0..tool_calls
            .len())
            .map(|_| std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)))
            .collect();
        let futs = tool_calls
            .iter()
            .zip(reservations)
            .enumerate()
            .map(|(i, (tc, reservation))| {
                let completed = completed_flags[i].clone();
                let permits = permits.clone();
                let tc = tc.clone();
                let allowed_tools = allowed_tools.clone();
                let fence_profession = fence_profession.clone();
                let fence_tools = fence_tools.clone();
                let batch_id = batch_id.clone();
                async move {
                    // L19：账本预留 + 准入许可由执行体自持到执行结束
                    //（借用形态；Drop 释放先于结果被消费方读取）。
                    let _reservation = reservation;
                    let _permit = permits.acquire_owned().await.ok();
                    let start = std::time::Instant::now();
                // 修2（2026-11-04）：环境约束预拒统一注册点（detached +
                // fence 两约束一处声明，与串行路径
                // [`super::tool_dispatch::environment_constraint_refusal`]
                // 同源——文案不外散 inline，新增约束只改注册点一处）。
                if let Some(refusal) = super::tool_dispatch::environment_constraint_refusal_for(
                    no_tools,
                    allowed_tools.as_deref(),
                    fence_profession.as_deref(),
                    &fence_tools,
                    &tc.name,
                ) {
                    warn!(
                        "[AgentLoop] Environment constraint refused tool '{}' (parallel batch): {}",
                        tc.name,
                        refusal.trim_start_matches("Error: ")
                    );
                    return PrecomputedTool {
                        result: refusal,
                        validation_failed: false,
                        duration_ms: start.elapsed().as_millis() as u64,
                    };
                }
                let (result, validation_failed) = match self.check_tool_args(&tc).await {
                    crate::args_validator::Outcome::Valid => (
                        self.handle_tool_call_at_depth_batched(&tc, context, depth, Some(&batch_id))
                            .await,
                        false,
                    ),
                    crate::args_validator::Outcome::Fixed(fixed_args) => {
                        info!(
                            "[AgentLoop] Auto-fixed args for tool '{}' (id={})",
                            tc.name, tc.id
                        );
                        let mut fixed = tc.clone();
                        fixed.arguments = fixed_args;
                        (
                            self.handle_tool_call_at_depth_batched(&fixed, context, depth, Some(&batch_id))
                                .await,
                            false,
                        )
                    }
                    crate::args_validator::Outcome::Invalid { message, .. } => {
                        warn!(
                            "[AgentLoop] Arg validation failed for tool '{}' (id={}): {}",
                            tc.name, tc.id, message
                        );
                        // L10-建设：回灌单一真相源（惰性供给时附 tools_schema_get
                        // 取回指引，补齐折叠后首调错参的自纠通路）。
                        (
                            validation_error_feedback(
                                &message,
                                self.current_lazy_tool_schema(),
                                &tc.name,
                            ),
                            true,
                        )
                    }
                };
                // 修④（BUG-C 2026-11-04）：完成旗标在此置位——与 hook 链内
                // ToolFinished 发射同点（handle_tool_call_at_depth 返回即事件
                // 已发），其后到旗标置位之间无 await（同一 poll 内同步完成）
                // → drop 窗口不存在：旗标 false ⇒ ToolFinished 未发，estop
                // 中断臂据此补发，不重不漏。
                completed.store(true, std::sync::atomic::Ordering::SeqCst);
                PrecomputedTool {
                    result,
                    validation_failed,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }
        });
        // 修④（BUG-C 2026-11-04）：biased estop-first select 包住 join_all
        // ——急停在批内窗口到达即中断（旧语义：批跑完、下一轮边界才停，
        // 等待最坏 = 整批墙钟）。中断 = drop join（在飞 future 全部取消；
        // exec 族子进程树由 Job Object RAII + gateway estop sweep 收杀，见
        // exec_tree 模块注释；web_fetch/MCP 网络 fut drop = 断连天然安全
        // ——drop 安全性审计全文见 tool_dispatch 臂 C 同款注释）。
        let mut estop_rx = self.security.subscribe();
        let joined = futures::future::join_all(futs);
        tokio::pin!(joined);
        tokio::select! {
            biased;
            _ = Self::wait_estop_engaged(estop_rx.as_mut()) => {
                warn!(
                    "[AgentLoop] E-stop interrupted parallel tool batch ({} calls).",
                    tool_calls.len()
                );
                // 逐项收口：!completed 补发中断面 ToolFinished（id 用与
                // dispatch 侧同一 call_id_for 派生——raw id 非空时精确命中
                // ToolStarted id，卡片不悬空；空 id = 时间戳兜底两侧必不
                // 一致 → 悬空，罕见，诚实边界记入修复报告）；completed 者
                // 事件已发，只补占位行（biased 下 join 同时就绪而 estop 先
                // 出线的窄竞态，真实结果被丢弃）。
                let rows: Vec<PrecomputedTool> = tool_calls
                    .iter()
                    .enumerate()
                    .map(|(i, tc)| {
                        let done = completed_flags[i].load(std::sync::atomic::Ordering::SeqCst);
                        if !done {
                            let call_id = crate::turn_wal::call_id_for(&tc.id, &tc.name);
                            self.emit_tool_interrupted(
                                &context.session_key,
                                &context.chat_id,
                                &call_id,
                                &tc.name,
                                batch_started,
                                "⛔ interrupted by e-stop (estop engaged mid-batch)",
                            );
                        }
                        PrecomputedTool {
                            result: if done {
                                "⛔ 已急停 (E-STOP)：该工具已完成，但结果被急停中断的批次收集丢弃，未进入上下文。不要重试。".to_string()
                            } else {
                                "⛔ ESTOP: 已急停 — 工具调用已被打断 (e-stop engaged mid-batch). 不要重试；告知用户当前处于急停状态，等待释放。".to_string()
                            },
                            validation_failed: false,
                            duration_ms: batch_started.elapsed().as_millis() as u64,
                        }
                    })
                    .collect();
                rows
            }
            results = joined => results,
        }
    }

    /// prompt-pack pro（M3）：工具描述分档取值单点。Pro 体系查
    /// [`crate::prompt::tool_description`] 档位表（mini→lean、normal/big→
    /// full 优先否则 lean，未命中回落注册表原文）；Classic 体系恒回落
    /// 原文（字节不变）。`build_tool_defs`（含恢复环经
    /// `effective_tool_defs` 的重建）都走这里。
    pub(crate) fn description_for(&self, name: &str, tool: &dyn Tool) -> String {
        if *self.prompt_system.read() != crate::prompt::PromptSystem::Pro {
            return tool.description();
        }
        let fallback = tool.description();
        crate::prompt::tool_description(name, &fallback, *self.tier.read()).to_string()
    }

    /// Build the LLM-visible tool definitions from the registry.
    ///
    /// Extracted verbatim from `run_llm_loop` (K1b, U14) so the post-LLM
    /// hook retry re-call rebuilds them from the same single source instead
    /// of duplicating the tier-filter/sort block (the recovery loops'
    /// full-rebuild variant now shares this ordering too — see
    /// `rebuild_full_tool_defs`).
    ///
    /// Mirrors Go's ToolRegistry.ToProviderDefs() which calls
    /// tool.Description() and tool.Parameters(). Sort by name so the order is
    /// stable across runs — a deterministic tool order gives reproducible
    /// behaviour and avoids unnecessary prompt variation between requests.
    pub(crate) fn build_tool_defs(&self) -> Vec<crate::types::ToolDefinition> {
        let tools_guard = self.tools.read();
        // Phase 3 (small-model-tool-robustness): tier-based toolset.
        // Empty allowed-list (Big/Auto) = show everything; Mini/Normal
        // see a restricted set to reduce small-model cognitive load.
        let allowed = nemesis_types::capability::tier_allowed_tools(*self.tier.read());
        // F8 (devtool-upgrade 阶段 3): third filter layer — config
        // `agents.hidden_tools` removes tools from the model's supply
        // entirely (dispatch re-checks the same list: double gate). Read
        // FRESH each call so dashboard/CLI edits apply from the next turn.
        let hidden = self.current_hidden_tools();
        // B4 (2026-10-19 connector 总开关)：`connectors.enabled` fresh-read
        // ——关 = connector_find/connector_invoke 整体从供给面消失（docker
        // daemon 停机语义；dispatch 同闸兜底陈旧 prompt cache）。
        let connectors_on = self.current_connectors_enabled();
        // B2 (2026-10-19 connector 独占接管)：独占隐藏表——闭包每次现算
        // （独占 server 前缀命中的当下 MCP 工具 − sidecar 豁免）。供给侧
        // 无出处概念：整表全隐；出处豁免只在 dispatch 端
        // （`connector-binding` 通道回灌放行）。
        let exclusive_hidden = self.current_exclusive_hidden_tools();
        // F4（二轮复评 2026-10-18）：tool_policy allow 白名单供给闸——非空 =
        // 白名单语义，不在名单（精确名匹配）的工具不供给。每回合现读槽值：
        // MCP 热加载的新工具自动落闸（旧「注册面 ∖ allow 折算」只吃注册期
        // 快照，动态新发现漏折——已被本闸取代）。空 = 无白名单语义。
        let policy_allow = self.tool_policy_allow();
        // W7（2026-10-25 加固）：MCP 惰性供给——lazy on 时 `mcp_*` 长尾
        // 不全量随轮上行，只留 meta 工具 `mcp_call`（模型两步走：mcp_list
        // 查 schema → mcp_call 调用；无中途注入，cache 字节稳定）。lazy off
        // = 过滤器恒真，行为零变化。
        let mcp_lazy = self.current_mcp_lazy_supply();
        // L10-建设（改进项三期 2026-11-02）：工具 schema 惰性供给——lazy on
        // 时全部工具 defs 只带 name+description（参数折叠为最小占位
        // `{"type":"object"}`），模型按需经 `tools_schema_get` 取回完整
        // schema（取回过的名字进 per-session 解锁账，build_messages 的
        // 「已解锁 schema 注记」节持续携带）。档位策略见
        // [`Self::current_lazy_tool_schema`]；lazy off = 恒 false，字节零变化。
        let schema_lazy = self.current_lazy_tool_schema();
        // F1 (devtool-upgrade 阶段 4): Plan 模式第四过滤层——只读白名单
        // ∩（MCP 前缀工具与 spawn 豁免：MCP 语义未知不猜；spawn 保留供给、
        // 分发闸兜底）。与 tier 过滤正交（Mini+Plan 取交集，验收项）。
        let plan_mode = *self.mode.read() == crate::types::AgentMode::Plan;
        let mut names: Vec<&String> = tools_guard.keys().collect();
        names.sort();
        names
            .into_iter()
            .filter(|name| {
                // WASM 插件 min-tier 供给闸（2026-09-30 插件体系复查 #2）：
                // 声明了最低档的工具绕过 tier 白名单、按档位秩比较
                // （active ≥ min 才供给）——mini/normal 白名单本就不含
                // `plugin.*` 名，逐名收录不可扩展，秩比较是唯一可行语义。
                // 无档位声明的工具走原白名单路径，行为不变。
                match tools_guard.get(name.as_str()).and_then(|t| t.min_tier()) {
                    Some(min) => active_tier_rank(*self.tier.read()) >= plugin_min_tier_rank(min),
                    // W7：lazy on 时 tier 白名单对 meta 两件套放行（mcp_call
                    // 调用 + mcp_list 查询，BUG-20261025-32 补查询面——
                    // mcp_call 的描述与错误臂都指路 mcp_list，缺它两步流程
                    // 第一步无工具可用；内层真实工具名在 dispatch 重走瀑布
                    // 时仍受内层校验）。
                    None => {
                        allowed.is_empty()
                            || allowed.contains(&name.as_str())
                            || (mcp_lazy && matches!(name.as_str(), "mcp_call" | "mcp_list"))
                    }
                }
            })
            .filter(|name| !tool_name_matches_hidden(&hidden, name))
            // B4：总开关关 → 连接器治理工具双名下线（名字真相源
            // nemesis-connectors FIND/INVOKE_TOOL_NAME；此处字面量与之一致
            // ——agent 对 connectors 是可选依赖，供给过滤在无 security 构建
            // 也要编译，字面量 + 测试钉住对齐）。
            .filter(|name| {
                connectors_on
                    || !(name.as_str() == "connector_find" || name.as_str() == "connector_invoke")
            })
            // B2：独占 server 的原生 mcp_* 工具从工具面消失（镜像 op 经
            // connector_invoke 接管路由——治理闸唯一入口）。
            .filter(|name| !tool_name_matches_hidden(&exclusive_hidden, name))
            .filter(|name| policy_allow.is_empty() || policy_allow.iter().any(|a| a == *name))
            .filter(|name| {
                // mcp_call 供给钉死在 lazy 旗标（off = 不出现，任何档位）；
                // mcp_list（两步流程的查询面，BUG-20261025-32）豁免长尾隐藏
                // 但不受 lazy 旗标钉死（off = 照常走前置各闸原路径，行为零
                // 变化）；其余 mcp_* 在 lazy on 时让位给 meta 两件套。
                if name.as_str() == "mcp_call" {
                    mcp_lazy
                } else if name.as_str() == "mcp_list" {
                    true
                } else {
                    !(mcp_lazy && name.starts_with("mcp_"))
                }
            })
            .filter(|name| {
                // L10-建设：tools_schema_get 供给钉死在 schema_lazy 旗标（同
                // mcp_call 形态）——lazy off（big/normal 显式关）= 全量
                // schema 随轮上行，取回工具无消费场景，不出现（mini/normal
                // 白名单成员无害隐藏）。
                name.as_str() != "tools_schema_get" || schema_lazy
            })
            .filter(|name| {
                !plan_mode
                    || Self::PLAN_MODE_TOOLS.contains(&name.as_str())
                    || name.starts_with("mcp_")
                    || name.as_str() == "spawn"
            })
            // N2（机会项）：present_plan 只在 plan 模式供给（build 模式下
            // 调用无意义且工具内已诚实报错——供给+调用双闸对齐）。
            .filter(|name| name.as_str() != "present_plan" || plan_mode)
            .filter_map(|name| tools_guard.get(name).map(|tool| (name, tool)))
            .map(|(name, tool)| crate::types::ToolDefinition {
                tool_type: "function".to_string(),
                function: crate::types::ToolFunctionDef {
                    name: name.clone(),
                    description: self.description_for(name, tool.as_ref()),
                    // L10-建设：惰性折叠——schema_lazy on 时参数折叠为最小
                    // 占位（name+description 全保，模型按需经 tools_schema_get
                    // 取回完整 schema）；取回工具自身豁免（模型必须先看到
                    // 它怎么用才能取别人的）。校验面不受影响：
                    // check_tool_args 读 registry 真实 schema，折叠占位不进校验。
                    parameters: if schema_lazy && name.as_str() != "tools_schema_get" {
                        serde_json::json!({ "type": "object" })
                    } else {
                        tool.parameters()
                    },
                },
            })
            .collect()
    }

    /// G0 (devtool-upgrade 阶段 3)：本回合生效的工具 defs 供给链——
    /// 全量注册 → tier 过滤 → **子代理白名单收窄**（`run_detached` 派生的
    /// instance 才带 `detached_allowed_tools`，普通回合 None 直通）→ 文档折叠。
    /// instance 级白名单（非 loop 级全局槽）：并发 detached 回合互不串扰。
    pub(crate) fn effective_tool_defs(
        &self,
        instance: &AgentInstance,
    ) -> Vec<crate::types::ToolDefinition> {
        // Swarm M1（裸提示词模式）：零工具供给直返空——纯文本单轮调用，
        // 模型看不到任何工具定义（优先级高于 tier/白名单/hidden 全链）。
        if instance.detached_no_tools() {
            return Vec::new();
        }
        let defs = self.build_tool_defs();
        // 修2（2026-11-04）：环境约束供给面过滤统一注册点（detached 白名单
        // + 职能围栏一处声明，与派发预拒
        // [`super::tool_dispatch::environment_constraint_refusal`] 同源双端
        // 对齐——供给闸只管 defs 可见性，上游端点注入的调用由派发闸现场
        // 裁决）。`question` 豁免照旧在过滤函数内生效（围栏内任务向人求助
        // 的唯一通道）。loop 级 config `tools` 恒空 → 主 loop / 普通对话 /
        // 项目 loop 零行为变化。
        let filtered = super::tool_dispatch::filter_defs_by_environment_constraints(
            defs,
            instance.detached_no_tools(),
            instance.detached_allowed_tools().as_deref(),
            &instance.config().tools,
        );
        // 空白名单 = 不设限（与 run_detached 的 `!allowed.is_empty()` 守卫
        // 同语义——否则 Some(空) 会把供给收窄到零工具，两端语义劈叉）；
        // 该守卫已内建于过滤函数的 `!allowed.is_empty()` 臂。
        self.apply_tool_doc_folding(filtered, instance)
    }

    /// Y1 (Phase4-a): read `agents.tool_doc_folding` from the current config
    /// generation (L18 世代快照读 — the dashboard/CLI can flip it while the
    /// gateway runs; new value lands next round via the round-start
    /// publication). Absent section, no generation (standalone loop / 绑定后
    /// 未发布) → `(false, default)` — folding off, tool defs byte-identical.
    pub(crate) fn current_tool_doc_folding(&self) -> (bool, usize) {
        const OFF: (bool, usize) = (false, crate::tool_doc_folding::DEFAULT_EXPAND_TOP_N);
        let Some(v) = self.config_generation() else {
            return OFF;
        };
        let sec = v.get("agents").and_then(|a| a.get("tool_doc_folding"));
        let enabled = sec
            .and_then(|s| s.get("enabled"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        let top_n = sec
            .and_then(|s| s.get("expand_top_n"))
            .and_then(|n| n.as_u64())
            .map(|n| n as usize)
            .unwrap_or(crate::tool_doc_folding::DEFAULT_EXPAND_TOP_N);
        (enabled, top_n)
    }

    /// F8 (devtool-upgrade 阶段 3): read `agents.hidden_tools` from the
    /// current config generation (L18 世代快照读 — dashboard/CLI edits take
    /// effect from the next turn without restarting the loop). Absent key, no
    /// generation (standalone loop) → empty list (nothing hidden).
    ///
    /// G1（2026-10-18 三方复评整改批三）：与 per-loop `extra_hidden_tools`
    /// （自建 agent tool_policy 折算，[`Self::set_extra_hidden_tools`]）
    /// 在此单点合并——供给闸（build_tool_defs）与 dispatch 闸共用本函数，
    /// 合并一处即双闸天然对齐。config 侧缺键/无代不影响 extra
    /// 侧生效；两侧去重合并，顺序不承诺（消费方做通配匹配不做全量比较）。
    pub(crate) fn current_hidden_tools(&self) -> Vec<String> {
        let mut merged: Vec<String> = self.extra_hidden_tools.read().clone();
        let config_list = self
            .config_generation()
            .and_then(|v| {
                v.get("agents")
                    .and_then(|a| a.get("hidden_tools"))
                    .and_then(|h| h.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| e.as_str())
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<String>>()
                    })
            })
            .unwrap_or_default();
        for name in config_list {
            if !name.is_empty() && !merged.contains(&name) {
                merged.push(name);
            }
        }
        merged
    }

    /// W7：MCP 惰性供给世代快照读（`agents.mcp_lazy_supply`，缺省 false）。
    /// current_hidden_tools 同款世代化模式——无 config_path / 缺键 / 坏 JSON
    /// → false（行为零变化面）。运行时改键下一轮生效。
    pub(crate) fn current_mcp_lazy_supply(&self) -> bool {
        self.config_generation()
            .and_then(|v| {
                v.get("agents")
                    .and_then(|a| a.get("mcp_lazy_supply"))
                    .and_then(|b| b.as_bool())
            })
            .unwrap_or(false)
    }

    /// L10-建设（改进项三期 2026-11-02）：工具 schema 惰性供给**档位单一
    /// 裁决**——build_tool_defs 折叠、供给闸、dispatch 拦截、build_messages
    /// 注记节四处同源消费，口径永不漂移：
    /// - Mini → 强制惰性（字节节省收益最大面；mini 拿不到取回工具=惰性
    ///   断供，供给白名单必含 tools_schema_get）；
    /// - Big | Auto → 恒全量（旗舰不缺上下文预算，惰性徒增取回往返）；
    /// - Normal → config `agents.lazy_tool_schema`（缺省 true=惰性，用户
    ///   拍板推荐方案；世代快照读同 [`Self::current_mcp_lazy_supply`]
    ///   世代化模式，无 config_path/缺键/坏 JSON = 缺省）。
    ///
    /// 与 mcp_lazy_supply 正交：那是 MCP 长尾**名面**两级化，这是全部工具
    /// **参数面**两级化；两者同开时 mcp_call 自身参数也折叠。
    pub(crate) fn current_lazy_tool_schema(&self) -> bool {
        match *self.tier.read() {
            nemesis_types::capability::ModelTier::Mini => true,
            nemesis_types::capability::ModelTier::Big
            | nemesis_types::capability::ModelTier::Auto => false,
            nemesis_types::capability::ModelTier::Normal => self
                .config_generation()
                .and_then(|v| {
                    v.get("agents")
                        .and_then(|a| a.get("lazy_tool_schema"))
                        .and_then(|b| b.as_bool())
                })
                .unwrap_or(true),
        }
    }

    /// L10-建设：记录一次 schema 解锁（dispatch 拦截内取回成功时调用）。
    /// per-session 只增不减、去重、按解锁顺序 append——注记渲染的前缀稳定
    /// 性质（prompt cache 只失效尾部）依赖 append-only。空 session_key
    /// 跳过（无会话归属的调用无处挂账）。进程内存态：重启即清，模型重取
    /// 即回（诚实边界，不持久化）。
    pub(crate) fn record_schema_unlock(&self, session_key: &str, tool: &str) {
        if session_key.is_empty() {
            return;
        }
        let mut ledger = self.tool_schema_unlocks.write();
        let entry = ledger.entry(session_key.to_string()).or_default();
        if !entry.iter().any(|n| n == tool) {
            entry.push(tool.to_string());
        }
    }

    /// L10-建设：渲染「已解锁 schema 注记」节（build_messages 消费）。
    /// **累积语义**（刻意，非 delta）：digest 注记消息每轮重建不持久——
    /// delta-only 注入会让已取 schema 在下一轮消失（只剩 history 里的
    /// tool result），模型无法持续对齐参数形态；渲染全部已解锁
    /// （append-only、按解锁顺序、前缀稳定）保证 schema 跨轮持续可见且
    /// 活过 compaction，新增解锁只失效 cache 尾部。无解锁 / session_key
    /// None/空 = 空串（零字节，调用方还须以 lazy 旗标门控）。registry
    /// 缺名（重装/改名后）跳过该名——账面与供给面诚实解耦。
    pub(crate) fn schema_unlock_section(&self, session_key: Option<&str>) -> String {
        let key = match session_key {
            Some(k) if !k.is_empty() => k,
            _ => return String::new(),
        };
        let names = match self.tool_schema_unlocks.read().get(key) {
            Some(v) if !v.is_empty() => v.clone(),
            _ => return String::new(),
        };
        let tools_guard = self.tools.read();
        let mut out = String::from(
            "# Unlocked Tool Schemas\n\n以下工具的完整参数 schema 已按需取回（惰性供给模式），持续有效——直接按参数形态调用即可，无需重复取回。\n",
        );
        let mut rendered = 0usize;
        for name in &names {
            let Some(tool) = tools_guard.get(name.as_str()) else {
                continue;
            };
            out.push_str(&format!(
                "\n## `{}`\n- **description**: {}\n- **parameters**: {}\n",
                name,
                self.description_for(name, tool.as_ref()),
                serde_json::to_string(&tool.parameters()).unwrap_or_else(|_| "{}".to_string())
            ));
            rendered += 1;
        }
        if rendered == 0 {
            return String::new();
        }
        out.push_str(
            "\n其余工具仍为惰性供给（仅 name+description）：按描述直接调用通常即可；参数校验被拒时先经 tools_schema_get 取回完整 schema 再重试。\n",
        );
        out
    }

    /// 角色隐藏世代快照读（2026-09-28 角色目录与分档供给；`current_hidden_tools`
    /// 同款世代化模式——读 `agents.roles.hidden`，无 config_path / 缺键 / 坏
    /// JSON → 空表）。目录是静态 17 项，通配语义不开放，按名精确匹配。
    pub(crate) fn current_hidden_roles(&self) -> Vec<String> {
        let Some(v) = self.config_generation() else {
            return Vec::new();
        };
        v.get("agents")
            .and_then(|a| a.get("roles"))
            .and_then(|r| r.get("hidden"))
            .and_then(|h| h.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default()
    }

    /// NB-122 批 ④（2026-10-16）：工具调用墙钟看门狗——世代快照读
    /// `agents.executor_tool_timeout_secs`（秒；L18 世代化）。缺键/无
    /// config_path/无代 = 默认 1800s；0 = 关（返回 None）。这是**卡死兜底闸
    /// 不是提速闸**：阈值要放得过合法长活（cargo build --release 10-20 分钟
    /// 量级）。
    pub(crate) fn current_executor_tool_timeout(&self) -> Option<std::time::Duration> {
        const DEFAULT_SECS: u64 = 1800;
        let secs = self
            .config_generation()
            .and_then(|v| {
                v.get("agents")
                    .and_then(|a| a.get("executor_tool_timeout_secs"))
                    .and_then(|n| n.as_u64())
            })
            .unwrap_or(DEFAULT_SECS);
        (secs > 0).then(|| std::time::Duration::from_secs(secs))
    }

    /// 角色供给的**单一裁决**：当前 tier 分档可见集 − 配置隐藏集（保持目录
    /// 顺序）。dispatch 闸与 `roles.list` 消费同一函数——两处口径永不漂移。
    /// tier → 线格式映射在本地（nemesis-prompts 零依赖不引 nemesis-types）；
    /// Auto/未知档按最宽档（与 `resolve_active_tier` 缺省 big 同哲学）。
    pub(crate) fn visible_roles(&self) -> Vec<&'static str> {
        let tier_str = match *self.tier.read() {
            nemesis_types::capability::ModelTier::Mini => "mini",
            nemesis_types::capability::ModelTier::Normal => "normal",
            nemesis_types::capability::ModelTier::Auto
            | nemesis_types::capability::ModelTier::Big => "big",
        };
        let hidden = self.current_hidden_roles();
        nemesis_prompts::subagents::SubagentRole::roles_visible_to(tier_str)
            .into_iter()
            .filter(|slug| !hidden.iter().any(|h| h == slug))
            .collect()
    }

    /// Y1 (Phase4-a): semantic tool-documentation folding, applied AFTER the
    /// tier filter and ORTHOGONAL to tool supply — folded tools stay
    /// callable with their full parameter schema; only the description text
    /// shrinks. Returns the defs byte-unchanged unless EVERY gate opens:
    /// config enabled, tier != Mini (the 13-tool core set has nothing to
    /// save), a wired memory manager (P3.1 embed backend), a non-empty
    /// latest user message, and successful embeddings for the query and
    /// every tool description (all-or-nothing — a tool that cannot be ranked
    /// must not be folded). Deterministic for a given (descriptions, query,
    /// embed backend) triple, so the two call sites (main call + hook retry
    /// re-call) and any rebuild that re-derives defs render the same bytes.
    pub(crate) fn apply_tool_doc_folding(
        &self,
        defs: Vec<crate::types::ToolDefinition>,
        instance: &AgentInstance,
    ) -> Vec<crate::types::ToolDefinition> {
        #[cfg(feature = "memory")]
        let (enabled, top_n) = self.current_tool_doc_folding();
        #[cfg(not(feature = "memory"))]
        let (enabled, _) = self.current_tool_doc_folding();
        if !enabled {
            return defs;
        }
        if *self.tier.read() == nemesis_types::capability::ModelTier::Mini {
            return defs;
        }
        #[cfg(feature = "memory")]
        {
            // Latest USER message is the ranking signal (same choice as
            // `prefetch_memory_context`).
            let query = instance
                .get_history()
                .iter()
                .rev()
                .find(|t| t.role == "user")
                .map(|t| t.content.clone())
                .unwrap_or_default();
            if query.trim().is_empty() {
                return defs;
            }
            let manager = match self.memory.memory_inject_manager.read().clone() {
                Some(m) => m,
                None => {
                    debug!(
                        "[AgentLoop] tool_doc_folding enabled but no memory manager (embed \
                         backend) wired — leaving docs unfolded"
                    );
                    return defs;
                }
            };
            let query_vec = match manager.embed_text(&query) {
                Some(v) => v,
                None => {
                    debug!(
                        "[AgentLoop] tool_doc_folding: query embedding failed — leaving docs \
                         unfolded"
                    );
                    return defs;
                }
            };
            let mut sims: std::collections::HashMap<String, f32> =
                std::collections::HashMap::with_capacity(defs.len());
            {
                // Cache guard held only for the embed-and-rank pass (the fold
                // render itself touches no shared state).
                let mut cache = self.memory.tool_vec_cache.write();
                for d in &defs {
                    let cached = cache.get(&d.function.name);
                    let vec = match cached {
                        Some((desc, v)) if desc == &d.function.description => v.clone(),
                        _ => match manager.embed_text(&d.function.description) {
                            Some(v) => {
                                cache.insert(
                                    d.function.name.clone(),
                                    (d.function.description.clone(), v.clone()),
                                );
                                v
                            }
                            None => {
                                debug!(
                                    "[AgentLoop] tool_doc_folding: embedding failed for tool \
                                     {} — leaving docs unfolded",
                                    d.function.name
                                );
                                return defs;
                            }
                        },
                    };
                    let sim = crate::tool_doc_folding::cosine(&query_vec, &vec);
                    sims.insert(d.function.name.clone(), sim);
                }
            }
            crate::tool_doc_folding::fold_tool_defs(defs, &sims, top_n)
        }
        #[cfg(not(feature = "memory"))]
        {
            // No embed backend compiled in — folding cannot rank; passthrough.
            let _ = instance;
            defs
        }
    }

    /// L10-量测（改进项三期 2026-11-02）：tool defs 供给面字节量测——
    /// 当前生效 defs（[`Self::build_tool_defs`] 单一真相源）序列化为
    /// JSON 的字节数 + 工具个数，供 `chat.context_status` 与 agent-bench
    /// 记分卡消费（惰性 schema 改造 L10-建设的前后对比基线）。
    ///
    /// 口径注记：provider wire 字节因 lane 而异（anthropic 形态字段名
    /// 不同），这里统一用 openai `ToolDefinition` 形态作**稳定代理指标**
    /// ——前后对比只看同口径差值，不主张等于真实 wire 字节。报告
    /// loop 级默认供给面（instance 级白名单 / plan 收窄 / 文档折叠不
    /// 在内——那些是回合态；量测语义是「这个 loop 现在会给模型多少工具」）。
    pub fn tool_defs_metrics(&self) -> serde_json::Value {
        let defs = self.build_tool_defs();
        let bytes = serde_json::to_vec(&defs).map(|v| v.len()).unwrap_or(0);
        serde_json::json!({
            "tools": defs.len(),
            "bytes": bytes,
        })
    }
}

// ---------------------------------------------------------------------------
// 自由函数归位（P1-c 自 loop.rs 根搬迁；仅增 pub(crate) 可见性标注）
// ---------------------------------------------------------------------------

/// F8 (devtool-upgrade 阶段 3): wildcard-aware membership test for
/// `agents.hidden_tools`. An entry matches a tool name either exactly or,
/// when it ends with `*`, as a prefix (`mcp_*` hides every MCP tool). A bare
/// `*` hides everything. Matching is case-sensitive (tool names are
/// lowercase by convention). Shared by BOTH gates — the supply side
/// ([`AgentLoop::build_tool_defs`]) and the dispatch side
/// ([`AgentLoop::handle_tool_call_at_depth`]) — so the two can never
/// disagree about what is hidden.
pub(crate) fn tool_name_matches_hidden(entries: &[String], name: &str) -> bool {
    entries.iter().any(|entry| {
        if let Some(prefix) = entry.strip_suffix('*') {
            name.starts_with(prefix)
        } else {
            entry == name
        }
    })
}

/// WASM 插件 min-tier 秩（2026-09-30 插件体系复查 #2）：mini=0 / normal=1 /
/// big=2。未知/空串按 big（=2，最严——不给声明走样的插件开白名单口子；
/// manifest 校验本就拦非法值，此分支纯合成数据兜底）。
pub(crate) fn plugin_min_tier_rank(tier: &str) -> u8 {
    match tier {
        "mini" => 0,
        "normal" => 1,
        _ => 2,
    }
}

/// 活跃模型档位秩（与 [`plugin_min_tier_rank`] 同一标尺；Auto 防御性按
/// big——loop 内 tier 启动时已 resolve，此分支纯兜底，语义与「缺省 big
/// 不饿着强模型」一致）。
pub(crate) fn active_tier_rank(tier: nemesis_types::capability::ModelTier) -> u8 {
    use nemesis_types::capability::ModelTier;
    match tier {
        ModelTier::Mini => 0,
        ModelTier::Normal => 1,
        ModelTier::Big | ModelTier::Auto => 2,
    }
}

/// L10-建设：args_validator Invalid 回灌的单一真相源（并行预计算臂
/// [`AgentLoop::precompute_tools`] 与串行批 [`AgentLoop::run_batch`]
/// 两处同源消费）。惰性供给开启时折叠占位使模型看不到真实参数面，
/// 首调错参是预期形态——回灌附取回指引，补齐自纠通路（否则小模型
/// 会反复盲试撞尽 validation 预算）。取回工具自身校验失败不指引
/// （它的 schema 恒全量，错参无折叠原因）。
pub(crate) fn validation_error_feedback(message: &str, schema_lazy: bool, tool: &str) -> String {
    let base = format!("Tool error: {}", message);
    if schema_lazy && tool != "tools_schema_get" {
        format!(
            "{}\n（惰性供给模式：该工具的参数 schema 未随轮上行。先调用 tools_schema_get 取回 `{}` 的完整 schema，再按 schema 重试。）",
            base, tool
        )
    } else {
        base
    }
}
