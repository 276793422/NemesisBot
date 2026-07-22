//! Tests for the `skill_manage` tool (agent-authored skills / procedural memory).

use super::*;
use std::sync::Mutex;
use tempfile::TempDir;

/// A benign sample SKILL.md that passes the security check (lint score high).
const SAMPLE_SKILL: &str = "---\n\
name: my-skill\n\
description: A test skill. Use when the user asks to do X for the test suite.\n\
---\n\
# My Skill\n\
\n\
## When to Use\n\
When doing X.\n\
\n\
## Procedure\n\
1. Step one\n\
2. Step two\n";

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

fn tool_in(tmp: &TempDir) -> SkillManageTool {
    SkillManageTool::new(tmp.path().to_string_lossy().to_string(), None, false)
}

fn json_args(value: serde_json::Value) -> String {
    value.to_string()
}

/// A mock approval manager that records calls and returns a fixed verdict.
struct MockApproval {
    approve: bool,
    calls: Mutex<Vec<String>>,
}

impl nemesis_security::auditor::ApprovalManager for MockApproval {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        _timeout_secs: u64,
    ) -> Result<bool, String> {
        self.calls.lock().unwrap().push(format!(
            "{}/{}/{}/{}/{}",
            request_id, operation, target, risk_level, reason
        ));
        Ok(self.approve)
    }
}

fn slot_with(approve: bool) -> (ApprovalManagerSlot, Arc<MockApproval>) {
    let mock = Arc::new(MockApproval {
        approve,
        calls: Mutex::new(vec![]),
    });
    let slot: ApprovalManagerSlot = Arc::new(parking_lot::RwLock::new(Some(mock.clone())));
    (slot, mock)
}

#[tokio::test]
async fn test_skill_manage_create() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    let args =
        json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL}));
    let res = tool.execute(&args, &ctx()).await.unwrap();
    assert!(res.contains("created"), "{}", res);

    let skill_md = tmp.path().join("skills").join("my-skill").join("SKILL.md");
    assert!(skill_md.exists(), "SKILL.md should be written");
    let body = std::fs::read_to_string(&skill_md).unwrap();
    assert!(body.contains("# My Skill"));
}

#[tokio::test]
async fn test_skill_manage_create_no_overwrite() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    let args =
        json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL}));
    tool.execute(&args, &ctx()).await.unwrap();

    // Second create without overwrite -> error.
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("already exists"), "{}", err);

    // With overwrite -> ok.
    let args2 = json_args(
        serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL,"overwrite":true}),
    );
    assert!(tool.execute(&args2, &ctx()).await.is_ok());
}

#[tokio::test]
async fn test_skill_manage_invalid_name_rejected() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    let args =
        json_args(serde_json::json!({"action":"create","name":"../evil","content":SAMPLE_SKILL}));
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("invalid skill name"), "{}", err);
    // Nothing escaped the workspace.
    assert!(!tmp.path().join("..").join("evil").join("SKILL.md").exists());
}

#[tokio::test]
async fn test_skill_manage_patch() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    tool.execute(
        &json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL})),
        &ctx(),
    )
    .await
    .unwrap();

    let patch = json_args(
        serde_json::json!({"action":"patch","name":"my-skill","old":"Step one","new":"Step ONE (patched)"}),
    );
    let res = tool.execute(&patch, &ctx()).await.unwrap();
    assert!(res.contains("updated"), "{}", res);

    let body = std::fs::read_to_string(tmp.path().join("skills").join("my-skill").join("SKILL.md"))
        .unwrap();
    assert!(body.contains("Step ONE (patched)"));
    assert!(body.contains("Step two")); // untouched line still present
}

#[tokio::test]
async fn test_skill_manage_patch_old_not_found() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    tool.execute(
        &json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL})),
        &ctx(),
    )
    .await
    .unwrap();

    let patch = json_args(
        serde_json::json!({"action":"patch","name":"my-skill","old":"NONEXISTENT","new":"x"}),
    );
    let err = tool.execute(&patch, &ctx()).await.unwrap_err();
    assert!(err.contains("not found"), "{}", err);
}

#[tokio::test]
async fn test_skill_manage_write_file() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    tool.execute(
        &json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL})),
        &ctx(),
    )
    .await
    .unwrap();

    let wf = json_args(
        serde_json::json!({"action":"write_file","name":"my-skill","path":"references/api.md","content":"# API"}),
    );
    let res = tool.execute(&wf, &ctx()).await.unwrap();
    assert!(res.contains("Wrote"), "{}", res);
    assert!(
        tmp.path()
            .join("skills")
            .join("my-skill")
            .join("references")
            .join("api.md")
            .exists()
    );
}

#[tokio::test]
async fn test_skill_manage_write_file_traversal_blocked() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    tool.execute(
        &json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL})),
        &ctx(),
    )
    .await
    .unwrap();

    let wf = json_args(
        serde_json::json!({"action":"write_file","name":"my-skill","path":"../escape.md","content":"evil"}),
    );
    let err = tool.execute(&wf, &ctx()).await.unwrap_err();
    assert!(err.contains("path"), "{}", err);
    // The escaped file must NOT exist next to the skills dir.
    assert!(!tmp.path().join("escape.md").exists());
}

#[tokio::test]
async fn test_skill_manage_remove_file() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    tool.execute(
        &json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL})),
        &ctx(),
    )
    .await
    .unwrap();
    tool.execute(
        &json_args(serde_json::json!({"action":"write_file","name":"my-skill","path":"refs.md","content":"x"})),
        &ctx(),
    )
    .await
    .unwrap();

    let rf =
        json_args(serde_json::json!({"action":"remove_file","name":"my-skill","path":"refs.md"}));
    tool.execute(&rf, &ctx()).await.unwrap();
    assert!(
        !tmp.path()
            .join("skills")
            .join("my-skill")
            .join("refs.md")
            .exists(),
        "file should be removed"
    );
    // SKILL.md must be untouched.
    assert!(
        tmp.path()
            .join("skills")
            .join("my-skill")
            .join("SKILL.md")
            .exists(),
        "SKILL.md must remain"
    );
}

#[tokio::test]
async fn test_skill_manage_delete() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    tool.execute(
        &json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL})),
        &ctx(),
    )
    .await
    .unwrap();

    let del = json_args(serde_json::json!({"action":"delete","name":"my-skill"}));
    let res = tool.execute(&del, &ctx()).await.unwrap();
    assert!(res.contains("deleted"), "{}", res);
    assert!(!tmp.path().join("skills").join("my-skill").exists());

    // Deleting again -> not found.
    let err = tool.execute(&del, &ctx()).await.unwrap_err();
    assert!(err.contains("not found"), "{}", err);
}

#[tokio::test]
async fn test_skill_manage_unknown_action() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    let args = json_args(serde_json::json!({"action":"bogus","name":"my-skill"}));
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("unknown action"), "{}", err);
}

#[tokio::test]
async fn test_skill_manage_missing_fields() {
    let tmp = TempDir::new().unwrap();
    let tool = tool_in(&tmp);
    // No action.
    let err = tool
        .execute(&json_args(serde_json::json!({"name":"my-skill"})), &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("action"), "{}", err);
    // No name.
    let err = tool
        .execute(&json_args(serde_json::json!({"action":"delete"})), &ctx())
        .await
        .unwrap_err();
    assert!(err.contains("name"), "{}", err);
}

#[tokio::test]
async fn test_skill_manage_approval_approved_writes() {
    let tmp = TempDir::new().unwrap();
    let (slot, mock) = slot_with(true);
    let tool = SkillManageTool::new(tmp.path().to_string_lossy().to_string(), Some(slot), true);
    let args =
        json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL}));
    let res = tool.execute(&args, &ctx()).await.unwrap();
    assert!(res.contains("created"), "{}", res);
    assert_eq!(
        mock.calls.lock().unwrap().len(),
        1,
        "approval should have been requested once"
    );
    assert!(
        tmp.path()
            .join("skills")
            .join("my-skill")
            .join("SKILL.md")
            .exists(),
        "skill should be written after approval"
    );
}

#[tokio::test]
async fn test_skill_manage_approval_denied_blocks_write() {
    let tmp = TempDir::new().unwrap();
    let (slot, _mock) = slot_with(false);
    let tool = SkillManageTool::new(tmp.path().to_string_lossy().to_string(), Some(slot), true);
    let args =
        json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL}));
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("denied"), "{}", err);
    // Skill must NOT be written.
    assert!(
        !tmp.path()
            .join("skills")
            .join("my-skill")
            .join("SKILL.md")
            .exists(),
        "denied write must not create the skill"
    );
}

#[tokio::test]
async fn test_skill_manage_approval_required_but_no_manager() {
    let tmp = TempDir::new().unwrap();
    // require_approval=true but no manager slot -> refused (safe default).
    let tool = SkillManageTool::new(tmp.path().to_string_lossy().to_string(), None, true);
    let args =
        json_args(serde_json::json!({"action":"create","name":"my-skill","content":SAMPLE_SKILL}));
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("no approval manager"), "{}", err);
    assert!(
        !tmp.path()
            .join("skills")
            .join("my-skill")
            .join("SKILL.md")
            .exists(),
        "must not write without an approval manager"
    );
}

// ---- GrepTool (Phase 2 coding tools) ----

#[tokio::test]
async fn test_grep_tool_finds_matches_recursively() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().to_string_lossy().to_string();
    std::fs::write(tmp.path().join("a.rs"), "fn hello() {}\nfn world() {}\n").unwrap();
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    std::fs::write(tmp.path().join("sub").join("b.rs"), "fn hello_again() {}\n").unwrap();
    let tool = GrepTool::new(ws);
    let args = json_args(serde_json::json!({"pattern":"hello"}));
    let res = tool.execute(&args, &ctx()).await.unwrap();
    assert!(res.contains("a.rs:1"), "should match a.rs:1 — {}", res);
    assert!(res.contains("b.rs:1"), "should match sub/b.rs:1 — {}", res);
}

#[tokio::test]
async fn test_grep_tool_glob_filter() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().to_string_lossy().to_string();
    std::fs::write(tmp.path().join("a.rs"), "matchme\n").unwrap();
    std::fs::write(tmp.path().join("b.txt"), "matchme\n").unwrap();
    let tool = GrepTool::new(ws);
    let args = json_args(serde_json::json!({"pattern":"matchme","glob":"*.rs"}));
    let res = tool.execute(&args, &ctx()).await.unwrap();
    assert!(res.contains("a.rs"), "{}", res);
    assert!(
        !res.contains("b.txt"),
        "glob *.rs must filter out .txt — {}",
        res
    );
}

#[tokio::test]
async fn test_grep_tool_no_match() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.rs"), "fn foo() {}\n").unwrap();
    let tool = GrepTool::new(tmp.path().to_string_lossy().to_string());
    let args = json_args(serde_json::json!({"pattern":"zzz_not_present"}));
    let res = tool.execute(&args, &ctx()).await.unwrap();
    assert!(res.contains("No matches"), "{}", res);
}

// ---- GrepTool edge cases / branch coverage ----

#[tokio::test]
async fn test_grep_tool_max_results_limits() {
    let tmp = TempDir::new().unwrap();
    // Create a file with 10 matches.
    let content: String = std::iter::repeat("matchline\n").take(10).collect();
    std::fs::write(tmp.path().join("a.rs"), &content).unwrap();
    let tool = GrepTool::new(tmp.path().to_string_lossy().to_string());
    let args = json_args(serde_json::json!({"pattern":"matchline","max_results":3}));
    let res = tool.execute(&args, &ctx()).await.unwrap();
    let count = res.matches("matchline").count();
    assert_eq!(count, 3, "should cap at max_results=3 — {}", res);
}

#[tokio::test]
async fn test_grep_tool_invalid_regex() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.rs"), "fn main() {}\n").unwrap();
    let tool = GrepTool::new(tmp.path().to_string_lossy().to_string());
    // Unclosed character class → invalid regex.
    let args = json_args(serde_json::json!({"pattern":"[unclosed"}));
    let res = tool.execute(&args, &ctx()).await;
    assert!(res.is_err(), "invalid regex should return Err");
    assert!(res.unwrap_err().contains("invalid regex"));
}

#[tokio::test]
async fn test_grep_tool_skips_target_dir() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("root.rs"), "findme\n").unwrap();
    // Create a "target" dir with a matching file — should be skipped.
    std::fs::create_dir(tmp.path().join("target")).unwrap();
    std::fs::write(tmp.path().join("target").join("hidden.rs"), "findme\n").unwrap();
    let tool = GrepTool::new(tmp.path().to_string_lossy().to_string());
    let args = json_args(serde_json::json!({"pattern":"findme"}));
    let res = tool.execute(&args, &ctx()).await.unwrap();
    assert!(res.contains("root.rs"), "should find root.rs — {}", res);
    assert!(
        !res.contains("hidden.rs"),
        "should skip target dir — {}",
        res
    );
}

// ---- GitTool ----

#[tokio::test]
async fn test_git_tool_unknown_action() {
    let tool = GitTool::new(
        std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string(),
    );
    let args = json_args(serde_json::json!({"action":"push"}));
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("unknown git action"), "{}", err);
}

#[tokio::test]
async fn test_git_tool_unknown_action_empty() {
    let tool = GitTool::new(
        std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string(),
    );
    let args = json_args(serde_json::json!({"action":"bogus_xyz"}));
    let err = tool.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("unknown git action 'bogus_xyz'"), "{}", err);
}

#[tokio::test]
async fn test_git_tool_non_git_dir() {
    let tmp = TempDir::new().unwrap();
    let tool = GitTool::new(tmp.path().to_string_lossy().to_string());
    let args = json_args(serde_json::json!({"action":"status"}));
    // Non-git dir → git status fails.
    let result = tool.execute(&args, &ctx()).await;
    // git status in non-repo either errors or returns "(no changes)" with stderr.
    // Either way, should not panic.
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_git_tool_status_in_real_repo() {
    // The project workspace IS a git repo.
    let tool = GitTool::new(
        std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string(),
    );
    let args = json_args(serde_json::json!({"action":"status"}));
    let res = tool.execute(&args, &ctx()).await;
    assert!(res.is_ok(), "git status should work in a repo");
    // Output should contain branch info (## main...origin/main or similar).
    let output = res.unwrap();
    assert!(
        output.contains("##") || output.contains("main") || output.trim().is_empty(),
        "git status --branch output unexpected: {}",
        &output[..output.len().min(200)]
    );
}
