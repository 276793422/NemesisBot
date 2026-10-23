//! nemesis-ziwei 单测（M0：estop 回调槽）。

use crate::estop_hook::{has_unload_hook, run_estop_unload, set_unload_hook};

#[test]
fn estop_hook_noop_when_absent() {
    // 槽未装配（本测试进程首次调用）：诚实 no-op，空报告。
    if !has_unload_hook() {
        assert!(run_estop_unload().is_empty());
    }
}

#[test]
fn estop_hook_invokes_registered_closure() {
    set_unload_hook(Box::new(|| {
        vec!["session dummy-monitor: unloaded".to_string()]
    }));
    let report = run_estop_unload();
    assert_eq!(report, vec!["session dummy-monitor: unloaded".to_string()]);
}
