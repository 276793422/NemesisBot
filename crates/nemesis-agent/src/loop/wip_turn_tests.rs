//! wip 侧车 loop 级咬合测试（批二 A，2026-11-22）：`run_agent_loop_internal`
//! 真实 turn 路径上的「注册 → hook 追加 → 折叠」生命周期、中断现场注入进
//! LLM 请求（请求录制捕获）、折叠后不幽灵注入、错误收尾同样折叠。
//! Journal 纯函数面归 `wip_journal/tests.rs`；此处只锁 loop 接线（接线
//! 漂移即红）。chat_log 落盘走 test_home 重定向（既有机理）。

use super::*;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// --- 请求录制 Mock（复刻 tests.rs 最小形态；测试模块私有） ---

struct RecordingMockProvider {
    responses: Mutex<Vec<Result<LlmResponse, String>>>,
    requests: Mutex<Vec<String>>,
}

impl RecordingMockProvider {
    fn new(responses: Vec<Result<LlmResponse, String>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn request_debugs(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for RecordingMockProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.requests.lock().unwrap().push(format!("{messages:?}"));
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "No more responses".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            responses.remove(0)
        }
    }
}

fn done_response(text: &str) -> Result<LlmResponse, String> {
    Ok(LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: text.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    })
}

fn tool_response() -> Result<LlmResponse, String> {
    Ok(LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "tc_wip".to_string(),
            name: "probe_tool".to_string(),
            arguments: "{}".to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    })
}

/// turn 进行中探针工具：execute 时 wip 目录里必须有在写文件（ToolStart
/// 已被 ToolEventHook 同步 append 在前——around 链先于 next()），且文件
/// 内容含本工具的 ToolStart 事件。
struct ProbeTool {
    wip_dir: PathBuf,
    seen: Arc<AtomicBool>,
    body_seen: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for ProbeTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        let entries: Vec<PathBuf> = std::fs::read_dir(&self.wip_dir)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        if let Some(f) = entries
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "jsonl"))
        {
            self.seen.store(true, Ordering::SeqCst);
            let body = std::fs::read_to_string(f).unwrap_or_default();
            if body.contains("probe_tool") {
                self.body_seen.store(true, Ordering::SeqCst);
            }
        }
        Ok("probe-ok".to_string())
    }
}

/// 残留现场种子（模拟崩溃遗留：手写一行合法 tool_start JSONL）。
fn seed_leftover_scene(root: &std::path::Path, session_key: &str, tool: &str) {
    let sanitized = nemesis_utils::sanitize::sanitize_path_segment(session_key);
    let dir = root.join("logs").join("wip");
    std::fs::create_dir_all(&dir).unwrap();
    let event = serde_json::json!({
        "t": "tool_start",
        "ts": 1_700_000_000_000u64,
        "call_id": "c0",
        "tool": tool,
        "args": "seed-args",
    });
    std::fs::write(dir.join(format!("{sanitized}.jsonl")), format!("{event}\n")).unwrap();
}

/// 组装：tempdir 工作区 + ToolEventHook（loop 自身 journal 同实例，复刻
/// factory 装配形态）+ 录制 provider。返回
/// (loop, provider, wip_dir, _tmpdir 守卫)。
struct TestRig {
    agent_loop: AgentLoop,
    provider: Arc<RecordingMockProvider>,
    wip_dir: PathBuf,
    _tmp: tempfile::TempDir,
}

fn build_rig(responses: Vec<Result<LlmResponse, String>>) -> TestRig {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let provider = Arc::new(RecordingMockProvider::new(responses));
    let config = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    };
    let agent_loop = AgentLoop::new(Box::new(ProviderArcMock(provider.clone())), config);
    agent_loop.set_workspace_root(root.clone());
    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    agent_loop.add_tool_hook(Arc::new(
        crate::tool_event_hook::ToolEventHook::new_with_sanitizer(
            tx,
            agent_loop.work_recorder(),
            agent_loop.output_preview_sanitizer(),
            agent_loop.wip_journal(),
        ),
    ));
    TestRig {
        agent_loop,
        provider,
        wip_dir: root.join("logs").join("wip"),
        _tmp: tmp,
    }
}

/// Arc 包装 provider（AgentLoop::new 收 Box；请求录制在 Arc 侧共享给断言）。
struct ProviderArcMock(Arc<RecordingMockProvider>);

#[async_trait]
impl LlmProvider for ProviderArcMock {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.0.chat(model, messages, options, tools).await
    }
}

async fn run_turn(rig: &TestRig, session_key: &str, user_message: &str) -> Result<String, String> {
    let token = tokio_util::sync::CancellationToken::new();
    rig.agent_loop
        .run_agent_loop_internal(
            session_key,
            user_message,
            "web",
            "chat1",
            false,
            &token,
            None,
            None,
            None,
            &[],
        )
        .await
}

fn wip_jsonl_files(wip_dir: &std::path::Path) -> Vec<PathBuf> {
    std::fs::read_dir(wip_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
                .collect()
        })
        .unwrap_or_default()
}

// -----------------------------------------------------------------------
// 用例 1：完整 turn 生命周期 —— 注入遗留现场 → 注册 → hook 追加 → 折叠。
// -----------------------------------------------------------------------
#[tokio::test]
async fn wip_full_turn_lifecycle_inject_register_append_fold() {
    crate::test_home::ensure_redirected();
    let sk = "web:wip-lifecycle:main";
    let mut rig = build_rig(vec![tool_response(), done_response("完成。")]);
    // 崩溃遗留现场：上一 turn 的 tool_start。
    seed_leftover_scene(rig._tmp.path(), sk, "web_fetch");

    let seen = Arc::new(AtomicBool::new(false));
    let body_seen = Arc::new(AtomicBool::new(false));
    rig.agent_loop.register_tool(
        "probe_tool".to_string(),
        Box::new(ProbeTool {
            wip_dir: rig.wip_dir.clone(),
            seen: seen.clone(),
            body_seen: body_seen.clone(),
        }),
    );

    let result = run_turn(&rig, sk, "继续之前的任务").await;
    assert!(result.is_ok(), "turn 应正常完成: {result:?}");

    // ① 注入：首轮 LLM 请求含「中断现场恢复」节与遗留事件原文。
    let reqs = rig.provider.request_debugs();
    assert!(!reqs.is_empty(), "至少一次 LLM 请求");
    assert!(
        reqs[0].contains("中断现场恢复"),
        "首轮请求应注入现场节: {}",
        &reqs[0][..reqs[0].len().min(2000)]
    );
    assert!(reqs[0].contains("web_fetch"), "遗留事件工具名在场");

    // ② turn 中追加：探针执行时 wip 文件已在写（hook ToolStart 先落）。
    assert!(seen.load(Ordering::SeqCst), "turn 进行中 wip 文件应在写");
    assert!(body_seen.load(Ordering::SeqCst), "文件应含本 turn 的事件");

    // ③ 折叠：turn 收尾后文件删除（本轮事件也随折叠消失）。
    assert!(
        wip_jsonl_files(&rig.wip_dir).is_empty(),
        "折叠后 wip 目录应为空"
    );
    // pending 一并清理：第二轮无幽灵注入。
    let _ = run_turn(&rig, sk, "再来一句").await;
    let reqs2 = rig.provider.request_debugs();
    assert!(reqs2.len() >= 2, "两轮两次请求: {}", reqs2.len());
    assert!(
        !reqs2[reqs2.len() - 1].contains("中断现场恢复"),
        "折叠后不得幽灵注入: {}",
        &reqs2[reqs2.len() - 1][..reqs2[reqs2.len() - 1].len().min(2000)]
    );
}

// -----------------------------------------------------------------------
// 用例 2：错误收尾同样折叠（现场随异常冲刷语义落 chat_log，wip 不残留）。
// -----------------------------------------------------------------------
#[tokio::test]
async fn wip_error_ending_still_folds() {
    crate::test_home::ensure_redirected();
    let sk = "web:wip-error:main";
    // 首轮正常（带工具调用，产出事件），次轮 provider 报错 → Error 收尾。
    let mut rig = build_rig(vec![tool_response(), Err("boom：非可重试错误".to_string())]);
    seed_leftover_scene(rig._tmp.path(), sk, "exec");

    let seen = Arc::new(AtomicBool::new(false));
    let body_seen = Arc::new(AtomicBool::new(false));
    rig.agent_loop.register_tool(
        "probe_tool".to_string(),
        Box::new(ProbeTool {
            wip_dir: rig.wip_dir.clone(),
            seen: seen.clone(),
            body_seen: body_seen.clone(),
        }),
    );

    let _ = run_turn(&rig, sk, "会失败的任务").await;
    // 错误收尾也走 finalize_turn_shared → 折叠：文件不残留。
    assert!(
        wip_jsonl_files(&rig.wip_dir).is_empty(),
        "错误收尾折叠后 wip 目录应为空"
    );
    assert!(rig.agent_loop.wip_journal().pending_for(sk).is_none());
    let _ = seen;
    let _ = body_seen;
}

// -----------------------------------------------------------------------
// 用例 3：多轮构建同一注入（peek 字节稳定）——带工具的两轮请求都含现场节。
// -----------------------------------------------------------------------
#[tokio::test]
async fn wip_injection_stable_across_rounds_of_same_turn() {
    crate::test_home::ensure_redirected();
    let sk = "web:wip-stable:main";
    let mut rig = build_rig(vec![tool_response(), done_response("完成。")]);
    seed_leftover_scene(rig._tmp.path(), sk, "grep");

    let seen = Arc::new(AtomicBool::new(false));
    let body_seen = Arc::new(AtomicBool::new(false));
    rig.agent_loop.register_tool(
        "probe_tool".to_string(),
        Box::new(ProbeTool {
            wip_dir: rig.wip_dir.clone(),
            seen: seen.clone(),
            body_seen,
        }),
    );

    let _ = run_turn(&rig, sk, "继续").await;
    let reqs = rig.provider.request_debugs();
    assert!(reqs.len() >= 2, "带工具 turn 至少两次请求: {}", reqs.len());
    for (i, req) in reqs.iter().enumerate() {
        assert!(
            req.contains("中断现场恢复"),
            "第 {i} 轮请求也应注入现场节（peek 全 turn 稳定）"
        );
        assert!(req.contains("grep"), "第 {i} 轮请求含遗留事件原文");
    }
}
