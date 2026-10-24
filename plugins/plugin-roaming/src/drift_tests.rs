//! 契约漂移闸（QUIC 插件化 M1）：
//! 1. 插件内 `roaming.rs` 副本必须与 nemesis-plugin 契约文件**逐字节相等**
//!    （ABI 真相源在 crates/nemesis-plugin/src/roaming.rs，宿主与插件各自
//!    持有副本——漂移即 ABI 分叉，build 必红）。
//! 2. 名册常量与宿主 rendezvous 同值（容量闸 / TTL 是跨端协作语义——一边
//!    改大一边改小 = 名册行为漂移）。

#[test]
fn roaming_contract_copy_is_byte_identical() {
    let contract = include_str!("../../../crates/nemesis-plugin/src/roaming.rs");
    let copy = include_str!("roaming.rs");
    assert_eq!(
        copy, contract,
        "契约副本漂移：以 crates/nemesis-plugin/src/roaming.rs 为真相源同步（cp 覆盖）"
    );
}

#[test]
fn member_registry_constants_match_host_literals() {
    // 名册常量与宿主 rendezvous.rs 同值纪律（容量闸 / TTL / via-relay 哨兵
    // 是跨端协作语义——一边改大一边改小 = 名册行为漂移）。宿主值以字面量
    // 钉死（M3 删 dev-dep 后无宿主类型可 import；宿主改动时本测试红字面
    // 提醒双侧同步）。
    assert_eq!(
        crate::types::MAX_MEMBERS,
        256usize,
        "名册容量闸与宿主不同值"
    );
    assert_eq!(
        crate::types::MEMBER_TTL_SECS,
        120u64,
        "成员 TTL 与宿主不同值"
    );
    assert_eq!(
        crate::types::VIA_RELAY_TAG,
        "via-relay",
        "via-relay 哨兵 tag 与宿主不同值"
    );
}
