//! ReadFileTool 读侧内存闸测试（BUG-20261110-041 回归锁）：
//! 超 20MB 帽文件拒绝（含 offset/limit 分段请求），帽内文件行为不变。
//! 同族横向覆盖（「每修一 BUG 必横向扫同类」）：edit_file / multiedit
//! 编辑全量加载拒绝 + write_file 超大旧文件跳过 diff 读（写入不受影响）。

use super::*;
use tempfile::TempDir;

#[tokio::test]
async fn test_read_file_oversized_rejected() {
    let tmp = TempDir::new().unwrap();
    let big = tmp.path().join("big.txt");
    // 帽 20MB：写 cap+1 字节触发拒绝。
    let payload = vec![b'a'; 20_000_000 + 1];
    tokio::fs::write(&big, &payload).await.unwrap();

    let tool = ReadFileTool::default();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // 无参全量读：拒绝。
    let args = serde_json::json!({ "path": big.to_string_lossy() }).to_string();
    let err = tool.execute(&args, &ctx).await.unwrap_err();
    assert!(err.contains("File too large for read_file"), "got: {err}");
    assert!(err.contains("cap 20MB"), "got: {err}");
    assert!(
        err.contains("exec"),
        "报错必须指引字节口径替代工具，got: {err}"
    );

    // offset/limit 分段请求：同样拒绝（字符口径分段也要全量加载）。
    let args = serde_json::json!({
        "path": big.to_string_lossy(),
        "offset": 0,
        "limit": 100
    })
    .to_string();
    let err = tool.execute(&args, &ctx).await.unwrap_err();
    assert!(err.contains("File too large for read_file"), "got: {err}");
}

#[tokio::test]
async fn test_read_file_at_cap_still_works() {
    let tmp = TempDir::new().unwrap();
    let edge = tmp.path().join("edge.txt");
    // 恰好帽内（20MB 整）：正常全量读，字节一致。
    let payload = vec![b'b'; 20_000_000];
    tokio::fs::write(&edge, &payload).await.unwrap();

    let tool = ReadFileTool::default();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let args = serde_json::json!({ "path": edge.to_string_lossy() }).to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    assert_eq!(result.len(), 20_000_000);
    assert!(result.bytes().all(|b| b == b'b'));
}

#[tokio::test]
async fn test_read_file_segmentation_unchanged_under_cap() {
    let tmp = TempDir::new().unwrap();
    let f = tmp.path().join("seg.txt");
    tokio::fs::write(&f, "hello 世界 segmentation")
        .await
        .unwrap();

    let tool = ReadFileTool::default();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let args = serde_json::json!({
        "path": f.to_string_lossy(),
        "offset": 6,
        "limit": 3
    })
    .to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    // 字符口径切片（多字节安全）不受内存闸影响：offset=6 跳过
    // "hello "，limit=3 取「世界 」（第 3 字符是空格）。
    assert!(result.contains("[read_file 分段]"), "got: {result}");
    assert!(result.contains("chars_returned=3"), "got: {result}");
    assert!(result.contains("世界"), "got: {result}");
}

#[tokio::test]
async fn test_edit_file_oversized_rejected() {
    let tmp = TempDir::new().unwrap();
    let big = tmp.path().join("big.txt");
    let payload = vec![b'a'; 20_000_000 + 1];
    tokio::fs::write(&big, &payload).await.unwrap();

    let tool = EditFileTool::default();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let args = serde_json::json!({
        "path": big.to_string_lossy(),
        "old_text": "aaa",
        "new_text": "bbb"
    })
    .to_string();
    let err = tool.execute(&args, &ctx).await.unwrap_err();
    assert!(err.contains("File too large for edit_file"), "got: {err}");
    assert!(err.contains("cap 20MB"), "got: {err}");
    assert!(
        err.contains("exec"),
        "报错必须指引字节口径替代工具，got: {err}"
    );
}

#[tokio::test]
async fn test_multiedit_oversized_rejected_atomically() {
    let tmp = TempDir::new().unwrap();
    // 大文件 + 小文件成批：大文件失败必须整批不落盘（原子性）。
    let big = tmp.path().join("big.txt");
    let small = tmp.path().join("small.txt");
    tokio::fs::write(&big, vec![b'a'; 20_000_000 + 1])
        .await
        .unwrap();
    tokio::fs::write(&small, "small content").await.unwrap();

    let tool = MultiEditTool::default();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    let args = serde_json::json!({
        "edits": [
            {"path": small.to_string_lossy(), "old_text": "small", "new_text": "SMALL"},
            {"path": big.to_string_lossy(), "old_text": "aaa", "new_text": "bbb"}
        ]
    })
    .to_string();
    let err = tool.execute(&args, &ctx).await.unwrap_err();
    assert!(err.contains("multiedit aborted"), "got: {err}");
    assert!(err.contains("File too large for multiedit"), "got: {err}");
    // 原子性：小文件编辑不得落盘。
    let after = tokio::fs::read_to_string(&small).await.unwrap();
    assert_eq!(after, "small content", "整批必须不落盘");
}

#[tokio::test]
async fn test_write_file_oversized_old_skips_diff_read_still_writes() {
    let tmp = TempDir::new().unwrap();
    let big = tmp.path().join("big.txt");
    tokio::fs::write(&big, vec![b'a'; 20_000_000 + 1])
        .await
        .unwrap();

    let tool = WriteFileTool::default();
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");

    // 覆盖超大旧文件：写入必须成功（截断大文件是合法操作），且不因
    // diff 读而全量加载旧内容。
    let args = serde_json::json!({
        "path": big.to_string_lossy(),
        "content": "fresh small content"
    })
    .to_string();
    let result = tool.execute(&args, &ctx).await.unwrap();
    assert!(result.contains("Successfully wrote"), "got: {result}");
    assert!(
        !result.contains("```diff"),
        "超大旧文件必须跳过 diff 读，got: {result}"
    );
    let after = tokio::fs::read_to_string(&big).await.unwrap();
    assert_eq!(after, "fresh small content");
}
