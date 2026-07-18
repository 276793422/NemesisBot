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
