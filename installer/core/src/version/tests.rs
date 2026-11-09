use super::*;
use std::cmp::Ordering;

#[test]
fn parse_forms() {
    assert_eq!(parse("1.2.3"), Some(vec![1, 2, 3]));
    assert_eq!(parse("v1.2.3"), Some(vec![1, 2, 3]), "tag 前缀 v 剥离");
    assert_eq!(parse(" 0.0.0.1 "), Some(vec![0, 0, 0, 1]), "空白剥离");
    assert_eq!(parse("1.2.3-beta"), Some(vec![1, 2, 3]), "数值前缀段");
    assert_eq!(parse(""), None);
    assert_eq!(parse("beta"), None);
    assert_eq!(parse("1.x.3"), None, "整段无数值 = 不可比");
}

#[test]
fn compare_matrix() {
    assert_eq!(compare("1.0.0", "1.0.0"), Some(Ordering::Equal));
    assert_eq!(
        compare("1.2.0", "1.10.0"),
        Some(Ordering::Less),
        "逐段数值比，10 > 2"
    );
    assert_eq!(compare("1.9.9", "1.10.0"), Some(Ordering::Less));
    assert_eq!(compare("2.0.0", "1.99.99"), Some(Ordering::Greater));
    assert_eq!(compare("v2.0", "2.0.0"), Some(Ordering::Equal), "段数补零");
    assert_eq!(compare("1.2", "1.2.0.0"), Some(Ordering::Equal));
    assert_eq!(compare("0.0.0.1", "0.0.1"), Some(Ordering::Less));
    // 不可比对（任一侧 None）。
    assert_eq!(compare("abc", "1.0.0"), None);
    assert_eq!(compare("1.0.0", "beta"), None);
}

#[test]
fn huge_segments_no_overflow() {
    // u64 边界段不 panic、比较正确。
    let big = u64::MAX.to_string();
    assert_eq!(compare(&big, "1"), Some(Ordering::Greater));
    assert_eq!(
        compare(&big, &format!("{big}.0")),
        Some(Ordering::Equal),
        "段数补零对齐"
    );
    assert_eq!(compare(&format!("{big}.1"), &big), Some(Ordering::Greater));
}
