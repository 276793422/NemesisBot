//! K1 纯函数测试：参数校验/归一 + 文本模式事件折叠。
//! （装配与 e2e 属于门 4 的 TestAIServer 实机验收，不在此重复。）

use super::*;

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

#[test]
fn validate_defaults_build_mode_text_format() {
    let v = validate(Some("do X".into()), None, None, None, None, None).unwrap();
    assert_eq!(v.task, TaskSource::Arg("do X".into()));
    assert_eq!(v.mode, nemesis_agent::types::AgentMode::Build);
    assert_eq!(v.format, OutputFormat::Text);
    assert_eq!(v.max_turns, 0, "缺省 = 0 = 用配置默认");
    assert!(v.model.is_none());
    assert!(v.workspace.is_none());
}

#[test]
fn validate_dash_and_missing_task_both_mean_stdin() {
    let v = validate(None, None, None, None, None, None).unwrap();
    assert_eq!(v.task, TaskSource::Stdin);
    let v = validate(Some("-".into()), None, None, None, None, None).unwrap();
    assert_eq!(v.task, TaskSource::Stdin);
}

#[test]
fn validate_mode_is_case_tolerant() {
    let v = validate(None, None, Some("PLAN"), None, None, None).unwrap();
    assert_eq!(v.mode, nemesis_agent::types::AgentMode::Plan);
    let v = validate(None, None, Some("build"), None, None, None).unwrap();
    assert_eq!(v.mode, nemesis_agent::types::AgentMode::Build);
}

#[test]
fn validate_unknown_mode_rejected_with_remedy() {
    let err = validate(None, None, Some("yolo"), None, None, None).unwrap_err();
    assert!(err.contains("yolo"), "错误要回显原值: {err}");
    assert!(
        err.contains("plan") && err.contains("build"),
        "错误要给合法值: {err}"
    );
}

#[test]
fn validate_format_json_lands_k2() {
    let v = validate(None, None, None, Some("json"), None, None).unwrap();
    assert_eq!(v.format, OutputFormat::Json, "K2 已落地：json 合法");
}

#[test]
fn validate_unknown_format_rejected() {
    let err = validate(None, None, None, Some("yaml"), None, None).unwrap_err();
    assert!(err.contains("yaml") && err.contains("text"), "{err}");
}

#[test]
fn validate_carries_workspace_model_and_turn_budget() {
    let v = validate(
        None,
        Some(PathBuf::from("/tmp/wsx")),
        None,
        None,
        Some(42),
        Some("test/testai-1.1".into()),
    )
    .unwrap();
    assert_eq!(v.workspace, Some(PathBuf::from("/tmp/wsx")));
    assert_eq!(v.max_turns, 42);
    assert_eq!(v.model.as_deref(), Some("test/testai-1.1"));
}

// ---------------------------------------------------------------------------
// fold_text
// ---------------------------------------------------------------------------

use nemesis_agent::types::AgentEvent;

#[test]
fn fold_text_prefers_done_over_error() {
    let events = vec![
        AgentEvent::Message("intermediate".into()),
        AgentEvent::Error("stale".into()),
        AgentEvent::Done("final answer".into()),
    ];
    let (done, error) = fold_text(&events);
    assert_eq!(done.as_deref(), Some("final answer"));
    // error 记录最后一条 Error（与 run_detached 折叠同构）；调用方先看
    // done —— (Some, _) 臂即成功，error 此时无语义。
    assert_eq!(error.as_deref(), Some("stale"));
}

#[test]
fn fold_text_error_without_done_is_the_failure() {
    let events = vec![AgentEvent::Error("boom".into())];
    let (done, error) = fold_text(&events);
    assert_eq!(done, None);
    assert_eq!(error.as_deref(), Some("boom"));
}

#[test]
fn fold_text_no_terminal_event_is_honest_empty() {
    let events = vec![AgentEvent::Message("partial".into())];
    let (done, error) = fold_text(&events);
    assert_eq!(done, None);
    assert_eq!(error, None);
}

#[test]
fn fold_text_ignores_tool_traffic() {
    let events = vec![
        AgentEvent::ToolCall(Vec::new()),
        AgentEvent::ToolResult(nemesis_agent::types::ToolCallResult {
            tool_name: "exec".into(),
            result: "out".into(),
            is_error: false,
        }),
        AgentEvent::Done("ok".into()),
    ];
    let (done, _) = fold_text(&events);
    assert_eq!(done.as_deref(), Some("ok"));
}

// ---------------------------------------------------------------------------
// K2 NDJSON 序列化
// ---------------------------------------------------------------------------

use nemesis_types::agent::AgentEvent as LiveEvent;

#[test]
fn live_tool_start_serializes_with_schema_fields() {
    let ev = LiveEvent::ToolStarted {
        session_key: "subagent:s".into(),
        chat_id: "subagent:s".into(),
        call_id: "c1".into(),
        tool: "exec".into(),
        args_preview: "{\"command\":\"ls\"}".into(),
    };
    let v: serde_json::Value = serde_json::from_str(&serialize_live_event(&ev).unwrap()).unwrap();
    assert_eq!(v["type"], "tool_start");
    assert_eq!(v["call_id"], "c1");
    assert_eq!(v["tool"], "exec");
    assert_eq!(v["args_preview"], "{\"command\":\"ls\"}");
}

#[test]
fn live_tool_end_serializes_with_result_and_ok() {
    let ev = LiveEvent::ToolFinished {
        session_key: "subagent:s".into(),
        chat_id: "subagent:s".into(),
        call_id: "c1".into(),
        tool: "exec".into(),
        duration_ms: 42,
        ok: false,
        result_preview: "Tool error: boom".into(),
    };
    let v: serde_json::Value = serde_json::from_str(&serialize_live_event(&ev).unwrap()).unwrap();
    assert_eq!(v["type"], "tool_end");
    assert_eq!(v["duration_ms"], 42);
    assert_eq!(v["ok"], false);
    assert_eq!(v["result_preview"], "Tool error: boom");
}

#[test]
fn live_events_outside_v1_contract_are_skipped() {
    // TodoUpdated / ModeChanged 不在 v1 固定 5-type 契约内——不发未知 type。
    let todo = LiveEvent::TodoUpdated {
        session_key: "s".into(),
        chat_id: "s".into(),
        todos: vec![],
    };
    let mode = LiveEvent::ModeChanged {
        session_key: "s".into(),
        chat_id: "s".into(),
        mode: "plan".into(),
    };
    assert_eq!(serialize_live_event(&todo), None);
    assert_eq!(serialize_live_event(&mode), None);
}

#[test]
fn terminal_events_serialize_turn_final_error_in_order() {
    let events = vec![
        AgentEvent::Message("thinking out loud".into()),
        AgentEvent::ToolCall(Vec::new()),
        AgentEvent::ToolResult(nemesis_agent::types::ToolCallResult {
            tool_name: "exec".into(),
            result: "out".into(),
            is_error: false,
        }),
        AgentEvent::Done("the answer".into()),
    ];
    let lines = serialize_terminal_events(&events);
    assert_eq!(lines.len(), 2, "工具流量不产生终结行");
    let t0: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let t1: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(t0["type"], "turn");
    assert_eq!(t0["text"], "thinking out loud");
    assert_eq!(t1["type"], "final");
    assert_eq!(t1["text"], "the answer");
}

#[test]
fn terminal_error_serializes_without_final() {
    let lines = serialize_terminal_events(&[AgentEvent::Error("boom".into())]);
    assert_eq!(lines.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(v["type"], "error");
    assert_eq!(v["message"], "boom");
}

#[test]
fn terminal_empty_is_empty_lines() {
    assert!(serialize_terminal_events(&[]).is_empty());
}

// ---------------------------------------------------------------------------
// wave_a（2026-09-25）：fold_and_finish 三臂 + run() 入口错误分支 +
// headless 全链（死地址 provider，与 agent s11b 同款 127.0.0.1:1 即刻拒绝）。
// stdin 相关测试依赖套件以 `< /dev/null` 运行（与 eval_rules 同约定）。
// ---------------------------------------------------------------------------

mod wave_a {
    #![allow(clippy::await_holding_lock)]
    use super::*;

    pub(super) struct HomeEnv {
        _guard: std::sync::MutexGuard<'static, ()>,
        _tmp: tempfile::TempDir,
        pub(super) home: std::path::PathBuf,
    }
    impl Drop for HomeEnv {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("NEMESISBOT_HOME") };
        }
    }
    pub(super) fn home_env() -> HomeEnv {
        let guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
        HomeEnv {
            _guard: guard,
            _tmp: tmp,
            home,
        }
    }

    pub(super) fn dead_provider_config() -> serde_json::Value {
        serde_json::json!({
            "agents": {"defaults": {"llm": "fake"}},
            "model_list": [{
                "model_name": "fake",
                "model": "openai/gpt-fake",
                "api_base": "http://127.0.0.1:1",
                "api_key": "k"
            }]
        })
    }

    #[test]
    fn fold_and_finish_done_prints_and_is_ok() {
        let res = fold_and_finish(&[AgentEvent::Done("final".into())]);
        assert!(res.is_ok());
    }

    #[test]
    fn fold_and_finish_error_without_done_is_err() {
        let err = fold_and_finish(&[AgentEvent::Error("boom".into())]).unwrap_err();
        assert!(err.to_string().contains("agent error: boom"), "{err}");
    }

    #[test]
    fn fold_and_finish_no_terminal_is_honest_err() {
        let err = fold_and_finish(&[AgentEvent::Message("partial".into())]).unwrap_err();
        assert!(
            err.to_string().contains("agent produced no output"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn run_stdin_eof_empty_task_bails() {
        let _th = home_env();
        // task=None → Stdin 源；EOF ⇒ 空任务 ⇒ 诚实报错。
        let err = run(&_th.home, None, None, None, None, None, None)
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("empty task: pass a prompt argument or pipe a task via stdin"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn run_config_missing_bails() {
        let _th = home_env();
        let err = run(&_th.home, Some("hi".into()), None, None, None, None, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Configuration not found"), "{err}");
    }

    #[tokio::test]
    async fn run_corrupt_config_bails() {
        let th = home_env();
        std::fs::write(th.home.join("config.json"), "{not json").unwrap();
        let err = run(&th.home, Some("hi".into()), None, None, None, None, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("failed to load config"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_headless_dead_provider_text_completes() {
        let th = home_env();
        std::fs::write(
            th.home.join("config.json"),
            dead_provider_config().to_string(),
        )
        .unwrap();
        // 全链装配（安全插件/工厂/tier）+ 死地址 LLM：LLM 层失败必须折进
        // 终结事件——要么 Done 携带失败文案（Ok），要么 Error → Err
        // "agent error: …"。不 panic、不挂起即契约。
        let res = run(
            &th.home,
            Some("say hi".into()),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        match res {
            Ok(()) => {}
            Err(e) => assert!(
                e.to_string().contains("agent error"),
                "LLM 失败必须以 agent error 语义浮出，实际：{e:?}"
            ),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_headless_dead_provider_json_completes() {
        let th = home_env();
        std::fs::write(
            th.home.join("config.json"),
            dead_provider_config().to_string(),
        )
        .unwrap();
        // json 模式：走 K2 事件通道分支（agent_event_tx Some + NDJSON 打印），
        // 同样不得 panic / 挂起。
        let res = run(
            &th.home,
            Some("say hi".into()),
            None,
            None,
            Some("json".into()),
            None,
            None,
        )
        .await;
        match res {
            Ok(()) => {}
            Err(e) => assert!(
                e.to_string().contains("agent error"),
                "json 模式 LLM 失败同样以 agent error 语义浮出，实际：{e:?}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// wave6（2026-09-25）：`--model` 覆盖臂——解析成功（工厂重建 provider +
// set_provider_and_model）与解析失败（"add it first" 引导文案）双臂。
// ---------------------------------------------------------------------------
mod wave6 {
    #![allow(clippy::await_holding_lock)]
    use super::wave_a::{dead_provider_config, home_env};
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn w6_run_model_override_resolves_and_swaps_provider() {
        let th = home_env();
        std::fs::write(
            th.home.join("config.json"),
            dead_provider_config().to_string(),
        )
        .unwrap();
        // --model fake：config 里已登记 fake（死端点）→ 解析成功 →
        // factory_cfg 构造 + create_provider + set_provider_and_model 全链
        //（死端点不产生 LLM 成功，但装配臂必须走通、不得 panic/挂起）。
        let res = run(
            &th.home,
            Some("hi".into()),
            None,
            None,
            None,
            None,
            Some("fake".into()),
        )
        .await;
        match res {
            Ok(()) => {}
            Err(e) => assert!(
                e.to_string().contains("agent error"),
                "装配成功后 LLM 失败仍须以 agent error 语义浮出：{e:?}"
            ),
        }
    }

    #[tokio::test]
    async fn w6_run_model_override_unknown_model_bails_with_remedy() {
        let th = home_env();
        std::fs::write(
            th.home.join("config.json"),
            dead_provider_config().to_string(),
        )
        .unwrap();
        // --model 不存在的模型 → resolve_model_config Err → 引导文案。
        let err = run(
            &th.home,
            Some("hi".into()),
            None,
            None,
            None,
            None,
            Some("w6-not-registered".into()),
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("w6-not-registered") && msg.contains("add it first"),
            "缺模型引导文案必须包含模型名与补救指引：{msg}"
        );
    }
}
