// P2-0 characterization + golden transcript harness
// (docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §7 T1/T2)。
//
// golden transcript = P2 解剖的等价性最强证据：每个场景把**全部可观测输出
// 向量**录成一份 JSON 基线（事件向量 / provider 实收 messages+tool_defs /
// observer 事件 / instance history / chat_log jsonl 行 / boundary sidecar
// 行），P2 各 PR 重放后归一化逐字节 diff 必须为零（§7 T2）。
//
// 基线在 P1 物理拆分后的语义零变化代码上录制（nemesis-agent 2278 全绿），
// 入库于 `loop/testdata/golden/<scenario>.json`。重录：GOLDEN_RECORD=1
// cargo test -p nemesis-agent golden（仅允许伴随行为变更 PR，见 §7.4）。
//
// 归一化清单（冻结；录制与比对两端同规则；清单外差异一律视为漂移）：
//   1. 递归剥离键：ts / time / timestamp / duration_ms / elapsed_ms /
//      updated_at（时间戳、耗时、Instant 序列化残留）。
//   2. trace_id 由 harness 固定传入（"trace-golden"，不经 run() 的
//      nanos 生成），全程确定性。
//   3. 会话键固定（golden_* 前缀 + GOLDEN_LOCK 进程内串行），无随机
//      后缀；文件用后即清。
//   4. K3 shell 注入的时间/环境快照：`# Current Time / Environment
//      snapshot` 段内的 `YYYY-MM-DD HH:MM (Weekday)` 行整行替换为
//      `<TIME_SNAPSHOT>`（随钟逐分漂移的已知波动面；段内其余内容钉死）。
// 平台路径分隔符不出现在任何录制面（全部相对内容），无需归一。
//
// T1 覆盖对照（§4.3 出口 → 锚点；★=本文件 golden 场景，☆=既有测试）：
//   cancel/estop 顶检        ★golden_estop_top  ☆tests(estop_*)
//   budget/max_turns 出口    ★golden_max_turns_exhausted ☆tests(length_*预算系)
//   LLM pre-hook 拦截        ☆(hooks 侧 lifecycle 测试；golden 场景不接 hook 基建)
//   select cancel/estop 臂   ★golden_estop_mid_llm
//   context 环耗尽/transient 耗尽  ☆tests(2416 AlwaysContextError/445 ErrorProvider)
//   post-hook Block/重呼      ☆(hooks 侧)
//   截断续写/续写预算耗尽     ★golden_length_continuation ★golden_length_budget_exhausted
//   steer escape/degenerate  ☆tests(degenerate/steer 系)
//   heartbeat/Accept/GiveUp  ★golden_plain_qa（Accept 主路径）☆tests(heartbeat)
//   批内 cancel/estop 双发   ☆tests(estop_blocks_remaining_tools_in_batch)
//   __ASYNC__/__BG_SPAWN__   ☆e3_tests / g4_background_spawn_tests（需 continuation
//                             基建，standalone 不可达，不进 golden 场景集）
//   escalation/validation    ★golden_validation_retry_budget
//   429 环 / transient 环 / context 环
//                            ★golden_rate_limit_retry ★golden_transient_retry
//                             ★golden_context_error_retry
//
// 刻意设计：本文件测试用进程级串行锁（GOLDEN_LOCK）保护固定会话键的
// chat_log/boundary 文件读写，guard 跨 async 测试体的 await 持有；
// #[tokio::test] 每个测试独立 current_thread runtime，持锁方在自己线程上
// 恢复运行，不会死锁。测试域统一豁免（逐处 allow 不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;

/// 进程内串行锁：golden 场景共用固定会话键落盘文件，防止并行互踩。
static GOLDEN_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// 归一化剥离键（冻结清单，见文件头）。
const NORMALIZE_STRIP_KEYS: [&str; 6] = [
    "ts",
    "time",
    "timestamp",
    "duration_ms",
    "elapsed_ms",
    "updated_at",
];

/// 行首是否为 `YYYY-MM-DD ` 时间戳形态（shell 注入的时间快照行，逐分漂移）。
fn is_timed_line(line: &str) -> bool {
    let b = line.as_bytes();
    b.len() >= 11
        && b[0..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
        && b[10] == b' '
}

/// 字符串值归一化：时间快照行整行替换（K3 注入的时间/环境快照随钟走，
/// 属已知波动面；行内其余快照内容保持钉死）。
fn normalize_string(s: &str) -> String {
    if !s.contains("# Current Time / Environment snapshot") {
        return s.to_string();
    }
    s.lines()
        .map(|line| {
            if is_timed_line(line) {
                "<TIME_SNAPSHOT>"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 递归归一化：剥离 [`NORMALIZE_STRIP_KEYS`] 命中的对象键 + 字符串时间行替换。
fn normalize_value(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            for k in NORMALIZE_STRIP_KEYS {
                map.remove(k);
            }
            for (_k, val) in map.iter_mut() {
                normalize_value(val);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                normalize_value(item);
            }
        }
        serde_json::Value::String(s) => {
            let norm = normalize_string(s);
            if norm != *s {
                *s = norm;
            }
        }
        _ => {}
    }
}

/// provider 一次实收调用（T2「provider 实收内容」录制面）。
struct CapturedCall {
    model: String,
    messages: Vec<LlmMessage>,
    tools: Vec<crate::types::ToolDefinition>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    reasoning_effort: Option<String>,
}

/// 金色录制 provider：按脚本出牌 + 捕获每次 chat() 的实收内容。
/// 脚本耗尽时的兜底与 tests::MockLlmProvider 一致（"No more responses"）。
struct GoldenCaptureProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    calls: Arc<std::sync::Mutex<Vec<CapturedCall>>>,
}

impl GoldenCaptureProvider {
    fn new(responses: Vec<LlmResponse>) -> (Self, Arc<std::sync::Mutex<Vec<CapturedCall>>>) {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                responses: std::sync::Mutex::new(responses),
                calls: calls.clone(),
            },
            calls,
        )
    }
}

#[async_trait]
impl LlmProvider for GoldenCaptureProvider {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let opts = options.unwrap_or_default();
        self.calls.lock().unwrap().push(CapturedCall {
            model: model.to_string(),
            messages,
            tools,
            max_tokens: opts.max_tokens,
            temperature: opts.temperature,
            reasoning_effort: opts.reasoning_effort.clone(),
        });
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "No more responses".to_string(),
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

// --- 响应构造器（与 tests.rs 同形状，字段显式防漂移） ---

fn assistant(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn tool_call_response(content: &str, calls: Vec<ToolCallInfo>) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: calls,
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// completion_tokens 顶满默认 max_tokens（8192）——截断信号。
fn cap_usage() -> Option<crate::loop_executor::ObserverUsageInfo> {
    Some(crate::loop_executor::ObserverUsageInfo {
        prompt_tokens: 100,
        completion_tokens: 8192,
        total_tokens: 8292,
        cached_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
    })
}

fn truncated(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: false,
        reasoning_content: None,
        usage: cap_usage(),
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

/// 场景工具：恒定输出（失败场景用 GoldenFailTool）。
struct GoldenTool {
    result: &'static str,
}

#[async_trait]
impl Tool for GoldenTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        Ok(self.result.to_string())
    }
}

/// 恒败工具（工具错误回灌路径）。
struct GoldenFailTool;

#[async_trait]
impl Tool for GoldenFailTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        Err("boom".to_string())
    }
}

/// 带 required 字段的工具（schema 校验失败 → 校验重试预算路径）。
struct GoldenStrictTool;

#[async_trait]
impl Tool for GoldenStrictTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        Ok("ok".to_string())
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})
    }
}

// --- transcript 组装 + 录制/比对 ---

#[derive(serde::Serialize)]
struct GoldenTranscript {
    scenario: String,
    /// run_with_trace 返回事件向量（有序逐字符串）。
    events: Vec<serde_json::Value>,
    /// observer callback 收到的 (event_type, data) 序列。
    observer: Vec<serde_json::Value>,
    /// provider 实收调用序列。
    calls: Vec<serde_json::Value>,
    /// instance 历史终态快照。
    history: Vec<serde_json::Value>,
    /// chat_log jsonl 行（sessions_log_dir/<key>.jsonl）。
    chat_log: Vec<serde_json::Value>,
    /// boundary sidecar 行（boundary/<key>.jsonl）。
    boundary: Vec<serde_json::Value>,
}

impl GoldenTranscript {
    fn finish(self) -> serde_json::Value {
        let mut v = serde_json::to_value(&self).unwrap();
        normalize_value(&mut v);
        v
    }
}

fn golden_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/loop/testdata/golden")
}

/// 录制（GOLDEN_RECORD=1）或逐字节比对基线；diff 非零即 panic（§7.4 情况 1）。
fn golden_assert(scenario: &str, transcript: GoldenTranscript) {
    let actual = transcript.finish();
    let pretty = serde_json::to_string_pretty(&actual).unwrap();
    let path = golden_dir().join(format!("{scenario}.json"));
    if std::env::var("GOLDEN_RECORD").is_ok() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(&path, &pretty).unwrap();
        println!("[golden] recorded baseline: {}", path.display());
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "[golden] baseline missing: {} ({e})——用 GOLDEN_RECORD=1 在未改动代码上录制",
            path.display()
        )
    });
    let expected_v: serde_json::Value = serde_json::from_str(&expected).unwrap();
    if expected_v != actual {
        // 找出首个分歧路径，报文可定位。
        let ep = serde_json::to_string_pretty(&expected_v).unwrap();
        for (i, (a, b)) in ep.lines().zip(pretty.lines()).enumerate() {
            if a != b {
                panic!(
                    "[golden] {scenario} 行为漂移 @line {}:\n  expected: {a}\n  actual:   {b}\n\
                     全量 diff 见两份 transcript；禁止改测试迁就代码（§7.4 情况 1）",
                    i + 1
                );
            }
        }
        panic!(
            "[golden] {scenario} 行为漂移（行数不同 expected={} actual={}）",
            ep.lines().count(),
            pretty.lines().count()
        );
    }
}

// --- 场景公共件 ---

fn golden_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
    }
}

/// 固定会话键（golden_* 无冒号 → sanitize 恒等映射，无嵌套目录）。
const SESSION_KEY: &str = "golden_scenario";

fn chat_log_path() -> std::path::PathBuf {
    let safe = nemesis_utils::sanitize::sanitize_path_segment(SESSION_KEY);
    nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{safe}.jsonl"))
}

fn boundary_log_path() -> std::path::PathBuf {
    let safe = nemesis_utils::sanitize::sanitize_path_segment(SESSION_KEY);
    nemesis_path::default_path_manager()
        .boundary_events_dir()
        .join(format!("{safe}.jsonl"))
}

fn cleanup_golden_files() {
    for p in [chat_log_path(), boundary_log_path()] {
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

fn read_jsonl(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// 场景驱动器：装配 loop + observer 捕获 + 固定 trace 跑一轮，收集全部录制面。
struct ScenarioRun {
    events: Vec<AgentEvent>,
    observer: Vec<(String, serde_json::Value)>,
    calls: Arc<std::sync::Mutex<Vec<CapturedCall>>>,
    instance: AgentInstance,
}

async fn drive(
    provider: GoldenCaptureProvider,
    calls: Arc<std::sync::Mutex<Vec<CapturedCall>>>,
    config: AgentConfig,
    wire: impl FnOnce(&mut AgentLoop),
    user_message: &str,
) -> ScenarioRun {
    drive_with_capture_provider(Box::new(provider), calls, config, wire, user_message).await
}

/// 同 [`drive`]，但 provider 由调用方自组（脚本逻辑需状态机时）。
async fn drive_with_capture_provider(
    provider: Box<dyn LlmProvider>,
    calls: Arc<std::sync::Mutex<Vec<CapturedCall>>>,
    config: AgentConfig,
    wire: impl FnOnce(&mut AgentLoop),
    user_message: &str,
) -> ScenarioRun {
    let mut agent_loop = AgentLoop::new(provider, config.clone());
    let observer_log: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = observer_log.clone();
    agent_loop.set_observer_callback(Arc::new(move |event: &str, data: &serde_json::Value| {
        sink.lock().unwrap().push((event.to_string(), data.clone()));
    }));
    wire(&mut agent_loop);
    let instance = AgentInstance::new(config);
    let context = RequestContext::new("web", "chat1", "user1", SESSION_KEY);
    let token = tokio_util::sync::CancellationToken::new();
    let events = agent_loop
        .run_with_trace(
            &instance,
            user_message,
            &context,
            "trace-golden",
            false,
            &token,
            None,
            &[],
        )
        .await;
    ScenarioRun {
        events,
        observer: std::mem::take(&mut *observer_log.lock().unwrap()),
        calls,
        instance,
    }
}

fn assemble(scenario: &str, run: ScenarioRun) -> GoldenTranscript {
    let mut call_values = Vec::new();
    for c in run.calls.lock().unwrap().iter() {
        call_values.push(serde_json::json!({
            "model": c.model,
            "messages": c.messages,
            "tools": c.tools,
            "max_tokens": c.max_tokens,
            "temperature": c.temperature,
            "reasoning_effort": c.reasoning_effort,
        }));
    }
    GoldenTranscript {
        scenario: scenario.to_string(),
        events: run
            .events
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect(),
        observer: run
            .observer
            .iter()
            .map(|(k, v)| serde_json::json!({"event": k, "data": v}))
            .collect(),
        calls: call_values,
        history: run
            .instance
            .get_history()
            .iter()
            .map(|t| serde_json::to_value(t).unwrap())
            .collect(),
        chat_log: read_jsonl(&chat_log_path()),
        boundary: read_jsonl(&boundary_log_path()),
    }
}

// --- 场景（T2 场景集；锚点对照见文件头 T1 表） ---

/// 纯问答：单轮 Accept 落地（6981 出口主路径）。
#[tokio::test]
async fn golden_plain_qa() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    let (provider, calls) = GoldenCaptureProvider::new(vec![assistant("4")]);
    let run = drive(
        provider,
        calls,
        golden_config(),
        |_| {},
        "What is two plus two?",
    )
    .await;
    golden_assert("plain_qa", assemble("plain_qa", run));
    cleanup_golden_files();
}

/// 工具调用轮：中间轮叙述 + 工具执行 + 结果回灌 + 终答。
#[tokio::test]
async fn golden_tool_round() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    let (provider, calls) = GoldenCaptureProvider::new(vec![
        tool_call_response(
            "Let me calculate.",
            vec![tool_call("tc_1", "calculator", r#"{"expr":"2+2"}"#)],
        ),
        assistant("The answer is 4."),
    ]);
    let run = drive(
        provider,
        calls,
        golden_config(),
        |l| {
            l.register_tool(
                "calculator".to_string(),
                Box::new(GoldenTool { result: "4" }),
            );
        },
        "calculate 2+2",
    )
    .await;
    golden_assert("tool_round", assemble("tool_round", run));
    cleanup_golden_files();
}

/// 工具执行失败：错误结果回灌（⑤′/⑥ 基础路径），模型终答认账。
#[tokio::test]
async fn golden_tool_error() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    let (provider, calls) = GoldenCaptureProvider::new(vec![
        tool_call_response("", vec![tool_call("tc_1", "calculator", "{}")]),
        assistant("Tool failed, sorry."),
    ]);
    let run = drive(
        provider,
        calls,
        golden_config(),
        |l| {
            l.register_tool("calculator".to_string(), Box::new(GoldenFailTool));
        },
        "do a doomed calculation",
    )
    .await;
    golden_assert("tool_error", assemble("tool_error", run));
    cleanup_golden_files();
}

/// 429 限流重试环：首败 (retry_after=2s) → max(2, 阶梯 5) 等待（tokio
/// start_paused 自动推进，零真实等待）→ 次呼成功。
#[tokio::test(start_paused = true)]
async fn golden_rate_limit_retry() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    struct RateLimitedThenSuccess {
        call_count: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl LlmProvider for RateLimitedThenSuccess {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            let count = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                Err("rate limited by provider p/m (retry_after=2s)".to_string())
            } else {
                Ok(assistant("Recovered after rate limit."))
            }
        }
    }
    let provider = RateLimitedThenSuccess {
        call_count: std::sync::atomic::AtomicUsize::new(0),
    };
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let run =
        drive_with_capture_provider(Box::new(provider), calls, golden_config(), |_| {}, "hello")
            .await;
    golden_assert("rate_limit_retry", assemble("rate_limit_retry", run));
    cleanup_golden_files();
}

/// transient 重试环：瞬时网络错误（立即失败 → backoff_eligible=false 零退避）
/// → 次呼成功。
#[tokio::test]
async fn golden_transient_retry() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    struct TransientThenSuccess {
        call_count: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl LlmProvider for TransientThenSuccess {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            let count = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                Err("connection reset by peer".to_string())
            } else {
                Ok(assistant("Recovered!"))
            }
        }
    }
    let provider = TransientThenSuccess {
        call_count: std::sync::atomic::AtomicUsize::new(0),
    };
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let run =
        drive_with_capture_provider(Box::new(provider), calls, golden_config(), |_| {}, "hello")
            .await;
    golden_assert("transient_retry", assemble("transient_retry", run));
    cleanup_golden_files();
}

/// context 压缩重试环：context 超限错误 → 压缩重试 → 次呼成功。
#[tokio::test]
async fn golden_context_error_retry() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    struct ContextErrorThenSuccess {
        call_count: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl LlmProvider for ContextErrorThenSuccess {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            let count = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                Err("context_length_exceeded: token limit".to_string())
            } else {
                Ok(assistant("Recovered!"))
            }
        }
    }
    let provider = ContextErrorThenSuccess {
        call_count: std::sync::atomic::AtomicUsize::new(0),
    };
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let run =
        drive_with_capture_provider(Box::new(provider), calls, golden_config(), |_| {}, "hello")
            .await;
    golden_assert("context_error_retry", assemble("context_error_retry", run));
    cleanup_golden_files();
}

/// 截断续写：max_tokens 顶格 → 续写环丢部分稿 → 次呼终答。
#[tokio::test]
async fn golden_length_continuation() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    let (provider, calls) = GoldenCaptureProvider::new(vec![
        truncated("partial preamble that got cut"),
        assistant("completed after continuing"),
    ]);
    let run = drive(provider, calls, golden_config(), |_| {}, "write a big file").await;
    golden_assert("length_continuation", assemble("length_continuation", run));
    cleanup_golden_files();
}

/// 续写预算耗尽：反复截断 → 「输出反复超过 max_tokens」诚实终局（6876 出口）。
#[tokio::test]
async fn golden_length_budget_exhausted() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    let (provider, calls) = GoldenCaptureProvider::new(vec![truncated("x"); 7]);
    let mut cfg = golden_config();
    cfg.max_turns = 12;
    let run = drive(provider, calls, cfg, |_| {}, "write a huge file").await;
    golden_assert(
        "length_budget_exhausted",
        assemble("length_budget_exhausted", run),
    );
    cleanup_golden_files();
}

/// 工具参数 schema 校验失败 → 校验重试预算 → 预算耗尽终局（escalation 出口）。
#[tokio::test]
async fn golden_validation_retry_budget() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    let (provider, calls) = GoldenCaptureProvider::new(vec![
        tool_call_response("", vec![tool_call("tc_1", "strict_tool", "{}")]),
        assistant("Giving up on the tool."),
    ]);
    let run = drive(
        provider,
        calls,
        golden_config(),
        |l| {
            l.register_tool("strict_tool".to_string(), Box::new(GoldenStrictTool));
        },
        "use the strict tool",
    )
    .await;
    golden_assert(
        "validation_retry_budget",
        assemble("validation_retry_budget", run),
    );
    cleanup_golden_files();
}

/// estop 在 LLM 调用中触发：select! estop 臂 → Done 终局（6157 出口）。
#[tokio::test]
async fn golden_estop_mid_llm() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    struct HangingProvider {
        entered: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait]
    impl LlmProvider for HangingProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            self.entered
                .store(true, std::sync::atomic::Ordering::SeqCst);
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
    let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let provider = HangingProvider {
        entered: entered.clone(),
    };
    let estop = Arc::new(crate::estop::EstopState::new());
    let estop_wire = estop.clone();
    let estop_task = estop.clone();
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let wire = move |l: &mut AgentLoop| {
        l.set_estop(estop_wire);
    };
    // 先装配再挂起触发：chat 进入后 50ms 触发 estop。
    let mut agent_loop = AgentLoop::new(Box::new(provider), golden_config());
    let observer_log: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = observer_log.clone();
    agent_loop.set_observer_callback(Arc::new(move |event: &str, data: &serde_json::Value| {
        sink.lock().unwrap().push((event.to_string(), data.clone()));
    }));
    wire(&mut agent_loop);
    let agent_loop = Arc::new(agent_loop);
    let instance = AgentInstance::new(golden_config());
    let context = RequestContext::new("web", "chat1", "user1", SESSION_KEY);
    let token = tokio_util::sync::CancellationToken::new();
    let entered_for_task = entered.clone();
    let task = tokio::spawn(async move {
        while !entered_for_task.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        estop_task.trigger();
    });
    let events = agent_loop
        .run_with_trace(
            &instance,
            "hang then estop",
            &context,
            "trace-golden",
            false,
            &token,
            None,
            &[],
        )
        .await;
    let _ = task.await;
    let run = ScenarioRun {
        events,
        observer: std::mem::take(&mut *observer_log.lock().unwrap()),
        calls,
        instance,
    };
    golden_assert("estop_mid_llm", assemble("estop_mid_llm", run));
    cleanup_golden_files();
}

/// estop 顶检：engaged 状态直接拒收（5735 系出口，Done 一次性）。
#[tokio::test]
async fn golden_estop_top() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    let (provider, calls) = GoldenCaptureProvider::new(vec![]);
    let estop = Arc::new(crate::estop::EstopState::new());
    estop.trigger();
    let run = drive(
        provider,
        calls,
        golden_config(),
        move |l| {
            l.set_estop(estop);
        },
        "hello",
    )
    .await;
    golden_assert("estop_top", assemble("estop_top", run));
    cleanup_golden_files();
}

/// max_turns 耗尽：模型永远要工具 → 预算出口 + grace nudge（5796 出口）。
#[tokio::test]
async fn golden_max_turns_exhausted() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    let responses: Vec<LlmResponse> = (0..8)
        .map(|i| tool_call_response("", vec![tool_call(&format!("tc_{i}"), "calculator", "{}")]))
        .collect();
    let (provider, calls) = GoldenCaptureProvider::new(responses);
    let mut cfg = golden_config();
    cfg.max_turns = 1;
    let run = drive(
        provider,
        calls,
        cfg,
        |l| {
            l.register_tool(
                "calculator".to_string(),
                Box::new(GoldenTool { result: "4" }),
            );
        },
        "loop forever",
    )
    .await;
    golden_assert("max_turns_exhausted", assemble("max_turns_exhausted", run));
    cleanup_golden_files();
}

/// 取消顶检：外部 cancel token 预先取消 → Done（cancel 顶检出口）。
#[tokio::test]
async fn golden_cancel_top() {
    let _g = GOLDEN_LOCK.lock();
    cleanup_golden_files();
    let (provider, calls) = GoldenCaptureProvider::new(vec![]);
    let mut agent_loop = AgentLoop::new(Box::new(provider), golden_config());
    let observer_log: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = observer_log.clone();
    agent_loop.set_observer_callback(Arc::new(move |event: &str, data: &serde_json::Value| {
        sink.lock().unwrap().push((event.to_string(), data.clone()));
    }));
    let instance = AgentInstance::new(golden_config());
    let context = RequestContext::new("web", "chat1", "user1", SESSION_KEY);
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let events = agent_loop
        .run_with_trace(
            &instance,
            "hello",
            &context,
            "trace-golden",
            false,
            &token,
            None,
            &[],
        )
        .await;
    let run = ScenarioRun {
        events,
        observer: std::mem::take(&mut *observer_log.lock().unwrap()),
        calls,
        instance,
    };
    golden_assert("cancel_top", assemble("cancel_top", run));
    cleanup_golden_files();
}
