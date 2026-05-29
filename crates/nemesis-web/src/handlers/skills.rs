//! Skills handler — installed/detail/uninstall/search/install/config/source management.

use crate::handlers::{read_workspace_file, require_workspace};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct SkillsHandler {
    _priv: (),
}

impl SkillsHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for SkillsHandler {
    fn module_name(&self) -> &str {
        "skills"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "installed" => self.installed(workspace),
            "detail" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.detail(workspace, &name)
            }
            "uninstall" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.uninstall(workspace, &name)
            }
            "search" => {
                let data = data.ok_or("missing data")?;
                let query = crate::handlers::get_str(&data, "query")?;
                self.search(&query)
            }
            "install" => {
                let data = data.ok_or("missing data")?;
                self.install(&data)
            }
            "config.get" => self.config_get(workspace),
            "config.save" => {
                let data = data.ok_or("missing data")?;
                self.config_save(workspace, &data)
            }
            "config.update" => {
                let data = data.ok_or("missing data")?;
                self.config_update(workspace, &data)
            }
            "source.list" => self.source_list(workspace),
            "source.add" => {
                let data = data.ok_or("missing data")?;
                self.source_add(workspace, &data).await
            }
            "source.add.manual" => {
                let data = data.ok_or("missing data")?;
                self.source_add_manual(workspace, &data)
            }
            "source.remove" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.source_remove(workspace, &name)
            }
            "source.toggle" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                let enabled = data.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                self.source_toggle(workspace, &name, enabled)
            }
            _ => Err(format!("unknown command: skills.{}", cmd)),
        }
    }
}

fn skills_config_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("config/config.skills.json")
}

fn load_config(workspace: &str) -> Result<nemesis_config::SkillsFullConfig, String> {
    let path = skills_config_path(workspace);
    nemesis_config::load_skills_config(&path).map_err(|e| format!("failed to load skills config: {}", e))
}

fn save_config(workspace: &str, cfg: &nemesis_config::SkillsFullConfig) -> Result<(), String> {
    let path = skills_config_path(workspace);
    nemesis_config::save_skills_config(&path, cfg).map_err(|e| format!("failed to save skills config: {}", e))
}

impl SkillsHandler {
    fn installed(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let skills_dir = PathBuf::from(workspace).join("skills");
        if !skills_dir.exists() {
            return Ok(Some(serde_json::json!({ "skills": [] })));
        }

        let mut skills = Vec::new();
        let read_dir = std::fs::read_dir(&skills_dir)
            .map_err(|e| format!("failed to read skills dir: {}", e))?;
        for entry in read_dir {
            let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
            let path = entry.path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let has_skill_md = path.join("SKILL.md").exists();
                let description = if has_skill_md {
                    read_workspace_file(workspace, &format!("skills/{}/SKILL.md", name))
                        .ok()
                        .and_then(|content| {
                            content.lines().next().map(|l| l.trim_start_matches('#').trim().to_string())
                        })
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                skills.push(serde_json::json!({
                    "name": name,
                    "has_skill_md": has_skill_md,
                    "description": description,
                }));
            }
        }

        Ok(Some(serde_json::json!({ "skills": skills })))
    }

    fn detail(&self, workspace: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let relative = format!("skills/{}/SKILL.md", name);
        let content = read_workspace_file(workspace, &relative)?;
        Ok(Some(serde_json::json!({
            "name": name,
            "content": content,
        })))
    }

    fn uninstall(&self, workspace: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let skill_dir = crate::handlers::resolve_path(workspace, &format!("skills/{}", name))?;
        if !skill_dir.exists() {
            return Err(format!("skill '{}' not found", name));
        }
        std::fs::remove_dir_all(&skill_dir)
            .map_err(|e| format!("failed to remove skill '{}': {}", name, e))?;
        Ok(Some(serde_json::json!({ "uninstalled": true, "name": name })))
    }

    fn search(&self, query: &str) -> Result<Option<serde_json::Value>, String> {
        let _ = query;
        Ok(Some(serde_json::json!({
            "query": query,
            "results": [],
            "message": "Skill search requires remote registry integration"
        })))
    }

    fn install(&self, _data: &serde_json::Value) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(serde_json::json!({
            "installed": false,
            "message": "Skill install requires remote registry integration"
        })))
    }

    fn config_get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let cfg = load_config(workspace)?;
        let json = serde_json::to_value(&cfg)
            .map_err(|e| format!("failed to serialize: {}", e))?;
        Ok(Some(json))
    }

    fn config_save(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let config: nemesis_config::SkillsFullConfig = serde_json::from_value(data.clone())
            .map_err(|e| format!("invalid skills config: {}", e))?;
        save_config(workspace, &config)?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }

    fn config_update(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let mut cfg = load_config(workspace)?;
        if let Some(v) = data.get("enabled").and_then(|v| v.as_bool()) {
            cfg.enabled = v;
        }
        if let Some(obj) = data.get("search_cache").and_then(|v| v.as_object()) {
            if let Some(v) = obj.get("enabled").and_then(|v| v.as_bool()) {
                cfg.search_cache.enabled = v;
            }
            if let Some(v) = obj.get("max_size").and_then(|v| v.as_i64()) {
                cfg.search_cache.max_size = v;
            }
            if let Some(v) = obj.get("ttl_seconds").and_then(|v| v.as_i64()) {
                cfg.search_cache.ttl_seconds = v;
            }
        }
        if let Some(v) = data.get("max_concurrent_searches").and_then(|v| v.as_i64()) {
            cfg.max_concurrent_searches = v;
        }
        if let Some(obj) = data.get("clawhub").and_then(|v| v.as_object()) {
            if let Some(v) = obj.get("enabled").and_then(|v| v.as_bool()) {
                cfg.clawhub.enabled = v;
            }
            if let Some(v) = obj.get("base_url").and_then(|v| v.as_str()) {
                cfg.clawhub.base_url = v.to_string();
            }
            if let Some(v) = obj.get("convex_url").and_then(|v| v.as_str()) {
                cfg.clawhub.convex_url = v.to_string();
            }
            if let Some(v) = obj.get("timeout").and_then(|v| v.as_i64()) {
                cfg.clawhub.timeout = v;
            }
        }
        save_config(workspace, &cfg)?;
        Ok(Some(serde_json::json!({ "updated": true })))
    }

    fn source_list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let cfg = load_config(workspace)?;
        let mut sources = Vec::new();

        for s in &cfg.github_sources {
            sources.push(serde_json::json!({
                "type": "github",
                "name": s.name,
                "repo": s.repo,
                "enabled": s.enabled,
                "branch": s.branch,
                "index_type": s.index_type,
                "skill_path_pattern": s.skill_path_pattern,
            }));
        }

        sources.push(serde_json::json!({
            "type": "clawhub",
            "name": "ClawHub",
            "base_url": cfg.clawhub.base_url,
            "enabled": cfg.clawhub.enabled,
            "deletable": false,
        }));

        Ok(Some(serde_json::json!({ "sources": sources })))
    }

    async fn source_add(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let url = crate::handlers::get_str(data, "url")?;
        let (owner, repo) = parse_github_url(&url)
            .map_err(|e| e.to_string())?;
        let full_repo = format!("{}/{}", owner, repo);
        let name = repo.clone();

        // Check duplicate
        let mut cfg = load_config(workspace)?;
        if cfg.github_sources.iter().any(|s| s.name == name) {
            return Err(format!("源 '{}' 已存在", name));
        }

        // Auto-detect structure in a blocking thread
        let owner_clone = owner.clone();
        let repo_clone = repo.clone();
        let detected = tokio::task::spawn_blocking(move || {
            detect_skill_structure(&owner_clone, &repo_clone)
        }).await
            .map_err(|e| format!("探测任务失败: {}", e))?;

        match detected {
            Ok((index_type, skill_path_pattern, branch)) => {
                cfg.github_sources.push(nemesis_config::GitHubSourceConfig {
                    name: name.clone(),
                    repo: full_repo.clone(),
                    enabled: true,
                    branch,
                    index_type,
                    index_path: String::new(),
                    skill_path_pattern,
                    timeout: 0,
                    max_size: 0,
                });
                save_config(workspace, &cfg)?;
                Ok(Some(serde_json::json!({
                    "success": true,
                    "source": { "type": "github", "name": name, "repo": full_repo }
                })))
            }
            Err(reason) => {
                Ok(Some(serde_json::json!({
                    "success": false,
                    "partial": { "name": name, "repo": full_repo },
                    "error": reason,
                })))
            }
        }
    }

    fn source_add_manual(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = crate::handlers::get_str(data, "name")?;
        let repo = crate::handlers::get_str(data, "repo")?;
        let branch = data.get("branch").and_then(|v| v.as_str()).unwrap_or("main").to_string();
        let index_type = data.get("index_type").and_then(|v| v.as_str()).unwrap_or("github_api").to_string();
        let skill_path_pattern = data.get("skill_path_pattern").and_then(|v| v.as_str()).unwrap_or("skills/{slug}/SKILL.md").to_string();

        let mut cfg = load_config(workspace)?;
        if cfg.github_sources.iter().any(|s| s.name == name) {
            return Err(format!("源 '{}' 已存在", name));
        }

        cfg.github_sources.push(nemesis_config::GitHubSourceConfig {
            name: name.clone(),
            repo: repo.clone(),
            enabled: true,
            branch,
            index_type,
            index_path: String::new(),
            skill_path_pattern,
            timeout: 0,
            max_size: 0,
        });
        save_config(workspace, &cfg)?;
        Ok(Some(serde_json::json!({
            "success": true,
            "source": { "type": "github", "name": name, "repo": repo }
        })))
    }

    fn source_remove(&self, workspace: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let mut cfg = load_config(workspace)?;
        let before = cfg.github_sources.len();
        cfg.github_sources.retain(|s| s.name != name);
        if cfg.github_sources.len() == before {
            return Err(format!("源 '{}' 不存在", name));
        }
        save_config(workspace, &cfg)?;
        Ok(Some(serde_json::json!({ "removed": true, "name": name })))
    }

    fn source_toggle(
        &self,
        workspace: &str,
        name: &str,
        enabled: bool,
    ) -> Result<Option<serde_json::Value>, String> {
        let mut cfg = load_config(workspace)?;

        // Check GitHub sources
        if let Some(source) = cfg.github_sources.iter_mut().find(|s| s.name == name) {
            source.enabled = enabled;
            save_config(workspace, &cfg)?;
            return Ok(Some(serde_json::json!({ "toggled": true, "name": name, "enabled": enabled })));
        }

        // Check ClawHub
        if name == "ClawHub" || name == "clawhub" {
            cfg.clawhub.enabled = enabled;
            save_config(workspace, &cfg)?;
            return Ok(Some(serde_json::json!({ "toggled": true, "name": name, "enabled": enabled })));
        }

        Err(format!("源 '{}' 不存在", name))
    }
}

// ---------------------------------------------------------------------------
// GitHub URL parsing and auto-detection (from CLI skills command)
// ---------------------------------------------------------------------------

fn parse_github_url(url: &str) -> Result<(String, String), String> {
    let url = url.trim().trim_end_matches('/');

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

    if url.starts_with("git@github.com:") {
        let stripped = url.strip_prefix("git@github.com:").unwrap_or("");
        let parts: Vec<&str> = stripped.trim_end_matches(".git").splitn(2, '/').collect();
        if parts.len() == 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    if url.contains('/') && !url.contains(' ') {
        let parts: Vec<&str> = url.splitn(2, '/').collect();
        if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    Err(format!(
        "无法解析 URL: {}。支持格式: https://github.com/user/repo, git@github.com:user/repo.git, user/repo",
        url
    ))
}

/// Auto-detect skill structure of a GitHub repo.
/// Returns Ok((index_type, skill_path_pattern, branch)) on success, Err(reason) on failure.
fn detect_skill_structure(owner: &str, repo: &str) -> Result<(String, String, String), String> {
    let branches = ["main", "master"];
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP 客户端创建失败: {}", e))?;

    for branch in &branches {
        let base_url = format!("https://api.github.com/repos/{}/{}/contents", owner, repo);

        // Mode A: skills/{slug}/SKILL.md
        let skills_url = format!("{}/skills?ref={}", base_url, branch);
        if let Ok(resp) = client
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
                        let check_url = format!("{}/skills/{}/SKILL.md?ref={}", base_url, dir, branch);
                        if let Ok(r) = client.get(&check_url).header("User-Agent", "nemesisbot").send() {
                            if r.status().is_success() {
                                has_skill_md = true;
                                break;
                            }
                        }
                    }

                    if has_skill_md {
                        return Ok((
                            "github_api".to_string(),
                            "skills/{slug}/SKILL.md".to_string(),
                            branch.to_string(),
                        ));
                    }
                }
            }
        }

        // Mode C: skills.json
        let index_url = format!(
            "https://raw.githubusercontent.com/{}/{}/{}/skills.json",
            owner, repo, branch
        );
        if let Ok(resp) = client.get(&index_url).header("User-Agent", "nemesisbot").send() {
            if resp.status().is_success() {
                if let Ok(data) = resp.text() {
                    if serde_json::from_str::<Vec<serde_json::Value>>(&data).is_ok() {
                        return Ok((
                            "skills_json".to_string(),
                            "skills.json".to_string(),
                            branch.to_string(),
                        ));
                    }
                }
            }
        }

        // Mode D: root-level {slug}/SKILL.md
        let root_url = format!("{}?ref={}", base_url, branch);
        if let Ok(resp) = client.get(&root_url).header("User-Agent", "nemesisbot").send() {
            if resp.status().is_success() {
                if let Ok(entries) = resp.json::<Vec<serde_json::Value>>() {
                    let skip_dirs = [
                        "src", "pkg", "cmd", "internal", "docs", ".github",
                        "test", "tests", "examples", "scripts", "config",
                    ];
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
                        if let Ok(r) = client.get(&check_url).header("User-Agent", "nemesisbot").send() {
                            if r.status().is_success() {
                                if let Ok(sub) = r.json::<Vec<serde_json::Value>>() {
                                    if sub.iter().any(|e| {
                                        e.get("name").and_then(|v| v.as_str()) == Some("SKILL.md")
                                            && e.get("type").and_then(|v| v.as_str()) == Some("file")
                                    }) {
                                        return Ok((
                                            "github_api".to_string(),
                                            format!("{}/SKILL.md", dir),
                                            branch.to_string(),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Err("无法探测仓库结构，可能是网络问题或仓库不包含 Skills（4 种探测模式均未匹配）".to_string())
}
