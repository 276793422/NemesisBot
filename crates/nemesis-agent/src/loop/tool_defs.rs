//! 工具表构建（build_tool_defs/effective_tool_defs）、文档折叠、并行安全预计算、PrecomputedTool、current_tool_doc_folding/hidden_tools。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
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
    /// by a 4-permit semaphore. Every dispatch goes through the depth-aware
    /// variant with the invoking instance's `detached_depth` — behaviorally
    /// identical to the old depth-0 dispatch for read-only tools (their
    /// `set_invocation_depth` is the trait's default no-op) and exactly
    /// what spawn needs for `max_depth` enforcement (G2/G3). Returns
    /// results in SOURCE ORDER (`join_all` preserves iteration order) so
    /// the serial guard-replay keeps the audit chain ordered = model
    /// source order (roadmap risk 3 hard constraint). cluster_rpc/exec/
    /// writers are never here (not parallel-safe) → the `__ASYNC__`
    /// continuation and executor paths are structurally excluded; spawn's
    /// own `__BG_SPAWN__` marker is handled by the unchanged inline-await
    /// replay in the for-loop.
    pub(crate) async fn precompute_parallel_batch(
        &self,
        tool_calls: &[ToolCallInfo],
        context: &RequestContext,
        depth: usize,
        // 压测①a（2026-09-30）：detached 工具闸随行（串行路径同款裁决，见
        // [`super::tool_dispatch::detached_tool_refusal`]）——并行安全批
        // （list_dir 等只读工具）同样不得绕过 no_tools / allowed_tools。
        no_tools: bool,
        allowed_tools: Option<Vec<String>>,
    ) -> Vec<PrecomputedTool> {
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let futs = tool_calls.iter().map(|tc| {
            let sem = sem.clone();
            let tc = tc.clone();
            let allowed_tools = allowed_tools.clone();
            async move {
                let _permit = sem.acquire().await.ok();
                let start = std::time::Instant::now();
                // detached 闸先行（与串行路径同位：校验/执行前的现场裁决）。
                if no_tools {
                    warn!(
                        "[AgentLoop] Detached no_tools gate refused tool '{}' (parallel batch)",
                        tc.name
                    );
                    return PrecomputedTool {
                        result: format!(
                            "Error: tool '{}' is refused — this run was started with tools disabled. Produce your final answer as plain text now. Do NOT call tools again.",
                            tc.name
                        ),
                        validation_failed: false,
                        duration_ms: start.elapsed().as_millis() as u64,
                    };
                }
                if let Some(allowed) = &allowed_tools
                    && !allowed.is_empty()
                    && !allowed.iter().any(|a| a == &tc.name)
                {
                    warn!(
                        "[AgentLoop] Detached allowed_tools gate refused tool '{}' (parallel batch)",
                        tc.name
                    );
                    return PrecomputedTool {
                        result: format!(
                            "Error: tool '{}' is outside the allowed tool set for this run (allowed: {}). Use only the allowed tools, or answer in plain text. Do NOT retry this tool.",
                            tc.name,
                            allowed.join(", ")
                        ),
                        validation_failed: false,
                        duration_ms: start.elapsed().as_millis() as u64,
                    };
                }
                let (result, validation_failed) = match self.check_tool_args(&tc) {
                    crate::args_validator::Outcome::Valid => (
                        self.handle_tool_call_at_depth(&tc, context, depth).await,
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
                            self.handle_tool_call_at_depth(&fixed, context, depth).await,
                            false,
                        )
                    }
                    crate::args_validator::Outcome::Invalid { message, .. } => {
                        warn!(
                            "[AgentLoop] Arg validation failed for tool '{}' (id={}): {}",
                            tc.name, tc.id, message
                        );
                        (format!("Tool error: {}", message), true)
                    }
                };
                PrecomputedTool {
                    result,
                    validation_failed,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }
        });
        futures::future::join_all(futs).await
    }

    /// prompt-pack pro（M3）：工具描述分档取值单点。Pro 体系查
    /// [`crate::prompt::tool_description`] 档位表（mini→lean、normal/big→
    /// full 优先否则 lean，未命中回落注册表原文）；Classic 体系恒回落
    /// 原文（字节不变）。`build_tool_defs` 与 `rebuild_full_tool_defs`
    /// （三处恢复环共用）都走这里。
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
    /// of duplicating the tier-filter/sort block (the transient-retry path
    /// predates the extraction and keeps its inline copy — untouched).
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
                    None => allowed.is_empty() || allowed.contains(&name.as_str()),
                }
            })
            .filter(|name| !tool_name_matches_hidden(&hidden, name))
            .filter(|name| {
                !plan_mode
                    || Self::PLAN_MODE_TOOLS.contains(&name.as_str())
                    || name.starts_with("mcp_")
                    || name.as_str() == "spawn"
            })
            .filter_map(|name| tools_guard.get(name).map(|tool| (name, tool)))
            .map(|(name, tool)| crate::types::ToolDefinition {
                tool_type: "function".to_string(),
                function: crate::types::ToolFunctionDef {
                    name: name.clone(),
                    description: self.description_for(name, tool.as_ref()),
                    parameters: tool.parameters(),
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
        let mut defs = self.build_tool_defs();
        if let Some(allowed) = instance.detached_allowed_tools()
            && !allowed.is_empty()
        {
            defs.retain(|d| allowed.contains(&d.function.name));
        }
        // 空白名单 = 不设限（与 run_detached 的 `!allowed.is_empty()` 守卫
        // 同语义——否则 Some(空) 会把供给收窄到零工具，两端语义劈叉）。
        self.apply_tool_doc_folding(defs, instance)
    }

    /// Y1 (Phase4-a): read `agents.tool_doc_folding` from config.json FRESH
    /// each call (same pattern as [`current_summarizer_prefix_reuse`] — the
    /// dashboard/CLI can flip it while the gateway runs). Absent section,
    /// unreadable file, or a standalone loop (no config_path) →
    /// `(false, default)` — folding off, tool defs byte-identical.
    pub(crate) fn current_tool_doc_folding(&self) -> (bool, usize) {
        const OFF: (bool, usize) = (false, crate::tool_doc_folding::DEFAULT_EXPAND_TOP_N);
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return OFF,
        };
        let v = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let Some(v) = v else {
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

    /// F8 (devtool-upgrade 阶段 3): read `agents.hidden_tools` from
    /// config.json FRESH each call (same pattern as
    /// [`Self::current_tool_doc_folding`] — dashboard/CLI edits take effect
    /// from the next turn without restarting the loop). Absent key,
    /// unreadable file, or a standalone loop (no config_path) → empty list
    /// (nothing hidden).
    pub(crate) fn current_hidden_tools(&self) -> Vec<String> {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let v = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        v.and_then(|v| {
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
        .unwrap_or_default()
    }

    /// 角色隐藏 fresh-read（2026-09-28 角色目录与分档供给；`current_hidden_tools`
    /// 同款模式——读 `agents.roles.hidden`，无 config_path / 缺键 / 坏 JSON →
    /// 空表）。目录是静态 17 项，通配语义不开放，按名精确匹配。
    pub(crate) fn current_hidden_roles(&self) -> Vec<String> {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let v = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        v.and_then(|v| {
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
        })
        .unwrap_or_default()
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

    /// 器官 4e（docs/PLAN §4.2）：三处恢复环共用的**全量** tool_defs 重建（原
    /// context 压缩环 / 429 环 / transient 环各内联一份的 map/collect 三连——
    /// 三处合一，语义保持全量不过折叠闸）。
    pub(crate) fn rebuild_full_tool_defs(&self) -> Vec<crate::types::ToolDefinition> {
        self.tools
            .read()
            .iter()
            .map(|(name, tool)| crate::types::ToolDefinition {
                tool_type: "function".to_string(),
                function: crate::types::ToolFunctionDef {
                    name: name.clone(),
                    description: self.description_for(name, tool.as_ref()),
                    parameters: tool.parameters(),
                },
            })
            .collect()
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
