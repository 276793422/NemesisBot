use super::*;

#[test]
fn bind_and_target() {
    let r = ConvRouter::new();
    assert_eq!(r.target("agent:main:session:abc"), None);
    r.bind("agent:main:session:abc", "web:deadbeef");
    assert_eq!(
        r.target("agent:main:session:abc"),
        Some("web:deadbeef".to_string())
    );
}

#[test]
fn latest_wins_overwrites() {
    let r = ConvRouter::new();
    r.bind("agent:main:session:abc", "web:1111");
    r.bind("agent:main:session:abc", "web:2222");
    assert_eq!(
        r.target("agent:main:session:abc"),
        Some("web:2222".to_string())
    );
    assert_eq!(r.len(), 1);
}

#[test]
fn empty_inputs_ignored() {
    let r = ConvRouter::new();
    r.bind("", "web:x");
    r.bind("agent:main:session:abc", "");
    assert!(r.is_empty());
}

#[test]
fn distinct_conversations_independent() {
    let r = ConvRouter::new();
    r.bind("agent:main:session:a", "web:1");
    r.bind("agent:main:session:b", "web:2");
    assert_eq!(r.target("agent:main:session:a"), Some("web:1".to_string()));
    assert_eq!(r.target("agent:main:session:b"), Some("web:2".to_string()));
    assert_eq!(r.len(), 2);
}

/// BUG-20261116-020 回归锁：绑定表在会话数风暴下必须有界。
/// key 空间 = session_key 全史（每条 web 入站 bind 一次），latest-wins
/// 只限 per-key 条目数、不限 key 数——无界累积 = BUG-20261113-170 同族
/// （每个见过一次的会话留一条死绑定直到进程重启）。
#[test]
fn binding_table_capped_under_cardinality_storm() {
    let r = ConvRouter::new();
    for i in 0..20_000 {
        r.bind(&format!("agent:main:session:storm{i}"), &format!("web:{i}"));
    }
    assert!(
        r.len() <= MAX_BINDINGS,
        "binding table grew to {} entries — per-session dead entries accumulating without bound",
        r.len()
    );
}

/// 清空后的自愈：被整体清掉的活跃会话在下一条入站重绑后立即可寻址。
#[test]
fn binding_table_rebinds_after_cap_clear() {
    let r = ConvRouter::new();
    for i in 0..MAX_BINDINGS {
        r.bind(&format!("agent:main:session:junk{i}"), &format!("web:{i}"));
    }
    // 同一活跃会话反复重绑（contains_key 短路，不得触发清空）。
    r.bind("agent:main:session:live", "web:live-1");
    r.bind("agent:main:session:live", "web:live-2");
    assert_eq!(
        r.target("agent:main:session:live"),
        Some("web:live-2".to_string())
    );
}
