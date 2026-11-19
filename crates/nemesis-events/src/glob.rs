//! 极简通配符匹配（`*` 任意串含空 / `?` 恰一字符，逐 char、大小写敏感）。
//!
//! 不引 glob crate：workspace 依赖无 glob，本匹配是唯一消费点（规则文件名
//! 过滤），30 行纯函数 + 单测即可。`*` 不跨路径分隔符的语义由「只对
//! file_name 匹配」天然成立（见 rules::rule_matches）。

pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    // 最后一个 `*` 的回溯点：通配失败时回到 * 后一位、文本前进一位重试。
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests;
