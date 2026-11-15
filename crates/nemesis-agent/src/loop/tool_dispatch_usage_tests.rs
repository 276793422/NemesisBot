// 工具使用率落账测试（usage-ranked tool index）：dispatch 包装层单点
// 记账——真实执行才入账（classify_result None 臂拒绝短路），执行过但
// 失败照常入账（使用热度 ≠ 成功率）。与 checks_capture 的拒绝分类
// 同源单点，不重实现判别器。

use super::*;

// ---------------------------------------------------------------------------
// 迷你 fixture（tool_dispatch_cov_tests 同款）
// ---------------------------------------------------------------------------

struct UsageLlmProvider;

#[async_trait]
impl LlmProvider for UsageLlmProvider {
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

fn usage_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 固定回复工具（可配 Ok/Err）。
#[derive(Debug)]
struct UsageStubTool {
    result: Result<String, String>,
}

impl UsageStubTool {
    fn ok() -> Box<Self> {
        Box::new(Self {
            result: Ok("usage stub ok".to_string()),
        })
    }
    fn failing() -> Box<Self> {
        Box::new(Self {
            result: Err("boom-usage".to_string()),
        })
    }
}

#[async_trait]
impl Tool for UsageStubTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        self.result.clone()
    }
}

fn usage_call(name: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: format!("usage-{name}"),
        name: name.to_string(),
        arguments: "{}".to_string(),
    }
}

fn usage_ctx() -> RequestContext {
    RequestContext::new(
        "web",
        "chat-usage",
        "usageuser",
        "agent:main:session:usagetd",
    )
}

fn usage_totals(ds: &nemesis_data::DataStore) -> std::collections::HashMap<String, i64> {
    ds.query_tool_usage_totals(0)
        .expect("query_tool_usage_totals must not fail")
        .into_iter()
        .map(|r| (r.tool, r.calls))
        .collect()
}

// ---------------------------------------------------------------------------
// 真实执行入账 / 拒绝不入账 / 执行失败照常入账
// ---------------------------------------------------------------------------

#[tokio::test]
async fn usage_recorded_on_real_execution() {
    let dir = tempfile::tempdir().unwrap();
    let ds = std::sync::Arc::new(
        nemesis_data::DataStore::open(&dir.path().join("usage.db")).expect("open datastore"),
    );
    let mut al = AgentLoop::new(Box::new(UsageLlmProvider), usage_config());
    al.register_tool("usage_probe".to_string(), UsageStubTool::ok());
    al.set_data_store(ds.clone());

    let out = al
        .handle_tool_call_at_depth(&usage_call("usage_probe"), &usage_ctx(), 0)
        .await;
    assert_eq!(out, "usage stub ok");

    let totals = usage_totals(&ds);
    assert_eq!(
        totals.get("usage_probe").copied(),
        Some(1),
        "一次真实执行 = 一笔账: {totals:?}"
    );

    // 第二次执行增量累计（天×工具 upsert）。
    al.handle_tool_call_at_depth(&usage_call("usage_probe"), &usage_ctx(), 0)
        .await;
    let totals = usage_totals(&ds);
    assert_eq!(totals.get("usage_probe").copied(), Some(2));
}

#[tokio::test]
async fn refusal_not_recorded_and_failure_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let ds = std::sync::Arc::new(
        nemesis_data::DataStore::open(&dir.path().join("usage2.db")).expect("open datastore"),
    );
    let mut al = AgentLoop::new(Box::new(UsageLlmProvider), usage_config());
    // 失败工具：执行过但 Err——使用热度照常入账。
    al.register_tool("usage_fail_probe".to_string(), UsageStubTool::failing());
    al.set_data_store(ds.clone());

    // 未知工具 = 框架拒绝（工具体从未执行）→ 不入账。
    let out = al
        .handle_tool_call_at_depth(&usage_call("no_such_tool"), &usage_ctx(), 0)
        .await;
    assert!(
        out.starts_with("Error: Unknown tool '"),
        "未知工具拒绝形态: {out}"
    );
    let totals = usage_totals(&ds);
    assert!(
        !totals.contains_key("no_such_tool"),
        "拒绝不入账: {totals:?}"
    );

    // 执行过但失败（`Tool error:` 前缀）→ 入账（使用 ≠ 成功）。
    let out = al
        .handle_tool_call_at_depth(&usage_call("usage_fail_probe"), &usage_ctx(), 0)
        .await;
    assert!(out.starts_with("Tool error:"), "失败执行形态: {out}");
    let totals = usage_totals(&ds);
    assert_eq!(
        totals.get("usage_fail_probe").copied(),
        Some(1),
        "执行过但失败照常入账: {totals:?}"
    );
}

#[tokio::test]
async fn no_data_store_is_silent_skip() {
    // 无账本（set_data_store 未调用）→ 记账静默跳过，执行照常。
    let mut al = AgentLoop::new(Box::new(UsageLlmProvider), usage_config());
    al.register_tool("usage_probe".to_string(), UsageStubTool::ok());
    let out = al
        .handle_tool_call_at_depth(&usage_call("usage_probe"), &usage_ctx(), 0)
        .await;
    assert_eq!(out, "usage stub ok", "无账本不反压执行");
}
