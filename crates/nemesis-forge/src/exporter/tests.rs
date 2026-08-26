use super::*;
use std::sync::Arc;

fn make_artifact(name: &str, kind: ArtifactKind, status: ArtifactStatus) -> Artifact {
    Artifact {
        id: format!("test-{}", name),
        name: name.to_string(),
        kind,
        version: "1.0".to_string(),
        status,
        content: format!("# {}\nTest content", name),
        tool_signature: vec![],
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
    }
}

#[tokio::test]
async fn test_export_skill() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);

    let path = exporter
        .export(
            ArtifactKind::Skill,
            "my-skill",
            "---\nname: test\n---\nContent",
        )
        .await
        .unwrap();

    assert!(path.exists());
    let content = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(content.contains("test"));
}

#[tokio::test]
async fn test_export_script() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);

    let path = exporter
        .export(ArtifactKind::Script, "my-script", "#!/bin/bash\necho hello")
        .await
        .unwrap();

    assert!(path.exists());
}

#[tokio::test]
async fn test_copy_skill_to_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);

    let path = exporter
        .copy_skill_to_workspace("test-skill", "content")
        .await
        .unwrap();

    assert!(path.to_string_lossy().contains("test-skill-forge"));
    assert!(path.exists());
}

#[tokio::test]
async fn test_export_artifact_with_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);

    // Write artifact content to its forge dir first
    let artifact = make_artifact("test-skill", ArtifactKind::Skill, ArtifactStatus::Active);
    let artifact_dir = dir.path().join("forge").join("skills").join("test-skill");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("SKILL.md"), &artifact.content).unwrap();

    let target_dir = dir.path().join("export");
    let export_dir = exporter
        .export_artifact(&artifact, &target_dir)
        .await
        .unwrap();

    // Check main file
    assert!(export_dir.join("SKILL.md").exists());

    // Check manifest
    let manifest_path = export_dir.join("forge-manifest.json");
    assert!(manifest_path.exists());
    let manifest: ExportManifest =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest.name, "test-skill");
    assert_eq!(manifest.version, "1.0");
    assert!(manifest.files.contains(&"SKILL.md".to_string()));
}

#[tokio::test]
async fn test_export_all_active_only() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(crate::registry::Registry::new(
        crate::types::RegistryConfig::default(),
    ));

    // Add artifacts
    let active = make_artifact("active-skill", ArtifactKind::Skill, ArtifactStatus::Active);
    registry.add(active.clone());

    let draft = make_artifact("draft-skill", ArtifactKind::Skill, ArtifactStatus::Draft);
    registry.add(draft.clone());

    let config = ExportConfig::with_registry(dir.path(), registry);
    let exporter = Exporter::new(config);

    // Write artifact content
    let artifact_dir = dir.path().join("forge").join("skills").join("active-skill");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("SKILL.md"), &active.content).unwrap();

    let target_dir = dir.path().join("export-all");
    let count = exporter.export_all(&target_dir).await.unwrap();

    assert_eq!(count, 1); // Only the active one
    assert!(target_dir.join("active-skill-1.0").exists());
    assert!(!target_dir.join("draft-skill-1.0").exists());
}

#[tokio::test]
async fn test_export_all_no_registry() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);

    let count = exporter
        .export_all(dir.path().join("target").as_path())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// --- Additional exporter tests ---

#[test]
fn test_export_config_new() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    assert_eq!(config.forge_dir, dir.path().join("forge"));
    assert_eq!(config.workspace, dir.path().to_path_buf());
}

#[tokio::test]
async fn test_export_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);
    let path = exporter
        .export(ArtifactKind::Mcp, "my-mcp", "mcp content")
        .await
        .unwrap();
    assert!(path.exists());
}

#[tokio::test]
async fn test_copy_skill_creates_directory() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);
    let ws_dir = dir.path().join("skills").join("new-skill-forge");
    assert!(!ws_dir.exists());
    exporter
        .copy_skill_to_workspace("new-skill", "test")
        .await
        .unwrap();
    assert!(ws_dir.exists());
    assert!(ws_dir.join("SKILL.md").exists());
}

#[tokio::test]
async fn test_export_manifest_json_structure() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);
    let artifact = make_artifact("manifest-test", ArtifactKind::Skill, ArtifactStatus::Active);
    let artifact_dir = dir
        .path()
        .join("forge")
        .join("skills")
        .join("manifest-test");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("SKILL.md"), &artifact.content).unwrap();
    let target_dir = dir.path().join("export");
    let export_dir = exporter
        .export_artifact(&artifact, &target_dir)
        .await
        .unwrap();
    let manifest_path = export_dir.join("forge-manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["type"], "skill");
}

#[tokio::test]
async fn test_export_artifact_draft_status() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);
    let artifact = make_artifact("draft", ArtifactKind::Skill, ArtifactStatus::Draft);
    let artifact_dir = dir.path().join("forge").join("skills").join("draft");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("SKILL.md"), &artifact.content).unwrap();
    let target_dir = dir.path().join("export");
    // Draft artifacts should still be exportable individually
    let result = exporter.export_artifact(&artifact, &target_dir).await;
    assert!(result.is_ok());
}

#[test]
fn test_exporter_forge_dir_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);
    assert!(exporter.forge_dir().ends_with("forge"));
}

#[test]
fn test_exporter_workspace_accessor() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);
    assert_eq!(exporter.workspace(), dir.path());
}

#[tokio::test]
async fn test_export_artifact_script_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);

    let artifact = make_artifact("test-script", ArtifactKind::Script, ArtifactStatus::Active);
    let artifact_dir = dir.path().join("forge").join("scripts").join("test-script");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("test-script"), &artifact.content).unwrap();

    let target_dir = dir.path().join("export");
    let export_dir = exporter
        .export_artifact(&artifact, &target_dir)
        .await
        .unwrap();
    assert!(export_dir.join("test-script").exists());
    assert!(export_dir.join("forge-manifest.json").exists());
}

#[tokio::test]
async fn test_export_artifact_mcp_type() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);

    let artifact = make_artifact("test-mcp", ArtifactKind::Mcp, ArtifactStatus::Active);
    let artifact_dir = dir.path().join("forge").join("mcp").join("test-mcp");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("server.py"), &artifact.content).unwrap();

    let target_dir = dir.path().join("export");
    let export_dir = exporter
        .export_artifact(&artifact, &target_dir)
        .await
        .unwrap();
    assert!(export_dir.join("server.py").exists());
}

#[tokio::test]
async fn test_export_artifact_with_project_files() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);

    let artifact = make_artifact("proj-skill", ArtifactKind::Skill, ArtifactStatus::Active);
    let artifact_dir = dir.path().join("forge").join("skills").join("proj-skill");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("SKILL.md"), &artifact.content).unwrap();
    std::fs::write(artifact_dir.join("requirements.txt"), "requests\n").unwrap();
    std::fs::write(artifact_dir.join("README.md"), "# Readme\n").unwrap();

    let target_dir = dir.path().join("export");
    let export_dir = exporter
        .export_artifact(&artifact, &target_dir)
        .await
        .unwrap();

    assert!(export_dir.join("requirements.txt").exists());
    assert!(export_dir.join("README.md").exists());
}

#[tokio::test]
async fn test_export_artifact_with_tests_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);

    let artifact = make_artifact("tested-skill", ArtifactKind::Skill, ArtifactStatus::Active);
    let artifact_dir = dir.path().join("forge").join("skills").join("tested-skill");
    std::fs::create_dir_all(artifact_dir.join("tests")).unwrap();
    std::fs::write(artifact_dir.join("SKILL.md"), &artifact.content).unwrap();
    std::fs::write(
        artifact_dir.join("tests/test_main.py"),
        "def test_it(): pass\n",
    )
    .unwrap();

    let target_dir = dir.path().join("export");
    let export_dir = exporter
        .export_artifact(&artifact, &target_dir)
        .await
        .unwrap();

    assert!(export_dir.join("tests").is_dir());
    assert!(export_dir.join("tests/test_main.py").exists());
}

#[tokio::test]
async fn test_export_all_with_empty_registry() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(crate::registry::Registry::new(
        crate::types::RegistryConfig::default(),
    ));
    let config = ExportConfig::with_registry(dir.path(), registry);
    let exporter = Exporter::new(config);

    let target_dir = dir.path().join("export");
    let count = exporter.export_all(&target_dir).await.unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_artifact_dir_skill() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);
    let path = exporter.artifact_dir(ArtifactKind::Skill, "test");
    assert!(path.to_string_lossy().contains("skills"));
    assert!(path.to_string_lossy().contains("test"));
}

#[tokio::test]
async fn test_artifact_dir_script() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);
    let path = exporter.artifact_dir(ArtifactKind::Script, "myscript");
    assert!(path.to_string_lossy().contains("scripts"));
}

#[tokio::test]
async fn test_artifact_dir_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);
    let path = exporter.artifact_dir(ArtifactKind::Mcp, "mymcp");
    assert!(path.to_string_lossy().contains("mcp"));
}

#[test]
fn test_export_manifest_serialization() {
    let manifest = ExportManifest {
        id: "test-id".to_string(),
        kind: "skill".to_string(),
        name: "test-name".to_string(),
        version: "1.0".to_string(),
        exported_at: "2026-01-01T00:00:00Z".to_string(),
        files: vec!["SKILL.md".to_string()],
    };
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(json.contains("test-id"));
    assert!(json.contains("skill"));
    assert!(json.contains("SKILL.md"));
}

#[test]
fn test_export_manifest_deserialization() {
    let json = r#"{"id":"x","type":"script","name":"n","version":"2.0","exported_at":"2026-01-01","files":[]}"#;
    let manifest: ExportManifest = serde_json::from_str(json).unwrap();
    assert_eq!(manifest.id, "x");
    assert_eq!(manifest.kind, "script");
    assert!(manifest.files.is_empty());
}

// --- copy_dir_recursive direct coverage ---

#[tokio::test]
async fn test_copy_dir_recursive_non_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("dst");
    tokio::fs::create_dir_all(&dst).await.unwrap();

    // Nonexistent path is not a dir -> empty result, no panic.
    let copied = copy_dir_recursive(Path::new("definitely/not/here"), &dst).await;
    assert!(copied.is_empty());

    // A FILE src is also not a dir -> empty result.
    let file_src = dir.path().join("src_file.txt");
    tokio::fs::write(&file_src, "x").await.unwrap();
    let copied = copy_dir_recursive(&file_src, &dst).await;
    assert!(copied.is_empty());
}

#[tokio::test]
async fn test_copy_dir_recursive_nested_subdirs() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    tokio::fs::create_dir_all(src.join("sub")).await.unwrap();
    tokio::fs::write(src.join("top.txt"), "top").await.unwrap();
    tokio::fs::write(src.join("sub").join("nested.txt"), "nested").await.unwrap();

    let copied = copy_dir_recursive(&src, &dst).await;
    assert_eq!(copied.len(), 2, "both files must be reported: {:?}", copied);
    assert!(copied.contains(&"top.txt".to_string()));
    assert!(
        copied.contains(&"sub/nested.txt".to_string()),
        "nested file reported with subdir prefix: {:?}",
        copied
    );
    assert!(dst.join("top.txt").exists());
    assert_eq!(
        tokio::fs::read_to_string(dst.join("sub").join("nested.txt"))
            .await
            .unwrap(),
        "nested"
    );
}

#[tokio::test]
async fn test_copy_dir_recursive_write_fail_skips_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    tokio::fs::create_dir_all(&src).await.unwrap();
    tokio::fs::create_dir_all(&dst).await.unwrap();
    tokio::fs::write(src.join("good.txt"), "good").await.unwrap();
    tokio::fs::write(src.join("blocked.txt"), "blocked").await.unwrap();

    // Pre-create a DIRECTORY where blocked.txt would be written -> write fails,
    // the file is skipped, but good.txt is still copied.
    tokio::fs::create_dir_all(dst.join("blocked.txt")).await.unwrap();

    let copied = copy_dir_recursive(&src, &dst).await;
    assert_eq!(copied.len(), 1, "only the writable file reported: {:?}", copied);
    assert!(copied.contains(&"good.txt".to_string()));
    assert!(dst.join("good.txt").exists());
}

// --- S8 coverage additions (quality-hardening goal 冲刺 S8) ---

#[tokio::test]
async fn test_s8_export_artifact_copies_project_files_and_tests_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = ExportConfig::new(dir.path());
    let exporter = Exporter::new(config);

    let artifact = make_artifact("proj-skill", ArtifactKind::Skill, ArtifactStatus::Active);

    // Project-structure files live next to the artifact's source dir.
    let src_dir = dir.path().join("forge").join("skills").join("proj-skill");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("requirements.txt"), "flask\n").unwrap();
    std::fs::write(src_dir.join("README.md"), "# readme").unwrap();

    // tests/ subdir with both a flat file and a nested subdirectory.
    let tests_src = src_dir.join("tests");
    std::fs::create_dir_all(tests_src.join("sub")).unwrap();
    std::fs::write(tests_src.join("test_a.py"), "def test_a(): pass").unwrap();
    std::fs::write(tests_src.join("sub").join("test_b.py"), "def test_b(): pass").unwrap();

    let target = dir.path().join("export");
    let out = exporter.export_artifact(&artifact, &target).await.unwrap();

    assert!(out.join("requirements.txt").exists());
    assert!(out.join("README.md").exists());
    assert!(out.join("tests").join("test_a.py").exists());
    assert!(out.join("tests").join("sub").join("test_b.py").exists());

    let manifest = std::fs::read_to_string(out.join("forge-manifest.json")).unwrap();
    assert!(manifest.contains("requirements.txt"));
    assert!(manifest.contains("tests/test_a.py"));
    assert!(manifest.contains("tests/sub/test_b.py"));
}
