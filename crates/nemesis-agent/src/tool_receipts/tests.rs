//! T1（扩展计划 D3）：工具收据单测——生成/校验往返、篡改拒绝、
//! 错误 key 拒绝（伪造面 = 猜 256-bit HMAC）。

use super::*;

#[test]
fn roundtrip_verifies() {
    let key = ReceiptKey::generate();
    let receipt = generate_receipt(
        &key,
        "exec",
        r#"{"command":"cargo test"}"#,
        "ok",
        1_726_000_000_000,
    );
    assert!(receipt.starts_with(RECEIPT_PREFIX));
    assert!(verify_receipt(
        &key,
        "exec",
        r#"{"command":"cargo test"}"#,
        "ok",
        1_726_000_000_000,
        &receipt,
    ));
}

#[test]
fn tampered_args_rejected() {
    let key = ReceiptKey::generate();
    let receipt = generate_receipt(&key, "exec", r#"{"command":"a"}"#, "ok", 1);
    assert!(!verify_receipt(
        &key,
        "exec",
        r#"{"command":"b"}"#,
        "ok",
        1,
        &receipt
    ));
}

#[test]
fn tampered_result_rejected() {
    let key = ReceiptKey::generate();
    let receipt = generate_receipt(&key, "exec", "{}", "exit code: 0", 1);
    assert!(!verify_receipt(
        &key,
        "exec",
        "{}",
        "exit code: 1",
        1,
        &receipt
    ));
}

#[test]
fn wrong_tool_or_ts_rejected() {
    let key = ReceiptKey::generate();
    let receipt = generate_receipt(&key, "exec", "{}", "ok", 1);
    assert!(!verify_receipt(&key, "edit_file", "{}", "ok", 1, &receipt));
    assert!(!verify_receipt(&key, "exec", "{}", "ok", 2, &receipt));
}

#[test]
fn wrong_key_rejected() {
    let key = ReceiptKey::generate();
    let other = ReceiptKey::generate();
    let receipt = generate_receipt(&key, "exec", "{}", "ok", 1);
    assert!(!verify_receipt(&other, "exec", "{}", "ok", 1, &receipt));
}

#[test]
fn distinct_keys_distinct_receipts() {
    let a = ReceiptKey::generate();
    let b = ReceiptKey::generate();
    let ra = generate_receipt(&a, "exec", "{}", "ok", 1);
    let rb = generate_receipt(&b, "exec", "{}", "ok", 1);
    assert_ne!(ra, rb);
}
