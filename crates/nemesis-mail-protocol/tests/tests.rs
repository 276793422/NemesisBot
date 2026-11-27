//! Message-ID 面（QQ/163 等公共邮箱反垃圾评分项）：生成 / 写头 / 提取三函数
//! 的契约——单一真相源（build_smtp_message 内部生成 + extract_message_id
//! 回执提取）保证「回执 id == 载荷头 id」，二次调用值不同 = 回执造假。

use nemesis_mail_protocol::{
    build_smtp_message, extract_message_id, generate_message_id, parse_email_body,
    prepare_smtp_payload,
};

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

// ---- BUG-20261120-039 回归锁：域名净化（显示名形态 + CR/LF 注入面） ----

#[test]
fn test_generate_message_id_display_name_form() {
    // 显示名形态：域名取尖括号内地址，不得产出 "com>" 畸形域
    let mid = generate_message_id("John Doe <john@example.com>");
    assert!(mid.ends_with("@example.com>"), "got: {mid}");
    assert_eq!(
        mid.matches('>').count(),
        1,
        "single closing bracket, got: {mid}"
    );
}

#[test]
fn test_generate_message_id_strips_crlf_header_injection() {
    // 纵深防御：from 混入 CR/LF 不得进 Message-ID（头断行注入面——
    // 对齐 build_smtp_message 对 From/To/Subject 的 header_safe 同族）。
    // 攻击串注入段不得含 '@'（含 '@' 时 rsplit_once 取到注入段的 '@'，
    // CRLF 落入被丢弃的 local part——旧代码也安全，测试失去区分度）。
    let mid = generate_message_id("user@qq.com\r\nX-Inject: pwned");
    assert!(!mid.contains('\r') && !mid.contains('\n'), "got: {mid:?}");
    // 端到端：build_smtp_message 头区每行都是已知字段前缀（注入的
    // "X-Inject: pwned" 断行头会同时打破行数与前缀断言）
    let msg = build_smtp_message("user@qq.com\r\nX-Inject: pwned", "to@x.com", "s", "b");
    let header_zone = msg.split("\r\n\r\n").next().unwrap_or("");
    let header_lines: Vec<&str> = header_zone.split("\r\n").collect();
    assert_eq!(header_lines.len(), 7, "header lines: {header_lines:?}");
    let known = [
        "From: ",
        "To: ",
        "Subject: ",
        "Date: ",
        "Message-ID: ",
        "MIME-Version: ",
        "Content-Type: ",
    ];
    for l in &header_lines {
        assert!(
            known.iter().any(|p| l.starts_with(p)),
            "unexpected header line: {l:?}"
        );
    }
}

// ---- BUG-20261120-040 回归锁：信封终结符只剥末行 ----

#[test]
fn test_parse_email_body_keeps_paren_line_in_content() {
    // 正文含 ")" 独立行：只剥末行信封终结符，正文行保留
    let responses = vec![
        "* 1 FETCH (BODY[TEXT] {6}".to_string(),
        ")".to_string(),
        "text".to_string(),
        ")".to_string(),
    ];
    let body = parse_email_body(&responses, "");
    assert_eq!(body, ")\ntext");
}

#[test]
fn test_parse_email_body_no_trailing_paren_envelope() {
    // 无信封 ")"（服务器省略形态）：末行是正文则不剥
    let responses = vec!["* 1 FETCH (BODY[TEXT] {3}".to_string(), "abc".to_string()];
    let body = parse_email_body(&responses, "");
    assert_eq!(body, "abc");
}

// ---- BUG-20261123-034 回归锁：DATA 载荷点填充覆盖首行行首点 ----

#[test]
fn test_prepare_smtp_payload_stuffs_first_line_leading_dot() {
    // 首行恰为单独 "."：不被填充时服务端当 DATA 终止符、余下字节当命令
    let out = prepare_smtp_payload(".\r\nQUIT\r\n");
    assert!(
        out.starts_with(".."),
        "first line must be stuffed, got: {out:?}"
    );
    // 首行以点开头的非终止形态同样补一个前导点
    let out = prepare_smtp_payload(".hidden\r\nbody");
    assert!(out.starts_with("..hidden"), "got: {out:?}");
}

#[test]
fn test_prepare_smtp_payload_stuffs_leading_dot_after_crlf_normalization() {
    // 裸 LF 形态的首行点：先归一成 CRLF 再判首字节——两条路径都要命中
    let out = prepare_smtp_payload(".\nbody");
    assert!(out.starts_with(".."), "got: {out:?}");
    let out = prepare_smtp_payload(".body");
    assert!(out.starts_with("..body"), "got: {out:?}");
}

#[test]
fn test_prepare_smtp_payload_stuffs_subsequent_dot_lines() {
    // 既有契约（BUG-20261112-111③）：后续行行首点填充不回退
    let out = prepare_smtp_payload("From: a@b\r\n\r\nline1\r\n.dot\r\n");
    assert!(out.contains("\r\n..dot\r\n"), "got: {out:?}");
    assert!(
        out.starts_with("From: a@b"),
        "header first line untouched, got: {out:?}"
    );
}

#[test]
fn test_prepare_smtp_payload_normal_eol_normalization_unchanged() {
    // 既有归一契约：裸 LF/CR → CRLF
    let out = prepare_smtp_payload("a\nb\rc\r\n.d");
    assert_eq!(out, "a\r\nb\r\nc\r\n..d");
}
