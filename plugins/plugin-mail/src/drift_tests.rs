//! 契约 drift 测试——插件侧副本与宿主真相源字节相等（双向复制纪律钉死）。
//!
//! 宿主真相源：`crates/nemesis-plugin/src/mail.rs`（相对路径从本文件起
//! 三级上溯到仓库根）。任何一侧单独修改都会在这里红——修改契约必须双侧
//! 同步 + bump `HOST_MAIL_ABI_VERSION`（契约文件头纪律）。

#[test]
fn contract_copy_is_byte_identical() {
    let mine = include_str!("mail.rs");
    let host = include_str!("../../../crates/nemesis-plugin/src/mail.rs");
    assert_eq!(
        mine, host,
        "契约副本与宿主真相源漂移——双向复制纪律被破坏（修改必须双侧同步）"
    );
}

#[test]
fn abi_version_matches_contract_v1() {
    // v1 = 生命周期 3 + 服务端 3 + 客户端 1 + 存储 6 + 地址 3 = 16 导出。
    assert_eq!(crate::HOST_MAIL_ABI_VERSION, 1);
}
