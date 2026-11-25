// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 agent_factory.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// 项目 loop 的 checkpoint 影子库目录——
/// `{主workspace}/logs/project_checkpoints/{pid}`（checkpoint 文件绝不
/// 落用户项目目录、不进用户仓库）。从 resolve_checkpoints_dir_in_workspace
/// 派生兄弟目录（logs 家族统一定义）；parent() 不可达 None，兜底拼接保总返回。
pub fn project_checkpoint_dir(main_workspace: &std::path::Path, project_id: &str) -> PathBuf {
    nemesis_path::resolve_checkpoints_dir_in_workspace(main_workspace)
        .parent()
        .map(|logs| logs.join("project_checkpoints").join(project_id))
        .unwrap_or_else(|| {
            main_workspace
                .join("logs")
                .join("project_checkpoints")
                .join(project_id)
        })
}

/// Build a resident AgentLoop for one project.
///
/// `project.path` is canonical (registry enforces it at create time;
/// canonicalize-once). `session_store` is the main loop's shared Arc — the
/// gateway extracts it via `agent_loop.session_store()`.
pub fn build_project_agent_loop(
    shared: &Arc<SharedResources>,
    project: &crate::projects::registry::ProjectEntry,
    session_store: Arc<nemesis_agent::session::SessionStore>,
) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>> {
    let loop_arc =
        build_agent_loop_from_spec(shared, &AgentLoopSpec::from_project(project), session_store)?;
    // 项目洞察注入（洞察页与场景拆分 2026-11-24）：只注入本项目的条目
    //（render_section_for(Some(pid))）——对话洞察（scope=None）不泄入项目
    // 会话，项目之间不串台。构造口径与主 loop 同源（activation_recurrence
    // 同读 config.forge.json）。
    // forge 门（BUG-20261125-065）：裁剪组合（minimal-iot
    // --no-default-features）下 nemesis-forge dep 被裁，无门即 E0433；
    // 非 forge 构建跳过洞察注入（loop 无 section source = 默认行为）。
    #[cfg(feature = "forge")]
    {
        let forge_cfg = nemesis_forge::config::load_forge_config(
            &shared
                .workspace_dir()
                .join("config")
                .join("config.forge.json"),
        );
        let insights_store =
            std::sync::Arc::new(nemesis_forge::insights::InsightStore::with_recurrence(
                &shared.workspace_dir(),
                forge_cfg.learn.activation_recurrence,
            ));
        let pid = project.id.clone();
        loop_arc.set_insights_section_source(std::sync::Arc::new(move || {
            insights_store.render_section_for(Some(&pid))
        }));
    }
    Ok(loop_arc)
}
/// 项目/记录 loop 的子代理 spawn 闭包。前台路径复用 run_detached
///（本 loop 内 detached 轮次）；后台路径（解锁）与主 spawn
/// 同源——spec 形态 loop 已接线 continuation manager（快照在本 loop 存取），
/// 完成消息按 parent session_key 回灌（manager.rs 按归属路由回本 loop 的
/// per-loop mpsc），不再全局短路进主 loop。Weak 捕获防环与主 spawn 同理。
/// 专家派发（2026-11-16 放开 v1 边界）：与主 spawn 同源接入——第 11 参
/// `expert` 非空经 ExpertsHandle 解析（提示词整体替换主人格 + 可选模型
/// 覆盖），第 12 参 `prompt`（自拟提示词降级/ad-hoc）同一管线零新旁路。
/// 专家资产恒从主 workspace 解析（workspace 级资产，与 skills/persona
/// 同类），子代理执行根保持在项目目录——「资产在主 ws、执行根在项目
/// 目录」先例（checkpoint 影子库 / 会话 home 同形）。
pub(super) fn inject_project_spawn_fn(
    loop_arc: &Arc<nemesis_agent::r#loop::AgentLoop>,
    spawn_slot: &Arc<std::sync::OnceLock<nemesis_agent::loop_tools::SpawnFn>>,
    // 件2：SubagentStart/Stop 事件源（None = 无桥，不发事件）。
    cc_bridge: Option<std::sync::Arc<nemesis_agent::cc_hooks::CcHookBridge>>,
    // 后台完成消息回灌 bus（`system` 通道 + SUBAGENT_CONTINUATION_PREFIX）。
    bus: &Arc<nemesis_bus::MessageBus>,
    // 子代理可见性：宿主形态标识（`project:{id}` / `agent:{id}`），
    // 注册表 `loop_origin` 字段赋值源。
    loop_origin: &str,
    // 专家子代理（None = 本 loop 无专家面，第 11 参非空诚实 Err）。
    experts: Option<nemesis_agent::loop_tools::ExpertsHandle>,
    // 专家模型覆盖解析器（factory 同源 reload_provider；纯数据件）。
    reloader: std::sync::Arc<ModelProviderReloader>,
) {
    // loop 侧镜像共享槽（client_spawn / WSAPI chat.spawn 消费同一闭包）。
    loop_arc.set_spawn_slot(spawn_slot.clone());
    // 子代理可见性：宿主形态标识进 loop——delegate 任务（集群
    // `__ASYNC__` 路径）入册时 loop 自己读（该路径在 loop 内部
    // tool_batch，够不着本闭包）。主 loop 不经此函数，缺省 `main`。
    loop_arc.set_loop_origin(loop_origin);
    let weak = Arc::downgrade(loop_arc);
    let bus = bus.clone();
    let loop_origin = loop_origin.to_string();
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
              parent_session_key: &str,
              expert: &str,
              prompt: &str| {
            let weak = weak.clone();
            let bus = bus.clone();
            // SpawnFn 的 Future 是 'static——&str 参数先拷贝成 owned。
            let agent_id = agent_id.to_string();
            let task = task.to_string();
            let tools_profile = tools_profile.to_string();
            let role_owned = role.to_string();
            // 专家点名（"" = 默认子代理；非空 = 专家派发，同主 spawn）。
            let expert = expert.trim().to_string();
            // 自拟提示词（"" = 非降级流；非空 = ad-hoc 派发；'static 边界
            // 先转 owned）。
            let prompt = prompt.trim().to_string();
            // 专家派发件（Fn 闭包不能把捕获 move 进 async 块——clone 进去）。
            let experts = experts.clone();
            let reloader = reloader.clone();
            // 子代理可见性：连接级 chat_id + 父会话键（同主 spawn）。
            let chat_id = chat_id.to_string();
            let parent_session_key = parent_session_key.to_string();
            // 宿主形态标识 per-call 本地化（外层 Fn 闭包捕获变量不可 move）。
            let loop_origin = loop_origin.clone();
            // 显式角色覆盖（语义与主 spawn 同源）。
            let explicit_role = nemesis_agent::prompt::SubagentRole::from_slug(role);
            // Fn 闭包不能把捕获 move 进 async 块——每次调用 clone 进去。
            let cc_bridge = cc_bridge.clone();
            // 件2：SubagentStart/Stop 的 payload 素材（两路径共用；后台路径
            // 再 clone 进 spawn 任务）。expert 模式下 agent_id 在解析后改写
            // （mut：`expert:{slug}` 注册表可见性）。
            let mut info = nemesis_agent::cc_hooks::SubagentInfo {
                agent_id,
                task,
                depth,
                background,
                tools_profile,
                loop_kind: "project",
                // 会话圈定锚点：项目子代理继承父会话（同主 spawn）。
                session_key: parent_session_key.clone(),
            };
            Box::pin(async move {
                // 专家派发（2026-11-16 放开 v1 边界，与主 spawn 同源）：
                // 非空 expert → ExpertsHandle 解析（提示词整体替换主人格 +
                // 可选模型覆盖；None handle = 诚实 Err）。非空 prompt（expert
                // 为空，互斥由 SpawnTool 预检保证）= 自拟提示词 ad-hoc 派发
                //（agent_id=expert:ad-hoc、role=None、提示词整体替换、model
                // 空 = 主模型），与专家派发同一管线零新旁路。SpawnTool 预检
                // 已验存在/启用，这里二次解析取产品（两读同源 ExpertsStore，
                // 无缓存无漂移）。
                let expert_dispatch = if !expert.is_empty() {
                    match &experts {
                        Some(h) => Some((h.resolve)(&expert)?),
                        None => {
                            return Err(format!(
                                "experts are not configured on this loop (requested '{expert}')"
                            ))
                        }
                    }
                } else if !prompt.is_empty() {
                    Some(nemesis_agent::loop_tools::ExpertDispatch {
                        slug: "ad-hoc".to_string(),
                        name: "ad-hoc".to_string(),
                        prompt: prompt.clone(),
                        model: String::new(),
                    })
                } else {
                    None
                };
                // 专家模型覆盖：非空 model = factory 同源 provider 解析
                //（reload_provider：raw config resolve/create/fallback 链）。
                // 解析失败分流（同主 spawn）：degrade 策略开 = 回落主模型
                // 照跑专家提示词 + 尾注记（提示词是价值本体，引擎坏不停
                // 车）；策略关 = 诚实 Err（派发语义不变形）。
                let (expert_model_swap, degrade_note) =
                    match expert_dispatch.as_ref().map(|d| d.model.trim()) {
                        Some(m) if !m.is_empty() => {
                            match nemesis_agent::r#loop::ProviderReloader::reload_provider(
                                reloader.as_ref(),
                                m,
                                None,
                            ) {
                                Ok(provider) => (Some((provider, m.to_string())), String::new()),
                                Err(e) => {
                                    let degrade = experts
                                        .as_ref()
                                        .map(|h| (h.policy)().degrade)
                                        .unwrap_or(false);
                                    if degrade {
                                        tracing::warn!(
                                            expert = %expert,
                                            model = %m,
                                            error = %e,
                                            "[spawn] expert model alias failed to resolve; degrading to the main model (agents.experts.degrade=true)"
                                        );
                                        (None, format!("\n\n[Note: the expert's model '{m}' failed to load ({e}); this sub-agent ran with the default model instead.]"))
                                    } else {
                                        return Err(e);
                                    }
                                }
                            }
                        }
                        _ => (None, String::new()),
                    };
                // 专家派发的注册表可见性：agent_id 改写 `expert:{slug}`
                //（子代理面板台账/钩子 payload 一眼分清专家与普通子代理；
                // 两条路径的 SubagentBegin / 事件 payload 都从这里取值）。
                if let Some(d) = &expert_dispatch {
                    info.agent_id = format!("expert:{}", d.slug);
                }
                // 档位 → 白名单（与主 spawn 同一映射点，防御纵深）。
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
                    // 件2：SubagentStart（exit 2 = 拒绝 spawn，`?` 上抛阻断文案）。
                    if let Some(bridge) = &cc_bridge {
                        bridge.dispatch_subagent_start(&info).await?;
                    }
                    tracing::debug!(
                        agent_id = %info.agent_id,
                        task_len = info.task.len(),
                        tools_profile = %info.tools_profile,
                        depth,
                        "[SpawnTool] project loop detached sub-agent (L6++)"
                    );
                    // 子代理可见性：入册 + 挂取消令牌（同主 spawn；
                    // loop_origin 按宿主形态标识， 详情视图按宿主过滤）。
                    let registry_id =
                        nemesis_agent::r#loop::AgentLoop::detached_session_key(None);
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
                            loop_origin: loop_origin.clone(),
                            bg_task_id: None,
                        },
                    );
                    nemesis_agent::subagent_registry::global().attach_token(
                        &registry_id,
                        cancel_token.clone(),
                    );
                    let result = agent_loop
                        .run_detached(
                            &info.task,
                            nemesis_agent::r#loop::DetachedOpts {
                                allowed_tools,
                                depth,
                                // prompt-pack pro：readonly 档 → 侦察员角色。
                                // 显式角色优先，缺省回落档位推导。专家派发 =
                                // 提示词整体替换，角色叠加无意义 → None。
                                role: if expert_dispatch.is_some() {
                                    None
                                } else {
                                    explicit_role.or_else(|| {
                                        nemesis_agent::loop_tools::detached_role_for_profile(
                                            &info.tools_profile,
                                        )
                                    })
                                },
                                // 专家系统提示词（整体替换主人格；None =
                                // 默认子代理旧行为字节不变）。
                                system_prompt: expert_dispatch
                                    .as_ref()
                                    .map(|d| d.prompt.as_str()),
                                // 专家模型覆盖（ready-made (provider, 模型名)；
                                // None = 主模型字节不变）。
                                model_swap: expert_model_swap.as_ref(),
                                // 子代理可见性：会话键/注册表 id 同键；取消
                                // 令牌直达执行链（注册表 cancel 面）。
                                session_key_override: Some(registry_id.clone()),
                                cancel_token: Some(cancel_token.clone()),
                                registry_id: Some(registry_id.clone()),
                                ..Default::default()
                            },
                        )
                        .await;
                    // 件2：SubagentStop（观察型；前台两态，理由同主 spawn）。
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
                    // 模型别名坏降级（同主 spawn）：成功结果尾部附注记
                    //（模型/用户都能看到引擎回落事实；失败路径本就是 Err）。
                    return match result {
                        Ok(text) if !degrade_note.is_empty() => {
                            Ok(format!("{text}{degrade_note}"))
                        }
                        other => other,
                    };
                }

                // 后台路径（解锁）：与主 spawn 同源。continuation
                // 快照由本 loop 的 manager 存取（spec 构建时已接线）；完成消息
                // 按父会话键回灌 bus，manager 按归属路由回本 loop。
                // 件2：SubagentStart 在任务真正 spawn 前发（拒绝则不产生
                // marker / 不 spawn 任务）。
                if let Some(bridge) = &cc_bridge {
                    bridge.dispatch_subagent_start(&info).await?;
                }
                // task_id 取号走 loop_continuation 单一真相源
                // next_bg_spawn_task_id（BUG-20261123-014：与主 spawn 各持
                // per-file 计数器的旧形态在同毫秒等值时碰撞 → bg_park 槽
                // 覆盖串台；「唯一性由时间戳承载」的前提失实——时间戳只有
                // 1ms 粒度，同毫秒消歧靠计数器，计数器必须单例）。
                let task_id = nemesis_agent::loop_continuation::next_bg_spawn_task_id();
                let bg_agent_id = info.agent_id.clone();
                let bg_task = info.task.clone();
                let bg_profile = info.tools_profile.clone();
                let bg_task_id = task_id.clone();
                let bg_bridge = cc_bridge.clone();
                let bg_info = info.clone();
                // 子代理可见性：后台任务在 marker 返回**前**入册
                //（列表立即可）+ 挂取消令牌。loop_origin = 宿主形态标识。
                let bg_registry_id =
                    nemesis_agent::r#loop::AgentLoop::detached_session_key(None);
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
                        loop_origin: loop_origin.clone(),
                        bg_task_id: Some(bg_task_id.clone()),
                    },
                );
                nemesis_agent::subagent_registry::global().attach_token(
                    &bg_registry_id,
                    bg_cancel_token.clone(),
                );
                // park 信箱注册（2026-11-19 spawn park 化）：先于
                // tokio::spawn——完成回灌（deliver）只可能晚于本行，杜绝
                // 「完成先到、park 未挂」竞态（与主 spawn 同契约）。
                nemesis_agent::bg_park::global().register(
                    &bg_task_id,
                    &parent_session_key,
                    &chat_id,
                );
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
                                "[SpawnTool] project loop background sub-agent started (D7)"
                            );
                            agent_loop
                                .run_detached(
                                    &bg_task,
                                    nemesis_agent::r#loop::DetachedOpts {
                                        // 后台任务沿用派生前已过检查的同一深度
                                        //（语义：后台化不另计深度）。
                                        allowed_tools,
                                        depth,
                                        // 专家派发同前台：提示词整体替换，角色
                                        // 叠加无意义 → None。
                                        role: if expert_dispatch.is_some() {
                                            None
                                        } else {
                                            explicit_role.or_else(|| {
                                                nemesis_agent::loop_tools::detached_role_for_profile(
                                                    &info.tools_profile,
                                                )
                                            })
                                        },
                                        // 专家系统提示词 + 模型覆盖（与前台同源）。
                                        system_prompt: expert_dispatch
                                            .as_ref()
                                            .map(|d| d.prompt.as_str()),
                                        model_swap: expert_model_swap.as_ref(),
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
                    // bus 前。三态语义同主 spawn。
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
                        Ok(text) => (
                            // 模型别名坏降级：后台路径同附尾注记（回灌
                            // 内容与前台语义一致）。
                            format!("{text}{}", degrade_note),
                            "ok",
                            None,
                        ),
                        Err(e) => (
                            format!("[后台子代理任务失败] {}", e),
                            "error",
                            Some(e.clone()),
                        ),
                    };
                    let mut msg = nemesis_types::channel::InboundMessage::continuation(
                        format!(
                            "{}{}",
                            nemesis_types::constants::SUBAGENT_CONTINUATION_PREFIX,
                            bg_task_id
                        ),
                        bg_parent_session_key.clone(),
                        content,
                    )
                    .with_metadata("status", status)
                    .with_metadata_opt("error", error.as_deref())
                    .with_metadata("source", "background_subagent");
                    // 子代理可见性：父会话键随行——项目会话派发的后台
                    // 任务完成回灌项目 loop（manager 按归属路由），不再全局
                    // 短路进主 loop（项目快照在项目 loop 的 continuation
                    // store，主 loop 加载必 miss）。
                    msg.session_key = bg_parent_session_key;
                    bus.publish_inbound(msg);
                    tracing::info!(
                        task_id = %bg_task_id,
                        status,
                        "[SpawnTool] project loop background sub-agent finished, completion published to bus"
                    );
                });
                Ok(format!("__BG_SPAWN__:{}", task_id))
            })
        },
    ));
}
