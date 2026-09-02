use super::*;

#[test]
fn test_safe_key_conversion() {
    let path = log_path("agent:main:main");
    assert!(path.to_string_lossy().contains("agent_main_main"));
    assert!(path.to_string_lossy().ends_with(".jsonl"));
}

#[test]
fn test_read_nonexistent() {
    let (msgs, total, has_more, oldest) = read_chat_log("test:nonexistent:session", 10, None);
    assert!(msgs.is_empty());
    assert_eq!(total, 0);
    assert!(!has_more);
    assert_eq!(oldest, 0);
}

#[test]
fn test_append_with_model_round_trip() {
    let key = "test:model-badge:roundtrip";
    delete_chat_log(key); // clean slate

    // user row: no model. assistant row: with model badge.
    append_chat_log_with_model(key, "user", "hi", None);
    append_chat_log_with_model(
        key,
        "assistant",
        "hello back",
        Some("deepseek/deepseek-v4-flash"),
    );

    let (msgs, total, _, _) = read_chat_log(key, 10, None);
    assert_eq!(total, 2);
    assert_eq!(msgs.len(), 2);
    // user row has no model field.
    assert_eq!(msgs[0]["role"].as_str(), Some("user"));
    assert!(msgs[0].get("model").is_none());
    // assistant row carries the model badge.
    assert_eq!(msgs[1]["role"].as_str(), Some("assistant"));
    assert_eq!(
        msgs[1]["model"].as_str(),
        Some("deepseek/deepseek-v4-flash")
    );

    // Legacy append_chat_log (model=None) writes no model field → backward compat.
    append_chat_log(key, "assistant", "legacy-no-model");
    let (msgs2, _, _, _) = read_chat_log(key, 10, None);
    let last = msgs2.last().unwrap();
    assert_eq!(last["content"].as_str(), Some("legacy-no-model"));
    assert!(last.get("model").is_none());

    delete_chat_log(key); // cleanup
}

/// Single-source-of-truth row predicate (2026-08-25 fork-fix round): the
/// fork's chat_log projection and the turns endpoint's `end_preview` MUST
/// use the same rule, or the dialog preview would disagree with what the
/// fork actually ends on. Pin it here.
#[test]
fn test_is_projected_chat_row() {
    use super::is_projected_chat_row;
    assert!(is_projected_chat_row("user", "anything"));
    assert!(is_projected_chat_row("user", "")); // user rows always project
    assert!(is_projected_chat_row("assistant", "reply"));
    assert!(!is_projected_chat_row("assistant", "")); // tool_calls intermediate
    assert!(!is_projected_chat_row("assistant", "   \n ")); // whitespace-only
    assert!(!is_projected_chat_row("tool", "result"));
    assert!(!is_projected_chat_row("system", "prompt"));
}

// 2026-08-25: delete_chat_log must also remove the title meta sidecar
// (orphan-file fix); clear_chat_log must KEEP it (title survives a clear).
#[test]
fn test_delete_chat_log_removes_meta_but_clear_keeps_it() {
    let key = format!(
        "test:meta:lifecycle:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    crate::chat_log::append_chat_log(&key, "user", "row");
    crate::chat_log::write_session_meta(&key, "title to keep");

    // clear: jsonl emptied, meta survives
    crate::chat_log::clear_chat_log(&key);
    let (rows, _, _, _) = crate::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(rows.len(), 0);
    assert_eq!(
        crate::chat_log::read_session_meta(&key).as_deref(),
        Some("title to keep")
    );

    // delete: everything gone including meta
    crate::chat_log::delete_chat_log(&key);
    assert!(crate::chat_log::read_session_meta(&key).is_none());
    let (rows2, _, _, _) = crate::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(rows2.len(), 0);
}

// --- W3a: error arms, cron/model markers, fork helpers, boundary sidecar ---

use crate::session::StoredMessage;

fn uniq_key(tag: &str) -> String {
    format!(
        "test:w3a:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn stored_msg(role: &str, content: &str) -> StoredMessage {
    StoredMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: "2026-08-25T10:00:00+08:00".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

/// append_chat_log_full with cron origin markers: both fields written, and
/// the rows parse back through read_chat_log.
#[test]
fn test_append_full_with_cron_markers() {
    let key = uniq_key("cron");
    append_chat_log_full(
        &key,
        "assistant",
        "from cron",
        Some("prov/model"),
        Some("job-42"),
        Some("nightly report"),
    );
    let (rows, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 1);
    assert_eq!(rows[0]["cron_job_id"].as_str(), Some("job-42"));
    assert_eq!(rows[0]["cron_job_name"].as_str(), Some("nightly report"));
    assert_eq!(rows[0]["model"].as_str(), Some("prov/model"));
    delete_chat_log(&key);
}

/// dirblock 挂死诊断（2026-09-02 extended-tests Linux nightly 首跑实录）。
///
/// Linux：后台线程每 90s 检查测试是否完成；未完成就把各线程内核态
/// （stat 的 wchan + syscall 号）经 [`crate::test_support::force_stderr`]
/// 旁路 libtest 捕获直接写 stderr，留挂死现场。其它平台：全 no-op 桩
/// （/proc 依赖，无观测意义），保持测试体平台无关调同一 API。
#[cfg(target_os = "linux")]
mod dirblock_diag {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    const DUMP_INTERVAL_SECS: u64 = 90;
    // 90s × 6 = 9 分钟的现场窗口；之后线程退出，进程留给 job 超时收割。
    const MAX_ROUNDS: u64 = 6;

    pub struct Watchdog {
        done: Arc<AtomicBool>,
        step: Arc<AtomicU8>,
    }

    impl Watchdog {
        pub fn start() -> Self {
            let done = Arc::new(AtomicBool::new(false));
            let step = Arc::new(AtomicU8::new(0));
            let (d2, s2) = (done.clone(), step.clone());
            let _ = std::thread::Builder::new()
                .name("dirblock-watchdog".into())
                .spawn(move || {
                    for round in 1..=MAX_ROUNDS {
                        std::thread::sleep(Duration::from_secs(DUMP_INTERVAL_SECS));
                        if d2.load(Ordering::Relaxed) {
                            return;
                        }
                        Self::dump(round, s2.load(Ordering::Relaxed));
                    }
                });
            Self { done, step }
        }

        pub fn step(&self, n: u8) {
            self.step.store(n, Ordering::Relaxed);
        }

        pub fn finish(&self) {
            self.done.store(true, Ordering::Relaxed);
        }

        fn dump(round: u64, step: u8) {
            let Ok(mut err) =
                std::fs::OpenOptions::new().write(true).open("/proc/self/fd/2")
            else {
                return;
            };
            let _ = writeln!(
                err,
                "[dirblock-watchdog] round {round}: test hung ~{}s, last step={step} \
                 (1=path 2=mkdir 3=append 4=read 5=asserts); per-thread kernel state:",
                round * DUMP_INTERVAL_SECS
            );
            let Ok(rd) = std::fs::read_dir("/proc/self/task") else {
                return;
            };
            for ent in rd.flatten() {
                let tid = ent.file_name().to_string_lossy().to_string();
                let stat = std::fs::read_to_string(format!("/proc/self/task/{tid}/stat"))
                    .unwrap_or_default();
                // stat 形如 `PID (comm) rest...`：comm 可含空格与 ')'，从最后一
                // 个 ')' 后切字段。rest[0] = state(field 3)，wchan = field 35
                // → rest 下标 32。
                let Some((_, after_lparen)) = stat.split_once('(') else {
                    continue;
                };
                let Some((comm, rest)) = after_lparen.rsplit_once(')') else {
                    continue;
                };
                let f: Vec<&str> = rest.split_whitespace().collect();
                let state = f.first().copied().unwrap_or("?");
                let wchan = f.get(32).copied().unwrap_or("?");
                let syscall = std::fs::read_to_string(format!("/proc/self/task/{tid}/syscall"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "-".into());
                let _ = writeln!(
                    err,
                    "  tid={tid} comm={comm} state={state} wchan={wchan} syscall={syscall}"
                );
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod dirblock_diag {
    /// 非 Linux no-op 桩：挂死观测依赖 /proc（Linux-only 语义），仅为保持
    /// 测试体平台无关调同一 API。
    pub struct Watchdog;

    impl Watchdog {
        pub fn start() -> Self {
            Watchdog
        }
        pub fn step(&self, _n: u8) {}
        pub fn finish(&self) {}
    }
}

/// A DIRECTORY squatting on the jsonl path: append warns and returns;
/// read_chat_log pass-1 open also fails (empty page, not a panic).
#[test]
fn test_directory_at_log_path_is_tolerated() {
    // 挂死背景：本测试在 CI Linux nightly（bb308f4）第一波挂死 2h（job
    // 120min 超时取消，全程唯一 60s 告警点名本测试）；本地/远端 Linux 单跑
    // 恒绿 0.02s，静态代码全路径有界，无法从代码推理定位 → watchdog 留现场。
    let watchdog = dirblock_diag::Watchdog::start();

    let key = uniq_key("dirblock");
    let path = log_path(&key);
    watchdog.step(1); // path resolved
    std::fs::create_dir_all(&path).unwrap();
    watchdog.step(2); // dir created
    append_chat_log(&key, "user", "blocked"); // warn + return
    watchdog.step(3); // append returned
    let (rows, total, _, _) = read_chat_log(&key, 10, None);
    watchdog.step(4); // read returned
    assert_eq!(total, 0);
    assert!(rows.is_empty());
    watchdog.step(5); // asserts passed — 只剩 remove_dir
    std::fs::remove_dir(&path).unwrap();
    watchdog.finish();
}

/// read_boundary_events on a missing sidecar and on a DIRECTORY squatting on
/// the sidecar path: both give an empty vec (open-err arm).
#[test]
fn test_boundary_events_missing_and_unreadable() {
    let key = uniq_key("boundary");
    assert!(read_boundary_events(&key).is_empty(), "missing sidecar");

    let path = boundary_path(&key);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    assert!(read_boundary_events(&key).is_empty(), "unreadable sidecar");
    // append_boundary_event hits the same open failure -> warn + return.
    append_boundary_event(&key, "turn_start", "blocked");
    std::fs::remove_dir(&path).unwrap();
}

/// append_boundary_event happy path + read-back through read_boundary_events.
#[test]
fn test_boundary_event_roundtrip() {
    let key = uniq_key("boundary-ok");
    append_boundary_event(&key, "turn_start", "r1");
    append_boundary_event(&key, "turn_end", "done");
    let evs = read_boundary_events(&key);
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0]["event"].as_str(), Some("turn_start"));
    assert_eq!(evs[1]["detail"].as_str(), Some("done"));
    assert_eq!(evs[0]["role"].as_str(), Some("boundary"));
    delete_chat_log(&key);
}

/// write_chat_log_rows: empty input -> 0; verbatim rows -> written count and
/// byte-faithful round-trip (extra fields survive).
#[test]
fn test_write_chat_log_rows_verbatim() {
    let key = uniq_key("rows");
    assert_eq!(write_chat_log_rows(&key, &[]), 0);

    let row = serde_json::json!({
        "role": "user",
        "content": "pick me",
        "timestamp": "2026-08-25T09:00:00+08:00",
        "model": "prov/keep",
        "cron_job_id": "job-9",
    });
    let n = write_chat_log_rows(&key, &[row]);
    assert_eq!(n, 1);
    let (rows, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 1);
    assert_eq!(rows[0]["model"].as_str(), Some("prov/keep"));
    assert_eq!(rows[0]["cron_job_id"].as_str(), Some("job-9"));
    delete_chat_log(&key);
}

/// copy_chat_log_prefix (superseded but kept): cuts at the at_turn-th user
/// row inclusive; at_turn=0 copies nothing.
#[test]
fn test_copy_chat_log_prefix_cuts_at_user_turn() {
    let src = uniq_key("cpsrc");
    let dst = uniq_key("cpdst");
    append_chat_log(&src, "user", "q1");
    append_chat_log(&src, "assistant", "a1");
    append_chat_log(&src, "user", "q2");
    append_chat_log(&src, "assistant", "a2");

    let n = copy_chat_log_prefix(&src, &dst, 1);
    assert_eq!(n, 2, "q1+a1 only");
    let (rows, total, _, _) = read_chat_log(&dst, 10, None);
    assert_eq!(total, 2);
    assert_eq!(rows[0]["content"].as_str(), Some("q1"));
    assert_eq!(rows[1]["content"].as_str(), Some("a1"));

    // at_turn=0: first row is a user row, turns(1) > 0 -> nothing copied.
    let dst2 = uniq_key("cpdst2");
    assert_eq!(copy_chat_log_prefix(&src, &dst2, 0), 0);

    delete_chat_log(&src);
    delete_chat_log(&dst);
    delete_chat_log(&dst2);
}

/// write_chat_log_from_store (superseded but kept): only user/assistant rows
/// with non-empty content project; timestamps come from the store.
#[test]
fn test_write_chat_log_from_store_projection() {
    let key = uniq_key("fromstore");
    let msgs = vec![
        stored_msg("system", "sys prompt"), // never projects
        stored_msg("user", "q1"),
        stored_msg("assistant", "   "), // tool_calls intermediate -> skipped
        stored_msg("tool", "raw result"), // never projects
        stored_msg("assistant", "final answer"),
    ];
    let n = write_chat_log_from_store(&key, &msgs);
    assert_eq!(n, 2);
    let (rows, total, _, _) = read_chat_log(&key, 10, None);
    assert_eq!(total, 2);
    assert_eq!(rows[0]["content"].as_str(), Some("q1"));
    assert_eq!(rows[1]["content"].as_str(), Some("final answer"));
    assert_eq!(
        rows[1]["timestamp"].as_str(),
        Some("2026-08-25T10:00:00+08:00"),
        "no re-stamping"
    );
    delete_chat_log(&key);

    // All-unprojectable input -> 0 lines, nothing written.
    let key2 = uniq_key("fromstore2");
    assert_eq!(write_chat_log_from_store(&key2, &[stored_msg("tool", "x")]), 0);
    delete_chat_log(&key2);
}

/// delete_chat_log with DIRECTORIES squatting on all three paths: removal
/// fails per path (warn arm) but must not panic.
#[test]
fn test_delete_chat_log_with_dir_squatters_does_not_panic() {
    let key = uniq_key("delsquatter");
    let lp = log_path(&key);
    let bp = boundary_path(&key);
    let mp = meta_path(&key);
    for p in [&lp, &bp, &mp] {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::create_dir_all(p).unwrap();
    }
    delete_chat_log(&key); // warn x3, no panic
    assert!(lp.is_dir() && bp.is_dir() && mp.is_dir(), "squatters remain");
    for p in [&lp, &bp, &mp] {
        std::fs::remove_dir(p).unwrap();
    }
}

/// clear_chat_log: happy path truncates both the message jsonl and an
/// existing boundary sidecar; a DIRECTORY at the jsonl path only warns.
#[test]
fn test_clear_chat_log_truncates_both_files() {
    let key = uniq_key("clear");
    append_chat_log(&key, "user", "row1");
    append_boundary_event(&key, "turn_start", "r1");
    clear_chat_log(&key);
    let (rows, _, _, _) = read_chat_log(&key, 10, None);
    assert!(rows.is_empty());
    assert!(read_boundary_events(&key).is_empty(), "sidecar truncated");
    delete_chat_log(&key);

    // warn arm: jsonl path is a directory.
    let key2 = uniq_key("clearblocked");
    let lp = log_path(&key2);
    std::fs::create_dir_all(&lp).unwrap();
    clear_chat_log(&key2); // warn + return
    std::fs::remove_dir(&lp).unwrap();
}

/// write_session_meta with a DIRECTORY at the meta path: warn only.
#[test]
fn test_write_session_meta_dir_squatter_warns() {
    let key = uniq_key("metablock");
    let mp = meta_path(&key);
    if let Some(parent) = mp.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::create_dir_all(&mp).unwrap();
    write_session_meta(&key, "t"); // warn + return
    assert!(read_session_meta(&key).is_none());
    std::fs::remove_dir(&mp).unwrap();
}
