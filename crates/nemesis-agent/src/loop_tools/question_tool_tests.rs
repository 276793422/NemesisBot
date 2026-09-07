//! F7 (2026-09-06): question 工具单元测试。
//!
//! 覆盖：注册门控（question_broker Some→注册 / None→不注册）、args 校验
//! （缺 question / 选项数量 / 非串选项）、回灌格式（单选一项 / 多选多项 /
//! 超时回灌按最佳判断继续）、槽空（Some(空槽)）诚实报错、请求透传
//! （chat_id/session_key/multi/id 自增）。

use super::{QuestionBrokerSlot, QuestionTool, SharedToolConfig, register_shared_tools};
use crate::context::RequestContext;
use crate::r#loop::Tool;
use nemesis_types::agent::{QuestionAsker, QuestionOutcome, QuestionRequest};
use std::sync::Arc;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

fn empty_slot() -> QuestionBrokerSlot {
    Arc::new(parking_lot::RwLock::new(None))
}

fn slot_with(broker: Arc<dyn QuestionAsker>) -> QuestionBrokerSlot {
    Arc::new(parking_lot::RwLock::new(Some(broker)))
}

/// 记录收到的请求并回放预设结局的 mock broker。
struct MockBroker {
    outcome: QuestionOutcome,
    seen: parking_lot::Mutex<Vec<QuestionRequest>>,
}

impl MockBroker {
    fn answered(items: &[&str]) -> Self {
        Self {
            outcome: QuestionOutcome::Answered(items.iter().map(|s| s.to_string()).collect()),
            seen: parking_lot::Mutex::new(Vec::new()),
        }
    }
    fn timeout() -> Self {
        Self {
            outcome: QuestionOutcome::Timeout,
            seen: parking_lot::Mutex::new(Vec::new()),
        }
    }
}

impl QuestionAsker for MockBroker {
    fn ask(&self, request: QuestionRequest) -> Result<QuestionOutcome, String> {
        self.seen.lock().push(request);
        Ok(self.outcome.clone())
    }
}

// ---------------------------------------------------------------------------
// 注册门控
// ---------------------------------------------------------------------------

#[test]
fn register_with_broker_inserts_question_tool() {
    let cfg = SharedToolConfig {
        question_broker: Some(slot_with(Arc::new(MockBroker::timeout()))),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(
        tools.contains_key("question"),
        "broker wired → question registered"
    );
}

#[test]
fn register_without_broker_omits_question_tool() {
    // 基线形态（None）不注册——模型看不到一个只会失败的调用。
    let cfg = SharedToolConfig::default();
    let tools = register_shared_tools(&cfg);
    assert!(!tools.contains_key("question"));
}

// ---------------------------------------------------------------------------
// schema / description
// ---------------------------------------------------------------------------

#[test]
fn schema_requires_question_and_options() {
    let tool = QuestionTool::new(empty_slot());
    let schema = tool.parameters();
    assert_eq!(schema["required"][0], "question");
    assert_eq!(schema["required"][1], "options");
    assert_eq!(schema["properties"]["options"]["items"]["type"], "string");
    let desc = tool.description();
    assert!(
        desc.contains("options"),
        "description should mention options"
    );
}

// ---------------------------------------------------------------------------
// execute：校验 + 回灌
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_happy_path_single_select() {
    let broker = Arc::new(MockBroker::answered(&["pnpm"]));
    let tool = QuestionTool::new(slot_with(broker.clone()));
    let out = tool
        .execute(
            r#"{"question":"用哪个包管理器?","options":["pnpm","npm"]}"#,
            &ctx(),
        )
        .await
        .unwrap();
    assert_eq!(out, "User selected: pnpm");

    let seen = broker.seen.lock();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].question, "用哪个包管理器?");
    assert_eq!(seen[0].options, vec!["pnpm".to_string(), "npm".to_string()]);
    assert!(!seen[0].multi, "multi defaults to false");
    assert_eq!(seen[0].chat_id, "chat1");
    assert_eq!(seen[0].session_key, "sess1");
    assert!(seen[0].timeout_secs > 0);
    assert!(
        seen[0].question_id.starts_with("q-"),
        "id must be broker-prefixed"
    );
}

#[tokio::test]
async fn execute_happy_path_multi_select_reports_count() {
    let broker = Arc::new(MockBroker::answered(&["pnpm", "npm"]));
    let tool = QuestionTool::new(slot_with(broker.clone()));
    let out = tool
        .execute(
            r#"{"question":"装哪些?","options":["pnpm","npm"],"multi":true}"#,
            &ctx(),
        )
        .await
        .unwrap();
    assert!(out.contains("2 option(s)"), "out: {}", out);
    assert!(out.contains("pnpm; npm"), "out: {}", out);
    assert!(broker.seen.lock()[0].multi);
}

#[tokio::test]
async fn execute_timeout_returns_proceed_with_best_judgment() {
    let tool = QuestionTool::new(slot_with(Arc::new(MockBroker::timeout())));
    let out = tool
        .execute(r#"{"question":"选一个","options":["a","b"]}"#, &ctx())
        .await
        .unwrap();
    // 超时不是错误：回灌继续指令（轮次照常推进）。
    assert!(out.contains("did not answer"), "out: {}", out);
    assert!(out.contains("best judgment"), "out: {}", out);
}

#[tokio::test]
async fn execute_validation_rejects_bad_args() {
    let tool = QuestionTool::new(slot_with(Arc::new(MockBroker::timeout())));

    // 缺 question。
    let err = tool
        .execute(r#"{"options":["a","b"]}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("question"), "err: {}", err);

    // 空白 question。
    let err = tool
        .execute(r#"{"question":"   ","options":["a","b"]}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("question"), "err: {}", err);

    // 缺 options。
    let err = tool
        .execute(r#"{"question":"q"}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("options"), "err: {}", err);

    // 选项不是纯字符串数组。
    let err = tool
        .execute(r#"{"question":"q","options":["a",3]}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("array of strings"), "err: {}", err);

    // 选项不足 2 / 超过 6。
    let err = tool
        .execute(r#"{"question":"q","options":["a"]}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("2-6"), "err: {}", err);
    let err = tool
        .execute(
            r#"{"question":"q","options":["1","2","3","4","5","6","7"]}"#,
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("2-6"), "err: {}", err);

    // 空串选项。
    let err = tool
        .execute(r#"{"question":"q","options":["a","  "]}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("non-empty"), "err: {}", err);

    // 非法 JSON。
    let err = tool.execute("not-json", &ctx()).await.unwrap_err();
    assert!(err.contains("Invalid JSON"), "err: {}", err);
}

#[tokio::test]
async fn execute_with_empty_slot_is_honest_error() {
    // 槽已装配但 broker 未填（装配竞态窗口）→ 诚实报错不 panic。
    let tool = QuestionTool::new(empty_slot());
    let err = tool
        .execute(r#"{"question":"q","options":["a","b"]}"#, &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("no interactive session"), "err: {}", err);
}

#[test]
fn question_ids_increase_monotonically() {
    let a = super::next_question_id();
    let b = super::next_question_id();
    assert_ne!(a, b, "concurrent asks must never share an id");
}
