//! J6 (devtool-upgrade 阶段 6)：图片超限自动降采样。
//!
//! 多模态管线的验真链对 >25MB 图片是硬拒绝（`image_path_detector::MAX_IMAGE_BYTES`）；
//! provider 另有分辨率上限（最长边 8000px 量级）。本模块给超限图片**一次降采样
//! 机会**：解码后按最长边阶梯（4096→2048→1024）缩放 + JPEG q85 重编码，产物
//! ≤8MB 才采用，落 uploads 暂存目录（内容寻址命名 `down_{sha8}.jpg`，同图复用；
//! 7 天 TTL 由 uploads 既有清扫统一覆盖——`sweep_uploads_older_than` 按目录
//! mtime 扫，不挑前缀）。**原图永不修改**（用户文件只读，产物另落 uploads）。
//!
//! 诚实边界（doc 钉死）：
//! - 输入维度安全阀 [`MAX_INPUT_DIMENSION`]（12000px）：解码内存 ≈ w·h·4，
//!   超阀诚实拒绝而非尝试（拒绝时保持原图直附语义失败前的原拒绝注记）。
//! - **GIF 不降采样**（重编码丢动画帧 = 静默语义变更），诚实拒绝。
//! - 解码失败（合法 magic 但坏 body）诚实拒绝。
//! - 开关 `agents.image_downscale`（默认**开**）由调用方 fresh-read 后以
//!   `uploads_dir: Option<&Path>` 传入——`None` = 开关关，调用方零行为变化。
//!
//! 缩放核为 Lanczos3；阶梯语义（4096→2048→1024 / JPEG q85 / 8MB 底线）为本侧 provider 适配。

use std::path::{Path, PathBuf};

use crate::image_path_detector;

/// 降采样触发阈值：文件字节数（与验真链同一 25MB 上限）。
pub(crate) const ENTRY_BYTES: u64 = image_path_detector::MAX_IMAGE_BYTES;

/// 降采样触发阈值：最长边像素（provider 分辨率上限量级）。
pub(crate) const MAX_DIMENSION: u32 = 8000;

/// 降采样阶梯（最长边 px）：逐级收紧，产物 ≤[`Limits::out_bytes`] 才采用。
const LADDER: [u32; 3] = [4096, 2048, 1024];

/// 输入维度安全阀：解码内存 ≈ w·h·4（12000²×4 ≈ 576MB 瞬时峰值）。
const MAX_INPUT_DIMENSION: u32 = 12000;

/// JPEG 重编码质量（对齐实施计划原文 q85）。
const JPEG_QUALITY: u8 = 85;

/// 降采样判定参数（生产默认 = [`DEFAULT_LIMITS`]；测试注入小阈值免合成
/// 25MB 实体文件）。字段 pub(crate)：仅 nemesis-agent 测试构造。
pub(crate) struct Limits {
    /// 触发降采样的文件字节下限。
    pub entry_bytes: u64,
    /// 触发降采样的最长边下限（px）。
    pub entry_dimension: u32,
    /// 产物字节上限（超过则尝试下一级阶梯）。
    pub out_bytes: usize,
}

pub(crate) const DEFAULT_LIMITS: Limits = Limits {
    entry_bytes: ENTRY_BYTES,
    entry_dimension: MAX_DIMENSION,
    out_bytes: 8 * 1024 * 1024,
};

/// 降采样闸判定结果。
#[derive(Debug)]
pub(crate) enum GateVerdict {
    /// 无需降采样（≤entry 阈值）——调用方按原图继续既有链。
    Keep,
    /// 已降采样——`path` 为 uploads 产物（内容寻址，同图复用），调用方以
    /// 产物替代原图走附加链。
    Replaced {
        path: PathBuf,
        from_bytes: u64,
        to_bytes: usize,
        longest_side: u32,
    },
}

/// 降采样闸（J6 唯一入口，`image_attach` 两个来源共用）。
///
/// `uploads_dir = None`（开关关）恒 [`GateVerdict::Keep`]——零行为变化。
/// 调用方对 `Err(reason)` 产出诚实拒绝注记（reason 已是人读文案）。
pub(crate) fn downscale_gate(
    src: &Path,
    uploads_dir: Option<&Path>,
) -> Result<GateVerdict, String> {
    match uploads_dir {
        None => Ok(GateVerdict::Keep),
        Some(dir) => downscale_gate_with(src, dir, &DEFAULT_LIMITS),
    }
}

pub(crate) fn downscale_gate_with(
    src: &Path,
    uploads_dir: &Path,
    limits: &Limits,
) -> Result<GateVerdict, String> {
    // 元数据：文件大小（不存在/不可读诚实 Err——调用方注记）。
    let size = std::fs::metadata(src)
        .map_err(|e| format!("无法读取图片元数据 ({}): {}", src.display(), e))?
        .len();

    // 头部读尺寸（不全解码）：ImageReader::open 是文件句柄读，无整包 slurp。
    let (w, h) = image::ImageReader::open(src)
        .map_err(|e| format!("无法打开图片 ({}): {}", src.display(), e))?
        .with_guessed_format()
        .map_err(|e| format!("图片格式嗅探失败 ({}): {}", src.display(), e))?
        .into_dimensions()
        .map_err(|e| format!("读取图片尺寸失败 ({}): {}", src.display(), e))?;
    let longest = w.max(h);

    // 未超任一阈值 → 原图直走（常规图代价 = 一次 header 读）。
    if size <= limits.entry_bytes && longest <= limits.entry_dimension {
        return Ok(GateVerdict::Keep);
    }
    // 维度安全阀：解码内存超限，不尝试（诚实拒绝）。
    if longest > MAX_INPUT_DIMENSION {
        return Err(format!(
            "图片尺寸 {}x{} 超过 {}px 安全上限，无法自动降采样",
            w, h, MAX_INPUT_DIMENSION
        ));
    }
    // GIF：重编码丢动画帧 = 静默语义变更，诚实拒绝。
    let head = std::fs::File::open(src)
        .and_then(|mut f| {
            use std::io::Read;
            let mut buf = [0u8; 12];
            f.read_exact(&mut buf)?;
            Ok(buf)
        })
        .map_err(|e| format!("无法读取图片文件头 ({}): {}", src.display(), e))?;
    if image_path_detector::ext_from_magic(&head) == Some("gif") {
        return Err("GIF 动图不支持自动降采样（会丢失动画帧）".to_string());
    }

    // 全量读入 + 解码（维度安全阀已过，内存有界）。
    let bytes =
        std::fs::read(src).map_err(|e| format!("无法读取图片 ({}): {}", src.display(), e))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| format!("图片解码失败 ({}): {}", src.display(), e))?;
    let flat = flatten_white(img);

    // 阶梯缩放 + JPEG q85：第一级可能不缩（longest ≤ 4096 但字节超限时
    // 纯重编码常已达标），后续级只缩不放。rung 与 entry_dimension 取 min：
    // 生产（entry=8000 > 1024）不 binding、行为不变；注入小阈值的测试态下
    // 兜底保证产物不再超入口维度阈值（触发原因必须被消除）。
    let mut sha8 = String::new();
    for &rung in &LADDER {
        let target = rung.min(limits.entry_dimension);
        let scaled: image::DynamicImage = if longest > target {
            flat.resize(target, target, image::imageops::FilterType::Lanczos3)
        } else {
            flat.clone()
        };
        let mut out = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
        scaled
            .write_with_encoder(encoder)
            .map_err(|e| format!("JPEG 重编码失败 ({}): {}", src.display(), e))?;
        if out.len() <= limits.out_bytes {
            if sha8.is_empty() {
                sha8 = content_sha8(&bytes);
            }
            // 内容寻址落盘：同图（同内容）复用既有产物（7 天 TTL 清扫覆盖）。
            let dest = uploads_dir.join(format!("down_{}.jpg", sha8));
            if !dest.exists() {
                std::fs::create_dir_all(uploads_dir)
                    .map_err(|e| format!("创建 uploads 目录失败: {}", e))?;
                std::fs::write(&dest, &out).map_err(|e| format!("写入降采样产物失败: {}", e))?;
            }
            return Ok(GateVerdict::Replaced {
                path: dest,
                from_bytes: size,
                to_bytes: out.len(),
                longest_side: scaled.width().max(scaled.height()),
            });
        }
    }
    Err(format!(
        "图片降采样至 {}px 后仍超过 {}MB 上限",
        LADDER[LADDER.len() - 1],
        limits.out_bytes / (1024 * 1024)
    ))
}

/// 透明像素铺白底（JPEG 无 alpha 通道，直接丢 alpha 会把透明区变黑/杂色）。
fn flatten_white(img: image::DynamicImage) -> image::DynamicImage {
    if img.color().has_alpha() {
        let (w, h) = (img.width(), img.height());
        let mut bg = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
        image::imageops::overlay(&mut bg, &img.to_rgba8(), 0, 0);
        image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(bg).to_rgb8())
    } else {
        image::DynamicImage::ImageRgb8(img.to_rgb8())
    }
}

/// 内容 sha256 前 8 字节 hex（命名寻址；sha2 已是 crate 依赖面）。
fn content_sha8(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest[..8].iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests;
