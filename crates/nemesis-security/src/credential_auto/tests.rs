//! credential_auto 单测（开关/计数/环/截断）。

use super::*;

#[test]
fn switch_lifecycle() {
    let st = CredentialAutoApproveState::new(false);
    assert!(!st.enabled());
    st.set_enabled(true);
    assert!(st.enabled());
    st.set_enabled(false);
    assert!(!st.enabled());
}

#[test]
fn counters_independent() {
    let st = CredentialAutoApproveState::new(false);
    st.bump_auto();
    st.bump_auto();
    st.bump_preauth();
    st.bump_denied();
    assert_eq!(st.counts(), (2, 1, 1));
}

#[test]
fn ring_cap_evicts_oldest() {
    let st = CredentialAutoApproveState::new(false);
    for i in 0..(CREDENTIAL_RECORDS_CAP + 10) {
        st.record(CredentialUseRecord {
            ts_ms: i as u64,
            tool: "exec".to_string(),
            fields: "env.PW".to_string(),
            aliases: "vault:t/pw".to_string(),
            target_summary: format!("cmd-{i}"),
            outcome: "auto",
            source: "credential_switch",
        });
    }
    let snap = st.records_snapshot();
    assert_eq!(snap.len(), CREDENTIAL_RECORDS_CAP);
    // 最旧 10 条被挤掉：首条 ts_ms = 10。
    assert_eq!(snap[0].ts_ms, 10);
    assert_eq!(
        snap.last().unwrap().ts_ms,
        (CREDENTIAL_RECORDS_CAP + 9) as u64
    );
}

#[test]
fn truncate_summary_char_safe() {
    // 多字节中文字符不被切开。
    let long = "命令".repeat(100);
    let out = truncate_target_summary(&long);
    assert!(out.chars().count() <= TARGET_SUMMARY_MAX + 1);
    assert!(out.ends_with('…'));
    assert_eq!(truncate_target_summary("short"), "short");
}
