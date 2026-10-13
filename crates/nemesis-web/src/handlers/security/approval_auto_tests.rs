//! §2 免打扰（2026-10-11 无人值守计划 Part 2）：approvals.status 双臂 +
//! config.save 热翻投影 + 默认形态 pin。

use super::*;

/// 槽触碰类测试统一在锁内跑（进程级共享态串行纪律）。
fn with_slot<T>(f: impl FnOnce() -> T) -> T {
    let _serial = APPROVAL_AUTO_TEST_LOCK.lock();
    // 每例自清理：先恢复未装配基线再进断言。
    set_approval_auto_for_test(None);
    let out = f();
    set_approval_auto_for_test(None);
    out
}

#[test]
fn approvals_status_uninjected_reports_injected_false() {
    with_slot(|| {
        let v = SecurityHandler::approvals_status();
        assert_eq!(v["injected"], false, "未装配 = 降级形态诚实标注");
        assert!(v.get("enabled").is_none());
    });
}

#[test]
fn approvals_status_injected_transfers_enabled_and_count() {
    with_slot(|| {
        let state = std::sync::Arc::new(
            nemesis_security::approval_auto::ApprovalAutoApproveState::new(true),
        );
        state.bump();
        state.bump();
        install_auto_approve(state);
        let v = SecurityHandler::approvals_status();
        assert_eq!(v["injected"], true);
        assert_eq!(v["enabled"], true);
        assert_eq!(v["auto_approved_count"], 2, "计数同源透传");
    });
}

#[test]
fn config_save_hot_flips_runtime_projection() {
    with_slot(|| {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().to_str().unwrap().to_string();
        let state = std::sync::Arc::new(
            nemesis_security::approval_auto::ApprovalAutoApproveState::new(false),
        );
        install_auto_approve(state.clone());

        // save true → 投影即时翻（auditor 下一问即读新值）。
        let handler = SecurityHandler::new();
        let data = serde_json::json!({
            "enabled": true,
            "approvals": { "auto_approve": true },
        });
        handler.config_save(&workspace, &data).unwrap();
        assert!(state.enabled(), "config.save 必须热翻运行期投影");

        // save false → 翻回。
        let data_off = serde_json::json!({
            "enabled": true,
            "approvals": { "auto_approve": false },
        });
        handler.config_save(&workspace, &data_off).unwrap();
        assert!(!state.enabled(), "关闭同样热翻");
    });
}
