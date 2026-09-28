//! 上下文压缩：CompactState、三级 context_window 解析链、summarize 失效判定、maybe_update_summary、force_compression(_opts)、compact_session、session_context_status、请求历史投影。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// ⑩ Per-session compaction tracking for graded tiers + stuck self-check.
/// Keyed by session; lives on `AgentLoop`.
#[derive(Default)]
pub(crate) struct CompactState {
    /// Whether the soft-tier (50%) notice has already been emitted this session
    /// (one-shot; do not nag).
    soft_noticed: bool,
    /// Token estimate at the time of the last summarization. The next
    /// summarization-triggered call compares against this to detect whether
    /// summarization is keeping up.
    pub(crate) last_summary_tokens: usize,
    /// Consecutive summarizations that failed to meaningfully reduce the
    /// prompt. At [`COMPACT_STUCK_LIMIT`] we pause auto-summarization.
    pub(crate) consecutive_failures: u32,
    /// Auto-summarization paused (stuck). Re-checked each call; cleared once
    /// the prompt drops back below the summarize threshold.
    stuck: bool,
}

/// ⑩ Soft tier: prompt at this fraction of context_window emits a one-shot
/// info notice (no summarization, cache-stable prefix intact).
const COMPACT_SOFT_RATIO: usize = 50; // % of context_window
/// ⑩ Summarize tier: at this fraction, trigger summarization.
const COMPACT_SUMMARIZE_RATIO: usize = 75; // % (unchanged from legacy behavior)
/// ⑩ Stuck: a summarization counts as ineffective when the prompt afterwards
/// is still at least this fraction of its pre-summarization size.
const COMPACT_STUCK_PLATEAU_RATIO: usize = 90; // %

/// N1（devtool-upgrade 阶段 1）：context_window 三级解析链的最后一级兜底。
/// 历史 32000 对现代 128k+ 模型意味着 24000 token 就触发压缩（编码场景过早
/// 失忆）——未配置且价目表未命中时按 128000 猜；猜大了也有既有溢出应急链
/// （`context_length_exceeded` → `force_compression` → 重试）自愈。
pub const FALLBACK_CONTEXT_WINDOW: usize = 128_000;

/// N1：三级 context_window 解析（纯函数，`AgentLoop` 与 CLI `model list` /
/// `model probe` 共用同一真相源）。返回 `(窗口, 来源)`：
/// - L1 config 显式 → `(Some(w), "config")`
/// - L2 价目表命中（`max_input_tokens > 0`，custom > downloaded > embedded
///   分层 + bare-suffix 匹配，`zhipu/glm-4.7`→`glm-4.7`）→ `(Some(w), "catalog")`
/// - L3 都没有 → `(None, "fallback-128k")`，调用方用
///   [`FALLBACK_CONTEXT_WINDOW`] 兜底。
pub fn resolve_context_window_tiered(
    cfg: Option<&serde_json::Value>,
    alias: &str,
    pricing: Option<&nemesis_data::PricingStore>,
) -> (Option<usize>, &'static str) {
    if let Some(cfg) = cfg
        && let Some(w) = nemesis_types::capability::resolve_context_window(cfg, alias)
    {
        return (Some(w as usize), "config");
    }
    if let Some(store) = pricing {
        // L2 第一轮：直接用活动别名查（standalone 形态下 alias 即 full
        // model 名，bare-suffix 匹配在 store.lookup 内部完成）。
        if let Some(w) = store
            .lookup(alias)
            .and_then(|p| p.max_input_tokens)
            .filter(|w| *w > 0)
        {
            return (Some(w as usize), "catalog");
        }
        // L2 第二轮：config 条目把别名映射到 full model 名（如
        // `glm`→`zhipu/glm-4.7`）时，用 full 名再查一轮——bare-suffix
        // `glm-4.7` 命中目录条目。
        if let Some(cfg) = cfg {
            let full = cfg
                .get("model_list")
                .and_then(|v| v.as_array())
                .and_then(|arr| {
                    arr.iter().find(|m| {
                        let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                        let full_model = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                        name == alias || full_model == alias
                    })
                })
                .and_then(|m| m.get("model"))
                .and_then(|v| v.as_str());
            if let Some(full) = full
                && let Some(w) = store
                    .lookup(full)
                    .and_then(|p| p.max_input_tokens)
                    .filter(|w| *w > 0)
            {
                return (Some(w as usize), "catalog");
            }
        }
    }
    (None, "fallback-128k")
}

/// ⑩ Stuck limit: after this many consecutive ineffective summarizations,
/// pause auto-summarization and warn.
const COMPACT_STUCK_LIMIT: u32 = 2;

/// P5：token 预算尾巴边界。从尾部按 MODEL-FACING 投影逐条回退累积，
/// 返回最小的 `start` 使 `history[start..]` 的估算 token ≤ budget——
/// 即逐字尾巴最多吃掉 `budget_tokens`。整段最后一条单独超预算时仍保留
/// 该条（尾巴至少含最近一条，与 pi keepRecentTokens 的语义一致）。
/// 纯 user/assistant 前缀下保证 ≤ 预算；随后的 tool_safe_boundary 回退
/// （永不切在 tool 对中间）可再轻微超出——那是正确性换预算的既定取舍。
pub(crate) fn token_budget_boundary(
    history: &[crate::types::ConversationTurn],
    budget_tokens: usize,
) -> usize {
    let mut acc = 0usize;
    let mut start = history.len();
    while start > 0 {
        let t = estimate_tokens_for_turns_projected(&history[start - 1..start]);
        // 首条（最后一条消息）无条件保留：`start < history.len()` 才 break。
        if acc + t > budget_tokens && start < history.len() {
            break;
        }
        acc += t;
        start -= 1;
    }
    start
}

/// P5 前的旧尾巴语义，现降级为回退路径：`compact_keep_recent_tokens = 0`
/// （显式配置）时 `maybe_update_summary` 的逐字尾巴仍按「保留近
/// [`K_TARGET`] 条」计算。默认路径（非 0）按 token 预算
/// （[`AgentLoop::current_compact_keep_recent_tokens`]）定界，见
/// [`token_budget_boundary`]。
pub(crate) const K_TARGET: usize = 6;

/// Aggressive verbatim-tail size used by `force_compression` (the last-resort
/// retry path on LLM context errors). Smaller than `K_TARGET` because the
/// situation is already an emergency — prefer a larger summarized prefix over
/// failing the request. A second compression folds everything into the summary
/// (tail → 0).
pub(crate) const SMALL_K_FORCE: usize = 2;

/// Session-keyed state maps on `AgentLoop` (`compact_state`, `summarizing`) are
/// size-bounded: when one exceeds this many entries we clear it wholesale. These
/// maps are best-effort — losing an entry just re-learns the state on the next
/// relevant call — and a wholesale clear under size pressure avoids the
/// unbounded-growth anti-pattern (one entry per session, never evicted, leaking
/// for the whole bot-process lifetime).
pub(crate) const SESSION_STATE_MAX_ENTRIES: usize = 512;

/// ⑩ Pure predicate: did a summarization fail to meaningfully reduce the
/// prompt? True when there was a prior summarization (`last_summary_tokens > 0`)
/// AND the current prompt is still at least [`COMPACT_STUCK_PLATEAU_RATIO`]% of
/// the pre-summarization size. Extracted so the threshold logic is unit-testable.
pub(crate) fn summarize_was_ineffective(last_summary_tokens: usize, current_tokens: usize) -> bool {
    last_summary_tokens > 0
        && current_tokens >= last_summary_tokens * COMPACT_STUCK_PLATEAU_RATIO / 100
}

/// Voice-playback suffix appended to the last user message when voice mode is
/// on (transient — never persisted to instance history / session_log).
/// T8 (U9 ②): hoisted from an inline literal so the replay ledger records the
/// exact same bytes the provider saw (single source of truth).
pub(crate) const VOICE_PLAYBACK_SUFFIX: &str = "（语音播报模式已开启，请用简洁、便于口语播报的方式回复，避免使用代码块、表格等不适合语音的内容。）";

/// Fold the summary cache over the history + enforce tool-pair consistency —
/// the persisted-derived projection every main-loop LLM request starts from.
///
/// T8 (U9 ②): extracted from `build_messages_with_memory` so the replay
/// ledger (`crate::replay`) rebuilds requests through the SAME code path —
/// production build and audit replay cannot drift apart (single source of
/// truth; same lesson as the T7 memory-tool schema-drift fix).
///
/// `summary` is `(text, covers_up_to)`; `None` sends the history verbatim.
/// `covers_up_to` indexes the full history vector including the system prompt
/// at index 0. The system prompt is never summarized — it is rebuilt as the
/// leading system message with the summary appended (so the cached prefix
/// stays stable between summary updates).
pub(crate) fn project_history_for_request(
    history: &[crate::types::ConversationTurn],
    summary: Option<(&str, usize)>,
) -> Vec<crate::types::ConversationTurn> {
    let mut turns: Vec<crate::types::ConversationTurn> = if let Some((text, covers_up_to)) = summary
    {
        let c_idx = covers_up_to.min(history.len());
        // Verbatim tail starts at c_idx but never re-includes the system
        // prompt at index 0 (it is rebuilt as the leading message below).
        let tail_start = c_idx.max(1).min(history.len());
        let summary_block = format!("\n\n## Summary of Previous Conversation\n\n{}", text);

        let mut out: Vec<crate::types::ConversationTurn> =
            Vec::with_capacity(history.len() - tail_start + 1);
        if history.first().is_some_and(|t| t.role == "system") {
            // Merge the summary into the configured system prompt (history[0]).
            let mut sys = history[0].clone();
            sys.content.push_str(&summary_block);
            out.push(sys);
        } else {
            // No system prompt at history[0]: emit the summary as a
            // dedicated leading system turn so the provider still sees it.
            out.push(crate::types::ConversationTurn {
                role: "system".to_string(),
                content: summary_block,
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: chrono::Local::now().to_rfc3339(),
                reasoning_content: None,
                tool_name: None,
                tool_result_projection: None,
                image_refs: Vec::new(),
            });
        }
        out.extend(history[tail_start..].iter().cloned());
        out
    } else {
        history.to_vec()
    };

    // X1 (U3 projection prune): tool results fold to their bounded
    // model-facing form HERE — history keeps the originals (recoverable
    // mid-sections, branchable history), the provider never sees an
    // oversized tool result. Recorded override wins (spill locator / guard
    // nudges — not recomputable); else the pure prune recompute. Idempotent:
    // prune output stays under the inline threshold, so old sessions whose
    // tool content is already pruned pass through byte-untouched. Because
    // replay rebuilds through this same function, the fold automatically
    // applies to audit replay too (no injection-ledger entry needed — the
    // transform is a pure function of the history state).
    for turn in &mut turns {
        if turn.role == "tool" {
            turn.content = turn.model_facing_content().into_owned();
        }
    }

    // Enforce tool-pair consistency at the LLM boundary. Upstream paths
    // (summarization, session save/load) can leave an assistant tool_call
    // whose result was dropped — or vice versa — and providers then reject
    // the whole request with 400 "insufficient tool messages following
    // tool_calls". Every main-loop LLM call's messages flow through here,
    // so repairing this local copy is the universal guarantee: the
    // provider never sees an inconsistent sequence, regardless of which
    // upstream path produced the history. (Non-destructive: the instance's
    // own history is untouched; only the outgoing view is cleaned.)
    crate::types::repair_tool_message_pairs(&mut turns);
    turns
}

impl AgentLoop {
    /// Advance the summary cache if the verbatim tail is over the context
    /// threshold.
    ///
    /// Inline-summarization pipeline (replaces Go's `maybeSummarize`). The
    /// summary cache covers `history[..covers_up_to]`; `build_messages` sends
    /// `history[covers_up_to..]` verbatim. Token pressure is therefore on the
    /// *tail* (what the LLM actually receives), so the threshold is evaluated
    /// against the tail, not the full history.
    ///
    /// P5（能力扩展 WS2）：尾巴边界从「条数 K_TARGET=6」改为「token 预算」
    /// （`agents.defaults.compact_keep_recent_tokens`，默认 20000；0 = 旧按
    /// 条数回退路径）。预算内从尾部逐条回退定界（[`token_budget_boundary`]），
    /// 再经 [`tool_safe_boundary`] 回退保证永不切在 tool_call/result 对中间。
    /// 压缩条件 = 尾巴超阈值 **且** `new_c > c`——后者保证摘要永远前进、
    /// 绝不重摘同一段（旧 `tail_len > K_TARGET` 的等价改写，并顺带封住
    /// tool 回退把边界顶回 c 之内的理论倒退）。History is never mutated
    /// (append-only); bounding is the session store's job.
    ///
    /// Persistence: this updates the in-memory cache on the instance. The save
    /// path persists the cache alongside the full history (see S3.3).
    pub(crate) async fn maybe_update_summary(
        &self,
        instance: &AgentInstance,
        session_key: &str,
        channel: &str,
        chat_id: &str,
    ) {
        let history = instance.get_history();
        // U16 (sixth batch) + N1 (devtool-upgrade 阶段 1): prefer the active
        // model's context_window via the three-tier chain (config explicit →
        // pricing catalog → instance default, now 128_000) over the
        // historical 32000. Falls back to the instance value when
        // unset/standalone.
        let context_window = self
            .current_context_window()
            .unwrap_or_else(|| instance.context_window());

        // C indexes the full history (system prompt at index 0); the verbatim
        // tail build_messages sends is history[C..]. Clamp to history length
        // for safety against a stale cache index.
        let cache = instance.get_summary_cache();
        let c = cache
            .as_ref()
            .map(|c| c.covers_up_to)
            .filter(|&c| c >= 1)
            .unwrap_or(0)
            .min(history.len());
        let existing_summary = cache.as_ref().map(|c| c.text.as_str()).unwrap_or("");

        // Token pressure is on the tail (system + summary + history[C..] is
        // what build_messages emits). The covered prefix is already folded into
        // the summary, so it does not count toward pressure. X1: measured over
        // the MODEL-FACING projection — history keeps tool originals since the
        // size gates moved to build_messages, so the raw estimate would count
        // a 70KB original the provider only ever sees as a bounded locator.
        let tail_tokens = estimate_tokens_for_turns_projected(&history[c..]);
        let soft = context_window * COMPACT_SOFT_RATIO / 100;
        let threshold = context_window * COMPACT_SUMMARIZE_RATIO / 100;

        // P5: new boundary — token budget (default) or legacy count fallback
        // (0), then back off any tool_call/result pair straddling it.
        // Summarize runs only when the tail is over threshold AND the new
        // boundary actually advances past C. A short tail that is huge in
        // tokens (a large system prompt or an early oversized tool result that
        // still fits the budget) can't be helped by advancing C — leave it to
        // force_compression.
        let keep_tokens = self.current_compact_keep_recent_tokens();
        let raw_new_c = if keep_tokens > 0 {
            token_budget_boundary(&history, keep_tokens)
        } else {
            history.len().saturating_sub(K_TARGET)
        };
        let new_c = tool_safe_boundary(&history, raw_new_c);
        let will_summarize = tail_tokens >= threshold && new_c > c;

        // ⑩ Graded tiers (soft / summarize) + stuck self-check on the tail.
        // Soft is info-log-only. The stuck counter only ticks when summarize
        // will ACTUALLY run — otherwise a chronically-over-threshold tail that
        // is too short to summarize (e.g. a big system prompt with few turns)
        // would tick the counter without ever attempting a summarize and pause
        // summarization before it gets the chance.
        let mut paused_stuck = false;
        {
            let mut states = self.compact_state.lock();
            if states.len() > SESSION_STATE_MAX_ENTRIES {
                states.clear();
            }
            let st = states.entry(session_key.to_string()).or_default();

            if tail_tokens >= soft && !st.soft_noticed {
                st.soft_noticed = true;
                info!(
                    "[AgentLoop] context tail at ~{}% of window ({} / {}); summarization will trigger at {}%",
                    tail_tokens * 100 / context_window.max(1),
                    tail_tokens,
                    context_window,
                    COMPACT_SUMMARIZE_RATIO
                );
            }

            if will_summarize {
                if summarize_was_ineffective(st.last_summary_tokens, tail_tokens) {
                    st.consecutive_failures += 1;
                } else {
                    st.consecutive_failures = 0;
                }
                if st.consecutive_failures >= COMPACT_STUCK_LIMIT {
                    if !st.stuck {
                        warn!(
                            "[AgentLoop] compaction stuck: summarization has not reduced the tail {} times in a row; pausing auto-summarization (raise context_window or reduce tool output)",
                            st.consecutive_failures
                        );
                        st.stuck = true;
                    }
                    paused_stuck = true;
                } else {
                    st.last_summary_tokens = tail_tokens;
                }
            } else if tail_tokens < threshold {
                // Breathing room — clear the stuck latch.
                st.consecutive_failures = 0;
                st.stuck = false;
            }
        }
        if paused_stuck {
            return;
        }

        if !will_summarize {
            return;
        }

        let summarize_key = format!("main:{}", session_key);
        {
            let mut map = self.summarizing.lock();
            if map.len() > SESSION_STATE_MAX_ENTRIES {
                map.clear();
            }
            if map.contains_key(&summarize_key) {
                return;
            }
            map.insert(summarize_key.clone(), true);
        }

        // New boundary already computed above (P5 token budget, or the
        // legacy K_TARGET count fallback): cover everything except the
        // budgeted verbatim tail kept for continuity. new_c > c is guaranteed
        // by the will_summarize check above, so we always advance and never
        // re-summarize the same prefix; tool_safe_boundary already backed the
        // boundary off any mid tool_call/result pair.

        let provider = self.provider.read().clone();
        let model = self.active_model.read().clone();
        let outbound_tx = self.outbound_tx.clone();
        let summarizing_flag = self.summarizing.clone();
        let observer_mgr = self.observer_manager.clone();
        let channel_owned = channel.to_string();
        let chat_id_owned = chat_id.to_string();
        let clear_key = summarize_key.clone();

        if !is_internal_channel(&channel_owned)
            && let Some(ref tx) = outbound_tx
        {
            let outbound = nemesis_types::channel::OutboundMessage {
                channel: channel_owned.clone(),
                chat_id: chat_id_owned.clone(),
                content: "Memory threshold reached. Optimizing conversation history...".to_string(),
                message_type: String::new(),
                meta: nemesis_types::channel::OutboundMeta {
                    model: None,
                    session_key: Some(clear_key.clone()),
                    source_node: None,
                },
            };
            let _ = tx.send(outbound).await;
        }

        // 方言 PreCompact（观察型，2026-08-29 三段化扩展）：exit 2 不阻止压缩
        // （稳定性机制）。先 clone Arc 再 await（不持锁跨 await）。
        let bridge_pre = self.cc_bridge.read().as_ref().cloned();
        if let Some(bridge) = bridge_pre {
            bridge.run_compact_hooks("auto", "pre").await;
        }

        // Fold the prefix history[..new_c] into the summary, merged with the
        // existing summary (which already covers history[..c]). summarize the
        // FULL prefix from source each time (no "keep last N" — that would
        // leave a gap between the summary and the verbatim tail).
        // P6：existing_summary 非空时走 UPDATE 迭代修订指令（见
        // build_summary_instruction）；P7：文件操作台账由 summarize_prefix_owned
        // 内部从同一前缀聚合注入。
        let prefix_refs: Vec<&crate::types::ConversationTurn> = history[..new_c].iter().collect();
        let summary = summarize_prefix_owned(
            &prefix_refs,
            existing_summary,
            new_c.saturating_sub(c),
            context_window,
            self.current_summarizer_prefix_reuse(),
            provider.as_ref(),
            &model,
            observer_mgr,
        )
        .await;

        if let Some(summary) = summary {
            instance.set_summary_cache(Some(crate::instance::SummaryCache {
                covers_up_to: new_c,
                text: summary,
            }));
        } else {
            // 2026-08-25 摘要静默失败修复：失败（或无可摘要内容）时绝不推进
            // covers —— 否则被折叠的上下文会静默丢失。Loud warn，不静默。
            warn!(
                "[AgentLoop] auto-summarization produced no summary for {} (LLM failure or no valid content); keeping full history, covers_up_to unchanged",
                session_key
            );
        }

        // 方言 PostCompact（观察型）：压缩尝试结束（成败皆触发）。
        let bridge_post = self.cc_bridge.read().as_ref().cloned();
        if let Some(bridge) = bridge_post {
            bridge.run_compact_hooks("auto", "post").await;
        }

        {
            let mut map = summarizing_flag.lock();
            map.remove(&clear_key);
        }
    }

    /// P5（能力扩展 WS2）：`agents.defaults.compact_keep_recent_tokens`
    /// （默认 20000，对齐 pi keepRecentTokens；**0 = 回退旧按条数
    /// K_TARGET 路径**）。fresh-read（同 `current_summarizer_prefix_reuse`
    /// 模式——config.json 唯一真相源，运行中改键下一轮生效）；standalone
    /// （无 config_path）→ 默认值。键路径与缺省值的单一真相源在
    /// `nemesis_config::resolve_compact_keep_recent_tokens`（typed
    /// `AgentDefaults` 字段同源）。
    pub(crate) fn current_compact_keep_recent_tokens(&self) -> usize {
        let cfg = self
            .config_path
            .read()
            .clone()
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        nemesis_config::resolve_compact_keep_recent_tokens(cfg.as_ref())
    }

    /// Force-compress by aggressively advancing the summary cache.
    ///
    /// Last-resort path used when the LLM reports a context error and the
    /// caller retries. Does NOT mutate history (history is append-only); it
    /// shrinks what `build_messages` emits by advancing `covers_up_to` and
    /// recomputing the summary over the larger covered prefix.
    ///
    /// Progressive: each call shrinks the verbatim tail further. The first call
    /// reduces the tail to [`SMALL_K_FORCE`] messages; if that still isn't
    /// enough (the caller will retry), the next call folds everything into the
    /// summary (tail → 0). Bounded by the caller's retry limit.
    pub async fn force_compression(&self, instance: &AgentInstance) {
        self.force_compression_opts(instance, false).await;
    }

    /// N2：`force_compression` 的参数化形态——`prefer_small=true` 时手动
    /// 入口（E6 `/compact`）优先用 `agents.small_model` 跑摘要；自动压缩
    /// 路径维持 `false`（主模型，质量敏感不降档）。
    ///
    /// 返回 `true` = 已发起摘要生成尝试；`false` = 无可推进内容（未触达 LLM）。
    pub async fn force_compression_opts(
        &self,
        instance: &AgentInstance,
        prefer_small: bool,
    ) -> bool {
        let history = instance.get_history();
        let cache = instance.get_summary_cache();
        let current_c = cache
            .as_ref()
            .map(|c| c.covers_up_to)
            .filter(|&c| c >= 1)
            .unwrap_or(0)
            .min(history.len());
        let existing_summary = cache.as_ref().map(|c| c.text.as_str()).unwrap_or("");

        // Shrink the verbatim tail: to SMALL_K_FORCE if it's still large,
        // otherwise cover everything (tail → 0) as the final resort.
        let current_tail_len = history.len().saturating_sub(current_c);
        let raw_c = if current_tail_len > SMALL_K_FORCE {
            history.len() - SMALL_K_FORCE
        } else {
            history.len()
        };
        // Keep tool_call/result pairs intact in the tail (don't let the boundary
        // drop an orphan result that the summary can't capture).
        let new_c = tool_safe_boundary(&history, raw_c);
        // Must advance and have a non-empty prefix to summarize.
        // 返回值区分两臂（2026-09-28 真模型验证发现）：false = 无可推进内容
        // （tail 已到 SMALL_K_FORCE 或 tool_safe_boundary 无法再前移），
        // true = 已进入摘要生成尝试。调用方（compact_session）据此把
        // 「无需压缩」与「摘要失败」分开上报，不再统一误报 LLM 失败。
        if new_c <= current_c || new_c == 0 {
            return false;
        }

        let (provider, model) = self.resolve_summary_provider(prefer_small);
        let observer_mgr = self.observer_manager.clone();

        // Fold the prefix history[..new_c] into the summary, merged with the
        // existing summary (which covers history[..current_c]).
        // P6/P7：同自动路径——UPDATE 迭代指令 + 文件操作台账注入。
        let prefix_refs: Vec<&crate::types::ConversationTurn> = history[..new_c].iter().collect();
        let summary = summarize_prefix_owned(
            &prefix_refs,
            existing_summary,
            new_c.saturating_sub(current_c),
            // U16: per-model context_window when declared (same preference
            // order as the threshold computation above).
            self.current_context_window()
                .unwrap_or_else(|| instance.context_window()),
            // T4: per-model prefix-reuse switch (false → old bare shape).
            self.current_summarizer_prefix_reuse(),
            provider.as_ref(),
            &model,
            observer_mgr,
        )
        .await;

        if let Some(summary) = summary {
            instance.set_summary_cache(Some(crate::instance::SummaryCache {
                covers_up_to: new_c,
                text: summary,
            }));
            info!(
                "[AgentLoop] Force-compressed: covers_up_to {} -> {} (verbatim tail {} -> {} messages)",
                current_c,
                new_c,
                current_tail_len,
                history.len() - new_c
            );
        } else {
            // 2026-08-25 摘要静默失败修复：force 路径同样绝不推进 covers。
            // 调用方的有界重试会放弃并向上报错（响亮失败优于静默失忆）。
            warn!(
                "[AgentLoop] force-compression produced no summary (LLM failure or no valid content); covers_up_to stays {}, the caller's bounded retry will surface the error",
                current_c
            );
        }
        true
    }

    // -----------------------------------------------------------------------
    // E6: manual session maintenance (/compact /clear)
    // -----------------------------------------------------------------------

    /// E6: 手动 compaction 入口。取（从 store 重建的）instance，调
    /// `force_compression` 推进摘要覆盖，再把新摘要持久化回 store（顺序同
    /// 回合末：先 summary+covers 后 history，见回合末块注释）。
    ///
    /// 成功回执带覆盖数（摘要覆盖前 N 条，保留近 M 条）；摘要 LLM 失败时返回
    /// Err（covers 不推进——2026-08-25 静默失忆修复的同一契约），历史保持不变。
    /// 无可推进内容（tail 已最简 / 边界无法前移）返回 Ok 诚实回执——2026-09-28
    /// 真模型验证发现此前该情形误报「LLM 调用失败」（根本没发起调用）。
    pub async fn compact_session(&self, session_key: &str) -> Result<String, String> {
        let instance = self.get_or_create_instance(session_key);
        let history_len = instance.get_history().len();
        if history_len == 0 {
            return Err("会话为空，无需压缩".to_string());
        }
        let before = instance
            .get_summary_cache()
            .map(|c| c.covers_up_to)
            .unwrap_or(0);

        // 方言 PreCompact（观察型）：手动压缩尝试开始。
        let bridge_pre = self.cc_bridge.read().as_ref().cloned();
        if let Some(bridge) = bridge_pre {
            bridge.run_compact_hooks("manual", "pre").await;
        }

        // N2：手动 /compact 是 `agents.small_model` 的唯一消费点——摘要用小
        // 省钱；自动压缩路径（context_length_exceeded 等）维持主模型。
        let attempted = self.force_compression_opts(&instance, true).await;

        // 方言 PostCompact（观察型）：压缩尝试结束（成败皆触发），与 auto 路径对称。
        let bridge_post = self.cc_bridge.read().as_ref().cloned();
        if let Some(bridge) = bridge_post {
            bridge.run_compact_hooks("manual", "post").await;
        }

        let Some(cache) = instance
            .get_summary_cache()
            .filter(|c| c.covers_up_to > before && !c.text.is_empty())
        else {
            if !attempted {
                // 无可推进内容 = 健康态（tail 已最简），不是失败——诚实告知，
                // 不冒充「LLM 调用失败」（2026-09-28 真模型验证发现的误报臂）。
                return Ok("✓ 无需压缩：会话历史已处于最简状态".to_string());
            }
            return Err("摘要生成失败（LLM 调用失败或无有效内容），历史保持不变".to_string());
        };

        // Persist: summary + covers BEFORE history（set_history 的
        // trim_to_limit 依赖 covers 判定哪些最旧消息可落盘丢弃，顺序错了会
        // 相互踩——同回合末持久化块的既有纪律）。
        if let Some(ref store) = self.session_store {
            store.get_or_create(session_key);
            store.set_summary(session_key, &cache.text);
            store.set_summary_covers_up_to(session_key, Some(cache.covers_up_to));
            let stored: Vec<crate::session::StoredMessage> = instance
                .get_history()
                .iter()
                .map(crate::session::StoredMessage::from)
                .collect();
            store.set_history(session_key, stored);
            if let Err(e) = store.save(session_key) {
                warn!(
                    "[AgentLoop] Failed to persist compacted session {}: {}",
                    session_key, e
                );
            }
        }

        let kept = history_len - cache.covers_up_to;
        info!(
            "[AgentLoop] Manual compact for {}: summary covers {} msgs, verbatim tail {} msgs",
            session_key, cache.covers_up_to, kept
        );
        Ok(format!(
            "✓ 已压缩：摘要覆盖前 {} 条，保留近 {} 条",
            cache.covers_up_to, kept
        ))
    }

    /// M5（devtool-upgrade 阶段 3）：会话级 context 占用快照（只读）。
    ///
    /// 口径与 `maybe_update_summary` 的压缩压力测量**同一公式**：
    /// `used` = 摘要覆盖点之后的逐字尾部（build_messages 实际发给 LLM 的
    /// 部分）按 MODEL-FACING 投影估算的 token 数；`window` = 三级解析链
    /// （config `context_window` → 价目表 → 实例默认）给出的窗口。
    /// 这样 Dashboard 显示的百分比就是「下一轮真实占用」而不是一个
    /// 另一套口径的近似值。
    ///
    /// 实例按回合从 store 重建（同 [`Self::compact_session`] 的读路径），
    /// 无驻留状态；store 未挂（standalone 简测）时按空历史计。
    pub fn session_context_status(&self, session_key: &str) -> serde_json::Value {
        let instance = self.get_or_create_instance(session_key);
        let history = instance.get_history();
        let window = self
            .current_context_window()
            .unwrap_or_else(|| instance.context_window());
        let cache = instance.get_summary_cache();
        let covers = cache
            .as_ref()
            .map(|c| c.covers_up_to)
            .filter(|&c| c >= 1)
            .unwrap_or(0)
            .min(history.len());
        let used = estimate_tokens_for_turns_projected(&history[covers..]);
        let pct = (used * 100).checked_div(window).unwrap_or(0).min(100);
        serde_json::json!({
            "session_key": session_key,
            "used_tokens": used,
            "window": window,
            "pct": pct,
            "history_len": history.len(),
            "covers_up_to": covers,
            "summarized": cache.is_some(),
        })
    }

    // -----------------------------------------------------------------------
    // Internal agent loop execution
    // -----------------------------------------------------------------------
}

// ---------------------------------------------------------------------------
// 自由函数归位（P1-c 自 loop.rs 根搬迁；仅增 pub(crate) 可见性标注）
// ---------------------------------------------------------------------------

// P6/P7 文本真相已上收 nemesis-prompts（aux 模块，2026-09-28 真源归一）：
// 六节 schema 与文件台账标题在此 re-export——本 crate 内既有 `super::*`
// 消费方（branch_summary、ws2 测试）路径不变。
pub(crate) use crate::prompt::{FILE_LEDGER_HEADING, SUMMARY_SCHEMA_SECTIONS};

/// P7：台账条目上限。长会话的文件操作无界膨胀会反噬摘要本身；超限保
/// 首次出现顺序截断，并聚合一行诚实注记。
pub(crate) const FILE_LEDGER_MAX_ENTRIES: usize = 50;

/// P7：文件操作台账条目（宿主从覆盖段 assistant tool_calls 聚合，见
/// [`collect_file_ops_ledger`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileOp {
    pub path: String,
    /// "write" | "edit" | "delete"（按工具名映射；目录工具不进账）。
    pub kind: &'static str,
}

/// P6：一次摘要请求的迭代上下文。
///
/// - `existing` 为空 = 首次全量摘要；非空 = UPDATE 迭代修订（指令层告知
///   模型「在旧摘要基础上修订」而非全量重生成）。注意：请求消息体保持
///   G1 前缀形状不变（system + 原样覆盖段 + 尾部指令），UPDATE 只改尾部
///   指令文本——指令是最后一条消息、不在 warm 前缀内，前缀字节不变
///   纪律不破（这是 P6 的硬约束，见总控 §WS2）。
/// - `new_segment_turns`：旧摘要覆盖点之后的新增消息轮数（new_c - c），
///   UPDATE 指令据此告知模型「前缀末尾约 N 条是新增内容」。
/// - `ledger`：P7 宿主文件操作台账（空 = 覆盖段无文件操作）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SummaryUpdate<'a> {
    pub existing: &'a str,
    pub new_segment_turns: usize,
    pub ledger: &'a [FileOp],
}

/// P6：构建摘要请求的尾部指令（batch / bare-concat / multipart 合并共用
/// 的单一真相源——三种请求形态的摘要语义必须一致）。
pub(crate) fn build_summary_instruction(update: &SummaryUpdate<'_>) -> String {
    let ledger_lines: Vec<String> = update
        .ledger
        .iter()
        .map(|op| format!("- [{}] {}", op.kind, op.path))
        .collect();
    crate::prompt::render_summary_instruction(
        if update.existing.is_empty() {
            None
        } else {
            Some(update.existing)
        },
        update.new_segment_turns,
        &ledger_lines,
    )
}

/// P6：schema 解析。六节标题齐 → `Some(归一化文本)`（从首个 schema 节
/// 标题行起截，剥掉模型客套前导）；任一缺失 → `None`（调用方回退自由
/// 文本原样使用——解析失败绝不允许丢摘要或报错）。
pub(crate) fn parse_structured_summary(reply: &str) -> Option<String> {
    let headings: Vec<&str> = reply.lines().filter_map(line_heading).collect();
    let all_present = SUMMARY_SCHEMA_SECTIONS
        .iter()
        .all(|s| headings.iter().any(|h| h == s));
    if !all_present {
        return None;
    }
    let start = reply
        .lines()
        .position(|l| line_heading(l).is_some_and(|h| SUMMARY_SCHEMA_SECTIONS.contains(&h)))
        .unwrap_or(0);
    let cut = reply.lines().skip(start).collect::<Vec<_>>().join("\n");
    let cut = cut.trim_end();
    (!cut.is_empty()).then(|| cut.to_string())
}

/// 单行 Markdown 标题词（`#`/`##`/`###` 前缀，剥后去空白；空标题 → None）。
fn line_heading(line: &str) -> Option<&str> {
    let t = line.trim();
    let rest = t
        .strip_prefix("###")
        .or_else(|| t.strip_prefix("##"))
        .or_else(|| t.strip_prefix("#"))?;
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

/// P7：从覆盖段 assistant tool_calls 聚合文件操作台账。
///
/// 纯历史驱动：D3 的 turn_file_changes 收集桶在 assistant 回复落盘时已
/// drain（写 chat_log jsonl），压缩时不可用；这里从 assistant 轮的
/// tool_calls args 直接解析——与 D3 preview_all 同信息源（声明式文件
/// 工具的 path 参数），但无需工具注册表/文件系统，compact 域内自洽可单测。
/// exec 等非声明式写盘不在账（诚实边界：与 D3 消息级映射同口径）。
///
/// 累积性：每次压缩都对完整前缀 history[..new_c] 重算，旧摘要已覆盖段
/// 的操作天然包含 → 台账随迭代更新单调累积。同 path 后声明 kind 覆盖、
/// 首次出现顺序保留（镜像 `chat_log::dedup_file_changes` 投影语义）。
pub(crate) fn collect_file_ops_ledger(messages: &[&crate::types::ConversationTurn]) -> Vec<FileOp> {
    fn push(
        out: &mut Vec<FileOp>,
        index: &mut std::collections::HashMap<String, usize>,
        path: String,
        kind: &'static str,
    ) {
        if path.is_empty() {
            return;
        }
        match index.get(&path) {
            Some(&i) => out[i].kind = kind,
            None => {
                index.insert(path.clone(), out.len());
                out.push(FileOp { path, kind });
            }
        }
    }
    fn arg_path(args: &serde_json::Value) -> Option<String> {
        args.get("path").and_then(|v| v.as_str()).map(String::from)
    }

    let mut out: Vec<FileOp> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for m in messages {
        if m.role != "assistant" {
            continue;
        }
        for tc in &m.tool_calls {
            let Ok(args) = serde_json::from_str::<serde_json::Value>(&tc.arguments) else {
                continue;
            };
            match tc.name.as_str() {
                "write_file" => {
                    if let Some(p) = arg_path(&args) {
                        push(&mut out, &mut index, p, "write");
                    }
                }
                "edit_file" | "append_file" => {
                    if let Some(p) = arg_path(&args) {
                        push(&mut out, &mut index, p, "edit");
                    }
                }
                "delete_file" => {
                    if let Some(p) = arg_path(&args) {
                        push(&mut out, &mut index, p, "delete");
                    }
                }
                "multiedit" => {
                    if let Some(edits) = args.get("edits").and_then(|v| v.as_array()) {
                        for e in edits {
                            if let Some(p) = arg_path(e) {
                                push(&mut out, &mut index, p, "edit");
                            }
                        }
                    }
                }
                // 其余工具（read/exec/git/...）不进台账。
                _ => {}
            }
        }
    }
    out
}

/// P7：台账节渲染。空台账 → None；超 [`FILE_LEDGER_MAX_ENTRIES`] 截断 +
/// 注记。
pub(crate) fn format_file_ledger_section(ledger: &[FileOp]) -> Option<String> {
    if ledger.is_empty() {
        return None;
    }
    let mut out = String::from(FILE_LEDGER_HEADING);
    out.push('\n');
    let shown = ledger.len().min(FILE_LEDGER_MAX_ENTRIES);
    for op in &ledger[..shown] {
        out.push_str(&format!("- [{}] {}\n", op.kind, op.path));
    }
    if ledger.len() > FILE_LEDGER_MAX_ENTRIES {
        out.push_str(&format!(
            "- （另有 {} 个文件操作未逐一列出）\n",
            ledger.len() - FILE_LEDGER_MAX_ENTRIES
        ));
    }
    Some(out.trim_end().to_string())
}

/// P6+P7 收尾：schema 解析（失败回退自由文本原样，绝不炸）+ 超宽消息
/// 省略注记 + 台账节宿主确定性追加（「压缩后摘要含台账节」不依赖模型
/// 自觉；摘要已含该标题则不重复）。空回复原样返回空——调用方的
/// `filter(|s| !s.is_empty())` 仍把它折叠回 None（2026-08-25 契约不变）。
pub(crate) fn finalize_summary_text(reply: &str, ledger: &[FileOp], omitted: bool) -> String {
    let mut text = parse_structured_summary(reply).unwrap_or_else(|| reply.trim().to_string());
    if omitted && !text.is_empty() {
        text.push_str(
            "\n[Note: Some oversized messages were omitted from this summary for efficiency.]",
        );
    }
    if !ledger.is_empty()
        && !text.contains(FILE_LEDGER_HEADING)
        && let Some(section) = format_file_ledger_section(ledger)
    {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&section);
    }
    text
}

/// T4 (U1): pre-G1 summary shape, restored as the per-model fallback
/// (`summarizer_prefix_reuse: false`).
///
/// This is the OLD request form the G1 refactor replaced: a single bare user
/// message whose content is the covered messages flattened as
/// `role: content` text lines, plus the instruction (and any existing-summary
/// context). It shares NO prefix with real requests and destroys structure
/// (tool_calls flatten to text) — which is exactly why it is NOT the default.
/// It remains useful for cheap summarizer models whose warm-KV-prefix
/// assumption G1 relies on does not hold (different tokenizer, no prompt
/// caching): a shape-neutral single message is the lowest-common-denominator
/// request those models handle reliably. The G1 prefix-reuse path
/// (summarize_multipart_owned / summarize_batch_owned) stays the default for
/// the main model; this function is invoked ONLY when the per-model switch
/// opts out.
/// Returns `Some(summary)` on a non-empty LLM reply; `None` on LLM failure or
/// empty reply (failure must propagate — never fold history behind a failed
/// summary; see the 2026-08-25 fix note on `summarize_prefix_owned`).
pub(crate) async fn summarize_bare_concat_owned(
    messages: &[&crate::types::ConversationTurn],
    update: &SummaryUpdate<'_>,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mut content = String::new();
    for m in messages {
        content.push_str(&format!("{}: {}\n", m.role, m.content));
    }
    // P6：结构化 schema 指令（UPDATE 语义/旧摘要/台账都在指令里——旧摘要
    // 不再前置拼接，单源 [`build_summary_instruction`]）。
    content.push_str(&build_summary_instruction(update));

    let llm_messages = vec![LlmMessage {
        role: "user".to_string(),
        content,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }];

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-bare-concat",
        model,
        || {
            let llm_messages = llm_messages.clone();
            async move {
                provider
                    .chat(
                        model,
                        llm_messages,
                        Some(aux_chat_options(AUX_SUMMARY_MAX_TOKENS)),
                        vec![],
                    )
                    .await
            }
        },
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => None,
        Some(Err(e)) => {
            warn!(
                "[AgentLoop] summarize_bare_concat_owned LLM call failed (summary NOT produced, history stays unfolded): {}",
                e
            );
            None
        }
        None => None,
    }
}

/// Multi-part summarization (standalone, works in spawned task).
///
/// G1 (U1): each part is summarized as `[system?, ...part messages,
/// instruction]` — a part is a contiguous slice of the covered prefix, so its
/// message list is a true ordered prefix subset of the main request's history
/// (prefix-cache friendly in the same way).
///
/// Returns `Some` only when BOTH parts produced real summaries. If either
/// part's LLM call fails → `None` (whole summarization fails; the caller must
/// keep the history unfolded — a half-empty merge prompt makes the merge model
/// answer with a "you didn't paste the summaries" complaint, and storing that
/// complaint as the summary silently amnesiates the covered prefix; this is
/// the exact production failure found in the legacy session 2026-08-25).
pub(crate) async fn summarize_multipart_owned(
    system_msg: Option<&LlmMessage>,
    messages: &[&crate::types::ConversationTurn],
    update: &SummaryUpdate<'_>,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mid = messages.len() / 2;
    let part1 = &messages[..mid];
    let part2 = &messages[mid..];

    // P6 UPDATE 语义挂在 part2（较新的一半——新增段尾部必在其中，指令里
    // 的「末尾约 N 条」按 part2 长度钳位）；part1 保持 fresh 常规摘要。
    // 台账两半都注入：Files 台账必须出现在最终合并结果里，不依赖分片运气。
    // （旧摘要在 part2 已折叠进其摘要文本，merge 只需合并两段。）
    let part1_update = SummaryUpdate {
        existing: "",
        new_segment_turns: 0,
        ledger: update.ledger,
    };
    let part2_update = SummaryUpdate {
        existing: update.existing,
        new_segment_turns: update.new_segment_turns.min(part2.len()),
        ledger: update.ledger,
    };
    let s1 = summarize_batch_owned(
        system_msg,
        part1,
        &part1_update,
        provider,
        model,
        observer_manager.clone(),
    )
    .await;
    let s2 = summarize_batch_owned(
        system_msg,
        part2,
        &part2_update,
        provider,
        model,
        observer_manager.clone(),
    )
    .await;

    let (s1, s2) = match (s1, s2) {
        (Some(a), Some(b)) => (a, b),
        (failed, _) => {
            warn!(
                "[AgentLoop] multipart summarization aborted: one part failed to summarize ({}); summary NOT produced, history stays unfolded",
                if failed.is_none() { "part 1" } else { "part 2" }
            );
            return None;
        }
    };

    // Merge via LLM. P6：合并指令同样要求六节 schema 输出（合并的是两段
    // 结构化摘要，产出必须仍是结构化的）。文本真相在 nemesis-prompts（aux）。
    let merge_prompt = crate::prompt::render_summary_merge_prompt(&s1, &s2);

    let llm_messages = vec![LlmMessage {
        role: "user".to_string(),
        content: merge_prompt,
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }];

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-multipart-merge",
        model,
        || {
            let llm_messages = llm_messages.clone();
            async move {
                provider
                    .chat(
                        model,
                        llm_messages,
                        Some(aux_chat_options(AUX_SUMMARY_MAX_TOKENS)),
                        vec![],
                    )
                    .await
            }
        },
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => {
            // Empty merge reply: both parts are validated non-empty —
            // concatenate them instead of losing the coverage.
            Some(format!("{}\n\n{}", s1, s2))
        }
        _ => {
            // Merge call failed: same fallback — both parts hold real
            // summaries, so concatenation preserves the information (the
            // next compaction round will re-fold them into a merged one).
            warn!(
                "[AgentLoop] multipart merge LLM call failed; falling back to concatenating the two (validated) part summaries"
            );
            Some(format!("{}\n\n{}", s1, s2))
        }
    }
}

/// Single-batch summarization (standalone, works in spawned task).
///
/// G1 (U1): the request is `[system?, ...covered messages (original
/// structure), instruction]` — a genuine prefix of the conversation plus the
/// trailing instruction, replacing the old single bare user message with
/// `role: content` text concatenation.
///
/// Returns `Some` on a non-empty LLM reply; `None` on LLM failure or empty
/// reply (failure propagates — see the 2026-08-25 fix note on
/// `summarize_prefix_owned`).
pub(crate) async fn summarize_batch_owned(
    system_msg: Option<&LlmMessage>,
    batch: &[&crate::types::ConversationTurn],
    update: &SummaryUpdate<'_>,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    let mut messages: Vec<LlmMessage> = Vec::with_capacity(batch.len() + 2);
    if let Some(sys) = system_msg {
        messages.push(sys.clone());
    }
    for m in batch {
        messages.push(conversation_turn_to_llm_message(m));
    }
    // Trailing instruction（P6：结构化 schema + UPDATE/台账上下文，单源
    // build_summary_instruction——prefix 字节不变纪律只覆盖前面的覆盖段，
    // 尾部指令文本可随迭代语义演进）。
    messages.push(LlmMessage {
        role: "user".to_string(),
        content: build_summary_instruction(update),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    });

    let response = emit_observer_events_around_llm(
        observer_manager.as_ref(),
        "summarize-batch",
        model,
        || {
            let messages = messages.clone();
            async move {
                provider
                    .chat(
                        model,
                        messages,
                        Some(aux_chat_options(AUX_SUMMARY_MAX_TOKENS)),
                        vec![],
                    )
                    .await
            }
        },
    )
    .await;

    match response {
        Some(Ok(resp)) if !resp.content.is_empty() => Some(resp.content),
        Some(Ok(_)) => {
            // 2026-09-28 真模型验证：此臂原为静默 None——thinking 系模型把
            // AUX_SUMMARY_MAX_TOKENS 烧在思考上 → text 空 → 摘要失败不可见，
            // 只剩 multipart abort 的一句间接 warn。响亮失败优于静默失忆。
            warn!(
                "[AgentLoop] summarize_batch_owned: LLM returned empty content (thinking-token exhaustion or refusal? budget={} tokens); summary NOT produced, history stays unfolded",
                AUX_SUMMARY_MAX_TOKENS
            );
            None
        }
        Some(Err(e)) => {
            warn!(
                "[AgentLoop] summarize_batch_owned LLM call failed (summary NOT produced, history stays unfolded): {}",
                e
            );
            None
        }
        None => None,
    }
}

/// Emit observer events (ConversationStart, LlmRequest, LlmResponse, ConversationEnd)
/// around a synchronous LLM call closure. Used by standalone summarization functions.
///
/// `make_call` 是工厂闭包（每次调用产出一个新请求 future）：空输出/瞬态失败
/// 由 [`with_one_retry`] 在同一超时窗口内重建请求重试一次（采样抖动兜底；
/// 总墙钟预算不随重试翻倍）。
pub(crate) async fn emit_observer_events_around_llm<F, Fut>(
    observer_manager: Option<&Arc<nemesis_observer::Manager>>,
    label: &str,
    model: &str,
    make_call: F,
) -> Option<Result<LlmResponse, String>>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<LlmResponse, String>>,
{
    use crate::loop_executor::ObserverEvent;

    let trace_id = format!(
        "{}-{}",
        label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );

    // Emit ConversationStart + LlmRequest before the call (await, no block_in_place).
    if let Some(mgr) = observer_manager {
        let start_event = ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: label.to_string(),
            channel: String::new(),
            chat_id: String::new(),
            sender_id: "summarizer".to_string(),
            content: String::new(),
        };
        mgr.emit(start_event.to_conversation_event()).await;

        let request_event = ObserverEvent::LlmRequest {
            trace_id: trace_id.clone(),
            round: 0,
            model: model.to_string(),
            messages: vec![],
            tools: vec![],
            messages_count: 0,
            tools_count: 0,
            provider_name: String::new(),
            api_key: String::new(),
            api_base: String::new(),
        };
        mgr.emit(request_event.to_conversation_event()).await;
    }

    // Execute the LLM call (async, no block_on). 墙钟上限走杂务旁路护栏
    // （bypass_llm::AUX_SUMMARY_TIMEOUT）——慢模型/挂死连接不拖住压缩任务；
    // 超时折叠为 Err，走既有失败语义（摘要不产出、历史保持不折叠）。
    // 空输出/瞬态失败在同一超时窗口内重试一次。**空→Err 归一必须在重试
    // 内层**：with_one_retry 只对 Err 重试，LlmResponse 携带空 content 是
    // 合法 Ok——归一放外层的话，thinking 系模型烧穿预算的空输出（主要失败
    // 形态）永远不触发重试（2026-09-28 复查修正；与 bypass_llm::
    // guarded_llm_call_retrying 同构）。工厂闭包重建请求；总墙钟预算不随
    // 重试翻倍。
    let start = std::time::Instant::now();
    let attempt = || {
        let r = make_call();
        async move {
            match r.await {
                Ok(resp) if resp.content.trim().is_empty() => {
                    warn!("[bypass:{label}] 模型返回空输出，按失败处理");
                    Err(format!("[bypass:{label}] 空输出，按失败处理"))
                }
                other => other,
            }
        }
    };
    let mut response = tokio::time::timeout(AUX_SUMMARY_TIMEOUT, with_one_retry(label, attempt))
        .await
        .unwrap_or_else(|_| {
            Err(format!(
                "[summarize] 摘要 LLM 调用超时（上限 {}s）",
                AUX_SUMMARY_TIMEOUT.as_secs()
            ))
        });
    let duration_ms = start.elapsed().as_millis() as u64;

    let (response_content, raw_req, raw_resp) = match &mut response {
        Ok(r) => {
            let content = r.content.clone();
            let req = r.raw_request_body.take();
            let resp = r.raw_response_body.take();
            (content, req, resp)
        }
        Err(_) => (String::new(), None, None),
    };

    // Emit LlmResponse + ConversationEnd after the call (await, sequential —
    // LlmResponse fully processed before ConversationEnd, as the old emit_sync intended).
    if let Some(mgr) = observer_manager {
        let response_event = ObserverEvent::LlmResponse {
            trace_id: trace_id.clone(),
            round: 0,
            duration_ms,
            has_tool_calls: false,
            content: response_content.clone(),
            tool_calls: vec![],
            tool_calls_count: 0,
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_request_body: raw_req,
            raw_response_body: raw_resp,
        };
        mgr.emit(response_event.to_conversation_event()).await;

        let end_event = ObserverEvent::ConversationEnd {
            trace_id,
            session_key: label.to_string(),
            total_rounds: 1,
            duration_ms,
            content: response_content,
            channel: String::new(),
            chat_id: String::new(),
        };
        mgr.emit(end_event.to_conversation_event()).await;
    }

    Some(response)
}

// ---------------------------------------------------------------------------
// Cluster integration helpers
// ---------------------------------------------------------------------------

/// Adjust a summarize boundary so the verbatim tail `history[new_c..]` doesn't
/// START in the middle of a tool_call/result pair.
///
/// `summarize_prefix_owned` only folds user/assistant `content` into the
/// summary — tool_calls and tool results are not summarized. If the boundary
/// landed between an assistant tool_call (covered by the summary only as text
/// content, which is often empty for a pure tool-call turn) and its tool
/// result, `repair_tool_message_pairs` would drop the orphan result from the
/// tail and the whole interaction would vanish from the LLM's view. Backing
/// `new_c` up past any leading tool messages moves the parent assistant (and
/// sibling results) into the verbatim tail, keeping the pair intact. (The old
/// `truncate_with_tool_pairs` did the equivalent by prepending the parent.)
///
/// The summary still covers `history[..returned_new_c]` and the tail is
/// `history[returned_new_c..]` — the gap-free invariant holds; the tail just
/// grows slightly past the budget boundary when a pair straddles the boundary.
pub(crate) fn tool_safe_boundary(
    history: &[crate::types::ConversationTurn],
    mut new_c: usize,
) -> usize {
    while new_c > 0 && new_c < history.len() && history[new_c].role == "tool" {
        new_c -= 1;
    }
    new_c
}

/// Summarize a contiguous prefix of the conversation, merging any existing
/// summary.
///
/// Summarizes **all** of `messages` (no internal "keep last N" step) — the
/// caller has already chosen the verbatim tail boundary (P5 token budget,
/// legacy K_TARGET count fallback), so every
/// message passed in is meant to be folded into the summary. Keeping a "last N"
/// here would leave a gap between the summary and the verbatim tail. Reuses the
/// multipart/batch machinery; merges `existing_summary` (which covers messages
/// before this prefix) into the result.
///
/// G1 (U1) prefix-reuse: the summary request is built as
/// `[system, ...original covered messages..., instruction]` — the same leading
/// messages the main loop sends (byte-equal per message), so the provider's
/// warm KV prefix from the last routed request is REUSED rather than
/// invalidated ("genuine prefix" principle). The old
/// form (single bare user message with `role: content` text concatenation)
/// shared no prefix with real requests and destroyed structure (tool_calls
/// flattened to text).
///
/// Returns `Some(summary)` if a non-empty summary was produced, `None`
/// otherwise. **None 的两种含义都不允许调用方推进 covers_up_to**：
/// (a) 前缀里没有可摘要的 user/assistant 内容；(b) 摘要 LLM 调用失败
/// （2026-08-25 摘要静默失败修复：此前 batch 失败返回空字符串，multipart
/// 拿两段空串去 merge，merge 模型回"你没把摘要贴给我"，这段**失败回复**
/// 被当摘要存进 store、covers 照常推进——被折叠的上下文静默丢失。现在
/// 失败一路传播为 None，调用方保持原 history 不折叠并 warn。）
///
/// T4 (U1) per-model switch: `prefix_reuse == false` falls back to the
/// pre-G1 shape (`summarize_bare_concat_owned`) — per-model config
/// `summarizer_prefix_reuse: false`, for cheap summarizer models that break
/// the assumed warm KV prefix. Default (true) keeps the prefix-reuse shape.
pub(crate) async fn summarize_prefix_owned(
    messages: &[&crate::types::ConversationTurn],
    existing_summary: &str,
    new_segment_turns: usize,
    context_window: usize,
    prefix_reuse: bool,
    provider: &dyn LlmProvider,
    model: &str,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
) -> Option<String> {
    // P7：文件操作台账——对传入的完整前缀聚合（调用方传的是
    // history[..new_c]，旧摘要已覆盖段天然在内 → 台账随迭代累积）。
    let ledger = collect_file_ops_ledger(messages);
    let update = SummaryUpdate {
        existing: existing_summary,
        new_segment_turns,
        ledger: &ledger,
    };

    // Oversized message guard.
    let max_msg_tokens = context_window / 2;
    let mut valid_messages: Vec<&crate::types::ConversationTurn> = Vec::new();
    let mut omitted = false;

    for m in messages {
        if m.role != "user" && m.role != "assistant" {
            continue;
        }
        let msg_tokens = crate::session::estimate_tokens(&m.content);
        if msg_tokens > max_msg_tokens {
            omitted = true;
            continue;
        }
        valid_messages.push(m);
    }

    if valid_messages.is_empty() {
        return None;
    }

    let final_summary = if !prefix_reuse {
        // T4 (U1): old shape — single bare user message, no structure.
        summarize_bare_concat_owned(&valid_messages, &update, provider, model, observer_manager)
            .await
    } else {
        // G1: the system prompt anchoring the prefix. `messages` is
        // history[..new_c]; history[0] is the system turn — include it verbatim
        // (WITHOUT the summary block the main loop appends: that would leak the
        // old summary into the prefix and change it between rounds).
        let system_msg: Option<LlmMessage> = messages
            .first()
            .filter(|m| m.role == "system")
            .map(|m| conversation_turn_to_llm_message(m));

        if valid_messages.len() > 10 {
            summarize_multipart_owned(
                system_msg.as_ref(),
                &valid_messages,
                &update,
                provider,
                model,
                observer_manager,
            )
            .await
        } else {
            summarize_batch_owned(
                system_msg.as_ref(),
                &valid_messages,
                &update,
                provider,
                model,
                observer_manager,
            )
            .await
        }
    };

    // P6+P7 收尾：schema 解析/回退 + 省略注记 + 台账节（单点，batch 与
    // multipart 两条路径共用同一收尾，不会漂移）。
    final_summary
        .map(|s| finalize_summary_text(&s, &ledger, omitted))
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// WS2（能力扩展 P5/P6/P7）：token 预算尾巴 / 结构化摘要 schema / 文件操作
// 台账测试。独立测试文件（生产文件只保留声明行，仓库纪律）。
// ---------------------------------------------------------------------------
#[cfg(test)]
mod ws2_compact_tests;
