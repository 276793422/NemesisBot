// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 agent_factory.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

#[cfg(feature = "cluster")]
use super::*;

/// Build a cluster agent loop (standalone mode, no bus).
///
/// Returns `(AgentLoop, AgentConfig)` — the AgentLoop for running tasks,
/// and the AgentConfig for creating per-task AgentInstance (carries system_prompt identity).
///
/// Shares the same tool set and MCP as the main agent.
///
/// 与主 agent 的差异清单（ASM-07 重写 ——旧清单声称「不接
/// session store / continuation manager / data store / security plugin」，
/// 其中三项早已接线、文档烂掉失去「有意排除记录」效力，正是漏接温床）：
/// - Standalone mode（`AgentLoop::new` 而非 `new_bus`；出站走集群回调）
/// - **接**：security 8 层（ASM-01，与主实例共享同一 plugin Arc）/
///   estop / workspace_root（F）/ tier+config_path / pricing（ASM-03）/
///   data store（①）/ session store（`sessions/cluster/` 独立目录）/
///   continuation manager（ASM-06，3-hop 链路里 B 端对下一跳就是 A 端）/
///   memory 审批闸（ASM-02，desktop+memory+security feature 时经
///   approval_slot）/ skills_loader / snapshot_role / spill / lsp /
///   checkpoint / ToolEventHook / forge 依赖（ASM-07）
/// - **有意不接**（均有因，勿当漏接「补齐」）：
///   - state manager：唯一消费点是 record_last_channel 崩溃恢复（主 loop
///     概念），装配只会在工作区急切建 state/
///   - channel manager：出站不走过路通道，经集群回调回 master
///   - commands_path（自定义 slash）：主 gateway 入口专属
///   - memory_inject（.1 自动注入）：远端 peer 任务上下文不得混入本地
///     用户私人记忆（与主 loop hooks 豁免同策略，见 注释）
///   - spawn 槽：v1 边界，sub-agent 只在主 gateway loop 开放
///   - rpc_cache TTL 清扫 / session 清扫：主 loop 侧唯一持有，不重复起
/// - Has its own observer_manager with ClusterRequestLoggerObserver
///   (writes LLM details to cluster_logs/{device_id}/{task_id}/)
/// - Has cluster reference (for cluster_rpc tool to work)
/// - System prompt loaded from `workspace/cluster/IDENTITY.md` + `SOUL.md`
/// - 工作区写围栏强制 restrict=true（ASM-04， 裁决——B 端执行远端半可信
///   任务，围栏不比本地可信会话松）
#[cfg(feature = "cluster")]
pub fn build_cluster_agent_loop(
    shared: &Arc<SharedResources>,
    cluster: Arc<nemesis_cluster::cluster::Cluster>,
) -> Result<(
    Arc<nemesis_agent::r#loop::AgentLoop>,
    nemesis_agent::types::AgentConfig,
    Option<Arc<crate::cluster_request_logger_observer::ClusterRequestLoggerObserver>>,
)> {
    use nemesis_agent::r#loop::AgentLoop;

    // 1. Re-read config.json from disk.
    let config_path = shared.home.join("config.json");
    let cfg = nemesis_config::load_config(&config_path)
        .map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))?;

    // 2. Resolve LLM model → create provider.
    let llm_ref = nemesis_config::get_effective_llm(Some(&cfg));
    // 装配守卫④：同 build_agent_loop——utility/classifier
    // 条目 loud 拒绝走 NullProvider 降级臂。
    let assembly = match nemesis_config::ensure_conversation_role(&cfg, &llm_ref) {
        Err(e) => Err(e),
        Ok(()) => nemesis_config::resolve_model_config(&cfg, &llm_ref).map_err(|e| e.to_string()),
    };
    let (resolution, _resolve_degraded) = match assembly {
        Ok(r) => (r, None),
        Err(e) => {
            warn!(
                "[AgentFactory] Failed to resolve model '{}': {} — 无 LLM 降级装配（NullProvider）",
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
    // 双击直启 goal：集群 agent 装配同样 or_null（worker 无模型时 loop 照建，
    // 派来的任务诚实报错回传，节点不因缺模型掉线）。
    let (provider, create_warn) = nemesis_providers::factory::create_provider_or_null(&factory_cfg);
    if let Some(w) = create_warn {
        warn!(
            "[AgentFactory] Cluster agent provider create failed: {} — NullProvider 装配",
            w
        );
    }
    // 默认跟随 wrapper（方案A）：不再把装配期 provider 实例烘焙
    // 进 loop——dashboard 热切默认模型后（含 NullProvider 配好 key 恢复），
    // 同一 loop 经槽委派立即用新默认，无需重启。这正是 09-22 集群事故
    // （切模型后节点仍报「启动时装配失败」）的根修点。
    let provider_arc: Arc<dyn nemesis_providers::router::LLMProvider> =
        nemesis_providers::default_slot::default_following(
            provider,
            &model_name,
            &factory_cfg.llm_ref,
        );

    // 3. Load cluster system prompt from workspace/cluster identity files
    //    （稳定前缀：IDENTITY → SOUL → EXPERTISE→ worker 契约
    //    （worker_discipline 闸，读 config.cluster.json）→ Workspace 行）。
    let cluster_app = nemesis_cluster::config_loader::load_app_config(&shared.workspace_dir());
    let system_prompt = load_cluster_system_prompt(&shared.home, cluster_app.worker_discipline);

    // 4. Create AgentConfig + AgentLoop (standalone mode, no bus).
    // max_turns 跟随 agents.defaults.max_tool_iterations（根修：
    // 此前硬编码 60——config 改值/重启装配都不生效，编码单 60 轮必撞暂停墙；
    // 0=不限与主 loop/project loop 同语义）。
    let config = nemesis_agent::types::AgentConfig {
        model: model_name.clone(),
        system_prompt,
        max_turns: if cfg.agents.defaults.max_tool_iterations <= 0 {
            0
        } else {
            cfg.agents.defaults.max_tool_iterations as u32
        },
        ..Default::default()
    };
    let adapter = ProviderAdapter::new(provider_arc, model_name.clone());
    let mut agent_loop = AgentLoop::new(Box::new(adapter), config.clone());

    // 5. Set cluster reference (enables cluster_rpc tool).
    // （clone 而非 move：cluster Arc 还要给 board_discuss 工具持有。）
    agent_loop.set_cluster(cluster.clone() as Arc<dyn std::any::Any + Send + Sync>);
    // 绑定全局急停状态（集群 agent 同样吃急停——peer_chat 跑完整工具链，不能漏）。
    agent_loop.set_estop(shared.estop.clone());
    // 件4：纪律闭环——集群 worker 免费获得
    // （同 loop 代码，任务描述带 marker 即参与；workspace 锚本节点根）。
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
        // BUG-20261106-042：与主 loop 同款——证伪 hook 带 security 句柄
        // （规则层管线前置）。security feature 关闭 = 旧行为（诚实降级）。
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

    // 用量账本（二期 token 回传的生产前提）：worker 侧 cluster agent 的
    // LLM 调用必须落 request_logs，任务结束时 extract_task_usage 才能从
    // 账本聚合出差值随回调回传 master。漏接线时账本恒空 → usage=None →
    // master 诚实跳过记账（不炸，但 token 回传静默失效）。装配序上本函数
    // 经 ClusterServiceAdapter 在 SharedResources（含 data_store）之后构建，
    // 与 build_agent_loop 的接线（本文件 set_data_store 处）同源。
    if let Some(ref ds) = shared.data_store {
        agent_loop.set_data_store(ds.clone());
    }

    // F- 接线（UAT round-2 实证，① 同型漏接）：cluster loop 必须
    // 注入工作区根——缺了它 cluster_agent::build_context 的 tool_path_base
    // 注入条件（workspace_root + exec 目录 sidecar 在场）恒 None，档案管线
    // 任务的相对路径重写静默失效（worker 文件落 workspace 根，变更集丢失）。
    agent_loop.set_workspace_root(shared.workspace_dir());

    // vault 域绑定 sidecar 权威路径（L26 修复 2026-11-11）：集群 loop 与
    // 主 loop 同源主 workspace config（worker 执行远端任务同样走域绑定闸）。
    agent_loop.set_vault_domain_config_path(shared.vault_domain_config_path());

    // ASM-01（横扫存量加固）：集群 loop 同接安全 8 层——此前漏接
    // 使 B 端执行远端任务时整条管线（注入/命令守卫/ABAC/凭据/DLP/SSRF/
    // 病毒扫描/审计链）静默旁路（loop.rs dispatch 对 None=直接跳过）。
    // 给项目 loop 接线时已明言「headless/ACP/项目会话都不是安全旁路」，
    // 集群面同理：B 端跑的是远端半可信任务，恰恰是最需要管线的地方。
    // plugin 共享同一 Arc：auditor 审批 manager / guardian judge 随注入一并
    // 生效。security.enabled=false 时 shared 为 None = 文档化合法态（与主
    // 实例一致），不视为漏项（ASM-08 断言按一致性裁决）。
    #[cfg(feature = "security")]
    if let Some(ref plugin) = shared.security_plugin {
        agent_loop.set_security_plugin(plugin.clone());
        info!("[AgentFactory] cluster loop security 8-layer pipeline attached (ASM-01)");
    }
    // 免打扰运行时态挂集群 loop（无人值守主战场——worker 执行 board
    // 任务时 doom-loop escalation 不停车等人）。槽未装配 = None = 现行为。
    #[cfg(feature = "security")]
    if let Some(state) = nemesis_web::handlers::security::approval_auto_state() {
        agent_loop.set_approval_auto_approve(state);
    }

    // ASM-02（横扫存量加固）：memory 审批闸接集群 loop——此前闸
    // 只在 gateway 后处理接给主 loop（commands/gateway.rs approval 装配段），
    // 集群 loop 的 gate 保持 None = memory_tools 对 None 放行，远端任务可无
    // 审批写/删本地记忆。approval_slot 由 gateway 装配期填（先建槽后填），
    // cluster loop 构建于 services 启动期（first_start/start），时序在填槽
    // 之后；槽空且 security 开启 = 时序异常，WARN 揪出来。
    // （二轮复评）：desktop 从门里去掉（与 GatewayMemoryGate
    // 本体的 memory+security 门对齐——desktop+memory+security 缺 desktop
    // 组合此前、desktop 关时集群 loop 白丢审批闸）。
    #[cfg(all(feature = "memory", feature = "security"))]
    if let Some(adapter) = shared.approval_slot.read().clone() {
        agent_loop.set_memory_approval_gate(Arc::new(
            crate::commands::gateway::GatewayMemoryGate::new(adapter),
        ));
    } else if shared.security_plugin.is_some() {
        tracing::warn!(
            "[AgentFactory] cluster loop: security 开启但 approval_slot 为空——\
             memory 审批闸缺席（gateway 装配时序异常？）"
        );
    }

    // ASM-03（横扫存量加固）：价目表注入（context_window）——
    // 集群 loop 此前漏接 → 窗口恒 128k fallback，大窗口模型被提前误压缩。
    // 与主/项目 loop 同一 workspace data 目录；open 失败诚实降级（同）。
    match nemesis_data::PricingStore::open(&nemesis_path::workspace_data_dir(&shared.home)) {
        Ok(store) => agent_loop.set_pricing_store(std::sync::Arc::new(store)),
        Err(e) => tracing::warn!(
            error = %e,
            "[AgentFactory] cluster loop pricing store open failed; context_window L2 disabled"
        ),
    }

    // ASM-07（横扫存量加固）：零散不对称补齐——
    // - skills_loader： skills 目录摘要注入，集群 agent 的 advertised
    //   catalog 与真实工具行为对齐；
    // - snapshot_role：与主 loop 同一 config 来源；
    // - ToolEventHook + agent_event_tx：集群任务的工具事件上 Dashboard
    //   tool_event 卡片（此前恒空白）。
    if let Some(ref loader) = shared.skills_loader {
        agent_loop.set_skills_loader(loader.clone());
    }
    agent_loop.set_snapshot_role(&cfg.agents.defaults.snapshot_role);
    if let Some(tx) = shared.agent_event_tx.clone() {
        agent_loop.add_tool_hook(std::sync::Arc::new(
            nemesis_agent::tool_event_hook::ToolEventHook::new(tx, agent_loop.work_recorder()),
        ));
    }
    agent_loop.set_agent_event_tx(shared.agent_event_tx.clone());
    // 集群 loop 同挂会话统计 sink（B 端任务会话同账本）。
    agent_loop.set_session_stats_sink(
        shared
            .session_stats
            .clone()
            .map(|a| a as std::sync::Arc<dyn nemesis_agent::SessionStatsSink>),
    );

    // ASM-06（横扫存量加固）：continuation manager——3-hop 链路里
    // B 端对下一跳就是 A 端，cluster loop 发起 cluster_rpc 时续行快照必须有
    // 处落。与主 loop 同一 {workspace}/cluster/rpc_cache 目录；TTL 清扫不
    // 重复起——主 loop 侧唯一持有（同项目 loop 先例）。
    agent_loop.set_continuation_manager(Arc::new(
        nemesis_agent::ContinuationManager::with_disk_store(&shared.workspace_dir()),
    ));

    // (arch review, U-list): the cluster agent must resolve
    // the same startup capability tier as the main agent. Before this, the
    // cluster loop always ran the AgentLoop::new default (Big = full 42-tool
    // set) even when the active model is a small model — the main agent's
    // mini-tier filtering never applied cluster-side. Same wiring as
    // build_agent_loop: resolve from the raw config.json Value (model_tier
    // is a dynamic field there), then remember the config path so the loop's
    // check_config_reload re-resolves the tier live when config.json changes
    // on disk (dashboard model add / CLI `model set-tier`) cluster-side too.
    let cfg_json: serde_json::Value = nemesis_utils::fsutil::read_config_string(&config_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let resolved_tier = nemesis_types::capability::resolve_active_tier(&cfg_json, &model_name);
    info!(
        "[AgentFactory] Cluster model '{}' capability tier: {}",
        model_name, resolved_tier
    );
    agent_loop.set_tier(resolved_tier);
    agent_loop.set_config_path(config_path.clone());
    // （凭据 vault）：集群 worker 同款凭据卡装配——B 端跑
    // 长任务撞 Auth 同样能发卡续行（broker 未 install = None 零回归）。
    agent_loop.set_provider_reloader(Arc::new(ModelProviderReloader {
        home: shared.home.clone(),
        workspace: shared.workspace_dir(),
    }));
    #[cfg(feature = "security")]
    if let Some(broker) = crate::credential_broker::current() {
        agent_loop.set_credential_broker(broker);
    }
    // 集群 agent 同样注入共享 LspManager（B 端跑长任务写码时也享受
    // 编辑后诊断回灌；同一实例，server 进程不翻倍）。
    #[cfg(feature = "lsp")]
    agent_loop.set_lsp_manager(shared.lsp_manager.clone());
    // 环尾 seq 查询注入（与主 loop 同源语义——B 端 loop
    // 虽不走 web history_request，统一注入保持装配一致性，防「这个 loop
    // 为什么没有」的漂移）。
    agent_loop.set_chat_seq_lookup(std::sync::Arc::new(nemesis_web::chat_event_log::latest_seq));

    // (arch review, U-list): enable tool-result spill for
    // the cluster agent too — cluster peer_chat is exactly the long-task /
    // huge-output case; without a spill root, oversized results (>64k chars)
    // degrade to the prune profile (head+tail 3600) with no full copy on
    // disk. Shares the main agent's spill root and retention setting.
    // NOTE: the daily-midnight cleanup task is already spawned by
    // build_agent_loop for this same root — deliberately NOT spawned again
    // here (a second timer would double-scan).
    // 迁回 workspace（设计指定位置），同主 agent。
    let spill_root =
        nemesis_path::resolve_spill_dir_in_workspace(&nemesis_path::workspace_dir(&shared.home));
    agent_loop.set_spill_root(spill_root.clone());
    let spill_retention = cfg.agents.defaults.spill_retention_days.max(0) as u64;
    if spill_retention > 0 {
        let deleted = nemesis_agent::spill::cleanup_expired(&spill_root, spill_retention);
        if deleted > 0 {
            info!(
                deleted,
                retention_days = spill_retention,
                "[AgentFactory] cluster spill startup cleanup (TTL={}d)",
                spill_retention
            );
        }
    }

    // 5b. Set observer callback to capture cluster task execution details (LLM + tool calls).
    {
        let log_cb: Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync> =
            Arc::new(|event_type: &str, data: &serde_json::Value| {
                // Only log cluster-related trace events.
                let trace_id = data.get("trace_id").and_then(|v| v.as_str()).unwrap_or("");
                if !trace_id.starts_with("cluster") {
                    return;
                }

                // Extract task_id from trace_id: "cluster-XXXXXXXX" or "cluster-resume-XXXXXXXX"
                let task_id = trace_id
                    .strip_prefix("cluster-resume-")
                    .unwrap_or_else(|| trace_id.strip_prefix("cluster-").unwrap_or(trace_id));

                match event_type {
                    "llm_request" => {
                        let round = data.get("round").and_then(|v| v.as_u64()).unwrap_or(0);
                        let model = data.get("model").and_then(|v| v.as_str()).unwrap_or("");
                        nemesis_cluster::cluster_log::write_cluster_log(
                            "task_llm_start",
                            serde_json::json!({
                                "task_id": task_id,
                                "round": round,
                                "model": model,
                            }),
                        );
                    }
                    "llm_response" => {
                        let round = data.get("round").and_then(|v| v.as_u64()).unwrap_or(0);
                        let duration_ms = data
                            .get("duration_ms")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let tokens = data.get("usage");
                        nemesis_cluster::cluster_log::write_cluster_log(
                            "task_llm_end",
                            serde_json::json!({
                                "task_id": task_id,
                                "round": round,
                                "duration_ms": duration_ms,
                                "tokens": tokens,
                            }),
                        );
                    }
                    "tool_call" => {
                        let tool_name =
                            data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
                        let success = data
                            .get("success")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let duration_ms = data
                            .get("duration_ms")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let round = data.get("round").and_then(|v| v.as_u64()).unwrap_or(0);
                        nemesis_cluster::cluster_log::write_cluster_log(
                            "task_tool_call",
                            serde_json::json!({
                                "task_id": task_id,
                                "round": round,
                                "tool": tool_name,
                                "duration_ms": duration_ms,
                                "success": success,
                            }),
                        );
                    }
                    _ => {}
                }
            });
        agent_loop.set_observer_callback(log_cb);
    }

    // 5c. Create dedicated observer_manager + ClusterRequestLoggerObserver.
    //
    // Independent from main agent's observer_manager — completely isolates
    // event dispatch. Observer writes LLM details to
    // `cluster_logs/{device_id}/{ts_ms}_{task_id}/` per task.
    //
    // Task context (task_id + device_id) is set/cleared by cluster_agent_loop
    // around each task execution.
    //
    // 执行档案根修（T-XFER-1~6 全败实证）：cluster 请求日志是
    // 看板档案回传的**功能数据源**（outbox 入队/启动清扫补入队/兜底拉取都按
    // `nemesis_path` 固定的 `logs/cluster_logs` 扫），不是调试日志——与
    // `logging.llm.enabled`/`llm.log_dir`（主 agent request_logs 的开关与
    // 目录）完全解耦：UAT harness 改端口时整段替换 logging 曾把它静默抹掉
    // （enabled），模板 log_dir 又指向 request_logs——两个方向都会让档案
    // 管线全链死且无任何告警。cluster agent 路径常开（同 session_log 先例；
    // 每 cluster 任务一个目录）+ 目录钉死 canonical 值。
    let cluster_observer: Option<
        Arc<crate::cluster_request_logger_observer::ClusterRequestLoggerObserver>,
    > = {
        let logging_config = nemesis_agent::request_logger::LoggingConfig {
            enabled: true,
            detail_level: nemesis_agent::request_logger::DetailLevel::Full,
            log_dir: "logs/cluster_logs".to_string(),
            save_raw: true,
        };
        let workspace_path = shared.workspace_dir();
        let observer = Arc::new(
            crate::cluster_request_logger_observer::ClusterRequestLoggerObserver::new(
                logging_config,
                &workspace_path,
            ),
        );

        // Create dedicated observer_manager and register the observer.
        // try_register 同步快路（manager 刚建未共享，无竞争）：factory 是
        // sync fn，旧 block_in_place 桥接在 current-thread runtime（如
        // #[tokio::test]）直接 panic—— 常开注册后 4 个 factory
        // 测试实证。锁意外被占 = 诚实装配失败，不 panic 不静默丢 observer。
        let mgr = Arc::new(nemesis_observer::Manager::new());
        if !mgr.try_register(observer.clone() as Arc<dyn nemesis_observer::Observer>) {
            return Err(anyhow::anyhow!(
                "cluster observer manager 注册锁被占（异常并发）"
            ));
        }
        agent_loop.set_observer_manager(mgr);

        info!(
            "[AgentFactory] ClusterRequestLoggerObserver registered (writes to logs/cluster_logs/{{device_id}}/{{task_id}}/)"
        );
        Some(observer)
    };

    // 6. Build tool config + register all tools + enable MCP.
    // spawn 槽此处**不注入**闭包——build_cluster_agent_loop 返回非 Arc
    // 的 AgentLoop（Arc 由 Cluster 调用方创建），Weak 注入点不在此；槽留空
    // 时 SpawnTool 诚实报 "not available"。v1 边界：sub-agent 只在主
    // gateway loop 开放（cluster B 端 spawn 见 后续）。
    let (mut tool_config, _unused_cluster_spawn_slot) =
        build_shared_tool_config(shared, &cfg, &model_name, None);
    // ASM-04（横扫存量加固）：集群 loop 工作区围栏强制 restrict=true。
    // 方向性事实：项目 loop（本地可信会话）强制 restrict=true，集群 loop
    // （执行远端半可信任务）此前跟随全局开关（默认 false）——B 端 file 工具
    // 默认无写围栏。远端任务才是最需要围栏的面，方向必须反转：强制 true，
    // root=shared.workspace_dir()（与 estop/workspace_root 接线同一根）。
    // editor 开关 × 围栏两层不认账修复：集群围栏接上开关闸——
    // 与 auditor 同一 EditorAccessState（集群 loop 本就共享同一 security
    // plugin），审批层认开关时围栏同一裁决，两层不再分叉；security 裁剪
    // 或未装配 = None 旧行为。
    #[cfg(feature = "security")]
    let editor_gate = shared
        .security_plugin
        .as_ref()
        .and_then(|p| p.auditor().editor_access())
        .map(|e| e.fence_gate());
    #[cfg(not(feature = "security"))]
    let editor_gate = None::<std::sync::Arc<dyn Fn(&str, bool) -> bool + Send + Sync>>;
    tool_config.workspace_boundary = Some(Arc::new(nemesis_agent::loop_tools::WorkspaceBoundary {
        root: shared.workspace_dir(),
        restrict: true,
        extra_roots: Vec::new(),
        editor_gate,
    }));
    // 集群 loop 同挂钩子边界（同一 Arc 实例）。
    if let Some(ref boundary) = tool_config.workspace_boundary {
        agent_loop.set_workspace_boundary(boundary.clone());
    }
    // Cluster agent does not use executor separation yet (B.0 scope: main agent
    // only). Pass None → all tools stay local. 常驻路由同不理（worker 执行
    // 语境是任务级一次性装配，无 tasks.db 持久实体可解析）。
    register_tools_and_mcp(&mut agent_loop, shared, &tool_config, None, None);

    // Swarm （主动发言通道）：board_discuss 仅注册进 cluster agent
    // ——被动唤醒（第三 select 臂）之外，worker 干活途中可主动向看板线程
    // 发言。master 本尊直写 store、主 agent 同理，都不装。tier 白名单不
    // 收录——与 cluster_rpc 同族（Big/Unresolved 全量可见；mini/normal 不
    // 给：跨节点发言是高信任交互）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    {
        agent_loop.register_tool(
            crate::board_discuss_tool::TOOL_NAME.to_string(),
            Box::new(crate::board_discuss_tool::BoardDiscussTool::new(
                cluster.clone(),
            )),
        );
        // 集群任务认领：board_claim 与 board_discuss 同点
        // 同族注册——worker 自主浏览/认领待领单（先到先得，master 原子
        // 裁决后落本节点工作队列）。tier 政策同 board_discuss（白名单不
        // 收录 = Big/Unresolved 全量可见，mini/normal 不给：跨节点认领
        // 与跨节点发言同信任级）。
        agent_loop.register_tool(
            crate::board_claim_tool::TOOL_NAME.to_string(),
            Box::new(crate::board_claim_tool::BoardClaimTool::new(
                cluster.clone(),
            )),
        );
        // 资产拉取/发布执行者：两端都装——worker 拉任务资产，master 的
        // cluster agent 也能拉 worker 交付物（同一工具、同一 bundle 语义）。
        // with_cluster 注入集群句柄：HTTP 直连连接级失败（跨网段）时走
        // asset.meta/asset.chunk RPC 分块兜底。
        agent_loop.register_tool(
            crate::board_asset_tool::TOOL_NAME.to_string(),
            Box::new(
                crate::board_asset_tool::BoardAssetTool::new(shared.workspace_dir())
                    .with_cluster(cluster.clone()),
            ),
        );
        info!(
            "[AgentFactory] board_discuss + board_claim + board_asset tools registered (cluster agent only)"
        );
    }

    // (arch review):.1 auto memory injection (per-round
    // injection of relevant local user memories before each LLM call) is
    // deliberately NOT wired for the cluster agent — same policy as the
    // hooks exemption in build_agent_loop (comment above there): a remote
    // peer's task context must not receive this local user's personal
    // memories. The wiring below is a different thing: it only stashes the
    // memory executor for the interactive memory tools (memory_store /
    // memory_forget with approval gating).
    //
    // Stash the memory executor so the gateway can attach an approval gate
    // post-construction (agent memory_store/forget require interactive
    // approval, never bypassed by YOLO/auto).
    #[cfg(feature = "memory")]
    {
        if let Some(ref exec) = tool_config.memory_executor {
            agent_loop.set_memory_executor(exec.clone());
        }
    }

    // 6b. Attach a dedicated SessionStore so cluster peer_chat can persist and
    // restore conversation history per (source_node_id, chat_id) pair.
    //
    // Path: `{workspace}/sessions/cluster/`
    // - Separate from main agent's `{workspace}/sessions/` to avoid any chance
    //   of file-name collision (sanitize_filename keeps `:` and `/` distinct
    //   from `_`, but a dedicated directory is the simpler invariant).
    // - cluster_agent.rs::execute_new_task reads from this store to restore
    //   history; writes back user + final assistant message after task completes.
    //
    // TTL: sessions older than 7 days are deleted at startup and via a daily
    // midnight task (spawn_daily_cleanup). Bounded disk usage without manual
    // intervention.
    {
        let cluster_sessions_dir = nemesis_path::resolve_sessions_dir_in_workspace(
            &nemesis_path::workspace_dir(&shared.home),
        )
        .join("cluster");
        let cluster_session_store = Arc::new(
            nemesis_agent::session::SessionStore::new_with_storage(&cluster_sessions_dir),
        );
        let deleted = cluster_session_store.cleanup_old_sessions(7);
        if deleted > 0 {
            info!(
                deleted,
                "[AgentFactory] Cluster SessionStore startup cleanup (TTL=7d)"
            );
        }
        spawn_daily_cleanup(cluster_session_store.clone(), "Cluster", None);
        agent_loop.set_session_store(cluster_session_store);
        info!(
            dir = %cluster_sessions_dir.display(),
            "[AgentFactory] Cluster SessionStore attached (for peer_chat history)"
        );
    }

    // 6b. Checkpoint store (edit safety net) for the cluster agent too.
    {
        let ws = shared.workspace_dir();
        // 同一 logs/checkpoints 目录：cluster 与 main 的 turn 计数各自独立，
        // 平铺文件共用（与既有行为一致）；目录已由上方主 loop 迁移。
        let store = Arc::new(nemesis_agent::checkpoint::CheckpointStore::new(
            Some(nemesis_path::resolve_checkpoints_dir_in_workspace(&ws)),
            ws,
        ));
        agent_loop.set_checkpoint_store(store);
    }

    // 7. Register cluster_rpc with call_fn + peers_fn (if available).
    if let (Some(rpc_config), Some(call_fn)) =
        (&shared.cluster_rpc_config, &shared.cluster_rpc_call_fn)
    {
        let mut cluster_rpc_tool = nemesis_agent::ClusterRpcTool::new(rpc_config.clone());
        cluster_rpc_tool.set_rpc_call_fn(call_fn.clone());
        if let Some(ref peers_fn) = shared.cluster_peers_fn {
            cluster_rpc_tool.set_peers_fn(peers_fn.clone());
        }
        if let Some(ref auto_fn) = shared.cluster_auto_target_fn {
            cluster_rpc_tool.set_auto_target_fn(auto_fn.clone());
        }
        cluster_rpc_tool.set_enabled(true);
        agent_loop.register_tool("cluster_rpc".to_string(), Box::new(cluster_rpc_tool));
        info!("[AgentFactory] cluster_rpc tool registered for cluster agent (enabled=true)");
    }

    // ASM-08：装配自检（与主/项目 loop 同一闸，=甲）。
    assert_gateway_critical_wiring(&agent_loop, shared, "集群 loop")?;

    info!(
        model = %model_name,
        tools = agent_loop.tool_count(),
        has_system_prompt = config.system_prompt.is_some(),
        "[AgentFactory] Cluster AgentLoop built successfully"
    );

    Ok((Arc::new(agent_loop), config, cluster_observer))
}

/// Load cluster system prompt from `workspace/cluster/IDENTITY.md` + `SOUL.md`.
///
/// Returns None if neither file exists (cluster agent runs without identity).
///
/// FT：尾部补工作区绝对路径行——B 端系统提示词此前只内嵌
/// 人格文件**内容**不给**位置**，worker 跑任务时相对路径读文件必然按 cwd
/// 解析（exe 直启形态读不到）。主 loop 的 identity 段已有
/// `**Workspace**: {}`（context.rs build_identity），此处对齐。
#[cfg(feature = "cluster")]
/// 集群 agent 稳定前缀装配（集群专业职能框架）：
/// `workspace/cluster/IDENTITY.md` → `SOUL.md` → `EXPERTISE.md`（断线
/// 修复：有则注入，无条件）→ worker 行为契约段（`worker_discipline`
/// 闸，缺省 on）→ **Workspace 行收尾**。全部段落在任务级职能后缀**之前**
/// （后缀由 cluster_agent 每任务追加）——稳定前缀字节跨任务不变，
/// prompt cache 跨任务命中。
///
/// 返回 None = 没有任何身份文件且契约关闭（裸跑，与现状兼容）。
pub(super) fn load_cluster_system_prompt(
    home: &std::path::Path,
    worker_discipline: bool,
) -> Option<String> {
    let cluster_dir = home.join("workspace").join("cluster");
    let mut parts = Vec::new();

    if let Ok(content) = std::fs::read_to_string(cluster_dir.join("IDENTITY.md"))
        && !content.trim().is_empty()
    {
        parts.push(content);
    }
    if let Ok(content) = std::fs::read_to_string(cluster_dir.join("SOUL.md"))
        && !content.trim().is_empty()
    {
        parts.push(content);
    }
    // （集群专业职能框架）：EXPERTISE.md 断线修复——档案曾经存在但装配
    // 从不读取。有则注入，无开关（职能方法论载体，稳定前缀组成部分）。
    if let Ok(content) = std::fs::read_to_string(cluster_dir.join("EXPERTISE.md"))
        && !content.trim().is_empty()
    {
        parts.push(content);
    }
    // worker 行为契约（数据非指令防御 + 汇报契约 + 工作区纪律），
    // `worker_discipline` 闸（config.cluster.json，缺省 on）。内置常量，
    // 无磁盘 IO；关掉 = 回退纯人格提示词。
    if worker_discipline {
        parts.push(nemesis_prompts::professions::CLUSTER_WORKER_CONTRACT.to_string());
    }

    if parts.is_empty() {
        info!("[AgentFactory] No cluster identity files found, running without system prompt");
        None
    } else {
        // 工作区根 = <home>/workspace（与装配处 set_workspace_root 同源）。
        // Workspace 行收在稳定前缀末尾（任务级职能后缀追加在其后——若放
        // 整条提示词末尾会被可变后缀隔断 cache 前缀）。
        let workspace = home.join("workspace");
        parts.push(format!(
            "**Workspace**: {}\nYour workspace is located at: {}。read_file/list_dir 等文件工具的相对路径以此为根。",
            workspace.display(),
            workspace.display()
        ));
        info!(
            files = parts.len(),
            "[AgentFactory] Cluster system prompt loaded from {} file(s)",
            parts.len()
        );
        Some(parts.join("\n\n---\n\n"))
    }
}

// ---------------------------------------------------------------------------
// build_project_agent_loop — 项目 loop 工厂（对话/项目双分组）
//
// 每个项目一个常驻 AgentLoop（方案 B）。与主 loop 的差异面（设计基线）：
// - 工作区锚项目目录（system prompt / 指令链 / 围栏 / todo / state / fs watcher）
// - 围栏强制 restrict=true（项目会话写入绝不越出项目目录，双分组安全底线）
// - 剥离 skills/memory/forge/cluster_rpc/continuation（项目 loop 禁异步续行）
// - checkpoint 影子库落主 workspace（不污染用户项目目录）
// - SessionStore 共享主 loop 的同一 Arc（存储全局集中，会话隔离靠 session_key）
// ---------------------------------------------------------------------------
