use super::*;

/// 回归：用户报告 "【**2026年7月3日 周五 下午两点四十**！】" 无法播放。
/// 根因——CJK 之间的 ASCII 空格让 Kokoro 抛 "Unknown token"（issue 2223），
/// 此前 normalize 只删 emoji 后的孤儿空格，漏了中文之间手打的空格。
#[test]
fn normalize_user_regression_no_cjk_adjacent_space() {
    let out = normalize_tts_text("【**2026年7月3日 周五 下午两点四十**！】");
    assert_eq!(out, "2026年7月3日周五下午两点四十!");
    assert!(
        !out.contains(' '),
        "normalized text must contain no ASCII space: {out:?}"
    );
}

#[test]
fn normalize_drops_space_between_cjk() {
    assert_eq!(normalize_tts_text("你 好"), "你好");
    assert_eq!(normalize_tts_text("3日 周五"), "3日周五");
    // 多个连续 CJK 间空格全去
    assert_eq!(normalize_tts_text("你 好 吗"), "你好吗");
    // CJK 与 ASCII 数字之间也去（前是 CJK）
    assert_eq!(normalize_tts_text("第3 章"), "第3章");
}

#[test]
fn normalize_keeps_english_spaces() {
    // 英文单词之间的空格必须保留（Kokoro 英文路径需要）
    assert_eq!(normalize_tts_text("hello world"), "hello world");
    assert_eq!(normalize_tts_text("it is 2026"), "it is 2026");
}

#[test]
fn normalize_drops_orphan_space_after_stripped() {
    // emoji 被剥后紧跟的空格也要去（原有逻辑不能回归）
    assert_eq!(normalize_tts_text("你好😀 世界"), "你好世界");
}

#[test]
fn normalize_collapses_adjacent_punct() {
    // 连续/相邻标点会触发 C++ 异常，只保留一个
    assert_eq!(normalize_tts_text("你好。。世界"), "你好.世界");
    assert_eq!(normalize_tts_text("真的吗？？"), "真的吗?");
}

#[test]
fn normalize_maps_fullwidth_punct() {
    assert_eq!(normalize_tts_text("你好，世界。"), "你好,世界.");
}

#[test]
fn normalize_strips_unsafe_symbols() {
    // 【】、markdown 星号、书名号等都不在白名单，必须剥掉（不能漏到 Kokoro）
    let out = normalize_tts_text("《**测试**》");
    assert_eq!(out, "测试");
}

// ===========================================================================
// S12 覆盖率冲刺（2026-08-26）：引擎构造分支 + FFI 结构甄别
//
// 无 DLL 时 `safe_create_offline_tts → get_raw_fn` 会 panic
// "sherpa-onnx not initialized"——should_panic 测试正好执行**全部**
// #[repr(C)] 结构构造（152-170 / 234-275）后在 FFI 查符号处 panic，
// 不加载任何真 DLL（红线遵守）。
// ===========================================================================

fn touch(dir: &Path, name: &str) {
    std::fs::write(dir.join(name), b"fixture-bytes").unwrap();
}

// ---------------------------------------------------------------------------
// TtsEngine::new —— 参数校验 fail-fast（不碰 FFI）
// ---------------------------------------------------------------------------

#[test]
fn tts_new_vits_missing_model_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let err = format!("{:#}", TtsEngine::new(tmp.path(), 1).err().expect("must fail"));
    assert!(err.contains("TTS model not found"), "{err}");
}

#[test]
fn tts_new_vits_missing_tokens_bails() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "model.onnx");
    let err = format!("{:#}", TtsEngine::new(tmp.path(), 1).err().expect("must fail"));
    assert!(err.contains("TTS tokens not found"), "{err}");
}

#[test]
fn tts_new_vits_missing_lexicon_bails() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "model.onnx");
    touch(tmp.path(), "tokens.txt");
    let err = format!("{:#}", TtsEngine::new(tmp.path(), 1).err().expect("must fail"));
    assert!(err.contains("TTS lexicon not found"), "{err}");
}

#[test]
fn tts_new_kokoro_missing_model_bails() {
    let tmp = tempfile::tempdir().unwrap();
    // voices.bin 在场 → 走 kokoro 分支；model.onnx 缺
    touch(tmp.path(), "voices.bin");
    let err = format!("{:#}", TtsEngine::new(tmp.path(), 1).err().expect("must fail"));
    assert!(err.contains("Kokoro TTS model not found"), "{err}");
}

#[test]
fn tts_new_kokoro_missing_tokens_bails() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "voices.bin");
    touch(tmp.path(), "model.onnx");
    let err = format!("{:#}", TtsEngine::new(tmp.path(), 1).err().expect("must fail"));
    assert!(err.contains("Kokoro tokens not found"), "{err}");
}

// ---------------------------------------------------------------------------
// TtsEngine::new —— 全套夹具 → 结构构造 → 未初始化 panic（不加载真 DLL）
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn tts_new_vits_full_fixture_builds_structs_until_ffi() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "model.onnx");
    touch(tmp.path(), "tokens.txt");
    touch(tmp.path(), "lexicon.txt");
    // espeak-ng-data / dict 都缺 → data_dir/dict_dir 空串分支
    let _ = TtsEngine::new(tmp.path(), 2);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn tts_new_vits_with_espeak_data_and_dict_dirs_builds_structs() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "model.onnx");
    touch(tmp.path(), "tokens.txt");
    touch(tmp.path(), "lexicon.txt");
    std::fs::create_dir_all(tmp.path().join("espeak-ng-data")).unwrap();
    std::fs::create_dir_all(tmp.path().join("dict")).unwrap();
    let _ = TtsEngine::new(tmp.path(), 2);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn tts_new_kokoro_full_fixture_both_lexicons_and_fsts() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "voices.bin");
    touch(tmp.path(), "model.onnx");
    touch(tmp.path(), "tokens.txt");
    touch(tmp.path(), "lexicon-us-en.txt");
    touch(tmp.path(), "lexicon-zh.txt");
    touch(tmp.path(), "date-zh.fst");
    touch(tmp.path(), "number-zh.fst");
    touch(tmp.path(), "phone-zh.fst");
    let _ = TtsEngine::new(tmp.path(), 2);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn tts_new_kokoro_en_lexicon_only() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "voices.bin");
    touch(tmp.path(), "model.onnx");
    touch(tmp.path(), "tokens.txt");
    touch(tmp.path(), "lexicon-us-en.txt");
    let _ = TtsEngine::new(tmp.path(), 2);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn tts_new_kokoro_zh_lexicon_only() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "voices.bin");
    touch(tmp.path(), "model.onnx");
    touch(tmp.path(), "tokens.txt");
    touch(tmp.path(), "lexicon-zh.txt");
    let _ = TtsEngine::new(tmp.path(), 2);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn tts_new_kokoro_no_lexicon_no_fsts() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "voices.bin");
    touch(tmp.path(), "model.onnx");
    touch(tmp.path(), "tokens.txt");
    // 无 lexicon / 无 fst → 空串分支
    let _ = TtsEngine::new(tmp.path(), 2);
}

// ---------------------------------------------------------------------------
// build_model_config / build_model_config_with_kokoro —— 纯函数直接断言
// ---------------------------------------------------------------------------

#[test]
fn build_model_config_vits_active_others_empty() {
    let empty = sherpa::null_cstr();
    let model = sherpa::to_cstr("m.onnx");
    let vits = sherpa::SherpaOnnxOfflineTtsVitsModelConfig {
        model: model.as_ptr(),
        lexicon: empty,
        tokens: empty,
        data_dir: empty,
        noise_scale: 0.1,
        noise_scale_w: 0.2,
        length_scale: 1.0,
        dict_dir: empty,
    };
    let provider = sherpa::to_cstr("cpu");
    let cfg = build_model_config(vits, 4, provider.as_ptr(), empty);

    assert_eq!(cfg.num_threads, 4);
    assert_eq!(cfg.debug, 0);
    assert_eq!(cfg.provider, provider.as_ptr());
    // VITS 原样透传
    assert_eq!(cfg.vits.model, model.as_ptr());
    assert_eq!(cfg.vits.noise_scale, 0.1);
    // 其余引擎槽位全部填 empty
    assert_eq!(cfg.matcha.acoustic_model, empty);
    assert_eq!(cfg.kokoro.model, empty);
    assert_eq!(cfg.kokoro.voices, empty);
    assert_eq!(cfg.kitten.model, empty);
    assert_eq!(cfg.zipvoice.encoder, empty);
    assert_eq!(cfg.pocket.lm_flow, empty);
    assert_eq!(cfg.supertonic.vocoder, empty);
}

// ---------------------------------------------------------------------------
// R6（2026-08-27）：build_kokoro_config 直调 voices 缺失臂 + 白盒 generate/Drop
// new() 用 voices.bin 探测分支，voices 缺失时永远走 VITS——kokoro 的
// voices.bin fail-fast 臂只能直调私有构造函数触达。
// ---------------------------------------------------------------------------

#[test]
fn tts_build_kokoro_direct_missing_voices_bails() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "model.onnx");
    let err = format!(
        "{:#}",
        TtsEngine::build_kokoro_config(tmp.path(), 1)
            .expect_err("must fail")
    );
    assert!(err.contains("Kokoro voices.bin not found"), "{err}");
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn tts_new_kokoro_full_fixture_with_espeak_data_and_dict_dirs() {
    // data_dir / dict_dir 存在分支（204-208 / 211-215 的 exists=true 臂）
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "voices.bin");
    touch(tmp.path(), "model.onnx");
    touch(tmp.path(), "tokens.txt");
    std::fs::create_dir_all(tmp.path().join("espeak-ng-data")).unwrap();
    std::fs::create_dir_all(tmp.path().join("dict")).unwrap();
    let _ = TtsEngine::new(tmp.path(), 2);
}

/// 白盒构造 TtsEngine（不触发任何 FFI 构造；tts 指针为 null）。
fn null_tts_engine() -> TtsEngine {
    TtsEngine {
        inner: std::sync::Mutex::new(TtsInner {
            tts: std::ptr::null(),
        }),
        model_dir: std::path::PathBuf::from("."),
        num_threads: 1,
        is_kokoro: false,
        sample_rate: 44100,
    }
}

#[test]
fn tts_generate_null_engine_panics_at_symbol_lookup() {
    // generate 的 normalize → cstr 封送 → lock 取指针 → FFI 符号查找全链路。
    // Drop 也 panic → catch_panic_msg + forget（见 test_util.rs）。
    let engine = null_tts_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.generate("你好 world", 0, 1.0));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn tts_drop_null_engine_panics_at_destroy() {
    let _ = null_tts_engine();
}

// ---------------------------------------------------------------------------
// R6：map_fullwidth_punct 直测（normalize 已覆盖逗/顿/句/问/叹，补分号冒号）
// ---------------------------------------------------------------------------

#[test]
fn map_fullwidth_punct_semicolon_and_colon() {
    assert_eq!(map_fullwidth_punct('；'), ';');
    assert_eq!(map_fullwidth_punct('：'), ':');
    // 非标点原样返回
    assert_eq!(map_fullwidth_punct('汉'), '汉');
    assert_eq!(map_fullwidth_punct('a'), 'a');
}

#[test]
fn build_model_config_with_kokoro_kokoro_active_vits_empty() {
    let empty = sherpa::null_cstr();
    let vits = sherpa::SherpaOnnxOfflineTtsVitsModelConfig {
        model: empty,
        lexicon: empty,
        tokens: empty,
        data_dir: empty,
        noise_scale: 0.0,
        noise_scale_w: 0.0,
        length_scale: 1.0,
        dict_dir: empty,
    };
    let voices = sherpa::to_cstr("voices.bin");
    let kokoro = sherpa::SherpaOnnxOfflineTtsKokoroModelConfig {
        model: empty,
        voices: voices.as_ptr(),
        tokens: empty,
        data_dir: empty,
        length_scale: 1.0,
        dict_dir: empty,
        lexicon: empty,
        lang: empty,
    };
    let provider = sherpa::to_cstr("cpu");
    let cfg = build_model_config_with_kokoro(vits, kokoro, 2, provider.as_ptr(), empty);

    assert_eq!(cfg.num_threads, 2);
    assert_eq!(cfg.kokoro.voices, voices.as_ptr());
    // VITS 槽位保持 empty
    assert_eq!(cfg.vits.model, empty);
    assert_eq!(cfg.vits.lexicon, empty);
    assert_eq!(cfg.matcha.vocoder, empty);
    assert_eq!(cfg.kitten.data_dir, empty);
    assert_eq!(cfg.zipvoice.decoder, empty);
    assert_eq!(cfg.pocket.vocab_json, empty);
    assert_eq!(cfg.supertonic.tts_json, empty);
}
