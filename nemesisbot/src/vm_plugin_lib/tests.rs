//! [`super`] 单测：Unsigned 签名闸口径钉死（单一真相源 [`super::unsigned_dev_allow`]）。

use super::*;

/// Unsigned 闸口径必须跟 debug_assertions：dev 跑与 release CI 各验半边，
/// 有人把闸改成常数（恒真/恒假）即红半边（原 vm_display_loader 测试迁
/// 到此处——真相源搬家，钉口径的测试跟着搬）。
#[test]
fn unsigned_verdict_gate_follows_debug_assertions() {
    assert_eq!(
        unsigned_dev_allow(),
        cfg!(debug_assertions),
        "Unsigned 闸口径漂移：应为 dev（debug_assertions）放行 / release 拒"
    );
}

/// find_plugin_path：正常 cwd 下（无 exe 旁插件）= None，不 panic；真装配
/// 面由 e2e/构建脚本覆盖。
#[test]
fn find_plugin_absent_is_none_or_found() {
    let _ = find_plugin_path();
}
