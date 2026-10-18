//! journal 单测：禁用形态 / 记账-查询往返 / 重开重放（崩溃重建形态）/
//! 环形逐出 + 盘上裁剪 / 坏行跳过 / 幂等记账 / 不可解析行 no-op。
//! 显式 `with_agent_id` 构造（无进程 env 竞态）；tempdir 真实落盘。

use nemesis_agent::executor_transport::ExecutorResponseWire;

use super::{JOURNAL_RING_CAP, Journal};

fn v2_line(call_id: &str, result: &str) -> String {
    serde_json::to_string(&ExecutorResponseWire {
        v: 2,
        call_id: call_id.to_string(),
        ok: true,
        result: result.to_string(),
        error: String::new(),
    })
    .unwrap()
}

fn journal_path(ws: &std::path::Path) -> std::path::PathBuf {
    ws.join(super::JOURNAL_REL)
}

#[test]
fn disabled_without_agent_id() {
    let ws = tempfile::tempdir().unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "");
    assert!(!j.is_enabled());
    assert_eq!(j.lookup("c-1"), None);
    // record 是 no-op：不落盘。
    j.record(&v2_line("c-1", "r"));
    assert_eq!(j.lookup("c-1"), None);
    assert!(!journal_path(ws.path()).exists(), "禁用形态不得写盘");
}

#[test]
fn record_lookup_roundtrip() {
    let ws = tempfile::tempdir().unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    assert!(j.is_enabled());
    let line = v2_line("c-1-1", "hello");
    j.record(&line);
    assert_eq!(j.lookup("c-1-1"), Some(line.as_str()));
    assert_eq!(j.lookup("missing"), None);
    // 空 call_id（v1 形态）恒 miss。
    assert_eq!(j.lookup(""), None);
}

#[test]
fn replay_hit_survives_reopen() {
    let ws = tempfile::tempdir().unwrap();
    let line = v2_line("c-2-1", "before-crash");
    {
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");
        j.record(&line);
    } // 进程「崩溃」——drop
    // 重开（supervisor 重建子进程形态）：历史账回载，replay 命中。
    let j = Journal::with_agent_id(ws.path(), "agent-a");
    assert_eq!(j.lookup("c-2-1"), Some(line.as_str()));
}

#[test]
fn corrupt_lines_skipped_on_load() {
    let ws = tempfile::tempdir().unwrap();
    let good1 = v2_line("c-3-1", "a");
    let good2 = v2_line("c-3-2", "b");
    let path = journal_path(ws.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // 坏行夹在两条好行之间（崩溃截断半行形态）。
    std::fs::write(
        &path,
        format!("{good1}\n{{\"ok\":false,\"result\":\"trunc\n{good2}\n"),
    )
    .unwrap();
    let j = Journal::with_agent_id(ws.path(), "agent-a");
    assert_eq!(j.lookup("c-3-1"), Some(good1.as_str()));
    assert_eq!(j.lookup("c-3-2"), Some(good2.as_str()));
}

#[test]
fn idempotent_record_first_wins() {
    let ws = tempfile::tempdir().unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    let line = v2_line("c-4-1", "once");
    j.record(&line);
    // 同 call_id 再记（内容不同也不覆盖——先到先得）。
    j.record(&v2_line("c-4-1", "twice"));
    assert_eq!(j.lookup("c-4-1"), Some(line.as_str()));
    let raw = std::fs::read_to_string(journal_path(ws.path())).unwrap();
    assert_eq!(raw.lines().count(), 1, "盘上恰好一行: {raw}");
}

#[test]
fn record_ignores_unparseable_line() {
    let ws = tempfile::tempdir().unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    j.record("not json");
    j.record(
        &serde_json::to_string(&ExecutorResponseWire {
            v: 0,
            call_id: String::new(),
            ok: false,
            result: String::new(),
            error: "no call id".into(),
        })
        .unwrap(),
    );
    assert!(
        !journal_path(ws.path()).exists(),
        "坏行/空 call_id 不得触发建目录或写盘"
    );
}

#[test]
fn ring_cap_evicts_oldest_and_compacts_on_reopen() {
    let ws = tempfile::tempdir().unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    let n = JOURNAL_RING_CAP as u32 + 5;
    for i in 1..=n {
        j.record(&v2_line(&format!("c-ring-{i}"), "x"));
    }
    // 内存侧：最旧 5 条被逐出，最新在场。
    assert_eq!(j.lookup("c-ring-1"), None);
    assert_eq!(j.lookup("c-ring-5"), None);
    let newest = v2_line(&format!("c-ring-{n}"), "x");
    assert_eq!(j.lookup(&format!("c-ring-{n}")), Some(newest.as_str()));
    drop(j);
    // 重开：盘上超限旧账裁剪回容量。
    let j2 = Journal::with_agent_id(ws.path(), "agent-a");
    let raw = std::fs::read_to_string(journal_path(ws.path())).unwrap();
    assert!(
        raw.lines().count() <= JOURNAL_RING_CAP,
        "盘上应裁剪回容量，实际 {} 行",
        raw.lines().count()
    );
    assert_eq!(j2.lookup("c-ring-1"), None);
    assert!(j2.lookup(&format!("c-ring-{n}")).is_some());
}

// ===========================================================================
// R2 两相账（2026-10-18 三方复评）：意图行 / 未决面 / 原子压缩 / 盘写失败
// 内存兜底。
// ===========================================================================

fn intent_line(call_id: &str, tool: &str) -> String {
    serde_json::to_string(&super::IntentLine {
        kind: super::KIND_INTENT,
        call_id,
        tool,
    })
    .unwrap()
}

/// 意图 → 重开（崩溃形态）→ 未决面存活；完成记账了结意图；再重开不再未决。
#[test]
fn intent_pending_survives_reopen_and_is_resolved_by_done() {
    let ws = tempfile::tempdir().unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    j.record_intent("c-i-1", "exec");
    assert_eq!(j.pending_intent("c-i-1"), Some("exec"), "记账即未决");
    drop(j);
    // 重开（supervisor 重建子进程形态）：意图从盘上回载。
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    assert_eq!(
        j.pending_intent("c-i-1"),
        Some("exec"),
        "跨重启未决意图必须存活"
    );
    // 完成记账 → 意图了结 → replay 命中完成账。
    j.record(&v2_line("c-i-1", "done-result"));
    assert_eq!(j.pending_intent("c-i-1"), None, "完成准入了结意图");
    assert_eq!(
        j.lookup("c-i-1"),
        Some(v2_line("c-i-1", "done-result").as_str())
    );
    drop(j);
    let j = Journal::with_agent_id(ws.path(), "agent-a");
    assert_eq!(j.pending_intent("c-i-1"), None);
    assert!(j.lookup("c-i-1").is_some());
}

/// 已有完成账的 call_id 记意图 = 迟到重复，不落盘不进未决面。
#[test]
fn intent_skipped_when_done_already_present() {
    let ws = tempfile::tempdir().unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    let line = v2_line("c-i-2", "done");
    j.record(&line);
    j.record_intent("c-i-2", "exec");
    assert_eq!(j.pending_intent("c-i-2"), None);
    let raw = std::fs::read_to_string(journal_path(ws.path())).unwrap();
    assert_eq!(raw.lines().count(), 1, "盘上仍只有完成一行: {raw}");
}

/// 盘写失败（`.runtime` 被文件占位 → 建目录失败）不再放弃记账：仍 admit
/// 进内存环——进程内 replay at-most-once 保持，跨重启退化 at-least-once。
#[test]
fn disk_write_failure_still_admits_to_memory() {
    let ws = tempfile::tempdir().unwrap();
    // `.runtime` 位置被普通文件占位 → append_line 的 create_dir_all 必败。
    let blocker = ws.path().join(".runtime");
    std::fs::write(&blocker, b"not a dir").unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    let line = v2_line("c-df-1", "memory-only");
    j.record(&line);
    assert_eq!(
        j.lookup("c-df-1"),
        Some(line.as_str()),
        "盘写失败必须仍有进程内 replay"
    );
    // 意图同理：内存未决面成立。
    j.record_intent("c-df-2", "exec");
    assert_eq!(j.pending_intent("c-df-2"), Some("exec"));
}

/// 未决意图不占环形容量、不由裁剪管理：压缩重写时意图行保活在前。
#[test]
fn intent_lines_survive_compaction_rewrite() {
    let ws = tempfile::tempdir().unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    j.record_intent("c-int-keep", "exec");
    let n = JOURNAL_RING_CAP as u32 + 5;
    for i in 1..=n {
        j.record(&v2_line(&format!("c-ci-{i}"), "x"));
    }
    drop(j);
    // 重开触发压缩（完成账超限）。
    let j2 = Journal::with_agent_id(ws.path(), "agent-a");
    assert_eq!(
        j2.pending_intent("c-int-keep"),
        Some("exec"),
        "压缩重写必须保活未决意图"
    );
    let raw = std::fs::read_to_string(journal_path(ws.path())).unwrap();
    assert!(
        raw.contains(r#""kind":"intent""#),
        "重写后的盘上账本必须仍含意图行"
    );
    assert!(j2.lookup(&format!("c-ci-{n}")).is_some());
}

/// 崩溃截断的半行意图（与半行完成同族）跳过不入内存；同行好账照常回载。
#[test]
fn half_written_intent_line_skipped_on_load() {
    let ws = tempfile::tempdir().unwrap();
    let path = journal_path(ws.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            "{}\n{{\"kind\":\"intent\",\"call_id\":\"c-trunc\n{}\n{}\n",
            intent_line("c-ok-intent", "exec"),
            intent_line("c-good-intent", "grep"),
            v2_line("c-done-1", "r")
        ),
    )
    .unwrap();
    let j = Journal::with_agent_id(ws.path(), "agent-a");
    assert_eq!(
        j.pending_intent("c-ok-intent"),
        Some("exec"),
        "完好意图行照常回载"
    );
    assert_eq!(
        j.pending_intent("c-good-intent"),
        Some("grep"),
        "完好意图行照常回载"
    );
    assert_eq!(j.pending_intent("c-trunc"), None, "截断半行必须跳过");
    assert!(j.lookup("c-done-1").is_some());
}
