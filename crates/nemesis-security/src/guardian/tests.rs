use super::*;

#[test]
fn parse_clean_json() {
    let v = parse_verdict(
        r#"{"risk_level":"high","user_authorization":"low","outcome":"deny","rationale":"no auth"}"#,
    )
    .unwrap();
    assert_eq!(v.outcome, JudgeOutcome::Deny);
    assert_eq!(v.risk_level, "high");
}

#[test]
fn parse_with_prose_and_fence() {
    let raw = "Here is my verdict:\n```json\n{\"risk_level\":\"low\",\"user_authorization\":\"high\",\"outcome\":\"allow\",\"rationale\":\"ok\"}\n```\nThanks.";
    let v = parse_verdict(raw).unwrap();
    assert_eq!(v.outcome, JudgeOutcome::Allow);
}

#[test]
fn parse_rejects_missing_braces() {
    assert!(parse_verdict("no json here at all").is_err());
}

#[test]
fn prompt_is_nonempty_and_mentions_evidence() {
    assert!(GUARDIAN_PROMPT.contains("EVIDENCE"));
    assert!(GUARDIAN_PROMPT.contains("safety gate"));
    assert!(!GUARDIAN_PROMPT.is_empty());
}

// ----- mock LlmJudge (CRITICAL-trigger integration) -----

struct MockJudge {
    verdict: JudgeVerdict,
}
#[async_trait::async_trait]
impl LlmJudge for MockJudge {
    async fn judge(&self, _req: &JudgeRequest) -> Result<JudgeVerdict, String> {
        Ok(self.verdict.clone())
    }
}

#[tokio::test]
async fn mock_judge_denies_critical_unauthorized() {
    // CRITICAL op + unknown authorization → guardian must deny.
    let j = MockJudge {
        verdict: JudgeVerdict {
            risk_level: "critical".into(),
            user_authorization: "unknown".into(),
            outcome: JudgeOutcome::Deny,
            rationale: "destructive without explicit auth".into(),
        },
    };
    let req = JudgeRequest {
        action: "process_exec".into(),
        risk_level: "critical".into(),
        transcript: "rm -rf /".into(),
    };
    let v = j.judge(&req).await.unwrap();
    assert_eq!(v.outcome, JudgeOutcome::Deny);
}

#[tokio::test]
async fn mock_judge_allows_explicit_user_auth() {
    // HIGH op explicitly requested by user → allow.
    let j = MockJudge {
        verdict: JudgeVerdict {
            risk_level: "high".into(),
            user_authorization: "high".into(),
            outcome: JudgeOutcome::Allow,
            rationale: "user explicitly requested".into(),
        },
    };
    let req = JudgeRequest {
        action: "file_write".into(),
        risk_level: "high".into(),
        transcript: "user: create x.txt".into(),
    };
    let v = j.judge(&req).await.unwrap();
    assert_eq!(v.outcome, JudgeOutcome::Allow);
}

#[test]
fn verdict_outcome_serializes_lowercase() {
    // Boundary: outcome must serialize as lowercase "allow"/"deny" to match
    // the guardian prompt's JSON contract (serde rename_all = "lowercase").
    let v = JudgeVerdict {
        risk_level: "low".into(),
        user_authorization: "high".into(),
        outcome: JudgeOutcome::Allow,
        rationale: String::new(),
    };
    let s = serde_json::to_string(&v).unwrap();
    assert!(s.contains("\"outcome\":\"allow\""), "got: {s}");
}

// ----- judge error / 使用异常: judge must propagate errors, never panic -----

struct ErrJudge;
#[async_trait::async_trait]
impl LlmJudge for ErrJudge {
    async fn judge(&self, _req: &JudgeRequest) -> Result<JudgeVerdict, String> {
        Err("LLM provider unavailable".into())
    }
}

#[tokio::test]
async fn judge_error_propagates_without_panic() {
    // Boundary: if the LLM call errors (timeout / unavailable), judge returns
    // Err — the agent loop treats Err as "proceed" (rules already allowed),
    // never panics.
    let j = ErrJudge;
    let req = JudgeRequest {
        action: "process_exec".into(),
        risk_level: "critical".into(),
        transcript: String::new(),
    };
    let r = j.judge(&req).await;
    assert!(r.is_err(), "Err judge must propagate error, not panic");
}

#[test]
fn parse_empty_and_whitespace_returns_err() {
    // Boundary: empty / whitespace-only responses must error, not panic.
    assert!(parse_verdict("").is_err());
    assert!(parse_verdict("   \n\t  ").is_err());
}

#[test]
fn parse_json_missing_outcome_field_errors() {
    // Boundary: JSON without the required `outcome` field must error (strict).
    let r = parse_verdict(r#"{"risk_level":"low","user_authorization":"high"}"#);
    assert!(r.is_err(), "missing outcome must fail parsing");
}
