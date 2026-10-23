//! payload_file —— payload DLL 落盘与版本戳（T1.6）。
//!
//! 决议 9：payload 双架构 DLL 编译期内嵌（feature 关 = 零字节），**首次使用**
//! 写到 `<workspace>/tools/ziwei/`，`ziwei.payload_dir` 可显式覆盖。
//!
//! 版本戳纪律：`<name>.stamp` 存编译期 sha256 快照；落盘前先读现有文件哈希
//! ——一致复用，不一致（payload 升级/被篡改）重写。嵌缺席且文件不在 = 诚实
//! 报错（不现场编译，不静默吞）。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::embed;

#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    #[error(
        "payload（{arch}）未内嵌且磁盘缺席：先构建 plugins/ziwei-agent-dll（双架构）或配置 ziwei.payload_dir 指向现成 DLL"
    )]
    Missing { arch: String },
    #[error("payload 落盘失败（{path}）: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("payload 大小越界（{arch} {size} 字节 > 1MB 闸）")]
    TooBig { arch: String, size: usize },
}

/// 单架构 payload 解析结果。
#[derive(Debug)]
pub struct ResolvedPayload {
    pub path: PathBuf,
    pub sha256: String,
}

fn sha256_hex(data: &[u8]) -> String {
    let d = Sha256::digest(data);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// 1MB / 架构 体积闸（决议 9）。
const PAYLOAD_SIZE_CAP: usize = 1 << 20;

/// 确保 payload 在磁盘上，返回绝对路径（attach 用）。
///
/// - `payload_dir_override`：config `ziwei.payload_dir`（直接用现成文件，不写不校验内嵌一致性）；
/// - `workspace`：工作区根（落 `<workspace>/tools/ziwei/`）。
pub fn ensure_payload(
    workspace: &Path,
    payload_dir_override: Option<&str>,
    arch: &str, // "x64" | "x86"
) -> Result<ResolvedPayload, PayloadError> {
    let file_name = format!("ziwei_agent_{arch}.dll");

    if let Some(dir) = payload_dir_override {
        let path = Path::new(dir).join(&file_name);
        let bytes = std::fs::read(&path).map_err(|e| PayloadError::Io {
            path: path.clone(),
            source: e,
        })?;
        return Ok(ResolvedPayload {
            path,
            sha256: sha256_hex(&bytes),
        });
    }

    // 内嵌字节（build.rs 编译期快照）
    let (embedded, embed_sha) = match arch {
        "x64" => (embed::PAYLOAD_X64_BYTES, embed::PAYLOAD_X64_SHA256),
        "x86" => (embed::PAYLOAD_X86_BYTES, embed::PAYLOAD_X86_SHA256),
        other => {
            return Err(PayloadError::Missing {
                arch: other.to_string(),
            });
        }
    };

    let dir = workspace.join("tools").join("ziwei");
    let path = dir.join(&file_name);

    if embedded.is_empty() {
        // 未内嵌：磁盘有现成的（上次落盘/手动放置）→ 直接用，但注记来源
        if path.is_file() {
            let bytes = std::fs::read(&path).map_err(|e| PayloadError::Io {
                path: path.clone(),
                source: e,
            })?;
            tracing::warn!(
                "[ziwei] payload 未内嵌，使用磁盘现存 {}（{}字节，无编译期一致性锚）",
                path.display(),
                bytes.len()
            );
            return Ok(ResolvedPayload {
                path,
                sha256: sha256_hex(&bytes),
            });
        }
        return Err(PayloadError::Missing {
            arch: arch.to_string(),
        });
    }

    if embedded.len() > PAYLOAD_SIZE_CAP {
        return Err(PayloadError::TooBig {
            arch: arch.to_string(),
            size: embedded.len(),
        });
    }

    // 版本戳：现有文件哈希 == 内嵌快照 → 复用；否则原子重写（tmp+rename）。
    let need_write = match std::fs::read(&path) {
        Ok(existing) => sha256_hex(&existing) != embed_sha,
        Err(_) => true,
    };
    if need_write {
        std::fs::create_dir_all(&dir).map_err(|e| PayloadError::Io {
            path: dir.clone(),
            source: e,
        })?;
        let tmp = dir.join(format!(
            ".{file_name}.tmp{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&tmp, embedded).map_err(|e| PayloadError::Io {
            path: tmp.clone(),
            source: e,
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| PayloadError::Io {
            path: path.clone(),
            source: e,
        })?;
        tracing::info!(
            "[ziwei] payload 落盘 {}（{}字节 sha256={embed_sha}）",
            path.display(),
            embedded.len()
        );
    }
    Ok(ResolvedPayload {
        path,
        sha256: embed_sha.to_string(),
    })
}

#[cfg(test)]
mod tests;
