//! exec_worker 测试：U11 用户态沙盒路径决策表（纯函数，跨平台）。
//! 真实行为断言（landlock 写外拒/写内放行）在 `tests/executor.rs` 的
//! Linux e2e（cfg linux）里。

use super::*;

#[cfg(feature = "sandbox")]
mod userland_plan {
    use super::userland::{plan, Plan};
    use nemesis_sandbox::backend::BackendForm;

    #[test]
    fn no_marker_runs_plain() {
        assert_eq!(plan(false, false, Some(BackendForm::SelfApply)), Plan::Plain);
        // 即使 bwrap 可用，没标记也不介入
        assert_eq!(
            plan(false, false, Some(BackendForm::WrapCommand)),
            Plan::Plain
        );
    }

    #[test]
    fn reexeced_instance_never_re_engages() {
        // 防环核心：盒内实例（REEXEC=1）见到标记也直接 Plain
        assert_eq!(plan(true, true, Some(BackendForm::SelfApply)), Plan::Plain);
        assert_eq!(plan(true, true, Some(BackendForm::WrapCommand)), Plan::Plain);
    }

    #[test]
    fn marker_with_no_backend_degrades_plain() {
        // 降级验收语义：无后端 → Plain（warn 由 engage 打），不 Err
        assert_eq!(plan(true, false, None), Plan::Plain);
    }

    #[test]
    fn marker_with_self_apply_backend_applies() {
        assert_eq!(
            plan(true, false, Some(BackendForm::SelfApply)),
            Plan::SelfApply
        );
    }

    #[test]
    fn marker_with_wrap_backend_reexeces() {
        assert_eq!(
            plan(true, false, Some(BackendForm::WrapCommand)),
            Plan::WrapReexec
        );
    }
}

// ---------------------------------------------------------------------------
// P5-2：engage 的严格模式改判（fail-closed）与默认 fail-open
// ---------------------------------------------------------------------------
// Windows 上 detect_backend() 恒 None（设计契约，Sandboxie 承担）→ engage
// 确定走 Plain 路径，strict 的 bail/不 bail 可确定性断言。**必须** cfg
// windows：Linux 上 detect_backend 可能返回 Some → SelfApply → engage 会
// 真对测试进程装 landlock（不可逆，污染同进程其他测试）。
#[cfg(all(feature = "sandbox", windows))]
mod engage_strict {
    use super::userland::{engage, Outcome};

    fn seed_home(strict: Option<bool>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = match strict {
            None => {
                r#"{ "executor": { "enabled": true, "sandbox": true } }"#.to_string()
            }
            Some(s) => format!(
                r#"{{ "executor": {{ "enabled": true, "sandbox": true, "strict": {s} }} }}"#
            ),
        };
        std::fs::write(dir.path().join("config.json"), body).expect("seed config.json");
        dir
    }

    #[test]
    fn strict_refuses_when_no_backend() {
        let home = seed_home(Some(true));
        let err = engage("/ws", Some(home.path())).expect_err("strict must refuse");
        assert!(err.to_string().contains("strict mode"), "err: {err}");
        assert!(
            err.to_string().contains("refusing to run unsandboxed"),
            "err: {err}"
        );
    }

    #[test]
    fn no_strict_config_keeps_fail_open() {
        // 缺 strict 键（= 默认 false）：无后端 → warn + Continue（现状字节不变）
        let home = seed_home(None);
        assert!(matches!(
            engage("/ws", Some(home.path())),
            Ok(Outcome::Continue)
        ));
    }

    #[test]
    fn strict_false_keeps_fail_open() {
        let home = seed_home(Some(false));
        assert!(matches!(
            engage("/ws", Some(home.path())),
            Ok(Outcome::Continue)
        ));
    }

    #[test]
    fn no_home_defaults_fail_open() {
        // home=None（测试/裸构造）→ strict 读不到 → false → fail-open
        assert!(matches!(engage("/ws", None), Ok(Outcome::Continue)));
    }
}
