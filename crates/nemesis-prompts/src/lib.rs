//! 提示词资产单一真相源（prompt pack pro M7）。
//!
//! 全仓 LLM 提示词文本集中存放在本 crate——未来升级提示词只改这里，
//! 不再全仓翻找。设计约束：
//!
//! - **零运行时依赖**：纯静态文本（`include_str!` 编译期嵌入或 Rust 字符串
//!   常量）+ 纯函数渲染。任何 crate 都可以依赖本 crate 而不引入反向依赖
//!   或循环。
//! - **只存文本与纯渲染，不存逻辑**：提示词的解析（JSON verdict、schema
//!   抽取）、调用护栏（超时/限 token）、业务判定全部留在各消费 crate；
//!   本 crate 的函数只做字符串拼装。
//! - **字节级平移纪律**：文本从消费方迁入时逐字节保持（golden/classic
//!   字节不变测试是安全网）；改动任何一条文本都必须跑对应消费方的
//!   回归测试。
//!
//! ## 资产清单（治理规范见 README.md）
//!
//! | 模块 | 内容 | 消费方 |
//! |---|---|---|
//! | [`system`] | 主 system prompt 段落池（Pre/Post 两层 17 段） | nemesis-agent |
//! | [`tools`] | 工具描述两档查表（60 lean + 24 full） | nemesis-agent |
//! | [`subagents`] | 子代理十角色模板 | nemesis-agent |
//! | [`slash`] | 内置 slash 深度模板（评审/排障/修复） | nemesis-agent |
//! | [`aux`] | 内部调用点文案（compact 九段式/合并/标题/快照两节） | nemesis-agent |
//! | [`guardian`] | 安全闸审计提示词 | nemesis-security |
//! | [`forge`] | Forge 产物质量评审 + 技能/脚本生成提示词 | nemesis-forge |
//! | [`board`] | 看板 planner/review/项目总结/冲突硬解 | nemesis-board / nemesisbot |
//! | [`workflow`] | 工作流分类/抽取节点提示词 | nemesis-workflow |
//! | [`spawn`] | 旧版子代理单句 system prompt（字节保留） | nemesis-tools |
//! | [`persona_gen`] | 集群人格生成三阶段（extract/author/audit） | nemesis-web |
//!
//! ## 文件形态约定
//!
//! - 大段纯静态模板：`*.md` 文件 + `include_str!`（段落池/描述/角色）。
//!   （原 `internals/compact.md` 七节摘要模板已随 Summarizer 真源归一退役，
//!   结构化摘要文本现以 Rust 常量/渲染函数存于 [`aux`]。）
//! - 带转义续行或精确字节要求的常量：Rust 字符串常量原样存放
//!   （guardian/board/workflow/spawn/forge），逐字节平移自原消费方。

// 文件名 aux_prompts.rs：`aux` 是 Windows 保留设备名，git-for-windows
// （core.protectNTFS）拒绝索引 aux.rs——文件名避开，#[path] 保持模块
// 公开路径 `nemesis_prompts::aux` 不变。
#[path = "aux_prompts.rs"]
pub mod aux;
pub mod board;
pub mod forge;
pub mod guardian;
pub mod persona_gen;
pub mod slash;
pub mod spawn;
pub mod subagents;
pub mod system;
pub mod tools;
pub mod workflow;

#[cfg(test)]
mod tests;
