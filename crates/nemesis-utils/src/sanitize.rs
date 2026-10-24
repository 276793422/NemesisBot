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
    // `cluster_rpc:{node_id}/board:{n}`——派发 chat 形态自 34a0650a 起恒为
    // 裸数字单号，`board:NB-` 从未被任何已提交代码产出，当年事故注记把
    // issue key 的 NB- 前缀误写进会话键分解式；节点 id 随主机名变长，
    // 两位数单号即可把全长推过 81 字符）——take(80) 使仅末字符不同的相
    // 邻单号双双截成同名，get_or_create 的磁盘 fallback 由此跨会话读到
    // 别人的历史——职能后缀进 system prompt 后回显串台才暴露。超限键改
    // 保区分度：头 60 字符 + `__` + FNV-1a64 hex16（纯 std、跨版本确定
    // 性），总长 78 ≤ 80；读写两侧同源同函数，形态自洽。≤80 的输入逐字
    // 节不变（存量文件名不受影响）；超限旧文件本就是碰撞脏数据，失联由
    // chat_log 重建 / TTL 自愈兜底。尾 `.` 修剪同趟进行（Windows 剥尾
    // 点；hash 后缀是 hex，永不触点）。
    let char_count = out.chars().count();
    // 尾点修剪先行（Windows 剥尾点），撞形判定看**修剪后**的形态——
    // `<60>__<16hex>.` 修剪后恰是 hash 形态，判定在修剪前会让这条 79 字
    // 符输入经直通+修剪重新抵达 hash 输出域，分离失效。
    let trimmed = out.trim_end_matches('.');
    // D12-4（2026-10-13 第 6 轮审计）：hash 输出形态与输入域分离——
    // 78 字符且形如 `<60>__<16 hex>` 的**合法输入**逐字节直通时，与超限
    // 输入的 hash 输出无法区分：两个不同 id 产出同一文件段（直通原样 vs
    // 另一个 head60 恰好同前缀的 id 的 hash 后缀撞形），读写两侧同函数
    // 自洽掩盖了跨段串读。hash 形态输出的不变量因此收紧为「只有 hash
    // 分支产出」：撞形输入强制进 hash 分支（输出仍是 head60__hash16，
    // 但输入域里不再有第二条路抵达该形态；FNV-1a64 碰撞界 2^-64 与既有
    // 方案同量级）。
    if char_count <= 80 && !is_hash_suffix_form(trimmed) {
        return trimmed.to_string();
    }
    let head: String = out.chars().take(60).collect();
    format!("{head}__{:016x}", fnv1a64(out.as_bytes()))
}

/// Escape one untrusted field for CSV output (RFC 4180)。
///
/// 含 `"` / `,` / `\n` / `\r` 的字段用引号包裹并把内部引号翻倍；普通字段
/// 逐字节直通（不加无谓引号）。单一真相源：nemesis-security 审计链的
/// 同名函数（原地委托至此）与 board 档案 usage.csv 的 task_id/worker_id
/// 列都走这里——worker_id 是用户自设自由文本（cluster init --name /
/// node.update_identity，无字符集校验），裸拼会把逗号/换行拼进结构化
/// CSV 错位移列（BUG-20261024-06）。比原 audit_log 版多认 `\r` 触发
/// （RFC 4180 同属须引号字符，审计链历史输出域无裸 CR，行为不变）。
pub fn sanitize_csv(s: &str) -> String {
    if s.contains('"') || s.contains(',') || s.contains('\n') || s.contains('\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// hash 后缀输出形态判定：78 字节 = 头 60 + `__` + 16 个小写 hex。
/// 只认 hash 分支会产出的形态（小写 hex、精确 78 字节）——直通域里的
/// 撞形输入由此识别并强制改道 hash 分支。
fn is_hash_suffix_form(out: &str) -> bool {
    out.len() == 78
        && out.as_bytes()[60..62] == *b"__"
        && out.as_bytes()[62..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
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
