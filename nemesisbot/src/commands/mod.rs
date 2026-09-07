//! CLI command modules.

/// L7（devtool-upgrade 阶段 7）：ACP server（编辑器等客户端 stdio 接入）。
pub mod acp;
pub mod agent;
#[cfg(feature = "auth")]
pub mod auth;
#[cfg(feature = "board")]
pub mod autopilot;
pub mod channel;
#[cfg(feature = "cluster")]
pub mod cluster;
pub mod cors;
pub mod credentials;
pub mod cron;
pub mod dashboard;
pub mod estop;
#[cfg(feature = "eval")]
pub mod eval;
/// `eval rules` 子命令组（规则管理；纯文件操作跨平台）。
#[cfg(feature = "eval")]
pub mod eval_rules;
#[cfg(feature = "forge")]
pub mod forge;
pub mod gateway;
pub mod history;
#[cfg(feature = "board")]
pub mod issue;
pub mod log;
pub mod mcp;
#[cfg(feature = "memory")]
pub mod memory;
#[cfg(feature = "migrate")]
pub mod migrate;
pub mod model;
pub mod persona;
/// K1（devtool-upgrade 阶段 4）：headless 单任务执行（无端口、无 gateway，
/// 安全 9 层全量生效，跑完即退）。
pub mod run;
#[cfg(feature = "sandbox")]
pub mod sandbox;
#[cfg(feature = "security")]
pub mod scanner;
#[cfg(feature = "security")]
pub mod security;
pub mod session;
pub mod shutdown;
pub mod skills;
pub mod status;
#[cfg(feature = "desktop")]
pub mod test_cmd;
#[cfg(feature = "voice")]
pub mod voice;
#[cfg(feature = "workflow")]
pub mod workflow;
