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

// BUG-20261117-086：非有限范数闸——零范数闸 `norm == 0.0` 拦不住 NaN
// （比较恒 false）也拦不住 inf（放行）。模型嵌入（sherpa-onnx 原始输出，
// 无上游有限校验）与 f32 平方累加（有限大元素即可溢出 inf）都能把
// 非有限范数送进除法。NaN 分数在 speaker_test 的 `similarity >= 0.0`
// 事件闸静默消失（事件无声丢失=不诚实）；inf 分数过闸判 matched=true
// 且 serde_json 把 inf 序列化为 null。非有限一律 0.0（不可比），与共享
// 实现 nemesis_types::utils::cosine_similarity_f32（BUG-20261116-015）
// 同一教义。
#[test]
fn cosine_similarity_nan_element_returns_zero_not_nan() {
    // 嵌入向量携带 NaN（模型输出异常形态）：两侧各测一次。
    let a = [f32::NAN, 1.0_f32];
    let b = [1.0_f32, 1.0];
    let s = cosine_similarity(&a, &b);
    assert!(
        s.is_finite(),
        "NaN element must not leak NaN score, got {s}"
    );
    assert_eq!(s, 0.0, "non-comparable vectors must score 0.0");

    let s2 = cosine_similarity(&b, &a);
    assert!(s2.is_finite(), "NaN on b side must not leak, got {s2}");
    assert_eq!(s2, 0.0);
}

#[test]
fn cosine_similarity_infinite_element_returns_zero() {
    // inf 元素 → 范数 inf → 旧代码 dot/(inf*inf) = NaN 静默泄漏。
    let a = [f32::INFINITY, 1.0_f32];
    let b = [f32::INFINITY, 1.0_f32];
    let s = cosine_similarity(&a, &b);
    assert!(
        s.is_finite(),
        "inf element must not leak non-finite, got {s}"
    );
    assert_eq!(s, 0.0);
}

#[test]
fn cosine_similarity_f32_square_overflow_returns_zero_not_nan() {
    // 有限大元素（1e20）平方溢出 f32 → 范数与点积双双 inf →
    // 旧代码 inf/inf = NaN。守卫必须在此之前拦截。
    let a = [1e20_f32, 1e20];
    let b = [1e20_f32, 1e20];
    let s = cosine_similarity(&a, &b);
    assert!(s.is_finite(), "f32 overflow must not leak NaN, got {s}");
    assert_eq!(s, 0.0);
}

// ===========================================================================
// 覆盖率冲刺：引擎构造分支 + FFI 结构甄别
// 无 DLL 时 sherpa_fn 包装器在符号查找处 panic "sherpa-onnx not initialized"
// ——should_panic 测试正好覆盖 #[repr(C)] 结构构造后到 FFI 调用之间的代码。
// ===========================================================================

#[test]
fn speaker_engine_new_missing_model_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let err = format!(
        "{:#}",
        SpeakerEngine::new(tmp.path(), 1).err().expect("must fail")
    );
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
        tmp.path()
            .join("3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx"),
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

// ===========================================================================
// 白盒 null 指针 → embed / manager 各方法的封送路径
// （走到 FFI 符号查找处 panic）。剩余不可达 = FFI 成功返回值语义（真 DLL）。
// 注意：两个引擎的 Drop 同样在 null 上 panic——持有 null 实例的测试一律
// `mem::forget` 收尾；方法 panic 用 `catch_panic_msg` 截住展开（should_panic
// 会双 panic fail-fast，见 test_util.rs）。
// ===========================================================================

fn null_speaker_engine() -> SpeakerEngine {
    SpeakerEngine {
        extractor: std::ptr::null(),
        dim: 192,
    }
}

fn null_speaker_manager() -> SpeakerManager {
    SpeakerManager {
        manager: std::ptr::null(),
        dim: 192,
    }
}

#[test]
fn speaker_engine_embedding_dim_is_pure_field_read() {
    let e = null_speaker_engine();
    assert_eq!(e.embedding_dim(), 192);
    std::mem::forget(e);
}

#[test]
fn speaker_engine_embed_panics_at_stream_creation() {
    let e = null_speaker_engine();
    let msg = crate::test_util::catch_panic_msg(|| e.embed(&[0.1; 1600], 16000));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(e);
}

#[test]
fn speaker_manager_dim_is_pure_field_read() {
    let m = null_speaker_manager();
    assert_eq!(m.dim(), 192);
    std::mem::forget(m);
}

#[test]
fn speaker_manager_register_multi_empty_returns_false_without_ffi() {
    // 空列表早退（129-131 纯逻辑），不碰 FFI
    let mut m = null_speaker_manager();
    assert!(!m.register_multi("alice", &[]));
    std::mem::forget(m);
}

#[test]
fn speaker_manager_register_panics_at_symbol_lookup() {
    let mut m = null_speaker_manager();
    let msg = crate::test_util::catch_panic_msg(|| m.register("alice", &[0.1; 192]));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(m);
}

#[test]
fn speaker_manager_register_multi_nonempty_panics_at_symbol_lookup() {
    let mut m = null_speaker_manager();
    let msg = crate::test_util::catch_panic_msg(|| {
        m.register_multi("alice", &[vec![0.1; 192], vec![0.2; 192]])
    });
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(m);
}

#[test]
fn speaker_manager_remove_panics_at_symbol_lookup() {
    let mut m = null_speaker_manager();
    let msg = crate::test_util::catch_panic_msg(|| m.remove("alice"));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(m);
}

#[test]
fn speaker_manager_verify_panics_at_symbol_lookup() {
    let m = null_speaker_manager();
    let msg = crate::test_util::catch_panic_msg(|| m.verify("alice", &[0.1; 192], 0.7));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(m);
}

#[test]
fn speaker_manager_search_panics_at_symbol_lookup() {
    let m = null_speaker_manager();
    let msg = crate::test_util::catch_panic_msg(|| m.search(&[0.1; 192], 0.7));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(m);
}

#[test]
fn speaker_manager_list_speakers_panics_at_symbol_lookup() {
    let m = null_speaker_manager();
    let msg = crate::test_util::catch_panic_msg(|| m.list_speakers());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(m);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn speaker_engine_drop_null_panics_at_destroy() {
    let _ = null_speaker_engine();
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn speaker_manager_drop_null_panics_at_destroy() {
    let _ = null_speaker_manager();
}

// ===========================================================================
// BUG-20261106-020：名字含内嵌 NUL（JSON "\u0000" 合法、WSAPI speaker 命令
// 原样透传）→ CString::new 失败。此前 .unwrap() panic 且发生在调用方持
// std Mutex 上下文（web handler 持 speaker_manager_state）→ 锁毒化 → 语音
// 子系统进程内瘫痪。修复：FFI 之前诚实 false。null manager 下若误入 FFI
// 会 panic "sherpa-onnx not initialized"——本测试同时锁「不 panic」与
// 「不进 FFI」两件事。
// ===========================================================================

#[test]
fn speaker_manager_nul_name_returns_false_before_ffi() {
    let mut m = null_speaker_manager();
    assert!(!m.register("a\0b", &[0.1; 192]));
    assert!(!m.register_multi("a\0b", &[vec![0.1; 192]]));
    assert!(!m.remove("a\0b"));
    assert!(!m.verify("a\0b", &[0.1; 192], 0.7));
    // 空 embeddings + NUL 名：早退在前，同样 false 不 panic
    assert!(!m.register_multi("a\0b", &[]));
    std::mem::forget(m);
}

// ===========================================================================
// BUG-20261125-044：ComputeEmbedding null 缓冲诚实错误 + Manager 四方法
// 长度闸。C API（Add/AddListFlattened/Verify/Search）不接收长度参数、按
// 构造时 dim 无条件读——错误长度切片（含 embed null 臂旧形态产出的空
// Vec）= 堆越界读。守卫缺失时这些调用会撞 FFI 符号查找 panic（无 DLL
// 环境），测试即红；守卫在位时提前 false/None，不 panic。
// ===========================================================================

#[test]
fn collect_embedding_null_buffer_is_err_not_empty_ok() {
    let err = format!(
        "{:#}",
        collect_embedding(std::ptr::null(), 192).expect_err("null buffer must be Err")
    );
    assert!(err.contains("null buffer"), "{err}");
    assert!(err.contains("192"), "dim must appear in the message: {err}");
}

#[test]
fn collect_embedding_copies_dim_floats_from_valid_buffer() {
    let buf = [1.5_f32, -2.0, 3.25];
    let v = collect_embedding(buf.as_ptr(), 3).expect("valid buffer must be Ok");
    assert_eq!(v, vec![1.5, -2.0, 3.25]);
}

#[test]
fn speaker_manager_register_wrong_length_returns_false_before_ffi() {
    let mut m = null_speaker_manager();
    assert!(
        !m.register("alice", &[]),
        "empty embedding must be rejected pre-FFI"
    );
    assert!(!m.register("alice", &[0.1; 191]), "191 != dim 192");
    assert!(!m.register("alice", &[0.1; 193]), "193 != dim 192");
    std::mem::forget(m);
}

#[test]
fn speaker_manager_register_multi_wrong_length_component_returns_false_before_ffi() {
    let mut m = null_speaker_manager();
    assert!(!m.register_multi("alice", &[vec![0.1; 192], vec![0.2; 191]]));
    std::mem::forget(m);
}

#[test]
fn speaker_manager_verify_wrong_length_returns_false_before_ffi() {
    let m = null_speaker_manager();
    assert!(!m.verify("alice", &[], 0.7));
    assert!(!m.verify("alice", &[0.1; 191], 0.7));
    std::mem::forget(m);
}

#[test]
fn speaker_manager_search_wrong_length_returns_none_before_ffi() {
    let m = null_speaker_manager();
    assert!(m.search(&[], 0.7).is_none());
    assert!(m.search(&[0.1; 191], 0.7).is_none());
    std::mem::forget(m);
}
