//! A3 记分卡/防倒退门禁单测（四轮回查补：`compare_baseline` 是防倒退
//! 门禁本体——纯函数、承重，此前整个 crate 零测试）。

use super::*;
use serde_json::json;
use std::path::PathBuf;

/// 写一份 baseline JSON（schema agent-bench/v1），返回文件路径。
/// `host = None` 模拟老格式基线（无 host 字段）。
fn baseline_file(dir: &std::path::Path, host: Option<&str>, scenarios: Vec<Value>) -> PathBuf {
    let mut base = json!({
        "schema": "agent-bench/v1",
        "started_at": "2026-10-21T00:00:00+08:00",
        "gateway_version": "test",
        "bench_wall_secs": 1,
        "scenarios": scenarios,
    });
    if let Some(h) = host {
        base["host"] = json!(h);
    }
    let path = dir.join("baseline.json");
    std::fs::write(&path, serde_json::to_string_pretty(&base).unwrap()).unwrap();
    path
}

/// 当前记分卡：单场景 `name`，`passes` 轮全过、每轮 `latency_ms`。
/// （host 字段覆写为测试值——`Scorecard::new` 从 env 取，不可控。）
fn card_with(name: &str, passes: usize, latency_ms: u64, host: &str) -> Scorecard {
    let mut card = Scorecard::new("test".to_string());
    card.host = host.to_string();
    let s = card.score(name);
    for _ in 0..passes {
        s.record_pass_at(latency_ms, 0);
    }
    card
}

fn scenario_entry(name: &str, pass_rate: f64, p50: u64) -> Value {
    json!({
        "name": name,
        "model": "testai-1.1",
        "runs": 1,
        "passed": 1,
        "failed": 0,
        "pass_rate": pass_rate,
        "latency_ms": { "p50": p50, "p95": p50, "max": p50 },
        "failure_notes": [],
    })
}

fn has_regression(findings: &[String]) -> bool {
    findings
        .iter()
        .any(|f| !f.starts_with("[note]") && !f.starts_with("[ok]"))
}

// 通过率回退任何机器都判（功能正确性无噪声）——异机也照样红。
#[test]
fn pass_rate_regression_fails_on_any_machine() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = baseline_file(
        dir.path(),
        Some("other-machine"),
        vec![scenario_entry("basic_chat", 1.0, 100)],
    );
    let card = card_with("basic_chat", 0, 100, "this-machine");
    let findings = compare_baseline(&card, &base, 0.30).unwrap();
    assert!(
        findings.iter().any(|f| f.contains("通过率回退")),
        "{findings:?}"
    );
    assert!(has_regression(&findings));
}

// 同机 + 延迟超容忍 → 延迟回退判门。
#[test]
fn latency_gate_same_host_over_tolerance() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = baseline_file(
        dir.path(),
        Some("this-machine"),
        vec![scenario_entry("basic_chat", 1.0, 1000)],
    );
    // 3 轮全 2000ms → p50 = 2000 > 1000 × 1.30。
    let card = card_with("basic_chat", 3, 2000, "this-machine");
    let findings = compare_baseline(&card, &base, 0.30).unwrap();
    assert!(
        findings.iter().any(|f| f.contains("p50 延迟回退")),
        "{findings:?}"
    );
    assert!(has_regression(&findings));
}

// 同机 + 延迟在容忍内 → 无回退（唯一的 [ok] 行）。
#[test]
fn latency_gate_same_host_within_tolerance() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = baseline_file(
        dir.path(),
        Some("this-machine"),
        vec![scenario_entry("basic_chat", 1.0, 1000)],
    );
    let card = card_with("basic_chat", 3, 1000, "this-machine");
    let findings = compare_baseline(&card, &base, 0.30).unwrap();
    assert_eq!(findings, vec!["[ok] 无回退".to_string()], "{findings:?}");
}

// 异机 → 延迟门整体跳过（只 [note]，10 倍延迟也不判）。
#[test]
fn latency_gate_cross_host_skipped() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = baseline_file(
        dir.path(),
        Some("other-machine"),
        vec![scenario_entry("basic_chat", 1.0, 1000)],
    );
    let card = card_with("basic_chat", 3, 10_000, "this-machine");
    let findings = compare_baseline(&card, &base, 0.30).unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.starts_with("[note]") && f.contains("其他机器")),
        "{findings:?}"
    );
    assert!(!findings.iter().any(|f| f.contains("p50 延迟回退")));
    assert!(!has_regression(&findings));
}

// 未知身份（任一侧 "unknown"）≠ 同机——不猜，跳过门。
#[test]
fn latency_gate_unknown_host_skipped() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = baseline_file(
        dir.path(),
        Some("unknown"),
        vec![scenario_entry("basic_chat", 1.0, 1000)],
    );
    let card = card_with("basic_chat", 3, 10_000, "unknown");
    let findings = compare_baseline(&card, &base, 0.30).unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.starts_with("[note]") && f.contains("unknown")),
        "{findings:?}"
    );
    assert!(!has_regression(&findings));

    // 单侧 unknown 同样跳过（基线有名字、本机 unknown）。
    let base2 = baseline_file(
        dir.path(),
        Some("this-machine"),
        vec![scenario_entry("basic_chat", 1.0, 1000)],
    );
    let card2 = card_with("basic_chat", 3, 10_000, "unknown");
    let findings2 = compare_baseline(&card2, &base2, 0.30).unwrap();
    assert!(!has_regression(&findings2), "{findings2:?}");
}

// 老基线无 host 字段 → 延迟门跳过 + 重新存基线提示。
#[test]
fn latency_gate_legacy_baseline_skipped() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = baseline_file(
        dir.path(),
        None,
        vec![scenario_entry("basic_chat", 1.0, 1000)],
    );
    let card = card_with("basic_chat", 3, 10_000, "this-machine");
    let findings = compare_baseline(&card, &base, 0.30).unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.contains("无 host 字段") && f.contains("--save-baseline")),
        "{findings:?}"
    );
    assert!(!has_regression(&findings));
}

// 覆盖缩水：基线有而本轮没跑的场景 = 倒退。
#[test]
fn coverage_shrinkage_is_regression() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = baseline_file(
        dir.path(),
        Some("this-machine"),
        vec![
            scenario_entry("basic_chat", 1.0, 100),
            scenario_entry("context_integrity", 1.0, 100),
        ],
    );
    let card = card_with("basic_chat", 1, 100, "this-machine");
    let findings = compare_baseline(&card, &base, 0.30).unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.contains("context_integrity") && f.contains("覆盖缩水")),
        "{findings:?}"
    );
    assert!(has_regression(&findings));
}

// 基线 p50 = 0（无效值）不比延迟门（除零/假象防护）。
#[test]
fn latency_gate_skips_zero_base_p50() {
    let dir = tempfile::TempDir::new().unwrap();
    let base = baseline_file(
        dir.path(),
        Some("this-machine"),
        vec![scenario_entry("basic_chat", 1.0, 0)],
    );
    let card = card_with("basic_chat", 3, 10_000, "this-machine");
    let findings = compare_baseline(&card, &base, 0.30).unwrap();
    assert_eq!(findings, vec!["[ok] 无回退".to_string()], "{findings:?}");
}

// chrome trace：事件数/时间轴单位/轨道序的基本形状契约。
#[test]
fn chrome_trace_shape() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut card = Scorecard::new("test".to_string());
    card.host = "h".to_string();
    {
        let s = card.score("basic_chat");
        s.record_pass_at(1500, 100);
        s.record_pass_at(2500, 200);
    }
    {
        let s = card.score("context_integrity");
        s.record_pass_at(300, 5_000);
    }
    let path = dir.path().join("trace.json");
    export_chrome_trace(&card, &path).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let events = v["traceEvents"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    // ms → µs。
    assert_eq!(events[0]["ts"], json!(100_000));
    assert_eq!(events[0]["dur"], json!(1_500_000));
    // tid 按场景序：场景 1 两轮同轨，场景 2 下一轨。
    assert_eq!(events[0]["tid"], json!(1));
    assert_eq!(events[1]["tid"], json!(1));
    assert_eq!(events[2]["tid"], json!(2));
    assert_eq!(v["displayTimeUnit"], json!("ms"));
    assert_eq!(v["metadata"]["host"], json!("h"));
}

// 指纹契约：永不为空、无首尾空白（取不到才 "unknown"；Linux CI 不导出
// HOSTNAME env 时 /proc 兜底——七轮回查钉住回归面）。
#[test]
fn host_fingerprint_is_never_empty() {
    let h = host_fingerprint();
    assert_eq!(h.trim(), h, "指纹不得带首尾空白: {h:?}");
    assert!(!h.is_empty());
}
