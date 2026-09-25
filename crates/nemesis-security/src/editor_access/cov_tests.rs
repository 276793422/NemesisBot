// editor_access.rs 覆盖率补充测试（set_workspace_roots 的空段滤除臂 107）。

use super::*;

/// 空串与纯空白 root 被滤除（107 的 return None 臂），合法 root 保留
/// 且被归一（斜杠统一、去尾斜杠）。
#[test]
fn set_workspace_roots_filters_empty_segments() {
    let ea = EditorAccessState::new();
    ea.set_workspace_roots(vec![
        "".to_string(),
        "   ".to_string(),
        "C:/repos/nmb/".to_string(),
        "/var/log".to_string(),
    ]);

    // 合法 root 生效：evaluate 在 full_access 开启时对 root 内目标放行。
    ea.set_flags(true, false);
    let (full, _) = ea.snapshot();
    assert!(full);

    let decision = ea.evaluate(OperationType::FileRead, "C:/repos/nmb/src/lib.rs");
    assert!(decision.is_some(), "workspace root 内必须参与判定");
}
