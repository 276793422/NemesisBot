//! WSAPI/UI 行为遥测（Forge 化重构批三，G6）。
//!
//! 此前 WSAPI 调用零持久化（ws_router 仅错误 warn、EventHub 1000 帧内存
//! 环形缓冲重启即丢）——「使用集群前先去诊断页点一遍」这类 UI 行为完全
//! 学不到。本模块在 router dispatch 点落盘调用流：`<ws>/logs/wsapi_usage.jsonl`
//! 每行 `{ts_ms, module, cmd, ok}`。
//!
//! 成本纪律：
//! - 全程 fire-and-forget（任何失败静默——遥测是学习侧旁路数据，绝不炸
//!   dispatch 主路径）；
//! - 同 (module, cmd) 30s 窗口去重（Dashboard 2-3s 轮询压缩成 30s 一条，
//!   8MB 上限 + 单代轮转 `.old`）；
//! - 写闸 = config `user_profile.ui_telemetry`（缺省 true；mtime 缓存，
//!   配置翻开关不重启生效）。关 = 不落盘（隐私面：存储即闸，不止不挖掘）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 同命令去重窗口（毫秒）。
const DEDUP_WINDOW_MS: i64 = 30_000;
/// 单文件轮转阈值（字节）。
const ROTATE_BYTES: u64 = 8 * 1024 * 1024;
/// 去重表容量上限（防 (module,cmd) 组合无界增长）。
const DEDUP_CAP: usize = 512;

/// (module, cmd) → 上次落盘 ts_ms（HashMap::new 非 const → LazyLock 包裹）。
static DEDUP: std::sync::LazyLock<parking_lot::Mutex<HashMap<(String, String), i64>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));
/// 写串行（多连接并发 dispatch 的 append 交错防线）。
static WRITE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
/// config 开关门控缓存：`(config_path, mtime_ns, enabled)`。
static GATE: parking_lot::Mutex<Option<(PathBuf, u128, bool)>> = parking_lot::Mutex::new(None);
/// 轮转判定的累计近似计数（省 stat：每 256 次写 stat 一次）。
static WRITE_COUNT: AtomicU64 = AtomicU64::new(0);

/// dispatch 点遥测入口（web ws_router 调用；forge feature 闸在调用侧）。
///
/// `workspace`/`home` 任一缺失 = 跳过（workspace 定落盘路径、home 定
/// config 门控路径——Dashboard 链路两者齐备）。
pub fn record_wsapi_call(
    workspace: Option<&str>,
    home: Option<&str>,
    module: &str,
    cmd: &str,
    ok: bool,
) {
    let (Some(workspace), Some(home)) = (workspace, home) else {
        return;
    };
    if !telemetry_enabled(home) {
        return;
    }
    // 去重：同命令 30s 窗口一条（轮询压缩；真实重复点击 >30s 间隔照记）。
    let now_ms = chrono::Utc::now().timestamp_millis();
    {
        let mut dedup = DEDUP.lock();
        let key = (module.to_string(), cmd.to_string());
        if let Some(last) = dedup.get(&key)
            && now_ms - *last < DEDUP_WINDOW_MS
        {
            return;
        }
        if dedup.len() >= DEDUP_CAP {
            dedup.clear(); // 简单防爆策略：清空重来（学习侧容忍）
        }
        dedup.insert(key, now_ms);
    }
    let line = serde_json::json!({
        "ts_ms": now_ms,
        "module": module,
        "cmd": cmd,
        "ok": ok,
    });
    let dir = Path::new(workspace).join("logs");
    let path = dir.join("wsapi_usage.jsonl");
    // 写路径串行 + fire-and-forget。
    let _w = WRITE_LOCK.lock();
    // 周期性轮转检查（每 256 次写 stat 一次，平时零 stat 成本）。
    if WRITE_COUNT
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(256)
    {
        rotate_if_oversized(&path);
    }
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    use std::io::Write;
    let _ = writeln!(f, "{line}");
}

/// config `user_profile.ui_telemetry` 门控（mtime 缓存：开关翻转不重启
/// 生效；config 读取失败 = 当关处理，不落盘）。
fn telemetry_enabled(home: &str) -> bool {
    let cfg_path = Path::new(home).join("config.json");
    let mtime = std::fs::metadata(&cfg_path)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        });
    let Some(mtime) = mtime else {
        return false; // config 不存在 = 学习未配置，不落盘
    };
    {
        let gate = GATE.lock();
        if let Some((p, m, enabled)) = gate.as_ref()
            && p == &cfg_path
            && *m == mtime
        {
            return *enabled;
        }
    }
    let enabled = nemesis_config::load_config(&cfg_path)
        .ok()
        .and_then(|cfg| cfg.user_profile)
        .map(|u| u.ui_telemetry_enabled_or(true))
        .unwrap_or(true);
    *GATE.lock() = Some((cfg_path, mtime, enabled));
    enabled
}

/// 超限轮转：`wsapi_usage.jsonl` → `wsapi_usage.jsonl.old`（单代，覆盖）。
pub(crate) fn rotate_if_oversized(path: &PathBuf) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() < ROTATE_BYTES {
        return;
    }
    let mut old = path.clone().into_os_string();
    old.push(".old");
    let _ = std::fs::rename(path, PathBuf::from(old));
}

#[cfg(test)]
mod tests;
