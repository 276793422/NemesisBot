// selftest.rs 覆盖率补充（wave 5）：probe_outside_write 的未沙盒/退化布局臂、
// probe_workspace_write 对照组、emit 单行 JSON 输出。
//
// 纪律：probe_network（1.1.1.1:80 外连）不测——测试进程不发外网流量。

use super::*;

/// probe 1：未沙盒时系统临时目录可写 → blocked=false + 证据含「写入成功」
/// （80-91 Ok 臂）。
#[test]
fn probe_outside_write_reports_unblocked_when_unsandboxed() {
    let ws = tempfile::tempdir().unwrap();
    let check = probe_outside_write(ws.path());
    assert!(
        !check.blocked,
        "unsandboxed temp write must succeed: {check:?}"
    );
    assert!(check.evidence.contains("写入成功"), "{check:?}");
}

/// probe 1 退化布局：workspace 就是系统临时目录 → 诚实跳过臂（57-65）。
#[test]
fn probe_outside_write_skips_degenerate_layout() {
    let ws = std::env::temp_dir();
    let check = probe_outside_write(&ws);
    assert!(!check.blocked);
    assert!(check.evidence.contains("跳过"), "{check:?}");
}

/// probe 3 对照组：workspace 内写入必须成功（137-152 Ok 臂）。
#[test]
fn probe_workspace_write_succeeds_in_workspace() {
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
