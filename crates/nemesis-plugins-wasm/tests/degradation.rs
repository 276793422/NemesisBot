//! 宿主壳 DLL 缺席降级：`PluginManager::new` = `NotAvailable`（诚实关闭
//! 子系统，gateway 侧 `build_plugin_stack` 据此整体置 None——不崩、不静默
//! 假装可用）。
//!
//! 独立 `[[test]]` 目标 = 独立进程：不受其他测试二进制的
//! `NEMESIS_WASM_HOST_DLL` 注入与 loader OnceLock 装载缓存串扰。

use std::sync::Arc;

use nemesis_plugins_wasm::PluginError;
use nemesis_plugins_wasm::host_impl::SecretResolver;
use nemesis_plugins_wasm::limits::PluginLimits;
use nemesis_plugins_wasm::registry::PluginManager;

struct NoSecrets;

impl SecretResolver for NoSecrets {
    fn resolve(&self, _alias: &str) -> Option<String> {
        None
    }
}

#[test]
fn host_dll_missing_degrades_to_not_available() {
    // env 候选指到必然不存在的路径；exe-相邻（target/debug/deps/）与仓库
    // 开发路径（deps 视角 = target/plugins/...）同样缺席 → 全候选落空。
    // SAFETY: 单线程测试进程早期，无并发读者。
    unsafe {
        std::env::set_var(
            "NEMESIS_WASM_HOST_DLL",
            "Z:/definitely/not/here/plugin_wasm.dll",
        );
    }
    let ws = tempfile::tempdir().expect("tempdir");
    let err = PluginManager::new(ws.path(), PluginLimits::default(), Arc::new(NoSecrets))
        .err()
        .expect("DLL 缺席必须失败");
    assert!(
        matches!(err, PluginError::NotAvailable(ref msg) if msg.contains("not found")),
        "期望 NotAvailable + 候选路径清单，实得：{err}"
    );
}
