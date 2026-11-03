//! LLM 代理调用方 key 鉴权面（2026-11-03，docs/PLAN/2026-11-03_LLM代理调用方key管理.md）。
//!
//! 共享主 token 不区分调用方；本模块给代理面加 per-caller key：
//! auth_middleware 在主 token 校验**之前**查本面（命中即按代理路径白名单
//! 放行并注入 [`ProxyCaller`] 身份；不命中回落主 token 老校验，行为不变）。
//!
//! 设计铁律：
//! - **不动 AppState**（加字段会连锁改 ~123 处测试构造点）——config 路径
//!   从 `state.workspace`（生产装配 = `<home>/workspace`）推导，key 表按需
//!   读盘 + mtime 单条目缓存（写盘即对下一请求生效，热生效不重启）。
//! - **路径白名单只开 LLM 代理两条 lane**——代理 key 打 Dashboard 其他 API
//!   一律不匹配（回落主 token 校验必 401），防借道提权读会话/文件/配置。
//!   白名单是**前置内存闸**：Dashboard 路径连盘都不 stat。
//! - **宁可无 key 回落主 token，不可让配置故障拦死老路径**——缺文件/
//!   损坏 = 空 key 表（宽和读在 `nemesis-config::proxy_keys_file::list`）。
//! - key 比对复用 [`crate::api_handlers::verify_token`]（XOR 恒时比较，
//!   比较语义单一真相源）；空 token 不参与匹配（防空串撞库）。

use crate::api_handlers::AppState;
use nemesis_config::proxy_keys_file::{self, ProxyKeyEntry};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

/// 代理调用方身份（auth_middleware 命中代理 key 后插入 request extensions；
/// 转发面 handler 经 `Option<axum::Extension<ProxyCaller>>` 提取——缺席 =
/// 共享 token 老形态，session_key 不带 caller 段）。`Arc<String>` 跨
/// async_stream 'static 块 clone 廉价。
#[derive(Debug, Clone)]
pub(crate) struct ProxyCaller(pub Arc<String>);

/// 代理 key 放行的路径白名单（exact match）——`/v1/models` 端点尚不存在，
/// 不虚列；新增代理面端点时同步扩此表。
const PROXY_KEY_ALLOWED_PATHS: [&str; 2] = ["/v1/chat/completions", "/api/chat/stream"];

/// mtime 单条目缓存：路径 → (mtime, key 表)。同进程通常只服务一个
/// workspace；路径换键 = 缓存未命中即重读（正确性不受影响）。mtime 取自
/// `write_file_atomic` 新建文件（每次保存必换 mtime）。
static CACHE: parking_lot::Mutex<Option<(PathBuf, SystemTime, Arc<Vec<ProxyKeyEntry>>)>> =
    parking_lot::Mutex::new(None);

/// 鉴权回落：token 匹配某条 enabled 代理 key 且路径在白名单内 →
/// `Some(ProxyCaller)`；否则 `None`（调用方回落主 token 校验）。
pub(crate) fn match_proxy_caller(state: &AppState, token: &str, path: &str) -> Option<ProxyCaller> {
    // 空 token 不参与 key 匹配（key 均非空，防空串撞库）。
    if token.is_empty() {
        return None;
    }
    // 白名单前置内存闸：Dashboard 路径连盘都不 stat（白名单外一律不查）。
    if !PROXY_KEY_ALLOWED_PATHS.contains(&path) {
        return None;
    }
    let keys = cached_keys(state)?;
    for k in keys.iter().filter(|k| k.enabled) {
        // 恒时比较复用 verify_token（比较语义单一真相源）。
        if crate::api_handlers::verify_token(k.key.trim(), token) {
            return Some(ProxyCaller(Arc::new(k.name.clone())));
        }
    }
    None
}

/// 代理 lane 记账会话键（proxy_usage 落账 session_key）：带 caller =
/// `proxy:{caller}:{lane}:{ids}`（使用统计页按调用方分行）；无 caller =
/// 旧形态 `proxy:{lane}:{ids}`（共享 token 部署账面不变）。
pub(crate) fn proxy_lane_session_key(
    caller: Option<&ProxyCaller>,
    lane: &str,
    ids: impl std::fmt::Display,
) -> String {
    match caller {
        Some(c) => format!("proxy:{}:{lane}:{ids}", c.0),
        None => format!("proxy:{lane}:{ids}"),
    }
}

/// key 表读盘 + mtime 缓存。workspace 未装配 / key 文件不存在 = None
///（调用方回落主 token 校验；前者顺带清缓存防陈旧）。
fn cached_keys(state: &AppState) -> Option<Arc<Vec<ProxyKeyEntry>>> {
    let config_dir = state
        .workspace
        .as_ref()
        .map(|w| PathBuf::from(w).join("config"))?;
    let path = proxy_keys_file::proxy_keys_config_path(&config_dir);
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let Some(mtime) = mtime else {
        // 文件不存在 = 零 key；顺带清缓存（文件被删后不再持陈旧表）。
        *CACHE.lock() = None;
        return None;
    };
    {
        let cache = CACHE.lock();
        if let Some((cached_path, cached_mtime, keys)) = cache.as_ref()
            && cached_path == &path
            && *cached_mtime == mtime
        {
            return Some(keys.clone());
        }
    }
    // miss → 重读（list 宽和读：损坏 = 空 + WARN，回落主 token 老路径）。
    let keys = Arc::new(proxy_keys_file::list(&config_dir));
    *CACHE.lock() = Some((path, mtime, keys.clone()));
    Some(keys)
}
