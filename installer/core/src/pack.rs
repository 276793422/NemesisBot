//! 打包引擎：发行目录 → tar → zstd → 追加骨架尾部 + manifest blob + NBSETUP1
//! 尾注（计划 §3/§7a ⑥）；另含外壳签名（⑦）与 keygen。
//!
//! 纪律：
//! - 文件收集**显式排序**（NTFS/ext4 目录序是平台实现细节——环境敏感清单）；
//! - 编译中间物（pdb/lib/exp/ilk/obj）、测试工具目录（tests/）、皮肤目录
//!   （skins/，支持在线下载，发行包不预置）与桌面/编辑器杂物（Thumbs.db
//!   等）一律排除，绝不进 payload；
//! - 必需文件缺席 **loud 拒绝打包**（绝不内嵌不完整载荷，计划 §7a ⑥）；
//! - v1 不保留 Windows 特殊文件属性（hidden/system 在发行载荷中缺席，
//!   README 诚实注记）；Unix 权限位经 manifest mode 保存。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use sha2::{Digest, Sha256};

use crate::format::Footer;
use crate::manifest::{Manifest, ManifestFileEntry};

/// 排除的编译中间物扩展名（小写匹配）。
const EXCLUDED_EXTS: [&str; 5] = ["pdb", "lib", "exp", "ilk", "obj"];

/// 排除的目录名（任何层级，小写匹配）。`tests/` 是构建脚本**故意** stage 进
/// `bin/bin_*/tests/` 的测试工具（ai-server/cluster-test/integration-test/
/// mcp-server，供本机开发流取用）——发行包边界在 pack 过滤（2026-11-10 用户
/// 验收实录：16 文件包混入 ~16MB 测试 exe；不改 staging，删了会断开发流）。
/// `skins/` 皮肤支持在线下载，发行包不预置（缺席 = 空表优雅降级，
/// `scan_skins` 对目录缺席返回空表；用户拍板 2026-11-10）。
const EXCLUDED_DIR_NAMES: [&str; 2] = ["tests", "skins"];

/// 排除的杂物文件名（资源管理器/系统/编辑器误入包的经典产物，小写匹配）。
const EXCLUDED_FILE_NAMES: [&str; 4] = ["thumbs.db", "desktop.ini", ".ds_store", ".gitignore"];

/// 各平台必需文件（缺席 = loud 拒绝打包）。
fn required_files(platform: &str) -> &'static [&'static str] {
    match platform {
        "windows" => &["nemesisbot.exe"],
        "linux" => &["nemesisbot"],
        _ => &[],
    }
}

/// 递归收集 payload 目录全量文件（绝对路径，排序稳定）。
pub fn collect_files(payload_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(payload_dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> =
        std::fs::read_dir(dir)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_lowercase();
        if p.is_dir() {
            if EXCLUDED_DIR_NAMES.contains(&name.as_str()) {
                continue;
            }
            walk(&p, out)?;
        } else {
            let ext = p
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if EXCLUDED_EXTS.contains(&ext.as_str()) || EXCLUDED_FILE_NAMES.contains(&name.as_str())
            {
                continue;
            }
            out.push(p);
        }
    }
    Ok(())
}

/// 相对路径归一（正斜杠；分隔符平台差异收口）。
pub fn rel_path(payload_dir: &Path, file: &Path) -> Result<String> {
    Ok(file
        .strip_prefix(payload_dir)
        .map_err(|_| anyhow!("文件不在载荷目录内: {}", file.display()))?
        .to_string_lossy()
        .replace('\\', "/"))
}

/// Unix 权限位：unix 下读真实 mode（保留执行位），非 unix 按扩展名推断。
pub fn file_mode(file: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(file) {
            return meta.permissions().mode() & 0o777;
        }
    }
    let exec_like = file
        .extension()
        .map(|x| {
            let x = x.to_string_lossy().to_lowercase();
            matches!(x.as_str(), "exe" | "dll" | "so" | "bat" | "sh" | "ps1")
        })
        .unwrap_or(false);
    if exec_like { 0o755 } else { 0o644 }
}

/// 打包参数。
pub struct PackOptions {
    /// 发行产物目录（bin/bin_windows 或 bin/bin_linux）。
    pub payload_dir: PathBuf,
    /// 骨架文件（未签名原始 exe）。
    pub skeleton: PathBuf,
    /// 输出安装包路径（父目录自动创建）。
    pub out: PathBuf,
    /// bot 版本（manifest.bot_version）。
    pub bot_version: String,
    /// windows | linux。
    pub platform: String,
    /// 签名密钥体系（keys.json，nemesis-verify 形态）；None = 不签。
    pub sign_keys: Option<PathBuf>,
    /// zstd 压缩级别（缺省 16）。
    pub zstd_level: i32,
}

/// 打包统计。
#[derive(Debug)]
pub struct PackStats {
    pub files: usize,
    pub payload_offset: u64,
    pub payload_len: u64,
    pub uncompressed_total: u64,
    pub out_len: u64,
    pub signed: bool,
}

/// 边写边哈希边计数的写入器（payload 区域 sha256 单遍完成）。
struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    count: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.count += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// 执行打包（§7a ⑥ 全流程）。
pub fn run(opts: &PackOptions) -> Result<PackStats> {
    if !crate::manifest::PLATFORMS.contains(&opts.platform.as_str()) {
        return Err(anyhow!(
            "--platform 值域非法: {}（windows|linux）",
            opts.platform
        ));
    }

    // ① 收集 + 必需文件闸（BTreeMap：按相对路径排序的确定性清单）。
    let files = collect_files(&opts.payload_dir)?;
    let mut rels = BTreeMap::new();
    for f in &files {
        rels.insert(rel_path(&opts.payload_dir, f)?, f.clone());
    }
    for req in required_files(&opts.platform) {
        if !rels.contains_key(*req) {
            return Err(anyhow!(
                "载荷缺必需文件「{req}」（拒绝打包不完整载荷）：{}",
                opts.payload_dir.display()
            ));
        }
    }
    // ② manifest 条目（全量读入算 sha256——发行载荷 ≤ 几百 MB，一次读可承受；
    // tar 写入阶段用流式 append_data 不二次驻留）。
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    for (rel, abs) in &rels {
        let bytes = std::fs::read(abs)?;
        total += bytes.len() as u64;
        entries.push(ManifestFileEntry {
            path: rel.clone(),
            sha256: hex::encode(Sha256::digest(&bytes)),
            size: bytes.len() as u64,
            mode: file_mode(abs),
        });
    }

    // ③ 写出：骨架 + zstd(tar 流)（边写边算区域 sha256/长度）。
    if let Some(parent) = opts.out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let skeleton = std::fs::read(&opts.skeleton)?;
    let payload_offset = skeleton.len() as u64;
    let mut out_file = std::io::BufWriter::new(std::fs::File::create(&opts.out)?);
    // 骨架先行落盘（在 HashingWriter 之前——payload sha/len 只覆盖压缩区）。
    out_file.write_all(&skeleton)?;
    let mut hw = HashingWriter {
        inner: out_file,
        hasher: Sha256::new(),
        count: 0,
    };
    {
        let mut zenc = zstd::stream::Encoder::new(&mut hw, opts.zstd_level)?;
        {
            let mut tarb = tar::Builder::new(&mut zenc);
            for (rel, abs) in &rels {
                let meta = std::fs::metadata(abs)?;
                let mut header = tar::Header::new_gnu();
                header.set_size(meta.len());
                header.set_mode(file_mode(abs));
                header.set_cksum();
                let mut f = std::fs::File::open(abs)?;
                tarb.append_data(&mut header, rel, &mut f)?;
            }
            tarb.finish()?;
        }
        zenc.finish()?;
    }
    let payload_len = hw.count;
    let payload_sha = hex::encode(hw.hasher.finalize());
    let mut out_file = hw.inner;
    out_file.flush()?;

    // ④ manifest → blob（签名形态 = JSON + NMBSIG v4 footer，与发行包同链）。
    let manifest = Manifest {
        format_version: crate::FORMAT_VERSION,
        bot_version: opts.bot_version.clone(),
        platform: opts.platform.clone(),
        created_at: crate::pkgfile::now_unix(),
        payload_offset,
        payload_len,
        payload_sha256: payload_sha,
        total_size: total,
        files: entries,
    };
    let json = manifest.to_json_vec()?;
    let (blob, signed) = match &opts.sign_keys {
        Some(keys) => {
            // SigningMaterial（非 KeyHierarchy）：sign 只需要叶私钥 + 全链三证书。
            // CI mint-leaf 产出是 SignOnly 形态（root_sk 恒空——根私钥离线是设计
            // 前提），KeyHierarchy::load 全量装载必拒；此 loader 与 exe-sign-tool
            // sign 同源，本地全量 keys.json（Full 形态）同样容忍。
            let mat = nemesis_verify::bundle::SigningMaterial::load(
                keys.to_str()
                    .ok_or_else(|| anyhow!("keys 路径非 UTF-8: {}", keys.display()))?,
            )?;
            let signed = nemesis_verify::verify::sign_content_v4(
                &json,
                &mat.leaf_sk,
                crate::pkgfile::now_unix(),
                &mat.chain(),
                Some("NemesisBot Setup"),
            )?;
            (signed, true)
        }
        None => (json, false),
    };

    // ⑤ manifest blob + NBSETUP1 尾注。
    let footer = Footer {
        manifest_offset: payload_offset + payload_len,
        manifest_len: blob.len() as u64,
        manifest_sha256: Sha256::digest(&blob).into(),
    };
    out_file.write_all(&blob)?;
    out_file.write_all(&footer.encode())?;
    out_file.flush()?;
    // Unix 执行位：包本体是「直接运行即安装器」的自执行形态（.run 惯例），
    // File::create 按 umask 落 644 → 运行 rc 126（2026-11-10 Linux E2E 实录，
    // 与卸载副本丢执行位同族）。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&opts.out, std::fs::Permissions::from_mode(0o755))?;
    }
    let out_len =
        payload_offset + payload_len + blob.len() as u64 + crate::format::FOOTER_SIZE as u64;

    Ok(PackStats {
        files: manifest.files.len(),
        payload_offset,
        payload_len,
        uncompressed_total: total,
        out_len,
        signed,
    })
}

/// 生成密钥体系（keys.json），返回根锚 hex（编译期注入/运行时 env 用）。
pub fn keygen(out: &Path) -> Result<String> {
    let kh = nemesis_verify::keygen::generate()?;
    let anchor = kh.root_anchor_fingerprint();
    kh.save(
        out.to_str()
            .ok_or_else(|| anyhow!("keys 路径非 UTF-8: {}", out.display()))?,
    )?;
    Ok(hex::encode(anchor))
}

/// 外壳签名（§7a ⑦）：对成品安装包文件 v4 签名（PE→证书表 / ELF/raw→v4
/// footer 自动分派；已签名 PE 二次追表诚实拒绝——nemesis-verify 同款语义）。
/// 原子写回（tmp + rename）。
pub fn sign_file(file: &Path, keys: &Path) -> Result<()> {
    let content = std::fs::read(file)?;
    // SigningMaterial（与 pack --sign 同 loader）：容忍 CI mint-leaf 的 SignOnly
    // 分包形态与本地全量 keys.json（Full），要求只有叶私钥 + 全链三证书。
    let mat = nemesis_verify::bundle::SigningMaterial::load(
        keys.to_str()
            .ok_or_else(|| anyhow!("keys 路径非 UTF-8: {}", keys.display()))?,
    )?;
    let signed = nemesis_verify::verify::sign_content_v4(
        &content,
        &mat.leaf_sk,
        crate::pkgfile::now_unix(),
        &mat.chain(),
        Some("NemesisBot Setup"),
    )?;
    let tmp = file.with_extension("nbsetup-signing.tmp");
    // 原子写回换 inode——unix 上保留原文件模式（包本体是可执行 .run；
    // build --sign 链先 pack 后 pack sign，不保留则 ⑤ 的 755 被 ⑤b 打回 644）。
    #[cfg(unix)]
    let saved_mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(file)?.permissions().mode()
    };
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        f.write_all(&signed)?;
        f.flush()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(saved_mode))?;
    }
    std::fs::rename(&tmp, file)?;
    Ok(())
}

#[cfg(test)]
mod tests;
