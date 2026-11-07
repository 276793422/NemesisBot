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
    build_agent_loop_from_spec(shared, &AgentLoopSpec::from_project(project), session_store)
}
/// 项目/记录 loop 的子代理 spawn 闭包。前台路径复用 run_detached
///（本 loop 内 detached 轮次）；后台路径（解锁）与主 spawn
/// 同源——spec 形态 loop 已接线 continuation manager（快照在本 loop 存取），
/// 完成消息按 parent session_key 回灌（manager.rs 按归属路由回本 loop 的
/// per-loop mpsc），不再全局短路进主 loop。Weak 捕获防环与主 spawn 同理。
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
) {
    // loop 侧镜像共享槽（client_spawn / WSAPI chat.spawn 消费同一闭包）。
    loop_arc.set_spawn_slot(spawn_slot.clone());
    // 子代理可见性：宿主形态标识进 loop——delegate 任务（集群
    // `__ASYNC__` 路径）入册时 loop 自己读（该路径在 loop 内部
    // tool_batch，够不着本闭包）。主 loop 不经此函数，缺省 `main`。
    loop_arc.set_loop_origin(loop_origin);
    use std::sync::atomic::{AtomicU64, Ordering};
    let weak = Arc::downgrade(loop_arc);
    let bus = bus.clone();
    let loop_origin = loop_origin.to_string();
    // 后台任务 id 计数器（同毫秒并发 spawn 防撞；与主 spawn 各自独立，
    // task_id 带毫秒时间戳全局唯一性由时间戳承载）。
    static BG_SPAWN_COUNTER: AtomicU64 = AtomicU64::new(0);
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
            // 再 clone 进 spawn 任务）。
            let info = nemesis_agent::cc_hooks::SubagentInfo {
                agent_id,
                task,
                depth,
                background,
                tools_profile,
                loop_kind: "project",
            };
            Box::pin(async move {
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
                    return result;
                }

                // 后台路径（解锁）：与主 spawn 同源。continuation
                // 快照由本 loop 的 manager 存取（spec 构建时已接线）；完成消息
                // 按父会话键回灌 bus，manager 按归属路由回本 loop。
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
                        "[SpawnTool] project loop background sub-agent finished, completion published to bus"
                    );
                });
                Ok(format!("__BG_SPAWN__:{}", task_id))
            })
        },
    ));
}
