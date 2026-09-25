// elevation.rs 覆盖率补充（wave 5）：is_elevated 只读探测（net session /
// Unix 等价实现）——不断言具体布尔值（取决于测试进程是否提权），只锁
// 「调用安全 + 返回一致」。

use super::is_elevated;

/// is_elevated 可安全调用且结果稳定（两次一致）。
#[test]
fn is_elevated_is_consistent_across_calls() {
    let a = is_elevated();
    let b = is_elevated();
    assert_eq!(a, b, "elevation state must not change mid-process");
}
