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
    ) -> Vec<PrecomputedTool> {
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let futs = tool_calls.iter().map(|tc| {
            let sem = sem.clone();
            let tc = tc.clone();
            async move {
                let _permit = sem.acquire().await.ok();
                let start = std::time::Instant::now();
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
            .filter(|name| allowed.is_empty() || allowed.contains(&name.as_str()))
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
                    description: tool.description(),
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
                    description: tool.description(),
                    parameters: tool.parameters(),
                },
            })
            .collect()
    }
}
