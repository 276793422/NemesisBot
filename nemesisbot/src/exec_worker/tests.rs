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
