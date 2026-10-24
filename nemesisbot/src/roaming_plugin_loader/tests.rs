//! roaming_plugin_loader 单测（无插件环境友好：CI/exe 旁一般无
//! plugin_roaming.dll——在场断言条件化，缺席路径恒可测）。

use super::*;

#[test]
fn host_vtable_version_matches_contract() {
    // 双向闸锚点：宿主 vtable 首字段必须与契约版本常量一致——构造错位
    // 会被插件 init 侧 -2 拒绝，此处编译期形态前移到断言钉死。
    assert_eq!(HOST_VTABLE.abi_version, HOST_ROAMING_ABI_VERSION);
    assert_eq!(HOST_ROAMING_ABI_VERSION, 2);
    // 回调全部就位（冻结面：宿主半边不许 None）。
    assert!(HOST_VTABLE.log.is_some());
    assert!(HOST_VTABLE.get_workspace_dir.is_some());
    assert!(HOST_VTABLE.audit_event.is_some());
    assert!(HOST_VTABLE.notify_incoming.is_some());
    assert!(HOST_VTABLE.notify_members.is_some());
}

#[test]
fn write_cstr_truncation_and_bounds() {
    let mut buf = [0 as c_char; 8];
    assert_eq!(write_cstr(buf.as_mut_ptr(), buf.len(), "abc"), 3);
    // 恰好放下（3 字节 + NUL = 4）。
    assert_eq!(write_cstr(buf.as_mut_ptr(), 4, "abc"), 3);
    // 截断 = -1。
    assert_eq!(write_cstr(buf.as_mut_ptr(), 3, "abc"), -1);
    assert_eq!(write_cstr(buf.as_mut_ptr(), 0, "abc"), -1);
    assert_eq!(write_cstr(std::ptr::null_mut(), 8, "abc"), -1);
}

#[test]
fn load_absent_is_absent_not_error() {
    // 插件文件在 exe 旁缺席（常态）→ Absent；在场则本测退化为幂等探测
    // （重复 load → Loaded），两种环境都绿。
    match load_roaming_plugin(Path::new("."), r#"{"relay":{}}"#) {
        RoamingLoadOutcome::Absent => assert!(!roaming_plugin_present()),
        RoamingLoadOutcome::Loaded => assert!(roaming_plugin_present()),
        RoamingLoadOutcome::Rejected(e) => {
            // 在场但被拒（签名/ABI）——记录形态即可，不算测红。
            assert!(!e.is_empty());
        }
    }
}

#[test]
fn call_roaming_without_handle_is_honest_error() {
    // 未装载（或本进程从未装配）→ 诚实 Err 不 panic。若恰好在场已装载
    // 则 status 应成功返回 JSON。
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let r = rt.block_on(call_roaming("roaming_status", RoamingPluginHandle::status));
    match plugin_handle() {
        None => assert!(r.is_err(), "未装载必须诚实报错"),
        Some(_) => assert!(r.is_ok(), "已装载 status 应成功: {r:?}"),
    }
}

#[test]
fn relay_init_json_raw_passthrough_and_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.quic.json");

    // 文件缺席 = 空节（插件侧宽容解析落默认档）。
    assert_eq!(relay_init_json(&path), r#"{"relay":{}}"#);

    // relay 段原节直传（raw——typed 往返不碰）；client 段兄弟键不跟随。
    std::fs::write(
        &path,
        r#"{"relay":{"enabled":true,"http_port":4000,"token":"x"},"client":{"relay_url":"http://r"}}"#,
    )
    .unwrap();
    assert_eq!(
        relay_init_json(&path),
        r#"{"relay":{"enabled":true,"http_port":4000,"token":"x"}}"#
    );

    // 非 JSON 文件 = 空节。
    std::fs::write(&path, "not json").unwrap();
    assert_eq!(relay_init_json(&path), r#"{"relay":{}}"#);

    // relay 段非对象 = 空节。
    std::fs::write(&path, r#"{"relay":42}"#).unwrap();
    assert_eq!(relay_init_json(&path), r#"{"relay":{}}"#);
}
