//! InstallPlan / InstallGate 测试。

use super::*;
use crate::security_check::check_skill_security;

fn sample_plan() -> InstallPlan {
    let content = "nmap -sV target";
    InstallPlan {
        slug: "demo-skill".to_string(),
        source: "github:acme/demo-skill".to_string(),
        requested_ref: String::new(),
        commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        published_at: Some(1_700_000_000),
        age_days: Some(30),
        age_decision: "ok".to_string(),
        files: vec![PlanFile {
            path: "SKILL.md".to_string(),
            sha256: "deadbeef".to_string(),
            bytes: 16,
        }],
        total_bytes: 16,
        trust: TrustState::ReviewRequired,
        trust_detail: "unsigned".to_string(),
        security: check_skill_security(content, "demo-skill", ""),
        warnings: vec!["suspicious flag".to_string()],
    }
}

#[test]
fn test_summary_contains_key_fields() {
    let summary = sample_plan().summary();
    assert!(summary.contains("demo-skill"));
    assert!(summary.contains("github:acme/demo-skill@0123456789ab"));
    assert!(summary.contains("review-required"));
    assert!(summary.contains("30 天"));
    assert!(summary.contains("⚠ suspicious flag"));
}

#[test]
fn test_plan_json_roundtrip() {
    let plan = sample_plan();
    let json = plan_to_json(&plan).unwrap();
    let parsed: InstallPlan = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.slug, plan.slug);
    assert_eq!(parsed.files.len(), 1);
    assert_eq!(parsed.commit, plan.commit);
    assert_eq!(parsed.trust, TrustState::ReviewRequired);
}

#[tokio::test]
async fn test_always_allow_gate() {
    let decision = AlwaysAllowGate.decide(&sample_plan()).await;
    assert_eq!(decision, InstallDecision::Approve);
}

#[tokio::test]
async fn test_always_deny_gate() {
    let decision = AlwaysDenyGate.decide(&sample_plan()).await;
    match decision {
        InstallDecision::Deny { reason } => assert!(reason.contains("denied by policy")),
        other => panic!("expected deny, got {:?}", other),
    }
}

#[tokio::test]
async fn test_custom_gate_receives_plan() {
    struct RecordingGate {
        seen_slug: std::sync::Mutex<Option<String>>,
    }

    #[async_trait]
    impl InstallGate for RecordingGate {
        async fn decide(&self, plan: &InstallPlan) -> InstallDecision {
            *self.seen_slug.lock().unwrap() = Some(plan.slug.clone());
            InstallDecision::Deny {
                reason: "not today".to_string(),
            }
        }
    }

    let gate = RecordingGate {
        seen_slug: std::sync::Mutex::new(None),
    };
    let decision = gate.decide(&sample_plan()).await;
    assert_eq!(
        gate.seen_slug.lock().unwrap().as_deref(),
        Some("demo-skill")
    );
    match decision {
        InstallDecision::Deny { reason } => assert_eq!(reason, "not today"),
        other => panic!("expected deny, got {:?}", other),
    }
}

#[test]
fn test_summary_without_commit_and_age() {
    let mut plan = sample_plan();
    plan.commit = String::new();
    plan.age_days = None;
    plan.age_decision = String::new();
    let summary = plan.summary();
    assert!(summary.contains("版本龄：未知"));
    assert!(!summary.contains('@'));
}
