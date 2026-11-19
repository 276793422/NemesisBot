use super::wildcard_match;

#[test]
fn exact_match() {
    assert!(wildcard_match("a.log", "a.log"));
    assert!(!wildcard_match("a.log", "b.log"));
    assert!(!wildcard_match("a.log", "a.logx"));
}

#[test]
fn star_matches_any_sequence_including_empty() {
    assert!(wildcard_match("*.log", "build.log"));
    assert!(wildcard_match("*.log", ".log"));
    assert!(!wildcard_match("*.log", "build.txt"));
    assert!(wildcard_match("*", "anything"));
    assert!(wildcard_match("*", ""));
    assert!(wildcard_match("a*z", "az"));
    assert!(wildcard_match("a*z", "aXYZz"));
    assert!(!wildcard_match("a*z", "aXYzZ"));
}

#[test]
fn question_matches_exactly_one_char() {
    assert!(wildcard_match("a?c", "abc"));
    assert!(!wildcard_match("a?c", "ac"));
    assert!(!wildcard_match("a?c", "abbc"));
}

#[test]
fn star_backtracks_on_later_mismatch() {
    // 经典回溯用例：第一个 * 贪婪过头后要退回重试。
    assert!(wildcard_match("*abc*", "xxabcyy"));
    assert!(wildcard_match("a*b*c", "aXbYc"));
    assert!(!wildcard_match("a*b*c", "aXbYd"));
    assert!(wildcard_match("*a*b*", "zzaabbzz"));
}

#[test]
fn unicode_chars_counted_per_char() {
    assert!(wildcard_match("?.txt", "日.txt"));
    assert!(!wildcard_match("??", "日"));
    assert!(wildcard_match("*报.log", "日报.log"));
}

#[test]
fn multiple_stars() {
    assert!(wildcard_match("*a*a*", "aaa"));
    assert!(!wildcard_match("*a*a*", "zz")); // 无 a → 不匹配
    assert!(!wildcard_match("*a*a*", "zzzz"));
    assert!(wildcard_match("*.log.*", "app.log.1"));
}
