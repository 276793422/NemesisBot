//! Registry URL 构造共享的 percent-encoding 助手（单一真相源）。
//!
//! 历史形态：`modelscope_registry::url_encode_component`、
//! `clawhub_registry` 内嵌 `urlencoding` 模块各自持有一份逐字节相同的
//! 实现——BUG-20261113-162 横扫发现 github 族（github_registry /
//! github_tree）第三份缺失：路径段/分支名裸插值进 URL，`#`/`?` 沦为
//! fragment/query 分隔符指向错误资源（GitHub 文件名与 git ref 均合法
//! 容纳这两字符）；legacy `GitHubRegistry::new` 与
//! `ClawHubRegistry::with_urls` 的 base_url 亦无尾斜杠归一（同族先例 =
//! BUG-20261109-012 telegram/bluesky/mastodon + modelscope 已归一）。
//! 现收敛到本模块，三处旧名 delegate 保持既有测试面不变。
//!
//! 编码规则：RFC 3986 unreserved 集（`A-Z a-z 0-9 - _ . ~`）直通，
//! 其余按 UTF-8 字节逐字节 `%XX`。**必须按字节迭代**：按 char 迭代后
//! `as u8` 会把多字节码点截断到低 8 位（'中' U+4E2D → `%2D` = '-'），
//! 静默腐蚀一切非 ASCII 检索词/slug/文件路径。
//!
//! 分段 vs 整路径：component 形态编码 `/`（单段语义——slug、分支名、
//! ref、query 值）；path 形态保留 `/` 结构、逐段编码（多段文件路径——
//! index_path、blob_path、skill_path_pattern 展开结果）。

/// Percent-encode 单个 URL 路径段/query 值（RFC 3986 unreserved 直通）。
pub fn percent_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Percent-encode 多段文件路径，保留 `/` 分隔符、逐段编码。
pub fn percent_encode_path(p: &str) -> String {
    p.split('/')
        .map(percent_encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// raw 内容 URL 构造单点：`{base}/{repo}/{ref}/{path}`（尾斜杠归一 +
/// 各段 percent-encoding）。BUG-20261113-162：github_registry 三处 join
/// 与 github_tree blob 下载此前裸插值，`#`/`?` 沦为 fragment/query 分隔
/// 符；base 带尾斜杠拼出 `//{repo}` 双斜杠路径。repo/path 为多段形态
/// （owner/repo、文件路径），ref/branch 为单段（合法可含 `#`）。
pub fn raw_content_url(raw_base_url: &str, repo: &str, reference: &str, path: &str) -> String {
    format!(
        "{}/{}/{}/{}",
        raw_base_url.trim_end_matches('/'),
        percent_encode_path(repo),
        percent_encode_component(reference),
        percent_encode_path(path)
    )
}

#[cfg(test)]
mod tests;
