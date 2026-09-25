//! LogFilter / ModelPricing 辅助类型覆盖率补充测试。

use crate::models::LogFilter;

/// 三维全空（None）→ is_empty = true。
#[test]
fn log_filter_all_none_is_empty() {
    let f = LogFilter::default();
    assert!(f.is_empty());
}

/// 空字符串视同未给（is_none_or 语义）。
#[test]
fn log_filter_empty_strings_are_empty() {
    let f = LogFilter {
        model: Some(String::new()),
        status: None,
        session_key: Some(String::new()),
    };
    assert!(f.is_empty());
}

/// 任一维度给出实质值 → 非空。
#[test]
fn log_filter_any_dimension_set_is_not_empty() {
    let by_model = LogFilter {
        model: Some("glm-4.7".to_string()),
        status: None,
        session_key: None,
    };
    assert!(!by_model.is_empty());

    let by_status = LogFilter {
        model: None,
        status: Some(200),
        session_key: None,
    };
    assert!(!by_status.is_empty());

    let by_session = LogFilter {
        model: None,
        status: None,
        session_key: Some("agent:main:s1".to_string()),
    };
    assert!(!by_session.is_empty());
}
