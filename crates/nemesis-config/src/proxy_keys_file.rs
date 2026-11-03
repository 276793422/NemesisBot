//! `config.proxy_keys.json`——LLM 代理调用方 key（2026-11-03；用户裁决：
//! 不进 config.json，独立文件与 [`crate::proxy_file`] 同族同范式）。
//!
//! 语义（与 [`crate::proxy_file`] / [`crate::plugin_wasm`] 开关文件同范式）：
//! - 文件缺失 = 未配置零 key：读路径空表，**不落盘不生成**（无默认模板，
//!   onboard 不预置）；用户首次 keys.add 才创建。
//! - 鉴权回落读点（nemesis-web auth_middleware）消费 [`list`]：缺文件/
//!   损坏 = 空 key 表 + WARN——按无 key 处理回落主 token 校验，**绝不因
//!   配置问题锁死共享 token 老路径**。
//! - 变更操作（[`add_key`] / [`remove_key`] / [`set_enabled`]）= 整段 RMW
//!   罩 `config_write_lock`（BUG-20261103-333 纪律：锁取在函数首行覆盖
//!   读底 + 落盘全程，并发保存不丢更新）。
//! - 纯 serde_json 读写，不进 typed Config——独立文件红利同 proxy_file：
//!   彻底绕开 typed save 静默抹键的回归类（2026-09-03）。
//!
//! 热生效：写盘即对下一个请求生效（读点按需读盘 + mtime 缓存在消费侧，
//! 本模块不管缓存）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 一条调用方 key。`name` 唯一（trim 后非空）；`key` 唯一（trim 后非空，
/// 自动生成形态 `nbk_` + 32 hex，手填不限制格式）；`enabled`=false 时鉴权
/// 匹配跳过该条；`created_at` = Unix 秒（add 时补戳，0 = 未戳）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyKeyEntry {
    pub name: String,
    pub key: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub note: String,
}

fn default_enabled() -> bool {
    true
}

/// `config.proxy_keys.json` 顶层（`#[serde(default)]` 全键兜底半 schema）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyKeysFile {
    #[serde(default)]
    pub keys: Vec<ProxyKeyEntry>,
}

/// 配置文件路径：`<config_dir>/config.proxy_keys.json`（单一真相源）。
pub fn proxy_keys_config_path(config_dir: &Path) -> PathBuf {
    config_dir.join("config.proxy_keys.json")
}

/// 读取 key 文件。`Ok(None)` = 文件不存在（未配置；**绝不因此落盘**）；
/// `Err` = 文件在但损坏/不可读（调用方 WARN + 按未配置走）。
pub fn load(config_dir: &Path) -> Result<Option<ProxyKeysFile>, String> {
    let path = proxy_keys_config_path(config_dir);
    let content = match nemesis_utils::fsutil::read_config_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取 {} 失败：{e}", path.display())),
    };
    serde_json::from_str::<ProxyKeysFile>(&content)
        .map(Some)
        .map_err(|e| format!("代理 key 配置 {} 解析失败：{e}", path.display()))
}

/// 宽和读（鉴权回落读点消费）：缺文件/损坏 = 空 key 表 + WARN。**宁可
/// 无 key 回落主 token，不可让配置故障拦死正常调用**。
pub fn list(config_dir: &Path) -> Vec<ProxyKeyEntry> {
    match load(config_dir) {
        Ok(Some(f)) => f.keys,
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::warn!("[ProxyKeys] {e}；代理调用方 key 按空表处理");
            Vec::new()
        }
    }
}

/// 全量校验（save_all 前调；add_key 内部走逐条强校验 + 查重）。
pub fn validate_entries(keys: &[ProxyKeyEntry]) -> Result<(), String> {
    for (i, k) in keys.iter().enumerate() {
        if k.name.trim().is_empty() {
            return Err(format!("第 {} 条 key 的调用方名称（name）不能为空", i + 1));
        }
        if k.key.trim().is_empty() {
            return Err(format!("第 {} 条 key 的 key 值不能为空", i + 1));
        }
    }
    for i in 0..keys.len() {
        for j in (i + 1)..keys.len() {
            if keys[i].name.trim() == keys[j].name.trim() {
                return Err(format!("调用方名称重复：{}", keys[i].name.trim()));
            }
            if keys[i].key.trim() == keys[j].key.trim() {
                return Err("key 值重复（不同调用方不能共用同一 key）".to_string());
            }
        }
    }
    Ok(())
}

/// 全量替换保存（Dashboard 批量编辑兜底；常规变更走 RMW 三件套）。
pub fn save_all(config_dir: &Path, keys: &[ProxyKeyEntry]) -> Result<PathBuf, String> {
    validate_entries(keys)?;
    let _serial = crate::config_write_lock();
    write_locked(
        config_dir,
        &ProxyKeysFile {
            keys: keys.to_vec(),
        },
    )
}

/// 新增一条 key（RMW 罩锁）。name/key trim 后强校验非空 + 查重；
/// `created_at` 为 0 时自动补 Unix 秒戳。返回落盘路径。
pub fn add_key(config_dir: &Path, mut entry: ProxyKeyEntry) -> Result<PathBuf, String> {
    entry.name = entry.name.trim().to_string();
    entry.key = entry.key.trim().to_string();
    if entry.name.is_empty() {
        return Err("调用方名称（name）不能为空".to_string());
    }
    if entry.key.is_empty() {
        return Err("key 值不能为空".to_string());
    }
    let _serial = crate::config_write_lock();
    let mut file = read_for_write(config_dir);
    if file.keys.iter().any(|k| k.name == entry.name) {
        return Err(format!("调用方名称已存在：{}", entry.name));
    }
    if file.keys.iter().any(|k| k.key == entry.key) {
        return Err("key 值已存在（不同调用方不能共用同一 key）".to_string());
    }
    if entry.created_at == 0 {
        entry.created_at = now_secs();
    }
    file.keys.push(entry);
    write_locked(config_dir, &file)
}

/// 按名称删除 key。`Ok(true)` = 已删；`Ok(false)` = 名称不存在。
pub fn remove_key(config_dir: &Path, name: &str) -> Result<bool, String> {
    let _serial = crate::config_write_lock();
    let mut file = read_for_write(config_dir);
    let before = file.keys.len();
    file.keys.retain(|k| k.name != name);
    let removed = file.keys.len() < before;
    if removed {
        write_locked(config_dir, &file)?;
    }
    Ok(removed)
}

/// 按名称启停 key。`Ok(true)` = 已改；`Ok(false)` = 名称不存在。
pub fn set_enabled(config_dir: &Path, name: &str, enabled: bool) -> Result<bool, String> {
    let _serial = crate::config_write_lock();
    let mut file = read_for_write(config_dir);
    let Some(k) = file.keys.iter_mut().find(|k| k.name == name) else {
        return Ok(false);
    };
    if k.enabled == enabled {
        return Ok(true); // 幂等：状态不变不落盘
    }
    k.enabled = enabled;
    write_locked(config_dir, &file)?;
    Ok(true)
}

/// 写前读底：文件缺失 = 空底（首次保存创建）；损坏 = WARN + 空底重建
///（无从解析保留——proxy_file 损坏上写同口径）。
fn read_for_write(config_dir: &Path) -> ProxyKeysFile {
    match load(config_dir) {
        Ok(Some(f)) => f,
        Ok(None) => ProxyKeysFile::default(),
        Err(e) => {
            tracing::warn!("[ProxyKeys] {e}；保存时以空底重建");
            ProxyKeysFile::default()
        }
    }
}

/// 落盘（调用方必须已持 config_write_lock——RMW 全程罩锁，本函数只负责
/// 创建目录 + 序列化 + 原子写，不再自行取锁）。
fn write_locked(config_dir: &Path, file: &ProxyKeysFile) -> Result<PathBuf, String> {
    let path = proxy_keys_config_path(config_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let json = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), (json + "\n").as_bytes(), 0o600)
        .map_err(|e| format!("写入代理 key 配置失败：{e}"))?;
    Ok(path)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
