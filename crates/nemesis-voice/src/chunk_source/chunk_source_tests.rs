//! WavSource 切块/重采样测试（W7 注入面）。WAV 编解码本体在 wav_tests；
//! MicSource 依赖真实采集设备，结构性豁免（台账 §9.4 同款）。

use super::*;
use std::fs;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nvoice_chunk_{}_{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn wav_source_chunks_and_exhausts() {
    let dir = temp_dir("chunks");
    let path = dir.join("in.wav");
    let samples: Vec<f32> = (0..1000).map(|i| (i as f32) * 0.001).collect();
    fs::write(&path, crate::wav::write_wav_mono16(&samples, 16000)).unwrap();

    let mut src = WavSource::from_file(&path, 16000, 400).unwrap();
    assert_eq!(src.remaining(), 1000);
    let mut total = 0usize;
    let mut chunks = 0usize;
    while let Some(c) = src.try_chunk() {
        assert!(c.len() <= 400);
        total += c.len();
        chunks += 1;
    }
    assert_eq!(total, 1000);
    assert_eq!(chunks, 3); // 400+400+200
    // 耗尽后再取 = None
    assert!(src.try_chunk().is_none());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn wav_source_resamples_to_target_rate() {
    let dir = temp_dir("resample");
    let path = dir.join("in.wav");
    // 8000Hz 800 样本 = 0.1s → 16000Hz 应约 1600 样本
    let samples: Vec<f32> = vec![0.1; 800];
    fs::write(&path, crate::wav::write_wav_mono16(&samples, 8000)).unwrap();

    let mut src = WavSource::from_file(&path, 16000, 2000).unwrap();
    let mut total = 0usize;
    while let Some(c) = src.try_chunk() {
        total += c.len();
    }
    // 线性插值近似：0.1s × 16000 = 1600 ± 100
    assert!(
        (1500..=1700).contains(&total),
        "resampled length {total} not ~1600"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn wav_source_missing_file_bails() {
    let dir = temp_dir("missing");
    let err = WavSource::from_file(&dir.join("nope.wav"), 16000, 400);
    assert!(err.is_err());
    let _ = fs::remove_dir_all(&dir);
}
