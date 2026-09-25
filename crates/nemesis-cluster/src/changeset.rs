//! 变更集（changeset）wire 类型与落盘/读取（看板项目档案 goal P4/E3+E8）。
//!
//! worker 任务终结时把「执行目录相对基线的改动」组装成变更集，**搭执行
//! 记录同一 outbox 载荷**（`payload/changeset/…`）——同一次分块传输原子
//! 落地，master 收到回执时变更集必已随行（保序：合并不可能抢在交付前）。
//!
//! 载荷布局（相对 outbox payload 根 / master 落地 files 根，同形）：
//!
//! ```text
//! changeset/
//! ├── changeset.json     本清单（base_commit + upserts/deletions 声明）
//! └── files/<rel>        各 upsert 文件原始内容（sha256 可核）
//! ```
//!
//! - **E8 归属**：`base_commit` 由 worker 申报（派发时 master 下发的 HEAD）；
//!   「是否属于活跃 dispatch 轮次」由 master 侧调用方按 dispatch 记录裁决，
//!   本模块只管 wire 形态 + 完整性核验。
//! - **D7 路径围栏**：清单声明路径在写入/读取两侧都过
//!   [`super::transfer::safe_relative_path`]（单点校验）。
//! - **完整性**：读取侧逐文件 SHA-256 核验（声明 sha256/size 双对账），
//!   任何失配 = 诚实 Err——合并绝不吃半套数据。

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::transfer::{safe_relative_path, sha256_hex};

/// 载荷内变更集子目录名（worker payload 与 master 落地侧同名同位）。
pub const CHANGESET_DIR_NAME: &str = "changeset";
/// 变更集清单文件名（变更集子目录内）。
pub const CHANGESET_MANIFEST_NAME: &str = "changeset.json";
/// 变更集文件内容子目录名（变更集子目录内）。
pub const CHANGESET_FILES_DIR: &str = "files";

/// 清单 schema 版本（当前唯一 1；读到更高版本 = 诚实拒绝）。
pub const CHANGESET_VERSION: u8 = 1;

// ---------------------------------------------------------------------------
// wire 类型
// ---------------------------------------------------------------------------

/// 变更集清单（`changeset.json`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChangesetManifest {
    pub version: u8,
    /// worker 申报的基线 commit hex（派发时 master 下发的 HEAD）。
    pub base_commit: String,
    pub upserts: Vec<ChangesetUpsert>,
    #[serde(default)]
    pub deletions: Vec<String>,
}

/// 变更集单文件 upsert 声明（内容在 `files/<path>`，此处只带指纹）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChangesetUpsert {
    /// `/` 分隔仓库相对路径。
    pub path: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub executable: bool,
}

/// 变更集单文件内容（读取侧解出；path 与声明一一对应）。
#[derive(Debug, Clone)]
pub struct ChangesetContent {
    pub path: String,
    pub content: Vec<u8>,
    pub executable: bool,
}

// ---------------------------------------------------------------------------
// 写入侧（worker：任务终结组装）
// ---------------------------------------------------------------------------

/// 组装变更集到 `dir`（`dir` = 变更集目录**本身**：落 `changeset.json` +
/// `files/<rel>` 平面布局；`changeset.json` 最后写——在场 = 完整，同 outbox
/// entry.json 的原子性约定）。入队时调用方（`enqueue_with_changeset`）把
/// 本目录整体拷为载荷内 `changeset/` 子目录，`read_changeset` 则从载荷根
/// 读——两侧约定不同属预期，勿"对齐"。
/// `contents` 与 `manifest.upserts` 必须一一对应（调用方排序一致；本函数
/// 逐条核 sha256/size 后落盘，防装配期错位）。
pub fn write_changeset(
    dir: &Path,
    manifest: &ChangesetManifest,
    contents: &[ChangesetContent],
) -> Result<(), String> {
    if manifest.version != CHANGESET_VERSION {
        return Err(format!(
            "变更集版本非法（期望 {CHANGESET_VERSION}）: {}",
            manifest.version
        ));
    }
    if manifest.upserts.len() != contents.len() {
        return Err(format!(
            "变更集声明 {} 条 upsert 与 {} 份内容不匹配",
            manifest.upserts.len(),
            contents.len()
        ));
    }
    let files_dir = dir.join(CHANGESET_FILES_DIR);
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(&files_dir).map_err(|e| format!("建变更集目录失败: {e}"))?;
    for (upsert, content) in manifest.upserts.iter().zip(contents.iter()) {
        if upsert.path != content.path {
            return Err(format!(
                "变更集内容错位：声明 {} 实得 {}",
                upsert.path, content.path
            ));
        }
        let rel = safe_relative_path(&upsert.path)?;
        let dest = files_dir.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("建 {} 父目录失败: {e}", upsert.path))?;
        }
        std::fs::write(&dest, &content.content)
            .map_err(|e| format!("写变更集文件 {} 失败: {e}", upsert.path))?;
        let got = sha256_hex(&content.content);
        if got != upsert.sha256 || content.content.len() as u64 != upsert.size {
            return Err(format!("变更集文件 {} 指纹/大小与声明不符", upsert.path));
        }
    }
    let json = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(CHANGESET_MANIFEST_NAME), json)
        .map_err(|e| format!("写变更集清单失败: {e}"))
}

// ---------------------------------------------------------------------------
// 读取侧（master：交付落定后写回路径消费）
// ---------------------------------------------------------------------------

/// 从载荷根读变更集（worker payload 目录 / master 落地 files 根同形）。
///
/// - 清单不在场 = `None`（纯执行记录交付，无变更集——合法形态）；
/// - 清单在场但任何一步失败 = `Some(Err)`（半套数据绝不进合并）；
/// - `Some(Ok)` = 清单 + 全部文件内容（逐条 SHA-256/大小核验通过）。
pub fn read_changeset(
    payload_root: &Path,
) -> Option<Result<(ChangesetManifest, Vec<ChangesetContent>), String>> {
    let manifest_path = payload_root
        .join(CHANGESET_DIR_NAME)
        .join(CHANGESET_MANIFEST_NAME);
    if !manifest_path.exists() {
        return None;
    }
    Some(read_changeset_inner(payload_root, &manifest_path))
}

fn read_changeset_inner(
    payload_root: &Path,
    manifest_path: &Path,
) -> Result<(ChangesetManifest, Vec<ChangesetContent>), String> {
    let raw =
        std::fs::read_to_string(manifest_path).map_err(|e| format!("读变更集清单失败: {e}"))?;
    let manifest: ChangesetManifest =
        serde_json::from_str(&raw).map_err(|e| format!("变更集清单解析失败: {e}"))?;
    if manifest.version != CHANGESET_VERSION {
        return Err(format!(
            "变更集版本非法（本端支持 {CHANGESET_VERSION}）: {}",
            manifest.version
        ));
    }
    // 路径围栏：清单声明路径先全量过闸（任何一条畸形 = 整个变更集拒收）。
    for u in &manifest.upserts {
        safe_relative_path(&u.path)?;
    }
    for d in &manifest.deletions {
        safe_relative_path(d)?;
    }
    let files_dir = payload_root
        .join(CHANGESET_DIR_NAME)
        .join(CHANGESET_FILES_DIR);
    let mut contents = Vec::with_capacity(manifest.upserts.len());
    for u in &manifest.upserts {
        let rel = safe_relative_path(&u.path)?;
        let path = files_dir.join(&rel);
        let data =
            std::fs::read(&path).map_err(|e| format!("变更集文件 {} 缺失或不可读: {e}", u.path))?;
        if data.len() as u64 != u.size {
            return Err(format!(
                "变更集文件 {} 大小失配（声明 {} 实得 {}）",
                u.path,
                u.size,
                data.len()
            ));
        }
        let got = sha256_hex(&data);
        if got != u.sha256 {
            return Err(format!("变更集文件 {} SHA-256 核验失败", u.path));
        }
        contents.push(ChangesetContent {
            path: u.path.clone(),
            content: data,
            executable: u.executable,
        });
    }
    Ok((manifest, contents))
}

#[cfg(test)]
mod tests;

// 覆盖率补充批次：写入/读取失败臂（版本、条数、父目录、指纹）。
#[cfg(test)]
mod cov_tests;
