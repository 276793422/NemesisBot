//! VM QEMU 平台安装矩阵（配置化单一真相源）。
//!
//! 用户拍板（2026-10-27）：「下载地址不能是写死的啊，必须是动态可配置啊，配置
//! 文件程序内嵌，然后本地检测，没有则释放，有则直接用。配置文件里面至少也要把
//! 各平台不同的版本都列进去，然后不同平台下载不同过的包」；二轮拍板：「矩阵里
//! 面没有项目，这并不影响你把配置补全，为什么你一定要盯着必须有项目呢。我们的
//! arm64 环境下的 安卓 系统包，不就是 arm64 的linux 基座么，你先把东西补全」
//! ——矩阵条目全格填真值，`produced` 判据 = **安装面是否成立**（有真实可用的
//! 源），与 guest 系统面进度（镜像/machine_spec/GuestArch 点亮）解耦；
//! `android-aarch64` 是刚需条目（`std::env::consts::OS` 在 Android target 返回
//! `"android"` 而非 `"linux"`，故独立成键）。
//!
//! 解析链四层（全部诚实）：
//! 1. `<workspace>/config/config.vm_qemu.json`（用户实时编辑面——最高优先）；
//! 2. 内嵌默认模板（[`EMBEDDED_TEMPLATE`] include_str! 一次解析进
//!    `OnceLock`）——1/2 结构同源，代码零硬编码 URL；
//! 3. archive 的 sha512 置空时回落 URL 同目录 `SHA512SUMS` sidecar 查表
//!    （[`super::install`] 既有 `parse_sha512_sidecar` 保留）；
//! 4. 都不命中 → [`unavailable_error`] 自服务报错（已知键 + 文件路径 + note）。
//!
//! 模板既是内嵌默认也是释放件（[`ensure_released`] 首次安装时写盘，已存在
//! 不覆盖——「没有则释放，有则直接用」）。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;

/// 内嵌默认模板（与解析器同 crate 同目录，`include_str!` 零跨目录脆弱）。
/// 既是内嵌默认也是首装释放件（[`ensure_released`]）。
pub const EMBEDDED_TEMPLATE: &str = include_str!("config.vm_qemu.default.json");

/// 模板释放后的 workspace 文件名（对齐 `config.<子系统>.json` 命名惯例）。
pub const CONFIG_FILE_NAME: &str = "config.vm_qemu.json";

static EMBEDDED: OnceLock<QemuPlatformMatrix> = OnceLock::new();

// ---------------------------------------------------------------------------
// 配置模型
// ---------------------------------------------------------------------------

/// 安装形态（未知值 serde loud 拒绝——配置面不做静默纠偏）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QemuInstallKind {
    /// 下载安装包 + 免安装器直解（windows-x86_64 weilnetz 形态）。
    Archive,
    /// 包管理器安装（apt/dnf/pacman 字段按宿主探测消费）。
    Packages,
    /// 登记在案、诚实不支持（当前五格无此形态，保留给未来真实无源平台）。
    None,
}

impl QemuInstallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Packages => "packages",
            Self::None => "none",
        }
    }
}

fn default_true() -> bool {
    true
}

/// 单平台安装条目。除 `kind` 外全部可缺省（按 kind 校验必填集）；
/// 未知字段 loud 拒绝（配置面 typo 早炸，不静默忽略埋雷）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QemuPlatformEntry {
    pub kind: QemuInstallKind,
    /// 安装面是否成立（判据=有真实可用源；false = 登记在案不参与执行，
    /// 只进报错文案——与 guest 系统面进度解耦，用户拍板）。
    #[serde(default = "default_true")]
    pub produced: bool,
    /// archive：安装包 URL。
    #[serde(default)]
    pub url: String,
    /// archive：SHA-512 钉值 hex128（显式信任锚）；空 = 回落 URL 同目录
    /// SHA512SUMS sidecar 查表。
    #[serde(default)]
    pub sha512: String,
    /// archive：布局标记（当前 `nsis-7z`，留扩展位）。
    #[serde(default)]
    pub layout: String,
    /// packages：Debian/Ubuntu（Termux 同字段）包清单（空格分隔）。
    #[serde(default)]
    pub apt: String,
    /// packages：Fedora/RHEL 包清单。
    #[serde(default)]
    pub dnf: String,
    /// packages：pacman/MSYS2 包名（单个或空格分隔）。
    #[serde(default)]
    pub pacman: String,
    /// 安装后应存在的二进制文件名（`[0]` = system 模拟器、`[1]` = img 工具；
    /// 探针与安装同源消费——[`super::probe`] 与 [`super::install`]）。
    #[serde(default)]
    pub binaries: Vec<String>,
    /// 人读注记（自服务报错文案透出：源形态、依赖原因、环境前置）。
    #[serde(default)]
    pub note: String,
}

/// 平台安装矩阵（顶层）。`version` 只认 1——更高版本 = 未知格式，诚实拒绝
/// （解析器可能误解新字段，假装读懂是静默错配）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QemuPlatformMatrix {
    pub version: u32,
    /// 键 = `{os}-{arch}`（`std::env::consts::OS`/`ARCH` 运行时值）。
    pub platforms: BTreeMap<String, QemuPlatformEntry>,
}

// ---------------------------------------------------------------------------
// 解析 + 校验
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum QemuConfigError {
    #[error("结构非法: {detail}")]
    Malformed { detail: String },
    #[error("条目 {key} 校验失败: {reason}")]
    InvalidEntry { key: String, reason: String },
    #[error("version={version} 不受支持（当前解析器只认 version=1）")]
    UnknownVersion { version: u32 },
    #[error("platforms 为空（至少需要一个平台条目）")]
    EmptyPlatforms,
    #[error("读取失败: {detail}")]
    Io { detail: String },
}

/// 解析 + 校验（文本 → 矩阵；不合法 loud 拒绝，不做静默补全）。
pub fn parse_matrix(json: &str) -> Result<QemuPlatformMatrix, QemuConfigError> {
    let matrix: QemuPlatformMatrix =
        serde_json::from_str(json).map_err(|e| QemuConfigError::Malformed {
            detail: e.to_string(),
        })?;
    validate_matrix(&matrix)?;
    Ok(matrix)
}

fn validate_matrix(m: &QemuPlatformMatrix) -> Result<(), QemuConfigError> {
    if m.version != 1 {
        return Err(QemuConfigError::UnknownVersion { version: m.version });
    }
    if m.platforms.is_empty() {
        return Err(QemuConfigError::EmptyPlatforms);
    }
    for (key, entry) in &m.platforms {
        validate_entry(key, entry)?;
    }
    Ok(())
}

fn validate_entry(key: &str, e: &QemuPlatformEntry) -> Result<(), QemuConfigError> {
    match e.kind {
        QemuInstallKind::Archive => {
            if e.url.trim().is_empty() {
                return Err(QemuConfigError::InvalidEntry {
                    key: key.to_string(),
                    reason: "kind=archive 但 url 为空".to_string(),
                });
            }
        }
        QemuInstallKind::Packages => {
            if e.apt.trim().is_empty() && e.dnf.trim().is_empty() && e.pacman.trim().is_empty() {
                return Err(QemuConfigError::InvalidEntry {
                    key: key.to_string(),
                    reason: "kind=packages 但 apt/dnf/pacman 全空".to_string(),
                });
            }
        }
        // none = 登记在案形态，无必填集。
        QemuInstallKind::None => {}
    }
    // archive/packages 都要求 binaries 对齐安装探针的最小集（system + img）；
    // none 无产物可断言，豁免。
    if !matches!(e.kind, QemuInstallKind::None) && e.binaries.len() < 2 {
        return Err(QemuConfigError::InvalidEntry {
            key: key.to_string(),
            reason: format!(
                "binaries 至少 2 项（system 模拟器 + img 工具），当前 {} 项",
                e.binaries.len()
            ),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 宿主键 / 内嵌矩阵 / 查询
// ---------------------------------------------------------------------------

/// 当前宿主的矩阵键：`{os}-{arch}`（运行时取 `std::env::consts`——Android
/// target 返回 `"android"`，独立条目的依据）。
pub fn host_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// 内嵌默认矩阵（进程内一次解析；模板与代码同编译期产物，坏 = 发布事故，
/// expect 是正确的失败方式）。
pub fn embedded_matrix() -> &'static QemuPlatformMatrix {
    EMBEDDED.get_or_init(|| {
        parse_matrix(EMBEDDED_TEMPLATE).expect(
            "内嵌 config.vm_qemu.default.json 必须合法（与解析器同 crate 编译，测试闸钉死）",
        )
    })
}

/// 按键查条目。
pub fn entry_for<'a>(m: &'a QemuPlatformMatrix, key: &str) -> Option<&'a QemuPlatformEntry> {
    m.platforms.get(key)
}

/// 宿主条目。
pub fn host_entry(m: &QemuPlatformMatrix) -> Option<&QemuPlatformEntry> {
    entry_for(m, &host_key())
}

/// 自服务报错：已知平台键清单 + 配置文件路径 + 该条目 note（命中时）。
pub fn unavailable_error(
    m: &QemuPlatformMatrix,
    key: &str,
    workspace_cfg: Option<&Path>,
) -> anyhow::Error {
    let known = m.platforms.keys().cloned().collect::<Vec<_>>().join(", ");
    let cfg_hint = match workspace_cfg {
        Some(p) => format!(
            "；可在 {} 为该平台补全条目（改完下次安装即生效）",
            p.display()
        ),
        None => format!(
            "；可在 <workspace>/config/{} 为该平台补全条目",
            CONFIG_FILE_NAME
        ),
    };
    let note = match m.platforms.get(key) {
        Some(e) if !e.note.is_empty() => format!("；注记：{}", e.note),
        _ => String::new(),
    };
    anyhow::anyhow!("平台 {key} 不在 VM QEMU 安装矩阵中（已知平台：{known}）{cfg_hint}{note}")
}

// ---------------------------------------------------------------------------
// 解析链第一/二层 + 释放
// ---------------------------------------------------------------------------

/// 矩阵实际消费来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixSource {
    /// workspace 配置文件解析成功（用户实时编辑面）。
    WorkspaceFile,
    /// 内嵌默认模板（文件不存在回落）。
    Embedded,
}

/// 解析链：workspace 文件优先；NotFound 回内嵌；文件存在但损坏 loud 报错
/// （返回值保留来源标记，caller 可将错误与来源一起透出）。
pub fn resolve_matrix(
    workspace_cfg: Option<&Path>,
) -> (Result<QemuPlatformMatrix, QemuConfigError>, MatrixSource) {
    let Some(path) = workspace_cfg else {
        return (Ok(embedded_matrix().clone()), MatrixSource::Embedded);
    };
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let r = parse_matrix(&text);
            (r, MatrixSource::WorkspaceFile)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (Ok(embedded_matrix().clone()), MatrixSource::Embedded)
        }
        Err(e) => (
            Err(QemuConfigError::Io {
                detail: format!("{}: {e}", path.display()),
            }),
            MatrixSource::WorkspaceFile,
        ),
    }
}

/// 首次安装时把内嵌模板释放到 workspace config 目录（不存在则写，已存在
/// 不覆盖——「没有则释放，有则直接用」的释放臂）。返回是否发生了释放。
pub fn ensure_released(config_dir: &Path) -> std::io::Result<bool> {
    let target = config_dir.join(CONFIG_FILE_NAME);
    if target.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(config_dir)?;
    std::fs::write(&target, EMBEDDED_TEMPLATE)?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// 安装消费辅助
// ---------------------------------------------------------------------------

/// 安装幂等检查用的二进制名对：`(system 模拟器, img 工具)`（binaries 前两
/// 项；校验闸保证 archive/packages 条目至少 2 项）。
pub fn required_binaries_for(e: &QemuPlatformEntry) -> anyhow::Result<(&str, &str)> {
    let system = e
        .binaries
        .first()
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("矩阵条目 binaries 缺 system 模拟器名"))?;
    let img = e
        .binaries
        .get(1)
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("矩阵条目 binaries 缺 img 工具名"))?;
    Ok((system, img))
}

#[cfg(test)]
mod tests;
