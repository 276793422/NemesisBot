use super::{sanitize_csv, sanitize_path_segment};

#[test]
fn plain_ids_pass_through() {
    assert_eq!(
        sanitize_path_segment("agent_main_session_123"),
        "agent_main_session_123"
    );
    assert_eq!(
        sanitize_path_segment("task-1726000000000_ab12"),
        "task-1726000000000_ab12"
    );
    assert_eq!(
        sanitize_path_segment("node-laptop-a1b2"),
        "node-laptop-a1b2"
    );
}

#[test]
fn colons_and_separators_collapse_to_underscore() {
    // 旧 `replace(':', "_")` 语义保留：复合键文件名与存量平面文件一致。
    assert_eq!(
        sanitize_path_segment("agent:main:session:1726"),
        "agent_main_session_1726"
    );
    assert_eq!(sanitize_path_segment("a/b\\c:d"), "a_b_c_d");
}

#[test]
fn slash_composite_key_flattens_instead_of_nesting() {
    // SAN-01 核心场景：B 端 `{node}/{chat}` 键不再拆出中间目录。
    assert_eq!(sanitize_path_segment("node-x/chat:123"), "node-x_chat_123");
}

#[test]
fn dot_guard_blocks_traversal_forms() {
    // SAN-03：`..` 原样放行会逃一级目录。
    assert_eq!(sanitize_path_segment(".."), "__");
    assert_eq!(sanitize_path_segment("..."), "___");
    assert_eq!(sanitize_path_segment("a..b"), "a._b"); // 首个 `.` 跟 alnum 保留
    assert_eq!(sanitize_path_segment(".hidden"), "_hidden");
    // 尾点修剪（Windows 文件名剥尾点，读侧会失配）。
    assert_eq!(sanitize_path_segment("a."), "a");
}

#[test]
fn empty_becomes_underscore() {
    assert_eq!(sanitize_path_segment(""), "_");
}

#[test]
fn length_capped_at_80_with_hash_suffix() {
    let long = sanitize_path_segment(&"a".repeat(120));
    assert_eq!(long.chars().count(), 78, "60 头 + 2 分隔 + 16 hex");
    assert!(long.starts_with(&"a".repeat(60)));
    assert!(long.contains("__"));
    // 截断留下的尾点被修剪（Windows 文件名剥尾点）；hash 后缀是 hex，
    // 超限形态永不触点。
    let dots = sanitize_path_segment(&format!("{}.", "a".repeat(90)));
    assert_eq!(dots.chars().count(), 78);
    assert!(!dots.ends_with('.'));
}

#[test]
fn over_limit_keys_differing_at_tail_do_not_collide() {
    // 2026-09-29 UAT 回归钉（2026-10-10 改真实形态重钉）：cluster 会话键
    // `cluster_rpc:{node_id}/board:{n}`——派发 chat 形态恒为裸数字单号
    // （`board:NB-` 从未被生产代码产出，原夹具误用 issue key 形态）。
    // 节点 id 随主机名变长（此处 60 字符形态真实存在），两位数单号即把
    // 全长推到 81——纯截断使 10…19 仅末字符不同却双双落同名 `…board_1`，
    // 磁盘 fallback 跨会话串读。修复后必须各得唯一文件名。
    let node = "cluster_rpc:node-laptop-mbrq578qxyz-b11dbf4f-905f-40d9-8530-a2238e5aaf5c";
    let a = sanitize_path_segment(&format!("{node}/board:10"));
    let b = sanitize_path_segment(&format!("{node}/board:11"));
    let c = sanitize_path_segment(&format!("{node}/board:12"));
    assert_eq!(format!("{node}/board:10").chars().count(), 81);
    assert_ne!(a, b);
    assert_ne!(b, c);
    assert_ne!(a, c);
    // 同输入两次调用一致（读写两侧同源）。
    assert_eq!(a, sanitize_path_segment(&format!("{node}/board:10")));
    // D12-4 域分离：hash 形态产物再消毒**不再**原样（hash 输出形态是
    // 输出域专属——若直通原样，任何能注入会话键的一方都能把 78 字符
    // 撞形 id 直接落到别人会话的文件段上）。映射仍确定（读写两侧同函
    // 数同输入恒同输出），只是 hash 形态段不再幂等。
    assert_ne!(sanitize_path_segment(&a), a);
}

#[test]
fn keys_at_or_under_80_stay_byte_identical() {
    // 恰 80 字符（72 前缀 + 个位数单号）走原路径，逐字节不变——存量文件
    // 不失联。
    let node = "cluster_rpc:node-laptop-mbrq578qxyz-b11dbf4f-905f-40d9-8530-a2238e5aaf5c";
    let k80 = format!("{node}/board:9"); // 72 + 8 = 80
    assert_eq!(k80.chars().count(), 80);
    assert_eq!(
        sanitize_path_segment(&k80),
        "cluster_rpc_node-laptop-mbrq578qxyz-b11dbf4f-905f-40d9-8530-a2238e5aaf5c_board_9"
    );
}

#[test]
fn whitelist_is_idempotent() {
    // 读侧对已消毒 stem 再消毒必须原样（logs.rs ledger_file_name 语义）。
    let once = sanitize_path_segment("web://user/chat|file?x");
    assert_eq!(sanitize_path_segment(&once), once);
    assert_eq!(once, "web___user_chat_file_x");
}

// ---------- D12-4（2026-10-13 第 6 轮审计）：hash 输出形态与输入域分离 ----------

#[test]
fn hash_form_lookalike_input_does_not_pass_through() {
    // 78 字符、形如 `<60>__<16hex>` 的合法输入曾逐字节直通——与超限输入
    // 的 hash 输出同形态，撞形 id 直接落到目标会话的文件段（确定性伪造，
    // 无需哈希碰撞）。域分离后必须改道 hash 分支。
    let lookalike = format!("{}__0123456789abcdef", "a".repeat(60));
    assert_eq!(lookalike.len(), 78);
    let out = sanitize_path_segment(&lookalike);
    assert_ne!(out, lookalike, "撞形输入不得直通");
    assert_eq!(out.len(), 78, "改道后仍是 hash 形态");
    assert!(out.starts_with(&"a".repeat(60)));
    // 尾点修剪后再撞形同样改道（`<60>__<16hex>.` 修剪后恰是 hash 形态）。
    let dotted = format!("{lookalike}.");
    assert_ne!(sanitize_path_segment(&dotted), dotted.trim_end_matches('.'));
    // 接近但不构成 hash 形态的输入照常直通（不扩大改道面）：
    // 77 字符（hex 少一位）不撞形。
    let short = format!("{}__0123456789abcde", "a".repeat(60));
    assert_eq!(sanitize_path_segment(&short), short);
    // 大写 hex 不是 hash 分支产出形态（输出恒小写），不撞形。
    let upper = format!("{}__0123456789ABCDEF", "a".repeat(60));
    assert_eq!(sanitize_path_segment(&upper), upper);
}

#[test]
fn hash_output_represented_as_id_lands_elsewhere() {
    // 攻击面回归钉：受害者超限会话键的文件段 V，作为**原始 id**再喂入
    // 不得落回 V（2026-09-29 纯截断碰撞根修的完整性补丁）。
    let node = "cluster_rpc:node-laptop-mbrq578qxyz-b11dbf4f-905f-40d9-8530-a2238e5aaf5c";
    let victim = sanitize_path_segment(&format!("{node}/board:10"));
    assert_eq!(victim.len(), 78);
    let relay = sanitize_path_segment(&victim);
    assert_ne!(relay, victim, "hash 段作原始 id 不得落回本段");
    // 确定性：同一输入恒同输出（读写两侧同函数）。
    assert_eq!(sanitize_path_segment(&victim), relay);
}

#[test]
fn csv_plain_field_passes_through_unquoted() {
    // 普通字段逐字节直通——档案 usage.csv 现网行形态不受影响（无谓引号
    // 会让既有消费者误判）。
    assert_eq!(sanitize_csv("task-a1"), "task-a1");
    assert_eq!(sanitize_csv("node-w"), "node-w");
    assert_eq!(sanitize_csv(""), "");
    assert_eq!(sanitize_csv("节点 甲"), "节点 甲");
}

#[test]
fn csv_special_chars_wrap_and_double_inner_quotes() {
    assert_eq!(sanitize_csv("a,b"), "\"a,b\"");
    assert_eq!(sanitize_csv("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(sanitize_csv("a,\"b\",c"), "\"a,\"\"b\"\",c\"");
    assert_eq!(sanitize_csv("line1\nline2"), "\"line1\nline2\"");
    // `\r` 也触发包裹（RFC 4180 须引号字符；比 audit_log 旧版多认）。
    assert_eq!(sanitize_csv("cr\rtail"), "\"cr\rtail\"");
}
