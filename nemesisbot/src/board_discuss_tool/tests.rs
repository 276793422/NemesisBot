//! board_discuss 工具单测（args 语义解析 + 无集群形态的诚实报错；
//! 不启动集群网络——Cluster::new 纯内存形态无节点表无 RPC client）。

use super::*;
use nemesis_agent::r#loop::Tool;

#[test]
fn test_parse_args_ok_and_defaults() {
    let a = match parse_discuss_args(
        r#"{"thread_kind": "issue", "thread_id": 42, "content": "  问题：怎么复现？  ", "reply_to": 9}"#,
    )
    .expect("valid args")
    {
        DiscussAction::Post(a) => a,
        DiscussAction::Read { .. } => panic!("带 content 必须解析为 Post"),
    };
    assert_eq!(a.thread_kind, "issue");
    assert_eq!(a.thread_id, 42);
    assert_eq!(a.content, "问题：怎么复现？", "content trim");
    assert_eq!(a.reply_to, Some(9));

    // reply_to 可选；channel 线程同构。
    let b = match parse_discuss_args(
        r#"{"thread_kind": "channel", "thread_id": 3, "content": "进展：已定位"}"#,
    )
    .expect("valid args")
    {
        DiscussAction::Post(b) => b,
        DiscussAction::Read { .. } => panic!("带 content 必须解析为 Post"),
    };
    assert_eq!(b.reply_to, None);
    assert_eq!(b.thread_kind, "channel");
}

#[test]
fn test_parse_args_read_mode() {
    // D2 读取臂：content 键缺席（或 null）= 读线程。
    let r =
        parse_discuss_args(r#"{"thread_kind": "issue", "thread_id": 7}"#).expect("无 content = 读");
    match r {
        DiscussAction::Read {
            thread_kind,
            thread_id,
        } => {
            assert_eq!(thread_kind, "issue");
            assert_eq!(thread_id, 7);
        }
        DiscussAction::Post(_) => panic!("无 content 必须解析为 Read"),
    }
    assert!(matches!(
        parse_discuss_args(r#"{"thread_kind": "channel", "thread_id": 1, "content": null}"#)
            .expect("content null 同读"),
        DiscussAction::Read { .. }
    ));

    // 读形态仍受 thread_kind/thread_id 词表约束。
    assert!(
        parse_discuss_args(r#"{"thread_kind": "file", "thread_id": 1}"#)
            .unwrap_err()
            .contains("thread_kind")
    );
    assert!(
        parse_discuss_args(r#"{"thread_kind": "issue"}"#)
            .unwrap_err()
            .contains("thread_id")
    );
    // content 在场但空 = 笔误不是读意图，诚实拒绝。
    assert!(
        parse_discuss_args(r#"{"thread_kind": "issue", "thread_id": 1, "content": ""}"#)
            .unwrap_err()
            .contains("content")
    );
    assert!(
        parse_discuss_args(r#"{"thread_kind": "issue", "thread_id": 1, "content": "   "}"#)
            .unwrap_err()
            .contains("content")
    );
    // content 非字符串类型同理（不是省略，是类型错误）。
    assert!(
        parse_discuss_args(r#"{"thread_kind": "issue", "thread_id": 1, "content": 42}"#)
            .unwrap_err()
            .contains("content")
    );
}

#[test]
fn test_format_comment_list() {
    // 入参 = nb_bus 完整信封响应（EnvelopeResponse 形 `{"ok","body":{..}}`），
    // messages 在 /body/messages——与 POST 臂 /body/seq、master 侧 sync 测试
    // body.body.messages 同口径（run4 实录：顶层形状是镜像 bug 的假形状）。
    let envelope =
        |inner: serde_json::Value| serde_json::json!({"ok": true, "error": null, "body": inner});

    // 空线程 = 诚实声明（等待中的正常中间态，不是错误）。
    let empty = format_comment_list(
        "issue",
        7,
        &envelope(serde_json::json!({
            "thread": {"kind": "issue", "id": 7},
            "messages": [],
        })),
    );
    assert!(empty.contains("no messages yet"), "{empty}");

    let body = envelope(serde_json::json!({
        "thread": {"kind": "issue", "id": 7},
        "messages": [
            {"id": 1, "sender": {"type": "agent", "id": "node-c"}, "content": "变体 A 可实现性风险：依赖 X"},
            {"id": 2, "sender": {"type": "agent", "id": "node-d"}, "content": "变体 B 可测性更好"},
        ],
    }));
    let out = format_comment_list("issue", 7, &body);
    assert!(out.contains("Thread issue:7 (2 messages)"), "{out}");
    assert!(
        out.contains("[#1] node-c: 变体 A 可实现性风险：依赖 X"),
        "{out}"
    );
    assert!(out.contains("[#2] node-d: 变体 B 可测性更好"), "{out}");

    // body.messages 缺键 → 按空处理（不 panic）。
    let missing = format_comment_list("issue", 7, &envelope(serde_json::json!({})));
    assert!(missing.contains("no messages yet"));
}

#[test]
fn test_parse_args_semantic_rejections() {
    // 词表外 thread_kind → Err；"Issue" 大小写宽容归一为合法（→ issue）。
    assert!(
        parse_discuss_args(r#"{"thread_kind": "file", "thread_id": 1, "content": "x"}"#)
            .unwrap_err()
            .contains("thread_kind")
    );
    assert!(
        parse_discuss_args(r#"{"thread_id": 1, "content": "x"}"#)
            .unwrap_err()
            .contains("thread_kind")
    );
    assert!(
        parse_discuss_args(r#"{"thread_kind": "Issue", "thread_id": 1, "content": "x"}"#).is_ok()
    );

    // 缺 thread_id / thread_id 非整数。
    assert!(
        parse_discuss_args(r#"{"thread_kind": "issue", "content": "x"}"#)
            .unwrap_err()
            .contains("thread_id")
    );
    assert!(
        parse_discuss_args(r#"{"thread_kind": "issue", "thread_id": "42", "content": "x"}"#)
            .unwrap_err()
            .contains("thread_id")
    );

    // 空 / 纯空白 content（在场但空 = 拒绝；缺席 = 读模式，见读臂测试）。
    assert!(
        parse_discuss_args(r#"{"thread_kind": "issue", "thread_id": 1, "content": "   "}"#)
            .unwrap_err()
            .contains("content")
    );

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
        node_name: String::new(),
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
        node_name: String::new(),
    }));
    let tool = BoardDiscussTool::new(cluster);
    let params = tool.parameters();
    // D2 读取臂：content 从 required 退出（缺席 = 读线程）。
    assert_eq!(
        params["required"],
        serde_json::json!(["thread_kind", "thread_id"])
    );
    assert!(tool.description().contains("coordinator"));
    assert!(tool.description().contains("READ"), "描述须声明读模式");
}
