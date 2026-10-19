// B2（2026-10-19 独占接管）+ A3-a（治理补全 P2 启停移 sidecar）测试。
//
// 覆盖面：三态写读往返；exempt 优先于 blocked（同名双录 = 豁免胜）；
// default = 从两表移除；三字段全空删文件不留空壳（enabled 有值必须
// 保留文件）；坏 JSON 读路径降级为空表（永不报错）；非法 connector id /
// 空工具名拒绝；多工具共存与逐个回收；enabled 往返 + effective 合并
// 语义（None = 清单缺省，Some = 用户意志胜）。

use super::*;

fn tmpws() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn default_missing_and_corrupt_are_baseline() {
    let ws = tmpws();

    // 缺文件 → 空表 → default。
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(tool_policy(&state, "mcp_s__t1"), ToolPolicy::Default);
    assert!(state.blocked.is_empty() && state.exempt.is_empty());

    // 坏 JSON → 空表（读路径永不报错，治理不因状态文件损坏放宽或崩溃）。
    let dir = state_dir(ws.path());
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("conn-a.json"), "{ not json").expect("write");
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(tool_policy(&state, "mcp_s__t1"), ToolPolicy::Default);

    // 非法 connector id（路径安全单点）→ 拒绝。
    assert!(state_path(ws.path(), "../escape").is_err());
    assert!(state_path(ws.path(), "UPPER").is_err());
    assert!(state_path(ws.path(), "").is_err());
    // set 路径同样拒绝（不接受半写的 sidecar）。
    assert!(set_tool_policy(ws.path(), "../escape", "t", ToolPolicy::Blocked).is_err());
}

#[test]
fn three_state_roundtrip() {
    let ws = tmpws();

    set_tool_policy(ws.path(), "conn-a", "mcp_s__t1", ToolPolicy::Blocked).expect("set blocked");
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(tool_policy(&state, "mcp_s__t1"), ToolPolicy::Blocked);

    set_tool_policy(ws.path(), "conn-a", "mcp_s__t2", ToolPolicy::Exempt).expect("set exempt");
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(tool_policy(&state, "mcp_s__t2"), ToolPolicy::Exempt);
    assert_eq!(
        tool_policy(&state, "mcp_s__t1"),
        ToolPolicy::Blocked,
        "互不干扰"
    );

    // 回 default：从两表移除，另一工具记录保留（文件仍在）。
    set_tool_policy(ws.path(), "conn-a", "mcp_s__t1", ToolPolicy::Default).expect("set default");
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(tool_policy(&state, "mcp_s__t1"), ToolPolicy::Default);
    assert_eq!(tool_policy(&state, "mcp_s__t2"), ToolPolicy::Exempt);
    assert!(state_path(ws.path(), "conn-a").unwrap().exists());
}

#[test]
fn exempt_wins_over_blocked_on_double_entry() {
    let ws = tmpws();
    // 先 blocked 后 exempt（set 路径本身互斥，这里手工构造双录文件——
    // 读路径判定必须稳定：豁免是更强的显式意志）。
    let dir = state_dir(ws.path());
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("conn-a.json"),
        r#"{"blocked": ["mcp_s__t1"], "exempt": ["mcp_s__t1"]}"#,
    )
    .expect("write");
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(
        tool_policy(&state, "mcp_s__t1"),
        ToolPolicy::Exempt,
        "同名双录 = 豁免胜（配置矛盾按更强的显式意志裁决）"
    );
}

#[test]
fn empty_state_deletes_sidecar_file() {
    let ws = tmpws();
    set_tool_policy(ws.path(), "conn-a", "mcp_s__t1", ToolPolicy::Blocked).expect("set");
    let path = state_path(ws.path(), "conn-a").unwrap();
    assert!(path.exists(), "有记录时文件在");

    // 归 default → 两表皆空 → 删文件不留空壳。
    set_tool_policy(ws.path(), "conn-a", "mcp_s__t1", ToolPolicy::Default).expect("unset");
    assert!(!path.exists(), "全空必须删 sidecar 文件");

    // 再归一次（文件已不在）= 幂等成功。
    set_tool_policy(ws.path(), "conn-a", "mcp_s__t1", ToolPolicy::Default).expect("idempotent");
}

#[test]
fn rejects_blank_tool_name_and_overwrites_corrupt() {
    let ws = tmpws();
    assert!(
        set_tool_policy(ws.path(), "conn-a", "   ", ToolPolicy::Blocked).is_err(),
        "空工具名拒绝"
    );

    // 坏 JSON 旧文件被整体覆盖（读按空表降级 → 写覆盖 = 修复）。
    let dir = state_dir(ws.path());
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("conn-a.json"), "garbage{{").expect("write");
    set_tool_policy(ws.path(), "conn-a", "mcp_s__t1", ToolPolicy::Exempt).expect("overwrite");
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(tool_policy(&state, "mcp_s__t1"), ToolPolicy::Exempt);
}

#[test]
fn enabled_roundtrip_and_effective_merge() {
    let ws = tmpws();

    // 无 sidecar：effective = 清单缺省。
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(state.enabled, None);
    assert!(effective_enabled(&state, true), "None → 清单缺省胜");
    assert!(!effective_enabled(&state, false));

    // 翻停用 → Some(false) 覆盖清单 true。
    set_enabled(ws.path(), "conn-a", false).expect("disable");
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(state.enabled, Some(false));
    assert!(!effective_enabled(&state, true), "sidecar 覆盖清单缺省");

    // 再启用 → Some(true)。
    set_enabled(ws.path(), "conn-a", true).expect("enable");
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(state.enabled, Some(true));
    assert!(effective_enabled(&state, false));

    // 非法 id 拒绝（路径安全单点）。
    assert!(set_enabled(ws.path(), "../escape", true).is_err());
}

#[test]
fn tool_policy_cleanup_preserves_enabled_bit() {
    let ws = tmpws();
    set_enabled(ws.path(), "conn-a", false).expect("disable");
    set_tool_policy(ws.path(), "conn-a", "mcp_s__t1", ToolPolicy::Blocked).expect("block");
    let path = state_path(ws.path(), "conn-a").unwrap();
    assert!(path.exists());

    // 例外表清空但 enabled 仍有值 → 文件必须保留（启停位是活状态）。
    set_tool_policy(ws.path(), "conn-a", "mcp_s__t1", ToolPolicy::Default).expect("unset");
    assert!(path.exists(), "enabled 有值时不能删 sidecar");
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(state.enabled, Some(false), "启停位幸存");

    // 存量 sidecar（无 enabled 键）反序列化 = None，零迁移。
    let dir = state_dir(ws.path());
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("legacy.json"),
        r#"{"blocked": ["mcp_s__t9"], "exempt": []}"#,
    )
    .expect("write legacy");
    let state = load_tool_state(ws.path(), "legacy");
    assert_eq!(state.enabled, None, "存量文件无 enabled 键 = None");
    assert_eq!(tool_policy(&state, "mcp_s__t9"), ToolPolicy::Blocked);
}
