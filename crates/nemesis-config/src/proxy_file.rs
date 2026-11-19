//! `config.proxy.json`——独立代理配置（2026-10-31 代理设置三项改造；用户裁决：
//! 不进 config.json，独立文件收「进程级环境变量代理 + 自更新下载代理」两类）。
//!
//! 语义（与 [`crate::plugin_wasm`] 开关文件同范式）：
//! - 文件缺失 = 未配置：读路径静默 None，**不落盘不生成**（无默认模板，
//!   onboard 不预置）；用户首次保存才创建。
//! - 空串 = 不设置该项（env 段=不碰该变量；selfupdate 段=直连）。
//! - 保存 = read-modify-write：env 段与 selfupdate 段各存各的互不覆盖。
//! - 保存时前缀校验（`http:// https:// socks5:// socks5h://`；
//!   no_proxy 例外=自由文本），非法 loud 拒绝不落盘。`socks://` 已除名
//!   （BUG-20261114-064）：reqwest/hyper-util 都不认这个 scheme——env 路径
//!   hyper-util `parse_env_uri` 对未知 scheme `return None` **静默忽略**（本
//!   进程客户端静默直连零诊断），显式路径 `Proxy::all` 构造期解析 Ok、连接
//!   期按 HTTP CONNECT 隧道打 SOCKS 端口每请求失败；保存期 loud 拒绝 +
//!   历史值读侧诚实降级是唯一闸口。
//! - 损坏 JSON：load 返回 `Err`（调用方 WARN + 按未配置走，不阻断启动）；
//!   损坏文件上保存 = 以空底重建（无从解析保留，plugin_wasm 同口径）。
//! - 纯 serde_json 读写，不进 typed Config——独立文件红利：彻底绕开 typed
//!   save 静默抹键的回归类（2026-09-03）。
//!
//! 生效时机（两段不同）：
//! - env 段：gateway 启动早期 `set_var` 注入进程环境（**重启生效**）——
//!   必须先于一切 reqwest client 构建；[`env_vars_to_apply`] 是注入清单的
//!   纯函数统一定义。
//! - selfupdate 段：更新下载/检查链路**调用时读盘**（下载是逐次操作，
//!   **保存即对下次生效**，免重启）——[`load_selfupdate_proxy`] 是唯一读点。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// env 段：进程环境变量代理（gateway 启动时注入，重启生效）。
/// 空串 = 不设置该变量（保持系统默认）。
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvProxyConfig {
    #[serde(default)]
    pub http_proxy: String,
    #[serde(default)]
    pub https_proxy: String,
    #[serde(default)]
    pub all_proxy: String,
    #[serde(default)]
    pub no_proxy: String,
}

// SEC-002 Debug 脱敏（BUG-20261114-063）：三个代理 URL 可携带 userinfo
// 凭据（http://user:pass@host:port——hyper-util from_env 与 reqwest
// Proxy::all 直接消费），与 ModelConfig.proxy 同脱敏面（BUG-20261111-068
// 口径）；no_proxy 是豁免主机列表（非凭据）原样输出。
impl std::fmt::Debug for EnvProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvProxyConfig")
            .field("no_proxy", &self.no_proxy)
            .field("http_proxy", &crate::debug_secret_value(&self.http_proxy))
            .field("https_proxy", &crate::debug_secret_value(&self.https_proxy))
            .field("all_proxy", &crate::debug_secret_value(&self.all_proxy))
            .finish()
    }
}

/// selfupdate 段：更新下载链路专用代理（调用时读取，保存即生效）。
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfupdateProxyConfig {
    #[serde(default)]
    pub proxy: String,
}

// SEC-002 Debug 脱敏（BUG-20261114-063）：同 EnvProxyConfig（userinfo 面）。
impl std::fmt::Debug for SelfupdateProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfupdateProxyConfig")
            .field("proxy", &crate::debug_secret_value(&self.proxy))
            .finish()
    }
}

/// `config.proxy.json` 顶层（两 section 各存各的；半 schema 缺键容错——
/// `#[serde(default)]` 全键兜底）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyFileConfig {
    #[serde(default)]
    pub env: EnvProxyConfig,
    #[serde(default)]
    pub selfupdate: SelfupdateProxyConfig,
}

/// 配置文件路径：`<config_dir>/config.proxy.json`。
pub fn proxy_config_path(config_dir: &Path) -> PathBuf {
    config_dir.join("config.proxy.json")
}

/// 代理地址前缀白名单（no_proxy 例外——排除列表是自由文本）。
/// 单一真相源：nemesis-web models handler 的 proxy 字段校验同表消费。
/// BUG-20261114-064：`socks://` 除名——reqwest（hyper-util `parse_env_uri`）
/// 只支持 http/https/socks4/socks4a/socks5/socks5h，"socks" scheme env 路径
/// 静默忽略、显式路径连接期按 HTTP CONNECT 打 SOCKS 端口失败。
pub const PROXY_PREFIXES: [&str; 4] = ["http://", "https://", "socks5://", "socks5h://"];

/// 单值校验：非空必须以白名单前缀开头（`is_no_proxy`=true 豁免）。
/// 空 = 合法（「不设置该项」语义）。
pub fn validate_proxy_value(value: &str, is_no_proxy: bool) -> Result<(), String> {
    let v = value.trim();
    if v.is_empty() || is_no_proxy {
        return Ok(());
    }
    if scheme_supported(v) {
        Ok(())
    } else {
        Err(format!(
            "代理地址必须以 {} 开头（留空 = 不设置）：{v}",
            PROXY_PREFIXES.join(" ")
        ))
    }
}

/// scheme 支持面判定（与 reqwest/hyper-util 实际支持的代理 scheme 对齐）：
/// 非空且以白名单前缀开头 = true。空值 = false（调用方按「未配置」先判空）。
/// 供注入点/读侧 WARN 用（BUG-20261114-064）。
pub fn scheme_supported(value: &str) -> bool {
    let v = value.trim();
    !v.is_empty() && PROXY_PREFIXES.iter().any(|p| v.starts_with(p))
}

/// 读取代理文件。`Ok(None)` = 文件不存在（未配置；调用方静默走系统默认，
/// **绝不因此落盘**）；`Err` = 文件在但损坏/不可读（调用方 WARN + 按未配置
/// 走，不阻断启动）。
pub fn load(config_dir: &Path) -> Result<Option<ProxyFileConfig>, String> {
    let path = proxy_config_path(config_dir);
    let content = match nemesis_utils::fsutil::read_config_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取 {} 失败：{e}", path.display())),
    };
    serde_json::from_str::<ProxyFileConfig>(&content)
        .map(Some)
        .map_err(|e| format!("代理配置 {} 解析失败：{e}", path.display()))
}

/// 便捷读：env 段（文件缺失/损坏 = 默认空——全不设置，损坏 WARN）。
pub fn load_env(config_dir: &Path) -> EnvProxyConfig {
    match load(config_dir) {
        Ok(Some(c)) => c.env,
        Ok(None) => EnvProxyConfig::default(),
        Err(e) => {
            tracing::warn!("[ProxyFile] {e}；环境变量代理按未配置处理");
            EnvProxyConfig::default()
        }
    }
}

/// 便捷读：selfupdate 段非空代理（文件缺失/损坏/空 = None 直连；损坏 WARN）。
/// 热生效语义的读点：下载是逐次操作，每次调用时读盘。
/// BUG-20261114-064：历史值读侧诚实降级——load 不校验既有配置（兼容性
/// 优先），scheme 不受 reqwest 支持（如 socks://）时交给下载链路 = 显式
/// Proxy 构造 Ok 但每请求按 HTTP CONNECT 打 SOCKS 端口失败；直连至少
/// 可预期。WARN 只报 scheme 不落完整 URL（userinfo 凭据面，SEC-002 口径）。
pub fn load_selfupdate_proxy(config_dir: &Path) -> Option<String> {
    match load(config_dir) {
        Ok(Some(c)) => {
            let p = c.selfupdate.proxy.trim().to_string();
            if p.is_empty() {
                return None;
            }
            if !scheme_supported(&p) {
                let scheme = p.split("://").next().unwrap_or("");
                tracing::warn!(
                    "[ProxyFile] 自更新代理 scheme \"{scheme}\" 不受 reqwest 支持（BUG-20261114-064），本次下载按直连处理；请改用 {} 或更新 config.proxy.json",
                    PROXY_PREFIXES.join(" ")
                );
                return None;
            }
            Some(p)
        }
        Ok(None) => None,
        Err(e) => {
            tracing::warn!("[ProxyFile] {e}；自更新代理按直连处理");
            None
        }
    }
}

/// 保存 env 段（read-modify-write：只动 env 段，selfupdate 段原样保留；
/// 文件不存在 = 首次创建——唯一合法的落盘时机）。写入前逐键校验。
/// 锁取在函数首行覆盖**整段 RMW**（读底 + 落盘）——
/// 旧形态锁只在 write 内取，读底段裸奔，并发两个保存（proxy.save 每请求
/// 独立 spawn）交错时后写者以自己读的旧底整体覆盖先写者（丢失更新）。
pub fn save_env(config_dir: &Path, env: &EnvProxyConfig) -> Result<PathBuf, String> {
    validate_env(env)?;
    let _serial = crate::config_write_lock();
    let mut file = read_for_write(config_dir);
    file.env = env.clone();
    write_locked(config_dir, &file)
}

/// 保存 selfupdate 段（read-modify-write：只动 selfupdate 段，env 段原样
/// 保留）。写入前校验。锁纪律同 save_env。
pub fn save_selfupdate(config_dir: &Path, proxy: &str) -> Result<PathBuf, String> {
    validate_proxy_value(proxy, false)?;
    let _serial = crate::config_write_lock();
    let mut file = read_for_write(config_dir);
    file.selfupdate = SelfupdateProxyConfig {
        proxy: proxy.trim().to_string(),
    };
    write_locked(config_dir, &file)
}

/// 非空值逐键前缀校验（no_proxy 豁免）。
fn validate_env(env: &EnvProxyConfig) -> Result<(), String> {
    validate_proxy_value(&env.http_proxy, false)?;
    validate_proxy_value(&env.https_proxy, false)?;
    validate_proxy_value(&env.all_proxy, false)?;
    validate_proxy_value(&env.no_proxy, true)
}

/// 写前读底：文件缺失 = 空底（首次保存创建）；损坏 = WARN + 空底重建
///（无从解析保留——与 plugin_wasm 损坏上写同口径）。
fn read_for_write(config_dir: &Path) -> ProxyFileConfig {
    match load(config_dir) {
        Ok(Some(c)) => c,
        Ok(None) => ProxyFileConfig::default(),
        Err(e) => {
            tracing::warn!("[ProxyFile] {e}；保存时以空底重建");
            ProxyFileConfig::default()
        }
    }
}

/// 落盘（调用方必须已持 config_write_lock—— 起 RMW 全程
/// 罩锁，本函数只负责创建目录 + 序列化 + 原子写，不再自行取锁）。
fn write_locked(config_dir: &Path, file: &ProxyFileConfig) -> Result<PathBuf, String> {
    let path = proxy_config_path(config_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let json = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), (json + "\n").as_bytes(), 0o600)
        .map_err(|e| format!("写入代理配置失败：{e}"))?;
    Ok(path)
}

/// env 段 → 待注入的环境变量对（纯函数，gateway 启动注入与单测共用单一
/// 真相源）。非空键产出大写+小写双对（hyper-util from_env 与 CLI 子进程
/// 两类读者都吃）；空串键不产出（不碰该变量）。顺序稳定
///（http → https → all → no）。
pub fn env_vars_to_apply(env: &EnvProxyConfig) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for (lower, upper, value) in [
        ("http_proxy", "HTTP_PROXY", &env.http_proxy),
        ("https_proxy", "HTTPS_PROXY", &env.https_proxy),
        ("all_proxy", "ALL_PROXY", &env.all_proxy),
        ("no_proxy", "NO_PROXY", &env.no_proxy),
    ] {
        let v = value.trim();
        if v.is_empty() {
            continue;
        }
        out.push((upper, v.to_string()));
        out.push((lower, v.to_string()));
    }
    out
}
