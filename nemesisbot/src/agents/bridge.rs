//! P2：[`nemesis_web::handlers::agents::AgentsBridge`] 的 nemesisbot 侧实现
//! （依赖方向：nemesisbot → nemesis-web 合法，反向不合法——trait 定义在
//! web 侧，实现落在 Manager 所在的这边）。
//!
//! 投影规则：AgentRecord → AgentInfo 一一映射；credential_refs 只出条数
//! 不出内容（凭据本体在 vault，web 面不回显）。create 的 tier 走显式
//! 白名单校验（空串/auto/mini/normal/big），非法值 loud 拒绝——tier 会
//! 进 AgentLoopSpec 供给闸，垃圾值宁可创建期报错。

use super::AgentManager;
use nemesis_web::handlers::agents::{AgentInfo, AgentSlotInfo, AgentsBridge};
use std::sync::Arc;

/// tier 白名单（空串与 auto 同义；与 `resolve_active_tier` 的档位集一致）。
fn validate_tier(tier: &str) -> Result<String, String> {
    match tier.trim() {
        "" | "auto" => Ok(String::new()),
        "mini" | "normal" | "big" => Ok(tier.trim().to_string()),
        other => Err(format!(
            "tier 非法: {other:?}（允许 auto/mini/normal/big，空串同 auto）"
        )),
    }
}

/// AgentRecord → web 投影（serde 序列化面；不出 tool_policy/persona_refs
/// 内部载荷——管理面最小可见）。
fn to_info(rec: nemesis_taskstore::AgentRecord) -> AgentInfo {
    let credential_count = rec.credential_refs.as_array().map(Vec::len).unwrap_or(0);
    AgentInfo {
        agent_id: rec.agent_id,
        name: rec.name,
        kind: rec.kind,
        status: rec.status,
        exec_root: rec.exec_root,
        home_path: rec.home_path,
        project_id: rec.project_id,
        tier: rec.tier,
        memory_namespace: rec.memory_namespace,
        credential_count,
        created_at: rec.created_at.clone(),
        // 未触碰过 = None → 回落 created_at（投影面不给 null——web 消费方
        // 按字符串渲染；语义「最后一次活动时间不早于创建时间」）。
        last_active_at: rec.last_active_at.unwrap_or_else(|| rec.created_at.clone()),
    }
}

#[async_trait::async_trait]
impl AgentsBridge for AgentManager {
    fn list(&self) -> Vec<AgentInfo> {
        // 列表面不给单条错误炸整个响应——读失败按空处理（tasks.db 打开期
        // 已在 boot 闸过；此处失败=存储层异常，get/create 臂会诚实上抛）。
        self.list()
            .unwrap_or_default()
            .into_iter()
            .map(to_info)
            .collect()
    }

    fn get(&self, agent_id: &str) -> Result<AgentInfo, String> {
        let rec = self
            .get(agent_id)?
            .ok_or_else(|| format!("agent 不存在: {agent_id}"))?;
        Ok(to_info(rec))
    }

    fn create(&self, name: &str, tier: &str) -> Result<AgentInfo, String> {
        let tier = validate_tier(tier)?;
        let rec = self.create_agent(super::CreateAgentParams {
            name: name.to_string(),
            tier,
            tool_policy: serde_json::json!({}),
            persona_refs: serde_json::Value::Null,
            credential_refs: serde_json::json!([]),
        })?;
        Ok(to_info(rec))
    }

    async fn remove(&self, agent_id: &str) -> Result<(), String> {
        self.remove(agent_id).await
    }

    // -- T9 生命周期面 ------------------------------------------------------------

    async fn sleep(&self, agent_id: &str) -> Result<bool, String> {
        match self.runtime() {
            Some(rt) => Ok(rt.sleep_agent(agent_id).await),
            None => Ok(false),
        }
    }

    async fn wake(&self, agent_id: &str) -> Result<(), String> {
        let rt = self
            .runtime()
            .ok_or_else(|| "常驻执行体未装配（supervisor 未接线）".to_string())?;
        // exec_root 取自记录（T7 同源语义：main=workspace / project=项目
        // 目录 / selfbuilt=home）。
        let rec = self
            .get(agent_id)?
            .ok_or_else(|| format!("agent 不存在: {agent_id}"))?;
        let exec_root = std::path::PathBuf::from(&rec.exec_root);
        rt.wake_agent(agent_id, &exec_root)
            .await
            .map_err(|e| e.to_string())?;
        // wake 是一次显式活动：记 last_active（P2 touch 消费点）。
        let _ = self.touch(agent_id);
        Ok(())
    }

    fn slots(&self) -> Vec<AgentSlotInfo> {
        match self.runtime() {
            Some(rt) => rt
                .slot_states()
                .into_iter()
                .map(|s| AgentSlotInfo {
                    agent_id: s.agent_id,
                    alive: s.alive,
                    idle_secs: s.idle_secs,
                })
                .collect(),
            None => Vec::new(),
        }
    }

    fn persistent_enabled(&self) -> bool {
        self.runtime()
            .map(|rt| rt.persistent_enabled())
            .unwrap_or(false)
    }

    // -- G1 会话面（agents.send / resolve_session_loop agents 臂）-------------

    fn loop_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>, String> {
        let lm = self.loop_manager().ok_or_else(|| {
            "agent 会话面未装配（loop manager 未接线，网关需为装配 gateway 模式）".to_string()
        })?;
        lm.loop_for_agent(agent_id)
    }

    async fn send_to_agent(
        &self,
        agent_id: &str,
        msg: nemesis_types::channel::InboundMessage,
    ) -> Result<(), String> {
        let lm = self.loop_manager().ok_or_else(|| {
            "agent 会话面未装配（loop manager 未接线，网关需为装配 gateway 模式）".to_string()
        })?;
        lm.send_message(agent_id, msg).await?;
        // 投递是一次显式活动：记 last_active（P2 touch 消费点，尽力而为）。
        let _ = self.touch(agent_id);
        Ok(())
    }
}
