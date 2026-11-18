//! 注入模板渲染：`{count}` / `{path}` / `{paths}` 占位符。
//!
//! BUG-20261119-176（渲染面）：替换是**单遍扫描 + 值内容不回扫**——此前
//! 逐占位符 `str::replace` 全串替换，值内容里恰好含后手占位符字面量会被
//! 二次替换（fs 路径可含 `{}`，Windows/Linux 均为合法文件名字符：监视到
//! 名为 `报告{path}备份.md` 的文件，投递消息里路径碎成
//! `dir\报告dir\报告{path}备份.md备份.md` 形态）。前缀吞噬（`{paths}` vs
//! `{path}`）由最长优先匹配天然承担，不再依赖替换顺序纪律。未知占位符
//! 原样保留（不猜）。非冲突输入逐字节等价旧实现。
//! 路径列名超过 [`MAX_PATHS_LISTED`] 时截断 + 「…等 N 个」注记——N 是
//! `paths.len()`（去重路径总数；BUG-20261118-165：此前用 count 即事件总数，
//! 同路径多事件场景把事件数冒充路径数，注记数字必然虚大）。事件总量由
//! `{count}` 承载，两个数字语义不同不可互替。

use super::throttle::MAX_PATHS_LISTED;

/// 单遍占位符替换：扫描 `template`，在每个 `{` 处按**名字最长优先**匹配
/// `vars` 中的 `{name}`，命中拼接值并跳过；未命中推进一个字符保住 `{`
/// 字面量。值内容永不回扫——值里的占位符样文本按字面量保留。
fn replace_placeholders_once(template: &str, mut vars: Vec<(&str, String)>) -> String {
    // 最长优先：同位置贪心吃长名（{paths} 先于 {path} 试匹配）。
    vars.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(brace) = rest.find('{') {
        out.push_str(&rest[..brace]);
        let after = &rest[brace..];
        let mut matched = false;
        for (name, value) in &vars {
            let mut needle = String::with_capacity(name.len() + 2);
            needle.push('{');
            needle.push_str(name);
            needle.push('}');
            if after.starts_with(&needle) {
                out.push_str(value);
                rest = &after[needle.len()..];
                matched = true;
                break;
            }
        }
        if !matched {
            let ch = rest[brace..].chars().next().unwrap_or('{');
            out.push(ch);
            rest = &rest[brace + ch.len_utf8()..];
        }
    }
    out.push_str(rest);
    out
}

/// 渲染注入内容正文（不含 `[事件:<rule_id>]` 徽标前缀——徽标在 manager 侧拼）。
pub fn render_prompt(template: &str, count: u32, paths: &[String]) -> String {
    let paths_text = if paths.len() <= MAX_PATHS_LISTED {
        paths.join("\n")
    } else {
        let shown: Vec<&str> = paths[..MAX_PATHS_LISTED]
            .iter()
            .map(String::as_str)
            .collect();
        // 注记用 paths.len()（去重路径总数）——count 是事件总数，同路径
        // 多事件时两者分叉，「…等 N 个」的 N 修饰的是路径列表（BUG-20261118-165）。
        format!("{}\n…等 {} 个", shown.join("\n"), paths.len())
    };
    replace_placeholders_once(
        template,
        vec![
            ("paths", paths_text),
            (
                "path",
                paths.first().map(String::as_str).unwrap_or("").to_string(),
            ),
            ("count", count.to_string()),
        ],
    )
}

/// 通用占位符渲染（新事件源：webhook/proc/sys）。`vars` 按占位符名替换
/// `{name}`（单遍扫描 + 最长优先 + 值不回扫，见模块头）；`{count}` 同批
/// 参与。未知占位符原样保留（不猜）。fs 源不走此函数（`render_prompt`
/// 保持字节不变——prompt cache / 既有测试锚定）。
pub fn render_prompt_vars(template: &str, count: u32, vars: &[(&str, String)]) -> String {
    let mut all: Vec<(&str, String)> = vars.iter().map(|(n, v)| (*n, v.clone())).collect();
    all.push(("count", count.to_string()));
    replace_placeholders_once(template, all)
}

/// 注入内容完整形态：徽标前缀 + 正文。徽标让事件消息在会话历史里可辨识
/// （项1 风险注记：事件注入占用会话历史，须标 event 来源）。
pub fn compose_content(rule_id: &str, rendered: &str) -> String {
    format!("[事件:{rule_id}] {rendered}")
}

#[cfg(test)]
mod tests;
