//! R100（会话持久化面审计）：jsonl 追加的行原子性——并发写同一会话不得撕行。
//!
//! 根因：`writeln!(file, "{}", entry)` 对 `serde_json::Value` 走 Display 的
//! 逐片段 `write_all`（append 模式下每个片段独立定位 EOF），同会话并发写
//! （spawned 续行 final 落盘 vs 主循环下一条 user 行）可把别人的行插进本行
//! JSON 片段中间 → 行撕裂（解析失败/缺字段）→ 消息从 UI 消失、rewind
//! 行号坐标漂移。修复 = 整行（含换行）序列化成 `String` 后单次 `write_all`
//! （O_APPEND / FILE_APPEND_DATA 每次 write 的定位是原子的，行间只可能
//! 排序交错、不可能半行插入）。
//!
//! 真红注记：撕裂是时序竞态，旧代码下本测试红是**概率性**的（多字段条目
//! + 多线程 + 高频次提高命中面）；修复后按 OS 追加原子性应当确定性绿。

use super::*;
use std::sync::Barrier;

fn key(tag: &str) -> String {
    format!("test:r100:{}_{}", tag, std::process::id())
}

#[test]
fn concurrent_appends_never_tear_lines() {
    let k = key("concurrent");
    delete_chat_log(&k);

    const THREADS: usize = 8;
    const LINES: usize = 200;
    // 多字段条目加宽旧实现的逐片段写窗口（images 数组 + cron 标记 + 模型徽标
    // + checkpoint_turn，一条 40+ 个 Display 片段）。
    let images: Vec<String> = (0..12)
        .map(|i| format!("/tmp/r100_img_{}.png", i))
        .collect();
    let barrier = std::sync::Arc::new(Barrier::new(THREADS));
    let mut handles = Vec::new();
    for t in 0..THREADS {
        let barrier = barrier.clone();
        let k = k.clone();
        let images = images.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for i in 0..LINES {
                let meta = ChatLogMeta {
                    model: Some("test/model-1"),
                    cron_job_id: Some("job-1"),
                    cron_job_name: Some("r100-tear"),
                    images: &images,
                    file_changes: &[],
                    deliverables: &[],
                    checkpoint_turn: Some(i),
                    steer: false,
                };
                append_chat_log_meta(&k, "assistant", &format!("t{}-line{}", t, i), &meta);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 断言一：行数精确（多余短行 = 撕裂把一行劈成两段的直接证据）。
    let text = std::fs::read_to_string(log_path(&k)).expect("日志文件必须存在");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        THREADS * LINES,
        "行数必须恰好 {}（撕裂会把一行劈开或多出空行），实际 {}",
        THREADS * LINES,
        lines.len()
    );
    // 断言二：逐行完整可解析且多字段齐备（撕裂行 parse 失败或缺字段）。
    for (idx, line) in lines.iter().enumerate() {
        let v: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("行 {} 必须可解析（撕裂现场: {:?}）: {}", idx, line, e));
        assert_eq!(
            v.get("cron_job_name").and_then(Value::as_str),
            Some("r100-tear"),
            "行 {} cron_job_name 缺失 = 撕裂",
            idx
        );
        assert!(
            v.get("images").map(Value::is_array).unwrap_or(false),
            "行 {} images 缺失 = 撕裂",
            idx
        );
        assert!(
            v.get("content")
                .and_then(Value::as_str)
                .map(|c| c.starts_with('t') && c.contains("-line"))
                .unwrap_or(false),
            "行 {} content 形态异常 = 撕裂: {:?}",
            idx,
            v.get("content")
        );
    }
    delete_chat_log(&k);
    // delete_chat_log 不清事件账本（jsonl/边界侧车/标题元数据三件套之外），
    // 账本文件按 t6 惯例单独摘除，防绿跑逐次泄漏。
    let _ = std::fs::remove_file(crate::event_ledger::ledger_path(&k));
}
