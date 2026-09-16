//! Path-segment sanitization for untrusted identifiers (SAN-05 单一真相源).
//!
//! Turns model/peer-influenced ids (session keys, task ids, device ids,
//! transfer ids) into safe filename/path segments. Before this module every
//! consumer had its own divergent sanitizer (bare `replace(':', "_")`,
//! assorted blacklists), so B-side composite keys containing `/` nested
//! directories under the store root (SAN-01/F-U4-4) and hostile `..` passed
//! some blacklists outright (SAN-03).

/// Sanitize an untrusted id into ONE safe path segment.
///
/// Whitelist: ASCII alphanumerics, `-`, `_`, and `.` only when it directly
/// follows an alphanumeric (so `..`, `a..`, `.x` collapse to `_`).
/// Everything else — `/`, `\`, `:`, spaces, control chars — becomes `_`.
/// A result that sanitizes to empty (or a bare dot-form) becomes `_`; the
/// result is capped at 80 chars so a hostile long id cannot blow path
/// limits. Trailing `.` is trimmed (Windows strips trailing dots in
/// filenames), whether it survived the guard or the cap.
///
/// Compatibility: for the chat_log id families actually produced at runtime
/// (`agent:main:session:*`, `chan:chat`, `task_*`, `bg_*`, node ids,
/// hostnames) the mapping equals chat_log's legacy `replace(':', "_")` — only
/// ids containing `/` (or `\`) map differently, which is exactly the fix (they
/// used to nest/escape); legacy nested directories are flattened by the
/// startup migration (`chat_log::migrate_nested_session_logs`).
/// D-4（复核 2026-09-16）：该等价性**不**自动覆盖其他 consumer 的旧
/// sanitizer——如 episodic 的 9 字符 blacklist 保留空格/非 ASCII/`@`，白名单
/// 会映射成 `_`。那类 store 迁移前必须带旧名 fallback 读路径（见
/// episodic::session_file 注释），不得默认逐字节等价。
pub fn sanitize_path_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        let ok = c.is_ascii_alphanumeric()
            || c == '-'
            || c == '_'
            || (c == '.' && out.ends_with(|p: char| p.is_ascii_alphanumeric()));
        out.push(if ok { c } else { '_' });
    }
    // Guard against `.`/`..` after sanitization and empty names.
    if out.is_empty() || out == "." || out == ".." {
        out.push('_');
    }
    // Cap length: a hostile very-long id should not blow path limits.
    // Trailing dots are trimmed in the same pass (Windows strips trailing
    // dots in filenames; a bare "a." would be mangled on re-read).
    let capped: String = out.chars().take(80).collect();
    capped.trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests;
