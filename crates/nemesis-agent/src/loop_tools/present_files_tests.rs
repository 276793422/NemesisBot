//! N5（2026-10-13 机会项）: present_files 交付物登记工具单元测试。
//!
//! 覆盖：注册门控（workspace+bucket 双 Some → 注册 / 任一 None → 不注册）、
//! schema（files 必填数组）、解析（path 必填 / title 缺省取文件名 / note
//! 可缺省）、存在性验真 all-or-nothing（缺失清单诚实回灌、半登记不发生）、
//! 相对路径按 workspace 解析 / 绝对路径放行、session_key 分桶、重复登记
//! 去重（dedup_keep-first）。

use super::{PresentFilesTool, SharedToolConfig, TurnDeliverablesBucket, register_shared_tools};
use crate::chat_log::PresentedFile;
use crate::context::RequestContext;
use crate::r#loop::Tool;
use std::sync::Arc;

fn ctx(session_key: &str) -> RequestContext {
    RequestContext::new("web", "chat1", "user1", session_key)
}

fn fresh_bucket() -> TurnDeliverablesBucket {
    Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()))
}

/// 临时工作区（每测试唯一，测试结束自清理；NTFS 延迟删除容忍——
/// 断言不依赖目录消失时序）。
fn temp_ws(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "n5_present_files_{}_{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create temp workspace");
    dir
}

fn tool_for(ws: &std::path::Path, bucket: TurnDeliverablesBucket) -> PresentFilesTool {
    PresentFilesTool::new(ws.to_string_lossy().to_string(), bucket)
}

// ---------------------------------------------------------------------------
// 注册门控
// ---------------------------------------------------------------------------

#[test]
fn register_with_workspace_and_bucket_inserts_present_files() {
    let bucket = fresh_bucket();
    let cfg = SharedToolConfig {
        workspace: Some(".".to_string()),
        turn_deliverables: Some(bucket),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(
        tools.contains_key("present_files"),
        "workspace+bucket 均接线 → present_files 注册"
    );
}

#[test]
fn register_without_bucket_omits_present_files() {
    // 只给 workspace 不给桶（register_default_tools 基线形态）→ 不注册。
    let cfg = SharedToolConfig {
        workspace: Some(".".to_string()),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(!tools.contains_key("present_files"));
}

// ---------------------------------------------------------------------------
// schema
// ---------------------------------------------------------------------------

#[test]
fn schema_requires_files_array() {
    let bucket = fresh_bucket();
    let tool = tool_for(std::path::Path::new("."), bucket);
    let schema = tool.parameters();
    assert_eq!(schema["required"][0], "files");
    assert_eq!(schema["properties"]["files"]["type"], "array");
    assert_eq!(
        schema["properties"]["files"]["items"]["required"][0], "path",
        "条目级 path 必填"
    );
    let desc = tool.description();
    assert!(
        desc.contains("deliverable") && desc.contains("exist"),
        "description 应说明登记语义与存在性前提: {desc}"
    );
}

// ---------------------------------------------------------------------------
// execute：解析 + 存在性 + 入桶
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registers_relative_paths_against_workspace_with_title_default() {
    let ws = temp_ws("rel");
    std::fs::write(ws.join("report.md"), "# report").expect("seed file");

    let bucket = fresh_bucket();
    let tool = tool_for(&ws, bucket.clone());
    let out = tool
        .execute(
            r#"{"files": [{"path": "report.md", "note": "final summary"}]}"#,
            &ctx("agent:main:session:s1"),
        )
        .await
        .expect("相对路径存在 → Ok");
    assert!(out.contains("1 deliverable"), "回灌登记数: {out}");

    let b = bucket.lock();
    let v = b.get("agent:main:session:s1").expect("session 桶存在");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].path, "report.md", "桶内保留登记原文（相对形态）");
    assert_eq!(v[0].title, "report.md", "title 缺省取文件名");
    assert_eq!(v[0].note, "final summary");

    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn absolute_paths_are_accepted() {
    let ws = temp_ws("abs");
    let outside = std::env::temp_dir().join(format!(
        "n5_outside_{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&outside, "x").expect("seed outside file");

    let bucket = fresh_bucket();
    let tool = tool_for(&ws, bucket.clone());
    let args = format!(
        r#"{{"files": [{{"path": {}}}]}}"#,
        serde_json::to_string(&outside).unwrap()
    );
    tool.execute(&args, &ctx("s-abs"))
        .await
        .expect("绝对路径存在 → 放行（与 media 点名路径同语义）");
    assert_eq!(bucket.lock().get("s-abs").map(|v| v.len()), Some(1));

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_file(&outside);
}

#[tokio::test]
async fn any_missing_file_rejects_whole_batch() {
    let ws = temp_ws("missing");
    std::fs::write(ws.join("good.md"), "ok").expect("seed good file");

    let bucket = fresh_bucket();
    let tool = tool_for(&ws, bucket.clone());
    let err = tool
        .execute(
            r#"{"files": [{"path": "good.md", "title": "G"}, {"path": "ghost.md"}]}"#,
            &ctx("s-miss"),
        )
        .await
        .expect_err("任一缺失必须整批拒绝");
    assert!(
        err.contains("ghost.md") && err.contains("do not exist"),
        "缺失清单诚实回灌: {err}"
    );
    assert!(
        bucket.lock().get("s-miss").is_none(),
        "all-or-nothing：半登记不发生"
    );

    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn re_register_same_path_dedupes_in_bucket() {
    let ws = temp_ws("dedupe");
    std::fs::write(ws.join("a.md"), "a").expect("seed");

    let bucket = fresh_bucket();
    let tool = tool_for(&ws, bucket.clone());
    let sk = "agent:main:session:dup";
    tool.execute(
        r#"{"files": [{"path": "a.md", "title": "first"}]}"#,
        &ctx(sk),
    )
    .await
    .expect("第一次登记 Ok");
    tool.execute(
        r#"{"files": [{"path": "a.md", "title": "second", "note": "v2"}]}"#,
        &ctx(sk),
    )
    .await
    .expect("第二次登记 Ok");

    // 工具侧收口去重 + drain 侧全局 dedup：最终投影唯一，title/note 取最后。
    let drained =
        crate::chat_log::dedup_presented_files(bucket.lock().get(sk).cloned().unwrap_or_default());
    assert_eq!(drained.len(), 1, "同 path 去重为一条: {drained:?}");
    assert_eq!(drained[0].title, "second");
    assert_eq!(drained[0].note, "v2");

    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn different_sessions_key_into_separate_buckets() {
    let ws = temp_ws("sessions");
    std::fs::write(ws.join("x.md"), "x").expect("seed");

    let bucket = fresh_bucket();
    let tool = tool_for(&ws, bucket.clone());
    tool.execute(r#"{"files": [{"path": "x.md"}]}"#, &ctx("sess:a"))
        .await
        .expect("sess:a 登记 Ok");
    tool.execute(r#"{"files": [{"path": "x.md"}]}"#, &ctx("sess:b"))
        .await
        .expect("sess:b 登记 Ok");

    let b = bucket.lock();
    assert_eq!(b.get("sess:a").map(|v| v.len()), Some(1));
    assert_eq!(b.get("sess:b").map(|v| v.len()), Some(1));

    let _ = std::fs::remove_dir_all(&ws);
}

// ---------------------------------------------------------------------------
// 校验
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validation_errors() {
    let ws = temp_ws("validate");
    let bucket = fresh_bucket();
    let tool = tool_for(&ws, bucket.clone());

    let err = tool
        .execute("not json", &ctx("s-v"))
        .await
        .expect_err("非 JSON 必须校验失败");
    assert!(err.contains("Invalid JSON"), "got: {err}");

    let err = tool
        .execute(r#"{"title": "no files"}"#, &ctx("s-v"))
        .await
        .expect_err("缺 files 必须校验失败");
    assert!(err.contains("files"), "got: {err}");

    let err = tool
        .execute(r#"{"files": []}"#, &ctx("s-v"))
        .await
        .expect_err("空 files 必须校验失败");
    assert!(err.contains("at least one"), "got: {err}");

    let err = tool
        .execute(r#"{"files": [{"title": "no path"}]}"#, &ctx("s-v"))
        .await
        .expect_err("缺 path 必须校验失败");
    assert!(err.contains("path"), "got: {err}");

    let err = tool
        .execute(r#"{"files": [{"path": "   "}]}"#, &ctx("s-v"))
        .await
        .expect_err("空 path 必须校验失败");
    assert!(err.contains("path"), "got: {err}");

    assert!(bucket.lock().get("s-v").is_none(), "校验失败路径不得入桶");
    let _ = std::fs::remove_dir_all(&ws);
}

/// PresentedFile 与工具回灌形态一致性（smoke：解析产物经 serde 无损）。
#[test]
fn presented_file_serde_shape_matches_wire() {
    let f = PresentedFile {
        path: "p.bin".to_string(),
        title: String::new(),
        note: String::new(),
    };
    let v = serde_json::to_value(&f).expect("serialize");
    assert_eq!(v["path"], "p.bin");
    assert_eq!(v["title"], "");
    assert_eq!(v["note"], "");
}
