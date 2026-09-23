//! 配置热载与访问器：set_config_path/config_mtime/check_config_reload、refresh_active_tier、current_* 访问器族、tier/mode/set_mode_with_event、set_provider_and_model、SmallModelSlot/小模型槽、E7 标题生成、诊断回灌（C3）。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// N2: 小模型槽位——provider 与其模型名绑在一起换，避免两者失配。
#[derive(Clone)]
pub(crate) struct SmallModelSlot {
    provider: Arc<dyn LlmProvider>,
    name: String,
}

/// E7: 自动标题的输入上限（首条 user 消息截断，计划原文 ≤500 字）。
pub(crate) const E7_TITLE_INPUT_MAX_CHARS: usize = 500;
/// E7: 自动标题的长度上限（计划原文 ≤24 字）。
pub(crate) const E7_TITLE_MAX_CHARS: usize = 24;

/// E7: 一次性 LLM 标题生成（无工具、非流式）。失败/空输出 → None（调用
/// 方诚实跳过，下轮回复再试）。
pub(crate) async fn generate_title_from_first_message(
    provider: &dyn LlmProvider,
    model: &str,
    first_user: &str,
) -> Option<String> {
    let prompt = format!(
        "根据以下用户请求，生成一个不超过{}字的会话标题。直接输出标题本身：不要引号、不要解释、不要换行。\n\n用户请求：{}",
        E7_TITLE_MAX_CHARS, first_user
    );
    let resp = provider
        .chat(
            model,
            vec![LlmMessage {
                role: "user".to_string(),
                content: prompt,
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
                images: Vec::new(),
            }],
            None,
            Vec::new(),
        )
        .await
        .ok()?;
    sanitize_generated_title(&resp.content)
}

/// E7: 标题清洗——取首行、剥首尾引号/反引号/空白、截到 [`E7_TITLE_MAX_CHARS`]
/// 字符（char 边界安全，CJK 不劈半）。清洗后为空 → None。
pub(crate) fn sanitize_generated_title(raw: &str) -> Option<String> {
    let first_line = raw.lines().next().unwrap_or("").trim();
    let unquoted = first_line.trim_matches(|c| {
        matches!(
            c,
            '"' | '\'' | '`' | '\u{201c}' | '\u{201d}' | '\u{300c}' | '\u{300d}'
        )
    });
    let cleaned = unquoted.trim();
    if cleaned.is_empty() {
        return None;
    }
    Some(cleaned.chars().take(E7_TITLE_MAX_CHARS).collect())
}

impl AgentLoop {
    /// Switch the active model by alias (resolved via `config.models`) or literal
    /// model id. Returns the resolved model id. Unknown aliases are used as-is
    /// (so `/model deepseek-v4-pro` works even without an alias entry).
    pub fn set_active_model(&self, alias_or_model: &str) -> String {
        let model = self
            .config
            .models
            .get(alias_or_model)
            .cloned()
            .unwrap_or_else(|| alias_or_model.to_string());
        *self.active_model.write() = model.clone();
        info!(
            "[AgentLoop] Active model set to {} (via '{}')",
            model, alias_or_model
        );
        // Phase 4a: re-resolve capability tier for the new model.
        self.refresh_active_tier();
        model
    }

    /// Available model aliases (from config.models), for `/model` listing.
    pub fn model_aliases(&self) -> Vec<String> {
        self.config.models.keys().cloned().collect()
    }
    /// N2：装配小模型杂务通道（工厂从 `agents.small_model` 解析后调用；
    /// 重复调用覆盖前值）。传 `None` 显式清除（= 回退主模型）。
    pub fn set_small_model(&self, provider: Option<(Arc<dyn LlmProvider>, String)>) {
        *self.small_model.write() = provider.map(|(p, name)| SmallModelSlot { provider: p, name });
    }

    /// N2：小模型槽位只读访问（E7 自动标题消费；未配置 = `None`，诚实跳过
    /// ——不烧主模型 token 生成标题）。
    pub(crate) fn small_model_slot(&self) -> Option<(Arc<dyn LlmProvider>, String)> {
        self.small_model
            .read()
            .as_ref()
            .map(|s| (s.provider.clone(), s.name.clone()))
    }

    /// N2：手动压缩（`compact_session`）的摘要供给解析——小模型已配置则用
    /// 之；未配置（或 `prefer_small=false`）诚实回退主模型。锁序固定
    /// small_model → provider → active_model，全库唯此一处取这三把锁。
    pub(crate) fn resolve_summary_provider(
        &self,
        prefer_small: bool,
    ) -> (Arc<dyn LlmProvider>, String) {
        if prefer_small {
            if let Some(slot) = self.small_model.read().as_ref() {
                return (slot.provider.clone(), slot.name.clone());
            }
            debug!(
                "[AgentLoop] agents.small_model not configured; manual compact falls back to the main model"
            );
        }
        (
            self.provider.read().clone(),
            self.active_model.read().clone(),
        )
    }

    /// Swap the LLM provider and model at runtime. Takes effect immediately
    /// for the next LLM call. In-flight requests continue with the old provider.
    pub fn set_provider_and_model(&self, provider: Arc<dyn LlmProvider>, model: String) {
        *self.provider.write() = provider;
        *self.active_model.write() = model;
        tracing::info!("[AgentLoop] Provider swapped at runtime");
        // Phase 4a: re-resolve capability tier for the new model.
        self.refresh_active_tier();
    }

    /// J5 (devtool-upgrade 阶段 6)：`agents.doom_loop_approval` fresh-read
    /// （F8 `current_hidden_tools` 同款模式——config.json 是唯一真相源，每次
    /// escalation 现读，运行时改键下一轮生效，无需重启）。无 config_path /
    /// 解析失败 = `false`（安全底座方向）。
    pub(crate) fn current_doom_loop_approval(&self) -> bool {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return false,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("agents")
                    .and_then(|a| a.get("doom_loop_approval"))
                    .and_then(|b| b.as_bool())
            })
            .unwrap_or(false)
    }

    /// J6：超限图片自动降采样开关（F8 模式 fresh-read config.json；默认**开**，
    /// 极性与 doom_loop_approval 相反——读不到/键缺失按开处理，保持降采样
    /// 能力可用；读取失败不阻塞轮次）。
    pub(crate) fn current_image_downscale(&self) -> bool {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return true,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("agents")
                    .and_then(|a| a.get("image_downscale"))
                    .and_then(|b| b.as_bool())
            })
            .unwrap_or(true)
    }

    /// 429 重试环（裁决④）：`agents.defaults.rate_limit_retries` fresh-read
    /// （F8 模式——config.json 唯一真相源，改键下一轮生效）。无 config_path /
    /// 解析失败 = 默认 10（default_rate_limit_retries 同源口径）。
    pub(crate) fn current_rate_limit_retries(&self) -> i64 {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return 10,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("agents")
                    .and_then(|a| a.get("defaults"))
                    .and_then(|d| d.get("rate_limit_retries"))
                    .and_then(|n| n.as_i64())
            })
            .unwrap_or(10)
    }

    /// BUG 2026-09-21 ②：`agents.defaults.<key>` fresh-read（F8 模式，同
    /// [`Self::current_rate_limit_retries`] 口径）。解析失败或缺键 = 默认值。
    fn agents_defaults_u64(&self, key: &str, default: u64) -> u64 {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return default,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| {
                v.get("agents")
                    .and_then(|a| a.get("defaults"))
                    .and_then(|d| d.get(key))
                    .and_then(|n| n.as_u64())
            })
            .unwrap_or(default)
    }

    /// BUG 2026-09-21 ②A：单次上游调用超时（秒）。0 = 关闭（旧行为）。
    pub(crate) fn current_provider_call_timeout_secs(&self) -> u64 {
        self.agents_defaults_u64(
            "provider_call_timeout_secs",
            DEFAULT_PROVIDER_CALL_TIMEOUT_SECS,
        )
    }

    /// BUG 2026-09-21 ②B：限流重试环总预算（秒）。0 = 不限。
    pub(crate) fn current_rate_limit_budget_secs(&self) -> u64 {
        self.agents_defaults_u64("rate_limit_budget_secs", DEFAULT_RATE_LIMIT_BUDGET_SECS)
    }

    /// Phase 4a: capability tier currently in effect (small-model-tool-robustness).
    pub fn tier(&self) -> nemesis_types::capability::ModelTier {
        *self.tier.read()
    }

    /// 当前活跃模型 id（解析后的模型名，非别名；审计/展示用——P5 冲突硬解
    /// 决策流卡记「哪个模型做的硬解」）。切换走 [`Self::set_active_model`]。
    pub fn active_model(&self) -> String {
        self.active_model.read().clone()
    }

    /// Phase 4a: override the capability tier (e.g. after resolving it from the
    /// active model config at construction, or after `model set-tier`).
    pub fn set_tier(&self, tier: nemesis_types::capability::ModelTier) {
        info!("[AgentLoop] Capability tier set: {}", tier);
        *self.tier.write() = tier;
    }

    /// F1（devtool-upgrade 阶段 4）：当前工作模式。
    pub fn mode(&self) -> crate::types::AgentMode {
        *self.mode.read()
    }

    /// F1：翻转工作模式并广播 `ModeChanged`（session_key/chat_id 供 web
    /// pump 路由到会话徽标；传空串 = 只进 SSE）。重复设置同一模式仍是
    /// 幂等无害（徽标确认刷新），事件照发——调用方（/plan /build 臂）已经
    /// 在语义上表达「我要切到 X」而非「X 变了」。
    pub fn set_mode_with_event(
        &self,
        mode: crate::types::AgentMode,
        session_key: &str,
        chat_id: &str,
    ) {
        *self.mode.write() = mode;
        info!("[AgentLoop] Agent mode set: {}", mode.as_str());
        let event = nemesis_types::agent::AgentEvent::ModeChanged {
            session_key: session_key.to_string(),
            chat_id: chat_id.to_string(),
            mode: mode.as_str().to_string(),
        };
        if let Some(tx) = self.agent_event_tx.read().as_ref() {
            // 无订阅者（CLI / 无人在线）= 观察者通道空转，静默忽略。
            let _ = tx.send(event);
        }
    }

    /// F1：Plan 模式只读供给白名单。命中者才进 tool_defs（MCP 前缀工具与
    /// spawn 除外——MCP 语义未知不猜，采取「plan 只 deny 已知写」
    /// 立场；spawn 保留供给，分发闸在子代理深度同样生效=纵深）。
    /// 曾前瞻列入的 question（F7）/multiedit（A7）均已落码（2026-09-06），
    /// 本表沿用语义不变。
    pub(crate) const PLAN_MODE_TOOLS: &'static [&'static str] = &[
        "read_file",
        "list_dir",
        "grep",
        "git",
        "web_fetch",
        "lsp",
        "mcp_list",
        "memory_search",
        "memory_list",
        "skills_list",
        "skills_info",
        "find_skills",
        "cli_reference",
        "cron",
        "sleep",
        "message",
        "history_search",
        "todowrite",
        "question",
    ];

    /// F1：分发端写类集合（Plan 模式拦截对象）。**有意偏离计划字面**
    /// （MOVE_TOOLS 全量并集会把 read_file/grep/list_dir/git 四个只读工具
    /// 也拦掉——与计划自己的供给白名单矛盾）：取「可变更外部状态」语义集
    /// = MOVE_TOOLS 去掉四个只读 + multiedit（A7 已落码，2026-09-06）。
    /// exec/run_script 可落盘必拦；git 的写子命令由安全 8 层管线与 D1 自
    /// 管，模式层不重复。
    pub(crate) const PLAN_MODE_WRITE_TOOLS: &'static [&'static str] = &[
        "exec",
        "run_script",
        // C8（2026-09-06）：cargo/npm/go 构建落 target//node_modules/——
        // 「可变更外部状态」同语义，plan 模式一并拦。
        "run_checks",
        "write_file",
        "edit_file",
        "append_file",
        "delete_file",
        "create_dir",
        "delete_dir",
        "multiedit",
    ];

    /// Phase 4a: set the config.json path. After this, the tier is re-resolved
    /// live from config.json on every model switch and whenever the file's mtime
    /// changes (dashboard model add, CLI `model set-tier`). config.json is the
    /// single source of truth — there is no stale per-model snapshot to keep in
    /// sync. Called by `agent_factory` at gateway startup.
    pub fn set_config_path(&self, path: std::path::PathBuf) {
        *self.config_path.write() = Some(path);
    }

    /// N1 (devtool-upgrade 阶段 1): inject the shared layered pricing store
    /// (workspace/data). L2 of the three-tier `context_window` resolution —
    /// when config.json has no explicit `context_window` for the active
    /// model, `PricingStore::lookup` (custom > downloaded > embedded, with
    /// bare-suffix matching) supplies `max_input_tokens`. `None` (not
    /// injected / open failed) skips straight to L3 fallback.
    pub fn set_pricing_store(&self, store: std::sync::Arc<nemesis_data::PricingStore>) {
        *self.pricing_store.write() = Some(store);
    }

    /// C3（devtool-upgrade 阶段 2）：注入共享 LspManager 单例（与 LspTool
    /// 同一实例——SharedResources.lsp_manager，见 C5）。编辑后诊断回灌
    /// （修复闭环）消费；`None`（未注入 / standalone）→ 反馈静默跳过。
    pub fn set_lsp_manager(&self, mgr: Arc<nemesis_lsp::LspManager>) {
        *self.lsp_manager.write() = Some(mgr);
    }

    /// C3：读 `agents.defaults.diagnostics_loop`（config.json 每次新鲜读，
    /// 同 [`Self::current_tool_doc_folding`] 模式——dashboard/CLI 可在网关
    /// 运行中翻转开关）。缺段 / standalone → 全默认（enabled=false）。
    pub(crate) fn current_diagnostics_loop(&self) -> nemesis_config::DiagnosticsLoopConfig {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return Default::default(),
        };
        let v = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let Some(v) = v else {
            return Default::default();
        };
        v.get("agents")
            .and_then(|a| a.get("defaults"))
            .and_then(|d| d.get("diagnostics_loop"))
            .and_then(|s| {
                serde_json::from_value::<nemesis_config::DiagnosticsLoopConfig>(s.clone()).ok()
            })
            .unwrap_or_default()
    }

    /// C3：编辑后诊断回灌核心（修复闭环：「落盘 → 触发诊断 → 等 ERROR →
    /// please fix」）。调用点在工具结果
    /// 进 spill/gate 管线**之前**——反馈与工具结果同走一条模型可见管线。
    ///
    /// 全部失败/未命中路径**静默原样返回**（开关关 / 非 write|edit / 无
    /// manager / 语言无服务器 / 同步失败 / 无 ERROR）——诊断永不拖垮工具
    /// 调用。ERROR 级取 ≤`max_errors` 条追加：
    /// `"\n\n[LSP] {n} error(s) detected in {path}, please fix:"` + 每条
    /// `"- L{line}:{col} {message} ({source})"`（1-based 显示，LSP 0-based
    /// 内部转换）。
    ///
    /// 文档同步用 [`nemesis_lsp::LspManager::touch_file`]（读盘下发）而非
    /// `notify_change`：edit_file 的最终内容无法从 args 重建，磁盘是唯一
    /// 真相源；write_file 读盘等价（execute 返回即写完）。
    pub(crate) async fn apply_diagnostics_feedback(
        mgr: Option<&nemesis_lsp::LspManager>,
        cfg: nemesis_config::DiagnosticsLoopConfig,
        tool_name: &str,
        path: &str,
        result: &str,
    ) -> String {
        if !cfg.enabled || !matches!(tool_name, "write_file" | "edit_file") {
            return result.to_string();
        }
        let Some(mgr) = mgr else {
            return result.to_string();
        };
        let p = std::path::Path::new(path);
        // 未注册语言 / 该语言无已安装服务器 → 原样（探测是纯 PATH 查找，
        // 不 spawn 进程）。
        let Some(lang) = nemesis_lsp::registry::lang_for_path(p) else {
            return result.to_string();
        };
        if !nemesis_lsp::registry::server_available(lang) {
            return result.to_string();
        }
        // 服务器同步失败 → 原样（best-effort）。
        if mgr.touch_file(p).await.is_err() {
            return result.to_string();
        }
        let diags = mgr.wait_for_diagnostics(p, 150, cfg.wait_max_ms).await;
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == 1)
            .take(cfg.max_errors)
            .collect();
        if errors.is_empty() {
            return result.to_string();
        }
        let mut out = String::with_capacity(result.len() + 96 * errors.len());
        out.push_str(result);
        out.push_str(&format!(
            "\n\n[LSP] {} error(s) detected in {}, please fix:",
            errors.len(),
            path
        ));
        for d in errors {
            out.push_str(&format!(
                "\n- L{}:{} {} ({})",
                d.range_start.0 + 1,
                d.range_start.1 + 1,
                d.message,
                d.source.as_deref().unwrap_or("lsp")
            ));
        }
        out
    }

    /// C3：dispatch 现场包装——读共享 manager + 新鲜 config 后委托核心。
    pub(crate) async fn diagnostics_feedback(
        &self,
        tool_name: &str,
        path: &str,
        result: &str,
    ) -> String {
        let mgr = self.lsp_manager.read().clone();
        Self::apply_diagnostics_feedback(
            mgr.as_deref(),
            self.current_diagnostics_loop(),
            tool_name,
            path,
            result,
        )
        .await
    }

    /// 内置 slash 命令名（与 [`Self::handle_command_with_context`] 的 match 臂
    /// **同步维护**）：自定义命令表命中这些名字时跳过改写——内置优先。
    /// `compact`/`clear` 是 E6 会话维护命令（gate 内 `parse_maintenance_command`
    /// 拦截，有副作用，同样不许被自定义命令表遮蔽）；`plan`/`build` 是 F1
    /// 模式切换（gate 内独立臂，要先拿 chat_id 发布 ModeChanged）。
    /// K3：清单本体收敛到 `nemesis_types::constants::BUILTIN_SLASH_COMMANDS`
    /// （dashboard `commands.list` 的 `builtins` 下发同源），这里只做转发。
    pub(crate) const BUILTIN_SLASH_COMMANDS: &'static [&'static str] =
        nemesis_types::constants::BUILTIN_SLASH_COMMANDS;

    /// Phase 4a: re-resolve the capability tier against the currently active
    /// model by reading config.json live. Per-model `model_tier`/`real_name`/
    /// `model_size_b` are honoured; a missing/unreadable config falls back to
    /// the name heuristic. Called after every model switch and on config change.
    // 热重载收编登记（2026-08-29）：此处非纯数据加载（读 config 后写多个
    // AgentLoop 状态），HotReloader<T> 的 fn(&Path)->T 形状装不下——保留独立
    // 实现并登记（计划原文"诚实优于强行统一"）。收编需 trait 形状的 reloader。
    pub(crate) fn refresh_active_tier(&self) {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return, // standalone mode — keep the startup tier
        };
        let active = self.active_model.read().clone();
        let tier = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| nemesis_types::capability::resolve_active_tier(&v, &active))
            .unwrap_or_else(|| {
                nemesis_types::capability::detect_tier(&nemesis_types::capability::TierHint {
                    full_model: Some(active.clone()),
                    real_name: None,
                    size_b: None,
                })
            });
        if *self.tier.read() != tier {
            info!(
                "[AgentLoop] Active model '{}' → capability tier {} (re-resolved from config.json)",
                active, tier
            );
            *self.tier.write() = tier;
        }
    }

    /// Resolve the active model's display id (`provider/name`, e.g.
    /// `deepseek/deepseek-v4-flash`) for the per-message "供应商·模型名"
    /// badge. Reads config.json fresh each call (called once per assistant
    /// turn, negligible cost) — no cached field, so it can never go stale when
    /// the model switches. Falls back to the bare `active_model` when config
    /// is unavailable (standalone mode) or the entry isn't found.
    pub(crate) fn current_display_model(&self) -> String {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return active,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| nemesis_types::capability::resolve_display_model(&v, &active))
            .unwrap_or(active)
    }

    /// Resolve the active model's per-model output token cap (`max_output_tokens`)
    /// from config.json, used as the chat request's `max_tokens`. Reads config
    /// fresh each call (like `current_display_model`). `None` when config is
    /// unavailable (standalone mode) or the field is absent — caller falls back
    /// to the 8192 default. Lets each model declare its real output ceiling so
    /// large files write in one shot instead of truncating at a blanket cap.
    /// H4 (U16 half): the active model's reasoning-effort tier from
    /// config.json (`model set-effort`). None when unset/"off"/standalone.
    pub(crate) fn current_reasoning_effort(&self) -> Option<String> {
        let active = self.active_model.read().clone();
        let path = self.config_path.read().clone()?;
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| nemesis_types::capability::resolve_reasoning_effort(&v, &active))
    }

    pub(crate) fn current_max_tokens(&self) -> Option<u32> {
        let active = self.active_model.read().clone();
        let path = self.config_path.read().clone()?;
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| nemesis_types::capability::resolve_max_output_tokens(&v, &active))
            .map(|n| n as u32)
        // ↑ i64 (from JSON) → u32; max_output_tokens is a non-negative count
    }

    /// U16 (sixth batch) + N1 (devtool-upgrade 阶段 1)：active 模型的
    /// context_window（input token capacity）**三级解析链**：
    /// L1 config.json 显式 `context_window`（用户最大）→ L2 价目表
    /// `max_input_tokens`（`PricingStore::lookup`，含 bare-suffix 匹配）→
    /// L3 [`FALLBACK_CONTEXT_WINDOW`]（128k，由调用方
    /// `instance.context_window()` 兜底）。读 config 每次新鲜（同
    /// `current_max_tokens` 模式）。This closes the S1-S7 leftover: the
    /// compaction thresholds in `maybe_summarize` were computed against a
    /// hardcoded 32000 regardless of the model's real window (a
    /// 200K-window model compacted 6× too early) — and N1 replaces that
    /// fallback itself (32k → 128k + catalog lookup).
    pub(crate) fn current_context_window(&self) -> Option<usize> {
        self.current_context_window_with_source().0
    }

    /// [`Self::current_context_window`] 带来源标记（N1 可观测）：
    /// `Some((窗口, "config" | "catalog"))`；L3 未命中 → `(None, "fallback-128k")`
    /// ——调用方用 `instance.context_window()`（= [`FALLBACK_CONTEXT_WINDOW`]）。
    pub(crate) fn current_context_window_with_source(&self) -> (Option<usize>, &'static str) {
        let active = self.active_model.read().clone();
        let cfg = self.config_path.read().clone().and_then(|path| {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        });
        let pricing = self.pricing_store.read().clone();
        resolve_context_window_tiered(cfg.as_ref(), &active, pricing.as_deref())
    }

    /// T10（多模态 goal）：active 模型的 vision 能力（读 config.json 新鲜
    /// 值，同 [`current_max_tokens`] 模式——config.json 是唯一真相源，模型
    /// 中途切换 / 磁盘改动都自然生效）。standalone / 条目缺失 → 默认放行。
    pub(crate) fn current_vision(&self) -> nemesis_types::capability::VisionResolution {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return nemesis_types::capability::vision_default_allow(),
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .map(|v| nemesis_types::capability::resolve_active_vision(&v, &active))
            .unwrap_or_else(nemesis_types::capability::vision_default_allow)
    }

    /// T4 (U1): per-model summarizer prefix-reuse switch from config.json
    /// (`summarizer_prefix_reuse`, default true — the main model keeps the
    /// G1 prefix-reuse summary shape). `false` → the summary request falls
    /// back to the pre-G1 shape (single bare user message with
    /// `role: content` text concatenation), for cheap summarizer models that
    /// break the assumed warm KV prefix. Reads config fresh each call (same
    /// pattern as [`current_max_tokens`]); standalone (no config_path) →
    /// default true.
    pub(crate) fn current_summarizer_prefix_reuse(&self) -> bool {
        let active = self.active_model.read().clone();
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return true,
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| nemesis_types::capability::resolve_summarizer_prefix_reuse(&v, &active))
            .unwrap_or(true)
    }

    /// Phase 4a: detect config.json on-disk changes (by mtime) and re-resolve
    /// the active model's tier if it changed. Runs once per LLM round, next to
    /// `check_mcp_reload`. Picks up dashboard model additions and CLI
    /// `model set-tier` while the gateway is running.
    pub(crate) fn check_config_reload(&self) {
        let path = match self.config_path.read().clone() {
            Some(p) => p,
            None => return,
        };
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        {
            let mut last = self.config_mtime.write();
            if mtime == *last {
                return; // unchanged since last check
            }
            *last = mtime;
        }
        debug!("[AgentLoop] config.json mtime changed; re-resolving capability tier");
        self.refresh_active_tier();
    }
}
