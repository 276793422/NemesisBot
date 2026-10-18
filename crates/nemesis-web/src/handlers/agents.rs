//! P2（2026-10-16）— `agents.list/get/create/remove` WSAPI + Agent 持久
//! 实体管理面。
//!
//! 依赖方向（同 [`crate::handlers::projects`] 先例）：nemesis-web 不能反向
//! 依赖 nemesisbot（`AgentManager` 与 tasks.db 在那边），所以这里定义
//! [`AgentsBridge`] trait + 模块级槽，nemesisbot 在 gateway 装配时把实现
//! 装进槽。handler 全部经 bridge 走；未装配（headless / 独立 web 测试）
//! 时诚实报错。
//!
//! main / project 形态不由本面创建：main 是启动 seed（身份投影），project
//! 镜像随 projects.* 走（注册表是路径真相源）。本面只创建/管理 selfbuilt。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_types::channel::InboundMessage;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Bridge trait + 进程级槽
// ---------------------------------------------------------------------------

/// agent 条目的 web 侧投影（serde 序列化直接进 WSAPI 响应）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub name: String,
    /// `main` / `project` / `selfbuilt`。
    pub kind: String,
    /// `provisioning` / `active` / `failed`。
    pub status: String,
    /// 隔离根（自建 = 自己 home；项目 = 项目目录）。
    pub exec_root: String,
    pub home_path: String,
    pub project_id: Option<String>,
    /// `auto` / `mini` / `normal` / `big`（空 = auto）。
    pub tier: String,
    /// 运行时后端档位（P5 第 4 步）：`process` / `vm`——工具执行走哪个
    /// 常驻执行体（下次调用现读生效，切档无需重启）。
    pub runtime_backend: String,
    /// 空 = 主 workspace 记忆域；`agents/<id>` = 独占域。
    pub memory_namespace: String,
    /// 凭据引用条数（引用清单不出 web 面——凭据本体在 vault）。
    pub credential_count: usize,
    pub created_at: String,
    pub last_active_at: String,
}

/// nemesisbot 侧 agent 实体管理能力的抽象（实现在 nemesisbot
/// `AgentManager`）。查询面全同步——真相源是 tasks.db（SQLite 快查）；
/// 生命周期面（T9）走异步——sleep/wake 涉及子进程停起（tokio IO）。
#[async_trait::async_trait]
pub trait AgentsBridge: Send + Sync {
    /// 列出全部记录（main/project/selfbuilt 三类组并存可见）。
    fn list(&self) -> Vec<AgentInfo>;
    /// 单条查询。
    fn get(&self, agent_id: &str) -> Result<AgentInfo, String>;
    /// 创建自建 agent（两阶段：provisioning → home 物化 → active）。
    /// `runtime_backend`：`process|vm`（空串同 process；值域校验在实现侧
    /// 单一真相源，非法值 loud 拒绝）。
    fn create(&self, name: &str, tier: &str, runtime_backend: &str) -> Result<AgentInfo, String>;
    /// 运行时后端切档（P5 第 4 步，G4 ⑦「上层无感」写入口）：落库后下一
    /// 次工具调用现读生效，无需重启 loop 或迁移会话。
    fn update_backend(&self, agent_id: &str, runtime_backend: &str) -> Result<AgentInfo, String>;
    /// 删除记录（main 不可删；物理 home 保留——解除实体注册不做文件清除）。
    /// 异步：删除联动常驻执行体停机（evict 等在途调用收尾）。
    async fn remove(&self, agent_id: &str) -> Result<(), String>;

    /// sleep（P3 T9）：常驻执行体优雅退出（状态全在 home 落盘）。返回
    /// false = 槽本来就不在（或常驻档未开——无进程可睡）。
    async fn sleep(&self, agent_id: &str) -> Result<bool, String>;

    /// wake（P3 T9）：按记录重拉常驻执行体（exec_root 取自记录；home 即
    /// 状态——journal/配置/浏览器 profile 从盘上回载）。错误 = 记录不存在 /
    /// 常驻档关闭 / spawn 失败（诚实上抛）。
    async fn wake(&self, agent_id: &str) -> Result<(), String>;

    /// 槽位快照（T8 宣告同源数据）：当前哪些 agent 有常驻槽、活没活。
    fn slots(&self) -> Vec<AgentSlotInfo>;

    /// 常驻档是否开启（config `executor.persistent.enabled` 现读）。
    fn persistent_enabled(&self) -> bool;

    /// G1：解析（惰性构建）自建 agent 的常驻 LLM loop。消费面 =
    /// `resolve_session_loop` 的 agents 臂（审批/提问/会话级命令路由）。
    /// 同步面——构建链（sqlite 记录读 + 工厂装配）无 await；错误 =
    /// 不存在 / main·project 正门重定向 / 构建失败（诚实上抛）。
    fn loop_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>, String>;

    /// G1：投递一条入站消息进 agent loop（`agents.send` 面；出站回复经
    /// loop 的 agent_outbound 链按 chat_id 回到发起连接）。
    async fn send_to_agent(&self, agent_id: &str, msg: InboundMessage) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// agent 会话键（G1 会话身份单一真相源）
// ---------------------------------------------------------------------------

/// agent 会话键形态：`agents:{agent_id}:session:{sid}`。sid 消毒同主会话
/// 纪律（[`nemesis_agent::session::SessionStore::sanitize_session_id`]
/// 单一真相源）。本函数与 [`agent_owner_of`] 是该形态的构造/解析单点。
pub fn agent_session_key(agent_id: &str, session_id: &str) -> String {
    let sid = nemesis_agent::session::SessionStore::sanitize_session_id(session_id);
    format!("agents:{agent_id}:session:{sid}")
}

/// session_key → 归属 agent_id（纯字符串解析；None = 非 agent 会话——
/// 主会话 / 项目会话形态照旧走各自路由）。
pub fn agent_owner_of(session_key: &str) -> Option<String> {
    let rest = session_key.strip_prefix("agents:")?;
    let (id, _sid) = rest.split_once(":session:")?;
    if id.is_empty() {
        return None;
    }
    Some(id.to_string())
}

/// 常驻槽位快照的 web 投影（T9；supervisor `SlotState` 的同构镜像——
/// web crate 不反向依赖 nemesisbot，字段手工对齐）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentSlotInfo {
    pub agent_id: String,
    pub alive: bool,
    pub idle_secs: u64,
}

static AGENTS_BRIDGE: std::sync::RwLock<Option<Arc<dyn AgentsBridge>>> =
    std::sync::RwLock::new(None);

/// gateway 装配期安装 bridge（nemesisbot `AgentManager` 的实现）。
pub fn install_agents_bridge(bridge: Arc<dyn AgentsBridge>) {
    *AGENTS_BRIDGE.write().expect("agents bridge lock") = Some(bridge);
}

/// 取当前 bridge（clone Arc 出来，不持锁返回）。
pub fn agents_bridge() -> Option<Arc<dyn AgentsBridge>> {
    AGENTS_BRIDGE.read().expect("agents bridge lock").clone()
}

fn require_bridge() -> Result<Arc<dyn AgentsBridge>, String> {
    agents_bridge().ok_or_else(|| {
        "agent 实体管理未装配（agents bridge 未接线，网关需为装配 gateway 模式）".to_string()
    })
}

#[cfg(test)]
pub(crate) fn set_agents_bridge_for_test(bridge: Option<Arc<dyn AgentsBridge>>) {
    *AGENTS_BRIDGE.write().expect("agents bridge lock") = bridge;
}

/// bridge 触碰类测试的共享串行锁（进程级槽是共享态——并行测试互相置换
/// 会 flake；同 PROJECTS_BRIDGE 家族纪律）。只在 tests 里 acquire。
#[cfg(test)]
pub(crate) static AGENTS_BRIDGE_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

// ---------------------------------------------------------------------------
// WSAPI handler
// ---------------------------------------------------------------------------

pub struct AgentsHandler;

#[async_trait::async_trait]
impl ModuleHandler for AgentsHandler {
    fn module_name(&self) -> &str {
        "agents"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list", "get", "create", "update", "remove", "sleep", "wake", "slots", "send",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let bridge = require_bridge()?;
        match cmd {
            "list" => {
                let agents = bridge.list();
                let count = agents.len();
                Ok(Some(
                    serde_json::json!({ "agents": agents, "count": count }),
                ))
            }
            "get" => {
                let data = data.ok_or("missing data")?;
                let agent_id = crate::handlers::get_str(&data, "agent_id")?;
                let info = bridge.get(&agent_id)?;
                Ok(Some(serde_json::json!({ "agent": info })))
            }
            "create" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                // tier 可选（缺省 auto；空串同 auto）。
                let tier = crate::handlers::get_str(&data, "tier").unwrap_or_default();
                // runtime_backend 可选（缺省 process；空串同 process）。
                let runtime_backend =
                    crate::handlers::get_str(&data, "runtime_backend").unwrap_or_default();
                let info = bridge.create(&name, &tier, &runtime_backend)?;
                Ok(Some(serde_json::json!({ "agent": info })))
            }
            // P5 第 4 步：运行时后端切档（G4 ⑦「上层无感」——落库即生效，
            // 下次工具调用现读，无需重启 loop）。
            "update" => {
                let data = data.ok_or("missing data")?;
                let agent_id = crate::handlers::get_str(&data, "agent_id")?;
                let runtime_backend = crate::handlers::get_str(&data, "runtime_backend")?;
                let info = bridge.update_backend(&agent_id, &runtime_backend)?;
                Ok(Some(serde_json::json!({
                    "agent": info,
                    "note": "档位已落库：下次工具调用现读生效，无需重启 loop；原档常驻实例如需释放走 sleep",
                })))
            }
            "remove" => {
                let data = data.ok_or("missing data")?;
                let agent_id = crate::handlers::get_str(&data, "agent_id")?;
                bridge.remove(&agent_id).await?;
                Ok(Some(serde_json::json!({
                    "removed": agent_id,
                    "note": "仅解除实体注册，未删除 home 目录内的任何文件",
                })))
            }
            // T9 生命周期面：sleep/wake（常驻执行体停起）+ slots（槽位快照）。
            "sleep" => {
                let data = data.ok_or("missing data")?;
                let agent_id = crate::handlers::get_str(&data, "agent_id")?;
                let slept = bridge.sleep(&agent_id).await?;
                Ok(Some(serde_json::json!({
                    "agent_id": agent_id,
                    "slept": slept,
                    "note": if slept {
                        "常驻执行体已退出，状态在 home 落盘"
                    } else {
                        "无在跑槽位（或常驻档未开）——无进程可睡"
                    },
                })))
            }
            "wake" => {
                let data = data.ok_or("missing data")?;
                let agent_id = crate::handlers::get_str(&data, "agent_id")?;
                bridge.wake(&agent_id).await?;
                Ok(Some(serde_json::json!({
                    "agent_id": agent_id,
                    "woken": true,
                    "note": "常驻执行体已按记录重拉，home 即状态",
                })))
            }
            "slots" => Ok(Some(serde_json::json!({
                "persistent_enabled": bridge.persistent_enabled(),
                "slots": bridge.slots(),
            }))),
            // G1：给自建 agent 的常驻 LLM loop 投递一条消息。用户行先入
            // 环（chat.sync / 历史可见性），不回 echo——本响应即发起方
            // ack；agent 回复经 loop 出站链路按 chat_id 回到本连接
            // （receive 帧，session_id 盖章仅 main 形态——v1 已知边界）。
            "send" => {
                let data = data.ok_or("missing data")?;
                let agent_id = crate::handlers::get_str(&data, "agent_id")?;
                let content = crate::handlers::get_str(&data, "content")?;
                if content.trim().is_empty() {
                    return Err("content 不能为空".to_string());
                }
                let session_id = data
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("default");
                let session_key = agent_session_key(&agent_id, session_id);
                let _seq =
                    crate::chat_event_log::record(&session_key, "user", &content, None, None);
                let msg = InboundMessage {
                    channel: "web".to_string(),
                    sender_id: "web".to_string(),
                    chat_id: ctx.chat_id.clone(),
                    content,
                    media: vec![],
                    session_key,
                    correlation_id: String::new(),
                    metadata: Default::default(),
                    voice_playback: None,
                };
                bridge.send_to_agent(&agent_id, msg).await?;
                Ok(Some(serde_json::json!({
                    "accepted": true,
                    "agent_id": agent_id,
                    "session_id": session_id,
                    "note": "回复经 agent loop 出站链路异步送达发起连接（receive 帧）",
                })))
            }
            _ => Err(format!("unknown agents cmd: {cmd}")),
        }
    }
}

#[cfg(test)]
mod agents_ws_tests;
