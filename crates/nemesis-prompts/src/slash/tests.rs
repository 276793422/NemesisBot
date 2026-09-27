//! slash 内置模板库测试（独立测试文件，对齐内联测试防回归纪律）。

use super::*;

#[test]
fn all_templates_deep_with_arguments_slot() {
    for name in ["security-review", "code-review", "debug", "fix"] {
        let tpl = lookup(name).unwrap_or_else(|| panic!("内置模板 {name} 应在场"));
        assert!(
            tpl.contains("$ARGUMENTS"),
            "内置模板 {name} 缺 $ARGUMENTS 占位（参数注入是本机制的契约）"
        );
        assert!(tpl.len() > 500, "深度模板 {name} 内容异常地短：{}B", tpl.len());
        assert!(tpl.contains("# 纪律"), "深度模板 {name} 缺纪律段");
    }
}

#[test]
fn expand_replaces_and_appends() {
    let tpl = lookup("debug").expect("debug 模板应在场");
    // $ARGUMENTS 被替换为参数。
    let rendered = expand(tpl, "登录后白屏");
    assert!(rendered.contains("登录后白屏"));
    assert!(!rendered.contains("$ARGUMENTS"), "替换后占位应消失");
    // 无占位符模板 + 带参数 → 追加独立段；无参数 → 原样。
    assert_eq!(expand("模板正文", "参数"), "模板正文\n\n参数");
    assert_eq!(expand("模板正文", ""), "模板正文");
}

#[test]
fn lookup_misses_honestly() {
    assert!(lookup("security-review").is_some());
    assert!(lookup("no-such-template").is_none());
}
