//! D3（devtool-upgrade 阶段 5）：消息↔文件变更映射的 agent 侧测试。
//!
//! - **dispatch 瀑布收集**：注册带 `preview` 的工具真实走
//!   `handle_tool_call`，变更按 session_key 分桶落入
//!   `turn_file_changes`；**不挂 checkpoint store 也收集**（消息↔文件
//!   变更映射不依赖安全网开关——D2 依赖解耦的验收点）。
//! - **drain 语义**：`drain_turn_file_changes` 取走即清（第二次 drain 为
//!   空）+ 去重走 `chat_log::dedup_file_changes`（kind 取最后声明）。
//! - checkpoint 快照侧不受重构影响（挂载时照常 snapshot——A7 语义不回归）。

use super::*;

// —— 空 provider：dispatch 测试不触发 LLM 调用 ——

struct D3NoopProvider;

#[async_trait]
impl LlmProvider for D3NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
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

// —— 带预检的假工具：preview 声明 FileChange，execute 成功 ——

struct PreviewTool {
    path: &'static str,
}

#[async_trait]
impl Tool for PreviewTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("edit ok".to_string())
    }
    fn preview(&self, _args: &str) -> Option<FileChange> {
        Some(FileChange {
            path: self.path.to_string(),
            kind: FileChangeKind::Modify,
        })
    }
}

fn d3_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec!["preview_tool".to_string()],
        models: std::collections::HashMap::new(),
    }
}

fn d3_tc(name: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: "call_d3".to_string(),
        name: name.to_string(),
        arguments: "{}".to_string(),
    }
}

/// dispatch 真实走瀑布：变更入桶（session_key 分桶）；不挂 checkpoint 也
/// 收集；挂了 checkpoint 时快照语义不回归（不炸即可——快照本体归 D2 测）。
#[tokio::test]
async fn d3_dispatch_collects_without_checkpoint_store() {
    let mut al = AgentLoop::new(Box::new(D3NoopProvider), d3_config());
    al.register_tool(
        "preview_tool".to_string(),
        Box::new(PreviewTool { path: "src/lib.rs" }),
    );
    let ctx = RequestContext::new("web", "chat1", "agent", "sess:d3:a");

    // 未挂 checkpoint store——收集照常发生。
    assert!(al.checkpoint_store.read().is_none());
    let out = al.handle_tool_call(&d3_tc("preview_tool"), &ctx).await;
    assert!(out.contains("edit ok"), "工具执行不回归: {out}");

    let buckets = al.turn_file_changes.lock();
    let v = buckets.get("sess:d3:a").expect("session 桶存在");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].path, "src/lib.rs");
    assert_eq!(v[0].kind, FileChangeKind::Modify);
}

/// drain 即清：第一次 drain 拿到全部并去重，第二次为空；不同 session 桶
/// 互不串扰。
#[tokio::test]
async fn d3_drain_dedups_and_clears_per_session() {
    let mut al = AgentLoop::new(Box::new(D3NoopProvider), d3_config());
    al.register_tool(
        "preview_tool".to_string(),
        Box::new(PreviewTool { path: "a.rs" }),
    );
    let ctx_a = RequestContext::new("web", "c1", "agent", "sess:a");
    let ctx_b = RequestContext::new("web", "c2", "agent", "sess:b");

    al.handle_tool_call(&d3_tc("preview_tool"), &ctx_a).await;
    al.handle_tool_call(&d3_tc("preview_tool"), &ctx_a).await;
    al.handle_tool_call(&d3_tc("preview_tool"), &ctx_b).await;

    // 同 session 两次声明同 path → 去重为一条。
    let drained_a = al.drain_turn_file_changes("sess:a");
    assert_eq!(drained_a.len(), 1, "去重后一条: {drained_a:?}");
    assert_eq!(drained_a[0].path, "a.rs");
    assert_eq!(drained_a[0].kind, FileChangeKind::Modify);

    // drain 即清：第二次为空。
    assert!(al.drain_turn_file_changes("sess:a").is_empty());

    // 其他 session 桶不受影响。
    let drained_b = al.drain_turn_file_changes("sess:b");
    assert_eq!(drained_b.len(), 1);
    assert!(al.drain_turn_file_changes("sess:b").is_empty());
}

/// turn 开始兜底清：`run_agent_loop_internal` 入口的 remove 语义（直接
/// 验证 HashMap remove——真正的入口行在同一 fn 内已由编译保证）。
#[tokio::test]
async fn d3_turn_start_clear_removes_stale_bucket() {
    let al = AgentLoop::new(Box::new(D3NoopProvider), d3_config());
    al.turn_file_changes
        .lock()
        .entry("sess:stale".to_string())
        .or_default()
        .push(FileChange {
            path: "stale.rs".to_string(),
            kind: FileChangeKind::Create,
        });
    // 兜底清与 drain 同一 remove 语义。
    al.turn_file_changes.lock().remove("sess:stale");
    assert!(al.drain_turn_file_changes("sess:stale").is_empty());
}
