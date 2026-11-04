use super::*;

fn tmpws() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn sample_receipt(n: u64, connector: &str) -> Receipt {
    Receipt {
        receipt_id: format!("RCPT-1000-{n:04}"),
        ts: "2026-10-14T08:00:00+00:00".into(),
        connector: connector.into(),
        op: "place_order".into(),
        risk: "critical".into(),
        class: "write".into(),
        outcome: "executed".into(),
        args_digest: args_digest(&serde_json::json!({"symbol": "AAPL", "qty": 10})),
        summary: format!("第 {n} 笔"),
        receipt_key: Some("order_id".into()),
        receipt_value: Some(format!("NB-ORD-{n}")),
        budget_consumed: None,
        approval_ref: Some("APPR-1".into()),
        session_key: Some("agent:main:session:s1".into()),
        undoable: true,
    }
}

#[test]
fn receipt_id_unique_and_formatted() {
    let a = new_receipt_id(1_700_000_000_000);
    let b = new_receipt_id(1_700_000_000_000);
    assert_ne!(a, b, "同毫秒也要单调");
    assert!(a.starts_with("RCPT-1700000000000-"));
    let seq: u64 = a.rsplit('-').next().expect("seq").parse().expect("u64");
    assert!(seq < 10_000, "seq 四位内（新进程重置）");
}

#[test]
fn args_digest_stable_and_sensitive_to_input() {
    let a = serde_json::json!({"symbol": "AAPL", "qty": 10});
    let b = serde_json::json!({"symbol": "AAPL", "qty": 11});
    let d1 = args_digest(&a);
    let d2 = args_digest(&a);
    let d3 = args_digest(&b);
    assert_eq!(d1, d2, "同 args 同 digest");
    assert_ne!(d1, d3);
    assert_eq!(d1.len(), 16, "sha256 前 16 hex");
    assert!(d1.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn append_and_list_roundtrip_newest_first() {
    let ws = tmpws();
    for n in 1..=3u64 {
        append_receipt(ws.path(), &sample_receipt(n, "paper-broker")).expect("append");
    }
    let all = list_receipts(ws.path(), 100, None).expect("list");
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].receipt_id, "RCPT-1000-0003", "新→旧");
    assert_eq!(all[2].receipt_id, "RCPT-1000-0001");

    let filtered = list_receipts(ws.path(), 100, Some("paper-broker")).expect("filter hit");
    assert_eq!(filtered.len(), 3);
    let filtered = list_receipts(ws.path(), 100, Some("nope")).expect("filter miss");
    assert!(filtered.is_empty());

    let limited = list_receipts(ws.path(), 2, None).expect("limit");
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0].receipt_id, "RCPT-1000-0003");
}

#[test]
fn serde_roundtrip_keeps_optional_fields_absent() {
    let minimal = Receipt {
        receipt_id: "RCPT-1-0001".into(),
        ts: "2026-10-14T08:00:00+00:00".into(),
        connector: "monitor".into(),
        op: "fetch".into(),
        risk: "low".into(),
        class: "read".into(),
        outcome: "executed".into(),
        args_digest: "0123456789abcdef".into(),
        summary: "拉取成功".into(),
        receipt_key: None,
        receipt_value: None,
        budget_consumed: None,
        approval_ref: None,
        session_key: None,
        undoable: false,
    };
    let line = serde_json::to_string(&minimal).expect("ser");
    assert!(!line.contains("receipt_key"), "None 字段不落盘");
    assert!(!line.contains("approval_ref"), "None 字段不落盘");
    let back: Receipt = serde_json::from_str(&line).expect("de");
    assert_eq!(back.receipt_id, minimal.receipt_id);
    assert!(back.receipt_key.is_none());
}

#[test]
fn legacy_line_without_undoable_deserializes() {
    // 撤销动词（2026-10-14）之前的存量回执行没有 undoable 字段——
    // undoable 缺 serde(default) 会把它们全部解析成损坏行静默跳过
    // （connectors_ws IT ids=[] 实证）。回归锁：历史行合法可读、
    // undoable 缺省 false。
    let legacy = r#"{"receipt_id":"RCPT-1000-0001","ts":"2026-10-13T08:00:00Z","connector":"demo","op":"quote","risk":"low","class":"read","outcome":"executed","args_digest":"0123456789abcdef","summary":"历史行"}"#;
    let back: Receipt = serde_json::from_str(legacy).expect("legacy line must deserialize");
    assert_eq!(back.receipt_id, "RCPT-1000-0001");
    assert!(!back.undoable, "缺省 undoable=false（历史行不可撤销）");
}

#[test]
fn list_skips_corrupt_lines() {
    let ws = tmpws();
    append_receipt(ws.path(), &sample_receipt(1, "c")).expect("append");
    // 追加一条损坏行
    let dir = receipts_dir(ws.path());
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .expect("dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    names.sort();
    let path = names.pop().expect("file");
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open");
    writeln!(f, "{{broken json").expect("append broken");
    append_receipt(ws.path(), &sample_receipt(2, "c")).expect("append after broken");

    let all = list_receipts(ws.path(), 100, None).expect("list with corrupt line");
    assert_eq!(all.len(), 2, "损坏行跳过不中断");
    assert_eq!(all[0].receipt_id, "RCPT-1000-0002");
}

// BUG-20261104-386：回执 jsonl 追加的行原子性——`append_receipt` 无锁，
// 多会话 agent loop 并发执行 connector 工具时 `writeln!`（正文与 "\n" 两
// 次 write）可粘行，list_receipts 对损坏行 warn + 跳过 = 回执（可撤销
// 凭据）静默丢失 → 撤销面缺口。修复 = 整行（含换行）单次 write_all
// （O_APPEND / FILE_APPEND_DATA 每次 write 定位原子）。与 auditor
// audit.jsonl / chat_log R100 同族。真红是概率性的（竞态），修复后确定性绿。
#[test]
fn b386_concurrent_receipt_appends_never_tear() {
    let ws = tmpws();
    const THREADS: usize = 8;
    const N: usize = 100;
    // 长 summary（≈220B）加宽旧实现两个 write 片段之间被插入的窗口。
    let filler = "y".repeat(200);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let mut handles = Vec::new();
    for t in 0..THREADS {
        let barrier = barrier.clone();
        let ws_path = ws.path().to_path_buf();
        let filler = filler.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for i in 0..N {
                let n = (t * N + i) as u64;
                let mut r = sample_receipt(n, "paper-broker");
                r.summary = format!("第 {n} 笔 {}", filler);
                append_receipt(&ws_path, &r).expect("append");
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let all = list_receipts(ws.path(), THREADS * N, None).expect("list");
    assert_eq!(
        all.len(),
        THREADS * N,
        "并发回执不得粘行丢失（预期 {}，实际 {}）",
        THREADS * N,
        all.len()
    );
    // 逐条 summary 尾部完整（粘行截断会丢 filler）。
    for r in &all {
        assert!(
            r.summary.ends_with(&filler),
            "回执 summary 尾部缺失 = 撕裂: {}",
            r.receipt_id
        );
    }
}
