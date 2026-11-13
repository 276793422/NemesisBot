//! [`crate::url_enc`] 共享编码助手的单测。
//! 断言与既有 modelscope/clawhub 私有实现的历史行为逐字节一致（delegate
//! 后三方行为不得漂移）。

use crate::url_enc::{percent_encode_component, percent_encode_path};

#[test]
fn component_unreserved_passthrough() {
    assert_eq!(percent_encode_component("PantherAng"), "PantherAng");
    assert_eq!(
        percent_encode_component("zhima_credit_tech"),
        "zhima_credit_tech"
    );
    assert_eq!(percent_encode_component("a.b-c_d~e"), "a.b-c_d~e");
}

#[test]
fn component_encodes_delimiters_and_reserved() {
    assert_eq!(percent_encode_component("@anthropics"), "%40anthropics");
    assert_eq!(percent_encode_component("a b/c"), "a%20b%2Fc");
    // `#`/`?` 是 BUG-20261113-162 的核心：不编码即沦为 fragment/query 分隔符
    assert_eq!(percent_encode_component("a#b"), "a%23b");
    assert_eq!(percent_encode_component("a?b"), "a%3Fb");
    assert_eq!(percent_encode_component("100%off"), "100%25off");
}

#[test]
fn component_non_ascii_is_utf8_bytes() {
    assert_eq!(percent_encode_component("支"), "%E6%94%AF");
}

#[test]
fn path_preserves_separator_encodes_segments() {
    assert_eq!(
        percent_encode_path("skills/pdf/SKILL.md"),
        "skills/pdf/SKILL.md"
    );
    assert_eq!(
        percent_encode_path("docs/my skill.md"),
        "docs/my%20skill.md"
    );
    assert_eq!(percent_encode_path("a#b/c?d.md"), "a%23b/c%3Fd.md");
    // 空段保留（空串输入原样返回）
    assert_eq!(percent_encode_path(""), "");
}
