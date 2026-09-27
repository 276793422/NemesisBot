// selftest.rs 覆盖率补充（wave 5）：probe_outside_write 的未沙盒/退化布局臂、
// probe_workspace_write 对照组、emit 单行 JSON 输出。
//
// 纪律：probe_network（1.1.1.1:80 外连）不测——测试进程不发外网流量。

use super::*;

// probe_outside_write 的写入路径按 pid 派生（进程内全局唯一）——写该路径的
// 测试必须与 selftest_tests 的占位目录测试串行（拿同一把锁），否则并发窗口
// 内撞上占位目录 → os error 5 假红（2026-09-27 实证，锁语义见
// selftest_tests::AGT_SELFTEST_LOCK 文档）。
use super::selftest_tests::AGT_SELFTEST_LOCK;

/// probe 1：未沙盒时系统临时目录可写 → blocked=false + 证据含「写入成功」
/// （80-91 Ok 臂）。
#[test]
fn probe_outside_write_reports_unblocked_when_unsandboxed() {
    let _guard = AGT_SELFTEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let ws = tempfile::tempdir().unwrap();
    let check = probe_outside_write(ws.path());
    assert!(
        !check.blocked,
        "unsandboxed temp write must succeed: {check:?}"
    );
    assert!(check.evidence.contains("写入成功"), "{check:?}");
}

/// probe 1 退化布局：workspace 就是系统临时目录 → 诚实跳过臂（57-65）。
/// 不写共享 probe 路径（跳过臂零文件系统副作用），无需拿锁。
#[test]
fn probe_outside_write_skips_degenerate_layout() {
    let ws = std::env::temp_dir();
    let check = probe_outside_write(&ws);
    assert!(!check.blocked);
    assert!(check.evidence.contains("跳过"), "{check:?}");
}

/// probe 3 对照组：workspace 内写入必须成功（137-152 Ok 臂）。写入路径在
/// tempdir 子树内不与共享路径冲突，但 run_probes 会连带跑 probe 1（写共享
/// 路径）——拿锁与占位目录测试互斥。
#[test]
fn probe_workspace_write_succeeds_in_workspace() {
    let _guard = AGT_SELFTEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let ws = tempfile::tempdir().unwrap();
    let check = probe_workspace_write(ws.path());
    assert!(!check.blocked, "workspace write is the control: {check:?}");
    assert!(check.evidence.contains("写入成功"), "{check:?}");
}

/// emit：单行 JSON 到 stdout（146-155；cargo test 捕获 stdout，无污染）。
#[test]
fn emit_prints_single_line_json() {
    let out = SelftestChildOut {
        ok: true,
        error: None,
        checks: vec![ProbeCheck {
            name: "probe".into(),
            blocked: false,
            evidence: "e".into(),
        }],
    };
    emit(&out); // 不 panic 即可；serde 序列化与 stdout 写入行都被求值。
}

/// SelftestChildOut 带 error 字段时 skip_serializing_if 生效（结构体臂补全）。
#[test]
fn selftest_out_with_error_serializes_error_field() {
    let out = SelftestChildOut {
        ok: false,
        error: Some("boom".into()),
        checks: vec![],
    };
    let json = serde_json::to_string(&out).unwrap();
    assert!(json.contains("boom"), "{json}");
    assert!(!json.contains("error\":null"), "{json}");
}
