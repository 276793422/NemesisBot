//! Unified path management.

use parking_lot::RwLock;
use std::path::{Path, PathBuf};

/// Environment variable names.
pub const ENV_HOME: &str = "NEMESISBOT_HOME";
pub const ENV_CONFIG: &str = "NEMESISBOT_CONFIG";
pub const ENV_MCP_CONFIG: &str = "NEMESISBOT_MCP_CONFIG";
pub const ENV_SECURITY_CONFIG: &str = "NEMESISBOT_SECURITY_CONFIG";
pub const ENV_SKILLS_CONFIG: &str = "NEMESISBOT_SKILLS_CONFIG";
pub const ENV_SCANNER_CONFIG: &str = "NEMESISBOT_SCANNER_CONFIG";
pub const DEFAULT_HOME_DIR: &str = ".nemesisbot";

/// Global local mode flag.
pub static mut LOCAL_MODE: bool = false;

/// Singleton state for `default_path_manager()`.
static DEFAULT_MANAGER: std::sync::OnceLock<PathManager> = std::sync::OnceLock::new();

/// Path manager for NemesisBot directories and files.
pub struct PathManager {
    home_dir: RwLock<PathBuf>,
    /// Override for config path (set via setter or env var).
    config_path: RwLock<Option<PathBuf>>,
    /// Override for MCP config path.
    mcp_config_path: RwLock<Option<PathBuf>>,
    /// Override for security config path.
    security_config_path: RwLock<Option<PathBuf>>,
    /// Override for skills config path.
    skills_config_path: RwLock<Option<PathBuf>>,
}

impl PathManager {
    /// Create a new path manager.
    pub fn new() -> Self {
        let home_dir = resolve_home_dir().unwrap_or_else(fallback_home_dir);
        Self {
            home_dir: RwLock::new(home_dir),
            config_path: RwLock::new(None),
            mcp_config_path: RwLock::new(None),
            security_config_path: RwLock::new(None),
            skills_config_path: RwLock::new(None),
        }
    }

    /// Create with a specific home directory.
    pub fn with_home(home_dir: PathBuf) -> Self {
        Self {
            home_dir: RwLock::new(home_dir),
            config_path: RwLock::new(None),
            mcp_config_path: RwLock::new(None),
            security_config_path: RwLock::new(None),
            skills_config_path: RwLock::new(None),
        }
    }

    /// Get the home directory.
    pub fn home_dir(&self) -> PathBuf {
        self.home_dir.read().clone()
    }

    /// Set a custom home directory (for testing or special cases).
    ///
    /// 仅影响本实例的后续路径解析；对 `default_path_manager()` 单例调用时
    /// 进程全局生效——测试用它把单例 home 重定向到临时目录（OnceLock 首次
    /// 初始化后无法换实例，setter 是唯一的运行时重定向缝）。改完记得恢复。
    pub fn set_home_dir(&self, home_dir: PathBuf) {
        *self.home_dir.write() = home_dir;
    }

    /// Get the config file path.
    /// Priority: setter override > NEMESISBOT_CONFIG env > default (home/config.json).
    pub fn config_path(&self) -> PathBuf {
        if let Some(ref p) = *self.config_path.read() {
            return p.clone();
        }
        if let Ok(env_path) = std::env::var(ENV_CONFIG) {
            return PathBuf::from(env_path);
        }
        self.home_dir.read().join("config.json")
    }

    /// Set a custom config path (for testing or special cases).
    pub fn set_config_path(&self, path: PathBuf) {
        *self.config_path.write() = Some(path);
    }

    /// Get the workspace directory.
    pub fn workspace(&self) -> PathBuf {
        self.home_dir.read().join("workspace")
    }

    /// Get the MCP config path.
    /// Priority: setter override > NEMESISBOT_MCP_CONFIG env > default.
    pub fn mcp_config_path(&self) -> PathBuf {
        if let Some(ref p) = *self.mcp_config_path.read() {
            return p.clone();
        }
        if let Ok(env_path) = std::env::var(ENV_MCP_CONFIG) {
            return PathBuf::from(env_path);
        }
        self.workspace().join("config").join("config.mcp.json")
    }

    /// Set a custom MCP config path.
    pub fn set_mcp_config_path(&self, path: PathBuf) {
        *self.mcp_config_path.write() = Some(path);
    }

    /// Get the security config path.
    /// Priority: setter override > NEMESISBOT_SECURITY_CONFIG env > default.
    pub fn security_config_path(&self) -> PathBuf {
        if let Some(ref p) = *self.security_config_path.read() {
            return p.clone();
        }
        if let Ok(env_path) = std::env::var(ENV_SECURITY_CONFIG) {
            return PathBuf::from(env_path);
        }
        self.workspace().join("config").join("config.security.json")
    }

    /// Set a custom security config path.
    pub fn set_security_config_path(&self, path: PathBuf) {
        *self.security_config_path.write() = Some(path);
    }

    /// Get the skills config path.
    /// Priority: setter override > NEMESISBOT_SKILLS_CONFIG env > default.
    pub fn skills_config_path(&self) -> PathBuf {
        if let Some(ref p) = *self.skills_config_path.read() {
            return p.clone();
        }
        if let Ok(env_path) = std::env::var(ENV_SKILLS_CONFIG) {
            return PathBuf::from(env_path);
        }
        self.workspace().join("config").join("config.skills.json")
    }

    /// Set a custom skills config path.
    pub fn set_skills_config_path(&self, path: PathBuf) {
        *self.skills_config_path.write() = Some(path);
    }

    /// Get the auth storage path.
    pub fn auth_path(&self) -> PathBuf {
        self.workspace().join("config").join("auth.json")
    }

    /// Get the audit log directory.
    pub fn audit_log_dir(&self) -> PathBuf {
        resolve_audit_log_dir_in_workspace(&self.workspace())
    }

    /// Get the sessions log directory.
    /// Chat history JSONL files are stored here, separate from session files
    /// used for LLM context recovery.
    pub fn sessions_log_dir(&self) -> PathBuf {
        resolve_session_logs_dir_in_workspace(&self.workspace())
    }

    /// Get the boundary-events sidecar directory (round-5 review fix).
    /// Turn/step boundary audit events live in `<session>.jsonl` files HERE,
    /// deliberately OUTSIDE `session_logs/` — scan_session_logs admits every
    /// `*.jsonl` in that dir, so a sidecar placed there would surface as a
    /// phantom session in the Dashboard session list.
    pub fn boundary_events_dir(&self) -> PathBuf {
        resolve_boundary_events_dir_in_workspace(&self.workspace())
    }

    /// Get the temp directory.
    pub fn temp_dir(&self) -> PathBuf {
        self.home_dir.read().join("workspace").join("temp")
    }

    /// Get the memory vector directory for enhanced memory storage.
    pub fn memory_vector_dir(&self) -> PathBuf {
        self.workspace().join("memory_vector")
    }

    /// Get agent-specific workspace.
    pub fn agent_workspace(&self, agent_id: &str) -> PathBuf {
        if agent_id.is_empty() || agent_id == "main" || agent_id == "default" {
            self.workspace()
        } else {
            self.home_dir.read().join(format!("workspace-{}", agent_id))
        }
    }
}

impl Default for PathManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Return the default singleton `PathManager`.
///
/// Mirrors the Go `DefaultPathManager()` function. Thread-safe initialization
/// via `OnceLock`.
pub fn default_path_manager() -> &'static PathManager {
    DEFAULT_MANAGER.get_or_init(PathManager::new)
}

/// Resolve the NemesisBot home directory.
///
/// Priority:
/// 1. LocalMode → `{cwd}/.nemesisbot`
/// 2. `NEMESISBOT_HOME` env → `{NEMESISBOT_HOME}/.nemesisbot`
/// 3. Auto-detect cwd → if `{cwd}/.nemesisbot` exists
/// 4. Exe directory → if `{exe_dir}/.nemesisbot` exists
/// 5. Default → `~/.nemesisbot`
pub fn resolve_home_dir() -> Result<PathBuf, String> {
    // Priority 1: LocalMode
    let local_mode = unsafe { LOCAL_MODE };
    if local_mode {
        let cwd = std::env::current_dir().map_err(|e| format!("cwd: {}", e))?;
        return Ok(cwd.join(DEFAULT_HOME_DIR));
    }

    // Priority 2: NEMESISBOT_HOME env var
    if let Ok(env_home) = std::env::var(ENV_HOME) {
        let expanded = expand_home(&env_home);
        return Ok(expanded.join(DEFAULT_HOME_DIR));
    }

    // Priority 3: Exe directory
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if exe_dir.join(DEFAULT_HOME_DIR).is_dir() {
                return Ok(exe_dir.join(DEFAULT_HOME_DIR));
            }
        }
    }

    // Priority 4: Auto-detect cwd
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {}", e))?;
    if cwd.join(DEFAULT_HOME_DIR).is_dir() {
        return Ok(cwd.join(DEFAULT_HOME_DIR));
    }

    // Priority 5: Default ~/.nemesisbot
    let home = dirs::home_dir().ok_or("cannot determine home directory")?;
    Ok(home.join(DEFAULT_HOME_DIR))
}

/// Expand ~ to home directory.
pub fn expand_home(path: &str) -> PathBuf {
    if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            if path.len() > 1 {
                return home.join(&path[2..]);
            }
            return home;
        }
    }
    PathBuf::from(path)
}

/// Check if local mode should be auto-detected (if .nemesisbot exists in cwd).
pub fn detect_local() -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join(DEFAULT_HOME_DIR).is_dir()
}

/// Set the local mode flag.
pub fn set_local_mode(enabled: bool) {
    unsafe {
        LOCAL_MODE = enabled;
    }
}

/// Check if local mode is enabled.
pub fn is_local_mode() -> bool {
    unsafe { LOCAL_MODE }
}

/// `<workspace>/config` —— 子系统配置区（config.*.json；用户 2026-08-28
/// 布局指令：子系统配置归 workspace/config，禁止丢 home 根）。
pub fn workspace_config_dir(workspace: &Path) -> PathBuf {
    workspace.join("config")
}

/// `<workspace>/config.json`（workspace 布局家族拼接点）。
///
/// ⚠️ 注意：主配置 config.json 的现行布局在 **home 根**（`<home>/config.json`，
/// 读方 `nemesis-config::load_config` / 写方 `common::config_path`，收敛见
/// path-single-source 报告债务 #1）。本函数当前**无生产调用方**，勿按函数名
/// 误当主配置路径使用。
pub fn resolve_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("config.json")
}

/// Resolve MCP config path within a specific workspace.
pub fn resolve_mcp_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("config.mcp.json")
}

/// `<workspace>/config/cors.json` —— CORS 配置（2026-08-29 收编：原游离在
/// `<home>/config/cors.json`，同 hooks.json 为路径大迁移漏网项）。
pub fn resolve_cors_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("cors.json")
}

/// `<workspace>/logs/gateway/` —— 网关进程每日滚动日志目录（2026-08-30 统一
/// 收编：`nemesisbot.YYYY-MM-DD` 产物原先散在 `logs/` 根，与 security_logs/
/// cluster_logs/request_logs 等子目录混放）。
pub fn resolve_gateway_logs_dir_in_workspace(workspace: &Path) -> PathBuf {
    logs_dir_in_workspace(workspace).join("gateway")
}

/// `<workspace>/logs/checkpoints/` —— 编辑安全网 turn 快照（2026-08-30 统一
/// 收编：原 `.checkpoints/` 散放 workspace 根，记录型数据归 logs 家族）。
pub fn resolve_checkpoints_dir_in_workspace(workspace: &Path) -> PathBuf {
    logs_dir_in_workspace(workspace).join("checkpoints")
}

/// `<workspace>/config/hooks.json` —— CC 方言钩子配置（2026-08-29 收编：
/// 原先游离在 `<home>/config/hooks.json`，为 08-28 路径大迁移的漏网项；
/// 读取方经 `migrate_legacy_home_hooks_config` 一次性迁移）。
pub fn resolve_hooks_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("hooks.json")
}

/// `<workspace>/config/config.commands.json` —— 自定义 slash 命令表
/// （快捷提示词发送器；AgentLoop 改写与 Dashboard CommandsView 同源）。
pub fn resolve_commands_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("config.commands.json")
}

/// Resolve security config path within a specific workspace.
pub fn resolve_security_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("config.security.json")
}

/// Resolve cluster config path within a specific workspace.
pub fn resolve_cluster_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("config.cluster.json")
}

/// Resolve skills config path within a specific workspace.
pub fn resolve_skills_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("config.skills.json")
}

/// Resolve scanner config path within a specific workspace.
pub fn resolve_scanner_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("config.scanner.json")
}

/// Resolve chat config path within a specific workspace.
pub fn resolve_chat_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("config.chat.json")
}

// =======================================================================
// workspace/data 派生数据区（2026-08-28 布局确立）
// =======================================================================

/// `<home>/workspace/data` —— 运行时派生数据统一目录（nemesisbot_data.db、
/// models_catalog.json 目录缓存等）。布局唯一拼接点：改 data 区位置只改这里，
/// 所有消费方经本模块取路径，禁止各自 join。
pub fn workspace_data_dir(home_dir: &Path) -> PathBuf {
    home_dir.join("workspace").join("data")
}

/// models.dev 目录缓存文件：`<home>/workspace/data/models_catalog.json`。
/// 读写方：CLI `model catalog-update` / `model add`（nemesisbot catalog.rs）
/// + Dashboard models 页（nemesis-web models.rs）——两侧都从本函数取路径。
pub fn models_catalog_cache_path(home_dir: &Path) -> PathBuf {
    workspace_data_dir(home_dir).join("models_catalog.json")
}

/// 旧版缓存位置（2026-08-28 前直接丢 home 根；保留仅供读时迁移与测试播种）。
pub fn legacy_models_catalog_cache_path(home_dir: &Path) -> PathBuf {
    home_dir.join("models_catalog.json")
}

/// 读时静默迁移：把旧 home 根缓存 rename 进 workspace/data，存量部署免手动
/// 迁移、不重新下载。best-effort——失败不阻塞读路径（下次 save_cache 重建）。
pub fn migrate_legacy_models_catalog_cache(home_dir: &Path) {
    let new_path = models_catalog_cache_path(home_dir);
    let legacy = legacy_models_catalog_cache_path(home_dir);
    if !legacy.exists() || new_path.exists() {
        return;
    }
    if let Some(parent) = new_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::rename(&legacy, &new_path);
}

// =======================================================================
// workspace 布局区（2026-08-28 路径唯一真相源扩展）
// =======================================================================
//
// 背景：全仓审计发现 peers.toml / state.toml / rpc_cache / logs/* /
// sessions / state 等路径在 3+ crate 里各自 join（跨 crate 读/写对曾真实
// 分叉过，2026-08-24 L2 抓到 catalog 读写路径分叉）。本区把 workspace
// 布局的每个子目录收敛为唯一拼接点：改布局只改这里，消费方禁止各自拼。

/// `<home>/workspace` —— 工作空间根。
pub fn workspace_dir(home_dir: &Path) -> PathBuf {
    home_dir.join("workspace")
}

// --- cluster 区（peers.toml 读写方：nemesis-cluster / nemesis-web / CLI） ---

/// `<workspace>/cluster` —— 集群身份/拓扑/续行快照根目录。
pub fn cluster_dir_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("cluster")
}

/// `<workspace>/cluster/peers.toml` —— 静态节点身份 + 已知 peers。
pub fn resolve_cluster_peers_path_in_workspace(workspace: &Path) -> PathBuf {
    cluster_dir_in_workspace(workspace).join("peers.toml")
}

/// `<workspace>/cluster/state.toml` —— 运行时发现的远程节点列表。
pub fn resolve_cluster_state_path_in_workspace(workspace: &Path) -> PathBuf {
    cluster_dir_in_workspace(workspace).join("state.toml")
}

/// `<workspace>/cluster/rpc_cache` —— 集群续行快照持久化目录。
pub fn resolve_cluster_rpc_cache_dir_in_workspace(workspace: &Path) -> PathBuf {
    cluster_dir_in_workspace(workspace).join("rpc_cache")
}

// --- 子系统配置区补充（与上方 resolve_*_config_path_in_workspace 同族） ---

/// `<workspace>/config/config.forge.json` —— Forge 自学习配置。
pub fn resolve_forge_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("config.forge.json")
}

/// `<workspace>/config/config.enhanced_memory.json` —— 增强内存配置。
pub fn resolve_enhanced_memory_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace_config_dir(workspace).join("config.enhanced_memory.json")
}

// --- 日志区（写方 nemesis-agent/cluster observer，读方 nemesis-web Logs 页） ---

/// `<workspace>/logs` —— 日志根目录。
pub fn logs_dir_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("logs")
}

/// `<workspace>/logs/request_logs` —— 主 Agent LLM 请求日志（写端 config
/// `logging.llm.request_log_dir` 的默认值；改默认值须同步这里）。
pub fn resolve_request_logs_dir_in_workspace(workspace: &Path) -> PathBuf {
    logs_dir_in_workspace(workspace).join("request_logs")
}

/// `<workspace>/logs/cluster_logs` —— 集群请求日志
/// （`{device_id}/{ts_ms}_{task_id}/` 目录结构）。
pub fn resolve_cluster_logs_dir_in_workspace(workspace: &Path) -> PathBuf {
    logs_dir_in_workspace(workspace).join("cluster_logs")
}

/// `<workspace>/logs/security_logs` —— 安全审计日志（镜像
/// [`PathManager::audit_log_dir`] 的默认布局）。
pub fn resolve_audit_log_dir_in_workspace(workspace: &Path) -> PathBuf {
    logs_dir_in_workspace(workspace).join("security_logs")
}

/// `<workspace>/logs/session_logs` —— 对话历史 JSONL（镜像
/// [`PathManager::sessions_log_dir`]）。
pub fn resolve_session_logs_dir_in_workspace(workspace: &Path) -> PathBuf {
    logs_dir_in_workspace(workspace).join("session_logs")
}

/// `<workspace>/logs/boundary` —— 轮次边界事件 sidecar（镜像
/// [`PathManager::boundary_events_dir`]；必须在 session_logs 之外，否则
/// 扫描会出幻影会话）。
pub fn resolve_boundary_events_dir_in_workspace(workspace: &Path) -> PathBuf {
    logs_dir_in_workspace(workspace).join("boundary")
}

// --- home 根 logs 区（不属于 workspace 的运行日志） ---

/// `<home>/logs` —— home 根日志区（LLM 请求 spill 等超大临时外溢）。
pub fn home_logs_dir(home_dir: &Path) -> PathBuf {
    home_dir.join("logs")
}

/// `<home>/logs/spill` —— 【已废弃位置，2026-08-31 迁回 workspace】LLM 请求
/// 外溢目录的旧位置。U4 设计（docs/PLAN/2026-08-21_dsh-alignment-update-list.md
/// U4 条目）指定 `<workspace>/logs/spill`：spill 定位器文件必须落在
/// `restrict_to_workspace` 限制范围内，agent 的 file 工具才读得到全文；
/// 实现期曾漂移到 home 根，且 2026-08-28 路径收敛只验证了内部一致性、
/// 未对照设计规格，把漂移固化了下来。新代码一律用
/// [`resolve_spill_dir_in_workspace`]；本函数保留供历史 spill 数据迁移
/// （把旧 `<home>/logs/spill` 内容搬进新根）参考。
pub fn resolve_spill_dir_for_home(home_dir: &Path) -> PathBuf {
    home_logs_dir(home_dir).join("spill")
}

/// `<workspace>/logs/spill` —— LLM 请求外溢目录（agent_factory 写 / Logs 页
/// 读）。U4 设计指定位置：处于 restrict_to_workspace 限制范围内，agent 可
/// 回读定位器指向的全文。
pub fn resolve_spill_dir_in_workspace(workspace: &Path) -> PathBuf {
    logs_dir_in_workspace(workspace).join("spill")
}

// --- 状态/会话区 ---

/// `<workspace>/sessions` —— LLM 上下文恢复用 session JSON。
pub fn resolve_sessions_dir_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("sessions")
}

/// `<workspace>/state` —— 运行时状态目录（gateway.json、workspace_state）。
pub fn resolve_state_dir_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("state")
}

/// `<workspace>/state/gateway.json` —— gateway 运行状态
/// （gateway 写 / dashboard、estop CLI 读）。
pub fn resolve_gateway_state_path_in_workspace(workspace: &Path) -> PathBuf {
    resolve_state_dir_in_workspace(workspace).join("gateway.json")
}

// --- 技能区 ---

/// `<workspace>/skills` —— 已安装技能根目录。
pub fn skills_dir_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("skills")
}

/// `<workspace>/skills/.cache` —— 技能搜索磁盘缓存目录。
pub fn resolve_skills_cache_dir_in_workspace(workspace: &Path) -> PathBuf {
    skills_dir_in_workspace(workspace).join(".cache")
}

// =======================================================================
// Top-level resolve functions (match Go's ResolveConfigPath, etc.)
// =======================================================================

/// Minimal config struct for workspace path resolution.
/// Avoids circular dependency by doing a minimal JSON load.
#[derive(serde::Deserialize)]
struct MinimalConfig {
    #[serde(default)]
    agents: MinimalAgents,
}

#[derive(serde::Deserialize, Default)]
struct MinimalAgents {
    #[serde(default)]
    defaults: MinimalDefaults,
}

#[derive(serde::Deserialize, Default)]
struct MinimalDefaults {
    #[serde(default)]
    workspace: String,
}

impl MinimalConfig {
    fn workspace_path(&self) -> Option<PathBuf> {
        let ws = &self.agents.defaults.workspace;
        if ws.is_empty() {
            return None;
        }
        if ws.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return Some(home.join(&ws[2..]));
            }
        }
        Some(PathBuf::from(ws))
    }
}

/// Try to load a minimal config to resolve workspace path.
fn load_config_for_workspace(config_path: &Path) -> Option<MinimalConfig> {
    let data = std::fs::read_to_string(config_path).ok()?;
    serde_json::from_str::<MinimalConfig>(&data).ok()
}

/// Resolve the main configuration file path.
/// Priority: NEMESISBOT_CONFIG env > LocalMode/auto-detect > Default.
pub fn resolve_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var(ENV_CONFIG) {
        return PathBuf::from(env_path);
    }

    let home_dir = resolve_home_dir().unwrap_or_else(fallback_home_dir);

    home_dir.join("config.json")
}

/// Fallback home directory when `resolve_home_dir()` fails.
/// Exposed for testability.
pub(crate) fn fallback_home_dir(_: String) -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(DEFAULT_HOME_DIR))
        .unwrap_or_else(|| PathBuf::from(".nemesisbot"))
}

/// Resolve the MCP configuration file path.
/// Priority: NEMESISBOT_MCP_CONFIG env > workspace/config/config.mcp.json > default.
pub fn resolve_mcp_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var(ENV_MCP_CONFIG) {
        return PathBuf::from(env_path);
    }

    let home_dir = resolve_home_dir().unwrap_or_else(fallback_home_dir);

    let config_path = home_dir.join("config.json");
    if let Some(cfg) = load_config_for_workspace(&config_path) {
        if let Some(workspace) = cfg.workspace_path() {
            return workspace.join("config").join("config.mcp.json");
        }
    }

    home_dir.join("config.mcp.json")
}

/// Resolve the security configuration file path.
/// Priority: NEMESISBOT_SECURITY_CONFIG env > workspace/config/config.security.json > default.
pub fn resolve_security_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var(ENV_SECURITY_CONFIG) {
        return PathBuf::from(env_path);
    }

    let home_dir = resolve_home_dir().unwrap_or_else(fallback_home_dir);

    let config_path = home_dir.join("config.json");
    if let Some(cfg) = load_config_for_workspace(&config_path) {
        if let Some(workspace) = cfg.workspace_path() {
            return workspace.join("config").join("config.security.json");
        }
    }

    home_dir.join("config.security.json")
}

/// Resolve the skills configuration file path.
/// Priority: NEMESISBOT_SKILLS_CONFIG env > workspace/config/config.skills.json > default.
pub fn resolve_skills_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var(ENV_SKILLS_CONFIG) {
        return PathBuf::from(env_path);
    }

    let home_dir = resolve_home_dir().unwrap_or_else(fallback_home_dir);

    let config_path = home_dir.join("config.json");
    if let Some(cfg) = load_config_for_workspace(&config_path) {
        if let Some(workspace) = cfg.workspace_path() {
            return workspace.join("config").join("config.skills.json");
        }
    }

    home_dir.join("config.skills.json")
}

/// Resolve the scanner configuration file path.
/// Priority: NEMESISBOT_SCANNER_CONFIG env > workspace/config/config.scanner.json > default.
pub fn resolve_scanner_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var(ENV_SCANNER_CONFIG) {
        return PathBuf::from(env_path);
    }

    let home_dir = resolve_home_dir().unwrap_or_else(fallback_home_dir);

    let config_path = home_dir.join("config.json");
    if let Some(cfg) = load_config_for_workspace(&config_path) {
        if let Some(workspace) = cfg.workspace_path() {
            return workspace.join("config").join("config.scanner.json");
        }
    }

    home_dir.join("config.scanner.json")
}

#[cfg(test)]
mod tests;
