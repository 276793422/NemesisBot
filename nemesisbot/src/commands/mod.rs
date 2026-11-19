//! CLI command modules.

/// （devtool-upgrade）：ACP server（编辑器等客户端 stdio 接入）。
pub mod acp;
pub mod agent;
/// 资产盘点快照读取（W1 #11+#6–10 AI-BOM；读 gateway 落盘的
/// logs/assets/inventory.json，BOM 投影与 WSAPI 同源）。
pub mod assets;

/// 用途登记种子内容（W1 #1；单一真相源 = nemesis-web charter 模块，
/// onboard 落盘与 WSAPI/文档面同源同形）。
pub(crate) fn charter_seed_json() -> &'static str {
    nemesis_web::handlers::charter::seed_json()
}
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
/// durable loop 管理 CLI（`loop` 子命令；项5；`loop` feature 门控）。
#[cfg(feature = "loop")]
pub mod loops;
pub mod mcp;
/// stdIO MCP server 入口（`nemesisbot mcp-serve`）。
pub mod mcp_serve;
#[cfg(feature = "memory")]
pub mod memory;
#[cfg(feature = "migrate")]
pub mod migrate;
pub mod model;
/// `onboard default` 初始化序列 + 双击直启 auto-init（Seed 种子模式）。
/// 从 main.rs 内联实现提取（双击直启 goal）。
pub mod onboard;
pub mod persona;
/// 用户习惯画像 CLI（自进化升级 B 线：list/enable/disable/veto/forget/pin）。
pub mod profile;
// QUIC CLI（与集群解耦：QUIC 自身配置/身份/中继是 QUIC 层
// 资产，不寄居集群命令族；relay 运行面在 plugin_roaming 插件）。
pub mod quic;
/// （devtool-upgrade）：headless 单任务执行（无端口、无 gateway，
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
// 系统集成 CLI（system register/unregister/status/cert；安装器 spawn 契约面，
// 恒编译——任何构建形态的安装器都要求 system 子命令在场）。
pub mod system;
#[cfg(feature = "ziwei")]
pub mod ziwei;
// 版本自切换通道 CLI（check/status 离线可用；apply 触发 gateway 侧编排）。
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
