// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 agent_factory.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// Build a fresh AgentLoop from disk config.
///
/// Re-reads `config.json`, workspace files, creates new provider,
/// registers all tools — identical to first-boot initialization.
pub fn build_agent_loop(
    shared: &Arc<SharedResources>,
) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>> {
    use nemesis_agent::r#loop::AgentLoop;
    use nemesis_agent::types::AgentConfig;

    // 1. Re-read config.json from disk.
    let config_path = shared.home.join("config.json");
    let cfg = nemesis_config::load_config(&config_path)
        .map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))?;

    // 2. Resolve LLM model → create fresh provider.
    let llm_ref = nemesis_config::get_effective_llm(Some(&cfg));
    // 装配守卫④：默认对话模型槽只收 conversation 角色——
    // utility/classifier 条目手改 config 才能到达该状态（CLI/WSAPI 写入侧
    // 守卫①②已拦），loud 报错并走下方既有 NullProvider 降级臂（不硬崩，
    // Dashboard 配好并设默认后热切恢复）。
    let assembly = match nemesis_config::ensure_conversation_role(&cfg, &llm_ref) {
        Err(e) => Err(e),
        Ok(()) => nemesis_config::resolve_model_config(&cfg, &llm_ref).map_err(|e| e.to_string()),
    };
    let (resolution, _resolve_degraded) = match assembly {
        Ok(r) => (r, None),
        Err(e) => {
            warn!(
                "[AgentFactory] Failed to resolve model '{}': {} — 无 LLM 降级装配（NullProvider，Dashboard 配好并设默认后热切恢复）",
                llm_ref, e
            );
            (
                nemesis_config::ProviderResolution {
                    model_name: llm_ref.clone(),
                    ..Default::default()
                },
                Some(e.to_string()),
            )
        }
    };
    let model_name = resolution.model_name.clone();

    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        proxy: resolution.proxy.clone(),
        stream_mode: resolution.stream_mode.clone(),
        cache_breakpoints: resolution.cache_breakpoints,
        extra_body: resolution.extra_body.clone(),
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key.clone(),
        api_base: resolution.api_base.clone(),
        workspace: shared.workspace_dir().to_string_lossy().to_string(),
        connect_mode: resolution.connect_mode,
        protocol: resolution.protocol.clone(),
        timeout_secs: resolution.timeout_secs,
        account_id: String::new(),
        headers: HashMap::new(),
    };
    // 双击直启 goal：启动装配用 or_null——全新安装没配模型时
    // 网关必须照样起（先见 Dashboard、后配 LLM 热切）；一次性入口才 strict。
    let (provider, create_warn) = nemesis_providers::factory::create_provider_or_null(&factory_cfg);
    if let Some(w) = create_warn {
        warn!(
            "[AgentFactory] Provider create failed: {} — NullProvider 装配",
            w
        );
    }
    // （扩展计划）：fallback 链装配——active 模型条目的 per-model
    // extra 键 `fallback_to: ["alias-b",...]` 逐级 resolve + create，
    // FallbackProvider 包住主 provider（冷却语义沿用既有实现，不重写）。
    // 链空/全部不可用 = 单模型，行为与现状字节一致；单级解析/创建失败
    // warn 跳过（诚实降级），不阻断装配。
    let provider_arc: Arc<dyn nemesis_providers::router::LLMProvider> =
        wrap_fallback_chain(&cfg, &model_name, &shared.workspace_dir(), provider);
    info!("[AgentFactory] Provider created for {}", model_name);

    // 3. Build system prompt from workspace files (IDENTITY.md, SOUL.md, etc.)
    let workspace_dir = shared.workspace_dir();
    // prompt-pack pro：agents.prompt_system 选体系——pro 走段落池组装
    // （Pre 段 → 人格文件 → 行为段池 → 环境段），classic 原路径字节不变。
    // 启动装配型：system prompt 随实例冻结，改键重启生效。
    let prompt_system = nemesis_agent::prompt::PromptSystem::from_config(&cfg.agents);
    let (system_prompt, prompt_split) = {
        let mut context_builder = nemesis_agent::context::ContextBuilder::new(&workspace_dir);
        context_builder.set_prompt_system(prompt_system);
        // 入口形态变体（gap ⑤）：Interactive 恒空（gateway 字节不变）；
        // headless/ACP 由各自入口在 SharedResources 上显式设置。
        context_builder.set_entrance(shared.entrance);
        // B 线画像 / E 线 tier 档位不再在此注入（动态注入迁移）：
        // 两段迁入 build_messages_inner 的 merged snapshot 每轮重渲染
        // （mtime/live-tier 门控），sweep 学习、veto、tier 热切、pro 体系
        // 全部即时生效——build 时冻结形态（classic-only + 重启才更新）已废。
        let skills_dir = workspace_dir.join("skills");
        if skills_dir.exists() {
            context_builder.load_skills(&skills_dir);
        }
        context_builder.build_system_prompt_with_sections(false)
    };
    info!(
        "[AgentFactory] System prompt built ({} chars, prompt_system={:?})",
        system_prompt.len(),
        prompt_system
    );

    // 4. Create ProviderAdapter + AgentConfig + AgentLoop.
    let adapter = ProviderAdapter::new(provider_arc.clone(), model_name.clone());
    // a (small-model-tool-robustness): resolve the startup model's tier
    // from config.json, and remember the path so the tier can be re-resolved
    // live when the model switches or config.json changes on disk (dashboard
    // add / CLI model set-tier). config.json is the single source of truth —
    // no stale snapshot.
    let cfg_json: serde_json::Value =
        nemesis_utils::fsutil::read_config_string(shared.home.join("config.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null);
    let resolved_tier = nemesis_types::capability::resolve_active_tier(&cfg_json, &model_name);
    info!(
        "[AgentFactory] Model '{}' capability tier: {}",
        model_name, resolved_tier
    );
    let agent_config = AgentConfig {
        model: model_name.clone(),
        system_prompt: if system_prompt.is_empty() {
            None
        } else {
            Some(system_prompt)
        },
        max_turns: if cfg.agents.defaults.max_tool_iterations <= 0 {
            // 0 (or negative) = unlimited opt-in. The run-loop treats
            // max_turns == 0 as "no cap"; see AgentLoop process_message.
            0
        } else {
            cfg.agents.defaults.max_tool_iterations as u32
        },
        system_prompt_split: Some(prompt_split),
        tools: Vec::new(),
        tool_fence_profession: None,
        models: std::collections::HashMap::new(),
    };

    let max_continuation_permits = cfg.agents.defaults.max_continuation_permits.max(0) as usize;
    // +: concurrent_request_mode from config.
    // Default is "queue" (config.default.json + serde default agree); explicit
    // "reject" keeps legacy busy-bounce; "steer" adds the `!` interrupt channel.
    // Unknown strings warn + parse_concurrent_mode fails safe to reject.
    let mode_str = cfg
        .agents
        .defaults
        .concurrent_request_mode
        .trim()
        .to_lowercase();
    if !matches!(mode_str.as_str(), "reject" | "queue" | "steer") {
        tracing::warn!(
            "[AgentFactory] unknown concurrent_request_mode '{}' — falling back to reject",
            cfg.agents.defaults.concurrent_request_mode
        );
    }
    let concurrent_mode = nemesis_agent::r#loop::parse_concurrent_mode(&mode_str);
    let queue_size = cfg.agents.defaults.queue_size.max(1) as usize;
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(adapter),
        agent_config,
        shared.agent_outbound_tx.clone(),
        concurrent_mode,
        queue_size,
        max_continuation_permits,
    );
    // 路由 config 接线——bindings / agents.list /
    // session.dm_scope 构造生产 resolver（此前四键只声名未读取，恒用
    // new_bus 硬编码默认）；identity_links 注入 route_message 的 RouteInput。
    // 全部键缺省 = 与硬编码默认同形，零回归。
    agent_loop.set_route_resolver(build_route_resolver(&cfg));
    agent_loop.set_identity_links(cfg.session.identity_links.clone());
    // D-4（多会话并行清账）：loop 级并发 turn 上限（安全阀，
    // 默认 8；0 = 不设限）。泵统一 gate+spawn 后跨会话天然并发，此值防
    // 极端场景无界并发。
    agent_loop.set_max_concurrent_turns(cfg.agents.defaults.max_concurrent_turns.max(0) as usize);
    // a: apply the resolved startup tier + remember the config path so
    // runtime model switches and dashboard/CLI config edits re-resolve it live.
    agent_loop.set_tier(resolved_tier);
    agent_loop.set_config_path(shared.home.join("config.json"));
    // （凭据 vault）：Auth 凭据卡装配——provider 重建器恒挂
    // （纯数据件）；broker 走全局槽（security feature 门控；headless/acp
    // 未 install → None → Auth 臂走原终局零回归）。
    agent_loop.set_provider_reloader(Arc::new(ModelProviderReloader {
        home: shared.home.clone(),
        workspace: shared.workspace_dir(),
    }));
    #[cfg(feature = "security")]
    if let Some(broker) = crate::credential_broker::current() {
        agent_loop.set_credential_broker(broker);
    }
    // prompt-pack pro：描述档位与 system prompt 同一开关（启动装配型）。
    agent_loop.set_prompt_system(prompt_system);
    // (devtool-upgrade)：共享 LspManager 注入——编辑后诊断回灌
    // （write_file/edit_file → touch → 等 ERROR → "please fix"）与 LspTool
    // 共用同一实例（server 进程不翻倍）。standalone（None）路径反馈静默跳过。
    #[cfg(feature = "lsp")]
    agent_loop.set_lsp_manager(shared.lsp_manager.clone());
    // （聊天切会话竞态）：环尾 seq 查询回调注入——历史响应
    // 附 last_seq 供前端剔除「先于快照渲染的 assistant 实时帧」。指向
    // nemesis-web 的 chat_event_log::latest_seq（crate 方向 agent←web 不可
    // 直调，经此闭包注入；gateway.rs 的 install_event_hub 同层装配）。
    agent_loop.set_chat_seq_lookup(std::sync::Arc::new(nemesis_web::chat_event_log::latest_seq));
    // (devtool-upgrade)：价目表注入——三级 context_window 解析链
    // 的 （config 未显式配置时按 max_input_tokens 猜）。打开失败（磁盘
    // 异常）诚实降级为 None → fallback-128k，不阻断 agent 启动。
    match nemesis_data::PricingStore::open(&nemesis_path::workspace_data_dir(&shared.home)) {
        Ok(store) => agent_loop.set_pricing_store(std::sync::Arc::new(store)),
        Err(e) => tracing::warn!(
            error = %e,
            "[AgentFactory] pricing store open failed; context_window L2 (catalog) disabled"
        ),
    }
    // 自定义 slash 命令表（改写型快捷提示词；集群 agent 不接——见
    // set_commands_path 注释）。文件缺失 = 空表，无副作用。
    agent_loop.set_commands_path(nemesis_path::resolve_commands_config_path_in_workspace(
        &shared.workspace_dir(),
    ));
    // 内建示例管线插件（around 计时）——「插件」页 可展示/启停的对象。
    // 经进程级单例槽注册（WSAPI plugins.set_metrics_enabled 翻转同一实例）。
    agent_loop.add_tool_hook(nemesis_agent::hooks::metrics_plugin_slot().clone());
    // 工具事件观察 hook——有订阅通道才挂（None = CLI/exec_worker 零开销）。
    // 纯观察者（不改写/不拦截），事件由 gateway 建、web pump 消费。
    if let Some(tx) = shared.agent_event_tx.clone() {
        agent_loop.add_tool_hook(std::sync::Arc::new(
            nemesis_agent::tool_event_hook::ToolEventHook::new(tx, agent_loop.work_recorder()),
        ));
    }
    // （devtool-upgrade）：事件发送端注入 loop——/plan /build 切换
    // 发布 ModeChanged（前端徽标实时刷新）。与 hook 注入不同，这里 None 也
    // 照常注入（loop 侧对 None 静默跳过）——standalone 模式切换本身仍可用。
    agent_loop.set_agent_event_tx(shared.agent_event_tx.clone());
    // 会话执行统计 sink 注入——loop 内部计数（重试/校验
    // 失败/压缩成功）上抛聚合器。None（headless/未装配）= 不统计零开销。
    agent_loop.set_session_stats_sink(
        shared
            .session_stats
            .clone()
            .map(|a| a as std::sync::Arc<dyn nemesis_agent::SessionStatsSink>),
    );
    // present_plan 计划审批句柄——plan 模式下模型把写好的计划
    // 交给用户裁决（question broker 同款阻塞等答）；批准=共享 mode Arc 翻转
    // build + ModeChanged 广播，驳回/超时=批次尾消费 verdict 结构性终止本轮。
    // broker 未装配（CLI headless 无 Dashboard）= 工具调用诚实报错，注册不受影响。
    let plan_present_handle = Arc::new(nemesis_agent::loop_tools::PlanPresentHandle {
        broker: shared.question_slot.clone(),
        mode: agent_loop.mode_arc(),
        agent_event_tx: shared.agent_event_tx.clone(),
        verdicts: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
    });
    agent_loop.set_plan_present_handle(plan_present_handle.clone());
    // enable tool-result spill under <workspace>/logs/spill — oversized
    // results (>64k chars) land there whole with a locator in-conversation.
    // 迁回 workspace（设计指定位置）：定位器必须在
    // restrict_to_workspace 限制范围内，agent 的 file 工具才读得到全文；
    // 旧 <home>/logs/spill 违反该约束，见 resolve_spill_dir_in_workspace。
    let spill_root =
        nemesis_path::resolve_spill_dir_in_workspace(&nemesis_path::workspace_dir(&shared.home));
    agent_loop.set_spill_root(spill_root.clone());
    // retention: startup sweep + daily midnight task. retention=0 disables.
    let spill_retention = cfg.agents.defaults.spill_retention_days.max(0) as u64;
    if spill_retention > 0 {
        let deleted = nemesis_agent::spill::cleanup_expired(&spill_root, spill_retention);
        if deleted > 0 {
            info!(
                deleted,
                retention_days = spill_retention,
                "[AgentFactory] spill startup cleanup (TTL={}d)",
                spill_retention
            );
        }
    }
    spawn_daily_spill_cleanup(spill_root, shared.home.join("config.json"));
    // (.2): skills-catalog digest injection — same loader the
    // skills_list tools use, so the advertised catalog matches reality.
    if let Some(ref loader) = shared.skills_loader {
        agent_loop.set_skills_loader(loader.clone());
    }
    // workspace instruction chain (AGENTS.md/CLAUDE.md) rides the
    // same merged context-digest injection.
    agent_loop.set_workspace_root(shared.workspace_dir());
    // vault 域绑定 sidecar 权威路径（L26 修复 2026-11-11：与 CLI `vault
    // bind` 写入侧同点，dispatch 闸 primary 消费）。
    agent_loop.set_vault_domain_config_path(shared.vault_domain_config_path());
    // Full-review: snapshot role from config ("user" default; "system"
    // for strict chat templates).
    agent_loop.set_snapshot_role(&cfg.agents.defaults.snapshot_role);
    // 绑定全局急停状态（每次重建都重新绑到 SharedResources 上的同一个 Arc，
    // 所以急停状态在 agent stop/start 后自动保持）。
    agent_loop.set_estop(shared.estop.clone());
    // 语音 realtime：热态装载（幂等，重建即从 config.chat.json 重刷）
    // + voice_prompt 旗注入（Arc<AtomicBool> 镜像，apply_realtime_config 同步刷）。
    nemesis_web::voice_relay::init_realtime_hot(&shared.workspace_dir().join("config"));
    agent_loop.set_voice_prompt(nemesis_web::voice_relay::voice_prompt_flag());
    // （重评估）turn WAL：`agents.turn_wal`（默认 true）→
    // 打开 `<workspace>/logs/turn_wal/wal.jsonl` 并注入 loop；dispatch 三态
    // 写入 + run_bus 启动恢复（断头工具调用补 interrupted + 续轮）都吃这个
    // Arc。打开失败（盘满/权限）诚实降级为无 WAL——warn 不阻断装配。
    // 作用域：仅主 loop（项目 loop 的会话经 resolve_session_loop 路由，
    // 续轮消息会错落主 loop 工作区，v1 明确不覆盖——见交付报告诚实边界）。
    if cfg.agents.turn_wal {
        let wal_dir = shared.workspace_dir().join("logs").join("turn_wal");
        match nemesis_agent::turn_wal::TurnWal::open(&wal_dir) {
            Ok(wal) => agent_loop.set_turn_wal(std::sync::Arc::new(wal)),
            Err(e) => warn!("[AgentFactory] turn WAL 打开失败（降级为无 WAL，不落盘不续轮）: {e}"),
        }
    }
    // 件4：纪律闭环—— 总开关（默认 false =
    // 全链路惰性：钩子不注册、/discipline 提示未启用）。开 = 注入共享态 +
    // 注册闸/证伪钩子；闸在 user pre hooks **首位**（先于方言桥；metrics/
    // 事件观察者本就不拦）。estop 同点装配（触发 = fail-open 整体停用）。
    if cfg.agents.discipline.enabled {
        let discipline_state = nemesis_agent::discipline::DisciplineState::new(
            true,
            shared.workspace_dir(),
            cfg.agents.defaults.restrict_to_workspace,
        );
        discipline_state.set_estop(shared.estop.clone());
        agent_loop.add_tool_hook(Arc::new(
            nemesis_agent::discipline::DisciplineGateHook::new(discipline_state.clone()),
        ));
        // BUG-20261106-042：证伪 hook 带 security 句柄——模型 authored 的
        // falsification_cmd 执行前过规则层安全管线（与主循环 exec 同源闸面）。
        // security feature 关闭 = 旧行为（无检查可装，诚实降级）。
        #[cfg(feature = "security")]
        agent_loop.add_lifecycle_hook(Arc::new(
            nemesis_agent::discipline::DisciplineFalsificationHook::new_with_security(
                discipline_state.clone(),
                shared.security_plugin.clone(),
            ),
        ));
        #[cfg(not(feature = "security"))]
        agent_loop.add_lifecycle_hook(Arc::new(
            nemesis_agent::discipline::DisciplineFalsificationHook::new(discipline_state.clone()),
        ));
        agent_loop.set_discipline(discipline_state);
    }
    // 对话生成（2026-09-22）：工作流引擎引用——workflow_edit 命名会话渲染
    // 当前定义块用（workflow 工具经 SharedToolConfig 持有引擎 Arc，
    // 此处只服务 loop 的 section 渲染）。
    #[cfg(feature = "workflow")]
    if let Some(ref wf_engine) = shared.workflow_engine {
        agent_loop.set_workflow_engine(wf_engine.clone());
    }
    // (devtool-upgrade): `agents.small_model` 小模型杂务通道——
    // 手动 /compact 的摘要走小模型省 token（自动压缩维持主模型，质量敏感）。
    // 模型缺失/解析失败 = warn + 跳过（loop 侧诚实回退主模型），绝不阻断
    // gateway 启动。集群 loop（build_cluster_agent_loop）刻意不装配：
    // /compact 只存在于主 gateway 入口。
    if let Some(small_ref) = cfg
        .agents
        .small_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match nemesis_config::resolve_model_config(&cfg, small_ref) {
            Ok(resolution) => {
                let small_factory_cfg = nemesis_providers::factory::FactoryConfig {
                    proxy: resolution.proxy.clone(),
                    stream_mode: resolution.stream_mode.clone(),
                    cache_breakpoints: resolution.cache_breakpoints,
                    extra_body: resolution.extra_body.clone(),
                    llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
                    api_key: resolution.api_key.clone(),
                    api_base: resolution.api_base.clone(),
                    workspace: shared.workspace_dir().to_string_lossy().to_string(),
                    connect_mode: resolution.connect_mode.clone(),
                    protocol: resolution.protocol.clone(),
                    timeout_secs: resolution.timeout_secs,
                    account_id: String::new(),
                    headers: HashMap::new(),
                };
                match nemesis_providers::factory::create_provider(&small_factory_cfg) {
                    Ok(provider) => {
                        let adapter = ProviderAdapter::new(provider, resolution.model_name.clone());
                        agent_loop
                            .set_small_model(Some((Arc::new(adapter), resolution.model_name)));
                        info!(
                            "[AgentFactory] Small model '{}' wired for manual compact summarization",
                            small_ref
                        );
                    }
                    Err(e) => warn!(
                        "[AgentFactory] agents.small_model '{}': provider create failed ({}); manual compact falls back to the main model",
                        small_ref, e
                    ),
                }
            }
            Err(e) => warn!(
                "[AgentFactory] agents.small_model '{}' not resolvable ({}); manual compact falls back to the main model",
                small_ref, e
            ),
        }
    }
    // hooks.json 方言层——workspace config 目录下有 hooks.json
    // 就加载并挂上（工具钩子 + 生命周期钩子）。集群 agent 不挂（远端节点跑
    // 本地用户任务，hook 拦截语义不跨节点复制——挂账决策）。加载失败 =
    // warn + 跳过（fail-open，见 cc_hooks::load_from_dir）。
    let mut cc_bridge: Option<std::sync::Arc<nemesis_agent::cc_hooks::CcHookBridge>> = None;
    #[allow(unused_assignments)]
    let mut cc_bridge_for_daily: Option<std::sync::Arc<nemesis_agent::cc_hooks::CcHookBridge>> =
        None;
    // 路径收编：hooks.json 落位 <workspace>/config/hooks.json
    // （nemesis-path 真相源）；legacy <home>/config/hooks.json copy-once 迁移。
    let ws_config_dir = nemesis_path::workspace_config_dir(&shared.workspace_dir());
    nemesis_agent::cc_hooks::migrate_legacy_home_hooks_config(
        &shared.home.join("config"),
        &ws_config_dir,
    );
    if let Some(bridge) =
        nemesis_agent::cc_hooks::CcHookBridge::load_from_dir(&ws_config_dir, shared.workspace_dir())
    {
        // SessionEnd 触发用（启动清理删除的会话逐个触发）。
        cc_bridge = Some(std::sync::Arc::clone(&bridge));
        cc_bridge_for_daily = Some(std::sync::Arc::clone(&bridge));
        // PreCompact/PostCompact 触发用（loop.rs 压缩流水线经 cc_bridge 调桥）。
        agent_loop.set_cc_hooks_bridge(std::sync::Arc::clone(&bridge));
        bridge.register(&agent_loop);
    }
    // Todo 收尾提醒：终答案接受后清单还有未完成项 → 注入
    // 一次性提醒再答一轮（lifecycle hook；stop_hook_active 自限每 turn
    // 一次 + MAX_TURN_END_CONTINUES 封顶 fail-open）。注册在用户 hooks 桥
    // 之后——first Continue wins，用户脚本优先于内建提醒。workspace 与
    // todowrite 工具同根（shared.workspace_dir()，见 TodoToolConfig 装配）。
    agent_loop.add_lifecycle_hook(std::sync::Arc::new(nemesis_agent::TodoCloseoutHook::new(
        shared.workspace_dir(),
    )));

    // 件3 层2：hooks 桥 + security 插件同在时，
    // 一次性延迟接线审批观察 manager。网关在 loop 构建**之后**（runtime
    // 装配期）才 set_approval_manager——构建时同步 get→wrap→set 会被覆写；
    // 见 spawn_approval_observer_once。
    #[cfg(feature = "security")]
    if let (Some(bridge), Some(plugin)) = (cc_bridge.as_ref(), shared.security_plugin.as_ref()) {
        spawn_approval_observer_once(std::sync::Arc::clone(bridge), std::sync::Arc::clone(plugin));
    }

    // 5. Session store (disk-persisted — new instance, same directory).
    {
        let sess_dir = common::sessions_dir(&shared.home);
        // Migrate legacy single-session main → multi-session legacy format
        // (idempotent + best-effort) BEFORE SessionStore loads from disk.
        nemesis_agent::session::SessionStore::migrate_legacy_main(&sess_dir);
        let store = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
            &sess_dir,
        ));
        // Startup cleanup: remove sessions older than 7 days.
        // 改用 detailed 版（返回被删会话的原始 session key），
        // 逐个触发方言 SessionEnd（观察型）。
        let removed = store.cleanup_old_sessions_detailed(7);
        let deleted = removed.len();
        if deleted > 0 {
            info!(
                deleted,
                "[AgentFactory] Main SessionStore startup cleanup (TTL=7d)"
            );
            // 在 runtime 内才 spawn（测试的同步构建路径安全跳过）。
            if let (Some(bridge), Ok(handle)) =
                (cc_bridge.as_ref(), tokio::runtime::Handle::try_current())
            {
                let bridge = std::sync::Arc::clone(bridge);
                let sessions = removed.clone();
                handle.spawn(async move {
                    for session_key in &sessions {
                        bridge.on_session_end(session_key, "expired").await;
                    }
                });
            }
        }
        // Daily midnight cleanup. Spawns a task that sleeps until the next local
        // midnight, then runs cleanup_old_sessions(7), and loops forever.
        // Best-effort: if the runtime shuts down, the task is cancelled.
        spawn_daily_cleanup(
            store.clone(),
            "Main",
            cc_bridge_for_daily.as_ref().map(std::sync::Arc::clone),
        );
        agent_loop.set_session_store(store);
        info!(
            "[AgentFactory] Session store initialized: {}",
            sess_dir.display()
        );
    }

    // 6. Workspace state manager (disk-based — new instance).
    {
        let state_mgr = nemesis_state::workspace_state::WorkspaceStateManager::new(&workspace_dir);
        agent_loop.set_state_manager(state_mgr);
    }

    // 7. Build tool config + register all tools + enable MCP.
    // 第二返回值是 spawn 共享槽原件——Arc<AgentLoop> 定型后注入闭包。
    let (mut tool_config, spawn_slot) = build_shared_tool_config(
        shared,
        &cfg,
        &model_name,
        Some(agent_loop.mcp_tool_snapshot()),
    );
    // present_plan 工具供给（plan 模式白名单 + SharedToolConfig
    // 注册同源；loop 侧 verdict 消费槽已装配）。
    tool_config.plan_present = Some(plan_present_handle);
    // 写后钩子路径解析边界与工具表同一 Arc（单一
    // 权威定义——format-on-save / 诊断回灌与执行器同一落点）。
    if let Some(ref boundary) = tool_config.workspace_boundary {
        agent_loop.set_workspace_boundary(boundary.clone());
    }
    // present_files 工具供给——交付物流水桶与 loop 共享同一
    // Arc（工具登记、assistant 回复落盘时 drain 进 chat_log deliverables）。
    tool_config.turn_deliverables = Some(agent_loop.turn_deliverables_bucket());

    // Executor separation: if enabled, MOVE tools are wrapped in RemoteExecutorTool
    // and dispatched to a per-call child process. sandbox=false → stdio transport
    // (Layer 1). sandbox=true → named-pipe transport + Start.exe wrap (real
    // Sandboxie box, Layer 2); requires the `sandbox` feature + `nemesisbot sandbox
    // install`. When the `sandbox` feature is compiled out, sandbox=true is ignored.
    //
    // the readiness/probe/degrade logic now lives in `exec_world:
    // build_executor_channel` (single source of truth) — the workflow engine's
    // ExecutionWorld uses the exact same assembly, so the agent tool layer and
    // workflow scripts share one switch chain (executor.enabled / executor.sandbox).
    let executor_channel = crate::exec_world::build_executor_channel(
        &shared.home,
        &workspace_dir,
        shared.config_store.handle(),
    )?;
    // 主 loop 常驻路由——解析闭包按 kind=main 现查 agents 表
    // （main 记录 exec_root=workspace，与 per-call 通道同一 cwd 锚语义）。
    let persistent_route = persistent_route_for(
        shared,
        persistent_resolve_by(|rec| rec.kind == nemesis_taskstore::agents::kind::MAIN),
    );
    register_tools_and_mcp(
        &mut agent_loop,
        shared,
        &tool_config,
        executor_channel.as_ref(),
        persistent_route.as_ref(),
    );

    // Stash the memory executor so the gateway can attach an approval gate
    // post-construction (agent memory_store/forget require interactive
    // approval, never bypassed by YOLO/auto).
    #[cfg(feature = "memory")]
    {
        if let Some(ref exec) = tool_config.memory_executor {
            agent_loop.set_memory_executor(exec.clone());
        }
    }

    // .1 (sixth batch): wire the auto-inject channel — the memory manager
    // (read-only retrieval, deliberately NOT the approval-gated executor)
    // plus the auto_inject/top_k flags from config.enhanced_memory.json.
    // Default (auto_inject=false) keeps message output byte-identical.
    // `shared.memory_manager` is None when `memory.enabled=false` in
    // config.json → no retrieval, injection stays off regardless of flags.
    #[cfg(feature = "memory")]
    {
        let config_dir = shared.workspace_dir().join("config");
        let (auto, top_k) = {
            let emb = nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir);
            (emb.auto_inject, emb.auto_inject_top_k)
        };
        agent_loop.set_memory_inject(shared.memory_manager.clone(), auto, top_k);
        if auto {
            info!("[AgentFactory] memory auto-inject enabled (top_k={top_k})");
        }
    }
    #[cfg(not(feature = "memory"))]
    {
        // Flags-only stub (no manager exists without the memory crate);
        // injection is structurally a no-op here regardless.
        agent_loop.set_memory_inject(false, 3);
    }

    // 8. Register ClusterRpcTool (using shared call_fn + peers_fn).
    if let (Some(config), Some(call_fn)) = (&shared.cluster_rpc_config, &shared.cluster_rpc_call_fn)
    {
        let mut cluster_rpc_tool = nemesis_agent::ClusterRpcTool::new(config.clone());
        cluster_rpc_tool.set_rpc_call_fn(call_fn.clone());
        if let Some(ref peers_fn) = shared.cluster_peers_fn {
            cluster_rpc_tool.set_peers_fn(peers_fn.clone());
        }
        if let Some(ref auto_fn) = shared.cluster_auto_target_fn {
            cluster_rpc_tool.set_auto_target_fn(auto_fn.clone());
        }
        cluster_rpc_tool.set_enabled(true);
        // Store the enabled flag for dynamic cluster start/stop.
        *shared.cluster_rpc_enabled.write() = Some(cluster_rpc_tool.enabled_arc());
        agent_loop.register_tool("cluster_rpc".to_string(), Box::new(cluster_rpc_tool));
        info!("[AgentFactory] cluster_rpc tool registered (enabled=true)");
    }

    // 8b. 全自动流转 /：board_issue 工具（主 agent 专属——「对 master
    // 说一句话建单」入口）。store 未装配（run/acp headless、board feature
    // 裁掉、DB 打开失败）= 不注册，零影响。worker cluster agent 不装：
    // worker 本地 board.db 是 dashboard 视图，建单落不到 master 权威库。
    #[cfg(all(feature = "board", feature = "cluster"))]
    if let Some(ref board_store) = shared.board_store {
        agent_loop.register_tool(
            crate::board_issue_tool::TOOL_NAME.to_string(),
            Box::new(crate::board_issue_tool::BoardIssueTool::new(
                board_store.clone(),
                shared.board_cluster.clone(),
                shared.board_moderator_slot.clone(),
                shared.board_home.clone(),
                shared.board_event_hub.clone(),
            )),
        );
        // 评审取证自取（evidence-on-demand）：档案根围栏只读。评审链的
        // EvidenceOnly/ReadOnly 白名单按名收窄供给；注册在这里意味着主对话
        // （big 档全量供给）也能按单号读项目档案——只读且围栏内，无害。
        agent_loop.register_tool(
            crate::board_evidence_tool::TOOL_NAME.to_string(),
            Box::new(crate::board_evidence_tool::BoardEvidenceTool::new(
                board_store.clone(),
            )),
        );
        info!("[AgentFactory] board_issue tool registered (master main agent)");
        info!("[AgentFactory] board_evidence_read tool registered (master main agent)");
    }

    // 8c. （WASM 插件框架）：插件工具桥注册（主 agent 专属——项目 loop
    // 工具子集不带、cluster worker agent 不装：远端节点不跑本地插件）。
    // manager 未装配（plugins.wasm.enabled=false / feature 裁掉 / 构建失败）
    // = 不注册，零影响。逐已注册 tool 插件 register_tool（plugin.<slug>.<base>
    // 全名）+ declare 操作类型（声明式 ABAC——声明后安全 8 层照跑）。
    #[cfg(feature = "plugins-wasm")]
    if let Some(ref pm) = shared.plugin_manager {
        let n = crate::plugin_bridge::register_plugin_tools(&mut agent_loop, pm);
        if n > 0 {
            info!("[AgentFactory] {} wasm plugin tool(s) registered", n);
        }
    }

    // 8d. 工具接管层：接管声明应用（内置名 → wasm 执行体，
    // wrapper 双态保降级）。在 8c 之后：先有插件工具供给面，再换内置执行
    // 体。总闸 config `plugins.wasm.takeover`（缺省 true）；热同步由
    // gateway ToolSync hook 翻侧表开关（注册表对象恒定，零 prompt cache
    // 抖动）。主 agent 专属——项目 loop 不经此段（同 8c 边界）。
    #[cfg(feature = "plugins-wasm")]
    if let Some(ref pm) = shared.plugin_manager {
        let gate = crate::plugin_bridge::read_runtime_config(&shared.home).takeover;
        let n = crate::plugin_bridge::apply_takeovers(&agent_loop, pm, gate);
        if n > 0 {
            info!(
                "[AgentFactory] {} builtin tool(s) taken over by wasm plugins",
                n
            );
        }
    }

    // 9. Continuation manager (disk-persisted — new instance).
    {
        let cont_mgr = Arc::new(nemesis_agent::ContinuationManager::with_disk_store(
            &workspace_dir,
        ));
        agent_loop.set_continuation_manager(cont_mgr.clone());
        // rpc_cache TTL: continuation snapshots whose callback
        // never arrives (peer died, task lost) used to accumulate in
        // {workspace}/cluster/rpc_cache/ forever. Same 7-day TTL as the
        // SessionStore: startup sweep + daily midnight.
        spawn_rpc_cache_cleanup(cont_mgr);
    }

    // 10. Inject shared Arc references.
    #[cfg(feature = "forge")]
    {
        if let Some(ref forge) = shared.forge {
            agent_loop.set_forge(forge.clone());
        }
    }
    #[cfg(feature = "security")]
    {
        if let Some(ref plugin) = shared.security_plugin {
            agent_loop.set_security_plugin(plugin.clone());
        }
        // 免打扰运行时态挂主 loop（doom-loop escalation 自动批准臂）；
        // 槽本体在 security_setup 全局安装，未装配 = None = 现行为。
        if let Some(state) = nemesis_web::handlers::security::approval_auto_state() {
            agent_loop.set_approval_auto_approve(state);
        }
    }
    // Checkpoint store (edit safety net): snapshots writer-tool file changes so
    // a rewind can restore them. One per agent loop under
    // {workspace}/logs/checkpoints/（统一收编进 logs 家族；
    // 旧.checkpoints/ 散放目录一次性 move 迁移）。
    {
        let ws = shared.workspace_dir();
        // 旧散放目录迁移（一次性）：存在且新位缺失 → 整目录 move。
        let legacy_cp = ws.join(".checkpoints");
        let new_cp = nemesis_path::resolve_checkpoints_dir_in_workspace(&ws);
        if legacy_cp.is_dir() && !new_cp.exists() {
            if let Some(new_parent) = new_cp.parent() {
                let _ = std::fs::create_dir_all(new_parent);
            }
            let _ = std::fs::rename(&legacy_cp, &new_cp);
        }
        let store = Arc::new(nemesis_agent::checkpoint::CheckpointStore::new(
            Some(new_cp),
            ws,
        ));
        agent_loop.set_checkpoint_store(store);
    }
    if let Some(ref mgr) = shared.observer_manager {
        agent_loop.set_observer_manager(mgr.clone());
    }
    if let Some(ref ds) = shared.data_store {
        agent_loop.set_data_store(ds.clone());
    }
    agent_loop.set_channel_manager(shared.enabled_channels.clone());

    // 11. Update Forge's LLM provider (old model may have been deleted).
    //     set_provider cascades to reflector + pipeline + learning_engine.
    #[cfg(feature = "forge")]
    {
        if let Some(ref forge) = shared.forge {
            // S8：Forge 评审/反思走 aux 账本（shared.data_store 归因）。
            let bridge = ForgeProviderBridge::new(provider_arc.clone(), model_name.clone())
                .with_data_store(shared.data_store.clone());
            forge.set_provider(Arc::new(bridge));
            // A 线行为回归执行器注入（同点级联到 pipeline 的回归闸）。
            let test_runner: Arc<dyn nemesis_forge::skill_tests::SkillTestRunner> = Arc::new(
                crate::forge_test_runner::BotSkillTestRunner::new(shared.home.clone()),
            );
            forge.set_skill_test_runner(test_runner.clone());
            // Stage 2 judge 层杂务模型回落链（与 CLI/WSAPI 的 judge 口径
            // 同源：judge_model → small_model → 主模型；chat 现场解析，
            // 模型热切即时生效）。
            forge.set_judge_caller(Arc::new(
                crate::forge_test_runner::ChainJudgeCaller::new(
                    shared.home.clone(),
                    crate::common::workspace_path(&shared.home),
                )
                // S8：judge 调用落 aux 账本（与上方 bridge 同源归因）。
                .with_data_store(shared.data_store.clone()),
            ));
            // web 槽（forge.test WSAPI 消费；PROJECTS_BRIDGE 同款槽模式）。
            nemesis_web::handlers::forge::set_forge_test_runner(test_runner);
            // B 线画像 sweep（OnceLock 一次守卫；enabled=false 时 sweep 内部 no-op）。
            // S8：反思通道的 LLM 调用同落 aux 账本（data_store 随任务带出）。
            crate::profile_service::ensure_sweep_spawned(
                shared.home.clone(),
                shared.data_store.clone(),
            );
            info!(
                "[AgentFactory] Forge provider updated to model {}",
                model_name
            );
        }
    }

    info!(
        model = %model_name,
        tools = agent_loop.tool_count(),
        "[AgentFactory] AgentLoop built successfully"
    );

    // Arc 定型后注入 sub-agent spawn 闭包（Weak::downgrade 需要 Arc）。
    // 闭包持有 bus 引用（后台任务完成回灌经 subagent_continuation 发布）。
    let agent_loop = Arc::new(agent_loop);
    // checkpoint gc 装配（config `checkpoint.gc`，
    // 默认关；enabled 才挂钩子——含 undo/redo 栈下界闭包，Weak 指 loop 防环）。
    // BUG-95（回头检查发现）：cluster agent 的 checkpoint store 与
    // 主 store 是独立实例**共享同一影子库目录**——GitBackend 的 repo Mutex 是
    // 实例级的，本 store gc 的 objects/ 换名窗口锁不住集群写入者；且保留集
    // 闭包只接主 loop 的 undo/redo 栈，集群栈引用的 tree 会被误清。故
    // config.cluster.enabled 时（config=统一定义）**不装配 gc**——宁可放弃
    // gc 也不引入双写入者竞态。项目 loop（build_agent_loop_from_spec）影子库
    // 独立（logs/project_checkpoints/<key>），不受此闸影响；过度保守边：
    // cluster feature 未编译但 config 残留 enabled=true 时同样被闸（诚实方向：
    // 宁可少 gc，不可错删）。
    {
        let gc_cfg = cfg
            .checkpoint
            .as_ref()
            .map(|c| c.gc.clone())
            .unwrap_or_default();
        let cluster_shadows_shared = cfg.cluster.as_ref().is_some_and(|c| c.enabled);
        if gc_cfg.enabled && cluster_shadows_shared {
            info!(
                "[AgentFactory] checkpoint gc 保持关闭：集群已启用（主/集群 checkpoint store 共享影子库，双写入者下 gc 存在换名竞态与保留集盲区）"
            );
        } else if gc_cfg.enabled
            && let Some(store) = agent_loop.checkpoint_store()
        {
            let loop_weak = Arc::downgrade(&agent_loop);
            store.set_gc(
                gc_cfg,
                Arc::new(move || {
                    loop_weak
                        .upgrade()
                        .map(|l| l.undo_stack_tree_refs())
                        .unwrap_or_default()
                }),
            );
            info!("[AgentFactory] checkpoint gc enabled (keep_turns/keep_days per config)");
        }
    }
    // 专家面（2026-11-13）：名册 handle + provider 解析器注入 spawn 闭包
    //（None = 无专家面，第 11 参非空时闭包诚实报错）。reloader 纯数据件，
    // 与 loop provider 重建器同构造（home/workspace 锚定）。
    inject_spawn_fn(
        &agent_loop,
        &spawn_slot,
        &shared.bus,
        cc_bridge.clone(),
        shared.experts.as_ref().map(|s| s.handle()),
        Arc::new(ModelProviderReloader {
            home: shared.home.clone(),
            workspace: shared.workspace_dir(),
        }),
    );
    // 插件 invoker 重挂（每次重建都要——Weak 指旧 loop，重启后
    // upgrade 失败 = guest tool-invoke 全哑）。estop watch 联动在 init_agent
    // spawn 一次（watcher 是 manager 级任务，不随 loop 重建泄）。
    // 样板工程：循环策略桥同点装配（三策略槽接管 +
    // llm-complete 面注入——Weak 同样必须每次重建重指；无 policy 组件 =
    // 零装桥，槽维持内置哨兵）。
    #[cfg(feature = "plugins-wasm")]
    if let Some(ref pm) = shared.plugin_manager {
        pm.set_invoker(Arc::new(crate::plugin_bridge::GatewayHostInvoker::new(
            &agent_loop,
        )));
        let np = crate::plugin_bridge::register_policy_bridges(&agent_loop, pm, &shared.estop);
        if np > 0 {
            info!("[AgentFactory] {} wasm loop policy point(s) bridged", np);
        }
    }
    // codemode：code_run 脚本工具。编译闸 `codemode`
    // feature + 运行闸 `agents.codemode.enabled`（默认开，Dashboard
    // Tools 页可关）+ min_tier big 双闸（工具自带）。仅主 gateway loop 装配：
    // 项目 loop（固定子集，与插件工具同一挂账边界）与集群 agent 不含；
    // 脚本内 tool() 走完整 dispatch 瀑布（同 GatewayHostInvoker 桥形态），
    // code_run 自身按 exec 档显式声明操作类型（不声明会走未知名 fail-closed
    // = ProcessExec，行为等价但每调一次 warn 一次——显式声明去噪 + 分类明示）。
    // 装配期无条件构造 + set_loop + 注入 loop（热生效联动的前提——开关切换
    // 复用同一实例，check_config_reload 比对注册状态增删；timeout/memory 是
    // 构造期值，改键需 loop 重建）。
    // （android 上 rquickjs 依赖缺席→ feature 静默无模块，注册点同 cfg 收口）。
    #[cfg(all(feature = "codemode", not(target_os = "android")))]
    {
        #[cfg(feature = "security")]
        nemesis_security::types::declare_tool_operation(
            nemesis_agent::code_run::CODE_RUN_TOOL_NAME,
            nemesis_security::types::OperationType::ProcessExec,
        );
        let code_tool = Arc::new(nemesis_agent::code_run::CodeRunTool::new(
            cfg.agents.codemode.timeout_secs,
            cfg.agents.codemode.memory_limit_mb,
        ));
        code_tool.set_loop(Arc::downgrade(&agent_loop));
        agent_loop.install_code_run_tool(code_tool.clone());
        if cfg.agents.codemode.enabled {
            agent_loop.register_plugin_tool(
                nemesis_agent::code_run::CODE_RUN_TOOL_NAME.to_string(),
                code_tool.clone(),
            );
        }
        info!(
            enabled = cfg.agents.codemode.enabled,
            timeout_secs = cfg.agents.codemode.timeout_secs,
            memory_limit_mb = cfg.agents.codemode.memory_limit_mb,
            "[AgentFactory] codemode code_run assembled"
        );
    }
    // 委派持续操控：delegate 四工具（start/send/output/
    // stop）。运行闸 `agents.delegate.enabled`（默认关）+ CLI 在场探测
    // （claude/codex 至少一个在 PATH，都没有则四件套不注册——诚实缺位不
    // 空挂）。仅主 gateway loop 装配（项目 loop 固定子集、集群 worker 不
    // 混外部 CLI 面的同一决策）；delegate_start=ProcessExec、send/output/
    // stop=Internal 显式声明（types.rs 静态表已映射，这里声明去未知名
    // warn 噪声）。estop 联动：manager 挂 shared.estop 同一 Arc——急停
    // 触发 = 全部 delegate 关 stdin + 树杀（watcher manager 级 lazy spawn）。
    if cfg.agents.delegate.enabled {
        let claude_path = nemesis_agent::loop_tools::cli_delegation::find_cli_on_path("claude");
        let codex_path = nemesis_agent::loop_tools::cli_delegation::find_cli_on_path("codex");
        if claude_path.is_some() || codex_path.is_some() {
            #[cfg(feature = "security")]
            {
                nemesis_security::types::declare_tool_operation(
                    "delegate_start",
                    nemesis_security::types::OperationType::ProcessExec,
                );
                for name in ["delegate_send", "delegate_output", "delegate_stop"] {
                    nemesis_security::types::declare_tool_operation(
                        name,
                        nemesis_security::types::OperationType::Internal,
                    );
                }
            }
            let delegate_manager =
                Arc::new(nemesis_agent::delegate_registry::DelegateManager::new());
            delegate_manager.attach_estop(shared.estop.clone());
            // delegate_start 的 cwd 围栏与 exec 族同规吃 editor 开关
            // （Full Access）闸（resolve_cwd 同一语义；resolve_cwd 第二
            // 调用方接线）。security 裁剪/auditor 缺席 = None 旧行为。
            #[cfg(feature = "security")]
            let delegate_editor_gate = shared
                .security_plugin
                .as_ref()
                .and_then(|p| p.auditor().editor_access())
                .map(|e| e.fence_gate());
            #[cfg(not(feature = "security"))]
            let delegate_editor_gate =
                None::<std::sync::Arc<dyn Fn(&str, bool) -> bool + Send + Sync>>;
            agent_loop.register_plugin_tool(
                "delegate_start".to_string(),
                Arc::new(
                    nemesis_agent::delegate_registry::DelegateStartTool::new(
                        &shared.workspace_dir().to_string_lossy(),
                        cfg.agents.defaults.restrict_to_workspace,
                        cfg.agents.delegate.max_lifetime_secs,
                        claude_path,
                        codex_path,
                        cfg.agents.delegate.permission_mode.clone(),
                        delegate_manager.clone(),
                    )
                    .with_editor_gate(delegate_editor_gate),
                ),
            );
            agent_loop.register_plugin_tool(
                "delegate_send".to_string(),
                Arc::new(nemesis_agent::delegate_registry::DelegateSendTool::new(
                    delegate_manager.clone(),
                )),
            );
            agent_loop.register_plugin_tool(
                "delegate_output".to_string(),
                Arc::new(nemesis_agent::delegate_registry::DelegateOutputTool::new(
                    delegate_manager.clone(),
                )),
            );
            agent_loop.register_plugin_tool(
                "delegate_stop".to_string(),
                Arc::new(nemesis_agent::delegate_registry::DelegateStopTool::new(
                    delegate_manager,
                )),
            );
            info!("[AgentFactory] delegate tools registered (start/send/output/stop, A3)");
        } else {
            info!(
                "[AgentFactory] agents.delegate.enabled but no claude/codex CLI on PATH — \
                 delegate tools not registered"
            );
        }
    }
    // selfupdate 工具面：selfupdate_check（只读）+
    // selfupdate_apply（提案——人类闸=编排内 CRITICAL 审批卡，「总是允许」
    // 恒忽略）。主 loop 独占（项目 loop 不该重启整个 gateway 杀掉兄弟项目；
    // 集群 worker 不参与版本治理）；操作类型显式声明（check=NetworkRequest、
    // apply=SystemInstall），不声明走未知名 fail-closed 行为等价但每次 warn。
    #[cfg(feature = "selfupdate")]
    {
        #[cfg(feature = "security")]
        {
            nemesis_security::types::declare_tool_operation(
                crate::selfupdate_tool::CHECK_TOOL_NAME,
                nemesis_security::types::OperationType::NetworkRequest,
            );
            nemesis_security::types::declare_tool_operation(
                crate::selfupdate_tool::APPLY_TOOL_NAME,
                nemesis_security::types::OperationType::SystemInstall,
            );
        }
        agent_loop.register_plugin_tool(
            crate::selfupdate_tool::CHECK_TOOL_NAME.to_string(),
            Arc::new(crate::selfupdate_tool::SelfupdateCheckTool),
        );
        agent_loop.register_plugin_tool(
            crate::selfupdate_tool::APPLY_TOOL_NAME.to_string(),
            Arc::new(crate::selfupdate_tool::SelfupdateApplyTool),
        );
        info!("[AgentFactory] selfupdate tools registered (check + apply, L9)");
    }
    // 行动代理：connector_find / connector_invoke 动态工具
    // 对 + 独占隐藏表供给器——装配细节收敛到
    // [`register_connector_governance`]（治理补全 起主 loop 与统一工厂
    // 项目/selfbuilt loop 共用同一治理面装配）。
    #[cfg(all(feature = "security", feature = "connectors"))]
    register_connector_governance(&agent_loop, shared);
    // 紫微系统.2：profile.tools[] → 动态工具注册（poll_report 可调；
    // call_fn/hook_capture 诚实报 实装中）。config 闸同 WSAPI 门关；
    // profile 新增后经 `ziwei.refresh` 幂等重注册（先清 zw_* 再装）。
    // 半热加载失配闸（BUG-1）：工具注册在 agent 构建时读 fresh
    // config，会话管理器却只在 gateway 进程启动时装进 OnceLock 槽——config
    // 改 true 后只重启 agent 会出现「工具可见但调用必败」。manager 不在场
    // = 诚实不注册 + loud warn 说清重启要求（headless run/acp 形态同理）。
    #[cfg(feature = "ziwei")]
    if cfg.ziwei.clone().unwrap_or_default().enabled {
        if nemesis_ziwei::session::global().is_none() {
            warn!(
                "[AgentFactory] config ziwei.enabled=true 但会话管理器未装配（gateway 进程启动时 config 为 false，或非 gateway / 非 Windows 形态）：跳过 ziwei 工具注册；修改配置后需重启 gateway 进程才能生效"
            );
        } else {
            let n =
                crate::ziwei_tools::register_profile_tools(&agent_loop, &shared.workspace_dir());
            crate::ziwei_tools::set_current_loop(agent_loop.clone());
            // 配置期向导元工具（.8）：LLM 经 {action, params} 驱动 SetupManager。
            crate::ziwei_tools::register_bridge_tool(&agent_loop, &shared.workspace_dir());
            if n > 0 {
                info!("[AgentFactory] ziwei dynamic tools registered ({n})");
            }
        }
    }
    // (devtool-upgrade)：workspace fs watcher——外部编辑下一轮
    // 以 <external_changes> 注记浮出（指令链文件走 digest 失效锚点）。句柄
    // 活在 AgentLoop 内、随其销毁；启动失败 warn 一次后禁用，不阻断装配。
    // 集群 agent 不接（远端节点跑本地用户任务，watcher 语义不跨节点复制——
    // 与 cc_hooks 同一挂账决策）。
    // （devtool-upgrade）：workspace fs watcher——指令链失效锚 +
    // 外部变更提示注入。启动失败在内部 warn 一次后放弃（不拖累 loop）。
    // 集群 agent 不挂 watcher（与 cc_hooks 同一挂账决策）。
    let _ = AgentLoop::start_fs_watcher(&agent_loop, &cfg.agents.fs_watcher);
    // ASM-08：装配自检——关键件 None 即启动 loud fail（用户裁决 =甲）。
    assert_gateway_critical_wiring(&agent_loop, shared, "主 gateway loop")?;
    Ok(agent_loop)
}

// ---------------------------------------------------------------------------
// Shared: tool registration + MCP
// ---------------------------------------------------------------------------
