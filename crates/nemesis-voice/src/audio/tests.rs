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
