    use super::normalize_lang_token;

    #[test]
    fn normalize_strips_token_wrapper() {
        assert_eq!(normalize_lang_token("<|zh|>"), "zh");
        assert_eq!(normalize_lang_token("<|en|>"), "en");
        assert_eq!(normalize_lang_token("  <|ja|>  "), "ja");
        assert_eq!(normalize_lang_token("<|yue|>"), "yue");
        assert_eq!(normalize_lang_token("zh"), "zh"); // 裸值原样返回
        assert_eq!(normalize_lang_token(""), "");
    }
