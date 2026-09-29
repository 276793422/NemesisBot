//! 错误类型（crate 内统一）。

use thiserror::Error;

/// WASM 插件框架错误。
#[derive(Debug, Error)]
pub enum PluginError {
    /// manifest 缺失/非法/未过校验。
    #[error("manifest invalid: {0}")]
    Manifest(String),

    /// 合同代际不认识（组件导出的 package 版本超出宿主支持）。
    #[error(
        "contract version mismatch: manifest api-version {manifest}, host supports {supported}"
    )]
    ContractVersion {
        /// manifest 声明的代际。
        manifest: u32,
        /// 宿主支持的代际。
        supported: u32,
    },

    /// 签名/信任校验失败（含 blocked 四态）。
    #[error("trust check failed: {0}")]
    Trust(String),

    /// wasm 文件缺失/hash 不符/超限。
    #[error("wasm payload invalid: {0}")]
    Wasm(String),

    /// 病毒扫描命中或扫描器故障。
    #[error("scan blocked: {0}")]
    Scan(String),

    /// 装配审批被拒。
    #[error("install rejected: {0}")]
    Rejected(String),

    /// 组件编译失败。
    #[error("compile failed: {0}")]
    Compile(String),

    /// 组件装载后自报与 manifest 不一致（get-metadata 对账）。
    #[error("metadata mismatch: {0}")]
    MetadataMismatch(String),

    /// 插件未安装或未启用。
    #[error("plugin not available: {0}")]
    NotAvailable(String),

    /// 执行超时（epoch 墙钟或外层 tokio 超时）。
    #[error("execution timeout after {ms}ms")]
    Timeout {
        /// 超时墙钟毫秒。
        ms: u64,
    },

    /// 并发实例满员。
    #[error("concurrent instance limit reached ({0})")]
    Busy(usize),

    /// guest trap（fuel 耗尽/epoch/内存超限/panic 等，含 trap 文本）。
    #[error("guest trap: {0}")]
    Trap(String),

    /// 宿主能力面故障（IO/代理/下游工具错误）。
    #[error("host capability failure: {0}")]
    Host(String),

    /// 观察者投递面故障（非事件内容问题）。
    #[error("observer pump failure: {0}")]
    Observer(String),

    /// lockfile / 实例配置 IO 故障。
    #[error("io failure: {0}")]
    Io(String),
}
