//! （扩展计划）：会话事件账本单测。
//! （账本写验全走显式路径——不触碰 default_path_manager 全局单例。）

use super::*;
use std::path::PathBuf;

fn temp_ledger(tag: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(format!("{tag}.jsonl"));
    (dir, path)
}

fn chat_row(role: &str, content: &str) -> serde_json::Value {
    serde_json::json!({ "role": role, "content": content, "timestamp": "2026-09-24T00:00:00+08:00" })
}

/// append/truncate/fork 三种 op 全链可验证，统计准确。
#[test]
fn record_and_verify_full_chain() {
    let (_d, path) = temp_ledger("chain");

    let r1 = chat_row("user", "帮我改 bug");
    let r2 = chat_row("assistant", "已修复");
    record_entry_at(
        &path,
        LedgerOp::Append,
        "chat_log",
        vec![LedgerRow::from_chat_row(&r1, false)],
    )
    .unwrap();
    record_entry_at(
        &path,
        LedgerOp::Append,
        "chat_log",
        vec![LedgerRow::from_chat_row(&r2, false)],
    )
    .unwrap();

    // truncate：被删行全文入账
    let deleted = chat_row("user", "这段会被 rewind 删掉");
    record_entry_at(
        &path,
        LedgerOp::Truncate,
        "chat_log",
        vec![LedgerRow::from_chat_row(&deleted, true)],
    )
    .unwrap();

    // fork：sha-only
    let forked = chat_row("user", "fork 前缀");
    record_entry_at(
        &path,
        LedgerOp::Fork,
        "chat_log",
        vec![LedgerRow::from_chat_row(&forked, false)],
    )
    .unwrap();

    let stats = ledger_verify(&path).expect("全链必须验证通过");
    assert_eq!(stats.events, 4);
    assert_eq!(stats.appends, 2);
    assert_eq!(stats.truncates, 1);
    assert_eq!(stats.forks, 1);
    assert_eq!(stats.rows_total, 4);
}

/// 篡改任何一行 → 该行起 hash 断裂，verify 红。
#[test]
fn tampered_line_fails_verify() {
    let (_d, path) = temp_ledger("tamper");

    let r1 = chat_row("user", "原始内容");
    record_entry_at(
        &path,
        LedgerOp::Append,
        "chat_log",
        vec![LedgerRow::from_chat_row(&r1, false)],
    )
    .unwrap();
    record_entry_at(&path, LedgerOp::Append, "chat_log", vec![]).unwrap();

    // 篡改第一行正文相关字段（改 actor——append 行的 sha 不含它，但 hash 链含）。
    let text = std::fs::read_to_string(&path).unwrap();
    let tampered = text.replace("\"actor\":\"chat_log\"", "\"actor\":\"forger\"");
    assert_ne!(text, tampered, "替换必须命中");
    std::fs::write(&path, tampered).unwrap();

    let err = ledger_verify(&path).expect_err("篡改必须红");
    assert!(err.contains("line 1"), "应定位到第 1 行，实际：{err}");
}

/// 删除中间一行 → 后继行 prev_hash 断裂。
#[test]
fn deleted_middle_line_fails_verify() {
    let (_d, path) = temp_ledger("delete");

    record_entry_at(&path, LedgerOp::Append, "chat_log", vec![]).unwrap();
    record_entry_at(&path, LedgerOp::Append, "chat_log", vec![]).unwrap();
    record_entry_at(&path, LedgerOp::Append, "chat_log", vec![]).unwrap();

    let lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(lines.len(), 3);
    // 删第 2 行：第 3 行的 seq=2 但链位置=1 → seq 闸先红（prev_hash 同断）。
    std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
    let err = ledger_verify(&path).expect_err("删行必须红");
    assert!(
        err.contains("seq 断裂") || err.contains("prev_hash"),
        "应报链断裂，实际：{err}"
    );
}

/// truncate 行的全文可找回（被删行原文 = 账本 content 字段）。
#[test]
fn truncated_rows_recoverable_from_ledger() {
    let (_d, path) = temp_ledger("recover");

    let deleted1 = chat_row("user", "rewind 删掉的提问");
    let deleted2 = chat_row("assistant", "rewind 删掉的回复");
    record_entry_at(
        &path,
        LedgerOp::Truncate,
        "chat_log",
        vec![
            LedgerRow::from_chat_row(&deleted1, true),
            LedgerRow::from_chat_row(&deleted2, true),
        ],
    )
    .unwrap();

    // 从账本找回被删行原文。
    let text = std::fs::read_to_string(&path).unwrap();
    let entry: LedgerEntry = serde_json::from_str(text.trim()).unwrap();
    let contents: Vec<&str> = entry
        .body
        .rows
        .iter()
        .filter_map(|r| r.content.as_deref())
        .collect();
    assert_eq!(contents, vec!["rewind 删掉的提问", "rewind 删掉的回复"]);
    assert!(ledger_verify(&path).is_ok());
}

/// 行内正文与 sha 矛盾（截断行被手改）→ verify 红。
#[test]
fn content_sha_mismatch_fails_verify() {
    let (_d, path) = temp_ledger("shamismatch");

    let deleted = chat_row("user", "原文");
    record_entry_at(
        &path,
        LedgerOp::Truncate,
        "chat_log",
        vec![LedgerRow::from_chat_row(&deleted, true)],
    )
    .unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    let tampered = text.replace("原文", "改过的假原文");
    std::fs::write(&path, tampered).unwrap();
    // 截断行被手改 → 正文字节变 → 链 hash 闸先红；sha 闸是 hash 巧合通过
    // 时的第二道（行内正文 vs content_sha256）。
    let err = ledger_verify(&path).expect_err("正文与 sha 矛盾必须红");
    assert!(
        err.contains("sha") || err.contains("hash 不匹配"),
        "应报 sha 不一致或链 hash 断裂，实际：{err}"
    );
}

/// append 行不带正文（内容策略：append sha-only）。
#[test]
fn append_rows_are_sha_only() {
    let (_d, path) = temp_ledger("appendonly");

    let r = chat_row("user", "正文不应入账本");
    record_entry_at(
        &path,
        LedgerOp::Append,
        "chat_log",
        vec![LedgerRow::from_chat_row(&r, false)],
    )
    .unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("正文不应入账本"), "append 行不得携带正文");
    let entry: LedgerEntry = serde_json::from_str(text.trim()).unwrap();
    assert!(entry.body.rows[0].content.is_none());
    assert_eq!(
        entry.body.rows[0].content_sha256,
        sha256_hex("正文不应入账本".as_bytes())
    );
}

/// 创世行 prev_hash = 全 0；空文件 verify = 全零统计不报错。
#[test]
fn genesis_and_empty_file() {
    let (_d, path) = temp_ledger("genesis");

    record_entry_at(&path, LedgerOp::Append, "chat_log", vec![]).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let entry: LedgerEntry = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(entry.prev_hash, GENESIS_PREV_HASH);
    assert_eq!(entry.body.seq, 0);

    let empty = _d.path().join("empty.jsonl");
    std::fs::write(&empty, "").unwrap();
    let stats = ledger_verify(&empty).unwrap();
    assert_eq!(stats.events, 0);
}

// ---------------------------------------------------------------------------
// 并发记账竞速（读链尾→追加两步无锁 → 重复 seq → verify
// 永久红）。两则：确定性红证（手工复现旧交错形态）+ 并发回归锁（持锁实现
// 下高竞争全链干净）。
// ---------------------------------------------------------------------------

/// 红证（确定性）：模拟修复前 record_entry_at 的无锁交错——两个并发方都
/// 在同一链尾坐标上构造并追加 → 第二条 seq 与前驱断裂，verify 红。修复后
/// 该交错只能由测试直接拼装底层 helper 产生，公共入口已全程持锁。
#[test]
fn bug_121_tail_race_interleave_breaks_chain() {
    let (_d, path) = temp_ledger("bug121_race_red");
    record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap();

    // 交错窗口：两个并发方先后读到同一链尾坐标（修复前 read_chain_tail
    // 无锁，这正是并发时的真实读数）。
    let tail_a = read_chain_tail_healed(&path).unwrap();
    let tail_b = read_chain_tail_healed(&path).unwrap();
    assert_eq!(tail_a.0, tail_b.0, "交错窗口内两方读到同一 seq");

    // 各自构造并追加（旧实现的两步在对方 write 之前都已完成读尾）。
    let line_a = build_entry_line(tail_a.0, &tail_a.1, LedgerOp::Append, "t", vec![]).unwrap();
    let line_b = build_entry_line(tail_b.0, &tail_b.1, LedgerOp::Append, "t", vec![]).unwrap();
    append_entry_line(&path, &line_a).unwrap();
    append_entry_line(&path, &line_b).unwrap();

    let err = ledger_verify(&path).unwrap_err();
    assert!(err.contains("seq"), "重复 seq 应断链，got: {err}");
}

/// 回归锁：8 线程 × 25 条屏障起跑并发记账（修复前概率性产生重复 seq →
/// verify 永久红），持锁实现下全链干净、计数准确。
#[test]
fn bug_121_concurrent_records_keep_chain_consistent() {
    let (_d, path) = temp_ledger("bug121_concurrent");
    const THREADS: usize = 8;
    const PER: usize = 25;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let mut handles = Vec::new();
    for t in 0..THREADS {
        let barrier = barrier.clone();
        let path = path.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for i in 0..PER {
                record_entry_at(
                    &path,
                    LedgerOp::Append,
                    "t",
                    vec![LedgerRow {
                        role: "user".to_string(),
                        content_sha256: sha256_hex(format!("t{t}-{i}").as_bytes()),
                        content: None,
                    }],
                )
                .unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let stats = ledger_verify(&path).unwrap();
    assert_eq!(stats.events, (THREADS * PER) as u64);
    assert_eq!(stats.appends, (THREADS * PER) as u64);
}

// ---------------------------------------------------------------------------
// 撕裂尾自愈（BUG-20261113-168）：崩溃残迹（半行/丢换行拼行/尾部坏行）
// 不得永久砖化追加路径；中间损坏拒绝自动修复。
// ---------------------------------------------------------------------------

/// 无换行半行残迹：下一笔记账自愈截断 + 正常追加，verify 绿。
#[test]
fn torn_tail_partial_line_heals_and_appends() {
    let (_d, path) = temp_ledger("torn_partial");
    record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap();

    // 模拟 writeln 中途崩溃：追加半行 JSON（无换行）。marker 取名避开
    // 正常条目字段（prev_hash 创世值本身含 "0000" 前缀，不能当残迹探针）。
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    f.write_all(b"{\"zz_torn_marker\":1").unwrap();
    drop(f);

    // 原实现：读尾解析 Err → 追加永久失败。修复后：自愈 + 追加成功。
    record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap();

    let stats = ledger_verify(&path).expect("自愈后全链必须可验证");
    assert_eq!(stats.events, 2);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("zz_torn_marker"), "残迹字节必须被截掉");
}

/// 丢换行 + 下一笔拼行：好行被拼进残行一起不可解析（完整行形态的尾部
/// 坏行）——同样属于尾部自愈区，截掉后链恢复可验证。
#[test]
fn torn_tail_merged_line_heals_and_appends() {
    let (_d, path) = temp_ledger("torn_merged");

    // 手工拼装崩溃现场：好行（无换行）+ 残迹（无换行）+ 换行结尾的坏行。
    let good = build_entry_line(0, GENESIS_PREV_HASH, LedgerOp::Append, "t", vec![]).unwrap();
    {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(good.as_bytes()).unwrap(); // 无 \n：content 写完、\n 未写
        f.write_all(b"{\"seq\":1,\"op\":\"append\"") // 下一笔残迹直接拼上
            .unwrap();
        f.write_all(b"\n{\"garbage\":true}\n") // 再一笔完整坏行
            .unwrap();
    }

    record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap();
    let stats = ledger_verify(&path).expect("拼行残迹自愈后必须可验证");
    // 好行被拼进残行一起不可解析：整条随残迹截掉（账本 best-effort 旁证，
    // WARN 带丢弃字节数；chat_log 本体才是真相源），链从 GENESIS 重启生长。
    assert_eq!(stats.events, 1);
}

/// 丢换行的完整好条目（writeln 的 content 与 "\n" 两次 write_all 之间的
/// 崩溃窗）：补换行恢复原条目而非截断——证据零丢失，链连续性保持。
#[test]
fn tail_entry_missing_newline_is_restored_not_truncated() {
    let (_d, path) = temp_ledger("torn_newline_restore");

    // 崩溃现场：完整条目字节已全部落盘、唯独换行没写。
    let good = build_entry_line(0, GENESIS_PREV_HASH, LedgerOp::Append, "t", vec![]).unwrap();
    std::fs::write(&path, good.as_bytes()).unwrap(); // 无 \n

    // 自愈 = 补换行恢复；随后记账接续该条目的链坐标（seq=1）。
    record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap();

    let stats = ledger_verify(&path).expect("恢复条目后全链必须可验证");
    assert_eq!(stats.events, 2, "原条目必须被恢复而非截断");
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "恢复行 + 新追加行");
    assert_eq!(lines[0], good, "恢复行字节与原条目一致");
}

/// 尾区垃圾+伪造条目（坏完整行 + 无换行合法条目拼在尾区）：整体不可解析
/// → 按崩溃残迹区截掉，锚点好行不受影响、链照常生长（设计语义钉死：
/// 尾区自愈只认「最后好行之后的字节皆残迹」；伪造条目被静默丢弃是此
/// 取舍的接受面——写文件对手本可重写全文件，链无外部锚，非对抗防线）。
/// 「坏完整行之后接好完整行」的真篡改形态由 middle_corruption 用例钉死。
#[test]
fn garbage_plus_forged_tail_is_truncated_and_chain_grows() {
    let (_d, path) = temp_ledger("garbage_forged_tail");
    record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap();

    let (_, tail_hash) = read_chain_tail_healed(&path).unwrap();
    let forged = build_entry_line(1, &tail_hash, LedgerOp::Append, "t", vec![]).unwrap();
    {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"{\"garbage\":true}\n").unwrap();
        f.write_all(forged.as_bytes()).unwrap(); // 无 \n：落在尾区
    }

    record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap();
    let stats = ledger_verify(&path).expect("截断自愈后全链必须可验证");
    assert_eq!(stats.events, 2, "锚点好行 + 自愈后新追加的一笔");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("garbage"), "垃圾行必须被截掉");
    assert_eq!(text.lines().count(), 2, "只许剩锚点好行 + 新追加行");
}

/// 中间损坏（坏行之后还有好行 = 篡改/介质损坏形态）：拒绝自动修复，
/// 原 Err 返回且文件字节零改动——静默截断会吃掉篡改证据。
#[test]
fn middle_corruption_hard_fails_without_repair() {
    let (_d, path) = temp_ledger("middle_corrupt");
    record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap();
    let line2 = build_entry_line(
        1,
        &{
            let (_, h) = read_chain_tail_healed(&path).unwrap();
            h
        },
        LedgerOp::Append,
        "t",
        vec![],
    )
    .unwrap();
    {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(line2.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
    }
    // 在中间插入坏行（好行之后）：把 line2 重写为 line2 前置坏行。
    let full = std::fs::read_to_string(&path).unwrap();
    let mut lines = full.lines();
    let first = lines.next().unwrap().to_string();
    let second = lines.next().unwrap().to_string();
    std::fs::write(&path, format!("{first}\n{{\"tampered\":1}}\n{second}\n")).unwrap();
    let before = std::fs::read(&path).unwrap();

    let err = record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap_err();
    assert!(
        err.contains("拒绝自动修复"),
        "中间损坏必须硬失败，got: {err}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "中间损坏形态下文件字节零改动"
    );
}

/// 空白行跳过口径与 verify 一致：尾部孤立换行（空白残迹）不误伤链。
#[test]
fn trailing_blank_lines_are_neutral() {
    let (_d, path) = temp_ledger("blank_tail");
    record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap();
    {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(b"\n\n").unwrap();
    }
    // 追加锚点钉在最后好行终点：空白尾巴被截掉，链照常生长。
    record_entry_at(&path, LedgerOp::Append, "t", vec![]).unwrap();
    let stats = ledger_verify(&path).unwrap();
    assert_eq!(stats.events, 2);
}
