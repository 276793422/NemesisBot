use super::*;
use nemesis_agent::instance::AgentInstance;
use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
use nemesis_agent::session::SessionStore;
use nemesis_agent::types::{AgentConfig, AgentEvent, ConversationTurn, ToolCallInfo};
use nemesis_cluster::cluster_task::{ClusterTask, TaskSource, TaskStatus};

// -- is_async_done -------------------------------------------------------
//
// Detection now keys off the `__CLUSTER_ASYNC__` marker in tool messages
// rather than the user-facing wording, so the message template can change
// freely without breaking multi-hop chain call detection.

#[test]
fn test_is_async_done_true() {
    let convo = vec![
        make_turn("user", "请帮我联系 Alex", vec![]),
        make_turn(
            "assistant",
            "",
            vec![ToolCallInfo {
                id: "tc_1".to_string(),
                name: "cluster_rpc".to_string(),
                arguments: "{}".to_string(),
            }],
        ),
        make_turn(
            "tool",
            "Request accepted by node-X. Task ID: auto-abc | __CLUSTER_ASYNC__{\"task_id\":\"auto-abc\",\"target\":\"node-X\"}",
            vec![],
        ),
    ];
    assert!(is_async_done(&convo));
}

#[test]
fn test_is_async_done_false_normal_done() {
    let convo = vec![
        make_turn("user", "你好", vec![]),
        make_turn("assistant", "你好呀", vec![]),
        make_turn("tool", "some regular tool output with no marker", vec![]),
    ];
    assert!(!is_async_done(&convo));
}

#[test]
fn test_is_async_done_empty() {
    let convo: Vec<ConversationTurn> = vec![];
    assert!(!is_async_done(&convo));
}

// -- extract_async_info --------------------------------------------------

fn make_turn(role: &str, content: &str, tool_calls: Vec<ToolCallInfo>) -> ConversationTurn {
    ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls,
        tool_call_id: None,
        timestamp: "2026-06-04T00:00:00Z".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

#[test]
fn test_extract_async_info_json_marker() {
    let tool_call = ToolCallInfo {
        id: "tc_456".to_string(),
        name: "cluster_rpc".to_string(),
        arguments: "{}".to_string(),
    };
    let conversation = vec![
        make_turn("user", "hello", vec![]),
        make_turn("assistant", "calling tool", vec![tool_call]),
        make_turn(
            "tool",
            "__CLUSTER_ASYNC__{\"task_id\":\"child-123\"}",
            vec![],
        ),
    ];
    let result = extract_async_info(&conversation);
    assert_eq!(
        result,
        Some(("child-123".to_string(), "tc_456".to_string()))
    );
}

#[test]
fn test_extract_async_info_text_fallback() {
    let tool_call = ToolCallInfo {
        id: "tc_789".to_string(),
        name: "cluster_rpc".to_string(),
        arguments: "{}".to_string(),
    };
    let conversation = vec![
        make_turn("user", "hello", vec![]),
        make_turn("assistant", "calling tool", vec![tool_call]),
        make_turn("tool", "Request accepted. Task ID: child-xyz", vec![]),
    ];
    let result = extract_async_info(&conversation);
    assert_eq!(
        result,
        Some(("child-xyz".to_string(), "tc_789".to_string()))
    );
}

#[test]
fn test_extract_async_info_none() {
    let conversation = vec![
        make_turn("user", "hello", vec![]),
        make_turn("assistant", "no tools called", vec![]),
    ];
    assert!(extract_async_info(&conversation).is_none());
}

#[test]
fn test_extract_async_info_no_tool_call_id() {
    let conversation = vec![
        make_turn("user", "hello", vec![]),
        make_turn(
            "tool",
            "__CLUSTER_ASYNC__{\"task_id\":\"child-456\"}",
            vec![],
        ),
    ];
    assert!(extract_async_info(&conversation).is_none());
}

// -- extract_final_message -----------------------------------------------

#[test]
fn test_extract_final_message() {
    let events = vec![
        AgentEvent::Message("intermediate".to_string()),
        AgentEvent::ToolCall(vec![]),
        AgentEvent::Message("more work".to_string()),
        AgentEvent::Done("final answer".to_string()),
    ];
    assert_eq!(extract_final_message(&events), "final answer");
}

#[test]
fn test_extract_final_message_no_done() {
    let events = vec![
        AgentEvent::Message("thinking".to_string()),
        AgentEvent::Error("something broke".to_string()),
    ];
    assert_eq!(extract_final_message(&events), "");
}

#[test]
fn test_extract_final_message_returns_last_done() {
    let events = vec![
        AgentEvent::Done("first done".to_string()),
        AgentEvent::Done("last done".to_string()),
    ];
    assert_eq!(extract_final_message(&events), "last done");
}

// -- build_context -------------------------------------------------------

#[test]
fn test_build_context() {
    let task = ClusterTask {
        task_id: "task-001".to_string(),
        source: TaskSource {
            node_id: "node-b".to_string(),
            rpc_address: "192.168.1.10:9000".to_string(),
            session_key: "sess-abc".to_string(),
        },
        status: TaskStatus::Pending,
        content: "hello".to_string(),
        conversation: None,
        waiting_for_task_id: None,
        waiting_tool_call_id: None,
        callback_result: None,
    };
    let ctx = build_context(&task);
    assert_eq!(ctx.channel, "cluster");
    // chat_id 现在等于 session_key（稳定），不再拼 task_id
    assert_eq!(ctx.chat_id, "sess-abc");
    assert_eq!(ctx.user, "node-b");
    assert_eq!(ctx.session_key, "sess-abc");
    assert!(ctx.correlation_id.is_none());
}

// -- restore_session_history / persist_session_history -------------------

/// Minimal mock LLM provider so we can construct an AgentLoop without spinning
/// up the real provider stack. None of these tests actually call the LLM —
/// they only exercise the SessionStore glue in helpers.
struct NullLlmProvider;

#[async_trait::async_trait]
impl LlmProvider for NullLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "null".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// Build an AgentLoop with an in-memory SessionStore for testing.
fn make_loop_with_session_store() -> (AgentLoop, std::sync::Arc<SessionStore>) {
    let mut agent_loop = AgentLoop::new(
        Box::new(NullLlmProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("test".to_string()),
            max_turns: 1,
            tools: vec![],
            ..Default::default()
        },
    );
    let store = std::sync::Arc::new(SessionStore::new_in_memory());
    agent_loop.set_session_store(store.clone());
    (agent_loop, store)
}

fn make_test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("test".to_string()),
        max_turns: 1,
        tools: vec![],
        ..Default::default()
    }
}

// -- degrade paths: missing SessionStore + save failure -------------------

/// Build an AgentLoop WITHOUT a SessionStore attached. Mirrors the production
/// path when `build_cluster_agent_loop` couldn't create the storage directory
/// (rare, but the code must degrade gracefully).
fn make_loop_without_session_store() -> AgentLoop {
    AgentLoop::new(
        Box::new(NullLlmProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("test".to_string()),
            max_turns: 1,
            tools: vec![],
            ..Default::default()
        },
    )
}

#[test]
fn test_restore_silent_when_no_session_store_attached() {
    // session_store() == None path: restore_session_history must return 0
    // and not panic. Same for persist_session_history (covered by next test).
    let agent_loop = make_loop_without_session_store();
    let instance = AgentInstance::new(make_test_config());

    let restored = restore_session_history(&agent_loop, &instance, "any-key");
    assert_eq!(restored, 0);
    assert_eq!(instance.get_history().len(), 1); // system prompt only
}

#[test]
fn test_persist_silent_when_no_session_store_attached() {
    let agent_loop = make_loop_without_session_store();
    let instance = AgentInstance::new(make_test_config());

    // Must not panic; must not modify the instance.
    persist_session_history(&agent_loop, &instance, "any-key");
    assert_eq!(instance.get_history().len(), 1);
}

#[test]
fn test_persist_save_failure_does_not_panic() {
    // Construct a disk-backed SessionStore, then trigger save() failure by
    // passing an invalid session key (".." sanitizes to "." which is rejected).
    // persist_session_history catches the Err and logs a warning instead of
    // propagating.
    let tmp = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(SessionStore::new_with_storage(tmp.path()));
    let mut agent_loop = AgentLoop::new(Box::new(NullLlmProvider), make_test_config());
    agent_loop.set_session_store(store.clone());

    let instance = AgentInstance::new(make_test_config());

    // Direct save("..") fails; verify the precondition holds.
    store.get_or_create("..");
    assert!(store.save("..").is_err());

    // persist_session_history with invalid key must swallow the error.
    persist_session_history(&agent_loop, &instance, "..");

    // Instance history is untouched (persist reads instance.get_history() to
    // write the store, but does not mutate the instance itself).
    assert_eq!(instance.get_history().len(), 1);
}

#[test]
fn test_restore_session_history_empty_store_returns_zero() {
    let (agent_loop, _store) = make_loop_with_session_store();
    let instance = AgentInstance::new(make_test_config());

    let restored = restore_session_history(&agent_loop, &instance, "nonexistent-key");
    assert_eq!(restored, 0);
    // Fresh instance has 1 system message (from config); restore adds nothing.
    assert_eq!(instance.get_history().len(), 1);
    assert_eq!(instance.get_history()[0].role, "system");
}

#[test]
fn test_persist_then_restore_roundtrip() {
    let (agent_loop, store) = make_loop_with_session_store();
    let instance = AgentInstance::new(make_test_config());
    instance.add_user_message("hello");
    instance.add_assistant_message("world", Vec::new(), None);
    // instance history = [system, user, assistant].

    // Post-refactor: persist writes the instance's FULL history (not just a
    // user/assistant pair), so the store stays coherent with covers_up_to.
    persist_session_history(&agent_loop, &instance, "sess-1");

    let msgs = store.get_history("sess-1");
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].role, "system");
    assert_eq!(msgs[1].role, "user");
    assert_eq!(msgs[1].content, "hello");
    assert_eq!(msgs[2].role, "assistant");
    assert_eq!(msgs[2].content, "world");

    // A fresh instance restores the same full history.
    let instance2 = AgentInstance::new(make_test_config());
    let restored = restore_session_history(&agent_loop, &instance2, "sess-1");
    assert_eq!(restored, 3);
    let history = instance2.get_history();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, "system");
    assert_eq!(history[1].content, "hello");
    assert_eq!(history[2].content, "world");
}

#[test]
fn test_different_session_keys_isolated() {
    let (agent_loop, store) = make_loop_with_session_store();

    let instance_a = AgentInstance::new(make_test_config());
    instance_a.add_user_message("hello-A");
    instance_a.add_assistant_message("world-A", Vec::new(), None);
    persist_session_history(&agent_loop, &instance_a, "sess-A");

    let instance_b = AgentInstance::new(make_test_config());
    instance_b.add_user_message("hello-B");
    instance_b.add_assistant_message("world-B", Vec::new(), None);
    persist_session_history(&agent_loop, &instance_b, "sess-B");

    // Each session key holds its own full history ([system, user, assistant]).
    assert_eq!(store.get_history("sess-A").len(), 3);
    assert_eq!(store.get_history("sess-B").len(), 3);

    // Restoring A must give A's messages, not B's.
    let instance_a2 = AgentInstance::new(make_test_config());
    let restored = restore_session_history(&agent_loop, &instance_a2, "sess-A");
    assert_eq!(restored, 3);
    let history = instance_a2.get_history();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, "system");
    assert_eq!(history[1].content, "hello-A");
    assert_eq!(history[2].content, "world-A");
}

#[test]
fn test_persist_grows_history_across_turns() {
    // Real cluster flow: each peer_chat restores the persisted history,
    // extends it with the new turn, and re-persists. Growth comes from
    // restore+extend (persist is a wholesale replace with the instance's full
    // history — it does not append on its own).
    let (agent_loop, store) = make_loop_with_session_store();

    // Turn 1.
    let inst1 = AgentInstance::new(make_test_config());
    inst1.add_user_message("msg-1");
    inst1.add_assistant_message("resp-1", Vec::new(), None);
    persist_session_history(&agent_loop, &inst1, "sess-multi");
    assert_eq!(store.get_history("sess-multi").len(), 3); // [sys, msg-1, resp-1]

    // Turn 2: restore, extend, persist.
    let inst2 = AgentInstance::new(make_test_config());
    restore_session_history(&agent_loop, &inst2, "sess-multi");
    inst2.add_user_message("msg-2");
    inst2.add_assistant_message("resp-2", Vec::new(), None);
    persist_session_history(&agent_loop, &inst2, "sess-multi");

    let msgs = store.get_history("sess-multi");
    assert_eq!(msgs.len(), 5); // [sys, msg-1, resp-1, msg-2, resp-2]
    assert_eq!(msgs[1].content, "msg-1");
    assert_eq!(msgs[2].content, "resp-1");
    assert_eq!(msgs[3].content, "msg-2");
    assert_eq!(msgs[4].content, "resp-2");
}

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：cluster_agent_loop 端到端。
//
// 直构 AgentLoop（脚本化 provider + 可注册的假 cluster_rpc 工具）+ 内存
// ClusterTaskList/ClusterWorkQueue + broadcast shutdown，覆盖主循环 select、
// execute_new_task（普通完成 / 异步挂起 / 标记不可解析→失败）、resume_task
// （回调后续行完成 / 二次异步）、handle_task_error、任务缺失跳过。
// rpc_client=None → send_task_callback 走无客户端跳过路径（无网络）。
// =========================================================================

mod loop_e2e {
    use super::*;
    use nemesis_agent::r#loop::{LlmMessage, LlmProvider, LlmResponse, Tool};
    use nemesis_agent::types::ChatOptions;
    use nemesis_agent::types::ToolDefinition;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 可脚本化 provider：第 1 次调用返回 cluster_rpc tool_call（若有），
    /// 之后返回固定 Done 文本。call_count 供断言。
    struct ScriptedProvider {
        first_tool_call: Option<ToolCallInfo>,
        final_text: String,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ScriptedProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<ChatOptions>,
            _tools: Vec<ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 && self.first_tool_call.is_some() {
                return Ok(LlmResponse {
                    content: String::new(),
                    tool_calls: vec![self.first_tool_call.clone().unwrap()],
                    finished: false,
                    reasoning_content: None,
                    usage: None,
                    raw_request_body: None,
                    raw_response_body: None,
                });
            }
            Ok(LlmResponse {
                content: self.final_text.clone(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    /// 假 cluster_rpc 工具：返回预设结果文本（用于制造 __CLUSTER_ASYNC__
    /// 标记的 tool 结果，无需真集群）。
    struct FakeClusterRpc {
        result: String,
    }

    #[async_trait::async_trait]
    impl Tool for FakeClusterRpc {
        async fn execute(
            &self,
            _args: &str,
            _context: &nemesis_agent::context::RequestContext,
        ) -> Result<String, String> {
            Ok(self.result.clone())
        }
    }

    fn base_config() -> AgentConfig {
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("cluster test".to_string()),
            max_turns: 4,
            tools: vec!["cluster_rpc".to_string()],
            ..Default::default()
        }
    }

    fn make_task(task_id: &str) -> ClusterTask {
        ClusterTask {
            task_id: task_id.to_string(),
            source: TaskSource {
                node_id: "node-b".to_string(),
                rpc_address: "127.0.0.1:9".to_string(),
                session_key: format!("sess-{task_id}"),
            },
            status: TaskStatus::Pending,
            content: "please do the thing".to_string(),
            conversation: None,
            waiting_for_task_id: None,
            waiting_tool_call_id: None,
            callback_result: None,
        }
    }

    /// 起一套 loop 基建（queue/task_list/shutdown + spawn 的 agent loop）。
    struct LoopRig {
        task_list: Arc<ClusterTaskList>,
        work_queue: Arc<ClusterWorkQueue>,
        shutdown_tx: tokio::sync::broadcast::Sender<()>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl LoopRig {
        async fn stop(self) {
            let _ = self.shutdown_tx.send(());
            let _ = self.handle.await;
        }
    }

    fn spawn_rig(agent_loop: AgentLoop, config: AgentConfig, data_dir: &std::path::Path) -> LoopRig {
        let task_list = Arc::new(ClusterTaskList::new(data_dir));
        let work_queue = Arc::new(ClusterWorkQueue::new(8));
        let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);
        let handle = tokio::spawn(cluster_agent_loop(
            Arc::new(agent_loop),
            config,
            work_queue.clone(),
            task_list.clone(),
            None, // rpc_client=None：send_task_callback 走无客户端跳过
            None, // 无 observer
            shutdown_rx,
        ));
        LoopRig { task_list, work_queue, shutdown_tx, handle }
    }

    async fn wait_until(deadline_ms: u64, f: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
        while !f() {
            assert!(
                std::time::Instant::now() < deadline,
                "condition not met within {deadline_ms}ms"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn new_task_runs_to_completion_and_is_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = ScriptedProvider {
            first_tool_call: None,
            final_text: "final-answer".to_string(),
            calls: AtomicUsize::new(0),
        };
        let agent_loop = AgentLoop::new(Box::new(provider), base_config());
        let rig = spawn_rig(agent_loop, base_config(), tmp.path());

        rig.task_list.create_task(make_task("t1"));
        rig.work_queue.submit("t1".to_string()).unwrap();

        // complete_task 会把任务从列表移除 → 等「出现过 Running 后被移除」。
        wait_until(10_000, || {
            matches!(
                rig.task_list.get_task("t1").map(|t| t.status),
                Some(TaskStatus::Running)
            ) || rig.task_list.get_task("t1").is_none()
        })
        .await;
        wait_until(10_000, || rig.task_list.get_task("t1").is_none()).await;

        rig.stop().await;
    }

    #[tokio::test]
    async fn async_task_round_trip_suspends_then_resumes_to_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = ScriptedProvider {
            first_tool_call: Some(ToolCallInfo {
                id: "tc_async".to_string(),
                name: "cluster_rpc".to_string(),
                arguments: "{}".to_string(),
            }),
            final_text: "resumed-final".to_string(),
            calls: AtomicUsize::new(0),
        };
        let mut agent_loop = AgentLoop::new(Box::new(provider), base_config());
        agent_loop.register_tool(
            "cluster_rpc".to_string(),
            Box::new(FakeClusterRpc {
                result: "__CLUSTER_ASYNC__{\"task_id\":\"child-1\"}".to_string(),
            }),
        );
        let rig = spawn_rig(agent_loop, base_config(), tmp.path());

        rig.task_list.create_task(make_task("t2"));
        rig.work_queue.submit("t2".to_string()).unwrap();

        // execute_new_task 检出 __CLUSTER_ASYNC__ → save_async_state → WaitingRemote。
        wait_until(10_000, || {
            matches!(
                rig.task_list.get_task("t2").map(|t| t.status),
                Some(TaskStatus::WaitingRemote)
            )
        })
        .await;
        let saved = rig.task_list.get_task("t2").unwrap();
        assert_eq!(saved.waiting_for_task_id.as_deref(), Some("child-1"));
        assert_eq!(saved.waiting_tool_call_id.as_deref(), Some("tc_async"));
        assert!(saved.conversation.is_some(), "conversation 快照必须已保存");

        // 模拟远端回调：inject_callback → Pending → 重新入队 → resume 路径。
        rig.task_list.inject_callback("t2", "remote-node answer");
        rig.work_queue.submit("t2".to_string()).unwrap();

        // resume_execution 后不再 async → complete（任务被移除）。
        wait_until(10_000, || rig.task_list.get_task("t2").is_none()).await;

        rig.stop().await;
    }

    #[tokio::test]
    async fn resumed_task_going_async_again_saves_new_waiting_state() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = ScriptedProvider {
            first_tool_call: Some(ToolCallInfo {
                id: "tc_hop1".to_string(),
                name: "cluster_rpc".to_string(),
                arguments: "{}".to_string(),
            }),
            final_text: "should-not-matter".to_string(),
            calls: AtomicUsize::new(0),
        };
        let mut agent_loop = AgentLoop::new(Box::new(provider), base_config());
        agent_loop.register_tool(
            "cluster_rpc".to_string(),
            Box::new(FakeClusterRpc {
                result: "__CLUSTER_ASYNC__{\"task_id\":\"child-2\"}".to_string(),
            }),
        );
        let rig = spawn_rig(agent_loop, base_config(), tmp.path());

        rig.task_list.create_task(make_task("t3"));
        rig.work_queue.submit("t3".to_string()).unwrap();
        wait_until(10_000, || {
            matches!(
                rig.task_list.get_task("t3").map(|t| t.status),
                Some(TaskStatus::WaitingRemote)
            )
        })
        .await;

        // 回调本身又带 __CLUSTER_ASYNC__ → replace_tool_result 后 marker 仍在
        // 会话里 → resume 走「二次异步」分支（save_async_state，不完成）。
        rig.task_list
            .inject_callback("t3", "__CLUSTER_ASYNC__{\"task_id\":\"child-3\"}");
        rig.work_queue.submit("t3".to_string()).unwrap();

        wait_until(10_000, || {
            rig.task_list
                .get_task("t3")
                .and_then(|t| t.waiting_for_task_id)
                .as_deref()
                == Some("child-3")
        })
        .await;
        // 任务仍未完成（没有走 complete）。
        assert!(rig.task_list.get_task("t3").is_some());

        rig.stop().await;
    }

    #[tokio::test]
    async fn resume_without_conversation_snapshot_fails_the_task() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = ScriptedProvider {
            first_tool_call: None,
            final_text: "unused".to_string(),
            calls: AtomicUsize::new(0),
        };
        let agent_loop = AgentLoop::new(Box::new(provider), base_config());
        let rig = spawn_rig(agent_loop, base_config(), tmp.path());

        // 有 callback_result 但无 conversation 快照 → resume_task Err →
        // handle_task_error → Failed → complete（移除）。
        let mut task = make_task("t4");
        task.callback_result = Some("late answer".to_string());
        rig.task_list.create_task(task);
        rig.work_queue.submit("t4".to_string()).unwrap();

        wait_until(10_000, || rig.task_list.get_task("t4").is_none()).await;

        rig.stop().await;
    }

    #[tokio::test]
    async fn unparseable_async_marker_fails_the_task() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = ScriptedProvider {
            first_tool_call: Some(ToolCallInfo {
                id: "tc_bad".to_string(),
                name: "cluster_rpc".to_string(),
                arguments: "{}".to_string(),
            }),
            final_text: "unused".to_string(),
            calls: AtomicUsize::new(0),
        };
        let mut agent_loop = AgentLoop::new(Box::new(provider), base_config());
        agent_loop.register_tool(
            "cluster_rpc".to_string(),
            Box::new(FakeClusterRpc {
                // 带 marker 但解析不出 child task id → extract_async_info None
                // → execute_new_task Err → handle_task_error。
                result: "__CLUSTER_ASYNC__garbage-no-task-id".to_string(),
            }),
        );
        let rig = spawn_rig(agent_loop, base_config(), tmp.path());

        rig.task_list.create_task(make_task("t5"));
        rig.work_queue.submit("t5".to_string()).unwrap();

        wait_until(10_000, || rig.task_list.get_task("t5").is_none()).await;

        rig.stop().await;
    }

    #[tokio::test]
    async fn unknown_task_id_is_skipped_and_loop_continues() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = ScriptedProvider {
            first_tool_call: None,
            final_text: "still-alive".to_string(),
            calls: AtomicUsize::new(0),
        };
        let agent_loop = AgentLoop::new(Box::new(provider), base_config());
        let rig = spawn_rig(agent_loop, base_config(), tmp.path());

        // 队列里塞一个任务表没有的 id → warn + continue；后续真任务仍被处理。
        rig.work_queue.submit("ghost-id".to_string()).unwrap();
        rig.task_list.create_task(make_task("t6"));
        rig.work_queue.submit("t6".to_string()).unwrap();

        wait_until(10_000, || rig.task_list.get_task("t6").is_none()).await;

        rig.stop().await;
    }
}
