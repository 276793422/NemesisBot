//! connector 注册表核心（AI 原生行动代理 P0，2026-10-14）。
//!
//! skills/MCP/工具三个现存 registry 之上的元数据层：connector.toml 清单
//! schema、workspace 注册表扫描、统一回执存储、预算护栏、纸面券商账本。
//! 执行绑定（http/tool/action）的消费方（connector_invoke 工具、安全管线
//! 动态风险映射、WSAPI 数据面）在 nemesisbot / nemesis-web 侧。
//!
//! 设计纪律（照搬全仓惯例）：清单结构 `deny_unknown_fields` 钉拼写；
//! `read_dir` 结果显式排序；损坏数据诚实报错不当空处理。

pub mod budget;
pub mod ledger;
pub mod manifest;
pub mod receipt;
pub mod registry;
