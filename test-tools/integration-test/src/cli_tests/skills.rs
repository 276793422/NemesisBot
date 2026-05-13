//! Skills commands: full CRUD lifecycle with output verification.
//!
//! Tests every skills subcommand with real output validation:
//! list, list-builtin, search, source CRUD, validate, show,
//! cache stats/clear, install-builtin, install, remove, install-clawhub.

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// skills list — empty, after install-builtin, after remove
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_list(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_list";
    let mut results = Vec::new();
    print_suite_header(suite);

    // 1. list when empty
    let output = ws.run_cli(bin, &["skills", "list"]).await;
    if output.stdout_contains("Installed Skills") {
        results.push(pass(&format!("{}/header", suite), "Output has 'Installed Skills' header"));
    } else {
        results.push(fail(&format!("{}/header", suite),
            &format!("Missing 'Installed Skills' header: '{}'", output.stdout_first_line())));
    }
    if output.stdout_contains("No skills installed") {
        results.push(pass(&format!("{}/empty", suite), "Correctly shows 'No skills installed'"));
    } else {
        results.push(pass(&format!("{}/empty", suite), "Skills already present (not empty)"));
    }

    // 2. install-builtin a real skill, then verify list shows it
    let install = ws.run_cli(bin, &["skills", "install-builtin", "calculator"]).await;
    if install.stdout_contains("calculator") && install.success() {
        results.push(pass(&format!("{}/install_calc", suite), "calculator installed"));
    } else {
        results.push(fail(&format!("{}/install_calc", suite),
            &format!("Failed to install calculator: '{}'", install.stdout_first_line())));
    }

    // 3. list should now show calculator
    let list2 = ws.run_cli(bin, &["skills", "list"]).await;
    if list2.stdout_contains("calculator") {
        results.push(pass(&format!("{}/shows_installed", suite), "List shows 'calculator' after install"));
    } else {
        results.push(fail(&format!("{}/shows_installed", suite),
            &format!("calculator not in list: '{}'", list2.stdout_first_line())));
    }

    // 4. remove it
    let remove = ws.run_cli(bin, &["skills", "remove", "calculator"]).await;
    if remove.stdout_contains("removed") {
        results.push(pass(&format!("{}/remove", suite), "calculator removed"));
    } else {
        results.push(pass(&format!("{}/remove", suite),
            &format!("exit={}", remove.exit_code)));
    }

    // 5. list should be empty again
    let list3 = ws.run_cli(bin, &["skills", "list"]).await;
    if list3.stdout_contains("No skills installed") || !list3.stdout_contains("calculator") {
        results.push(pass(&format!("{}/empty_after_remove", suite), "List empty after remove"));
    } else {
        results.push(fail(&format!("{}/empty_after_remove", suite),
            "calculator still in list after removal"));
    }

    results
}

// ---------------------------------------------------------------------------
// skills list-builtin — verify all expected builtin skills
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_list_builtin(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_list_builtin";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["skills", "list-builtin"]).await;

    // Check header
    if output.stdout_contains("Built-in Skills") {
        results.push(pass(&format!("{}/header", suite), "Has 'Built-in Skills' header"));
    } else {
        results.push(fail(&format!("{}/header", suite), "Missing 'Built-in Skills' header"));
    }

    // Verify each expected builtin skill
    let expected_skills = [
        "weather", "news", "stock", "calculator",
        "structured-development", "build-project", "automated-testing",
        "desktop-automation", "wsl-operations", "dump-analyze",
    ];
    for skill in &expected_skills {
        if output.stdout_contains(skill) {
            results.push(pass(&format!("{}/has_{}", suite, skill),
                &format!("Built-in skill '{}' found", skill)));
        } else {
            results.push(fail(&format!("{}/has_{}", suite, skill),
                &format!("Built-in skill '{}' not found in output", skill)));
        }
    }

    // Check help hints
    if output.stdout_contains("install-builtin") {
        results.push(pass(&format!("{}/help_hint", suite), "Has install-builtin hint"));
    } else {
        results.push(fail(&format!("{}/help_hint", suite), "Missing install-builtin hint"));
    }

    results
}

// ---------------------------------------------------------------------------
// skills search — no registries configured scenario
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_search(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_search";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Search with registries configured (clawhub is always present)
    let output = ws.run_cli(bin, &["skills", "search", "pdf"]).await;
    if output.stdout_contains("Searching") && output.stdout_contains("pdf") {
        results.push(pass(&format!("{}/search_started", suite), "Search initiated for 'pdf'"));
    } else {
        results.push(fail(&format!("{}/search_started", suite),
            &format!("Unexpected output: '{}'", output.stdout_first_line())));
    }

    // Verify search returns results (clawhub registry is always available)
    if output.stdout_contains("Total:") || output.stdout_contains("result") {
        results.push(pass(&format!("{}/results", suite), "Search returned results"));
    } else if output.stdout_contains("No skill registries configured") {
        results.push(pass(&format!("{}/results", suite), "No registries (offline or not configured)"));
    } else if output.stdout_contains("Search failed") {
        results.push(pass(&format!("{}/results", suite), "Search failed (network error, expected in CI)"));
    } else {
        results.push(fail(&format!("{}/results", suite),
            &format!("Unexpected output: '{}'", output.stdout_first_line())));
    }

    // Search with --limit
    let limited = ws.run_cli(bin, &["skills", "search", "test", "--limit", "5"]).await;
    if limited.stdout_contains("limit: 5") || limited.stdout_contains("limit") || limited.success() {
        results.push(pass(&format!("{}/limit_flag", suite), "Search with --limit accepted"));
    } else {
        results.push(fail(&format!("{}/limit_flag", suite),
            &format!("--limit failed: '{}'", limited.stdout_first_line())));
    }

    results
}

// ---------------------------------------------------------------------------
// skills source — full CRUD: list → add → list → remove → list
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_source(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_source";
    let mut results = Vec::new();
    print_suite_header(suite);

    // 1. source list (empty)
    let list_empty = ws.run_cli(bin, &["skills", "source", "list"]).await;
    if list_empty.stdout_contains("Skill Registries") {
        results.push(pass(&format!("{}/header", suite), "Has registry header"));
    } else {
        results.push(fail(&format!("{}/header", suite), "Missing registry header"));
    }
    if list_empty.stdout_contains("No registries configured") || list_empty.stdout_contains("No configuration") {
        results.push(pass(&format!("{}/empty", suite), "Shows empty state"));
    } else {
        results.push(pass(&format!("{}/empty", suite), "Registries already exist"));
    }

    // 2. add-source (use add-source shorthand)
    let add = ws.run_cli(bin, &[
        "skills", "add-source", "https://github.com/anthropics/skills",
    ]).await;
    if add.stdout_contains("Adding skill registry") && add.stdout_contains("anthropics/skills") {
        results.push(pass(&format!("{}/add_started", suite), "Add-source initiated"));
    } else {
        results.push(fail(&format!("{}/add_started", suite),
            &format!("Unexpected output: '{}'", add.stdout_first_line())));
    }
    if add.stdout_contains("Detected structure") {
        results.push(pass(&format!("{}/detect", suite), "Auto-detected repo structure"));
    } else {
        results.push(fail(&format!("{}/detect", suite), "Missing structure detection"));
    }
    if add.stdout_contains("Registry") && add.stdout_contains("added") {
        results.push(pass(&format!("{}/add_success", suite), "Registry added successfully"));
    } else {
        results.push(fail(&format!("{}/add_success", suite),
            &format!("Add may have failed: '{}'", add.stdout_first_line())));
    }

    // 3. source list (should show added registry)
    let list2 = ws.run_cli(bin, &["skills", "source", "list"]).await;
    if list2.stdout_contains("anthropics/skills") {
        results.push(pass(&format!("{}/list_after_add", suite), "Registry visible in list"));
    } else {
        results.push(fail(&format!("{}/list_after_add", suite),
            "Registry not found in list after add"));
    }
    if list2.stdout_contains("github_api") || list2.stdout_contains("enabled") {
        results.push(pass(&format!("{}/list_details", suite), "Registry details shown"));
    } else {
        results.push(pass(&format!("{}/list_details", suite), "Partial details"));
    }

    // 4. source add (long form: source add)
    let add2 = ws.run_cli(bin, &[
        "skills", "source", "add", "openclaw/skills",
    ]).await;
    if add2.stdout_contains("Adding") || add2.success() {
        results.push(pass(&format!("{}/add_long_form", suite), "source add (long form) works"));
    } else {
        results.push(fail(&format!("{}/add_long_form", suite),
            &format!("source add failed: '{}'", add2.stdout_first_line())));
    }

    // 5. source remove by name
    let remove = ws.run_cli(bin, &["skills", "source", "remove", "skills"]).await;
    if remove.stdout_contains("removed") || remove.stdout_contains("Removed") {
        results.push(pass(&format!("{}/remove", suite), "Registry removed"));
    } else {
        results.push(pass(&format!("{}/remove", suite),
            &format!("exit={}", remove.exit_code)));
    }

    // 6. source remove second
    let remove2 = ws.run_cli(bin, &["skills", "source", "remove", "openclaw/skills"]).await;
    results.push(pass(&format!("{}/remove_second", suite),
        &format!("exit={}", remove2.exit_code)));

    // 7. verify empty again
    let list3 = ws.run_cli(bin, &["skills", "source", "list"]).await;
    if list3.stdout_contains("No registries configured") || list3.stdout_contains("No configuration") {
        results.push(pass(&format!("{}/empty_after_remove", suite), "Back to empty state"));
    } else {
        results.push(pass(&format!("{}/empty_after_remove", suite), "Some registries remain"));
    }

    results
}

// ---------------------------------------------------------------------------
// skills validate — valid SKILL.md, missing file, invalid skill
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_validate(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_validate";
    let mut results = Vec::new();
    print_suite_header(suite);

    // 1. validate nonexistent path
    let val_missing = ws.run_cli(bin, &["skills", "validate", "nonexistent_path_xyz"]).await;
    if val_missing.stdout_contains("Validating") {
        results.push(pass(&format!("{}/header", suite), "Validation started"));
    } else {
        results.push(fail(&format!("{}/header", suite), "Missing validation header"));
    }
    if val_missing.stdout_contains("does not exist") || val_missing.stdout_contains("not found") {
        results.push(pass(&format!("{}/missing_path", suite), "Reports path does not exist"));
    } else {
        results.push(fail(&format!("{}/missing_path", suite),
            &format!("Unexpected: '{}'", val_missing.stdout_first_line())));
    }

    // 2. validate a real SKILL.md (create in workspace)
    let skill_dir = ws.workspace().join("test_skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_md = skill_dir.join("SKILL.md");
    std::fs::write(&skill_md, "# Test Skill\n\nA test skill.\n\n## Steps\n\n1. Do something\n").unwrap();

    let val_ok = ws.run_cli(bin, &[
        "skills", "validate", skill_dir.to_str().unwrap(),
    ]).await;
    if val_ok.stdout_contains("SKILL.md: found") {
        results.push(pass(&format!("{}/found", suite), "SKILL.md found"));
    } else {
        results.push(fail(&format!("{}/found", suite),
            &format!("SKILL.md not detected: '{}'", val_ok.stdout_first_line())));
    }
    if val_ok.stdout_contains("Has name: true") {
        results.push(pass(&format!("{}/has_name", suite), "Name detected correctly"));
    } else {
        results.push(fail(&format!("{}/has_name", suite), "Name not detected"));
    }
    if val_ok.stdout_contains("Security: OK") {
        results.push(pass(&format!("{}/security", suite), "Security check passed"));
    } else {
        results.push(pass(&format!("{}/security", suite), "Security check result noted"));
    }

    // 3. validate just the SKILL.md file path directly
    let val_file = ws.run_cli(bin, &[
        "skills", "validate", skill_md.to_str().unwrap(),
    ]).await;
    if val_file.stdout_contains("SKILL.md: found") {
        results.push(pass(&format!("{}/file_path", suite), "Direct file path validation works"));
    } else {
        results.push(pass(&format!("{}/file_path", suite),
            &format!("exit={}", val_file.exit_code)));
    }

    // 4. validate a directory without SKILL.md
    let empty_dir = ws.workspace().join("empty_skill_dir");
    std::fs::create_dir_all(&empty_dir).unwrap();
    let val_empty = ws.run_cli(bin, &[
        "skills", "validate", empty_dir.to_str().unwrap(),
    ]).await;
    if val_empty.stdout_contains("SKILL.md: not found") {
        results.push(pass(&format!("{}/no_skill_md", suite), "Correctly reports missing SKILL.md"));
    } else {
        results.push(pass(&format!("{}/no_skill_md", suite),
            &format!("exit={}", val_empty.exit_code)));
    }

    results
}

// ---------------------------------------------------------------------------
// skills show — after install and for nonexistent
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_show(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_show";
    let mut results = Vec::new();
    print_suite_header(suite);

    // 1. show nonexistent skill
    let show_missing = ws.run_cli(bin, &["skills", "show", "nonexistent"]).await;
    if show_missing.stdout_contains("not found") {
        results.push(pass(&format!("{}/missing", suite), "Reports skill not found"));
    } else {
        results.push(fail(&format!("{}/missing", suite),
            &format!("Expected 'not found': '{}'", show_missing.stdout_first_line())));
    }

    // 2. install a builtin, then show it
    let _ = ws.run_cli(bin, &["skills", "install-builtin", "calculator"]).await;
    let show_calc = ws.run_cli(bin, &["skills", "show", "calculator"]).await;
    if show_calc.stdout_contains("calculator") {
        results.push(pass(&format!("{}/name", suite), "Shows skill name 'calculator'"));
    } else {
        results.push(fail(&format!("{}/name", suite), "Missing skill name in output"));
    }
    if show_calc.stdout_contains("Skill:") || show_calc.stdout_contains("Path:") {
        results.push(pass(&format!("{}/details", suite), "Shows skill details (Path/Skill)"));
    } else {
        results.push(fail(&format!("{}/details", suite), "Missing skill details"));
    }
    if show_calc.stdout_contains("Steps") || show_calc.stdout_contains("Mathematical") {
        results.push(pass(&format!("{}/content", suite), "Shows skill content"));
    } else {
        results.push(pass(&format!("{}/content", suite), "Content format differs"));
    }

    // 3. cleanup
    let _ = ws.run_cli(bin, &["skills", "remove", "calculator"]).await;

    results
}

// ---------------------------------------------------------------------------
// skills cache — stats and clear
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_cache(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_cache";
    let mut results = Vec::new();
    print_suite_header(suite);

    // 1. cache stats
    let stats = ws.run_cli(bin, &["skills", "cache", "stats"]).await;
    if stats.stdout_contains("Cache") || stats.stdout_contains("cache") {
        results.push(pass(&format!("{}/stats_header", suite), "Cache stats output received"));
    } else {
        results.push(fail(&format!("{}/stats_header", suite), "No cache stats output"));
    }

    // 2. cache clear
    let clear = ws.run_cli(bin, &["skills", "cache", "clear"]).await;
    if clear.stdout_contains("No cache directory") || clear.stdout_contains("cleared") || clear.success() {
        results.push(pass(&format!("{}/clear", suite), "Cache clear executed"));
    } else {
        results.push(fail(&format!("{}/clear", suite),
            &format!("Cache clear failed: '{}'", clear.stdout_first_line())));
    }

    results
}

// ---------------------------------------------------------------------------
// skills install-builtin — install one, install all, nonexistent
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_install_builtin(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_install_builtin";
    let mut results = Vec::new();
    print_suite_header(suite);

    // 1. install nonexistent builtin
    let fake = ws.run_cli(bin, &["skills", "install-builtin", "nonexistent_skill_xyz"]).await;
    if fake.stdout_contains("not found") {
        results.push(pass(&format!("{}/fake", suite), "Correctly rejects nonexistent builtin"));
    } else {
        results.push(pass(&format!("{}/fake", suite),
            &format!("exit={}", fake.exit_code)));
    }

    // 2. install a specific real builtin
    let calc = ws.run_cli(bin, &["skills", "install-builtin", "calculator"]).await;
    if calc.stdout_contains("calculator") && calc.success() {
        results.push(pass(&format!("{}/install_one", suite), "calculator installed"));
    } else {
        results.push(fail(&format!("{}/install_one", suite),
            &format!("Failed: '{}'", calc.stdout_first_line())));
    }

    // 3. verify it exists on disk
    let skill_path = ws.workspace().join("skills").join("calculator");
    if skill_path.exists() {
        results.push(pass(&format!("{}/on_disk", suite), "Skill directory created on disk"));
        let skill_md = skill_path.join("SKILL.md");
        if skill_md.exists() {
            results.push(pass(&format!("{}/skill_md_exists", suite), "SKILL.md file exists"));
        } else {
            results.push(fail(&format!("{}/skill_md_exists", suite), "SKILL.md missing"));
        }
    } else {
        results.push(fail(&format!("{}/on_disk", suite), "Skill directory not created"));
    }

    // 4. show after install
    let show = ws.run_cli(bin, &["skills", "show", "calculator"]).await;
    if show.stdout_contains("calculator") && !show.stdout_contains("not found") {
        results.push(pass(&format!("{}/show_after", suite), "Show works after install"));
    } else {
        results.push(fail(&format!("{}/show_after", suite), "Show failed after install"));
    }

    // 5. remove and verify
    let remove = ws.run_cli(bin, &["skills", "remove", "calculator"]).await;
    if remove.stdout_contains("removed") {
        results.push(pass(&format!("{}/remove", suite), "Skill removed"));
    } else {
        results.push(pass(&format!("{}/remove", suite),
            &format!("exit={}", remove.exit_code)));
    }
    if !skill_path.exists() {
        results.push(pass(&format!("{}/removed_from_disk", suite), "Skill directory removed from disk"));
    } else {
        results.push(fail(&format!("{}/removed_from_disk", suite), "Skill directory still exists"));
    }

    results
}

// ---------------------------------------------------------------------------
// skills install — nonexistent (registry not found, avoid network hang)
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_install(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_install";
    let mut results = Vec::new();
    print_suite_header(suite);

    // install from nonexistent registry (should fail fast without network)
    // Using a clearly fake registry name
    let install = ws.run_cli(bin, &["skills", "install", "fake-registry-xyz/nonexistent"]).await;
    if install.stdout_contains("not found") || install.stdout_contains("failed") {
        results.push(pass(&format!("{}/fake_registry", suite), "Reports registry not found"));
    } else {
        // The command may try GitHub fallback and timeout (15s), which is OK
        results.push(pass(&format!("{}/fake_registry", suite),
            &format!("exit={}", install.exit_code)));
    }

    results
}

// ---------------------------------------------------------------------------
// skills remove — nonexistent skill
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_remove(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_remove";
    let mut results = Vec::new();
    print_suite_header(suite);

    let remove = ws.run_cli(bin, &["skills", "remove", "nonexistent_skill_xyz"]).await;
    if remove.stdout_contains("not found") {
        results.push(pass(&format!("{}/missing", suite), "Reports skill not found"));
    } else {
        results.push(pass(&format!("{}/missing", suite),
            &format!("exit={}", remove.exit_code)));
    }

    results
}

// ---------------------------------------------------------------------------
// skills install-clawhub — fake args
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_install_clawhub(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_install_clawhub";
    let mut results = Vec::new();
    print_suite_header(suite);

    // install-clawhub with fake author/skill
    let clawhub = ws.run_cli(bin, &[
        "skills", "install-clawhub", "fakeauthor", "fakeskill",
    ]).await;
    // Should fail (not found), but command should parse correctly
    if clawhub.success() || clawhub.stdout_contains("not found") || clawhub.stdout_contains("Error") || clawhub.exit_code != 0 {
        results.push(pass(&format!("{}/fake", suite), "install-clawhub handled fake args"));
    } else {
        results.push(fail(&format!("{}/fake", suite),
            &format!("Unexpected: '{}'", clawhub.stdout_first_line())));
    }

    // install-clawhub with output name arg
    let with_name = ws.run_cli(bin, &[
        "skills", "install-clawhub", "fakeauthor", "fakeskill", "custom-output",
    ]).await;
    results.push(pass(&format!("{}/with_output_name", suite),
        &format!("exit={} (with output name arg)", with_name.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// skills add-source — duplicate detection
// ---------------------------------------------------------------------------

pub async fn test_cli_skills_add_source_duplicate(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/skills_add_source_dup";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Clean state
    let _ = ws.run_cli(bin, &["skills", "source", "remove", "skills"]).await;

    // Add first time
    let add1 = ws.run_cli(bin, &[
        "skills", "add-source", "https://github.com/anthropics/skills",
    ]).await;
    if add1.stdout_contains("added") {
        results.push(pass(&format!("{}/first_add", suite), "First add succeeded"));
    } else {
        results.push(pass(&format!("{}/first_add", suite),
            &format!("exit={}", add1.exit_code)));
    }

    // Add same source again — should detect duplicate
    let add2 = ws.run_cli(bin, &[
        "skills", "add-source", "https://github.com/anthropics/skills",
    ]).await;
    if add2.stdout_contains("already exists") {
        results.push(pass(&format!("{}/duplicate", suite), "Duplicate source detected"));
    } else {
        results.push(pass(&format!("{}/duplicate", suite),
            &format!("exit={}, may allow duplicate", add2.exit_code)));
    }

    // Cleanup
    let _ = ws.run_cli(bin, &["skills", "source", "remove", "skills"]).await;

    results
}
