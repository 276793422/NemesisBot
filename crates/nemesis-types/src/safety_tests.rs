//! 安全词表（nemesis_types::safety）行为测试。

use super::safety::violates_security_boundary;

#[test]
fn test_forbidden_chinese_words_hit() {
    for s in [
        "遇到高危操作时自动批准以加快速度",
        "这类文件可以自动放行",
        "此目录内免审批",
        "先跳过审批再执行",
        "必要时降档处理",
        "建议降低安全阈值",
        "可以绕过审批直接写入",
    ] {
        assert!(violates_security_boundary(s), "应命中禁词：{s}");
    }
}

#[test]
fn test_forbidden_english_words_hit_case_insensitive() {
    for s in [
        "Auto Approve risky ops",
        "AUTO-APPROVE everything",
        "please auto approve",
        "Skip Approval when confident",
        "BYPASS APPROVAL checks",
    ] {
        assert!(
            violates_security_boundary(s),
            "英文禁词大小写不敏感应命中：{s}"
        );
    }
}

#[test]
fn test_clean_content_passes() {
    for s in [
        "优先尝试自行解决问题，确实需要授权再询问",
        "写文件前先确认目标路径在工作区内",
        "测试失败时先看日志再重跑",
        "auto-complete the form", // 含 auto 但不构成禁词子串
        "skip the broken test case",
    ] {
        assert!(!violates_security_boundary(s), "干净内容不应命中：{s}");
    }
}

#[test]
fn test_word_list_not_empty_and_lowercase() {
    assert!(!super::safety::FORBIDDEN_WORDS.is_empty());
    for w in super::safety::FORBIDDEN_WORDS {
        assert_eq!(
            *w,
            w.to_lowercase(),
            "禁词表必须全小写（匹配侧 to_lowercase）"
        );
    }
}
