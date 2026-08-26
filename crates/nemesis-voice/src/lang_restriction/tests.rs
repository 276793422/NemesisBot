use super::*;

#[test]
fn sensevoice_declares_remedy_others_do_not() {
    // SenseVoice 声明需要补救
    let r = default_remedy_for_model("sensevoice-small").expect("sensevoice 需要补救");
    assert!(r.allowed.contains("zh"));
    assert!(r.allowed.contains("en"));
    assert_eq!(r.fallback, "en");

    // 其它模型不声明（换模型 → 自动无补救）
    assert!(default_remedy_for_model("whisper-large-v3").is_none());
    assert!(default_remedy_for_model("paraformer-zh").is_none());
    assert!(default_remedy_for_model("").is_none());
}

#[test]
fn model_match_is_case_insensitive() {
    assert!(default_remedy_for_model("SenseVoice-Small").is_some());
}

#[test]
fn needs_remedy_only_for_disallowed_concrete_lang() {
    let mut allowed = HashSet::new();
    allowed.insert("zh".to_string());
    allowed.insert("en".to_string());
    let remedy = Remedy {
        allowed,
        fallback: "en".into(),
    };

    assert!(!remedy.needs_remedy(Some("zh"))); // 中文放行
    assert!(!remedy.needs_remedy(Some("en"))); // 英文放行
    assert!(remedy.needs_remedy(Some("ja"))); // 日语要补救
    assert!(remedy.needs_remedy(Some("ko"))); // 韩语要补救
    assert!(remedy.needs_remedy(Some("yue"))); // 粤语要补救
    assert!(!remedy.needs_remedy(None)); // 检测不到语言，保守不补救
}

// ===========================================================================
// S12 覆盖率冲刺（2026-08-26）：LangRestriction::new/apply/Drop
// apply 的 needs_remedy=false 两臂（None / 白名单内）不碰识别器，纯逻辑可达；
// needs_remedy=true 臂走 decode_recognizer（null 识别器 → FFI 符号查找 panic，
// 无 DLL 红线内）。Drop 的 null 识别器分支是 no-op。
// ===========================================================================

use super::{LangRestriction, Remedy};

fn zh_remedy() -> Remedy {
    let mut allowed = std::collections::HashSet::new();
    allowed.insert("zh".to_string());
    allowed.insert("en".to_string());
    Remedy {
        allowed,
        fallback: "zh".to_string(),
    }
}

#[test]
fn lang_restriction_apply_none_detected_returns_ok_none() {
    let lr = LangRestriction::new(zh_remedy(), std::ptr::null());
    let out = lr.apply(None, &[0.1; 64], 16000).unwrap();
    assert!(out.is_none(), "no language detected → no remedy: {out:?}");
}

#[test]
fn lang_restriction_apply_allowed_language_returns_ok_none() {
    let lr = LangRestriction::new(zh_remedy(), std::ptr::null());
    for allowed in ["zh", "en"] {
        let out = lr.apply(Some(allowed), &[0.1; 64], 16000).unwrap();
        assert!(out.is_none(), "{allowed} is whitelisted → no remedy: {out:?}");
    }
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn lang_restriction_apply_disallowed_language_decodes_with_fallback() {
    // "ja" 不在白名单 → 走 decode_recognizer → 无 DLL 在符号查找处 panic
    // （覆盖 128-129 两行；成功路径需要真识别器，结构性豁免）
    let lr = LangRestriction::new(zh_remedy(), std::ptr::null());
    let _ = lr.apply(Some("ja"), &[0.1; 64], 16000);
}

#[test]
fn lang_restriction_drop_null_recognizer_is_noop() {
    // Drop 的 null 分支（135 行 false 臂）：构造后直接离开作用域，不得 panic
    let lr = LangRestriction::new(zh_remedy(), std::ptr::null());
    drop(lr);
}
