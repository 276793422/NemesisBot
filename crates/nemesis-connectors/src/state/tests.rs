// （独占接管）+ -a（治理补全 启停移 sidecar）测试。
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

/// 撤销防重标记（边界补强）：mark → is roundtrip；无标记 =
/// false；形态异常的凭条 id 不落盘也不误报。
#[test]
fn undone_marker_roundtrip_and_shape_guard() {
    let ws = tmpws();
    let ws = ws.path();

    assert!(!is_undone(ws, "RCPT-1-0001"), "无标记 = 未撤销");
    mark_undone(ws, "RCPT-1-0001", Some("RCPT-1-0002"));
    assert!(is_undone(ws, "RCPT-1-0001"), "标记后 = 已撤销");
    // 标记文件内容带审计字段（撤销凭条 id + 时间戳）。
    let body = std::fs::read_to_string(state_dir(ws).join("_undone").join("RCPT-1-0001.json"))
        .expect("marker file");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["undo_receipt_id"], "RCPT-1-0002");
    assert!(v["marked_at_ms"].is_i64());

    // 形态异常（路径分隔符 /.. / 超长 / 空串）：不落盘、不误报、不 panic。
    for bad in ["../escape", "a/b", "..", ".", "", &"x".repeat(129)] {
        mark_undone(ws, bad, None);
        assert!(!is_undone(ws, bad), "异常形态不落盘: {bad:?}");
    }
    // 目录下只有合法标记文件。
    let entries: Vec<_> = std::fs::read_dir(state_dir(ws).join("_undone"))
        .expect("undone dir")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "只落了合法标记");
}

// BUG-20261120-025 横向同族回归锁：记事本「带 BOM 的 UTF-8」保存的
// sidecar——旧形态裸 read_to_string 的前导 \u{FEFF} 会让 parse 失败走空表
// 降级臂 = 停用位意志静默丢失（本函数注释点名的「意志丢失风险」被 BOM
// 直接触发）。read_config_string 剥 BOM 后停用位必须照常生效。
#[test]
fn bom_prefixed_sidecar_still_applies_policy() {
    let ws = tmpws();
    set_tool_policy(ws.path(), "conn-a", "mcp_s__t1", ToolPolicy::Blocked).expect("set");
    let path = state_dir(ws.path()).join("conn-a.json");
    let plain = std::fs::read_to_string(&path).expect("read sidecar");
    std::fs::write(&path, format!("\u{feff}{plain}")).expect("rewrite with BOM");
    let state = load_tool_state(ws.path(), "conn-a");
    assert_eq!(
        tool_policy(&state, "mcp_s__t1"),
        ToolPolicy::Blocked,
        "BOM 前缀 sidecar 的停用位不得静默回落基线"
    );
}

// BUG-20261125-008 回归锁：`[]` 文档曾从 serde visit_seq 零元素臂溜过 =
// 静默 Default（停用位/例外表清空，与坏 JSON 降级同向但**零诊断**——文件
// 明明在场却完全不可见地丢失意志）。修复 = ensure_object 前置，违形进既
// 有「解析失败按空表降级」WARN 臂。WARN 捕获（自制最小 Subscriber；不引
// tracing-subscriber dev-dep）。set_default 是 thread-local，并行互不污染。
mod warn_capture {
    use std::sync::{Arc, Mutex};

    struct EventText(Arc<Mutex<Vec<String>>>);

    struct MsgVisitor(String);
    impl tracing::field::Visit for MsgVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                if !self.0.is_empty() {
                    self.0.push(' ');
                }
                self.0.push_str(&format!("{value:?}"));
            } else {
                self.0.push_str(&format!(" {}={value:?}", field.name()));
            }
        }
    }

    impl tracing::Subscriber for EventText {
        fn enabled(&self, _meta: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(1)
        }
        fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            let mut v = MsgVisitor(String::new());
            event.record(&mut v);
            self.0
                .lock()
                .unwrap()
                .push(format!("[{}] {}", event.metadata().level(), v.0));
        }
        fn enter(&self, _span: &tracing::Id) {}
        fn exit(&self, _span: &tracing::Id) {}
    }

    /// thread-local 装捕获 subscriber；返回 (guard, 缓冲)，guard 持有至断言完。
    pub fn capture() -> (tracing::subscriber::DefaultGuard, Arc<Mutex<Vec<String>>>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let guard = tracing::subscriber::set_default(EventText(buf.clone()));
        (guard, buf)
    }
}

#[test]
fn array_document_sidecar_warns_and_falls_back_baseline() {
    let ws = tmpws();
    let dir = state_dir(ws.path());
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("conn-arr.json"), "[]").expect("write");
    let (_guard, buf) = warn_capture::capture();
    let state = load_tool_state(ws.path(), "conn-arr");
    assert_eq!(
        tool_policy(&state, "mcp_s__t1"),
        ToolPolicy::Default,
        "`[]` 零条目 → 基线（读路径永不报错语义保持）"
    );
    let logs = buf.lock().unwrap().join("\n");
    assert!(
        logs.contains("must be a JSON object"),
        "违形必须 WARN（此前零诊断）：logs={logs}"
    );
    assert!(logs.contains("array"), "形状必须可见：logs={logs}");
}
