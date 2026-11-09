// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 agent_factory.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// 路由 config 接线——把 `bindings` / `agents.list` /
/// `session.dm_scope` 映射成生产 [`nemesis_routing::RouteResolver`]。
///
/// 此前四个 config 键只声名未读取（typed round-trip 假活），生产 resolver
/// 一直是 `new_bus` 的硬编码默认（单 main agent、Main scope、空身份链）。
/// 映射语义：config `BindingMatch.guild_id/team_id` 空串 = 无约束（
/// Option None，可落 account 级匹配）；`peer` 原样透传（空串字段的取舍
/// 由 nemesis-routing 自身的语义裁决）；`agents.list` 非空时 `pick_agent`
/// 按其校验/回退默认，空列表 = 保留绑定原始 agent id（与旧行为同形）。
/// 全部键缺省 → resolver 行为与旧硬编码字节一致（零回归）。
pub(super) fn build_route_resolver(cfg: &nemesis_config::Config) -> nemesis_routing::RouteResolver {
    let bindings = cfg
        .bindings
        .iter()
        .map(|b| nemesis_routing::AgentBinding {
            agent_id: b.agent_id.clone(),
            match_channel: b.r#match.channel.clone(),
            match_account_id: b.r#match.account_id.clone(),
            match_peer_kind: b.r#match.peer.as_ref().map(|p| p.kind.clone()),
            match_peer_id: b.r#match.peer.as_ref().map(|p| p.id.clone()),
            match_guild_id: (!b.r#match.guild_id.is_empty()).then(|| b.r#match.guild_id.clone()),
            match_team_id: (!b.r#match.team_id.is_empty()).then(|| b.r#match.team_id.clone()),
        })
        .collect();
    let agents = cfg
        .agents
        .list
        .iter()
        .map(|a| nemesis_routing::AgentDef {
            id: a.id.clone(),
            is_default: a.default,
        })
        .collect();
    nemesis_routing::RouteResolver::new(nemesis_routing::RouteConfig {
        bindings,
        agents,
        dm_scope: cfg.session.dm_scope.clone(),
    })
}

/// （扩展计划）：fallback 链装配（启动路径）——active 模型条目的
/// per-model extra 键 `fallback_to: ["alias-b",...]`（BTreeMap flatten
/// 透传，无需 typed 字段）。本函数只做 config → [`FallbackLevel`] 解析；
/// 装配语义（自引用跳过/单级构造失败 warn 跳过/FallbackProvider 包装或
/// 原样返回主 provider）单点在
/// `nemesis_providers::fallback_provider:assemble_fallback_chain`——运行
/// 期热切路径（nemesis-web models handler）同源消费，两条路径不漂移。
/// `fallback_to` 缺失/全不可用 → 原样返回主 provider（行为与现状字节一致）。
pub(super) fn wrap_fallback_chain(
    cfg: &nemesis_config::Config,
    model_name: &str,
    workspace: &std::path::Path,
    primary: Arc<dyn nemesis_providers::router::LLMProvider>,
) -> Arc<dyn nemesis_providers::router::LLMProvider> {
    let fallback_aliases: Vec<String> = cfg
        .model_list
        .iter()
        .find(|m| m.model_name == model_name)
        .and_then(|m| m.extra.get("fallback_to"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if fallback_aliases.is_empty() {
        return primary;
    }

    let mut levels = Vec::new();
    for alias in &fallback_aliases {
        // 防自引用/重复主条目——链里出现自己只会空转冷却（装配单点另有
        // 同款闸，这里提前跳过免做无谓 resolve）。
        if alias == model_name {
            continue;
        }
        match nemesis_config::resolve_model_config(cfg, alias) {
            Ok(fr) => levels.push(nemesis_providers::fallback_provider::FallbackLevel {
                alias: alias.clone(),
                model: fr.model_name.clone(),
                factory: nemesis_providers::factory::FactoryConfig {
                    proxy: fr.proxy.clone(),
                    stream_mode: fr.stream_mode.clone(),
                    cache_breakpoints: fr.cache_breakpoints,
                    extra_body: fr.extra_body.clone(),
                    llm_ref: format!("{}/{}", fr.provider_name, fr.model_name),
                    api_key: fr.api_key.clone(),
                    api_base: fr.api_base.clone(),
                    workspace: workspace.to_string_lossy().to_string(),
                    connect_mode: fr.connect_mode,
                    protocol: fr.protocol.clone(),
                    timeout_secs: fr.timeout_secs,
                    account_id: String::new(),
                    headers: HashMap::new(),
                },
            }),
            Err(e) => {
                warn!(
                    "[AgentFactory] fallback '{}' resolve failed: {} — 跳过该级",
                    alias, e
                );
            }
        }
    }
    nemesis_providers::fallback_provider::assemble_fallback_chain(model_name, primary, levels)
}

/// （凭据 vault）：Auth 凭据卡的 provider 重建器。持
/// config.json 所在 home 与 workspace；reload 用 **raw JSON patch**（不经
/// typed round-trip——typed save 抹 untyped per-model 键的先例，extra
/// flatten 双保险只护 typed 保存路径，这里根本不走它）+ factory 同源
/// resolve/create + fallback 链包装，与 [`build_agent_loop`] 装配语义一致。
pub(crate) struct ModelProviderReloader {
    pub(super) home: PathBuf,
    pub(super) workspace: PathBuf,
}

impl ModelProviderReloader {
    /// 读 raw config.json（读/解析失败 = Err，诚实上抛）。
    fn read_raw(&self) -> Result<serde_json::Value, String> {
        let s = nemesis_utils::fsutil::read_config_string(self.home.join("config.json"))
            .map_err(|e| format!("读 config.json 失败: {e}"))?;
        serde_json::from_str(&s).map_err(|e| format!("解析 config.json 失败: {e}"))
    }
}

impl nemesis_agent::r#loop::ProviderReloader for ModelProviderReloader {
    fn credential_request_fields(&self, model: &str) -> (String, String) {
        let proposed = format!("model/{model}");
        // raw 读条目 api_key（model_name/model 双形态精确匹配，无前缀门控——
        // resolve_model_config 根修同口径）。已是 `vault:<alias>`
        // = 轮换语义：沿用既有别名（卡片覆盖写，created_at 保留）。
        let existing = self.read_raw().ok().and_then(|raw| {
            raw.get("model_list")?
                .as_array()?
                .iter()
                .find_map(|e| {
                    let m = e.get("model").and_then(|v| v.as_str());
                    let mn = e.get("model_name").and_then(|v| v.as_str());
                    (m == Some(model) || mn == Some(model))
                        .then(|| e.get("api_key").and_then(|k| k.as_str()).map(String::from))
                })
                .flatten()
        });
        match existing {
            Some(k) if k.starts_with(nemesis_config::VAULT_PREFIX) => {
                let alias = k[nemesis_config::VAULT_PREFIX.len()..].trim().to_string();
                if alias.is_empty() {
                    (proposed.clone(), proposed)
                } else {
                    (proposed, alias)
                }
            }
            _ => (proposed.clone(), proposed),
        }
    }

    fn reload_provider(
        &self,
        model: &str,
        patch_alias: Option<&str>,
    ) -> Result<Arc<dyn nemesis_agent::r#loop::LlmProvider>, String> {
        // 1. raw patch：条目 api_key → `vault:<alias>`（原子写，失败原文件
        // 不动；凭据值本体不进 config——只写引用）。
        if let Some(alias) = patch_alias {
            let mut raw = self.read_raw()?;
            let list = raw
                .get_mut("model_list")
                .and_then(|v| v.as_array_mut())
                .ok_or_else(|| "config.json 无 model_list 数组".to_string())?;
            let entry = list
                .iter_mut()
                .find(|e| {
                    e.get("model").and_then(|m| m.as_str()) == Some(model)
                        || e.get("model_name").and_then(|m| m.as_str()) == Some(model)
                })
                .ok_or_else(|| format!("config.json 无模型条目 `{model}`"))?;
            entry["api_key"] =
                serde_json::Value::String(format!("{}{alias}", nemesis_config::VAULT_PREFIX));
            let pretty = serde_json::to_string_pretty(&raw)
                .map_err(|e| format!("序列化 config.json 失败: {e}"))?;
            nemesis_utils::write_file_atomic(
                &self.home.join("config.json").to_string_lossy(),
                pretty.as_bytes(),
                0o600,
            )
            .map_err(|e| format!("写 config.json 失败: {e}"))?;
        }
        // 2. factory 同源 resolve + create + fallback 链（build_agent_loop
        // 第 1-2 步同款；创建失败 = Some(warn) → Err 诚实上抛，不降级
        // NullProvider——拿 Null 重试必然再吃 Auth 错，纯属烧轮次）。
        let config_path = self.home.join("config.json");
        let cfg = nemesis_config::load_config(&config_path)
            .map_err(|e| format!("load_config 失败: {e}"))?;
        let resolution = nemesis_config::resolve_model_config(&cfg, model)
            .map_err(|e| format!("resolve_model_config(`{model}`) 失败: {e}"))?;
        let factory_cfg = nemesis_providers::factory::FactoryConfig {
            proxy: resolution.proxy.clone(),
            stream_mode: resolution.stream_mode.clone(),
            cache_breakpoints: resolution.cache_breakpoints,
            extra_body: resolution.extra_body.clone(),
            llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
            api_key: resolution.api_key.clone(),
            api_base: resolution.api_base.clone(),
            workspace: self.workspace.to_string_lossy().to_string(),
            connect_mode: resolution.connect_mode,
            protocol: resolution.protocol.clone(),
            timeout_secs: resolution.timeout_secs,
            account_id: String::new(),
            headers: HashMap::new(),
        };
        let (provider, create_warn) =
            nemesis_providers::factory::create_provider_or_null(&factory_cfg);
        if let Some(w) = create_warn {
            return Err(format!("provider 构建失败: {w}"));
        }
        let provider_arc: Arc<dyn nemesis_providers::router::LLMProvider> =
            wrap_fallback_chain(&cfg, model, &self.workspace, provider);
        Ok(Arc::new(ProviderAdapter::new(
            provider_arc,
            resolution.model_name,
        )))
    }
}
/// Build SharedToolConfig from shared resources + fresh config.
///
/// Extracted from build_agent_loop so both main and cluster agents
/// share the same tool configuration logic.
///
/// 同时返回 spawn 共享槽（`Arc<OnceLock<SpawnFn>>`）——SharedToolConfig
/// 里那份是克隆，闭包注入必须在 AgentLoop Arc 定型**之后**（Weak 需要
/// Arc::downgrade），所以调用方要保留这份原件做延迟注入。
#[allow(clippy::type_complexity)]
pub(super) fn build_shared_tool_config(
    shared: &Arc<SharedResources>,
    cfg: &nemesis_config::Config,
    model_name: &str,
    mcp_tool_snapshot: Option<
        Arc<parking_lot::RwLock<Vec<nemesis_agent::r#loop::McpToolSnapshotEntry>>>,
    >,
) -> (
    nemesis_agent::SharedToolConfig,
    Arc<std::sync::OnceLock<nemesis_agent::loop_tools::SpawnFn>>,
) {
    let workspace_dir = shared.workspace_dir();

    // spawn 共享槽原件（SharedToolConfig 拿克隆；本原件留给调用方
    // 在 Arc<AgentLoop> 定型后注入 Weak 闭包）。
    let spawn_slot: Arc<std::sync::OnceLock<nemesis_agent::loop_tools::SpawnFn>> =
        Arc::new(std::sync::OnceLock::new());

    let config = nemesis_agent::SharedToolConfig {
        workspace: Some(workspace_dir.to_string_lossy().to_string()),
        // 文件工具工作区边界（write/edit/append 纵深
        // 防御，不单靠安全 8 层管线）；开关跟 agents.defaults
        // .restrict_to_workspace 配置走。
        workspace_boundary: Some(Arc::new(nemesis_agent::loop_tools::WorkspaceBoundary {
            root: workspace_dir.clone(),
            restrict: cfg.agents.defaults.restrict_to_workspace,
            extra_roots: Vec::new(),
            // 主会话不接开关闸（暂不处理）：None = 围栏
            // 旧行为，editor 开关只管审批层，与本围栏互不感知。
            editor_gate: None,
        })),
        // todowrite tool — workspace storage + TodoUpdated
        // broadcast（与 ToolEventHook 共用同一 sender；gateway 建、
        // web pump 消费）。
        todo: Some(nemesis_agent::loop_tools::TodoToolConfig {
            workspace: workspace_dir,
            event_tx: shared.agent_event_tx.clone(),
        }),
        cron_service: Some(shared.cron_service.clone()),
        forge_executor: shared.forge_executor.clone(),
        forge: shared.forge.clone(),
        #[cfg(feature = "memory")]
        memory_executor: shared.memory_manager.as_ref().map(|mgr| {
            Arc::new(nemesis_memory::memory_tools::MemoryToolExecutor::new(
                mgr.clone(),
            ))
        }),
        #[cfg(not(feature = "memory"))]
        memory_executor: None,
        skills_loader: shared.skills_loader.clone(),
        skills_registry: shared.skills_registry.clone(),
        web_search: {
            let web = &cfg.tools.web;
            let any_enabled = web.brave.enabled || web.duckduckgo.enabled || web.perplexity.enabled;
            if any_enabled {
                Some(nemesis_agent::loop_tools::WebSearchConfig {
                    // vault：api_key 支持 vault:/env:/yaml: 引用。
                    brave_api_key: if web.brave.api_key.is_empty() {
                        None
                    } else {
                        Some(crate::common::resolve_secret_or_empty(
                            &web.brave.api_key,
                            "tools.web.brave.api_key",
                        ))
                    },
                    brave_max_results: web.brave.max_results.max(1) as usize,
                    brave_enabled: web.brave.enabled,
                    duckduckgo_max_results: web.duckduckgo.max_results.max(1) as usize,
                    duckduckgo_enabled: web.duckduckgo.enabled,
                    // vault：api_key 支持 vault:/env:/yaml: 引用。
                    perplexity_api_key: if web.perplexity.api_key.is_empty() {
                        None
                    } else {
                        Some(crate::common::resolve_secret_or_empty(
                            &web.perplexity.api_key,
                            "tools.web.perplexity.api_key",
                        ))
                    },
                    perplexity_max_results: web.perplexity.max_results.max(1) as usize,
                    perplexity_enabled: web.perplexity.enabled,
                })
            } else {
                None
            }
        },
        spawn: Some(nemesis_agent::loop_tools::SpawnConfig {
            default_model: model_name.to_string(),
            max_concurrent: 4,
            // spawn 深度上限走 config（agents.subagent.max_depth，默认 1）。
            max_depth: cfg.agents.subagent.max_depth,
        }),
        // spawn 共享槽——factory 在 Arc<AgentLoop> 定型后注入
        // Weak<AgentLoop> 闭包（见 build_agent_loop 尾部 inject_spawn_fn）。
        spawn_slot: Some(spawn_slot.clone()),
        cluster_rpc: None, // Registered separately with call_fn
        mcp_tool_snapshot,
        workflow_engine: shared.workflow_engine.clone(),
        approval_manager: Some(shared.approval_slot.clone()),
        // question 工具 broker 槽（gateway 晚填 WebQuestionBroker）。
        question_broker: Some(shared.question_slot.clone()),
        // /：install_skill 工具审批门槽（gateway
        // 晚填 LateWebSkillsGate；headless 形态槽内恒 None = 无审批面直装）。
        skills_install_gate_slot: Some(shared.skills_install_gate_slot.clone()),
        // present_plan 默认不注册——主 loop / 项目 loop 的
        // builder 持 PlanPresentHandle 后覆写（plan 模式语义只在那两处可达；
        // 集群 loop 恒 None）。
        plan_present: None,
        // present_files 默认不注册——主 loop / 项目 loop 的
        // builder 注入交付物桶后覆写；集群 loop 恒 None（B 端 worker 的
        // 文件路径在 A 端用户文件系统上不存在，交付卡必是幽灵条目）。
        turn_deliverables: None,
        skills_manage_approval: cfg
            .skills
            .as_ref()
            .map(|s| s.manage_approval)
            .unwrap_or(false),
        // (half): opt-in claude_code delegation tool.
        claude_code_tool_enabled: cfg.agents.claude_code_tool.enabled,
        claude_code_tool_timeout_secs: cfg.agents.claude_code_tool.timeout_secs,
        claude_code_tool_permission_mode: cfg.agents.claude_code_tool.permission_mode.clone(),
        // (other half): opt-in codex delegation tool.
        codex_tool_enabled: cfg.agents.codex_tool.enabled,
        codex_tool_timeout_secs: cfg.agents.codex_tool.timeout_secs,
        codex_tool_sandbox: cfg.agents.codex_tool.sandbox.clone(),
        // opt-in read-only LSP tool (further gated by a PATH
        // probe at registration — see register_shared_tools).
        lsp_tool_enabled: cfg.agents.lsp_tool.enabled,
        lsp_tool_timeout_secs: cfg.agents.lsp_tool.timeout_secs,
        lsp_tool_idle_secs: cfg.agents.lsp_tool.idle_secs,
        // hand the shared LSP manager to the tool so the
        // gateway's graceful shutdown can close every language-server session
        // (see SharedResources.lsp_manager).
        #[cfg(feature = "lsp")]
        lsp_manager: Some(shared.lsp_manager.clone()),
        // SSRF guard host for web_fetch's per-hop redirect
        // re-check (same plugin the gateway's Layer-6 pipeline uses).
        #[cfg(feature = "security")]
        security: shared.security_plugin.clone(),
        #[cfg(not(feature = "security"))]
        security: None,
        // background process trio — gateway-level registry
        // singleton (survives agent restarts, kills residual children on drop).
        background_registry: Some(Arc::clone(&shared.background_registry)),
        // 2026-11-06：硬件总线工具（i2c/spi）注册总闸（`tools.hardware`
        // 段 `enabled` 键，默认关）。
        hardware_tools_enabled: cfg.tools.hardware.enabled,
        // （三批合并）：GPIO/I2C 地址白名单（config
        // `tools.hardware` 段；空段列表 = 内置默认 0x08-0x77）。
        hardware_policy: Some(Arc::new(nemesis_tools::hardware::HardwarePolicy {
            i2c_allow_ranges: cfg.tools.hardware.i2c_allow_ranges.clone(),
            i2c_deny_ranges: cfg.tools.hardware.i2c_deny_ranges.clone(),
        })),
        // workspace 写租约（`agents.lease_enabled` 默认开）——
        // 装配期消费：Some = register_shared_tools 把写类工具换成租约守
        // 卫包装（跨主 loop / 项目 loop / executor 子进程互斥写）。持
        // 有者名带进程 PID，前端 system.lease_status 展示。
        workspace_lease: if cfg.agents.lease_enabled {
            Some(Arc::new(
                nemesis_agent::workspace_lease::WorkspaceLease::new(
                    &shared.workspace_dir(),
                    &format!("gateway:pid:{}", std::process::id()),
                ),
            ))
        } else {
            None
        },
    };
    (config, spawn_slot)
}

/// 把「复用主 loop 跑 detached 轮次」的闭包注入 spawn 共享槽。
///
/// 捕获 `Weak<AgentLoop>`（计划原文写“捕获 Arc”——实现层偏离：Arc 直捕
/// 会成环 AgentLoop→tools→SpawnTool→slot→closure→AgentLoop 永久泄漏；
/// Weak 升级失败时诚实报错）。v1 边界：`model`/`channel`/`chat_id`/
/// `agent_id` 仅信息性（detached 轮次继承主 loop 的模型与 tier，子代理
/// 输出作为 tool 结果回灌、不走 channel 投递）；`DetachedOpts` 全默认
/// （再加 readonly/full 工具档）。
///
/// `bus` 用于后台路径的任务完成回灌——`background=true` 时闭包侧
/// `tokio::spawn` 包住 run_detached 并立即返回 `__BG_SPAWN__:{task_id}`
/// marker（loop 侧见 marker 后存续行快照 + 中间消息收尾回合）；任务完成
/// 后向 bus 发布 `subagent_continuation:{task_id}`（gate_inbound 拦截 →
/// dispatch_continuation → handle_cluster_continuation 全复用）。bus 在
/// 任意时刻可发布（不依赖注入时序、不 cluster 门控）——适配器 start()
/// 先建 inbound 通道后起步 loop，回灌消息经 bridge 正常入站；agent 重启
/// 窗口期发布（bridge 未挂）由 broadcast 无订阅者丢弃 + 启动期诚实丢失
/// 注入兜底（见 adapters.rs）。
pub(super) fn inject_spawn_fn(
    loop_arc: &Arc<nemesis_agent::r#loop::AgentLoop>,
    spawn_slot: &Arc<std::sync::OnceLock<nemesis_agent::loop_tools::SpawnFn>>,
    bus: &Arc<nemesis_bus::MessageBus>,
    // 件2：SubagentStart/Stop 事件源（None = 无桥，不发事件——集群/裸构建）。
    cc_bridge: Option<std::sync::Arc<nemesis_agent::cc_hooks::CcHookBridge>>,
) {
    use std::sync::atomic::{AtomicU64, Ordering};
    // 后台任务 id 计数器（同毫秒并发 spawn 防撞）。
    static BG_SPAWN_COUNTER: AtomicU64 = AtomicU64::new(0);
    // loop 侧镜像共享槽（client_spawn / WSAPI chat.spawn 消费同一闭包）。
    loop_arc.set_spawn_slot(spawn_slot.clone());
    let weak = Arc::downgrade(loop_arc);
    let bus = bus.clone();
    let _ = spawn_slot.set(Arc::new(
        move |agent_id: &str,
              task: &str,
              _model: &str,
              _channel: &str,
              chat_id: &str,
              tools_profile: &str,
              depth: usize,
              background: bool,
              role: &str,
              parent_session_key: &str| {
            let weak = weak.clone();
            let bus = bus.clone();
            // SpawnFn 的 Future 是 'static——&str 参数先拷贝成 owned。
            let agent_id = agent_id.to_string();
            let task = task.to_string();
            let tools_profile = tools_profile.to_string();
            let role_owned = role.to_string();
            // 子代理可见性：连接级 chat_id（`web:{connId}`，泵按它回推
            // 父会话房间）+ 父会话键（注册表台账 + 后台回灌路由信号）。
            let chat_id = chat_id.to_string();
            let parent_session_key = parent_session_key.to_string();
            // 显式角色覆盖（"" = 自动 → 沿用档位推导；非空 = 已过
            // from_slug 校验的 slug，直接转角色）。解析在闭包内做（&str
            // 参数不可跨 'static 边界）。
            let explicit_role = nemesis_agent::prompt::SubagentRole::from_slug(role);
            // Fn 闭包不能把捕获 move 进 async 块——每次调用 clone 进去。
            let cc_bridge = cc_bridge.clone();
            // 件2：SubagentStart/Stop 的 payload 素材（两路径共用；后台路径
            // 再 clone 进 spawn 任务）。
            let info = nemesis_agent::cc_hooks::SubagentInfo {
                agent_id,
                task,
                depth,
                background,
                tools_profile,
                loop_kind: "main",
            };
            Box::pin(async move {
                // 档位 → 白名单（readonly 缺省；full 不设限）。SpawnTool
                // 侧已校验过，这里再拦一次（防御纵深，闭包是唯一映射点）。
                let allowed_tools = match nemesis_agent::loop_tools::detached_tools_for_profile(
                    &info.tools_profile,
                ) {
                    Ok(a) => a,
                    Err(e) => return Err(e),
                };
                if !background {
                    let agent_loop = weak
                        .upgrade()
                        .ok_or_else(|| "agent loop is gone (shutdown in progress)".to_string())?;
                    // 件2：SubagentStart（exit 2 / JSON block = 拒绝 spawn，
                    // 已裁决；`?` 把阻断文案作 SpawnFn 的 Err 上抛）。
                    if let Some(bridge) = &cc_bridge {
                        bridge.dispatch_subagent_start(&info).await?;
                    }
                    tracing::debug!(
                        agent_id = %info.agent_id,
                        task_len = info.task.len(),
                        tools_profile = %info.tools_profile,
                        depth,
                        "[SpawnTool] running detached sub-agent (G0/G1/G2)"
                    );
                    // 子代理可见性：入册 + 挂取消令牌。会话键与注册表 id
                    // 同键（`subagent:{uuid}`，详情视图按 session_key join
                    // chat_event_log 环）；origin 按 agent_id 区分（client=
                    // 客户端手动派发，其余=模型自主 spawn）。
                    let registry_id = nemesis_agent::r#loop::AgentLoop::detached_session_key(None);
                    let cancel_token = Arc::new(tokio_util::sync::CancellationToken::new());
                    nemesis_agent::subagent_registry::global().begin(
                        nemesis_agent::subagent_registry::SubagentBegin {
                            id: registry_id.clone(),
                            parent_session_key: parent_session_key.clone(),
                            parent_chat_id: chat_id.clone(),
                            origin: if info.agent_id == "client" {
                                "client_spawn"
                            } else {
                                "model_spawn"
                            }
                            .to_string(),
                            agent_id: info.agent_id.clone(),
                            task: info.task.clone(),
                            role: role_owned.clone(),
                            tools_profile: info.tools_profile.clone(),
                            depth,
                            background: false,
                            loop_origin: "main".to_string(),
                            bg_task_id: None,
                        },
                    );
                    nemesis_agent::subagent_registry::global()
                        .attach_token(&registry_id, cancel_token.clone());
                    let result = agent_loop
                        .run_detached(
                            &info.task,
                            nemesis_agent::r#loop::DetachedOpts {
                                allowed_tools,
                                // 子代理深度（父深度 + 1，已过 max_depth 检查）
                                // 写到子 instance，供其 dispatch 再触发深度检查。
                                depth,
                                // prompt-pack pro：readonly 档 → 侦察员角色。
                                // 显式角色优先，缺省回落档位推导。
                                role: explicit_role.or_else(|| {
                                    nemesis_agent::loop_tools::detached_role_for_profile(
                                        &info.tools_profile,
                                    )
                                }),
                                // 子代理可见性：会话键/注册表 id 同键；取消
                                // 令牌直达执行链（注册表 cancel 面）。
                                session_key_override: Some(registry_id.clone()),
                                cancel_token: Some(cancel_token.clone()),
                                registry_id: Some(registry_id.clone()),
                                ..Default::default()
                            },
                        )
                        .await;
                    // 件2：SubagentStop（观察型）。前台无独立「取消」内核信号
                    // （estop 在 spawn 前拦截；upgrade 失败在 Start 前短路），
                    // Ok=completed / Err=failed 两态，Err 摘录随 payload。
                    if let Some(bridge) = &cc_bridge {
                        let outcome = if result.is_ok() {
                            "completed"
                        } else {
                            "failed"
                        };
                        bridge
                            .dispatch_subagent_stop(
                                &info,
                                outcome,
                                result.as_ref().err().map(String::as_str),
                            )
                            .await;
                    }
                    return result;
                }

                // 后台路径：tokio::spawn 包住 run_detached，本调用立即
                // 返回 marker。task_id 前缀 = loop_continuation:
                // BG_SPAWN_TASK_PREFIX（与恢复端 list_bg_spawn_pending_sync
                // 统一定义）。
                // 件2：SubagentStart 在任务真正 spawn 前发（拒绝则不产生
                // marker / 不 spawn 任务）。
                if let Some(bridge) = &cc_bridge {
                    bridge.dispatch_subagent_start(&info).await?;
                }
                let task_id = format!(
                    "{}{}_{}",
                    nemesis_agent::loop_continuation::BG_SPAWN_TASK_PREFIX,
                    chrono::Utc::now().timestamp_millis(),
                    BG_SPAWN_COUNTER.fetch_add(1, Ordering::Relaxed),
                );
                let bg_agent_id = info.agent_id.clone();
                let bg_task = info.task.clone();
                let bg_profile = info.tools_profile.clone();
                let bg_task_id = task_id.clone();
                let bg_bridge = cc_bridge.clone();
                let bg_info = info.clone();
                // 子代理可见性：后台任务在 marker 返回**前**入册
                // （列表立即可）+ 挂取消令牌（注册表「终止」直达执行链）。
                // 会话键与注册表 id 同键（详情视图 join chat_event_log 环）。
                let bg_registry_id = nemesis_agent::r#loop::AgentLoop::detached_session_key(None);
                let bg_cancel_token = Arc::new(tokio_util::sync::CancellationToken::new());
                nemesis_agent::subagent_registry::global().begin(
                    nemesis_agent::subagent_registry::SubagentBegin {
                        id: bg_registry_id.clone(),
                        parent_session_key: parent_session_key.clone(),
                        parent_chat_id: chat_id.clone(),
                        origin: if info.agent_id == "client" {
                            "client_spawn"
                        } else {
                            "model_spawn"
                        }
                        .to_string(),
                        agent_id: info.agent_id.clone(),
                        task: info.task.clone(),
                        role: role_owned.clone(),
                        tools_profile: info.tools_profile.clone(),
                        depth,
                        background: true,
                        loop_origin: "main".to_string(),
                        bg_task_id: Some(bg_task_id.clone()),
                    },
                );
                nemesis_agent::subagent_registry::global()
                    .attach_token(&bg_registry_id, bg_cancel_token.clone());
                let bg_parent_session_key = parent_session_key.clone();
                tokio::spawn(async move {
                    let result = match weak.upgrade() {
                        Some(agent_loop) => {
                            tracing::debug!(
                                agent_id = %bg_agent_id,
                                task_len = bg_task.len(),
                                tools_profile = %bg_profile,
                                depth,
                                task_id = %bg_task_id,
                                "[SpawnTool] background sub-agent started (G4)"
                            );
                            agent_loop
                                .run_detached(
                                    &bg_task,
                                    nemesis_agent::r#loop::DetachedOpts {
                                        // 后台任务沿用派生前已过检查的同一深度
                                        //（语义：后台化不另计深度）。
                                        allowed_tools,
                                        depth,
                                        // prompt-pack pro：readonly 档 → 侦察员角色。
                                        // 显式角色优先，缺省回落档位推导。
                                        role: explicit_role.or_else(|| {
                                            nemesis_agent::loop_tools::detached_role_for_profile(
                                                &info.tools_profile,
                                            )
                                        }),
                                        // 子代理可见性：会话键/注册表 id 同键；
                                        // 取消令牌直达执行链；完成状态自动收尾。
                                        session_key_override: Some(bg_registry_id.clone()),
                                        cancel_token: Some(bg_cancel_token.clone()),
                                        registry_id: Some(bg_registry_id.clone()),
                                        ..Default::default()
                                    },
                                )
                                .await
                        }
                        None => {
                            // 注册表兜底：upgrade 失败时 run_detached 未调用，
                            // opts 的 registry_id 收尾不会发生——显式落 Failed
                            // 防 Running 幽灵（正常路径由 opts 自动收尾，两路
                            // 互斥不双写）。
                            nemesis_agent::subagent_registry::global().finish(
                                &bg_registry_id,
                                nemesis_agent::subagent_registry::SubagentStatus::Failed,
                                None,
                                None,
                                Some("agent loop is gone (gateway shutting down)".to_string()),
                            );
                            Err("agent loop is gone (gateway shutting down)".to_string())
                        }
                    };
                    // 件2：SubagentStop（观察型）——完成后、continuation 回灌
                    // bus 前。「cancelled」= 任务 spawn 后 loop 已亡（路径
                    // 唯一取消形态，upgrade 失败）；「failed」= 其余 Err。
                    if let Some(bridge) = &bg_bridge {
                        let outcome = match &result {
                            Ok(_) => "completed",
                            Err(e) if e.contains("agent loop is gone") => "cancelled",
                            Err(_) => "failed",
                        };
                        bridge
                            .dispatch_subagent_stop(
                                &bg_info,
                                outcome,
                                result.as_ref().err().map(String::as_str),
                            )
                            .await;
                    }
                    let (content, status, error) = match &result {
                        Ok(text) => (text.clone(), "ok", None),
                        Err(e) => (
                            format!("[后台子代理任务失败] {}", e),
                            "error",
                            Some(e.clone()),
                        ),
                    };
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("status".to_string(), status.to_string());
                    if let Some(ref e) = error {
                        metadata.insert("error".to_string(), e.clone());
                    }
                    metadata.insert("source".to_string(), "background_subagent".to_string());
                    bus.publish_inbound(nemesis_types::channel::InboundMessage {
                        channel: "system".to_string(),
                        sender_id: format!(
                            "{}{}",
                            nemesis_types::constants::SUBAGENT_CONTINUATION_PREFIX,
                            bg_task_id
                        ),
                        chat_id: String::new(),
                        content,
                        media: Vec::new(),
                        // 子代理可见性：父会话键随行——项目会话派发的后台
                        // 任务完成回灌项目 loop（manager 按归属路由），不再全局
                        // 短路进主 loop（项目快照在项目 loop 的 continuation
                        // store，主 loop 加载必 miss）。
                        session_key: bg_parent_session_key,
                        correlation_id: String::new(),
                        metadata,
                        voice_playback: None,
                    });
                    tracing::info!(
                        task_id = %bg_task_id,
                        status,
                        "[SpawnTool] background sub-agent finished, completion published to bus"
                    );
                });
                Ok(format!("__BG_SPAWN__:{}", task_id))
            })
        },
    ));
}

/// 图像模型解析（generate_image 注册闸的单一裁决点）。
///
/// `tools.image_gen.model` 别名优先（别名可命中 model_name 或 wire model
/// 名，且必须是 images-openai 条目——工具只说 images/generations wire）；
/// 未指定时取**唯一** images-openai 条目，多于一个 = 歧义诚实报错。
/// 返回 (api_base, api_key, wire model 名)。
pub(crate) fn resolve_image_model(
    config: &nemesis_config::Config,
) -> Result<(String, String, String), String> {
    let alias = config
        .tools
        .image_gen
        .model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let entries: Vec<&nemesis_config::ModelConfig> = match alias {
        Some(a) => {
            let hits: Vec<_> = config
                .model_list
                .iter()
                .filter(|m| m.model_name == a || m.model == a)
                .collect();
            match hits.len() {
                0 => {
                    return Err(format!(
                        "tools.image_gen.model '{a}' 在 model_list 中不存在"
                    ));
                }
                1 => hits,
                n => {
                    return Err(format!(
                        "tools.image_gen.model '{a}' 命中 {n} 个条目，请在 model_list 去重"
                    ));
                }
            }
        }
        None => {
            let hits: Vec<_> = config
                .model_list
                .iter()
                .filter(|m| nemesis_types::capability::is_image_protocol(&m.protocol))
                .collect();
            match hits.len() {
                0 => {
                    return Err(
                        "未配置图像模型（model_list 无 images-openai 条目；先用 model add \
                         --protocol images-openai 添加，或以 tools.image_gen.model 指定别名）"
                            .to_string(),
                    );
                }
                1 => hits,
                n => {
                    return Err(format!(
                        "images-openai 条目有 {n} 个（歧义），请在 tools.image_gen.model 指定别名"
                    ));
                }
            }
        }
    };
    let m = entries[0];
    if !nemesis_types::capability::is_image_protocol(&m.protocol) {
        return Err(format!(
            "图像模型条目 '{}' 的 protocol 不是 images-openai（generate_image 只说 images/generations wire）",
            if m.model_name.is_empty() {
                &m.model
            } else {
                &m.model_name
            }
        ));
    }
    if m.api_base.trim().is_empty() {
        return Err(format!(
            "图像模型条目 '{}' 未配置 api_base",
            if m.model_name.is_empty() {
                &m.model
            } else {
                &m.model_name
            }
        ));
    }
    Ok((m.api_base.clone(), m.api_key.clone(), m.model.clone()))
}

/// 常驻路由的 agent 身份解析闭包（统一定义 = tasks.db agents 表
/// 经进程槽 manager 现读——loop 建成早于实体 seed（首次 boot 时序：
/// build_agent_loop 先于 agents 桥 seed），静态绑定必空，只能调用时解析）。
/// 谓词选中记录 → `(agent_id, exec_root)`（exec_root 来自记录——main=
/// workspace / project=项目目录 / selfbuilt=home，supervisor spawn 时写进
/// 子进程 env）；manager 未装 / 记录缺席 / 读库失败 = `None`（工具调用时
/// 诚实回落 per-call 路径）。
pub(super) fn persistent_resolve_by(
    pred: impl Fn(&nemesis_taskstore::AgentRecord) -> bool + Send + Sync + 'static,
) -> std::sync::Arc<dyn Fn() -> Option<(String, std::path::PathBuf)> + Send + Sync> {
    std::sync::Arc::new(move || {
        let mgr = crate::agents::manager()?;
        mgr.list()
            .ok()?
            .into_iter()
            .find(|rec| pred(rec))
            .map(|rec| {
                (
                    rec.agent_id.clone(),
                    std::path::PathBuf::from(&rec.exec_root),
                )
            })
    })
}

/// 第 4 步：档位路由执行器——常驻执行体的 per-agent 分派器。每次调用
/// **现读** agents 表记录的 `runtime_backend`（统一定义；`update_backend`
/// 落库后下一次调用即生效，无需重建 loop—— ⑦「上层无感切档」的实现锚）：
/// - `vm` → manager 的 VM 后端槽（未装配 = `Unavailable`，上层回落 per-call
///   并 info 留痕——与「开关关」同语义的诚实降级）；
/// - `process` / manager 未装 / 读库失败 → 进程档（保守默认， 行为不变；
///   off-state 记录默认全 `process`，行为与 字节一致）。
///
/// 进程监督器 Arc 在 loop 建成时捕获；VM 槽经 manager **每调用现取**——
/// VM supervisor 装配晚于 loop 构建（post_agent 晚注入纪律），静态绑定必空。
struct BackendRoutingExecutor {
    process: Arc<crate::agents::runtime::AgentRuntimeSupervisor>,
}

#[async_trait::async_trait]
impl nemesis_agent::remote_executor_tool::PersistentExecutor for BackendRoutingExecutor {
    async fn execute_persistent(
        &self,
        agent_id: &str,
        exec_root: &std::path::Path,
        tool: &str,
        args: &str,
        context: &nemesis_agent::context::RequestContext,
    ) -> Result<String, nemesis_agent::remote_executor_tool::PersistentCallError> {
        use nemesis_agent::remote_executor_tool::PersistentCallError;

        let mgr = crate::agents::manager();
        let backend = mgr
            .as_ref()
            .and_then(|m| m.get(agent_id).ok().flatten())
            .map(|rec| rec.runtime_backend)
            .unwrap_or_default();
        if backend == nemesis_taskstore::agents::runtime_backend::VM {
            let vm = mgr
                .as_ref()
                .and_then(|m| m.vm_backend())
                .ok_or_else(|| {
                    // 档位为 vm 的 agent 不经宿主执行——host_fallback=false
                    // 禁回落（宿主与 guest 文件视图不同）。
                    PersistentCallError::Unavailable {
                        message: format!(
                            "agent {agent_id} 档位为 vm 但 VM 后端未装配（vm feature 关闭或 gateway 未装配）"
                        ),
                        host_fallback: false,
                    }
                })?;
            return vm
                .execute_persistent(agent_id, exec_root, tool, args, context)
                .await;
        }
        self.process
            .execute_persistent(agent_id, exec_root, tool, args, context)
            .await
    }
}

/// 常驻路由构造（supervisor 在场 + 解析闭包 → PersistentRoute）。
/// None = 无 supervisor 形态（headless run / acp / 测试），工厂跳过挂接。
/// 第 4 步起 executor 一律经档位路由执行器（per-agent `runtime_backend`
/// 现读分派 vm/process——VM 槽缺席时 VM 档记 Unavailable 回落 per-call，
/// 进程档行为与 一致）。
pub(super) fn persistent_route_for(
    shared: &SharedResources,
    resolve: std::sync::Arc<dyn Fn() -> Option<(String, std::path::PathBuf)> + Send + Sync>,
) -> Option<nemesis_agent::PersistentRoute> {
    let process = shared.agent_runtime.as_ref()?;
    let executor: Arc<dyn nemesis_agent::remote_executor_tool::PersistentExecutor> =
        Arc::new(BackendRoutingExecutor {
            process: process.clone(),
        });
    Some(nemesis_agent::PersistentRoute { resolve, executor })
}

/// 行动代理 + 治理补全：connector 治理面装配——
/// `connector_find` / `connector_invoke` 注册 + 独占隐藏表供给器注入。
/// 主 loop（[`build_agent_loop`]）与统一工厂（[`build_agent_loop_from_spec`]：
/// 项目 loop / selfbuilt 记录 loop）共用，独占接管不再有项目会话旁路；
/// 集群 worker 维持不开行动面（v1 决策）。
///
/// **调用时机**：loop `Arc` 定型之后（`set_loop` 存 `Weak`、独占闭包回读
/// 快照都要 `Weak<Arc>`）；注册本身无条件（总闸可见性裁决全走运行期
/// 双闸：build_tool_defs 供给过滤 + dispatch 诚实拒绝，均 fresh-read
/// config）——注册无害，工具定义挂在注册表不等于供给给模型。注册期静态
/// 声明只给 connector_find（纯读编排面 → Internal LOW）；connector_invoke
/// **不静态声明**——per-op 风险经 `Tool::operation_override` 每次调用投影
/// （dispatch 嗅探写 metadata 三键，管线按 op_type 裁决），清单解析不出 =
/// 未知名 fail-closed CRITICAL 兜底。
///
/// 治理资产恒锚主 workspace（`shared.workspace_dir()`：连接器注册表 /
/// sidecar / 凭据 / 总闸 config.json）——项目 loop 的 exec_root 是项目目录，
/// 治理配置不随执行根走。
#[cfg(all(feature = "security", feature = "connectors"))]
pub(crate) fn register_connector_governance(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    shared: &Arc<SharedResources>,
) {
    nemesis_security::types::declare_tool_operation(
        crate::connector_tools::FIND_TOOL_NAME,
        nemesis_security::types::OperationType::Internal,
    );
    let connector_ws = shared.workspace_dir();
    let find_tool = Arc::new(crate::connector_tools::ConnectorFindTool::new(
        connector_ws.clone(),
    ));
    agent_loop.register_plugin_tool(
        crate::connector_tools::FIND_TOOL_NAME.to_string(),
        find_tool.clone(),
    );
    let invoke_tool = Arc::new(crate::connector_tools::ConnectorInvokeTool::new(
        connector_ws.clone(),
        shared.home.join("config.json"),
        shared.security_plugin.clone(),
    ));
    agent_loop.register_plugin_tool(
        crate::connector_tools::INVOKE_TOOL_NAME.to_string(),
        invoke_tool.clone(),
    );
    // 两个 connector 工具都要 loop 快照——invoke 解析镜像 op
    // （args_schema + 镜像成员判定），find 枚举镜像面（connector_find
    // 输出的 mirrors 数组）。
    find_tool.set_loop(Arc::downgrade(agent_loop));
    invoke_tool.set_loop(Arc::downgrade(agent_loop));
    // （收官批）：远程执行面——集群 RPC 通道在场时注入 remote_call_fn
    // （与 cluster_rpc 工具同源闭包；cluster 关闭 = None = target_node
    // 诚实报错）。凭据不跨节点：payload 无凭据，对端用自己的 vault。
    if let Some(ref call_fn) = shared.cluster_rpc_call_fn {
        invoke_tool.set_remote_call_fn(call_fn.clone());
    }
    // （独占接管）：独占隐藏表供给器——闭包每次现扫
    // （注册表 + sidecar + MCP 工具快照，磁盘真相），算出的完整工具名
    // 清单由 AgentLoop 并入供给过滤与 dispatch 出处感知闸（见
    // tool_defs/tool_dispatch）。
    let exclusive_loop = Arc::downgrade(agent_loop);
    agent_loop.set_exclusive_hidden_provider(Arc::new(move || {
        let Some(loop_arc) = exclusive_loop.upgrade() else {
            return Vec::new();
        };
        let names: Vec<String> = loop_arc
            .mcp_tool_snapshot()
            .read()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        nemesis_connectors::registry::exclusive_hidden_tools(&connector_ws, &names)
    }));
    info!("[AgentFactory] connector governance registered (find + invoke + exclusive provider)");
}

/// Register all tools and enable MCP on the given AgentLoop.
///
/// Shared between main agent and cluster agent. The caller is responsible for
/// registering cluster_rpc with call_fn after this call.
/// VM 档执行环境注记（审计收口）：vm 档 agent 每轮 build_messages 注入
/// guest 路径语义——治「模型拿宿主 Windows 绝对路径打 VM 文件工具吃
/// ENOENT」的迷惑（真机实测：list_dir C:\… → guest ENOENT）。常量文本
/// 跨轮字节稳定，prompt cache 前缀不受损。
const EXEC_ENV_VM_NOTE: &str = "exec_env: 文件/exec 工具当前经 VM guest（Linux）执行，工作区根 = /workspace（POSIX 路径形态）；不要使用宿主 Windows 绝对路径（C:\\…）";

pub(super) fn register_tools_and_mcp(
    agent_loop: &mut nemesis_agent::r#loop::AgentLoop,
    shared: &Arc<SharedResources>,
    tool_config: &nemesis_agent::SharedToolConfig,
    executor_channel: Option<&Arc<nemesis_agent::ExecutorChannel>>,
    persistent: Option<&nemesis_agent::PersistentRoute>,
) {
    let mut all_tools = nemesis_agent::register_shared_tools(tool_config);

    // Executor separation (Layer 1): replace the MOVE tool set with
    // RemoteExecutorTool bridges that proxy execute() to a child process.
    // Metadata (description / parameters / preview) delegates to the local
    // impl, so the LLM sees byte-identical schemas and the checkpoint safety
    // net still snapshots file writes.
    //
    // 常驻路由在场时每个桥挂 persistent 询问（身份解析成功 → 常驻
    // 执行体；Unavailable → 本通道 per-call 回落；ToolFailed → 原样上抛）。
    if let Some(channel) = executor_channel {
        for name in nemesis_agent::MOVE_TOOLS {
            if let Some(local) = all_tools.remove(*name) {
                let mut bridge = nemesis_agent::RemoteExecutorTool::new(
                    (*name).to_string(),
                    local,
                    channel.clone(),
                );
                if let Some(route) = persistent {
                    bridge = bridge.with_persistent(route.clone());
                }
                all_tools.insert((*name).to_string(), Box::new(bridge));
            }
        }
        // VM 档执行环境注记（审计收口）：桥真正挂上才安装 resolver——
        // 每轮 build_messages 现查档位（与 BackendRoutingExecutor 同源
        // resolve 闭包），vm 档注入 guest 路径语义行，process 档空串字节
        // 不变。安装闸放在桥挂载成功分支内：executor.enabled=false 的死
        // 配置形态工具根本不路由 VM，注入「文件走 VM」会是主动撒谎（该
        // 形态由下方 else 臂 warn 负责）。
        if let Some(route) = persistent {
            let resolve = route.resolve.clone();
            agent_loop.set_exec_env_resolver(std::sync::Arc::new(move || {
                let Some((agent_id, _)) = resolve() else {
                    return String::new();
                };
                crate::agents::manager()
                    .as_ref()
                    .and_then(|m| m.get(&agent_id).ok().flatten())
                    .filter(|rec| {
                        rec.runtime_backend == nemesis_taskstore::agents::runtime_backend::VM
                    })
                    .map(|_| EXEC_ENV_VM_NOTE.to_string())
                    .unwrap_or_default()
            }));
        }
    } else {
        // 死配置闸（审计发现的诚实缺口告警半边）：executor.enabled=false
        // 时上面的桥挂载整段跳过，BackendRoutingExecutor 无消费点——agents
        // 表任何 vm 档记录在此形态下 runtime_backend=vm 静默无效（工具全在
        // 宿主执行）。装配期是同时知道「桥没挂」与「有 vm 档」两个事实的
        // 唯一低成本检测点，每次 loop 重建都会过（覆盖启动/重启/改配后的
        // 下一次重建）。
        let vm_agents: Vec<String> = crate::agents::manager()
            .as_ref()
            .and_then(|m| m.list().ok())
            .map(|recs| {
                recs.iter()
                    .filter(|r| r.runtime_backend == nemesis_taskstore::agents::runtime_backend::VM)
                    .map(|r| r.agent_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if !vm_agents.is_empty() {
            tracing::warn!(
                agents = ?vm_agents,
                "[AgentFactory] executor 分离未启用（config.json executor.enabled=false），但 agents 表存在 vm 档记录：runtime_backend=vm 静默无效，文件/exec 工具仍在宿主执行。启用 VM 后端需置 executor.enabled=true 并重启"
            );
        }
    }

    // browser 工具（VM 档 guest 内 headless chromium + CDP，guest
    // 执行体 = nemesis-guest src/browser.rs）。注册闸 = persistent 路由
    // 在场（gateway 形态；headless run/acp 无 agents 表解析面，注册了也
    // 永远解析失败——诚实缺省，generate_image 同款注册闸风格）。执行时
    // 现查 agents 表档位，非 VM 档诚实报错（进程档没有浏览器执行体，
    // 不回落宿主——与 ToolFailed「绝不回落」语义同源）。
    if let Some(route) = persistent {
        all_tools.insert(
            crate::browser_tool::TOOL_NAME.to_string(),
            Box::new(crate::browser_tool::VmBrowserTool::new(
                route.resolve.clone(),
            )),
        );
    }

    let tool_count = all_tools.len();
    for (name, tool) in all_tools {
        agent_loop.register_tool(name, tool);
    }

    // 集群专业职能框架：generate_image 工具（三 loop 统一注册面：主/
    // 项目/集群）。注册闸 = 图像模型可解析——未配置/歧义/缺 api_base =
    // 不注册 + info 诚实缺省（图像是可选能力，不炸装配）；解析失败的
    // 具体原因打在日志里，模型管理页可照单补配。
    {
        let cfg_handle = shared.config_store.handle();
        let cfg_guard = cfg_handle.read();
        match resolve_image_model(&cfg_guard) {
            Ok((api_base, api_key, model)) => {
                let timeout = cfg_guard.tools.image_gen.effective_timeout_secs();
                let output_dir = shared.workspace_dir().join("images");
                agent_loop.register_tool(
                    crate::image_gen_tool::TOOL_NAME.to_string(),
                    Box::new(crate::image_gen_tool::GenerateImageTool::new(
                        api_base,
                        api_key,
                        model.clone(),
                        timeout,
                        output_dir,
                    )),
                );
                info!("[AgentFactory] generate_image tool registered (model={model})");
            }
            Err(e) => {
                info!("[AgentFactory] generate_image 未注册: {e}");
            }
        }
    }

    if shared.mcp_enabled {
        agent_loop.enable_mcp_reload(shared.mcp_config_path.clone());
    }

    info!(
        "[AgentFactory] Tools registered: {}{}",
        tool_count,
        if shared.mcp_enabled { " + MCP" } else { "" }
    );
}

// ---------------------------------------------------------------------------
// build_cluster_agent_loop — cluster agent factory
// ---------------------------------------------------------------------------
/// tool_policy JSON →（hidden 折算表, allow 白名单）。deny ∪ hidden
/// 原样合并（通配语义由消费方 `tool_name_matches_hidden` 承担）；allow
/// 原样透传（白名单折叠需注册面，build 侧算）。非字符串条目忽略、
/// 空串剔除；非对象 / 缺键 = 空（无策略语义）。
pub(crate) fn tool_policy_fold(policy: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    let pull = |key: &str| -> Vec<String> {
        policy
            .get(key)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default()
    };
    let mut hidden = pull("hidden");
    for name in pull("deny") {
        if !hidden.contains(&name) {
            hidden.push(name);
        }
    }
    (hidden, pull("allow"))
}

/// persona_refs JSON → 人格目录。`{"dir": "<abs path>"}` = Some；
/// Null / 缺 dir / 空串 = None（ContextBuilder 锚 exec_root）。路径不在此
/// 校验存在性——build 侧目录缺席时 ContextBuilder 自然回落空身份（与
/// 项目目录无 IDENTITY 同态），注册时不因软引用缺席拒绝创建。
pub(crate) fn persona_dir_from_refs(refs: &serde_json::Value) -> Option<std::path::PathBuf> {
    refs.get("dir")
        .and_then(|d| d.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
}

/// 记忆分域 layout 统一定义：namespace → 增强内存 data_dir。
/// 自建 agent 独占域 = `workspace/memory/<namespace>/memory_vector`
/// （episodic jsonl / 向量库 / graph 全部落域内，主 corpus 不可）。
#[cfg(feature = "memory")]
fn namespaced_memory_data_dir(workspace: &std::path::Path, namespace: &str) -> std::path::PathBuf {
    workspace
        .join("memory")
        .join(namespace)
        .join("memory_vector")
}

/// 记忆分域选域决策统一定义。空 namespace（main/项目镜像）= None
/// （剥离，增强内存是主 workspace 资产，行为零变化）；非空且增强内存
/// 总开关开 = 本域 manager；总开关关 = None（全域一致关闭，自建域不旁路）。
#[cfg(feature = "memory")]
pub(super) fn memory_manager_for_namespace(
    shared: &SharedResources,
    namespace: &str,
) -> Option<Arc<nemesis_memory::manager::MemoryManager>> {
    if namespace.is_empty() {
        return None;
    }
    memory_manager_for_data_dir(
        shared,
        &namespaced_memory_data_dir(&shared.workspace_dir(), namespace),
    )
}

/// （项目 home 收编）：显式 data_dir 选域（namespace 链路的
/// 姊妹函数——namespace 的唯一历史用途就是拼 data_dir，项目形态 data_dir
/// 落 agent home，不经 workspace 拼路径）。总开关关 = None（全域一致关闭，
/// 项目域不旁路）；目录由调用方（build 侧物化）保证在场。
#[cfg(feature = "memory")]
pub(super) fn memory_manager_for_data_dir(
    shared: &SharedResources,
    data_dir: &std::path::Path,
) -> Option<Arc<nemesis_memory::manager::MemoryManager>> {
    shared.memory_manager.as_ref()?;
    Some(Arc::new(
        nemesis_memory::manager::MemoryManager::with_config_dir(
            data_dir,
            &shared.workspace_dir().join("config"),
        ),
    ))
}
/// 距下一个本地午夜的秒数（naive 墙钟差，不做时区查找）。
///
/// 旧实现 `Local.from_local_datetime(..).unwrap()` 在「本地午夜不存在」的
/// 时区（春令时跳变恰好落在 00:00——古巴/智利等真实存在）拿到
/// `LocalResult::None` → unwrap panic → 清扫 task 死亡，SessionStore /
/// rpc_cache / spill 三条 TTL 循环进程内永久停摆。naive 墙钟差天然免疫：
/// 跳变夜墙钟差比真实时长短 1h，睡完恰好落在跳变后第一个有效时刻
/// （01:00）；回拨夜会早醒一次（幂等清理，无害）。
pub(super) fn secs_until_next_local_midnight<Tz: chrono::TimeZone>(
    now: chrono::DateTime<Tz>,
) -> u64 {
    let tomorrow_midnight = now
        .date_naive()
        .succ_opt()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .unwrap_or_else(|| now.date_naive().and_hms_opt(23, 59, 59).unwrap());
    tomorrow_midnight
        .signed_duration_since(now.naive_local())
        .num_seconds()
        .max(60) as u64
}

/// Spawn a background task that runs `cleanup_old_sessions(7)` every day at local midnight.
///
/// The task sleeps until the next local midnight, runs cleanup, then loops.
/// If the tokio runtime shuts down (gateway stop), the task is cancelled and
/// no further cleanups run — startup cleanup in `build_*_agent_loop` covers
/// the next start.
///
/// `label` is used for logging only ("Main" / "Cluster").
pub(super) fn spawn_daily_cleanup(
    store: Arc<nemesis_agent::session::SessionStore>,
    label: &str,
    cc_bridge: Option<std::sync::Arc<nemesis_agent::cc_hooks::CcHookBridge>>,
) {
    let label = label.to_string();
    tokio::spawn(async move {
        loop {
            let sleep_secs = secs_until_next_local_midnight(chrono::Local::now());

            tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;

            // detailed 版返回被删 key，逐个触发方言 SessionEnd
            // （观察型；无桥/cluster 侧 = 只清理不触发）。
            let removed = store.cleanup_old_sessions_detailed(7);
            let deleted = removed.len();
            if deleted > 0 {
                info!(
                    deleted,
                    label = %label,
                    "[AgentFactory] {} SessionStore daily midnight cleanup (TTL=7d)",
                    label
                );
                if let Some(bridge) = &cc_bridge {
                    let bridge = std::sync::Arc::clone(bridge);
                    let keys = removed.clone();
                    tokio::spawn(async move {
                        for key in &keys {
                            bridge.on_session_end(key, "expired").await;
                        }
                    });
                }
            }
        }
    });
}

/// rpc_cache continuation-snapshot TTL sweep (mirrors `spawn_daily_cleanup`).
///
/// `ContinuationManager::cleanup_old_snapshots(7d)` had no
/// production caller — snapshots whose callback never arrives accumulated
/// in `{workspace}/cluster/rpc_cache/` forever (and would be re-recovered
/// into memory by `recover_to_manager` after every restart). This runs a
/// sweep immediately at startup, then daily at local midnight, same as the
/// SessionStore TTL. Once-guard: `build_agent_loop` runs again on agent
/// rebuilds — only the first spawn survives for the process lifetime.
pub(super) fn spawn_rpc_cache_cleanup(mgr: Arc<nemesis_agent::ContinuationManager>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RPC_CACHE_CLEANUP_SPAWNED: AtomicBool = AtomicBool::new(false);
    if RPC_CACHE_CLEANUP_SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    const TTL: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
    tokio::spawn(async move {
        // Startup sweep.
        let removed = mgr.cleanup_old_snapshots(TTL).await;
        if removed > 0 {
            info!(
                removed,
                "[AgentFactory] rpc_cache continuation startup cleanup (TTL=7d)"
            );
        }
        loop {
            let sleep_secs = secs_until_next_local_midnight(chrono::Local::now());

            tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;

            let removed = mgr.cleanup_old_snapshots(TTL).await;
            if removed > 0 {
                info!(
                    removed,
                    "[AgentFactory] rpc_cache continuation midnight cleanup (TTL=7d)"
                );
            }
        }
    });
}

/// daily-midnight spill retention sweep (mirrors `spawn_daily_cleanup`).
///
/// Retention days are re-read from config.json on EVERY run, so a dashboard
/// edit of `agents.spill_retention_days` applies at the next midnight without
/// a restart; `0` skips that night's sweep. Once-guard: `build_agent_loop`
/// runs again on agent rebuilds (persona activate etc.) — only the first
/// spawn survives for the process lifetime (one home per process).
pub(super) fn spawn_daily_spill_cleanup(
    spill_root: std::path::PathBuf,
    config_path: std::path::PathBuf,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SPILL_CLEANUP_SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPILL_CLEANUP_SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            let sleep_secs = secs_until_next_local_midnight(chrono::Local::now());

            tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;

            // Live-read retention (default 7 on unreadable config; negative -> 0 -> skip).
            let retention = nemesis_config::load_config(&config_path)
                .map(|c| c.agents.defaults.spill_retention_days)
                .unwrap_or(7)
                .max(0) as u64;
            if retention == 0 {
                continue;
            }
            let deleted = nemesis_agent::spill::cleanup_expired(&spill_root, retention);
            if deleted > 0 {
                info!(
                    deleted,
                    retention_days = retention,
                    "[AgentFactory] spill daily midnight cleanup (TTL={}d)",
                    retention
                );
            }
        }
    });
}

/// (.4): semantic-embedder assembly helper. The providers Router is a
/// complete-but-not-yet-wired module (no production consumer builds it
/// today); this helper is the ready-made bridge for when it enters the
/// factory chain: it wires the memory manager's embedding backend (ONNX
/// plugin when loaded, n-gram fallback otherwise) into the router as the
/// SemanticEmbedder. Compilation-verified; invoked at Router adoption time.
#[allow(dead_code)]
#[cfg(feature = "memory")]
pub fn attach_semantic_embedder(
    router: &nemesis_providers::router::Router,
    memory: Option<&std::sync::Arc<nemesis_memory::manager::MemoryManager>>,
) {
    let Some(memory) = memory else {
        return; // no memory subsystem => Semantic degrades to fallback
    };
    let mgr = memory.clone();
    router.set_semantic_embedder(std::sync::Arc::new(move |text: &str| mgr.embed_text(text)));
}
