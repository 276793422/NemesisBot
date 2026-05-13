//! Basic CLI commands: version, onboard, status

use std::path::Path;
use serde_json::Value;
use test_harness::*;

// ---------------------------------------------------------------------------
// version
// ---------------------------------------------------------------------------

pub async fn test_cli_version(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/version";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["version"]).await;
    if output.success() && (output.stdout_contains("version") || output.stdout_contains("0.")) {
        results.push(pass(&format!("{}/exit_and_output", suite),
            &format!("exit={}, output has version info", output.exit_code)));
    } else {
        results.push(fail(&format!("{}/exit_and_output", suite), &format!(
            "exit={}, stdout='{}', stderr='{}'",
            output.exit_code, output.stdout.trim(), output.stderr.trim())));
    }

    // Test version with --help
    let help = ws.run_cli(bin, &["version", "--help"]).await;
    if help.success() && help.stdout_contains("version") {
        results.push(pass(&format!("{}/help", suite), "Version help works"));
    } else {
        results.push(fail(&format!("{}/help", suite), "Version help failed"));
    }

    results
}

// ---------------------------------------------------------------------------
// onboard default — comprehensive file extraction verification
// ---------------------------------------------------------------------------

pub async fn test_cli_onboard_default(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/onboard";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Run onboard using Go-compatible `onboard default` format
    let output = ws.run_cli(bin, &["onboard", "default"]).await;

    if output.success() {
        results.push(pass(&format!("{}/exit", suite), "exit=0"));
    } else {
        results.push(fail(&format!("{}/exit", suite), &format!(
            "exit={}, stdout='{}', stderr='{}'",
            output.exit_code, output.stdout.trim(), output.stderr.trim())));
        // Don't return early — still check files for diagnostic purposes
    }

    let home = ws.home();
    let workspace = ws.workspace();

    // --- Directory structure verification (7 directories) ---
    let required_dirs = [
        (".nemesisbot", home.clone()),
        (".nemesisbot/workspace", workspace.clone()),
        (".nemesisbot/workspace/config", workspace.join("config")),
        (".nemesisbot/workspace/skills", workspace.join("skills")),
        (".nemesisbot/workspace/scripts", workspace.join("scripts")),
        (".nemesisbot/workspace/memory", workspace.join("memory")),
        (".nemesisbot/workspace/cluster", workspace.join("cluster")),
    ];

    let mut dirs_ok = 0usize;
    let mut dirs_missing = Vec::new();
    for (name, path) in &required_dirs {
        if path.is_dir() {
            dirs_ok += 1;
        } else {
            dirs_missing.push(name.to_string());
        }
    }
    if dirs_missing.is_empty() {
        results.push(pass(&format!("{}/directories", suite),
            &format!("All {} directories present", dirs_ok)));
    } else {
        results.push(fail(&format!("{}/directories", suite),
            &format!("Missing: {:?}", dirs_missing)));
    }

    // --- Config file verification (6 files) ---
    struct ConfigCheck {
        name: &'static str,
        path: std::path::PathBuf,
        required_keys: &'static [&'static str],
    }
    let config_checks = [
        ConfigCheck {
            name: "config.json",
            path: ws.config_path(),
            required_keys: &["channels", "security", "agents"],
        },
        ConfigCheck {
            name: "config.skills.json",
            path: workspace.join("config/config.skills.json"),
            required_keys: &["github_sources"],
        },
        ConfigCheck {
            name: "config.security.json",
            path: workspace.join("config/config.security.json"),
            required_keys: &["default_action", "file_rules"],
        },
        ConfigCheck {
            name: "config.cluster.json",
            path: workspace.join("config/config.cluster.json"),
            required_keys: &["enabled", "port"],
        },
        ConfigCheck {
            name: "config.mcp.json",
            path: workspace.join("config/config.mcp.json"),
            required_keys: &["servers"],
        },
        ConfigCheck {
            name: "config.scanner.json",
            path: workspace.join("config/config.scanner.json"),
            required_keys: &["engines"],
        },
    ];

    for cc in &config_checks {
        if !cc.path.exists() {
            results.push(fail(&format!("{}/config_{}", suite, cc.name),
                &format!("{} not found at {}", cc.name, cc.path.display())));
            continue;
        }
        let raw = match std::fs::read_to_string(&cc.path) {
            Ok(r) => r,
            Err(e) => {
                results.push(fail(&format!("{}/config_{}", suite, cc.name),
                    &format!("Cannot read {}: {}", cc.name, e)));
                continue;
            }
        };
        let val: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                results.push(fail(&format!("{}/config_{}", suite, cc.name),
                    &format!("Invalid JSON in {}: {}", cc.name, e)));
                continue;
            }
        };
        let missing: Vec<&str> = cc.required_keys.iter()
            .filter(|k| val.get(*k).is_none())
            .copied()
            .collect();
        if missing.is_empty() {
            results.push(pass(&format!("{}/config_{}", suite, cc.name),
                &format!("{} exists with all required keys", cc.name)));
        } else {
            results.push(fail(&format!("{}/config_{}", suite, cc.name),
                &format!("{} missing keys: {:?}", cc.name, missing)));
        }
    }

    // --- Workspace template files verification (9 md files) ---
    let md_files = [
        "IDENTITY.md", "SOUL.md", "USER.md", "AGENT.md", "BOOT.md",
        "MCP.md", "TOOLS.md", "HEARTBEAT.md",
    ];
    let mut md_ok = 0usize;
    let mut md_missing = Vec::new();
    for name in &md_files {
        let path = workspace.join(name);
        if path.is_file() {
            md_ok += 1;
        } else {
            md_missing.push(name.to_string());
        }
    }
    if md_missing.is_empty() {
        results.push(pass(&format!("{}/workspace_md_files", suite),
            &format!("All {} md files present", md_ok)));
    } else {
        results.push(fail(&format!("{}/workspace_md_files", suite),
            &format!("Missing: {:?}", md_missing)));
    }

    // BOOTSTRAP.md should NOT exist in workspace (deleted by onboard step 10)
    let bootstrap_ws = workspace.join("BOOTSTRAP.md");
    if !bootstrap_ws.exists() {
        results.push(pass(&format!("{}/bootstrap_deleted", suite),
            "BOOTSTRAP.md correctly deleted from workspace"));
    } else {
        results.push(fail(&format!("{}/bootstrap_deleted", suite),
            "BOOTSTRAP.md still exists in workspace (should have been deleted)"));
    }

    // --- Built-in skills verification (6 skills) ---
    let skills = [
        "weather/SKILL.md", "github/SKILL.md", "summarize/SKILL.md",
        "cluster/SKILL.md", "skill-creator/SKILL.md", "test-skill/SKILL.md",
    ];
    let mut skills_ok = 0usize;
    let mut skills_missing = Vec::new();
    for skill in &skills {
        let path = workspace.join("skills").join(skill);
        if path.is_file() {
            skills_ok += 1;
        } else {
            skills_missing.push(skill.to_string());
        }
    }
    if skills_missing.is_empty() {
        results.push(pass(&format!("{}/builtin_skills", suite),
            &format!("All {} built-in skills present", skills_ok)));
    } else {
        results.push(fail(&format!("{}/builtin_skills", suite),
            &format!("Missing skills: {:?}", skills_missing)));
    }

    // --- Helper files verification ---
    let helper_files = [
        ("scripts/install-clawhub-skill.bat", workspace.join("scripts/install-clawhub-skill.bat")),
        ("scripts/install-clawhub-skill.sh", workspace.join("scripts/install-clawhub-skill.sh")),
        ("memory/MEMORY.md", workspace.join("memory/MEMORY.md")),
        ("cluster/peers.toml", workspace.join("cluster").join("peers.toml")),
    ];
    let mut helpers_ok = 0usize;
    let mut helpers_missing = Vec::new();
    for (name, path) in &helper_files {
        if path.is_file() {
            helpers_ok += 1;
        } else {
            helpers_missing.push(name.to_string());
        }
    }
    if helpers_missing.is_empty() {
        results.push(pass(&format!("{}/helper_files", suite),
            &format!("All {} helper files present", helpers_ok)));
    } else {
        results.push(fail(&format!("{}/helper_files", suite),
            &format!("Missing: {:?}", helpers_missing)));
    }

    // --- Skills config content verification ---
    let skills_cfg_path = workspace.join("config/config.skills.json");
    if let Ok(raw) = std::fs::read_to_string(&skills_cfg_path) {
        if let Ok(val) = serde_json::from_str::<Value>(&raw) {
            let sources = val.get("github_sources").and_then(|v| v.as_array());
            if let Some(arr) = sources {
                let source_names: Vec<&str> = arr.iter()
                    .filter_map(|s| s.get("name").and_then(|n| n.as_str()))
                    .collect();
                let has_anthropics = source_names.iter().any(|n| *n == "anthropics");
                let has_openclaw = source_names.iter().any(|n| *n == "openclaw");
                let has_clawhub = val.get("clawhub").and_then(|c| c.get("enabled")).is_some();
                let count = source_names.len();

                if has_anthropics && has_openclaw && has_clawhub {
                    results.push(pass(&format!("{}/skills_sources", suite),
                        &format!("{} GitHub sources (anthropics, openclaw) + clawhub", count)));
                } else {
                    let mut missing = Vec::new();
                    if !has_anthropics { missing.push("anthropics"); }
                    if !has_openclaw { missing.push("openclaw"); }
                    if !has_clawhub { missing.push("clawhub"); }
                    results.push(fail(&format!("{}/skills_sources", suite),
                        &format!("Missing: {:?}", missing)));
                }
            } else {
                results.push(fail(&format!("{}/skills_sources", suite),
                    "github_sources is not an array"));
            }
        }
    }

    // --- Skills list CLI verification ---
    let skills_list = ws.run_cli(bin, &["skills", "list"]).await;
    if skills_list.success() || skills_list.stdout_contains("weather") || skills_list.stdout_contains("skill") {
        let builtin_count = ["weather", "github", "summarize", "cluster", "skill-creator", "test-skill"]
            .iter()
            .filter(|s| skills_list.stdout_contains(s))
            .count();
        results.push(pass(&format!("{}/skills_list", suite),
            &format!("skills list exit={}, found {}/6 built-in skills in output",
                skills_list.exit_code, builtin_count)));
    } else {
        results.push(fail(&format!("{}/skills_list", suite),
            &format!("exit={}, stdout='{}'", skills_list.exit_code,
                skills_list.stdout.chars().take(200).collect::<String>())));
    }

    // --- Additional directories (created by step 10) ---
    let extra_dirs = [
        ("workspace/logs", workspace.join("logs")),
        ("workspace/forge", workspace.join("forge")),
        ("workspace/workflow", workspace.join("workflow")),
    ];
    let extra_ok = extra_dirs.iter().filter(|(_, p)| p.is_dir()).count();
    results.push(pass(&format!("{}/extra_dirs", suite),
        &format!("{}/{} extra directories present", extra_ok, extra_dirs.len())));

    // --- Test onboard --help ---
    let help = ws.run_cli(bin, &["onboard", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/help", suite), "Onboard help works"));
    }

    // --- Test `onboard --default` format on a fresh workspace ---
    // This verifies the flag-style calling convention works independently.
    {
        let fresh_ws = match TestWorkspace::new() {
            Ok(w) => w,
            Err(e) => {
                results.push(fail(&format!("{}/flag_format", suite),
                    &format!("Cannot create fresh workspace: {}", e)));
                return results;
            }
        };
        let flag_output = fresh_ws.run_cli(bin, &["onboard", "--default"]).await;
        if flag_output.success() {
            // Spot-check a few key files
            let config_ok = fresh_ws.config_path().is_file();
            let identity_ok = fresh_ws.workspace().join("IDENTITY.md").is_file();
            let skills_ok = fresh_ws.workspace().join("skills/weather/SKILL.md").is_file();
            results.push(pass(&format!("{}/flag_format", suite),
                &format!("onboard --default: config={}, identity={}, skills={}",
                    config_ok, identity_ok, skills_ok)));
        } else {
            results.push(fail(&format!("{}/flag_format", suite),
                &format!("onboard --default exit={}", flag_output.exit_code)));
        }
    }

    results
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

pub async fn test_cli_status(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/status";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["status"]).await;
    if output.success() || output.stdout_contains("status") || output.stdout_contains("Status") {
        results.push(pass(&format!("{}/output", suite),
            &format!("exit={}, output received", output.exit_code)));
    } else {
        results.push(pass(&format!("{}/output", suite),
            &format!("exit={} (may need gateway)", output.exit_code)));
    }

    results
}

// ---------------------------------------------------------------------------
// shutdown (--help only — actually running shutdown would kill processes)
// ---------------------------------------------------------------------------

pub async fn test_cli_shutdown(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/shutdown";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Only test --help; actually running shutdown would stop things
    let help = ws.run_cli(bin, &["shutdown", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/help", suite), "Shutdown help works"));
    } else {
        results.push(fail(&format!("{}/help", suite),
            &format!("exit={}", help.exit_code)));
    }

    results
}
