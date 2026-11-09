//! vault_ref 测试：前缀路由、解析器槽位生命周期、resolve_secret_field 前缀链。

use super::*;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 全局解析器槽位是进程单例——触及它的测试互斥（并行测试会互相踩安装）。
static GLOBAL_LOCK: Mutex<()> = Mutex::new(());

/// 测试结束清理全局槽位（全局状态 hygiene）。
struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        clear_global_vault_resolver();
    }
}

/// 非 vault 值返回 None（前缀不相交，路由不劫持其它链路）。
#[test]
fn non_vault_value_returns_none() {
    let _g = GLOBAL_LOCK.lock();
    let _c = Cleanup;
    assert!(resolve_vault_reference("sk-literal").is_none());
    assert!(resolve_vault_reference("env:MY_VAR").is_none());
    assert!(resolve_vault_reference("yaml:alias").is_none());
    assert!(resolve_vault_reference("").is_none());
    // 只有恰好等于前缀本身也算空别名（Err 路径，不是 None）。
    assert!(resolve_vault_reference("vault:").unwrap().is_err());
}

/// 解析器未安装：诚实报错（不是静默降级为空值/字面量）。
#[test]
fn resolver_unset_errors_loud() {
    let _g = GLOBAL_LOCK.lock();
    let _c = Cleanup;
    clear_global_vault_resolver();
    let err = resolve_vault_reference("vault:any")
        .expect("vault: 前缀必须进入本函数")
        .unwrap_err();
    assert!(err.contains("未安装"), "got: {err}");
}

/// 安装后解析生效；闭包收到的就是别名（前缀已剥）。
#[test]
fn resolver_receives_alias_and_resolves() {
    let _g = GLOBAL_LOCK.lock();
    let _c = Cleanup;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    set_global_vault_resolver(Arc::new(move |alias| {
        counter.fetch_add(1, Ordering::SeqCst);
        if alias == "good" {
            Ok("secret-value".into())
        } else {
            Err(format!("别名不存在: {alias}"))
        }
    }));
    let got = resolve_vault_reference("vault:good").unwrap().unwrap();
    assert_eq!(got, "secret-value");
    let err = resolve_vault_reference("vault:bad").unwrap().unwrap_err();
    assert!(err.contains("不存在"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// resolve_secret_field：env: 分支三态。
#[test]
fn secret_field_env_branch() {
    let _g = GLOBAL_LOCK.lock();
    let _c = Cleanup;
    // edition 2024：env 变动是 unsafe（credentials/tests.rs 同款）。
    unsafe { std::env::set_var("NEMESISBOT_TEST_VAULT_VAR", "env-value") };
    assert_eq!(
        resolve_secret_field("env:NEMESISBOT_TEST_VAULT_VAR", "test.field").unwrap(),
        "env-value"
    );
    unsafe { std::env::set_var("NEMESISBOT_TEST_VAULT_EMPTY", "") };
    assert!(resolve_secret_field("env:NEMESISBOT_TEST_VAULT_EMPTY", "test.field").is_err());
    assert!(resolve_secret_field("env:NEMESISBOT_TEST_VAULT_MISSING", "test.field").is_err());
    assert!(resolve_secret_field("env:", "test.field").is_err());
    unsafe {
        std::env::remove_var("NEMESISBOT_TEST_VAULT_VAR");
        std::env::remove_var("NEMESISBOT_TEST_VAULT_EMPTY");
    }
}

/// resolve_secret_field：vault: 委托全局解析器，字面量原样通过。
#[test]
fn secret_field_vault_and_literal() {
    let _g = GLOBAL_LOCK.lock();
    let _c = Cleanup;
    set_global_vault_resolver(Arc::new(|alias| {
        if alias == "k" {
            Ok("v".into())
        } else {
            Err("nope".into())
        }
    }));
    assert_eq!(resolve_secret_field("vault:k", "test.field").unwrap(), "v");
    let err = resolve_secret_field("vault:missing", "test.field")
        .unwrap_err()
        .to_string();
    assert!(err.contains("test.field"), "错误应带字段上下文: {err}");
    assert_eq!(
        resolve_secret_field("sk-literal-123", "test.field").unwrap(),
        "sk-literal-123"
    );
}

#[test]
fn is_secret_reference_prefix_dispatch() {
    assert!(super::is_secret_reference("env:HOME"));
    assert!(super::is_secret_reference("yaml:creds"));
    assert!(super::is_secret_reference("vault:openai-main"));
    assert!(!super::is_secret_reference("sk-literal"));
    assert!(!super::is_secret_reference("Bearer vault:x")); // 内嵌不算整值引用
    assert!(!super::is_secret_reference(""));
}

// ===========================================================================
// B4（2026-11-08 批次④）全局写入器槽位：question 凭据形态的收取通道。
// 与 resolver 槽位同模式（None = 诚实报错；后装覆盖先装）。
// ===========================================================================

/// 写入器未安装：write_vault_secret 诚实报错（带 CLI 预存指引），不是
/// 静默成功。
#[test]
fn writer_unset_errors_loud() {
    let _g = GLOBAL_LOCK.lock();
    let _c = Cleanup;
    clear_global_vault_writer();
    let err = write_vault_secret("agent/s1/key", "v").unwrap_err();
    assert!(err.contains("未安装"), "got: {err}");
    assert!(err.contains("vault set"), "应带 CLI 预存指引: {err}");
}

/// 安装后写入生效：闭包收到裸别名与原值；别名空值照样透传（空别名/
/// 空值校验归 vault 层，本槽只做通道）。
#[test]
fn writer_receives_alias_and_value() {
    let _g = GLOBAL_LOCK.lock();
    let _c = Cleanup;
    clear_global_vault_writer(); // 前测 panic 泄漏防御
    let got = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let sink = got.clone();
    set_global_vault_writer(Arc::new(move |alias, value| {
        sink.lock().push((alias.to_string(), value.to_string()));
        Ok(())
    }));
    write_vault_secret("agent/s1/api_key", "sk-test").unwrap();
    write_vault_secret("empty-alias-ok", "").unwrap();
    clear_global_vault_writer();
    assert_eq!(
        got.lock().clone(),
        vec![
            ("agent/s1/api_key".to_string(), "sk-test".to_string()),
            ("empty-alias-ok".to_string(), String::new()),
        ]
    );
}

/// 写入失败透传：vault 层错误原样上抛（fail loud，不包糖衣）。
#[test]
fn writer_error_propagates() {
    let _g = GLOBAL_LOCK.lock();
    let _c = Cleanup;
    clear_global_vault_writer(); // 前测 panic 泄漏防御
    set_global_vault_writer(Arc::new(|_alias, _value| Err("vault 已锁定".to_string())));
    let err = write_vault_secret("a", "v").unwrap_err();
    clear_global_vault_writer();
    assert!(err.contains("vault 已锁定"), "got: {err}");
}

/// 清除后回到 None 臂（槽位生命周期对称）。
#[test]
fn writer_clear_restores_none() {
    let _g = GLOBAL_LOCK.lock();
    let _c = Cleanup;
    set_global_vault_writer(Arc::new(|_, _| Ok(())));
    clear_global_vault_writer();
    assert!(global_vault_writer().is_none());
    assert!(write_vault_secret("x", "y").is_err());
}

// ===== strip_templates（2026-11-10 B4 会话凭据误拦修复：安全层模板豁免
// 的单一语法判定点）。无全局槽位接触，无锁需求。 =====

/// 完整模板剔除；模板前后文本保留。
#[test]
fn strip_templates_removes_complete_and_keeps_surroundings() {
    assert_eq!(
        strip_templates("password = \"{{vault:agent/s1/pw}}\""),
        "password = \"\""
    );
    assert_eq!(strip_templates("a{{vault:x/y}}b{{vault:z}}c"), "abc");
}

/// 无模板 → 原样返回（零成本快速路径）。
#[test]
fn strip_templates_passthrough_without_head() {
    assert_eq!(
        strip_templates("plain password = \"secret\""),
        "plain password = \"secret\""
    );
    assert_eq!(strip_templates(""), "");
}

/// 未闭合（`{{vault:x` 无 `}}`）→ HEAD 字面量保留，不豁免该段。
#[test]
fn strip_templates_keeps_unclosed_literal() {
    assert_eq!(strip_templates("{{vault:x"), "{{vault:x");
    assert_eq!(
        strip_templates("a {{vault:x}} b {{vault:unclosed"),
        "a  b {{vault:unclosed"
    );
}

/// 空别名（trim 后空）→ 非法模板形态，原文保留。
#[test]
fn strip_templates_keeps_empty_alias() {
    assert_eq!(strip_templates("{{vault:}}"), "{{vault:}}");
    assert_eq!(strip_templates("{{vault:   }}"), "{{vault:   }}");
}

/// 嵌套畸形（内层提前闭合）→ 按最近 `}}` 判定该段为完整模板剔除，
/// 余下尾随 `}}` 保留（该形态展开层越界报错 fail loud，安全层放行的
/// 是无真值字面文本）。
#[test]
fn strip_templates_nested_form_strips_innermost_close() {
    assert_eq!(strip_templates("{{vault:{{vault:x}}}}"), "}}");
}

/// 空串输入直通。
#[test]
fn strip_templates_empty_input() {
    assert_eq!(strip_templates(""), "");
}

// ===== strip_reference_forms（2026-11-10 裸形态批：模板 + 裸引用 `vault:`
// `<alias>` 双形态豁免的单一语法判定点。语义/防回退说明见函数主文档。）=====

/// 裸引用剔除：带引号与无引号形态都剥干净，前后文保留。
#[test]
fn strip_reference_forms_removes_bare_ref() {
    assert_eq!(
        strip_reference_forms("password = \"vault:zope/main\""),
        "password = \"\""
    );
    assert_eq!(
        strip_reference_forms("password = vault:zope/main"),
        "password = "
    );
    // 多级别名（含路径段）整个别名段一起剥。
    assert_eq!(
        strip_reference_forms("api_key: vault:openai-main"),
        "api_key: "
    );
}

/// 模板与裸引用组合：两条路径都剥（模板先剥，残余裸引用再剥）。
#[test]
fn strip_reference_forms_combined_forms() {
    assert_eq!(
        strip_reference_forms("a={{vault:x/y}} b=vault:z c=plain"),
        "a= b= c=plain"
    );
}

/// `vault:` 后无别名字符（空值/`{{vault:}}` 闭合位）→ 非引用形态原文保留。
#[test]
fn strip_reference_forms_keeps_empty_alias() {
    assert_eq!(
        strip_reference_forms("password = \"vault:\""),
        "password = \"vault:\""
    );
    assert_eq!(
        strip_reference_forms("password = \"{{vault:}}\""),
        "password = \"{{vault:}}\""
    );
}

/// 独立词元闸：`vault:` 前是别名字符 = 普通单词一部分（如 `myvault:`），
/// 不剥——真秘密恰好内嵌 `vault:` 子串时残余仍满足规则照旧拦。
#[test]
fn strip_reference_forms_requires_standalone_token() {
    assert_eq!(
        strip_reference_forms("password = \"myvault:secret123456\""),
        "password = \"myvault:secret123456\""
    );
    // 前置是标点/空白/行首 = 独立词元，正常剥。
    assert_eq!(strip_reference_forms("vault:x/y"), "");
    assert_eq!(strip_reference_forms("foo(\"vault:alias1\")"), "foo(\"\")");
}

/// 别名 charset 边界：别名段遇非别名字符断段，断点后的残余参与重扫
///（`vault:abc$%^12345` 剥 `vault:abc` 后剩 `$%^12345`）——正是凭据扫描
///「剥离后残余仍命中照旧拦」防线的输入形态。
#[test]
fn strip_reference_forms_alias_stops_at_non_alias_char() {
    assert_eq!(
        strip_reference_forms("password = \"vault:abc$%^12345\""),
        "password = \"$%^12345\""
    );
    assert_eq!(strip_reference_forms("vault:x,y"), ",y");
}

/// CJK 别名（会话名经 sanitize_path_segment 可含 CJK）整段剥。
#[test]
fn strip_reference_forms_cjk_alias() {
    assert_eq!(
        strip_reference_forms("password = \"vault:agent/会话1/数据库密码\""),
        "password = \"\""
    );
}

/// 剥离残余保留：`vault:x` 前后的空格/文本原样（消费方重扫语义依赖此形态）。
#[test]
fn strip_reference_forms_preserves_gaps() {
    assert_eq!(
        strip_reference_forms("prefix vault:x suffix"),
        "prefix  suffix"
    );
}
