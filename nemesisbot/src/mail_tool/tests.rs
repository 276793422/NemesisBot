//! `mail` 工具单测（纯函数层：schema 漂移 / parse / tier / 分档投影）。
//! 插件调用链路（send 真发 / list / read 实调）归 UAT U12 矩阵——
//! DLL 缺席的测试进程无法覆盖。

use super::*;
use nemesis_agent::r#loop::Tool;

// ---------------------------------------------------------------------------
// schema 漂移测试（B4 门禁行：钉死无 delete 动作）
// ---------------------------------------------------------------------------

#[test]
fn schema_action_enum_is_exactly_send_list_read() {
    let schema = MailTool::new().parameters();
    let action = &schema["properties"]["action"];
    let enum_vals: Vec<&str> = action["enum"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(enum_vals, vec!["send", "list", "read"], "{action}");
    // required 只留 action（拍平平铺纪律——分支专属 required 由 parse 守）。
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(required, vec!["action"], "{schema}");
}

#[test]
fn schema_property_keys_exact_set() {
    // 键集精确钉死（**不可删红线**：delete 在 schema 层面不存在，非隐藏
    // ——新增动作/字段必须显式过本测试，防静默扩面）。
    let schema = MailTool::new().parameters();
    let props = schema["properties"].as_object().unwrap();
    assert!(
        !props.contains_key("delete"),
        "delete must not exist in mail tool schema (capability matrix red line): {props:?}"
    );
    let mut keys: Vec<&str> = props.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "action",
            "body",
            "id",
            "limit",
            "mailbox",
            "offset",
            "subject",
            "to",
            "unread_only"
        ],
        "{keys:?}"
    );
}

// ---------------------------------------------------------------------------
// parse
// ---------------------------------------------------------------------------

#[test]
fn parse_send_full_and_defaults() {
    let cmd =
        parse_mail_args(r#"{"action":"send","to":"zoo@example.com","subject":"hi","body":"yo"}"#)
            .unwrap();
    match cmd {
        MailCommand::Send { to, subject, body } => {
            assert_eq!(to, "zoo@example.com");
            assert_eq!(subject, "hi");
            assert_eq!(body, "yo");
        }
        other => panic!("expected Send, got {other:?}"),
    }
    // subject/body 缺省空串（合法——邮件可无主题）。
    let cmd = parse_mail_args(r#"{"action":"send","to":"a@b.c"}"#).unwrap();
    match cmd {
        MailCommand::Send { to, subject, body } => {
            assert_eq!(to, "a@b.c");
            assert_eq!(subject, "");
            assert_eq!(body, "");
        }
        other => panic!("expected Send, got {other:?}"),
    }
    // 缺 to → 诚实报错。
    let err = parse_mail_args(r#"{"action":"send","subject":"x"}"#).unwrap_err();
    assert!(err.contains("to"), "{err}");
}

#[test]
fn parse_list_defaults_and_bounds() {
    let cmd = parse_mail_args(r#"{"action":"list"}"#).unwrap();
    match cmd {
        MailCommand::List {
            mailbox,
            unread_only,
            limit,
            offset,
        } => {
            assert_eq!(mailbox, "");
            assert!(!unread_only);
            assert_eq!(limit, 20);
            assert_eq!(offset, 0);
        }
        other => panic!("expected List, got {other:?}"),
    }
    let cmd = parse_mail_args(
        r#"{"action":"list","mailbox":"zoo@x.com","unread_only":true,"limit":50,"offset":10}"#,
    )
    .unwrap();
    match cmd {
        MailCommand::List {
            mailbox,
            unread_only,
            limit,
            offset,
        } => {
            assert_eq!(mailbox, "zoo@x.com");
            assert!(unread_only);
            assert_eq!(limit, 50);
            assert_eq!(offset, 10);
        }
        other => panic!("expected List, got {other:?}"),
    }
    // limit 越界（0 / >100）→ 诚实报错。
    assert!(parse_mail_args(r#"{"action":"list","limit":0}"#).is_err());
    assert!(parse_mail_args(r#"{"action":"list","limit":101}"#).is_err());
}

#[test]
fn parse_read_requires_id() {
    let cmd = parse_mail_args(r#"{"action":"read","id":"m-123"}"#).unwrap();
    match cmd {
        MailCommand::Read { id } => assert_eq!(id, "m-123"),
        other => panic!("expected Read, got {other:?}"),
    }
    let err = parse_mail_args(r#"{"action":"read"}"#).unwrap_err();
    assert!(err.contains("id"), "{err}");
}

#[test]
fn parse_rejects_unknown_action_and_bad_json() {
    // **delete 是未知 action**——即使模型幻觉构造也被 parse 诚实拒绝
    // （schema 漂移测试之外的运行时第二道闸）。
    let err = parse_mail_args(r#"{"action":"delete","id":"m-1"}"#).unwrap_err();
    assert!(err.contains("unknown action"), "{err}");
    assert!(err.contains("delete"), "{err}");
    assert!(parse_mail_args(r#"{"action":"wipe"}"#).is_err());
    assert!(parse_mail_args("not json").is_err());
    // 缺 action。
    assert!(parse_mail_args(r#"{"to":"a@b.c"}"#).is_err());
}

// ---------------------------------------------------------------------------
// tier 与分档投影
// ---------------------------------------------------------------------------

#[test]
fn min_tier_is_normal() {
    // 计划 B4：「tier 默认 normal+，防 mini 误用外传面」——双闸秩比较。
    assert_eq!(MailTool::new().min_tier(), Some("normal"));
}

#[test]
fn operation_override_per_action() {
    let t = MailTool::new();
    let ov = t
        .operation_override(r#"{"action":"send","to":"a@b.c"}"#)
        .unwrap();
    assert_eq!(ov.op_type, "network_request", "send = 外发网络面");
    assert!(ov.force_approval.is_none());

    let ov = t.operation_override(r#"{"action":"list"}"#).unwrap();
    assert_eq!(ov.op_type, "internal", "list = LOW 纯查询");

    let ov = t
        .operation_override(r#"{"action":"read","id":"m-1"}"#)
        .unwrap();
    assert_eq!(ov.op_type, "internal", "read = LOW 纯查询");

    // args 畸形 / 缺 action = None → 静态表兜底（保守档 NetworkRequest）。
    assert!(t.operation_override("not json").is_none());
    assert!(t.operation_override(r#"{"to":"a@b.c"}"#).is_none());
}
