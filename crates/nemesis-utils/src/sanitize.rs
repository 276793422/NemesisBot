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
/// A result that sanitizes to empty (or a bare dot-form) becomes `_`. The
/// result is capped at 80 chars so a hostile long id cannot blow path
/// limits — over-limit ids keep a deterministic hash suffix instead of a
/// bare truncation, which would collide (see fn doc). Trailing `.` is
/// trimmed (Windows strips trailing dots in filenames), whether it survived
/// the guard or the cap.
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
    // Cap length: a hostile very-long id should not blow path limits. BUT
    // pure truncation collides（2026-09-29 UAT 实证：cluster 会话键
    // `cluster_rpc:{57 字符 node_id}/board:NB-10` 与 `…NB-11` 全长 81 字
    // 符，take(80) 双双截成 `…board_NB-1`，get_or_create 的磁盘 fallback
    // 由此跨会话读到别人的历史——职能后缀进 system prompt 后回显串台才
    // 暴露）。超限键改保区分度：头 60 字符 + `__` + FNV-1a64 hex16（纯
    // std、跨版本确定性），总长 78 ≤ 80；读写两侧同源同函数，形态自洽。
    // ≤80 的输入逐字节不变（存量文件名不受影响）；超限旧文件本就是碰撞
    // 脏数据，失联由 chat_log 重建 / TTL 自愈兜底。尾 `.` 修剪同趟进行
    // （Windows 剥尾点；hash 后缀是 hex，永不触点）。
    let char_count = out.chars().count();
    if char_count <= 80 {
        return out.trim_end_matches('.').to_string();
    }
    let head: String = out.chars().take(60).collect();
    format!("{head}__{:016x}", fnv1a64(out.as_bytes()))
}

/// FNV-1a 64-bit（零依赖确定性哈希，仅供超长段的区分后缀）。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests;
