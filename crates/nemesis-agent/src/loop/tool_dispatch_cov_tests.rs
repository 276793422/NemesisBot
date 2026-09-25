// loop/tool_dispatch.rs 覆盖率补充测试（is_lsp_write_call / F1 plan 写
// 放行 / F-U3-2 工作副本路径基准重写 / dispatch 终态三臂：成功、工具报
// 错、未知工具）。
//
// 安全管线后续层（limits 审批 / deny 回灌 / guardian 语义二审）依赖
// SecurityPlugin + 审批通道装配，由 security crate 与 approval-test E2E
// 覆盖，本文件不重复。

use super::*;

// ---------------------------------------------------------------------------
// 迷你 fixture
// ---------------------------------------------------------------------------

struct CovLlmProvider;

#[async_trait]
impl LlmProvider for CovLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn cov_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

/// 固定回复工具（可配 Ok/Err），记录最近一次收到的 args。
#[derive(Debug)]
struct StubTool {
    result: Result<String, String>,
    seen: std::sync::Mutex<std::collections::VecDeque<String>>,
}

impl StubTool {
    fn ok() -> Box<Self> {
        Box::new(Self {
            result: Ok("stub ok".to_string()),
            seen: Default::default(),
        })
    }
    fn failing() -> Box<Self> {
        Box::new(Self {
            result: Err("boom-cov".to_string()),
            seen: Default::default(),
        })
    }
    /// Ok 但文本是错误形态（tool_result_indicates_error 的文本短路臂）。
    fn ok_with(text: &str) -> Box<Self> {
        Box::new(Self {
            result: Ok(text.to_string()),
            seen: Default::default(),
        })
    }
}

#[async_trait]
impl Tool for StubTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        self.seen.lock().unwrap().push_back(args.to_string());
        self.result.clone()
    }
}

fn call(name: &str, args: serde_json::Value) -> ToolCallInfo {
    ToolCallInfo {
        id: format!("cov-{name}"),
        name: name.to_string(),
        arguments: args.to_string(),
    }
}

fn ctx_with_base(base: Option<&std::path::Path>) -> RequestContext {
    let mut ctx = RequestContext::new("web", "chat-1", "covuser", "agent:main:session:covtd");
    ctx.tool_path_base = base.map(|p| p.to_path_buf());
    ctx
}

// ---------------------------------------------------------------------------
// is_lsp_write_call：op 探测 + 解析失败从严当写
// ---------------------------------------------------------------------------

#[test]
fn lsp_write_call_table() {
    // 非 lsp 工具一律只读。
    assert!(!AgentLoop::is_lsp_write_call(
        "read_file",
        r#"{"op":"rename"}"#
    ));
    assert!(!AgentLoop::is_lsp_write_call("", "{}"));

    // rename = 写。
    assert!(AgentLoop::is_lsp_write_call("lsp", r#"{"op":"rename"}"#));
    // 其余 op = 读。
    assert!(!AgentLoop::is_lsp_write_call(
        "lsp",
        r#"{"op":"definition"}"#
    ));
    // op 缺失 / 非法 JSON → 从严当写（plan 模式拒绝成本不对称）。
    assert!(AgentLoop::is_lsp_write_call("lsp", "{}"));
    assert!(AgentLoop::is_lsp_write_call("lsp", "not json at all"));
}

// ---------------------------------------------------------------------------
// plan_mode_write_allowed：唯一写放行 = write_file 且落 plans/ 前缀
// ---------------------------------------------------------------------------

#[test]
fn plan_write_allowed_requires_workspace_root() {
    let al = AgentLoop::new(Box::new(CovLlmProvider), cov_config());
    // workspace_root 未注入（standalone）→ 无放行锚点，一律拦截。
    assert!(!al.plan_mode_write_allowed("write_file", r#"{"path":"plans/a.md"}"#));
}

#[test]
fn plan_write_allowed_table() {
    let dir = tempfile::tempdir().unwrap();
    let al = AgentLoop::new(Box::new(CovLlmProvider), cov_config());
    al.set_workspace_root(dir.path().to_path_buf());

    // 只有 write_file 放行；其他写工具一律拦截。
    assert!(!al.plan_mode_write_allowed("edit_file", r#"{"path":"plans/a.md"}"#));
    assert!(!al.plan_mode_write_allowed("exec", r#"{"cmd":"echo hi"}"#));

    // 非法 JSON / 缺 path → 拦截。
    assert!(!al.plan_mode_write_allowed("write_file", "not json"));
    assert!(!al.plan_mode_write_allowed("write_file", r#"{"content":"no path"}"#));

    // 相对路径：plans/ 内放行，外拦。
    assert!(al.plan_mode_write_allowed("write_file", r#"{"path":"plans/plan-a.md"}"#));
    assert!(!al.plan_mode_write_allowed("write_file", r#"{"path":"src/lib.rs"}"#));
    // plans 前缀必须是路径分量（plans-x/ 不是 plans/）。
    assert!(!al.plan_mode_write_allowed("write_file", r#"{"path":"plans-x/a.md"}"#));

    // 绝对路径：落在 <root>/plans 内放行，外拦。
    let inside = dir.path().join("plans").join("abs.md");
    let outside = dir.path().join("src").join("abs.rs");
    assert!(al.plan_mode_write_allowed(
        "write_file",
        &serde_json::json!({ "path": inside.to_string_lossy() }).to_string()
    ));
    assert!(!al.plan_mode_write_allowed(
        "write_file",
        &serde_json::json!({ "path": outside.to_string_lossy() }).to_string()
    ));

    // 目标不存在（plans/ 目录未创建）→ 按最长存在祖先解析仍放行。
    assert!(al.plan_mode_write_allowed(
        "write_file",
        &serde_json::json!({ "path": "plans/deep/new.md" }).to_string()
    ));
}

// ---------------------------------------------------------------------------
// rewrite_tool_paths_for_base：工作副本路径基准重写决策表
// ---------------------------------------------------------------------------

#[test]
fn rewrite_paths_disabled_without_base() {
    let c = call("read_file", serde_json::json!({ "path": "rel.txt" }));
    assert!(AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(None)).is_none());
}

#[test]
fn rewrite_paths_skips_non_path_tools_and_bad_json() {
    let base = tempfile::tempdir().unwrap();
    // 非文件/执行类工具 → 不重写。
    let c = call("sleep", serde_json::json!({ "secs": 1 }));
    assert!(
        AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(Some(base.path()))).is_none()
    );
    // 参数不是合法 JSON → 原样透传（args_validator 后续报）。
    let c = ToolCallInfo {
        id: "cov-bad".into(),
        name: "read_file".into(),
        arguments: "not json".into(),
    };
    assert!(
        AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(Some(base.path()))).is_none()
    );
    // 文件工具但路径已是绝对 → 无变化 → None（不重建调用）。
    let abs = base.path().join("a.txt");
    let c = call(
        "read_file",
        serde_json::json!({ "path": abs.to_string_lossy() }),
    );
    assert!(
        AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(Some(base.path()))).is_none()
    );
}

#[test]
fn rewrite_paths_joins_relative_file_paths() {
    let base = tempfile::tempdir().unwrap();
    // 单 path 工具。
    let c = call("read_file", serde_json::json!({ "path": "rel.txt" }));
    let fixed = AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(Some(base.path())))
        .expect("relative path must be rewritten");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&fixed.arguments).unwrap()["path"],
        serde_json::Value::String(base.path().join("rel.txt").to_string_lossy().into_owned())
    );

    // multiedit：edits[].path 逐条重写。
    let c = call(
        "multiedit",
        serde_json::json!({ "edits": [ {"path": "a.rs"}, {"path": "b.rs"} ] }),
    );
    let fixed = AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(Some(base.path())))
        .expect("multiedit paths must be rewritten");
    let args = serde_json::from_str::<serde_json::Value>(&fixed.arguments).unwrap();
    let edits = args["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 2);
    for (i, name) in ["a.rs", "b.rs"].iter().enumerate() {
        assert_eq!(
            edits[i]["path"],
            serde_json::Value::String(base.path().join(name).to_string_lossy().into_owned())
        );
    }
}

#[test]
fn rewrite_paths_handles_exec_cwd_shapes() {
    let base = tempfile::tempdir().unwrap();
    // exec：缺 cwd → 注入 base；空 cwd → 视同缺省；相对 cwd → join。
    let c = call("exec", serde_json::json!({ "cmd": "ls" }));
    let fixed = AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(Some(base.path())))
        .expect("missing cwd must be injected");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&fixed.arguments).unwrap()["cwd"],
        serde_json::Value::String(base.path().to_string_lossy().into_owned())
    );

    let c = call("exec", serde_json::json!({ "cmd": "ls", "cwd": "   " }));
    let fixed = AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(Some(base.path())))
        .expect("blank cwd must be treated as default");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&fixed.arguments).unwrap()["cwd"],
        serde_json::Value::String(base.path().to_string_lossy().into_owned())
    );

    let c = call("exec", serde_json::json!({ "cmd": "ls", "cwd": "sub" }));
    let fixed = AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(Some(base.path())))
        .expect("relative cwd must be joined");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&fixed.arguments).unwrap()["cwd"],
        serde_json::Value::String(base.path().join("sub").to_string_lossy().into_owned())
    );

    // async_shell：键名是 working_dir。
    let c = call("async_shell", serde_json::json!({ "cmd": "ls" }));
    let fixed = AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(Some(base.path())))
        .expect("missing working_dir must be injected");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&fixed.arguments).unwrap()["working_dir"],
        serde_json::Value::String(base.path().to_string_lossy().into_owned())
    );
}

// ---------------------------------------------------------------------------
// handle_tool_call 终态三臂 + 调用点重写（Some 臂）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_unknown_tool_reports_error() {
    let al = AgentLoop::new(Box::new(CovLlmProvider), cov_config());
    let out = al
        .handle_tool_call(
            &call("no_such_cov_tool", serde_json::json!({})),
            &ctx_with_base(None),
        )
        .await;
    assert!(
        out.contains("Unknown tool") && out.contains("no_such_cov_tool"),
        "got: {out}"
    );
}

#[tokio::test]
async fn dispatch_failing_tool_wraps_error() {
    let mut al = AgentLoop::new(Box::new(CovLlmProvider), cov_config());
    al.register_tool("cov_fail".to_string(), StubTool::failing());
    let out = al
        .handle_tool_call(
            &call("cov_fail", serde_json::json!({})),
            &ctx_with_base(None),
        )
        .await;
    assert!(
        out.contains("Tool error:") && out.contains("boom-cov"),
        "got: {out}"
    );
}

#[tokio::test]
async fn dispatch_ok_tool_returns_result() {
    let mut al = AgentLoop::new(Box::new(CovLlmProvider), cov_config());
    al.register_tool("cov_ok".to_string(), StubTool::ok());
    let out = al
        .handle_tool_call(&call("cov_ok", serde_json::json!({})), &ctx_with_base(None))
        .await;
    assert_eq!(out, "stub ok");
}

/// 调用点 Some 臂：tool_path_base 存在时，重写后的 args 才进工具
/// （注册名为 read_file 的 stub —— 注册键与工具类型无关）。
#[tokio::test]
async fn dispatch_rewrites_paths_before_tool_sees_them() {
    use std::sync::Mutex;

    struct RecordingTool {
        seen: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Tool for RecordingTool {
        async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
            self.seen.lock().unwrap().push(args.to_string());
            Ok("recorded".to_string())
        }
    }

    let base = tempfile::tempdir().unwrap();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let mut al = AgentLoop::new(Box::new(CovLlmProvider), cov_config());
    al.register_tool(
        "read_file".to_string(),
        Box::new(RecordingTool {
            seen: Arc::clone(&seen),
        }),
    );

    let out = al
        .handle_tool_call(
            &call("read_file", serde_json::json!({ "path": "rel.txt" })),
            &ctx_with_base(Some(base.path())),
        )
        .await;
    assert_eq!(out, "recorded");

    let got = seen.lock().unwrap();
    assert_eq!(got.len(), 1);
    let args: serde_json::Value = serde_json::from_str(&got[0]).unwrap();
    assert_eq!(
        args["path"],
        serde_json::Value::String(base.path().join("rel.txt").to_string_lossy().into_owned()),
        "tool must see the rewritten absolute path"
    );
}

// ---------------------------------------------------------------------------
// wave5c：A6 format-on-save 路径收集臂（write_file/edit_file 单路径 +
// multiedit 去重保序）+ 注册工具失败臂
// ---------------------------------------------------------------------------

/// write_file 成功 → format_paths 单路径收集 + run_post_hooks 空表 no-op
///（format 配置未注入 → format_on_save 静默原样返回，永不拖垮调用）。
#[tokio::test]
async fn dispatch_write_file_success_collects_format_path() {
    let dir = tempfile::tempdir().unwrap();
    let mut al = AgentLoop::new(Box::new(CovLlmProvider), cov_config());
    al.register_tool("write_file".to_string(), StubTool::ok());
    let target = dir.path().join("a.rs");
    let out = al
        .handle_tool_call(
            &call(
                "write_file",
                serde_json::json!({ "path": target.to_string_lossy() }),
            ),
            &ctx_with_base(None),
        )
        .await;
    assert_eq!(out, "stub ok");
}

/// multiedit 成功 → edits[].path 去重保序收集（同文件只格式化一次）。
#[tokio::test]
async fn dispatch_multiedit_success_dedups_format_paths() {
    let mut al = AgentLoop::new(Box::new(CovLlmProvider), cov_config());
    al.register_tool("multiedit".to_string(), StubTool::ok());
    let out = al
        .handle_tool_call(
            &call(
                "multiedit",
                serde_json::json!({ "edits": [
                    { "path": "src/a.rs" },
                    { "path": "src/a.rs" },
                    { "path": "src/b.rs" }
                ]}),
            ),
            &ctx_with_base(None),
        )
        .await;
    assert_eq!(out, "stub ok");
}

/// 工具返回错误字符串（非 Err）→ format 收集被 tool_result_indicates_error
/// 短路（不进格式化循环），结果原样透传。
#[tokio::test]
async fn dispatch_error_text_result_skips_format_paths() {
    let mut al = AgentLoop::new(Box::new(CovLlmProvider), cov_config());
    al.register_tool(
        "edit_file".to_string(),
        StubTool::ok_with("Error: old_text not found"),
    );
    let out = al
        .handle_tool_call(
            &call("edit_file", serde_json::json!({ "path": "x.rs" })),
            &ctx_with_base(None),
        )
        .await;
    assert_eq!(out, "Error: old_text not found");
}

/// path 槽不是字符串（数字/对象）→ rewrite_rel 内层 if 落空（不 panic、
/// 不重写、原样透传给 args_validator 报类型错）。
#[test]
fn rewrite_paths_tolerates_non_string_path_slot() {
    let base = tempfile::tempdir().unwrap();
    let c = call("read_file", serde_json::json!({ "path": 123 }));
    // 非字符串槽 → 不产生重写（None = 无变化透传）。
    assert!(
        AgentLoop::rewrite_tool_paths_for_base(&c, &ctx_with_base(Some(base.path()))).is_none()
    );
}
