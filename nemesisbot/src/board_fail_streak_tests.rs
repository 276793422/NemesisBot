//! M3 同因熔断账本测试：签名稳定性 / 阈值三连跳 / 签名翻转清零 / 持久化
//! 跨实例记账 / 清账复位。

use super::*;

fn anchor(raw: &str) -> nemesis_board::AnchorCheck {
    nemesis_board::AnchorCheck {
        raw: raw.to_string(),
        kind: nemesis_board::AnchorKind::FileExists,
        target: "rel/path.txt".to_string(),
        pattern: None,
    }
}

fn result(raw: &str, passed: bool) -> nemesis_board::AnchorResult {
    nemesis_board::AnchorResult {
        anchor: anchor(raw),
        passed,
        detail: if passed {
            "存在".into()
        } else {
            "缺失".into()
        },
    }
}

fn temp_ws(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("nb-fail-streak-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 签名 = 排序失败锚点 raw 集合 join（顺序无关）；全过/无锚点 =
/// fail:no-anchors。
#[test]
fn signature_sorts_failed_anchors_and_falls_back() {
    let results = vec![
        result("b.txt", true),
        result("a.txt", false),
        result("c.txt", false),
    ];
    assert_eq!(
        failure_signature(&results),
        "a.txt|c.txt",
        "失败集排序后 join"
    );

    let all_pass = vec![result("a.txt", true), result("b.txt", true)];
    assert_eq!(failure_signature(&all_pass), "fail:no-anchors");
    assert_eq!(failure_signature(&[]), "fail:no-anchors");
}

/// 同签名：第 1、2 次 Continue，第 3 次 Tripped（阈值 = FAIL_STREAK_THRESHOLD）。
#[test]
fn same_signature_trips_at_threshold() {
    let ws = temp_ws("trip");
    let mut ledger = FailStreakLedger::load(&ws);
    assert_eq!(
        ledger.record_and_check(7, "sig-a"),
        StreakOutcome::Continue(1)
    );
    assert_eq!(
        ledger.record_and_check(7, "sig-a"),
        StreakOutcome::Continue(2)
    );
    assert_eq!(
        ledger.record_and_check(7, "sig-a"),
        StreakOutcome::Tripped(3)
    );
    assert_eq!(FAIL_STREAK_THRESHOLD, 3);
    let _ = std::fs::remove_dir_all(&ws);
}

/// 签名翻转：计数清零重计（worker 换了失败形态不算旧账）。
#[test]
fn signature_flip_resets_counter() {
    let ws = temp_ws("flip");
    let mut ledger = FailStreakLedger::load(&ws);
    let _ = ledger.record_and_check(7, "sig-a");
    let _ = ledger.record_and_check(7, "sig-a");
    // 翻转 → 从 1 重新起算，不 Tripped。
    assert_eq!(
        ledger.record_and_check(7, "sig-b"),
        StreakOutcome::Continue(1)
    );
    assert_eq!(
        ledger.record_and_check(7, "sig-b"),
        StreakOutcome::Continue(2)
    );
    assert_eq!(
        ledger.record_and_check(7, "sig-b"),
        StreakOutcome::Tripped(3)
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// 不同 issue 各自记账互不干扰。
#[test]
fn issues_ledger_independently() {
    let ws = temp_ws("multi");
    let mut ledger = FailStreakLedger::load(&ws);
    let _ = ledger.record_and_check(1, "sig-a");
    let _ = ledger.record_and_check(1, "sig-a");
    assert_eq!(
        ledger.record_and_check(2, "sig-a"),
        StreakOutcome::Continue(1),
        "issue 2 不受 issue 1 计数影响"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// 持久化：重启（重新 load）后计数延续，第 3 次 Tripped——gateway 重启
/// 不丢账。
#[test]
fn ledger_persists_across_reload() {
    let ws = temp_ws("persist");
    {
        let mut ledger = FailStreakLedger::load(&ws);
        let _ = ledger.record_and_check(9, "sig-x");
        let _ = ledger.record_and_check(9, "sig-x");
    }
    let mut ledger2 = FailStreakLedger::load(&ws);
    assert_eq!(
        ledger2.record_and_check(9, "sig-x"),
        StreakOutcome::Tripped(3),
        "重启后第 3 次同签名即熔断"
    );
    let _ = std::fs::remove_dir_all(&ws);
}

/// 清账复位：Tripped 后 clear → 下一次从 1 起算（人工处理/estop 后作废）。
#[test]
fn clear_resets_account() {
    let ws = temp_ws("clear");
    let mut ledger = FailStreakLedger::load(&ws);
    let _ = ledger.record_and_check(5, "sig-a");
    let _ = ledger.record_and_check(5, "sig-a");
    ledger.clear(5);
    assert_eq!(
        ledger.record_and_check(5, "sig-a"),
        StreakOutcome::Continue(1)
    );
    // 清不存在的账是幂等 no-op。
    ledger.clear(5);
    ledger.clear(999);
    let _ = std::fs::remove_dir_all(&ws);
}

/// 损坏账本文件 = 诚实从空账本起（不炸决策链）。
#[test]
fn corrupt_file_starts_fresh() {
    let ws = temp_ws("corrupt");
    std::fs::create_dir_all(ws.join("board")).unwrap();
    std::fs::write(ws.join("board").join("fail_streaks.json"), "not json{{").unwrap();
    let mut ledger = FailStreakLedger::load(&ws);
    assert_eq!(
        ledger.record_and_check(3, "sig-a"),
        StreakOutcome::Continue(1)
    );
    let _ = std::fs::remove_dir_all(&ws);
}
