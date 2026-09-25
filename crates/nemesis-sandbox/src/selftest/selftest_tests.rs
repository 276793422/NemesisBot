//! G7 (D2)：selftest 探针单测（无沙盒直跑）——
//! 对照组必须成功；无沙盒时外部写/网络探测如实报告「未拦截」。
//! 拦截语义（landlock/bwrap/Seatbelt 生效时的 blocked:true）由 WSL2 真机
//! 验收（R2）覆盖 —— 单测环境没有沙盒，无法也不应伪造 blocked 结果。

use super::*;
use std::path::PathBuf;

/// AGT：run_probes/probe_outside_write 的写入路径是
/// `temp/nemesis_selftest_probe_{pid}.txt` —— 同一测试进程内所有测试线程
/// 共享同一 PID（同一文件名）。占位目录测试存在期间，并发断言「外部写
/// 允许」的测试会撞上目录 → os error 5 → 假失败。串行化兜底。
static AGT_SELFTEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn probes_unsandboxed_report_allowed_not_blocked() {
    let _guard = AGT_SELFTEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let workspace =
        std::env::temp_dir().join(format!("nemesis_selftest_ws_{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();
    let checks = run_probes(&workspace);
    std::fs::remove_dir_all(&workspace).ok();

    assert_eq!(checks.len(), 3);
    // 对照组：workspace 内写入必须成功。
    assert!(
        !checks[2].blocked,
        "control probe must pass: {:?}",
        checks[2]
    );
    // 无沙盒：外部写探测如实报告「写入成功（未拦截）」。
    assert!(
        !checks[0].blocked,
        "unsandboxed outside-write must be allowed: {:?}",
        checks[0]
    );
    // 每条 evidence 非空（诚实证据，不是空串）。
    for c in &checks {
        assert!(!c.evidence.is_empty());
    }
}

#[test]
fn child_out_serializes_single_compact_line_without_null_error() {
    let out = SelftestChildOut {
        ok: true,
        error: None,
        checks: vec![ProbeCheck {
            name: "probe".to_string(),
            blocked: true,
            evidence: "denied".to_string(),
        }],
    };
    let s = serde_json::to_string(&out).unwrap();
    assert!(s.contains("\"ok\":true"));
    assert!(!s.contains("\"error\""), "None error must be skipped: {s}");
    assert!(!s.contains('\n'));

    // degenerate 布局分支：workspace 含临时目录 → 跳过探测（不假装测过）。
    let tmp = std::env::temp_dir();
    let check = probe_outside_write(&tmp); // workspace = temp_dir 本身
    assert!(!check.blocked);
    assert!(check.evidence.contains("跳过"));
    let _: Option<PathBuf> = None;
}

// ---------------------------------------------------------------------------
// AGT 覆盖率批次（2026-09-24）：两个 Err 臂 + emit 的成功臂。
// - 外部写 Err 臂：探测目标路径被同名**目录**占位 → fs::write 必败 →
//   blocked=true（诚实证据臂，无需真沙盒/防火墙）。
// - 对照组 Err 臂：workspace 挂在**文件**路径之下 → join 后写入必败。
// - emit：单行 JSON 落 stdout（serde 成功 + writeln + flush）。
// ---------------------------------------------------------------------------

#[test]
fn probe_outside_write_reports_blocked_when_target_is_directory() {
    let _guard = AGT_SELFTEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let workspace =
        std::env::temp_dir().join(format!("nemesis_selftest_ws3_{}", std::process::id()));
    std::fs::create_dir_all(&workspace).unwrap();
    // 占位目录：probe_outside_write 的写入路径与之同名 → 写目录必败
    let probe_path =
        std::env::temp_dir().join(format!("nemesis_selftest_probe_{}.txt", std::process::id()));
    std::fs::create_dir_all(&probe_path).unwrap();
    let checks = run_probes(&workspace);
    let _ = std::fs::remove_dir(&probe_path);
    let _ = std::fs::remove_dir_all(&workspace);
    assert!(checks[0].blocked, "{:?}", checks[0]);
    assert!(checks[0].evidence.contains("写入被拒"), "{:?}", checks[0]);
}

#[test]
fn probe_workspace_write_reports_blocked_when_workspace_under_file() {
    let _guard = AGT_SELFTEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // workspace = <已存在的文件>/sub → 对照组写入路径的父目录是文件 → 必败
    let file =
        std::env::temp_dir().join(format!("nemesis_selftest_file_{}.bin", std::process::id()));
    std::fs::write(&file, b"x").unwrap();
    let fake_ws = file.join("sub");
    let checks = run_probes(&fake_ws);
    let _ = std::fs::remove_file(&file);
    assert!(checks[2].blocked, "{:?}", checks[2]);
    assert!(checks[2].evidence.contains("异常"), "{:?}", checks[2]);
    // 外部写探测不受影响（temp 根可写）
    assert!(!checks[0].blocked, "{:?}", checks[0]);
}

#[test]
fn emit_prints_single_json_line_with_error_field() {
    let out = SelftestChildOut {
        ok: false,
        error: Some("boom".to_string()),
        checks: vec![ProbeCheck {
            name: "p".to_string(),
            blocked: false,
            evidence: "e".to_string(),
        }],
    };
    // 成功臂：serde 序列化 + writeln + flush（stdout 被测试 harness 捕获）
    emit(&out);
}
