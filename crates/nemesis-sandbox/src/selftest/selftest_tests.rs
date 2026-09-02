//! G7 (D2)：selftest 探针单测（无沙盒直跑）——
//! 对照组必须成功；无沙盒时外部写/网络探测如实报告「未拦截」。
//! 拦截语义（landlock/bwrap/Seatbelt 生效时的 blocked:true）由 WSL2 真机
//! 验收（R2）覆盖 —— 单测环境没有沙盒，无法也不应伪造 blocked 结果。

use super::*;
use std::path::PathBuf;

#[test]
fn probes_unsandboxed_report_allowed_not_blocked() {
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
