//! C（2026-11-22 批二）：logs.usage 十六族占用统计测试。
//!
//! 夹具：tempdir workspace + 用生产 resolver/写端同构路径造族文件。
//! 钉住三面契约：①族表齐全恰 16 族（漏族=前端占用条缺行）；
//! ②逐族 bytes/entries 与盘上实测一致（含单文件族 heartbeat、混合根族
//! ziwei 的 reports 树）；③缺席族诚实 0/0、totals 恒等于逐族合计。

use super::*;

/// 在 dir 下写 content（自动建父目录），返回写入字节数。
fn write_file(dir: &std::path::Path, rel: &str, content: &str) -> u64 {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
    content.len() as u64
}

#[test]
fn usage_reports_sixteen_families_with_accurate_stats() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let logs_root = nemesis_path::logs_dir_in_workspace(ws);

    // 三族夹具：request_logs（resolver 定位）、terminal（logs 根拼装）、
    // heartbeat（单文件族）、ziwei（混合根族 reports 树）。
    let b1 = write_file(
        &nemesis_path::resolve_request_logs_dir_in_workspace(ws),
        "20261122_abc/00.request.md",
        "0123456789",
    ); // 10 B, 1 file
    let b2 = write_file(
        &nemesis_path::resolve_request_logs_dir_in_workspace(ws),
        "20261122_abc/01.AI.Response.raw.json",
        "ab",
    ); // 2 B, 1 file
    let b3 = write_file(&logs_root.join("terminal"), "term-1.log", "xyz"); // 3 B
    // heartbeat：logs 根下的已知单文件。
    std::fs::create_dir_all(&logs_root).unwrap();
    std::fs::write(logs_root.join("heartbeat.log"), "hb").unwrap();
    let b5 = write_file(
        &ws.join("reports").join("ziwei-verify").join("p1"),
        "report.json",
        "1234",
    ); // 4 B

    let out = LogsHandler.usage(ws.to_str().unwrap()).unwrap().unwrap();
    let families = out["families"].as_object().expect("families map");
    assert_eq!(
        families.len(),
        16,
        "十六族齐全: {:?}",
        families.keys().collect::<Vec<_>>()
    );
    for name in [
        "session_logs",
        "boundary",
        "request_logs",
        "terminal",
        "event_ledger",
        "trash",
        "gateway",
        "cluster_logs",
        "eval",
        "capture",
        "dreaming",
        "calibration",
        "deliveries",
        "exec_checkpoints",
        "heartbeat",
        "ziwei",
    ] {
        assert!(families.contains_key(name), "缺族 {name}");
        assert!(
            families[name]["bytes"].is_u64() && families[name]["entries"].is_u64(),
            "族 {name} 形态必须是 {{bytes, entries}}"
        );
    }

    assert_eq!(families["request_logs"]["bytes"], b1 + b2, "out = {out}");
    assert_eq!(families["request_logs"]["entries"], 2);
    assert_eq!(families["terminal"]["bytes"], b3);
    assert_eq!(families["terminal"]["entries"], 1);
    assert_eq!(families["heartbeat"]["bytes"], 2, "单文件族在场=1 件");
    assert_eq!(families["heartbeat"]["entries"], 1);
    assert_eq!(families["ziwei"]["bytes"], b5, "ziwei reports 树合计");
    assert_eq!(families["ziwei"]["entries"], 1);

    // totals = 逐族合计（sum over families，与夹具总和一致）。
    let sum_bytes: u64 = families
        .values()
        .map(|v| v["bytes"].as_u64().unwrap())
        .sum();
    let sum_entries: u64 = families
        .values()
        .map(|v| v["entries"].as_u64().unwrap())
        .sum();
    assert_eq!(out["total_bytes"], sum_bytes);
    assert_eq!(out["total_entries"], sum_entries);
    assert_eq!(
        out["total_bytes"],
        b1 + b2 + b3 + 2 + b5,
        "totals 与盘上实测一致: {out}"
    );
    assert_eq!(out["total_entries"], 2 + 1 + 1 + 1);
}

#[test]
fn usage_missing_families_report_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let out = LogsHandler
        .usage(tmp.path().to_str().unwrap())
        .unwrap()
        .unwrap();
    let families = out["families"].as_object().unwrap();
    assert_eq!(families.len(), 16);
    for (name, v) in families {
        assert_eq!(v["bytes"], 0, "缺席族 {name} 应 0/0: {v}");
        assert_eq!(v["entries"], 0);
    }
    assert_eq!(out["total_bytes"], 0);
    assert_eq!(out["total_entries"], 0);
}

#[test]
fn usage_session_and_trash_resolvers_hit() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write_file(
        &nemesis_path::resolve_session_logs_dir_in_workspace(ws),
        "agent_main_session_s1.jsonl",
        "{}",
    );
    write_file(
        &nemesis_path::resolve_trash_sessions_dir_in_workspace(ws),
        "tr-1.jsonl",
        "abcd",
    );

    let out = LogsHandler.usage(ws.to_str().unwrap()).unwrap().unwrap();
    let families = out["families"].as_object().unwrap();
    assert_eq!(families["session_logs"]["entries"], 1);
    assert_eq!(families["session_logs"]["bytes"], 2);
    assert_eq!(families["trash"]["entries"], 1);
    assert_eq!(families["trash"]["bytes"], 4);
}
