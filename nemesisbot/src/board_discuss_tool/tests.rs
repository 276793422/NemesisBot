//! board_discuss 工具单测（args 语义解析 + 无集群形态的诚实报错；
//! 不启动集群网络——Cluster::new 纯内存形态无节点表无 RPC client）。

use super::*;
use nemesis_agent::r#loop::Tool;

#[test]
fn test_parse_args_ok_and_defaults() {
    let a = parse_discuss_args(
        r#"{"thread_kind": "issue", "thread_id": 42, "content": "  问题：怎么复现？  ", "reply_to": 9}"#,
    )
    .expect("valid args");
    assert_eq!(a.thread_kind, "issue");
    assert_eq!(a.thread_id, 42);
    assert_eq!(a.content, "问题：怎么复现？", "content trim");
    assert_eq!(a.reply_to, Some(9));

    // reply_to 可选；channel 线程同构。
    let b = parse_discuss_args(
        r#"{"thread_kind": "channel", "thread_id": 3, "content": "进展：已定位"}"#,
    )
    .expect("valid args");
    assert_eq!(b.reply_to, None);
    assert_eq!(b.thread_kind, "channel");
}

#[test]
fn test_parse_args_semantic_rejections() {
    // 词表外 thread_kind → Err；"Issue" 大小写宽容归一为合法（→ issue）。
    assert!(parse_discuss_args(r#"{"thread_kind": "file", "thread_id": 1, "content": "x"}"#)
        .unwrap_err()
        .contains("thread_kind"));
    assert!(parse_discuss_args(r#"{"thread_id": 1, "content": "x"}"#)
        .unwrap_err()
        .contains("thread_kind"));
    assert!(parse_discuss_args(
        r#"{"thread_kind": "Issue", "thread_id": 1, "content": "x"}"#
    )
    .is_ok());

    // 缺 thread_id / thread_id 非整数。
    assert!(parse_discuss_args(r#"{"thread_kind": "issue", "content": "x"}"#)
        .unwrap_err()
        .contains("thread_id"));
    assert!(parse_discuss_args(
        r#"{"thread_kind": "issue", "thread_id": "42", "content": "x"}"#
    )
    .unwrap_err()
    .contains("thread_id"));

    // 空 / 纯空白 content。
    assert!(parse_discuss_args(r#"{"thread_kind": "issue", "thread_id": 1}"#)
        .unwrap_err()
        .contains("content"));
    assert!(parse_discuss_args(
        r#"{"thread_kind": "issue", "thread_id": 1, "content": "   "}"#
    )
    .unwrap_err()
    .contains("content"));

    // 非 JSON。
    assert!(parse_discuss_args("not json").unwrap_err().contains("JSON"));
}

#[tokio::test]
async fn test_execute_honest_error_without_cluster() {
    // 纯内存 Cluster：无节点表 → 找不到 coordinator，诚实报错（不 panic、
    // 不静默假成功）。
    let cluster = Arc::new(Cluster::new(nemesis_cluster::types::ClusterConfig {
        node_id: "node-b".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
    }));
    let tool = BoardDiscussTool::new(cluster);
    let ctx = nemesis_agent::context::RequestContext::new("board", "sess", "node-b", "sess");
    let err = tool
        .execute(
            r#"{"thread_kind": "issue", "thread_id": 42, "content": "hi"}"#,
            &ctx,
        )
        .await
        .expect_err("must fail honestly without coordinator");
    assert!(err.contains("no online coordinator"), "err={err}");
}

#[test]
fn test_tool_meta() {
    assert_eq!(TOOL_NAME, "board_discuss");
    let cluster = Arc::new(Cluster::new(nemesis_cluster::types::ClusterConfig {
        node_id: "node-b".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
    }));
    let tool = BoardDiscussTool::new(cluster);
    let params = tool.parameters();
    assert_eq!(
        params["required"],
        serde_json::json!(["thread_kind", "thread_id", "content"])
    );
    assert!(tool.description().contains("coordinator"));
}
