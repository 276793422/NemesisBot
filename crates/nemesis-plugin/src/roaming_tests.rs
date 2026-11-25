//! roaming 契约宿主侧回归锁（BUG-20261125-023）。
//!
//! `identity_set` 的 `secret_key_hex` 来自 WSAPI `node.update_identity` 用户
//! 输入，曾用裸 `format!` 拼 JSON——含 `"`/`\` 的输入破坏载荷（同文件
//! trust_trust/trust_revoke/trust_unrevoke 三处都走 `json_arg`，唯一漏网）。
//! 修复 = 提取 `identity_set_arg` 对称走转义；本文件钉死转义后的 wire 形态。

use crate::roaming::identity_set_arg;

#[test]
fn b023_identity_set_arg_escapes_quotes_and_backslashes() {
    // 含引号与反斜杠的输入：转义后必须仍是合法 JSON，且字段值 round-trip
    // 等于原输入（插件侧 serde 解析拿到的就是原文，走诚实 hex 校验拒绝，
    // 而不是 JSON parse 失败 / 字段注入）。
    let raw = r#"ab"\cd"#;
    let arg = identity_set_arg(raw);
    assert_eq!(arg, r#"{"secret_key_hex":"ab\"\\cd"}"#);
    let v: serde_json::Value =
        serde_json::from_str(&arg).expect("escaped identity_set arg must parse as JSON");
    assert_eq!(v["secret_key_hex"], raw);
}

#[test]
fn b023_identity_set_arg_plain_hex_unchanged() {
    // 常规 hex 输入 wire 形态逐字节不变（对齐旧合法载荷，无契约漂移）。
    let arg = identity_set_arg("a3f9c1");
    assert_eq!(arg, r#"{"secret_key_hex":"a3f9c1"}"#);
    let v: serde_json::Value = serde_json::from_str(&arg).expect("plain arg must parse");
    assert_eq!(v["secret_key_hex"], "a3f9c1");
}
