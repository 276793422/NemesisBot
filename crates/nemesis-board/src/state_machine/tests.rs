//! 状态机转移表测试（§1.1 全覆盖）。

use super::*;
use crate::models::IssueStatus::*;

#[test]
fn test_valid_transitions_from_table() {
    // (from, 合法目标集)——逐条对照 §1.1 转移表。
    let table: &[(IssueStatus, &[IssueStatus])] = &[
        (Backlog, &[Todo, InProgress, Done, Blocked, Cancelled]),
        (Todo, &[InProgress, Done, Blocked, Cancelled]),
        (InProgress, &[InReview, Done, Blocked, Cancelled]),
        (InReview, &[InProgress, Done, Blocked, Cancelled]),
        (Blocked, &[Todo, InProgress, Cancelled]),
    ];
    for (from, targets) in table {
        for to in *targets {
            assert!(can_transition(*from, *to), "expected legal: {from} → {to}");
            assert!(
                validate_transition(*from, *to).is_ok(),
                "validate_transition should accept {from} → {to}"
            );
        }
    }
}

#[test]
fn test_invalid_transitions_rejected() {
    // 逐条挑几个跨级跳转：backlog→in_review、todo→in_review、blocked→in_review、
    // blocked→done、in_progress→todo（不走 in_review 回退）。
    let illegal = [
        (Backlog, InReview),
        (Todo, InReview),
        (Blocked, InReview),
        (Blocked, Done),
        (InProgress, Todo),
        (InReview, Todo),
    ];
    for (from, to) in illegal {
        assert!(!can_transition(from, to), "expected illegal: {from} → {to}");
        let err = validate_transition(from, to).unwrap_err();
        assert!(
            err.contains("非法状态转移"),
            "error should be descriptive: {err}"
        );
    }
}

#[test]
fn test_self_transition_rejected() {
    let all = [
        Backlog, Todo, InProgress, InReview, Done, Blocked, Cancelled,
    ];
    for s in all {
        assert!(
            !can_transition(s, s),
            "self-transition must be illegal: {s}"
        );
        let err = validate_transition(s, s).unwrap_err();
        assert!(err.contains("已处于"), "self-transition error: {err}");
    }
}

#[test]
fn test_terminal_states_are_absorbing() {
    let all = [
        Backlog, Todo, InProgress, InReview, Done, Blocked, Cancelled,
    ];
    for s in all {
        assert!(!can_transition(Done, s), "done must be terminal (→ {s})");
    }
    // cancelled 唯一终态出口 = backlog（发现④ reopen，2026-09-15）；其余
    // 目标全部非法。
    for s in all {
        if s == Backlog {
            assert!(
                can_transition(Cancelled, s),
                "cancelled → backlog (reopen) must be legal"
            );
        } else {
            assert!(
                !can_transition(Cancelled, s),
                "cancelled only exits to backlog (→ {s})"
            );
        }
    }
    assert!(Done.is_terminal());
    assert!(Cancelled.is_terminal());
    assert!(!Backlog.is_terminal());
}

#[test]
fn test_validate_transition_error_lists_targets() {
    // backlog 不能直接 in_review；错误信息应列出 backlog 的合法目标集。
    let err = validate_transition(Backlog, InReview).unwrap_err();
    assert!(
        err.contains("todo/in_progress/done/blocked/cancelled"),
        "{err}"
    );
}

#[test]
fn test_cancelled_reopen_exit_and_targets_string() {
    // cancelled 的对外可读目标集要指向 reopen 通路（发现④）。
    assert!(
        Cancelled.allowed_targets().contains("backlog"),
        "cancelled allowed_targets must mention reopen: {}",
        Cancelled.allowed_targets()
    );
    assert!(Done.allowed_targets().contains("终态"));
    // validate 路径同样放行 cancelled → backlog。
    assert!(validate_transition(Cancelled, Backlog).is_ok());
}
