//! [`crate::mcp_serve`] 单元测试：工具清单契约 / 协议通知判定 / 轻量记忆
//! 检索 / K1 装配结构断言（security on/off 行为差异——安全同源证据之一，
//! 端到端行为证据见 `tests/mcp_serve_stdio.rs` 审计链用例）。

use super::*;

// ---------------------------------------------------------------------------
// 工具清单契约
// ---------------------------------------------------------------------------

#[test]
fn tool_definitions_has_four_v1_tools() {
    let defs = tool_definitions();
    let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["board_issue_query", "memory_search", "run", "sessions_list"],
        "v1 工具面必须是这四个"
    );
    for d in &defs {
        assert!(!d.name.is_empty());
        assert!(
            d.description
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "工具 {} 必须带描述（外部 MCP 客户端靠它选型）",
            d.name
        );
        assert_eq!(
            d.input_schema["type"], "object",
            "{} schema 顶层 object",
            d.name
        );
    }
}

#[test]
fn run_and_memory_search_declare_required_args() {
    let defs = tool_definitions();
    let run = defs.iter().find(|d| d.name == "run").unwrap();
    assert_eq!(
        run.input_schema["required"],
        serde_json::json!(["task"]),
        "run 必填 task"
    );
    let mem = defs.iter().find(|d| d.name == "memory_search").unwrap();
    assert_eq!(
        mem.input_schema["required"],
        serde_json::json!(["query"]),
        "memory_search 必填 query"
    );
}

// ---------------------------------------------------------------------------
// 协议通知判定
// ---------------------------------------------------------------------------

#[test]
fn is_notification_distinguishes_frames() {
    // 带 id 的请求 → 不是通知（要回帧）。
    assert!(!is_notification(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#
    ));
    assert!(!is_notification(
        r#"{"jsonrpc":"2.0","id":"abc","method":"tools/list"}"#
    ));
    // 无 id 帧（notifications/initialized）→ 通知（不回帧）。
    assert!(is_notification(
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
    ));
    // id 显式为 null = JSON-RPC 里仍算「有 id」位置……按无 id 字段语义判定
    // 为通知——get("id") 返回 Some(Value::Null)？不：serde_json 的 get 对
    // "id": null 返回 Some(Null)，所以带 null id 的帧不算通知（会回帧，
    // handle_raw 的响应 id 也是 Null——两端对得上）。
    assert!(!is_notification(
        r#"{"jsonrpc":"2.0","id":null,"method":"x"}"#
    ));
    // 垃圾行 → 不是通知（走 handle_raw 的 -32700 回帧路径）。
    assert!(!is_notification("not json at all"));
}

// ---------------------------------------------------------------------------
// 轻量记忆检索（workspace/memory 文本回落路径）
// ---------------------------------------------------------------------------

fn write_file(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn search_memory_files_finds_case_insensitive_with_line_numbers() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write_file(
        &ws.join("memory").join("MEMORY.md"),
        "# Memory\n\n用户偏好深色主题。\nDeployment uses blue-green.\n",
    );
    write_file(
        &ws.join("memory").join("notes").join("sub.md"),
        "deployment checklist: run tests\n",
    );

    let hits = search_memory_files(ws, "deployment", 10);
    assert_eq!(hits.len(), 2, "大小写不敏感命中两个文件");
    assert_eq!(hits[0].file, "memory/MEMORY.md");
    assert_eq!(hits[0].line_no, 4);
    assert_eq!(hits[0].line, "Deployment uses blue-green.");
    assert_eq!(hits[1].file, "memory/notes/sub.md");
    assert_eq!(hits[1].line_no, 1);
}

#[test]
fn search_memory_files_respects_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    write_file(
        &ws.join("memory").join("MEMORY.md"),
        "alpha\nalpha\nalpha\nalpha\nalpha\n",
    );
    let hits = search_memory_files(ws, "alpha", 3);
    assert_eq!(hits.len(), 3, "limit 截断");
}

#[test]
fn search_memory_files_missing_dir_is_empty_not_error() {
    let tmp = tempfile::tempdir().unwrap();
    let hits = search_memory_files(tmp.path(), "anything", 10);
    assert!(hits.is_empty(), "memory 目录不存在 = 空结果");
}

#[test]
fn search_memory_files_miss_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    write_file(
        &tmp.path().join("memory").join("MEMORY.md"),
        "完全无关内容\n",
    );
    assert!(search_memory_files(tmp.path(), "nonexistent-xyz", 10).is_empty());
}

// ---------------------------------------------------------------------------
// K1 装配结构断言（security on/off 行为差异）
// ---------------------------------------------------------------------------

/// 最小可装配 config（死地址 provider——装配不拨号，与 run/tests.rs 同款）。
fn dead_provider_config() -> serde_json::Value {
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

/// 造一个带 config.json 的临时 home（调用方再按需改写 security 键）。
fn temp_home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("workspace")).unwrap();
    std::fs::write(
        tmp.path().join("config.json"),
        dead_provider_config().to_string(),
    )
    .unwrap();
    tmp
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemble_missing_config_is_honest_err() {
    let tmp = tempfile::tempdir().unwrap();
    let err = assemble(&tmp.path()).await.unwrap_err();
    assert!(err.contains("Configuration not found"), "{err}");
    assert!(err.contains("nemesisbot onboard default"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemble_security_on_reports_pipeline_active() {
    let tmp = temp_home();
    let asm = assemble(&tmp.path()).await.expect("装配应成功");
    assert!(
        asm.security_active,
        "security.enabled 缺省 true → 安全 8 层管线必须真实在位"
    );
    assert!(asm.workspace.ends_with("workspace"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemble_security_off_reports_pipeline_absent() {
    let tmp = temp_home();
    // 显式关安全 → 结构断言面翻 false（诚实呈现，绝不谎报）。
    let cfg_path = tmp.path().join("config.json");
    let mut cfg = dead_provider_config();
    cfg["security"] = serde_json::json!({"enabled": false});
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();

    let asm = assemble(&tmp.path()).await.expect("装配应成功");
    assert!(
        !asm.security_active,
        "security.enabled=false → security_active 必须为 false"
    );
}

#[cfg(feature = "memory")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemble_memory_flag_gates_executor() {
    // 关（缺省）：无执行器。
    let tmp = temp_home();
    let asm = assemble(&tmp.path()).await.expect("装配应成功");
    assert!(asm.memory_executor.is_none(), "memory.enabled 缺省关");

    // 开：执行器在位（gateway ctx.rs 同款构造；无 ONNX 插件自动降级 basic）。
    let tmp2 = temp_home();
    let cfg_path = tmp2.path().join("config.json");
    let mut cfg = dead_provider_config();
    cfg["memory"] = serde_json::json!({"enabled": true});
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();
    let asm2 = assemble(&tmp2.path()).await.expect("装配应成功");
    assert!(
        asm2.memory_executor.is_some(),
        "memory.enabled=true → 执行器在位"
    );
}
