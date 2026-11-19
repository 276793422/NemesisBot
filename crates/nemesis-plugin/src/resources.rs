//! 附加资源插件契约（C-ABI 解冻第三例：纯数据资源载体）。
//!
//! 单文件契约：本文件是宿主侧真相源；**插件侧不持副本**（与 roaming/vm 的
//! 双向复制纪律不同——资源插件的 4 导出全为独立自由函数，无 vtable、无
//! 回调，契约面即本文件的常量与 fn 类型；插件 crate 依自身同形定义实现，
//! 两侧一致性由 ABI 版本握手 + loader 符号解析保证，无需 drift 测试）。
//!
//! # 线程纪律
//!
//! 全部导出为纯数据查询（OnceLock 只读索引 + 每调用独立分配）——任意线程
//! 并发调用安全，宿主**无需** spawn_blocking（roaming 的单缓冲串行纪律在
//! 此不存在）；返回串必须经 [`ResourcesPluginHandle::free`] 归还。
//!
//! # 返回约定
//!
//! `prs_manifest` / `prs_get` 返回单次分配的 `*mut c_char`（UTF-8 JSON），
//! 宿主用完调 `prs_free`；NULL = 插件内部失败（宿主按缺席降级）。
//! `prs_get` envelope：`{"ok":true,"content":...}` /
//! `{"ok":false,"err_code":"...","err_msg":"..."}`。
//!
//! # 版本史
//!
//! - v1（2026-11-12）：4 导出（abi_version / manifest / get / free）；
//!   首个资源种类 persona-catalog（人格超市上游快照）。

use std::os::raw::c_char;
use std::sync::OnceLock;

/// 宿主侧 ABI 版本（插件侧 `prs_abi_version` 返回值须相等——双向闸 host 半边）。
pub const HOST_RESOURCES_ABI_VERSION: u32 = 1;

/// 首个资源种类：人格目录（人格超市上游快照）。消费臂（nemesis-web persona /
/// CLI persona）按此 kind 找 manifest 条目——插件无该 kind = 诚实 unavailable。
pub const RESOURCES_KIND_PERSONA_CATALOG: &str = "persona-catalog";

// ---------------------------------------------------------------------------
// 导出符号名（loader need_sym 解析用；与插件 #[unsafe(no_mangle)] 名逐字对应）
// ---------------------------------------------------------------------------

pub const PRS_ABI_VERSION_SYMBOL: &str = "prs_abi_version";
pub const PRS_MANIFEST_SYMBOL: &str = "prs_manifest";
pub const PRS_GET_SYMBOL: &str = "prs_get";
pub const PRS_FREE_SYMBOL: &str = "prs_free";

// ---------------------------------------------------------------------------
// 导出 fn 类型
// ---------------------------------------------------------------------------

pub type PrsAbiVersionFn = extern "C" fn() -> u32;

/// 返回 manifest 全量 JSON（含各 kind 的 tree；树内路径，语义过滤在宿主消费侧）。
pub type PrsStrFn = unsafe extern "C" fn() -> *mut c_char;

/// `prs_get(kind, id)`：kind = 资源种类（如 persona-catalog），id = tree 内路径。
/// envelope JSON（成功/失败统一形状）；NULL = 插件内部失败。
pub type PrsGetFn = unsafe extern "C" fn(kind: *const c_char, id: *const c_char) -> *mut c_char;

/// 归还插件侧分配（NULL 容忍）。
pub type PrsFreeFn = unsafe extern "C" fn(p: *mut c_char);

/// 已装载插件句柄（fn 指针集）。Library 由 loader `Box::leak`（进程生命周期），
/// fn 指针因此恒有效；`Copy`（roaming 句柄同款——值传递不涉所有权转移）。
#[derive(Debug, Clone, Copy)]
pub struct ResourcesPluginHandle {
    pub manifest: PrsStrFn,
    pub get: PrsGetFn,
    pub free: PrsFreeFn,
}

impl ResourcesPluginHandle {
    /// 调 `prs_manifest` 并转 String（NULL = Err）。
    ///
    /// # Safety
    ///
    /// 句柄 fn 指针须来自 loader 完成符号解析与 ABI 握手的插件（loader 纪律）。
    pub unsafe fn manifest_str(&self) -> Result<String, String> {
        let p = unsafe { (self.manifest)() };
        if p.is_null() {
            return Err("prs_manifest 返回 NULL（插件内部失败）".to_string());
        }
        Ok(cstr_to_string(p, self.free))
    }

    /// 调 `prs_get(kind, id)` 并解 envelope → content / Err(带 err_code)。
    ///
    /// # Safety
    ///
    /// 同 [`Self::manifest_str`]。
    pub unsafe fn get_content(&self, kind: &str, id: &str) -> Result<String, String> {
        let k = std::ffi::CString::new(kind).map_err(|_| "kind 含 NUL 字节".to_string())?;
        let i = std::ffi::CString::new(id).map_err(|_| format!("id 含 NUL 字节: {id}"))?;
        let p = unsafe { (self.get)(k.as_ptr(), i.as_ptr()) };
        if p.is_null() {
            return Err("prs_get 返回 NULL（插件内部失败）".to_string());
        }
        let raw = cstr_to_string(p, self.free);
        parse_get_envelope(&raw)
    }
}

/// C 串 → String 并立即归还插件（单点：lossy 解码 + free）。
fn cstr_to_string(p: *mut c_char, free: PrsFreeFn) -> String {
    let s = unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned();
    unsafe { free(p) };
    s
}

/// prs_get envelope 解析（单一出口：ok 分支取 content；err 分支带 code+msg）。
pub fn parse_get_envelope(raw: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        // 字节切片会在多字节 UTF-8 字符中间 panic（宿主不崩红线）——按字符截断。
        let head: String = raw.chars().take(200).collect();
        format!("prs_get 返回非 JSON: {e}（raw 前 200 字符: {head}）")
    })?;
    if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
        return v
            .get("content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "prs_get envelope 缺 content 字段".to_string());
    }
    let code = v
        .get("err_code")
        .and_then(|c| c.as_str())
        .unwrap_or("unknown");
    let msg = v.get("err_msg").and_then(|m| m.as_str()).unwrap_or("");
    Err(format!("[{code}] {msg}"))
}

// ---------------------------------------------------------------------------
// manifest 解析（宿主侧消费形状——与插件 resource_filter 同形，serde 宽容）
// ---------------------------------------------------------------------------

/// 单资源条目（manifest.resources[] 元素）。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResourceEntry {
    pub kind: String,
    pub id: String,
    #[serde(default)]
    pub upstream_repo: String,
    pub upstream_commit: String,
    #[serde(default)]
    pub fetched_at_utc: String,
    #[serde(default)]
    pub file_count: usize,
    #[serde(default)]
    pub license: String,
    /// 路径清单（tree 内路径 = prs_get 的 id）。
    #[serde(default)]
    pub tree: Vec<String>,
}

/// 插件 manifest（prs_manifest JSON 的宿主侧形状）。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResourcesManifest {
    pub abi: u32,
    #[serde(default)]
    pub built_at_utc: String,
    /// `fetched`（REFRESH 直拉）/ `pre-downloaded`（FILE 脚本预下载）/
    /// `baseline`（仓库基线兜底）。
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub embed_source: String,
    #[serde(default)]
    pub resources: Vec<ResourceEntry>,
}

impl ResourcesManifest {
    /// 解析 manifest JSON（ABI 不匹配 / 损坏 → Err，消费方按缺席处理）。
    pub fn parse(raw: &str) -> Result<Self, String> {
        serde_json::from_str(raw).map_err(|e| format!("manifest 解析失败: {e}"))
    }

    /// 取指定 kind 的资源条目（首个命中；宿主消费臂据此判断插件是否承载
    /// 该 kind——无 = 诚实 unavailable）。
    pub fn entry(&self, kind: &str) -> Option<&ResourceEntry> {
        self.resources.iter().find(|r| r.kind == kind)
    }
}

impl ResourceEntry {
    /// 人读快照标签：`commit 前 7 位 @ YYYY-MM-DD`（前端徽标 / CLI stderr
    /// 注记 / refresh 诚实报错共用；fetched_at 缺失时只出 commit 段）。
    pub fn short_label(&self) -> String {
        let short = self.upstream_commit.chars().take(7).collect::<String>();
        let date = self.fetched_at_utc.split('T').next().unwrap_or("");
        if date.is_empty() {
            short
        } else {
            format!("{short} @ {date}")
        }
    }
}

// ---------------------------------------------------------------------------
// 快照访问层（web / CLI 消费臂共用的单一入口；语义过滤由消费方注入闭包
// ——「语义过滤留消费侧」纪律：插件侧只做 kind 定位与机械枚举）
// ---------------------------------------------------------------------------

/// 快照树枚举：定位 kind 条目 → 按 `filter`（消费方语义过滤）筛路径。
/// 插件缺席 / 无该 kind / manifest 损坏 / 过滤后为空 = `None`（纯在线）。
pub fn snapshot_tree_filtered(
    kind: &str,
    filter: impl Fn(&str) -> bool,
) -> Option<(Vec<String>, ResourceEntry)> {
    let raw = unsafe { resources_plugin_handle()?.manifest_str() }.ok()?;
    let manifest = ResourcesManifest::parse(&raw).ok()?;
    let entry = manifest.entry(kind)?;
    let tree: Vec<String> = entry.tree.iter().filter(|p| filter(p)).cloned().collect();
    if tree.is_empty() {
        None
    } else {
        Some((tree, entry.clone()))
    }
}

/// 快照读单文件（id = tree 内路径）。插件缺席 / envelope 错误全归 Err
/// （not_found 的真缺失语义由调用方与上游错误区分）。
pub fn snapshot_content(kind: &str, path: &str) -> Result<String, String> {
    let h = resources_plugin_handle().ok_or_else(|| "resources 插件未装载".to_string())?;
    unsafe { h.get_content(kind, path) }
}

// ---------------------------------------------------------------------------
// 进程内桥（bin 侧 loader 装载后注入；workspace 消费方 peek——roaming
// plugin_bridge 同款单向注入纪律）
// ---------------------------------------------------------------------------

static BRIDGE: OnceLock<&'static ResourcesPluginHandle> = OnceLock::new();

/// 注入已装载句柄（loader 专属：dlopen + ABI 握手 + 符号解析完成后调用）。
/// 'static 约束 = loader 侧 `Box::leak` 的刻意泄漏——句柄生命周期即进程。
pub fn set_resources_plugin_handle(h: &'static ResourcesPluginHandle) {
    let _ = BRIDGE.set(h); // 重复装载竞态：先到者赢，等价成功
}

/// 已注入句柄（`None` = 插件未装载/装载失败/ABI 不匹配——消费方诚实降级）。
pub fn resources_plugin_handle() -> Option<&'static ResourcesPluginHandle> {
    BRIDGE.get().copied()
}

#[cfg(test)]
mod tests;
