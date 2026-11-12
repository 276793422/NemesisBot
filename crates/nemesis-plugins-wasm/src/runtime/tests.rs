use super::*;
use crate::egress::EgressPolicy;

struct NoSecrets;
impl crate::host_impl::SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

fn frame(slug: &str, kind: PluginKind, limits: PluginLimits) -> CallFrame {
    // 数据目录每帧独立（preopen 需要目录在场；并行用例互不踩踏）。
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "wasm-rt-data-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::create_dir_all(&data_dir);
    let (ring_cap, line_max) = (limits.log_ring_capacity, limits.log_line_max_bytes);
    CallFrame {
        observer_frame: false,
        slug: slug.to_string(),
        kind,
        limits,
        logs: Arc::new(crate::host_impl::PluginLogBuffer::new(ring_cap, line_max)),
        config: Arc::new(crate::host_impl::InstanceConfig::default()),
        secrets: Arc::new(NoSecrets),
        workspace_root: std::env::temp_dir(),
        egress: Arc::new(EgressPolicy::default()),
        invoker: None,
        llm: None,
        secret_write: None,
        config_write: None,
        data_dir,
        audit: None,
        session_key: String::new(),
    }
}

fn runtime() -> PluginRuntime {
    crate::test_support::ensure_host_dll();
    let cache = tempfile::tempdir().expect("cache");
    PluginRuntime::new(PluginLimits::default(), Some(cache.path())).expect("runtime")
}

fn compile_fixture(rt: &PluginRuntime, slug_dir: &str, artifact: &str) -> Option<u64> {
    let path = crate::test_support::fixture_wasm(slug_dir, artifact)?;
    let bytes = std::fs::read(&path).expect("read fixture");
    rt.compile_component(&bytes).ok()
}

// -- extract_session_key ------------------------------------------------

#[test]
fn session_key_extraction() {
    assert_eq!(
        extract_session_key(r#"{"session_key":"flow:s/i","op":"echo"}"#),
        "flow:s/i"
    );
    assert_eq!(extract_session_key(r#"{"op":"echo"}"#), "");
    assert_eq!(extract_session_key("not json"), "");
    assert_eq!(
        extract_session_key(r#"{"session_key":42}"#),
        "",
        "非字符串 = 空"
    );
}

// -- 编译失败 + 无效句柄路径 -----------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn compile_garbage_fails() {
    let rt = runtime();
    let err = rt
        .compile_component(b"not wasm bytes")
        .err()
        .unwrap_or_else(|| panic!("compile"));
    assert!(matches!(err, PluginError::Compile(_)), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn run_call_invalid_component_fails_cleanly() {
    let rt = runtime();
    // component=0 无效哨兵：store_new 失败 → env 就地回收（CallSession
    // Err 臂），不泄漏不 panic。
    let err = rt
        .probe_metadata(0, frame("ghost", PluginKind::Tool, PluginLimits::default()))
        .await
        .err()
        .unwrap_or_else(|| panic!("invalid component"));
    assert!(matches!(err, PluginError::Host(_)), "{err:?}");
}

// -- 满员闸 -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn gate_busy_when_exhausted() {
    crate::test_support::ensure_host_dll();
    let mut limits = PluginLimits::default();
    limits.max_instances = 0;
    let cache = tempfile::tempdir().expect("cache");
    let rt = PluginRuntime::new(limits, Some(cache.path())).expect("runtime");
    let err = rt
        .probe_metadata(0, frame("ghost", PluginKind::Tool, PluginLimits::default()))
        .await
        .err()
        .unwrap_or_else(|| panic!("busy"));
    assert!(matches!(err, PluginError::Busy(0)), "{err:?}");
}

// -- 探针全链（真实 fixture 组件；缺席 = 诚实跳过）-----------------------------

#[tokio::test(flavor = "multi_thread")]
async fn probe_metadata_roundtrip_tool_fixture() {
    let rt = runtime();
    let Some(comp) = compile_fixture(&rt, "textstat", "textstat_plugin") else {
        eprintln!("SKIP：textstat fixture 缺席");
        return;
    };
    let mut f = frame("textstat", PluginKind::Tool, PluginLimits::default());
    // 探针帧纪律：能力面全剥离（invoker 等 None 覆写幂等）。
    f.invoker = None;
    let meta = rt.probe_metadata(comp, f).await.expect("probe");
    assert_eq!(meta.name, "textstat");
    assert_eq!(meta.operation_type, "read");
    assert!(
        serde_json::from_str::<serde_json::Value>(&meta.parameters_json)
            .expect("schema")
            .is_object()
    );
    // 二次探针：fresh-store 无驻留，结论确定性一致。
    let meta2 = rt
        .probe_metadata(
            comp,
            frame("textstat", PluginKind::Tool, PluginLimits::default()),
        )
        .await
        .expect("probe again");
    assert_eq!(meta2.name, "textstat");
}

#[tokio::test(flavor = "multi_thread")]
async fn execute_tool_roundtrip_and_persistence() {
    let rt = runtime();
    let Some(comp) = compile_fixture(&rt, "textstat", "textstat_plugin") else {
        eprintln!("SKIP：textstat fixture 缺席");
        return;
    };
    let out = rt
        .execute_tool(
            comp,
            frame("textstat", PluginKind::Tool, PluginLimits::default()),
            r#"{"text":"hello world"}"#.into(),
            ExecContext {
                session_key: "sess-rt".into(),
                call_id: "call-rt".into(),
            },
        )
        .await
        .expect("execute");
    assert!(!out.is_error);
    assert!(
        out.content.contains("\"words\":2"),
        "content: {}",
        out.content
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn call_observe_roundtrip_and_trap_isolation() {
    let rt = runtime();
    let Some(comp) = compile_fixture(&rt, "activity-log", "activity_log_plugin") else {
        eprintln!("SKIP：activity-log fixture 缺席");
        return;
    };
    // 正常事件。
    rt.call_observe(
        comp,
        frame(
            "activity-log",
            PluginKind::Observer,
            PluginLimits::default(),
        ),
        r#"{"type":"tool_end","tool":"exec"}"#.into(),
    )
    .await
    .expect("observe");
    // trap marker 事件：guest panic → Trap 错误归类（事件丢弃语义）。
    let err = rt
        .call_observe(
            comp,
            frame(
                "activity-log",
                PluginKind::Observer,
                PluginLimits::default(),
            ),
            r#"{"type":"tool_start","trap":true}"#.into(),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("trap"));
    assert!(
        matches!(err, PluginError::Trap(_)) || matches!(err, PluginError::Host(_)),
        "trap 或宿主面错误: {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_probe_and_three_points() {
    let rt = runtime();
    let Some(comp) = compile_fixture(&rt, "policy-sample", "policy_sample_plugin") else {
        eprintln!("SKIP：policy-sample fixture 缺席");
        return;
    };
    let meta = rt
        .probe_policy_metadata(
            comp,
            frame("policy-sample", PluginKind::Policy, PluginLimits::default()),
        )
        .await
        .expect("policy meta");
    assert!(
        meta.compact || meta.final_judge || meta.validation,
        "至少接管一点"
    );
    // 三点调用（JSON 进出；墙钟按点推导）。
    for point in [
        PolicyPoint::Validation,
        PolicyPoint::Compact,
        PolicyPoint::FinalJudge,
    ] {
        let out = rt
            .call_policy(
                comp,
                frame("policy-sample", PluginKind::Policy, PluginLimits::default()),
                point,
                r#"{"session_key":"s1","tool":"read_file","args":{"path":"a.txt"}}"#.into(),
            )
            .await
            .expect("verdict");
        assert!(out.contains("verdict"), "裁决 JSON: {out}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn call_ui_and_flow_step_roundtrip() {
    let rt = runtime();
    // 面板：panel-echo（fixture 缺席 = 跳过）。
    if let Some(comp) = compile_fixture(&rt, "panel-echo", "panel_echo_plugin") {
        let reply = rt
            .call_ui(
                comp,
                frame("panel-echo", PluginKind::Panel, PluginLimits::default()),
                r#"{"command":"echo","args":{"text":"面板你好"},"session_key":"ui:s"}"#.into(),
            )
            .await
            .expect("echo");
        assert!(reply.contains("\"ok\":true"), "reply: {reply}");
    } else {
        eprintln!("SKIP：panel-echo fixture 缺席");
    }
    // flow 步骤：stepdemo（fixture 缺席 = 跳过）。
    if let Some(comp) = compile_fixture(&rt, "stepdemo", "stepdemo_plugin") {
        let meta = rt
            .probe_step_metadata(
                comp,
                frame("stepdemo", PluginKind::FlowStep, PluginLimits::default()),
            )
            .await
            .expect("step meta");
        assert!(meta.kinds.contains(&"echo".to_string()));
        let out = rt
            .call_flow_step(
                comp,
                frame("stepdemo", PluginKind::FlowStep, PluginLimits::default()),
                r#"{"op":"echo","args":{"text":"流程"},"session_key":"flow:s/i"}"#.into(),
            )
            .await
            .expect("echo step");
        assert!(out.contains("\"status\":\"done\""), "out: {out}");
    } else {
        eprintln!("SKIP：stepdemo fixture 缺席");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_export_maps_to_bad_args() {
    let rt = runtime();
    let Some(comp) = compile_fixture(&rt, "textstat", "textstat_plugin") else {
        eprintln!("SKIP：textstat fixture 缺席");
        return;
    };
    // CallSession 直连未知导出名 → 壳 BAD_ARGS 状态码 → Host 错误。
    let limits = PluginLimits::default();
    let to = limits.timeout_ms;
    let f = frame("textstat", PluginKind::Tool, limits.clone());
    let err = rt
        .run_call(comp, f, to, move |session: &CallSession| {
            session.call("no-such-export", "{}", to).map(|_| ())
        })
        .await
        .err()
        .unwrap_or_else(|| panic!("unknown export"));
    assert!(err.to_string().contains("bad call args"), "{err:?}");
}

// -- CallSession env 生命周期（Drop 兜底回收）---------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn call_session_drop_is_tolerant() {
    let rt = runtime();
    let Some(comp) = compile_fixture(&rt, "textstat", "textstat_plugin") else {
        eprintln!("SKIP：textstat fixture 缺席");
        return;
    };
    let limits = PluginLimits::default();
    let to = limits.timeout_ms;
    let f = frame("textstat", PluginKind::Tool, limits.clone());
    // session 正常消费（call_export 一次性）后 Drop → store_drop 容忍 no-op。
    rt.run_call(comp, f, to, move |session: &CallSession| {
        let out = session.call("get-metadata", "{}", to)?;
        bindings::parse_get_metadata(&out)
            .map(|_| ())
            .map_err(|he| PluginError::MetadataMismatch(format!("get-metadata host-error: {he:?}")))
    })
    .await
    .expect("session consumed");
    // 纯构造 + Drop（不调用）：env 回收路径独立验证。
    let f2 = frame("textstat", PluginKind::Tool, limits.clone());
    rt.run_call(comp, f2, limits.timeout_ms, move |_s: &CallSession| Ok(()))
        .await
        .expect("drop-only session");
}

// -- FrameBudget 消耗进 caps（预算语义经回调闸已由 callbacks 测试钉死）----------

#[test]
fn call_frame_carries_limits_and_budget() {
    let mut limits = PluginLimits::default();
    limits.fuel = 123;
    limits.host_call_budget = 7;
    let f = frame("x", PluginKind::Tool, limits.clone());
    assert_eq!(f.limits.fuel, 123);
    let budget = crate::host_impl::FrameBudget::new(0);
    assert!(budget.consume().is_err());
}
