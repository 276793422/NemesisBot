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

/// estop 钩子在 tokio async 上下文里同步调用不得 panic（回归锁）：
/// gateway estop watcher 是 tokio::spawn 的任务，修复前钩子闭包取
/// `Handle::try_current()` 后 `block_on`——tokio enter_runtime 对已进入
/// runtime 的线程直接 panic（「Cannot start a runtime from within a
/// runtime」），watcher 任务死亡 → 紫微会话不卸载且后续 estop 静默失效。
/// 修复 = 闭包内专用线程 + 独立 current_thread runtime + mpsc 桥接。
#[cfg(windows)]
#[tokio::test]
async fn estop_hook_survives_async_context() {
    use std::sync::Arc;

    let workspace =
        std::env::temp_dir().join(format!("ziwei-estop-regress-{}", std::process::id()));
    let mgr = Arc::new(crate::session::SessionManager::new(&workspace, None, 4));
    crate::session::set_global(mgr);
    // 覆盖语义：把真实生产闭包钉进槽（不依赖其他测试留下的槽状态）。
    crate::session::register_estop_hook();
    // 当前函数体就是 tokio 任务上下文——修复前此处 panic。
    let report = crate::estop_hook::run_estop_unload();
    // 空管理器零会话 → 空报告；不炸即为本回归锁本体。
    assert!(report.is_empty(), "空管理器应零报告: {report:?}");
}
