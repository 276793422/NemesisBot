//! env 槽硬校验回归（BUG-20261116-025）：parse_env_slot 对
//! 空键 / 含 `=` 键 / 含 NUL 键 / 含 NUL 值诚实 Err——此前漏检到
//! std::process::Command::env 的 panic 面（Unix 键空/`=`/NUL panic、
//! 全平台 NUL panic），绕过「形态错误 → Err fail loud（工具不执行，
//! 模型可自纠）」契约：release 默认 profile 被 tool_dispatch
//! catch_unwind 兜成合成错误，iotsmall panic=abort 直接炸进程。
//! 校验单点覆盖四消费方（exec / run_checks / run_script /
//! background_start）。

use super::*;
use crate::context::RequestContext;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

/// 正常键值对照（修复不收窄合法面）。
#[test]
fn parse_env_slot_accepts_normal_pairs() {
    let val = serde_json::json!({"env": {"API_TOKEN": "v1", "OTHER": "v2"}});
    let pairs = parse_env_slot(&val).unwrap().expect("non-empty env");
    assert_eq!(pairs.len(), 2);
    assert!(pairs.contains(&("API_TOKEN".to_string(), "v1".to_string())));
    assert!(pairs.contains(&("OTHER".to_string(), "v2".to_string())));
}

/// 缺席 / 空对象 = None（零行为变化，守卫既有语义）。
#[test]
fn parse_env_slot_absent_or_empty_is_none() {
    assert!(parse_env_slot(&serde_json::json!({})).unwrap().is_none());
    assert!(
        parse_env_slot(&serde_json::json!({"env": {}}))
            .unwrap()
            .is_none()
    );
}

/// 空键 → Err（Unix std panic 面；全平台统一拒）。
#[test]
fn parse_env_slot_rejects_empty_key() {
    let val = serde_json::json!({"env": {"": "v"}});
    let err = parse_env_slot(&val).unwrap_err();
    assert!(err.contains("must not be empty"), "消息形态: {err}");
}

/// 含 `=` 键 → Err（Unix std panic 面；Windows 不 panic 但环境块语义
/// 同样病态，全平台统一拒）。
#[test]
fn parse_env_slot_rejects_eq_in_key() {
    let val = serde_json::json!({"env": {"A=B": "v"}});
    let err = parse_env_slot(&val).unwrap_err();
    assert!(err.contains("'='"), "消息形态: {err}");
}

/// 含 NUL 键 → Err（全平台 std panic 面）。
#[test]
fn parse_env_slot_rejects_nul_in_key() {
    let val = serde_json::json!({"env": {"A\u{0000}B": "v"}});
    let err = parse_env_slot(&val).unwrap_err();
    assert!(err.contains("NUL"), "消息形态: {err}");
}

/// 含 NUL 值 → Err（全平台 std panic 面）。
#[test]
fn parse_env_slot_rejects_nul_in_value() {
    let val = serde_json::json!({"env": {"K": "v\u{0000}x"}});
    let err = parse_env_slot(&val).unwrap_err();
    assert!(err.contains("NUL"), "消息形态: {err}");
}

/// execute 面：ExecTool 收到 NUL 键 env → Err 诚实回灌（spawn 之前拦截，
/// 不 panic 不起子进程）。修复前该测试在 `cmd.envs()` 处 panic（红证）。
#[tokio::test]
async fn exec_tool_execute_nul_env_key_is_honest_err() {
    let t = ExecTool::new("/tmp/ws", true);
    let args = serde_json::json!({
        "command": "echo hi",
        "env": {"BAD\u{0000}KEY": "v"}
    })
    .to_string();
    let err = t.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("Invalid 'env' argument"), "消息形态: {err}");
    assert!(err.contains("NUL"), "消息形态: {err}");
}
