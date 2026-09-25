// test_util.rs 覆盖率补充测试（catch_panic_msg 四种载荷臂 + 无 panic 时
// 的断言 panic 臂 + ensure_global_subscriber 幂等）。
//
// 豁免：无。

use super::*;

/// String 载荷 → 原样返回。
#[test]
fn catch_panic_msg_string_payload() {
    let msg = catch_panic_msg(|| -> () { panic!("{}", "string payload panic") });
    assert_eq!(msg, "string payload panic");
}

/// &str 载荷 → 转字符串返回。
#[test]
fn catch_panic_msg_str_payload() {
    let msg = catch_panic_msg(|| -> () { panic!("str payload panic") });
    assert_eq!(msg, "str payload panic");
}

/// 非字符串载荷（i32）→ 兜底文案。
#[test]
fn catch_panic_msg_non_string_payload() {
    let msg = catch_panic_msg(|| -> () {
        std::panic::panic_any(42_i32);
    });
    assert_eq!(msg, "<non-string panic payload>");
}

/// 闭包没 panic → 函数自身 panic（36 的断言臂）。
#[test]
#[should_panic(expected = "expected a panic but none happened")]
fn catch_panic_msg_no_panic_asserts() {
    let _ = catch_panic_msg(|| 1 + 1);
}

/// ensure_global_subscriber：重复调用为 no-op，不 panic。
#[test]
fn ensure_global_subscriber_is_idempotent() {
    ensure_global_subscriber();
    ensure_global_subscriber();
}
