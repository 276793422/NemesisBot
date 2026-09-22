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
