//! 内置 slash 深度模板库（斜杠 = 提示词模板机制的产品内置层）。
//!
//! 自定义命令（`config.commands.json`）与技能斜杠化之间的一层：产品自带
//! 的深度工作流提示词（安全评审、代码评审、排障、修复纪律），`/name args`
//! 触发后整段展开为提示词、`$ARGUMENTS` 注入参数。文本单一真相源在
//! `slash/*.md`（编译期嵌入）。
//!
//! 优先级（`nemesis-agent` 侧 rewrite 链）：自定义命令 > 内置模板 > 技能
//! 回落——用户显式配置永远可覆盖内置默认。
//!
//! 信任边界：模板是编译期产品文本（可信）；`$ARGUMENTS` 只做纯文本替换，
//! **不做** shell 注入执行（`` !`cmd` `` 展开是用户自定义命令路径专属，
//! 内置路径不放大攻击面）。

/// 内置模板条目：`(命令名, 模板正文)`。展开语义与自定义命令一致：
/// 模板含 `$ARGUMENTS` → 替换注入；模板无占位符且带参数 → 参数追加为
/// 独立段（对用户更友好：忘写占位符时参数不被吞）。
static BUILTIN_TEMPLATES: &[(&str, &str)] = &[
    ("security-review", include_str!("slash/security-review.md")),
    ("code-review", include_str!("slash/code-review.md")),
    ("debug", include_str!("slash/debug.md")),
    ("fix", include_str!("slash/fix.md")),
];

/// 查内置模板；未命中 `None`（调用方走下一层解析，如技能回落）。
pub fn lookup(name: &str) -> Option<&'static str> {
    BUILTIN_TEMPLATES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
}

/// 展开模板：`$ARGUMENTS` 替换 / 参数追加（与自定义命令同语义，见模块级
/// 注释）。纯文本操作，无 I/O。
pub fn expand(template: &str, args: &str) -> String {
    if template.contains("$ARGUMENTS") {
        template.replace("$ARGUMENTS", args)
    } else if !args.is_empty() {
        format!("{}\n\n{}", template, args)
    } else {
        template.to_string()
    }
}

#[cfg(test)]
mod tests;
