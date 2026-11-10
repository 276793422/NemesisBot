//! editor 放行开关（Full Access / 外部写删）× 工具层围栏一致性回归锁
//! （2026-11-01 BUG 修复）。
//!
//! 修复前缺陷：三个放行开关全部活在审批层（auditor 短路，
//! editor_access::evaluate），工具层的文件围栏 `validate_workspace_path`
//! 从不读开关状态。开关全开时审批层放行的项目外操作，在工具层被无卡
//! 硬拒——「两个全开 = 真·全放」（docs/REPORT/2026-09-20_FullAccess编辑器
//! 放行开关实施报告.md）在项目会话 / 集群 worker 装配形态下失信。
//!
//! 修复形态：装配处把围栏 `editor_gate` 接到与 auditor **同一个**
//! `EditorAccessState`（fence_gate 直接委托 evaluate，两层同一裁决源）。
//! 本文件全绿即回归锁：
//! - 承诺半边 2 测：审批层认开关（与 editor_access/tests.rs 互证）；
//! - 端到端 3 测（原 bug_*）：开关开 → 围栏放行（读族随 full，写删界外
//!   随 full+ext）；
//! - 对照 1 测：开关全关（闸在场上但判 false）→ 围栏照常拦——修复不得
//!   把围栏变装饰。
//! - exec 族事故重放 2 测（2026-11-10 追加）：exec 族围栏接入开关闸的
//!   端到端——Full Access 开 → exec 界外 cwd 放行（事故形态）；全关 →
//!   照常拒且消息带解锁指引。合成闸矩阵在 loop_tools/exec_editor_gate_tests.rs。
//!
//! 装配形态对齐生产（工具经 register_shared_tools 装配 = 生产同一装配函数）：
//! - 项目会话（agent_factory::build_project_agent_loop， 2026-10-31）：
//!   root=项目目录，restrict=true，extra_roots=[agent_home]；
//! - 集群 worker（build_cluster_agent_loop，ASM-04 2026-09-16）：
//!   root=主 workspace，restrict=true，extra_roots=[]。

#![cfg(feature = "security")]

use nemesis_agent::context::RequestContext;
use nemesis_agent::loop_tools::{SharedToolConfig, WorkspaceBoundary, register_shared_tools};
use nemesis_security::editor_access::EditorAccessState;
use nemesis_security::{OperationType, SecurityDecision};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn ctx() -> RequestContext {
    RequestContext {
        channel: "web".to_string(),
        chat_id: "chat".to_string(),
        user: "u".to_string(),
        session_key: "agent:test/editor_fence_gap".to_string(),
        correlation_id: None,
        async_callback: None,
        tool_path_base: None,
        approval_ref: None,
    }
}

fn ps(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_efg_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 项目会话装配形态（agent_factory.rs build_project_agent_loop 同构）：
/// 围栏带 editor 开关闸——生产修复形态（同一 EditorAccessState 的
/// fence_gate，判定与审批层 evaluate 同源）。
fn project_boundary(
    project: &Path,
    agent_home: &Path,
    editor: &Arc<EditorAccessState>,
) -> WorkspaceBoundary {
    WorkspaceBoundary {
        root: project.to_path_buf(),
        restrict: true,
        extra_roots: vec![agent_home.to_path_buf()],
        editor_gate: Some(editor.fence_gate()),
    }
}

/// 集群 worker 装配形态（agent_factory.rs build_cluster_agent_loop 同构，
/// ASM-04：强制 restrict=true，root=主 workspace，无第二根）。
fn cluster_boundary(main_ws: &Path, editor: &Arc<EditorAccessState>) -> WorkspaceBoundary {
    WorkspaceBoundary {
        root: main_ws.to_path_buf(),
        restrict: true,
        extra_roots: Vec::new(),
        editor_gate: Some(editor.fence_gate()),
    }
}

const WRITE_DELETE: [OperationType; 4] = [
    OperationType::FileWrite,
    OperationType::FileDelete,
    OperationType::DirCreate,
    OperationType::DirDelete,
];

// ---------------------------------------------------------------------------
// 承诺半边：审批层认开关（auditor 短路同源 editor_access::evaluate）
// ——当前应为绿，与既有 editor_access/tests.rs 互为印证。
// ---------------------------------------------------------------------------

#[test]
fn approval_layer_full_access_allows_main_workspace_read() {
    // 开关一 Full Access：主工作区读放行（读族随开关一直接放行）。
    let main_ws = temp_dir("promise_main");
    let ref_file = main_ws.join("reference.md");
    std::fs::write(&ref_file, "reference-content").unwrap();

    let editor = EditorAccessState::new();
    editor.set_workspace_roots(vec![ps(&main_ws)]);
    editor.set_flags(true, false);

    let (decision, _, policy) = editor
        .evaluate(OperationType::FileRead, &ps(&ref_file))
        .unwrap_or_else(|| panic!("Full Access 开启时主工作区读必须在审批层放行"));
    assert_eq!(decision, SecurityDecision::Allowed);
    assert_eq!(policy, "editor_access:full");

    let _ = std::fs::remove_dir_all(&main_ws);
}

#[test]
fn approval_layer_full_plus_ext_allows_outside_write_delete() {
    // 双开：工作区外写删放行（「两个全开 = 真·全放」的审批层半边）。
    let main_ws = temp_dir("promise_ws2");
    let outside = temp_dir("promise_out2");
    let target = outside.join("artifact.txt");

    let editor = EditorAccessState::new();
    editor.set_workspace_roots(vec![ps(&main_ws)]);
    editor.set_flags(true, true);

    for op in WRITE_DELETE {
        let (decision, _, policy) = editor
            .evaluate(op, &ps(&target))
            .unwrap_or_else(|| panic!("{op:?} 双开后工作区外必须在审批层放行"));
        assert_eq!(decision, SecurityDecision::Allowed);
        assert_eq!(policy, "editor_access:full+ext", "{op:?}");
    }

    let _ = std::fs::remove_dir_all(&main_ws);
    let _ = std::fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------
// 端到端承诺：开关全开 → 工具层围栏放行
// ——当前应为红（BUG 本体）：同一操作审批层 Allowed、工具层 access denied。
// 修复后翻绿成为回归锁。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bug_project_fence_blocks_read_despite_full_access() {
    // BUG 面 1：项目会话 + Full Access 开 → 读主工作区参考文件。
    // 用户可见症状：read_file 报 access denied，Full Access 承诺的工具层
    // 半边失信（不止外部写删一个开关失信）。
    let main_ws = temp_dir("bug1_main");
    let project = temp_dir("bug1_proj");
    let agent_home = temp_dir("bug1_home");
    let ref_file = main_ws.join("reference.md");
    std::fs::write(&ref_file, "reference-content").unwrap();

    // 真实开关态：Full Access 开（roots 注入主工作区，同 security_setup 装配）。
    let editor = Arc::new(EditorAccessState::new());
    editor.set_workspace_roots(vec![ps(&main_ws)]);
    editor.set_flags(true, false);

    // 审批层裁决（auditor 短路同源）：放行。
    let (decision, _, policy) = editor
        .evaluate(OperationType::FileRead, &ps(&ref_file))
        .unwrap_or_else(|| panic!("审批层必须放行"));
    assert_eq!(decision, SecurityDecision::Allowed, "{policy}");

    // 工具层：项目会话装配形态（root=项目目录，restrict=true，extra=[agent_home]，
    // 带开关闸——生产修复形态）。
    let cfg = SharedToolConfig {
        exec_output_tx: None,
        workspace_boundary: Some(Arc::new(project_boundary(&project, &agent_home, &editor))),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    let read = tools.get("read_file").expect("read_file 必须注册");
    let out = read
        .execute(
            &serde_json::json!({"path": ps(&ref_file)}).to_string(),
            &ctx(),
        )
        .await;

    assert!(
        out.is_ok(),
        "BUG：Full Access 开启时审批层已放行({policy})，项目会话工具层仍拒绝读主工作区文件: {out:?}"
    );
    assert_eq!(out.unwrap(), "reference-content");

    let _ = std::fs::remove_dir_all(&main_ws);
    let _ = std::fs::remove_dir_all(&project);
    let _ = std::fs::remove_dir_all(&agent_home);
}

#[tokio::test]
async fn bug_project_fence_blocks_write_despite_full_plus_ext() {
    // BUG 面 2：项目会话 + 双开 → 写主工作区。
    // 用户可见症状：「两个全开 = 真·全放」不成立，write_file 落盘被拦。
    let main_ws = temp_dir("bug2_main");
    let project = temp_dir("bug2_proj");
    let agent_home = temp_dir("bug2_home");
    let target = main_ws.join("artifact.txt");

    // 真实开关态：双开（外部写删开 ⇒ 服务端联动 full 也开）。
    let editor = Arc::new(EditorAccessState::new());
    editor.set_workspace_roots(vec![ps(&main_ws)]);
    editor.set_flags(true, true);

    let (decision, _, policy) = editor
        .evaluate(OperationType::FileWrite, &ps(&target))
        .unwrap_or_else(|| panic!("审批层必须放行"));
    assert_eq!(decision, SecurityDecision::Allowed, "{policy}");

    let cfg = SharedToolConfig {
        exec_output_tx: None,
        workspace_boundary: Some(Arc::new(project_boundary(&project, &agent_home, &editor))),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    let write = tools.get("write_file").expect("write_file 必须注册");
    let out = write
        .execute(
            &serde_json::json!({"path": ps(&target), "content": "body"}).to_string(),
            &ctx(),
        )
        .await;

    assert!(
        out.is_ok(),
        "BUG：Full Access + 外部写删双开时审批层已放行({policy})，项目会话工具层仍拒绝写主工作区: {out:?}"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "body");

    let _ = std::fs::remove_dir_all(&main_ws);
    let _ = std::fs::remove_dir_all(&project);
    let _ = std::fs::remove_dir_all(&agent_home);
}

#[tokio::test]
async fn bug_cluster_fence_blocks_write_despite_full_plus_ext() {
    // BUG 面 3：集群 worker（root=主 workspace，ASM-04 强制围栏）+ 双开
    // → 写主 workspace 之外。审批层认开关（集群与主 loop 共享同一
    // security plugin / auditor），围栏不认——同一矛盾，集群装配形态。
    let main_ws = temp_dir("bug3_main");
    let outside = temp_dir("bug3_out");
    let target = outside.join("artifact.txt");

    let editor = Arc::new(EditorAccessState::new());
    editor.set_workspace_roots(vec![ps(&main_ws)]);
    editor.set_flags(true, true);

    let (decision, _, policy) = editor
        .evaluate(OperationType::FileWrite, &ps(&target))
        .unwrap_or_else(|| panic!("审批层必须放行"));
    assert_eq!(decision, SecurityDecision::Allowed, "{policy}");

    let cfg = SharedToolConfig {
        exec_output_tx: None,
        workspace_boundary: Some(Arc::new(cluster_boundary(&main_ws, &editor))),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    let write = tools.get("write_file").expect("write_file 必须注册");
    let out = write
        .execute(
            &serde_json::json!({"path": ps(&target), "content": "body"}).to_string(),
            &ctx(),
        )
        .await;

    assert!(
        out.is_ok(),
        "BUG：双开时审批层已放行({policy})，集群 worker 围栏仍拒绝写主 workspace 之外: {out:?}"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "body");

    let _ = std::fs::remove_dir_all(&main_ws);
    let _ = std::fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------
// 对照组：开关全关时围栏拒绝是**正确**行为（修复不得破坏这半边）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn control_switches_off_fence_still_blocks() {
    // 开关全关：审批层回落 None（走规则/审批），围栏闸在场但判 false →
    // 围栏照常拦界外——这是修复必须保持的现状半边（不因修复把围栏变
    // 装饰；与 bug 测的区别只在开关态，闸本身恒在场）。
    let main_ws = temp_dir("ctrl_main");
    let project = temp_dir("ctrl_proj");
    let agent_home = temp_dir("ctrl_home");
    let ref_file = main_ws.join("reference.md");
    std::fs::write(&ref_file, "reference-content").unwrap();

    let editor = Arc::new(EditorAccessState::new());
    editor.set_workspace_roots(vec![ps(&main_ws)]);
    editor.set_flags(false, false);
    assert_eq!(editor.snapshot(), (false, false));

    let cfg = SharedToolConfig {
        exec_output_tx: None,
        workspace_boundary: Some(Arc::new(project_boundary(&project, &agent_home, &editor))),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    let read = tools.get("read_file").expect("read_file 必须注册");
    let err = read
        .execute(
            &serde_json::json!({"path": ps(&ref_file)}).to_string(),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("access denied"), "{err}");
    assert!(err.contains("outside the workspace"), "{err}");

    let _ = std::fs::remove_dir_all(&main_ws);
    let _ = std::fs::remove_dir_all(&project);
    let _ = std::fs::remove_dir_all(&agent_home);
}

// ---------------------------------------------------------------------------
// exec 族事故重放（2026-11-10）：Full Access 开 → exec 界外 cwd 放行
// ——事故形态：workspace=D:\Lenovo\...，exec git status --short --branch
// cwd=D:\AI\NemesisBot_Rust，0.003s Access denied，无 SECURITY BLOCKED
// （审批层已放行、工具层围栏不认开关）。修复后该形态必须翻绿。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bug_exec_blocks_outside_cwd_despite_full_access() {
    let main_ws = temp_dir("bug4_main");
    let outside = temp_dir("bug4_out");

    // 真实开关态：Full Access 开（同事故会话的按钮动作）。
    let editor = Arc::new(EditorAccessState::new());
    editor.set_workspace_roots(vec![ps(&main_ws)]);
    editor.set_flags(true, false);

    // 审批层半边：exec 族在 Full Access 体系下整体放行（editor_access
    // 设计原文「exec 族不分内外整体放行」，evaluate 走非写删族无条件臂）。
    let (decision, _, policy) = editor
        .evaluate(OperationType::ProcessExec, &ps(&outside))
        .unwrap_or_else(|| panic!("审批层必须放行 exec 族"));
    assert_eq!(decision, SecurityDecision::Allowed, "{policy}");

    // 生产装配形态：workspace=主 ws + boundary（restrict=true，带闸）→
    // register_shared_tools 同一装配函数（exec 经 boundary 槽继承闸）。
    let cfg = SharedToolConfig {
        workspace: Some(ps(&main_ws)),
        exec_output_tx: None,
        workspace_boundary: Some(Arc::new(cluster_boundary(&main_ws, &editor))),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    let exec = tools.get("exec").expect("exec 必须注册");
    let out = exec
        .execute(
            &serde_json::json!({
                "command": "echo incident-replay",
                "cwd": ps(&outside),
            })
            .to_string(),
            &ctx(),
        )
        .await;

    assert!(
        out.is_ok(),
        "BUG：Full Access 开启时审批层已放行({policy})，exec 仍拒绝界外 cwd（事故形态）: {out:?}"
    );
    assert!(out.unwrap().contains("incident-replay"));

    let _ = std::fs::remove_dir_all(&main_ws);
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn control_switches_off_exec_still_blocks_with_hint() {
    // 对照组：开关全关 → exec 界外 cwd 照常拒绝（拒绝消息带解锁指引，
    // 前缀 Access denied: ... outside workspace 不变——既有断言依赖）。
    let main_ws = temp_dir("ctrl_exec_main");
    let outside = temp_dir("ctrl_exec_out");

    let editor = Arc::new(EditorAccessState::new());
    editor.set_workspace_roots(vec![ps(&main_ws)]);
    editor.set_flags(false, false);

    let cfg = SharedToolConfig {
        workspace: Some(ps(&main_ws)),
        exec_output_tx: None,
        workspace_boundary: Some(Arc::new(cluster_boundary(&main_ws, &editor))),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    let exec = tools.get("exec").expect("exec 必须注册");
    let err = exec
        .execute(
            &serde_json::json!({
                "command": "echo should-not-run",
                "cwd": ps(&outside),
            })
            .to_string(),
            &ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.contains("Access denied"), "{err}");
    assert!(err.contains("outside workspace"), "{err}");
    assert!(err.contains("Full Access"), "{err}");

    let _ = std::fs::remove_dir_all(&main_ws);
    let _ = std::fs::remove_dir_all(&outside);
}
