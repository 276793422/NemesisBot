//! WS4 供应链安装门端到端测试（P11-P16）。
//!
//! 用 wiremock 模拟 GitHub commits/trees/raw 三个端点，走 `SkillInstaller`
//! 真实漏斗：pin 解析 → staging 下载 → 验签 → 安全扫描 → 版本龄 → 审批门 →
//! lockfile 记账。registry 路径的 staging/审批/记账接线也在本文件覆盖。

use std::sync::{Arc, Mutex};

use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::install_gate::{AlwaysAllowGate, AlwaysDenyGate, InstallDecision, InstallPlan};
use crate::lockfile::SkillsLockfile;
use crate::registry::RegistryManager;
use crate::registry::SkillRegistry;
use crate::types::{
    BrowseResult, BrowseSort, InstallResult, SkillContent, SkillMeta, SkillSearchResult,
};
use nemesis_types::error::Result;

/// 安全内容（不触发任何 lint 规则）。
const SAFE_SKILL_MD: &str = "# Demo Skill\n\nPrint a friendly greeting.\n";

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn commit_date_days_ago(days: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339()
}

/// 挂 commits + trees + raw 三个端点。
async fn mount_github(
    server: &MockServer,
    repo: &str,
    sha: &str,
    commit_date: &str,
    files: &[(&str, &str)],
) {
    let tree_entries: Vec<serde_json::Value> = files
        .iter()
        .map(|(p, _)| serde_json::json!({ "path": p, "type": "blob" }))
        .collect();

    // commits API（HEAD 与显式 ref 都挂同一响应）。
    Mock::given(method("GET"))
        .and(path(format!("/repos/{}/commits/HEAD", repo)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sha": sha,
            "commit": { "committer": { "date": commit_date } }
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/repos/{}/commits/v1.2", repo)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sha": sha,
            "commit": { "committer": { "date": commit_date } }
        })))
        .mount(server)
        .await;

    // trees API（ref 固定为解析后的 sha）。
    Mock::given(method("GET"))
        .and(path(format!("/repos/{}/git/trees/{}", repo, sha)))
        .and(query_param("recursive", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sha": sha,
            "tree": tree_entries,
            "truncated": false
        })))
        .mount(server)
        .await;

    // raw 内容（路径形如 /{repo}/{sha}/{blob}）。
    for (blob_path, content) in files {
        Mock::given(method("GET"))
            .and(path(format!("/{}/{}/{}", repo, sha, blob_path)))
            .respond_with(ResponseTemplate::new(200).set_body_string(content.to_string()))
            .mount(server)
            .await;
    }
}

fn make_installer(workspace: &std::path::Path, server: &MockServer) -> SkillInstaller {
    let mut installer = SkillInstaller::new(&workspace.to_string_lossy());
    installer.set_github_base_url(&server.uri());
    installer.set_github_api_url(&server.uri());
    installer
}

// ---------------------------------------------------------------------------
// parse_github_ref
// ---------------------------------------------------------------------------

#[test]
fn test_parse_github_ref_accepts_owner_repo_and_ref() {
    let (repo, reference) = SkillInstaller::parse_github_ref("acme/demo-skill").unwrap();
    assert_eq!(repo, "acme/demo-skill");
    assert_eq!(reference, "");

    let (repo, reference) = SkillInstaller::parse_github_ref("acme/demo-skill@v1.2").unwrap();
    assert_eq!(repo, "acme/demo-skill");
    assert_eq!(reference, "v1.2");

    let (_repo, reference) = SkillInstaller::parse_github_ref(
        "acme/demo-skill@0123456789abcdef0123456789abcdef01234567",
    )
    .unwrap();
    assert_eq!(reference, "0123456789abcdef0123456789abcdef01234567");
}

#[test]
fn test_parse_github_ref_rejects_bad_shapes() {
    // 缺 owner/repo 两段。
    assert!(SkillInstaller::parse_github_ref("just-a-name").is_err());
    assert!(SkillInstaller::parse_github_ref("a/b/c").is_err());
    // 路径注入。
    assert!(SkillInstaller::parse_github_ref("../etc/passwd").is_err());
    assert!(SkillInstaller::parse_github_ref("a/b@..%2f").is_err());
    assert!(SkillInstaller::parse_github_ref("a/b@feat/x").is_err());
    // 空白非法（脏 slug 在任何网络语句前就地拒绝）。
    assert!(SkillInstaller::parse_github_ref("a/bad slug").is_err());
}

// ---------------------------------------------------------------------------
// P12 pin + P14 lockfile 记账
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_install_github_pins_commit_records_lockfile_and_origin() {
    let server = MockServer::start().await;
    mount_github(
        &server,
        "acme/demo-skill",
        SHA,
        &commit_date_days_ago(365),
        &[("SKILL.md", SAFE_SKILL_MD)],
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let installer = make_installer(tmp.path(), &server);

    let outcome = installer.install_github("acme/demo-skill").await.unwrap();
    assert_eq!(outcome.slug, "demo-skill");
    assert_eq!(outcome.commit, SHA);
    assert_eq!(outcome.files_installed, 1);
    assert_eq!(outcome.trust, crate::trust::TrustState::ReviewRequired);

    // 技能落位。
    assert!(tmp.path().join("skills/demo-skill/SKILL.md").exists());
    // staging 清空。
    assert!(!tmp.path().join(".skill-staging/demo-skill").exists());

    // P14 lockfile：commit pin + 逐文件 sha256 记账。
    let lock = SkillsLockfile::load(tmp.path());
    let entry = lock.get("demo-skill").expect("lockfile entry recorded");
    assert_eq!(entry.commit, SHA);
    assert_eq!(entry.source, "github:acme/demo-skill");
    assert_eq!(entry.verified_state, "review-required");
    let expected_sha = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(SAFE_SKILL_MD.as_bytes());
        format!("{:x}", h.finalize())
    };
    assert_eq!(
        entry.files.get("SKILL.md").map(String::as_str),
        Some(expected_sha.as_str())
    );

    // origin tracking（GitHub 路径 registry=github, version=短 commit）。
    let origin = installer.get_origin_tracking("demo-skill").unwrap();
    assert_eq!(origin.registry, "github");
    assert_eq!(origin.installed_version, &SHA[..12]);

    // plan 数据面（published_at 来自 commits API 的 commit date）。
    assert!(outcome.plan.published_at.is_some());
    assert!(outcome.plan.age_days.unwrap_or(0) >= 364);
}

#[tokio::test]
async fn test_install_github_explicit_ref_uses_given_ref() {
    let server = MockServer::start().await;
    mount_github(
        &server,
        "acme/pinned",
        SHA,
        &commit_date_days_ago(100),
        &[("SKILL.md", SAFE_SKILL_MD)],
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let installer = make_installer(tmp.path(), &server);

    let outcome = installer.install_github("acme/pinned@v1.2").await.unwrap();
    assert_eq!(outcome.plan.requested_ref, "v1.2");
    assert_eq!(outcome.commit, SHA);
    assert!(tmp.path().join("skills/pinned/SKILL.md").exists());
}

// ---------------------------------------------------------------------------
// P13 审批门
// ---------------------------------------------------------------------------

/// 记录收到的 plan 并拒绝的自定义 gate。
struct RecordingDenyGate {
    seen_plans: Mutex<Vec<InstallPlan>>,
}

#[async_trait::async_trait]
impl crate::install_gate::InstallGate for RecordingDenyGate {
    async fn decide(&self, plan: &InstallPlan) -> InstallDecision {
        self.seen_plans.lock().unwrap().push(plan.clone());
        InstallDecision::Deny {
            reason: "not on allowlist".to_string(),
        }
    }
}

#[tokio::test]
async fn test_install_github_denied_by_gate_leaves_no_trace() {
    let server = MockServer::start().await;
    mount_github(
        &server,
        "acme/gated",
        SHA,
        &commit_date_days_ago(30),
        &[("SKILL.md", SAFE_SKILL_MD)],
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut installer = make_installer(tmp.path(), &server);
    let gate = Arc::new(RecordingDenyGate {
        seen_plans: Mutex::new(Vec::new()),
    });
    installer.set_install_gate(gate.clone());

    let err = installer
        .install_github("acme/gated")
        .await
        .expect_err("deny must fail the install");
    assert!(
        err.to_string().contains("denied by approval gate"),
        "unexpected error: {}",
        err
    );
    assert!(err.to_string().contains("not on allowlist"));

    // gate 收到的 plan 数据完整（文件清单 + 四态 + 安全摘要）。
    let seen = gate.seen_plans.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].slug, "gated");
    assert_eq!(seen[0].files.len(), 1);
    assert_eq!(seen[0].files[0].path, "SKILL.md");
    assert_eq!(seen[0].trust, crate::trust::TrustState::ReviewRequired);
    assert!(!seen[0].security.blocked);
    drop(seen);

    // 全程不落盘：skills/ 与 staging 都没有，lockfile 无记录。
    assert!(!tmp.path().join("skills/gated").exists());
    assert!(!tmp.path().join(".skill-staging/gated").exists());
    assert!(SkillsLockfile::load(tmp.path()).get("gated").is_none());
}

#[tokio::test]
async fn test_install_github_always_allow_gate_installs() {
    let server = MockServer::start().await;
    mount_github(
        &server,
        "acme/allowed",
        SHA,
        &commit_date_days_ago(30),
        &[("SKILL.md", SAFE_SKILL_MD)],
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut installer = make_installer(tmp.path(), &server);
    installer.set_install_gate(Arc::new(AlwaysAllowGate));

    installer.install_github("acme/allowed").await.unwrap();
    assert!(tmp.path().join("skills/allowed/SKILL.md").exists());
}

// ---------------------------------------------------------------------------
// P11 验签
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_install_github_strict_mode_blocks_unsigned() {
    let server = MockServer::start().await;
    mount_github(
        &server,
        "acme/unsigned",
        SHA,
        &commit_date_days_ago(30),
        &[("SKILL.md", SAFE_SKILL_MD)],
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut installer = make_installer(tmp.path(), &server);
    installer.set_allow_unsigned(false);

    let err = installer
        .install_github("acme/unsigned")
        .await
        .expect_err("strict mode must block unsigned");
    assert!(
        err.to_string()
            .contains("blocked by signature verification"),
        "unexpected error: {}",
        err
    );
    assert!(!tmp.path().join("skills/unsigned").exists());
    assert!(!tmp.path().join(".skill-staging/unsigned").exists());
}

#[cfg(feature = "security")]
#[tokio::test]
async fn test_install_github_signed_skill_trusted_state() {
    use nemesis_security::signature::{SignatureVerifier, generate_key_pair};

    let server = MockServer::start().await;

    // 1) 先在别处组装同样内容的目录并签名，取 .signature 清单。
    // （私钥文件必须放签名目录之外——非点开头文件会被算进 manifest。）
    let kp = generate_key_pair().unwrap();
    let key_dir = tempfile::tempdir().unwrap();
    let sign_dir = tempfile::tempdir().unwrap();
    std::fs::write(sign_dir.path().join("SKILL.md"), SAFE_SKILL_MD).unwrap();
    let key_file = key_dir.path().join("key.hex");
    std::fs::write(&key_file, &kp.private_key).unwrap();
    crate::signer::SkillSigner::new()
        .sign_skill(
            &sign_dir.path().to_string_lossy(),
            &key_file.to_string_lossy(),
        )
        .unwrap();
    let signature_json = std::fs::read_to_string(sign_dir.path().join(".signature")).unwrap();

    // 2) 信任库：把签名者公钥登记为 Verified 并落盘。
    let trust_dir = tempfile::tempdir().unwrap();
    let trust_path = trust_dir.path().join("skill_trust.json");
    SignatureVerifier::with_persistence(&trust_path).add_trusted_key(&kp.public_key, "acme");

    // 3) mock 服务端同时提供 SKILL.md 与 .signature。
    mount_github(
        &server,
        "acme/signed",
        SHA,
        &commit_date_days_ago(30),
        &[("SKILL.md", SAFE_SKILL_MD), (".signature", &signature_json)],
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut installer = make_installer(tmp.path(), &server);
    installer.set_trust_store_path(trust_path);

    let outcome = installer.install_github("acme/signed").await.unwrap();
    assert_eq!(outcome.trust, crate::trust::TrustState::Trusted);

    let lock = SkillsLockfile::load(tmp.path());
    let entry = lock.get("signed").unwrap();
    assert_eq!(entry.verified_state, "trusted");
    // .signature 是点开头文件：不进 lockfile 记账。
    assert!(entry.files.contains_key("SKILL.md"));
    assert!(!entry.files.keys().any(|p| p.starts_with('.')));
}

// ---------------------------------------------------------------------------
// P16 版本龄
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_age_policy_warn_allows_install() {
    let server = MockServer::start().await;
    mount_github(
        &server,
        "acme/young",
        SHA,
        &commit_date_days_ago(10),
        &[("SKILL.md", SAFE_SKILL_MD)],
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut installer = make_installer(tmp.path(), &server);
    installer.set_age_policy(30, "warn");

    let outcome = installer.install_github("acme/young").await.unwrap();
    assert!(tmp.path().join("skills/young/SKILL.md").exists());
    let msg = format!(
        "skill is {} days old, minimum required age is 30 days",
        outcome.plan.age_days.unwrap()
    );
    assert!(
        outcome.plan.warnings.iter().any(|w| w.contains(&msg)),
        "warnings: {:?}",
        outcome.plan.warnings
    );
    assert!(outcome.plan.age_decision.starts_with("warn:"));
}

#[tokio::test]
async fn test_age_policy_block_rejects_young() {
    let server = MockServer::start().await;
    mount_github(
        &server,
        "acme/tooyoung",
        SHA,
        &commit_date_days_ago(2),
        &[("SKILL.md", SAFE_SKILL_MD)],
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut installer = make_installer(tmp.path(), &server);
    installer.set_age_policy(30, "block");

    let err = installer
        .install_github("acme/tooyoung")
        .await
        .expect_err("block policy must reject young skill");
    assert!(
        err.to_string().contains("blocked by age policy"),
        "unexpected error: {}",
        err
    );
    assert!(!tmp.path().join("skills/tooyoung").exists());
    assert!(!tmp.path().join(".skill-staging/tooyoung").exists());
}

#[tokio::test]
async fn test_age_check_disabled_by_default() {
    let server = MockServer::start().await;
    mount_github(
        &server,
        "acme/fresh",
        SHA,
        &commit_date_days_ago(0),
        &[("SKILL.md", SAFE_SKILL_MD)],
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let installer = make_installer(tmp.path(), &server);
    // 默认 min_age_days=0：当天发布的技能也照装；plan 照常回报年龄（0 天）。
    let outcome = installer.install_github("acme/fresh").await.unwrap();
    assert_eq!(outcome.plan.age_days, Some(0));
    assert!(tmp.path().join("skills/fresh/SKILL.md").exists());
}

// ---------------------------------------------------------------------------
// registry 路径的 staging / 审批 / lockfile 接线
// ---------------------------------------------------------------------------

/// 写两个文件的测试 registry。
struct InlineRegistry;

#[async_trait::async_trait]
impl SkillRegistry for InlineRegistry {
    fn name(&self) -> &str {
        "inline"
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SkillSearchResult>> {
        Ok(Vec::new())
    }

    async fn get_skill_meta(&self, slug: &str) -> Result<SkillMeta> {
        Ok(SkillMeta {
            slug: slug.to_string(),
            display_name: slug.to_string(),
            summary: "inline".to_string(),
            latest_version: "1.0".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: "inline".to_string(),
            author: String::new(),
            downloads: 0,
            published_at: Some(commit_date_days_ago_as_ts(400)),
        })
    }

    async fn download_and_install(
        &self,
        _slug: &str,
        version: &str,
        target_dir: &str,
    ) -> Result<InstallResult> {
        std::fs::create_dir_all(target_dir).unwrap();
        std::fs::write(format!("{}/SKILL.md", target_dir), SAFE_SKILL_MD).unwrap();
        std::fs::create_dir_all(format!("{}/scripts", target_dir)).unwrap();
        std::fs::write(format!("{}/scripts/run.sh", target_dir), "echo hi\n").unwrap();
        Ok(InstallResult {
            version: version.to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            summary: "ok".to_string(),
        })
    }

    async fn get_skill_content(&self, _slug: &str) -> Result<SkillContent> {
        Err(nemesis_types::error::NemesisError::Other(
            "not implemented".to_string(),
        ))
    }

    async fn browse(
        &self,
        _sort: &BrowseSort,
        _limit: usize,
        _cursor: &str,
    ) -> Result<BrowseResult> {
        Err(nemesis_types::error::NemesisError::Other(
            "not implemented".to_string(),
        ))
    }
}

fn commit_date_days_ago_as_ts(days: i64) -> i64 {
    (chrono::Utc::now() - chrono::Duration::days(days)).timestamp()
}

fn registry_installer(workspace: &std::path::Path) -> SkillInstaller {
    let mut installer = SkillInstaller::new(&workspace.to_string_lossy());
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(InlineRegistry));
    installer.set_registry_manager(manager);
    installer
}

#[tokio::test]
async fn test_registry_install_records_lockfile_and_respects_gate() {
    let tmp = tempfile::tempdir().unwrap();

    // 放行：落位 + lockfile 记账（registry 来源，验签态 review-required）。
    let mut installer = registry_installer(tmp.path());
    installer.set_install_gate(Arc::new(AlwaysAllowGate));
    installer.install("inline", "toolkit", "1.0").await.unwrap();
    assert!(tmp.path().join("skills/toolkit/SKILL.md").exists());
    assert!(tmp.path().join("skills/toolkit/scripts/run.sh").exists());
    assert!(!tmp.path().join(".skill-staging/toolkit").exists());

    let lock = SkillsLockfile::load(tmp.path());
    let entry = lock.get("toolkit").expect("registry install recorded");
    assert_eq!(entry.source, "registry:inline/toolkit");
    assert_eq!(entry.verified_state, "review-required");
    assert_eq!(entry.files.len(), 2);
    assert!(entry.files.contains_key("SKILL.md"));
    assert!(entry.files.contains_key("scripts/run.sh"));

    // 拒绝：第二个技能不落盘、staging 清理、lockfile 不新增。
    let mut installer2 = registry_installer(tmp.path());
    installer2.set_install_gate(Arc::new(AlwaysDenyGate));
    let err = installer2
        .install("inline", "other", "1.0")
        .await
        .expect_err("deny must fail");
    assert!(err.to_string().contains("denied by approval gate"));
    assert!(!tmp.path().join("skills/other").exists());
    assert!(!tmp.path().join(".skill-staging/other").exists());
    assert!(SkillsLockfile::load(tmp.path()).get("other").is_none());
}

// ---------------------------------------------------------------------------
// P14 漂移检测（installer 面）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_verify_drift_reports_modification() {
    let server = MockServer::start().await;
    mount_github(
        &server,
        "acme/drift",
        SHA,
        &commit_date_days_ago(30),
        &[("SKILL.md", SAFE_SKILL_MD)],
    )
    .await;

    let tmp = tempfile::tempdir().unwrap();
    let installer = make_installer(tmp.path(), &server);
    installer.install_github("acme/drift").await.unwrap();

    // 干净。
    let report = installer.verify_skill_drift("drift").unwrap();
    assert!(report.clean, "summary: {}", report.summary());

    // 篡改 SKILL.md 后报 modified。
    std::fs::write(tmp.path().join("skills/drift/SKILL.md"), "# Tampered\n").unwrap();
    let report = installer.verify_skill_drift("drift").unwrap();
    assert!(!report.clean);
    assert_eq!(report.modified_files, vec!["SKILL.md".to_string()]);

    // 未记账技能 = NotFound。
    assert!(installer.verify_skill_drift("nope").is_err());

    // 全量检测覆盖到被篡改的条目。
    let all = installer.verify_all_drift();
    assert!(all.iter().any(|r| r.slug == "drift" && !r.clean));
}

// 卸载路径安全：slug 只能是单一目录名（2026-09-25 WSAPI 接线时加固——
// 旧 web handler 有 resolve_path 围栏，installer 路径必须同强度）。
#[tokio::test]
async fn uninstall_rejects_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let installer = make_installer(tmp.path(), &wiremock::MockServer::start().await);

    for bad in ["../escape", "a/b", r"a\b", "..", ".hidden", ""] {
        let err = installer
            .uninstall(bad)
            .expect_err("traversal must be rejected");
        match err {
            nemesis_types::error::NemesisError::Validation(msg) => {
                assert!(msg.contains("path traversal denied"), "msg: {msg}");
            }
            other => panic!("expected Validation error for {bad:?}, got {other:?}"),
        }
    }

    // 未命中围栏的正常缺失技能仍是 NotFound（不是 Validation）。
    let err = installer.uninstall("ghost").unwrap_err();
    assert!(
        matches!(err, nemesis_types::error::NemesisError::NotFound(_)),
        "err: {err:?}"
    );
}
