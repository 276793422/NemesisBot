//! 隐藏验收：next_multiple_of / prev_multiple_of 在 rhs = -1、lhs = MIN 的
//! 极端组合下不得溢出 panic（debug 构建下崩溃即失败），结果必须自洽。
extern crate num_integer;
use num_integer::Integer;

#[test]
fn multiple_of_neg_one_does_not_overflow() {
    let min = i64::MIN;
    assert_eq!(
        min.next_multiple_of(&-1),
        min,
        "i64::MIN.next_multiple_of(-1) must be i64::MIN (MIN is a multiple of -1)"
    );
    let min32 = i32::MIN;
    assert_eq!(min32.next_multiple_of(&-1), min32);
    assert_eq!(5i64.next_multiple_of(&-1), 5, "x.next_multiple_of(-1) must equal x");
    assert_eq!(5i32.next_multiple_of(&-1), 5);

    assert_eq!(min.prev_multiple_of(&-1), min);
    assert_eq!(min32.prev_multiple_of(&-1), min32);
    assert_eq!(5i64.prev_multiple_of(&-1), 5);
    assert_eq!(5i32.prev_multiple_of(&-1), 5);
}
