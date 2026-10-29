//! WAV 编解码纯逻辑测试（解析 / 写入 round-trip / 垃圾输入拒绝 / 立体声下混）。

use super::*;

#[test]
fn wav_write_parse_roundtrip_pcm16() {
    let samples: Vec<f32> = (0..160).map(|i| (i as f32 / 160.0) * 0.5 - 0.25).collect();
    let bytes = write_wav_mono16(&samples, 16000);
    let (parsed, sr) = parse_wav(&bytes).unwrap();
    assert_eq!(sr, 16000);
    assert_eq!(parsed.len(), 160);
    // PCM16 量化误差 ±1/32768，留点余量
    for (a, b) in samples.iter().zip(parsed.iter()) {
        assert!((a - b).abs() < 1e-4, "{} vs {}", a, b);
    }
}

#[test]
fn wav_parse_rejects_garbage() {
    assert!(parse_wav(b"not a wav at all").is_err());
    // RIFF 头对但缺 fmt/data
    let mut b = b"RIFF\x04\x00\x00\x00WAVE".to_vec();
    assert!(parse_wav(&b).is_err());
    b.extend_from_slice(b"data\x00\x00\x00\x00");
    assert!(parse_wav(&b).is_err()); // fmt 缺失
}

#[test]
fn wav_parse_stereo_downmix() {
    // 手工组 stereo PCM16：L=0.5, R=-0.5 → mono 0
    let mut data = Vec::new();
    for _ in 0..4 {
        data.extend_from_slice(&16384i16.to_le_bytes());
        data.extend_from_slice(&(-16384i16).to_le_bytes());
    }
    let mut b = b"RIFF".to_vec();
    b.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&2u16.to_le_bytes()); // stereo
    b.extend_from_slice(&8000u32.to_le_bytes());
    b.extend_from_slice(&32000u32.to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    let (mono, sr) = parse_wav(&b).unwrap();
    assert_eq!(sr, 8000);
    assert_eq!(mono.len(), 4);
    for v in &mono {
        assert!(v.abs() < 1e-6);
    }
}

#[test]
fn wav_write_clamps_and_silence() {
    let bytes = write_wav_mono16(&[2.0, -2.0, 0.0], 16000);
    let (parsed, _) = parse_wav(&bytes).unwrap();
    assert_eq!(parsed.len(), 3);
    assert!((parsed[0] - 1.0).abs() < 1e-3, "clamp to +full scale");
    assert!((parsed[1] + 1.0).abs() < 1e-3, "clamp to -full scale");
    assert!(parsed[2].abs() < 1e-6);
}

// ===== 撒谎 chunk size 算术溢出（BUG 2026-10-29 横扫角度 B）=====

/// 纯函数契约（64 位）：u32::MAX 步进 = 2^32 精确无饱和，语义与旧式
/// `body_start + size.div_ceil(2)*2` 逐字节一致（偶数 size 同样精确）。
#[cfg(target_pointer_width = "64")]
#[test]
fn advance_chunk_pos_64bit_exact() {
    assert_eq!(advance_chunk_pos(12, u32::MAX), 12 + (1u64 << 32) as usize);
    assert_eq!(advance_chunk_pos(20, 7), 28); // 奇数 size 对齐 +1
    assert_eq!(advance_chunk_pos(20, 8), 28); // 偶数 size 原样
    assert_eq!(advance_chunk_pos(20, 0), 20);
}

/// 纯函数契约（32 位）：`size = 0xFFFFFFFF` 旧式 `div_ceil(2)*2` 溢出
/// （release 回绕 0 → 步进错乱按字节垃圾重解析；debug panic）。饱和
/// 形态必须落 `usize::MAX`，主循环条件自然判出界收尾。
#[cfg(target_pointer_width = "32")]
#[test]
fn advance_chunk_pos_32bit_saturates() {
    assert_eq!(advance_chunk_pos(12, u32::MAX), usize::MAX);
}

/// 集成回归锁：data chunk 后跟一个 size=0xFFFFFFFF 的撒谎 chunk——
/// 旧式算术在 32 位 debug 下 panic / release 下错乱重解析；任何平台
/// 都必须正常终止并取回 data chunk 内容（不 hang、不 panic）。
#[test]
fn wav_parse_survives_liar_chunk_size() {
    let mut data = Vec::new();
    data.extend_from_slice(&16384i16.to_le_bytes());
    data.extend_from_slice(&(-8192i16).to_le_bytes());
    let mut b = b"RIFF".to_vec();
    b.extend_from_slice(&0u32.to_le_bytes()); // RIFF size 解析器不校验
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&8000u32.to_le_bytes());
    b.extend_from_slice(&16000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    // 撒谎 chunk：声明 0xFFFFFFFF，实际 0 字节
    b.extend_from_slice(b"JUNK");
    b.extend_from_slice(&u32::MAX.to_le_bytes());
    let (mono, sr) = parse_wav(&b).unwrap();
    assert_eq!(sr, 8000);
    assert_eq!(mono.len(), 2);
    assert!((mono[0] - 0.5).abs() < 1e-3);
}
