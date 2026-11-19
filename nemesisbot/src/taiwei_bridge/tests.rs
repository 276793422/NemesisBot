//! taiwei_bridge 单测：action JSON 构造契约（与 taiwei-impl ffi::FfiAction
//! 反序列化端逐字段对齐——两侧漂移由 taiwei ffi tests + 本文件双侧钉住）。
//! 纯函数面，零 FFI 调用、零 IO。

use super::*;

fn ws() -> PathBuf {
    PathBuf::from("/home/user/.nemesisbot/workspace")
}

#[test]
fn emit_full_payload_field_alignment() {
    let action = TaiweiAction::Emit {
        kind: "exit".into(),
        install_id: Some("inst-1".into()),
        duration_secs: Some(42),
        exit_reason: Some("ctrl+c".into()),
        reason: Some("issue".into()),
        endpoint: Some("http://x:1".into()),
        workspace: Some(PathBuf::from("/tmp/ws")),
    };
    let v: serde_json::Value =
        serde_json::from_str(&build_action_json(&action, Path::new("/h"), &ws()))
            .expect("emit JSON 必须合法");
    assert_eq!(v["action"], "emit");
    assert_eq!(v["kind"], "exit");
    assert_eq!(v["install_id"], "inst-1");
    assert_eq!(v["duration_secs"], 42);
    assert_eq!(v["exit_reason"], "ctrl+c");
    assert_eq!(v["reason"], "issue");
    assert_eq!(v["endpoint"], "http://x:1");
    assert_eq!(v["workspace"], "/tmp/ws");
}

#[test]
fn emit_minimal_payload_defaults_workspace_explicit() {
    // workspace 缺省 = 宿主解析的标准 workspace（桥接侧恒传显式路径——
    // FFI 侧 home 推断兜底永远不该被触发）
    let action = TaiweiAction::Emit {
        kind: "startup".into(),
        install_id: None,
        duration_secs: None,
        exit_reason: None,
        reason: None,
        endpoint: None,
        workspace: None,
    };
    let v: serde_json::Value =
        serde_json::from_str(&build_action_json(&action, Path::new("/h"), &ws())).unwrap();
    assert_eq!(v["kind"], "startup");
    let expected_ws: String = ws().to_string_lossy().into_owned();
    assert_eq!(v["workspace"], expected_ws);
    assert!(v["install_id"].is_null());
    assert!(v["duration_secs"].is_null());
}

#[test]
fn status_payload_carries_default_workspace() {
    let action = TaiweiAction::Status { workspace: None };
    let v: serde_json::Value =
        serde_json::from_str(&build_action_json(&action, Path::new("/h"), &ws())).unwrap();
    assert_eq!(v["action"], "status");
    let expected_ws: String = ws().to_string_lossy().into_owned();
    assert_eq!(v["workspace"], expected_ws);
}

#[test]
fn switch_payloads_carry_kebab_action_tags() {
    for (action, expect) in [
        (TaiweiAction::Enable, "enable"),
        (TaiweiAction::Disable, "disable"),
        (TaiweiAction::PreciseEnable, "enable-precise"),
        (TaiweiAction::PreciseDisable, "disable-precise"),
    ] {
        let v: serde_json::Value =
            serde_json::from_str(&build_action_json(&action, Path::new("/h"), &ws()))
                .unwrap_or_else(|e| panic!("{expect} JSON 解析失败: {e}"));
        assert_eq!(v["action"], expect, "变体 {expect} 的 action tag 走样");
    }
}

#[test]
fn machine_id_ensure_has_no_workspace() {
    let action = TaiweiAction::MachineId(MachineIdCommand::Ensure);
    let v: serde_json::Value =
        serde_json::from_str(&build_action_json(&action, Path::new("/h"), &ws())).unwrap();
    assert_eq!(v["action"], "machine-id");
    assert_eq!(v["sub"], "ensure");
}

#[test]
fn machine_id_remove_passes_workspace_verbatim() {
    // remove 的 workspace 是「可选连带清理」语义——None 必须保持 null
    //（不填缺省），Some 原样透传
    let action = TaiweiAction::MachineId(MachineIdCommand::Remove { workspace: None });
    let v: serde_json::Value =
        serde_json::from_str(&build_action_json(&action, Path::new("/h"), &ws())).unwrap();
    assert_eq!(v["sub"], "remove");
    assert!(v["workspace"].is_null());

    let action = TaiweiAction::MachineId(MachineIdCommand::Remove {
        workspace: Some(PathBuf::from("/tmp/ws")),
    });
    let v: serde_json::Value =
        serde_json::from_str(&build_action_json(&action, Path::new("/h"), &ws())).unwrap();
    assert_eq!(v["workspace"], "/tmp/ws");
}

#[test]
fn abi_version_constant_matches_taiwei_contract() {
    // ABI v1：两侧常量漂移会被运行期握手咬住；此断言把「手滑改常量」
    // 提到测试面可见（taiwei 侧同值断言在其 ffi/tests.rs）。
    assert_eq!(NB_TW_ABI_VERSION, 1);
}
