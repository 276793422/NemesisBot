//! eval_worker 单测：打点观察者 + 分层分析 + 汇总（纯逻辑，不进 Sandboxie）。
//!
//! `run()`/`run_inner()` 需要真实 agent loop + 盒内环境（结构性）；
//! 这里测的是观察者协议（on_event 过滤/记录/取尽）、`run_layers` 各层
//! 分析分支（用安全层引擎的真实现，输入用确定性的样例）、`summarize`
//! 汇总计数和 `ToolTag` 序列化形状。

// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

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

    // ── r9（覆盖率补测批 2026-08-27）：dlp 入站命中臂。既有 layers 夹具只
    //    钉过出站(dlp_out)与无命中两侧，scan_tool_input 的 has_matches=true
    //    路径（346 行）从未点亮——Luhn 通过的确定性卡号样本补上这块。
    #[tokio::test]
    async fn credit_card_number_in_args_flags_dlp_inbound() {
        let (_dir, obs) = observer_in_tmp_home();
        // 4111111111111111 是 Luhn 校验通过的标准样例卡号（DLP 引擎 confidence
        // 分级 + Luhn 校验后的确定性命中样本）。
        let args = serde_json::json!({"command": "echo card=4111111111111111"});
        let f = obs.run_layers("exec", &args, &args.to_string(), "").await;
        let hits = f.dlp_in.expect("Luhn-valid card in args must produce inbound DLP findings");
        assert!(!hits.is_empty(), "入站命中必须有 summary");
        assert!(!hits[0].is_empty(), "summary 不能是空串");
        // 空 result → 出站不产出（与入站对称）。
        assert!(f.dlp_out.is_none());
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

// =========================================================================
// 覆盖率补测 wave B（2026-08-27）：run_inner 成功链 + agent loop 错误映射
// + run_layers 补充分支 + Observer::name。
//
// - 成功链用进程内 mock LLM：std TcpListener 手写一发 HTTP/1.1 应答
//   （本 crate 无 wiremock dev-dep，也不允许为此改 Cargo.toml）。
//   HttpProvider POST {base}/chat/completions（base 不带 /v1），最小可解析
//   应答体仿 http_provider_extra_tests.rs 的 wiremock 夹具契约。
// - 拒绝端点统一用 127.0.0.1:1（连接立即拒绝、零网络外联）。
// - run_inner:95 block_in_place 在 current-thread 测试 runtime 会 panic →
//   run() 全链两个测试用多线程 flavor。
// - env 窗口持 crate::GLOBAL_STATE_LOCK；guard 复刻 run_error_paths 的
//   prev-value Option-match 恢复纪律（跨模块私有，故本地复刻）。
// =========================================================================
mod wave_b {
    use super::*;
    use nemesis_observer::Observer;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// RAII：设置 NEMESISBOT_EVAL_WORKSPACE，Drop 按 prev-value Option 恢复。
    struct WaveBEvalEnvGuard(Option<String>);
    impl WaveBEvalEnvGuard {
        fn set_workspace(path: &std::path::Path) -> Self {
            let saved = std::env::var("NEMESISBOT_EVAL_WORKSPACE").ok();
            unsafe { std::env::set_var("NEMESISBOT_EVAL_WORKSPACE", path) };
            Self(saved)
        }
    }
    impl Drop for WaveBEvalEnvGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(v) => unsafe { std::env::set_var("NEMESISBOT_EVAL_WORKSPACE", v) },
                None => unsafe { std::env::remove_var("NEMESISBOT_EVAL_WORKSPACE") },
            }
        }
    }

    /// 最小可解析 config.json：模型带【非 openai】provider 前缀（wbprov/），
    /// 工厂 resolve 落 HttpCompat(HttpProvider)，base_url 即给定 api_base、
    /// 端点 /chat/completions。⚠️ 不能用裸名：parse_model_ref 对无前缀名
    /// 默认 provider="openai" → 工厂映射 Codex（POST {base}/responses,
    /// Chat 补全格式解析为空 → 3 次空 final → turn_guard 放弃文案）。
    fn wave_b_write_llm_config(ws: &std::path::Path, api_base: &str) {
        let cfg = serde_json::json!({
            "agents": {"defaults": {"llm": "wbprov/waveb-probe"}},
            "model_list": [{
                "model_name": "waveb-probe",
                "model": "wbprov/waveb-probe",
                "api_key": "waveb-fake-key",
                "api_base": api_base,
            }],
        });
        std::fs::write(
            ws.join("config.json"),
            serde_json::to_string_pretty(&cfg).unwrap(),
        )
        .unwrap();
    }

    fn wave_b_header_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n")
    }

    fn wave_b_content_length(headers: &[u8]) -> usize {
        String::from_utf8_lossy(headers)
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                if !k.trim().eq_ignore_ascii_case("content-length") {
                    return None;
                }
                v.trim().parse::<usize>().ok()
            })
            .unwrap_or(0)
    }

    /// 进程内 mock LLM：任何请求都回一条 finish_reason=stop 的补全。
    /// accept 用 nonblocking+轮询以便 stop 置位后退出线程；served 上限兜底。
    fn wave_b_spawn_mock_llm() -> (String, Arc<AtomicBool>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::spawn(move || {
            let body = concat!(
                r#"{"choices":[{"index":0,"message":{"role":"assistant","#,
                r#""content":"waveb mock reply"},"finish_reason":"stop"}],"#,
                r#""usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}}"#,
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let mut served = 0usize;
            while served < 16 && !stop2.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        served += 1;
                        // 读完整请求头+体（Content-Length 口径）再应答，
                        // 防止大请求体下早答导致半读。
                        let mut buf = Vec::new();
                        let mut chunk = [0u8; 8192];
                        loop {
                            match stream.read(&mut chunk) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    buf.extend_from_slice(&chunk[..n]);
                                    if let Some(pos) = wave_b_header_end(&buf)
                                        && buf.len()
                                            >= pos + 4 + wave_b_content_length(&buf[..pos])
                                        {
                                            break;
                                        }
                                }
                            }
                        }
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.flush();
                        // Connection: close → drop(stream) 即断开。
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        (format!("http://{addr}"), stop)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_worker_writes_full_report_after_successful_llm_round() {
        let ws = tempfile::tempdir().unwrap();
        let (base, stop) = wave_b_spawn_mock_llm();
        wave_b_write_llm_config(ws.path(), &base);

        // env 为进程全局：锁横跨整个 run()（与 run_error_paths 同纪律，
        // 并行测试互踩会让报告写进别人的 workspace）。
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _env = WaveBEvalEnvGuard::set_workspace(ws.path());

        // 兜底护栏：单轮 mock 补全应秒级完成；超时=链路挂死直接失败。
        let res = tokio::time::timeout(std::time::Duration::from_secs(90), run()).await;
        stop.store(true, Ordering::Release);

        res.expect("run() 不得超时")
            .expect("成功 LLM 轮次 → run() Ok");

        // 入口标记在场（进入过 worker main）。
        let alive =
            std::fs::read_to_string(ws.path().join("worker_alive.txt")).expect("entry marker");
        assert!(alive.contains("pid="));

        // 三件报告齐活 + 内容正确性。
        let report = ws.path().join("logs").join("eval");
        let final_md = std::fs::read_to_string(report.join("final_response.md"))
            .expect("final_response.md 必须落盘");
        assert!(
            final_md.contains("waveb mock reply"),
            "最终回复应为 mock 内容原文: {final_md}"
        );

        let trace: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(report.join("tool_trace.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            trace.as_array().map(|a| a.len()),
            Some(0),
            "零工具调用 → tool_trace=[]"
        );

        let findings: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(report.join("security_findings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(findings["total_tool_calls"], 0);

        assert!(
            !ws.path().join("worker_error.txt").exists(),
            "成功路径不得写 worker_error.txt"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_worker_maps_agent_loop_llm_error_into_worker_error_txt() {
        let ws = tempfile::tempdir().unwrap();
        // 127.0.0.1:1 连接拒绝 → LLM 层瞬态重试烧尽 → Error 事件、无 Done →
        // process_direct Err → :119 map_err 成 "agent loop error: ..."。
        wave_b_write_llm_config(ws.path(), "http://127.0.0.1:1/v1");

        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _env = WaveBEvalEnvGuard::set_workspace(ws.path());

        let res = tokio::time::timeout(std::time::Duration::from_secs(90), run()).await;
        let err = res.expect("run() 不得超时").expect_err("LLM 全灭必须 Err");
        let msg = format!("{err:#}");
        assert!(msg.contains("agent loop error"), "err: {msg}");

        // run() 错误链必须落 worker_error.txt（盒内唯一诊断线索）。
        let werr = std::fs::read_to_string(ws.path().join("worker_error.txt"))
            .expect("worker_error.txt 必须落盘");
        assert!(werr.contains("agent loop error"), "werr: {werr}");
    }

    // ---------------------------------------------------------------------
    // Observer 协议 + run_layers 分支补充（layers/observer_protocol 空档）
    // ---------------------------------------------------------------------

    fn wave_b_observer_in_tmp_home() -> (tempfile::TempDir, EvalTaggingObserver) {
        let dir = tempfile::tempdir().expect("tempdir");
        let shared = Arc::new(crate::agent_factory::SharedResources {
            home: dir.path().to_path_buf(),
            ..Default::default()
        });
        (dir, EvalTaggingObserver::new(&shared))
    }

    #[test]
    fn wave_b_observer_name_is_eval_tagging() {
        let (_d, obs) = wave_b_observer_in_tmp_home();
        assert_eq!(obs.name(), "eval-tagging");
    }

    #[tokio::test]
    async fn wave_b_run_layers_credential_hit_in_tool_result_flags_outbound() {
        let (_d, obs) = wave_b_observer_in_tmp_home();
        let args = serde_json::json!({"command": "cat secrets.env"});
        // 结果带 AWS 访问键（credential/tests.rs 的确定性样例）→ 出站扫描命中，
        // credentials_out Some(vec![summary])（既有夹具只钉过入站/无命中两态）。
        let result = "The output is key=AKIAIOSFODNN7EXAMPLE123456";
        let f = obs.run_layers("exec", &args, &args.to_string(), result).await;
        let cout = f.credentials_out.expect("出站凭据命中必须有 finding");
        assert!(!cout.is_empty());
        // 入参干净 → 入站仍为 None（对称语义固定）。
        assert!(f.credentials_in.is_none());
    }

    #[tokio::test]
    async fn wave_b_run_layers_dlp_hit_in_tool_result_flags_dlp_outbound() {
        let (_d, obs) = wave_b_observer_in_tmp_home();
        let args = serde_json::json!({"command": "echo done"});
        // 结果带 Luhn 通过的 Visa 卡号（dlp/tests.rs 的确定性样例）→ dlp_out Some。
        let result = "Card: 4111111111111111";
        let f = obs.run_layers("exec", &args, &args.to_string(), result).await;
        let dout = f.dlp_out.expect("DLP 出站命中必须有 finding");
        assert!(!dout.is_empty());
    }

    #[tokio::test]
    async fn wave_b_run_layers_public_ip_literal_url_passes_ssrf_without_network() {
        let (_d, obs) = wave_b_observer_in_tmp_home();
        // 1.1.1.1 是公网字面量：非 loopback/metadata/private/link-local/
        // reserved 且 blocked_nets 默认空 → resolver.rs 对 IP 直判短路（不
        // 发 DNS 不建连接）→ validate_url Ok(())。既有夹具只钉过拦截面。
        let args = serde_json::json!({"url": "http://1.1.1.1/dns-query"});
        let f = obs.run_layers("web_fetch", &args, &args.to_string(), "").await;
        let s = f.ssrf.expect("url 字段 → ssrf finding 必产出");
        assert!(!s.blocked, "公网 IP 字面量不应拦: {}", s.reason);
        assert_eq!(s.url, "http://1.1.1.1/dns-query");
        assert!(s.reason.is_empty(), "Ok 臂 reason 为空串");
    }

    /// 一次 run_layers 同时点亮全部安全层引擎块（wave C 收口：分类定性轮
    /// 标出的各引擎 if-let 块尾部残余区域）——blocked 命令 + 入站凭据命中 +
    /// 出入站凭据/DLP + SSRF 拦截在同一调用内共存，注入层照常产出。
    #[tokio::test]
    async fn wave_b_run_layers_all_engine_blocks_report_findings_in_one_call() {
        let (_d, obs) = wave_b_observer_in_tmp_home();
        let args = serde_json::json!({
            "command": "rm -rf /",
            "url": "http://169.254.169.254/latest/meta-data"
        });
        // 入参串同时埋凭据（AWS 访问键样例）与出参埋凭据 + Luhn 通过的卡号
        // （credential/dlp tests 钉过的确定性样例口径）。
        let args_str = concat!(
            r#"{"command":"rm -rf /","url":"http://169.254.169.254/latest/meta-data","#,
            r#""secret":"AKIAIOSFODNN7EXAMPLE"}"#
        );
        let result = "key=AKIAIOSFODNN7EXAMPLE123456 Card: 4111111111111111";
        let f = obs.run_layers("exec", &args, args_str, result).await;

        // L1 注入：总是产出 finding 结构。
        assert!(f.injection.is_some(), "L1 注入层必须产出 finding");
        if let Some(inj) = &f.injection {
            // level 序列化契约（plan C2 修复）：小写字符串，非双重引号包装。
            assert!(!inj.level.starts_with('"'), "level 不得带字面引号: {}", inj.level);
        }
        // L2 command guard：rm -rf / 确定性拦截。
        let cg = f.command_guard.expect("command 字段 → guard finding");
        assert!(cg.blocked, "rm -rf / 必须拦");
        assert!(!cg.reason.is_empty());
        // L3 credentials：双向命中。
        let cin = f.credentials_in.expect("入站凭据命中");
        assert!(!cin.is_empty());
        let cout = f.credentials_out.expect("出站凭据命中");
        assert!(!cout.is_empty());
        // L5 SSRF：metadata 地址确定性拦截。
        let s = f.ssrf.expect("url 字段 → ssrf finding");
        assert!(s.blocked);
        // L4 DLP 出站（卡号）在此一并走通；入站方向布尔两臂均已由
        // 既有/上句夹具覆盖，此处不再加约束钉死检测器灵敏度。
        let dout = f.dlp_out.expect("出站 DLP 命中");
        assert!(!dout.is_empty());
    }
}
