//! P21（2026-09-25 能力扩展 WS1）沙盒拒绝台账测试。
//!
//! 覆盖验收点：注入假拒绝 → 台账追加一行合法 JSON + 查询返回；幂等/并发
//! 追加不炸（行级完整性）；损坏行诚实跳过；启发式分类决策表；面向模型
//! 文案的自纠要素。全平台编译（denial.rs 无 cfg 门控——Windows Sandboxie
//! 与 Linux 用户态后端共用同一台账面）。

use super::denial::*;

fn rec(backend: &str, op: &str, target: &str, reason: &str, visible: bool) -> DenialRecord {
    new_record(backend, op, target, reason, visible)
}

// ---------------------------------------------------------------------------
// 追加 + 查询 roundtrip（验收主链）
// ---------------------------------------------------------------------------

/// 注入一条假拒绝 → 文件出现一行合法 JSON（六字段齐全）→ 查询返回同值。
#[test]
fn append_then_read_roundtrip() {
    let ws = tempfile::tempdir().unwrap();
    let r = rec(
        "landlock",
        "write_file",
        "/etc/passwd",
        "os error 13 (EACCES)",
        true,
    );
    append_denial(ws.path(), &r).expect("append must succeed");

    // 台账文件落在 <workspace>/logs/sandbox_denials.jsonl
    assert!(ledger_path(ws.path()).exists(), "ledger file created");

    let got = read_denials(ws.path(), 50);
    assert_eq!(got.len(), 1, "exactly one record");
    assert_eq!(got[0], r, "roundtrip preserves the record verbatim");
    assert_eq!(got[0].backend, "landlock");
    assert_eq!(got[0].op, "write_file");
    assert_eq!(got[0].target, "/etc/passwd");
    assert_eq!(got[0].reason, "os error 13 (EACCES)");
    assert!(got[0].model_visible);
    // ts 是 RFC3339 形态（毫秒精度 + Z 后缀）
    assert!(got[0].ts.ends_with('Z'), "ts: {}", got[0].ts);
    assert!(got[0].ts.contains('T'), "ts: {}", got[0].ts);
}

/// 幂等追加：同一条记录写两遍 = 两行（台账是事件账本，不去重——语义诚实）。
#[test]
fn append_is_append_not_overwrite() {
    let ws = tempfile::tempdir().unwrap();
    let r = rec(
        "sandboxie",
        "exec",
        "cmd /c del C:\\x",
        "Access is denied",
        true,
    );
    append_denial(ws.path(), &r).unwrap();
    append_denial(ws.path(), &r).unwrap();
    assert_eq!(read_denials(ws.path(), 100).len(), 2);
}

/// limit 取文件尾 N 条（最近拒绝）；limit 大于总量 = 全量。
#[test]
fn read_limit_returns_tail() {
    let ws = tempfile::tempdir().unwrap();
    for i in 0..5 {
        append_denial(ws.path(), &rec("bwrap", &format!("op{i}"), "t", "r", false)).unwrap();
    }
    let tail = read_denials(ws.path(), 2);
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].op, "op3", "tail keeps file order");
    assert_eq!(tail[1].op, "op4");
    assert_eq!(read_denials(ws.path(), 100).len(), 5);
    assert_eq!(read_denials(ws.path(), 0).len(), 0);
}

/// 文件缺失（无拒绝发生过）→ 空 vec，不报错。
#[test]
fn read_missing_ledger_is_empty() {
    let ws = tempfile::tempdir().unwrap();
    assert!(read_denials(ws.path(), 50).is_empty());
}

/// 损坏行（半截写入 / 非 JSON）诚实跳过，好行照常返回——不炸、不整文件作废。
#[test]
fn corrupt_lines_are_skipped() {
    let ws = tempfile::tempdir().unwrap();
    let path = ledger_path(ws.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let good = serde_json::to_string(&rec("landlock", "edit_file", "/t", "e", false)).unwrap();
    std::fs::write(
        &path,
        format!("{good}\n{{\"half-written\"\n\nnot json at all\n{good}\n"),
    )
    .unwrap();
    let got = read_denials(ws.path(), 50);
    assert_eq!(got.len(), 2, "only the two well-formed lines survive");
    assert_eq!(got[0].op, "edit_file");
}

/// 并发追加不撕裂：8 线程 × 25 条全部完整可解析（多 executor 子进程并发
/// 写同一台账的生产形态缩影——线程版同样走 open(append)+write+close）。
#[test]
fn concurrent_appends_do_not_tear_lines() {
    let ws = tempfile::tempdir().unwrap();
    let ws_path = ws.path().to_path_buf();
    let handles: Vec<_> = (0..8)
        .map(|w| {
            let ws_path = ws_path.clone();
            std::thread::spawn(move || {
                for i in 0..25 {
                    append_denial(
                        &ws_path,
                        &rec(
                            "landlock",
                            &format!("w{w}"),
                            &format!("t{w}/{i}"),
                            "os error 13",
                            true,
                        ),
                    )
                    .expect("append must succeed");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("writer thread");
    }
    let all = read_denials(ws.path(), 1000);
    assert_eq!(all.len(), 200, "every appended line must be parseable");
    assert!(
        all.iter().all(|r| r.op.starts_with('w')),
        "no torn/corrupt records: {:?}",
        all.iter().map(|r| &r.op).take(5).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 启发式分类 + 面向模型文案
// ---------------------------------------------------------------------------

/// looks_like_denial 决策表：Linux errno / Windows 文案 / 沙盒自产文案命中；
/// 普通错误不误命中。
#[test]
fn looks_like_denial_decision_table() {
    for positive in [
        "write failed: os error 13 (EACCES)",
        "Permission denied (os error 13)",
        "Access is denied. (os error 5)",
        "EPERM: Operation not permitted",
        "bwrap: creating new namespace failed",
        "operation blocked: denied by sandbox NemesisBox",
        "sandboxie: cannot create box process",
    ] {
        assert!(looks_like_denial(positive), "must classify: {positive}");
    }
    for negative in [
        "file not found (os error 2)",
        "invalid utf-8 in args",
        "tool timed out after 30s",
        "boom",
        "",
        // 裸 "sandbox" 单词形态不再命中（后端自身故障 ≠ 沙盒拒绝——
        // 误记账会污染台账并把普通错误改写成拦截文案）
        "sandbox exec failed",
        "sandbox backend unavailable, falling back",
    ] {
        assert!(
            !looks_like_denial(negative),
            "must NOT classify: {negative:?}"
        );
    }
}

/// 面向模型文案的自纠四要素：点名后端、点名工作区出口、给下一步、保留原因。
#[test]
fn model_facing_text_contains_self_correct_elements() {
    let text = model_facing_text(
        "landlock",
        "write_file",
        "/etc/hosts",
        "os error 13 (EACCES)",
        "/home/bot/.nemesisbot/workspace",
    );
    assert!(text.contains("[沙盒拦截]"), "names the block: {text}");
    assert!(text.contains("landlock"), "names the backend: {text}");
    assert!(text.contains("write_file"), "names the op: {text}");
    assert!(
        text.contains("/home/bot/.nemesisbot/workspace"),
        "workspace exit door: {text}"
    );
    assert!(text.contains("os error 13"), "keeps raw reason: {text}");
    assert!(text.contains("下一步"), "next-step guidance: {text}");
}

/// 长目标/原因预览截断（按 char，不给台账塞超长行）。
#[test]
fn preview_target_truncates_long_strings() {
    let long = "x".repeat(5000);
    let p = preview_target(&long);
    assert!(p.chars().count() <= 301, "truncated: {}", p.chars().count());
    assert!(p.ends_with('…'), "ellipsis suffix");
    // 中文按 char 截断，不劈 UTF-8 字节边界
    let zh = "中".repeat(400);
    let pzh = preview_target(&zh);
    assert!(pzh.chars().count() <= 301);
    assert!(serde_json::to_string(&rec("bwrap", "op", &long, "r", true)).is_ok());
}
