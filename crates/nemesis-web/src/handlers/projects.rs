//! L6++ G4（2026-09-08）— `projects.list/create/remove/rename/open_dir`
//! WSAPI + 会话归属解析（resolve）基础设施。
//!
//! 依赖方向（关键）：nemesis-web 不能反向依赖 nemesisbot（项目注册表与
//! 项目 loop 生命周期管理器 `ProjectLoopManager` 在那边），所以这里定义
//! [`ProjectsBridge`] trait（先例 = nemesis-channels 的 `WebServerOps`：
//! trait 定义在消费方，实现方在上游 crate），nemesisbot 在 gateway 装配时
//! 把 manager 实现装进模块级 [`PROJECTS_BRIDGE`] 槽。handler 全部经 bridge
//! 走，不持具体类型；未装配（headless / 独立 web 测试）时诚实报错。
//!
//! [`resolve_session_loop`] 是「这条会话该用哪个 AgentLoop」的单一裁决点：
//! bridge 归属命中 → 项目 loop（不可用则诚实报错）；否则主槽
//! `AppState.agent_loop`（**不扩 AppState**——字面量 69 文件 105 处不碰）。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::r#loop::AgentLoop;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Bridge trait + 进程级槽
// ---------------------------------------------------------------------------

/// 项目条目的 web 侧投影（serde 序列化直接进 WSAPI 响应）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    /// canonical（剥 verbatim）后的项目目录绝对路径。
    pub path: String,
    /// RFC3339 创建时间。
    pub created_at: String,
    /// 项目 loop 是否在运行。
    pub running: bool,
}

/// nemesisbot 侧项目管理能力的抽象（[`ProjectLoopManager`](动态)）。
/// 全部同步方法——注册表是文件读写，loop 句柄是内存查表。
pub trait ProjectsBridge: Send + Sync {
    /// 列出全部项目（lenient：注册表损坏给空 + warn）。
    fn list(&self) -> Vec<ProjectInfo>;
    /// 创建项目（校验链：路径存在→canonicalize→重叠→上限→写 registry→
    /// spawn loop；错误为诚实中文文案，直接回显前端）。
    fn create(&self, name: &str, path: &str) -> Result<ProjectInfo, String>;
    /// 移除项目（只解除分组——不删会话与项目目录内的任何文件）。
    /// 内部：写 registry + stop loop。返回被移除条目（前端提示用）。
    fn remove(&self, project_id: &str) -> Result<ProjectInfo, String>;
    /// 重命名（path 不可改——Phase 1 语义）。
    fn rename(&self, project_id: &str, new_name: &str) -> Result<ProjectInfo, String>;
    /// 会话归属查询（session_key → project_id；None = 对话组）。
    fn owner_of(&self, session_key: &str) -> Option<String>;
    /// 项目 loop 句柄（未运行 / 目录消失 = None——调用方诚实报错）。
    fn loop_for_session(&self, project_id: &str) -> Option<Arc<AgentLoop>>;
    /// 项目目录（fs.tree / chat.todo_get 的根切换用；None = 未注册）。
    fn project_path(&self, project_id: &str) -> Option<std::path::PathBuf>;
    /// 不可用项目的展示名（默认 = pid 兜底；真实三级回落——注册表名 →
    /// 会话 sidecar project_path 尾段 → pid——由 nemesisbot
    /// ProjectLoopManager 覆写实现，bridge 侧测试假件零改动）。
    fn display_label(&self, project_id: &str, _session_key: &str) -> String {
        project_id.to_string()
    }
    /// 项目会话绑定（sessions.create 带 project_id）：校验项目存在 +
    /// 目录在 → 烧 sidecar project 字段 + 登记内存索引。
    fn bind_session(&self, session_key: &str, project_id: &str) -> Result<(), String>;
    /// 摘除会话归属（sessions.delete/clear 联动）：摘内存索引 + 清 sidecar
    /// 的 project 字段（title 等保留；sidecar 已随 delete_chat_log 删除时
    /// best-effort no-op）。
    fn forget_session(&self, session_key: &str);
}

/// 进程级 bridge 槽。**设计偏差说明**（vs impl plan 原文「OnceLock」）：
/// OnceLock 只能 set 一次， nemesis-web 的 crate 测试需要「未装配诚实报错」
/// 与「装配后各命令臂」两类形态并存（并行测试进程内共享槽）——`RwLock<
/// Option<_>>` 保有完全相同的生产语义（gateway 装配期安装一次、handler
/// 只读）且让测试可以置换。gateway 侧重复安装按 latest-wins（每个 gateway
/// 实例装自己的 manager）。
static PROJECTS_BRIDGE: std::sync::RwLock<Option<Arc<dyn ProjectsBridge>>> =
    std::sync::RwLock::new(None);

/// gateway 装配期安装 bridge（nemesisbot `ProjectLoopManager` 的实现）。
pub fn install_projects_bridge(bridge: Arc<dyn ProjectsBridge>) {
    *PROJECTS_BRIDGE.write().expect("projects bridge lock") = Some(bridge);
}

/// 取当前 bridge（clone Arc 出来，不持锁返回）。
pub fn projects_bridge() -> Option<Arc<dyn ProjectsBridge>> {
    PROJECTS_BRIDGE
        .read()
        .expect("projects bridge lock")
        .clone()
}

fn require_bridge() -> Result<Arc<dyn ProjectsBridge>, String> {
    projects_bridge()
        .ok_or_else(|| "项目管理未装配（projects bridge 未接线，网关需为装配 gateway 模式）".to_string())
}

#[cfg(test)]
pub(crate) fn set_projects_bridge_for_test(bridge: Option<Arc<dyn ProjectsBridge>>) {
    *PROJECTS_BRIDGE.write().expect("projects bridge lock") = bridge;
}

/// bridge 触碰类测试的共享串行锁（进程级槽是共享态——并行测试互相置换
/// 会 flake；同 env-test-race 家族纪律）。只在 tests 里 acquire。
#[cfg(test)]
pub(crate) static BRIDGE_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

// ---------------------------------------------------------------------------
// resolve：这条会话该用哪个 AgentLoop（单一裁决点）
// ---------------------------------------------------------------------------

/// 归属解析：bridge 归属命中 → 项目 loop（不可用诚实报错，与 bus 路由的
/// send_unroutable_error 同语义）；无归属或 bridge 未装配 → 主槽
/// `AppState.agent_loop`。调用方（chat/tools/approval/question/agent 各
/// handler）拿到的 Arc 直接替换原先的 `ctx.state.agent_loop.read().clone()`。
pub fn resolve_session_loop(
    ctx: &RequestContext,
    session_key: &str,
) -> Result<Arc<AgentLoop>, String> {
    if let Some(bridge) = projects_bridge()
        && let Some(pid) = bridge.owner_of(session_key)
    {
        return bridge.loop_for_session(&pid).ok_or_else(|| {
            let name = bridge.display_label(&pid, session_key);
            format!("项目「{name}」当前不可用（目录缺失或已移除），无法操作该会话")
        });
    }
    ctx.state
        .agent_loop
        .read()
        .clone()
        .ok_or_else(|| "agent loop not running".to_string())
}

/// 项目会话的 workspace 根切换（fs.tree / fs.complete_path / chat.todo_get
/// 用）：归属命中且项目已注册 → 项目目录；否则 None（保持主 workspace）。
/// 返回 canonical 绝对路径字符串。
pub fn project_root_for_session(session_key: &str) -> Option<String> {
    let bridge = projects_bridge()?;
    let pid = bridge.owner_of(session_key)?;
    bridge
        .project_path(&pid)
        .map(|p| p.to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------
// WSAPI handler（projects.list / create / remove / rename / open_dir）
// ---------------------------------------------------------------------------

/// `open_dir` 目标解析：**只允许打开注册表已登记的项目目录**（防任意路径
/// 被诱导打开——打开能力虽然轻，入口也收在已知集合内）。未注册 / 目录已
/// 消失（inactive）都诚实报错。
pub(crate) fn resolve_open_target(
    bridge: &dyn ProjectsBridge,
    project_id: &str,
) -> Result<std::path::PathBuf, String> {
    let path = bridge
        .project_path(project_id)
        .ok_or_else(|| format!("项目不存在: {project_id}"))?;
    if !path.is_dir() {
        let name = bridge
            .list()
            .into_iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name)
            .unwrap_or_else(|| project_id.to_string());
        return Err(format!(
            "项目「{name}」的目录不存在或已移动：{}",
            path.display()
        ));
    }
    Ok(path)
}

/// 用系统文件管理器打开目录（Windows=explorer / macOS=open / Linux=
/// xdg-open）。spawn 后不等待——文件管理器的生命周期不归网关管；参数经
/// `Command::arg` 直传不经 shell，路径含空格/特殊字符安全。
fn open_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    let spawn = || -> std::io::Result<std::process::Child> {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW：explorer 是 GUI 程序本无控制台，旗标只防
            // 意外父控台窗口（同 Windows 后台进程纪律：绝不弹新窗口）。
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            std::process::Command::new("explorer")
                .arg(path)
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open").arg(path).spawn()
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::process::Command::new("xdg-open").arg(path).spawn()
        }
    };
    spawn()
        .map(|_| ())
        .map_err(|e| format!("打开目录失败: {e}"))
}

pub struct ProjectsHandler;

#[async_trait::async_trait]
impl ModuleHandler for ProjectsHandler {
    fn module_name(&self) -> &str {
        "projects"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list", "create", "remove", "rename", "open_dir"]
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
                let projects = bridge.list();
                let count = projects.len();
                Ok(Some(serde_json::json!({ "projects": projects, "count": count })))
            }
            "create" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                let path = crate::handlers::get_str(&data, "path")?;
                let info = bridge.create(&name, &path)?;
                Ok(Some(serde_json::json!({ "project": info })))
            }
            "remove" => {
                let data = data.ok_or("missing data")?;
                let project_id = crate::handlers::get_str(&data, "project_id")?;
                let info = bridge.remove(&project_id)?;
                // 文案与设计档 §3.4 对齐：移除只解除分组，不删任何文件。
                Ok(Some(serde_json::json!({
                    "removed": info,
                    "note": "仅解除分组，未删除会话与项目目录内的任何文件",
                })))
            }
            "rename" => {
                let data = data.ok_or("missing data")?;
                let project_id = crate::handlers::get_str(&data, "project_id")?;
                let name = crate::handlers::get_str(&data, "name")?;
                let info = bridge.rename(&project_id, &name)?;
                Ok(Some(serde_json::json!({ "project": info })))
            }
            "open_dir" => {
                let data = data.ok_or("missing data")?;
                let project_id = crate::handlers::get_str(&data, "project_id")?;
                let path = resolve_open_target(bridge.as_ref(), &project_id)?;
                open_in_file_manager(&path)?;
                Ok(Some(serde_json::json!({ "opened": path.to_string_lossy() })))
            }
            _ => Err(format!("unknown projects cmd: {cmd}")),
        }
    }
}
