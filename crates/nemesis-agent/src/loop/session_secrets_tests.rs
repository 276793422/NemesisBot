//! B4（2026-11-08 批次④）会话凭据占位符测试。
//!
//! 覆盖：展开正常路径（顶层字符串槽 / 对象槽叶）/ 多处嵌入 / 作用域闸
//! 越界**不查库** / 别名不存在诚实报错 / 无模板直通（字节级透传）/
//! 非 JSON 透传 / 键名不改写 / 无闭合字面量保留 / 空别名拒绝 / 结果掩码
//!（长度降序 / 空清单直通）/ 前缀单一真相源格式。
//! 槽位收紧（2026-11-11）：槽外完整模板 fail loud（placement 优先于
//! scope）/ 无闭合片段槽外不报错 / 槽键下深嵌套与数组属检测面 /
//! 无声明槽工具任何模板都拒。

use super::credential_injection_tests::VAULT_SLOT_LOCK;
use super::session_secrets::{
    MASK_TEXT, expand_session_placeholders_in_slots, mask_secrets_in_result, received_text,
    session_scope_prefix,
};
use nemesis_config::{clear_global_vault_resolver, set_global_vault_resolver};
use std::sync::Arc;

// 全局解析器是**进程单例**——触及它的测试统一互斥在
// `credential_injection_tests::VAULT_SLOT_LOCK`（BUG-20261110-049）：并行
// 测试会互相踩安装（实录：resolver 断言收到别人测试的别名），文件级
// static 锁对其他文件的持锁者互不互斥 = 假互斥。

/// 测试结束清理全局槽位（panic 路径也要恢复——全局状态 hygiene）。
struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        clear_global_vault_resolver();
    }
}

/// 展开路径的 JSON 断言辅助：两侧按 Value 语义比对（serde Map 键序归一
/// 是序列化伪影，不该钉字节序）。
fn assert_json_eq(got: &str, want: &str) {
    let g: serde_json::Value =
        serde_json::from_str(got).unwrap_or_else(|e| panic!("got 不是合法 JSON: {e}: {got}"));
    let w: serde_json::Value =
        serde_json::from_str(want).unwrap_or_else(|e| panic!("want 不是合法 JSON: {e}: {want}"));
    assert_eq!(g, w);
}

#[test]
fn passthrough_non_json_args() {
    let raw = "not json at all {";
    let (out, secrets) =
        expand_session_placeholders_in_slots(raw, "sess-1", &["env"]).expect("passthrough");
    assert_eq!(out, raw);
    assert!(secrets.is_empty());
}

/// 无模板头 → 快速路径字节级原样返回（不经 serde round-trip，键序/浮点
/// 形态等序列化伪影全避开——工具侧无感）。
#[test]
fn passthrough_no_template_byte_identical() {
    let raw = r#"{"url":"https://example.com","n":3,"nested":{"k":"v"},"f":1.50}"#;
    let (out, secrets) =
        expand_session_placeholders_in_slots(raw, "sess-1", &["env"]).expect("passthrough");
    assert_eq!(out, raw);
    assert!(secrets.is_empty());
}

/// 顶层字符串槽（web_fetch headers 同形态）：槽面内展开。
#[test]
fn expand_basic_within_scope_top_level_string_slot() {
    let _g = VAULT_SLOT_LOCK.lock();
    let _cleanup = Cleanup;
    set_global_vault_resolver(Arc::new(|alias| {
        assert_eq!(alias, "agent/sess-1/token");
        Ok("sk-real-secret-value".to_string())
    }));
    let raw = r#"{"url":"https://api.example.com","token":"{{vault:agent/sess-1/token}}"}"#;
    let (out, secrets) =
        expand_session_placeholders_in_slots(raw, "sess-1", &["token"]).expect("in-scope expands");
    assert_json_eq(
        &out,
        r#"{"url":"https://api.example.com","token":"sk-real-secret-value"}"#,
    );
    assert_eq!(secrets, vec!["sk-real-secret-value".to_string()]);
}

/// 对象槽叶子（exec 族 env 同形态）：深度恒 1 的字符串叶展开。
#[test]
fn expand_embedded_and_multiple() {
    let _g = VAULT_SLOT_LOCK.lock();
    let _cleanup = Cleanup;
    set_global_vault_resolver(Arc::new(|alias| match alias {
        "agent/sess-1/a" => Ok("AAA".to_string()),
        "agent/sess-1/b" => Ok("BBB".to_string()),
        _ => Err("unexpected".to_string()),
    }));
    // 同串多处 + 跨字段 + 嵌入更长文本（都在 env 对象槽叶内）。
    let raw = r#"{"env":{"cmd":"login --user u --pass {{vault:agent/sess-1/a}} --extra {{vault:agent/sess-1/b}}","note":"prefix {{vault:agent/sess-1/a}} suffix"}}"#;
    let (out, secrets) =
        expand_session_placeholders_in_slots(raw, "sess-1", &["env"]).expect("expands");
    assert_json_eq(
        &out,
        r#"{"env":{"cmd":"login --user u --pass AAA --extra BBB","note":"prefix AAA suffix"}}"#,
    );
    // 同一明文出现两次也收集两次（掩码按值幂等，重复无害）。
    assert_eq!(secrets.len(), 3);
}

#[test]
fn object_keys_not_rewritten() {
    let _g = VAULT_SLOT_LOCK.lock();
    let _cleanup = Cleanup;
    set_global_vault_resolver(Arc::new(|_alias| Ok("X".to_string())));
    // 占位符语义是「值里引用凭据」：键名不改写（宁缺毋滥）——键上的模板
    // 形态不触发检测也不展开，值原样、无收集（resolver 未触达也能证明
    // 改写没发生在键上）。
    let raw = r#"{"{{vault:agent/sess-1/k}}":"v"}"#;
    let (out, secrets) =
        expand_session_placeholders_in_slots(raw, "sess-1", &["env"]).expect("keys untouched");
    assert_json_eq(&out, raw);
    assert!(secrets.is_empty());
}

#[test]
fn out_of_scope_alias_rejected_without_lookup() {
    let _g = VAULT_SLOT_LOCK.lock();
    let _cleanup = Cleanup;
    // resolver 一旦被调用即 panic（测试失败）——证明越界臂不查库、
    // 不给别名存在性信号（B4 主防线）。
    set_global_vault_resolver(Arc::new(|_alias| {
        panic!("out-of-scope alias must not reach the resolver");
    }));
    let raw = r#"{"env":{"k":"{{vault:global/other/token}}"}}"#;
    let err = expand_session_placeholders_in_slots(raw, "sess-1", &["env"])
        .expect_err("out of scope rejects");
    assert!(
        err.contains("越界"),
        "error should name the violation: {err}"
    );
    assert!(
        err.contains("global/other/token"),
        "alias is model-authored, not secret: {err}"
    );
}

#[test]
fn empty_alias_rejected() {
    let raw = r#"{"env":{"k":"{{vault:}}"}}"#;
    let err = expand_session_placeholders_in_slots(raw, "sess-1", &["env"])
        .expect_err("empty alias rejects");
    assert!(err.contains("别名为空"), "{err}");
}

#[test]
fn unclosed_template_kept_literal() {
    let _g = VAULT_SLOT_LOCK.lock();
    let _cleanup = Cleanup;
    // 无闭合 = 非合法模板形态，按字面量保留（同串后续合法模板照常展开）。
    set_global_vault_resolver(Arc::new(|alias| {
        assert_eq!(alias, "agent/sess-1/ok");
        Ok("V".to_string())
    }));
    let raw =
        r#"{"env":{"a":"head {{vault:agent/sess-1/ok no tail","b":"{{vault:agent/sess-1/ok}}"}}"#;
    let (out, secrets) =
        expand_session_placeholders_in_slots(raw, "sess-1", &["env"]).expect("expands");
    assert_json_eq(
        &out,
        r#"{"env":{"a":"head {{vault:agent/sess-1/ok no tail","b":"V"}}"#,
    );
    assert_eq!(secrets, vec!["V".to_string()]);
}

#[test]
fn unknown_alias_error_propagates() {
    let _g = VAULT_SLOT_LOCK.lock();
    let _cleanup = Cleanup;
    set_global_vault_resolver(Arc::new(|_alias| Err("别名不存在".to_string())));
    let raw = r#"{"env":{"k":"{{vault:agent/sess-1/missing}}"}}"#;
    let err =
        expand_session_placeholders_in_slots(raw, "sess-1", &["env"]).expect_err("unknown alias");
    assert!(err.contains("解析失败"), "{err}");
    assert!(
        err.contains("别名不存在"),
        "vault layer error text passes through: {err}"
    );
}

#[test]
fn empty_secret_expands_but_not_collected() {
    let _g = VAULT_SLOT_LOCK.lock();
    let _cleanup = Cleanup;
    set_global_vault_resolver(Arc::new(|_alias| Ok(String::new())));
    let raw = r#"{"env":{"k":"{{vault:agent/sess-1/empty}}"}}"#;
    let (out, secrets) =
        expand_session_placeholders_in_slots(raw, "sess-1", &["env"]).expect("expands");
    assert_json_eq(&out, r#"{"env":{"k":""}}"#);
    // 空值不收集（掩码空串替换无意义）。
    assert!(secrets.is_empty());
}

#[test]
fn alias_trim_insensitive() {
    let _g = VAULT_SLOT_LOCK.lock();
    let _cleanup = Cleanup;
    // 模板内两侧空白容忍（模型书写噪声）；闸面按 trim 后比对。
    set_global_vault_resolver(Arc::new(|alias| {
        assert_eq!(alias, "agent/sess-1/t");
        Ok("T".to_string())
    }));
    let raw = r#"{"env":{"k":"{{vault: agent/sess-1/t }}"}}"#;
    let (out, _) = expand_session_placeholders_in_slots(raw, "sess-1", &["env"]).expect("expands");
    assert_json_eq(&out, r#"{"env":{"k":"T"}}"#);
}

// ===== 槽位收紧（2026-11-11，用户拍板）：槽外完整模板 fail loud。 =====

/// 槽外（command 普通参数）完整模板 → placement Err，工具不执行、
/// resolver 不触达（检测先于解析——无查库无存在性信号）。
#[test]
fn template_outside_slot_fails_loud() {
    let _g = VAULT_SLOT_LOCK.lock();
    let _cleanup = Cleanup;
    set_global_vault_resolver(Arc::new(|_alias| {
        panic!("outside-slot template must not reach the resolver");
    }));
    let raw =
        r#"{"command":"curl -H \"Authorization: Bearer {{vault:agent/sess-1/t}}\"","env":{}}"#;
    let err = expand_session_placeholders_in_slots(raw, "sess-1", &["env"])
        .expect_err("outside-slot template rejects");
    assert!(err.contains("OUTSIDE SLOT"), "{err}");
    assert!(err.contains("command"), "field path should be named: {err}");
    assert!(
        err.contains("agent/sess-1/t"),
        "alias is model-authored, not secret: {err}"
    );
    assert!(err.contains("env"), "declared slots should be named: {err}");
    // 真"值不进命令行"不变量的自证：报错原文里不含执行后的真值（此处
    // resolver 根本没跑，无真值可言）。
    assert!(!err.contains("sk-real"), "{err}");
}

/// placement 优先于 scope：槽外越界别名报 placement 错（不是越界错）。
#[test]
fn placement_precedence_over_scope() {
    let _g = VAULT_SLOT_LOCK.lock();
    let _cleanup = Cleanup;
    set_global_vault_resolver(Arc::new(|_alias| {
        panic!("must not reach the resolver");
    }));
    let raw = r#"{"command":"{{vault:wrong-node/x}}","env":{}}"#;
    let err = expand_session_placeholders_in_slots(raw, "sess-1", &["env"])
        .expect_err("outside-slot template rejects");
    assert!(err.contains("OUTSIDE SLOT"), "placement wins: {err}");
    assert!(!err.contains("越界"), "not a scope error: {err}");
}

/// 槽外无闭合片段 = 字面量保留不报错（文档/示例文本误伤零化）。
#[test]
fn unclosed_fragment_outside_slot_is_literal() {
    let raw = r#"{"command":"docs: {{vault:agent/sess-1/x is an example","env":{}}"#;
    let (out, secrets) =
        expand_session_placeholders_in_slots(raw, "sess-1", &["env"]).expect("literal kept");
    assert_json_eq(&out, raw);
    assert!(secrets.is_empty());
}

/// 槽键下深度 >1 的嵌套属检测面（宁缺毋滥，与声明槽改写同形契约）。
#[test]
fn deep_nested_under_slot_detected() {
    let raw = r#"{"env":{"a":{"b":"{{vault:agent/sess-1/x}}"}}}"#;
    let err = expand_session_placeholders_in_slots(raw, "sess-1", &["env"])
        .expect_err("depth>1 under slot is outside surface");
    assert!(err.contains("OUTSIDE SLOT"), "{err}");
    assert!(err.contains("env.a.b"), "nested field path: {err}");
}

/// 槽键下的数组同样属检测面（对象槽只有深度恒 1 的字符串叶是槽面）。
#[test]
fn array_under_slot_detected() {
    let raw = r#"{"env":{"a":["{{vault:agent/sess-1/x}}"]}}"#;
    let err = expand_session_placeholders_in_slots(raw, "sess-1", &["env"])
        .expect_err("array under slot is outside surface");
    assert!(err.contains("OUTSIDE SLOT"), "{err}");
    assert!(err.contains("env.a.0"), "array index path: {err}");
}

/// 无声明槽位工具：任何模板都拒（报错点名「无声明凭据槽位」）。
#[test]
fn no_declared_slots_rejects_any_template() {
    let raw = r#"{"k":"{{vault:agent/sess-1/x}}"}"#;
    let err = expand_session_placeholders_in_slots(raw, "sess-1", &[])
        .expect_err("no slots = no placeholder surface");
    assert!(err.contains("OUTSIDE SLOT"), "{err}");
    assert!(err.contains("无声明凭据槽位"), "{err}");
}

#[test]
fn mask_replaces_all_occurrences() {
    let out = mask_secrets_in_result(
        "token=abc123 in header abc123 and abc123again",
        &["abc123".to_string()],
    );
    assert_eq!(
        out,
        format!("token={MASK_TEXT} in header {MASK_TEXT} and {MASK_TEXT}again")
    );
}

#[test]
fn mask_longest_first() {
    // 长值包含短值：必须长值先替换，否则短值先替换会撕裂长值（前缀泄漏）。
    let out = mask_secrets_in_result(
        "payload: abcdef standalone abc",
        &["abc".to_string(), "abcdef".to_string()],
    );
    assert_eq!(out, format!("payload: {MASK_TEXT} standalone {MASK_TEXT}"));
}

#[test]
fn mask_empty_list_passthrough() {
    let out = mask_secrets_in_result("untouched", &[]);
    assert_eq!(out, "untouched");
}

#[test]
fn scope_prefix_single_source_format() {
    // 收取侧（question secret 写入）与使用侧（expand 闸）共用本函数——
    // 这里钉住格式本身：agent/<sanitize(session_key)>/。
    let prefix = session_scope_prefix("sess-1");
    assert!(prefix.starts_with("agent/"));
    assert!(
        prefix.ends_with("/sess-1/") || prefix.contains("/sess-1/"),
        "{prefix}"
    );
    // 特殊字符会话键经 sanitize_path_segment 归一（与前缀生成同源）。
    let weird = session_scope_prefix("a/b\\c:d");
    assert!(!weird.contains('\\'), "sanitized: {weird}");
    assert!(!weird.contains(':'), "sanitized: {weird}");
}

#[test]
fn received_text_format() {
    assert_eq!(received_text("api_key"), "[凭据已接收: api_key]");
}
