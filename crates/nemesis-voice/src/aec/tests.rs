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
    let err = format!(
        "{:#}",
        super::init(std::path::Path::new("Z:/no/such/aec.dll")).unwrap_err()
    );
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

// ===========================================================================
// R6（2026-08-27）：白盒 null 句柄 → 攒帧/裁剪纯逻辑 + process_one_frame /
// Drop 的封送路径（走到 AEC 符号查找处 panic）。剩余不可达 = FFI 成功语义
// （真 aec.dll）+ symbol() 第二闭包内层（无已加载库恒不可达）。
// ===========================================================================

/// 白盒构造 SpeexAec：handle 为 null，不触发任何 FFI。
fn null_speex(frame_size: usize) -> SpeexAec {
    SpeexAec {
        handle: std::ptr::null_mut(),
        frame_size,
        near_buf: Vec::new(),
        far_buf: Vec::new(),
        out_buf: Vec::new(),
        rec_s16: Vec::new(),
        echo_s16: Vec::new(),
        out_s16: vec![0i16; frame_size],
    }
}

#[test]
fn speex_process_small_near_returns_empty_without_ffi() {
    // near 不足一帧 → while 不进 → 返回空 Vec（不碰 FFI）
    let mut aec = null_speex(160);
    let out = aec.process(&[0.1; 100], &[0.2; 100]);
    assert!(out.is_empty());
    assert_eq!(aec.near_buf.len(), 100, "near 应攒在缓冲里");
    assert_eq!(aec.far_buf.len(), 100, "far 应攒在缓冲里");
}

#[test]
fn speex_process_trims_oversized_far_buffer() {
    // far 堆积超过 10 帧上限 → 裁掉超额（262-265 trim 臂）
    let mut aec = null_speex(160);
    let _ = aec.process(&[0.1; 100], &[0.2; 160 * 12]);
    assert_eq!(aec.far_buf.len(), 160 * 10, "far 应被裁到 10 帧上限");
}

#[test]
fn speex_process_takes_accumulated_out_buf() {
    // out_buf 已有待返回样本 → process 取走并清空（267 mem::take）
    let mut aec = null_speex(160);
    aec.out_buf = vec![0.5; 4];
    let out = aec.process(&[], &[]);
    assert_eq!(out, vec![0.5, 0.5, 0.5, 0.5]);
    assert!(aec.out_buf.is_empty(), "取走后 out_buf 应清空");
}

#[test]
#[should_panic(expected = "AEC lib not initialized")]
fn speex_process_one_frame_full_far_panics_at_symbol_lookup() {
    // far ≥ frame：取整帧 + drain（217-224 臂）→ AecCancelEcho 符号查找 panic
    let mut aec = null_speex(160);
    aec.near_buf = vec![0.1; 160];
    aec.far_buf = vec![0.2; 200];
    aec.process_one_frame();
}

#[test]
#[should_panic(expected = "AEC lib not initialized")]
fn speex_process_one_frame_short_far_pads_silence_then_panics() {
    // far 不足一帧：取全部 + 静音补齐（225-228 臂）→ 符号查找 panic
    let mut aec = null_speex(160);
    aec.near_buf = vec![0.1; 160];
    aec.far_buf = vec![0.2; 80];
    aec.process_one_frame();
    // panic 前已 drain —— 不可断言，should_panic 即验证
}

#[test]
#[should_panic(expected = "AEC lib not initialized")]
fn speex_process_full_near_frame_panics_via_process_loop() {
    // process 攒够整帧 → while 进 → process_one_frame → panic
    let mut aec = null_speex(160);
    let _ = aec.process(&[0.1; 160], &[0.2; 160]);
}

#[test]
fn speex_drop_null_handle_is_noop() {
    // null 句柄 → Drop 跳过销毁（273 false 臂），不得 panic
    let aec = null_speex(160);
    drop(aec);
}

#[test]
#[should_panic(expected = "AEC lib not initialized")]
fn speex_drop_nonnull_handle_panics_at_destroy() {
    // 非 null 句柄（悬垂但不解引用）→ AecDestroy 符号查找 panic（274-276）
    let mut aec = null_speex(160);
    aec.handle = std::ptr::dangling_mut::<Aec>();
    drop(aec);
}
