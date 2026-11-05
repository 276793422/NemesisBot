//! 客户端驱动委派（2026-09-28 角色目录与分档供给）：WSAPI `chat.spawn` 的
//! loop 侧入口。
//!
//! 客户端（Dashboard 委派卡）直接把任务派给指定角色的子代理——同一
//! detached 通道与模型侧 `spawn` 工具共用（同一 SpawnFn 闭包），工具档位/
//! 深度/角色模板/安全管线全部同源。区别只在入口：模型的 spawn 走工具派发
//! 闸（角色目录闸在 dispatch），客户端 spawn 的角色合法性由 handler 侧
//! [`Self:check_client_role`] 用同一 [`Self:visible_roles`] 裁决——两处
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
    /// [`Self:visible_roles`]——前端下拉看到的集合就是模型能用的集合。
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

    /// 发布委派回执落会话事件（[`nemesis_types:agent:AgentEvent:
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
                },
            );
            if !log_existed {
                // ：WSAPI 入口无 sender 上下文，principal 空串 = 不落盘
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
                        },
                    );
                    if let Some(ref store) = self.session_store {
                        store.add_message(parent_session_key, "assistant", &note);
                        let _ = store.save(parent_session_key);
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
