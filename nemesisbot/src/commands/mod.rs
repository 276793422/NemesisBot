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
// 行动代理连接器 CLI（`nemesisbot connector`；connectors feature 门控，
// nemesis-connectors 整包随它进出——连带 nemesis-security）。
#[cfg(feature = "connectors")]
pub mod connector;
pub mod cors;
pub mod credentials;
pub mod cron;
pub mod dashboard;
#[cfg(feature = "board")]
pub mod dossier;
pub mod estop;
#[cfg(feature = "eval")]
pub mod eval;
/// `eval rules` 子命令组（规则管理；纯文件操作跨平台）。
#[cfg(feature = "eval")]
pub mod eval_rules;
#[cfg(feature = "forge")]
pub mod forge;
pub mod gateway;
#[cfg(feature = "history-search")]
pub mod history;
#[cfg(feature = "board")]
pub mod issue;
pub mod log;
pub mod mcp;
/// P23（能力扩展 WS10）：stdIO MCP server 入口（`nemesisbot mcp-serve`）。
pub mod mcp_serve;
#[cfg(feature = "memory")]
pub mod memory;
#[cfg(feature = "migrate")]
pub mod migrate;
pub mod model;
/// `onboard default` 初始化序列 + 双击直启 auto-init（Seed 种子模式）。
/// 从 main.rs 内联实现提取（双击直启 goal 2026-09-17）。
pub mod onboard;
pub mod persona;
/// 用户习惯画像 CLI（自进化升级 B 线：list/enable/disable/veto/forget/pin）。
pub mod profile;
// QUIC 中继 CLI（roaming-relay feature 穿透 nemesis-cluster/roaming-relay）。
#[cfg(feature = "roaming-relay")]
pub mod quic;
/// K1（devtool-upgrade 阶段 4）：headless 单任务执行（无端口、无 gateway，
/// 安全 9 层全量生效，跑完即退）。
pub mod run;
#[cfg(feature = "sandbox")]
pub mod sandbox;
// VM 环境自举 CLI（第 10 层安装面；vm feature 穿透 nemesis-sandbox/vm）。
#[cfg(feature = "vm")]
pub mod vm;
// 病毒扫描 CLI（scanner feature 从 security 拆出；implies security）。
#[cfg(feature = "scanner")]
pub mod scanner;
// 紫微系统 CLI（三方程序受控接管；ziwei feature 门控，config ziwei.enabled
// 运行闸默认关）。
#[cfg(feature = "security")]
pub mod security;
pub mod session;
#[cfg(feature = "ziwei")]
pub mod ziwei;
// L9 版本自切换通道 CLI（check/status 离线可用；apply 触发 gateway 侧编排）。
#[cfg(feature = "selfupdate")]
pub mod selfupdate;
pub mod shutdown;
// WASM 插件 CLI（plugins-wasm feature 从 security 派生；implies security）。
#[cfg(feature = "plugins-wasm")]
pub mod plugin;
pub mod skills;
pub mod status;
#[cfg(feature = "desktop")]
pub mod test_cmd;
#[cfg(feature = "security")]
pub mod vault;
#[cfg(feature = "voice")]
pub mod voice;
#[cfg(feature = "workflow")]
pub mod workflow;
