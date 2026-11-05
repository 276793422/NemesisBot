//! （2026-11-01）：审批卡 diff 预览单测——分派表按序首中、四种形态
//! 全命中、失败路径诚实注记、截断与二进制边界。

use super::*;

// ---------- 分派表 ----------

#[test]
fn dispatch_non_object_args_returns_none() {
    // args 缺失 / 非对象 = 不产预览（保持现状，卡片无预览区）。
    assert!(build_preview("file_write", "t", &serde_json::Value::Null).is_none());
    assert!(build_preview("file_write", "t", &serde_json::json!("str")).is_none());
    assert!(build_preview("file_write", "t", &serde_json::json!([1])).is_none());
}

#[test]
fn dispatch_command_form_hits_before_others_present() {
    // 只有 command 形态命中；path+content 同场时不抢（按序首中是确定性契约）。
    let args = serde_json::json!({ "command": "ls -la", "cwd": "/tmp" });
    let (text, fmt) = build_preview("process_exec", "t", &args).unwrap();
    assert_eq!(fmt, "text");
    assert!(text.contains("ls -la"));
    assert!(text.contains("/tmp"));
}

#[test]
fn dispatch_edits_takes_priority_over_write_form() {
    // multiedit 的 args 可能同时带 path 之外的字段——edits[] 在分派表最前。
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.txt");
    std::fs::write(&p, "hello\n").unwrap();
    let args = serde_json::json!({
        "edits": [{ "path": p.to_str().unwrap(), "old_text": "hello", "new_text": "world" }]
    });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "diff");
    assert!(text.contains("-hello"));
    assert!(text.contains("+world"));
}

#[test]
fn dispatch_write_form_path_content() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("new.txt");
    let args = serde_json::json!({ "path": p.to_str().unwrap(), "content": "abc\n" });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "diff");
    assert!(text.contains("+abc"));
}

#[test]
fn dispatch_edit_form_old_new_text() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("e.txt");
    std::fs::write(&p, "one two three\n").unwrap();
    let args = serde_json::json!({
        "path": p.to_str().unwrap(), "old_text": "two", "new_text": "2"
    });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "diff");
    assert!(text.contains("-one two three"));
    assert!(text.contains("+one 2 three"));
}

#[test]
fn dispatch_no_matching_form_returns_none() {
    // 对象 args 但无任何已知形态字段 = 不产预览。
    let args = serde_json::json!({ "foo": 1 });
    assert!(build_preview("file_write", "t", &args).is_none());
}

#[test]
fn dispatch_confusing_args_content_wins_over_command() {
    // 对抗 （形态混淆）：args 同时带 content 与 command——分派表按序首中
    // （write 先于 command），一次只产一种 preview，不拼接。
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("mix.txt");
    let args = serde_json::json!({
        "path": p.to_str().unwrap(),
        "content": "hello\n",
        "command": "rm -rf /"
    });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "diff", "write 形态胜出");
    assert!(text.contains("+hello"));
    assert!(!text.contains("将执行命令"), "不拼接命令现场");
    assert!(!text.contains("rm -rf /"), "命令文本不混入");
}

// ---------- write 形态 ----------

#[test]
fn write_new_file_full_addition_diff() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("fresh.txt");
    let args = serde_json::json!({ "path": p.to_str().unwrap(), "content": "l1\nl2\n" });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "diff");
    // 空文件行数修正：空侧记 0。
    assert!(text.contains("@@ -0,0 +1,2 @@"), "hunk header: {text}");
    assert!(text.starts_with("--- a/"));
    assert!(text.contains("+++ b/"));
    assert!(text.contains("+l1"));
    assert!(text.contains("+l2"));
    assert!(!text.contains("-l1"));
}

#[test]
fn write_existing_file_replacement_diff() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("exist.txt");
    std::fs::write(&p, "keep\nold value\nkeep tail\n").unwrap();
    let args = serde_json::json!({ "path": p.to_str().unwrap(), "content": "keep\nnew value\nkeep tail\n" });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "diff");
    assert!(text.contains("-old value"));
    assert!(text.contains("+new value"));
    // 上下文行（空格前缀）在场。
    assert!(text.contains("\n keep\n"));
}

#[test]
fn write_binary_target_annotated_no_diff() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("bin.dat");
    let mut bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
    bytes.extend_from_slice(&[0u8; 16]); // NUL 在 8KB 探测窗内 → 二进制
    std::fs::write(&p, &bytes).unwrap();
    let args = serde_json::json!({ "path": p.to_str().unwrap(), "content": "text\n" });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "text");
    assert!(text.contains("二进制文件"));
}

#[test]
fn write_oversized_target_downgrades_to_note() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("big.txt");
    // >1MB 现盘文件：旧侧不读入，注记形态降级。
    let big = "x".repeat(MAX_DIFF_SOURCE_BYTES as usize + 1);
    std::fs::write(&p, &big).unwrap();
    let args = serde_json::json!({ "path": p.to_str().unwrap(), "content": "small\n" });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "text");
    assert!(text.contains("较大"));
    assert!(!text.contains("+small"));
}

#[test]
fn write_read_failure_degrades_to_note_with_new_content() {
    // 目录冒充文件路径：metadata 成功但 read 失败 → 注记 + 新内容围栏。
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("as_dir");
    std::fs::create_dir(&p).unwrap();
    let args = serde_json::json!({ "path": p.to_str().unwrap(), "content": "secret-ish\n" });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "text");
    assert!(text.contains("读取当前文件失败"));
    assert!(text.contains("secret-ish"));
}

// ---------- edit 形态 ----------

#[test]
fn edit_old_text_miss_honest_note() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("m.txt");
    std::fs::write(&p, "actual content\n").unwrap();
    let args = serde_json::json!({
        "path": p.to_str().unwrap(), "old_text": "NOT PRESENT", "new_text": "x"
    });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "text");
    // 预览忠实原则：不命中就如实说执行时会报错，不假装能替换。
    assert!(text.contains("未在当前文件中命中"));
    assert!(text.contains("执行时将报错"));
}

#[test]
fn edit_missing_file_degrades_to_note() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("nope.txt");
    let args = serde_json::json!({
        "path": p.to_str().unwrap(), "old_text": "a", "new_text": "b"
    });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "text");
    assert!(text.contains("读取当前文件失败"));
}

// ---------- multiedit 形态 ----------

#[test]
fn multiedit_same_file_sequential_accumulation() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("seq.txt");
    std::fs::write(&p, "A B C\n").unwrap();
    // 同文件两条按序累积：第二条看到第一条的结果。
    let args = serde_json::json!({
        "edits": [
            { "path": p.to_str().unwrap(), "old_text": "A", "new_text": "X" },
            { "path": p.to_str().unwrap(), "old_text": "X B", "new_text": "Y" }
        ]
    });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "diff");
    assert!(text.contains("-A B C"));
    assert!(text.contains("+Y C"));
}

#[test]
fn multiedit_miss_enters_notes_not_diff() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("n.txt");
    std::fs::write(&p, "ok\n").unwrap();
    let args = serde_json::json!({
        "edits": [
            { "path": p.to_str().unwrap(), "old_text": "ok", "new_text": "done" },
            { "path": p.to_str().unwrap(), "old_text": "MISS", "new_text": "x" }
        ]
    });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "diff");
    assert!(text.contains("+done"));
    assert!(text.contains("未命中"));
    assert!(text.contains("整批不落盘"));
}

#[test]
fn multiedit_empty_or_malformed_returns_none() {
    // 空 edits 数组 = 不产预览。
    assert!(build_preview("file_write", "t", &serde_json::json!({ "edits": [] })).is_none());
    // 条目形态不齐（缺 new_text）= 诚实不产。
    let args = serde_json::json!({ "edits": [{ "path": "x", "old_text": "a" }] });
    assert!(build_preview("file_write", "t", &args).is_none());
}

// ---------- command 形态 ----------

#[test]
fn command_preview_shows_verbatim_command_and_cwd() {
    let args = serde_json::json!({ "command": "echo $SECRET_TOKEN", "cwd": "C:/work" });
    let (text, fmt) = build_preview("process_exec", "t", &args).unwrap();
    assert_eq!(fmt, "text");
    // 命令逐字展示（忠实原则）——env 变量名在命令里就显示变量名。
    assert!(text.contains("echo $SECRET_TOKEN"));
    assert!(text.contains("C:/work"));
}

#[test]
fn command_preview_default_cwd_placeholder() {
    let args = serde_json::json!({ "command": "dir" });
    let (text, _) = build_preview("process_exec", "t", &args).unwrap();
    assert!(text.contains("(默认工作目录)"));
    // 空 cwd 同样落占位符。
    let args2 = serde_json::json!({ "command": "dir", "cwd": "" });
    let (text2, _) = build_preview("process_exec", "t", &args2).unwrap();
    assert!(text2.contains("(默认工作目录)"));
}

// ---------- 截断与边界 ----------

#[test]
fn truncate_large_multibyte_preview_at_char_boundary() {
    // 中文每字 3 字节：造一段超过 64KB 的多字节内容，截断点必须落在
    // char boundary 上且尾注记在场。
    let line = "中文内容行\n";
    let mut content = String::new();
    while content.len() < MAX_PREVIEW_BYTES + 4096 {
        content.push_str(line);
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("cn.txt");
    let args = serde_json::json!({ "path": p.to_str().unwrap(), "content": content });
    let (text, fmt) = build_preview("file_write", "t", &args).unwrap();
    assert_eq!(fmt, "diff");
    assert!(text.len() <= MAX_PREVIEW_BYTES + 128, "截断后不应远超上限");
    assert!(text.contains("[预览已截断"));
    // 末尾不是劈开的 UTF-8：能按 char 迭代即边界安全（from_utf8 校验）。
    assert!(std::str::from_utf8(text.as_bytes()).is_ok());
}

#[test]
fn unified_diff_no_change_returns_empty() {
    assert_eq!(unified_diff("same\n", "same\n", "x"), "");
    assert_eq!(unified_diff("", "", "x"), "");
}

#[test]
fn unified_diff_exact_minimal_form() {
    // 标准最小 unified diff：上下文计入 hunk 头行数；变化行不重复出现在
    // 上下文里（回归锁：前导上下文曾把 -行本身包进去、尾随上下文曾把整段
    // 公共后缀全吐出）。
    let d = unified_diff(
        "keep\nold value\nkeep tail\n",
        "keep\nnew value\nkeep tail\n",
        "f.txt",
    );
    assert_eq!(
        d,
        "--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n keep\n-old value\n+new value\n keep tail\n"
    );
}

#[test]
fn unified_diff_context_lines_bounded() {
    // 长文件中段单行变化：前后各带 1 行上下文，不是全量行。
    let old: String = (0..20).map(|i| format!("line{i}\n")).collect();
    let new = old.replace("line10", "LINE10");
    let d = unified_diff(&old, &new, "f.txt");
    let body_lines = d.lines().count();
    // 头 2 + hunk 1 + 上下文 2 + 变更 2 = 7 行；全量 21 行 = 错。
    assert!(
        body_lines <= 9,
        "diff 应只带紧邻上下文，实际 {body_lines} 行：\n{d}"
    );
    // 变化行只以 +/- 形态出现一次，不以上下文形态重复。
    assert_eq!(d.matches("\n-line10\n").count(), 1);
    assert_eq!(d.matches("\n+LINE10\n").count(), 1);
    assert!(
        !d.contains("\n line10\n"),
        "旧变化行不得作为上下文重复：\n{d}"
    );
    assert!(d.contains("\n line9\n"), "前导上下文 1 行");
    assert!(d.contains("\n line11\n"), "尾随上下文 1 行");
    assert!(!d.contains("\n line19\n"), "远端尾行不得混入：\n{d}");
}

#[test]
fn human_size_units() {
    assert_eq!(human_size(512), "512 B");
    assert_eq!(human_size(2048), "2.0 KB");
    assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
}
