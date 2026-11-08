// B 批待办 2 确定性并发回归锁（barrier/channel 驱动，零 sleep）。
//
// 场景 ②（旧请求延迟失败不得清除新请求状态）：Queue 模式下 A 路 chat
// 门控后失败收尾，B 路已排队 —— A 的错误收尾不得破坏 B 的排队/重注入/
// 执行链；出站序列 = ⏳ 排队回执 → Error processing message → B 终答，
// 会话 busy 终态归零。
//
// 场景 ③（审批等待中 turn 被取消 → 迟到的批准不得执行）两层：
//   - 单元层（无 security）：cancel_session / cancel_all_sessions 与
//     approval_responder 槽联动，drain 参数（session_key + reason）正确；
//   - 集成层（security）：审批卡阻塞等待中取消 turn → drain 送拒绝裁决
//     → 阻塞立即醒来 → 工具不执行 → 终态「已取消」→ 迟到批准落空。
//     与既有 estop 全量清理（gateway watcher）/ 会话 clear 定向清理
//     （chat.rs）同槽不同挂点。
//
// 场景 ①（取消后延迟成功终态仍 Cancelled）已被既有
// s9_tests::cancel_during_tool_execution_breaks_turn 钉住（工具成功返回 +
// 终态已取消），不重复落锁。

use super::*;
use crate::test_support::capture_logs;
use nemesis_types::agent::ApprovalResponder;

// ---------------------------------------------------------------------------
// 共享 fixture
// ---------------------------------------------------------------------------

fn cfg() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn plain(content: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: "web:chat1".to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

fn resp(content: &str) -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn tool_resp(calls: Vec<crate::types::ToolCallInfo>) -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: String::new(),
        tool_calls: calls,
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn one_call(id: &str, name: &str) -> crate::types::ToolCallInfo {
    crate::types::ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        arguments: "{}".to_string(),
    }
}

/// 收 outbound 直到 200ms 静默（spawn 的回合任务不保证在 runner 返回前
/// 完成；timeout 等待非 sleep 竞态驱动）。
async fn drain_outbound(
    out_rx: &mut tokio::sync::mpsc::Receiver<nemesis_types::channel::OutboundMessage>,
) -> Vec<String> {
    let mut got = Vec::new();
    while let Ok(Some(m)) =
        tokio::time::timeout(std::time::Duration::from_millis(200), out_rx.recv()).await
    {
        got.push(m.content);
    }
    got
}

// ---------------------------------------------------------------------------
// 场景 ③ 单元层：取消方法与 approval_responder 槽联动（无 security）
// ---------------------------------------------------------------------------

/// 记录 drain 调用与会话裁决的 responder 假件。
#[derive(Default)]
struct RecordingResponder {
    drains: std::sync::Mutex<Vec<(String, String)>>,
    responds: std::sync::Mutex<Vec<(String, bool)>>,
}

impl RecordingResponder {
    fn new() -> Self {
        Self::default()
    }
}

impl nemesis_types::agent::ApprovalResponder for RecordingResponder {
    fn respond(
        &self,
        request_id: &str,
        approved: bool,
        _always: bool,
        _note: Option<String>,
    ) -> Result<bool, String> {
        self.responds
            .lock()
            .unwrap()
            .push((request_id.to_string(), approved));
        Ok(true)
    }
    fn pending(&self) -> Vec<serde_json::Value> {
        Vec::new()
    }
    fn resolve_orphans_for_session(&self, session_key: &str, reason: &str) -> usize {
        self.drains
            .lock()
            .unwrap()
            .push((session_key.to_string(), reason.to_string()));
        1
    }
}

/// 无 responder 槽（None）形态：取消路径 no-op，不炸。
#[tokio::test]
async fn cancel_without_responder_slot_is_noop() {
    let _logs = capture_logs();
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), cfg());
    agent_loop.cancel_tokens.insert(
        "agent:main:main".to_string(),
        tokio_util::sync::CancellationToken::new(),
    );
    assert!(agent_loop.cancel_session("agent:main:main"));
    // 无 responder 槽 = drain 臂 no-op（不炸不 panic）。token 在 turn 尾巴
    // （remove_cancel_token）前仍登记在册——本测试无回合在跑，条目仍在，
    // cancel_all 按表内存在计数再次取消它。
    assert_eq!(agent_loop.cancel_all_sessions(), 1);
}

/// 单会话取消：drain 只命中该会话，参数 =（session_key, "turn cancelled"）。
#[tokio::test]
async fn cancel_session_drains_only_that_sessions_pending_cards() {
    let _logs = capture_logs();
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), cfg());
    let rec = std::sync::Arc::new(RecordingResponder::new());
    agent_loop.set_approval_responder(rec.clone());
    agent_loop.cancel_tokens.insert(
        "agent:main:main".to_string(),
        tokio_util::sync::CancellationToken::new(),
    );
    agent_loop.cancel_tokens.insert(
        "agent:other:s2".to_string(),
        tokio_util::sync::CancellationToken::new(),
    );

    assert!(agent_loop.cancel_session("agent:main:main"));
    assert_eq!(
        rec.drains.lock().unwrap().clone(),
        vec![("agent:main:main".to_string(), "turn cancelled".to_string())],
        "定向取消只 drain 该会话的审批卡"
    );
}

/// 全量取消：每个活跃会话各 drain 一次；返回取消数。
#[tokio::test]
async fn cancel_all_sessions_drains_every_active_session() {
    let _logs = capture_logs();
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), cfg());
    let rec = std::sync::Arc::new(RecordingResponder::new());
    agent_loop.set_approval_responder(rec.clone());
    for k in ["agent:main:main", "agent:other:s2", "agent:other:s3"] {
        agent_loop
            .cancel_tokens
            .insert(k.to_string(), tokio_util::sync::CancellationToken::new());
    }

    assert_eq!(agent_loop.cancel_all_sessions(), 3);
    let mut drains = rec.drains.lock().unwrap().clone();
    drains.sort();
    assert_eq!(
        drains,
        vec![
            ("agent:main:main".to_string(), "turn cancelled".to_string()),
            ("agent:other:s2".to_string(), "turn cancelled".to_string()),
            ("agent:other:s3".to_string(), "turn cancelled".to_string()),
        ],
        "全量取消逐会话 drain，reason 同单会话语义"
    );
}

// ---------------------------------------------------------------------------
// 场景 ②：Queue 模式旧 turn 失败收尾不破坏已排队新 turn（零 sleep）
// ---------------------------------------------------------------------------

/// A 路 chat 门控后失败；B 路（重注入后的下一路）直接成功。
struct FailThenOkProvider {
    calls: std::sync::atomic::AtomicUsize,
    started_a: std::sync::Arc<tokio::sync::Notify>,
    release_a: std::sync::Arc<tokio::sync::Notify>,
    started_b: std::sync::Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl LlmProvider for FailThenOkProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            self.started_a.notify_one();
            self.release_a.notified().await;
            Err("boom-a".to_string())
        } else {
            self.started_b.notify_one();
            Ok(resp("B-done"))
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_mode_stale_turn_failure_keeps_queued_turn_intact() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let provider = std::sync::Arc::new(FailThenOkProvider {
        calls: std::sync::atomic::AtomicUsize::new(0),
        started_a: std::sync::Arc::new(tokio::sync::Notify::new()),
        release_a: std::sync::Arc::new(tokio::sync::Notify::new()),
        started_b: std::sync::Arc::new(tokio::sync::Notify::new()),
    });
    let agent_loop = AgentLoop::new_bus(
        Box::new(FailThenOkProviderHandle(provider.clone())),
        cfg(),
        out_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );
    let handle = std::sync::Arc::new(agent_loop);
    let runner = tokio::spawn(handle.clone().run_bus_arc(in_rx));

    // A 路进 chat（busy 已置位——SessionBusyGuard 在 process_admitted 头部）。
    in_tx.send(plain("stale turn a")).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        provider.started_a.notified(),
    )
    .await
    .expect("turn A chat started");
    // B 到达 → busy 入队。⏳ 回执抵达 = B 已确定在队列（确定性入队证据，
    // 替代旧测试的 sleep 间隙）。
    in_tx.send(plain("queued turn b")).await.unwrap();
    let receipt = tokio::time::timeout(std::time::Duration::from_secs(3), out_rx.recv())
        .await
        .expect("enqueue receipt in time")
        .expect("outbound");
    assert!(
        receipt.content.contains("已排队"),
        "B 必须拿到排队回执（busy 入队成立），got: {}",
        receipt.content
    );
    // 入队成立的可观测面：busy 状态键 = 路由裁决键（bus 模式默认解析器
    // 把非预置入站键改写为 `agent:main:main`，dm_scope=main），不是入站
    // session_key 字面量。此刻 A 在跑 → busy=true。queue_length 是旧计数
    // 器（busy 排队实体在 crate::inbox，不递增它）——队列证据由 ⏳ 回执
    // 承担，这里只钉 busy 位与键。
    assert_eq!(
        handle.get_session_busy_state("agent:main:main"),
        (true, 0),
        "B 入队后该会话必须呈 busy（键=路由裁决键；queue_length 为旧计数器恒 0）"
    );

    // 放行 A → 失败收尾 → 尾部 drain 重注入 B → B 的 chat 开跑。
    provider.release_a.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        provider.started_b.notified(),
    )
    .await
    .expect("turn B chat started after A's failure tail drain");
    drop(in_tx);
    let _ = runner.await;

    let mut outs = vec![receipt.content];
    outs.extend(drain_outbound(&mut out_rx).await);
    assert!(
        outs.iter()
            .any(|c| c.contains("Error processing message") && c.contains("boom-a")),
        "A 的失败必须以错误回复收场（不得静默），序列: {outs:?}"
    );
    assert!(
        outs.iter().any(|c| c.contains("B-done")),
        "B 必须在 A 失败后照常执行完成（A 的失败收尾不得吞掉已排队 turn），序列: {outs:?}"
    );
    assert!(
        !outs.iter().any(|c| c.contains("排队已满")),
        "Queue 模式不得把 B 弹回（那是 Reject 语义），序列: {outs:?}"
    );
    assert_eq!(
        handle.get_session_busy_state("agent:main:main"),
        (false, 0),
        "两轮全部收尾后 busy/队列必须归零（A 失败不得留下脏状态）"
    );
}

/// Box 包装（Arc 内部状态 + clone 进 provider 对象）。
struct FailThenOkProviderHandle(std::sync::Arc<FailThenOkProvider>);

#[async_trait::async_trait]
impl LlmProvider for FailThenOkProviderHandle {
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

// ---------------------------------------------------------------------------
// 场景 ③ 集成层：审批阻塞中取消 turn（security 面）
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
mod security_integration {
    use super::*;
    use nemesis_security::auditor::{ApprovalContext, ApprovalManager, ApprovalVerdict};

    /// 全关规则层 + default_action="ask" 的插件（审批是唯一活动闸，
    /// 未注册操作类型 fail-closed CRITICAL → RequireApproval）。
    fn ask_plugin() -> std::sync::Arc<nemesis_security::pipeline::SecurityPlugin> {
        std::sync::Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
            nemesis_security::pipeline::SecurityPluginConfig {
                enabled: true,
                command_guard_enabled: false,
                injection_enabled: false,
                credential_enabled: false,
                dlp_enabled: false,
                ssrf_enabled: false,
                default_action: "ask".to_string(),
                ..Default::default()
            },
        ))
    }

    /// 阻塞型审批管理器：进入即 notify + 挂起等裁决通道（复刻
    /// WebApprovalManager 的等待形态）；drain 送来的拒绝裁决让它立刻返回。
    struct BlockingApprovalManager {
        entered: std::sync::Arc<tokio::sync::Notify>,
        gate_rx: std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<ApprovalVerdict>>>,
        seen_request: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    }

    impl ApprovalManager for BlockingApprovalManager {
        fn is_running(&self) -> bool {
            true
        }
        fn request_approval_sync(
            &self,
            _request_id: &str,
            _operation: &str,
            _target: &str,
            _risk_level: &str,
            _reason: &str,
            _timeout_secs: u64,
        ) -> Result<ApprovalVerdict, String> {
            Ok(ApprovalVerdict {
                approved: false,
                note: Some("fallback arm should not be hit".to_string()),
            })
        }
        fn request_approval_sync_ctx(
            &self,
            request_id: &str,
            _operation: &str,
            _target: &str,
            _risk_level: &str,
            _reason: &str,
            _timeout_secs: u64,
            _ctx: &ApprovalContext,
        ) -> Result<ApprovalVerdict, String> {
            *self.seen_request.lock().unwrap() = Some(request_id.to_string());
            let mut rx = self
                .gate_rx
                .lock()
                .unwrap()
                .take()
                .expect("only one approval expected per test");
            self.entered.notify_one();
            let verdict = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move { rx.recv().await })
            });
            match verdict {
                Some(v) => Ok(v),
                None => Ok(ApprovalVerdict {
                    approved: false,
                    note: Some("gate dropped".to_string()),
                }),
            }
        }
    }

    /// 模拟 WebApprovalManager drain 语义的 responder：resolve_orphans_for_session
    /// 记录调用 + 向阻塞中的裁决等待送**拒绝**（清理绝不产生批准）。
    struct DenyForwardingResponder {
        gate_tx: tokio::sync::mpsc::Sender<ApprovalVerdict>,
        drains: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
        responds: std::sync::Arc<std::sync::Mutex<Vec<(String, bool)>>>,
    }

    impl nemesis_types::agent::ApprovalResponder for DenyForwardingResponder {
        fn respond(
            &self,
            request_id: &str,
            approved: bool,
            _always: bool,
            _note: Option<String>,
        ) -> Result<bool, String> {
            self.responds
                .lock()
                .unwrap()
                .push((request_id.to_string(), approved));
            Ok(true)
        }
        fn pending(&self) -> Vec<serde_json::Value> {
            Vec::new()
        }
        fn resolve_orphans_for_session(&self, session_key: &str, reason: &str) -> usize {
            self.drains
                .lock()
                .unwrap()
                .push((session_key.to_string(), reason.to_string()));
            let _ = self.gate_tx.try_send(ApprovalVerdict {
                approved: false,
                note: Some("turn cancelled — approval card drained".to_string()),
            });
            1
        }
    }

    /// 计数工具：审批拒绝后必须保持 0（迟到批准不得让它跑起来）。
    struct CountingTool {
        count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Tool for CountingTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("ran".to_string())
        }
    }

    struct OneToolCallProvider;

    #[async_trait::async_trait]
    impl LlmProvider for OneToolCallProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Ok(tool_resp(vec![one_call("c1", "cl_counter_tool")]))
        }
    }

    /// 全链回归锁：审批卡阻塞等待 → cancel_session → 卡以拒绝收场 →
    /// 工具不执行 → 终态「已取消」→ 迟到批准落空。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_during_pending_approval_denies_card_and_tool_never_runs() {
        let _logs = capture_logs();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
        let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
        let plugin = ask_plugin();
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let seen_request = std::sync::Arc::new(std::sync::Mutex::new(None));
        let (gate_tx, gate_rx) = tokio::sync::mpsc::channel::<ApprovalVerdict>(1);
        plugin
            .auditor()
            .set_approval_manager(std::sync::Arc::new(BlockingApprovalManager {
                entered: entered.clone(),
                gate_rx: std::sync::Mutex::new(Some(gate_rx)),
                seen_request: seen_request.clone(),
            }));

        let drains = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let responds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let responder = std::sync::Arc::new(DenyForwardingResponder {
            gate_tx,
            drains: drains.clone(),
            responds: responds.clone(),
        });

        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut agent_loop = AgentLoop::new_bus(
            Box::new(OneToolCallProvider),
            cfg(),
            out_tx,
            ConcurrentMode::Reject,
            8,
            0,
        );
        agent_loop.set_security_plugin(plugin);
        agent_loop.set_approval_responder(responder.clone());
        agent_loop.register_tool(
            "cl_counter_tool".to_string(),
            Box::new(CountingTool {
                count: count.clone(),
            }),
        );
        let handle = std::sync::Arc::new(agent_loop);
        let runner = tokio::spawn(handle.clone().run_bus_arc(in_rx));

        in_tx
            .send(plain("cancel while approval pending"))
            .await
            .unwrap();
        // 确定性入口：审批管理器已进入阻塞等待（token 必已创建）。
        tokio::time::timeout(std::time::Duration::from_secs(3), entered.notified())
            .await
            .expect("approval manager entered blocking wait");

        // 取消 → drain → 拒绝裁决送达 → 阻塞醒来 → 工具不执行。
        // 取消键 = 路由裁决键（bus 默认解析器改写为 agent:main:main），
        // 与 create_cancel_token 同一裁决（admission.rs resolve 分支）。
        assert!(
            handle.cancel_session("agent:main:main"),
            "cancel token existed and was cancelled"
        );
        let out = tokio::time::timeout(std::time::Duration::from_secs(3), out_rx.recv())
            .await
            .expect("cancelled done published in time")
            .expect("outbound");
        assert!(
            out.content.contains("已取消"),
            "取消必须收场 turn（拒绝的审批卡不得让 turn 继续跑完），got: {}",
            out.content
        );
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "审批被 drain 拒绝后工具体不得执行"
        );
        assert_eq!(
            drains.lock().unwrap().clone(),
            vec![("agent:main:main".to_string(), "turn cancelled".to_string())],
            "cancel_session 必须定向 drain 该会话的审批卡（键=路由裁决键）"
        );

        // 迟到批准：落 responder 台账，但工具早已不可能执行（幂等记账落空）。
        let req_id = seen_request
            .lock()
            .unwrap()
            .clone()
            .expect("request id captured by blocking manager");
        responder.respond(&req_id, true, false, None).unwrap();
        assert_eq!(
            responds.lock().unwrap().clone(),
            vec![(req_id, true)],
            "迟到批准到达 responder（模拟 dashboard 已点击）"
        );
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "迟到批准不得产生执行效果（副作用不得发生在取消之后）"
        );
        // 收口：drop in_tx → pump 的 recv() 返回 None → 泵退出（与 Queue
        // 测试同款；不 drop 则 runner.await 无限等泵——泵 parked 在存活的
        // recv() 上永不结束）。
        drop(in_tx);
        let _ = runner.await;
    }
}

// ---------------------------------------------------------------------------
// 场景 ③ 单元层 provider（无消息 ever）
// ---------------------------------------------------------------------------

struct NoopProvider;

#[async_trait::async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(resp("noop"))
    }
}
