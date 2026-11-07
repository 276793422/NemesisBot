//! （devtool-upgrade）：会话级 diff 查看器（session_file_diff）
//! 编排测试。
//!
//! 双形态（git 影子库 / JSON 快照）基线对比 + 诚实报错（checkpoint 未挂
//! 载 / 未知路径）。夹具同 stage_two_turns 时序：begin → snapshot 捕
//! pre-edit → 工具改文件 → 推进下 turn。

use super::*;
use crate::chat_log::delete_chat_log;
use crate::r#loop::{FileChange, FileChangeKind};

struct M3NoopProvider;

#[async_trait]
impl LlmProvider for M3NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn m3_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn m3_uniq_key(tag: &str) -> String {
    format!(
        "test:m3:{}:{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn m3_modify(rel: &str) -> FileChange {
    FileChange {
        path: rel.to_string(),
        kind: FileChangeKind::Modify,
    }
}

/// 摆两轮文件演进：v1 →（turn 1）→ v2 →（turn 2）→ v3。begin/snapshot
/// 同真实施时序。返回 (loop, key, dir)。
async fn stage_two_turns(tag: &str, git_init: bool) -> (AgentLoop, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    if git_init {
        git2::Repository::init(&root).unwrap();
    }
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let al = AgentLoop::new(Box::new(M3NoopProvider), m3_config());
    al.set_checkpoint_store(Arc::new(crate::checkpoint::CheckpointStore::new(
        None, root,
    )));
    let key = m3_uniq_key(tag);
    delete_chat_log(&key);

    let cp = al.attached_checkpoint().unwrap();

    cp.begin(1, "turn 1");
    cp.snapshot(&m3_modify("code.txt")).await;
    std::fs::write(dir.path().join("ws").join("code.txt"), "v2").unwrap();

    cp.begin(2, "turn 2");
    cp.snapshot(&m3_modify("code.txt")).await;
    std::fs::write(dir.path().join("ws").join("code.txt"), "v3").unwrap();

    (al, key, dir)
}

/// git 形态：基线 = 首个声明条目（turn 1 begin tree = v1），diff 呈
/// -v1/+v3，backend=git，note 空。
#[tokio::test]
async fn m3_diff_git_mode_against_first_declared_tree() {
    let (al, key, dir) = stage_two_turns("git", true).await;

    let out = al
        .session_file_diff(&key, "code.txt")
        .await
        .expect("已声明文件有基线");
    assert_eq!(out["backend"], "git", "{out}");
    assert_eq!(out["base_turn"], 1);
    assert_eq!(out["head_on_disk"], true);
    assert_eq!(out["note"], "", "有差异无注记: {out}");
    let diff = out["diff"].as_str().unwrap();
    assert!(diff.contains("-v1"), "{diff}");
    assert!(diff.contains("+v3"), "{diff}");
    assert!(diff.contains("--- a/code.txt"), "unified 头带路径: {diff}");

    delete_chat_log(&key);
    let _ = dir;
}

/// JSON 形态：基线 = 快照 content（v1），backend=json。
#[tokio::test]
async fn m3_diff_json_mode_against_snapshot_content() {
    let (al, key, _dir) = stage_two_turns("json", false).await;

    let out = al.session_file_diff(&key, "code.txt").await.unwrap();
    assert_eq!(out["backend"], "json", "{out}");
    assert_eq!(out["base_turn"], 1);
    let diff = out["diff"].as_str().unwrap();
    assert!(diff.contains("-v1") && diff.contains("+v3"), "{diff}");

    delete_chat_log(&key);
}

/// 文件已被删：head=空串呈全删 + head_on_disk=false 诚实标注。
#[tokio::test]
async fn m3_diff_deleted_file_reports_head_missing() {
    let (al, key, dir) = stage_two_turns("gone", true).await;
    std::fs::remove_file(dir.path().join("ws").join("code.txt")).unwrap();

    let out = al.session_file_diff(&key, "code.txt").await.unwrap();
    assert_eq!(out["head_on_disk"], false, "{out}");
    assert!(
        out["note"].as_str().unwrap().contains("已不在磁盘"),
        "{out}"
    );

    delete_chat_log(&key);
}

/// checkpoint 未挂载 / 未知路径 → 诚实 Err（不静默空 diff）。
#[tokio::test]
async fn m3_diff_errors_without_store_or_unknown_path() {
    // 未挂载 checkpoint store。
    let al = AgentLoop::new(Box::new(M3NoopProvider), m3_config());
    let err = al
        .session_file_diff(&m3_uniq_key("nostore"), "code.txt")
        .await
        .unwrap_err();
    assert!(err.contains("checkpoint 未挂载"), "{err}");

    // 挂载了但路径从未声明。
    let (al, key, _dir) = stage_two_turns("unknown", true).await;
    let err = al
        .session_file_diff(&key, "never_touched.txt")
        .await
        .unwrap_err();
    assert!(err.contains("无 checkpoint 基线"), "{err}");

    delete_chat_log(&key);
}

/// 红绿：git 形态 + **绝对路径声明**（真实工具入参常态
/// 形态——preview 存的是 args 逐字串）。修复前声明/查询串原样喂
/// `tree.get_path` 被 git2 拒（"should be relative, not a windows
/// prefix"），diff 必报错；修复后树接口走 repo_relative_path 归一。
#[tokio::test]
async fn m3_diff_git_mode_absolute_declared_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    let abs = root.join("code.txt");
    std::fs::write(&abs, "v1").unwrap();

    let al = AgentLoop::new(Box::new(M3NoopProvider), m3_config());
    al.set_checkpoint_store(Arc::new(crate::checkpoint::CheckpointStore::new(
        None,
        root.clone(),
    )));
    let key = m3_uniq_key("abs");
    delete_chat_log(&key);
    let cp = al.attached_checkpoint().unwrap();

    cp.begin(1, "turn 1");
    cp.snapshot(&FileChange {
        path: abs.to_string_lossy().to_string(),
        kind: FileChangeKind::Modify,
    })
    .await;
    std::fs::write(&abs, "v2").unwrap();

    // 绝对路径查询（与声明同形态）——修复前此处 Err。
    let abs_str = abs.to_string_lossy().to_string();
    let out = al
        .session_file_diff(&key, &abs_str)
        .await
        .expect("绝对路径声明有基线且树读取成功");
    assert_eq!(out["backend"], "git", "{out}");
    assert_eq!(out["base_turn"], 1);
    let diff = out["diff"].as_str().unwrap();
    assert!(diff.contains("-v1") && diff.contains("+v2"), "{diff}");
    assert_eq!(out["path"], abs_str, "回执回显调用方路径");

    // 跨形态：repo 相对路径查询命中同一基线（comparable 归一）。hunk 一
    // 致、header 标签随调用方形态（unified_diff 既有语义）。
    let out_rel = al
        .session_file_diff(&key, "code.txt")
        .await
        .expect("相对路径跨形态命中");
    let diff_rel = out_rel["diff"].as_str().unwrap();
    assert!(
        diff_rel.contains("-v1") && diff_rel.contains("+v2"),
        "{out_rel}"
    );
    assert!(diff_rel.contains("--- a/code.txt"), "{out_rel}");

    // `\\?\` verbose 前缀形态查询（canonicalize 产物常见形态）同命中。
    let verbose = format!(r"\\?\{}", abs_str);
    let out_verbose = al
        .session_file_diff(&key, &verbose)
        .await
        .expect("verbose 前缀跨形态命中");
    let diff_verbose = out_verbose["diff"].as_str().unwrap();
    assert!(
        diff_verbose.contains("-v1") && diff_verbose.contains("+v2"),
        "{out_verbose}"
    );

    delete_chat_log(&key);
    let _ = dir;
}
