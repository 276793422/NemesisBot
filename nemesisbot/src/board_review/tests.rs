//! Swarm M4：三态处置决策表单测（纯函数，零 I/O）。
//!
//! 评审 I/O 链路（评论/状态转移/重派）由 cluster-uat T28 端到端覆盖；
//! 此处钉死决策表的全部边界组合，防三态语义回归。

use super::*;

use nemesis_board::ReviewVerdict;

// ---------- decide_review_action ----------

#[test]
fn pass_with_auto_accept_moves_to_done() {
    assert_eq!(
        decide_review_action(ReviewVerdict::Pass, 0, 2, true),
        ReviewAction::AutoAccept
    );
}

#[test]
fn pass_without_auto_accept_stays_in_review() {
    // 默认配置（auto_accept=false）：PASS 只出意见不越权。
    assert_eq!(
        decide_review_action(ReviewVerdict::Pass, 0, 2, false),
        ReviewAction::SuggestManual
    );
}

#[test]
fn fail_redispatches_while_budget_remains() {
    // round=0（首派失败）与 round=1（一次重派后仍失败）都还剩预算。
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 0, 2, false),
        ReviewAction::Redispatch
    );
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 1, 2, false),
        ReviewAction::Redispatch
    );
}

#[test]
fn fail_at_budget_limit_escalates() {
    // round=2 >= max_redispatch=2 → 保险丝熔断，转人工。
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 2, 2, false),
        ReviewAction::EscalateHuman
    );
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 3, 2, true),
        ReviewAction::EscalateHuman
    );
}

#[test]
fn fail_with_zero_budget_never_redispatches() {
    // max_redispatch=0 = 关闭自动重派（FAIL 直转人工）。
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 0, 0, false),
        ReviewAction::EscalateHuman
    );
}

#[test]
fn unsure_always_escalates() {
    assert_eq!(
        decide_review_action(ReviewVerdict::Unsure, 0, 2, false),
        ReviewAction::EscalateHuman
    );
    assert_eq!(
        decide_review_action(ReviewVerdict::Unsure, 5, 2, true),
        ReviewAction::EscalateHuman
    );
}

// ---------- render_reasons ----------

#[test]
fn reasons_render_as_bullet_list() {
    let s = render_reasons(&["自检对照成立".to_string(), "交付物齐全".to_string()]);
    assert!(s.contains("- 自检对照成立"));
    assert!(s.contains("- 交付物齐全"));
}

#[test]
fn empty_reasons_get_honest_note() {
    let s = render_reasons(&[]);
    assert!(s.contains("未给出理由"));
}
