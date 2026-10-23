//! call_fn 参数编码的契约测试（从 runtime.rs 迁出，见 scripts/check-inline-tests.sh 纪律）。

use super::{callfn_args_from_slice, parse_callfn_args};

#[test]
fn callfn_args_slice_contract() {
    assert_eq!(
        callfn_args_from_slice(&[serde_json::json!(1), serde_json::json!(2)]).unwrap(),
        [1, 2, 0, 0]
    );
    // 负数按补码（指针宽度语义）
    assert_eq!(
        callfn_args_from_slice(&[serde_json::json!(-1)]).unwrap(),
        [u64::MAX, 0, 0, 0]
    );
    // 浮点/字符串 loud 拒绝
    assert!(callfn_args_from_slice(&[serde_json::json!(1.5)]).is_err());
    assert!(callfn_args_from_slice(&[serde_json::json!("x")]).is_err());
    // 超 4 拒绝
    assert!(
        callfn_args_from_slice(&[
            serde_json::json!(1),
            serde_json::json!(2),
            serde_json::json!(3),
            serde_json::json!(4),
            serde_json::json!(5)
        ])
        .is_err()
    );
}

#[test]
fn callfn_args_tool_json() {
    assert_eq!(parse_callfn_args(r#"{"args":[9]}"#).unwrap(), [9, 0, 0, 0]);
    assert_eq!(parse_callfn_args("{}").unwrap(), [0, 0, 0, 0]);
    assert!(parse_callfn_args("not json").is_err());
    assert!(parse_callfn_args(r#"{"args":"x"}"#).is_err());
}
