//! Message-ID 面（QQ/163 等公共邮箱反垃圾评分项）：生成 / 写头 / 提取三函数
//! 的契约——单一真相源（build_smtp_message 内部生成 + extract_message_id
//! 回执提取）保证「回执 id == 载荷头 id」，二次调用值不同 = 回执造假。

use nemesis_mail_protocol::{build_smtp_message, extract_message_id, generate_message_id};

#[test]
fn test_build_smtp_message_writes_message_id_header() {
    let msg = build_smtp_message("bot@example.com", "user@x.com", "Hi", "body");
    // 头区单行、Date 之后 MIME-Version 之前（顺序稳定即提取前缀匹配成立）
    assert!(
        msg.contains("\r\nMessage-ID: <"),
        "missing Message-ID header, got: {msg:?}"
    );
    // 行以 > 收尾（RFC 5322 id-quoted 形态）
    let line = msg
        .split("\r\n")
        .find(|l| l.starts_with("Message-ID: "))
        .expect("Message-ID line");
    assert!(line.ends_with('>'), "id must be <...>, got: {line:?}");
}

#[test]
fn test_extract_message_id_roundtrip_matches_payload() {
    let msg = build_smtp_message("bot@example.com", "user@x.com", "Hi", "body");
    let mid = extract_message_id(&msg);
    assert!(mid.starts_with('<') && mid.ends_with('>'), "got: {mid:?}");
    // 回执一致性契约：提取值 == 载荷头里的值（同一条消息，不是重新生成）
    let line_mid = msg
        .split("\r\n")
        .find_map(|l| l.strip_prefix("Message-ID: "))
        .expect("header present")
        .trim();
    assert_eq!(mid, line_mid);
}

#[test]
fn test_extract_message_id_missing_header_is_empty() {
    assert_eq!(extract_message_id("From: a@x\r\n\r\nbody"), "");
    assert_eq!(extract_message_id(""), "");
}

#[test]
fn test_generate_message_id_domain_and_shape() {
    let mid = generate_message_id("bot@example.com");
    assert!(
        mid.starts_with('<') && mid.ends_with("@example.com>"),
        "got: {mid}"
    );
    // <millis.nanos@domain> 两段数字
    let inner = mid.trim_start_matches('<').trim_end_matches('>');
    let (left, domain) = inner.split_once('@').expect("domain part");
    assert_eq!(domain, "example.com");
    assert!(
        left.split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    );
    // 无 @ 的 from → 兜底域
    let mid2 = generate_message_id("opaque-sender");
    assert!(mid2.ends_with("@mailbot.local>"), "got: {mid2}");
}

#[test]
fn test_generate_message_id_unique_per_call() {
    let a = generate_message_id("a@x.com");
    let b = generate_message_id("a@x.com");
    assert_ne!(a, b, "two calls must not collide (millis.nanos)");
}
