//! B4 误拦修复（2026-11-10）管线级回归：跨 Layer 4 凭据扫描 + Layer 5
//! DLP + 会话占位符展开 + 结果掩码的协同链。
//!
//! 钦定时序（tool_dispatch.rs）：安全 8 层先跑（各层看占位符**原文**，
//! 匹配级模板豁免让完整合法引用不被误拦）→ dispatch 最内层展开 →
//! execute（模拟工具收真值）→ 结果掩码。本文件按此时序逐环断言；
//! 负面臂验证明文同形态照旧拦（豁免不弱化明文检测）。
//!
//! 需 security feature（nemesis-security 是 optional 依赖）。

use super::session_secrets::{expand_session_placeholders, mask_secrets_in_result};
use nemesis_config::{clear_global_vault_resolver, set_global_vault_resolver};
use nemesis_security::credential::Scanner as CredentialScanner;
use nemesis_security::dlp::DlpEngine;
use parking_lot::Mutex;
use std::sync::Arc;

/// 全局解析器是进程单例——触及它的测试互斥（同 session_secrets_tests 惯例）。
static RESOLVER_LOCK: Mutex<()> = Mutex::new(());

struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        clear_global_vault_resolver();
    }
}

/// 一段真实形态的 exec 脚本 args（含赋值结构的完整会话凭据引用）。
/// 脚本内用**单引号**（JSON 双引号需转义成 `\"`，反斜杠会卡住
/// `['"]` 锚——那形态两层本来就匹配不上，不构成误拦面）。修复前：
/// secret_assignment（Layer 4）与 secret_password_assignment
///（Layer 5 DLP Medium→block）双双误命中，工具被拦。
const REF_ARGS: &str = r#"{"command":"login.sh","env":"password = '{{vault:agent/sess-9/dbpw}}'"}"#;

/// 同形态明文版（负面臂：豁免不得弱化明文检测）。
const PLAIN_ARGS: &str = r#"{"command":"login.sh","env":"password = 'hunter2secret'"}"#;

/// 管线级：Layer 4 扫描引用原文不拦 → 同一原文展开收真值 → 结果掩码。
/// 三环共享同一 raw args 字符串（dispatch 时序的真实形态）。
#[test]
fn pipeline_reference_passes_layer4_then_expands_and_masks() {
    let _lock = RESOLVER_LOCK.lock();
    let _cleanup = Cleanup;

    // 环① Layer 4：占位符原文过凭据扫描 → 豁免（不拦）。
    let scanner = CredentialScanner::new(true, "block");
    let l4 = scanner.scan_content(REF_ARGS);
    assert!(!l4.has_matches, "Layer 4 误拦引用: {:?}", l4.matches);

    // 环② Layer 5：DLP 同一原文 → 不 block。
    let dlp = DlpEngine::new(true, "block");
    let l5 = dlp.scan_text(REF_ARGS);
    assert!(!l5.should_block, "Layer 5 误拦引用: {:?}", l5.matches);

    // 环③ dispatch 最内层展开：同一原文 → 模拟工具收真值。
    set_global_vault_resolver(Arc::new(|alias| {
        assert_eq!(alias, "agent/sess-9/dbpw");
        Ok("pg-real-password-9f8e".to_string())
    }));
    let (expanded, secrets) =
        expand_session_placeholders(REF_ARGS, "sess-9").expect("in-scope reference expands");
    assert!(
        expanded.contains("pg-real-password-9f8e"),
        "工具应收到真值: {expanded}"
    );
    assert!(!expanded.contains("{{vault:"), "模板应全部替换: {expanded}");
    assert_eq!(secrets, vec!["pg-real-password-9f8e".to_string()]);

    // 环④ 结果掩码：真值不出现在工具结果里。
    let tool_output = "login ok with password pg-real-password-9f8e, session started";
    let masked = mask_secrets_in_result(tool_output, &secrets);
    assert!(
        !masked.contains("pg-real-password-9f8e"),
        "真值必须被掩码: {masked}"
    );
    assert!(masked.contains("[凭据已掩码]"), "掩码占位应在场: {masked}");
}

/// 负面臂：明文同形态 args → Layer 4 与 Layer 5 双双照旧拦
///（安全层拒绝时工具不执行，展开层不会被触达——断言分层职责）。
#[test]
fn pipeline_plaintext_still_blocked_at_both_layers() {
    let scanner = CredentialScanner::new(true, "block");
    let l4 = scanner.scan_content(PLAIN_ARGS);
    assert!(l4.has_matches, "Layer 4 必须照拦明文: {:?}", l4.matches);

    let dlp = DlpEngine::new(true, "block");
    let l5 = dlp.scan_text(PLAIN_ARGS);
    assert!(l5.should_block, "Layer 5 必须照拦明文: {:?}", l5.matches);
}

/// 展开层失败路径：越界别名在安全层放行后由展开层 fail loud
///（安全层不查库不越权裁决，作用域闸归展开层——分层职责断言）。
#[test]
fn pipeline_out_of_scope_fails_loud_after_security_pass() {
    let scanner = CredentialScanner::new(true, "block");
    let args = r#"{"env":"password = '{{vault:other-node/token}}'"}"#;
    let l4 = scanner.scan_content(args);
    assert!(!l4.has_matches, "引用形态应过 Layer 4: {:?}", l4.matches);

    let err = expand_session_placeholders(args, "sess-9").expect_err("越界别名必须 fail loud");
    assert!(err.contains("越界"), "报错应含越界语义: {err}");
}

// ===== 裸引用形态（2026-11-10 裸形态批，用户拍板选 1）：裸
// `vault:<alias>` 是配置文件合法写法（write_file/edit_file 落盘场景，
// 非 exec 脚本）——两层安全放行后展开层**不消费**裸形态（裸引用归
// credential_injection 声明槽改写），args 原样透传。 =====

/// 一段真实形态的 write_file args：TOML 配置写裸引用（生产误拦实锤面）。
/// 修复前：带引号裸引用命中 Layer 4 secret_assignment；无引号形态
///（TOML 亦可裸值）命中 Layer 5 secret_password_assignment。
const BARE_CONFIG_ARGS: &str = r#"{"path":"config/app.toml","content":"[db]\npassword = \"vault:zope/main\"\ntimeout = 30\n"}"#;

/// 无引号形态（YAML/裸值配置行，Layer 5 独有误拦面）。
const BARE_CONFIG_ARGS_UNQUOTED: &str =
    r#"{"path":"config/app.yaml","content":"db:\n  password: vault:zope/main\n"}"#;

/// 裸引用写配置文件：Layer 4/Layer 5 双放行（引用不是明文秘密），且
/// 展开层不动它（无 `{{vault:` → expand 零改动原样透传）。
#[test]
fn pipeline_bare_reference_passes_and_flows_untouched() {
    let scanner = CredentialScanner::new(true, "block");
    let dlp = DlpEngine::new(true, "block");

    for (label, args) in [
        ("quoted TOML", BARE_CONFIG_ARGS),
        ("unquoted YAML", BARE_CONFIG_ARGS_UNQUOTED),
    ] {
        let l4 = scanner.scan_content(args);
        assert!(
            !l4.has_matches,
            "Layer 4 误拦裸引用（{label}）: {:?}",
            l4.matches
        );
        let l5 = dlp.scan_text(args);
        assert!(
            !l5.should_block,
            "Layer 5 误拦裸引用（{label}）: {:?}",
            l5.matches
        );
        // 展开层对无模板形态的 args 零改动（裸引用的值装配归声明槽改写，
        // 见 credential_injection——本断言钉「展开层不碰裸形态」的边界）。
        let (expanded, secrets) =
            expand_session_placeholders(args, "sess-9").expect("no template = no expansion");
        assert_eq!(expanded, args, "裸形态必须原样透传（{label}）");
        assert!(secrets.is_empty(), "无模板不应收明文（{label}）");
    }
}
