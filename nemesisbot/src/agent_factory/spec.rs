// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 agent_factory.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// 常驻 AgentLoop 统一工厂的最小差异面。
///
/// 项目 loop 与 agents 表记录（自建/main）loop 的装配体完全同源
/// （安全 8 层/estop/围栏/discipline 全生效），差异收敛为四个字段：
/// - `exec_root`：工作区锚（项目目录 / agent home / 主 workspace）
/// - `display_name`：日志与事件可观测名
/// - `checkpoint_key`：影子库键（logs/project_checkpoints/<key>）——项目
///   形态=project_id（存量连续性），记录形态=agent_id
/// - `instructions_trusted`：指令信任起点（注册表受信态 / 记录形态默认
///   verbatim——信任策略的运行时热更新仍锚各 loop 既有通道）
pub struct AgentLoopSpec {
    pub exec_root: std::path::PathBuf,
    pub display_name: String,
    pub checkpoint_key: String,
    pub instructions_trusted: bool,
    /// 记忆命名空间：空 = 主 workspace 域（main/项目镜像，增强内存
    /// 剥离维持）；`agents/<id>` = 自建 agent 独占域（增强内存挂本域
    /// manager，jsonl/向量库落 `memory/<ns>/`）。读取点在 memory 装配块
    /// 内（memory 关的收窄态只写不读，诚实 allow）。
    #[cfg_attr(not(feature = "memory"), allow(dead_code))]
    pub memory_namespace: String,
    /// 常驻路由身份解析闭包（None = 该形态不挂常驻——当前全部
    /// 形态都有解析谓词，None 仅出现在测试手搓 spec）。闭包在每次工具
    /// 调用时现查 agents 表（boot 时序：loop 建成早于实体 seed）。
    pub persistent_resolve:
        Option<std::sync::Arc<dyn Fn() -> Option<(String, std::path::PathBuf)> + Send + Sync>>,
    /// （三方复评）：记录级能力档位契约（`auto|mini|
    /// normal|big`，空 = auto = config 驱动，主/项目形态恒空零影响）。
    /// 非空时 build 侧解析成 [`nemesis_types::capability:ModelTier`] 后
    /// `set_tier_override` 钉死——config 重解析/换模型不覆盖记录档位。
    pub tier: String,
    /// tool_policy 折算的隐藏清单（deny ∪ hidden 原样，通配语义同
    /// `agents.hidden_tools`）。 后 allow 白名单不再折入此表。
    pub tool_policy_hidden: Vec<String>,
    /// tool_policy allow 白名单原样。（二轮复评）：非空时
    /// build 侧 `set_tool_policy_allow` **直挂 loop 槽**（旧「注册面 ∖ allow」
    /// 折算取缔——折算只吃注册期快照，MCP 热加载的新工具漏拦；槽值双闸
    /// 每回合现读，动态新工具天然落闸）。空 = 无白名单语义。
    pub tool_policy_allow: Vec<String>,
    /// 人格目录覆盖（persona_refs `{"dir": <abs path>}`）。Some 时
    /// system prompt 的身份装配根用它（IDENTITY/SOUL 等从该目录读），
    /// workspace 根仍锚 exec_root（指令链/围栏不随人格目录走）。
    /// None = ContextBuilder 锚 exec_root（主/项目形态零变化）。
    pub persona_dir: Option<std::path::PathBuf>,
    /// per-agent 浏览器 profile 目录。Some = 本 loop
    /// 装配的 MCP server（配置 opt-in `browser_profile: true` 的浏览器类）
    /// spawn 时注入 `--user-data-dir <dir>`（登录态/cookie per-agent 隔离，
    /// 落 agent home browser-profile）。None = env 回落语义（主/项目形态
    /// 恒 None 零变化；selfbuilt 由 AgentLoopManager 按 agent home 赋值）。
    pub browser_profile: Option<std::path::PathBuf>,
    /// （项目 home 收编）：项目 agent home 目录（agents 表
    /// `home_path`，Bot 域数据根）。Some = 围栏第二根 + 记忆/plan/checkpoint
    /// 落 home；None = 全部缺席（主/记录形态零变化；项目形态记录缺位时
    /// 也为 None——agents 记录是增强面，缺位 warn 降级不拒建 loop）。
    pub agent_home: Option<std::path::PathBuf>,
    /// 子代理可见性：本 loop 的宿主形态标识——`project:{id}`（项目
    /// loop）/ `agent:{id}`（记录/自建 loop）/ `main`（主 loop，不经 spec
    /// 构建）。子代理注册表 `loop_origin` 字段的赋值源：详情视图按它区分
    /// 「哪个宿主派发的子代理」，项目页/agent 页过滤台账用。
    pub loop_origin: String,
}

impl AgentLoopSpec {
    /// 项目注册表条目 → 规格（语义零变化；checkpoint 键=project_id
    /// 保存量影子库连续）。
    pub fn from_project(project: &crate::projects::registry::ProjectEntry) -> Self {
        // 项目镜像记录（kind=project + project_id 匹配）——记录由
        // sync_project_agents 物化，exec_root=项目目录。字段先克隆进
        // 'static 闭包（借用不能跨出工厂函数）。
        let project_id = project.id.clone();
        // 子代理可见性：宿主形态标识（先算——project_id 随后 move
        // 进 persistent_resolve 闭包）。
        let loop_origin = format!("project:{project_id}");
        // （项目 home 收编）：项目镜像记录现查一次拿 home_path。
        // 时序保证：sync_project_agents 先于项目 loop 构建——gateway boot
        // 在 agents seed（install_manager + 批量 sync）之后才 start_all
        // （真机 E2E 返工：此前 start_all 先于 seed，boot 期查询恒落空）；
        // 运行期 projects.create 走 spawn_project 内置 per-entry sync。
        // 记录缺位（seed warn 降级 / tasks.db 打不开）= None，装配侧
        // warn + home 面全部缺席（围栏单根 / 记忆剥离 / plan 落项目目录
        // 旧行为），不拒建 loop。
        let agent_home = crate::agents::manager().and_then(|mgr| {
            mgr.list().ok()?.into_iter().find_map(|rec| {
                let matched = rec.kind == nemesis_taskstore::agents::kind::PROJECT
                    && rec.project_id.as_deref() == Some(project_id.as_str());
                if !matched {
                    return None;
                }
                if rec.home_path.is_empty() {
                    None
                } else {
                    Some(std::path::PathBuf::from(rec.home_path))
                }
            })
        });
        Self {
            exec_root: project.path.clone(),
            display_name: project.name.clone(),
            checkpoint_key: project.id.clone(),
            instructions_trusted: project.instructions_trusted,
            memory_namespace: String::new(),
            persistent_resolve: Some(persistent_resolve_by(move |rec| {
                rec.kind == nemesis_taskstore::agents::kind::PROJECT
                    && rec.project_id.as_deref() == Some(project_id.as_str())
            })),
            // 项目镜像形态零差异（tier/tool_policy/persona 是 selfbuilt
            // 记录的契约面，项目镜像不带）。
            tier: String::new(),
            tool_policy_hidden: Vec::new(),
            tool_policy_allow: Vec::new(),
            persona_dir: None,
            // 项目 loop 共享形态（浏览器 profile 不 per-project）。
            browser_profile: None,
            agent_home,
            loop_origin,
        }
    }

    /// agents 表记录 → 规格。checkpoint 键=project_id（项目镜像形态保
    /// 连续）否则 agent_id；`instructions_trusted` 默认 verbatim（记录是
    /// bot 管理的身份，home 由本系统物化；项目注册表的信任策略不经本
    /// 路径——那由 from_project 承载）。记忆命名空间按记录走（selfbuilt
    /// = `agents/<id>`，project 镜像 = 空）。
    ///
    /// （三方复评）：tier/tool_policy/persona_refs
    /// 全量映射进 spec（消费面 [`AgentLoopManager`] 落地，生产调用方
    /// 在场；键选择规则由 projects_factory_tests 的 `spec_from_record`
    /// 两则钉住）。
    pub fn from_record(record: &nemesis_taskstore::AgentRecord) -> Self {
        // 记录形态按 agent_id 精确解析（记录即身份真相源）。id 先克隆
        // 进 'static 闭包（同 from_project）。
        let agent_id = record.agent_id.clone();
        let (tool_policy_hidden, tool_policy_allow) = tool_policy_fold(&record.tool_policy);
        Self {
            exec_root: std::path::PathBuf::from(&record.exec_root),
            display_name: record.name.clone(),
            checkpoint_key: record
                .project_id
                .clone()
                .unwrap_or_else(|| record.agent_id.clone()),
            instructions_trusted: true,
            memory_namespace: record.memory_namespace.clone(),
            persistent_resolve: Some(persistent_resolve_by(move |rec| rec.agent_id == agent_id)),
            tier: record.tier.clone(),
            tool_policy_hidden,
            tool_policy_allow,
            persona_dir: persona_dir_from_refs(&record.persona_refs),
            // 记录映射不携带（workspace 根不在记录视野内）——selfbuilt
            // 由 AgentLoopManager 按 agent home 赋值（nemesis-path 单一
            // 派生式 `resolve_agent_browser_profile_in_workspace`）。
            browser_profile: None,
            // 记录形态（selfbuilt/main）不设第二根——selfbuilt 的 home
            // 就是 exec_root（两根重合无意义），main 无 home 语义。
            agent_home: None,
            loop_origin: format!("agent:{}", record.agent_id),
        }
    }
}
/// 统一常驻 loop 工厂（项目 loop 与 agent 记录 loop 同源）。
///
/// 差异面全部来自 [`AgentLoopSpec`]；其余装配（config 解析、安全 8 层、
/// estop、围栏强制 restrict、discipline、checkpoint 影子库、事件 hook）
/// 与主 loop 同一构造链。
pub fn build_agent_loop_from_spec(
    shared: &Arc<SharedResources>,
    spec: &AgentLoopSpec,
    session_store: Arc<nemesis_agent::session::SessionStore>,
) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>> {
    use nemesis_agent::r#loop::AgentLoop;
    use nemesis_agent::types::AgentConfig;

    // 路径注册时已 canonicalize，这里只做存在性防御——目录消失诚实报错
    // （manager 侧 warn + skip，不炸 gateway）。
    if !spec.exec_root.is_dir() {
        anyhow::bail!("执行根目录不存在或不可用: {}", spec.exec_root.display());
    }
    let project_dir = spec.exec_root.clone();

    // （项目 home 收编）：项目 agent home 物化（memory/plans/
    // checkpoints 子目录 ensure）。None = 非项目形态（主/记录）零变化。
    // 物化失败与「执行根目录不存在」同姿态——诚实报错（manager 侧 warn +
    // skip 不炸 gateway）：home 数据面写不进去的 loop 装了也是哑的。
    let agent_home = match spec.agent_home.clone() {
        Some(home) => {
            for sub in ["memory", "plans", "checkpoints"] {
                let dir = home.join(sub);
                std::fs::create_dir_all(&dir).map_err(|e| {
                    anyhow::anyhow!("agent home 子目录物化失败: {} ({})", dir.display(), e)
                })?;
            }
            Some(home)
        }
        None => None,
    };

    // 1. config.json + 模型解析：与主 loop 同源（同一 config.json 唯一依据）。
    //    模型热切（修订）：spawn 后不再"重启才生效"——
    //    models.set_default / agent start 经 ProjectsBridge::reload_provider_all
    //    调 ProjectLoopManager::reload_providers 对在跑项目 loop 热换 provider
    //    （与主 loop 的 runtime swap 同构）。
    let config_path = shared.home.join("config.json");
    let cfg = nemesis_config::load_config(&config_path)
        .map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))?;
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
        workspace: project_dir.to_string_lossy().to_string(),
        connect_mode: resolution.connect_mode,
        protocol: resolution.protocol.clone(),
        timeout_secs: resolution.timeout_secs,
        account_id: String::new(),
        headers: HashMap::new(),
    };
    // 双击直启 goal：项目 loop 装配 or_null（主实例无模型时项目 loop 照建）。
    let (provider, create_warn) = nemesis_providers::factory::create_provider_or_null(&factory_cfg);
    if let Some(w) = create_warn {
        warn!(
            "[AgentFactory] Project agent provider create failed: {} — NullProvider 装配",
            w
        );
    }
    let provider_arc: Arc<dyn nemesis_providers::router::LLMProvider> = provider;

    // 2. 轻量基线 system prompt：项目目录自身的 IDENTITY/SOUL（若用户放置），
    //    不 load_skills（skills 是主 workspace 资产， 边界不进项目 loop）。
    //    prompt 体系与主 loop 同一开关（agents.prompt_system）。
    let prompt_system = nemesis_agent::prompt::PromptSystem::from_config(&cfg.agents);
    let (system_prompt, prompt_split) = {
        // persona_dir 覆盖在场 = 身份装配根换人格目录（IDENTITY/SOUL
        // 等从该目录读）；workspace 指令链/围栏仍锚 exec_root（下方
        // set_workspace_root 不受影响）。目录缺席 → ContextBuilder 自然
        // 回落空身份（软引用语义，不因人格目录缺席拒绝装配）。
        let prompt_root = spec.persona_dir.as_deref().unwrap_or(project_dir.as_path());
        let mut context_builder = nemesis_agent::context::ContextBuilder::new(prompt_root);
        // （项目 home 收编）：人格顺序拼接 home → 项目目录
        // （单一文件拆两份，两段都渲染、不合并不去重）。override
        // 只作用于 bootstrap 六文件——prompt_root/记忆模板/skills 目录/
        // workspace 显示等其余段零变化。
        if let Some(ref home) = agent_home {
            context_builder.set_bootstrap_roots(vec![home.clone(), project_dir.clone()]);
        }
        context_builder.set_prompt_system(prompt_system);
        context_builder.build_system_prompt_with_sections(false)
    };
    info!(
        project = %spec.display_name,
        "[AgentFactory] project system prompt built ({} chars, prompt_system={:?})",
        system_prompt.len(),
        prompt_system
    );

    // 3. AgentConfig + tier（与主 loop 同一解析链）。
    let cfg_json: serde_json::Value = nemesis_utils::fsutil::read_config_string(&config_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let resolved_tier = nemesis_types::capability::resolve_active_tier(&cfg_json, &model_name);
    let agent_config = AgentConfig {
        model: model_name.clone(),
        system_prompt: if system_prompt.is_empty() {
            None
        } else {
            Some(system_prompt)
        },
        system_prompt_split: Some(prompt_split),
        max_turns: if cfg.agents.defaults.max_tool_iterations <= 0 {
            0
        } else {
            cfg.agents.defaults.max_tool_iterations as u32
        },
        tools: Vec::new(),
        tool_fence_profession: None,
        models: std::collections::HashMap::new(),
    };

    let max_continuation_permits = cfg.agents.defaults.max_continuation_permits.max(0) as usize;
    let concurrent_mode = nemesis_agent::r#loop::parse_concurrent_mode(
        &cfg.agents
            .defaults
            .concurrent_request_mode
            .trim()
            .to_lowercase(),
    );
    let queue_size = cfg.agents.defaults.queue_size.max(1) as usize;
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(ProviderAdapter::new(
            provider_arc.clone(),
            model_name.clone(),
        )),
        agent_config,
        shared.agent_outbound_tx.clone(),
        concurrent_mode,
        queue_size,
        max_continuation_permits,
    );
    // 项目 loop 与主 loop 同源路由 config（web 会话键预置
    // agent: 前缀走 honor 分支不受影响；缺省 = 硬编码默认同形零回归）。
    agent_loop.set_route_resolver(build_route_resolver(&cfg));
    agent_loop.set_identity_links(cfg.session.identity_links.clone());
    // D-4（多会话并行清账）：项目 loop 同主 loop——并发 turn
    // 上限安全阀（同会话仍串行）。
    agent_loop.set_max_concurrent_turns(cfg.agents.defaults.max_concurrent_turns.max(0) as usize);
    agent_loop.set_tier(resolved_tier);
    agent_loop.set_config_path(config_path.clone());
    // （凭据 vault）：项目 loop 同款凭据卡装配（broker 未
    // install = None 零回归；config 与主 loop 同一份，patch 语义一致）。
    agent_loop.set_provider_reloader(Arc::new(ModelProviderReloader {
        home: shared.home.clone(),
        workspace: shared.workspace_dir(),
    }));
    #[cfg(feature = "security")]
    if let Some(broker) = crate::credential_broker::current() {
        agent_loop.set_credential_broker(broker);
    }
    // per-agent 浏览器 profile（spec 携带；须在
    // enable_mcp_reload 之前——装配期随 manager 转 MCP spawn 链）。None =
    // 主/项目形态零变化。
    agent_loop.set_browser_profile(
        spec.browser_profile
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    // 记录级 tier 钉死（spec.tier 非空且非 auto）。Some = 记录是唯一
    // 权威定义——refresh_active_tier 短路，config 重解析/换模型不覆盖记录
    // 档位；脏值 loud warn 回落 config 驱动（不炸装配）。
    let spec_tier = spec.tier.trim();
    if !spec_tier.is_empty() {
        match serde_json::from_value::<nemesis_types::capability::ModelTier>(
            serde_json::Value::String(spec_tier.to_string()),
        ) {
            Ok(t) if !t.is_auto() => agent_loop.set_tier_override(Some(t)),
            Ok(_) => {} // auto = config 驱动（与空串同义）
            Err(_) => warn!(
                "[AgentFactory] spec tier '{}' 非法（auto|mini|normal|big）— 回落 config 驱动",
                spec_tier
            ),
        }
    }
    // prompt-pack pro：项目 loop 与主 loop 同一开关、同一描述档位链。
    agent_loop.set_prompt_system(prompt_system);
    #[cfg(feature = "lsp")]
    agent_loop.set_lsp_manager(shared.lsp_manager.clone());
    // （聊天切会话竞态）：项目 loop 同样注入环尾 seq 查询——
    // 项目会话在 Dashboard 侧切换/轮询的竞态面与主会话完全同构。
    agent_loop.set_chat_seq_lookup(std::sync::Arc::new(nemesis_web::chat_event_log::latest_seq));
    // 价目表注入（context_window）——共享主 workspace data 目录。
    match nemesis_data::PricingStore::open(&nemesis_path::workspace_data_dir(&shared.home)) {
        Ok(store) => agent_loop.set_pricing_store(std::sync::Arc::new(store)),
        Err(e) => tracing::warn!(
            error = %e,
            "[AgentFactory] project loop pricing store open failed; context_window L2 disabled"
        ),
    }
    // 自定义 slash 命令表：用户级资产，锚主 workspace（与主 loop 同一份）。
    agent_loop.set_commands_path(nemesis_path::resolve_commands_config_path_in_workspace(
        &shared.workspace_dir(),
    ));
    // 内建示例管线插件 + 工具事件观察 hook（与主 loop 同一 broadcast 通道，
    // Dashboard tool_event 卡片对项目会话同样可见——事件带 session 身份，
    // 前端按会话过滤，不串台）。
    agent_loop.add_tool_hook(nemesis_agent::hooks::metrics_plugin_slot().clone());
    if let Some(tx) = shared.agent_event_tx.clone() {
        agent_loop.add_tool_hook(std::sync::Arc::new(
            nemesis_agent::tool_event_hook::ToolEventHook::new(tx, agent_loop.work_recorder()),
        ));
    }
    agent_loop.set_agent_event_tx(shared.agent_event_tx.clone());
    // 项目 loop 同挂会话统计 sink（项目会话同账本）。
    agent_loop.set_session_stats_sink(
        shared
            .session_stats
            .clone()
            .map(|a| a as std::sync::Arc<dyn nemesis_agent::SessionStatsSink>),
    );
    // 项目 loop 同样接 present_plan——项目会话的 plan 模式
    // 经 chat.set_mode 按 loop 路由，审批卡（question broker 全局单例）与
    // mode 翻转都锚本 loop 的共享句柄。
    let project_plan_present = Arc::new(nemesis_agent::loop_tools::PlanPresentHandle {
        broker: shared.question_slot.clone(),
        mode: agent_loop.mode_arc(),
        agent_event_tx: shared.agent_event_tx.clone(),
        verdicts: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
    });
    agent_loop.set_plan_present_handle(project_plan_present.clone());
    // spill 锚主 workspace（定位器给用户看；写侧围栏只拦
    // write/edit/append，read 不受限，跨根定位器项目 loop 也读得到）。
    // 清扫任务不重复起——主 loop 侧唯一持有。
    let spill_root =
        nemesis_path::resolve_spill_dir_in_workspace(&nemesis_path::workspace_dir(&shared.home));
    agent_loop.set_spill_root(spill_root);
    // 指令链（AGENTS.md/CLAUDE.md）根锚项目目录——项目可带自己的指令
    // 文件，与主 workspace 互不影响。
    agent_loop.set_workspace_root(project_dir.clone());
    // vault 域绑定 sidecar 权威路径（L26 修复 2026-11-11）：**主 workspace
    // 锚**——项目 loop 的 workspace_root 是项目目录（上面一行），不可作
    // 推导根；恒走 SharedResources 单一推导点（与 CLI 写入侧同点）。
    agent_loop.set_vault_domain_config_path(shared.vault_domain_config_path());
    // （项目 home 收编）：plan 模式产物根 = `<agent_home>/
    // plans`（项目目录/plans 高危禁写，计划产物只落 Bot 域 home）。
    // home 缺席（记录缺位降级形态）= 不注入，回落 workspace_root/plans
    // 旧行为。
    if let Some(ref home) = agent_home {
        agent_loop.set_plan_products_root(home.join("plans"));
    }
    // 指令信任档解析——注册表条目受信（缺省）= verbatim
    // （主循环行为）；未受信按 config projects.instruction_trust 全局策略
    // 降级（annotate=横幅 / isolated=整段跳过）。运行中 `projects.trust`
    // 经 ProjectsBridge 热更新本 loop，装配值只是起点。
    let instruction_trust = if spec.instructions_trusted {
        nemesis_agent::workspace_instructions::InstructionTrust::Trusted
    } else {
        nemesis_agent::workspace_instructions::InstructionTrust::from_policy(
            cfg.projects
                .as_ref()
                .map(|p| p.instruction_trust.as_str())
                .unwrap_or_default(),
        )
    };
    agent_loop.set_instruction_trust(instruction_trust);
    agent_loop.set_snapshot_role(&cfg.agents.defaults.snapshot_role);
    // 急停：绑同一个 Arc——estop 触发连项目 loop 一起冻结（手验项）。
    agent_loop.set_estop(shared.estop.clone());
    // 语音 realtime：项目会话同样可发起语音对话（接力注册经
    // resolve_session_loop 路由到项目 loop）；热态装载幂等，voice_prompt
    // 旗与主 loop 共享同一 Arc 镜像。
    nemesis_web::voice_relay::init_realtime_hot(&shared.workspace_dir().join("config"));
    agent_loop.set_voice_prompt(nemesis_web::voice_relay::voice_prompt_flag());
    // 件4：纪律闭环——项目 loop 同款（workspace
    // 锚项目目录：声明/证伪产物随项目变更集，评审可）。
    if cfg.agents.discipline.enabled {
        let discipline_state = nemesis_agent::discipline::DisciplineState::new(
            true,
            project_dir.clone(),
            cfg.agents.defaults.restrict_to_workspace,
        );
        discipline_state.set_estop(shared.estop.clone());
        agent_loop.add_tool_hook(Arc::new(
            nemesis_agent::discipline::DisciplineGateHook::new(discipline_state.clone()),
        ));
        agent_loop.add_lifecycle_hook(Arc::new(
            nemesis_agent::discipline::DisciplineFalsificationHook::new(discipline_state.clone()),
        ));
        agent_loop.set_discipline(discipline_state);
    }
    // Class A 根修：安全 8 层管线与 gateway 同源是硬约束
    // （headless/ACP/项目会话都不是安全旁路）。此前项目 loop 漏注入 → 工具
    // dispatch 完全绕过管线（注入检测/ABAC/凭据/DLP/病毒扫描/审计链全跳
    // 过，审批 ask 永不触发）。plugin 是共享 Arc：auditor 的审批 manager /
    // guardian judge 也是 plugin 级装配，随本注入一并生效。
    #[cfg(feature = "security")]
    {
        if let Some(ref plugin) = shared.security_plugin {
            agent_loop.set_security_plugin(plugin.clone());
        }
        // 免打扰运行时态挂项目 loop（同主 loop——项目会话不是安全旁路，
        // 免打扰也不旁路）。槽未装配 = None = 现行为。
        if let Some(state) = nemesis_web::handlers::security::approval_auto_state() {
            agent_loop.set_approval_auto_approve(state);
        }
    }
    // small_model 杂务通道与主 loop 同配置。
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
                    workspace: project_dir.to_string_lossy().to_string(),
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
                    }
                    Err(e) => warn!(
                        "[AgentFactory] project small model '{}': provider create failed ({})",
                        small_ref, e
                    ),
                }
            }
            Err(e) => warn!(
                "[AgentFactory] project small model '{}' not resolvable ({})",
                small_ref, e
            ),
        }
    }
    // cc_hooks 与主 loop 同一份 workspace config 目录（用户级 hook 方言
    // 跨项目一致）；相对路径锚项目目录。
    let ws_config_dir = nemesis_path::workspace_config_dir(&shared.workspace_dir());
    // 件2：桥句柄提升到构建作用域——spawn 闭包发 SubagentStart/Stop 用。
    let mut project_cc_bridge: Option<std::sync::Arc<nemesis_agent::cc_hooks::CcHookBridge>> = None;
    if let Some(bridge) =
        nemesis_agent::cc_hooks::CcHookBridge::load_from_dir(&ws_config_dir, project_dir.clone())
    {
        agent_loop.set_cc_hooks_bridge(std::sync::Arc::clone(&bridge));
        project_cc_bridge = Some(std::sync::Arc::clone(&bridge));
        bridge.register(&agent_loop);
    }
    // Todo 收尾提醒：同主 loop——项目会话正是 todowrite 忘
    // 收尾的重灾区。workspace 与项目 loop 的 TodoToolConfig 同根
    // （project_dir）。注册在用户 hooks 桥之后（用户脚本优先）。
    agent_loop.add_lifecycle_hook(std::sync::Arc::new(nemesis_agent::TodoCloseoutHook::new(
        project_dir.clone(),
    )));

    // 4. Session store：共享主 loop 的同一 Arc（存储全局集中、会话隔离靠
    //    session_key；清扫任务主 loop 侧唯一持有，这里不重复起）。
    agent_loop.set_session_store(session_store);

    // 5. Workspace state manager：**不装配**（保持 None——其唯一消费点是
    //    record_last_channel/last_chat_id 崩溃恢复，主 loop 概念；装配会在
    //    用户项目目录急切创建 {project}/state/，违反「用户仓库不容污染」。
    //    AgentLoop 对 None 优雅降级，零风险）。

    // 6. 工具注册：主配置构造后字段覆盖（项目锚 + 剥离三资产件套）。
    let (mut tool_config, spawn_slot) = build_shared_tool_config(
        shared,
        &cfg,
        &model_name,
        Some(agent_loop.mcp_tool_snapshot()),
    );
    // present_plan 工具供给（与 loop verdict 消费槽同一句柄）。
    tool_config.plan_present = Some(project_plan_present);
    // 项目 loop 同样接 present_files——交付物桶锚本 loop
    // （chat_log deliverables 落主 workspace logs，项目会话一样有交付卡）。
    tool_config.turn_deliverables = Some(agent_loop.turn_deliverables_bucket());
    tool_config.workspace = Some(project_dir.to_string_lossy().to_string());
    // 纵深防御：围栏锚项目目录且强制 restrict=true（项目模式不跟随全局
    // restrict_to_workspace 开关——项目会话写入绝不越出项目目录）。
    // （项目 home 收编）：第二根 = 项目 agent home（
    // home 必须放行——人格/记忆/plan 产物是 Bot 域数据根）；None = 单根
    // 旧行为（记录缺位降级形态）。
    // editor 开关 × 围栏两层不认账修复（本 BUG 本体装配点）：
    // 围栏接开关闸——与 auditor 同一 EditorAccessState 的 fence_gate，
    // Full Access 开 → 界外读族放行，双开 → 界外写删族放行（判定与审批
    // 层 evaluate 同源）；security 裁剪或未装配 = None 旧行为。
    #[cfg(feature = "security")]
    let editor_gate = shared
        .security_plugin
        .as_ref()
        .and_then(|p| p.auditor().editor_access())
        .map(|e| e.fence_gate());
    #[cfg(not(feature = "security"))]
    let editor_gate = None::<std::sync::Arc<dyn Fn(&str, bool) -> bool + Send + Sync>>;
    tool_config.workspace_boundary = Some(Arc::new(nemesis_agent::loop_tools::WorkspaceBoundary {
        root: project_dir.clone(),
        restrict: true,
        extra_roots: agent_home.iter().cloned().collect(),
        editor_gate,
    }));
    // 项目 loop 同挂钩子边界（同一 Arc 实例）。
    if let Some(ref boundary) = tool_config.workspace_boundary {
        agent_loop.set_workspace_boundary(boundary.clone());
    }
    tool_config.todo = Some(nemesis_agent::loop_tools::TodoToolConfig {
        workspace: project_dir.clone(),
        event_tx: shared.agent_event_tx.clone(),
    });
    // 剥离：skills/forge 不挂项目 loop（None 覆盖对 cfg 双臂类型
    // 都成立——字段在 feature 关闭时本就是 Option<>）。工具集差异 =
    // 项目 prompt 更短 + 两 loop 共有工具 schema 交集字节一致（测试钉死）。
    tool_config.skills_loader = None;
    tool_config.skills_registry = None;
    tool_config.forge = None;
    tool_config.forge_executor = None;
    // 记忆分域：namespace 空（main/项目镜像）维持 剥离——增强
    // 内存是主 workspace 资产，行为零变化；非空（自建 agent `agents/<id>`）
    // 挂本域 manager（jsonl 与向量库都落自己域），主 corpus 不可见。增强
    // 内存总开关关（shared.memory_manager=None）= 全域一致关闭，自建域
    // 不旁路。选域决策单一权威 = [`memory_manager_for_namespace`]。
    // （项目 home 收编）：项目 home 在场 = 记忆数据落
    // `<home>/memory/memory_vector`（项目记忆与会话记忆隔离），
    // 与主会话/自建域互不可见；config 仍用主 ws（既有语义）。
    tool_config.memory_executor = {
        #[cfg(feature = "memory")]
        {
            let mgr = if let Some(ref home) = agent_home {
                memory_manager_for_data_dir(shared, &home.join("memory").join("memory_vector"))
            } else {
                memory_manager_for_namespace(shared, &spec.memory_namespace)
            };
            // 会话资产区（2026-11-12，拍板「只在分叉时分叉」）：项目 loop
            // 同注 fork 独立域解析器——homes_root = 项目目录（项目会话的
            // home 树根，与 workspace_root/围栏同根）；非 fork 会话回落
            // 上面选出的 mgr（主镜像域 / 项目 agent 域，行为零变化）。
            mgr.map(|mgr| {
                let homes_root = project_dir.clone();
                let shared_for_domains = shared.clone();
                let exec = nemesis_memory::memory_tools::MemoryToolExecutor::new(mgr)
                    .with_session_domain_resolver(Arc::new(move |session_key| {
                        let dir = nemesis_agent::session_home::resolve_session_memory_domain(
                            &homes_root,
                            session_key,
                        )?;
                        memory_manager_for_data_dir(&shared_for_domains, &dir)
                    }));
                Arc::new(exec)
            })
        }
        #[cfg(not(feature = "memory"))]
        {
            None
        }
    };

    // executor 分离通道锚项目目录（项目会话的 exec/MOVE 工具经子进程在项目
    // cwd 执行；enabled/sandbox 开关与主 loop 同源 config）。：常驻
    // 路由按 spec 的解析闭包挂接（项目=kind+project_id 记录匹配；记录缺
    // 席时调用时诚实回落 per-call）。：围栏第二根（项目 agent home）
    // 透传 per-call 子进程 env——开关两侧围栏同根。
    let executor_channel = crate::exec_world::build_executor_channel_with_extra_roots(
        &shared.home,
        &project_dir,
        shared.config_store.handle(),
        agent_home.iter().cloned().collect(),
    )?;
    let persistent_route = spec
        .persistent_resolve
        .as_ref()
        .and_then(|resolve| persistent_route_for(shared, resolve.clone()));
    register_tools_and_mcp(
        &mut agent_loop,
        shared,
        &tool_config,
        executor_channel.as_ref(),
        persistent_route.as_ref(),
    );

    // 与主 loop 同源（agent_factory 上方主装配:1142-1150 同款）：把注册期
    // 构造的 executor stash 进 loop——set_memory_approval_gate 读的就是这个
    // 槽，缺 stash = 挂闸静默 no-op（真机 E2E 撞见：项目会话
    // memory_store 零审批直写，闸调用看着在场实际空转）。
    #[cfg(feature = "memory")]
    if let Some(ref exec) = tool_config.memory_executor {
        agent_loop.set_memory_executor(exec.clone());
    }

    // memory 审批闸与主 loop 同源（真机 E2E 撞见的不对称：主
    // loop 在 runtime 装配挂 GatewayMemoryGate，项目 loop 此前从不挂——
    // 项目会话 memory_store/memory_forget 零审批直写，违背「项目会话不
    // 是安全旁路」）。
    //
    // 时序（真机日志实锤，推翻本注释初版「构建点在填槽之后」）：
    // boot 期项目 loop 由 post_agent start_all 构建（ProjectLoops starting
    // .499），而 approval_slot 在 runtime 审批装配才填（Approval manager
    // wired.669，晚约 170ms）——构建期急读必空。故挂槽版闸（from_slot）：
    // approve 时点才解析 manager，调用时槽早已填齐；槽真空（审批装配缺席）
    // = deny fail-closed（闸内 WARN 揪出），绝不静默放行。
    #[cfg(all(feature = "memory", feature = "security"))]
    if shared.security_plugin.is_some() {
        agent_loop.set_memory_approval_gate(Arc::new(
            crate::commands::gateway::GatewayMemoryGate::from_slot(shared.approval_slot.clone()),
        ));
    }

    // tool_policy 双闸输入（current_hidden_tools 单点合并 → 供给闸 +
    // dispatch 闸同源生效）。deny/hidden 折算保留（黑名单无增量工具问题，
    // 通配语义同 agents.hidden_tools）。（二轮复评）：allow
    // 白名单改**直挂 loop 槽**（set_tool_policy_allow）——旧「注册面 ∖ allow」
    // 折算只吃注册期快照，MCP 热加载的新工具不在快照里就漏拦（注释自认的
    // 已知边界）；双闸现读槽值后动态新工具天然落闸，折算取缔。
    {
        let hidden = spec.tool_policy_hidden.clone();
        if !hidden.is_empty() {
            agent_loop.set_extra_hidden_tools(hidden);
        }
        if !spec.tool_policy_allow.is_empty() {
            agent_loop.set_tool_policy_allow(spec.tool_policy_allow.clone());
        }
    }

    // Checkpoint 影子库：turn 记录 + git 影子库都落主 workspace
    // logs/project_checkpoints/{pid}/（不污染用户项目目录——项目带.git
    // 时 CheckpointStore 默认影子位置从 root 派生会落进项目目录，必须
    // new_with_shadow 显式指定；root=项目目录仅作.git 后端探测锚）。
    // （项目 home 收编）：项目 home 在场 = 影子库迁
    // `<home>/checkpoints/`（Bot 域数据根）；旧数据不迁移——存量
    // 项目 rewind/fork 回档失效（可接受）。None = 主 ws 旧路径（记录缺位
    // 降级形态连续性保留）。
    {
        let cp_dir = match agent_home.as_ref() {
            Some(home) => home.join("checkpoints"),
            None => project_checkpoint_dir(&shared.workspace_dir(), &spec.checkpoint_key),
        };
        if let Some(parent) = cp_dir.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let store = Arc::new(nemesis_agent::checkpoint::CheckpointStore::new_with_shadow(
            Some(cp_dir.clone()),
            project_dir.clone(),
            cp_dir.join("repo.git"),
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

    // 子代理可见性：spec 形态 loop 接线 continuation manager（共享
    // 主 workspace 的 `cluster/rpc_cache/` 盘上 store）——`__BG_SPAWN__`
    // 后台子代理的续行快照/回灌从此在本 loop 闭合（快照存这里、完成消息
    // 按 parent session_key 路由回这里）。cluster_rpc 工具仍不装配（无
    // `__ASYNC__` 产生源），cluster 恢复循环对空 peer_id 快照本就跳过；
    // rpc_cache TTL 清扫仍主 loop 侧唯一持有（共享目录，不重复扫）。
    agent_loop.set_continuation_manager(Arc::new(
        nemesis_agent::ContinuationManager::with_disk_store(&shared.workspace_dir()),
    ));

    // 显式不装配（与主 loop 的差异面）：cluster_rpc 工具（无跨节点 RPC 面，
    // 续行只剩本 loop 的后台子代理形态，见上）/ memory_inject（增强内存是
    // 主 workspace 资产）/ forge provider 更新（forge 不挂）/ workspace
    // state manager（见上）/ spawn_daily_* 清扫任务（共享存储的清扫主
    // loop 侧唯一持有）。

    info!(
        model = %model_name,
        tools = agent_loop.tool_count(),
        project = %spec.display_name,
        "[AgentFactory] project AgentLoop built"
    );

    // Arc 定型后注入子代理 spawn 闭包（Weak 防环，与主 spawn 同理）。
    let agent_loop = Arc::new(agent_loop);
    // checkpoint gc 装配（项目 loop 独立影子
    // 库 `logs/project_checkpoints/<key>/repo.git`，同默认关；Weak 防环同主
    // loop 形态）。
    {
        let gc_cfg = shared
            .config_store
            .handle()
            .read()
            .checkpoint
            .as_ref()
            .map(|c| c.gc.clone())
            .unwrap_or_default();
        if gc_cfg.enabled
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
        }
    }
    inject_project_spawn_fn(
        &agent_loop,
        &spawn_slot,
        project_cc_bridge.clone(),
        &shared.bus,
        &spec.loop_origin,
    );
    // 治理补全：connector 治理面与主 loop 同源——项目/
    // selfbuilt loop 同装 MCP（enable_mcp_reload），独占接管与 connector
    // 工具必须同覆盖，否则项目会话成为独占旁路（治理资产仍锚主 workspace，
    // 见 register_connector_governance 文档）。cfg 对齐注册条件。
    #[cfg(all(feature = "security", feature = "connectors"))]
    register_connector_governance(&agent_loop, shared);
    // fs watcher 锚项目目录（外部编辑注记只看项目内变更）。
    let _ = AgentLoop::start_fs_watcher(&agent_loop, &cfg.agents.fs_watcher);
    // ASM-08：装配自检（与主/集群 loop 同一闸，=甲）。
    assert_gateway_critical_wiring(&agent_loop, shared, "项目 loop")?;
    Ok(agent_loop)
}

/// 件3 层2：审批 manager 观察接线——一次性延迟
/// 任务。网关在运行期装配（`runtime` 步骤，loop 构建之后）才把真实审批
/// manager 挂上 auditor 槽，构建时同步 get→wrap→set 必被覆写；改为后台
/// 轮询：manager 出现即包 [`nemesis_agent::cc_hooks:ObservingApprovalManager`]
/// 回写槽位。进程级旗标保证只包一次（loop 重建/agent 重启不重复包；
/// wrapper 的 inner 是包装时的 manager，此后无人再换槽）。超时 = warn
/// fail-open（审批照常工作，只是观察不到）。无 runtime（部分测试的同步
/// 构建路径）诚实跳过。
#[cfg(feature = "security")]
pub(super) fn spawn_approval_observer_once(
    bridge: std::sync::Arc<nemesis_agent::cc_hooks::CcHookBridge>,
    plugin: std::sync::Arc<nemesis_security::pipeline::SecurityPlugin>,
) {
    static WIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if WIRED.load(std::sync::atomic::Ordering::Acquire) {
        return; // 已包过（loop 重建路径）——槽位仍是 wrapper。
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::debug!("[AgentFactory] approval observer skipped: no tokio runtime");
        return;
    };
    handle.spawn(async move {
        // 50ms × 1200 = 60s 封顶。网关装配在启动即挂 manager，60s 绰绰有余。
        for _ in 0..1200 {
            if let Some(mgr) = plugin.auditor().get_approval_manager() {
                if WIRED.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    return; // 并发重建已包过。
                }
                let wrapped =
                    nemesis_agent::cc_hooks::ObservingApprovalManager::new(mgr, bridge);
                plugin
                    .auditor()
                    .set_approval_manager(std::sync::Arc::new(wrapped));
                info!(
                    "[AgentFactory] approval observer wired (Notification: security_ask/auditor)"
                );
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        warn!(
            "[AgentFactory] approval observer gave up after 60s: no approval manager appeared (approval still works, just unobserved)"
        );
    });
}
