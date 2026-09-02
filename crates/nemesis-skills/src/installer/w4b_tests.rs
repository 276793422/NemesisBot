//! W4b 补测（批次 14）：SkillInstaller 的 HTTP 安装流与安全拦截流。
//!
//! 覆盖缺口（此前 0 wiremock 使用 `set_github_base_url` seam）：
//! - `install_from_github`：wiremock HTTP 成功/404/500、安全拦截清理、
//!   低质量警告不拦截、repo 路径末段取技能名、GitHub 路径不写 origin
//! - `install()`：经真实写文件的 registry 验证 malware_blocked 清理、
//!   security-check 拦截清理、suspicious 仅警告、无 SKILL.md 跳过检查、
//!   下载错误传播、origin tracking 落盘与回读

use super::*;
use std::sync::Arc;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
// install_from_github：wiremock HTTP 流
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_install_from_github_http_success_writes_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/owner/repo/main/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(CLEAN_CONTENT))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.set_github_base_url(&server.uri());

    installer.install_from_github("owner/repo").await.unwrap();

    let skill_md = dir.path().join("skills").join("repo").join("SKILL.md");
    assert!(skill_md.exists());
    let written = std::fs::read_to_string(&skill_md).unwrap();
    assert_eq!(written, CLEAN_CONTENT);

    let check = installer.last_security_check().unwrap();
    assert!(!check.blocked);
}

#[tokio::test]
async fn test_install_from_github_http_404_returns_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/owner/repo/main/SKILL.md"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.set_github_base_url(&server.uri());

    let err = installer
        .install_from_github("owner/repo")
        .await
        .unwrap_err();
    assert!(format!("{}", err).contains("HTTP 404"), "got: {}", err);

    // 失败发生在写盘前，目录不应被创建。
    assert!(!dir.path().join("skills").join("repo").exists());
}

#[tokio::test]
async fn test_install_from_github_http_500_returns_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/owner/repo/main/SKILL.md"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.set_github_base_url(&server.uri());

    let err = installer
        .install_from_github("owner/repo")
        .await
        .unwrap_err();
    assert!(format!("{}", err).contains("HTTP 500"), "got: {}", err);
}

#[tokio::test]
async fn test_install_from_github_security_blocked_cleans_up() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/owner/repo/main/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(DESTRUCTIVE_CONTENT))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.set_github_base_url(&server.uri());

    let err = installer
        .install_from_github("owner/repo")
        .await
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("blocked by security check"), "got: {}", msg);

    // 拦截后目录必须被清理。
    let skill_dir = dir.path().join("skills").join("repo");
    assert!(!skill_dir.exists());

    // 检查结果仍被记录（供 UI/日志展示拦截原因）。
    let check = installer.last_security_check().unwrap();
    assert!(check.blocked);
}

#[tokio::test]
async fn test_install_from_github_low_quality_still_succeeds() {
    let server = MockServer::start().await;
    // 极简正文：质量分会低（< 40 触发 warn），但 lint 无危险模式 → 不拦截。
    Mock::given(method("GET"))
        .and(path("/owner/repo/main/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.set_github_base_url(&server.uri());

    installer.install_from_github("owner/repo").await.unwrap();

    let skill_md = dir.path().join("skills").join("repo").join("SKILL.md");
    assert!(skill_md.exists());

    let check = installer.last_security_check().unwrap();
    assert!(!check.blocked);
    let quality = check.quality_score.expect("quality score always present");
    assert!(
        quality.overall < 40.0,
        "expected low quality, got {}",
        quality.overall
    );
}

#[tokio::test]
async fn test_install_from_github_no_origin_tracking_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/owner/repo/main/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(CLEAN_CONTENT))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.set_github_base_url(&server.uri());

    installer.install_from_github("owner/repo").await.unwrap();

    // GitHub 直装路径不写 origin tracking（只有 registry 路径写）。
    let origin = dir
        .path()
        .join("skills")
        .join("repo")
        .join(".skill-origin.json");
    assert!(!origin.exists());
    assert!(installer.get_origin_tracking("repo").is_err());
}

#[tokio::test]
async fn test_install_from_github_name_is_last_path_segment() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/org/my-repo/main/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(CLEAN_CONTENT))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.set_github_base_url(&server.uri());

    installer.install_from_github("org/my-repo").await.unwrap();

    // 技能名取 repo 路径末段，而非完整 "org/my-repo"。
    let skill_dir = dir.path().join("skills").join("my-repo");
    assert!(skill_dir.join("SKILL.md").exists());
    assert!(!dir.path().join("skills").join("org").exists());
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
// S5 coverage: set_last_security_check / lint-not-passed warn / warnings warn /
// list_available_skills_from_registry mapping
// ---------------------------------------------------------------------------

/// Content with exactly one Medium Recon warning: warnings non-empty but
/// lint still passes (score 0.95, no critical/high).
const ONE_WARNING_CONTENT: &str = "# Skill\nRun ps aux to inspect processes.\n";

/// Nine Medium Recon warnings: score 0.55 -> lint NOT passed, but >= 0.3 and
/// no Destructive -> NOT blocked by security check.
const LINT_FAIL_CONTENT: &str = "# Skill\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\nRun ps aux.\n";

#[test]
fn test_set_last_security_check_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    assert!(installer.last_security_check().is_none());

    let result = check_skill_security(CLEAN_CONTENT, "s5-setter", "");
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
async fn test_install_from_github_single_warning_still_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/owner/warn-repo/main/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ONE_WARNING_CONTENT))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.set_github_base_url(&server.uri());

    installer
        .install_from_github("owner/warn-repo")
        .await
        .unwrap();

    let skill_md = dir.path().join("skills").join("warn-repo").join("SKILL.md");
    assert!(skill_md.exists(), "warned skill must still install");
    let check = installer.last_security_check().unwrap();
    assert!(!check.blocked);
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
