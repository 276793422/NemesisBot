//! Skills command - manage skills (install, list, remove, search, show, cache).
//!
//! Uses nemesis_skills crate for registry management and search.

use anyhow::Result;
use crate::common;

#[derive(clap::Subcommand)]
pub enum SkillsAction {
    /// List installed skills
    List,
    /// Search for skills in registries
    Search {
        /// Search query (optional - shows all if omitted)
        query: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Install a skill from registry or GitHub
    Install {
        /// Skill reference (registry/slug or user/repo)
        skill: String,
    },
    /// Remove an installed skill
    Remove {
        /// Skill name to remove
        name: String,
    },
    /// Manage skill registries
    Source {
        #[command(subcommand)]
        action: SourceAction,
    },
    /// Add a skill source (shorthand for 'source add')
    #[command(name = "add-source")]
    AddSource {
        /// GitHub URL of the skill source
        url: String,
    },
    /// Validate a skill file
    Validate {
        /// Path to SKILL.md or skill directory
        path: String,
    },
    /// Show skill details
    Show {
        /// Skill name
        name: String,
    },
    /// Manage search cache
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Install built-in skills to workspace
    #[command(name = "install-builtin")]
    InstallBuiltin {
        /// Skill name (omit for all)
        name: Option<String>,
    },
    /// List available built-in skills
    #[command(name = "list-builtin")]
    ListBuiltin,
    /// Install a skill from ClawHub (legacy)
    #[command(name = "install-clawhub")]
    InstallClawhub {
        /// Author name
        author: String,
        /// Skill name
        skill_name: String,
        /// Output directory name (defaults to skill_name)
        output_name: Option<String>,
    },
}

#[derive(clap::Subcommand)]
pub enum SourceAction {
    /// List configured registries
    List,
    /// Add a new skill registry (auto-detects structure)
    Add {
        /// GitHub URL of the registry (e.g., https://github.com/user/repo, user/repo, git@github.com:user/repo.git)
        url: String,
    },
    /// Remove a skill registry
    Remove {
        /// Registry name
        name: String,
    },
}

#[derive(clap::Subcommand)]
pub enum CacheAction {
    /// Show cache statistics
    Stats,
    /// Clear the search cache
    Clear,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a GitHub URL into owner/repo format.
/// Supports: https://github.com/user/repo, git@github.com:user/repo.git, user/repo
fn parse_github_url(url: &str) -> Result<(String, String)> {
    let url = url.trim().trim_end_matches('/');

    // https://github.com/user/repo
    if url.starts_with("https://github.com/") || url.starts_with("http://github.com/") {
        let stripped = url
            .strip_prefix("https://github.com/")
            .or_else(|| url.strip_prefix("http://github.com/"))
            .unwrap_or("");
        let parts: Vec<&str> = stripped.splitn(2, '/').collect();
        if parts.len() == 2 {
            let repo = parts[1].trim_end_matches(".git").to_string();
            return Ok((parts[0].to_string(), repo));
        }
    }

    // git@github.com:user/repo.git
    if url.starts_with("git@github.com:") {
        let stripped = url.strip_prefix("git@github.com:").unwrap_or("");
        let parts: Vec<&str> = stripped.trim_end_matches(".git").splitn(2, '/').collect();
        if parts.len() == 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // user/repo shorthand
    if url.contains('/') && !url.contains(' ') {
        let parts: Vec<&str> = url.splitn(2, '/').collect();
        if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    Err(anyhow::anyhow!("Invalid GitHub URL: {}. Supported formats: https://github.com/user/repo, git@github.com:user/repo.git, user/repo", url))
}

/// Load or create registry config from skills config file.
fn load_registry_config(skills_cfg: &std::path::Path) -> nemesis_skills::types::RegistryConfig {
    if skills_cfg.exists() {
        if let Ok(data) = std::fs::read_to_string(skills_cfg) {
            if let Ok(config) = serde_json::from_str::<nemesis_skills::types::RegistryConfig>(&data) {
                return config;
            }
        }
    }
    nemesis_skills::types::RegistryConfig::default()
}

/// Save registry config to skills config file.
fn save_registry_config(skills_cfg: &std::path::Path, config: &nemesis_skills::types::RegistryConfig) -> Result<()> {
    let dir = skills_cfg.parent().unwrap();
    let _ = std::fs::create_dir_all(dir);
    std::fs::write(skills_cfg, serde_json::to_string_pretty(config).unwrap_or_default())?;
    Ok(())
}

/// Built-in skills definitions (aligned with Go).
fn get_builtin_skills() -> Vec<(&'static str, &'static str)> {
    vec![
        ("weather", "Weather information lookup"),
        ("news", "News headlines and summaries"),
        ("stock", "Stock price queries"),
        ("calculator", "Mathematical calculations"),
        ("structured-development", "Structured development workflow with phases (plan -> develop -> test -> review)"),
        ("build-project", "Project build workflow with version injection"),
        ("automated-testing", "Automated testing workflow with TestAIServer"),
        ("desktop-automation", "Windows desktop window operations (window-mcp)"),
        ("wsl-operations", "WSL environment operations and management"),
        ("dump-analyze", "Dump file analysis and debugging"),
    ]
}

/// Detect the skill structure of a GitHub repository.
/// Returns (index_type, skill_path_pattern, branch).
fn detect_skill_structure(owner: &str, repo: &str) -> (String, String, String) {
    let branches = ["main", "master"];

    for branch in &branches {
        let base_url = format!("https://api.github.com/repos/{}/{}/contents", owner, repo);

        // Mode A: skills/{slug}/SKILL.md (two-level)
        let skills_url = format!("{}{}?ref={}", base_url, "/skills", branch);
        if let Ok(resp) = reqwest::blocking::Client::new()
            .get(&skills_url)
            .header("User-Agent", "nemesisbot")
            .send()
        {
            if resp.status().is_success() {
                if let Ok(entries) = resp.json::<Vec<serde_json::Value>>() {
                    let skill_dirs: Vec<&str> = entries.iter()
                        .filter_map(|e| {
                            if e.get("type").and_then(|v| v.as_str()) == Some("dir") {
                                e.get("name").and_then(|v| v.as_str())
                            } else {
                                None
                            }
                        })
                        .take(5)
                        .collect();

                    let mut has_skill_md = false;
                    for dir in &skill_dirs {
                        let check_url = format!("{}{}{}{}/SKILL.md?ref={}", base_url, "/skills/", dir, "", branch);
                        if let Ok(r) = reqwest::blocking::Client::new()
                            .get(&check_url)
                            .header("User-Agent", "nemesisbot")
                            .send()
                        {
                            if r.status().is_success() {
                                has_skill_md = true;
                                break;
                            }
                        }
                    }

                    if has_skill_md {
                        return ("github_api".to_string(), "skills/{slug}/SKILL.md".to_string(), branch.to_string());
                    }
                }
            }
        }

        // Mode B: skills/{author}/{slug}/SKILL.md (three-level)
        // Check if first subdirectory of skills/ contains subdirectories with SKILL.md
        // This is handled by Mode A pattern already for most cases

        // Mode C: skills.json index
        let index_url = format!("https://raw.githubusercontent.com/{}/{}/{}/skills.json", owner, repo, branch);
        if let Ok(resp) = reqwest::blocking::Client::new()
            .get(&index_url)
            .header("User-Agent", "nemesisbot")
            .send()
        {
            if resp.status().is_success() {
                if let Ok(data) = resp.text() {
                    if serde_json::from_str::<Vec<serde_json::Value>>(&data).is_ok() {
                        return ("skills_json".to_string(), "skills.json".to_string(), branch.to_string());
                    }
                }
            }
        }

        // Mode D: {slug}/SKILL.md at root level
        let root_url = format!("{}?ref={}", base_url, branch);
        if let Ok(resp) = reqwest::blocking::Client::new()
            .get(&root_url)
            .header("User-Agent", "nemesisbot")
            .send()
        {
            if resp.status().is_success() {
                if let Ok(entries) = resp.json::<Vec<serde_json::Value>>() {
                    let skip_dirs = ["src", "pkg", "cmd", "internal", "docs", ".github", "test", "tests", "examples", "scripts", "config"];
                    let root_dirs: Vec<&str> = entries.iter()
                        .filter_map(|e| {
                            if e.get("type").and_then(|v| v.as_str()) == Some("dir") {
                                let name = e.get("name").and_then(|v| v.as_str()).unwrap_or("");
                                if !skip_dirs.contains(&name) && !name.starts_with('.') {
                                    Some(name)
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .take(5)
                        .collect();

                    for dir in &root_dirs {
                        let check_url = format!("{}/{}?ref={}", base_url, dir, branch);
                        if let Ok(r) = reqwest::blocking::Client::new()
                            .get(&check_url)
                            .header("User-Agent", "nemesisbot")
                            .send()
                        {
                            if r.status().is_success() {
                                if let Ok(sub_entries) = r.json::<Vec<serde_json::Value>>() {
                                    if sub_entries.iter().any(|e| {
                                        e.get("name").and_then(|v| v.as_str()) == Some("SKILL.md")
                                            && e.get("type").and_then(|v| v.as_str()) == Some("file")
                                    }) {
                                        return ("github_api".to_string(), format!("{}/SKILL.md", dir), branch.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Default fallback
    ("github_api".to_string(), "skills/{slug}/SKILL.md".to_string(), "main".to_string())
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

fn cmd_list(skills_dir: &std::path::Path) -> Result<()> {
    println!("Installed Skills");
    println!("================");
    if skills_dir.exists() {
        let mut entries: Vec<_> = std::fs::read_dir(skills_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .collect();
        entries.sort_by_key(|e| e.file_name());

        if entries.is_empty() {
            println!("  No skills installed.");
        } else {
            // Determine builtin skill names
            let builtins: Vec<&str> = get_builtin_skills().iter().map(|(n, _)| *n).collect();

            for entry in &entries {
                let name = entry.file_name().to_string_lossy().to_string();
                let has_skill_md = entry.path().join("SKILL.md").exists();
                let is_forge = name.ends_with("-forge");

                // Determine source type
                let source_type = if is_forge {
                    "forge"
                } else if builtins.contains(&name.as_str()) {
                    "builtin"
                } else {
                    "local"
                };

                // Try to read description from SKILL.md
                let desc = if has_skill_md {
                    std::fs::read_to_string(entry.path().join("SKILL.md"))
                        .ok()
                        .and_then(|content| {
                            content.lines()
                                .find(|l| l.trim().starts_with("description:") || l.trim().starts_with("# "))
                                .map(|l| {
                                    let l = l.trim();
                                    if l.starts_with('#') {
                                        l.trim_start_matches('#').trim().to_string()
                                    } else {
                                        l.trim_start_matches("description:").trim().trim_matches('"').to_string()
                                    }
                                })
                        })
                        .unwrap_or_default()
                } else {
                    String::new()
                };

                let badge = if is_forge { " (forge)" } else if !has_skill_md { " (no SKILL.md)" } else { "" };
                if desc.is_empty() {
                    println!("  {}{} [{}]", name, badge, source_type);
                } else {
                    println!("  {}{} [{}] - {}", name, badge, source_type, desc);
                }
            }
        }
    } else {
        println!("  Skills directory not found.");
    }
    println!();
    println!("Search for skills: nemesisbot skills search <query>");
    println!("Install a skill: nemesisbot skills install <registry/slug>");
    Ok(())
}

async fn cmd_search(skills_cfg: &std::path::Path, query: &str, limit: usize) -> Result<()> {
    println!("🔍 Searching for '{}' (limit: {})...", query, limit);

    let config = load_registry_config(skills_cfg);
    let manager = nemesis_skills::registry::RegistryManager::from_config(config);

    let registry_names = manager.registries();
    if registry_names.is_empty() {
        println!("  No skill registries configured.");
        println!("  Add a source: nemesisbot skills add-source <github-url>");
        println!("  Or install directly: nemesisbot skills install <owner/repo>");
        return Ok(());
    }

    println!("  Searching {} registry/ies: {}...", registry_names.len(), registry_names.join(", "));

    match manager.search_all(query, limit).await {
        Ok(grouped_results) => {
            let mut total = 0;
            for group in &grouped_results {
                if group.results.is_empty() {
                    continue;
                }
                println!();
                println!("  {} ({} result(s){})", group.registry_name, group.results.len(),
                    if group.truncated { ", truncated" } else { "" });
                println!("  {}", "-".repeat(50));
                for (i, result) in group.results.iter().enumerate() {
                    let score = format!("{:.0}%", result.score * 100.0);
                    println!("  {}. {} (score: {})", i + 1, result.slug, score);
                    if !result.display_name.is_empty() {
                        println!("     Name: {}", result.display_name);
                    }
                    if !result.summary.is_empty() {
                        let truncated_summary = if result.summary.chars().count() > 100 {
                            format!("{}...", result.summary.chars().take(97).collect::<String>())
                        } else {
                            result.summary.clone()
                        };
                        println!("     Description: {}", truncated_summary);
                    }
                    println!("     Install: nemesisbot skills install {}/{}", group.registry_name, result.slug);
                }
                total += group.results.len();
            }
            println!();
            println!("  Total: {} result(s) from {} registry/ies", total, grouped_results.len());
        }
        Err(e) => {
            println!("  Search failed: {}", e);
            println!();
            println!("  Fallback: Try installing directly from GitHub:");
            println!("    nemesisbot skills install <owner>/<repo>");
        }
    }
    Ok(())
}

async fn cmd_install(skills_dir: &std::path::Path, skills_cfg: &std::path::Path, skill_ref: &str) -> Result<()> {
    // Parse registry/slug format
    let (registry_name, slug) = if skill_ref.contains('/') {
        let parts: Vec<&str> = skill_ref.splitn(2, '/').collect();
        (parts[0].to_string(), parts[1].to_string())
    } else {
        // Try to find in any registry
        println!("No registry specified. Searching for '{}'...", skill_ref);
        let config = load_registry_config(skills_cfg);
        let manager = nemesis_skills::registry::RegistryManager::from_config(config);

        match manager.search(skill_ref, 1).await {
            Ok(results) if !results.is_empty() => {
                let found = &results[0];
                println!("  Found: {} in {}", found.slug, found.registry_name);
                (found.registry_name.clone(), found.slug.clone())
            }
            _ => {
                println!("  Skill '{}' not found in any registry.", skill_ref);
                println!("  Trying GitHub fallback...");
                // Fallback: try as GitHub repo
                return cmd_install_github(skills_dir, skill_ref).await;
            }
        }
    };

    println!("📥 Installing skill: {}/{}", registry_name, slug);
    let config = load_registry_config(skills_cfg);
    let manager = nemesis_skills::registry::RegistryManager::from_config(config);

    let target_dir = skills_dir.join(&slug).to_string_lossy().to_string();
    let _ = std::fs::create_dir_all(skills_dir);

    match manager.install(&registry_name, &slug, &target_dir).await {
        Ok(version) => {
            println!("  ✅ Installed: {} v{}", slug, version);
            println!("  Location: {}", target_dir);
        }
        Err(e) => {
            println!("  Install from registry failed: {}", e);
            // Fallback to GitHub
            println!("  Trying GitHub fallback...");
            let full_ref = format!("{}/{}", registry_name, slug);
            return cmd_install_github(skills_dir, &full_ref).await;
        }
    }
    Ok(())
}

/// Install a skill directly from a GitHub repository.
async fn cmd_install_github(skills_dir: &std::path::Path, repo_path: &str) -> Result<()> {
    let (owner, repo) = parse_github_url(repo_path)?;
    let skill_name = repo.clone();

    println!("  Downloading from GitHub: {}/{}", owner, repo);

    // Try to download SKILL.md from common locations
    let branches = ["main", "master"];
    let paths = [
        format!("skills/{}/SKILL.md", skill_name),
        format!("skills/{}/{}/SKILL.md", owner, skill_name),
        format!("{}/SKILL.md", skill_name),
        "SKILL.md".to_string(),
    ];

    for branch in &branches {
        for path in &paths {
            let url = format!("https://raw.githubusercontent.com/{}/{}/{}/{}", owner, repo, branch, path);
            if let Ok(resp) = reqwest::Client::new()
                .get(&url)
                .header("User-Agent", "nemesisbot")
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await
            {
                if resp.status().is_success() {
                    if let Ok(content) = resp.text().await {
                        let target = skills_dir.join(&skill_name);
                        let _ = std::fs::create_dir_all(&target);
                        std::fs::write(target.join("SKILL.md"), &content)?;
                        println!("  Installed: {} (from GitHub)", skill_name);
                        println!("  Location: {}", target.display());
                        return Ok(());
                    }
                }
            }
        }
    }

    println!("  Failed to download skill from GitHub: {}/{}", owner, repo);
    Ok(())
}

fn cmd_remove(skills_dir: &std::path::Path, name: &str) -> Result<()> {
    let skill_path = skills_dir.join(name);
    if skill_path.exists() {
        std::fs::remove_dir_all(&skill_path)?;
        println!("🗑️ Skill removed: {}", name);
    } else {
        println!("Skill not found: {}", name);
    }
    Ok(())
}

fn cmd_show(skills_dir: &std::path::Path, name: &str) -> Result<()> {
    let skill_dir = skills_dir.join(name);
    if !skill_dir.exists() {
        println!("Skill '{}' not found.", name);
        return Ok(());
    }

    let skill_md = skill_dir.join("SKILL.md");
    if skill_md.exists() {
        let content = std::fs::read_to_string(&skill_md)?;
        println!("Skill: {}", name);
        println!("Path: {}", skill_dir.display());
        println!("{}", "=".repeat(60));
        println!("{}", content);
    } else {
        println!("Skill: {}", name);
        println!("Path: {}", skill_dir.display());
        println!("  (No SKILL.md found)");

        // List files in directory
        if let Ok(entries) = std::fs::read_dir(&skill_dir) {
            let files: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            if !files.is_empty() {
                println!("  Files:");
                for f in &files {
                    println!("    {}", f.file_name().to_string_lossy());
                }
            }
        }
    }
    Ok(())
}

async fn cmd_cache_stats(skills_cfg: &std::path::Path) -> Result<()> {
    let config = load_registry_config(skills_cfg);

    println!("Search Cache Statistics:");
    println!("-------------------------");

    if !config.search_cache.enabled {
        println!("  Cache: disabled");
        println!("  Enable in {} with search_cache.enabled = true", skills_cfg.display());
        return Ok(());
    }

    println!("  Max size: {} entries", config.search_cache.max_size);
    println!("  TTL: {} seconds", config.search_cache.ttl_secs);

    // Show runtime cache stats from the file-based cache
    let cache_dir = skills_cfg.parent().unwrap()
        .parent().unwrap()
        .join("workspace").join("skills").join(".cache");

    // Cache stats file for hit/miss tracking
    let stats_file = cache_dir.join(".stats.json");

    if cache_dir.exists() {
        let entries: Vec<_> = std::fs::read_dir(&cache_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != ".stats.json")
            .collect();
        let count = entries.len();
        let total_size: u64 = entries.iter()
            .filter_map(|e| e.metadata().ok().map(|m| m.len()))
            .sum();

        // Read hit/miss stats
        let (hits, misses) = if stats_file.exists() {
            if let Ok(data) = std::fs::read_to_string(&stats_file) {
                let stats: serde_json::Value = serde_json::from_str(&data).unwrap_or_default();
                let h = stats.get("hits").and_then(|v| v.as_u64()).unwrap_or(0);
                let m = stats.get("misses").and_then(|v| v.as_u64()).unwrap_or(0);
                (h, m)
            } else {
                (0, 0)
            }
        } else {
            (0, 0)
        };

        let total_requests = hits + misses;
        let hit_rate = if total_requests > 0 {
            format!("{:.1}%", (hits as f64 / total_requests as f64) * 100.0)
        } else {
            "N/A".to_string()
        };

        println!("  Entries:      {} / {}", count, config.search_cache.max_size);
        println!("  Memory Usage: ~{} bytes", total_size);
        println!("  Cache Hits:   {}", hits);
        println!("  Cache Misses: {}", misses);
        println!("  Hit Rate:     {}", hit_rate);

        // Performance rating
        let rating = if count == 0 {
            "No entries yet"
        } else if total_requests > 0 && hits as f64 / total_requests as f64 >= 0.8 {
            "Excellent"
        } else if total_requests > 0 && hits as f64 / total_requests as f64 >= 0.5 {
            "Good"
        } else if count >= config.search_cache.max_size as usize * 80 / 100 {
            "Excellent (by fill)"
        } else if count >= config.search_cache.max_size as usize * 50 / 100 {
            "Good (by fill)"
        } else {
            "Low"
        };
        println!("  Performance:  {}", rating);
    } else {
        println!("  Entries:      0 / {}", config.search_cache.max_size);
        println!("  Cache Hits:   0");
        println!("  Cache Misses: 0");
        println!("  Hit Rate:     N/A");
        println!("  Performance:  No entries yet");
    }
    Ok(())
}

async fn cmd_cache_clear(skills_cfg: &std::path::Path) -> Result<()> {
    let cache_dir = skills_cfg.parent().unwrap()
        .parent().unwrap()
        .join("workspace").join("skills").join(".cache");

    if cache_dir.exists() {
        let count = std::fs::read_dir(&cache_dir)?
            .filter_map(|e| e.ok())
            .count();
        let total_size: u64 = std::fs::read_dir(&cache_dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.metadata().ok().map(|m| m.len()))
            .sum();

        std::fs::remove_dir_all(&cache_dir)?;
        println!("Search cache cleared.");
        println!("  Removed: {} entries (~{} bytes freed)", count, total_size);
    } else {
        println!("No cache directory found.");
    }
    Ok(())
}

fn cmd_source_list(skills_cfg: &std::path::Path) -> Result<()> {
    println!("📋 Skill Registries");
    println!("================");

    if !skills_cfg.exists() {
        println!("  No configuration file found.");
        println!("  Add a registry: nemesisbot skills source add <github-url>");
        return Ok(());
    }

    let config = load_registry_config(skills_cfg);
    let mut found_any = false;

    // Show GitHub sources (new format)
    for source in &config.github_sources {
        found_any = true;
        println!("  {} - {} (branch: {}, type: {}, enabled: {})",
            source.name, source.repo, source.branch, source.index_type, source.enabled);
    }

    // Show legacy sources
    for source in &config.github_sources_legacy {
        found_any = true;
        println!("  {} - {} (branch: {})", source.name, source.url, source.branch);
    }

    // Show ClawHub
    if config.clawhub.enabled {
        found_any = true;
        println!("  ClawHub - {} (enabled)", config.clawhub.base_url);
    }

    if !found_any {
        println!("  No registries configured.");
    }
    Ok(())
}

fn cmd_source_add(skills_cfg: &std::path::Path, url: &str) -> Result<()> {
    let (owner, repo) = parse_github_url(url)?;
    let full_repo = format!("{}/{}", owner, repo);

    println!("➕ Adding skill registry: {} ({})", repo, full_repo);

    // Verify repo exists via GitHub API (retry up to 3 times with 2s delay)
    let api_url = format!("https://api.github.com/repos/{}", full_repo);
    let max_retries = 3;
    let mut verified = false;

    for attempt in 1..=max_retries {
        match reqwest::blocking::Client::new()
            .get(&api_url)
            .header("User-Agent", "nemesisbot")
            .send()
        {
            Ok(resp) => {
                if resp.status() == reqwest::StatusCode::NOT_FOUND {
                    println!("Error: Repository '{}' not found (404).", full_repo);
                    return Ok(());
                } else if resp.status() == reqwest::StatusCode::FORBIDDEN {
                    println!("Warning: GitHub API rate limit hit. Proceeding without verification.");
                    verified = true;
                    break;
                } else if resp.status().is_success() {
                    verified = true;
                    break;
                } else {
                    println!("Warning: Could not verify repository (status: {}).", resp.status());
                    if attempt < max_retries {
                        println!("  Retrying in 2s... (attempt {}/{})", attempt + 1, max_retries);
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        continue;
                    }
                }
            }
            Err(e) => {
                println!("Warning: Could not verify repository: {}.", e);
                if attempt < max_retries {
                    println!("  Retrying in 2s... (attempt {}/{})", attempt + 1, max_retries);
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    continue;
                }
            }
        }
    }

    if !verified {
        println!("Warning: Proceeding without repository verification.");
    }

    // Auto-detect skill structure
    let (index_type, skill_path_pattern, branch) = detect_skill_structure(&owner, &repo);
    println!("  Detected structure: {} (pattern: {}, branch: {})", index_type, skill_path_pattern, branch);

    // Generate unique name
    let name = repo.clone();

    let mut config = load_registry_config(skills_cfg);

    // Check for duplicates
    if config.github_sources.iter().any(|s| s.name == name) {
        println!("Error: Registry source '{}' already exists.", name);
        return Ok(());
    }

    // Add new GitHub source with detected settings
    config.github_sources.push(nemesis_skills::types::GitHubSourceConfig {
        name: name.clone(),
        repo: full_repo.clone(),
        enabled: true,
        branch: branch.clone(),
        index_type: index_type.clone(),
        index_path: if index_type == "skills_json" { "skills.json".to_string() } else { String::new() },
        skill_path_pattern: skill_path_pattern.clone(),
        timeout_secs: 0,
        max_size: 0,
    });

    // Also add to legacy sources for backward compat
    config.github_sources_legacy.push(nemesis_skills::types::GithubSource {
        name: name.clone(),
        url: url.to_string(),
        branch: branch.clone(),
    });

    save_registry_config(skills_cfg, &config)?;
    println!("✅ Registry '{}' added: {} (type: {}, pattern: {})", name, full_repo, index_type, skill_path_pattern);
    Ok(())
}

fn cmd_source_remove(skills_cfg: &std::path::Path, name: &str) -> Result<()> {
    println!("Removing registry: {}", name);

    let mut config = load_registry_config(skills_cfg);

    let before = config.github_sources.len();
    config.github_sources.retain(|s| s.name != name);
    let removed = config.github_sources.len() < before;

    config.github_sources_legacy.retain(|s| s.name != name);

    if removed {
        save_registry_config(skills_cfg, &config)?;
        println!("Registry removed.");
    } else {
        println!("Registry '{}' not found.", name);
    }
    Ok(())
}

fn cmd_validate(path: &str) -> Result<()> {
    println!("Validating skill: {}", path);
    let skill_path = std::path::Path::new(path);
    if !skill_path.exists() {
        println!("  Error: Path does not exist.");
        return Ok(());
    }
    let skill_md = if skill_path.is_dir() {
        skill_path.join("SKILL.md")
    } else {
        skill_path.to_path_buf()
    };
    if skill_md.exists() {
        println!("  SKILL.md: found");
        if let Ok(content) = std::fs::read_to_string(&skill_md) {
            let has_name = content.lines().any(|l| l.trim().starts_with("name:") || l.trim().starts_with("# "));
            let has_description = content.lines().any(|l| l.trim().starts_with("description:"));
            let has_steps = content.lines().any(|l| l.trim().starts_with("steps:"));
            println!("  Has name: {}", has_name);
            println!("  Has description: {}", has_description);
            println!("  Has steps: {}", has_steps);

            // Run security check
            let skill_name = skill_path.file_name().unwrap_or_default().to_string_lossy();
            let check = nemesis_skills::security_check::check_skill_security(
                &content, &skill_name, "",
            );
            if check.blocked {
                println!("  Security: BLOCKED ({})", check.block_reason);
            } else if !check.lint_result.warnings.is_empty() {
                println!("  Security: warnings found ({} issues)", check.lint_result.warnings.len());
            } else {
                println!("  Security: OK");
            }
        }
    } else {
        println!("  SKILL.md: not found");
    }
    Ok(())
}

fn cmd_install_builtin(skills_dir: &std::path::Path, name: Option<&str>) -> Result<()> {
    let _ = std::fs::create_dir_all(skills_dir);
    let builtins = get_builtin_skills();

    let to_install: Vec<_> = if let Some(name) = name {
        builtins.into_iter().filter(|(n, _)| *n == name).collect()
    } else {
        builtins
    };

    if to_install.is_empty() {
        println!("Built-in skill '{}' not found.", name.unwrap_or(""));
        println!("Available: nemesisbot skills list-builtin");
        return Ok(());
    }

    for (skill_name, desc) in &to_install {
        let target = skills_dir.join(skill_name);
        if target.exists() {
            println!("  {} (already exists, skipping)", skill_name);
            continue;
        }
        std::fs::create_dir_all(&target)?;

        let skill_md = format!(
            "# {}\n\n{}\n\n## Steps\n\n1. Follow the workflow defined in this skill\n",
            skill_name, desc
        );
        std::fs::write(target.join("SKILL.md"), skill_md)?;
        println!("  {} installed", skill_name);
    }

    println!("Done. {} skill(s) processed.", to_install.len());
    Ok(())
}

fn cmd_list_builtin() -> Result<()> {
    println!("Built-in Skills");
    println!("===============");

    let builtins = get_builtin_skills();
    for (name, desc) in &builtins {
        println!("  {} - {}", name, desc);
    }
    println!();
    println!("Install all: nemesisbot skills install-builtin");
    println!("Install one: nemesisbot skills install-builtin <name>");
    Ok(())
}

fn cmd_install_clawhub(skills_dir: &std::path::Path, author: &str, skill_name: &str, output_name: Option<&str>) -> Result<()> {
    let out_name = output_name.unwrap_or(skill_name);
    println!("Installing from ClawHub: {}/{}", author, skill_name);
    println!("  Note: This is a legacy command. Use 'nemesisbot skills install clawhub/{}' instead.", skill_name);

    // Download SKILL.md from GitHub raw URL
    let url = format!(
        "https://raw.githubusercontent.com/openclaw/skills/main/skills/{}/{}/SKILL.md",
        author, skill_name
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new());

    match client.get(&url).header("User-Agent", "nemesisbot").send() {
        Ok(resp) => {
            if resp.status().is_success() {
                if let Ok(content) = resp.text() {
                    let target = skills_dir.join(out_name);
                    let _ = std::fs::create_dir_all(&target);
                    std::fs::write(target.join("SKILL.md"), &content)?;
                    println!("  Installed: {} -> {}", skill_name, target.display());
                } else {
                    println!("  Failed to read response.");
                }
            } else {
                println!("  Failed to download: HTTP {}", resp.status());
                println!("  Troubleshooting:");
                println!("    - Check that {}/{} exists in openclaw/skills", author, skill_name);
                println!("    - Try: nemesisbot skills search {}", skill_name);
            }
        }
        Err(e) => {
            println!("  Failed to download: {}", e);
            println!("  Check your internet connection and try again.");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Main dispatch
// ---------------------------------------------------------------------------

pub fn run(action: SkillsAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let skills_dir = common::workspace_path(&home).join("skills");
    let skills_cfg = common::skills_config_path(&home);

    match action {
        SkillsAction::List => cmd_list(&skills_dir)?,
        SkillsAction::Search { query, limit } => {
            let q = query.as_deref().unwrap_or("");
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_search(&skills_cfg, q, limit))
            })?;
            result
        }
        SkillsAction::Install { skill } => {
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_install(&skills_dir, &skills_cfg, &skill))
            })?;
            result
        }
        SkillsAction::Remove { name } => cmd_remove(&skills_dir, &name)?,
        SkillsAction::Source { action } => {
            match action {
                SourceAction::List => cmd_source_list(&skills_cfg)?,
                SourceAction::Add { url } => cmd_source_add(&skills_cfg, &url)?,
                SourceAction::Remove { name } => cmd_source_remove(&skills_cfg, &name)?,
            }
        }
        SkillsAction::AddSource { url } => cmd_source_add(&skills_cfg, &url)?,
        SkillsAction::Validate { path } => cmd_validate(&path)?,
        SkillsAction::Show { name } => cmd_show(&skills_dir, &name)?,
        SkillsAction::Cache { action } => {
            match action {
                CacheAction::Stats => {
                    let result = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(cmd_cache_stats(&skills_cfg))
                    })?;
                    result
                }
                CacheAction::Clear => {
                    let result = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(cmd_cache_clear(&skills_cfg))
                    })?;
                    result
                }
            }
        }
        SkillsAction::InstallBuiltin { name } => cmd_install_builtin(&skills_dir, name.as_deref())?,
        SkillsAction::ListBuiltin => cmd_list_builtin()?,
        SkillsAction::InstallClawhub { author, skill_name, output_name } => {
            cmd_install_clawhub(&skills_dir, &author, &skill_name, output_name.as_deref())?
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_github_url_https() {
        let (owner, repo) = parse_github_url("https://github.com/anthropics/skills").unwrap();
        assert_eq!(owner, "anthropics");
        assert_eq!(repo, "skills");
    }

    #[test]
    fn test_parse_github_url_https_with_git() {
        let (owner, repo) = parse_github_url("https://github.com/openclaw/skills.git").unwrap();
        assert_eq!(owner, "openclaw");
        assert_eq!(repo, "skills");
    }

    #[test]
    fn test_parse_github_url_http() {
        let (owner, repo) = parse_github_url("http://github.com/user/repo").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_github_url_git_at() {
        let (owner, repo) = parse_github_url("git@github.com:user/repo.git").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_github_url_git_at_no_git_suffix() {
        let (owner, repo) = parse_github_url("git@github.com:myorg/myrepo").unwrap();
        assert_eq!(owner, "myorg");
        assert_eq!(repo, "myrepo");
    }

    #[test]
    fn test_parse_github_url_shorthand() {
        let (owner, repo) = parse_github_url("user/repo").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_github_url_trailing_slash() {
        let (owner, repo) = parse_github_url("https://github.com/user/repo/").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn test_parse_github_url_invalid_no_slash() {
        let result = parse_github_url("noslash");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_github_url_invalid_empty() {
        let result = parse_github_url("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_github_url_invalid_space() {
        let result = parse_github_url("user name/repo");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_github_url_empty_parts() {
        let result = parse_github_url("/repo");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_builtin_skills_count() {
        let skills = get_builtin_skills();
        assert_eq!(skills.len(), 10);
    }

    #[test]
    fn test_get_builtin_skills_has_weather() {
        let skills = get_builtin_skills();
        assert!(skills.iter().any(|(n, _)| *n == "weather"));
    }

    #[test]
    fn test_get_builtin_skills_has_structured_development() {
        let skills = get_builtin_skills();
        assert!(skills.iter().any(|(n, _)| *n == "structured-development"));
    }

    #[test]
    fn test_get_builtin_skills_descriptions_nonempty() {
        let skills = get_builtin_skills();
        for (name, desc) in &skills {
            assert!(!desc.is_empty(), "Skill '{}' has empty description", name);
        }
    }

    #[test]
    fn test_load_registry_config_no_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.skills.json");
        let config = load_registry_config(&path);
        // Should return default config
        assert!(config.github_sources.is_empty());
    }

    #[test]
    fn test_load_registry_config_with_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.skills.json");
        let data = serde_json::json!({
            "github_sources": [{
                "name": "test",
                "repo": "user/test",
                "enabled": true,
                "branch": "main",
                "index_type": "github_api",
                "skill_path_pattern": "skills/{slug}/SKILL.md"
            }],
            "github_sources_legacy": [],
            "clawhub": {"enabled": false, "base_url": ""},
            "search_cache": {"enabled": true, "max_size": 100, "ttl_secs": 300}
        });
        std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();

        let config = load_registry_config(&path);
        assert_eq!(config.github_sources.len(), 1);
        assert_eq!(config.github_sources[0].name, "test");
    }

    #[test]
    fn test_save_and_load_registry_config_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.skills.json");
        let mut config = nemesis_skills::types::RegistryConfig::default();
        config.github_sources.push(nemesis_skills::types::GitHubSourceConfig {
            name: "mysource".to_string(),
            repo: "org/repo".to_string(),
            enabled: true,
            branch: "main".to_string(),
            index_type: "github_api".to_string(),
            index_path: String::new(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 0,
            max_size: 0,
        });

        save_registry_config(&path, &config).unwrap();
        let loaded = load_registry_config(&path);
        assert_eq!(loaded.github_sources.len(), 1);
        assert_eq!(loaded.github_sources[0].name, "mysource");
    }

    #[test]
    fn test_cmd_remove_nonexistent_skill() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();

        // Should succeed even if skill doesn't exist
        cmd_remove(&skills_dir, "nonexistent").unwrap();
    }

    #[test]
    fn test_cmd_remove_existing_skill() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        let skill_path = skills_dir.join("test-skill");
        std::fs::create_dir_all(&skill_path).unwrap();
        std::fs::write(skill_path.join("SKILL.md"), "# Test Skill").unwrap();

        cmd_remove(&skills_dir, "test-skill").unwrap();
        assert!(!skill_path.exists());
    }

    #[test]
    fn test_cmd_show_existing_skill_with_skill_md() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        let skill_path = skills_dir.join("demo");
        std::fs::create_dir_all(&skill_path).unwrap();
        std::fs::write(skill_path.join("SKILL.md"), "# Demo Skill\nA demo.").unwrap();

        cmd_show(&skills_dir, "demo").unwrap();
    }

    #[test]
    fn test_cmd_show_nonexistent_skill() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();

        cmd_show(&skills_dir, "nonexistent").unwrap();
    }

    #[test]
    fn test_cmd_source_remove_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.skills.json");
        let config = nemesis_skills::types::RegistryConfig::default();
        save_registry_config(&path, &config).unwrap();

        cmd_source_remove(&path, "nonexistent").unwrap();
    }

    #[test]
    fn test_cmd_install_builtin_creates_skill() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");

        cmd_install_builtin(&skills_dir, Some("weather")).unwrap();

        let skill_md = skills_dir.join("weather").join("SKILL.md");
        assert!(skill_md.exists());
        let content = std::fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("weather"));
    }

    #[test]
    fn test_cmd_install_builtin_already_exists() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        let skill_path = skills_dir.join("calculator");
        std::fs::create_dir_all(&skill_path).unwrap();
        std::fs::write(skill_path.join("SKILL.md"), "original").unwrap();

        cmd_install_builtin(&skills_dir, Some("calculator")).unwrap();

        // Should NOT overwrite
        let content = std::fs::read_to_string(skill_path.join("SKILL.md")).unwrap();
        assert_eq!(content, "original");
    }

    #[test]
    fn test_cmd_install_builtin_unknown_skill() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");

        cmd_install_builtin(&skills_dir, Some("nonexistent_skill_xyz")).unwrap();
        // Should report not found but not crash
    }

    #[test]
    fn test_cmd_list_no_dir() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("nonexistent");
        cmd_list(&skills_dir).unwrap();
    }

    #[test]
    fn test_cmd_list_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        cmd_list(&skills_dir).unwrap();
    }

    #[test]
    fn test_cmd_source_list_no_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.skills.json");
        cmd_source_list(&path).unwrap();
    }

    #[test]
    fn test_cmd_validate_nonexistent_path() {
        cmd_validate("/nonexistent/path").unwrap();
    }

    #[test]
    fn test_cmd_validate_with_skill_md() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Test\nname: test\ndescription: A test skill\nsteps:\n- step1").unwrap();

        cmd_validate(&skill_dir.to_string_lossy()).unwrap();
    }
}
