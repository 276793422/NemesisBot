use super::*;
use nemesis_security::signature::TrustLevel;

// -- 四态展示与可装性 ---------------------------------------------------

#[test]
fn trust_state_as_str_and_installable() {
    assert_eq!(PluginTrustState::Trusted.as_str(), "trusted");
    assert_eq!(
        PluginTrustState::ReviewRecommended.as_str(),
        "review-recommended"
    );
    assert_eq!(PluginTrustState::ReviewRequired.as_str(), "review-required");
    assert_eq!(PluginTrustState::Blocked.as_str(), "blocked");
    assert!(PluginTrustState::Trusted.installable());
    assert!(PluginTrustState::ReviewRecommended.installable());
    assert!(PluginTrustState::ReviewRequired.installable());
    assert!(!PluginTrustState::Blocked.installable());
}

// -- 归约四态 -----------------------------------------------------------

#[test]
fn trust_state_unsigned_matrix() {
    let unsigned = |allow| {
        VerificationOutcome {
            signed: false,
            valid: None,
            trust_level: None,
            public_key: String::new(),
            error: String::new(),
        }
        .trust_state(allow)
    };
    assert_eq!(unsigned(true), PluginTrustState::ReviewRequired);
    assert_eq!(unsigned(false), PluginTrustState::Blocked);
}

#[test]
fn trust_state_signed_matrix() {
    let mk = |valid: Option<bool>, level: Option<&str>| VerificationOutcome {
        signed: true,
        valid,
        trust_level: level.map(str::to_string),
        public_key: "ab".repeat(32),
        error: String::new(),
    };
    assert_eq!(
        mk(Some(true), Some("verified")).trust_state(false),
        PluginTrustState::Trusted
    );
    assert_eq!(
        mk(Some(true), Some("community")).trust_state(false),
        PluginTrustState::ReviewRecommended
    );
    // 未知级别从紧（非 Trusted）——ReviewRecommended 可继续装配。
    assert_eq!(
        mk(Some(true), Some("other")).trust_state(false),
        PluginTrustState::ReviewRecommended
    );
    assert_eq!(
        mk(Some(true), None).trust_state(false),
        PluginTrustState::ReviewRecommended
    );
    // valid 缺席按无效。
    assert_eq!(
        mk(None, Some("verified")).trust_state(false),
        PluginTrustState::Blocked
    );
    assert_eq!(
        mk(Some(false), Some("verified")).trust_state(false),
        PluginTrustState::Blocked
    );
}

// -- strip_signature ----------------------------------------------------

#[test]
fn strip_signature_removes_block_and_keeps_body() {
    let raw = "api-version = 1\nslug = \"x\"\n\n[signature]\nalgorithm = \"ed25519\"\npublic-key = \"ab\"\nsignature = \"cd\"\nsigned-at = \"2026-01-01T00:00:00Z\"\n";
    let (canonical, block) = strip_signature(raw);
    assert!(block.is_some());
    let b = block.expect("block");
    assert_eq!(b.algorithm, "ed25519");
    assert_eq!(b.public_key, "ab");
    assert_eq!(b.signature, "cd");
    assert!(!canonical.contains("signature"));
    assert!(canonical.contains("slug = \"x\""));
}

#[test]
fn strip_signature_without_block_and_invalid_toml() {
    let (canonical, block) = strip_signature("api-version = 1\nslug = \"x\"\n");
    assert!(block.is_none());
    assert!(canonical.contains("slug"));
    // 非 TOML 文本：解析失败时原文原样返回（带出签名块结论——解析不到
    // 块 = None）。
    let (canonical, block) = strip_signature("this is not toml [(((");
    assert!(block.is_none());
    assert_eq!(canonical, "this is not toml [(((");
}

// -- verify_manifest（真 Ed25519 密钥对）---------------------------------

fn manifest_body() -> String {
    let sha = "a".repeat(64);
    format!(
        "api-version = 1\nslug = \"demo\"\nkind = \"tool\"\nname = \"Demo\"\nversion = \"0.1.0\"\nwasm-sha256 = \"{sha}\"\n"
    )
}

fn signed_manifest(verifier: &nemesis_security::signature::SignatureVerifier) -> String {
    let canonical = manifest_body();
    let kp = nemesis_security::signature::generate_key_pair().expect("keypair");
    verifier.add_trusted_key(&kp.public_key, "test-signer");
    let sig =
        nemesis_security::signature::sign_content_hex(&canonical, &kp.private_key).expect("sign");
    format!(
        "{canonical}\n[signature]\nalgorithm = \"ed25519\"\npublic-key = \"{}\"\nsignature = \"{sig}\"\nsigned-at = \"2026-01-01T00:00:00Z\"\n",
        kp.public_key
    )
}

#[test]
fn verify_manifest_unsigned() {
    let verifier = nemesis_security::signature::SignatureVerifier::new();
    let (m, outcome) = verify_manifest(&manifest_body(), &verifier).expect("verify");
    assert_eq!(m.slug, "demo");
    assert!(!outcome.signed);
    assert!(outcome.valid.is_none());
    assert!(outcome.public_key.is_empty());
    assert_eq!(outcome.trust_state(true), PluginTrustState::ReviewRequired);
}

#[test]
fn verify_manifest_valid_signature_trusted_key() {
    let verifier = nemesis_security::signature::SignatureVerifier::new();
    let raw = signed_manifest(&verifier);
    let (m, outcome) = verify_manifest(&raw, &verifier).expect("verify");
    assert_eq!(m.slug, "demo");
    assert!(outcome.signed);
    assert_eq!(outcome.valid, Some(true));
    assert_eq!(outcome.trust_level.as_deref(), Some("verified"));
    assert_eq!(outcome.error, "");
    assert_eq!(outcome.trust_state(false), PluginTrustState::Trusted);
}

#[test]
fn verify_manifest_valid_signature_untrusted_key() {
    let verifier = nemesis_security::signature::SignatureVerifier::new();
    let canonical = manifest_body();
    let kp = nemesis_security::signature::generate_key_pair().expect("keypair");
    let sig =
        nemesis_security::signature::sign_content_hex(&canonical, &kp.private_key).expect("sign");
    let raw = format!(
        "{canonical}\n[signature]\nalgorithm = \"ed25519\"\npublic-key = \"{}\"\nsignature = \"{sig}\"\n",
        kp.public_key
    );
    let (_, outcome) = verify_manifest(&raw, &verifier).expect("verify");
    assert_eq!(outcome.valid, Some(false));
    assert_eq!(outcome.trust_level.as_deref(), Some("unknown"));
    assert!(
        outcome.error.contains("not in trust store"),
        "error: {}",
        outcome.error
    );
    assert_eq!(outcome.trust_state(false), PluginTrustState::Blocked);
}

#[test]
fn verify_manifest_tampered_body() {
    let verifier = nemesis_security::signature::SignatureVerifier::new();
    let raw = signed_manifest(&verifier);
    // 换版本号 = 篡改 canonical 字节。
    let tampered = raw.replace("version = \"0.1.0\"", "version = \"9.9.9\"");
    let (_, outcome) = verify_manifest(&tampered, &verifier).expect("verify");
    assert_eq!(outcome.valid, Some(false));
    assert!(outcome.error.contains("verification failed"));
}

#[test]
fn verify_manifest_unsupported_algorithm() {
    let verifier = nemesis_security::signature::SignatureVerifier::new();
    let raw = format!(
        "{}\n[signature]\nalgorithm = \"rsa\"\npublic-key = \"ab\"\nsignature = \"cd\"\n",
        manifest_body()
    );
    let (_, outcome) = verify_manifest(&raw, &verifier).expect("verify");
    assert!(outcome.signed);
    assert_eq!(outcome.valid, Some(false));
    assert!(outcome.error.contains("unsupported algorithm: rsa"));
}

#[test]
fn verify_manifest_unparsable_body() {
    let verifier = nemesis_security::signature::SignatureVerifier::new();
    let err = verify_manifest("not a manifest", &verifier)
        .err()
        .unwrap_or_else(|| panic!("parse fails"));
    assert!(err.to_string().contains("manifest invalid"));
}

#[test]
fn verify_manifest_community_level() {
    let verifier = nemesis_security::signature::SignatureVerifier::new();
    let canonical = manifest_body();
    let kp = nemesis_security::signature::generate_key_pair().expect("keypair");
    verifier
        .trust_store_ref()
        .add_key(&kp.public_key, "community-signer", TrustLevel::Community);
    let sig =
        nemesis_security::signature::sign_content_hex(&canonical, &kp.private_key).expect("sign");
    let raw = format!(
        "{canonical}\n[signature]\nalgorithm = \"ed25519\"\npublic-key = \"{}\"\nsignature = \"{sig}\"\n",
        kp.public_key
    );
    let (_, outcome) = verify_manifest(&raw, &verifier).expect("verify");
    assert_eq!(outcome.trust_level.as_deref(), Some("community"));
    assert_eq!(
        outcome.trust_state(false),
        PluginTrustState::ReviewRecommended
    );
}

// -- trust_state_for ----------------------------------------------------

#[test]
fn trust_state_for_unsigned_strict_gives_actionable_text() {
    let outcome = VerificationOutcome {
        signed: false,
        valid: None,
        trust_level: None,
        public_key: String::new(),
        error: String::new(),
    };
    let err = trust_state_for(&outcome, false)
        .err()
        .unwrap_or_else(|| panic!("strict rejects"));
    let msg = err.to_string();
    assert!(msg.contains("--allow-unsigned"), "可行动文案: {msg}");
    // 宽容旗标放行。
    assert_eq!(
        trust_state_for(&outcome, true).expect("allow"),
        PluginTrustState::ReviewRequired
    );
}

#[test]
fn trust_state_for_signed_block_passes_through_error() {
    let outcome = VerificationOutcome {
        signed: true,
        valid: Some(false),
        trust_level: None,
        public_key: "ab".repeat(32),
        error: "signature verification failed (tampered manifest?)".into(),
    };
    let err = trust_state_for(&outcome, true)
        .err()
        .unwrap_or_else(|| panic!("invalid signature blocked"));
    assert!(err.to_string().contains("tampered manifest"));
    // 无签名失败细节时（signed=true 但 error 空）也不空串。
    let outcome = VerificationOutcome {
        signed: true,
        valid: Some(false),
        trust_level: None,
        public_key: "ab".repeat(32),
        error: String::new(),
    };
    let err = trust_state_for(&outcome, true)
        .err()
        .unwrap_or_else(|| panic!("invalid signature blocked"));
    assert!(!err.to_string().is_empty());
}

#[test]
fn trust_state_for_installable_states_pass() {
    for (signed, valid, level, allow, want) in [
        (
            true,
            Some(true),
            Some("verified"),
            false,
            PluginTrustState::Trusted,
        ),
        (
            true,
            Some(true),
            Some("community"),
            false,
            PluginTrustState::ReviewRecommended,
        ),
        (false, None, None, true, PluginTrustState::ReviewRequired),
    ] {
        let outcome = VerificationOutcome {
            signed,
            valid,
            trust_level: level.map(str::to_string),
            public_key: "ab".repeat(32),
            error: String::new(),
        };
        assert_eq!(trust_state_for(&outcome, allow).expect("ok"), want);
    }
}
