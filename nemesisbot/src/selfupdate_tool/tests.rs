//! selfupdate 工具面轻测试：清单形态 + apply 未装配诚实报错路径（离线，
//! 不触网——check 的 execute 直连 Release API，只测纯面；apply 的真编排
//! 在 nemesis-web selfupdate 套件覆盖）。

use super::{APPLY_TOOL_NAME, CHECK_TOOL_NAME, SelfupdateApplyTool, SelfupdateCheckTool};
use nemesis_agent::r#loop::Tool;

#[test]
fn tool_names_are_stable() {
    // 注册键 = LLM 可见名；改名会破既有会话的调用习惯 + 操作类型声明表。
    assert_eq!(CHECK_TOOL_NAME, "selfupdate_check");
    assert_eq!(APPLY_TOOL_NAME, "selfupdate_apply");
}

#[test]
fn manifests_nonempty_and_readonly_flag() {
    let check = SelfupdateCheckTool;
    assert!(!check.description().is_empty());
    assert!(check.parameters().is_object());
    // 只读检查：进 U5 并行读池（fail-closed 默认的例外须显式声明）。
    assert!(check.is_read_only());

    let apply = SelfupdateApplyTool;
    assert!(!apply.description().is_empty());
    assert!(apply.parameters().is_object());
    // apply 有真实副作用（重启面）——绝不进并行读池。
    assert!(!apply.is_read_only());
}

#[tokio::test]
async fn apply_without_manager_errors_honestly() {
    // 测试进程不跑 gateway 装配 → manager 槽必空 → 诚实报「未装配」，
    // 不 panic、不假成功。真编排路径（受理/闸门/审批卡）归 nemesis-web
    // selfupdate 套件与本文件外的 gateway 集成面。
    let apply = SelfupdateApplyTool;
    for args in ["{}", r#"{"allow_downgrade":true}"#] {
        let err = apply
            .execute(args, &default_ctx())
            .await
            .expect_err("未装配必须报错");
        assert!(err.contains("未装配"), "{err}");
    }
}

/// 最小 RequestContext（工具 execute 不消费上下文，仅签名需要）。
fn default_ctx() -> nemesis_agent::context::RequestContext {
    nemesis_agent::context::RequestContext::new("test", "c-test", "s-test", "test-session")
}
