//! S2②：主线工具输出凭据/DLP 复扫测试（凭据/DLP 双路 + 组合 + 旁路 +
//! action 观测档不改写语义）。
//!
//! 走 `rescan_pair` 组装核直扫扫描器组合（不必构造整个 SecurityPlugin），
//! 另加一例经 `SecurityPlugin::new(Default)` 的入口级集成（覆盖
//! accessors 解包胶水）。

use super::output_rescan::{rescan_pair, rescan_tool_output};
use nemesis_security::credential::Scanner as CredentialScanner;
use nemesis_security::dlp::{DlpConfig, DlpEngine};
use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};

/// AWS 示例键：凭据表 `aws_access_key` 与 DLP 表 `aws_access_key`（High）
/// 双双命中，且不含触发 secret_assignment 等附带规则的关键词。
const AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
/// Luhn 合法 visa：DLP `visa`（Medium→block）命中；凭据表无信用卡规则，
/// 是纯 DLP 侧样本。
const VISA: &str = "4111111111111111";

#[test]
fn credential_hit_is_redacted_with_honest_note() {
    let scanner = CredentialScanner::new(true, "block");
    let input = format!("file content value={AWS_KEY} trailing text");
    let out = rescan_pair(Some(&scanner), None, "read_file", input);

    assert!(!out.contains(AWS_KEY), "明文键必须消失：{out}");
    assert!(
        out.contains("[REDACTED_CREDENTIAL]"),
        "遮蔽 token 应在场：{out}"
    );
    assert!(
        out.contains("[输出已脱敏: 检测到疑似凭据 1 处"),
        "尾注必须告知模型输出被改写：{out}"
    );
}

#[test]
fn credential_warn_action_is_observe_only() {
    // action="warn" = 只观测：scan_tool_output 已 WARN 记账，不改写输出
    //（出侧改写必须由明确配置授权）。
    let scanner = CredentialScanner::new(true, "warn");
    let input = format!("value={AWS_KEY} trailing text");
    let out = rescan_pair(Some(&scanner), None, "exec", input.clone());
    assert_eq!(out, input, "warn 档不得改写输出");
    assert!(!out.contains("输出已脱敏"), "观测档不得加尾注");
}

#[test]
fn credential_clean_output_passes_through_byte_identical() {
    let scanner = CredentialScanner::new(true, "block");
    let input = "plain tool output without any secrets inside, long enough".to_string();
    let out = rescan_pair(Some(&scanner), None, "web_fetch", input.clone());
    assert_eq!(out, input, "干净输出必须字节级直通（零拷贝语义面）");
    assert!(!out.contains("输出已脱敏"));
}

#[test]
fn dlp_hit_is_redacted_with_honest_note() {
    let engine = DlpEngine::with_config(DlpConfig::default()); // action=block
    let input = format!("customer paid with card {VISA} yesterday");
    let out = rescan_pair(None, Some(&engine), "read_file", input);

    assert!(!out.contains(VISA), "卡号必须消失：{out}");
    assert!(out.contains("[REDACTED]"), "DLP 遮蔽 token 应在场：{out}");
    assert!(
        out.contains("[输出已脱敏: DLP 检测到敏感信息"),
        "尾注必须在场：{out}"
    );
}

#[test]
fn dlp_observe_only_matches_left_intact() {
    // email = Low confidence → low_confidence_action="log" → 全观测档，
    // 不改写（对任意网页内容抓取的邮箱/电话误伤面保持克制）。
    let engine = DlpEngine::with_config(DlpConfig::default());
    let input = "contact alice@example.com for details about the release timeline".to_string();
    let out = rescan_pair(None, Some(&engine), "web_fetch", input.clone());
    assert_eq!(out, input, "全观测档命中不得改写输出");
}

#[test]
fn both_scanners_redact_and_both_notes_appended() {
    // 双样本双路：AWS 键（凭据+DLP 都认）+ 纯 DLP 卡号。凭据先跑并遮蔽
    // AWS 键后，DLP 仍靠卡号独立命中——两路各出一条尾注。
    let scanner = CredentialScanner::new(true, "block");
    let engine = DlpEngine::with_config(DlpConfig::default());
    let input = format!("k={AWS_KEY} card {VISA} end");
    let out = rescan_pair(Some(&scanner), Some(&engine), "exec", input);

    assert!(!out.contains(AWS_KEY) && !out.contains(VISA));
    assert!(out.contains("检测到疑似凭据 1 处"), "凭据尾注缺席：{out}");
    assert!(out.contains("DLP 检测到敏感信息"), "DLP 尾注缺席：{out}");
    // 两条尾注各占一行追加在正文之后。
    let note_pos = out.find("[输出已脱敏").expect("至少一条尾注");
    assert!(
        out[note_pos..].matches("[输出已脱敏").count() >= 2,
        "应有两条独立尾注：{out}"
    );
}

#[test]
fn none_plugin_is_passthrough() {
    let input = format!("value={AWS_KEY}");
    assert_eq!(
        rescan_pair(None, None, "exec", input.clone()),
        input,
        "双 None 直通"
    );
    assert_eq!(
        rescan_tool_output(None, "exec", input.clone()),
        input,
        "入口 None（安全模块未装配）直通"
    );
    assert_eq!(
        rescan_pair(None, None, "exec", String::new()),
        String::new()
    );
}

#[test]
fn security_plugin_end_to_end_rescans_output() {
    // 入口级集成：Default 配置 credential+dlp 全开（audit 链默认关，
    // 构造无副作用）——覆盖 accessors 解包胶水。
    let plugin = SecurityPlugin::new(SecurityPluginConfig {
        credential_enabled: true,
        dlp_enabled: true,
        ..Default::default()
    });
    let input = format!("leaked {AWS_KEY} and card {VISA}");
    let out = rescan_tool_output(Some(&plugin), "read_file", input);

    assert!(!out.contains(AWS_KEY), "入口级：明文键必须被遮蔽：{out}");
    assert!(!out.contains(VISA), "入口级：卡号必须被遮蔽：{out}");
    assert!(out.contains("输出已脱敏"), "入口级：尾注必须在场：{out}");
}
