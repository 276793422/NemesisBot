//! Fire-and-forget spawn 收尸单一真相源（BUG-20261117-087）。
//!
//! 「spawn 后不等待」的工具类子进程（打开目录/浏览器/文件管理器）在
//! std::process 语义下 drop `Child` 不产生任何回收：Unix 上子进程退出后
//! 成为**僵尸进程**（内核进程表项 + PID 占用），直到父进程退出才被
//! init 回收——gateway 是长命常驻进程，dashboard 每次「打开目录」、每轮
//! OAuth 浏览器唤起都漏一个僵尸，持续累积。tokio 运行时对 `tokio::process::
//! Child` 有孤儿回收（orphan reaper），但 `std::process::Child` 无此待遇。
//! Windows 无僵尸概念（句柄随 drop 关闭），此前裸 spawn 在 Windows 上
//! 无害——缺陷只咬 Unix 部署。
//!
//! 修复形态：spawn 成功后交后台线程 `wait()` 收尸，子进程与父进程生命周期
//! 解绑（detach 语义）。收尸线程失败（极端线程资源耗尽）时闭包连同 Child
//! 一并丢弃——此刻子进程已在跑，返回的 Err 只描述「收尸退化」，不撤回
//! 已成功的打开动作，调用方按原有错误面呈现。
//!
//! 返回值携带子进程 PID：调用方通常忽略，测试与运维侧可据此做进程级
//! 断言（收尸验证 / 残留清查）。
//!
//! 消费方：`nemesis-auth::oauth::open_browser_impl`（三平台浏览器唤起）+
//! `nemesis-web::handlers::shell_open`（目录/文件/关联程序打开，该文件的
//! 同名本地包装退化为转发）。

use std::process::Child;

/// Spawn 一个进程并在后台收尸（成功即返回子进程 PID，不等子进程退出）。
///
/// - spawn 失败 → `Err`（子进程未启动，调用方诚实报错，与裸 spawn 同面）。
/// - spawn 成功 → 子进程交后台收尸线程 `wait()`；函数立即返回 `Ok(pid)`。
/// - 收尸线程创建失败（极端资源耗尽）→ `Err`，此时子进程**已在运行**且
///   本次调用退化为无收尸裸 spawn（Unix 下会留一个僵尸）——诚实报错而非
///   假装成功。
pub fn spawn_detached(make: impl FnOnce() -> std::io::Result<Child>) -> Result<u32, String> {
    let mut child = Some(make().map_err(|e| e.to_string())?);
    let pid = child.as_ref().unwrap().id();
    let reaper = std::thread::Builder::new()
        .name("spawn-reap".into())
        .spawn(move || {
            // Builder::spawn 失败时闭包（连同 Child）被整体丢弃——
            // take() 只在闭包真的跑起来时才有值。
            if let Some(mut c) = child.take() {
                let _ = c.wait();
            }
        });
    reaper
        .map(|_| pid)
        .map_err(|e| format!("spawn-reap thread failed (process left unreaped): {e}"))
}

#[cfg(test)]
mod tests;
