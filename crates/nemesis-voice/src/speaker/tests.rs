use super::*;

#[test]
fn cosine_similarity_identical_vectors_is_one() {
    let a = [1.0_f32, 2.0, 3.0, 4.0];
    let s = cosine_similarity(&a, &a);
    assert!(
        (s - 1.0).abs() < 1e-5,
        "identical vectors should give 1.0, got {}",
        s
    );
}

#[test]
fn cosine_similarity_orthogonal_vectors_is_zero() {
    // [1,0] · [0,1] = 0 → cosine = 0
    let a = [1.0_f32, 0.0];
    let b = [0.0_f32, 1.0];
    let s = cosine_similarity(&a, &b);
    assert!(
        s.abs() < 1e-5,
        "orthogonal vectors should give 0.0, got {}",
        s
    );
}

#[test]
fn cosine_similarity_opposite_vectors_is_minus_one() {
    let a = [1.0_f32, 1.0];
    let b = [-1.0_f32, -1.0];
    let s = cosine_similarity(&a, &b);
    assert!(
        (s - (-1.0)).abs() < 1e-5,
        "opposite vectors should give -1.0, got {}",
        s
    );
}

#[test]
fn cosine_similarity_mismatched_lengths_returns_zero() {
    let a = [1.0_f32, 2.0, 3.0];
    let b = [1.0_f32, 2.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn cosine_similarity_empty_vectors_returns_zero() {
    let a: [f32; 0] = [];
    let b: [f32; 0] = [];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn cosine_similarity_zero_norm_a_returns_zero() {
    let a = [0.0_f32, 0.0, 0.0];
    let b = [1.0_f32, 2.0, 3.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn cosine_similarity_zero_norm_b_returns_zero() {
    let a = [1.0_f32, 2.0, 3.0];
    let b = [0.0_f32, 0.0, 0.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn cosine_similarity_normalized_angle_corresponds_to_value() {
    // 60-degree angle between unit vectors → cos(60°) ≈ 0.5
    let a = [1.0_f32, 0.0];
    let b = [0.5_f32, 3.0_f32.sqrt() / 2.0]; // 60° from a
    let s = cosine_similarity(&a, &b);
    assert!(
        (s - 0.5).abs() < 1e-5,
        "60° angle → cosine ≈ 0.5, got {}",
        s
    );
}

#[test]
fn cosine_similarity_single_element_vectors() {
    let a = [2.0_f32];
    let b = [3.0_f32];
    // [2] · [3] = 6; |2| = 2, |3| = 3; cosine = 6 / (2*3) = 1.0
    assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);
}

#[test]
fn cosine_similarity_negative_single_element() {
    let a = [2.0_f32];
    let b = [-3.0_f32];
    // cos = -6 / (2*3) = -1.0
    assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 1e-5);
}

#[test]
fn cosine_similarity_large_vectors_do_not_overflow_to_nan() {
    // Stress: 1024-dim embeddings (typical speaker embedding size)
    let a: Vec<f32> = (0..1024).map(|i| (i as f32) / 1024.0).collect();
    let b: Vec<f32> = (0..1024).map(|i| ((i + 100) as f32) / 1024.0).collect();
    let s = cosine_similarity(&a, &b);
    assert!(s.is_finite(), "result should be finite");
    assert!(
        s > 0.0 && s <= 1.0 + 1e-5,
        "similar vectors → positive cos, got {}",
        s
    );
}

// ===========================================================================
// S12 覆盖率冲刺（2026-08-26）：引擎构造分支 + FFI 结构甄别
// 无 DLL 时 sherpa_fn 包装器在符号查找处 panic "sherpa-onnx not initialized"
// ——should_panic 测试正好覆盖 #[repr(C)] 结构构造后到 FFI 调用之间的代码。
// ===========================================================================

#[test]
fn speaker_engine_new_missing_model_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let err = format!("{:#}", SpeakerEngine::new(tmp.path(), 1).err().expect("must fail"));
    assert!(err.contains("Speaker model not found"), "{err}");
    assert!(
        err.contains("3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx"),
        "{err}"
    );
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn speaker_engine_new_with_model_builds_structs_until_ffi() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx"),
        b"fixture",
    )
    .unwrap();
    let _ = SpeakerEngine::new(tmp.path(), 2);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn speaker_manager_new_panics_at_symbol_lookup() {
    let _ = SpeakerManager::new(192);
}
