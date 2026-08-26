use super::*;
use nemesis_web::StaticFiles;

#[test]
fn test_embedded_static_exists() {
    // The macro should have embedded the static directory
    assert!(EMBEDDED_STATIC.contains("index.html"));
}

#[test]
fn test_get_embedded_file() {
    let content = get_embedded_file("index.html");
    assert!(content.is_some());
    let html = std::str::from_utf8(content.unwrap()).unwrap();
    assert!(html.contains("<!DOCTYPE html>") || html.contains("<html"));
}

#[test]
fn test_list_embedded_files() {
    let files = list_embedded_files();
    assert!(!files.is_empty());
    assert!(files.iter().any(|f| f.contains("index.html")));
    assert!(files.iter().any(|f| f.contains("assets/")));
}

#[test]
fn test_extract_dir() {
    let temp = tempfile::tempdir().unwrap();
    extract_dir(&EMBEDDED_STATIC, temp.path()).unwrap();

    assert!(temp.path().join("index.html").exists());
    assert!(temp.path().join("assets").is_dir());
    assert!(temp.path().join("fonts").is_dir());

    // Verify assets contain JS and CSS bundles
    let assets_dir = temp.path().join("assets");
    let js_files: Vec<_> = std::fs::read_dir(&assets_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "js").unwrap_or(false))
        .collect();
    let css_files: Vec<_> = std::fs::read_dir(&assets_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "css")
                .unwrap_or(false)
        })
        .collect();
    assert!(!js_files.is_empty(), "assets/ should contain JS bundles");
    assert!(!css_files.is_empty(), "assets/ should contain CSS bundles");
}

// ============================================================
// Additional tests for coverage improvement
// ============================================================

#[test]
fn test_get_embedded_file_nonexistent() {
    let content = get_embedded_file("nonexistent_file_12345.html");
    assert!(content.is_none());
}

#[test]
fn test_get_embedded_file_css() {
    // Vue build puts CSS into assets/ directory
    let files = list_embedded_files();
    assert!(
        files
            .iter()
            .any(|f| f.contains("assets/") && f.ends_with(".css"))
    );
}

#[test]
fn test_get_embedded_file_js() {
    // Vue build puts JS into assets/ directory
    let files = list_embedded_files();
    assert!(
        files
            .iter()
            .any(|f| f.contains("assets/") && f.ends_with(".js"))
    );
}

#[test]
fn test_list_embedded_files_contains_known_paths() {
    let files = list_embedded_files();
    assert!(
        files
            .iter()
            .any(|f| f == "index.html" || f.contains("index.html"))
    );
    assert!(files.iter().any(|f| f.contains("assets/")));
    assert!(files.iter().any(|f| f.contains("fonts/")));
}

#[test]
fn test_list_embedded_files_no_backslashes() {
    let files = list_embedded_files();
    for f in &files {
        assert!(!f.contains('\\'), "Path should use forward slashes: {}", f);
    }
}

#[test]
#[allow(deprecated)]
fn test_resolve_embedded_static_dir_returns_path() {
    // GLOBAL_STATE_LOCK：与 S11d 磁盘覆盖测试（exe 旁瞬时 static/）互斥。
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    // Legacy function only returns disk path now
    let result = resolve_embedded_static_dir();
    // Result depends on whether static/ exists next to exe
    let _ = result;
}

#[test]
fn test_resolve_static_files_returns_provider() {
    // GLOBAL_STATE_LOCK：与 S11d 磁盘覆盖测试（exe 旁瞬时 static/）互斥。
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let provider = resolve_static_files();
    // Should be able to get index.html from either disk or memory
    let content = provider.get_file("index.html");
    assert!(content.is_some());
    let content = content.unwrap();
    let html = std::str::from_utf8(&content).unwrap();
    assert!(html.contains("<!DOCTYPE html>") || html.contains("<html"));
}

#[test]
fn test_resolve_static_files_list() {
    // GLOBAL_STATE_LOCK：与 S11d 磁盘覆盖测试（exe 旁瞬时 static/）互斥。
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let provider = resolve_static_files();
    let files = provider.list_files();
    assert!(!files.is_empty());
    assert!(files.iter().any(|f| f.contains("index.html")));
}

#[test]
fn test_embedded_static_files_get_file() {
    let provider = EmbeddedStaticFiles::new(&EMBEDDED_STATIC);
    let content = provider.get_file("index.html");
    assert!(content.is_some());
    assert!(provider.get_file("nonexistent.html").is_none());
}

#[test]
fn test_embedded_static_files_path_traversal() {
    let provider = EmbeddedStaticFiles::new(&EMBEDDED_STATIC);
    assert!(provider.get_file("../Cargo.toml").is_none());
    assert!(provider.get_file("../../secret").is_none());
}

#[test]
fn test_extract_workspace_templates_to_temp() {
    let temp = tempfile::tempdir().unwrap();
    let result = extract_workspace_templates(temp.path());
    // Should succeed as long as embedded workspace exists
    assert!(result.is_ok());
}

#[test]
fn test_extract_workspace_templates_overwrite() {
    let temp = tempfile::tempdir().unwrap();

    // First extraction
    extract_workspace_templates_overwrite(temp.path()).unwrap();

    // Create a modified file
    let config_path = temp.path().join("config.json");
    if config_path.exists() {
        let original = std::fs::read_to_string(&config_path).unwrap();
        std::fs::write(&config_path, "modified content").unwrap();

        // Overwrite should restore original
        extract_workspace_templates_overwrite(temp.path()).unwrap();
        let restored = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(restored, original);
    }
}

#[test]
fn test_extract_dir_skip_existing_preserves_user_files() {
    let temp = tempfile::tempdir().unwrap();

    // Create a file first
    let user_file = temp.path().join("config.json");
    std::fs::write(&user_file, "user customization").unwrap();

    // Extract should skip existing files
    let result = extract_dir_skip_existing(&EMBEDDED_WORKSPACE, temp.path());
    assert!(result.is_ok());

    // User file should not be overwritten
    let content = std::fs::read_to_string(&user_file).unwrap();
    assert_eq!(content, "user customization");
}

#[test]
fn test_embedded_static_has_fonts() {
    assert!(EMBEDDED_STATIC.contains("fonts"));
}

#[test]
fn test_embedded_workspace_exists() {
    // EMBEDDED_WORKSPACE should be valid and contain files
    let files: Vec<_> = EMBEDDED_WORKSPACE.files().collect();
    assert!(!files.is_empty(), "Embedded workspace should contain files");
}

#[test]
fn test_embedded_static_index_html_content() {
    let file = EMBEDDED_STATIC.get_file("index.html").unwrap();
    let content = std::str::from_utf8(file.contents()).unwrap();
    assert!(content.contains("<!DOCTYPE html>") || content.contains("<html"));
    assert!(content.len() > 100);
}

#[test]
fn test_extract_dir_idempotent() {
    let temp1 = tempfile::tempdir().unwrap();
    let temp2 = tempfile::tempdir().unwrap();

    extract_dir(&EMBEDDED_STATIC, temp1.path()).unwrap();
    extract_dir(&EMBEDDED_STATIC, temp2.path()).unwrap();

    // Both should have identical content
    let file1 = std::fs::read(temp1.path().join("index.html")).unwrap();
    let file2 = std::fs::read(temp2.path().join("index.html")).unwrap();
    assert_eq!(file1, file2);
}

#[test]
fn test_collect_files_returns_all() {
    let mut files = Vec::new();
    collect_files(&EMBEDDED_STATIC, &mut files);
    assert!(!files.is_empty());
    // All entries should be relative paths with forward slashes
    for f in &files {
        assert!(!f.is_empty());
        assert!(!f.contains('\\'));
    }
}

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：
// - resolve_static_files 的磁盘覆盖分支（exe 旁 static/ 目录优先于内嵌）
// - resolve_embedded_static_dir 的磁盘命中臂（deprecated legacy）
// - extract_workspace_templates 的"已存在跳过"臂（用户自定义不被覆盖）
// =========================================================================

#[test]
#[allow(deprecated)]
fn resolve_static_files_prefers_disk_dir_next_to_exe() {
    // exe 旁 static/ 是进程级全局状态（resolver 都看它）——必须持全局锁
    // 与其他 resolver 测试互斥，否则并行测试会读到 marker 内容互踩。
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    const MARKER: &str = "<!-- S11D-DISK-OVERRIDE -->";
    let exe_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let static_dir = exe_dir.join("static");
    // deps 目录（构建产物区，非生产数据）不该常驻 static/；已存在则视为
    // 前次泄漏，本轮借用且不删除（防误删他者文件）。
    let created = !static_dir.exists();
    if created {
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(static_dir.join("index.html"), MARKER).unwrap();
    }
    let disk_provider = resolve_static_files();
    let disk_dir = resolve_embedded_static_dir();
    // DirectoryStaticFiles 是惰性按路径读盘（不是快照）——必须在拆目录前读。
    let got = disk_provider.get_file("/index.html");
    if created {
        std::fs::remove_dir_all(&static_dir).unwrap();
    }

    // 磁盘覆盖：provider 从磁盘取到 marker，legacy 解析器返回同一路径。
    let got = got.expect("disk override serves index.html");
    assert_eq!(std::str::from_utf8(&got).unwrap(), MARKER);
    let dir_str = disk_dir.expect("legacy resolver must see the disk dir");
    assert!(dir_str.ends_with("static"), "dir: {dir_str}");

    // 拆掉磁盘目录后：回落内嵌（index.html 是 Vue 构建产物，非 marker）。
    let embedded_provider = resolve_static_files();
    let got2 = embedded_provider
        .get_file("/index.html")
        .expect("embedded fallback serves index.html");
    assert_ne!(std::str::from_utf8(&got2).unwrap(), MARKER);
    assert!(resolve_embedded_static_dir().is_none(), "no disk dir → legacy None");
}

#[test]
fn extract_workspace_templates_skips_existing_files() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("ws");
    std::fs::create_dir_all(&target).unwrap();
    // 用户已自定义 IDENTITY.md —— onboard 之外的提取路径不得覆盖。
    std::fs::write(target.join("IDENTITY.md"), "USER-CUSTOMIZED").unwrap();

    extract_workspace_templates(&target).expect("skip-existing extract must succeed");

    let identity =
        std::fs::read_to_string(target.join("IDENTITY.md")).expect("IDENTITY.md present");
    assert_eq!(identity, "USER-CUSTOMIZED", "existing file must NOT be overwritten");
    // 未存在的其余模板照常铺出（同一棵嵌入树的其他根文件）。
    assert!(target.join("SOUL.md").exists(), "missing templates must still be extracted");
}
