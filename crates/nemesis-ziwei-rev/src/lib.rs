//! nemesis-ziwei-rev —— 紫微系统配置期分析引擎（总控； 落地）。
//!
//! 纯分析库：输入 `Vec<u8> + base_rva + PE 元数据`，输出结构化结果；
//! 不碰进程/管道（内存读取编排在 nemesis-ziwei `setup` 模块）。
//!
//! - `pe`：轻量 PE 解析（file/memory 双布局映射 + export/import 表）
//! - `disasm`：iced-x86 封装（x64+x86 双模式）+ nop 串识别（.1）
//! - `funcs`：函数边界递归下降（种子=入口点+导出+直接调用目标）（.1）
//! - `xref`：字符串引用 / 导入调用 / 立即数 / 数据引用四索引（.2）
//! - `crtsigs`：MSVC CRT 折叠判定器（导入模块启发 + 字节签名库）（.3）
//! - `snapshot`：快照落盘/取回/清扫（.4）
//! - `diff`：行为页 diff（.5；.4 hook 计数增强复用）
//! - `package`：LLM 上下文打包 + LlmFn hook（不给裸全量汇编）（.6）
//! - `candidates`：锚候选启发式（AnchorType 载荷形态 + 置信注记）
//! - `recover`：找回流程逐锚重扫（填充；本里程碑只留接口）（.3）
//!
//! 窄门面纪律：本 crate 只被 `nemesis-ziwei` 依赖，bot 其余
//! 模块禁止直依赖。

pub mod candidates;
pub mod container;
pub mod crtsigs;
pub mod diff;
pub mod disasm;
pub mod engine;
pub mod funcs;
pub mod package;
pub mod pe;
pub mod recover;
pub mod snapshot;
pub mod xref;

#[cfg(test)]
mod tests;
