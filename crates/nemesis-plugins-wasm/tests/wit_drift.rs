//! WIT 合同漂移测试：三份 `plugin.wit` 副本必须字节相等——
//! ① 仓库根 `wit/v0/plugin.wit`（合同单一真相源）
//! ② 宿主 `crates/nemesis-plugins-wasm/wit/plugin.wit`（bindings 生成源）
//! ③ SDK `crates/nemesis-plugin-sdk/wit/plugin.wit`（三方开发随 SDK 分发）
//!
//! 宿主与 guest 的绑定生成自不同副本；副本漂移 = 运行期实例化失败
//! （contract version mismatch / 类型不匹配），必须在编译期测试拦住。

use std::path::Path;

#[test]
fn wit_contract_copies_are_in_sync() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../wit/v0/plugin.wit");
    let host = Path::new(env!("CARGO_MANIFEST_DIR")).join("wit/plugin.wit");
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../nemesis-plugin-sdk/wit/plugin.wit");

    let read = |p: &Path| {
        std::fs::read_to_string(p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
    };
    let (root_s, host_s, sdk_s) = (read(&root), read(&host), read(&sdk));
    assert_eq!(
        host_s,
        root_s,
        "宿主 wit 副本漂移：请同步 {} → {}",
        root.display(),
        host.display()
    );
    assert_eq!(
        sdk_s,
        root_s,
        "SDK wit 副本漂移：请同步 {} → {}",
        root.display(),
        sdk.display()
    );
}
