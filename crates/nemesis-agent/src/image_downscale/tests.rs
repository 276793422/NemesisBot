//! J6 单测：降采样闸决策矩阵（Keep/Replaced/诚实 Err 三态 + 复用 + alpha 铺白）。
//! 阈值经 `Limits` 注入小数值，免合成 25MB 实体文件（attach 层集成不做——
//! 接线 None 分支由既有 attach 测试钉住）。

use std::io::Cursor;
use std::path::PathBuf;

use super::{GateVerdict, Limits, downscale_gate_with};

fn tiny_limits() -> Limits {
    Limits {
        // 2048：JPEG 最小编码开销 ~600B 量级（纯色 32x24 q85 实测 644B），
        // 500 会误触发 keep 用例。
        entry_bytes: 2048,
        entry_dimension: 100,
        out_bytes: 200 * 1024,
    }
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "nemesis_test_downscale_{}_{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 确定性伪随机噪声 RGB 图（index-based LCG，无 rand 依赖；噪声使 JPEG
/// q100 体积远超 tiny entry_bytes）。
fn noise_rgb(w: u32, h: u32) -> image::RgbImage {
    let mut img = image::RgbImage::new(w, h);
    let mut state: u64 = 0x9E3779B97F4A7C15;
    for px in img.pixels_mut() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let b = (state >> 33) as u8;
        *px = image::Rgb([b, b.wrapping_mul(7), b.wrapping_mul(13)]);
    }
    img
}

/// 纯色图（JPEG 压缩后极小，用于构造"低于全部阈值"的 Keep 用例）。
fn solid_rgb(w: u32, h: u32, c: [u8; 3]) -> image::RgbImage {
    image::RgbImage::from_pixel(w, h, image::Rgb(c))
}

fn encode_jpeg(img: &image::RgbImage, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cursor = Cursor::new(&mut out);
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
    image::DynamicImage::ImageRgb8(img.clone())
        .write_with_encoder(encoder)
        .unwrap();
    out
}

fn write_file(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.0.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn keep_when_under_thresholds() {
    let dir = TempDir::new("keep");
    // 纯色 32x24 JPEG q85 ≈ 数百字节：字节、维度均低于 tiny 阈值 → Keep。
    let src = write_file(
        &dir,
        "small.png",
        &encode_jpeg(&solid_rgb(32, 24, [10, 20, 30]), 85),
    );
    let v = downscale_gate_with(&src, &dir.0, &tiny_limits()).unwrap();
    assert!(matches!(v, GateVerdict::Keep), "低阈值图应 Keep");
}

#[test]
fn disabled_when_uploads_none_is_caller_side() {
    // None = 开关关：downscale_gate 恒 Keep，不做任何 IO 判定。
    // 这里验证 with 形态在"未超阈值"下 Keep；None 短路在 downscale_gate 里，
    // 行为等价性由 image_attach 既有测试（不传 downscale）钉住。
    let dir = TempDir::new("disabled");
    let src = write_file(&dir, "small.jpg", &encode_jpeg(&noise_rgb(32, 24), 85));
    let limits = Limits {
        entry_bytes: u64::MAX,
        entry_dimension: u32::MAX,
        out_bytes: 1,
    };
    let v = downscale_gate_with(&src, &dir.0, &limits).unwrap();
    assert!(matches!(v, GateVerdict::Keep));
}

#[test]
fn replaces_and_resizes_to_rung() {
    let dir = TempDir::new("replace");
    // 200x150 噪声 jpeg：entry_dimension=100 触发（阶梯第一级缩到 ≤100）。
    let bytes = encode_jpeg(&noise_rgb(200, 150), 95);
    let src = write_file(&dir, "big.jpg", &bytes);
    let v = downscale_gate_with(&src, &dir.0, &tiny_limits()).unwrap();
    let GateVerdict::Replaced {
        path,
        from_bytes,
        to_bytes,
        longest_side,
    } = v
    else {
        panic!("expected Replaced, got Keep");
    };
    assert_eq!(from_bytes as usize, bytes.len());
    assert!(to_bytes > 0 && to_bytes <= tiny_limits().out_bytes);
    assert_eq!(longest_side, 100, "200x150 → 100x75");
    assert!(
        path.starts_with(&dir.0)
            && path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("down_")
    );
    // 产物是合法 JPEG（magic 复验）。
    let head = std::fs::read(&path).unwrap();
    assert_eq!(&head[..3], &[0xFF, 0xD8, 0xFF]);
}

#[test]
fn reuses_content_addressed_product() {
    let dir = TempDir::new("reuse");
    let src = write_file(&dir, "big.jpg", &encode_jpeg(&noise_rgb(200, 150), 95));
    let v1 = downscale_gate_with(&src, &dir.0, &tiny_limits()).unwrap();
    let v2 = downscale_gate_with(&src, &dir.0, &tiny_limits()).unwrap();
    let (GateVerdict::Replaced { path: p1, .. }, GateVerdict::Replaced { path: p2, .. }) = (v1, v2)
    else {
        panic!("expected Replaced both times");
    };
    assert_eq!(p1, p2, "同内容复用同一产物（TTL 复用语义）");
}

#[test]
fn rejects_gif_honestly() {
    let dir = TempDir::new("gif");
    let mut gif = Vec::new();
    image::DynamicImage::ImageRgb8(noise_rgb(64, 48))
        .write_to(&mut Cursor::new(&mut gif), image::ImageFormat::Gif)
        .unwrap();
    let src = write_file(&dir, "anim.gif", &gif);
    let err = downscale_gate_with(&src, &dir.0, &tiny_limits()).unwrap_err();
    assert!(err.contains("GIF"), "honest GIF boundary: {}", err);
}

#[test]
fn rejects_dimension_over_safety_valve() {
    let dir = TempDir::new("valve");
    // 12500x2：触发 entry（>8000px）但撞 12000px 安全阀。
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(noise_rgb(12500, 2))
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let src = write_file(&dir, "wide.png", &png);
    let err = downscale_gate_with(&src, &dir.0, &tiny_limits()).unwrap_err();
    assert!(err.contains("安全上限"), "{}", err);
}

#[test]
fn rejects_corrupt_body_honestly() {
    let dir = TempDir::new("corrupt");
    // 合法 PNG magic + 垃圾 body：尺寸读取/解码阶段诚实 Err。
    let mut bad = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    for _ in 0..4 {
        bad.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    }
    let src = write_file(&dir, "bad.png", &bad);
    assert!(downscale_gate_with(&src, &dir.0, &tiny_limits()).is_err());
}

#[test]
fn flatten_alpha_onto_white() {
    let dir = TempDir::new("alpha");
    // 200x150 全透明红噪声 RGBA PNG（噪声保证字节超 tiny entry → 触发降采样；
    // 透明区在 JPEG 产物中应铺白底）。
    let mut img = image::RgbaImage::from_pixel(200, 150, image::Rgba([255, 0, 0, 0]));
    {
        let noise = noise_rgb(200, 150);
        for (x, y, n) in noise.enumerate_pixels() {
            let p = img.get_pixel_mut(x, y);
            *p = image::Rgba([n[0], n[1], n[2], 0]);
        }
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let src = write_file(&dir, "alpha.png", &png);
    let v = downscale_gate_with(&src, &dir.0, &tiny_limits()).unwrap();
    let GateVerdict::Replaced { path, .. } = v else {
        panic!("expected Replaced");
    };
    let product = image::load_from_memory(&std::fs::read(&path).unwrap()).unwrap();
    use image::GenericImageView as _;
    // alpha=0 的像素铺白后应来自背景白色（噪声值只来自 RGB 分量的透明叠加…
    // overlay 语义：src alpha=0 → 结果=背景白）。
    let px = product.get_pixel(0, 0);
    assert!(
        px[0] > 240 && px[1] > 240 && px[2] > 240,
        "transparent pixel must flatten onto white, got {:?}",
        px
    );
}
