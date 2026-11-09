//! 打包引擎：发行目录 → tar → zstd → 追加骨架尾部 + manifest blob + NBSETUP1
//! 尾注（计划 §3/§7a ⑥）；另含外壳签名（⑦）与 keygen。
//!
//! 纪律：
//! - 文件收集**显式排序**（NTFS/ext4 目录序是平台实现细节——环境敏感清单）；
//! - 编译中间物（pdb/lib/exp/ilk/obj）一律排除，绝不进 payload；
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
        if p.is_dir() {
            walk(&p, out)?;
        } else {
            let ext = p
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if EXCLUDED_EXTS.contains(&ext.as_str()) {
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
            let kh = nemesis_verify::keygen::KeyHierarchy::load(
                keys.to_str()
                    .ok_or_else(|| anyhow!("keys 路径非 UTF-8: {}", keys.display()))?,
            )?;
            let signed = nemesis_verify::verify::sign_content_v4(
                &json,
                &kh.leaf_sk,
                crate::pkgfile::now_unix(),
                &kh.chain(),
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
    let kh = nemesis_verify::keygen::KeyHierarchy::load(
        keys.to_str()
            .ok_or_else(|| anyhow!("keys 路径非 UTF-8: {}", keys.display()))?,
    )?;
    let signed = nemesis_verify::verify::sign_content_v4(
        &content,
        &kh.leaf_sk,
        crate::pkgfile::now_unix(),
        &kh.chain(),
        Some("NemesisBot Setup"),
    )?;
    let tmp = file.with_extension("nbsetup-signing.tmp");
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        f.write_all(&signed)?;
        f.flush()?;
    }
    std::fs::rename(&tmp, file)?;
    Ok(())
}

#[cfg(test)]
mod tests;
