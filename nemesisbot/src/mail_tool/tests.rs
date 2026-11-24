//! `mail` 工具单测（纯函数层：schema 漂移 / parse / tier / 分档投影 +
//! 工具面热同步注册表语义）。插件调用链路（send 真发 / list / read 实调）
//! 归 UAT U12 矩阵——DLL 缺席的测试进程无法覆盖。

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
    // BUG-20261124-015：offset 越域（> u32::MAX）→ loud 报错而非裸 `as u32`
    // 截断（2^32 截成 0、2^32+9 截成 9 = 分页偏移静默错页）。
    let err = parse_mail_args(r#"{"action":"list","offset":4294967296}"#).unwrap_err();
    assert!(err.contains("offset"), "{err}");
    assert!(parse_mail_args(r#"{"action":"list","offset":9000000000}"#).is_err());
    // u32::MAX 边界内合法（透传）。
    assert!(parse_mail_args(r#"{"action":"list","offset":4294967295}"#).is_ok());
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

// ---------------------------------------------------------------------------
// 工具面热同步（2026-11-20 第二批）
// ---------------------------------------------------------------------------

/// 测试用最小 loop（adapters/tests 同款 MockLlmProvider 形态——本模块
/// 只碰工具注册表，provider 永不被调）。
struct SyncMockProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for SyncMockProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        unreachable!("sync_mail_tool_into_loop tests never run the loop")
    }
}

fn make_sync_test_loop() -> nemesis_agent::r#loop::AgentLoop {
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(16);
    nemesis_agent::r#loop::AgentLoop::new_bus(
        Box::new(SyncMockProvider),
        nemesis_agent::types::AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("test".to_string()),
            system_prompt_split: None,
            max_turns: 1,
            tools: vec![],
            ..Default::default()
        },
        outbound_tx,
        nemesis_agent::r#loop::ConcurrentMode::Reject,
        16,
        0,
    )
}

#[test]
fn sync_registers_when_absent_and_is_idempotent() {
    let al = make_sync_test_loop();
    assert!(!al.tool_names().iter().any(|n| n == TOOL_NAME));
    // 首次：注册成功。
    assert!(sync_mail_tool_into_loop(&al));
    assert!(al.tool_names().iter().any(|n| n == TOOL_NAME));
    // 重触发（幂等）：已注册 = false，不产生 `mail_2` 撞名僵尸。
    assert!(!sync_mail_tool_into_loop(&al));
    let mail_names: Vec<_> = al
        .tool_names()
        .into_iter()
        .filter(|n| n.starts_with(TOOL_NAME))
        .collect();
    assert_eq!(mail_names, vec![TOOL_NAME], "{mail_names:?}");
}

// 槽机制（set_tool_sync_hook / notify_tool_sync）**刻意无专测**：OnceLock
// 首个胜出 + 进程级全局——同 bin 测试进程里 cov_tests / gateway tests 的
// init_post_agent 全装配会合法抢注生产 hook，任何专测槽的用例都与测试
// 顺序竞态（2026-11-20 实录：单跑绿、全量红，notify 转交到生产闭包计数
// 0）。槽本体 5 行 wasm `set_tool_sync_hook` 先例复制品；生产注入行由
// cov_tests 的 init_post_agent 全装配覆盖；真实行为保证 =
// `sync_registers_when_absent_and_is_idempotent`。
