//! exec_worker 测试：U11 用户态沙盒路径决策表（纯函数，跨平台）。
//! 真实行为断言（landlock 写外拒/写内放行）在 `tests/executor.rs` 的
//! Linux e2e（cfg linux）里。

// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;

#[cfg(feature = "sandbox")]
mod userland_plan {
    use super::userland::{Plan, plan};
    use nemesis_sandbox::backend::BackendForm;

    #[test]
    fn no_marker_runs_plain() {
        assert_eq!(
            plan(false, false, Some(BackendForm::SelfApply)),
            Plan::Plain
        );
        // 即使 bwrap 可用，没标记也不介入
        assert_eq!(
            plan(false, false, Some(BackendForm::WrapCommand)),
            Plan::Plain
        );
    }

    #[test]
    fn reexeced_instance_never_re_engages() {
        // 防环核心：盒内实例（REEXEC=1）见到标记也直接 Plain
        assert_eq!(plan(true, true, Some(BackendForm::SelfApply)), Plan::Plain);
        assert_eq!(
            plan(true, true, Some(BackendForm::WrapCommand)),
            Plan::Plain
        );
    }

    #[test]
    fn marker_with_no_backend_degrades_plain() {
        // 降级验收语义：无后端 → Plain（warn 由 engage 打），不 Err
        assert_eq!(plan(true, false, None), Plan::Plain);
    }

    #[test]
    fn marker_with_self_apply_backend_applies() {
        assert_eq!(
            plan(true, false, Some(BackendForm::SelfApply)),
            Plan::SelfApply
        );
    }

    #[test]
    fn marker_with_wrap_backend_reexeces() {
        assert_eq!(
            plan(true, false, Some(BackendForm::WrapCommand)),
            Plan::WrapReexec
        );
    }
}

// ---------------------------------------------------------------------------
// P5-2：engage 的严格模式改判（fail-closed）与默认 fail-open
// ---------------------------------------------------------------------------
// Windows 上 detect_backend() 恒 None（设计契约，Sandboxie 承担）→ engage
// 确定走 Plain 路径，strict 的 bail/不 bail 可确定性断言。**必须** cfg
// windows：Linux 上 detect_backend 可能返回 Some → SelfApply → engage 会
// 真对测试进程装 landlock（不可逆，污染同进程其他测试）。
#[cfg(all(feature = "sandbox", windows))]
mod engage_strict {
    use super::userland::{Outcome, engage};

    fn seed_home(strict: Option<bool>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = match strict {
            None => r#"{ "executor": { "enabled": true, "sandbox": true } }"#.to_string(),
            Some(s) => format!(
                r#"{{ "executor": {{ "enabled": true, "sandbox": true, "strict": {s} }} }}"#
            ),
        };
        std::fs::write(dir.path().join("config.json"), body).expect("seed config.json");
        dir
    }

    #[test]
    fn strict_refuses_when_no_backend() {
        let home = seed_home(Some(true));
        let err = engage("/ws", Some(home.path()), false).expect_err("strict must refuse");
        assert!(err.to_string().contains("strict mode"), "err: {err}");
        assert!(
            err.to_string().contains("refusing to run unsandboxed"),
            "err: {err}"
        );
    }

    #[test]
    fn no_strict_config_keeps_fail_open() {
        // 缺 strict 键（= 默认 false）：无后端 → warn + Continue（现状字节不变）
        let home = seed_home(None);
        assert!(matches!(
            engage("/ws", Some(home.path()), false),
            Ok(Outcome::Continue)
        ));
    }

    #[test]
    fn strict_false_keeps_fail_open() {
        let home = seed_home(Some(false));
        assert!(matches!(
            engage("/ws", Some(home.path()), false),
            Ok(Outcome::Continue)
        ));
    }

    #[test]
    fn no_home_defaults_fail_open() {
        // home=None（测试/裸构造）→ strict 读不到 → false → fail-open
        assert!(matches!(engage("/ws", None, false), Ok(Outcome::Continue)));
    }

    #[test]
    fn spawn_fenced_satisfies_strict_even_without_userland_backend() {
        // 三轮复查根修回归钉：workspace-dacl 受限令牌在 spawn 时已把内核写
        // 围栏装上——Plain 臂（无用户态后端可选）不得再触发 strict 拒绝。
        // 此前 dacl+strict+sandbox=false 组合会被「no userland sandbox
        // backend」虚假全拒（围栏明明活着）。
        let home = seed_home(Some(true));
        assert!(matches!(
            engage("/ws", Some(home.path()), true),
            Ok(Outcome::Continue)
        ));
    }
}

// ---------------------------------------------------------------------------
// serve_line 协议主体（一行 JSON 请求 → 一行响应）——纯逻辑 + mock 工具 +
// 禁用 journal（per-call 形态），不 spawn 任何进程。stdio/pipe 两个 transport
// loop 只是对 serve_line 的包装，它们本身需要真 stdio/管道（结构性，见
// tests/executor.rs 的真链路 e2e）。journal 语义见下方 serve_journal。
// ---------------------------------------------------------------------------
mod dispatch_protocol {
    use super::*;
    use crate::exec_worker::journal::Journal;
    use nemesis_agent::context::RequestContext;
    use nemesis_agent::r#loop::Tool;
    use std::collections::HashMap;

    /// serve_line 测试壳：禁用 journal → 去换行 → 解析 JSON。
    async fn serve(tools: &HashMap<String, Box<dyn Tool>>, line: &str) -> serde_json::Value {
        let mut j = Journal::disabled();
        let out = serve_line(line, tools, &mut j).await;
        serde_json::from_str(out.trim_end_matches(['\n', '\r'])).expect("response line json")
    }

    /// 恒 Ok 的哑工具：回显 args + 记录收到的 args/ctx 供透传断言。
    struct EchoTool {
        seen: std::sync::Mutex<Vec<(String, String, String, String)>>,
    }

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        async fn execute(&self, args: &str, ctx: &RequestContext) -> Result<String, String> {
            self.seen.lock().unwrap().push((
                args.to_string(),
                ctx.channel.clone(),
                ctx.user.clone(),
                ctx.session_key.clone(),
            ));
            Ok(format!("ok:{args}"))
        }
    }

    /// 恒 Err 的哑工具。
    struct BoomTool;

    #[async_trait::async_trait]
    impl Tool for BoomTool {
        async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
            Err("boom".to_string())
        }
    }

    fn registry_with_echo() -> (HashMap<String, Box<dyn Tool>>, std::sync::Arc<EchoTool>) {
        let recorder = std::sync::Arc::new(EchoTool {
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let mut m: HashMap<String, Box<dyn Tool>> = HashMap::new();
        // 通过句柄转发：注册表里放一个引用同一记录端的轻量包装。
        struct FwdTool(std::sync::Arc<EchoTool>);
        #[async_trait::async_trait]
        impl Tool for FwdTool {
            async fn execute(&self, args: &str, ctx: &RequestContext) -> Result<String, String> {
                self.0.execute(args, ctx).await
            }
        }
        m.insert("echo".to_string(), Box::new(FwdTool(recorder.clone())));
        m.insert("boom".to_string(), Box::new(BoomTool));
        (m, recorder)
    }

    const CTX_JSON: &str = r#"{"channel":"web","chat_id":"c1","user":"u1","session_key":"s1"}"#;

    #[tokio::test]
    async fn malformed_line_is_rejected_without_panicking() {
        let (tools, _) = registry_with_echo();
        for bad in ["", "not json", "{", r#"{"tool": 123}"#, "[]"] {
            let v = serve(&tools, bad).await;
            assert_eq!(v["ok"], serde_json::json!(false), "line {bad:?}: {v}");
            assert!(
                v["error"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("bad request line"),
                "line {bad:?}: {v}"
            );
            assert_eq!(v["result"], "", "line {bad:?}: {v}");
        }
    }

    #[tokio::test]
    async fn bad_context_field_is_reported_as_context_error() {
        let (tools, _) = registry_with_echo();
        // tool/args 合法但 context 不是 RequestContext 形状（类型错）。
        let line = r#"{"tool":"echo","args":"","context":123}"#;
        let v = serve(&tools, line).await;
        assert_eq!(v["ok"], serde_json::json!(false), "{v}");
        assert!(
            v["error"].as_str().unwrap_or("").starts_with("bad context"),
            "{v}"
        );
    }

    #[tokio::test]
    async fn missing_context_required_fields_is_context_error() {
        let (tools, _) = registry_with_echo();
        // context 是 JSON 对象但缺 required 字段（channel 等 String 无 default）。
        let line = r#"{"tool":"echo","args":"","context":{"channel":"web"}}"#;
        let v = serve(&tools, line).await;
        assert_eq!(v["ok"], serde_json::json!(false), "{v}");
        assert!(
            v["error"].as_str().unwrap_or("").starts_with("bad context"),
            "{v}"
        );
    }

    #[tokio::test]
    async fn unknown_tool_is_rejected_by_name() {
        let (tools, _) = registry_with_echo();
        let line = format!(r#"{{"tool":"nope","args":"","context":{CTX_JSON}}}"#);
        let v = serve(&tools, &line).await;
        assert_eq!(v["ok"], serde_json::json!(false), "{v}");
        assert_eq!(v["error"], "unknown tool: nope", "{v}");
    }

    #[tokio::test]
    async fn ok_tool_result_round_trips_with_args_passthrough() {
        let (tools, echo) = registry_with_echo();
        let line = format!(r#"{{"tool":"echo","args":"{{\"x\":1}}","context":{CTX_JSON}}}"#);
        let v = serve(&tools, &line).await;
        assert_eq!(v["ok"], serde_json::json!(true), "{v}");
        assert_eq!(v["result"], r#"ok:{"x":1}"#, "{v}");
        assert_eq!(v["error"], "", "{v}");
        let seen = echo.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "tool executed exactly once");
        assert_eq!(seen[0].0, r#"{"x":1}"#, "args passed through verbatim");
        assert_eq!(seen[0].1, "web", "context.channel reconstructed");
        assert_eq!(seen[0].2, "u1", "context.user reconstructed");
        assert_eq!(seen[0].3, "s1", "context.session_key reconstructed");
    }

    #[tokio::test]
    async fn err_tool_maps_to_error_response() {
        let (tools, _) = registry_with_echo();
        let line = format!(r#"{{"tool":"boom","args":"","context":{CTX_JSON}}}"#);
        let v = serve(&tools, &line).await;
        assert_eq!(v["ok"], serde_json::json!(false), "{v}");
        assert_eq!(v["error"], "boom", "{v}");
        assert_eq!(v["result"], "", "{v}");
    }

    #[tokio::test]
    async fn optional_context_fields_default_cleanly() {
        // correlation_id 缺省 → None；async_callback serde(skip) → None（重建语义）。
        let (tools, echo) = registry_with_echo();
        let line = r#"{"tool":"echo","args":"a","context":{"channel":"rpc","chat_id":"c","user":"u","session_key":"s","correlation_id":"corr-42"}}"#;
        let v = serve(&tools, line).await;
        assert_eq!(v["ok"], serde_json::json!(true), "{v}");
        let seen = echo.seen.lock().unwrap();
        assert_eq!(seen[0].1, "rpc");
    }
}

// =========================================================================
// P3 T2：serve_line × journal 幂等语义（协议主体层）。
// - v2 请求：响应回显 v=2 + call_id，dispatch 结果记账；重发同 call_id =
//   replay 命中（工具不重执行、结果逐字节一致）
// - v1 请求：响应保持 v1 字节形态（无 v/call_id 键），journal 不记空 id
// - 崩溃重建形态：预置历史账（上一进程记录）→ replay 命中不执行工具
// =========================================================================
mod serve_journal {
    use super::*;
    use crate::exec_worker::journal::Journal;
    use nemesis_agent::context::RequestContext;
    use nemesis_agent::executor_transport::{ExecutorResponseWire, PROTOCOL_V2};
    use nemesis_agent::r#loop::Tool;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// 计数工具：每次真实执行 +1（replay 命中不得执行——结果停在 run-1）。
    struct CountTool(Mutex<u32>);

    #[async_trait::async_trait]
    impl Tool for CountTool {
        async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
            let mut n = self.0.lock().unwrap();
            *n += 1;
            Ok(format!("run-{n}"))
        }
    }

    fn registry() -> HashMap<String, Box<dyn Tool>> {
        let mut m: HashMap<String, Box<dyn Tool>> = HashMap::new();
        m.insert("count".to_string(), Box::new(CountTool(Mutex::new(0))));
        m
    }

    const CTX: &str = r#"{"channel":"web","chat_id":"c1","user":"u1","session_key":"s1"}"#;

    fn v2_req(call_id: &str) -> String {
        format!(r#"{{"v":2,"call_id":"{call_id}","tool":"count","args":"{{}}","context":{CTX}}}"#)
    }

    #[tokio::test]
    async fn v2_response_echoes_call_id_and_journals_execution() {
        let tools = registry();
        let ws = tempfile::tempdir().unwrap();
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");
        let req = v2_req("c-7-1");

        let out1 = serve_line(&req, &tools, &mut j).await;
        let v1: serde_json::Value = serde_json::from_str(out1.trim()).unwrap();
        assert_eq!(v1["v"], 2, "{v1}");
        assert_eq!(v1["call_id"], "c-7-1", "{v1}");
        assert_eq!(v1["result"], "run-1", "{v1}");
        // 执行结果已入账（at-most-once 的前提）。
        assert!(j.lookup("c-7-1").is_some(), "dispatch 结果必须记账");

        // 重发同 call_id：replay 命中——逐字节一致、工具不重执行。
        let out2 = serve_line(&req, &tools, &mut j).await;
        assert_eq!(out1, out2, "replay 必须回缓存行");
        let v2: serde_json::Value = serde_json::from_str(out2.trim()).unwrap();
        assert_eq!(v2["result"], "run-1", "重发不得重执行（否则 run-2）: {v2}");
    }

    #[tokio::test]
    async fn v1_request_response_stays_v1_and_journal_stays_untouched() {
        let tools = registry();
        let ws = tempfile::tempdir().unwrap();
        // journal 启用也要对 v1（无 call_id）保持零记账——per-call 字节不变。
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");
        let req = format!(r#"{{"tool":"count","args":"{{}}","context":{CTX}}}"#);

        let out = serve_line(&req, &tools, &mut j).await;
        assert!(!out.contains("\"v\""), "v1 响应不得带 v 键: {out}");
        assert!(!out.contains("call_id"), "v1 响应不得带 call_id: {out}");
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["result"], "run-1", "{v}");
        // 空 call_id 不入账：journal 文件根本不落。
        assert!(
            !ws.path().join(".runtime/journal.jsonl").exists(),
            "v1 调用不得记账"
        );
    }

    #[tokio::test]
    async fn replay_hit_serves_cached_line_without_executing() {
        let tools = registry();
        let ws = tempfile::tempdir().unwrap();
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");
        // 预置历史账（模拟崩溃前进程完成的调用）。
        let cached = serde_json::to_string(&ExecutorResponseWire {
            v: PROTOCOL_V2,
            call_id: "c-8-1".into(),
            ok: true,
            result: "from-dead-process".into(),
            error: String::new(),
        })
        .unwrap();
        j.record(&cached);

        let out = serve_line(&v2_req("c-8-1"), &tools, &mut j).await;
        assert_eq!(out.trim(), cached, "必须原样回缓存行");
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["result"], "from-dead-process", "{v}");
    }

    #[tokio::test]
    async fn v2_error_response_also_echoes_call_id() {
        // v2 请求打未知工具：错误臂也必须回显 call_id（gateway 侧
        // read_response 按它做匹配校验，缺回显会被判 protocol 串话）。
        let mut m: HashMap<String, Box<dyn Tool>> = HashMap::new();
        m.insert("count".to_string(), Box::new(CountTool(Mutex::new(0))));
        let tools = m;
        let ws = tempfile::tempdir().unwrap();
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");
        let req =
            format!(r#"{{"v":2,"call_id":"c-9-1","tool":"nope","args":"{{}}","context":{CTX}}}"#);
        let out = serve_line(&req, &tools, &mut j).await;
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["v"], 2, "{v}");
        assert_eq!(v["call_id"], "c-9-1", "{v}");
        assert_eq!(v["error"], "unknown tool: nope", "{v}");
    }

    // -----------------------------------------------------------------
    // R2 两相账 replay 分类（2026-10-18 三方复评）。
    // -----------------------------------------------------------------

    fn v2_req_tool(call_id: &str, tool: &str) -> String {
        format!(r#"{{"v":2,"call_id":"{call_id}","tool":"{tool}","args":"{{}}","context":{CTX}}}"#)
    }

    /// 预置盘上意图行（模拟崩溃前进程写了 intent、没来得及写 done）。
    fn preset_intent(ws: &std::path::Path, call_id: &str, tool: &str) {
        let p = ws.join(super::super::journal::JOURNAL_REL);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .unwrap();
        writeln!(
            f,
            r#"{{"kind":"intent","call_id":"{call_id}","tool":"{tool}"}}"#
        )
        .unwrap();
    }

    fn registry_named(name: &str) -> HashMap<String, Box<dyn Tool>> {
        let mut m: HashMap<String, Box<dyn Tool>> = HashMap::new();
        m.insert(name.to_string(), Box::new(CountTool(Mutex::new(0))));
        m
    }

    /// 只读白名单工具的未决意图 → **安全重执行**（run-1），执行后落完成账、
    /// 意图了结；重发同 call_id = replay 缓存（不二次执行）。
    #[tokio::test]
    async fn pending_intent_readonly_tool_reexecutes_then_lands_in_done_ledger() {
        let tools = registry_named("read_file");
        let ws = tempfile::tempdir().unwrap();
        preset_intent(ws.path(), "c-ro-1", "read_file");
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");
        assert_eq!(j.pending_intent("c-ro-1"), Some("read_file"));

        let req = v2_req_tool("c-ro-1", "read_file");
        let out1 = serve_line(&req, &tools, &mut j).await;
        let v: serde_json::Value = serde_json::from_str(out1.trim()).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "{v}");
        assert_eq!(v["result"], "run-1", "只读白名单必须重执行: {v}");
        assert!(j.lookup("c-ro-1").is_some(), "执行结果落完成账");
        assert_eq!(j.pending_intent("c-ro-1"), None, "完成准入了结意图");
        // 重发 = replay 缓存（不二次执行 → 停在 run-1）。
        let out2 = serve_line(&req, &tools, &mut j).await;
        assert_eq!(out1, out2, "重发必须回缓存行");
    }

    /// 非白名单工具的未决意图 → 诚实「结果未知」错误（工具**不执行**），
    /// 错误记为完成账；重发同 call_id = 字节一致回缓存错误；跨重开同样
    /// replay（不重复尝试；新 call_id = 模型显式再选）。
    #[tokio::test]
    async fn pending_intent_non_readonly_returns_honest_unknown_and_records_done() {
        let tools = registry_named("exec");
        let ws = tempfile::tempdir().unwrap();
        preset_intent(ws.path(), "c-uk-1", "exec");
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");
        assert_eq!(j.pending_intent("c-uk-1"), Some("exec"));

        let req = v2_req_tool("c-uk-1", "exec");
        let out1 = serve_line(&req, &tools, &mut j).await;
        let v: serde_json::Value = serde_json::from_str(out1.trim()).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "{v}");
        assert_eq!(v["v"], 2, "{v}");
        assert_eq!(v["call_id"], "c-uk-1", "{v}");
        let err = v["error"].as_str().unwrap_or("");
        assert!(
            err.contains("结果未知") && err.contains("勿盲重试") && err.contains("exec"),
            "诚实错误必须说清 outcome-unknown + 工具名: {err}"
        );
        // 工具未执行（执行了 result 会是 run-1 而错误行 result 恒空）。
        assert_eq!(v["result"], "", "{v}");
        // 错误已记为完成账 + 意图了结。
        assert!(j.lookup("c-uk-1").is_some(), "诚实错误必须记为完成账");
        assert_eq!(j.pending_intent("c-uk-1"), None);
        // 重发 = 字节一致回缓存错误。
        let out2 = serve_line(&req, &tools, &mut j).await;
        assert_eq!(out1, out2, "重发必须回缓存错误（不重复尝试）");
        // 跨重开（崩溃重建形态）同样 replay 缓存错误。
        let mut j2 = Journal::with_agent_id(ws.path(), "agent-a");
        let out3 = serve_line(&req, &tools, &mut j2).await;
        assert_eq!(out1, out3, "跨重开必须 replay 缓存错误");
    }

    /// 新调用两相账顺序：意图行先落、完成行后落；完成后意图不再未决。
    #[tokio::test]
    async fn fresh_call_writes_intent_then_done() {
        let tools = registry();
        let ws = tempfile::tempdir().unwrap();
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");
        let out = serve_line(&v2_req("c-fresh-1"), &tools, &mut j).await;
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["result"], "run-1", "{v}");
        assert_eq!(j.pending_intent("c-fresh-1"), None, "完成后意图了结");
        let raw =
            std::fs::read_to_string(ws.path().join(super::super::journal::JOURNAL_REL)).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 2, "两相账 = 意图行 + 完成行: {raw}");
        assert!(
            lines[0].contains(r#""kind":"intent""#) && lines[0].contains("c-fresh-1"),
            "第一行必须是意图行: {}",
            lines[0]
        );
        assert!(
            lines[1].contains(r#""result":"run-1""#),
            "第二行必须是完成行: {}",
            lines[1]
        );
    }

    // -----------------------------------------------------------------
    // F2（二轮复评 2026-10-18）：意图未可靠落盘 → 有副作用工具拒绝 dispatch。
    // -----------------------------------------------------------------

    /// 意图落盘失败（`.runtime` 被文件占位）+ 非只读工具 → 诚实拒绝，工具
    /// **不执行**（result 恒空，执行了会是 run-1）；拒绝记为完成账（进程内
    /// replay），重发同 call_id = 字节一致回缓存错误。
    #[tokio::test]
    async fn intent_not_durable_non_readonly_tool_refused_without_executing() {
        let tools = registry_named("exec");
        let ws = tempfile::tempdir().unwrap();
        let blocker = ws.path().join(".runtime");
        std::fs::write(&blocker, b"not a dir").unwrap();
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");

        let req = v2_req_tool("c-nd-1", "exec");
        let out = serve_line(&req, &tools, &mut j).await;
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "{v}");
        assert_eq!(v["v"], 2, "{v}");
        assert_eq!(v["call_id"], "c-nd-1", "{v}");
        let err = v["error"].as_str().unwrap_or("");
        assert!(
            err.contains("意图落盘失败") && err.contains("拒绝执行") && err.contains("新 call_id"),
            "诚实错误必须说清拒绝原因与重试方式: {err}"
        );
        // 工具未执行（执行了 result 会是 run-1 而错误行 result 恒空）。
        assert_eq!(v["result"], "", "{v}");
        // 拒绝记为完成账（内存 admit）→ 重发回缓存错误，不重复尝试。
        assert!(j.lookup("c-nd-1").is_some(), "拒绝必须记为完成账");
        let out2 = serve_line(&req, &tools, &mut j).await;
        assert_eq!(out, out2, "重发必须回缓存错误（不重复尝试）");
    }

    /// 意图落盘失败 + 只读白名单工具 → 照常执行（可用性取舍：读不改状态，
    /// 最坏 = 重启后重读一次）。
    #[tokio::test]
    async fn intent_not_durable_readonly_tool_still_executes() {
        let tools = registry_named("read_file");
        let ws = tempfile::tempdir().unwrap();
        let blocker = ws.path().join(".runtime");
        std::fs::write(&blocker, b"not a dir").unwrap();
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");

        let out = serve_line(&v2_req_tool("c-nd-2", "read_file"), &tools, &mut j).await;
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["ok"], serde_json::json!(true), "{v}");
        assert_eq!(v["result"], "run-1", "只读白名单在账本故障时必须放行: {v}");
    }

    /// 存储修复后恢复执行：拒绝发生在存储故障窗口内；修复 + 新实例（等价
    /// 重启）→ 新 call_id 的非只读工具照常 dispatch（sticky 只限本实例）。
    #[tokio::test]
    async fn storage_repair_restores_non_readonly_dispatch() {
        let tools = registry_named("exec");
        let ws = tempfile::tempdir().unwrap();
        let blocker = ws.path().join(".runtime");
        std::fs::write(&blocker, b"not a dir").unwrap();
        let mut j = Journal::with_agent_id(ws.path(), "agent-a");
        let out = serve_line(&v2_req_tool("c-nd-3", "exec"), &tools, &mut j).await;
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "{v}");
        assert_eq!(v["result"], "", "故障窗口内不得执行: {v}");

        // 修复存储 + 新 journal 实例（等价执行体重启）。
        std::fs::remove_file(&blocker).unwrap();
        let mut j2 = Journal::with_agent_id(ws.path(), "agent-a");
        let out2 = serve_line(&v2_req_tool("c-nd-4", "exec"), &tools, &mut j2).await;
        let v2: serde_json::Value = serde_json::from_str(out2.trim()).unwrap();
        assert_eq!(v2["ok"], serde_json::json!(true), "{v2}");
        assert_eq!(v2["result"], "run-1", "修复后非只读工具必须恢复执行: {v2}");
    }
}

/// run() 前置守卫：workspace env 缺失必须干净报错（而不是进 stdio 循环挂死）。
/// GLOBAL_STATE_LOCK：remove_var 是进程级环境操作，与其它 env 测试互斥。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
async fn run_requires_workspace_env() {
    let _guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    unsafe {
        std::env::remove_var("NEMESISBOT_EXECUTOR_WORKSPACE");
    }
    let err = run()
        .await
        .expect_err("missing workspace env must fail fast");
    assert!(
        err.to_string().contains("NEMESISBOT_EXECUTOR_WORKSPACE"),
        "err: {err:#}"
    );
}

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：executor_main 粘合层。
// - stdio 传输：无 PIPE/无 SANDBOX 标记 → register_shared_tools + stdio_loop
//   （cargo test 的 stdin 是管道 EOF → 立即干净退出）。
// - 沙盒标记：fail-open（warn+Continue）与 P5-2 严格模式（engage Err →
//   emit_error_response 一行协议错误 + Ok 退出，不读 stdin）。
// - Windows 具名管道传输：run() 全链（server 假 gateway 一来一回 + EOF）。
// =========================================================================

// 整 mod Windows 形态（3/3 测试全走 Windows 进程边界；sandbox 项另有
// feature 双门控，随 mod 一并消失于 Linux）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod executor_main_glue {
    use super::*;

    /// env RAII：设置一组 executor env，Drop 全清（测间互不泄漏）。
    struct ExecEnvGuard {
        vars: Vec<&'static str>,
    }
    impl ExecEnvGuard {
        fn set(workspace: &std::path::Path) -> Self {
            let mut vars = vec![
                "NEMESISBOT_EXECUTOR_WORKSPACE",
                "NEMESISBOT_EXECUTOR_SANDBOX",
                "NEMESISBOT_EXECUTOR_REEXEC",
                "NEMESISBOT_EXECUTOR_HOME",
            ];
            if cfg!(windows) {
                vars.push("NEMESISBOT_EXECUTOR_PIPE");
            }
            unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_WORKSPACE", workspace) };
            for v in vars.iter().skip(1) {
                unsafe { std::env::remove_var(v) };
            }
            Self { vars }
        }
        // Only the sandbox+windows tests below call these; gate them with the
        // same cfg so other targets don't see them as dead code
        // (Linux CI clippy runs with -D warnings).
        #[cfg(all(feature = "sandbox", windows))]
        fn set_marker_and_home(&mut self, home: &std::path::Path) {
            unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_SANDBOX", "1") };
            unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_HOME", home) };
        }
    }
    impl Drop for ExecEnvGuard {
        fn drop(&mut self) {
            for v in self.vars.iter() {
                unsafe { std::env::remove_var(v) };
            }
        }
    }

    #[cfg(all(feature = "sandbox", windows))]
    fn config_with(strict: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("config.json"),
            format!(
                r#"{{ "executor": {{ "enabled": true, "sandbox": true, "strict": {strict} }} }}"#
            ),
        )
        .expect("seed config");
        dir
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn run_stdio_transport_registers_tools_and_exits_on_eof() {
        let _guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let _env = ExecEnvGuard::set(tmp.path());

        // stdio（无 PIPE）+ 无沙盒标记：注册共享工具集后 stdin EOF → Ok。
        run()
            .await
            .expect("stdio transport must exit cleanly on EOF");
    }

    #[cfg(all(feature = "sandbox", windows))]
    #[tokio::test]
    async fn run_with_sandbox_marker_fail_open_continues_to_stdio() {
        let _guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = config_with(false);
        let mut env = ExecEnvGuard::set(tmp.path());
        env.set_marker_and_home(home.path());

        // Windows 无用户态后端 + strict=false → warn + Continue → stdio EOF Ok。
        run()
            .await
            .expect("fail-open marker must continue to the tool loop");
    }

    #[cfg(all(feature = "sandbox", windows))]
    #[tokio::test]
    async fn run_with_sandbox_marker_strict_emits_error_line_and_exits_ok() {
        let _guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = config_with(true);
        let mut env = ExecEnvGuard::set(tmp.path());
        env.set_marker_and_home(home.path());

        // P5-2：engage Err（严格模式拒绝）→ emit_error_response（stdout 一行
        // 协议错误，libtest 捕获）→ Ok(()) —— 不读 stdin、不挂死。
        run()
            .await
            .expect("engage failure must exit Ok after emitting the error line");
    }
}

/// Windows 具名管道传输：run() 全链（gateway 侧假 server 一来一回 + EOF 退出）。
#[cfg(windows)]
mod pipe_transport_via_run {
    use super::*;
    use nemesis_agent::executor_pipe::{create_server, pipe_name, unique_pipe_id};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_over_named_pipe_round_trip_then_clean_eof_exit() {
        let _guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let name = pipe_name(&unique_pipe_id());
        let mut server = create_server(&name).expect("create pipe server");

        unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_WORKSPACE", tmp.path()) };
        unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_PIPE", &name) };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_SANDBOX") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_REEXEC") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_HOME") };

        // run() 在专用线程里自建 current_thread runtime（与生产形态一致）。
        let worker = std::thread::Builder::new()
            .name("t-exec-pipe-worker".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("worker runtime");
                rt.block_on(run())
            })
            .expect("spawn worker thread");

        // gateway 侧：等子进程连上 → 发一行请求 → 读一行响应。
        server.connect().await.expect("server side connect");
        let req = r#"{"tool":"__no_such_tool__","args":"{}","context":{"channel":"web","chat_id":"c1","user":"u1","session_key":"s1"}}"#;
        server
            .write_all(format!("{req}\n").as_bytes())
            .await
            .expect("write request");
        server.flush().await.expect("flush request");

        let mut reader = BufReader::new(&mut server).lines();
        let resp_line = reader
            .next_line()
            .await
            .expect("read response")
            .expect("response line present");
        let v: serde_json::Value = serde_json::from_str(&resp_line).expect("response json");
        assert_eq!(v["ok"], false, "unknown tool must fail: {resp_line}");
        assert!(
            v["error"].as_str().unwrap_or("").contains("unknown tool"),
            "resp: {resp_line}"
        );

        // gateway 关管道 → 子进程 EOF → run() 干净退出 Ok。
        drop(reader);
        drop(server);
        let res = worker.join().expect("worker join");
        res.expect("pipe transport run must exit Ok on EOF");

        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_PIPE") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_WORKSPACE") };
    }
}

// =========================================================================
// wave_b（覆盖率补测 2026-08-27）：pipe_loop 的读错误臂（exec_worker.rs:358）
// ——tokio lines() 对非 UTF-8 行返回 InvalidData → pipe_loop 上抛
// "pipe read: ..."（区别于 Ok(None) 的干净 EOF 退出）。沿用上面同一套
// 具名管道 + worker 线程夹具；全程零子进程、零生产端口。
// =========================================================================
#[cfg(windows)]
mod wave_b {
    use super::*;
    use nemesis_agent::executor_pipe::{create_server, pipe_name, unique_pipe_id};
    use tokio::io::AsyncWriteExt;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_pipe_read_invalid_utf8_line_surfaces_error() {
        let _guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let name = pipe_name(&unique_pipe_id());
        let mut server = create_server(&name).expect("create pipe server");

        unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_WORKSPACE", tmp.path()) };
        unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_PIPE", &name) };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_SANDBOX") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_REEXEC") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_HOME") };

        // run() 在专用线程里自建 current_thread runtime（与生产形态一致）。
        let worker = std::thread::Builder::new()
            .name("t-exec-pipe-badutf8".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("worker runtime");
                rt.block_on(run())
            })
            .expect("spawn worker thread");

        // 假 gateway 写入一行带非法 UTF-8 字节的请求（以 \n 结尾触发行解码）：
        // lines() 解码失败 → Err(InvalidData) → pipe_loop 的 pipe-read 错误臂。
        server.connect().await.expect("server side connect");
        let bad: Vec<u8> = b"\xff\xfe not-utf8 \x80 payload\n".to_vec();
        server
            .write_all(&bad)
            .await
            .expect("write invalid utf8 line");
        server.flush().await.expect("flush");
        drop(server); // 关管道，别让服务端悬着

        let res = worker.join().expect("worker join");
        let err = res.expect_err("非 UTF-8 行必须让 pipe loop 以错误退出");
        assert!(
            err.to_string().contains("pipe read"),
            "必须是 pipe read 错误臂: {err:#}"
        );

        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_PIPE") };
        unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_WORKSPACE") };
    }
}

// =========================================================================
// P21（2026-09-25）：沙盒拒绝台账钩子（sandbox_denial 模块）。
// - active_backend_label：无沙盒 None / PIPE 单独存在不算标签 / env→后端名
// - on_tool_error：无沙盒不改写不记账；非拒绝错误透传；拒绝错误 → 可自纠
//   文案 + 台账一行合法 JSON（workspace 取自 env）
// 全程零子进程；env 操作走进程级 GLOBAL_STATE_LOCK 串行。
// =========================================================================
#[cfg(feature = "sandbox")]
mod sandbox_denial_hooks {
    use crate::exec_worker::sandbox_denial;

    /// env RAII：清掉三个干扰键，Drop 时恢复原状（持 GLOBAL_STATE_LOCK 使用）。
    struct DenialEnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }
    impl DenialEnvGuard {
        fn fresh() -> Self {
            let keys = [
                "NEMESISBOT_EXECUTOR_PIPE",
                "NEMESISBOT_SANDBOX_BACKEND",
                "NEMESISBOT_EXECUTOR_WORKSPACE",
            ];
            let mut saved = Vec::new();
            for k in keys {
                saved.push((k, std::env::var_os(k)));
                unsafe { std::env::remove_var(k) };
            }
            Self { saved }
        }
    }
    impl Drop for DenialEnvGuard {
        fn drop(&mut self) {
            for (k, v) in self.saved.drain(..) {
                unsafe {
                    if let Some(v) = v {
                        std::env::set_var(k, v);
                    } else {
                        std::env::remove_var(k);
                    }
                }
            }
        }
    }

    fn with_lock<T>(f: impl FnOnce() -> T) -> T {
        let _g = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        f()
    }

    /// ENGAGED_BACKEND 是进程级 OnceLock（glue 用例在 Linux 上 engage 成功会
    /// 全局占坑且不可重置）——占坑后 label 恒 Some，env 路径断言无法隔离，
    /// 诚实跳过（Windows 上 OnceLock 恒空，断言完整生效）。
    fn oncelock_already_engaged() -> bool {
        let _env = DenialEnvGuard::fresh();
        sandbox_denial::active_backend_label().is_some()
    }

    #[test]
    fn label_is_none_without_sandbox_env() {
        with_lock(|| {
            if oncelock_already_engaged() {
                eprintln!("skip: ENGAGED_BACKEND OnceLock occupied by earlier test");
                return;
            }
            let _env = DenialEnvGuard::fresh();
            assert!(sandbox_denial::active_backend_label().is_none());
        });
    }

    #[test]
    fn label_reads_backend_env_pipe_alone_is_not_a_label() {
        with_lock(|| {
            if oncelock_already_engaged() {
                eprintln!("skip: ENGAGED_BACKEND OnceLock occupied by earlier test");
                return;
            }
            let _env = DenialEnvGuard::fresh();
            // PIPE 单独存在 = 无沙盒（冒标修复：PIPE 只是传输通道，无盒
            // PIPE 传输不得推断出 "sandboxie"——真盒由 gateway 注入 backend
            // env 才算 engaged）。
            unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_PIPE", r"\\.\pipe\NemesisBox_1") };
            assert!(sandbox_denial::active_backend_label().is_none());
            // 真盒（gateway wrap 注入 backend env）→ sandboxie 标签生效。
            unsafe { std::env::set_var("NEMESISBOT_SANDBOX_BACKEND", "sandboxie") };
            assert_eq!(
                sandbox_denial::active_backend_label().as_deref(),
                Some("sandboxie"),
                "env 注入的后端名（真盒 wrap / bwrap 盒内实例路径）"
            );
            unsafe { std::env::remove_var("NEMESISBOT_EXECUTOR_PIPE") };
            unsafe { std::env::set_var("NEMESISBOT_SANDBOX_BACKEND", "landlock") };
            assert_eq!(
                sandbox_denial::active_backend_label().as_deref(),
                Some("landlock"),
                "env 注入的后端名（bwrap 盒内实例路径）"
            );
            // "none" 视作未 engaged（不记账）
            unsafe { std::env::set_var("NEMESISBOT_SANDBOX_BACKEND", "none") };
            assert!(sandbox_denial::active_backend_label().is_none());
        });
    }

    #[test]
    fn on_tool_error_passthrough_without_backend_or_denial_shape() {
        with_lock(|| {
            let _env = DenialEnvGuard::fresh();
            // 无后端：错误原样透传（普通工具错误与沙盒无关）。Linux 上
            // OnceLock 被 glue 用例占坑时 label 恒 Some，但该错误不像拒绝
            // （os error 2 不在拒绝形态表），透传断言依旧成立。
            assert_eq!(
                sandbox_denial::on_tool_error("write_file", "{}", "file not found (os error 2)"),
                "file not found (os error 2)"
            );
            // 有后端但错误不像沙盒拒绝：同样透传、不建台账
            let engaged = oncelock_already_engaged();
            if !engaged {
                unsafe { std::env::set_var("NEMESISBOT_SANDBOX_BACKEND", "landlock") };
            }
            let ws = tempfile::tempdir().unwrap();
            unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_WORKSPACE", ws.path()) };
            assert_eq!(
                sandbox_denial::on_tool_error("write_file", "{}", "invalid utf-8"),
                "invalid utf-8"
            );
            assert!(!ws.path().join("logs").exists(), "非拒绝错误不建台账");
        });
    }

    #[test]
    fn on_tool_error_denial_rewrites_text_and_appends_ledger() {
        with_lock(|| {
            let _env = DenialEnvGuard::fresh();
            if !oncelock_already_engaged() {
                unsafe { std::env::set_var("NEMESISBOT_SANDBOX_BACKEND", "landlock") };
            }
            let ws = tempfile::tempdir().unwrap();
            unsafe { std::env::set_var("NEMESISBOT_EXECUTOR_WORKSPACE", ws.path()) };

            let args = r#"{"path":"/etc/hosts","content":"x"}"#;
            let out = sandbox_denial::on_tool_error("write_file", args, "os error 13 (EACCES)");
            // 可自纠文案：点名后端 + 工作区出口 + 保留原因
            assert!(out.contains("[沙盒拦截]"), "{out}");
            assert!(out.contains("write_file"), "{out}");
            assert!(out.contains("os error 13"), "{out}");
            // 台账一行合法 JSON，model_visible=true
            let ledger = ws.path().join("logs").join("sandbox_denials.jsonl");
            let raw = std::fs::read_to_string(&ledger).expect("ledger written");
            let v: serde_json::Value =
                serde_json::from_str(raw.trim()).expect("exactly one legal JSON line");
            assert_eq!(v["op"], "write_file");
            assert_eq!(v["model_visible"], true);
            assert!(
                v["target"].as_str().unwrap().contains("/etc/hosts"),
                "target = args preview: {}",
                v["target"]
            );
        });
    }
}
//
// dispatch 协议四臂已由上方 dispatch_protocol 单测逐行钉死；本批补的是
// stdio_loop 本体的 读一行→dispatch→序列化→write_all→flush→再读 循环
// （exec_worker.rs ~380-388）——libtest 内进程的 stdin 是死的、无法注入多行，
// 只能按 gateway 生产同款 spawn 形态（ROLE=executor + 绝对路径 WORKSPACE、
// 不带任何 CLI 参数 = main() 短路在 clap 之前）喂两条协议请求：
//   ① 坏 JSON 行      → {"ok":false,...,"error":"bad request line: ..."}
//   ② 未知工具合法行   → {"ok":false,...,"error":"unknown tool: r10_no_such_tool"}
// 然后关 stdin 让子进程在 EOF 干净退出（退码 0）。全程看门狗限时强杀保护。
//
// 隔离：WORKSPACE 指向独立 tempdir 绝对路径；显式 env_remove 掉 PIPE/SANDBOX/
// REEXEC/HOME 四个干扰键，绝不触任何真实 home。不占任何端口。
// 仅 Windows：resolve_nemesisbot_bin 解析 .exe（与其他子进程测试同门）。
// =========================================================================
#[cfg(windows)]
mod wave_r10_stdio_subprocess {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use test_harness::resolve_nemesisbot_bin;

    /// executor 子进程协议全链：坏 JSON 行 + 未知工具行 → 逐行错误响应；
    /// stdin EOF → 干净退出 0。覆盖 stdio_loop 的读-分发-写-刷循环体。
    #[test]
    fn r10_exec_child_stdio_two_request_protocol_round_trip() {
        let bin = match resolve_nemesisbot_bin() {
            Ok(p) => p,
            Err(e) => panic!("nemesisbot.exe 未找到（先构建或设 NEMESISBOT_TEST_BIN）: {e}"),
        };
        let tmp = tempfile::tempdir().expect("tempdir workspace");

        let mut child = Command::new(&bin)
            .env("NEMESISBOT_ROLE", "executor")
            // 绝对路径 tempdir 工作空间；生产 spawn 形态同款 env 键
            .env("NEMESISBOT_EXECUTOR_WORKSPACE", tmp.path())
            .env_remove("NEMESISBOT_EXECUTOR_PIPE")
            .env_remove("NEMESISBOT_EXECUTOR_SANDBOX")
            .env_remove("NEMESISBOT_EXECUTOR_REEXEC")
            .env_remove("NEMESISBOT_EXECUTOR_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn executor-role child");

        // stdout/stderr 各一条排水线程（防管道堵死 + 收集诊断证据）
        let out_pipe = child.stdout.take().expect("piped stdout");
        let err_pipe = child.stderr.take().expect("piped stderr");
        let out_handle = std::thread::spawn(move || {
            let mut lines = Vec::new();
            for l in std::io::BufReader::new(out_pipe).lines() {
                match l {
                    Ok(x) => lines.push(x),
                    Err(_) => break,
                }
            }
            lines
        });
        let err_handle = std::thread::spawn(move || {
            let mut s = String::new();
            use std::io::Read;
            let _ = std::io::BufReader::new(err_pipe).read_to_string(&mut s);
            s
        });

        // 写两条协议请求后关 stdin → 子进程处理两行 → EOF → run_loop Ok(()) 退出
        let bad_line = "{ definitely broken json";
        let req = serde_json::json!({
            "tool": "r10_no_such_tool",
            "args": "{\"k\":1}",
            "context": { "channel": "web", "chat_id": "c1", "user": "u1", "session_key": "s1" },
        });
        {
            let mut sin = child.stdin.take().expect("piped stdin");
            writeln!(sin, "{bad_line}")
                .and_then(|_| writeln!(sin, "{req}"))
                .expect("write protocol lines");
            let _ = sin.flush();
            drop(sin); // EOF 触发器
        }

        // 限时收齐恰好两行响应
        let deadline = Instant::now() + Duration::from_secs(60);
        let lines = loop {
            if out_handle.is_finished() {
                break out_handle.join().expect("stdout drain join");
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for two protocol responses"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        assert_eq!(
            lines.len(),
            2,
            "exactly one response per request line expected, got:\n{lines:?}"
        );

        // 行①：坏 JSON → "bad request line: ..." 且 ok=false
        let r1: serde_json::Value = serde_json::from_str(&lines[0])
            .unwrap_or_else(|e| panic!("response line 1 not JSON ({e}): {:?}", lines[0]));
        assert_eq!(r1["ok"], serde_json::json!(false), "{r1}");
        assert!(
            r1["error"]
                .as_str()
                .unwrap_or("")
                .starts_with("bad request line"),
            "line1 must be a bad-request-line error: {r1}"
        );

        // 行②：未知工具合法行 → "unknown tool: r10_no_such_tool"（指向工具名）
        let r2: serde_json::Value = serde_json::from_str(&lines[1])
            .unwrap_or_else(|e| panic!("response line 2 not JSON ({e}): {:?}", lines[1]));
        assert_eq!(r2["ok"], serde_json::json!(false), "{r2}");
        assert_eq!(
            r2["error"].as_str(),
            Some("unknown tool: r10_no_such_tool"),
            "line2 must name the unknown tool verbatim: {r2}"
        );

        // EOF → 子进程自退 0；限时兜底强杀防挂死测试二进制
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut status_opt = None;
        while Instant::now() < deadline {
            if let Ok(Some(s)) = child.try_wait() {
                status_opt = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        match status_opt {
            Some(status) => assert!(
                status.success(),
                "child must exit cleanly on stdin EOF, got {status}; stderr:\n{}",
                err_handle.join().unwrap_or_default()
            ),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("executor child didn't exit after stdin EOF within 30s");
            }
        }
        drop(err_handle);
    }
}

// ---------------------------------------------------------------------------
// B1（2026-10-31 项目 home 收编）：围栏第二根 env 解析
// ---------------------------------------------------------------------------

#[test]
fn parse_extra_roots_env_roundtrip() {
    // 与 ExecutorChannel::env_pairs 的 join_paths 序列化互逆（同一分隔符
    // 语义；Windows `;` / Unix `:`——两侧都走 std 同源函数，不硬编码）。
    let roots = vec![std::path::PathBuf::from("/ws/agents/a1/home")];
    let joined = std::env::join_paths(&roots).unwrap();
    assert_eq!(parse_extra_roots_env(Some(joined)), roots);
}

#[test]
fn parse_extra_roots_env_unset_is_empty() {
    assert!(parse_extra_roots_env(None).is_empty(), "未设 = 单根旧行为");
}

#[test]
fn parse_extra_roots_env_multi() {
    let roots = vec![
        std::path::PathBuf::from("/a"),
        std::path::PathBuf::from("/b"),
    ];
    let joined = std::env::join_paths(&roots).unwrap();
    assert_eq!(parse_extra_roots_env(Some(joined)), roots);
}
