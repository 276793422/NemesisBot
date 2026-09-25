// registry.rs 覆盖率补充测试（lsp_language_id 全语言臂 41-45）。

use super::*;

/// lsp_language_id：五语言各归其位（41-45 逐臂）。
#[test]
fn lsp_language_id_covers_all_langs() {
    assert_eq!(Lang::Rust.lsp_language_id(), "rust");
    assert_eq!(Lang::Go.lsp_language_id(), "go");
    assert_eq!(Lang::TypeScript.lsp_language_id(), "typescript");
    assert_eq!(Lang::Python.lsp_language_id(), "python");
    assert_eq!(Lang::C.lsp_language_id(), "cpp");
}

/// probe_available：只返回表内语言、去重，且与 find_command 的单语言探测
/// 结论一致（装了 rust-analyzer ⇒ 必含 Rust；没装 ⇒ 必不含）。
#[test]
fn probe_available_matches_per_lang_find_command_truth() {
    let probed = probe_available();
    let mut seen = std::collections::HashSet::new();
    for lang in &probed {
        assert!(spec_for(*lang).is_some(), "探到的语言必须在表内：{lang:?}");
        assert!(seen.insert(*lang), "不得重复：{lang:?}");
    }
    assert_eq!(
        probed.contains(&Lang::Rust),
        find_command("rust-analyzer").is_some(),
        "probe_available 与逐语言 server_available 必须同真同假"
    );
    assert_eq!(
        probed.contains(&Lang::Go),
        server_available(Lang::Go),
        "Go 同理"
    );
}
