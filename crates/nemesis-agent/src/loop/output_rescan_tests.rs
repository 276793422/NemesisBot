//! ②：主线工具输出凭据/DLP 复扫测试（凭据/DLP 双路 + 组合 + 旁路 +
//! action 观测档不改写语义 + P1-1 审计链记账 + vault 引用零误伤）。
//!
//! 走 `rescan_pair` 组装核直扫扫描器组合（不必构造整个 SecurityPlugin），
//! 另加一例经 `SecurityPlugin::new(...)` 的入口级集成（覆盖 accessors
//! 解包胶水 + 审计链落账 + 台账零内容纪律）。

use super::output_rescan::{rescan_pair, rescan_tool_output};
use nemesis_security::credential::Scanner as CredentialScanner;
use nemesis_security::dlp::{DlpConfig, DlpEngine};
use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};

/// AWS 示例键：凭据表 `aws_access_key` 与 DLP 表 `aws_access_key`（High）
/// 双双命中，且不含触发 secret_assignment 等附带规则的关键词。
const AWS_KEY: &str = "AKIANCDC6VFQTFGRMBKW";
/// Luhn 合法 visa：DLP `visa`（Medium→block）命中；凭据表无信用卡规则，
/// 是纯 DLP 侧样本。
const VISA: &str = "4111111111111111";

#[test]
fn credential_hit_is_redacted_with_honest_note() {
    let scanner = CredentialScanner::new(true, "block");
    let input = format!("file content value={AWS_KEY} trailing text");
    let (out, hits) = rescan_pair(Some(&scanner), None, "read_file", input);

    assert!(!out.contains(AWS_KEY), "明文键必须消失：{out}");
    assert!(
        out.contains("[REDACTED_CREDENTIAL]"),
        "遮蔽 token 应在场：{out}"
    );
    assert!(
        out.contains("[输出已脱敏: 检测到疑似凭据 1 处"),
        "尾注必须告知模型输出被改写：{out}"
    );
    assert_eq!(hits.len(), 1, "命中摘要必须产出：{hits:?}");
    assert_eq!(hits[0].kind, "tool.output.credential_rescan");
    assert_eq!(hits[0].count, 1);
    assert!(hits[0].redacted, "block 档 = redacted");
    assert_eq!(hits[0].rules, vec!["aws_access_key"]);
}

#[test]
fn credential_warn_action_is_observe_only() {
    // action="warn" = 只观测：scan_tool_output 已 WARN 记账，不改写输出
    //（出侧改写必须由明确配置授权）。P1-1：观测档命中同样产出摘要
    //（redacted=false），供入口层落审计链「observed」档。
    let scanner = CredentialScanner::new(true, "warn");
    let input = format!("value={AWS_KEY} trailing text");
    let (out, hits) = rescan_pair(Some(&scanner), None, "exec", input.clone());
    assert_eq!(out, input, "warn 档不得改写输出");
    assert!(!out.contains("输出已脱敏"), "观测档不得加尾注");
    assert_eq!(hits.len(), 1);
    assert!(!hits[0].redacted, "观测档摘要 redacted=false");
}

#[test]
fn credential_clean_output_passes_through_byte_identical() {
    let scanner = CredentialScanner::new(true, "block");
    let input = "plain tool output without any secrets inside, long enough".to_string();
    let (out, hits) = rescan_pair(Some(&scanner), None, "web_fetch", input.clone());
    assert_eq!(out, input, "干净输出必须字节级直通（零拷贝语义面）");
    assert!(!out.contains("输出已脱敏"));
    assert!(hits.is_empty(), "零命中零摘要（零台账面）");
}

#[test]
fn dlp_hit_is_redacted_with_honest_note() {
    let engine = DlpEngine::with_config(DlpConfig::default()); // action=block
    let input = format!("customer paid with card {VISA} yesterday");
    let (out, hits) = rescan_pair(None, Some(&engine), "read_file", input);

    assert!(!out.contains(VISA), "卡号必须消失：{out}");
    assert!(out.contains("[REDACTED]"), "DLP 遮蔽 token 应在场：{out}");
    assert!(
        out.contains("[输出已脱敏: DLP 检测到敏感信息"),
        "尾注必须在场：{out}"
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, "tool.output.dlp_rescan");
    assert!(hits[0].redacted);
}

#[test]
fn dlp_observe_only_matches_left_intact() {
    // email = Low confidence → low_confidence_action="log" → 全观测档，
    // 不改写（对任意网页内容抓取的邮箱/电话误伤面保持克制）。
    let engine = DlpEngine::with_config(DlpConfig::default());
    let input = "contact alice@example.com for details about the release timeline".to_string();
    let (out, hits) = rescan_pair(None, Some(&engine), "web_fetch", input.clone());
    assert_eq!(out, input, "全观测档命中不得改写输出");
    assert_eq!(hits.len(), 1, "观测档命中仍产出摘要（observed 档记账）");
    assert!(!hits[0].redacted);
}

#[test]
fn both_scanners_redact_and_both_notes_appended() {
    // 双样本双路：AWS 键（凭据+DLP 都认）+ 纯 DLP 卡号。凭据先跑并遮蔽
    // AWS 键后，DLP 仍靠卡号独立命中——两路各出一条尾注。
    let scanner = CredentialScanner::new(true, "block");
    let engine = DlpEngine::with_config(DlpConfig::default());
    let input = format!("k={AWS_KEY} card {VISA} end");
    let (out, hits) = rescan_pair(Some(&scanner), Some(&engine), "exec", input);

    assert!(!out.contains(AWS_KEY) && !out.contains(VISA));
    assert!(out.contains("检测到疑似凭据 1 处"), "凭据尾注缺席：{out}");
    assert!(out.contains("DLP 检测到敏感信息"), "DLP 尾注缺席：{out}");
    // 两条尾注各占一行追加在正文之后。
    let note_pos = out.find("[输出已脱敏").expect("至少一条尾注");
    assert!(
        out[note_pos..].matches("[输出已脱敏").count() >= 2,
        "应有两条独立尾注：{out}"
    );
    // 摘要两路各一条，operation 名可区分（审计链分面过滤用）。
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].kind, "tool.output.credential_rescan");
    assert_eq!(hits[1].kind, "tool.output.dlp_rescan");
}

#[test]
fn none_plugin_is_passthrough() {
    let input = format!("value={AWS_KEY}");
    assert_eq!(
        rescan_pair(None, None, "exec", input.clone()).0,
        input,
        "双 None 直通"
    );
    assert_eq!(
        rescan_tool_output(None, "exec", input.clone()),
        input,
        "入口 None（安全模块未装配）直通"
    );
    assert_eq!(
        rescan_pair(None, None, "exec", String::new()).0,
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

// ===== P1-1：审计链记账 + 台账零内容 + vault 引用零误伤 =====

/// 建带审计链的临时插件（链落 tmpdir jsonl；返回 (插件, 链文件路径)）。
fn plugin_with_audit_chain(credential: bool, dlp: bool) -> (SecurityPlugin, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let chain_path = dir.path().join("audit_chain.jsonl");
    let plugin = SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        credential_enabled: credential,
        dlp_enabled: dlp,
        audit_chain_enabled: true,
        audit_chain_path: Some(chain_path.to_string_lossy().into_owned()),
        ..Default::default()
    });
    // TempDir 句柄随函数返回释放会删目录——泄漏之，测试进程退出即回收。
    std::mem::forget(dir);
    (plugin, chain_path)
}

#[test]
fn credential_hit_writes_audit_chain_record() {
    let (plugin, chain_path) = plugin_with_audit_chain(true, false);
    let input = format!("read result value={AWS_KEY} trailing");
    let out = rescan_tool_output(Some(&plugin), "read_file", input);

    assert!(!out.contains(AWS_KEY), "遮蔽语义零回归");
    let chain = std::fs::read_to_string(&chain_path).expect("审计链文件应存在");
    assert!(
        chain.contains("tool.output.credential_rescan"),
        "台账必须有 operation 记录：{chain}"
    );
    assert!(chain.contains("aws_access_key"), "规则名应入台账：{chain}");
    assert!(chain.contains("redacted"), "处置档位应入台账：{chain}");
    // 零内容纪律：明文键不得进台账（元数据 only）。
    assert!(!chain.contains(AWS_KEY), "台账不得携带命中内容：{chain}");
}

#[test]
fn observe_only_hit_reports_observed_summary_without_tampering() {
    // warn 档 scanner：命中不遮蔽，摘要 observed（redacted=false → 入口层
    // 台账 decision="observed"，与 redacted 同一记账循环）。生产 plugin
    // 构造恒 block（pipeline.rs:260），观测档只能经直构 scanner 面验证；
    // 链面 redacted 记账由 credential_hit_writes_audit_chain_record 钉住。
    let scanner = CredentialScanner::new(true, "warn");
    let input = format!("value={AWS_KEY} kept intact");
    let (out, hits) = rescan_pair(Some(&scanner), None, "exec", input.clone());
    assert_eq!(out, input, "warn 档内容零改动");
    assert_eq!(hits.len(), 1);
    assert!(!hits[0].redacted, "观测档 redacted=false → 台账 observed");
}

#[test]
fn vault_reference_forms_never_hit_and_never_record() {
    // P1-1 验收：`{{vault:…}}`/`vault:alias` 引用形态是配置合法写法，
    // 豁免经扫描器 exemptions 表 VaultRescan（strip_reference_forms 同源）
    // 继承——零命中、零尾注、零台账。本测试钉继承不回退。
    let (plugin, chain_path) = plugin_with_audit_chain(true, false);
    let input = r#"config sample:
api_key = "{{vault:agent/s1/openai-key}}"
password = "vault:zope/main"
db_pass = {{vault:main/db}}
"#;
    let (out, hits) = rescan_pair(
        plugin.credential_scanner(),
        None,
        "read_file",
        input.to_string(),
    );
    assert_eq!(out, input, "引用形态输出必须字节级直通（零误伤）：{out}");
    assert!(hits.is_empty(), "引用形态不得产出命中摘要：{hits:?}");
    let chain = std::fs::read_to_string(&chain_path).unwrap_or_default();
    assert!(
        !chain.contains("tool.output.credential_rescan"),
        "引用形态不得落台账：{chain}"
    );
}

#[test]
fn clean_output_writes_no_audit_record() {
    let (plugin, chain_path) = plugin_with_audit_chain(true, true);
    let input = "ordinary tool output with no secrets at all, just prose".to_string();
    let out = rescan_tool_output(Some(&plugin), "web_fetch", input);
    assert!(!out.contains("输出已脱敏"));
    let chain = std::fs::read_to_string(&chain_path).unwrap_or_default();
    assert!(!chain.contains("tool.output."), "干净输出零台账：{chain}");
}
