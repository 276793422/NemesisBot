//! BUG-20261119-185 回归锁：install 漏斗 wasm 编译卸载 blocking 线程池。
//!
//! `nvwa_component_new` → wasmtime `Component::new` 是即时全量编译（重 CPU、
//! 且无 fuel/epoch/墙钟任何预算闸）；install_core / load_one 两条 async
//! 路径此前在 tokio worker 上 inline 跑，修复后经 `compile_component_async`
//! 卸载 blocking 线程池。本锁保证 async 编译路径对真 fixture 组件功能等价
//! （有效句柄）。
//!
//! **诚实边界（首轮实验记录）**：曾尝试「current_thread executor + ticker
//! 进展」的行为锁（inline 回归形态会饿死唯一 worker → 零 tick）——单跑
//! 全绿，并入全量套件并行执行后 ticks=0 假红：cargo 并行测试打满 CPU 核
//! 时，OS 调度延迟可让本测试线程在编译窗口内完全不被调度（tokio 层
//! executor 空闲 ≠ OS 层线程被调度）。断言依赖 OS 调度行为=环境敏感，
//! 按仓库纪律收窄为仅功能等价锁；offload 纪律由调用点结构保证。
//!
//! fixture = 真实 cdylib 组件按需构建，构建失败诚实 SKIP（套件既有纪律）。

use crate::test_support::{default_manager, fixture_wasm};

fn fixture_bytes() -> Option<Vec<u8>> {
    let path = fixture_wasm("textstat", "textstat_plugin")?;
    std::fs::read(&path).ok()
}

#[tokio::test]
async fn compile_component_async_yields_valid_handle() {
    let Some(bytes) = fixture_bytes() else {
        eprintln!("SKIP：fixture 组件构建失败（环境缺 wasm32-wasip2 或网络）");
        return;
    };
    let (_root, mgr) = default_manager();
    let handle = mgr
        .runtime()
        .compile_component_async(bytes)
        .await
        .expect("compile must succeed via blocking pool");
    assert_ne!(handle, 0, "valid component must yield a nonzero handle");
    mgr.runtime().host.component_drop(handle);
}
