//! Full Access 编辑器放行开关测试矩阵(2026-09-20 用户裁决)。
//!
//! 覆盖:开关 4 组合 × op 族 × 项目内外 × 路径形态(大小写/反斜杠/
//! verbatim/UNC/兄弟前缀/相对/`..`/`~`/空)× roots 注入与漂移 × 重启回关。

use std::sync::Arc;

use super::*;

const ROOT: &str = "D:/Workspace";
const INSIDE: &str = "D:/Workspace/proj-a/src/main.rs";
const OUTSIDE: &str = "D:/elsewhere/data.txt";

/// 建一个注入了 roots 的状态(归一化对大小写/尾斜杠的容错由 roots 侧负责)。
fn state_with_roots(roots: &[&str]) -> Arc<EditorAccessState> {
    let s = EditorAccessState::new();
    s.set_workspace_roots(roots.iter().map(|r| r.to_string()).collect());
    s
}

/// 写删族四成员便利数组。
const WRITE_DELETE: &[OperationType] = &[
    OperationType::FileWrite,
    OperationType::FileDelete,
    OperationType::DirCreate,
    OperationType::DirDelete,
];

// ---------------------------------------------------------------------------
// 开关关闭:整体不短路(evaluate 恒 None,回落原判定链)
// ---------------------------------------------------------------------------

#[test]
fn all_off_never_short_circuits() {
    let s = EditorAccessState::new();
    s.set_workspace_roots(vec![ROOT.to_string()]);

    assert_eq!(s.snapshot(), (false, false));
    // 写删族项目内、exec、读——全部 None(旧行为,走规则/审批)。
    for op in WRITE_DELETE {
        assert_eq!(s.evaluate(*op, INSIDE), None, "{op:?} 在开关全关时不得短路");
    }
    assert_eq!(s.evaluate(OperationType::ProcessExec, "python -V"), None);
    assert_eq!(s.evaluate(OperationType::FileRead, OUTSIDE), None);
}

// ---------------------------------------------------------------------------
// 开关一 Full Access:写删族项目内放行 / 项目外回落
// ---------------------------------------------------------------------------

#[test]
fn full_on_inside_write_delete_allowed() {
    let s = state_with_roots(&[ROOT]);
    s.set_flags(true, false);

    for op in WRITE_DELETE {
        let (decision, _, policy) = s
            .evaluate(*op, INSIDE)
            .unwrap_or_else(|| panic!("{op:?} 项目内写删必须短路放行"));
        assert_eq!(decision, SecurityDecision::Allowed);
        assert_eq!(policy, "editor_access:full");
    }
}

#[test]
fn full_on_outside_write_delete_falls_back() {
    let s = state_with_roots(&[ROOT]);
    s.set_flags(true, false);

    // 项目外写删:开关二未开 → None 回落(仍走规则/审批,保留治理)。
    for op in WRITE_DELETE {
        assert_eq!(
            s.evaluate(*op, OUTSIDE),
            None,
            "{op:?} 项目外写删在开关二未开时必须回落"
        );
    }
}

#[test]
fn full_plus_ext_outside_write_delete_allowed() {
    let s = state_with_roots(&[ROOT]);
    s.set_flags(true, true);
    assert_eq!(s.snapshot(), (true, true));

    for op in WRITE_DELETE {
        let (decision, _, policy) = s
            .evaluate(*op, OUTSIDE)
            .unwrap_or_else(|| panic!("{op:?} 双开后项目外写删必须放行"));
        assert_eq!(decision, SecurityDecision::Allowed);
        assert_eq!(policy, "editor_access:full+ext");
    }
}

#[test]
fn ext_alone_implies_full() {
    // 服务端联动收口:ext=true ⇒ full=true(UI 禁用联动只是体验)。
    let s = state_with_roots(&[ROOT]);
    s.set_flags(false, true);

    assert_eq!(s.snapshot(), (true, true), "ext=true 必须联动拉起 full");
    let (decision, _, policy) = s.evaluate(OperationType::FileDelete, OUTSIDE).unwrap();
    assert_eq!(decision, SecurityDecision::Allowed);
    assert_eq!(policy, "editor_access:full+ext");
}

// ---------------------------------------------------------------------------
// 其余族:开关一开启即放行,不分内外(exec 假精度比不做更危险——裁决)
// ---------------------------------------------------------------------------

#[test]
fn non_write_ops_allowed_anywhere_when_full() {
    let s = state_with_roots(&[ROOT]);
    s.set_flags(true, false);

    let ops = [
        OperationType::FileRead,
        OperationType::DirRead,
        OperationType::ProcessExec,
        OperationType::ProcessSpawn,
        OperationType::NetworkDownload,
        OperationType::NetworkUpload,
        OperationType::NetworkRequest,
        OperationType::SystemShutdown,
        OperationType::HardwareGPIO,
    ];
    for op in ops {
        // 项目外也放(exec 族整体放行的裁决语义)。
        let (decision, _, policy) = s
            .evaluate(op, OUTSIDE)
            .unwrap_or_else(|| panic!("{op:?} 必须随开关一直接放行"));
        assert_eq!(decision, SecurityDecision::Allowed);
        assert_eq!(policy, "editor_access:full");
    }
}

#[test]
fn registry_ops_treated_as_non_write_family() {
    // 死臂(当前无工具映射):按「其余全放」归入开关一直接放行。
    // 将来若有工具映射进该族且语义应视作写删,挪 WRITE_DELETE_OPS 即可。
    let s = state_with_roots(&[ROOT]);
    s.set_flags(true, false);

    for op in [
        OperationType::RegistryRead,
        OperationType::RegistryWrite,
        OperationType::RegistryDelete,
    ] {
        let (decision, _, policy) = s.evaluate(op, OUTSIDE).unwrap();
        assert_eq!(decision, SecurityDecision::Allowed);
        assert_eq!(policy, "editor_access:full");
    }
}

// ---------------------------------------------------------------------------
// 路径形态归一化与逃逸硬化
// ---------------------------------------------------------------------------

#[test]
fn relative_paths_inside_unless_escape() {
    let s = state_with_roots(&[ROOT]);
    s.set_flags(true, false);

    // 相对路径 = 工具链语义相对 workspace 解析 → inside。
    let (decision, _, _) = s.evaluate(OperationType::FileWrite, "src/main.rs").unwrap();
    assert_eq!(decision, SecurityDecision::Allowed);

    // 逃逸硬化:`..` 段 / `~` 头 → outside(fail-safe 回落)。
    assert_eq!(s.evaluate(OperationType::FileWrite, "a/../b"), None);
    assert_eq!(s.evaluate(OperationType::FileWrite, "../outside.txt"), None);
    assert_eq!(s.evaluate(OperationType::FileWrite, "~"), None);
    assert_eq!(s.evaluate(OperationType::FileWrite, "~/secret"), None);
}

#[test]
fn empty_target_falls_back_for_write_delete() {
    let s = state_with_roots(&[ROOT]);
    s.set_flags(true, false);

    assert_eq!(s.evaluate(OperationType::FileWrite, ""), None);
    assert_eq!(s.evaluate(OperationType::DirDelete, "   "), None);
}

#[test]
fn path_form_normalization() {
    // roots 大小写混写照样归一化;目标侧大小写/反斜杠/verbatim 全对齐。
    let s = state_with_roots(&["D:/Workspace/"]);
    s.set_flags(true, false);

    let inside_forms = [
        "d:/workspace/proj-a/src/main.rs",     // 小写
        "D:\\Workspace\\proj-a\\Cargo.toml",   // 反斜杠
        "\\\\?\\D:\\workspace\\proj-a\\a.txt", // verbatim 前缀
        "d:/workspace",                        // 根自身(建目录=写删族)
    ];
    for t in inside_forms {
        let (decision, _, policy) = s
            .evaluate(OperationType::FileWrite, t)
            .unwrap_or_else(|| panic!("{t} 必须判 inside"));
        assert_eq!(decision, SecurityDecision::Allowed);
        assert_eq!(policy, "editor_access:full", "{t}");
    }

    // 兄弟前缀不得误命中(分隔符感知)。
    assert_eq!(
        s.evaluate(OperationType::FileWrite, "d:/workspace2/x.txt"),
        None,
        "workspace2 是兄弟目录,分隔符感知前缀不得命中"
    );
}

#[test]
fn unc_roots_and_targets() {
    let s = state_with_roots(&["//SERVER/Share"]);
    s.set_flags(true, false);

    let (decision, _, _) = s
        .evaluate(OperationType::FileWrite, "//server/share/a.txt")
        .unwrap();
    assert_eq!(decision, SecurityDecision::Allowed);
    // verbatim UNC:剥 //?/unc/ 后与 root 对齐。
    let (decision, _, _) = s
        .evaluate(OperationType::FileWrite, "\\\\?\\UNC\\server\\share\\b.txt")
        .unwrap();
    assert_eq!(decision, SecurityDecision::Allowed);
    assert_eq!(
        s.evaluate(OperationType::FileDelete, "//server/other/c.txt"),
        None
    );
}

#[test]
fn no_roots_injected_fails_safe() {
    // roots 未注入(gateway 未装配)→ 写删族一律 outside → fail-safe 回落。
    let s = EditorAccessState::new();
    s.set_flags(true, false);
    assert_eq!(
        s.evaluate(OperationType::FileWrite, "d:/workspace/x.txt"),
        None
    );
}

// ---------------------------------------------------------------------------
// 运行时态:重启语义(双关)+ roots 漂移刷新
// ---------------------------------------------------------------------------

#[test]
fn reset_flags_emulates_process_restart() {
    let s = state_with_roots(&[ROOT]);
    s.set_flags(true, true);
    assert_eq!(s.snapshot(), (true, true));

    // 进程重启 = 新建状态(双关),此处以重置模拟:判定必须回到 None。
    s.set_flags(false, false);
    assert_eq!(s.snapshot(), (false, false));
    assert_eq!(s.evaluate(OperationType::ProcessExec, "python -V"), None);
    assert_eq!(s.evaluate(OperationType::FileWrite, INSIDE), None);
}

#[test]
fn roots_can_be_refreshed_at_runtime() {
    // 运行期新建项目:roots 刷新后立即生效(editor.get/set 每次刷新纠正)。
    let s = state_with_roots(&[ROOT]);
    s.set_flags(true, false);
    assert_eq!(
        s.evaluate(OperationType::FileWrite, "d:/newproj/x.txt"),
        None
    );

    s.set_workspace_roots(vec![ROOT.to_string(), "D:/NewProj".to_string()]);
    let (decision, _, _) = s
        .evaluate(OperationType::FileWrite, "d:/newproj/x.txt")
        .unwrap();
    assert_eq!(decision, SecurityDecision::Allowed);
}

// ---------------------------------------------------------------------------
// 私有归一化助手直测(锁定归一化契约)
// ---------------------------------------------------------------------------

#[test]
fn normalize_target_contract() {
    assert_eq!(normalize_target(""), None);
    assert_eq!(normalize_target("   "), None);
    assert_eq!(
        normalize_target("  \\\\?\\C:\\A\\B.TXT "),
        Some("c:/a/b.txt".to_string())
    );
    assert_eq!(
        normalize_target("\\\\?\\UNC\\srv\\share\\f"),
        Some("//srv/share/f".to_string())
    );
    assert_eq!(normalize_target("rel/path"), Some("rel/path".to_string()));
}

#[test]
fn is_absolute_normalized_contract() {
    assert!(is_absolute_normalized("c:/x"));
    assert!(is_absolute_normalized("/x"));
    assert!(is_absolute_normalized("//srv/share"));
    assert!(!is_absolute_normalized("rel/x"));
    assert!(!is_absolute_normalized("x"));
}
