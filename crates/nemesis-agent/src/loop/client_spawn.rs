//! 客户端驱动委派（2026-09-28 角色目录与分档供给）：WSAPI `chat.spawn` 的
//! loop 侧入口。
//!
//! 客户端（Dashboard 委派卡）直接把任务派给指定角色的子代理——同一
//! detached 通道与模型侧 `spawn` 工具共用（同一 SpawnFn 闭包），工具档位/
//! 深度/角色模板/安全管线全部同源。区别只在入口：模型的 spawn 走工具派发
//! 闸（角色目录闸在 dispatch），客户端 spawn 的角色合法性由 handler 侧
//! [`Self::check_client_role`] 用同一 [`Self::visible_roles`] 裁决——两处
//! 口径永不漂移。

use super::*;

impl AgentLoop {
    /// 注入 spawn 共享槽镜像（factory 组装后调用；与 SpawnTool 持同一 Arc）。
    pub fn set_spawn_slot(
        &self,
        slot: std::sync::Arc<std::sync::OnceLock<crate::loop_tools::SpawnFn>>,
    ) {
        *self.spawn_slot.write() = Some(slot);
    }

    /// 注入宿主形态标识（子代理可见性，factory 组装后调用）：`main` /
    /// `project:{pid}` / `agent:{id}`——delegate 任务入册时抄给注册表
    /// `loop_origin`。缺省 `main`（主 loop 不注入即正确）。
    pub fn set_loop_origin(&self, origin: &str) {
        *self.loop_origin.write() = origin.to_string();
    }

    /// 子代理飞行中驾驶（项3）：把一条中途指令推入目标子代理会话键的
    /// steer 槽（inbox next_step，[`crate::inbox::Inbox::enqueue_steer`]）。
    /// 子代理 `run_llm_loop` 每轮开头的 `claim_steer_messages`（器官 2，
    /// 对 `channel == "subagent"` 上下文同样运行）在下一轮 LLM 前消费——
    /// @文件展开 / 媒体附加 / chat_log `[steer]` 行（steer:true meta，
    /// rewind/fork 跳过）全部复用主 loop steer 同源管线，审计不断。
    ///
    /// 校验链（单一裁决点，WSAPI `subagents.steer` 与模型工具
    /// `subagent_steer` 双入口共用）：
    /// ① estop 生效中 = 诚实拒绝（WSAPI 侧 handler 另有前置闸，此处是
    ///    工具路径的兜底——两层闸不互斥）；
    /// ② 注册表在册（id 即子代理会话键）；
    /// ③ 非终态（飞行中驾驶只对运行中任务生效）；
    /// ④ `loop_origin` 与本 loop 一致——跨引擎 steer 会推进永远不会被
    ///    消费的 inbox（子代理跑在别的 loop 上），诚实拒绝而非静默丢失。
    ///
    /// 指令内容加 `[父代理指令] ` 前缀（子代理可辨来源：这是编排方的
    /// 中途修正，不是任务原文）。终局竞态诚实边界：指令入队后子代理可能
    /// 恰在收尾——run_detached_events 终点 `inbox.clear` 会丢弃未消费
    /// 的 steer（注入是瞬态指令，错过即逝；ack 先行属乐观确认）。
    pub fn steer_subagent(&self, id: &str, instruction: &str) -> Result<String, String> {
        let instruction = instruction.trim();
        if instruction.is_empty() {
            return Err("steer instruction is empty".to_string());
        }
        if self.security.is_engaged() {
            return Err("⛔ 急停（estop）生效中，子代理驾驶已冻结：release 后恢复".to_string());
        }
        let t = crate::subagent_registry::global()
            .get(id)
            .ok_or_else(|| format!("task not found: {id}"))?;
        if t.status.is_terminal() {
            let status_str = serde_json::to_value(t.status)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".to_string());
            return Err(format!(
                "子代理已终态（{status_str}），steer 拒绝——飞行中驾驶只对运行中任务生效"
            ));
        }
        let origin = self.loop_origin();
        if t.loop_origin != origin {
            return Err(format!(
                "子代理归属另一执行引擎（{}，本引擎 {origin}），不能跨引擎 steer",
                t.loop_origin
            ));
        }
        let queued = crate::inbox::QueuedMessage {
            msg: nemesis_types::channel::InboundMessage {
                channel: "subagent".to_string(),
                sender_id: "steer".to_string(),
                chat_id: id.to_string(),
                content: format!("[父代理指令] {instruction}"),
                media: Vec::new(),
                session_key: id.to_string(),
                correlation_id: String::new(),
                metadata: std::collections::HashMap::new(),
                voice_playback: None,
                // 直塞门：enqueue_steer 直接进子代理 steer 队列（不经 busy
                // gate），必须显式 User 才能过 detection net；steer 指令
                // 语义 = 用户驾驶意志，User 类直塞不变。
                msg_class: nemesis_types::channel::MsgClass::User,
            },
            timestamp: chrono::Local::now().to_rfc3339(),
            priority: crate::inbox::MessagePriority::Normal,
        };
        match self.inbox.enqueue_steer(id, queued) {
            crate::inbox::EnqueueOutcome::QueuedForNextStep => {
                tracing::info!(
                    "[Steer] subagent steer queued: id={id}, pending={}",
                    self.inbox.pending(id).1
                );
                Ok(format!(
                    "steer 已注入子代理（将在其下一轮开始前生效）：{instruction}"
                ))
            }
            _ => Err(format!(
                "目标子代理会话队列已满（容量 {}），steer 拒绝——稍后重试或先取消子代理",
                self.inbox.capacity()
            )),
        }
    }

    /// 当前宿主形态标识（getter 返回 owned——注册表 begin 持有）。
    pub fn loop_origin(&self) -> String {
        self.loop_origin.read().clone()
    }

    /// 客户端 spawn 前的角色合法性检查（handler 侧裁决）：未知 slug /
    /// 分档外 / `agents.roles.hidden` 隐藏 = `Err`（错误串与 dispatch 闸
    /// 同文案形态）。空 role = `Ok`（缺省角色，档位推导）。
    pub fn check_client_role(&self, role: &str) -> Result<(), String> {
        let role = role.trim();
        if role.is_empty() {
            return Ok(());
        }
        let visible = self.visible_roles();
        if visible.contains(&role) {
            return Ok(());
        }
        let known = nemesis_prompts::subagents::SubagentRole::from_slug(role).is_some();
        Err(if known {
            format!(
                "role '{role}' is not available for the current model tier, or it is hidden via agents.roles.hidden. Available roles: {}.",
                visible.join(", ")
            )
        } else {
            format!(
                "Unknown role '{role}'. Valid roles: {}.",
                nemesis_prompts::subagents::SubagentRole::catalog()
                    .iter()
                    .map(|(slug, _, _)| *slug)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    /// WSAPI `roles.list` 的数据面：当前 tier + 目录全量（slug/职责/最低
    /// 档/当前可见性/配置隐藏）。可见性裁决与 dispatch 闸共用
    /// [`Self::visible_roles`]——前端下拉看到的集合就是模型能用的集合。
    pub fn roles_surface(&self) -> serde_json::Value {
        let tier_str = match *self.tier.read() {
            nemesis_types::capability::ModelTier::Mini => "mini",
            nemesis_types::capability::ModelTier::Normal => "normal",
            nemesis_types::capability::ModelTier::Auto
            | nemesis_types::capability::ModelTier::Big => "big",
        };
        let visible = self.visible_roles();
        let hidden = self.current_hidden_roles();
        let roles: Vec<serde_json::Value> = nemesis_prompts::subagents::SubagentRole::catalog()
            .iter()
            .map(|(slug, desc, min)| {
                serde_json::json!({
                    "slug": slug,
                    "description": desc,
                    "min_tier": min,
                    "visible": visible.contains(slug),
                    "hidden": hidden.iter().any(|h| h == slug),
                })
            })
            .collect();
        serde_json::json!({
            "tier": tier_str,
            "visible_count": visible.len(),
            "roles": roles,
        })
    }

    /// 发布委派回执落会话事件（[`nemesis_types::agent:AgentEvent:
    /// SubagentReplyLanded`]）。持久化**之后**调用（前端收到即 resync，
    /// 行必须已在盘上）；agent_event_tx 未装配（CLI / B 端 worker）为
    /// no-op。摘录 rune 安全截断 500 字符（toast 展示用，全文在 chat_log）。
    fn emit_subagent_reply_landed(
        &self,
        parent_session_key: &str,
        parent_chat_id: &str,
        subagent_id: &str,
        status: &str,
        summary: &str,
    ) {
        if let Some(ref tx) = *self.agent_event_tx.read() {
            let summary: String = summary.chars().take(500).collect();
            let _ = tx.send(nemesis_types::agent::AgentEvent::SubagentReplyLanded {
                session_key: parent_session_key.to_string(),
                chat_id: parent_chat_id.to_string(),
                subagent_id: subagent_id.to_string(),
                status: status.to_string(),
                summary,
            });
        }
    }

    /// 客户端驱动委派：走与模型 spawn 同一 SpawnFn（detached 子代理，
    /// 前台等待最终回复）。`role` 空串 = 缺省（档位推导）；`tools_profile`
    /// 按 SpawnTool 同款值域（"readonly"|"full"，调用方归一化非空传入）。
    ///
    /// 子代理可见性（BUG 修复，路由修正同日）：`parent_chat_id`
    /// = 发起连接级房间 id（`web:{连接id}`，SessionManager send_queue 的键
    /// ——server.rs 事件泵剥前缀查房间表投递子代理工具事件与回执；会话级
    /// id 查不到房间 = 帧静默丢弃）；`parent_session_key` = 发起会话键（
    /// 注册表台账归属 + SubagentDrawer 按会话过滤 + 前端 payload.session_id
    /// 注入）——此前两参写死空串，注册表 `pcid/psk` 双空导致抽屉匹配不上、
    /// 工具事件无父房间可投。
    ///
    /// 父会话持久化（与 admission 早落盘 + 集群续行回复持久化同源语义）：
    /// 委派请求先行落 user 行（会话首建发 SessionCreated，侧栏即时出现），
    /// 成功回复落 assistant 行、失败落错误注记行——chat_log + SessionStore
    /// 双写，+2 行不变量保持（rewind 结构假设），会话不留悬空请求。
    /// `parent_session_key` 空 = 只派发不落盘（防御分支；handler 恒传实值）。
    /// 槽未注入 / 闭包未 set = 诚实 `Err`。
    pub async fn client_spawn(
        &self,
        task: &str,
        role: &str,
        tools_profile: &str,
        parent_chat_id: &str,
        parent_session_key: &str,
    ) -> Result<String, String> {
        let slot = self.spawn_slot.read().clone();
        let spawn_fn = slot.as_ref().and_then(|s| s.get()).ok_or_else(|| {
            "sub-agent spawning is not available on this loop (no spawn channel configured)"
                .to_string()
        })?;
        if !parent_session_key.is_empty() {
            // ×self-heal 同序：store 先物化（get_or_create 内存命中短路，
            // 重建分支在 user 行落盘**前**走完——admission.rs 同序防双计入）。
            if let Some(ref store) = self.session_store {
                let _ = store.get_or_create(parent_session_key);
            }
            let log_existed = Self::session_log_exists_before_append(parent_session_key);
            crate::chat_log::append_chat_log_meta(
                parent_session_key,
                "user",
                task,
                &crate::chat_log::ChatLogMeta {
                    model: None,
                    cron_job_id: None,
                    cron_job_name: None,
                    images: &[],
                    file_changes: &[],
                    deliverables: &[],
                    checkpoint_turn: None, // 委派非 loop turn，无 checkpoint 锚
                    steer: false,
                    work: None,

                    source_node: None,
                    system_notice: None, // LLM 发起的委派任务原文，正常用户气泡
                    channel: None,       // agent 内部委派，非真人通道消息
                },
            );
            if !log_existed {
                // WSAPI 入口无 sender 上下文，principal 空串 = 不落盘
                // （函数内部短路）。
                crate::chat_log::write_session_meta_principal(parent_session_key, "");
                self.emit_session_created(parent_session_key);
            }
            if let Some(ref store) = self.session_store {
                store.add_message(parent_session_key, "user", task);
                if let Err(e) = store.save(parent_session_key) {
                    tracing::warn!(
                        "[client_spawn] session store save failed for {parent_session_key}: {e}"
                    );
                }
            }
        }
        let result = spawn_fn(
            "client",
            task,
            "", // model：空 = SpawnConfig 缺省模型（detached 沿用主模型）
            "web",
            parent_chat_id,
            tools_profile,
            1, // 深度 = 直接子代理（与主代理 spawn 同级；SpawnFn 侧 DetachedOpts.depth）
            false,
            role,
            parent_session_key,
            "", // expert：UI 手动派发无专家语义（专家走模型自主 spawn 面板）
            "", // prompt：UI 手动派发无自拟提示词语义（ad-hoc 降级二跳走模型自主面）
            // 所有权血统（2026-11-28 堵洞①）：主会话 root 恒 = session_key
            //（admission 构造同源），UI 委派子代理继承之。
            parent_session_key,
        )
        .await;
        // 子代理注册表 id（回执事件的台账锚）：按父会话键过滤取最新一条
        // （并发多委派时 list 首条可能是别的委派，不能用全局最新；查不到 =
        // 空串诚实降级——前端 resync/抽屉刷新都不依赖该 id）。
        let registry_hint = crate::subagent_registry::global()
            .list()
            .into_iter()
            .find(|t| t.parent_session_key == parent_session_key)
            .map(|t| t.id)
            .unwrap_or_default();
        match result {
            Ok(reply) => {
                if !parent_session_key.is_empty() {
                    // 续行回复持久化同款双写（chat_log 权威源 + SessionStore
                    // 上下文），模型徽标随行。
                    crate::chat_log::append_chat_log_with_model(
                        parent_session_key,
                        "assistant",
                        &reply,
                        Some(&self.current_display_model()),
                    );
                    if let Some(ref store) = self.session_store {
                        store.add_message(parent_session_key, "assistant", &reply);
                        if let Err(e) = store.save(parent_session_key) {
                            tracing::warn!(
                                "[client_spawn] session store save failed for {parent_session_key}: {e}"
                            );
                        }
                    }
                }
                // 回执事件（弹窗 fire-and-forget 化的完成信号）在落盘后发。
                self.emit_subagent_reply_landed(
                    parent_session_key,
                    parent_chat_id,
                    &registry_hint,
                    "done",
                    &reply,
                );
                Ok(reply)
            }
            Err(e) => {
                // 失败落错误注记行（与普通 turn 错误收尾落盘纪律一致——
                // 会话历史连贯，不留无回复的悬空请求）。
                if !parent_session_key.is_empty() {
                    let note = format!("[委派失败] {e}");
                    crate::chat_log::append_chat_log_meta(
                        parent_session_key,
                        "assistant",
                        &note,
                        &crate::chat_log::ChatLogMeta {
                            model: None,
                            cron_job_id: None,
                            cron_job_name: None,
                            images: &[],
                            file_changes: &[],
                            deliverables: &[],
                            checkpoint_turn: None,
                            steer: false,
                            work: None,

                            source_node: None,
                            system_notice: None,
                            channel: None,
                        },
                    );
                    if let Some(ref store) = self.session_store {
                        store.add_message(parent_session_key, "assistant", &note);
                        // save 失败 warn 对齐同文件 181/223 两臂（全仓唯一
                        // let _ = .save 静默形态；失败注记路径上下文丢失本就
                        // 异常，静默吞错 = 磁盘故障零痕迹）。
                        if let Err(e) = store.save(parent_session_key) {
                            tracing::warn!(
                                "[client_spawn] session store save failed for {parent_session_key}: {e}"
                            );
                        }
                    }
                }
                self.emit_subagent_reply_landed(
                    parent_session_key,
                    parent_chat_id,
                    &registry_hint,
                    "failed",
                    &e,
                );
                Err(e)
            }
        }
    }
}
