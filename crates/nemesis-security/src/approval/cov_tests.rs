// approval.rs 覆盖率补充测试（set_config 的 info 行 407）。
//
// 豁免：604（多开审批弹窗的超时臂）——前置是子审批窗口进程创建成功
// （spawn 失败在 557 即 Err 上抛），无头测试环境没有 plugin-ui 弹窗
// 进程，超时分支不可达。

use super::*;

/// set_config 记录新配置（407 的 tracing info 行）。
#[test]
fn set_config_updates_and_logs() {
    let mgr = MultiProcessApprovalManager::with_default_timeout();

    let mut cfg = ApprovalConfig::default();
    cfg.enabled = true;
    cfg.timeout = std::time::Duration::from_secs(45);
    cfg.min_risk_level = "HIGH".to_string();
    mgr.set_config(cfg);

    let cur = mgr.config.read().clone();
    assert_eq!(cur.timeout.as_secs(), 45);
    assert_eq!(cur.min_risk_level, "HIGH");
}
