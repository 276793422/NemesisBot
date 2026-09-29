//! Security types: OperationType, DangerLevel, etc.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Operation type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OperationType {
    // File operations
    FileRead,
    FileWrite,
    FileDelete,
    // Directory operations
    DirRead,
    DirCreate,
    DirDelete,
    // Process operations
    ProcessExec,
    ProcessSpawn,
    ProcessKill,
    ProcessSuspend,
    // Network operations
    NetworkDownload,
    NetworkUpload,
    NetworkRequest,
    // Hardware operations
    HardwareI2C,
    HardwareSPI,
    HardwareGPIO,
    // System operations
    SystemShutdown,
    SystemReboot,
    SystemConfig,
    SystemService,
    SystemInstall,
    // Registry operations
    RegistryRead,
    RegistryWrite,
    RegistryDelete,
    // 进程内编排/查询/交互（无外部副作用或副作用经自闸二次过滤）：
    // 定时器、会话内问答卡、静态表查询、子代理/工作流触发（其内部工具
    // 调用各自过闸）。W5 盲点修复配套：给「纯内置编排面」一个诚实的
    // LOW 档家，避免它们误挂 ProcessExec/Network* 名实不符的档位。
    Internal,
}

impl fmt::Display for OperationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::FileRead => "file_read",
            Self::FileWrite => "file_write",
            Self::FileDelete => "file_delete",
            Self::DirRead => "dir_read",
            Self::DirCreate => "dir_create",
            Self::DirDelete => "dir_delete",
            Self::ProcessExec => "process_exec",
            Self::ProcessSpawn => "process_spawn",
            Self::ProcessKill => "process_kill",
            Self::ProcessSuspend => "process_suspend",
            Self::NetworkDownload => "network_download",
            Self::NetworkUpload => "network_upload",
            Self::NetworkRequest => "network_request",
            Self::HardwareI2C => "hardware_i2c",
            Self::HardwareSPI => "hardware_spi",
            Self::HardwareGPIO => "hardware_gpio",
            Self::SystemShutdown => "system_shutdown",
            Self::SystemReboot => "system_reboot",
            Self::SystemConfig => "system_config",
            Self::SystemService => "system_service",
            Self::SystemInstall => "system_install",
            Self::RegistryRead => "registry_read",
            Self::RegistryWrite => "registry_write",
            Self::RegistryDelete => "registry_delete",
            Self::Internal => "internal",
        };
        write!(f, "{}", s)
    }
}

/// Danger level for operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DangerLevel {
    Low = 0,
    Medium = 1,
    High = 2,
    Critical = 3,
}

impl fmt::Display for DangerLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Low => write!(f, "LOW"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::High => write!(f, "HIGH"),
            Self::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// Get danger level for an operation type.
pub fn get_danger_level(op: OperationType) -> DangerLevel {
    match op {
        OperationType::FileRead | OperationType::DirRead | OperationType::Internal => {
            DangerLevel::Low
        }
        OperationType::NetworkDownload | OperationType::NetworkRequest => DangerLevel::Medium,
        OperationType::FileWrite
        | OperationType::FileDelete
        | OperationType::DirCreate
        | OperationType::DirDelete
        | OperationType::ProcessSpawn => DangerLevel::High,
        OperationType::ProcessExec
        | OperationType::ProcessKill
        | OperationType::SystemShutdown
        | OperationType::SystemReboot
        | OperationType::SystemConfig
        | OperationType::SystemService
        | OperationType::SystemInstall
        | OperationType::RegistryWrite
        | OperationType::RegistryDelete => DangerLevel::Critical,
        _ => DangerLevel::Medium,
    }
}

/// Security rule for ABAC evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityRule {
    pub pattern: String,
    pub action: String,
    #[serde(default)]
    pub comment: String,
}

/// Tool invocation for security checks.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub tool_name: String,
    pub args: serde_json::Value,
    pub user: String,
    pub source: String,
    pub metadata: std::collections::HashMap<String, String>,
}

/// F5 (devtool-upgrade 阶段 2): structured deny feedback from the 8-layer
/// pipeline. Replaces the free-text `Option<String>` so consumers (agent loop
/// replay to the model, image gate, tests) can present layer / policy /
/// summary / suggestion separately.
/// `summary` 保留各层原文；`policy` 与审计 JSONL 的 policy 列同标识
/// （injection_detector / command_guard / abac / …），两处可对账。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenyInfo {
    /// Which layer denied: injection / command / abac / credential / dlp /
    /// ssrf / virus.
    pub layer: &'static str,
    /// The policy/component that fired (same identifier as the audit
    /// JSONL policy column).
    pub policy: String,
    /// Original free-text reason (unchanged from the pre-F5 message).
    pub summary: String,
    /// Fixed per-layer remediation hint for the model.
    pub suggestion: Option<String>,
}

/// Security decision result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityDecision {
    Allowed,
    Denied,
    RequireApproval,
}

impl fmt::Display for SecurityDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allowed => write!(f, "allowed"),
            Self::Denied => write!(f, "denied"),
            Self::RequireApproval => write!(f, "require_approval"),
        }
    }
}

/// Fine-grained permission configuration controlling allowed operation types,
/// target patterns, and approval requirements.
#[derive(Debug, Clone)]
pub struct Permission {
    /// Allowed operation types (true = allowed).
    pub allowed_types: HashMap<OperationType, bool>,
    /// Target patterns that are explicitly allowed.
    pub allowed_targets: Vec<String>,
    /// Target patterns that are explicitly denied.
    pub denied_targets: Vec<String>,
    /// Operation types that require human approval.
    pub require_approval: HashMap<OperationType, bool>,
    /// Maximum danger level that is permitted.
    pub max_danger_level: DangerLevel,
}

impl Permission {
    /// Create a new default permission with everything denied.
    pub fn new() -> Self {
        Self {
            allowed_types: HashMap::new(),
            allowed_targets: Vec::new(),
            denied_targets: Vec::new(),
            require_approval: HashMap::new(),
            max_danger_level: DangerLevel::Low,
        }
    }

    /// Check whether a specific operation type is allowed.
    pub fn is_operation_allowed(&self, op_type: &OperationType) -> bool {
        self.allowed_types.get(op_type).copied().unwrap_or(false)
    }

    /// Check whether a specific operation type requires approval.
    pub fn requires_approval(&self, op_type: &OperationType) -> bool {
        self.require_approval.get(op_type).copied().unwrap_or(false)
    }

    /// Check whether a target string matches any denied pattern.
    pub fn is_target_denied(&self, target: &str) -> bool {
        self.denied_targets
            .iter()
            .any(|pattern| target.contains(pattern) || matches_pattern(target, pattern))
    }

    /// Check whether a target string matches any allowed pattern.
    pub fn is_target_allowed(&self, target: &str) -> bool {
        self.allowed_targets
            .iter()
            .any(|pattern| target.contains(pattern) || matches_pattern(target, pattern))
    }
}

impl Default for Permission {
    fn default() -> Self {
        Self::new()
    }
}

/// A single policy rule with attribute-based matching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Rule name for identification.
    pub name: String,
    /// Match by operation type (None = match all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_op_type: Option<OperationType>,
    /// Match by target pattern (None = match all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_target: Option<String>,
    /// Match by user (None = match all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_user: Option<String>,
    /// Match by source (None = match all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_source: Option<String>,
    /// Minimum danger level to match (None = match all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_danger: Option<DangerLevel>,
    /// Action to take when matched: "allow", "deny", "ask".
    pub action: String,
    /// Human-readable reason for the action.
    #[serde(default)]
    pub reason: String,
}

/// A security policy containing a set of rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Policy name.
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Whether this policy is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Ordered list of rules to evaluate.
    pub rules: Vec<PolicyRule>,
    /// Default action when no rule matches: "allow", "deny", "ask".
    #[serde(default = "default_action_deny")]
    pub default_action: String,
    /// If true, only log violations without blocking.
    #[serde(default)]
    pub log_only: bool,
    /// Whether multi-factor authentication is required.
    #[serde(default)]
    pub require_mfa: bool,
}

fn default_true() -> bool {
    true
}

fn default_action_deny() -> String {
    "deny".to_string()
}

/// Simple glob-style pattern match for permission target checks.
/// Supports `*` as a wildcard that matches any sequence of characters.
pub fn matches_pattern(target: &str, pattern: &str) -> bool {
    // Use the crate-level matcher for wildcard patterns
    crate::matcher::match_pattern(pattern, target)
}

// ---------------------------------------------------------------------------
// 进程级工具操作类型声明表（W5 盲点修复配套）
// ---------------------------------------------------------------------------
// 动态注册面（MCP 桥 / WASM 插件桥）在工具注册期声明操作类型；管线查表
// lookup-first（声明优先于内置表）。修复前的盲点：内置表 `_ => None` +
// 管线 `None => allow`（fail-open），任何不在表内的工具名整体跳过 8 层
// （连注入检测都不跑）。修复后：动态工具注册即声明，未知名在管线层按
// CRITICAL fail-closed（见 pipeline::effective_tool_operation）。

use std::sync::OnceLock;
use std::sync::RwLock;

static DECLARED_TOOL_OPERATIONS: OnceLock<RwLock<HashMap<String, OperationType>>> = OnceLock::new();

fn declared_operations() -> &'static RwLock<HashMap<String, OperationType>> {
    DECLARED_TOOL_OPERATIONS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 声明（或覆盖）一个工具名的操作类型（动态注册面在注册期调用；
/// 升级/重注册语义 = 覆盖）。
pub fn declare_tool_operation(tool_name: &str, op: OperationType) {
    let mut map = declared_operations()
        .write()
        .unwrap_or_else(|p| p.into_inner());
    map.insert(tool_name.to_string(), op);
}

/// 撤销声明（工具注销时调用；未声明过 = no-op）。
pub fn undeclare_tool_operation(tool_name: &str) {
    let mut map = declared_operations()
        .write()
        .unwrap_or_else(|p| p.into_inner());
    map.remove(tool_name);
}

/// 按前缀批量撤销（MCP server 重载 / 插件卸载场景）。
pub fn undeclare_tool_operations_with_prefix(prefix: &str) {
    let mut map = declared_operations()
        .write()
        .unwrap_or_else(|p| p.into_inner());
    map.retain(|name, _| !name.starts_with(prefix));
}

/// 读声明（管线内部用；外部走 [`tool_to_operation`]）。
fn declared_tool_operation(tool_name: &str) -> Option<OperationType> {
    declared_operations()
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(tool_name)
        .copied()
}

/// Map tool name to operation type.
///
/// 查表顺序：进程级声明（动态注册面）→ 内置表。`None` 的消费方（管线）
/// 按 fail-closed 处理；本函数保持 `Option` 返回以兼容既有调用点
/// （`is_critical_tool` / `tool_danger_level` 等各自决定未知名姿态）。
pub fn tool_to_operation(tool_name: &str) -> Option<OperationType> {
    if let Some(op) = declared_tool_operation(tool_name) {
        return Some(op);
    }
    match tool_name {
        "read_file" | "file_exists" => Some(OperationType::FileRead),
        "write_file" | "edit_file" | "append_file" | "multiedit" => {
            Some(OperationType::FileWrite)
        }
        "delete_file" => Some(OperationType::FileDelete),
        "list_directory" | "list_dir" => Some(OperationType::DirRead),
        "create_directory" | "create_dir" => Some(OperationType::DirCreate),
        "delete_directory" | "delete_dir" => Some(OperationType::DirDelete),
        // B4（2026-09-05）：background_start 语义等同 exec（起进程）→ 同档
        // ProcessExec，命令本体照常过 8 层管线。background_output /
        // background_kill 只操作本注册表内的自有任务（命令已在 start 时过
        // 闸，读自有缓冲/杀自属子进程不构成新攻击面）——W5 盲点修复前走
        // 「未知名放行」分支，修复后未知名 = CRITICAL fail-closed，这里
        // 显式映射 Internal（LOW）保留原例外语义，不靠未知名兜底。
        "exec" | "execute_command" | "shell" | "exec_async" | "background_start" | "cron"
        | "run_script"
        // C8：构建/测试 runner（MOVE_TOOLS 成员，写 target/；plan 模式
        // 同源派生拦截）。
        | "run_checks"
        // U13：外部 CLI 委派（claude/codex 子进程跑任务）。
        | "claude_code" | "codex_delegate" => Some(OperationType::ProcessExec),
        // F3（2026-09-22 审计修复）：`git`/`grep` 同为 MOVE_TOOLS 但此前未
        // 分类 → 管线 None 放行（fail-open），U10 注释承诺的 declared_
        // operation_type 机制未落地，这里直接补表。FileRead=LOW 让 8 层
        // 全部生效（注入检测/凭据扫描/DLP/审计链）；git 的写子命令由工具
        // 白名单收口（只暴露 add/commit 等 D1 安全写），危险写走 exec。
        "git" | "grep" => Some(OperationType::FileRead),
        "spawn" => Some(OperationType::ProcessSpawn),
        // 桌面自动化：控制其他应用窗口/键鼠（效果面 ≈ 操纵 GUI 程序）→
        // ProcessSpawn（HIGH）。不上 CRITICAL：日常自动化工具，CRITICAL
        // 会挂 guardian LLM 二审常开。
        "desktop" => Some(OperationType::ProcessSpawn),
        "kill" | "kill_process" => Some(OperationType::ProcessKill),
        "download" | "install_skill" => Some(OperationType::NetworkDownload),
        "upload" => Some(OperationType::NetworkUpload),
        "http_request" | "web_request" | "web_fetch" | "web_search" | "cluster_rpc"
        | "find_skills" => Some(OperationType::NetworkRequest),
        // 浏览器自动化 = 网络面向能力；MCP 发现会拉起配置的 MCP server
        // 进程（stdio）/HTTP 连接——工具本身只列举，不给 exec 档。
        "browser" | "mcp_discover" => Some(OperationType::NetworkRequest),
        "screen_capture" => Some(OperationType::FileWrite),
        // H1（2026-09-05）：todowrite 本质是 workspace 内写文件（sessions/
        // todo_*.json），归 FileWrite 走同类审查（空 target 不匹配任何
        // ABAC 规则 → default action 兜底，默认配置放行）。
        "todowrite" => Some(OperationType::FileWrite),
        // 对话生成（2026-09-22）：workflow_create 落草稿 YAML 到 workspace
        // workflow/drafts/（写文件语义）。
        "workflow_create" => Some(OperationType::FileWrite),
        // 图像生成（集群专业职能框架 M4）：出站请求发往装配期解析的固定
        // 端点（prompt 为载荷）+ 结果落工作区 images/（工具内构造性钉死，
        // 拒 `..`/绝对路径）。归 NetworkRequest（MEDIUM）让注入检测/凭据
        // 扫描/DLP/审计链全跑；不做 executor 隔离（不出 MOVE_TOOLS——
        // 沙盒断网反而打不通端点）。
        "generate_image" => Some(OperationType::NetworkRequest),
        // 只读语义代码查询（L1/U19）。
        "lsp" => Some(OperationType::FileRead),
        // 硬件具名档（此前未映射 → 旧未知名放行；补齐后走具名类型）。
        "i2c" => Some(OperationType::HardwareI2C),
        "spi" => Some(OperationType::HardwareSPI),
        // 看板数据面（workspace 内 SQLite 建单/流转/评论 + 拆解派发）。
        "board_issue" => Some(OperationType::FileWrite),
        // 技能文件写（agent 自写 procedural memory）。
        "skill_manage" => Some(OperationType::FileWrite),
        "complete_bootstrap" => Some(OperationType::FileDelete),
        // 增强记忆：store 落记忆文件/向量库（写），forget 删条目（删），
        // search/list 纯查询。
        "memory_store" => Some(OperationType::FileWrite),
        "memory_forget" => Some(OperationType::FileDelete),
        // 进程内编排/查询/交互面（LOW）：纯会话机制（发消息/睡眠/问答卡/
        // 子代理编排——其内部工具调用各自过闸）、静态表查询、B4 自有任务
        // 读写、工作流触发（节点各自过闸）。这些工具没有外部副作用面，
        // 挂 exec/network 档名实不符；Internal = LOW 且全 8 层照跑
        // （注入检测/凭据扫描/DLP/审计链不再像修复前那样整体跳过）。
        "message" | "sleep" | "question" | "subagent" | "skills_list" | "skills_info"
        | "cli_reference" | "history_search" | "mcp_list" | "workflow_run"
        | "workflow_capabilities" | "memory_search" | "memory_list" | "background_output"
        | "background_kill" => Some(OperationType::Internal),
        _ => None,
    }
}

/// Extract target from tool arguments.
pub fn extract_target(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "read_file" | "write_file" | "edit_file" | "append_file" | "delete_file" => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "list_directory" | "list_dir" | "create_directory" | "create_dir" | "delete_directory"
        | "delete_dir" => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "exec" | "execute_command" | "spawn" | "shell" | "exec_async" | "background_start" => args
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "download" | "upload" | "http_request" | "web_request" | "web_fetch" | "web_search"
        | "find_skills" => args
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "cluster_rpc" => args
            .get("peer_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "cron" => args
            .get("command")
            .or_else(|| args.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "screen_capture" => args
            .get("save_path")
            .or_else(|| args.get("path"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        // 图像生成（M4）：审计 target = 输出相对路径（prompt 是自由文本，
        // 注入检测/凭据扫描层已全文过筛，不重复进 target 字段）。
        "generate_image" => args
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "install_skill" => args
            .get("url")
            .or_else(|| args.get("source"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// Extract URL from tool arguments.
pub fn extract_url(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "download" | "upload" | "http_request" | "web_request" | "web_fetch" | "web_search"
        | "install_skill" | "find_skills" => args
            .get("url")
            .or_else(|| args.get("source"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        // cluster_rpc's peer_id is resolved by the internal cluster peer registry,
        // not through DNS. SSRF protection does not apply to internal cluster routing.
        "cluster_rpc" => String::new(),
        _ => String::new(),
    }
}

/// Check if a command is safe.
pub fn is_safe_command(command: &str) -> (bool, String) {
    use std::sync::OnceLock;
    static DANGEROUS: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    let patterns = DANGEROUS.get_or_init(|| {
        let raw = [
            r"(?i)\brm\s+-[rf]{1,2}\b",
            r"(?i)\bdel\s+/[fq]\b",
            r"(?i)\b(format|mkfs)\b",
            r"(?i)\bdd\s+if=",
            r"(?i)\b(shutdown|reboot|poweroff)\b",
            r"(?i)\bsudo\b",
            r"(?i)\bchmod\s+[0-7]{3,4}\b",
            r"(?i)\bchown\b",
        ];
        raw.iter()
            .filter_map(|p| regex::Regex::new(p).ok())
            .collect()
    });

    for re in patterns {
        if re.is_match(command) {
            return (false, "command contains dangerous pattern".to_string());
        }
    }
    (true, String::new())
}

/// Validate path is within workspace and safe.
pub fn validate_path(path: &str, workspace: &str) -> Result<String, String> {
    use nemesis_path::paths::canonicalize_for_compare;
    // 2026-09-01 8.3 短名统一修复（与 auditor::validate_path_internal 同款）：
    // 裸 canonicalize + 词法回退在「workspace 已存在（canonicalize 成长名）
    // 而 path 尚不存在（create 前守卫常态，回退保留 RUNNER~1 短名）」时前缀
    // 比较恒 false → 根内写入全被误拒。canonicalize_for_compare 借最长存在
    // 祖先对齐双方表示后再比。
    let abs_path = canonicalize_for_compare(std::path::Path::new(path));

    if !workspace.is_empty() {
        let abs_workspace = canonicalize_for_compare(std::path::Path::new(workspace));

        match abs_path.strip_prefix(&abs_workspace) {
            Ok(rel) => {
                if rel.starts_with("..") {
                    return Err("access denied: path outside workspace".to_string());
                }
            }
            Err(_) => {
                // If strip_prefix fails, check if the path starts with workspace
                if !abs_path.starts_with(&abs_workspace) {
                    return Err("access denied: path outside workspace".to_string());
                }
            }
        }
    }

    // Check dangerous system paths —— 对**原始输入**与规范化结果都查：
    // 规范化会把 POSIX 风格输入经根祖先拼成 Windows 盘符路径（/etc/passwd →
    // C:\etc\passwd），只查规范化结果会漏掉字面前缀命中（2026-09-01）。
    let dangerous = [
        "/etc/passwd",
        "/etc/shadow",
        "/etc/sudoers",
        "C:\\Windows\\System32\\drivers\\etc\\hosts",
    ];
    for candidate in [path, abs_path.to_string_lossy().as_ref()] {
        for d in &dangerous {
            if candidate.starts_with(d) {
                return Err("access denied: protected system path".to_string());
            }
        }
    }

    Ok(abs_path.to_string_lossy().to_string())
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
