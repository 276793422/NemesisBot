//! 安装引擎：版本裁决（Fresh/Repair/Upgrade/Downgrade）→ staging 释放 →
//! 原子提交 → 卸载（计划 §5/§6/§7）。
//!
//! 纪律：
//! - **staging 先行**：全部释放进 `<target>/.nbsetup-stage-*`，校验全过后
//!   逐文件 rename 提交——中途失败目标目录零污染（「零残留」承诺）；
//! - **install.json 最后落盘**：提交成功的哨兵——中断后无标记 = 下次
//!   按 Fresh 覆盖装（诚实恢复路径）；
//! - **升级模式跳过 `workspace/` 前缀**（v1 载荷本就不含 workspace——
//!   数据住产品默认 home `~/.nemesisbot`，在安装目录之外——此规则是
//!   载荷未来带回 workspace 模板时的前向保险）；
//! - 释放路径全部过 [`crate::pkgfile::sanitize_entry_path`]（拒绝对路径/
//!   `..`/盘符）；tar 条目不在 manifest = 硬失败（清单权威）。

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fsutil;
use crate::manifest::Manifest;
use crate::pkgfile::{self, Progress};

/// 安装标记文件（`<target>/install.json`）：版本探测与卸载清单真相源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRecord {
    pub format_version: u32,
    pub bot_version: String,
    /// 安装时刻 Unix 秒。
    pub install_time: u64,
    /// 已提交文件（相对安装根）。
    pub files: Vec<String>,
    /// 是否保留了骨架卸载副本（uninstall.exe）。
    pub uninstall_copy: bool,
}

const RECORD_FILE: &str = "install.json";

/// 安装类型裁决。
#[derive(Debug, PartialEq)]
pub enum InstallPlan {
    /// 无既有安装。
    Fresh,
    /// 同版本重装（修复）。
    Repair(String),
    /// 覆盖升级。
    Upgrade(String),
    /// 降级（默认拒绝，`--force` 豁免）。
    Downgrade { current: String, incoming: String },
}

/// 读取既有安装并裁决类型。`incoming` 为空 = 无包信息（仅查现状）。
pub fn plan(target_dir: &Path, incoming: Option<&str>) -> Result<InstallPlan> {
    let record_path = target_dir.join(RECORD_FILE);
    if !record_path.exists() {
        return Ok(InstallPlan::Fresh);
    }
    let current = read_record(&record_path)?
        .map(|r| r.bot_version)
        .ok_or_else(|| anyhow!("install.json 损坏且不可解析: {}", record_path.display()))?;
    let Some(incoming) = incoming else {
        return Ok(InstallPlan::Repair(current));
    };
    match crate::version::compare(&current, incoming) {
        Some(std::cmp::Ordering::Equal) => Ok(InstallPlan::Repair(current)),
        Some(std::cmp::Ordering::Less) => Ok(InstallPlan::Upgrade(current)),
        Some(std::cmp::Ordering::Greater) => Ok(InstallPlan::Downgrade {
            current,
            incoming: incoming.to_string(),
        }),
        None => Err(anyhow!("版本号不可比较：已有 {current} vs 待装 {incoming}")),
    }
}

fn read_record(path: &Path) -> Result<Option<InstallRecord>> {
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| anyhow!("install.json 损坏不可解析（{}）: {e}", path.display()))
}

/// 信任分级执行策略（计划 §4a 的静默/GUI 两形态接线点）。
#[derive(Debug, Clone)]
pub enum SigPolicy {
    /// 静默形态：红级需 `accept_risk=true`，缺 = [`TypedFail::SigRisk`]；
    /// 黄级 WARN 后继续；绿级静默。
    Enforce { accept_risk: bool },
    /// GUI 形态：知情确认已在向导层收集，直接放行（分级横幅已展示）。
    PreApproved,
}

/// 类型化失败（bin 侧退出码映射：Integrity→2 / SigRisk→5 / Downgrade→1）。
#[derive(Debug)]
pub enum TypedFail {
    Integrity(String),
    SigRisk(String),
    DowngradeRejected { current: String, incoming: String },
}

impl std::fmt::Display for TypedFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypedFail::Integrity(e) => write!(f, "完整性校验失败: {e}"),
            TypedFail::SigRisk(e) => write!(
                f,
                "签名异常且未经知情确认: {e}（静默模式加 --accept-signature-risk 继续）"
            ),
            TypedFail::DowngradeRejected { current, incoming } => {
                write!(
                    f,
                    "版本降级被拒：已有 {current} > 待装 {incoming}（--force 豁免）"
                )
            }
        }
    }
}
impl std::error::Error for TypedFail {}

/// 安装参数。
pub struct InstallOptions {
    pub target_dir: PathBuf,
    pub force_downgrade: bool,
    pub sig_policy: SigPolicy,
    /// 保留骨架卸载副本（Windows 形态：`uninstall.exe` = 骨架字节段副本）。
    pub keep_uninstall_copy: bool,
}

/// 安装结果。
#[derive(Debug)]
pub struct InstallResult {
    pub install_dir: PathBuf,
    pub bot_version: String,
    pub files_installed: usize,
    /// 升级模式跳过的 workspace 文件数（前向保险规则的可见性）。
    pub workspace_skipped: usize,
}

/// 执行安装（全流程：验签分级 → 版本裁决 → staging 释放 → 提交）。
pub fn perform(
    package_path: &Path,
    opts: &InstallOptions,
    progress: Option<&dyn Fn(Progress)>,
) -> Result<InstallResult> {
    // ① 读包（结构/完整性硬闸在 inspect 内）。
    let info = pkgfile::inspect(package_path)?;

    // ② 信任分级（计划 §4a）。
    match &opts.sig_policy {
        SigPolicy::PreApproved => {}
        SigPolicy::Enforce { accept_risk } => match info.sig.grade {
            crate::sig::SigGrade::Green => {}
            crate::sig::SigGrade::Yellow => {
                eprintln!("[WARN] {}", info.sig.text);
            }
            crate::sig::SigGrade::Red if !*accept_risk => {
                return Err(anyhow::Error::new(TypedFail::SigRisk(
                    info.sig.text.clone(),
                )));
            }
            crate::sig::SigGrade::Red => {
                eprintln!("[WARN] 已按 --accept-signature-risk 继续（红色级知情确认）");
            }
        },
    }

    // ③ 版本裁决。
    match plan(&opts.target_dir, Some(&info.manifest.bot_version))? {
        InstallPlan::Downgrade { current, incoming } if !opts.force_downgrade => {
            return Err(anyhow::Error::new(TypedFail::DowngradeRejected {
                current,
                incoming,
            }));
        }
        _ => {}
    }

    let report = |p: &Progress| {
        if let Some(f) = progress {
            f(p.clone());
        }
    };

    // ④ staging 目录（target 内保证同卷 rename 原子性）+ 陈尸清理。
    let target_existed = opts.target_dir.exists();
    let staging = staging_dir(&opts.target_dir)?;
    fsutil::remove_dir_all(&staging)?;
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("建 staging 目录失败: {}", staging.display()))?;
    let result = perform_inner(package_path, &info.manifest, opts, &staging, &report);
    if result.is_err() {
        // 失败零残留：staging 整体删除（best-effort，失败不掩盖主错误）；
        // 目标目录是本次新建的则一并摘除（空目录——非空说明有既有安装，
        // 不动）。
        let _ = fsutil::remove_dir_all(&staging);
        if !target_existed {
            let _ = std::fs::remove_dir(&opts.target_dir);
        }
    }
    let (installed, skipped) = result?;

    Ok(InstallResult {
        install_dir: opts.target_dir.clone(),
        bot_version: info.manifest.bot_version.clone(),
        files_installed: installed,
        workspace_skipped: skipped,
    })
}

fn staging_dir(target: &Path) -> Result<PathBuf> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(target.join(format!(".nbsetup-stage-{}-{millis}", std::process::id())))
}

/// 卸载副本文件名（按包平台：windows=uninstall.exe / linux=uninstall）。
pub fn uninstall_copy_name(platform: &str) -> &'static str {
    if platform == "linux" {
        "uninstall"
    } else {
        "uninstall.exe"
    }
}

fn perform_inner(
    package_path: &Path,
    manifest: &Manifest,
    opts: &InstallOptions,
    staging: &Path,
    report: &dyn Fn(&Progress),
) -> Result<(usize, usize)> {
    // ⑤ payload 区域哈希（释放前硬闸）。
    {
        let mut file = std::fs::File::open(package_path)?;
        report(&Progress::Hashing(0.0));
        let total = manifest.payload_len;
        let mut reader = pkgfile::take_region(&mut file, manifest.payload_offset, total)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 256 * 1024];
        let mut done: u64 = 0;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            done += n as u64;
            report(&Progress::Hashing(done as f32 / total as f32));
        }
        if done != total {
            return Err(anyhow::Error::new(TypedFail::Integrity(format!(
                "payload 区域长度 {done} ≠ manifest 记载 {total}"
            ))));
        }
        let got = hex::encode(hasher.finalize());
        if got != manifest.payload_sha256 {
            return Err(anyhow::Error::new(TypedFail::Integrity(format!(
                "payload sha256 不符：manifest={} 实际={got}",
                manifest.payload_sha256
            ))));
        }
    }

    // ⑥ 流式解压：zstd 帧 → tar 条目 → staging（逐文件 sha256 校验）。
    let mut file = std::fs::File::open(package_path)?;
    let region = pkgfile::take_region(&mut file, manifest.payload_offset, manifest.payload_len)?;
    let decoder = ruzstd::StreamingDecoder::new(region)
        .map_err(|e| anyhow::Error::new(TypedFail::Integrity(format!("zstd 流解码失败: {e}"))))?;
    let mut archive = tar::Archive::new(decoder);

    let index = manifest.entry_map();
    let total_files = manifest.files.len();
    let mut seen: HashSet<String> = HashSet::new();
    let mut skipped_workspace: HashSet<String> = HashSet::new();
    let is_upgrade = plan(&opts.target_dir, None)?.meta_version().is_some();

    for entry in archive
        .entries()
        .map_err(|e| anyhow::Error::new(TypedFail::Integrity(format!("tar 流读取中断: {e}"))))?
    {
        let mut entry = entry.map_err(|e| {
            anyhow::Error::new(TypedFail::Integrity(format!("tar 条目读取中断: {e}")))
        })?;
        let raw = entry.path()?.to_path_buf();
        let rel = pkgfile::sanitize_entry_path(&raw)?;
        let expect = index.get(rel.as_str()).ok_or_else(|| {
            anyhow::Error::new(TypedFail::Integrity(format!(
                "tar 条目「{rel}」不在 manifest 清单内（清单权威）"
            )))
        })?;
        if seen.contains(&rel) {
            return Err(anyhow::Error::new(TypedFail::Integrity(format!(
                "tar 重复条目: {rel}"
            ))));
        }
        // 升级模式跳过 workspace 前缀（前向保险；v1 载荷不含）。跳过 = 不
        // 释放也不提交——seen 照记（清单完整性对账），committed 集合不含。
        if is_upgrade && rel.starts_with("workspace/") {
            skipped_workspace.insert(rel.clone());
            seen.insert(rel);
            continue;
        }
        let dest = staging.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("建释放目录失败: {}", parent.display()))?;
        }
        let mut hasher = Sha256::new();
        let mut out = std::fs::File::create(&dest)
            .with_context(|| format!("释放文件失败: {}", dest.display()))?;
        let mut buf = [0u8; 128 * 1024];
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
        }
        out.flush()?;
        drop(out);
        let got = hex::encode(hasher.finalize());
        if got != expect.sha256 {
            return Err(anyhow::Error::new(TypedFail::Integrity(format!(
                "文件 sha256 不符: {rel}（expect {} got {got}）",
                expect.sha256
            ))));
        }
        set_mode(&dest, expect.mode);
        seen.insert(rel);
        report(&Progress::Extracting(seen.len(), total_files));
    }

    // 清单完整性：manifest 有而 tar 缺 = 包损坏。
    let missing: Vec<&str> = index
        .keys()
        .filter(|p| !seen.contains(**p))
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(anyhow::Error::new(TypedFail::Integrity(format!(
            "manifest 条目在 tar 流中缺席（{} 项）: {}",
            missing.len(),
            missing
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ))));
    }

    // ⑦ 安装记录写入 staging（哨兵最后提交）。files = 实际提交集（不含
    // 升级跳过项；uninstall 副本若保留也入册）。
    let mut committed: Vec<String> = seen.difference(&skipped_workspace).cloned().collect();
    committed.sort();
    let record = InstallRecord {
        format_version: crate::FORMAT_VERSION,
        bot_version: manifest.bot_version.clone(),
        install_time: pkgfile::now_unix(),
        files: committed.clone(),
        uninstall_copy: opts.keep_uninstall_copy,
    };
    if opts.keep_uninstall_copy {
        // 卸载副本 = 骨架字节段（[0, payload_offset)）。打包后签名的外壳
        // 证书表/v4 footer 在 EOF 区——副本是未签名形态（启动走黄色提示，
        // 诚实边界见 README）。副本名按平台：windows=uninstall.exe，
        // linux=uninstall（平台可执行惯例）。
        let copy_name = uninstall_copy_name(&manifest.platform);
        let mut pkg = std::fs::File::open(package_path)?;
        let mut skeleton = vec![0u8; manifest.payload_offset as usize];
        pkg.read_exact(&mut skeleton)?;
        std::fs::write(staging.join(copy_name), &skeleton)?;
    }
    std::fs::write(
        staging.join(RECORD_FILE),
        serde_json::to_vec_pretty(&record)?,
    )
    .with_context(|| format!("写安装记录失败: {}", staging.join(RECORD_FILE).display()))?;

    // ⑧ 原子提交：staging → target 逐文件 rename（同卷），install.json 最后。
    std::fs::create_dir_all(&opts.target_dir)
        .with_context(|| format!("建安装目录失败: {}", opts.target_dir.display()))?;
    let mut committed_count = 0usize;
    if opts.keep_uninstall_copy {
        let copy_name = uninstall_copy_name(&manifest.platform);
        std::fs::rename(staging.join(copy_name), opts.target_dir.join(copy_name))
            .with_context(|| format!("提交卸载副本失败 → {}", opts.target_dir.display()))?;
        committed_count += 1;
    }
    for rel in &committed {
        let src = staging.join(rel);
        let dst = opts.target_dir.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("建提交目录失败: {}", parent.display()))?;
        }
        std::fs::rename(&src, &dst)
            .with_context(|| format!("提交失败: {} → {}", src.display(), dst.display()))?;
        committed_count += 1;
    }
    std::fs::rename(staging.join(RECORD_FILE), opts.target_dir.join(RECORD_FILE)).with_context(
        || {
            format!(
                "提交安装记录失败 → {}",
                opts.target_dir.join(RECORD_FILE).display()
            )
        },
    )?;
    committed_count += 1;
    fsutil::remove_dir_all(staging)?;

    Ok((committed_count, skipped_workspace.len()))
}

/// Unix 权限位落盘（Windows 无权限位语义，跳过）。
fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if mode != 0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

/// 卸载：按 install.json 清单逐文件韧性删除 + 剪空目录 + 摘标记。
///
/// 返回 `(成功删除数, 失败清单)`——失败常见形态：卸载器自身正被运行
/// （Windows 进程镜像锁定），bin 侧对自身路径做延迟自删接手。
pub fn uninstall(install_dir: &Path) -> Result<(usize, Vec<(String, String)>)> {
    let record_path = install_dir.join(RECORD_FILE);
    let record = read_record(&record_path)?.ok_or_else(|| {
        anyhow!(
            "install.json 缺失或为空，无法卸载: {}",
            record_path.display()
        )
    })?;
    let mut deleted = 0usize;
    let mut failures = Vec::new();
    for rel in &record.files {
        let path = install_dir.join(rel);
        match fsutil::delete_file(&path) {
            Ok(()) => deleted += 1,
            Err(e) => failures.push((rel.clone(), e.to_string())),
        }
    }
    // 剪空目录（从最深文件目录向上）。
    let mut dirs: Vec<PathBuf> = record
        .files
        .iter()
        .filter_map(|rel| install_dir.join(rel).parent().map(Path::to_path_buf))
        .collect();
    dirs.sort();
    dirs.dedup();
    for dir in dirs.iter().rev() {
        fsutil::prune_empty_dirs(dir, install_dir);
    }
    fsutil::delete_file(&record_path)?;
    // 目录空了就摘掉（非空 = 卸载器自身或未知文件在场，诚实保留）。
    let empty = std::fs::read_dir(install_dir).is_ok_and(|mut it| it.next().is_none());
    if empty {
        let _ = std::fs::remove_dir(install_dir);
    }
    Ok((deleted, failures))
}

impl InstallPlan {
    /// 已有安装版本（Fresh = None）。
    pub fn meta_version(&self) -> Option<&str> {
        match self {
            InstallPlan::Fresh => None,
            InstallPlan::Repair(v) | InstallPlan::Upgrade(v) => Some(v),
            InstallPlan::Downgrade { current, .. } => Some(current),
        }
    }
}

#[cfg(test)]
mod tests;
