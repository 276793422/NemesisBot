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
