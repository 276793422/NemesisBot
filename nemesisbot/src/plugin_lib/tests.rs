//! [`super`] 单测：Unsigned 签名闸口径 + find_path 钉死（泛化真相源搬家）。

use super::*;

/// Unsigned 闸口径必须跟 debug_assertions：dev 跑与 release CI 各验半边，
/// 有人把闸改成常数（恒真/恒假）即红半边。
#[test]
fn unsigned_verdict_gate_follows_debug_assertions() {
    assert_eq!(
        unsigned_dev_allow(),
        cfg!(debug_assertions),
        "Unsigned 闸口径漂移：应为 dev（debug_assertions）放行 / release 拒"
    );
}

/// find_path：正常 cwd 下（无 exe 旁插件）= None，不 panic；真装配
/// 面由 e2e/构建脚本覆盖。两个 spec 各验半边（文件名分派不炸）。
#[test]
fn find_path_absent_is_none_or_found() {
    let _ = PluginLibSpec::VM.find_path();
    let _ = PluginLibSpec::ROAMING.find_path();
}
