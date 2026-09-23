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

/// Target number of trailing messages kept verbatim (not summarized) when the
/// summary cache advances. The tail `history[C..]` is held at ~`K_TARGET`
/// messages by `maybe_update_summary` (C = len - K_TARGET). Small enough that
/// the LLM always sees recent context verbatim, large enough to ride out a
/// few tool-call rounds between summarizations.
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
    /// against the tail, not the full history. When the tail exceeds the
    /// threshold and is longer than `K_TARGET`, the cache advances to
    /// `covers_up_to = len - K_TARGET` and the newly-covered prefix is folded
    /// into the summary. History is never mutated (append-only); bounding is
    /// the session store's job.
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
        let tail_len = history.len().saturating_sub(c);
        let soft = context_window * COMPACT_SOFT_RATIO / 100;
        let threshold = context_window * COMPACT_SUMMARIZE_RATIO / 100;
        // Summarize runs only when the tail is over threshold AND long enough to
        // shrink (more than K_TARGET messages past C). A short tail that is huge
        // in tokens (a large system prompt or an early oversized tool result)
        // can't be helped by advancing C — leave it to force_compression.
        let will_summarize = tail_tokens >= threshold && tail_len > K_TARGET;

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

        // New boundary: cover everything except the last K_TARGET messages
        // (the verbatim tail kept for continuity). new_C > c is guaranteed by
        // the tail_len > K_TARGET check above, so we always advance and never
        // re-summarize the same prefix. Adjust so the tail doesn't start mid
        // tool_call/result pair (keeps the pair verbatim, not dropped by repair).
        let new_c = tool_safe_boundary(&history, history.len() - K_TARGET);

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
        let prefix_refs: Vec<&crate::types::ConversationTurn> = history[..new_c].iter().collect();
        let summary = summarize_prefix_owned(
            &prefix_refs,
            existing_summary,
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
    pub async fn force_compression_opts(&self, instance: &AgentInstance, prefer_small: bool) {
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
        if new_c <= current_c || new_c == 0 {
            return;
        }

        let (provider, model) = self.resolve_summary_provider(prefer_small);
        let observer_mgr = self.observer_manager.clone();

        // Fold the prefix history[..new_c] into the summary, merged with the
        // existing summary (which covers history[..current_c]).
        let prefix_refs: Vec<&crate::types::ConversationTurn> = history[..new_c].iter().collect();
        let summary = summarize_prefix_owned(
            &prefix_refs,
            existing_summary,
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
    }

    // -----------------------------------------------------------------------
    // E6: manual session maintenance (/compact /clear)
    // -----------------------------------------------------------------------

    /// E6: 手动 compaction 入口。取（从 store 重建的）instance，调
    /// `force_compression` 推进摘要覆盖，再把新摘要持久化回 store（顺序同
    /// 回合末：先 summary+covers 后 history，见回合末块注释）。
    ///
    /// 成功回执带覆盖数（摘要覆盖前 N 条，保留近 M 条）；摘要 LLM 失败或
    /// 无可压缩内容时返回 Err（covers 不推进——2026-08-25 静默失忆修复的
    /// 同一契约），历史保持不变。
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
        self.force_compression_opts(&instance, true).await;

        // 方言 PostCompact（观察型）：压缩尝试结束（成败皆触发），与 auto 路径对称。
        let bridge_post = self.cc_bridge.read().as_ref().cloned();
        if let Some(bridge) = bridge_post {
            bridge.run_compact_hooks("manual", "post").await;
        }

        let Some(cache) = instance
            .get_summary_cache()
            .filter(|c| c.covers_up_to > before && !c.text.is_empty())
        else {
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
