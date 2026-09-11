// G0 (devtool-upgrade 阶段 3)：SpawnTool 生产化 + run_detached 测试。
//
// 覆盖面（对应实施计划 G0 验收）：
// ① 无 spawn_fn（旧装配路径）诚实报错，行为不回归；
// ② 闭包参数透传——task 原样、agent_id 缺省 "default"、model=SpawnConfig
//    .default_model、channel/chat_id 来自 RequestContext；schema 中 agent_id
//    可选（schema/实现一致化）；
// ③ max_concurrent 信号量真排队（确定性：许可被占住时第二个不得放行；
//    生产形状：5 并发 × max 4）；
// ④ instance 级 detached_allowed_tools 经 effective_tool_defs 收窄供给，
//    普通回合（None）全量直通；
// ⑤ run_detached 三态提取——Done 优先 / Error 映射 Err。
//
// 自带迷你 Mock（本文件独立，不与 tests.rs 共享私有类型）。

use super::*;

/// 恒定回复的迷你 provider（同 tests.rs MockLlmProvider 形状）。
struct DetachedMockProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    fail: bool,
}

impl DetachedMockProvider {
    fn ok(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
            fail: false,
        }
    }
    fn failing() -> Self {
        Self {
            responses: std::sync::Mutex::new(Vec::new()),
            fail: true,
        }
    }
}

#[async_trait]
impl LlmProvider for DetachedMockProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        if self.fail {
            return Err("provider exploded".to_string());
        }
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "no more responses".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

fn spawn_config(max_concurrent: usize) -> crate::loop_tools::SpawnConfig {
    crate::loop_tools::SpawnConfig {
        default_model: "test-model".to_string(),
        max_concurrent,
        // G2：默认 1（顶层可 spawn，子代理不可再 spawn）——绝大多数用例
        // 都是顶层深度 0 的调用，1 = 放行不干扰。
        max_depth: 1,
    }
}

/// 迷你测试工具（与 tests.rs 的 MockTool 同形；兄弟模块私有不可共享）。
struct EchoTool {
    result: String,
}

#[async_trait]
impl Tool for EchoTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(self.result.clone())
    }
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["echo_alpha".to_string()],
        models: std::collections::HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// ① 无 spawn_fn（旧装配路径）→ 诚实报错，不回归
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_without_slot_returns_honest_error() {
    let tool = crate::loop_tools::SpawnTool::new(spawn_config(4));
    let ctx = RequestContext::new("web", "chat1", "user1", "session1");
    let err = tool
        .execute(r#"{"task":"do thing"}"#, &ctx)
        .await
        .expect_err("无 spawn_fn 必须报错而非静默成功");
    assert!(
        err.contains("not available"),
        "报错须说明子代理不可用，实际: {err}"
    );
}

// ---------------------------------------------------------------------------
// ② 闭包参数透传 + schema agent_id 可选
// ---------------------------------------------------------------------------

type CapturedCall = (
    String, // agent_id
    String, // task
    String, // model
    String, // channel
    String, // chat_id
);

#[tokio::test]
async fn spawn_passes_args_and_defaults_to_closure() {
    let mut tool = crate::loop_tools::SpawnTool::new(spawn_config(4));
    let captured: std::sync::Arc<std::sync::Mutex<Option<CapturedCall>>> = Default::default();
    let cap = captured.clone();
    tool.set_spawn_fn(Arc::new(
        move |agent_id: &str,
              task: &str,
              model: &str,
              channel: &str,
              chat_id: &str,
              _t: &str,
              _d: usize,
              _bg: bool| {
            let cap = cap.clone();
            // &str 先转 owned（Future 'static）。
            let (agent_id, task, model, channel, chat_id) = (
                agent_id.to_string(),
                task.to_string(),
                model.to_string(),
                channel.to_string(),
                chat_id.to_string(),
            );
            Box::pin(async move {
                *cap.lock().unwrap() = Some((agent_id, task, model, channel, chat_id));
                Ok("sub done".to_string())
            })
        },
    ));

    // schema：agent_id 必须可选（不在 required），task 必须必填。
    let schema = tool.parameters();
    let required = schema["required"].as_array().cloned().unwrap_or_default();
    assert!(
        !required.iter().any(|v| v == "agent_id"),
        "agent_id 必须可选（schema/实现一致化），required={required:?}"
    );
    assert!(
        required.iter().any(|v| v == "task"),
        "task 必须必填，required={required:?}"
    );

    let ctx = RequestContext::new("web", "chat-9", "user1", "session1");
    let out = tool
        .execute(r#"{"task":"summarize file"}"#, &ctx)
        .await
        .expect("注入闭包后 spawn 必须成功");
    assert_eq!(out, "sub done");

    let got = captured.lock().unwrap().clone().expect("闭包必须被调用");
    assert_eq!(got.0, "default", "缺省 agent_id 应传 \"default\"");
    assert_eq!(got.1, "summarize file", "task 原样透传");
    assert_eq!(got.2, "test-model", "model = SpawnConfig.default_model");
    assert_eq!(got.3, "web", "channel 来自 RequestContext");
    assert_eq!(got.4, "chat-9", "chat_id 来自 RequestContext");
}

#[tokio::test]
async fn spawn_allows_custom_agent_id_passthrough() {
    let mut tool = crate::loop_tools::SpawnTool::new(spawn_config(4));
    let captured: std::sync::Arc<std::sync::Mutex<Option<CapturedCall>>> = Default::default();
    let cap = captured.clone();
    tool.set_spawn_fn(Arc::new(
        move |agent_id: &str,
              task: &str,
              _m: &str,
              _c: &str,
              _ch: &str,
              _t: &str,
              _d: usize,
              _bg: bool| {
            let cap = cap.clone();
            let agent_id = agent_id.to_string();
            let task = task.to_string();
            Box::pin(async move {
                *cap.lock().unwrap() =
                    Some((agent_id, task, String::new(), String::new(), String::new()));
                Ok("ok".to_string())
            })
        },
    ));
    let ctx = RequestContext::new("web", "c", "u", "s");
    tool.execute(r#"{"task":"t","agent_id":"reviewer"}"#, &ctx)
        .await
        .expect("显式 agent_id 应原样透传");
    assert_eq!(captured.lock().unwrap().clone().unwrap().0, "reviewer");
}

// ---------------------------------------------------------------------------
// ③ 信号量真排队
// ---------------------------------------------------------------------------

/// 确定性排队证明：max_concurrent=1，第一个 spawn 的闭包阻塞占住许可；
/// 第二个必须等待（不得并发放行）；释放后依次完成。
#[tokio::test]
async fn spawn_semaphore_queues_beyond_max_concurrent() {
    let mut tool = crate::loop_tools::SpawnTool::new(spawn_config(1));
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());

    let started_c = started.clone();
    let release_c = release.clone();
    tool.set_spawn_fn(Arc::new(
        move |_a: &str,
              task: &str,
              _m: &str,
              _c: &str,
              _ch: &str,
              _t: &str,
              _d: usize,
              _bg: bool| {
            let started = started_c.clone();
            let release = release_c.clone();
            let task = task.to_string();
            Box::pin(async move {
                started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if task == "first" {
                    // 占住唯一许可，直到主测试线程放行。
                    release.notified().await;
                }
                Ok(format!("done:{task}"))
            })
        },
    ));
    let tool = Arc::new(tool);

    let t1 = {
        let tool = tool.clone();
        tokio::spawn(async move {
            let ctx = RequestContext::new("web", "c", "u", "s");
            tool.execute(r#"{"task":"first"}"#, &ctx).await
        })
    };
    // 等 first 实际拿到许可。
    for _ in 0..400 {
        if started.load(std::sync::atomic::Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        started.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "first 必须已进入闭包"
    );

    let t2 = {
        let tool = tool.clone();
        tokio::spawn(async move {
            let ctx = RequestContext::new("web", "c", "u", "s");
            tool.execute(r#"{"task":"second"}"#, &ctx).await
        })
    };
    // 给 second 充分的「本不该运行」窗口。
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "许可被占住时第二个 spawn 必须排队，不得并发放行"
    );

    release.notify_one();
    let r1 = t1.await.unwrap().expect("first 应成功");
    let r2 = t2.await.unwrap().expect("second 排队后应成功");
    assert_eq!(r1, "done:first");
    assert_eq!(r2, "done:second");
    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "释放后第二个必须真正执行"
    );
}

/// 生产形状（验收原文）：并发 5 个 spawn、max_concurrent=4 → 第 5 个排队。
/// 硬不变量：任意时刻在飞闭包数 ≤ 4；5 个全部完成。
#[tokio::test]
async fn spawn_five_concurrent_with_max_four_never_exceeds_four() {
    let mut tool = crate::loop_tools::SpawnTool::new(spawn_config(4));
    let in_flight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (in_f, peak_c) = (in_flight.clone(), peak.clone());
    tool.set_spawn_fn(Arc::new(
        move |_a: &str,
              task: &str,
              _m: &str,
              _c: &str,
              _ch: &str,
              _t: &str,
              _d: usize,
              _bg: bool| {
            let in_flight = in_f.clone();
            let peak = peak_c.clone();
            let task = task.to_string();
            Box::pin(async move {
                let now = in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                peak.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                in_flight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                Ok(format!("done:{task}"))
            })
        },
    ));
    let tool = Arc::new(tool);

    let mut handles = Vec::new();
    for i in 0..5 {
        let tool = tool.clone();
        handles.push(tokio::spawn(async move {
            let ctx = RequestContext::new("web", "c", "u", "s");
            tool.execute(&format!(r#"{{"task":"t{i}"}}"#), &ctx).await
        }));
    }
    let mut completed = 0;
    for h in handles {
        h.await.unwrap().expect("5 个并发 spawn 全部应成功");
        completed += 1;
    }
    assert_eq!(completed, 5);
    assert!(
        peak.load(std::sync::atomic::Ordering::SeqCst) <= 4,
        "并发峰值不得超过 max_concurrent=4，实际 {}",
        peak.load(std::sync::atomic::Ordering::SeqCst)
    );
}

// ---------------------------------------------------------------------------
// ④ instance 级白名单经 effective_tool_defs 收窄
// ---------------------------------------------------------------------------

fn def_names(defs: &[crate::types::ToolDefinition]) -> Vec<String> {
    defs.iter().map(|d| d.function.name.clone()).collect()
}

#[tokio::test]
async fn effective_tool_defs_filters_detached_allowlist() {
    let mut agent_loop = AgentLoop::new(Box::new(DetachedMockProvider::ok(vec![])), test_config());
    agent_loop.register_tool(
        "echo_alpha".to_string(),
        Box::new(EchoTool {
            result: "a".to_string(),
        }),
    );
    agent_loop.register_tool(
        "echo_beta".to_string(),
        Box::new(EchoTool {
            result: "b".to_string(),
        }),
    );

    // 普通回合（None 白名单）：standalone loop tier=Big → 空白名单直通全量。
    let plain = AgentInstance::new(test_config());
    let names = def_names(&agent_loop.effective_tool_defs(&plain));
    assert!(
        names.contains(&"echo_alpha".to_string()) && names.contains(&"echo_beta".to_string()),
        "普通回合应全量直通，实际 {names:?}"
    );

    // detached 白名单收窄：只剩 echo_alpha。
    let detached = AgentInstance::new(test_config());
    detached.set_detached_allowed_tools(Some(vec!["echo_alpha".to_string()]));
    let names = def_names(&agent_loop.effective_tool_defs(&detached));
    assert_eq!(
        names,
        vec!["echo_alpha".to_string()],
        "白名单必须精确收窄供给"
    );

    // 空白名单 = 不设限（run_detached 的 Some(空) 视为 None 语义）。
    let empty = AgentInstance::new(test_config());
    empty.set_detached_allowed_tools(Some(vec![]));
    let names = def_names(&agent_loop.effective_tool_defs(&empty));
    assert!(
        names.contains(&"echo_beta".to_string()),
        "空白名单不得收窄，实际 {names:?}"
    );
}

// ---------------------------------------------------------------------------
// ⑤ run_detached 三态提取
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_detached_returns_done_text() {
    let provider = DetachedMockProvider::ok(vec![LlmResponse {
        content: "sub-agent final answer".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let out = agent_loop
        .run_detached("summarize the file", DetachedOpts::default())
        .await
        .expect("Done 事件应提取为 Ok");
    assert_eq!(out, "sub-agent final answer");
}

#[tokio::test]
async fn run_detached_maps_error_event_to_err() {
    let agent_loop = AgentLoop::new(Box::new(DetachedMockProvider::failing()), test_config());
    let err = agent_loop
        .run_detached("task", DetachedOpts::default())
        .await
        .expect_err("Error 事件应映射为 Err");
    assert!(
        err.contains("provider exploded"),
        "错误原因必须透传，实际: {err}"
    );
}

#[tokio::test]
async fn run_detached_allowlist_flows_into_instance() {
    // 端到端：opts.allowed_tools 经 run_detached 落到派生 instance，
    // LLM 只看到白名单内的工具 defs（provider 捕获每次请求收到的 defs）。
    struct DefsCaptureProvider {
        seen: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }
    #[async_trait]
    impl LlmProvider for DefsCaptureProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            self.seen
                .lock()
                .unwrap()
                .push(tools.iter().map(|d| d.function.name.clone()).collect());
            Ok(LlmResponse {
                content: "done".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    let seen: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>> = Default::default();
    let provider = DefsCaptureProvider { seen: seen.clone() };

    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    agent_loop.register_tool(
        "echo_alpha".to_string(),
        Box::new(EchoTool {
            result: "a".to_string(),
        }),
    );
    agent_loop.register_tool(
        "echo_beta".to_string(),
        Box::new(EchoTool {
            result: "b".to_string(),
        }),
    );

    agent_loop
        .run_detached(
            "task",
            DetachedOpts {
                allowed_tools: Some(&["echo_alpha"]),
                ..DetachedOpts::default()
            },
        )
        .await
        .expect("detached 轮次应成功");

    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty(), "provider 应至少收到一次请求");
    for defs in seen.iter() {
        assert!(
            defs.contains(&"echo_alpha".to_string()),
            "白名单内工具必须供给: {defs:?}"
        );
        assert!(
            !defs.contains(&"echo_beta".to_string()),
            "白名单外工具不得供给: {defs:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// G1：tools 档位（readonly 缺省 / full / 未知拒绝）+ readonly 白名单供给
// ---------------------------------------------------------------------------

#[test]
fn detached_tools_profile_mapping() {
    use crate::loop_tools::detached_tools_for_profile;

    // readonly → 白名单：读集 7 项齐、不含任何写/执行工具。
    let ro = detached_tools_for_profile("readonly")
        .expect("readonly 合法档位")
        .expect("readonly 必须映射为白名单");
    assert_eq!(ro.len(), 7, "白名单清单变动须同步本断言");
    for t in [
        "read_file",
        "list_dir",
        "grep",
        "git",
        "web_fetch",
        "lsp",
        "cli_reference",
    ] {
        assert!(ro.contains(&t), "readonly 白名单缺 {t}: {ro:?}");
    }
    assert!(
        !ro.contains(&"write_file") && !ro.contains(&"exec") && !ro.contains(&"edit_file"),
        "readonly 不得含写/执行工具: {ro:?}"
    );

    // full → None（不设限，仍经父 tier 过滤）。
    assert!(
        detached_tools_for_profile("full").unwrap().is_none(),
        "full 必须映射为不设限"
    );

    // 未知档位 → Err（调用方诚实回灌，不静默降级）。
    let err = detached_tools_for_profile("yolo").expect_err("未知档位必须拒绝");
    assert!(
        err.contains("readonly") && err.contains("full"),
        "错误文案须列合法值: {err}"
    );
}

#[test]
fn spawn_schema_declares_tools_enum_optional() {
    let tool = crate::loop_tools::SpawnTool::new(spawn_config(4));
    let schema = tool.parameters();
    let tools = &schema["properties"]["tools"];
    assert_eq!(tools["enum"][0], "readonly", "schema enum 第一档 readonly");
    assert_eq!(tools["enum"][1], "full", "schema enum 第二档 full");
    let required = schema["required"].as_array().cloned().unwrap_or_default();
    assert!(
        !required.iter().any(|v| v == "tools"),
        "tools 必须可选（缺省 readonly），required={required:?}"
    );
}

/// 闭包第 6 参 = 档位原文：缺省 readonly、显式 readonly/full 原样透传。
#[tokio::test]
async fn spawn_tools_profile_flows_to_closure() {
    let mut tool = crate::loop_tools::SpawnTool::new(spawn_config(4));
    let seen: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let s = seen.clone();
    tool.set_spawn_fn(Arc::new(
        move |_a: &str,
              task: &str,
              _m: &str,
              _c: &str,
              _ch: &str,
              tools: &str,
              _d: usize,
              _bg: bool| {
            let s = s.clone();
            let task = task.to_string();
            let tools = tools.to_string();
            Box::pin(async move {
                s.lock().unwrap().push(format!("{task}:{tools}"));
                Ok("ok".to_string())
            })
        },
    ));
    let ctx = RequestContext::new("web", "c", "u", "s");
    tool.execute(r#"{"task":"a"}"#, &ctx)
        .await
        .expect("缺省档位应成功");
    tool.execute(r#"{"task":"b","tools":"readonly"}"#, &ctx)
        .await
        .expect("显式 readonly 应成功");
    tool.execute(r#"{"task":"c","tools":"full"}"#, &ctx)
        .await
        .expect("full 应成功");
    assert_eq!(
        seen.lock().unwrap().clone(),
        vec!["a:readonly", "b:readonly", "c:full"],
        "档位缺省与透传语义"
    );
}

#[tokio::test]
async fn spawn_tools_profile_unknown_rejected_before_spawn() {
    let mut tool = crate::loop_tools::SpawnTool::new(spawn_config(4));
    let called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = called.clone();
    tool.set_spawn_fn(Arc::new(
        move |_a: &str, _t: &str, _m: &str, _c: &str, _ch: &str, _p: &str, _d: usize, _bg: bool| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok("should not run".to_string()) })
        },
    ));
    let ctx = RequestContext::new("web", "c", "u", "s");
    let err = tool
        .execute(r#"{"task":"x","tools":"exec_everything"}"#, &ctx)
        .await
        .expect_err("未知档位必须在 spawn 前诚实拒绝");
    assert!(
        err.contains("readonly") && err.contains("full"),
        "错误文案须列合法值供模型自纠: {err}"
    );
    assert_eq!(
        called.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "拒绝路径不得触碰 spawn 闭包"
    );
}

/// 验收（G1）：readonly 档子代理——① 供给含白名单读工具、无 write_file；
/// ② 模型硬调 write_file → dispatch 回灌 "Error: Unknown tool 'write_file'"，
/// 下一轮 LLM 请求可见（模型可自纠，readonly 子代理写不出去）。
#[tokio::test]
async fn readonly_profile_hides_writer_and_feeds_unknown_tool_back() {
    // 两段式 provider：第 1 轮记 defs 并返回 write_file 工具调用；
    // 第 2 轮记全部消息并收尾。状态放 Arc（provider 会被 move 进 Box）。
    struct PhaseState {
        calls: std::sync::atomic::AtomicUsize,
        first_defs: std::sync::Mutex<Vec<String>>,
        second_messages: std::sync::Mutex<Vec<String>>,
    }
    struct TwoPhase {
        state: Arc<PhaseState>,
    }
    #[async_trait]
    impl LlmProvider for TwoPhase {
        async fn chat(
            &self,
            _model: &str,
            messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            let n = self
                .state
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                *self.state.first_defs.lock().unwrap() =
                    tools.iter().map(|d| d.function.name.clone()).collect();
                return Ok(LlmResponse {
                    content: String::new(),
                    tool_calls: vec![ToolCallInfo {
                        id: "tc_1".to_string(),
                        name: "write_file".to_string(),
                        arguments: r#"{"path":"x","content":"y"}"#.to_string(),
                    }],
                    finished: false,
                    reasoning_content: None,
                    usage: None,
                    raw_request_body: None,
                    raw_response_body: None,
                });
            }
            *self.state.second_messages.lock().unwrap() = messages
                .iter()
                .map(|m| format!("[{}] {}", m.role, m.content))
                .collect();
            Ok(LlmResponse {
                content: "gave up".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    let state = Arc::new(PhaseState {
        calls: std::sync::atomic::AtomicUsize::new(0),
        first_defs: std::sync::Mutex::new(Vec::new()),
        second_messages: std::sync::Mutex::new(Vec::new()),
    });
    let mut agent_loop = AgentLoop::new(
        Box::new(TwoPhase {
            state: state.clone(),
        }),
        test_config(),
    );
    // 只注册白名单内的读工具（write_file 从未注册——readonly 档的现实形态）。
    agent_loop.register_tool(
        "read_file".to_string(),
        Box::new(EchoTool {
            result: "file body".to_string(),
        }),
    );

    agent_loop
        .run_detached(
            "peek then write",
            DetachedOpts {
                allowed_tools: Some(crate::loop_tools::DETACHED_READONLY_TOOLS),
                ..DetachedOpts::default()
            },
        )
        .await
        .expect("readonly 轮次应成功收尾");

    let defs = state.first_defs.lock().unwrap().clone();
    assert!(
        defs.contains(&"read_file".to_string()),
        "白名单内读工具必须供给: {defs:?}"
    );
    assert!(
        !defs.contains(&"write_file".to_string()),
        "write_file 不得进 readonly 供给: {defs:?}"
    );

    let messages = state.second_messages.lock().unwrap().join("\n");
    assert!(
        messages.contains("Unknown tool 'write_file'"),
        "硬调 write_file 必须被 dispatch 回灌 unknown tool 错误，实际消息:\n{messages}"
    );
}

// ---------------------------------------------------------------------------
// G2：深度限制（agents.subagent.max_depth）
// ---------------------------------------------------------------------------

/// 单元级：max_depth=2、父深度 1（子代理）发起 spawn → 放行，闭包收到
/// 子深度 2（孙代理）——「max_depth=2 配置时孙代理可达」验收。
#[tokio::test]
async fn spawn_depth_within_limit_flows_child_depth_to_closure() {
    let mut tool = crate::loop_tools::SpawnTool::new(crate::loop_tools::SpawnConfig {
        default_model: "test-model".to_string(),
        max_concurrent: 4,
        max_depth: 2,
    });
    let seen: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let s = seen.clone();
    tool.set_spawn_fn(Arc::new(
        move |_a: &str,
              task: &str,
              _m: &str,
              _c: &str,
              _ch: &str,
              _p: &str,
              depth: usize,
              _bg: bool| {
            let s = s.clone();
            let task = task.to_string();
            Box::pin(async move {
                s.lock().unwrap().push(format!("{task}:d{depth}"));
                Ok("grandchild ran".to_string())
            })
        },
    ));

    // 模拟子代理（深度 1）上下文里的一次 spawn 调度。
    tool.set_invocation_depth(1);
    let ctx = RequestContext::new("web", "c", "u", "s");
    let out = tool
        .execute(r#"{"task":"grandchild"}"#, &ctx)
        .await
        .expect("depth 1 + max_depth 2 必须放行（孙代理可达）");
    assert_eq!(out, "grandchild ran");
    assert_eq!(
        seen.lock().unwrap().clone(),
        vec!["grandchild:d2"],
        "闭包必须收到子深度 = 父深度 + 1"
    );
}

/// 单元级：max_depth=1、父深度 1（子代理）发起 spawn → 在占信号量/触
/// 闭包之前诚实拒绝，文案带上限与自救指引。
#[tokio::test]
async fn spawn_depth_limit_rejects_before_spawn() {
    let mut tool = crate::loop_tools::SpawnTool::new(spawn_config(4)); // max_depth=1
    let called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = called.clone();
    tool.set_spawn_fn(Arc::new(
        move |_a: &str, _t: &str, _m: &str, _c: &str, _ch: &str, _p: &str, _d: usize, _bg: bool| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok("should not run".to_string()) })
        },
    ));

    tool.set_invocation_depth(1);
    let ctx = RequestContext::new("web", "c", "u", "s");
    let err = tool
        .execute(r#"{"task":"grandchild"}"#, &ctx)
        .await
        .expect_err("depth 1 + max_depth 1 必须拒绝孙代理");
    assert!(
        err.contains("Sub-agent depth limit (1) reached"),
        "拒绝文案须带上限数字供模型识别，实际: {err}"
    );
    assert_eq!(
        called.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "拒绝路径不得触碰 spawn 闭包"
    );

    // 顶层（深度 0）同一工具照常放行——限制只作用于超限层。
    tool.set_invocation_depth(0);
    tool.execute(r#"{"task":"top"}"#, &ctx)
        .await
        .expect("深度 0 + max_depth 1 必须放行顶层 spawn");
}

/// 验收（G2）e2e：`run_detached` 派生的子代理（depth=1）的 LLM 调 spawn →
/// 串行分发注入 instance 深度 → SpawnTool 拒绝 → 错误作为工具结果回灌
/// 下一轮 LLM 请求（模型可见，能改道自己完成任务）。
#[tokio::test]
async fn run_detached_depth_enforcement_grandchild_rejected() {
    // 两段式 provider：第 1 轮（子代理上下文）返回 spawn 工具调用；
    // 第 2 轮记全部消息并收尾。状态放 Arc（provider 会被 move 进 Box）。
    struct PhaseState {
        calls: std::sync::atomic::AtomicUsize,
        second_messages: std::sync::Mutex<Vec<String>>,
    }
    struct TwoPhase {
        state: Arc<PhaseState>,
    }
    #[async_trait]
    impl LlmProvider for TwoPhase {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            let n = self
                .state
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                return Ok(LlmResponse {
                    content: String::new(),
                    tool_calls: vec![ToolCallInfo {
                        id: "tc_g2".to_string(),
                        name: "spawn".to_string(),
                        arguments: r#"{"task":"grandchild task"}"#.to_string(),
                    }],
                    finished: false,
                    reasoning_content: None,
                    usage: None,
                    raw_request_body: None,
                    raw_response_body: None,
                });
            }
            *self.state.second_messages.lock().unwrap() = _messages
                .iter()
                .map(|m| format!("[{}] {}", m.role, m.content))
                .collect();
            Ok(LlmResponse {
                content: "completed myself instead".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    let state = Arc::new(PhaseState {
        calls: std::sync::atomic::AtomicUsize::new(0),
        second_messages: std::sync::Mutex::new(Vec::new()),
    });
    let mut agent_loop = AgentLoop::new(
        Box::new(TwoPhase {
            state: state.clone(),
        }),
        test_config(),
    );
    // 注册真 SpawnTool（max_depth=1 默认档）：子代理（depth 1）调它必被拒。
    let mut spawn_tool = crate::loop_tools::SpawnTool::new(spawn_config(4));
    spawn_tool.set_spawn_fn(Arc::new(
        |_a: &str, _t: &str, _m: &str, _c: &str, _ch: &str, _p: &str, _d: usize, _bg: bool| {
            Box::pin(async { Ok("grandchild should never run".to_string()) })
        },
    ));
    agent_loop.register_tool("spawn".to_string(), Box::new(spawn_tool));

    // 关键：opts.depth=1 模拟「这个 detached 轮次本身是子代理」——
    // instance.detached_depth()=1 会被串行分发注入 SpawnTool。
    let out = agent_loop
        .run_detached(
            "child task",
            DetachedOpts {
                depth: 1,
                ..DetachedOpts::default()
            },
        )
        .await
        .expect("子代理轮次本身应成功收尾（拒绝只是工具结果）");
    assert_eq!(out, "completed myself instead");

    let messages = state.second_messages.lock().unwrap().join("\n");
    assert!(
        messages.contains("Sub-agent depth limit (1) reached"),
        "深度拒绝必须作为工具结果回灌下一轮 LLM 请求，实际消息:\n{messages}"
    );
    assert!(
        !messages.contains("grandchild should never run"),
        "被拒的 spawn 不得真的跑到闭包"
    );
}

// ---------------------------------------------------------------------------
// ⑥ 裸提示词模式（Swarm M1）：system_prompt 覆盖 + no_tools 零供给 + label
// ---------------------------------------------------------------------------

/// 捕获每次请求的 (首条 system 消息内容, 工具名列表) 的迷你 provider。
struct BareCaptureProvider {
    seen: std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<String>)>>>,
}

#[async_trait]
impl LlmProvider for BareCaptureProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let system = messages
            .first()
            .filter(|m| m.role == "system")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        self.seen.lock().unwrap().push((
            system,
            tools.iter().map(|d| d.function.name.clone()).collect(),
        ));
        Ok(LlmResponse {
            content: "{\"plan\":[]}".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

#[tokio::test]
async fn bare_mode_replaces_persona_and_hides_all_tools() {
    let seen: std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<String>)>>> = Default::default();
    let mut agent_loop = AgentLoop::new(
        Box::new(BareCaptureProvider { seen: seen.clone() }),
        test_config(), // 人格 = "You are a test assistant."
    );
    agent_loop.register_tool(
        "echo_alpha".to_string(),
        Box::new(EchoTool {
            result: "a".to_string(),
        }),
    );

    let out = agent_loop
        .run_detached(
            "拆解这个任务",
            DetachedOpts {
                system_prompt: Some("You are a JSON planning machine."),
                no_tools: true,
                label: Some("board-planner"),
                max_turns: 1,
                ..DetachedOpts::default()
            },
        )
        .await
        .expect("bare 模式单轮应成功");
    assert_eq!(out, "{\"plan\":[]}");

    let captures = seen.lock().unwrap();
    assert_eq!(
        captures.len(),
        1,
        "max_turns=1 + 零工具 = 恰一次 LLM 调用，实际 {} 次",
        captures.len()
    );
    let (system, tools) = &captures[0];
    assert_eq!(
        system, "You are a JSON planning machine.",
        "裸提示词必须整体替换人格 system prompt，实际: {system}"
    );
    assert!(!system.contains("test assistant"), "人格字符串不得残留");
    assert!(tools.is_empty(), "no_tools 必须零供给，实际 {tools:?}");
}

#[tokio::test]
async fn default_detached_keeps_tool_supply() {
    // 对照组：不带 no_tools（Default=false）供给链照常——防裸模式误伤常态
    // detached 路径（spawn 工具 / headless run 都依赖全量供给）。
    let seen: std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<String>)>>> = Default::default();
    let mut agent_loop = AgentLoop::new(
        Box::new(BareCaptureProvider { seen: seen.clone() }),
        test_config(),
    );
    agent_loop.register_tool(
        "echo_alpha".to_string(),
        Box::new(EchoTool {
            result: "a".to_string(),
        }),
    );

    agent_loop
        .run_detached("task", DetachedOpts::default())
        .await
        .expect("常态 detached 应成功");

    let captures = seen.lock().unwrap();
    assert!(
        !captures.is_empty() && captures[0].1.contains(&"echo_alpha".to_string()),
        "常态 detached 必须保留工具供给，实际 {:?}",
        captures.first().map(|c| &c.1)
    );
}

#[tokio::test]
async fn detached_session_key_label_format() {
    let labeled = AgentLoop::detached_session_key(Some("board-planner"));
    assert!(
        labeled.starts_with("subagent:board-planner:"),
        "label 必须作为 session_key 中段（日志检索锚点），实际: {labeled}"
    );
    let plain = AgentLoop::detached_session_key(None);
    assert!(
        plain.starts_with("subagent:") && !plain["subagent:".len()..].contains(':'),
        "无 label 维持 subagent:{{uuid}} 现状，实际: {plain}"
    );
}

// ---------------------------------------------------------------------------
// ⑥ Swarm G13：detached 路径补合成 ConversationStart/End
// ---------------------------------------------------------------------------

/// 请求日志观察者（RequestLoggerObserver/ClusterRequestLoggerObserver）的
/// active 表以 start 事件注册 trace_id，缺 start 则 LlmRequest/LlmResponse
/// 全部被静默丢弃——评审/子代理/无头任务的 LLM 调用将不可回放。本测试锁：
/// run_detached 全程发射 start → LlmRequest → LlmResponse → end 的有序闭环。
#[tokio::test]
async fn run_detached_emits_observer_lifecycle_for_request_logging() {
    use nemesis_observer::{ConversationEvent, EventType, Manager, Observer};
    use std::sync::Mutex as StdMutex;

    struct RecordingObserver {
        events: StdMutex<Vec<EventType>>,
    }
    #[async_trait::async_trait]
    impl Observer for RecordingObserver {
        fn name(&self) -> &str {
            "recorder"
        }
        async fn on_event(&self, event: ConversationEvent) {
            self.events.lock().unwrap().push(event.event_type);
        }
    }

    let provider = DetachedMockProvider::ok(vec![LlmResponse {
        content: "sub-agent final answer".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let recorder = Arc::new(RecordingObserver {
        events: StdMutex::new(Vec::new()),
    });
    let mgr = Arc::new(Manager::new());
    mgr.register(recorder.clone()).await;
    agent_loop.set_observer_manager(mgr);

    let out = agent_loop
        .run_detached("summarize the file", DetachedOpts::default())
        .await
        .expect("Done 事件应提取为 Ok");
    assert_eq!(out, "sub-agent final answer");

    let seq = recorder.events.lock().unwrap().clone();
    let pos = |t: EventType| seq.iter().position(|e| *e == t);
    let (start, req, resp, end) = (
        pos(EventType::ConversationStart),
        pos(EventType::LlmRequest),
        pos(EventType::LlmResponse),
        pos(EventType::ConversationEnd),
    );
    assert!(
        start.is_some(),
        "必须发射 ConversationStart（trace 注册锚）"
    );
    assert!(
        req.is_some() && resp.is_some(),
        "LLM 请求/响应事件必须到达观察者"
    );
    assert!(end.is_some(), "必须发射 ConversationEnd（active 表收尾）");
    assert!(
        start.unwrap() < req.unwrap()
            && req.unwrap() < resp.unwrap()
            && resp.unwrap() < end.unwrap(),
        "事件必须按 start → request → response → end 有序，实际顺序: {:?}",
        seq
    );
}
