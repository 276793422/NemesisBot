//! Global constants.

/// Default configuration file name.
pub const CONFIG_FILE: &str = "config.json";

/// Default workspace directory name.
pub const WORKSPACE_DIR: &str = ".nemesisbot";

/// Default identity file.
pub const IDENTITY_FILE: &str = "IDENTITY.md";

/// Default soul file.
pub const SOUL_FILE: &str = "SOUL.md";

/// Default user preferences file.
pub const USER_FILE: &str = "USER.md";

/// RPC correlation ID prefix.
pub const RPC_PREFIX: &str = "[rpc:";

/// Cluster continuation message prefix.
pub const CLUSTER_CONTINUATION_PREFIX: &str = "cluster_continuation:";

/// G4 (devtool-upgrade 阶段 3)：后台 subagent 完成回灌消息前缀。
/// 生成端 = agent_factory 注入的 spawn 闭包（后台任务完成时向 bus 发布），
/// 消费端 = AgentLoop gate_inbound 拦截后走 dispatch_continuation（与集群
/// 续行同构复用 handle_cluster_continuation）。
pub const SUBAGENT_CONTINUATION_PREFIX: &str = "subagent_continuation:";

/// Default agent max iterations.
pub const DEFAULT_MAX_ITERATIONS: u32 = 60;

/// Default context token limit.
pub const DEFAULT_MAX_CONTEXT_TOKENS: usize = 128_000;

/// RPC client timeout (60 minutes).
pub const RPC_CLIENT_TIMEOUT_SECS: u64 = 3600;

/// Peer chat handler timeout (59 minutes).
pub const PEER_CHAT_TIMEOUT_SECS: u64 = 3540;

/// RPC channel timeout (24 hours, safety net).
pub const RPC_CHANNEL_TIMEOUT_SECS: u64 = 86400;

/// Default broadcast channel capacity.
pub const BUS_CHANNEL_CAPACITY: usize = 1024;

/// Default cleanup interval.
pub const CLEANUP_INTERVAL_SECS: u64 = 30;

/// Default scanner config file.
pub const SCANNER_CONFIG_FILE: &str = "config.scanner.json";

/// Forge workspace directory.
pub const FORGE_DIR: &str = "forge";

/// Cluster workspace directory.
pub const CLUSTER_DIR: &str = "cluster";

/// Skills directory.
pub const SKILLS_DIR: &str = "Skills";

/// Internal channels that should not be exposed to external users.
pub const INTERNAL_CHANNELS: &[&str] = &["cli", "system", "subagent"];

/// K3（devtool-upgrade 阶段 4）：内置 slash 命令名——单一真相源。AgentLoop
/// 的短路匹配（`rewrite_custom_command` 跳过内置名 + `handle_command` 的
/// match 臂）与 dashboard `commands.list` 下发给前端补全菜单的 `builtins`
/// 都从这里读，防止两处清单漂移。
pub const BUILTIN_SLASH_COMMANDS: &[&str] = &[
    "help", "model", "show", "list", "switch", "compact", "clear", "plan", "build",
];

/// Check if a channel is an internal channel.
pub fn is_internal_channel(channel: &str) -> bool {
    INTERNAL_CHANNELS.contains(&channel)
}
