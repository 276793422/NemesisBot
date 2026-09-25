// resolver.rs 覆盖率补充测试（is_reserved 的 IPv4 组播臂 222 + 分类函数
// 行为锁定）。
//
// 豁免（死防御臂，仅记录不硬凑）：
// - 72（ControlChars）：tab/\r/\n 被 url crate 按 URL 规范在解析前剥除
//   （见下方行为锁定测试），其余控制字符在 url crate 内部即报
//   InvalidDomainCharacter——交不到 contains_control_chars 手里。
// - 76（InvalidHost 的 '@'/'[' 形态）：EmbeddedCredentials 检查（69-74）
//   先于本臂——'@' 早已被 username 分流；"["/"]" 被 url crate 拒绝。
// - 83（InvalidPort）：port 已是 u16，to_string().parse::<u16>() 恒 Ok，
//   条件永假。
// - 264（NoAddresses）：tokio lookup_host 失败走 Err→DnsFailed；Ok 但空
//   迭代在系统解析器上无已知触发形态。

use super::*;

/// 组播（224.0.0.0/4）与广播必须判保留（222 的 `octets[0] >= 224` 臂）。
#[test]
fn reserved_ip_covers_multicast_and_broadcast() {
    assert!(is_reserved_ip(&"224.0.0.1".parse::<IpAddr>().unwrap()));
    assert!(is_reserved_ip(
        &"239.255.255.250".parse::<IpAddr>().unwrap()
    ));
    assert!(is_reserved_ip(
        &"255.255.255.255".parse::<IpAddr>().unwrap()
    ));
    // 对照：公网地址不误伤。
    assert!(!is_reserved_ip(&"1.1.1.1".parse::<IpAddr>().unwrap()));
    assert!(!is_reserved_ip(&"8.8.8.8".parse::<IpAddr>().unwrap()));
}

/// 分类函数行为锁定：私网/环回/link-local 判定与 parse_url 基本形态。
#[test]
fn classification_and_parse_url_behavior_locks() {
    assert!(is_private_ip(&"10.1.2.3".parse::<IpAddr>().unwrap()));
    assert!(is_private_ip(&"192.168.1.1".parse::<IpAddr>().unwrap()));
    assert!(!is_private_ip(&"8.8.4.4".parse::<IpAddr>().unwrap()));
    assert!(is_loopback_ip(&"127.0.0.1".parse::<IpAddr>().unwrap()));
    assert!(is_link_local_ip(&"169.254.1.1".parse::<IpAddr>().unwrap()));

    // parse_url 正常/异常形态（控制字符与裸 '@' 形态在上面的豁免里论证：
    // 它们到不了对应臂，但整体仍须被拒）。
    let ok = parse_url("http://example.com:8080/path").unwrap();
    assert_eq!(ok.scheme, "http");
    assert_eq!(ok.host, "example.com");
    assert_eq!(ok.port, Some(8080));

    // 无 scheme 自动补 http。
    let bare = parse_url("example.com").unwrap();
    assert_eq!(bare.scheme, "http");

    // 凭据内嵌 → EmbeddedCredentials（76 的前置臂）。
    assert!(parse_url("http://user:pw@example.com").is_err());

    // Tab 会被 url crate 按 URL 规范在解析前剥除（host 变回 example.com，
    // 解析成功）——这正是 72 臂死因的一半；其余控制字符（如 \u{1}）由
    // url crate 直接报 InvalidDomainCharacter（Err），同样到不了 72。
    let tab = parse_url("http://exa\tmple.com").unwrap();
    assert_eq!(tab.host, "example.com", "url crate 必须剥除 tab");
    assert!(parse_url("http://exa\u{1}mple.com").is_err());
}
