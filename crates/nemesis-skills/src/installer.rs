//! Skill installer - downloads, validates, and installs skills.
//!
//! Handles:
//! - GitHub-based skill download（legacy 单文件 + P12 pinned 全树两路）
//! - 安装门单一漏斗（P11-P16）：验签 → pin+完整性 → 安全扫描 → 版本龄 →
//!   审批卡 → lockfile 记账，任何一环 block 即终止并给可读理由
//! - Pre-install lint + quality checks
//! - Post-install validation
//! - Origin tracking metadata
//! - Registry-based installation

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tracing::{debug, warn};

use nemesis_types::error::{NemesisError, Result};
use serde::{Deserialize, Serialize};

use crate::install_gate::{InstallDecision, InstallPlan, PlanFile, SharedInstallGate};
use crate::lockfile::{LockedSkill, SkillsLockfile};
use crate::security_check::{check_skill_security, check_skill_security_dir};
use crate::trust::{TrustState, VerificationOutcome};
use crate::types::{AvailableSkill, InstallResult, SecurityCheckResult, SkillOrigin};

/// 浏览器 UA（GitHub API 要求 User-Agent，测试服务器无所谓）。
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// GitHub 安装结果（P12 pinned 路径）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallOutcome {
    /// 技能名（目标目录名）。
    pub slug: String,
    /// 来源（`github:<owner>/<repo>`）。
    pub source: String,
    /// 锁定的 commit SHA。
    pub commit: String,
    /// 信任四态。
    pub trust: TrustState,
    /// 实际安装文件数。
    pub files_installed: usize,
    /// 完整审批卡。
    pub plan: InstallPlan,
}

/// Skill installer that manages downloading and installing skills from registries.
pub struct SkillInstaller {
    workspace: PathBuf,
    registry_manager: Option<crate::registry::RegistryManager>,
    github_base_url: String,
    /// GitHub API base（P12 commit 解析；测试 seam 可换 wiremock）。
    github_api_url: String,
    /// `skills.allow_unsigned`（P11；默认 true 兼容存量，false = strict 拒绝无签名）。
    allow_unsigned: bool,
    /// `skills.min_age_days`（P16；默认 0 = 关）。
    min_age_days: i64,
    /// `skills.min_age_policy`（P16；"warn" | "block"）。
    min_age_policy: String,
    /// TrustStore 文件路径（P11；默认 `<workspace>/config/skill_trust.json`）。
    trust_store_path: Option<PathBuf>,
    /// 装前审批门（P13；None = AlwaysAllow，等价 `skills.install_approval=false`）。
    install_gate: Mutex<Option<SharedInstallGate>>,
    last_security_check: Mutex<Option<SecurityCheckResult>>,
}

impl SkillInstaller {
    /// Create a new installer for the given workspace directory.
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            registry_manager: None,
            github_base_url: "https://raw.githubusercontent.com".to_string(),
            github_api_url: "https://api.github.com".to_string(),
            allow_unsigned: true,
            min_age_days: 0,
            min_age_policy: "warn".to_string(),
            trust_store_path: None,
            install_gate: Mutex::new(None),
            last_security_check: Mutex::new(None),
        }
    }

    /// Set the registry manager for advanced installation features.
    pub fn set_registry_manager(&mut self, manager: crate::registry::RegistryManager) {
        self.registry_manager = Some(manager);
    }

    /// Set the GitHub base URL (for testing).
    pub fn set_github_base_url(&mut self, url: &str) {
        self.github_base_url = url.to_string();
    }

    /// Set the GitHub API base URL (for testing; P12 commit 解析用).
    pub fn set_github_api_url(&mut self, url: &str) {
        self.github_api_url = url.to_string();
    }

    /// Set `skills.allow_unsigned`（P11；缺省 true）。
    pub fn set_allow_unsigned(&mut self, allow: bool) {
        self.allow_unsigned = allow;
    }

    /// Set `skills.min_age_days` / `skills.min_age_policy`（P16；policy: warn|block）。
    pub fn set_age_policy(&mut self, min_age_days: i64, policy: &str) {
        self.min_age_days = min_age_days;
        self.min_age_policy = if policy.eq_ignore_ascii_case("block") {
            "block".to_string()
        } else {
            "warn".to_string()
        };
    }

    /// Set the TrustStore file path (P11; 缺省 `<workspace>/config/skill_trust.json`).
    pub fn set_trust_store_path(&mut self, path: PathBuf) {
        self.trust_store_path = Some(path);
    }

    /// Set the install approval gate (P13; None = AlwaysAllow).
    pub fn set_install_gate(&mut self, gate: SharedInstallGate) {
        *self.install_gate.lock().unwrap() = Some(gate);
    }

    /// Borrow the effective gate: configured one or AlwaysAllow fallback.
    fn gate_or_default(&self) -> SharedInstallGate {
        self.install_gate
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(crate::install_gate::AlwaysAllowGate))
    }

    /// Check whether a registry manager is configured.
    pub fn has_registry_manager(&self) -> bool {
        self.registry_manager.is_some()
    }

    /// Get a reference to the registry manager, if configured.
    ///
    /// Returns `Some(&RegistryManager)` if a registry manager has been set,
    /// or `None` otherwise.
    pub fn get_registry_manager(&self) -> Option<&crate::registry::RegistryManager> {
        self.registry_manager.as_ref()
    }

    /// Get the result from the most recent security check.
    pub fn last_security_check(&self) -> Option<SecurityCheckResult> {
        self.last_security_check.lock().unwrap().clone()
    }

    /// Store the result from the most recent security check.
    ///
    /// Mirrors Go `setLastSecurityCheck`.
    #[allow(dead_code)]
    fn set_last_security_check(&self, result: SecurityCheckResult) {
        let mut last = self.last_security_check.lock().unwrap();
        *last = Some(result);
    }

    // ==================== WS4 供应链安装门（P11-P16） ====================
    //
    // 漏斗顺序（任何一环 block 即终止并给可读理由）：
    //   验签(P11) → pin+完整性(P12) → 安全扫描(P15) → 版本龄(P16)
    //   → 审批卡(P13) → lockfile 记账(P14)
    // 审批通过前内容只落在 staging（`<workspace>/.skill-staging/<slug>`，
    // 不放 skills/ 下——loader 扫描不跳过点开头目录，放里面会被当成技能加载）。

    /// staging 目录（审批通过前下载暂存）。
    fn staging_dir(&self, slug: &str) -> PathBuf {
        self.workspace.join(".skill-staging").join(slug)
    }

    /// 解析 `owner/repo[@ref]` → (repo, requested_ref)。
    /// ref 允许 40 位 sha / tag / 分支短名；禁 `..`、`/`、`\` 防路径注入。
    fn parse_github_ref(input: &str) -> Result<(String, String)> {
        let (repo, requested) = match input.split_once('@') {
            Some((r, reference)) => (r, reference),
            None => (input, ""),
        };
        let parts: Vec<&str> = repo.split('/').collect();
        if parts.len() != 2
            || parts[0].is_empty()
            || parts[1].is_empty()
            || parts.iter().any(|p| p.contains(".."))
            // 空白非法（slug 带空格的脏 ref 在任何网络语句前就地拒绝——
            // 离线可测的 poison 终局）。
            || input.contains(char::is_whitespace)
        {
            return Err(NemesisError::Validation(format!(
                "invalid github repo '{}': expected 'owner/repo[@ref]'",
                input
            )));
        }
        if requested.contains("..") || requested.contains('/') || requested.contains('\\') {
            return Err(NemesisError::Validation(format!(
                "invalid ref '{}': '/' and '..' are not allowed",
                requested
            )));
        }
        Ok((repo.to_string(), requested.to_string()))
    }

    /// P12：GitHub commits API 统一解析 ref（sha/tag/branch/缺省 HEAD）→ (sha, commit date)。
    async fn resolve_commit(
        client: &reqwest::Client,
        api_base: &str,
        repo: &str,
        requested_ref: &str,
    ) -> Result<(String, Option<i64>)> {
        let reference = if requested_ref.is_empty() {
            "HEAD"
        } else {
            requested_ref
        };
        let url = format!("{}/repos/{}/commits/{}", api_base, repo, reference);
        let response = client
            .get(&url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to resolve commit: {}", e)))?;
        if !response.status().is_success() {
            return Err(NemesisError::NotFound(format!(
                "failed to resolve ref '{}' for '{}': HTTP {}",
                reference,
                repo,
                response.status()
            )));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to parse commit response: {}", e)))?;
        let sha = body["sha"].as_str().unwrap_or_default().to_string();
        if sha.is_empty() {
            return Err(NemesisError::Other(format!(
                "commit response for '{}' missing sha",
                reference
            )));
        }
        // commit.committer.date（RFC3339）→ unix 秒；缺失容忍（age 检查按未知放行）。
        let published_at = body["commit"]["committer"]["date"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp());
        Ok((sha, published_at))
    }

    /// P11：对目录做验签 → `VerificationOutcome`（四态归约交给 trust 层）。
    #[cfg(feature = "security")]
    fn verify_signature_state(&self, dir: &Path) -> VerificationOutcome {
        if !dir.join(".signature").exists() {
            return VerificationOutcome::default();
        }
        let config_path = self
            .trust_store_path
            .clone()
            .unwrap_or_else(|| self.workspace.join("config").join("skill_trust.json"));
        let signer = crate::signer::SkillSigner::with_persistence(&config_path.to_string_lossy());
        match signer.verify_skill(&dir.to_string_lossy()) {
            Ok(v) => VerificationOutcome {
                signed: true,
                valid: Some(v.valid),
                trust_level: Some(v.trust_level.to_string()),
                public_key: v.public_key,
                error: v.error,
            },
            Err(e) => VerificationOutcome {
                signed: true,
                valid: Some(false),
                trust_level: None,
                public_key: String::new(),
                error: e.to_string(),
            },
        }
    }

    /// 无 security feature 时验签不上膛：诚实按未签名处理（由 allow_unsigned 裁决）。
    #[cfg(not(feature = "security"))]
    fn verify_signature_state(&self, _dir: &Path) -> VerificationOutcome {
        VerificationOutcome::default()
    }

    /// P11：按验签结论出四态；blocked 时附带可读理由并返回 Err（调用方清理 staging）。
    fn enforce_trust(&self, outcome: &VerificationOutcome) -> Result<TrustState> {
        let trust = outcome.trust_state(self.allow_unsigned);
        if trust.installable() {
            return Ok(trust);
        }
        let detail = if outcome.error.is_empty() {
            "signature missing, tampered, or key untrusted/revoked"
        } else {
            &outcome.error
        };
        Err(NemesisError::Security(format!(
            "blocked by signature verification ({}): {}",
            trust, detail
        )))
    }

    /// P16：版本龄检查。发布时间未知 → (None, "")。
    /// min_age_days=0（默认）= 关，只回报年龄不裁决；block 策略返回 Err，
    /// warn 策略把文案追加进 warnings。
    fn check_age(
        &self,
        published_at: Option<i64>,
        warnings: &mut Vec<String>,
    ) -> Result<(Option<i64>, String)> {
        let Some(published) = published_at else {
            return Ok((None, String::new()));
        };
        let now = chrono::Utc::now().timestamp();
        let age_days = (now - published) / 86_400;
        if self.min_age_days <= 0 || age_days >= self.min_age_days {
            return Ok((Some(age_days), "ok".to_string()));
        }
        let msg = format!(
            "skill is {} days old, minimum required age is {} days",
            age_days, self.min_age_days
        );
        if self.min_age_policy == "block" {
            return Err(NemesisError::Security(format!(
                "blocked by age policy: {}",
                msg
            )));
        }
        warnings.push(msg.clone());
        Ok((Some(age_days), format!("warn: {}", msg)))
    }

    /// P12-P16：GitHub 安装 plan 阶段——解析 pin、下载到 staging、逐文件 sha256、
    /// 验签、安全扫描、版本龄；产出审批卡。block 环节清理 staging 后报错。
    pub async fn plan_github_install(&self, repo_ref: &str) -> Result<InstallPlan> {
        let (repo, requested_ref) = Self::parse_github_ref(repo_ref)?;
        let slug = repo.split('/').next_back().unwrap_or("skill").to_string();
        let skill_dir = self.workspace.join("skills").join(&slug);
        if skill_dir.exists() {
            return Err(NemesisError::Validation(format!(
                "skill '{}' already exists",
                slug
            )));
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| NemesisError::Other(format!("failed to create HTTP client: {}", e)))?;

        // P12：pin 解析（无 @ref 时取默认分支当前 commit 记账，重装按 pin 精确取）。
        let (commit, published_at) =
            Self::resolve_commit(&client, &self.github_api_url, &repo, &requested_ref).await?;

        // 下载到 staging（SHA 同时充当 trees API 与 raw URL 的 ref）。
        let staging = self.staging_dir(&slug);
        let _ = std::fs::remove_dir_all(&staging);
        if let Err(e) = crate::github_tree::download_skill_tree_from_github(
            &client,
            &self.github_api_url,
            &self.github_base_url,
            &repo,
            &commit,
            "", // 仓库根为技能根（空前缀匹配全部 blob）
            &staging.to_string_lossy(),
            0,
        )
        .await
        {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }

        // P11：验签（security_check 前置）。
        let outcome = self.verify_signature_state(&staging);
        let trust = match self.enforce_trust(&outcome) {
            Ok(t) => t,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        let mut trust_detail = outcome.error.clone();
        if trust_detail.is_empty() && !outcome.public_key.is_empty() {
            trust_detail = format!(
                "signed by {}…",
                &outcome.public_key[..outcome.public_key.len().min(16)]
            );
        }
        if !outcome.signed {
            trust_detail = "unsigned".to_string();
        }

        // P15：SKILL.md 安全扫描（GitHub 路径缺 SKILL.md = 诚实报错，不是静默装空壳）。
        // M5：lint 面扩展到 staging 整目录可执行面（SKILL.md + 辅助 .md +
        // 脚本形态）——藏在 scripts/ 里的恶意载荷不再绕过检查。
        let skill_md_path = staging.join("SKILL.md");
        if !skill_md_path.exists() {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(NemesisError::Validation(format!(
                "skill '{}' has no SKILL.md at repo root",
                slug
            )));
        }
        let content = std::fs::read_to_string(&skill_md_path).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staging);
            NemesisError::Io(e)
        })?;
        let security = check_skill_security_dir(&staging, &content, &slug, "");
        self.set_last_security_check(security.clone());
        if security.blocked {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(NemesisError::Security(format!(
                "skill '{}' blocked by security check: {}",
                slug, security.block_reason
            )));
        }

        // P16：版本龄。
        let mut warnings = Vec::new();
        if !security.lint_result.passed {
            warnings.push(format!(
                "security lint: {} warnings (score {:.0}/100)",
                security.lint_result.warnings.len(),
                security.lint_result.score * 100.0
            ));
        }
        let (age_days, age_decision) = match self.check_age(published_at, &mut warnings) {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        };

        // 逐文件 sha256（跳点开头文件，与 lockfile/签名 manifest 同口径）。
        let files_map = match SkillsLockfile::compute_dir_hashes(&staging) {
            Ok(h) => h,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        let mut files = Vec::new();
        let mut total_bytes = 0u64;
        for (path, sha) in &files_map {
            let bytes = std::fs::metadata(staging.join(path))
                .map(|m| m.len())
                .unwrap_or(0);
            total_bytes += bytes;
            files.push(PlanFile {
                path: path.clone(),
                sha256: sha.clone(),
                bytes,
            });
        }

        Ok(InstallPlan {
            slug,
            source: format!("github:{}", repo),
            requested_ref,
            commit,
            published_at,
            age_days,
            age_decision,
            files,
            total_bytes,
            trust,
            trust_detail,
            security,
            warnings,
        })
    }

    /// P13/P14：commit 阶段——gate 裁决；approve 则 staging rename 落位 + lockfile 记账。
    pub async fn commit_install(&self, plan: InstallPlan) -> Result<InstallOutcome> {
        let staging = self.staging_dir(&plan.slug);
        let gate = self.gate_or_default();
        match gate.decide(&plan).await {
            InstallDecision::Approve => {}
            InstallDecision::Deny { reason } => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(NemesisError::Security(format!(
                    "install denied by approval gate ({}): {}",
                    gate.name(),
                    if reason.is_empty() {
                        "no reason given"
                    } else {
                        &reason
                    }
                )));
            }
        }

        let target = self.workspace.join("skills").join(&plan.slug);
        // plan 阶段已检查不存在；二次防御（并发竞态兜底）。
        if target.exists() {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(NemesisError::Validation(format!(
                "skill '{}' already exists",
                plan.slug
            )));
        }
        // skills/ 父目录可能尚不存在（Windows rename 不自动建父目录）。
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(NemesisError::Io)?;
        }
        std::fs::rename(&staging, &target).map_err(|e| {
            let _ = std::fs::remove_dir_all(&staging);
            NemesisError::Io(e)
        })?;

        // origin tracking（GitHub 路径 registry 名固定 "github"，version = 短 commit）。
        let short_commit: String = plan.commit.chars().take(12).collect();
        if let Err(e) = self.write_origin_tracking(
            &target.to_string_lossy(),
            "github",
            &plan.slug,
            &if short_commit.is_empty() {
                "unknown".to_string()
            } else {
                short_commit
            },
        ) {
            warn!("failed to write origin tracking: {}", e);
        }

        // P14：lockfile 记账。
        self.record_lockfile_entry(&LockedSkill {
            slug: plan.slug.clone(),
            source: plan.source.clone(),
            commit: plan.commit.clone(),
            files: plan
                .files
                .iter()
                .map(|f| (f.path.clone(), f.sha256.clone()))
                .collect(),
            installed_at: chrono::Local::now().timestamp(),
            verified_state: plan.trust.as_str().to_string(),
        });

        debug!(
            "Skill '{}' installed from {} @ {} ({} files, trust={})",
            plan.slug,
            plan.source,
            &plan.commit[..plan.commit.len().min(12)],
            plan.files.len(),
            plan.trust
        );

        let files_installed = plan.files.len();
        let trust = plan.trust;
        Ok(InstallOutcome {
            slug: plan.slug.clone(),
            source: plan.source.clone(),
            commit: plan.commit.clone(),
            trust,
            files_installed,
            plan,
        })
    }

    /// P14：写一条 lockfile 记账（load→record→save；失败只 warn——记账面非执行面）。
    fn record_lockfile_entry(&self, entry: &LockedSkill) {
        let mut lock = SkillsLockfile::load(&self.workspace);
        lock.record(entry.clone());
        if let Err(e) = lock.save(&self.workspace) {
            warn!("failed to save skills.lock.json: {}", e);
        }
    }

    /// 一步式 GitHub 安装（plan + commit，用自方 gate）。
    pub async fn install_github(&self, repo_ref: &str) -> Result<InstallOutcome> {
        let plan = self.plan_github_install(repo_ref).await?;
        self.commit_install(plan).await
    }

    /// P14：单技能漂移检测（未记账 = NotFound）。
    pub fn verify_skill_drift(&self, slug: &str) -> Result<crate::lockfile::DriftReport> {
        let lock = SkillsLockfile::load(&self.workspace);
        if lock.get(slug).is_none() {
            return Err(NemesisError::NotFound(format!(
                "skill '{}' not recorded in skills.lock.json",
                slug
            )));
        }
        Ok(lock.verify_drift(&self.workspace, slug))
    }

    /// P14：全量漂移检测（按 lockfile 记账逐条检测）。
    pub fn verify_all_drift(&self) -> Vec<crate::lockfile::DriftReport> {
        let lock = SkillsLockfile::load(&self.workspace);
        lock.skills
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|slug| lock.verify_drift(&self.workspace, &slug))
            .collect()
    }

    /// Install a skill from a named registry (Go-compatible signature).
    ///
    /// Like `install` but returns only success/failure without the detailed
    /// `InstallResult`. This matches the Go `InstallFromRegistry` error-only
    /// return convention.
    pub async fn install_from_registry(
        &self,
        registry_name: &str,
        slug: &str,
        version: &str,
    ) -> Result<()> {
        self.install(registry_name, slug, version).await?;
        Ok(())
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Check whether a registry with the given name exists.
    pub fn has_registry(&self, name: &str) -> bool {
        self.registry_manager
            .as_ref()
            .map(|rm| rm.get_registry(name).is_some())
            .unwrap_or(false)
    }

    /// Search all configured registries for skills matching the query.
    ///
    /// Results are returned grouped by registry source.
    pub async fn search_all(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::types::RegistrySearchResult>> {
        let manager = self.registry_manager.as_ref().ok_or_else(|| {
            NemesisError::Validation("registry manager not configured".to_string())
        })?;
        manager.search_all(query, limit).await
    }

    /// Flatten grouped search results into a single list.
    ///
    /// Utility for callers that don't need per-registry grouping.
    pub fn flatten_search_results(
        grouped: &[crate::types::RegistrySearchResult],
    ) -> Vec<crate::types::SkillSearchResult> {
        grouped.iter().flat_map(|g| g.results.clone()).collect()
    }

    /// Install a skill from a named registry.
    ///
    /// WS4 漏斗（registry 路径）：下载到 staging → malware 闸 → 验签(P11) →
    /// 安全扫描(P15) → 版本龄(P16, lenient) → 审批门(P13) → 落位 + lockfile(P14)。
    /// Mirrors Go `InstallFromRegistry`（对外签名/返回不变）。
    pub async fn install(
        &self,
        registry_name: &str,
        slug: &str,
        version: &str,
    ) -> Result<InstallResult> {
        let manager = self.registry_manager.as_ref().ok_or_else(|| {
            NemesisError::Validation("registry manager not configured".to_string())
        })?;

        let skill_dir = self.workspace.join("skills").join(slug);

        if skill_dir.exists() {
            return Err(NemesisError::Validation(format!(
                "skill '{}' already exists",
                slug
            )));
        }

        let registry = manager.get_registry(registry_name).ok_or_else(|| {
            NemesisError::NotFound(format!("registry '{}' not found", registry_name))
        })?;

        // 下载到 staging（审批通过前不落 skills/）。
        let staging = self.staging_dir(slug);
        let _ = std::fs::remove_dir_all(&staging);
        let dl_result = match registry
            .download_and_install(slug, version, &staging.to_string_lossy())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
        };

        // Check if the download result indicates malware or suspicious content.
        if dl_result.is_malware_blocked {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(NemesisError::Security(format!(
                "skill '{}' was blocked as malware",
                slug
            )));
        }

        if dl_result.is_suspicious {
            warn!("Warning: Skill '{}' is marked as suspicious", slug);
        }

        // P11：验签（security_check 前置；无 .signature 时由 allow_unsigned 裁决）。
        // L1（2026-09-26 复查）：拒绝时先清 staging 再上抛——审批未过的内容
        // 不得残留（plan_github_install 同场景本就有清理，此处对齐）。
        let outcome = self.verify_signature_state(&staging);
        if let Err(e) = self.enforce_trust(&outcome) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }

        // P15：SKILL.md 安全检查（缺 SKILL.md = 跳过检查、不记录——历史契约）。
        // M5：lint 面扩展到 staging 整目录可执行面（同 plan_github_install）。
        let mut security_result = SecurityCheckResult {
            lint_result: crate::lint::LintResult {
                skill_name: String::new(),
                passed: true,
                score: 1.0,
                warnings: Vec::new(),
            },
            quality_score: None,
            blocked: false,
            block_reason: String::new(),
        };
        let skill_md_path = staging.join("SKILL.md");
        if skill_md_path.exists()
            && let Ok(content) = std::fs::read_to_string(&skill_md_path)
        {
            let check_result = check_skill_security_dir(&staging, &content, slug, "");
            security_result = check_result.clone();
            {
                let mut last = self.last_security_check.lock().unwrap();
                *last = Some(check_result.clone());
            }

            if check_result.blocked {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(NemesisError::Security(format!(
                    "skill '{}' blocked by security check: {}",
                    slug, check_result.block_reason
                )));
            }

            if !check_result.lint_result.passed {
                warn!(
                    "Security warnings for '{}' (score: {:.0}/100, {} issues)",
                    slug,
                    check_result.lint_result.score * 100.0,
                    check_result.lint_result.warnings.len()
                );
            }

            if let Some(ref quality) = check_result.quality_score {
                debug!("Quality score for '{}': {:.0}/100", slug, quality.overall);
            }
        }

        // P16：版本龄（registry 元数据缺失/查询失败 lenient 放行）。
        let mut age_warnings = Vec::new();
        let published_at = match registry.get_skill_meta(slug).await {
            Ok(meta) => meta.published_at,
            Err(_) => None,
        };
        let (reg_age_days, reg_age_decision) = self.check_age(published_at, &mut age_warnings)?;

        // P13：审批门（缺省 AlwaysAllowGate = install_approval 关闭语义）。
        // 逐文件 sha256（staging 不存在的 registry 实现按空清单处理——历史契约：
        // StubRegistryProvider 不写任何文件也要能装成功）。
        let files_map = if staging.exists() {
            match SkillsLockfile::compute_dir_hashes(&staging) {
                Ok(h) => h,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(e);
                }
            }
        } else {
            std::collections::BTreeMap::new()
        };
        let mut total_bytes = 0u64;
        let mut plan_files = Vec::new();
        for (path, sha) in &files_map {
            let bytes = std::fs::metadata(staging.join(path))
                .map(|m| m.len())
                .unwrap_or(0);
            total_bytes += bytes;
            plan_files.push(crate::install_gate::PlanFile {
                path: path.clone(),
                sha256: sha.clone(),
                bytes,
            });
        }
        let plan = InstallPlan {
            slug: slug.to_string(),
            source: format!("registry:{}/{}", registry_name, slug),
            requested_ref: version.to_string(),
            commit: String::new(),
            published_at,
            age_days: reg_age_days,
            age_decision: reg_age_decision,
            files: plan_files,
            total_bytes,
            trust: outcome.trust_state(self.allow_unsigned),
            trust_detail: if outcome.signed {
                outcome.error.clone()
            } else {
                "unsigned".to_string()
            },
            security: security_result,
            warnings: age_warnings,
        };
        let gate = self.gate_or_default();
        if let InstallDecision::Deny { reason } = gate.decide(&plan).await {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(NemesisError::Security(format!(
                "install denied by approval gate ({}): {}",
                gate.name(),
                if reason.is_empty() {
                    "no reason given"
                } else {
                    &reason
                }
            )));
        }

        // 落位 + P14 lockfile 记账（skills/ 父目录可能尚不存在；
        // staging 不存在 = registry 没写任何文件——建空目录落位，保持历史行为）。
        if let Some(parent) = skill_dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                let _ = std::fs::remove_dir_all(&staging);
                NemesisError::Io(e)
            })?;
        }
        if staging.exists() {
            if let Err(e) = std::fs::rename(&staging, &skill_dir) {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(NemesisError::Io(e));
            }
        } else {
            std::fs::create_dir_all(&skill_dir).map_err(NemesisError::Io)?;
        }

        // Write origin tracking.
        if let Err(e) = self.write_origin_tracking(
            &skill_dir.to_string_lossy(),
            registry_name,
            slug,
            &dl_result.version,
        ) {
            warn!("failed to write origin tracking: {}", e);
        }

        self.record_lockfile_entry(&LockedSkill {
            slug: slug.to_string(),
            source: format!("registry:{}/{}", registry_name, slug),
            commit: String::new(),
            files: files_map,
            installed_at: chrono::Local::now().timestamp(),
            verified_state: plan.trust.as_str().to_string(),
        });

        debug!(
            "Skill '{}' (version {}) installed from registry '{}'",
            slug, dl_result.version, registry_name
        );

        Ok(InstallResult {
            version: dl_result.version,
            is_malware_blocked: dl_result.is_malware_blocked,
            is_suspicious: dl_result.is_suspicious,
            summary: dl_result.summary,
        })
    }

    /// Install a skill from a GitHub repository.
    ///
    /// Downloads the SKILL.md file from the repository's main branch,
    /// runs security checks, and installs to the workspace skills directory.
    pub async fn install_from_github(&self, repo: &str) -> Result<()> {
        let skill_name = Path::new(repo)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| repo.to_string());

        let skill_dir = self.workspace.join("skills").join(&skill_name);

        if skill_dir.exists() {
            return Err(NemesisError::Validation(format!(
                "skill '{}' already exists",
                skill_name
            )));
        }

        let url = format!("{}/{}/main/SKILL.md", self.github_base_url, repo);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| NemesisError::Other(format!("failed to create HTTP client: {}", e)))?;

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to fetch skill: {}", e)))?;

        if response.status() != reqwest::StatusCode::OK {
            return Err(NemesisError::Other(format!(
                "failed to fetch skill: HTTP {}",
                response.status()
            )));
        }

        let body = response
            .text()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to read response: {}", e)))?;

        // Create skill directory.
        std::fs::create_dir_all(&skill_dir).map_err(NemesisError::Io)?;

        // Write SKILL.md atomically.
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(&skill_path, &body).map_err(|e| {
            // Clean up on failure.
            let _ = std::fs::remove_dir_all(&skill_dir);
            NemesisError::Io(e)
        })?;

        // Security check.
        let check_result = check_skill_security(&body, &skill_name, "");
        {
            let mut last = self.last_security_check.lock().unwrap();
            *last = Some(check_result.clone());
        }

        if check_result.blocked {
            let _ = std::fs::remove_dir_all(&skill_dir);
            return Err(NemesisError::Security(format!(
                "skill '{}' blocked by security check: {}",
                skill_name, check_result.block_reason
            )));
        }

        if !check_result.lint_result.warnings.is_empty() {
            warn!(
                "skill has security warnings (score: {:.2}, {} issues)",
                check_result.lint_result.score,
                check_result.lint_result.warnings.len()
            );
        }

        if let Some(ref quality) = check_result.quality_score
            && quality.overall < 40.0
        {
            warn!(
                "skill has low quality score (score: {:.0}/100)",
                quality.overall
            );
        }

        debug!("Installed skill '{}' from GitHub", skill_name);
        Ok(())
    }

    /// Uninstall a skill by name.
    pub fn uninstall(&self, skill_name: &str) -> Result<()> {
        // 路径安全：slug 只能是单一目录名。拒绝路径分隔符 / `..` / 点开头
        // （防止 `../../` 越出 workspace/skills 边界 remove_dir_all 任意目录）。
        if skill_name.is_empty()
            || skill_name.contains('/')
            || skill_name.contains('\\')
            || skill_name.contains("..")
            || skill_name.starts_with('.')
        {
            return Err(NemesisError::Validation(format!(
                "path traversal denied: invalid skill name '{}'",
                skill_name
            )));
        }
        let skill_dir = self.workspace.join("skills").join(skill_name);

        if !skill_dir.exists() {
            return Err(NemesisError::NotFound(format!(
                "skill '{}' not found",
                skill_name
            )));
        }

        std::fs::remove_dir_all(&skill_dir).map_err(NemesisError::Io)?;

        // P14：lockfile 同步移除（失败只 warn——目录已删，记账滞后可被 verify 检出）。
        let mut lock = SkillsLockfile::load(&self.workspace);
        if lock.remove(skill_name)
            && let Err(e) = lock.save(&self.workspace)
        {
            warn!("failed to update skills.lock.json after uninstall: {}", e);
        }

        debug!("Uninstalled skill '{}'", skill_name);
        Ok(())
    }

    /// List available skills from configured registries.
    pub async fn list_available_skills(&self) -> Result<Vec<AvailableSkill>> {
        // If registry manager is configured, use it for better results
        if let Some(manager) = &self.registry_manager {
            return self.list_available_skills_from_registry(manager).await;
        }

        // Fallback to original GitHub implementation
        self.list_available_skills_from_github().await
    }

    /// Search across all configured registries.
    ///
    /// Results are returned grouped by registry source.
    /// Alias for `search_all` matching the Go API naming.
    pub async fn search_registries(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::types::RegistrySearchResult>> {
        self.search_all(query, limit).await
    }

    /// List available skills using the registry manager.
    ///
    /// Uses `search_all` to get grouped results, then flattens them,
    /// matching the Go `listAvailableSkillsFromRegistry` implementation.
    async fn list_available_skills_from_registry(
        &self,
        manager: &crate::registry::RegistryManager,
    ) -> Result<Vec<AvailableSkill>> {
        let grouped = manager.search_all("", 100).await.unwrap_or_default();

        // Flatten grouped results into a single list of skills.
        let all_results = SkillInstaller::flatten_search_results(&grouped);

        Ok(all_results
            .into_iter()
            .map(|r| AvailableSkill {
                name: r.slug,
                repository: String::new(),
                description: r.summary,
                author: String::new(),
                tags: vec![r.registry_name],
            })
            .collect())
    }

    /// List available skills by fetching from GitHub skills repository.
    ///
    /// Fetches the skills.json index file from the default GitHub repository.
    async fn list_available_skills_from_github(&self) -> Result<Vec<AvailableSkill>> {
        let url = "https://raw.githubusercontent.com/276793422/nemesisbot-skills/main/skills.json";

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| NemesisError::Other(format!("failed to create HTTP client: {}", e)))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to fetch skills list: {}", e)))?;

        if response.status() != reqwest::StatusCode::OK {
            return Err(NemesisError::Other(format!(
                "failed to fetch skills list: HTTP {}",
                response.status()
            )));
        }

        let body = response
            .text()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to read response: {}", e)))?;

        let skills: Vec<AvailableSkill> = serde_json::from_str(&body)
            .map_err(|e| NemesisError::Other(format!("failed to parse skills list: {}", e)))?;

        Ok(skills)
    }

    /// Write origin tracking metadata to the skill directory.
    fn write_origin_tracking(
        &self,
        skill_dir: &str,
        registry_name: &str,
        slug: &str,
        version: &str,
    ) -> Result<()> {
        let origin = SkillOrigin {
            version: 1,
            registry: registry_name.to_string(),
            slug: slug.to_string(),
            installed_version: version.to_string(),
            installed_at: chrono::Local::now().timestamp(),
        };

        let data = serde_json::to_string_pretty(&origin).map_err(NemesisError::Serialization)?;

        let origin_path = Path::new(skill_dir).join(".skill-origin.json");
        std::fs::write(&origin_path, data).map_err(NemesisError::Io)?;

        debug!(
            "Wrote origin tracking for '{}' from '{}' version '{}'",
            slug, registry_name, version
        );
        Ok(())
    }

    /// Read origin tracking metadata for a skill.
    pub fn get_origin_tracking(&self, skill_name: &str) -> Result<SkillOrigin> {
        let origin_path = self
            .workspace
            .join("skills")
            .join(skill_name)
            .join(".skill-origin.json");

        if !origin_path.exists() {
            return Err(NemesisError::NotFound(format!(
                "origin file not found for skill '{}'",
                skill_name
            )));
        }

        let data = std::fs::read_to_string(&origin_path).map_err(NemesisError::Io)?;
        let origin: SkillOrigin =
            serde_json::from_str(&data).map_err(NemesisError::Serialization)?;

        Ok(origin)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod w4b_tests;
#[cfg(test)]
mod ws4_tests;
