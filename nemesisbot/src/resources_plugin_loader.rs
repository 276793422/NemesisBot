//! plugin_resources 宿主装载器（C-ABI 解冻第三例：纯数据资源载体；装载
//! 纪律沿用 [`crate::roaming_plugin_loader`] / [`crate::vm_display_loader`]）。
//!
//! 与 roaming 的形态差异：**无状态纯数据查询面**——无 init、无 vtable、无
//! 常驻服务、无停机语义（装载即静止；进程终态无需 shutdown 调用，dlopen
//! 的库随进程消亡）。符号面 4 导出（abi_version / manifest / get / free），
//! 契约住 [`nemesis_plugin::resources`]。
//!
//! 装载链（任一步失败 = 诚实拒绝 + 留痕，绝不静默装坏库）：
//! 1. 名册开关闸：`config.plugin.native.json` 的 `plugin_resources` 键
//!    （roaming/vm/onnx/ui 同款义务；文件探测之前——关了就是不装）——
//!    关 = `DisabledBySwitch`（用户意志，非故障：注记 + 纯在线）；
//! 2. 在场探测：[`crate::plugin_lib`]（统一定义查找链）——缺席 = `Absent`
//!    （常态，静默；人格超市走纯在线，现有行为）；
//! 3. v4 签名闸：Valid 放行 / Unsigned dev warn 放行 release 拒 / 其余拒；
//! 4. dlopen → `Box::leak`（进程生命周期）；
//! 5. `prs_abi_version()` 握手：不匹配 = `Rejected`（WARN，宿主侧按缺席处理）；
//! 6. 3 符号解析（manifest / get / free——缺一即拒，不做部分装载）；
//! 7. 句柄注入 [`nemesis_plugin::resources::set_resources_plugin_handle`] 桥
//!    ——workspace 消费方（nemesis-web persona / CLI persona）peek 桥即可。
//!
//! **名册注册**（2026-11-12 用户拍板，覆盖设计文档 §4.7「无名册开关」旧
//! 拍板）：`NativePluginId::PluginResources` 全套注册——枚举 + ALL 目录 +
//! [`crate::native_plugins::resolve_load_path`] 特判臂 + Dashboard 本地
//! 插件库花名册（plugins handler native_entry）。未注册的库文件会被
//! `scan_unrecognized` 列为「未知库」——注册即身份，这也是开关与签名
//! 状态在清单页可见的前提。
//!
//! 装载时机：gateway 启动装配点一次 + CLI persona 命令按需（幂等：桥槽
//! 已有句柄直接 `Loaded`，不重复 dlopen）。消费侧**同步 FFI 直调**——纯
//! 数据查询无阻塞语义，无需 roaming 的 spawn_blocking 包装。

use nemesis_plugin::resources::{
    self, HOST_RESOURCES_ABI_VERSION, PRS_ABI_VERSION_SYMBOL, PRS_FREE_SYMBOL, PRS_GET_SYMBOL,
    PRS_MANIFEST_SYMBOL, ResourcesPluginHandle,
};

use crate::plugin_lib::{PluginLibSpec, SignatureVerdict, unsigned_dev_allow, verify_signature};

/// 装载结果：`Absent` = 插件文件缺席（常态，静默——消费方走纯在线）；
/// `Loaded` = 装载成功（含幂等重入）；`DisabledBySwitch` = 名册开关关
/// （用户意志，非故障——消费方按缺席处理）；`Rejected` = 在场但装载被拒
/// （签名/ABI/符号缺——原因已留痕，消费方按缺席处理 + WARN 已出）。
#[derive(Debug)]
pub enum ResourcesLoadOutcome {
    Absent,
    Loaded,
    DisabledBySwitch,
    Rejected(String),
}

// ---------------------------------------------------------------------------
// 装载主流程（单一真相源：gateway 启动 + CLI persona 双入口共用）
// ---------------------------------------------------------------------------

/// 装载 plugin_resources（幂等）。`config_dir` = workspace config 目录
/// （名册开关 `config.plugin.native.json` 所在；roaming 调用点同款推导：
/// [`nemesis_path::workspace_config_dir`]）。
pub fn load_resources_plugin(config_dir: &std::path::Path) -> ResourcesLoadOutcome {
    // ---- 名册开关闸（roaming 同款义务与同款次序：先于幂等/文件探测——
    //      关了就是不装，已装载态下也如实报 DisabledBySwitch） ----
    if !nemesis_config::plugins_native::load_enabled(
        config_dir,
        nemesis_config::plugins_native::NativePluginId::PluginResources,
    ) {
        tracing::info!(
            "[plugin-resources] 名册开关关（本地插件库 plugin_resources=disabled）\
             ——不装载，人格超市走纯在线"
        );
        return ResourcesLoadOutcome::DisabledBySwitch;
    }
    if resources::resources_plugin_handle().is_some() {
        return ResourcesLoadOutcome::Loaded;
    }
    let Some(path) = PluginLibSpec::RESOURCES.find_path() else {
        return ResourcesLoadOutcome::Absent;
    };

    // ---- 装载步（签名 → dlopen → ABI 握手 → 符号解析；独立函数供 ABI
    //      不匹配注入测试直调——绕开桥槽幂等，进程全局静态不影响） ----
    let handle = match load_library_handle(&path) {
        Ok(h) => h,
        Err(rejection) => return ResourcesLoadOutcome::Rejected(rejection),
    };

    // ---- 句柄注入桥（workspace 消费方 peek） ----
    let leaked: &'static ResourcesPluginHandle = Box::leak(Box::new(handle));
    resources::set_resources_plugin_handle(leaked);
    tracing::info!(
        "[plugin-resources] 装载成功（ABI v{}）：{}",
        HOST_RESOURCES_ABI_VERSION,
        path.display()
    );
    ResourcesLoadOutcome::Loaded
}

/// 纯装载步：签名闸 → dlopen → ABI 握手 → 3 符号解析。Err = 拒绝原因
/// （已留痕）。独立于名册开关/幂等/桥注入——ABI 不匹配注入测试直调本函数
/// （错版 fixture 永远进不了桥槽，无静态干扰）。
fn load_library_handle(path: &std::path::Path) -> Result<ResourcesPluginHandle, String> {
    // ---- 签名闸（roaming 同款四分支） ----
    match verify_signature(path) {
        SignatureVerdict::Valid => {}
        SignatureVerdict::Unsigned => {
            let msg = "plugin-resources 无 v4 签名（或 verify feature 关/锚缺席，无从验证）";
            if unsigned_dev_allow() {
                tracing::warn!(
                    "[plugin-resources] {msg}——开发形态 warn 放行（release 拒绝）：{}",
                    path.display()
                );
            } else {
                tracing::error!(
                    "[plugin-resources] {msg}，release 形态拒绝装载：{}",
                    path.display()
                );
                return Err(format!("{msg}，release 形态拒绝装载"));
            }
        }
        SignatureVerdict::HardReject(reason) => {
            tracing::error!(
                "[plugin-resources] v4 签名验证未过，拒绝装载（{}）：{reason}",
                path.display()
            );
            return Err(format!("签名验证未过：{reason}"));
        }
    }

    // ---- dlopen（进程生命周期，Box::leak 刻意泄漏——句柄 fn 指针恒有效） ----
    let lib = match unsafe { libloading::Library::new(path) } {
        Ok(l) => l,
        Err(e) => {
            let msg = format!("库加载失败（{}）：{e}", path.display());
            tracing::error!("[plugin-resources] {msg}");
            return Err(msg);
        }
    };
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));

    macro_rules! need_sym {
        ($sym:expr, $ty:ty) => {
            match unsafe { lib.get::<$ty>($sym.as_bytes()) } {
                Ok(s) => *s,
                Err(e) => {
                    let msg = format!("插件缺 {} 导出（ABI v1 冻结面不完整）：{e}", $sym);
                    tracing::error!("[plugin-resources] {msg}");
                    return Err(msg);
                }
            }
        };
    }

    // ---- ABI 握手（双向闸 host 半边） ----
    let abi_version: nemesis_plugin::resources::PrsAbiVersionFn = need_sym!(
        PRS_ABI_VERSION_SYMBOL,
        nemesis_plugin::resources::PrsAbiVersionFn
    );
    let plugin_abi = abi_version();
    if plugin_abi != HOST_RESOURCES_ABI_VERSION {
        let msg = format!(
            "插件 ABI 版本不匹配：插件 v{plugin_abi}，宿主 v{HOST_RESOURCES_ABI_VERSION}\
             （升级或降级 plugin-resources 到匹配版本）"
        );
        tracing::warn!("[plugin-resources] {msg}——按缺席处理（人格超市走纯在线）");
        return Err(msg);
    }

    // ---- 3 符号解析（冻结面完整——缺一即拒） ----
    Ok(ResourcesPluginHandle {
        manifest: need_sym!(PRS_MANIFEST_SYMBOL, nemesis_plugin::resources::PrsStrFn),
        get: need_sym!(PRS_GET_SYMBOL, nemesis_plugin::resources::PrsGetFn),
        free: need_sym!(PRS_FREE_SYMBOL, nemesis_plugin::resources::PrsFreeFn),
    })
}

/// 已装载句柄（`None` = 插件未装载——缺席/失败/ABI 不匹配）。
/// CLI/web 消费面统一走 [`nemesis_plugin::resources::resources_plugin_handle`]；
/// 本函数为 bin 侧对称 API（roaming plugin_handle 同款），现无 bin 调用点。
#[allow(dead_code)]
pub fn plugin_handle() -> Option<&'static ResourcesPluginHandle> {
    resources::resources_plugin_handle()
}

#[cfg(test)]
mod tests;
