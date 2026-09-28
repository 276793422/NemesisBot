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
