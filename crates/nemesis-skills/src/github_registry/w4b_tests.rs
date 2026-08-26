//! W4b 补测（批次 14）：GitHubRegistry 下载安装树路径与截断告警。
//!
//! 覆盖缺口（github_extra_tests 已盖 search/meta/browse 的 HTTP 面，
//! 但 download_and_install 的树下载成功路径未盖）：
//! - `download_and_install` 两层模式树下载成功（Trees API + raw 多文件落盘）
//! - `download_and_install` 三层 slug 带 author（dir_prefix = skills/{author}/{slug}）
//! - `download_and_install` 三层 slug 不带 author → 回退 legacy 单文件
//! - `download_and_install` 版本缺省链（version 空 + meta 空 → "main"）
//! - `search` 三层 Trees API truncated=true 告警臂（仍正常返回）
//! - `get_skill_content` {author} 模式的 URL 构造分支
//! - `download_skill_tree` 公开包装器（api/raw 双 server）

use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn api_config(repo: &str) -> GitHubSourceConfig {
    GitHubSourceConfig {
        name: "test".to_string(),
        repo: repo.to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 5,
        max_size: 1024 * 1024,
    }
}

fn author_config(repo: &str) -> GitHubSourceConfig {
    GitHubSourceConfig {
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        ..api_config(repo)
    }
}

fn tree_body(paths: &[(&str, &str)], truncated: bool) -> String {
    let entries: Vec<String> = paths
        .iter()
        .map(|(t, p)| {
            format!(
                r#"{{"path": "{}", "type": "{}", "sha": "abc", "size": 100, "url": "u"}}"#,
                p, t
            )
        })
        .collect();
    format!(
        r#"{{"sha": "root", "tree": [{}], "truncated": {}}}"#,
        entries.join(","),
        truncated
    )
}

/// 挂载 Trees API + raw 内容两个 mock（同一 server）。
async fn mount_tree_and_raw(server: &MockServer, repo: &str, tree_json: &str, blobs: &[(&str, &str)]) {
    Mock::given(method("GET"))
        .and(path(format!("/repos/{}/git/trees/main", repo)))
        .respond_with(ResponseTemplate::new(200).set_body_string(tree_json.to_string()))
        .mount(server)
        .await;
    for (blob_path, body) in blobs {
        Mock::given(method("GET"))
            .and(path(format!("/{}/main/{}", repo, blob_path)))
            .respond_with(ResponseTemplate::new(200).set_body_string(body.to_string()))
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn test_download_and_install_tree_success_writes_multiple_files() {
    let server = MockServer::start().await;
    let mut reg = GitHubRegistry::from_source(&api_config("octo/skills"));
    reg.base_url = server.uri();
    reg.set_github_api_url(&server.uri());

    let tree = tree_body(
        &[
            ("blob", "skills/pdf/SKILL.md"),
            ("blob", "skills/pdf/reference.md"),
            ("tree", "skills/pdf/assets"),
            ("blob", "skills/pdf/assets/diagram.txt"),
            ("blob", "skills/other/SKILL.md"), // 前缀外，不应下载
        ],
        false,
    );
    mount_tree_and_raw(
        &server,
        "octo/skills",
        &tree,
        &[
            ("skills/pdf/SKILL.md", "# PDF Skill"),
            ("skills/pdf/reference.md", "reference content"),
            ("skills/pdf/assets/diagram.txt", "diagram"),
        ],
    )
    .await;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("pdf");
    let result = reg
        .download_and_install("pdf", "1.2", &target.to_string_lossy())
        .await
        .unwrap();

    assert_eq!(result.version, "1.2");
    assert!(!result.is_malware_blocked);

    // 前缀内 3 个 blob 全部落盘（含子目录）。
    assert_eq!(
        std::fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "# PDF Skill"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("reference.md")).unwrap(),
        "reference content"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("assets").join("diagram.txt")).unwrap(),
        "diagram"
    );
    // 前缀外文件不落盘。
    assert!(!target.join("../other/SKILL.md").exists());
}

#[tokio::test]
async fn test_download_and_install_author_slug_rejected_by_validation() {
    // 三层 {author} 模式下，download_and_install 的 slug 校验先于前缀展开：
    // 含 "/" 的 author-qualified slug 会被 validate_skill_identifier 拒绝
    // （skill_dir_prefix 的 author 分支实际只能经 download_skill_tree 到达）。
    let reg = GitHubRegistry::from_source(&author_config("octo/skills"));
    let dir = tempfile::tempdir().unwrap();

    let err = reg
        .download_and_install("anthropics/pdf", "", &dir.path().join("t").to_string_lossy())
        .await
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("invalid slug") && msg.contains("path separators"),
        "got: {}",
        msg
    );
    assert!(!dir.path().join("t").exists());
}

#[tokio::test]
async fn test_download_and_install_author_pattern_plain_slug_falls_back_to_legacy() {
    let server = MockServer::start().await;
    let mut reg = GitHubRegistry::from_source(&author_config("octo/skills"));
    reg.base_url = server.uri();
    // 注意：github_api index 的 get_skill_meta 不发 HTTP，meta 默认成功。

    // 不带 author 的 slug 无法确定三层前缀 → 回退 legacy 单文件下载。
    // build_skill_url 对 {author} 模式 + 无 author slug 只替换 {slug}，
    // {author} 保持字面量；reqwest/url 会把花括号百分号编码（{→%7B }→%7D）。
    Mock::given(method("GET"))
        .and(path("/octo/skills/main/skills/%7Bauthor%7D/pdf/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# legacy skill"))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("pdf");
    let result = reg
        .download_and_install("pdf", "", &target.to_string_lossy())
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "# legacy skill"
    );
    // github_api index 的 meta 无 HTTP 即成功 → latest_version="latest" 优先于 "main"。
    assert_eq!(result.version, "latest");
}

#[tokio::test]
async fn test_download_and_install_meta_failure_uses_version_fallback() {
    let server = MockServer::start().await;
    // skills_json index：get_skill_meta 会发 HTTP 且 404 → meta 取默认。
    let cfg = GitHubSourceConfig {
        index_type: "skills_json".to_string(),
        index_path: "skills.json".to_string(),
        skill_path_pattern: String::new(), // 空 pattern → 跳过树下载走 legacy
        ..api_config("octo/skills")
    };
    let mut reg = GitHubRegistry::from_source(&cfg);
    reg.base_url = server.uri();

    Mock::given(method("GET"))
        .and(path("/octo/skills/main/skills.json"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    // legacy URL：pattern 空 → build_skill_url 得 /octo/skills/main/（get_skill_content 同款）
    // 实际会请求 {base}/{repo}/{branch}/{pattern.replace} = /octo/skills/main/
    Mock::given(method("GET"))
        .and(path("/octo/skills/main/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# fallback"))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("s");
    let result = reg
        .download_and_install("s", "", &target.to_string_lossy())
        .await
        .unwrap();

    // meta 失败 → 默认 meta.latest_version = version("") → "main"。
    assert_eq!(result.version, "main");
    assert_eq!(
        std::fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "# fallback"
    );
}

#[tokio::test]
async fn test_search_three_layer_truncated_warn_still_returns_results() {
    let server = MockServer::start().await;
    let mut reg = GitHubRegistry::from_source(&author_config("octo/skills"));
    reg.set_github_api_url(&server.uri());

    let tree = tree_body(
        &[("blob", "skills/anthropics/pdf/SKILL.md")],
        true, // truncated=true → 告警臂，但结果照常返回
    );
    Mock::given(method("GET"))
        .and(path("/repos/octo/skills/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tree))
        .mount(&server)
        .await;

    let results = reg.search("", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].slug, "pdf");
    assert_eq!(results[0].download_path, "skills/anthropics/pdf/SKILL.md");
}

#[tokio::test]
async fn test_get_skill_content_author_pattern_builds_author_url() {
    let server = MockServer::start().await;
    let mut reg = GitHubRegistry::from_source(&author_config("octo/skills"));
    reg.base_url = server.uri();

    Mock::given(method("GET"))
        .and(path("/octo/skills/main/skills/anthropics/pdf/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# content here"))
        .mount(&server)
        .await;

    let content = reg.get_skill_content("anthropics/pdf").await.unwrap();
    assert_eq!(content.slug, "anthropics/pdf");
    assert_eq!(content.filename, "SKILL.md");
    assert_eq!(content.content, "# content here");
}

#[tokio::test]
async fn test_download_skill_tree_wrapper_uses_both_urls() {
    let api_server = MockServer::start().await;
    let raw_server = MockServer::start().await;

    let mut reg = GitHubRegistry::from_source(&api_config("octo/skills"));
    reg.base_url = raw_server.uri();
    reg.set_github_api_url(&api_server.uri());

    let tree = tree_body(&[("blob", "skills/pdf/SKILL.md")], false);
    Mock::given(method("GET"))
        .and(path("/repos/octo/skills/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tree))
        .mount(&api_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/octo/skills/main/skills/pdf/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# via wrapper"))
        .mount(&raw_server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("pdf");
    reg.download_skill_tree("skills/pdf", &target.to_string_lossy())
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(target.join("SKILL.md")).unwrap(),
        "# via wrapper"
    );
}
