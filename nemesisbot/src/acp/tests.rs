//! acp.rs 测试 —— 纯函数映射层 + duplex 全流程协议测试（假驱动，无 LLM）。
//!
//! transport 泛化（serve 收 AsyncRead/AsyncWrite）+ SessionFactory 注入
//! 让协议层不需要真实 agent 装配即可全流程验证：握手 / -32700 / -32601 /
//! 事件流顺序 / 取消 / permission 往返。AcpApprovalManager 的阻塞桥用
//! multi_thread flavor（block_in_place 在 current_thread runtime 会 panic）。
//!
//! 生产装配用例（RealSessionFactory / LoopDriver）持 GLOBAL_STATE_LOCK 跨
//! await（credentials 全局路径 / vault 解析器 / path manager 单例）——
//! 同 nemesisbot/src/tests.rs 顶部先例，tests 域统一豁免该 lint。
#![allow(clippy::await_holding_lock)]

use super::*;
use tokio::io::{BufReader, Lines};

// ---------------------------------------------------------------------------
// 纯函数映射
// ---------------------------------------------------------------------------

#[test]
fn tool_kind_mapping_conservative() {
    assert_eq!(map_tool_kind("exec"), "execute");
    assert_eq!(map_tool_kind("async_shell"), "execute");
    assert_eq!(map_tool_kind("read_file"), "read");
    assert_eq!(map_tool_kind("grep"), "read");
    assert_eq!(map_tool_kind("write_file"), "edit");
    assert_eq!(map_tool_kind("edit_file"), "edit");
    assert_eq!(map_tool_kind("web_fetch"), "fetch");
    assert_eq!(map_tool_kind("todowrite"), "think");
    // 认不出的一律 other（客户端按 other 渲染，不会错）。
    assert_eq!(map_tool_kind("mcp_some_unknown_tool"), "other");
    assert_eq!(map_tool_kind(""), "other");
}

#[test]
fn content_blocks_text_join_and_resource_link_degrade() {
    let blocks = vec![
        json!({"type": "text", "text": "看看这个文件"}),
        json!({"type": "resource_link", "uri": "file:///tmp/a.rs", "title": "a.rs"}),
        json!({"type": "text", "text": "再看看"}),
    ];
    let t = content_blocks_to_text(&blocks).unwrap();
    assert_eq!(
        t,
        "看看这个文件\n[引用资源] file:///tmp/a.rs (a.rs)\n再看看"
    );

    // 无 title 用 name；都无就只给 uri。
    let t2 = content_blocks_to_text(&[json!({"type": "resource_link", "uri": "file:///b.txt"})])
        .unwrap();
    assert_eq!(t2, "[引用资源] file:///b.txt");
}

#[test]
fn content_blocks_reject_unadvertised_and_malformed() {
    // image/audio/resource 未宣告（promptCapabilities 全 false）→ 客户端违约。
    for ty in ["image", "audio", "resource"] {
        let err = content_blocks_to_text(&[json!({"type": ty})]).unwrap_err();
        assert!(err.contains(ty), "err should name the block type: {err}");
    }
    assert!(content_blocks_to_text(&[json!({"text": "no type"})]).is_err());
    assert_eq!(content_blocks_to_text(&[]).unwrap_err(), "empty prompt");
}

#[test]
fn validate_cwd_rules() {
    let err = validate_cwd("relative/path").unwrap_err();
    assert!(err.contains("absolute"), "got: {err}");
    let missing = std::env::temp_dir().join("acp-nonexistent-check-tmp");
    let _ = std::fs::remove_dir_all(&missing);
    assert!(validate_cwd(missing.to_str().unwrap()).is_err());
    let dir = tempfile::tempdir().unwrap();
    validate_cwd(dir.path().to_str().unwrap()).unwrap();
}

#[test]
fn stop_reason_and_final_text_mapping() {
    assert_eq!(TurnOutcome::Completed("m".into()).stop_reason(), "end_turn");
    assert_eq!(TurnOutcome::Failed("e".into()).stop_reason(), "refusal");
    assert_eq!(TurnOutcome::Cancelled.stop_reason(), "cancelled");
    assert_eq!(TurnOutcome::Completed("m".into()).final_text(), Some("m"));
    assert_eq!(TurnOutcome::Failed("e".into()).final_text(), Some("e"));
    assert_eq!(TurnOutcome::Cancelled.final_text(), None);
}

#[test]
fn tool_update_payloads_shape() {
    let s = tool_started_update("call-1", "exec", "ls -la");
    assert_eq!(s["sessionUpdate"], "tool_call");
    assert_eq!(s["toolCallId"], "call-1");
    assert_eq!(s["title"], "exec");
    assert_eq!(s["kind"], "execute");
    assert_eq!(s["status"], "in_progress");
    assert_eq!(s["rawInput"], "ls -la");

    let f = tool_finished_update("call-1", true, "ok output");
    assert_eq!(f["sessionUpdate"], "tool_call_update");
    assert_eq!(f["status"], "completed");
    assert_eq!(f["rawOutput"], "ok output");
    let f2 = tool_finished_update("call-2", false, "Tool error: x");
    assert_eq!(f2["status"], "failed");
}

#[test]
fn permission_outcome_parsing() {
    let sel_allow = json!({"outcome": {"outcome": "selected", "optionId": "allow_once"}});
    assert_eq!(
        parse_permission_outcome(&sel_allow),
        Some(PermissionOutcome::Allowed)
    );
    // lenient：客户端硬回 always 变体也按语义收下
    let sel_always = json!({"outcome": {"outcome": "selected", "optionId": "allow_always"}});
    assert_eq!(
        parse_permission_outcome(&sel_always),
        Some(PermissionOutcome::Allowed)
    );
    let sel_reject = json!({"outcome": {"outcome": "selected", "optionId": "reject_once"}});
    assert_eq!(
        parse_permission_outcome(&sel_reject),
        Some(PermissionOutcome::Denied)
    );
    let cancelled = json!({"outcome": {"outcome": "cancelled"}});
    assert_eq!(
        parse_permission_outcome(&cancelled),
        Some(PermissionOutcome::Denied)
    );
    // 不可解析 → None（调用方语义：失败关闭按拒绝）。
    assert_eq!(parse_permission_outcome(&json!({"garbage": 1})), None);
    assert_eq!(
        parse_permission_outcome(&json!({"outcome": {"outcome": "selected", "optionId": "???"}})),
        None
    );
}

// ---------------------------------------------------------------------------
// duplex 协议全流程（假驱动）
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum FakeAction {
    Emit(Value),
    /// gate.request(...) 等裁决，outcome 记入共享 perm_log。
    Perm(&'static str),
    WaitCancel,
    Finish(TurnOutcome),
}

#[derive(Clone)]
struct FakeDriver {
    actions: Vec<FakeAction>,
    gate: Arc<PermissionGate>,
    perm_log: Arc<Mutex<Vec<PermissionOutcome>>>,
}

impl PromptDriver for FakeDriver {
    fn prompt<'a>(
        &'a self,
        _text: String,
        sink: UpdateSink,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = TurnOutcome> + Send + 'a>> {
        Box::pin(async move {
            for a in &self.actions {
                match a {
                    FakeAction::Emit(v) => {
                        sink.send(v.clone()).await.expect("update sink closed");
                    }
                    FakeAction::Perm(tool) => {
                        let o = self
                            .gate
                            .request(
                                tool,
                                "target-x",
                                "HIGH",
                                "test reason",
                                Duration::from_secs(5),
                            )
                            .await;
                        self.perm_log.lock().unwrap().push(o);
                    }
                    FakeAction::WaitCancel => cancel.cancelled().await,
                    FakeAction::Finish(o) => return o.clone(),
                }
            }
            TurnOutcome::Completed("fake done".to_string())
        })
    }
}

#[derive(Clone)]
struct FakeFactory {
    script: Vec<FakeAction>,
    created: Arc<Mutex<Vec<(String, Arc<PermissionGate>)>>>,
    perm_log: Arc<Mutex<Vec<PermissionOutcome>>>,
}

impl SessionFactory for FakeFactory {
    fn create<'a>(
        &'a self,
        _cwd: PathBuf,
        session_id: String,
        gate: Arc<PermissionGate>,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn PromptDriver>, String>> + Send + 'a>> {
        Box::pin(async move {
            self.created
                .lock()
                .unwrap()
                .push((session_id.clone(), Arc::clone(&gate)));
            Ok(Box::new(FakeDriver {
                actions: self.script.clone(),
                gate,
                perm_log: Arc::clone(&self.perm_log),
            }) as Box<dyn PromptDriver>)
        })
    }
}

struct TestClient {
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    lines: Lines<BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
}

impl TestClient {
    fn new(stream: tokio::io::DuplexStream) -> Self {
        // tokio::io::split 返回 (ReadHalf, WriteHalf)。
        let (read, write) = tokio::io::split(stream);
        Self {
            write,
            lines: BufReader::new(read).lines(),
        }
    }

    async fn send(&mut self, v: &Value) {
        let mut line = serde_json::to_string(v).unwrap();
        line.push('\n');
        self.write.write_all(line.as_bytes()).await.unwrap();
        self.write.flush().await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
            .await
            .expect("timeout waiting for frame")
            .expect("server side closed unexpectedly")
            .expect("read line failed");
        serde_json::from_str(&line).expect("frame not JSON")
    }

    /// 宽预算版 recv：LLM 恢复环重试含退避（秒级），prompt 结局帧会晚于
    /// 默认 5s 到达（LoopDriver 生产装配用例用）。
    async fn recv_within(&mut self, budget: Duration) -> Value {
        let line = tokio::time::timeout(budget, self.lines.next_line())
            .await
            .expect("timeout waiting for frame")
            .expect("server side closed unexpectedly")
            .expect("read line failed");
        serde_json::from_str(&line).expect("frame not JSON")
    }
}

fn init_request(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "initialize",
           "params": {"protocolVersion": 1, "clientCapabilities": {}}})
}

/// 起一个 duplex server（假工厂），返回客户端 + 共享记录。
async fn start_server(
    script: Vec<FakeAction>,
) -> (
    TestClient,
    Arc<Mutex<Vec<(String, Arc<PermissionGate>)>>>,
    Arc<Mutex<Vec<PermissionOutcome>>>,
) {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (sr, sw) = tokio::io::split(server_side);
    let created = Arc::new(Mutex::new(Vec::new()));
    let perm_log = Arc::new(Mutex::new(Vec::new()));
    let factory = Arc::new(FakeFactory {
        script,
        created: Arc::clone(&created),
        perm_log: Arc::clone(&perm_log),
    });
    tokio::spawn(serve(sr, sw, factory, "test-1.0".to_string()));
    (TestClient::new(client_side), created, perm_log)
}

#[tokio::test]
async fn parse_error_yields_minus_32700() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.write.write_all(b"this is not json\n").await.unwrap();
    c.write.flush().await.unwrap();
    let frame = c.recv().await;
    assert_eq!(frame["error"]["code"], -32700);
}

#[tokio::test]
async fn unknown_method_request_yields_minus_32601_and_notification_stays_silent() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.send(&json!({"jsonrpc": "2.0", "id": 7, "method": "foo/bar", "params": {}}))
        .await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 7);
    assert_eq!(frame["error"]["code"], -32601);
    assert!(
        frame["error"]["message"]
            .as_str()
            .unwrap()
            .contains("foo/bar")
    );

    // 未知通知必须静默（无 id 不回包）：紧随 initialize，下一帧必须是
    // initialize 的响应——若通知被回包会插队到它前面。
    c.send(&json!({"jsonrpc": "2.0", "method": "whatever/xxx", "params": {}}))
        .await;
    c.send(&init_request(1)).await;
    let frame = c.recv().await;
    assert_eq!(
        frame["id"], 1,
        "unknown notification must not produce a frame"
    );
    assert!(frame.get("result").is_some());
}

#[tokio::test]
async fn initialize_handshake_shape() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.send(&init_request(1)).await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 1);
    let r = &frame["result"];
    assert_eq!(r["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(r["agentInfo"]["name"], "nemesisbot");
    assert_eq!(r["agentCapabilities"]["loadSession"], false);
    assert_eq!(r["agentCapabilities"]["promptCapabilities"]["image"], false);
    assert_eq!(r["agentCapabilities"]["mcpCapabilities"]["http"], false);
    assert_eq!(r["authMethods"], json!([]));
}

#[tokio::test]
async fn session_new_rejects_bad_cwd() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;

    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "session/new",
                   "params": {"cwd": "relative/path"}}))
        .await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 1);
    assert_eq!(frame["error"]["code"], -32602);

    let missing = std::env::temp_dir().join("acp-nonexistent-session-new");
    let _ = std::fs::remove_dir_all(&missing);
    c.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
                   "params": {"cwd": missing.to_string_lossy()}}))
        .await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 2);
    assert_eq!(frame["error"]["code"], -32602);
}

/// 全流程：initialize → session/new（真 tempdir cwd）→ prompt 事件流 →
/// end_turn。事件顺序钉死：tool_call update → 最终 chunk → 响应。
#[tokio::test]
async fn full_turn_streams_updates_then_end_turn() {
    let script = vec![
        FakeAction::Emit(tool_started_update("call-1", "exec", "ls -la")),
        FakeAction::Finish(TurnOutcome::Completed("done text".to_string())),
    ];
    let (mut c, created, _) = start_server(script).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;

    let dir = tempfile::tempdir().unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "session/new",
                   "params": {"cwd": dir.path().to_string_lossy()}}))
        .await;
    let frame = c.recv().await;
    let sid = frame["result"]["sessionId"].as_str().unwrap().to_string();
    assert!(!sid.is_empty());
    // 工厂收到了 cwd + gate（装配 seam 打通）。显式块作用域收掉 guard
    // （clippy await_holding_lock 不认手动 drop）。
    {
        let created = created.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].0, sid);
    }

    c.send(
        &json!({"jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                   "params": {"sessionId": sid, "prompt": [
                       {"type": "text", "text": "跑个命令"}]}}),
    )
    .await;
    // 帧 1：工具 update（sessionId 盖章）。
    let frame = c.recv().await;
    assert_eq!(frame["method"], "session/update");
    assert_eq!(frame["params"]["sessionId"], sid.as_str());
    assert_eq!(frame["params"]["update"]["sessionUpdate"], "tool_call");
    assert_eq!(frame["params"]["update"]["toolCallId"], "call-1");
    assert_eq!(frame["params"]["update"]["status"], "in_progress");
    // 帧 2：最终文本 chunk。
    let frame = c.recv().await;
    assert_eq!(
        frame["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    assert_eq!(frame["params"]["update"]["content"]["text"], "done text");
    // 帧 3：响应。
    let frame = c.recv().await;
    assert_eq!(frame["id"], 2);
    assert_eq!(frame["result"]["stopReason"], "end_turn");
}

#[tokio::test]
async fn failed_turn_maps_refusal_with_error_text() {
    let script = vec![FakeAction::Finish(TurnOutcome::Failed("boom".to_string()))];
    let (mut c, _, _) = start_server(script).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;
    let dir = tempfile::tempdir().unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "session/new",
                   "params": {"cwd": dir.path().to_string_lossy()}}))
        .await;
    let sid = c.recv().await["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    c.send(
        &json!({"jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                   "params": {"sessionId": sid, "prompt": [{"type": "text", "text": "x"}]}}),
    )
    .await;
    let frame = c.recv().await;
    assert_eq!(frame["params"]["update"]["content"]["text"], "boom");
    let frame = c.recv().await;
    assert_eq!(frame["result"]["stopReason"], "refusal");
}

#[tokio::test]
async fn cancel_mid_turn_returns_cancelled() {
    let script = vec![
        FakeAction::Emit(tool_started_update("call-1", "exec", "sleep")),
        FakeAction::WaitCancel,
        // 真驱动语义（LoopDriver）：取消后在飞轮尽快返回 Cancelled（不
        // 把半截产物当完成文本）。
        FakeAction::Finish(TurnOutcome::Cancelled),
    ];
    let (mut c, _, _) = start_server(script).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;
    let dir = tempfile::tempdir().unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "session/new",
                   "params": {"cwd": dir.path().to_string_lossy()}}))
        .await;
    let sid = c.recv().await["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    c.send(
        &json!({"jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                   "params": {"sessionId": sid, "prompt": [{"type": "text", "text": "x"}]}}),
    )
    .await;
    // 先收到在飞 update，再发 cancel（通知，无 id）。
    let frame = c.recv().await;
    assert_eq!(frame["method"], "session/update");
    c.send(&json!({"jsonrpc": "2.0", "method": "session/cancel",
                   "params": {"sessionId": sid}}))
        .await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 2);
    assert_eq!(frame["result"]["stopReason"], "cancelled");
}

#[tokio::test]
async fn permission_roundtrip_allow_once() {
    let script = vec![
        FakeAction::Perm("exec"),
        FakeAction::Finish(TurnOutcome::Completed("ok".to_string())),
    ];
    let (mut c, _, perm_log) = start_server(script).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;
    let dir = tempfile::tempdir().unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "session/new",
                   "params": {"cwd": dir.path().to_string_lossy()}}))
        .await;
    let sid = c.recv().await["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    c.send(
        &json!({"jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                   "params": {"sessionId": sid, "prompt": [{"type": "text", "text": "x"}]}}),
    )
    .await;
    // 收到 permission 请求（REQUEST：有 id + options）。
    let frame = c.recv().await;
    assert_eq!(frame["method"], "session/request_permission");
    let perm_id = frame["id"].as_str().unwrap().to_string();
    assert!(perm_id.starts_with("perm-"), "got: {perm_id}");
    assert_eq!(frame["params"]["sessionId"], sid.as_str());
    assert_eq!(frame["params"]["toolCall"]["title"], "exec");
    let options = frame["params"]["options"].as_array().unwrap();
    assert_eq!(
        options.len(),
        2,
        "v1 只供 allow_once/reject_once（无 always 规则表背书）"
    );
    // 回 allow_once → 驱动继续 → end_turn。
    c.send(&json!({"jsonrpc": "2.0", "id": perm_id,
                   "result": {"outcome": {"outcome": "selected", "optionId": "allow_once"}}}))
        .await;
    let frame = c.recv().await;
    assert_eq!(frame["params"]["update"]["content"]["text"], "ok");
    let frame = c.recv().await;
    assert_eq!(frame["result"]["stopReason"], "end_turn");
    assert_eq!(*perm_log.lock().unwrap(), vec![PermissionOutcome::Allowed]);
}

#[tokio::test]
async fn prompt_to_unknown_session_errors() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;
    c.send(
        &json!({"jsonrpc": "2.0", "id": 9, "method": "session/prompt",
                   "params": {"sessionId": "nope", "prompt": [{"type": "text", "text": "x"}]}}),
    )
    .await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 9);
    assert_eq!(frame["error"]["code"], -32602);
    assert!(frame["error"]["message"].as_str().unwrap().contains("nope"));
}

// ---------------------------------------------------------------------------
// AcpApprovalManager（M7 → ACP 桥；multi_thread：block_in_place 依赖）
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread")]
async fn approval_manager_maps_verdicts_and_fails_closed() {
    use nemesis_security::auditor::ApprovalManager;

    let (tx, _rx) = mpsc::channel::<Value>(16);
    let pending = Arc::new(PendingPermissions::new());
    let gate = Arc::new(PermissionGate::new(
        "s1".to_string(),
        tx,
        Arc::clone(&pending),
    ));
    let mgr = AcpApprovalManager { gate };

    // 批准路径：后台 100ms 后回 allow_once。
    let p2 = Arc::clone(&pending);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(p2.resolve(
            "perm-0",
            Some(&json!({"outcome": {"outcome": "selected", "optionId": "allow_once"}}))
        ));
    });
    let v = mgr
        .request_approval_sync("r1", "exec", "t", "HIGH", "why", 5)
        .unwrap();
    assert!(v.approved);
    assert!(v.note.is_none());

    // 超时路径：无人 resolve → 失败关闭（denied + note）。
    let v2 = mgr
        .request_approval_sync("r2", "exec", "t", "HIGH", "why", 1)
        .unwrap();
    assert!(!v2.approved);
    assert!(v2.note.is_some());
}

#[tokio::test]
async fn gate_denies_when_outbound_closed() {
    let (tx, rx) = mpsc::channel::<Value>(16);
    drop(rx); // 客户端断开
    let gate = PermissionGate::new("s1".to_string(), tx, Arc::new(PendingPermissions::new()));
    let o = gate
        .request("exec", "t", "HIGH", "why", Duration::from_secs(1))
        .await;
    assert_eq!(o, PermissionOutcome::Denied);
}

#[tokio::test]
async fn pending_resolve_unknown_id_is_honest_false() {
    let pending = PendingPermissions::new();
    assert!(!pending.resolve(
        "ghost",
        Some(&json!({"outcome": {"outcome": "selected", "optionId": "allow_once"}}))
    ));
    // 迟到 resolve（已消费）同样 false。register 只返回接收端（发送端
    // 存在登记表里）；弃掉接收端 → send 失败 → false。
    let rx = pending.register("p-1");
    drop(rx);
    assert!(!pending.resolve(
        "p-1",
        Some(&json!({"outcome": {"outcome": "selected", "optionId": "allow_once"}}))
    ));
}

// ---------------------------------------------------------------------------
// Coverage 追加（2026-09-24）：版本协商失配 / mcpServers 忽略 / 工厂失败 /
// prompt 缺参与空块 / cancel 静默 / 空行跳过 / EOF 收尾 / 生产装配
// （RealSessionFactory + LoopDriver）。
// ---------------------------------------------------------------------------

/// permission 响应载荷再兜一层：outcome 存在但不是对象 / 内层 outcome 缺失
/// （`_ => None` 兜底臂——调用方语义：不可解析 = 失败关闭按拒绝）。
#[test]
fn permission_outcome_non_object_and_missing_inner_are_none() {
    assert_eq!(parse_permission_outcome(&json!({"outcome": 42})), None);
    assert_eq!(parse_permission_outcome(&json!({"outcome": {}})), None);
}

/// 版本协商：client 报不支持的版本 → 回我们最新（PROTOCOL_VERSION），
/// 客户端不支持则自行断开（spec 语义）。
#[tokio::test]
async fn initialize_version_mismatch_negotiates_server_latest() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.send(&json!({"jsonrpc": "2.0", "id": 3, "method": "initialize",
                   "params": {"protocolVersion": 99}}))
        .await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 3);
    assert_eq!(
        frame["result"]["protocolVersion"], PROTOCOL_VERSION,
        "client 99 ≠ ours → negotiated 回我们最新"
    );
}

/// initialize 作通知（无 id）发送：静默不回包（643-645 else 臂）——
/// 下一帧必须是后续请求的响应，若通知被回包会插队。
#[tokio::test]
async fn initialize_notification_without_id_is_silent() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.send(&json!({"jsonrpc": "2.0", "method": "initialize",
                   "params": {"protocolVersion": 1}}))
        .await;
    c.send(&init_request(5)).await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 5, "initialize 通知不得产生任何帧");
}

/// session/new 带 mcpServers：v1 诚实边界——忽略 + stderr warn，
/// session 照常创建（不因参数不支持而失败）。
#[tokio::test]
async fn session_new_ignores_mcp_servers_with_warning() {
    let (mut c, created, _) = start_server(vec![]).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;
    let dir = tempfile::tempdir().unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "session/new",
                   "params": {"cwd": dir.path().to_string_lossy(),
                              "mcpServers": [{"command": "a"}, {"command": "b"}]}}))
        .await;
    let frame = c.recv().await;
    let sid = frame["result"]["sessionId"].as_str().unwrap().to_string();
    assert!(!sid.is_empty());
    assert_eq!(created.lock().unwrap().len(), 1, "session 照常装配");
}

/// 恒败工厂：session 装配失败 → -32603（internal error），错误文本透传。
struct ErrFactory;

impl SessionFactory for ErrFactory {
    fn create<'a>(
        &'a self,
        _cwd: PathBuf,
        _session_id: String,
        _gate: Arc<PermissionGate>,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn PromptDriver>, String>> + Send + 'a>> {
        Box::pin(async { Err("boom".to_string()) })
    }
}

/// 自定义工厂版 start_server（ErrFactory / RealSessionFactory 用）。
async fn start_with_factory(factory: Arc<dyn SessionFactory>) -> TestClient {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (sr, sw) = tokio::io::split(server_side);
    tokio::spawn(serve(sr, sw, factory, "test-1.0".to_string()));
    TestClient::new(client_side)
}

#[tokio::test]
async fn session_new_factory_failure_maps_to_minus_32603() {
    let mut c = start_with_factory(Arc::new(ErrFactory)).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;
    let dir = tempfile::tempdir().unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "session/new",
                   "params": {"cwd": dir.path().to_string_lossy()}}))
        .await;
    let frame = c.recv().await;
    assert_eq!(frame["error"]["code"], -32603);
    assert!(
        frame["error"]["message"].as_str().unwrap().contains("boom"),
        "工厂错误文本必须透传: {}",
        frame["error"]["message"]
    );
}

/// prompt 缺 sessionId → -32602 "missing sessionId"（531-534 else 臂）。
#[tokio::test]
async fn prompt_without_session_id_is_minus_32602() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;
    c.send(
        &json!({"jsonrpc": "2.0", "id": 4, "method": "session/prompt",
                   "params": {"prompt": [{"type": "text", "text": "x"}]}}),
    )
    .await;
    let frame = c.recv().await;
    assert_eq!(frame["error"]["code"], -32602);
    assert!(
        frame["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing sessionId")
    );
}

/// prompt 空块数组：session 有效但内容为空 → -32602 "empty prompt"
/// （content_blocks_to_text Err 臂 → 553-556 透传）。
#[tokio::test]
async fn prompt_with_empty_blocks_is_minus_32602() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;
    let dir = tempfile::tempdir().unwrap();
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "session/new",
                   "params": {"cwd": dir.path().to_string_lossy()}}))
        .await;
    let sid = c.recv().await["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    c.send(
        &json!({"jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                   "params": {"sessionId": sid, "prompt": []}}),
    )
    .await;
    let frame = c.recv().await;
    assert_eq!(frame["error"]["code"], -32602);
    assert_eq!(frame["error"]["message"], "empty prompt");
}

/// session/cancel 缺 sessionId / 未知会话：双双静默（606-618 早退臂）——
/// 下一帧必须是后续请求的响应。
#[tokio::test]
async fn cancel_without_or_unknown_session_is_silent() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.send(&json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {}}))
        .await;
    c.send(&json!({"jsonrpc": "2.0", "method": "session/cancel",
                   "params": {"sessionId": "ghost"}}))
        .await;
    c.send(&init_request(6)).await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 6, "cancel 缺参/未知会话都必须静默");
}

/// 空白行跳过（687-689）：不回 -32700、不断流，后续帧照常处理。
#[tokio::test]
async fn blank_lines_are_skipped() {
    let (mut c, _, _) = start_server(vec![]).await;
    c.write.write_all(b"\n   \n\t\n").await.unwrap();
    c.write.flush().await.unwrap();
    c.send(&init_request(8)).await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 8, "空行必须跳过（不得回 -32700 或断流）");
}

/// EOF 收尾（697-699 + 711-738）：客户端断开 → 读循环 EOF → serve 排干
/// 出站缓冲、writer task 退出 → 返回 Ok（协议层干净关闭，无错误上抛）。
#[tokio::test]
async fn eof_ends_server_cleanly_after_draining_output() {
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (sr, sw) = tokio::io::split(server_side);
    let factory = Arc::new(FakeFactory {
        script: vec![],
        created: Arc::new(Mutex::new(Vec::new())),
        perm_log: Arc::new(Mutex::new(Vec::new())),
    });
    let handle = tokio::spawn(serve(sr, sw, factory, "test-1.0".to_string()));
    let mut c = TestClient::new(client_side);
    c.send(&init_request(1)).await;
    let frame = c.recv().await;
    assert_eq!(frame["id"], 1);
    // 客户端整体 drop → duplex 关闭 → serve 应自然收尾。
    drop(c);
    let r = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("serve must end after EOF")
        .expect("serve task panicked");
    assert!(r.is_ok(), "serve must return Ok on clean EOF: {r:?}");
}

/// M1a hook 事件 → update 载荷映射（830-846）：ToolStarted/ToolFinished
/// 映射为 tool_call / tool_call_update，其余变体（ModeChanged 等未宣告
/// 形态）诚实跳过返回 None。
#[test]
fn map_hook_event_maps_tool_events_and_skips_rest() {
    let started = nemesis_types::agent::AgentEvent::ToolStarted {
        session_key: "acp:s1".into(),
        chat_id: "c1".into(),
        call_id: "call-9".into(),
        tool: "grep".into(),
        args_preview: "{}".into(),
    };
    let u = map_hook_event(&started).expect("ToolStarted 必须映射");
    assert_eq!(u["sessionUpdate"], "tool_call");
    assert_eq!(u["toolCallId"], "call-9");
    assert_eq!(u["kind"], "read", "grep → read");

    let finished = nemesis_types::agent::AgentEvent::ToolFinished {
        session_key: "acp:s1".into(),
        chat_id: "c1".into(),
        call_id: "call-9".into(),
        tool: "grep".into(),
        duration_ms: 12,
        ok: false,
        result_preview: "Tool error: x".into(),
    };
    let u2 = map_hook_event(&finished).expect("ToolFinished 必须映射");
    assert_eq!(u2["sessionUpdate"], "tool_call_update");
    assert_eq!(u2["status"], "failed");

    // 未宣告形态：TodoUpdated/ModeChanged/RoundText v1 不透。
    assert!(
        map_hook_event(&nemesis_types::agent::AgentEvent::ModeChanged {
            session_key: "s".into(),
            chat_id: "c".into(),
            mode: "plan".into(),
        })
        .is_none()
    );
}

/// 审批桥 is_running：gate 恒在线（编辑器即审批面）。
#[cfg(feature = "security")]
#[test]
fn approval_manager_is_running_reports_true() {
    use nemesis_security::auditor::ApprovalManager;
    let (tx, _rx) = mpsc::channel::<Value>(4);
    let gate = Arc::new(PermissionGate::new(
        "s1".to_string(),
        tx,
        Arc::new(PendingPermissions::new()),
    ));
    let mgr = AcpApprovalManager { gate };
    assert!(mgr.is_running());
}

/// RealSessionFactory：配置缺席 → fail loud（headless 不隐式 onboard，
/// 与 run.rs 同纪律）。纯路径检查，不触进程全局，无需锁。
#[tokio::test]
async fn real_factory_without_config_fails_loud() {
    let tmp = tempfile::TempDir::new().unwrap();
    let factory = RealSessionFactory {
        home: tmp.path().join("nohome"),
    };
    let (tx, _rx) = mpsc::channel::<Value>(4);
    let gate = Arc::new(PermissionGate::new(
        "s0".to_string(),
        tx,
        Arc::new(PendingPermissions::new()),
    ));
    let err = match factory
        .create(tmp.path().to_path_buf(), "sid".into(), gate)
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("配置缺席必须 Err（headless 不隐式 onboard）"),
    };
    assert!(err.contains("Configuration not found"), "got: {err}");
    assert!(err.contains("onboard"), "报错必须带补救指引: {err}");
}

/// 生产装配全流程（K1 同源）：config.json 在（同 agent_factory::tests 先例，
/// api_base 指向 127.0.0.1:9 连接拒绝、离线装配不发请求）→ session/new
/// 装配成功 → prompt 打到不可达 LLM → Error 结局映射 stopReason=refusal
/// （失败诚实呈现）。进程全局注入（credentials 路径 / vault 解析器 /
/// NEMESISBOT_HOME）→ 持 GLOBAL_STATE_LOCK 串行 + EnvHomeGuard 隔离。
#[cfg(windows)] // Windows-form test (EnvHomeGuard + process globals; Linux nightly excluded)
#[tokio::test]
async fn real_factory_assembles_session_and_unreachable_llm_maps_to_refusal() {
    let _lock = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "acp-model" } },
        "model_list": [{
            "model_name": "acp-model",
            "model": "testai/acp-model",
            "api_key": "test-key",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        }]
    });
    std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();
    let _env = crate::tests::EnvHomeGuard::point_at(&home);

    let factory = Arc::new(RealSessionFactory { home: home.clone() });
    let mut c = start_with_factory(factory).await;
    c.send(&init_request(0)).await;
    let _ = c.recv().await;
    c.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "session/new",
                   "params": {"cwd": tmp.path().to_string_lossy()}}))
        .await;
    let frame = c.recv().await;
    let sid = frame["result"]["sessionId"]
        .as_str()
        .expect("K1 装配必须离线成功（provider 构造不发请求）")
        .to_string();

    // prompt → LLM 不可达（127.0.0.1:9 连接拒绝）→ loop 侧恢复环重试耗尽
    // （含退避，实测 ~15s）→ Error 事件 → refusal + 错误文本收尾 chunk。
    c.send(
        &json!({"jsonrpc": "2.0", "id": 2, "method": "session/prompt",
                   "params": {"sessionId": sid, "prompt": [{"type": "text", "text": "hi"}]}}),
    )
    .await;
    let frame = loop {
        let f = c.recv_within(Duration::from_secs(115)).await;
        if f.get("id").is_some() {
            break f; // prompt 响应（此前可能有收尾 chunk 通知）
        }
        // update 通知：错误结局的收尾 chunk 必须先于响应到达。
        assert_eq!(f["method"], "session/update");
        assert_eq!(
            f["params"]["update"]["sessionUpdate"], "agent_message_chunk",
            "失败结局的收尾文本: {f}"
        );
    };
    assert_eq!(frame["id"], 2);
    assert_eq!(
        frame["result"]["stopReason"], "refusal",
        "LLM 失败必须诚实映射 refusal"
    );
}
