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
    fn create(&self, name: &str, tier: &str) -> Result<AgentInfo, String>;
    /// 删除记录（main 不可删；物理 home 保留——解除实体注册不做文件清除）。
    fn remove(&self, agent_id: &str) -> Result<(), String>;

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
        &["list", "get", "create", "remove", "sleep", "wake", "slots"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        _ctx: &RequestContext,
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
                let info = bridge.create(&name, &tier)?;
                Ok(Some(serde_json::json!({ "agent": info })))
            }
            "remove" => {
                let data = data.ok_or("missing data")?;
                let agent_id = crate::handlers::get_str(&data, "agent_id")?;
                bridge.remove(&agent_id)?;
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
            _ => Err(format!("unknown agents cmd: {cmd}")),
        }
    }
}

#[cfg(test)]
mod agents_ws_tests;
