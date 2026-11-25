//! 会话级封禁用户面提示回归锁（2026-11-24 用户事故批）。
//!
//! 实录：上游返回 403 + `"code":"session_blocked_by_cyber_policy"`（封的
//! 是会话身份不是单条请求），用户在死会话里反复删除轮次/重发均撞同一封
//! 禁——旧文案「多为内容风控/权限/区域限制」给不出可行动出路（换会话）。
//!
//! 锁面：
//! - `forbidden_user_hint` 分流——码族/实测短语命中 → 会话级封禁文案
//!   （含「请新建会话」出路）；普通 403 → 旧文案**逐字节不变**（提示词
//!   缓存/既有 e2e 断言兼容）；
//! - 词表负例——普通内容风控 403（"content was flagged"）不得误入会话
//!   封禁文案（两案处置不同）。

use super::recovery::forbidden_user_hint;

/// 实录错误原文（CC Switch 代理 + codex lane，字段截取自用户事故报告）。
const REAL_SESSION_BLOCK_ERR: &str = r#"forbidden by provider codex/gpt-6.2-sol: status 403: {"error":{"message":"CC Switch local proxy failed while handling Codex endpoint /responses. Provider: demo; model: gpt-6.2-sol; upstream_status: HTTP 403; cause: 该会话已被网络安全策略屏蔽，请开启新会话 / This session is blocked by cyber-security policy, please start a new session","type":"permission_error","code":"session_blocked_by_cyber_policy","param":null,"provider":"demo"}}"#;

const PLAIN_403_HINT: &str =
    "上游拒绝访问（403）：多为内容风控/权限/区域限制，重试无意义，已停止重试";

#[test]
fn real_session_block_error_gets_new_session_guidance() {
    let hint = forbidden_user_hint(REAL_SESSION_BLOCK_ERR);
    assert!(hint.contains("请新建会话"), "hint = {hint}");
    assert!(hint.contains("会话级"), "hint = {hint}");
    assert_ne!(hint, PLAIN_403_HINT);
}

#[test]
fn session_blocked_code_substring_hits() {
    // 裸码形态（无 403 字样/英文短语的码族锚）。
    assert!(
        forbidden_user_hint("upstream rejected: code session_blocked_by_cyber_policy")
            .contains("请新建会话")
    );
    // 大小写不敏感（码变体大小写形态）。
    assert!(forbidden_user_hint("Session_Blocked_By_Policy").contains("请新建会话"));
}

#[test]
fn plain_content_policy_403_keeps_old_hint_verbatim() {
    // 普通内容风控 403：换内容可解，不得给「新建会话」误导。
    let flagged =
        "forbidden by provider demo: status 403: This content was flagged per our content policy";
    assert_eq!(forbidden_user_hint(flagged), PLAIN_403_HINT);
    let region = "上游拒绝访问（403）status 403 region restricted";
    assert_eq!(forbidden_user_hint(region), PLAIN_403_HINT);
}

#[test]
fn permission_error_code_without_session_markers_keeps_old_hint() {
    // anthropic permission_error（普通 403 语义码，无会话封禁标记）。
    assert_eq!(
        forbidden_user_hint(r#"status 403: {"type":"permission_error","message":"denied"}"#),
        PLAIN_403_HINT
    );
}
