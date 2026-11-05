//! 补测：SkillInstaller 的安全拦截流。
//!
//! 覆盖面：
//! - `install()`：经真实写文件的 registry（FileWritingRegistry）验证
//!   malware_blocked 清理、security-check 拦截清理、suspicious 仅警告、
//!   无 SKILL.md 跳过检查、下载错误传播、origin tracking 落盘与回读、
//!   set/last security check roundtrip、lint-not-passed 警告、
//!   registry 搜索结果映射
//!
//! 历史注记：本文件原有 8 个 `install_from_github`
//! （legacy 单文件 main 分支路径）wiremock 测试随方法删除——该方法绕过
//! 安装漏斗（验签/pin/审批卡/lockfile），零生产消费方，属旁路
//! 死 API；GitHub 安装统一走 `install_github`（plan+commit 完整漏斗）。

use super::*;
use std::sync::Arc;

/// 干净内容：过 security_check 不产生拦截。
const CLEAN_CONTENT: &str = "# Safe Skill\nThis skill does safe things like reading files.";

/// 危险内容：rm -rf 触发 Destructive 类别（security_check/tests.rs 已钉行为）。
const DESTRUCTIVE_CONTENT: &str = "Run this: rm -rf / && sudo chmod 777 /everything";

/// 测试用 registry：真实向 target_dir 写 SKILL.md，返回可配置标志。
struct FileWritingRegistry {
    skill_md_content: String,
    malware_blocked: bool,
    suspicious: bool,
    fail_download: bool,
    write_skill_md: bool,
}

impl FileWritingRegistry {
    fn new(skill_md: &str) -> Self {
        Self {
            skill_md_content: skill_md.to_string(),
            malware_blocked: false,
            suspicious: false,
            fail_download: false,
            write_skill_md: true,
        }
    }

    fn with_flags(mut self, malware_blocked: bool, suspicious: bool) -> Self {
        self.malware_blocked = malware_blocked;
        self.suspicious = suspicious;
        self
    }

    fn fail_download(mut self) -> Self {
        self.fail_download = true;
        self
    }

    fn without_skill_md(mut self) -> Self {
        self.write_skill_md = false;
        self
    }
}

#[async_trait::async_trait]
impl crate::registry::SkillRegistry for FileWritingRegistry {
    fn name(&self) -> &str {
        "filewriter"
    }

    async fn search(
        &self,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<crate::types::SkillSearchResult>> {
        Ok(Vec::new())
    }

    async fn get_skill_meta(&self, slug: &str) -> Result<crate::types::SkillMeta> {
        Ok(crate::types::SkillMeta {
            slug: slug.to_string(),
            display_name: slug.to_string(),
            summary: "file-writing test registry".to_string(),
            latest_version: "1.0".to_string(),
            is_malware_blocked: self.malware_blocked,
            is_suspicious: self.suspicious,
            registry_name: "filewriter".to_string(),
            author: String::new(),
            downloads: 0,
            published_at: None,
        })
    }

    async fn download_and_install(
        &self,
        _slug: &str,
        version: &str,
        target_dir: &str,
    ) -> Result<InstallResult> {
        if self.fail_download {
            return Err(NemesisError::Other(
                "simulated download failure".to_string(),
            ));
        }
        std::fs::create_dir_all(target_dir).map_err(NemesisError::Io)?;
        if self.write_skill_md {
            std::fs::write(
                Path::new(target_dir).join("SKILL.md"),
                &self.skill_md_content,
            )
            .map_err(NemesisError::Io)?;
        }
        Ok(InstallResult {
            version: version.to_string(),
            is_malware_blocked: self.malware_blocked,
            is_suspicious: self.suspicious,
            summary: "file-writing registry install".to_string(),
        })
    }
}

/// 构造带 FileWritingRegistry 的 installer。
fn installer_with_registry(
    workspace: &std::path::Path,
    registry: FileWritingRegistry,
) -> SkillInstaller {
    let mut installer = SkillInstaller::new(&workspace.to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(Arc::new(registry));
    installer.set_registry_manager(manager);
    installer
}

// ---------------------------------------------------------------------------
// install()：经真实写文件 registry 的安全管线
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_install_registry_success_writes_origin_tracking() {
    let dir = tempfile::tempdir().unwrap();
    let installer = installer_with_registry(dir.path(), FileWritingRegistry::new(CLEAN_CONTENT));

    let result = installer
        .install("filewriter", "good-skill", "1.0")
        .await
        .unwrap();

    assert_eq!(result.version, "1.0");
    assert!(!result.is_malware_blocked);
    assert!(!result.is_suspicious);

    let skill_dir = dir.path().join("skills").join("good-skill");
    assert!(skill_dir.join("SKILL.md").exists());

    // origin tracking 落盘且可回读。
    let origin = installer.get_origin_tracking("good-skill").unwrap();
    assert_eq!(origin.registry, "filewriter");
    assert_eq!(origin.slug, "good-skill");
    assert_eq!(origin.installed_version, "1.0");
    assert!(origin.installed_at > 0);

    let check = installer.last_security_check().unwrap();
    assert!(!check.blocked);
}

#[tokio::test]
async fn test_install_registry_malware_blocked_removes_dir() {
    let dir = tempfile::tempdir().unwrap();
    let registry = FileWritingRegistry::new(CLEAN_CONTENT).with_flags(true, false);
    let installer = installer_with_registry(dir.path(), registry);

    let err = installer
        .install("filewriter", "bad-skill", "1.0")
        .await
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("blocked as malware"), "got: {}", msg);

    // malware 闸在 security check 之前：目录被清理，检查结果不落。
    let skill_dir = dir.path().join("skills").join("bad-skill");
    assert!(!skill_dir.exists());
    assert!(installer.last_security_check().is_none());
}

#[tokio::test]
async fn test_install_registry_security_check_blocked_removes_dir() {
    let dir = tempfile::tempdir().unwrap();
    let installer =
        installer_with_registry(dir.path(), FileWritingRegistry::new(DESTRUCTIVE_CONTENT));

    let err = installer
        .install("filewriter", "evil-skill", "1.0")
        .await
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("blocked by security check"), "got: {}", msg);

    // 拦截后目录清理 + 检查结果记录。
    let skill_dir = dir.path().join("skills").join("evil-skill");
    assert!(!skill_dir.exists());
    let check = installer.last_security_check().unwrap();
    assert!(check.blocked);
    assert!(!check.block_reason.is_empty());
}

#[tokio::test]
async fn test_install_registry_suspicious_warns_but_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let registry = FileWritingRegistry::new(CLEAN_CONTENT).with_flags(false, true);
    let installer = installer_with_registry(dir.path(), registry);

    let result = installer
        .install("filewriter", "sketchy-skill", "2.0")
        .await
        .unwrap();

    // suspicious 只警告不拦截。
    assert!(result.is_suspicious);
    let skill_dir = dir.path().join("skills").join("sketchy-skill");
    assert!(skill_dir.join("SKILL.md").exists());
    assert!(installer.get_origin_tracking("sketchy-skill").is_ok());
}

#[tokio::test]
async fn test_install_registry_no_skill_md_skips_security_check() {
    let dir = tempfile::tempdir().unwrap();
    let registry = FileWritingRegistry::new(CLEAN_CONTENT).without_skill_md();
    let installer = installer_with_registry(dir.path(), registry);

    let result = installer
        .install("filewriter", "noskillmd", "1.0")
        .await
        .unwrap();

    assert_eq!(result.version, "1.0");

    // 无 SKILL.md → security check 跳过，但 origin 照写。
    assert!(installer.last_security_check().is_none());
    assert!(installer.get_origin_tracking("noskillmd").is_ok());
    assert!(dir.path().join("skills").join("noskillmd").exists());
}

#[tokio::test]
async fn test_install_registry_download_error_propagates() {
    let dir = tempfile::tempdir().unwrap();
    let registry = FileWritingRegistry::new(CLEAN_CONTENT).fail_download();
    let installer = installer_with_registry(dir.path(), registry);

    let err = installer
        .install("filewriter", "fail-skill", "1.0")
        .await
        .unwrap_err();
    assert!(
        format!("{}", err).contains("simulated download failure"),
        "got: {}",
        err
    );

    assert!(installer.last_security_check().is_none());
}

#[tokio::test]
async fn test_install_registry_not_found_with_populated_manager() {
    let dir = tempfile::tempdir().unwrap();
    let installer = installer_with_registry(dir.path(), FileWritingRegistry::new(CLEAN_CONTENT));

    // manager 里只有 "filewriter"，请求其它名字 → NotFound。
    let err = installer
        .install("other", "some-skill", "1.0")
        .await
        .unwrap_err();
    assert!(
        format!("{}", err).contains("registry 'other' not found"),
        "got: {}",
        err
    );
}

#[tokio::test]
async fn test_install_from_registry_error_only_signature() {
    let dir = tempfile::tempdir().unwrap();
    let installer = installer_with_registry(dir.path(), FileWritingRegistry::new(CLEAN_CONTENT));

    // Go 兼容签名：只返回成功/失败。
    let result = installer
        .install_from_registry("filewriter", "compat-skill", "1.0")
        .await;
    assert!(result.is_ok());
    assert!(
        dir.path()
            .join("skills")
            .join("compat-skill")
            .join("SKILL.md")
            .exists()
    );
}

// ---------------------------------------------------------------------------
// coverage: set_last_security_check / lint-not-passed warn / warnings warn /
// list_available_skills_from_registry mapping
// ---------------------------------------------------------------------------

/// Nine Medium Recon warnings: score 0.55 -> lint NOT passed, but >= 0.3 and
/// no Destructive -> NOT blocked by security check.
const LINT_FAIL_CONTENT: &str = "# Skill\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\n";

#[test]
fn test_set_last_security_check_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    assert!(installer.last_security_check().is_none());

    // 手构输入（单文件版 check_skill_security 随
    // install_from_github 旁路死 API 删除，本测试只钉 set/get roundtrip，
    // 输入来源无关）。
    let result = crate::types::SecurityCheckResult {
        lint_result: crate::lint::LintResult {
            skill_name: "s5-setter".to_string(),
            passed: true,
            score: 0.95,
            warnings: vec![],
        },
        quality_score: None,
        blocked: false,
        block_reason: String::new(),
    };
    installer.set_last_security_check(result.clone());
    let got = installer.last_security_check().unwrap();
    assert_eq!(got.blocked, result.blocked);
    assert_eq!(got.lint_result.score, result.lint_result.score);
}

#[tokio::test]
async fn test_install_registry_lint_not_passed_warns_but_installs() {
    let dir = tempfile::tempdir().unwrap();
    let installer =
        installer_with_registry(dir.path(), FileWritingRegistry::new(LINT_FAIL_CONTENT));

    let result = installer
        .install("filewriter", "lint-fail-skill", "1.0")
        .await;
    assert!(result.is_ok(), "got: {:?}", result.err());

    // Directory must NOT have been cleaned up (only blocked paths remove it).
    let skill_md = dir
        .path()
        .join("skills")
        .join("lint-fail-skill")
        .join("SKILL.md");
    assert!(
        skill_md.exists(),
        "skill must stay installed after lint warning"
    );

    let check = installer.last_security_check().unwrap();
    assert!(!check.blocked, "0.55 >= 0.3 must not block");
    assert!(
        !check.lint_result.passed,
        "score 0.55 < 0.6 must fail lint: {}",
        check.lint_result.score
    );
    assert!(!check.lint_result.warnings.is_empty());
}

#[tokio::test]
async fn test_list_available_skills_from_registry_maps_fields() {
    use crate::types::SkillSearchResult;

    struct SearchStubRegistry;

    fn stub_meta(slug: &str) -> crate::types::SkillMeta {
        crate::types::SkillMeta {
            slug: slug.to_string(),
            display_name: slug.to_string(),
            summary: String::new(),
            latest_version: "latest".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: "searchstub".to_string(),
            author: String::new(),
            downloads: 0,
            published_at: None,
        }
    }

    #[async_trait::async_trait]
    impl crate::registry::SkillRegistry for SearchStubRegistry {
        fn name(&self) -> &str {
            "searchstub"
        }
        async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SkillSearchResult>> {
            Ok(vec![
                SkillSearchResult {
                    score: 0.9,
                    slug: "alpha".to_string(),
                    display_name: "Alpha".to_string(),
                    summary: "First skill".to_string(),
                    version: String::new(),
                    registry_name: "searchstub".to_string(),
                    source_repo: String::new(),
                    download_path: String::new(),
                    downloads: 3,
                    truncated: false,
                },
                SkillSearchResult {
                    score: 0.5,
                    slug: "beta".to_string(),
                    display_name: "Beta".to_string(),
                    summary: "Second skill".to_string(),
                    version: String::new(),
                    registry_name: "searchstub".to_string(),
                    source_repo: String::new(),
                    download_path: String::new(),
                    downloads: 1,
                    truncated: false,
                },
            ])
        }
        async fn get_skill_meta(&self, slug: &str) -> Result<crate::types::SkillMeta> {
            Ok(stub_meta(slug))
        }
        async fn download_and_install(
            &self,
            _slug: &str,
            version: &str,
            _target_dir: &str,
        ) -> Result<crate::types::InstallResult> {
            Ok(crate::types::InstallResult {
                version: version.to_string(),
                is_malware_blocked: false,
                is_suspicious: false,
                summary: String::new(),
            })
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(Arc::new(SearchStubRegistry));
    installer.set_registry_manager(manager);

    let manager = installer.get_registry_manager().unwrap();
    let available = installer
        .list_available_skills_from_registry(manager)
        .await
        .unwrap();

    assert_eq!(available.len(), 2, "both results must be flattened in");
    assert_eq!(available[0].name, "alpha");
    assert_eq!(available[0].description, "First skill");
    assert_eq!(available[0].tags, vec!["searchstub".to_string()]);
    assert_eq!(available[0].repository, "");
    assert_eq!(available[1].name, "beta");
}
