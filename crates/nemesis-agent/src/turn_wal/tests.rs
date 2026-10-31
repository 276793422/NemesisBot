//! L8 turn WAL 单测：三态落盘 / 断头恢复幂等 / TTL 清扫 / 轮转 / 坏行容忍。
//! 独立文件（check-inline-tests 纪律）：`#[cfg(test)] mod tests;` 声明在
//! turn_wal.rs 尾部，全部用例落这里。

use super::*;

fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "nemesis-turnwal-{}-{}-{tag}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).expect("create tmp dir");
    d
}

fn call(tag: &str) -> WalCall {
    WalCall {
        session_key: format!("agent:main:session:s-{tag}"),
        channel: "web".into(),
        chat_id: format!("chat-{tag}"),
        user: "tester".into(),
        call_id: format!("call-{tag}"),
        tool: "exec".into(),
    }
}

fn read_lines(p: &Path) -> Vec<WalRecord> {
    match std::fs::File::open(p) {
        Ok(f) => BufReader::new(f)
            .lines()
            .map_while(Result::ok)
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(&l).ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[tokio::test]
async fn three_state_finished_leaves_no_candidate() {
    let dir = tmp_dir("finished");
    let wal = TurnWal::open(&dir).expect("open");
    let c = call("fin");
    wal.log(&c, STATE_ADMITTED, "echo hi", None);
    wal.log(&c, STATE_STARTED, "echo hi", None);
    wal.log(&c, STATE_FINISHED, "echo hi", Some(true));

    let candidates = wal.recover_and_finalize();
    assert!(
        candidates.is_empty(),
        "finished 调用不该产生续轮候选: {candidates:?}"
    );
    // drain 后当前文件只可能含 interrupted 留痕行（此处为零）。
    let lines = read_lines(wal.path());
    assert!(lines.is_empty(), "drain 后应为空文件，实际 {lines:?}");
    assert!(!wal.old_path.exists() || read_lines(&wal.old_path).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn started_without_finished_becomes_candidate_and_is_idempotent() {
    let dir = tmp_dir("interrupted");
    let wal = TurnWal::open(&dir).expect("open");
    let c = call("itpt");
    wal.log(&c, STATE_ADMITTED, "write_file a.txt", None);
    wal.log(&c, STATE_STARTED, "write_file a.txt", None);

    let candidates = wal.recover_and_finalize();
    assert_eq!(candidates.len(), 1, "断头调用应产生恰好 1 个候选");
    let cand = &candidates[0].record;
    assert_eq!(cand.call_id, "call-itpt");
    assert_eq!(cand.tool, "exec");
    assert_eq!(cand.session_key, "agent:main:session:s-itpt");
    assert_eq!(cand.channel, "web");
    assert_eq!(cand.args_preview, "write_file a.txt");

    // 补写的 interrupted 行已落盘。
    let lines = read_lines(wal.path());
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].state, STATE_INTERRUPTED);

    // 幂等：二次恢复零候选（interrupted 算收尾）。
    let again = wal.recover_and_finalize();
    assert!(again.is_empty(), "二次恢复应幂等: {again:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn admitted_without_start_also_interrupted() {
    let dir = tmp_dir("admitted-only");
    let wal = TurnWal::open(&dir).expect("open");
    let c = call("adm");
    wal.log(&c, STATE_ADMITTED, "exec sleep", None);

    let candidates = wal.recover_and_finalize();
    assert_eq!(candidates.len(), 1, "admitted-only 也是断头调用");
    assert_eq!(candidates[0].record.state, STATE_ADMITTED);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn args_preview_truncated_by_chars() {
    let dir = tmp_dir("preview");
    let wal = TurnWal::open(&dir).expect("open");
    let c = call("trunc");
    let long = "中".repeat(500);
    wal.log(&c, STATE_ADMITTED, &long, None);
    let lines = read_lines(wal.path());
    assert_eq!(lines.len(), 1);
    let got = &lines[0].args_preview;
    assert_eq!(got.chars().count(), ARGS_PREVIEW_MAX_CHARS + 1); // + 省略号
    assert!(got.ends_with('…'));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn ttl_stale_interrupted_not_resumed() {
    let dir = tmp_dir("ttl");
    let wal = TurnWal::open(&dir)
        .expect("open")
        .with_limits(DEFAULT_MAX_BYTES, 1); // TTL=1s
    let c = call("stale");
    wal.log(&c, STATE_STARTED, "old work", None);
    // 等过 TTL。
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let candidates = wal.recover_and_finalize();
    assert!(
        candidates.is_empty(),
        "超龄断头调用不该续轮: {candidates:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rotation_keeps_newest_generation_and_recovery_spans_both() {
    let dir = tmp_dir("rotate");
    // max_bytes 压到很小强制轮转；TTL 长给足。
    let wal = TurnWal::open(&dir).expect("open").with_limits(512, 3600);
    let done = call("done");
    wal.log(&done, STATE_ADMITTED, "old finished work", None);
    wal.log(&done, STATE_STARTED, "old finished work", None);
    wal.log(&done, STATE_FINISHED, "old finished work", Some(true));
    // 此刻大约 3 行 × ~180B —— 再写一个断头调用，撑过 512B 触发轮转。
    let hung = call("hung");
    for i in 0..3 {
        wal.log(
            &hung,
            STATE_STARTED,
            &format!("long preview payload to push size over the rotation threshold #{i}"),
            None,
        );
    }
    assert!(wal.old_path.exists(), "触顶后应存在 .old 一代");

    let candidates = wal.recover_and_finalize();
    assert_eq!(candidates.len(), 1, "跨代归并后断头调用仍是 1 个");
    assert_eq!(candidates[0].record.call_id, "call-hung");
    // 轮转 + drain 后 .old 应被清掉。
    assert!(!wal.old_path.exists(), "drain 应删除 .old");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn malformed_lines_skipped_during_recovery() {
    let dir = tmp_dir("badlines");
    let wal = TurnWal::open(&dir).expect("open");
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(wal.path())
            .expect("append");
        use std::io::Write;
        writeln!(f, "{{not json").unwrap();
        writeln!(f).unwrap();
        let rec = WalRecord {
            ts_ms: now_ms(),
            session_key: "agent:main:session:s-bad".into(),
            channel: "web".into(),
            chat_id: "chat-bad".into(),
            user: "t".into(),
            call_id: "call-bad".into(),
            tool: "exec".into(),
            args_preview: "x".into(),
            state: STATE_STARTED.into(),
            ok: None,
        };
        writeln!(f, "{}", serde_json::to_string(&rec).unwrap()).unwrap();
    }
    let candidates = wal.recover_and_finalize();
    assert_eq!(candidates.len(), 1, "坏行被跳过后正常恢复");
    assert_eq!(candidates[0].record.call_id, "call-bad");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn call_id_for_empty_raw_generates() {
    let generated = call_id_for("", "exec");
    assert!(generated.ends_with("-exec"));
    assert!(generated.len() > "exec".len());
    assert_eq!(call_id_for("abc", "exec"), "abc");
}

/// BUG-20261030-204：轮转失败不杀 WAL、不静默丢行。老代码轮转判定+rename
/// 在文件锁外做——rename 失败（.old 位点被占/瞬时句柄）时 guard 永久停在
/// None，此后所有 log 行被 `if let Some` **静默**丢弃（无 per-write warn、
/// M2 degraded 报告永不触发）。修复后轮转在锁内做 + 失败重开原文件续写。
#[tokio::test]
async fn rotation_failure_does_not_kill_wal_or_drop_lines() {
    let dir = tmp_dir("rotate-fail");
    let wal = TurnWal::open(&dir).expect("open").with_limits(512, 3600);
    // .old 位点放一个目录：remove_file 失败（ignored）→ rename 撞目录
    // 在 Windows/Unix 上都必败——确定性的轮转失败注入。
    std::fs::create_dir_all(&wal.old_path).expect("make .old a dir");

    let c = call("rf");
    // 首行 ~700B 撑过 512B 阈值 → 轮转尝试（必失败）→ 行仍须落盘。
    wal.log(&c, STATE_ADMITTED, &"x".repeat(600), None);
    let lines = read_lines(wal.path());
    assert_eq!(lines.len(), 1, "轮转失败当行不得静默丢弃: {lines:?}");
    assert_eq!(lines[0].state, STATE_ADMITTED);

    // 后续行继续可写（轮转失败 ≠ WAL 永久死亡）。
    wal.log(&c, STATE_FINISHED, "done", Some(true));
    let lines = read_lines(wal.path());
    assert_eq!(lines.len(), 2, "轮转失败后 WAL 必须继续可写: {lines:?}");
    assert_eq!(lines[1].state, STATE_FINISHED);

    let _ = std::fs::remove_dir_all(&dir);
}
