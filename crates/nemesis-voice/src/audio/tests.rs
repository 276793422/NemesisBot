use super::*;

#[test]
fn resampler_ratio_same_rate_is_one() {
    let r = Resampler::new(16000, 16000).unwrap();
    assert_eq!(r.ratio(), 1.0);
}

#[test]
fn resampler_ratio_downsample_is_less_than_one() {
    // 44100 → 16000
    let r = Resampler::new(44100, 16000).unwrap();
    let ratio = r.ratio();
    assert!((ratio - 16000.0 / 44100.0).abs() < 1e-6);
}

#[test]
fn resampler_ratio_upsample_is_greater_than_one() {
    let r = Resampler::new(8000, 16000).unwrap();
    assert!(r.ratio() > 1.0);
}

#[test]
fn resampler_same_rate_returns_input_unchanged() {
    let mut r = Resampler::new(16000, 16000).unwrap();
    let input = vec![0.1, 0.2, 0.3, 0.4];
    let out = r.resample(&input);
    assert_eq!(out, input);
}

#[test]
fn resampler_empty_input_returns_empty() {
    let mut r = Resampler::new(8000, 16000).unwrap();
    let out = r.resample(&[]);
    assert!(out.is_empty());
}

#[test]
fn resampler_downsample_output_length_shrinks_proportionally() {
    let mut r = Resampler::new(16000, 8000).unwrap();
    let input: Vec<f32> = (0..160).map(|i| i as f32 / 160.0).collect();
    let out = r.resample(&input);
    // 160 samples / (16000/8000) = 80 samples expected
    assert!(
        (out.len() as i32 - 80).abs() <= 1,
        "expected ~80 samples, got {}",
        out.len()
    );
}

#[test]
fn resampler_upsample_output_length_grows_proportionally() {
    let mut r = Resampler::new(8000, 16000).unwrap();
    let input: Vec<f32> = (0..40).map(|i| i as f32 / 40.0).collect();
    let out = r.resample(&input);
    // 40 * (16000/8000) = 80 samples expected
    assert!(
        (out.len() as i32 - 80).abs() <= 1,
        "expected ~80 samples, got {}",
        out.len()
    );
}

#[test]
fn resampler_preserves_constant_signal_amplitude() {
    // Linear interpolation of a constant signal should yield the same constant
    let mut r = Resampler::new(8000, 16000).unwrap();
    let input = vec![0.5_f32; 100];
    let out = r.resample(&input);
    // All interior samples should be 0.5 (boundary effects may differ)
    let interior_max = out.iter().take(150).cloned().fold(0.0_f32, f32::max);
    let interior_min = out.iter().take(150).cloned().fold(1.0_f32, f32::min);
    assert!(
        (interior_max - 0.5).abs() < 1e-5 && (interior_min - 0.5).abs() < 1e-5,
        "interior samples should be 0.5, got [{}, {}]",
        interior_min,
        interior_max
    );
}

#[test]
fn resampler_reset_is_noop_safe() {
    let mut r = Resampler::new(8000, 16000).unwrap();
    r.reset(); // Should not panic, should not change state
    assert!(r.ratio() > 1.0);
}

#[test]
fn resampler_new_succeeds_with_unusual_rates() {
    // Edge: very low rates
    let r = Resampler::new(1, 2).unwrap();
    assert_eq!(r.ratio(), 2.0);

    // Edge: very high rates
    let r2 = Resampler::new(192000, 48000).unwrap();
    assert!(r2.ratio() < 1.0);
}

// ===========================================================================
// S12 覆盖率冲刺（2026-08-26）：设备枚举 / 伪设备名 fail-fast / far-end 单例
// 只做 cpal 设备**枚举**与名字匹配失败路径——不 open 任何真实音频流
// （真麦克风/真扬声器打开是红线，报告里列结构性豁免）。
// ===========================================================================

#[test]
fn list_devices_enumerates_or_fails_cleanly() {
    // 有音频栈的机器：返回设备列表（输入在前、输出在后，索引连续）；
    // 无音频栈的 CI：允许 Err（host 构造失败），但绝不能 panic。
    match list_devices() {
        Ok(devices) => {
            for (i, d) in devices.iter().enumerate() {
                assert_eq!(d.index, i, "indices must be contiguous (name={})", d.name);
                assert!(!d.name.is_empty() || d.name == "Unknown");
            }
        }
        Err(e) => {
            let msg = format!("{e}");
            assert!(!msg.is_empty());
        }
    }
}

#[test]
fn audio_capture_bogus_device_name_bails_without_opening_stream() {
    let err = format!("{:#}", AudioCapture::new("no-such-input-device-xyz").err().expect("must fail"));
    assert!(err.contains("Input device 'no-such-input-device-xyz' not found"), "{err}");
}

#[test]
fn audio_playback_bogus_device_name_bails_without_opening_stream() {
    let err = format!(
        "{:#}",
        AudioPlayback::new("no-such-output-device-xyz", 16000, 1.0)
            .err()
            .expect("must fail")
    );
    assert!(err.contains("Output device 'no-such-output-device-xyz' not found"), "{err}");
}

#[test]
fn far_end_buffer_returns_shared_handle() {
    let a = far_end_buffer();
    let b = far_end_buffer();
    assert!(Arc::ptr_eq(&a, &b), "must be the same shared buffer");
    // 写进一个句柄，另一个句柄可见（AEC 读端语义）
    a.lock().unwrap().push_back(0.5f32);
    assert_eq!(b.lock().unwrap().back(), Some(&0.5));
    // 清理，避免污染其他测试读到的 far-end
    b.lock().unwrap().clear();
}

#[test]
fn far_end_sample_rate_defaults_to_48000() {
    // 从未创建过播放设备时的兜底默认（166 行）；不写 FAR_END_RATE 保持默认语义
    assert_eq!(far_end_sample_rate(), 48000);
}

#[test]
fn resampler_ratio_and_reset() {
    let mut r = Resampler::new(48000, 16000).unwrap();
    assert!((r.ratio() - 1.0 / 3.0).abs() < 1e-6);
    let out = r.resample(&[1.0, 0.5, 1.0, 0.5, 1.0, 0.5]);
    assert!(!out.is_empty());
    r.reset(); // no-op，但钉住 API 不 panic
    // 同率直通
    let mut same = Resampler::new(16000, 16000).unwrap();
    assert_eq!(same.ratio(), 1.0);
    assert_eq!(same.resample(&[0.25, -0.25]), vec![0.25, -0.25]);
}
