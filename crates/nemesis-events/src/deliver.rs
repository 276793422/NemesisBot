//! 注入模板渲染：`{count}` / `{path}` / `{paths}` 占位符。
//!
//! 占位符替换顺序纪律：`{paths}` 必须先于 `{path}` 替换——否则 `{paths}` 会
//! 被 `{path}` 的替换值吃掉尾字符变形态。未知占位符原样保留（不猜）。
//! 路径列名超过 [`MAX_PATHS_LISTED`] 时截断 + 「…等 N 个」注记（诚实总量
//! 由 `{count}` 承载）。

use super::throttle::MAX_PATHS_LISTED;

/// 渲染注入内容正文（不含 `[事件:<rule_id>]` 徽标前缀——徽标在 manager 侧拼）。
pub fn render_prompt(template: &str, count: u32, paths: &[String]) -> String {
    let paths_text = if paths.len() <= MAX_PATHS_LISTED {
        paths.join("\n")
    } else {
        let shown: Vec<&str> = paths[..MAX_PATHS_LISTED]
            .iter()
            .map(String::as_str)
            .collect();
        format!("{}\n…等 {} 个", shown.join("\n"), count)
    };
    template
        .replace("{count}", &count.to_string())
        .replace("{paths}", &paths_text)
        .replace("{path}", paths.first().map(String::as_str).unwrap_or(""))
}

/// 通用占位符渲染（新事件源：webhook/proc/sys）。`vars` 按占位符名替换
/// `{name}`；同名前缀吞噬纪律与 fs 渲染一致——**按名字长度降序替换**
/// （`{payload}` 先于 `{pay}`，长名先替换不吃短名）。
/// 未知占位符原样保留（不猜）。fs 源不走此函数（`render_prompt` 保持
/// 字节不变——prompt cache / 既有测试锚定）。
pub fn render_prompt_vars(template: &str, count: u32, vars: &[(&str, String)]) -> String {
    let mut out = template.replace("{count}", &count.to_string());
    let mut sorted: Vec<&(&str, String)> = vars.iter().collect();
    sorted.sort_by(|a, b| a.0.len().cmp(&b.0.len()).reverse());
    for (name, value) in sorted {
        let needle = format!("{{{name}}}");
        out = out.replace(&needle, value);
    }
    out
}

/// 注入内容完整形态：徽标前缀 + 正文。徽标让事件消息在会话历史里可辨识
/// （项1 风险注记：事件注入占用会话历史，须标 event 来源）。
pub fn compose_content(rule_id: &str, rendered: &str) -> String {
    format!("[事件:{rule_id}] {rendered}")
}

#[cfg(test)]
mod tests;
