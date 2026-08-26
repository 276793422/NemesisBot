use super::*;

#[test]
fn f32_s16_roundtrip_mid() {
    // f32_to_s16 encodes with ×32767 (symmetric scaling) and s16_to_f32 decodes
    // with ÷32768; combined with `as i16` truncation, the worst-case round-trip
    // error is bounded by ~2/32767 (truncation step + encode/decode scale asymmetry).
    for &v in &[0.0, 0.5, -0.5, 1.0, -1.0, 0.1234, -0.9876] {
        let back = s16_to_f32(f32_to_s16(v));
        assert!((back - v).abs() < 2.0 / 32767.0 + 1e-6, "v={v} back={back}");
    }
}

#[test]
fn f32_to_s16_clamps() {
    // Symmetric ×32767 scaling: -1.0 maps to -32767 (not -32768), keeping the
    // mapping symmetric around 0 and avoiding the asymmetric i16 negative extreme.
    assert_eq!(f32_to_s16(2.0), 32767);
    assert_eq!(f32_to_s16(-2.0), -32767);
    assert_eq!(f32_to_s16(0.0), 0);
}

// ===========================================================================
// S12 覆盖率冲刺（2026-08-26）：init fail-fast + 未初始化判定 + 符号查找 panic
// SpeexAec 的 process/process_one_frame/Drop 需要真 SpeexDLL（aec.dll dlopen）
// ——结构性豁免。这里只测"没加载 DLL"时的一切可达路径。
// ===========================================================================

#[test]
fn init_bad_path_returns_err_not_panic() {
    let err = format!("{:#}", super::init(std::path::Path::new("Z:/no/such/aec.dll")).unwrap_err());
    assert!(err.contains("Failed to load AEC lib"), "{err}");
    assert!(err.contains("Z:/no/such/aec.dll"), "{err}");
}

#[test]
fn init_dummy_file_is_not_a_valid_dll() {
    // 非 PE 文件 → libloading 报错 → Err（Windows %1 不是有效 Win32 应用）
    let tmp = tempfile::tempdir().unwrap();
    let fake = tmp.path().join("aec.dll");
    std::fs::write(&fake, b"definitely-not-a-pe-file").unwrap();
    let res = super::init(&fake);
    assert!(res.is_err(), "dummy file must not load as DLL");
}

#[test]
fn is_initialized_false_without_load() {
    // 本测试二进制从不加载真 AEC DLL；OnceLock 未设置 → false
    assert!(!super::is_initialized());
}

#[test]
#[should_panic(expected = "AEC lib not initialized")]
fn speex_aec_new_panics_when_lib_missing() {
    let _ = SpeexAec::new(160, 2048, 16000, true);
}

#[test]
#[should_panic(expected = "AEC lib not initialized")]
fn speex_aec_with_defaults_panics_when_lib_missing() {
    let _ = SpeexAec::with_defaults();
}
