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
        image_refs: Vec::new(),
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    fn spawn_rig(
        agent_loop: AgentLoop,
        config: AgentConfig,
        data_dir: &std::path::Path,
    ) -> LoopRig {
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
        LoopRig {
            task_list,
            work_queue,
            shutdown_tx,
            handle,
        }
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

// =========================================================================
// wave_b（coverage 补测批次 B）
//
// 目标 miss：resume-Err 臂（90-97）、restored>0 日志（151）、
// execute/resume 的 observer 生命周期臂（161-168 / 174-184 / 271-278 /
// 284-294）、truncate_str 长串两分支（348-353 / 355）、summary cache
// restore 的 covers=Some/clamp 与 legacy=None 两臂 + persist 的 cache-Some
// 写路径（403-417 / 444-447）、extract_async_info 前驱回看的两种未命中形态
// （528-529）、count_llm_rounds 两形态（554-560）。
// 豁免不碰：48-49（queue.next() None break——队列内部持有 tx，结构性死码）。
// 本地复刻 loop_e2e 的 provider/rig（兄弟 mod 私有项不可见）。
// =========================================================================

mod wave_b {
    use super::*;
    use nemesis_agent::r#loop::{LlmMessage, LlmProvider, LlmResponse, Tool};
    use nemesis_agent::request_logger::LoggingConfig;
    use nemesis_agent::types::{ChatOptions, ToolDefinition};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // -- 直接单测：truncate_str ---------------------------------------------

    /// ASCII 超长串 → char_indices().nth(max_len) 命中 Some(idx) 分支：
    /// 截到 max_len 个字符再补 "..."。
    #[test]
    fn wb_truncate_str_long_ascii_appends_ellipsis() {
        let s = "a".repeat(250);
        let out = truncate_str(&s, 200);
        assert_eq!(out.chars().count(), 203);
        assert!(out.ends_with("..."));
        assert_eq!(&out[..200], "a".repeat(200));
    }

    /// 多字节串：字节长超限但字符数没超 → nth(max_len)==None → 原样返回，
    /// 绝不做非边界切片（str 切多字节会 panic，见 MEMORY 教训）。
    #[test]
    fn wb_truncate_str_multibyte_under_char_limit_returns_original() {
        let s = "字".repeat(100); // 300 bytes / 100 chars
        let out = truncate_str(&s, 200);
        assert_eq!(out, s, "char count under limit → untouched");
    }

    // -- 直接单测：extract_async_info 前驱回看未命中形态 ---------------------

    /// marker 前一turn不是 assistant（user）→ tool_call_id 拿不到 → None。
    #[test]
    fn wb_extract_async_info_prev_turn_not_assistant_returns_none() {
        let convo = vec![
            make_turn("user", "直接贴 marker", vec![]),
            make_turn(
                "tool",
                "__CLUSTER_ASYNC__{\"task_id\":\"wb-no-assist\"}",
                vec![],
            ),
        ];
        assert!(extract_async_info(&convo).is_none());
    }

    /// 前一turn是 assistant 但没有 tool_calls → first()==None → None。
    #[test]
    fn wb_extract_async_info_prev_assistant_without_tool_calls_returns_none() {
        let convo = vec![
            make_turn("user", "hi", vec![]),
            make_turn("assistant", "无工具调用的回合", vec![]),
            make_turn(
                "tool",
                "__CLUSTER_ASYNC__{\"task_id\":\"wb-no-tc\"}",
                vec![],
            ),
        ];
        assert!(extract_async_info(&convo).is_none());
    }

    // -- 直接单测：count_llm_rounds -----------------------------------------

    /// 空事件序列 → ToolCall 计数 0 + 1 = 1。
    #[test]
    fn wb_count_llm_rounds_empty_events_is_one() {
        assert_eq!(count_llm_rounds(&[]), 1);
    }

    /// N 个 ToolCall 事件 → N+1（镜像主 agent 公式）。
    #[test]
    fn wb_count_llm_rounds_counts_tool_calls_plus_one() {
        let events = vec![
            AgentEvent::Message("start".to_string()),
            AgentEvent::ToolCall(vec![]),
            AgentEvent::Message("mid".to_string()),
            AgentEvent::ToolCall(vec![]),
            AgentEvent::Done("done".to_string()),
        ];
        assert_eq!(count_llm_rounds(&events), 3);
    }

    // -- summary cache 持久化 / 恢复双臂 --------------------------------------

    use nemesis_agent::instance::SummaryCache;

    /// persist 带 SummaryCache → store 写入 text+covers_up_to（444-447 臂）；
    /// 新实例 restore 时 covers=Some 走 clamp 分支恢复缓存（Some 臂）。
    #[test]
    fn wb_persist_then_restore_carries_summary_cache_with_covers() {
        let (agent_loop, store) = make_loop_with_session_store();
        let inst = AgentInstance::new(make_test_config());
        inst.add_user_message("wb-cache-q");
        inst.add_assistant_message("wb-cache-a", Vec::new(), None);
        inst.set_summary_cache(Some(SummaryCache {
            covers_up_to: 2,
            text: "wb-summary-text".to_string(),
        }));
        persist_session_history(&agent_loop, &inst, "sess-wb-covers");

        // store 侧：cache-Some 写路径生效。
        assert_eq!(store.get_summary("sess-wb-covers"), "wb-summary-text");
        assert_eq!(store.get_summary_covers_up_to("sess-wb-covers"), Some(2));

        // restore 侧：covers=Some → clamp(1..=len) 保住索引并重建实例缓存。
        let fresh = AgentInstance::new(make_test_config());
        let restored = restore_session_history(&agent_loop, &fresh, "sess-wb-covers");
        assert_eq!(restored, 3); // [system, user, assistant]
        let cache = fresh.get_summary_cache().expect("summary cache restored");
        assert_eq!(cache.text, "wb-summary-text");
        assert_eq!(cache.covers_up_to, 2);
    }

    /// legacy 存量（有 summary 文本但无 covers 索引）→ restore 走
    /// take_while(system).count().max(1) 默认推导分支（None 臂）。
    #[test]
    fn wb_restore_legacy_summary_without_index_computes_default_cover() {
        let (agent_loop, store) = make_loop_with_session_store();
        let inst = AgentInstance::new(make_test_config());
        inst.add_user_message("wb-legacy-q");
        inst.add_assistant_message("wb-legacy-a", Vec::new(), None);
        // 无 summary cache → persist 走 None 臂写空摘要。
        persist_session_history(&agent_loop, &inst, "sess-wb-legacy");

        // 手工制造 legacy 磁盘形态：只有文本、没有索引。
        store.set_summary("sess-wb-legacy", "wb-legacy-summary");

        let fresh = AgentInstance::new(make_test_config());
        let restored = restore_session_history(&agent_loop, &fresh, "sess-wb-legacy");
        assert_eq!(restored, 3);
        let cache = fresh.get_summary_cache().expect("legacy summary restored");
        assert_eq!(cache.text, "wb-legacy-summary");
        // [system,...] 前缀只数出 system 提示一条 → max(1) 兜底为 1。
        assert_eq!(cache.covers_up_to, 1);
    }

    // -- 端到端 rig（本地复刻 loop_e2e 形态）---------------------------------

    struct WbScriptedProvider {
        first_tool_call: Option<ToolCallInfo>,
        final_text: String,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for WbScriptedProvider {
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

    /// 假 cluster_rpc 工具：返回 __CLUSTER_ASYNC__ 标记驱动异步挂起路径。
    struct WbFakeClusterRpc {
        result: String,
    }

    #[async_trait::async_trait]
    impl Tool for WbFakeClusterRpc {
        async fn execute(
            &self,
            _args: &str,
            _context: &nemesis_agent::context::RequestContext,
        ) -> Result<String, String> {
            Ok(self.result.clone())
        }
    }

    fn wb_config(tools: bool) -> AgentConfig {
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("cluster wave-b test".to_string()),
            max_turns: 4,
            tools: if tools {
                vec!["cluster_rpc".to_string()]
            } else {
                vec![]
            },
            ..Default::default()
        }
    }

    fn wb_make_task(task_id: &str) -> ClusterTask {
        ClusterTask {
            task_id: task_id.to_string(),
            source: TaskSource {
                node_id: "node-wb".to_string(),
                rpc_address: "127.0.0.1:9".to_string(),
                session_key: format!("sess-{task_id}"),
            },
            status: TaskStatus::Pending,
            content: "wave-b please do the thing".to_string(),
            conversation: None,
            waiting_for_task_id: None,
            waiting_tool_call_id: None,
            callback_result: None,
        }
    }

    async fn wb_wait_until(deadline_ms: u64, f: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
        while !f() {
            assert!(
                std::time::Instant::now() < deadline,
                "condition not met within {deadline_ms}ms"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    struct WbRig {
        task_list: Arc<ClusterTaskList>,
        work_queue: Arc<ClusterWorkQueue>,
        shutdown_tx: tokio::sync::broadcast::Sender<()>,
        handle: tokio::task::JoinHandle<()>,
    }

    impl WbRig {
        async fn stop(self) {
            let _ = self.shutdown_tx.send(());
            let _ = self.handle.await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_wb_rig(
        agent_loop: AgentLoop,
        config: AgentConfig,
        data_dir: &std::path::Path,
        observer: Option<Arc<crate::cluster_request_logger_observer::ClusterRequestLoggerObserver>>,
    ) -> WbRig {
        let task_list = Arc::new(ClusterTaskList::new(data_dir));
        let work_queue = Arc::new(ClusterWorkQueue::new(8));
        let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);
        let handle = tokio::spawn(cluster_agent_loop(
            Arc::new(agent_loop),
            config,
            work_queue.clone(),
            task_list.clone(),
            None, // rpc_client=None：回调走无客户端跳过（无网络）
            observer,
            shutdown_rx,
        ));
        WbRig {
            task_list,
            work_queue,
            shutdown_tx,
            handle,
        }
    }

    /// resume-Err 臂（90-91/96-97）：conversation 快照存在、callback_result
    /// 存在、但 waiting_tool_call_id 缺失 → resume_task 在
    /// ok_or("No waiting_tool_call_id") 处失败 → handle_task_error → Failed
    /// → complete（任务移除）。
    /// （既有 loop_e2e::resume_without_conversation_snapshot… 因
    /// conversation=None 根本没进 resume 分支——本测试补上真命中。）
    #[tokio::test]
    async fn wb_resume_missing_waiting_tool_call_id_fails_the_task() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = WbScriptedProvider {
            first_tool_call: None,
            final_text: "unused".to_string(),
            calls: AtomicUsize::new(0),
        };
        let agent_loop = AgentLoop::new(Box::new(provider), wb_config(false));
        let rig = spawn_wb_rig(agent_loop, wb_config(false), tmp.path(), None);

        let mut task = wb_make_task("t-wb-resume-err");
        task.conversation = Some(serde_json::json!([])); // 可反序列化的空快照
        task.callback_result = Some("late reply".to_string());
        // waiting_tool_call_id 保持 None。
        rig.task_list.create_task(task);
        rig.work_queue
            .submit("t-wb-resume-err".to_string())
            .unwrap();

        wb_wait_until(10_000, || {
            rig.task_list.get_task("t-wb-resume-err").is_none()
        })
        .await;

        rig.stop().await;
    }

    /// 新任务执行时带 SessionStore 种子历史 + ClusterRequestLoggerObserver：
    /// restore>0 日志臂、observer start/end 臂全部走到；
    /// 完成后 full history 回写 store（restore 3 条 + 本轮 user/assistant）。
    #[tokio::test]
    async fn wb_execute_restores_history_and_emits_observer_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = WbScriptedProvider {
            first_tool_call: None,
            final_text: "wb-obs-done".to_string(),
            calls: AtomicUsize::new(0),
        };
        let mut agent_loop = AgentLoop::new(Box::new(provider), wb_config(false));
        let store = Arc::new(SessionStore::new_in_memory());
        agent_loop.set_session_store(store.clone());

        // 种子上一轮历史：[system, user, assistant]。
        let seed = AgentInstance::new(wb_config(false));
        seed.add_user_message("earlier question");
        seed.add_assistant_message("earlier answer", Vec::new(), None);
        persist_session_history(&agent_loop, &seed, "sess-t-wb-obs");

        // LoggingConfig 默认 enabled=false → observer 臂执行但零磁盘副作用。
        let observer = Arc::new(
            crate::cluster_request_logger_observer::ClusterRequestLoggerObserver::new(
                LoggingConfig::default(),
                tmp.path(),
            ),
        );
        let rig = spawn_wb_rig(agent_loop, wb_config(false), tmp.path(), Some(observer));

        rig.task_list.create_task(wb_make_task("t-wb-obs"));
        rig.work_queue.submit("t-wb-obs".to_string()).unwrap();
        wb_wait_until(10_000, || rig.task_list.get_task("t-wb-obs").is_none()).await;

        // store 收尾状态：seed 3 条 + 本轮 user("...content") + assistant("wb-obs-done")。
        let msgs = store.get_history("sess-t-wb-obs");
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[1].content, "earlier question");
        assert_eq!(msgs[2].content, "earlier answer");
        assert_eq!(msgs[3].role, "user");
        assert_eq!(msgs[4].role, "assistant");
        assert_eq!(msgs[4].content, "wb-obs-done");

        rig.stop().await;
    }

    /// resume 全流程 observer 臂（271-278 / 284-294）：
    /// 先异步挂起（WaitingRemote + save_async_state），回调注入后 resume
    /// 完成 → observer 的 set_task_context / emit_conversation_start/end /
    /// clear_task_context 全部在 resume 路径各走一遍。
    #[tokio::test]
    async fn wb_resume_path_emits_observer_start_end_arms() {
        let tmp = tempfile::tempdir().unwrap();
        let provider = WbScriptedProvider {
            first_tool_call: Some(ToolCallInfo {
                id: "tc-wb-obs".to_string(),
                name: "cluster_rpc".to_string(),
                arguments: "{}".to_string(),
            }),
            final_text: "wb-resume-final".to_string(),
            calls: AtomicUsize::new(0),
        };
        let mut agent_loop = AgentLoop::new(Box::new(provider), wb_config(true));
        agent_loop.register_tool(
            "cluster_rpc".to_string(),
            Box::new(WbFakeClusterRpc {
                result: "__CLUSTER_ASYNC__{\"task_id\":\"wb-child\"}".to_string(),
            }),
        );
        let observer = Arc::new(
            crate::cluster_request_logger_observer::ClusterRequestLoggerObserver::new(
                LoggingConfig::default(),
                tmp.path(),
            ),
        );
        let rig = spawn_wb_rig(agent_loop, wb_config(true), tmp.path(), Some(observer));

        rig.task_list.create_task(wb_make_task("t-wb-resume-obs"));
        rig.work_queue
            .submit("t-wb-resume-obs".to_string())
            .unwrap();

        // 第一段：exec 异步挂起。
        wb_wait_until(10_000, || {
            matches!(
                rig.task_list.get_task("t-wb-resume-obs").map(|t| t.status),
                Some(TaskStatus::WaitingRemote)
            )
        })
        .await;
        let saved = rig.task_list.get_task("t-wb-resume-obs").unwrap();
        assert_eq!(saved.waiting_for_task_id.as_deref(), Some("wb-child"));
        assert_eq!(saved.waiting_tool_call_id.as_deref(), Some("tc-wb-obs"));
        assert!(saved.conversation.is_some());

        // 第二段：纯文本回调 → replace_tool_result 抹掉 marker → 同步完成。
        rig.task_list
            .inject_callback("t-wb-resume-obs", "plain remote answer");
        rig.work_queue
            .submit("t-wb-resume-obs".to_string())
            .unwrap();
        wb_wait_until(10_000, || {
            rig.task_list.get_task("t-wb-resume-obs").is_none()
        })
        .await;

        rig.stop().await;
    }
}

// =========================================================================
// wave_c 补测（coverage 补测批次 C）：restore 的 covers 索引边界。
//
// 目标 miss 邻域：restore_session_history 里 SummaryCache 重建的
// clamp(1..=len) 边界（0 下溢钳到 1、超大上溢钳到 history.len()）。
// 另注（豁免结论，基于读码）：
// - cluster_agent.rs:48-49（work queue closed → break）：ClusterWorkQueue
//   自持 mpsc Sender（cluster_task.rs:80 结构体字段），Arc<Queue> 存活期间
//   next() 不可能返回 None —— 结构性死码，测试不可达；
// - :528-529 前驱回看未命中形态：wave_b 已有两条直测覆盖该路径（map 单行
//   区域颗粒度噪声），无新的可达形态；
// - :417 内层 !history.is_empty() 假臂：line 394 已保证 messages 非空、
//   set_history 后必非空 —— 同为结构性死码。
// =========================================================================

mod wave_c {
    use super::*;
    use nemesis_agent::instance::SummaryCache;

    /// covers=Some(0)（下界溢出垃圾值）→ clamp 到 1，绝不让 covers 落在
    /// 历史[1..=len]范围之外。
    #[test]
    fn wc_restore_clamps_zero_covers_index_to_floor_one() {
        let (agent_loop, _store) = make_loop_with_session_store();
        let inst = AgentInstance::new(make_test_config());
        inst.add_user_message("wc-floor-q");
        inst.add_assistant_message("wc-floor-a", Vec::new(), None);
        inst.set_summary_cache(Some(SummaryCache {
            covers_up_to: 0,
            text: "wc-summary".to_string(),
        }));
        persist_session_history(&agent_loop, &inst, "sess-wc-floor");

        let fresh = AgentInstance::new(make_test_config());
        let restored = restore_session_history(&agent_loop, &fresh, "sess-wc-floor");
        assert_eq!(restored, 3);
        let cache = fresh.get_summary_cache().expect("cache restored");
        assert_eq!(cache.covers_up_to, 1, "0 must clamp to lower bound 1");
        assert_eq!(cache.text, "wc-summary");
    }

    /// covers=Some(999)（远超历史长度的垃圾值）→ clamp 到 history.len()。
    #[test]
    fn wc_restore_clamps_oversized_covers_index_to_history_len() {
        let (agent_loop, _store) = make_loop_with_session_store();
        let inst = AgentInstance::new(make_test_config());
        inst.add_user_message("wc-ceil-q");
        inst.add_assistant_message("wc-ceil-a", Vec::new(), None);
        inst.set_summary_cache(Some(SummaryCache {
            covers_up_to: 999,
            text: "wc-ceil-summary".to_string(),
        }));
        persist_session_history(&agent_loop, &inst, "sess-wc-ceil");

        let fresh = AgentInstance::new(make_test_config());
        let restored = restore_session_history(&agent_loop, &fresh, "sess-wc-ceil");
        assert_eq!(restored, 3);
        let cache = fresh.get_summary_cache().expect("cache restored");
        assert_eq!(cache.covers_up_to, 3, "999 must clamp to history length");
        assert_eq!(cache.text, "wc-ceil-summary");
    }
}
