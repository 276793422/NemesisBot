//! editor 开关（Full Access）× exec 族围栏接线测试（editor 开关 × exec
//! 围栏两层不认账修复，2026-11-10）。
//!
//! 修复前缺陷：Full Access 开关只活在审批层与文件围栏
//! （WorkspaceBoundary::editor_gate，且仅项目/集群 loop 接线），exec 族
//! 四处 cwd 围栏（exec / run_script / exec_async / background_start，
//! 外加 delegate_start 复用 resolve_cwd）从不读开关——按钮承诺的
//! 「执行放行」在工具层失信（2026-11-10 事故：exec git status 界外 cwd
//! 三连拒，0.003s，无 SECURITY BLOCKED）。
//!
//! 修复形态：四处围栏拒绝分支先问 fence_gate 闭包（exec 族 = 整体放行
//! 族，读族臂载体），闸不放行才拒绝（消息带解锁指引）。本文件用合成
//! 闸覆盖四处围栏的闸开/闸关/闸缺席矩阵 + registry 分发接线；真实
//! EditorAccessState 端到端在 tests/editor_switch_fence_gap_tests.rs。
//!
//! 装配纪律锚：默认构造（无 with_editor_gate）= 旧行为字节不变——
//! 基线注册、executor 分离子进程（exec_worker 哑执行体）、全部既有测试。

use super::{AsyncExecTool, EditorGate, ExecTool, RunScriptTool, Tool};
use crate::background_registry::{BackgroundProcessRegistry, BackgroundStartTool, resolve_cwd};
use crate::context::RequestContext;
use std::path::PathBuf;
use std::sync::Arc;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_lt_gate_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn gate_allow() -> EditorGate {
    Arc::new(|_p: &str, _w: bool| true)
}

fn gate_deny() -> EditorGate {
    Arc::new(|_p: &str, _w: bool| false)
}

// ---------------------------------------------------------------------------
// exec：闸开 / 闸关 / 闸缺席 / 界内不受闸影响
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exec_gate_absent_denies_outside_with_hint() {
    let ws = temp_dir("exec_absent_ws");
    let outside = temp_dir("exec_absent_out");
    let tool = ExecTool::new(&ws.to_string_lossy(), true);
    let args =
        serde_json::json!({"command": "echo hello", "cwd": outside.to_string_lossy()}).to_string();
    let err = tool
        .execute(&args, &ctx())
        .await
        .expect_err("gate 缺席必须维持拒绝");
    assert!(err.contains("Access denied"), "{err}");
    assert!(err.contains("outside workspace"), "{err}");
    // 拒绝消息带解锁指引（事故复盘：用户不知道去哪开）。
    assert!(err.contains("Full Access"), "{err}");
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn exec_gate_allow_runs_outside_cwd() {
    let ws = temp_dir("exec_allow_ws");
    let outside = temp_dir("exec_allow_out");
    let tool = ExecTool::new(&ws.to_string_lossy(), true).with_editor_gate(Some(gate_allow()));
    let args = serde_json::json!({"command": "echo gate-open", "cwd": outside.to_string_lossy()})
        .to_string();
    let out = tool
        .execute(&args, &ctx())
        .await
        .expect("闸开（Full Access）时界外 cwd 必须放行");
    assert!(out.contains("gate-open"), "{out}");
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn exec_gate_deny_still_denies_with_hint() {
    // 闸在场但判 false（开关全关形态）：拒绝是正确行为——闸只能把拒绝
    // 翻成放行，不能反向变成装饰。
    let ws = temp_dir("exec_deny_ws");
    let outside = temp_dir("exec_deny_out");
    let tool = ExecTool::new(&ws.to_string_lossy(), true).with_editor_gate(Some(gate_deny()));
    let args =
        serde_json::json!({"command": "echo hello", "cwd": outside.to_string_lossy()}).to_string();
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(
        err.contains("outside workspace") && err.contains("Full Access"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn exec_gate_allow_keeps_inside_cwd_normal() {
    // 界内 cwd 与闸无关（gate 只活在「界外即将拒绝」分支）。
    let ws = temp_dir("exec_inside_ws");
    let tool = ExecTool::new(&ws.to_string_lossy(), true).with_editor_gate(Some(gate_allow()));
    let args = serde_json::json!({"command": "echo inside"}).to_string();
    let out = tool.execute(&args, &ctx()).await.unwrap();
    assert!(out.contains("inside"), "{out}");
    let _ = std::fs::remove_dir_all(&ws);
}

// ---------------------------------------------------------------------------
// run_script：同款矩阵（代表性两面：闸开放行 / 缺席拒绝带指引）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_script_gate_allow_runs_outside_cwd() {
    let ws = temp_dir("rs_allow_ws");
    let outside = temp_dir("rs_allow_out");
    let tool = RunScriptTool::new(&ws.to_string_lossy(), true).with_editor_gate(Some(gate_allow()));
    let args = serde_json::json!({
        "interpreter": "bash",
        "flag": "-c",
        "script": "echo rs-gate",
        "cwd": outside.to_string_lossy(),
    })
    .to_string();
    let out = tool
        .execute(&args, &ctx())
        .await
        .expect("闸开时 run_script 界外 cwd 必须放行");
    assert!(out.contains("rs-gate"), "{out}");
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn run_script_gate_absent_denies_with_hint() {
    let ws = temp_dir("rs_absent_ws");
    let outside = temp_dir("rs_absent_out");
    let tool = RunScriptTool::new(&ws.to_string_lossy(), true);
    let args = serde_json::json!({
        "interpreter": "bash",
        "flag": "-c",
        "script": "echo hi",
        "cwd": outside.to_string_lossy(),
    })
    .to_string();
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(
        err.contains("Access denied") && err.contains("Full Access"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------
// exec_async（SEALED 封存，接线保恢复路径安全）：闸开放行
// ---------------------------------------------------------------------------

#[tokio::test]
async fn async_exec_gate_allow_runs_outside_cwd() {
    let ws = temp_dir("ax_allow_ws");
    let outside = temp_dir("ax_allow_out");
    let tool = AsyncExecTool::new(&ws.to_string_lossy(), true).with_editor_gate(Some(gate_allow()));
    let args = serde_json::json!({
        "command": "echo ax-gate",
        "wait_seconds": 3,
        "cwd": outside.to_string_lossy(),
    })
    .to_string();
    let out = tool
        .execute(&args, &ctx())
        .await
        .expect("闸开时 exec_async 界外 cwd 必须放行");
    assert!(out.contains("ax-gate"), "{out}");
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------
// resolve_cwd（background_start + delegate_start 共用）：闸矩阵
// ---------------------------------------------------------------------------

#[test]
fn resolve_cwd_gate_matrix() {
    let ws = temp_dir("cwd_ws");
    let outside = temp_dir("cwd_out");
    let ws_s = ws.to_string_lossy().to_string();
    let out_s = outside.to_string_lossy().to_string();

    // 闸缺席 + 界外 → 拒（旧行为）。
    let err = resolve_cwd(&ws_s, true, Some(&out_s), None).unwrap_err();
    assert!(
        err.contains("outside workspace") && err.contains("Full Access"),
        "{err}"
    );
    // 闸开 → 放行，返回解析后的绝对路径。
    let ok = resolve_cwd(&ws_s, true, Some(&out_s), Some(&gate_allow())).unwrap();
    assert_eq!(ok, out_s);
    // 闸判 false → 拒（闸不是装饰）。
    let err = resolve_cwd(&ws_s, true, Some(&out_s), Some(&gate_deny())).unwrap_err();
    assert!(err.contains("outside workspace"), "{err}");
    // 界内路径闸开照常放行（闸不碰界内）。
    resolve_cwd(&ws_s, true, None, Some(&gate_allow())).unwrap();

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------
// background_start：闸开起任务 / 闸缺席拒绝带指引
// ---------------------------------------------------------------------------

#[tokio::test]
async fn background_start_gate_allow_starts_outside_cwd() {
    let ws = temp_dir("bg_allow_ws");
    let outside = temp_dir("bg_allow_out");
    let tool = BackgroundStartTool::new(
        &ws.to_string_lossy(),
        true,
        Arc::new(BackgroundProcessRegistry::new()),
    )
    .with_editor_gate(Some(gate_allow()));
    let args = serde_json::json!({
        "command": "echo bg-gate",
        "notify": false,
        "cwd": outside.to_string_lossy(),
    })
    .to_string();
    let out = tool
        .execute(&args, &ctx())
        .await
        .expect("闸开时 background_start 界外 cwd 必须放行");
    assert!(out.contains("job_id"), "{out}");
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn background_start_gate_absent_denies_with_hint() {
    let ws = temp_dir("bg_absent_ws");
    let outside = temp_dir("bg_absent_out");
    let tool = BackgroundStartTool::new(
        &ws.to_string_lossy(),
        true,
        Arc::new(BackgroundProcessRegistry::new()),
    );
    let args = serde_json::json!({
        "command": "echo hi",
        "cwd": outside.to_string_lossy(),
    })
    .to_string();
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(
        err.contains("Access denied") && err.contains("Full Access"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------
// registry 分发：boundary 槽里的闸 → 注册表里的 exec 工具行为化继承
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_shared_tools_exec_inherits_editor_gate() {
    let ws = temp_dir("reg_gate_ws");
    let outside = temp_dir("reg_gate_out");
    let boundary = super::WorkspaceBoundary {
        root: ws.clone(),
        restrict: true,
        extra_roots: Vec::new(),
        editor_gate: Some(gate_allow()),
    };
    let cfg = super::SharedToolConfig {
        workspace: Some(ws.to_string_lossy().to_string()),
        workspace_boundary: Some(Arc::new(boundary)),
        exec_output_tx: None,
        ..Default::default()
    };
    let tools = super::register_shared_tools(&cfg);
    let exec = tools.get("exec").expect("exec 必须注册");
    let args = serde_json::json!({"command": "echo dispatch", "cwd": outside.to_string_lossy()})
        .to_string();
    let out = exec
        .execute(&args, &ctx())
        .await
        .expect("boundary 槽带闸 → exec 注册形态必须继承（生产装配路径）");
    assert!(out.contains("dispatch"), "{out}");

    // 对照：boundary 闸缺席 → 注册形态维持拒绝（默认装配旧行为）。
    let boundary_plain = super::WorkspaceBoundary {
        root: ws.clone(),
        restrict: true,
        extra_roots: Vec::new(),
        editor_gate: None,
    };
    let cfg_plain = super::SharedToolConfig {
        workspace: Some(ws.to_string_lossy().to_string()),
        workspace_boundary: Some(Arc::new(boundary_plain)),
        exec_output_tx: None,
        ..Default::default()
    };
    let tools_plain = super::register_shared_tools(&cfg_plain);
    let exec_plain = tools_plain.get("exec").expect("exec 必须注册");
    let err = exec_plain.execute(&args, &ctx()).await.unwrap_err();
    assert!(
        err.contains("outside workspace") && err.contains("Full Access"),
        "{err}"
    );

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&outside);
}
