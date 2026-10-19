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
/// 进内存环——进程内 replay at-most-once 保持；账本置不可信（见 F2 用例）。
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

// ===========================================================================
// F1/F2（二轮复评 2026-10-18）：重写中断恢复 + durable 闸
// ===========================================================================

/// F1：current 缺席 + tmp 在场（Unix rename 未完成 / 旧实现删-改间隙遗留）
/// → open 采纳 tmp 为账本，保留集内的完成记录与未决意图全部可见。
#[test]
fn recovery_adopts_tmp_when_current_missing() {
    let ws = tempfile::tempdir().unwrap();
    let path = journal_path(ws.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let tmp = path.with_file_name("journal.jsonl.tmp");
    std::fs::write(
        &tmp,
        format!(
            "{}\n{}\n",
            intent_line("c-rec-i", "exec"),
            v2_line("c-rec-done", "done-r")
        ),
    )
    .unwrap();
    let j = Journal::with_agent_id(ws.path(), "agent-a");
    assert_eq!(
        j.pending_intent("c-rec-i"),
        Some("exec"),
        "tmp 账本采纳后未决意图必须可见"
    );
    assert!(
        j.lookup("c-rec-done").is_some(),
        "tmp 账本采纳后完成记录必须可见"
    );
    assert!(!tmp.exists(), "采纳后 tmp 应已 rename 归位");
}

/// F1：current 缺席 + bak + tmp（Windows 舞步「current→bak」后中断）→
/// 优先恢复 bak（旧账 = tmp 保留集的信息超集，恢复即无丢失），tmp 作
/// 废弃重写残留清理。
#[test]
fn recovery_prefers_bak_when_current_missing() {
    let ws = tempfile::tempdir().unwrap();
    let path = journal_path(ws.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bak = path.with_file_name("journal.jsonl.bak");
    let tmp = path.with_file_name("journal.jsonl.tmp");
    std::fs::write(&bak, format!("{}\n", v2_line("c-rec-old", "old-r"))).unwrap();
    std::fs::write(&tmp, format!("{}\n", v2_line("c-rec-new", "new-r"))).unwrap();
    let j = Journal::with_agent_id(ws.path(), "agent-a");
    assert!(j.lookup("c-rec-old").is_some(), "应采纳 bak（旧账超集）");
    assert_eq!(j.lookup("c-rec-new"), None, "tmp 是废弃重写残留，不采纳");
    assert!(!bak.exists() && !tmp.exists(), "恢复后两处残留应清干净");
}

/// F1：current 在场 + tmp/bak 残留（替换已完成、清理未做）→ 残留清干净，
/// 账本照常回载。
#[test]
fn recovery_cleans_leftovers_when_current_present() {
    let ws = tempfile::tempdir().unwrap();
    let path = journal_path(ws.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("{}\n", v2_line("c-rec-cur", "cur-r"))).unwrap();
    let bak = path.with_file_name("journal.jsonl.bak");
    let tmp = path.with_file_name("journal.jsonl.tmp");
    std::fs::write(&bak, b"stale-bak").unwrap();
    std::fs::write(&tmp, b"stale-tmp").unwrap();
    let j = Journal::with_agent_id(ws.path(), "agent-a");
    assert!(j.lookup("c-rec-cur").is_some());
    assert!(
        !bak.exists() && !tmp.exists(),
        "在场账本下的重写残留应清干净"
    );
}

/// F2：加载读错误（journal.jsonl 是目录 → 读必败且非 NotFound）→
/// durable=false：record_intent 返回 false（serve_line 闸的旗标源），
/// 内存未决面照常登记——不当空账静默放行。
#[test]
fn load_read_failure_marks_ledger_not_durable() {
    let ws = tempfile::tempdir().unwrap();
    let path = journal_path(ws.path());
    std::fs::create_dir_all(&path).unwrap(); // 目录占位 → read_to_string 必败
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    assert!(
        !j.record_intent("c-ro-x", "exec"),
        "读失败 = 账本不可信，意图不得报可靠落盘"
    );
    assert_eq!(j.pending_intent("c-ro-x"), Some("exec"), "内存未决面仍登记");
}

/// 压缩重写失败的**在场臂**分类（tmp 目录占位 → fs::write(tmp) 必败 →
/// atomic_rewrite Err，current 完好原地）：durable 保持 true（append-only
/// 继续成立，账本面健康不得过度拒绝），保留集照常可见、新写入照常落盘。
/// 缺席臂（Windows 舞步双 rename 失败 → durable=false）不可黑盒确定性
/// 构造——需 bak 在两次 rename 之间被外部句柄锁住（AV 时序竞态），由
/// journal.rs 内联注释钉住契约。
#[test]
fn compaction_failure_with_current_present_keeps_durable() {
    let ws = tempfile::tempdir().unwrap();
    let path = journal_path(ws.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let n = JOURNAL_RING_CAP + 8; // 超限 → open 触发压缩
    let mut body = String::new();
    for i in 1..=n {
        body.push_str(&v2_line(&format!("c-cf-{i}"), "r"));
        body.push('\n');
    }
    std::fs::write(&path, body).unwrap();
    // tmp 目录占位 → 压缩 write(tmp) 必败（is a directory）。
    let tmp = path.with_file_name("journal.jsonl.tmp");
    std::fs::create_dir(&tmp).unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    // 保留集照常回载（环形容量内的最新记录）。
    assert!(
        j.lookup(&format!("c-cf-{n}")).is_some(),
        "重写失败不影响内存保留集"
    );
    // 分类在场臂：durable 不降级，意图照常报可靠落盘。
    assert!(
        j.record_intent("c-cf-intent", "exec"),
        "current 在场的重写失败 = append-only 健康，不得误置不可信"
    );
    // 新写入照常 append 到原地账本；重开后仍可见（盘面未被破坏）。
    let done = v2_line("c-cf-after", "done-r");
    j.record(&done);
    drop(j);
    let j2 = Journal::with_agent_id(ws.path(), "agent-a");
    assert_eq!(
        j2.lookup("c-cf-after"),
        Some(done.as_str()),
        "append-only 继续落盘"
    );
}

/// F2：append 失败 → durable=false 且 **sticky**——同一实例上修复存储也
/// 不复信（本进程生命周期内有副作用工具保持拒绝）；新实例（等价重启）
/// 读正常账恢复可信。
#[test]
fn append_failure_marks_not_durable_sticky() {
    let ws = tempfile::tempdir().unwrap();
    let blocker = ws.path().join(".runtime");
    std::fs::write(&blocker, b"not a dir").unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    assert!(
        !j.record_intent("c-st-1", "exec"),
        "append 失败 = 不可靠落盘"
    );
    // 存储故障移除（「修复」）——sticky：本实例不复信。
    std::fs::remove_file(&blocker).unwrap();
    assert!(
        !j.record_intent("c-st-2", "exec"),
        "sticky：修复后同实例不复信"
    );
    assert_eq!(j.pending_intent("c-st-1"), Some("exec"));
    assert_eq!(j.pending_intent("c-st-2"), Some("exec"));
    drop(j);
    let mut j2 = Journal::with_agent_id(ws.path(), "agent-a");
    assert!(
        j2.record_intent("c-st-3", "exec"),
        "存储正常的新实例恢复可信"
    );
}

/// 正常路径：record_intent 返回 true（可靠落盘）；幂等重记返回当前可信态；
/// 禁用形态恒 true（无持久面也无闸——per-call 零变化）。
#[test]
fn record_intent_returns_durability() {
    let ws = tempfile::tempdir().unwrap();
    let mut j = Journal::with_agent_id(ws.path(), "agent-a");
    assert!(j.record_intent("c-dur-1", "exec"));
    assert!(j.record_intent("c-dur-1", "exec"), "幂等重记返回当前可信态");
    let mut dis = Journal::disabled();
    assert!(dis.record_intent("c-dur-2", "exec"), "禁用 = 无闸");
}
