//! 自建 agent 常驻 LLM loop 生命周期管理器（G1，2026-10-18 三方复评整改批三）。
//!
//! 职责：按 agents 表记录（kind=selfbuilt）惰性构建并常驻 [`AgentLoop`]——
//! 与项目 loop（`crate::projects::manager::ProjectLoopManager`）完全同构的
//! 装配链（统一工厂 [`crate::agent_factory::build_agent_loop_from_spec`]，
//! 安全 8 层/estop/围栏/discipline 全生效——agent 会话不是安全旁路），差异
//! 收敛在 [`crate::agent_factory::AgentLoopSpec::from_record`] 的记录映射。
//!
//! 消息面：**不走 bus 订阅**（v1 WSAPI-only——`agents.send` 经
//! [`nemesis_web::handlers::agents::AgentsBridge::send_to_agent`] 直接注入
//! 本 manager 的 per-loop mpsc，镜像项目调度 `route_one` 的转发目标形态）。
//! loop 出站走 `shared.agent_outbound_tx` → bus → web 通道 → 发起方 WS 连接
//! （chat_id 寻址），与主/项目会话的回包链同源。
//!
//! 生命周期：惰性构建（首次 `agents.send` / bridge 触达时）；`AgentManager::
//! remove` 联动 [`Self::remove`] 停机摘表（R4 协调点——记录删了 loop 不留）；
//! gateway teardown 走 [`Self::stop_all`]。
//!
//! 刻意持 [`SqliteAgentsStore`] 而非 `Arc<AgentManager>`：manager 已持本
//! manager 的槽（remove 联动），反向再持 manager 即 Arc 引用环——store 是
//! 真相源的直接句柄，够用且无环。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tracing::{info, warn};

use nemesis_agent::session::SessionStore;
use nemesis_taskstore::{SqliteAgentsStore, TaskDb};
use nemesis_types::channel::InboundMessage;

use crate::agent_factory::SharedResources;

#[cfg(test)]
mod tests;

struct AgentLoopHandle {
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    agent_task: tokio::task::JoinHandle<()>,
}

/// 自建 agent 常驻 loop 的生命周期管理（gateway 装配期创建一个实例，挂在
/// [`crate::agents::AgentManager`] 的槽上；teardown `stop_all`）。
pub struct AgentLoopManager {
    shared: Arc<SharedResources>,
    store: SqliteAgentsStore,
    /// 主 workspace 根（凭据 pre-flight 解析的锚）。
    workspace: PathBuf,
    session_store: Arc<SessionStore>,
    rt: tokio::runtime::Handle,
    state: Mutex<HashMap<String, AgentLoopHandle>>,
    /// agent_id → loop 的 mpsc 发送端（`send_to_agent` 注入目标）。与
    /// `state` 分离同项目 manager 纪律：注入只依赖通道；spawn 成对插入、
    /// remove 成对摘除。
    channels: Mutex<HashMap<String, tokio::sync::mpsc::Sender<InboundMessage>>>,
}

impl AgentLoopManager {
    pub fn new(
        shared: Arc<SharedResources>,
        db: Arc<TaskDb>,
        session_store: Arc<SessionStore>,
    ) -> Self {
        let workspace = shared.workspace_dir();
        Self {
            shared,
            store: SqliteAgentsStore::from_db(db),
            workspace,
            session_store,
            rt: tokio::runtime::Handle::current(),
            state: Mutex::new(HashMap::new()),
            channels: Mutex::new(HashMap::new()),
        }
    }

    /// 解析（惰性构建）agent 的常驻 loop。幂等（已在跑 = 直接返回）。
    ///
    /// 诚实拒绝面（全部在构建之前——不建半途 loop）：
    /// - 记录不存在：`agent 不存在`
    /// - kind != selfbuilt：main/project 各有正门（main 会话走对话组
    ///   `chat.send`；项目会话走 projects API + 项目注册路由）——本 manager
    ///   不重复承载
    /// - status != active：provisioning/failed 记录不可会话
    fn resolve_or_build(
        &self,
        agent_id: &str,
    ) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>, String> {
        {
            let state = self.state.lock().unwrap();
            if let Some(h) = state.get(agent_id) {
                return Ok(h.agent_loop.clone());
            }
        }
        let record = self
            .store
            .get(agent_id)?
            .ok_or_else(|| format!("agent 不存在: {agent_id}"))?;
        if record.kind != nemesis_taskstore::agents::kind::SELFBUILT {
            let gate = if record.kind == nemesis_taskstore::agents::kind::MAIN {
                "main agent 会话走对话组（chat.send / Dashboard 对话视图）"
            } else {
                "项目 agent 会话走项目分组（projects API / 项目会话路由）"
            };
            return Err(format!(
                "agent {agent_id}（{}）不经本通道：{gate}",
                record.kind
            ));
        }
        if record.status != nemesis_taskstore::agents::status::ACTIVE {
            return Err(format!(
                "agent {agent_id} 状态为 {}，当前不可会话（provisioning 会随启动 sweep 收敛；failed 需人工处理）",
                record.status
            ));
        }
        // 凭据 pre-flight（G1 v1 消费形态：装配期解析可观测——解析失败
        // warn+skip 不阻断；凭据的实际消费走既有 credential_injection 的
        // 工具入参 `vault:` 别名面（最后时刻改写，本体不出 gateway））。
        self.preflight_credentials(&record);
        let mut spec = crate::agent_factory::AgentLoopSpec::from_record(&record);
        // G2（2026-10-18 批四）：per-agent 浏览器身份目录（agent home 的
        // browser-profile/——配置 opt-in `browser_profile: true` 的浏览器类
        // MCP server spawn 时注入 `--user-data-dir`，登录态/cookie per-agent
        // 隔离、跨任务跨重建保持）。派生式单一真相源 = nemesis-path。
        spec.browser_profile = Some(nemesis_path::resolve_agent_browser_profile_in_workspace(
            &self.workspace,
            &record.agent_id,
        ));
        let agent_loop = crate::agent_factory::build_agent_loop_from_spec(
            &self.shared,
            &spec,
            self.session_store.clone(),
        )
        .map_err(|e| format!("构建 agent loop 失败: {e}"))?;
        // 与主 loop 的 AgentLoopServiceAdapter / 项目 spawn 同构：per-loop
        // mpsc 入站通道 + reinject tx（queue-drain 回灌走同一条路）+ 后台
        // 任务消费。
        let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(1024);
        agent_loop.set_reinject_tx(inbound_tx.clone());
        let loop_for_task = agent_loop.clone();
        let agent_task = self.rt.spawn(async move {
            loop_for_task.run_bus_arc(inbound_rx).await;
        });
        let mut state = self.state.lock().unwrap();
        if state.contains_key(agent_id) {
            // 极小窗口的双 spawn（resolve_or_build 单调用方+幂等闸几乎不
            // 触发）；输者清理自己（镜像 spawn_project）。
            drop(state);
            agent_loop.stop();
            agent_loop.clear_session_busy();
            agent_task.abort();
            return Ok(self
                .state
                .lock()
                .unwrap()
                .get(agent_id)
                .map(|h| h.agent_loop.clone())
                .expect("double-spawn guard: winner handle must exist"));
        }
        state.insert(
            agent_id.to_string(),
            AgentLoopHandle {
                agent_loop: agent_loop.clone(),
                agent_task,
            },
        );
        drop(state);
        self.channels
            .lock()
            .unwrap()
            .insert(agent_id.to_string(), inbound_tx);
        info!(agent_id = %agent_id, "[AgentLoops] agent loop 已启动");
        Ok(agent_loop)
    }

    /// 凭据 pre-flight：逐条解析记录的 credential_refs（warn+skip，只做
    /// 装配期可观测——vault 缺失/口令不可用不阻断 loop 构建）。
    fn preflight_credentials(&self, record: &nemesis_taskstore::AgentRecord) {
        let Some(refs) = record.credential_refs.as_array() else {
            return;
        };
        if refs.is_empty() {
            return;
        }
        #[cfg(feature = "security")]
        for r in refs {
            let Some(alias) = r.as_str() else { continue };
            let Some(name) = alias
                .strip_prefix(&format!("vault:agent/{}/", record.agent_id))
                .filter(|n| !n.is_empty())
            else {
                continue;
            };
            match crate::agents::resolve_agent_credential(&self.workspace, &record.agent_id, name) {
                Ok(_) => {}
                Err(e) => warn!(
                    agent_id = %record.agent_id,
                    "[AgentLoops] 凭据 pre-flight 解析失败（warn+skip，不阻断装配；工具入参 vault: 别名消费时同样现解析）: {e}"
                ),
            }
        }
        #[cfg(not(feature = "security"))]
        {
            // security 裁剪形态：vault 不在场，引用面解析层整个缺失——只
            // 留痕（记录带 credential_refs 的 agent 在裁剪形态本就无法消费
            // 凭据，创建面也不该引用出 vault）。
            warn!(
                agent_id = %record.agent_id,
                "[AgentLoops] security feature 未编译，credential_refs 无法 pre-flight（vault 面缺席）"
            );
        }
    }

    /// 投递一条入站消息进 agent loop（不存在/不可会话 = 诚实报错；通道
    /// 关闭 = loop 刚被 remove 的窗口，同样诚实报错）。
    pub async fn send_message(&self, agent_id: &str, msg: InboundMessage) -> Result<(), String> {
        self.resolve_or_build(agent_id)?;
        let tx = self
            .channels
            .lock()
            .unwrap()
            .get(agent_id)
            .cloned()
            .ok_or_else(|| format!("agent {agent_id} loop 通道不在场（刚被移除？）"))?;
        tx.send(msg)
            .await
            .map_err(|_| format!("agent {agent_id} loop 已停止，消息未投递"))?;
        Ok(())
    }

    /// 取运行中 agent loop 的 Arc（bridge `loop_for_agent` 消费）。
    pub fn loop_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>, String> {
        self.resolve_or_build(agent_id)
    }

    /// 停止并摘除单个 agent loop（R4 联动：`AgentManager::remove` 在删记录
    /// 前调用）。镜像项目 `stop_project`：先摘两表再停——此后 send 诚实
    /// 报错，不静默丢。
    pub fn remove(&self, agent_id: &str) -> bool {
        let handle = self.state.lock().unwrap().remove(agent_id);
        self.channels.lock().unwrap().remove(agent_id);
        let Some(handle) = handle else {
            return false;
        };
        handle.agent_loop.stop();
        handle.agent_loop.clear_session_busy();
        // 镜像 AgentLoopServiceAdapter::stop：用户显式删除，abort 掉在途
        // 轮次（未完成的回复可接受）；session busy 已清，不产生死锁会话。
        handle.agent_task.abort();
        info!(agent_id = %agent_id, "[AgentLoops] agent loop 已停止（记录已删联动）");
        true
    }

    /// 停止全部 agent loop（gateway teardown）。
    pub fn stop_all(&self) {
        let ids: Vec<String> = self.state.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.remove(&id);
        }
    }

    /// agent loop 是否在跑（可观测面）。`#[allow(dead_code)]`：当前唯一
    /// 消费方是本 crate 测试（round-trip/remove 用例）；Dashboard agent
    /// 管理页接入「loop 存活」徽标时转正（同 credential_alias 的 P5 形态）。
    #[allow(dead_code)]
    pub fn is_running(&self, agent_id: &str) -> bool {
        self.state.lock().unwrap().contains_key(agent_id)
    }
}
