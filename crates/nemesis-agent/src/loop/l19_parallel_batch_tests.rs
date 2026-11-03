//! L19（改进项三期 2026-11-03）：U5 并行批接统一 WorkPool 后的行为钉子。
//!
//! - 批级预检：>128 调用 / args 字节超 8 MiB → 全批类型化拒绝（零执行
//!   零挂起——带执行计数器的工具断言计数不动）。
//! - 账本预留失败路径经共享池（OnceLock 单池），正常批行为不变
//!   （多调用全成功 + 源序——join_all 保序是串行 guard 回放的审计链前提）。
//! - 既有三臂行为（Valid/Fixed/Invalid）由 s9_tests 的
//!   `precompute_parallel_batch_validation_arms` 钉死，此处不重复。

use super::*;
use crate::test_support::capture_logs;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 进程级串行闸：本文件四个用例共享受 OnceLock 全局池的账本，cargo 并行
/// 调度下 128 上限批占住账本会让并行的正常批中段被拒（那正是跨批背压的
/// 生产语义，但钉子测试需要互斥）。有意跨 await 持有——批执行全程占着
/// 账本预留。
static L19_POOL_SER: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

// ---------- 本地 mock（各测试文件私有，与 s9_tests 同构） ----------

struct MockLlmProvider;

#[async_trait]
impl LlmProvider for MockLlmProvider {
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

/// 带执行计数器的 echo 工具：overload 拒绝路径必须零执行。
struct CountingEchoTool {
    executions: std::sync::Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for CountingEchoTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(format!("echo:{}", args))
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn l19_ctx() -> RequestContext {
    RequestContext {
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        user: "u1".to_string(),
        session_key: "agent:test/l19pb".to_string(),
        correlation_id: None,
        async_callback: None,
        tool_path_base: None,
        approval_ref: None,
    }
}

fn echo_call(id: &str, args: &str) -> crate::types::ToolCallInfo {
    crate::types::ToolCallInfo {
        id: id.to_string(),
        name: "l19echo".to_string(),
        arguments: args.to_string(),
    }
}

fn oversized_call(id: &str, bytes: usize) -> crate::types::ToolCallInfo {
    crate::types::ToolCallInfo {
        id: id.to_string(),
        name: "l19echo".to_string(),
        // {"pad":"..."} 外壳 + 填充到目标字节数。
        arguments: format!(r#"{{"pad":"{}"}}"#, "x".repeat(bytes.saturating_sub(12))),
    }
}

/// 调用数超 128 上限 → 全批拒绝，零执行（预检在 fut 构造前短路）。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // L19_POOL_SER 序列化闸有意跨 await 持有
async fn l19_batch_over_call_cap_rejects_whole_batch_without_execution() {
    let _ser = L19_POOL_SER.lock();
    let _logs = capture_logs();
    let executions = std::sync::Arc::new(AtomicUsize::new(0));
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider), test_config());
    agent_loop.register_tool(
        "l19echo".to_string(),
        Box::new(CountingEchoTool {
            executions: std::sync::Arc::clone(&executions),
        }),
    );

    let calls: Vec<_> = (0..=PARALLEL_BATCH_MAX_CALLS)
        .map(|i| echo_call(&format!("t{i}"), &format!(r#"{{"i":{i}}}"#)))
        .collect();
    let out = agent_loop
        .precompute_parallel_batch(&calls, &l19_ctx(), 0, false, None, None, Vec::new())
        .await;

    assert_eq!(out.len(), calls.len(), "每调用一条拒绝（不丢调用）");
    for (i, pc) in out.iter().enumerate() {
        assert!(
            pc.result.contains("pool overloaded (rejected, not queued)")
                || pc.result.contains("calls exceeds the batch cap"),
            "第 {i} 条应为类型化拒绝: {}",
            pc.result
        );
        assert!(pc.result.contains("Split the batch"), "{}", pc.result);
        assert!(!pc.validation_failed, "拒绝非 validation 失败");
    }
    assert_eq!(
        executions.load(Ordering::SeqCst),
        0,
        "全批拒绝 = 零执行（含上限内的调用也不跑）"
    );
}

/// args 累计字节超 8 MiB → 全批拒绝，零执行。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // L19_POOL_SER 序列化闸有意跨 await 持有
async fn l19_batch_over_input_bytes_cap_rejects_whole_batch_without_execution() {
    let _ser = L19_POOL_SER.lock();
    let _logs = capture_logs();
    let executions = std::sync::Arc::new(AtomicUsize::new(0));
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider), test_config());
    agent_loop.register_tool(
        "l19echo".to_string(),
        Box::new(CountingEchoTool {
            executions: std::sync::Arc::clone(&executions),
        }),
    );

    // 两个 ~4.5 MiB 调用：单批累计 > 8 MiB（单调用不超→证明是累计口径）。
    let half = PARALLEL_BATCH_MAX_INPUT_BYTES / 2 + 1024 * 1024;
    let calls = vec![oversized_call("b0", half), oversized_call("b1", half)];
    let out = agent_loop
        .precompute_parallel_batch(&calls, &l19_ctx(), 0, false, None, None, Vec::new())
        .await;

    assert_eq!(out.len(), 2);
    for pc in &out {
        assert!(
            pc.result.contains("combined tool arguments"),
            "字节上限拒绝文案: {}",
            pc.result
        );
        assert!(pc.result.contains("rejected"), "{}", pc.result);
    }
    assert_eq!(executions.load(Ordering::SeqCst), 0, "零执行");
}

/// 恰在上限的单批（128 调用）必须整体预留成功并全部执行——「预检=池
/// 容量连贯」钉子（旧共享池形态下小机器 max_pending < 128 会中段拒绝，
/// 专属池 pending 上限=批上限后此回归封死）。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // L19_POOL_SER 序列化闸有意跨 await 持有
async fn l19_batch_at_call_cap_reserves_and_executes_whole_batch() {
    let _ser = L19_POOL_SER.lock();
    let _logs = capture_logs();
    let executions = std::sync::Arc::new(AtomicUsize::new(0));
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider), test_config());
    agent_loop.register_tool(
        "l19echo".to_string(),
        Box::new(CountingEchoTool {
            executions: std::sync::Arc::clone(&executions),
        }),
    );

    let calls: Vec<_> = (0..PARALLEL_BATCH_MAX_CALLS)
        .map(|i| echo_call(&format!("c{i}"), &format!(r#"{{"i":{i}}}"#)))
        .collect();
    let out = agent_loop
        .precompute_parallel_batch(&calls, &l19_ctx(), 0, false, None, None, Vec::new())
        .await;

    assert_eq!(out.len(), PARALLEL_BATCH_MAX_CALLS);
    for (i, pc) in out.iter().enumerate() {
        assert!(
            pc.result.contains(&format!(r#"echo:{{"i":{i}}}"#)),
            "上限批第 {i} 条应正常执行: {}",
            pc.result
        );
    }
    assert_eq!(
        executions.load(Ordering::SeqCst),
        PARALLEL_BATCH_MAX_CALLS,
        "上限批 = 全部执行"
    );
}

/// 正常批（小调用）行为不变：全部执行成功 + 源序（join_all 保序）。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // L19_POOL_SER 序列化闸有意跨 await 持有
async fn l19_normal_batch_unchanged_source_order() {
    let _ser = L19_POOL_SER.lock();
    let _logs = capture_logs();
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider), test_config());
    agent_loop.register_tool(
        "l19echo".to_string(),
        Box::new(CountingEchoTool {
            executions: std::sync::Arc::new(AtomicUsize::new(0)),
        }),
    );

    let calls: Vec<_> = (0..8)
        .map(|i| echo_call(&format!("n{i}"), &format!(r#"{{"i":{i}}}"#)))
        .collect();
    let out = agent_loop
        .precompute_parallel_batch(&calls, &l19_ctx(), 0, false, None, None, Vec::new())
        .await;

    assert_eq!(out.len(), 8);
    for (i, pc) in out.iter().enumerate() {
        assert!(
            pc.result.contains(&format!(r#"echo:{{"i":{i}}}"#)),
            "源序第 {i} 条应为第 {i} 个调用的结果: {}",
            pc.result
        );
        assert!(!pc.validation_failed);
    }
}
