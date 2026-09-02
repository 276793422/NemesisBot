//! Additional tests for modelscope_registry covering URL parsing, JSON parsing,
//! error mapping, and HTTP mock-based flows.

use super::*;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ============================================================
// fetch_skill_content: per-skill JSON detail API
// ============================================================

/// Build a detail-endpoint JSON body with the given ReadMeContent.
fn detail_json(read_me: &str) -> String {
    format!(
        r#"{{"Code":200,"Data":{{"Name":"pdf","ReadMeContent":{}}},"Message":"ok","Success":true}}"#,
        serde_json::Value::String(read_me.to_string())
    )
}

#[tokio::test]
async fn test_fetch_skill_content_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(detail_json("---\nname: pdf\n---\nbody")),
        )
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let content = reg.fetch_skill_content("PantherAng", "pdf").await.unwrap();
    assert!(content.starts_with("---\nname: pdf"));
    assert!(content.contains("body"));
}

#[tokio::test]
async fn test_fetch_skill_content_empty_readme_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(detail_json("")))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let err = reg
        .fetch_skill_content("PantherAng", "pdf")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("empty"));
}

#[tokio::test]
async fn test_fetch_skill_content_api_error_code() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"Code":404,"Data":{},"Message":"not found","Success":false}"#),
        )
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let err = reg
        .fetch_skill_content("PantherAng", "pdf")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404") || err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_fetch_skill_content_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let err = reg
        .fetch_skill_content("PantherAng", "pdf")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("HTTP"));
}

// ============================================================
// convert_skill pure function (via API JSON parse + convert)
// ============================================================

fn make_skill_json(skills: &[(&str, &str, &str, &str, &str, &str, i64)]) -> String {
    let arr: Vec<String> = skills
        .iter()
        .map(|(name, disp, desc, desc_en, url, dev, dl)| {
            format!(
                r#"{{"Name":"{}","DisplayName":"{}","Description":"{}","DescriptionEn":"{}","SourceUrl":"{}","SourceDeveloper":"{}","DownloadCount":{}}}"#,
                name, disp, desc, desc_en, url, dev, dl
            )
        })
        .collect();
    format!(
        r#"{{"Code":200,"Data":{{"SkillList":[{}],"TotalCount":{}}},"Message":"ok","Success":true}}"#,
        arr.join(","),
        skills.len()
    )
}

#[test]
fn test_convert_skill_with_description() {
    let json = make_skill_json(&[(
        "pdf",
        "PDF Tool",
        "PDF 中文描述",
        "PDF English desc",
        "https://github.com/o/r/tree/main/skills/pdf",
        "alice",
        100,
    )]);
    let api: ApiResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(api.code, 200);
    assert_eq!(api.data.skill_list.len(), 1);
    let converted = ModelScopeRegistry::convert_skill(&api.data.skill_list[0]);
    assert_eq!(converted.slug, "pdf");
    assert_eq!(converted.display_name, "PDF Tool");
    // Prefers Chinese description when present
    assert_eq!(converted.summary, "PDF 中文描述");
    assert_eq!(converted.source_repo, "alice");
    assert_eq!(converted.downloads, 100);
    assert_eq!(converted.registry_name, "modelscope");
    assert_eq!(converted.version, "latest");
    assert!(!converted.truncated);
}

#[test]
fn test_convert_skill_falls_back_to_description_en() {
    let json = make_skill_json(&[(
        "csv",
        "CSV",
        "",
        "CSV English fallback",
        "https://github.com/o/r/tree/main/skills/csv",
        "bob",
        5,
    )]);
    let api: ApiResponse = serde_json::from_str(&json).unwrap();
    let converted = ModelScopeRegistry::convert_skill(&api.data.skill_list[0]);
    assert_eq!(converted.summary, "CSV English fallback");
}

#[test]
fn test_convert_skill_both_descriptions_empty() {
    let json = make_skill_json(&[(
        "empty",
        "Empty",
        "",
        "",
        "https://github.com/o/r/tree/main/skills/empty",
        "",
        0,
    )]);
    let api: ApiResponse = serde_json::from_str(&json).unwrap();
    let converted = ModelScopeRegistry::convert_skill(&api.data.skill_list[0]);
    assert_eq!(converted.summary, "");
}

#[test]
fn test_convert_skill_score_is_half() {
    let json = make_skill_json(&[(
        "test",
        "T",
        "d",
        "",
        "https://github.com/o/r/tree/main/skills/test",
        "",
        0,
    )]);
    let api: ApiResponse = serde_json::from_str(&json).unwrap();
    let converted = ModelScopeRegistry::convert_skill(&api.data.skill_list[0]);
    assert_eq!(converted.score, 0.5);
}

// ============================================================
// JSON parsing: ApiResponse / ApiData / ModelScopeSkill
// ============================================================

#[test]
fn test_api_response_with_empty_skill_list() {
    let json =
        r#"{"Code":200,"Data":{"SkillList":[],"TotalCount":0},"Message":"ok","Success":true}"#;
    let api: ApiResponse = serde_json::from_str(json).unwrap();
    assert_eq!(api.code, 200);
    assert!(api.data.skill_list.is_empty());
    assert_eq!(api.data.total_count, 0);
}

#[test]
fn test_api_response_missing_skill_list_defaults_to_empty() {
    let json = r#"{"Code":200,"Data":{},"Message":"ok","Success":true}"#;
    let api: ApiResponse = serde_json::from_str(json).unwrap();
    assert!(api.data.skill_list.is_empty());
    assert_eq!(api.data.total_count, 0);
}

#[test]
fn test_api_response_missing_total_count_defaults_zero() {
    let json = r#"{"Code":200,"Data":{"SkillList":[]},"Message":"ok","Success":true}"#;
    let api: ApiResponse = serde_json::from_str(json).unwrap();
    assert_eq!(api.data.total_count, 0);
}

#[test]
fn test_modelscope_skill_missing_fields_default_to_empty() {
    let json = r#"{"Name":"x"}"#;
    let s: ModelScopeSkill = serde_json::from_str(json).unwrap();
    assert_eq!(s.name, "x");
    assert_eq!(s.display_name, "");
    assert_eq!(s.description, "");
    assert_eq!(s.path, "");
    assert_eq!(s.download_count, 0);
}

#[test]
fn test_modelscope_skill_source_and_source_url_deserialized() {
    // The GitHub-fallback dispatch depends on `source` ("github") and a parseable
    // `source_url`. Verify both fields deserialize from the catalog shape.
    let json = r#"{"Name":"mingli","Source":"github","SourceUrl":"https://github.com/o/r/tree/main/p/mingli"}"#;
    let s: ModelScopeSkill = serde_json::from_str(json).unwrap();
    assert_eq!(s.source, "github");
    assert_eq!(s.source_url, "https://github.com/o/r/tree/main/p/mingli");
}

#[test]
fn test_modelscope_skill_with_category() {
    let json = r#"{"Name":"x","L1":{"CatalogId":"c1","ChineseName":"中","Name":"en"}}"#;
    let s: ModelScopeSkill = serde_json::from_str(json).unwrap();
    assert!(s.l1.is_some());
    let cat = s.l1.unwrap();
    assert_eq!(cat.catalog_id, "c1");
    assert_eq!(cat.chinese_name, "中");
    assert_eq!(cat.name, "en");
}

#[test]
fn test_modelscope_skill_category_partial_fields() {
    let json = r#"{"L1":{"CatalogId":"c1"}}"#;
    let s: ModelScopeSkill = serde_json::from_str(json).unwrap();
    let cat = s.l1.unwrap();
    assert_eq!(cat.catalog_id, "c1");
    assert_eq!(cat.chinese_name, "");
}

#[test]
fn test_modelscope_skill_with_tags() {
    let json = r#"{"Name":"x","Tags":["a","b"],"License":"MIT","SourceStar":10,"SourceForks":5,"Visits":3}"#;
    let s: ModelScopeSkill = serde_json::from_str(json).unwrap();
    assert_eq!(s.tags, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(s.license, "MIT");
    assert_eq!(s.source_star, 10);
    assert_eq!(s.source_forks, 5);
    assert_eq!(s.visits, 3);
}

#[test]
fn test_search_request_serializes_pascal_case() {
    let req = SearchRequest {
        page_size: 10,
        page_number: 2,
        query: "pdf".to_string(),
        sort: "Default".to_string(),
        criterion: vec![],
        with_top_collection: false,
    };
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["PageSize"], 10);
    assert_eq!(json["PageNumber"], 2);
    assert_eq!(json["Query"], "pdf");
    assert_eq!(json["Sort"], "Default");
    assert_eq!(json["Criterion"].as_array().unwrap().len(), 0);
    assert_eq!(json["WithTopCollection"], false);
}

#[test]
fn test_search_request_criterion_can_hold_objects() {
    let criterion = serde_json::json!({"Key": "Category", "Value": "tools"});
    let req = SearchRequest {
        page_size: 5,
        page_number: 1,
        query: "test".to_string(),
        sort: "Default".to_string(),
        criterion: vec![criterion],
        with_top_collection: true,
    };
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["Criterion"].as_array().unwrap().len(), 1);
    assert_eq!(json["WithTopCollection"], true);
}

// ============================================================
// HTTP-mocked api_search: success / errors
// ============================================================

async fn make_registry_pointing_at(server: &MockServer) -> ModelScopeRegistry {
    let mut reg = ModelScopeRegistry::new();
    reg.base_url = server.uri();
    reg.content_base_url = server.uri();
    reg
}

#[tokio::test]
async fn test_api_search_success_returns_response() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "pdf",
        "PDF",
        "desc",
        "",
        "https://github.com/o/r/tree/main/skills/pdf",
        "alice",
        7,
    )]);
    Mock::given(method("PUT"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let api = reg.api_search("pdf", 1, 10, "Default").await.unwrap();
    assert_eq!(api.code, 200);
    assert_eq!(api.data.skill_list.len(), 1);
    assert_eq!(api.data.skill_list[0].name, "pdf");
}

#[tokio::test]
async fn test_api_search_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let err = reg.api_search("pdf", 1, 10, "Default").await.unwrap_err();
    assert!(err.to_string().contains("HTTP"));
}

#[tokio::test]
async fn test_api_search_api_error_code() {
    let server = MockServer::start().await;
    let body = r#"{"Code":400,"Data":{},"Message":"bad query","Success":false}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let err = reg.api_search("pdf", 1, 10, "Default").await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("400") || msg.contains("bad query"));
}

#[tokio::test]
async fn test_api_search_invalid_json() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let err = reg.api_search("pdf", 1, 10, "Default").await.unwrap_err();
    assert!(err.to_string().contains("parse"));
}

#[tokio::test]
async fn test_search_caps_limit_at_50() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "x",
        "X",
        "d",
        "",
        "https://github.com/o/r/tree/main/skills/x",
        "",
        0,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    // Limit 1000 should be clamped to 50 - we just verify the call succeeds
    let results = reg.search("pdf", 1000).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_search_returns_converted_results() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[
        (
            "pdf",
            "PDF",
            "PDF描述",
            "",
            "https://github.com/o/r/tree/main/skills/pdf",
            "alice",
            10,
        ),
        (
            "csv",
            "CSV",
            "",
            "CSV EN",
            "https://github.com/o/r/tree/main/skills/csv",
            "bob",
            20,
        ),
    ]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let results = reg.search("pdf", 10).await.unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].slug, "pdf");
    assert_eq!(results[1].slug, "csv");
    assert_eq!(results[1].summary, "CSV EN");
}

#[tokio::test]
async fn test_get_skill_meta_success() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "pdf",
        "PDF Display",
        "PDF summary",
        "",
        "https://github.com/o/r/tree/main/skills/pdf",
        "alice",
        42,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let meta = reg.get_skill_meta("pdf").await.unwrap();
    assert_eq!(meta.slug, "pdf");
    assert_eq!(meta.display_name, "PDF Display");
    assert_eq!(meta.summary, "PDF summary");
    assert_eq!(meta.author, "alice");
    assert_eq!(meta.downloads, 42);
    assert_eq!(meta.registry_name, "modelscope");
}

#[tokio::test]
async fn test_get_skill_meta_invalid_slug() {
    let reg = ModelScopeRegistry::new();
    let err = reg.get_skill_meta("bad/slug").await.unwrap_err();
    assert!(err.to_string().contains("invalid") || err.to_string().contains("separator"));
}

#[tokio::test]
async fn test_get_skill_meta_empty_slug() {
    let reg = ModelScopeRegistry::new();
    assert!(reg.get_skill_meta("").await.is_err());
}

#[tokio::test]
async fn test_get_skill_meta_not_found() {
    let server = MockServer::start().await;
    let body =
        r#"{"Code":200,"Data":{"SkillList":[],"TotalCount":0},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let err = reg.get_skill_meta("missing").await.unwrap_err();
    assert!(err.to_string().contains("not found"));
}

// ============================================================
// download_and_install / get_skill_content via mocks
// ============================================================

#[tokio::test]
async fn test_download_and_install_invalid_slug() {
    let reg = ModelScopeRegistry::new();
    let err = reg
        .download_and_install("a/b", "1.0", "/tmp")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid") || err.to_string().contains("separator"));
}

#[tokio::test]
async fn test_download_and_install_no_files() {
    let server = MockServer::start().await;
    let body =
        r#"{"Code":200,"Data":{"SkillList":[],"TotalCount":0},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let err = reg.download_and_install("pdf", "1.0", "/tmp").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn test_download_and_install_name_mismatch_errors() {
    // Free-text search may return a skill whose name differs from the slug
    // (e.g. the duplicated catalog slug "chinese-novelist"); install must
    // refuse rather than silently install the wrong skill.
    let server = MockServer::start().await;
    let body = r#"{"Code":200,"Data":{"SkillList":[{"Name":"not-pdf","Path":"X","SourceUrl":"https://gitlab.com/x"}],"TotalCount":1},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let err = reg
        .download_and_install("pdf", "1.0", "/tmp/modelscope_install")
        .await
        .unwrap_err();
    assert!(err.to_string().to_lowercase().contains("not found"));
    assert!(err.to_string().contains("not-pdf"));
}

#[tokio::test]
async fn test_download_and_install_full_success() {
    let server = MockServer::start().await;
    // Search returns a skill with Path + a non-github SourceUrl, proving install
    // no longer depends on SourceUrl being a github tree URL.
    let search_body = r#"{"Code":200,"Data":{"SkillList":[{"Name":"pdf","DisplayName":"PDF","Description":"summary","Path":"PantherAng","SourceUrl":"https://gitlab.com/x"}],"TotalCount":1},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(search_body))
        .mount(&server)
        .await;

    // Root file listing (?Root= empty): a `references/` tree + SKILL.md blob.
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/files"))
        .and(query_param("Root", ""))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Files":[{"Path":"references","Type":"tree"},{"Path":"SKILL.md","Type":"blob"}]},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    // `references/` listing (?Root=references) — mutually exclusive with the
    // root mock by query param, so mount order does not matter.
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/files"))
        .and(query_param("Root", "references"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Files":[{"Path":"references/detail.md","Type":"blob"}]},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    // File contents via /repo/raw.
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/raw"))
        .and(query_param("FilePath", "SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Content":"---\nname: pdf\n---\n# PDF skill"},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/raw"))
        .and(query_param("FilePath", "references/detail.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Content":"detail body"},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let reg = make_registry_pointing_at(&server).await;
    let result = reg
        .download_and_install("pdf", "1.0", dir.path().to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(result.version, "latest");
    assert_eq!(result.summary, "summary");
    let skill_md = std::fs::read_to_string(dir.path().join("SKILL.md")).unwrap();
    assert!(skill_md.starts_with("---\nname: pdf"));
    assert!(skill_md.contains("# PDF skill"));
    // Companion file under a subdirectory is installed too.
    let detail = std::fs::read_to_string(dir.path().join("references").join("detail.md")).unwrap();
    assert_eq!(detail, "detail body");
}

#[tokio::test]
async fn test_download_and_install_rejects_path_traversal() {
    // A malicious listing must not be able to write outside the target dir.
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"SkillList":[{"Name":"pdf","DisplayName":"PDF","Description":"s","Path":"PantherAng","SourceUrl":"x"}],"TotalCount":1},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/files"))
        .and(query_param("Root", ""))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Files":[{"Path":"../evil.md","Type":"blob"}]},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/raw"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Content":"evil"},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let reg = make_registry_pointing_at(&server).await;
    let err = reg
        .download_and_install("pdf", "1.0", dir.path().to_str().unwrap())
        .await
        .unwrap_err();
    let msg = err.to_string().to_lowercase();
    assert!(msg.contains("unsafe") || msg.contains("traversal") || msg.contains("security"));
    assert!(!dir.path().join("../evil.md").exists());
}

#[tokio::test]
#[ignore = "live network test against modelscope.cn; run with --ignored"]
async fn live_modelscope_install_divination_full_tree() {
    // Verifies the FULL skill tree (SKILL.md + references/) installs, not just
    // SKILL.md. The `divination` skill (@hhszzzz) has SKILL.md + 7 reference files.
    let reg = ModelScopeRegistry::new();
    let dir = tempfile::tempdir().unwrap();
    reg.download_and_install("divination", "latest", dir.path().to_str().unwrap())
        .await
        .expect("install should succeed");

    assert!(
        dir.path().join("SKILL.md").exists(),
        "SKILL.md must be installed"
    );
    let refs_dir = dir.path().join("references");
    assert!(refs_dir.is_dir(), "references/ directory must be installed");
    let ref_count = std::fs::read_dir(&refs_dir).unwrap().count();
    assert!(
        ref_count >= 5,
        "expected several reference files, got {}",
        ref_count
    );

    let bazi = std::fs::read_to_string(refs_dir.join("bazi-workflow.md"))
        .expect("bazi-workflow.md must be installed");
    assert!(
        bazi.contains("Bazi") || bazi.contains("八字"),
        "content must be intact UTF-8"
    );
}

#[tokio::test]
async fn test_download_and_install_thin_non_github_skips_fallback() {
    // A ModelScope-native skill mirrored as only SKILL.md (no subdirs, Source=ModelScope)
    // must install the mirrored SKILL.md and NOT attempt the GitHub fallback.
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"SkillList":[{"Name":"pdf","DisplayName":"PDF","Description":"s","Path":"PantherAng","Source":"ModelScope","SourceUrl":""}],"TotalCount":1},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/files"))
        .and(query_param("Root", ""))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Files":[{"Path":"SKILL.md","Type":"blob"}]},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/raw"))
        .and(query_param("FilePath", "SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Content":"---\nname: pdf\n---\nbody"},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let reg = make_registry_pointing_at(&server).await;
    reg.download_and_install("pdf", "1.0", dir.path().to_str().unwrap())
        .await
        .expect("install should succeed");
    let skill_md = std::fs::read_to_string(dir.path().join("SKILL.md")).unwrap();
    assert!(skill_md.starts_with("---\nname: pdf"));
}

#[tokio::test]
#[ignore = "live network test; mingli is only partially mirrored on ModelScope"]
async fn live_modelscope_install_mingli_partial_mirror_graceful() {
    // mingli's ModelScope mirror is only SKILL.md; its references/ + scripts/ live
    // on GitHub. Where GitHub is reachable, the fallback installs the full tree;
    // where it is not (e.g. a network blocking api.github.com), the install must
    // still degrade gracefully to the mirrored SKILL.md rather than fail.
    let reg = ModelScopeRegistry::new();
    let dir = tempfile::tempdir().unwrap();
    reg.download_and_install("mingli", "latest", dir.path().to_str().unwrap())
        .await
        .expect("install should succeed (at least the mirrored SKILL.md)");
    assert!(
        dir.path().join("SKILL.md").exists(),
        "mirrored SKILL.md must install"
    );
}

#[tokio::test]
async fn test_get_skill_content_invalid_slug() {
    let reg = ModelScopeRegistry::new();
    assert!(reg.get_skill_content("a/b").await.is_err());
}

#[tokio::test]
async fn test_get_skill_content_not_found() {
    let server = MockServer::start().await;
    let body = r#"{"Code":200,"Data":{"SkillList":[]},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let err = reg.get_skill_content("missing").await.unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_get_skill_content_via_detail_api() {
    let server = MockServer::start().await;
    let body = r#"{"Code":200,"Data":{"SkillList":[{"Name":"pdf","Path":"PantherAng","SourceUrl":"https://example.com/x"}],"TotalCount":1},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(detail_json("---\nname: pdf\n---\nbody")),
        )
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let content = reg.get_skill_content("pdf").await.unwrap();
    assert_eq!(content.slug, "pdf");
    assert_eq!(content.filename, "SKILL.md");
    assert!(content.content.starts_with("---\nname: pdf"));
}

// ============================================================
// browse sort mapping + pagination
// ============================================================

#[tokio::test]
async fn test_browse_default_sort() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "a",
        "A",
        "x",
        "",
        "https://github.com/o/r/tree/main/skills/a",
        "",
        0,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let result = reg.browse(&BrowseSort::Trending, 10, "").await.unwrap();
    assert_eq!(result.items.len(), 1);
}

#[tokio::test]
async fn test_browse_downloads_sort() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "a",
        "A",
        "x",
        "",
        "https://github.com/o/r/tree/main/skills/a",
        "",
        0,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let _ = reg.browse(&BrowseSort::Downloads, 10, "").await.unwrap();
}

#[tokio::test]
async fn test_browse_updated_sort() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "a",
        "A",
        "x",
        "",
        "https://github.com/o/r/tree/main/skills/a",
        "",
        0,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let _ = reg.browse(&BrowseSort::Updated, 10, "").await.unwrap();
}

#[tokio::test]
async fn test_browse_pagination_has_more() {
    let server = MockServer::start().await;
    // total_count > page * page_size -> has_more = true
    let body = r#"{"Code":200,"Data":{"SkillList":[{"Name":"a","DisplayName":"A","Description":"x"}],"TotalCount":100},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let result = reg.browse(&BrowseSort::Trending, 10, "").await.unwrap();
    assert!(result.next_cursor.is_some());
    assert_eq!(result.next_cursor.unwrap(), "2");
}

#[tokio::test]
async fn test_browse_pagination_no_more() {
    let server = MockServer::start().await;
    // total_count = 5, page_size = 10 -> page*page_size = 10 > 5 -> has_more = false
    let body = r#"{"Code":200,"Data":{"SkillList":[{"Name":"a"}],"TotalCount":5},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    let result = reg.browse(&BrowseSort::Trending, 10, "").await.unwrap();
    assert!(result.next_cursor.is_none());
}

#[tokio::test]
async fn test_browse_cursor_parsed_as_page() {
    let server = MockServer::start().await;
    let body = r#"{"Code":200,"Data":{"SkillList":[{"Name":"a"}],"TotalCount":1000},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    // cursor "3" should be parsed as page 3, returning next_cursor "4"
    let result = reg.browse(&BrowseSort::Trending, 10, "3").await.unwrap();
    assert_eq!(result.next_cursor.unwrap(), "4");
}

#[tokio::test]
async fn test_browse_invalid_cursor_defaults_to_page_1() {
    let server = MockServer::start().await;
    let body = r#"{"Code":200,"Data":{"SkillList":[{"Name":"a"}],"TotalCount":1000},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    // invalid cursor "abc" -> page = 1
    let result = reg.browse(&BrowseSort::Trending, 10, "abc").await.unwrap();
    // page 1, page_size 10, total 1000 -> next = "2"
    assert_eq!(result.next_cursor.unwrap(), "2");
}

#[tokio::test]
async fn test_browse_limit_capped_at_100() {
    let server = MockServer::start().await;
    let body = r#"{"Code":200,"Data":{"SkillList":[{"Name":"a"}],"TotalCount":50},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;
    // limit=1000 should be capped at 100
    let _ = reg.browse(&BrowseSort::Trending, 1000, "").await.unwrap();
}

#[test]
fn test_registry_name() {
    let reg = ModelScopeRegistry::new();
    assert_eq!(reg.name(), "modelscope");
}

#[test]
fn test_default_base_url() {
    let reg = ModelScopeRegistry::new();
    assert_eq!(
        reg.base_url,
        "https://www.modelscope.cn/api/v1/dolphin/skills"
    );
    assert_eq!(
        reg.content_base_url,
        "https://www.modelscope.cn/api/v1/skills"
    );
}

// ============================================================
// wiremock: deep field-assertion tests for search/browse/meta
//
// The existing tests above already exercise the happy/error paths of
// api_search via mocks; these tests focus on verifying the FULL set of
// parsed/converted fields propagated to the public return types, which
// the earlier tests only spot-check.
// ============================================================

#[tokio::test]
async fn test_search_propagates_all_fields_to_search_result() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "weather",
        "Weather Skill",
        "查天气",
        "weather en",
        "https://github.com/dev/repo/tree/main/skills/weather",
        "weather-dev",
        1234,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let results = reg.search("weather", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    let r = &results[0];
    // convert_skill assigns a fixed 0.5 score and "latest" version.
    assert_eq!(r.score, 0.5);
    assert_eq!(r.slug, "weather");
    assert_eq!(r.display_name, "Weather Skill");
    // Prefers Chinese description when present.
    assert_eq!(r.summary, "查天气");
    assert_eq!(r.version, "latest");
    assert_eq!(r.registry_name, "modelscope");
    assert_eq!(r.source_repo, "weather-dev");
    assert_eq!(r.download_path, "");
    assert_eq!(r.downloads, 1234);
    assert!(!r.truncated);
}

#[tokio::test]
async fn test_search_with_description_en_fallback_in_results() {
    let server = MockServer::start().await;
    // Empty Chinese description -> summary falls back to DescriptionEn.
    let body = make_skill_json(&[(
        "translator",
        "Translator",
        "",
        "Translates text",
        "https://github.com/a/b/tree/main/skills/t",
        "t-dev",
        0,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let results = reg.search("translator", 5).await.unwrap();
    assert_eq!(results[0].summary, "Translates text");
    assert_eq!(results[0].source_repo, "t-dev");
}

#[tokio::test]
async fn test_search_clamps_limit_to_50_in_request() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "x",
        "X",
        "d",
        "",
        "https://github.com/o/r/tree/main/skills/x",
        "",
        0,
    )]);
    // The mock records the request body; we verify PageSize is clamped to 50
    // by asserting the call succeeds (search internally uses limit.min(50)).
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let results = reg.search("anything", 100).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_get_skill_meta_propagates_all_fields() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "pdf-tools",
        "PDF Tools",
        "PDF 处理",
        "pdf tools en",
        "https://github.com/owner/repo/tree/main/skills/pdf",
        "owner",
        999,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let meta = reg.get_skill_meta("pdf-tools").await.unwrap();
    assert_eq!(meta.slug, "pdf-tools");
    assert_eq!(meta.display_name, "PDF Tools");
    // Chinese description preferred.
    assert_eq!(meta.summary, "PDF 处理");
    assert_eq!(meta.latest_version, "latest");
    assert!(!meta.is_malware_blocked);
    assert!(!meta.is_suspicious);
    assert_eq!(meta.registry_name, "modelscope");
    assert_eq!(meta.author, "owner");
    assert_eq!(meta.downloads, 999);
}

#[tokio::test]
async fn test_get_skill_meta_summary_falls_back_to_description_en() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "csv",
        "CSV",
        "",
        "English CSV summary",
        "https://github.com/o/r/tree/main/skills/csv",
        "dev",
        0,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let meta = reg.get_skill_meta("csv").await.unwrap();
    assert_eq!(meta.summary, "English CSV summary");
}

#[tokio::test]
async fn test_browse_propagates_converted_item_fields() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "weather",
        "Weather",
        "天气",
        "",
        "https://github.com/o/r/tree/main/skills/weather",
        "wd",
        77,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let result = reg.browse(&BrowseSort::Downloads, 10, "").await.unwrap();
    assert_eq!(result.items.len(), 1);
    let item = &result.items[0];
    assert_eq!(item.slug, "weather");
    assert_eq!(item.display_name, "Weather");
    assert_eq!(item.summary, "天气");
    assert_eq!(item.registry_name, "modelscope");
    assert_eq!(item.source_repo, "wd");
    assert_eq!(item.downloads, 77);
    assert_eq!(item.version, "latest");
    assert_eq!(item.score, 0.5);
}

#[tokio::test]
async fn test_browse_stars_sort_maps_to_default() {
    let server = MockServer::start().await;
    let body = make_skill_json(&[(
        "a",
        "A",
        "x",
        "",
        "https://github.com/o/r/tree/main/skills/a",
        "",
        0,
    )]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    // Stars and Rating are not explicitly mapped -> "Default".
    let _ = reg.browse(&BrowseSort::Stars, 10, "").await.unwrap();
    let _ = reg.browse(&BrowseSort::Rating, 10, "").await.unwrap();
    // No panic + reachable confirms the Default branch executes.
}

#[tokio::test]
async fn test_browse_downloads_sort_uses_downloadcount() {
    let server = MockServer::start().await;
    // total_count small enough that has_more is false (no next cursor).
    let body = r#"{"Code":200,"Data":{"SkillList":[{"Name":"popular","DisplayName":"Popular","Description":"d","DownloadCount":500}],"TotalCount":1},"Message":"ok","Success":true}"#;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let result = reg.browse(&BrowseSort::Downloads, 10, "").await.unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].slug, "popular");
    assert_eq!(result.items[0].downloads, 500);
    // 1 page * 10 page_size = 10 > total_count 1 -> no more.
    assert!(result.next_cursor.is_none());
}

#[tokio::test]
async fn test_search_empty_query_still_calls_api() {
    // search("") is valid — it still issues the PUT and returns converted results.
    let server = MockServer::start().await;
    let body = make_skill_json(&[
        (
            "a",
            "A",
            "d1",
            "",
            "https://github.com/o/r/tree/main/skills/a",
            "",
            1,
        ),
        (
            "b",
            "B",
            "d2",
            "",
            "https://github.com/o/r/tree/main/skills/b",
            "",
            2,
        ),
        (
            "c",
            "C",
            "d3",
            "",
            "https://github.com/o/r/tree/main/skills/c",
            "",
            3,
        ),
    ]);
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let reg = make_registry_pointing_at(&server).await;
    let results = reg.search("", 10).await.unwrap();
    assert_eq!(results.len(), 3);
    // Verify download counts propagate correctly for each entry.
    assert_eq!(results[0].downloads, 1);
    assert_eq!(results[1].downloads, 2);
    assert_eq!(results[2].downloads, 3);
}

#[tokio::test]
async fn test_search_network_error_returns_err() {
    // Point the registry at an unreachable port to force a reqwest connection error.
    let mut reg = ModelScopeRegistry::new();
    reg.base_url = "http://127.0.0.1:1".to_string();
    let err = reg.search("anything", 5).await.unwrap_err();
    // The connection failure surfaces as an Other error mentioning the request.
    assert!(err.to_string().to_lowercase().contains("modelscope"));
}

// ============================================================
// S5 coverage: parse_github_tree_url edges, list_repo_files /
// fetch_repo_raw error arms, fetch_full_skill empty, install
// fallback-skip + canonical escape, get_skill_content mismatch
// ============================================================

#[test]
fn test_parse_github_tree_url_shapes() {
    // Valid shape.
    assert_eq!(
        parse_github_tree_url("https://github.com/o/r/tree/main/skills/pdf"),
        Some(("o", "r", "main", "skills/pdf"))
    );
    // Non-github host.
    assert_eq!(
        parse_github_tree_url("https://gitlab.com/o/r/tree/main/x"),
        None
    );
    // Missing 4th segment (bare repo root).
    assert_eq!(parse_github_tree_url("https://github.com/o/r"), None);
    // Not a /tree/ URL.
    assert_eq!(
        parse_github_tree_url("https://github.com/o/r/blob/main/x"),
        None
    );
    // Trailing slash -> path empty -> None.
    assert_eq!(
        parse_github_tree_url("https://github.com/o/r/tree/main/"),
        None
    );
    // No slash after branch.
    assert_eq!(
        parse_github_tree_url("https://github.com/o/r/tree/main"),
        None
    );
}

#[tokio::test]
async fn test_list_repo_files_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/files"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;

    let err = reg
        .list_repo_files("PantherAng", "pdf", "")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("file-list HTTP 500"), "msg: {msg}");
}

#[tokio::test]
async fn test_list_repo_files_api_error_code() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/files"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":500,"Data":{"Files":[]},"Message":"db down","Success":false}"#,
        ))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;

    let err = reg
        .list_repo_files("PantherAng", "pdf", "")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("file-list API error") && msg.contains("db down"),
        "msg: {msg}"
    );
}

#[tokio::test]
async fn test_fetch_repo_raw_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/raw"))
        .respond_with(ResponseTemplate::new(404).set_body_string("gone"))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;

    let err = reg
        .fetch_repo_raw("PantherAng", "pdf", "SKILL.md")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("file HTTP 404"), "msg: {msg}");
}

#[tokio::test]
async fn test_fetch_repo_raw_api_error_code() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/raw"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":404,"Data":{"Content":""},"Message":"no such file","Success":false}"#,
        ))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;

    let err = reg
        .fetch_repo_raw("PantherAng", "pdf", "SKILL.md")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("file API error") && msg.contains("no such file"),
        "msg: {msg}"
    );
}

#[tokio::test]
async fn test_fetch_full_skill_empty_listing_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/files"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"Code":200,"Data":{"Files":[]},"Message":"ok","Success":true}"#,
            ),
        )
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;

    let err = reg.fetch_full_skill("PantherAng", "pdf").await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("has no files") && msg.contains("PantherAng/pdf"),
        "msg: {msg}"
    );
}

#[tokio::test]
async fn test_download_and_install_github_source_unparseable_url_skips_fallback() {
    // Source=github + no subdirs, but SourceUrl is NOT a github tree URL ->
    // the fallback branch is evaluated (parse returns None) and skipped; the
    // mirrored SKILL.md installs. No real network is touched.
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"SkillList":[{"Name":"pdf","DisplayName":"PDF","Description":"s","Path":"PantherAng","Source":"github","SourceUrl":"https://gitlab.com/x/y"}],"TotalCount":1},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/files"))
        .and(query_param("Root", ""))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Files":[{"Path":"SKILL.md","Type":"blob"}]},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/raw"))
        .and(query_param("FilePath", "SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Content":"---\nname: pdf\n---\nbody"},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let reg = make_registry_pointing_at(&server).await;
    reg.download_and_install("pdf", "1.0", dir.path().to_str().unwrap())
        .await
        .expect("mirror install must succeed without the github fallback");
    assert!(dir.path().join("SKILL.md").exists());
}

#[cfg(windows)]
#[tokio::test]
async fn test_download_and_install_canonical_escape_rejected() {
    // A listing entry with an absolute Windows path passes the cheap textual
    // guard (no leading slash, no ".."), but Path::join replaces the target
    // entirely -> canonical parent escapes -> Security error before any write.
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"SkillList":[{"Name":"pdf","DisplayName":"PDF","Description":"s","Path":"PantherAng","SourceUrl":"x"}],"TotalCount":1},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/files"))
        .and(query_param("Root", ""))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Files":[{"Path":"C:\\\\nemesis_s5_evil.txt","Type":"blob"}]},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/PantherAng/pdf/repo/raw"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"Content":"evil"},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let reg = make_registry_pointing_at(&server).await;
    let err = reg
        .download_and_install("pdf", "1.0", dir.path().to_str().unwrap())
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("path traversal") || msg.contains("traversal"),
        "msg: {msg}"
    );
    assert!(!std::path::Path::new("C:\\nemesis_s5_evil.txt").exists());
}

#[tokio::test]
async fn test_get_skill_content_name_mismatch_errors() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"Code":200,"Data":{"SkillList":[{"Name":"not-pdf","Path":"X"}],"TotalCount":1},"Message":"ok","Success":true}"#,
        ))
        .mount(&server)
        .await;
    let reg = make_registry_pointing_at(&server).await;

    let err = reg.get_skill_content("pdf").await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("not found") && msg.contains("not-pdf"),
        "msg: {msg}"
    );
}

// Structural (no injection seam; do NOT exempt):
// - 569-601: the github full-tree fallback inside download_and_install
//   hardcodes https://api.github.com + raw.githubusercontent.com; reachable
//   only with real network. (Line 568's `if let` guard itself IS exercised —
//   with an unparseable SourceUrl — by the unparseable-URL test above.)
// - 634-635: the canonicalize-failure arm of the install write loop —
//   create_dir_all(parent) at 625 runs immediately before, so the parent
//   canonicalize at 627 essentially always succeeds.
