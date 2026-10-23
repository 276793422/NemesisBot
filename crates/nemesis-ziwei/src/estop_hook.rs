//! estop → 全会话卸载的回调槽（T0.7；总控 §9.4）。
//!
//! 设计：紫微会话管理器（M1 落地的 `SessionManager`）在装配时把「全部会话
//! 进入卸载序列」的闭包注册进本槽；gateway 的 estop 订阅任务（M1 接线）在
//! engaged 上升沿调用 [`run_estop_unload`]。槽为空（bridge 未装配）= no-op
//! ——治理接线面先于实现存在，急停路径不因紫微缺席而炸。
//!
//! 卸载是**降险动作**，不受审批矩阵约束（§9.2）；每次触发必须产出审计
//! 报告行（返回值由调用方写审计）。

use std::sync::{Arc, Mutex};

/// 卸载回调形态：返回逐会话卸载报告行（会话名 + 结果）。
pub type UnloadFn = dyn Fn() -> Vec<String> + Send + Sync;

static UNLOAD_HOOK: Mutex<Option<Arc<UnloadFn>>> = Mutex::new(None);

/// 注册卸载回调（会话管理器装配时调用；重复注册覆盖——gateway 只装配一次，
/// 覆盖语义留给测试）。
pub fn set_unload_hook(f: Box<UnloadFn>) {
    *UNLOAD_HOOK.lock().unwrap() = Some(Arc::from(f));
}

/// 槽是否已装配（诊断/测试用）。
pub fn has_unload_hook() -> bool {
    UNLOAD_HOOK.lock().unwrap().is_some()
}

/// 急停触发：执行全部会话卸载序列，返回审计报告行。
///
/// 槽为空 = 紫微未装配 → 返回空报告（no-op，诚实语义）。
pub fn run_estop_unload() -> Vec<String> {
    // clone Arc 后立即放锁再调——闭包内再入本函数不死锁，并发覆盖
    // 不影响已在执行的闭包。
    let hook = UNLOAD_HOOK.lock().unwrap().clone();
    match hook {
        Some(f) => {
            let report = f();
            tracing::warn!("[ziwei] estop 卸载序列完成：{} 条会话报告", report.len());
            report
        }
        None => {
            tracing::debug!("[ziwei] estop 卸载槽未装配（紫微未启用），no-op");
            Vec::new()
        }
    }
}
