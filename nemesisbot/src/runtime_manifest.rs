//! A1 运行时完整性清单（工程模式专项，2026-10-19）。
//!
//! 打包部署（build-windows.bat / build-linux.sh / build-android.bat /
//! customize 流程）在产物目录落 `runtime-manifest.json`
//! （`{version, git_commit, generated_at, files: [{path, sha256, size}]}`，
//! 生成器单一真相源 = `scripts/gen_runtime_manifest.py`），覆盖主二进制 +
//! `static/` 前端全量文件。gateway 启动早期逐件校验 sha256。
//!
//! 三态语义（刻意设计，与打包硬拒语义的分歧保 cargo 开发态可用）：
//! - manifest 不在 → 一次性 warn（非打包构建 / manifest 缺失），**不阻塞**；
//! - manifest 在且全过 → debug 日志一行；
//! - manifest 在但有缺件 / hash 不符 → **响亮指名报错**（哪个文件、期望/
//!   实际 sha256）+ 拒绝启动；`gateway --skip-runtime-check` 显式逃生门
//!   （运维场景必须显式输入，无静默旁路）。
//!
//! 半残部署检测面：换装中断（static 部分覆盖）、predeploy 备份残留、
//! 手工拷贝漏件——启动期响亮定性，不再靠运行时诡异故障倒查。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 清单文件名（产物目录根；生成器与校验器同源）。
pub const MANIFEST_FILENAME: &str = "runtime-manifest.json";

/// 单文件清单条目。`path` 为相对产物目录根的正斜杠路径（跨平台稳定，
/// 不随打包平台路径分隔符漂移）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// runtime-manifest.json 顶层结构。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeManifest {
    pub version: String,
    pub git_commit: String,
    pub generated_at: String,
    pub files: Vec<ManifestEntry>,
}

/// 校验违例（指名定性：哪个文件、期望什么、实际什么）。
#[derive(Debug, Clone, PartialEq)]
pub enum ManifestViolation {
    /// 清单列出的文件在盘上缺失。
    Missing { path: String },
    /// 文件在但 sha256 不符（篡改 / 部分写入 / 版本错配）。
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
}

impl std::fmt::Display for ManifestViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { path } => write!(f, "missing file: {path}"),
            Self::HashMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "hash mismatch: {path} (expected sha256 {expected}, actual {actual})"
            ),
        }
    }
}

/// 启动校验三态结果。
#[derive(Debug, Clone, PartialEq)]
pub enum StartupCheck {
    /// manifest 不在（非打包构建 / manifest 缺失）——warn 不阻塞。
    Absent,
    /// manifest 在且全量通过。
    Verified { files: usize },
    /// manifest 在但有违例——调用方（gateway）必须拒绝启动。
    Failed(Vec<ManifestViolation>),
}

/// sha256 小写 hex（校验器与生成器共享同一编码形态）。
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 读取产物目录下的 runtime-manifest.json。
///
/// 返回 `Ok(None)` = 文件不存在（Absent 态）；IO 读取错误（存在但读不了/
/// 坏 JSON）按存在处理返回 Err——半残现场响亮定性，不当 Absent 静默放行。
pub fn load_manifest(deploy_dir: &Path) -> Result<Option<RuntimeManifest>, String> {
    let path = deploy_dir.join(MANIFEST_FILENAME);
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let manifest: RuntimeManifest =
        serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    Ok(Some(manifest))
}

/// 逐件校验：对清单每条目查存在 + sha256。顺序 = 清单文件序（稳定报告）。
pub fn verify_deploy_dir(deploy_dir: &Path, manifest: &RuntimeManifest) -> Vec<ManifestViolation> {
    let mut violations = Vec::new();
    for entry in &manifest.files {
        let file_path = deploy_dir.join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let bytes = match std::fs::read(&file_path) {
            Ok(b) => b,
            Err(_) => {
                violations.push(ManifestViolation::Missing {
                    path: entry.path.clone(),
                });
                continue;
            }
        };
        let actual = sha256_hex(&bytes);
        if actual != entry.sha256.to_lowercase() {
            violations.push(ManifestViolation::HashMismatch {
                path: entry.path.clone(),
                expected: entry.sha256.clone(),
                actual,
            });
        }
    }
    violations
}

/// 启动校验入口（三态聚合）：load + verify。
pub fn check_deploy_dir(deploy_dir: &Path) -> Result<StartupCheck, String> {
    match load_manifest(deploy_dir)? {
        None => Ok(StartupCheck::Absent),
        Some(manifest) => {
            let violations = verify_deploy_dir(deploy_dir, &manifest);
            if violations.is_empty() {
                Ok(StartupCheck::Verified {
                    files: manifest.files.len(),
                })
            } else {
                Ok(StartupCheck::Failed(violations))
            }
        }
    }
}

/// gateway 启动早期调用：按三态语义落日志；`Failed` = Err 拒启。
///
/// `skip`（`gateway --skip-runtime-check`）= 显式逃生门：跳过全部检查
/// （含 Absent warn），debug 一行留痕。
pub fn check_startup(skip: bool) -> Result<(), anyhow::Error> {
    if skip {
        tracing::debug!("[runtime-manifest] check skipped (--skip-runtime-check)");
        return Ok(());
    }
    let Some(deploy_dir) = exe_deploy_dir() else {
        return Ok(()); // exe 路径解析失败（极端嵌入场景）= Absent 语义
    };
    match check_deploy_dir(&deploy_dir) {
        Ok(StartupCheck::Absent) => {
            tracing::warn!(
                "[runtime-manifest] {} not found next to the executable — \
                 non-packaged build or incomplete deployment; skipping integrity check",
                MANIFEST_FILENAME
            );
            Ok(())
        }
        Ok(StartupCheck::Verified { files }) => {
            tracing::debug!("[runtime-manifest] verified {files} files");
            Ok(())
        }
        Ok(StartupCheck::Failed(violations)) => {
            eprintln!(
                "[runtime-manifest] RUNTIME INTEGRITY CHECK FAILED ({} violation(s)) — refusing to start.",
                violations.len()
            );
            eprintln!(
                "[runtime-manifest] The deployment next to {} is incomplete or modified.",
                deploy_dir.display()
            );
            for v in &violations {
                eprintln!("[runtime-manifest]   - {v}");
            }
            eprintln!(
                "[runtime-manifest] Reinstall / redeploy the full bundle, or pass \
                 `gateway --skip-runtime-check` to explicitly bypass this check."
            );
            tracing::error!(
                "[runtime-manifest] integrity check failed ({} violation(s)): {violations:?}",
                violations.len()
            );
            anyhow::bail!("runtime integrity check failed; see stderr for details")
        }
        Err(e) => {
            // manifest 在但读不了 / 坏 JSON：同 Failed 语义（响亮拒启）。
            eprintln!("[runtime-manifest] RUNTIME MANIFEST UNREADABLE — refusing to start.");
            eprintln!("[runtime-manifest]   - {e}");
            tracing::error!("[runtime-manifest] manifest unreadable: {e}");
            anyhow::bail!("runtime manifest unreadable: {e}")
        }
    }
}

/// exe 旁部署目录（manifest 所在地）。`current_exe` 失败 = None（Absent 语义）。
fn exe_deploy_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|p| p.to_path_buf())
}

#[cfg(test)]
mod tests;
