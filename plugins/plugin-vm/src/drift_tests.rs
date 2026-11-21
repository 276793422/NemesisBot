//! 契约 drift 测试——插件侧副本与宿主真相源字节相等（lib.rs 头注明的
//! 「字节一致纪律」的自动化守卫）。
//!
//! 宿主真相源：`crates/nemesis-plugin/src/vm_display.rs`（相对路径从本文件
//! 起三级上溯到仓库根）。任何一侧单独修改都会在这里红——修改契约必须双侧
//! 同步 + bump `HOST_VM_DISPLAY_ABI_VERSION`（双向闸两侧共同锚定的常量）。
//!
//! 守卫缺失的教训（BUG-20261121-002）：真相源 doc 曾被单独改坏（错误码
//! 负号被剥成 `（1）（2）（3）（4）` 与自身 `-1` 常量矛盾、`std::os:raw`
//! typo），插件副本侧保留正确版本——「手工同步」纪律已静默失效过一次；
//! 同族 plugin-mail/mail.rs 早有同款测试且全程有效（旁证该形态可行）。

#[test]
fn vm_display_copy_is_byte_identical() {
    let mine = include_str!("vm_display.rs");
    let host = include_str!("../../../crates/nemesis-plugin/src/vm_display.rs");
    assert_eq!(
        mine, host,
        "vm_display.rs 副本与宿主真相源漂移——字节一致纪律被破坏（修改必须双侧同步）"
    );
}

#[test]
fn vm_display_abi_version_pinned() {
    // 双向闸两侧共同锚定的版本常量；意外 bump 在这里红（bump 必须双侧
    // 同步 + 双向闸语义评审，不是顺手改一个数字）。
    assert_eq!(crate::vm_display::HOST_VM_DISPLAY_ABI_VERSION, 1);
}
