//! Swarm M3 资产 RPC 兜底通路（2026-09-20，提供方侧）。
//!
//! 跨网段场景：资产引用束的 `node_url` 是提供方 HTTP 基址，消费方与
//! 提供方不在同一网段时 HTTP 直连根本不可达（连接级失败）。本模块在
//! **集群 RPC 通道**上补一条兜底路：`asset.meta`（元信息）+ `asset.chunk`
//! （256KB 分块读盘，base64 进 JSON——与 transfer.rs 同款形态，1MiB 服务端
//! 钳制内 16MB 帧上限富余）。复用集群连接零新增网络设施；资产完整性
//! 仍以 bundle.sha256 为准（消费方落盘前照校验）。
//!
//! 验证链与 web 下载端点（handlers/board_asset.rs）**同构**：
//! sanitize → 表登记白名单（未登记 ref 一律「不存在」，防枚举）→
//! HMAC+过期验签 → 文件存在。密钥读本节点 `asset_secret.key`
//! （load-or-create 幂等，与 publish/下载端点同一文件）；每次请求独立
//! 验签（无状态纯函数，token 即授权——与 HTTP query 完全同构）。
//!
//! 注册面：gateway 装配（board+cluster 双 feature，每节点都注册——
//! 任何节点都是潜在提供方，worker 产物反走同一条路）。集群未启动时
//! 注册失败静默（与 peer_chat 同款忽略策略）。

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use nemesis_board::BoardStore;
use nemesis_board::asset_token::{
    AssetTokenError, load_or_create_secret, sanitize_asset_ref, verify_asset_token,
};

/// RPC action：元信息（实际文件 size + 登记时 sha256）。
pub const ACTION_ASSET_META: &str = "asset.meta";
/// RPC action：分块读取（`{asset_ref, asset_token, expires_at, offset, len}`
/// → `{data(base64), eof}`）。
pub const ACTION_ASSET_CHUNK: &str = "asset.chunk";

/// 服务端单块读取上限。客户端按 256KB 请求；这里钳到 1MiB——transfer.rs
/// 同款量级（base64 后 ~1.4MB JSON，16MB 帧上限富余），防一次大读占满帧。
pub const MAX_CHUNK_LEN: u64 = 1024 * 1024;

/// handler 依赖：workspace（资产目录/密钥路径解析）+ 本地资产登记表
/// （表白名单——与 web 下载端点同构；None = 诚实拒绝「registry 不可用」，
/// 不退化为无白名单服务）。
#[derive(Clone)]
pub(crate) struct AssetRpcDeps {
    pub workspace: PathBuf,
    pub board_store: Option<Arc<BoardStore>>,
}

/// payload 字符串字段提取（trim + 非空）。
fn str_field(payload: &serde_json::Value, key: &str) -> Result<String, String> {
    let s = payload
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if s.is_empty() {
        return Err(format!("missing or empty {key}"));
    }
    Ok(s)
}

/// 验证链（meta/chunk 共用）：参数提取 → ref 白名单 → 表登记白名单 →
/// HMAC+过期 → 文件存在。返回（文件路径，登记行）。
fn authorize(
    deps: &AssetRpcDeps,
    payload: &serde_json::Value,
) -> Result<(PathBuf, nemesis_board::models::BoardAsset), String> {
    let asset_ref = str_field(payload, "asset_ref")?;
    let token = str_field(payload, "asset_token")?;
    let expires_at = payload
        .get("expires_at")
        .and_then(|v| v.as_i64())
        .ok_or("missing or non-integer expires_at")?;

    // 字符白名单先行（与下载端点同序）：非法名根本不值得查表。
    sanitize_asset_ref(&asset_ref)?;

    let secret = load_or_create_secret(&nemesis_path::resolve_asset_secret_path_in_workspace(
        &deps.workspace,
    ))?;
    let store = deps
        .board_store
        .as_ref()
        .ok_or("asset registry not available on this node")?;
    // 表登记白名单：未登记的 ref 一律「不存在」（防枚举，下载端点同款）。
    let asset = match store.lookup_asset(&asset_ref)? {
        Some(a) => a,
        None => return Err("asset not found".into()),
    };

    // HMAC + 过期（常量时间比对在 verify 内部）。
    match verify_asset_token(&secret, &asset_ref, expires_at, &token) {
        Ok(()) => {}
        Err(AssetTokenError::Expired) => {
            return Err("asset token expired — request a fresh reference".into());
        }
        Err(AssetTokenError::Invalid) => return Err("asset token invalid".into()),
    }

    let file =
        nemesis_path::resolve_board_assets_dir_in_workspace(&deps.workspace).join(&asset_ref);
    if !file.is_file() {
        return Err("asset registered but content missing on disk".into());
    }
    Ok((file, asset))
}

/// 构造 `asset.meta` handler：`{asset_ref, asset_token, expires_at}` →
/// `{size, sha256}`。sha256 回**登记值**（publish 时算过，不重复全文件
/// 哈希）；消费方拿它与 bundle.sha256 比对，不一致即提供方文件漂移。
pub(crate) fn build_meta_handler(deps: AssetRpcDeps) -> nemesis_cluster::rpc::server::RpcHandlerFn {
    Box::new(move |payload| {
        let (file, asset) = authorize(&deps, &payload)?;
        let size = std::fs::metadata(&file)
            .map_err(|e| format!("stat {}: {e}", file.display()))?
            .len();
        Ok(serde_json::json!({ "size": size, "sha256": asset.sha256 }))
    })
}

/// 构造 `asset.chunk` handler：`{..., offset, len}` → `{data(base64), eof}`。
/// len 钳到 [`MAX_CHUNK_LEN`]；offset 越界诚实报错（客户端按 size 步进，
/// 正常循环不会越界——越界即参数错，不静默截断）。越界判定用**实际文件
/// 长度**（与 asset.meta 回的 size 同源——登记值可能滞后于盘上实体，两处
/// 必须一致，否则消费方按 meta.size 步进会在登记边界被 chunk 拒绝）。
pub(crate) fn build_chunk_handler(
    deps: AssetRpcDeps,
) -> nemesis_cluster::rpc::server::RpcHandlerFn {
    Box::new(move |payload| {
        let (file, _asset) = authorize(&deps, &payload)?;
        let offset = payload
            .get("offset")
            .and_then(|v| v.as_u64())
            .ok_or("missing or non-integer offset")?;
        let want = payload
            .get("len")
            .and_then(|v| v.as_u64())
            .ok_or("missing or non-integer len")?;
        if want == 0 {
            return Err("len must be > 0".into());
        }
        let len = want.min(MAX_CHUNK_LEN);
        let file_size = std::fs::metadata(&file)
            .map_err(|e| format!("stat {}: {e}", file.display()))?
            .len();
        if offset >= file_size {
            return Err(format!("offset {offset} beyond asset size {file_size}"));
        }

        let mut f =
            std::fs::File::open(&file).map_err(|e| format!("open {}: {e}", file.display()))?;
        f.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("seek {}: {e}", file.display()))?;
        let mut buf = vec![0u8; len as usize];
        let mut filled: u64 = 0;
        while filled < len {
            let n = f
                .read(&mut buf[filled as usize..])
                .map_err(|e| format!("read {}: {e}", file.display()))?;
            if n == 0 {
                break;
            }
            filled += n as u64;
        }
        buf.truncate(filled as usize);
        let eof = offset + filled >= file_size;
        Ok(serde_json::json!({
            "data": nemesis_cluster::transfer::b64_encode(&buf),
            "eof": eof,
        }))
    })
}

#[cfg(test)]
mod tests;
