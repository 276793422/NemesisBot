// dlp.rs 覆盖率补充测试（scan_text 动态规则命中臂 258 / redact_content
// 的禁用规则跳过臂 322 与命中收尾 328）。
//
// 豁免：328 本身可由「动态规则命中」覆盖（if-let Ok 的块收尾在 Ok 时
// 执行）——由 redact 动态规则测试驱动，非死臂；add_rule 的模式校验
// （369）保证进来的 regex 恒合法，Err 臂不可达但 328 不需要它。

use super::*;
use serde_json::json;

fn cov_rule(name: &str, pattern: &str, enabled: bool) -> DlpRule {
    DlpRule {
        name: name.to_string(),
        category: "coverage".to_string(),
        pattern: pattern.to_string(),
        enabled,
        action: "redact".to_string(),
        confidence: DlpConfidence::High,
    }
}

/// 动态规则命中 → scan_text 推入匹配（258）；redact_content 用同规则
/// 打码（328 的 Ok 块收尾）。
#[test]
fn dynamic_rule_match_drives_scan_and_redact() {
    let dlp = DlpEngine::new(true, "redact");
    dlp.add_rule(cov_rule("cov-token", "COVTOKEN[0-9]{6}", true))
        .unwrap();

    let text = "request id COVTOKEN123456 embedded";
    let r = dlp.scan_text(text);
    assert!(r.has_matches, "{:?}", r.summary);
    assert!(r.matches.iter().any(|m| m.rule_name == "cov-token"));

    let red = dlp.redact_content(text);
    assert!(!red.contains("COVTOKEN123456"), "命中必须被替换: {red}");
    assert!(
        red.contains("request id") && red.contains("embedded"),
        "{red}"
    );
}

/// 禁用的动态规则在 redact 中被跳过（322 continue），不产生替换。
#[test]
fn disabled_dynamic_rule_skipped_in_redact() {
    let dlp = DlpEngine::new(true, "redact");
    dlp.add_rule(cov_rule("cov-off", "SECRETOFF[0-9]{4}", false))
        .unwrap();

    let text = "value SECRETOFF9999 stays";
    let red = dlp.redact_content(text);
    assert!(red.contains("SECRETOFF9999"), "禁用规则不得替换: {red}");
}

/// 对照：scan_tool_input 走 extract_text 路径。
#[test]
fn scan_tool_input_extracts_text_from_args() {
    let dlp = DlpEngine::new(true, "block");
    dlp.add_rule(cov_rule("cov-args", "ARGSECRET[0-9]{5}", true))
        .unwrap();
    let r = dlp.scan_tool_input("write_file", &json!({"content": "x ARGSECRET12345 y"}));
    assert!(r.has_matches);
}
