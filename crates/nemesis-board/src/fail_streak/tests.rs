//! M3/§6.4 同因熔断账本测试：新签名格式（verdict 维度）/ 阈值三连跳 /
//! 签名翻转清零（含旧格式存量兼容）/ 持久化跨实例记账 / 清账复位。

use super::*;
use crate::anchor::{AnchorCheck, AnchorKind, AnchorResult};
use crate::review::ReviewVerdict;

fn anchor(raw: &str) -> AnchorCheck {
    AnchorCheck {
        raw: raw.to_string(),
        kind: AnchorKind::FileExists,
        target: "rel/path.txt".to_string(),
        pattern: None,
    }
}

fn result(raw: &str, passed: bool) -> AnchorResult {
    AnchorResult {
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

/// §6.4A 新签名格式：`{verdict 小写}|{排序失败锚点集}`；全过/无锚点 =
/// `{verdict 小写}|no-anchors`。FAIL 与 UNSURE 不再坍缩同签名。
#[test]
fn signature_carries_verdict_and_sorts_failed_anchors() {
    let results = vec![
        result("b.txt", true),
        result("a.txt", false),
        result("c.txt", false),
    ];
    assert_eq!(
        failure_signature(&results, &ReviewVerdict::Fail),
        "fail|a.txt|c.txt",
        "失败集排序后 join，verdict 前缀"
    );

    let all_pass = vec![result("a.txt", true), result("b.txt", true)];
    assert_eq!(
        failure_signature(&all_pass, &ReviewVerdict::Fail),
        "fail|no-anchors"
    );
    assert_eq!(
        failure_signature(&[], &ReviewVerdict::Fail),
        "fail|no-anchors",
        "空锚点组 = no-anchors"
    );
    // UNSURE 恒定签名（锚点 FAIL 已短路成 FAIL，UNSURE 时失败集恒空）。
    assert_eq!(
        failure_signature(&all_pass, &ReviewVerdict::Unsure),
        "unsure|no-anchors"
    );
    assert_eq!(
        failure_signature(&[], &ReviewVerdict::Unsure),
        UNSURE_SIGNATURE
    );
    // FAIL 与 UNSURE 同锚点形态不同签名（§6.4A 的核心去坍缩）。
    assert_ne!(
        failure_signature(&[], &ReviewVerdict::Fail),
        failure_signature(&[], &ReviewVerdict::Unsure)
    );
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

/// 签名翻转：计数清零重计（worker 换了失败形态不算旧账；含 §6.4A 新旧
/// 格式翻转——FAIL 轮与 UNSURE 轮交替各自重计）。
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

/// §6.4A 存量兼容：旧格式签名（无 verdict 前缀的 `fail:no-anchors`）遇新
/// 格式签名 = 翻转清零重计，零迁移（文档承诺）。
#[test]
fn legacy_signature_entry_flips_to_new_format() {
    let ws = temp_ws("legacy");
    std::fs::create_dir_all(ws.join("board")).unwrap();
    std::fs::write(
        ws.join("board").join("fail_streaks.json"),
        r#"{"issues":{"11":{"signature":"fail:no-anchors","count":3}}}"#,
    )
    .unwrap();
    let mut ledger = FailStreakLedger::load(&ws);
    // 存量 count=3 的旧条目遇新签名 → 清零重计为 1（不秒熔断）。
    assert_eq!(
        ledger.record_and_check(11, "fail|no-anchors"),
        StreakOutcome::Continue(1),
        "旧格式存量条目遇新签名翻转清零"
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

/// §6.4B UNSURE 收敛闸判定矩阵：count=1 继续重派 / count=2 转人工 /
/// 0=不限（旧行为 pin）/ FAIL 轮不受此闸。
#[test]
fn unsure_gate_matrix() {
    // UNSURE + 阈值 2：第 1 轮继续重派，第 2 轮转人工。
    assert!(!unsure_gate_trips(
        &ReviewVerdict::Unsure,
        &StreakOutcome::Continue(1),
        2
    ));
    assert!(unsure_gate_trips(
        &ReviewVerdict::Unsure,
        &StreakOutcome::Continue(2),
        2
    ));
    // 0 = 不限（完全旧行为：任意轮数都继续重派）。
    assert!(!unsure_gate_trips(
        &ReviewVerdict::Unsure,
        &StreakOutcome::Continue(5),
        0
    ));
    // FAIL 轮不受此闸（M3 熔断管辖）。
    assert!(!unsure_gate_trips(
        &ReviewVerdict::Fail,
        &StreakOutcome::Continue(2),
        2
    ));
    assert!(!unsure_gate_trips(
        &ReviewVerdict::Fail,
        &StreakOutcome::Continue(9),
        2
    ));
    // PASS 不进重派臂，防御性 pin。
    assert!(!unsure_gate_trips(
        &ReviewVerdict::Pass,
        &StreakOutcome::Continue(9),
        2
    ));
}
