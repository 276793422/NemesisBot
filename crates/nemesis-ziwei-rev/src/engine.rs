//! 分析引擎能力探测（WSAPI `ziwei.status` 的 setup_engine 面）。

use serde::Serialize;

/// 引擎能力/版本信息。
#[derive(Debug, Clone, Serialize)]
pub struct EngineInfo {
    pub available: bool,
    pub disasm_archs: &'static [&'static str],
    pub note: String,
}

/// M3 起：iced-x86 decoder 常驻（纯分析库，无运行时依赖探测）。
pub fn engine_info() -> EngineInfo {
    EngineInfo {
        available: true,
        disasm_archs: &["x64", "x86"],
        note: "配置期分析引擎已启用（M3：pe/disasm/funcs/xref/crtsigs/snapshot/diff/package；recover 随 M5）"
            .to_string(),
    }
}
