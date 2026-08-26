//! eval_worker 单测：打点观察者 + 分层分析 + 汇总（纯逻辑，不进 Sandboxie）。
//!
//! `run()`/`run_inner()` 需要真实 agent loop + 盒内环境（结构性）；
//! 这里测的是观察者协议（on_event 过滤/记录/取尽）、`run_layers` 各层
//! 分析分支（用安全层引擎的真实现，输入用确定性的样例）、`summarize`
//! 汇总计数和 `ToolTag` 序列化形状。

use super::*;

// ---------------------------------------------------------------------------
// summarize —— 汇总计数（私有类型经子模块可见）
// ---------------------------------------------------------------------------

fn tag_with(findings: LayerFindings) -> ToolTag {
    ToolTag {
        tool_name: "exec".to_string(),
        arguments: serde_json::json!({}),
        result: None,
        success: true,
        duration_ms: 1,
        llm_round: 1,
        timestamp: "t".to_string(),
        findings,
    }
}

#[test]
fn summarize_empty_is_all_zero() {
    let s = summarize(&[]);
    assert_eq!(s.total_tool_calls, 0);
    assert_eq!(s.injection_hits, 0);
    assert_eq!(s.blocked_commands, 0);
    assert_eq!(s.credential_hits_in, 0);
    assert_eq!(s.credential_hits_out, 0);
    assert_eq!(s.dlp_hits_in, 0);
    assert_eq!(s.dlp_hits_out, 0);
    assert_eq!(s.ssrf_blocks, 0);
}

#[test]
fn summarize_counts_every_dimension_once_per_tag() {
    let hit_all = LayerFindings {
        injection: Some(InjectionFinding {
            is_injection: true,
            score: 0.9,
            level: "high".to_string(),
        }),
        command_guard: Some(CommandFinding {
            blocked: true,
            reason: "dangerous".to_string(),
        }),
        credentials_in: Some(vec!["aws-key".to_string()]),
        credentials_out: Some(vec!["gh-token".to_string()]),
        dlp_in: Some(vec!["phone".to_string()]),
        dlp_out: Some(vec!["email".to_string()]),
        ssrf: Some(SsrfFinding {
            url: "http://169.254.169.254/".to_string(),
            blocked: true,
            reason: "metadata".to_string(),
        }),
    };
    // 三个 tag：1 个全命中 + 2 个无命中 → 每维 = 1，总数 = 3。
    let tags = vec![
        tag_with(hit_all.clone()),
        tag_with(LayerFindings::default()),
        tag_with(LayerFindings::default()),
    ];
    let s = summarize(&tags);
    assert_eq!(s.total_tool_calls, 3);
    assert_eq!(s.injection_hits, 1);
    assert_eq!(s.blocked_commands, 1);
    assert_eq!(s.credential_hits_in, 1);
    assert_eq!(s.credential_hits_out, 1);
    assert_eq!(s.dlp_hits_in, 1);
    assert_eq!(s.dlp_hits_out, 1);
    assert_eq!(s.ssrf_blocks, 1);
}

#[test]
fn summarize_ignores_non_hits_and_empty_vecs() {
    // 注入未命中 / 命令未拦 / credentials Some(vec![]) 都不计。
    let no_hits = LayerFindings {
        injection: Some(InjectionFinding {
            is_injection: false,
            score: 0.1,
            level: "low".to_string(),
        }),
        command_guard: Some(CommandFinding {
            blocked: false,
            reason: String::new(),
        }),
        credentials_in: Some(vec![]),
        credentials_out: Some(vec![]),
        dlp_in: Some(vec![]),
        dlp_out: Some(vec![]),
        ssrf: Some(SsrfFinding {
            url: "http://example.com/".to_string(),
            blocked: false,
            reason: String::new(),
        }),
    };
    let s = summarize(&[tag_with(no_hits)]);
    assert_eq!(s.total_tool_calls, 1);
    assert_eq!(s.injection_hits, 0);
    assert_eq!(s.blocked_commands, 0);
    assert_eq!(s.credential_hits_in, 0, "empty Some(vec![]) must not count");
    assert_eq!(s.ssrf_blocks, 0);
}

// ---------------------------------------------------------------------------
// EvalTaggingObserver —— run_layers（真引擎 + 确定性输入）
// ---------------------------------------------------------------------------

mod layers {
    use super::super::*;

    fn observer_in_tmp_home() -> (tempfile::TempDir, EvalTaggingObserver) {
        let dir = tempfile::tempdir().expect("tempdir");
        let shared = std::sync::Arc::new(crate::agent_factory::SharedResources {
            home: dir.path().to_path_buf(),
            ..Default::default()
        });
        // EvalTaggingObserver::new 只用 shared.home（build_security_plugin
        // 忽略 home），插件 enabled=false 只分析不拦截。
        let obs = EvalTaggingObserver::new(&shared);
        (dir, obs)
    }

    #[tokio::test]
    async fn benign_exec_command_is_analyzed_not_blocked() {
        let (_dir, obs) = observer_in_tmp_home();
        let args = serde_json::json!({"command": "echo hi"});
        let f = obs
            .run_layers("exec", &args, &args.to_string(), "hi\n")
            .await;
        // 良性命令：guard 跑过且未拦。
        let cg = f.command_guard.expect("command field → guard runs");
        assert!(!cg.blocked, "echo must not be blocked");
        // 没有 url 字段 → ssrf 层不产出。
        assert!(f.ssrf.is_none());
        // 结果干净 → credentials_out / dlp_out 无命中。
        assert!(f.credentials_out.is_none());
        assert!(f.dlp_out.is_none());
        // 注入层总是产出（是否命中由引擎判）。
        assert!(f.injection.is_some());
    }

    #[tokio::test]
    async fn destructive_command_is_flagged_blocked() {
        let (_dir, obs) = observer_in_tmp_home();
        // `rm -rf /` 在 command guard 静态黑名单里（crates/nemesis-security
        // /src/command/tests.rs 钉过），确定性拦。
        let args = serde_json::json!({"command": "rm -rf /"});
        let f = obs.run_layers("exec", &args, &args.to_string(), "").await;
        let cg = f.command_guard.expect("guard finding present");
        assert!(cg.blocked, "rm -rf / must be blocked");
        assert!(!cg.reason.is_empty(), "blocked must carry a reason");
    }

    #[tokio::test]
    async fn no_command_field_skips_command_guard() {
        let (_dir, obs) = observer_in_tmp_home();
        let args = serde_json::json!({"path": "/tmp/x"});
        let f = obs.run_layers("read_file", &args, &args.to_string(), "").await;
        assert!(f.command_guard.is_none(), "no command field → no guard finding");
    }

    #[tokio::test]
    async fn cloud_metadata_url_is_flagged_ssrf_blocked() {
        let (_dir, obs) = observer_in_tmp_home();
        // 169.254.169.254 链路本地地址被 SSRF guard 无条件拦
        //（ssrf.rs resolve_and_validate_locked）。
        let args = serde_json::json!({"url": "http://169.254.169.254/latest/meta-data"});
        let f = obs.run_layers("web_fetch", &args, &args.to_string(), "").await;
        let s = f.ssrf.expect("url field → ssrf layer runs");
        assert!(s.blocked, "metadata endpoint must be blocked");
        assert_eq!(s.url, "http://169.254.169.254/latest/meta-data");
        assert!(!s.reason.is_empty());
    }

    #[tokio::test]
    async fn args_with_known_credential_are_flagged_inbound() {
        let (_dir, obs) = observer_in_tmp_home();
        // AWS 访问键样例（credential/tests.rs 钉过的确定性样例）。
        let args = serde_json::json!({"command": "echo key=AKIAIOSFODNN7EXAMPLE"});
        let f = obs.run_layers("exec", &args, &args.to_string(), "").await;
        let cin = f.credentials_in.expect("credential must be detected in args");
        assert!(!cin.is_empty());
        // 无 result → credentials_out 不产出。
        assert!(f.credentials_out.is_none());
    }

    #[tokio::test]
    async fn ordinary_args_produce_no_credential_findings() {
        let (_dir, obs) = observer_in_tmp_home();
        let args = serde_json::json!({"command": "echo hello world"});
        let f = obs.run_layers("exec", &args, &args.to_string(), "hello world\n").await;
        assert!(f.credentials_in.is_none());
        assert!(f.credentials_out.is_none());
    }
}

// ---------------------------------------------------------------------------
// Observer 协议 —— on_event 过滤 + 记录 + take_tags 取尽
// ---------------------------------------------------------------------------

mod observer_protocol {
    use super::super::*;
    use nemesis_observer::{ConversationEvent, ConversationStartData, EventData, EventType};
    use std::collections::HashMap;

    fn observer_in_tmp_home() -> (tempfile::TempDir, EvalTaggingObserver) {
        let dir = tempfile::tempdir().expect("tempdir");
        let shared = std::sync::Arc::new(crate::agent_factory::SharedResources {
            home: dir.path().to_path_buf(),
            ..Default::default()
        });
        (dir, EvalTaggingObserver::new(&shared))
    }

    fn tool_call_event(tool: &str, result: Option<&str>) -> ConversationEvent {
        let mut arguments = HashMap::new();
        arguments.insert("command".to_string(), serde_json::json!("echo hi"));
        ConversationEvent {
            event_type: EventType::ToolCall,
            trace_id: "t-1".to_string(),
            timestamp: chrono::Local::now(),
            data: EventData::ToolCall(nemesis_observer::ToolCallData {
                tool_name: tool.to_string(),
                arguments,
                success: true,
                duration: std::time::Duration::from_millis(7),
                error: None,
                llm_round: 3,
                chain_pos: 1,
                result: result.map(|r| r.to_string()),
            }),
        }
    }

    #[tokio::test]
    async fn non_tool_call_events_are_ignored() {
        let (_dir, obs) = observer_in_tmp_home();
        let ev = ConversationEvent {
            event_type: EventType::ConversationStart,
            trace_id: "t-0".to_string(),
            timestamp: chrono::Local::now(),
            data: EventData::ConversationStart(ConversationStartData {
                session_key: "s".to_string(),
                channel: "web".to_string(),
                chat_id: "c".to_string(),
                sender_id: "u".to_string(),
                content: "hello".to_string(),
            }),
        };
        obs.on_event(ev).await;
        assert!(obs.take_tags().is_empty(), "non-ToolCall events must not tag");
    }

    #[tokio::test]
    async fn tool_call_event_is_recorded_with_mapped_fields() {
        let (_dir, obs) = observer_in_tmp_home();
        obs.on_event(tool_call_event("exec", Some("hi\n"))).await;
        let tags = obs.take_tags();
        assert_eq!(tags.len(), 1);
        let t = &tags[0];
        assert_eq!(t.tool_name, "exec");
        assert!(t.success);
        assert_eq!(t.result.as_deref(), Some("hi\n"));
        assert_eq!(t.duration_ms, 7, "Duration 7ms → duration_ms");
        assert_eq!(t.llm_round, 3);
        assert_eq!(
            t.arguments.get("command").and_then(|v| v.as_str()),
            Some("echo hi"),
            "HashMap arguments → JSON object value"
        );
        assert!(!t.timestamp.is_empty());
        // 附带分析也跑了（benign 命令 → guard Some 未拦）。
        let cg = t.findings.command_guard.as_ref().expect("findings attached");
        assert!(!cg.blocked);
    }

    #[tokio::test]
    async fn take_tags_drains_the_buffer() {
        let (_dir, obs) = observer_in_tmp_home();
        obs.on_event(tool_call_event("exec", None)).await;
        assert_eq!(obs.take_tags().len(), 1);
        assert!(obs.take_tags().is_empty(), "take_tags must drain (取尽)");
    }

    #[tokio::test]
    async fn multiple_events_accumulate_in_order() {
        let (_dir, obs) = observer_in_tmp_home();
        obs.on_event(tool_call_event("exec", None)).await;
        obs.on_event(tool_call_event("read_file", None)).await;
        let tags = obs.take_tags();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].tool_name, "exec");
        assert_eq!(tags[1].tool_name, "read_file");
    }
}

// ---------------------------------------------------------------------------
// 序列化形状 —— tool_trace.json 的落盘格式契约
// ---------------------------------------------------------------------------

#[test]
fn tool_tag_serializes_with_all_report_fields() {
    let tag = tag_with(LayerFindings {
        injection: Some(InjectionFinding {
            is_injection: false,
            score: 0.25,
            level: "low".to_string(),
        }),
        command_guard: None,
        credentials_in: None,
        credentials_out: None,
        dlp_in: None,
        dlp_out: None,
        ssrf: None,
    });
    let v = serde_json::to_value(&tag).expect("ToolTag serializes");
    for key in [
        "tool_name",
        "arguments",
        "result",
        "success",
        "duration_ms",
        "llm_round",
        "timestamp",
        "findings",
    ] {
        assert!(v.get(key).is_some(), "tool_trace.json field {key} missing");
    }
    let inj = v["findings"].get("injection");
    assert!(inj.is_some());
    assert_eq!(v["findings"]["injection"]["level"], "low");
}

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：run() 入口错误路径。
// 成功路径需要真 LLM（结构性，eval 命令的 e2e 在 S11b 批次覆盖）；
// 这里钉三段失败链：①env 未设 ②config.json 坏 ③agent 工厂失败——
// 全部要求 worker_error.txt 落盘（盒内 stderr 被 Start.exe 吞掉，这个
// 文件是唯一诊断线索）。
// =========================================================================

mod run_error_paths {
    use super::super::*;

    /// RAII：设置 NEMESISBOT_EVAL_WORKSPACE，Drop 恢复原值。
    /// env 变更必须持 crate::GLOBAL_STATE_LOCK 串行（调用侧 set 窗口持锁）。
    struct EvalEnvGuard(Option<String>);
    impl EvalEnvGuard {
        fn set_workspace(path: &std::path::Path) -> Self {
            let saved = std::env::var("NEMESISBOT_EVAL_WORKSPACE").ok();
            unsafe { std::env::set_var("NEMESISBOT_EVAL_WORKSPACE", path) };
            Self(saved)
        }
    }
    impl Drop for EvalEnvGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(v) => unsafe { std::env::set_var("NEMESISBOT_EVAL_WORKSPACE", v) },
                None => unsafe { std::env::remove_var("NEMESISBOT_EVAL_WORKSPACE") },
            }
        }
    }

    #[tokio::test]
    async fn run_err_when_workspace_env_missing() {
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        // 确保未设置（保存并删除；测完恢复）。
        let saved = std::env::var("NEMESISBOT_EVAL_WORKSPACE").ok();
        unsafe { std::env::remove_var("NEMESISBOT_EVAL_WORKSPACE") };

        let err = run().await.expect_err("missing env must fail");
        assert!(
            err.to_string().contains("NEMESISBOT_EVAL_WORKSPACE not set"),
            "err: {err:#}"
        );
        // env 未设 → 连 worker_error.txt 的落点都没有，只能报错。

        match saved {
            Some(v) => unsafe { std::env::set_var("NEMESISBOT_EVAL_WORKSPACE", v) },
            None => unsafe { std::env::remove_var("NEMESISBOT_EVAL_WORKSPACE") },
        }
    }

    #[tokio::test]
    async fn run_err_on_broken_config_writes_worker_error_and_alive_marker() {
        let tmp = tempfile::tempdir().unwrap();
        // 入口标记总会写（证明 worker main 到达过）。
        std::fs::write(tmp.path().join("config.json"), "{ not valid json").unwrap();

        // 锁必须横跨整个 run()：env 是进程级全局，并行测试互踩会让
        // worker_error.txt 写进别人的 workspace（本次失败根因）。
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _env = EvalEnvGuard::set_workspace(tmp.path());

        let err = run().await.expect_err("broken config must fail");
        assert!(
            format!("{err:#}").contains("load eval config"),
            "err: {err:#}"
        );
        // worker_alive.txt：入口标记（无论后续成败都写）。
        let alive = std::fs::read_to_string(tmp.path().join("worker_alive.txt"))
            .expect("entry marker must be written");
        assert!(alive.contains("pid="));
        // worker_error.txt：错误链落盘（盒内唯一诊断线索）。
        let werr = std::fs::read_to_string(tmp.path().join("worker_error.txt"))
            .expect("worker_error.txt must be written on error");
        assert!(werr.contains("load eval config"), "werr: {werr}");
    }

    #[tokio::test]
    async fn run_err_when_agent_factory_fails_writes_worker_error() {
        let tmp = tempfile::tempdir().unwrap();
        // config.json 合法但没有可用模型（load 回落默认 zhipu 无 key）→
        // 工厂在 provider 创建处失败（离线、快）。
        std::fs::write(tmp.path().join("config.json"), "{}").unwrap();

        // 锁必须横跨整个 run()：env 是进程级全局，并行测试互踩会让
        // worker_error.txt 写进别人的 workspace（本次失败根因）。
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _env = EvalEnvGuard::set_workspace(tmp.path());

        let err = run().await.expect_err("factory failure must propagate");
        assert!(
            format!("{err:#}").contains("build agent loop for eval-agent"),
            "err: {err:#}"
        );
        let werr = std::fs::read_to_string(tmp.path().join("worker_error.txt"))
            .expect("worker_error.txt must be written");
        assert!(werr.contains("build agent loop"), "werr: {werr}");
    }
}
