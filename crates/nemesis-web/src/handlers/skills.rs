//! Skills handler — installed/detail/uninstall/search/install/config.get/config.save.

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
            _ => Err(format!("unknown command: skills.{}", cmd)),
        }
    }
}

fn skills_config_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("config/config.skills.json")
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
                            // Extract first line as description
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
        // Stub — requires nemesis-skills integration with remote registry
        Ok(Some(serde_json::json!({
            "query": query,
            "results": [],
            "message": "Skill search requires remote registry integration"
        })))
    }

    fn install(&self, _data: &serde_json::Value) -> Result<Option<serde_json::Value>, String> {
        // Stub — requires nemesis-skills integration with remote registry
        Ok(Some(serde_json::json!({
            "installed": false,
            "message": "Skill install requires remote registry integration"
        })))
    }

    fn config_get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = skills_config_path(workspace);
        let config = nemesis_config::load_skills_config(&path)
            .map_err(|e| format!("failed to load skills config: {}", e))?;
        let json = serde_json::to_value(&config)
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
        let path = skills_config_path(workspace);
        nemesis_config::save_skills_config(&path, &config)
            .map_err(|e| format!("failed to save skills config: {}", e))?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }
}
