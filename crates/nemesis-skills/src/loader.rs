//! Skill loader - scans directories and parses SKILL.md frontmatter.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use nemesis_types::error::{NemesisError, Result};
use regex::Regex;
use tracing::{debug, warn};

use crate::lint::{LintResult, SkillLinter};
use crate::types::SkillInfo;

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;

/// Validates a skill name: alphanumeric with hyphens.
fn is_valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME_LENGTH {
        return false;
    }
    let re = Regex::new(r"^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$").unwrap();
    re.is_match(name)
}

/// XML-escape special characters.
///
/// Mirrors Go `escapeXML`. Public for reuse by external callers.
pub fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Metadata extracted from a SKILL.md file's YAML/JSON frontmatter.
///
/// Mirrors the Go `SkillMetadata` struct from `module/skills/loader.go`.
#[derive(Debug, Clone)]
pub struct SkillMetadata {
    /// Skill name (from frontmatter or directory name).
    pub name: String,
    /// Skill description.
    pub description: String,
}

/// Extract the raw frontmatter string from SKILL.md content.
///
/// Returns the content between the opening `---` and closing `---` delimiters,
/// or an empty string if no frontmatter is found.
///
/// Supports Unix (`\n`), Windows (`\r\n`), and classic Mac (`\r`) line endings.
pub fn extract_frontmatter(content: &str) -> String {
    let re = Regex::new(r"(?s)^---[\r\n]+(.*?)[\r\n]+---").unwrap();
    if let Some(captures) = re.captures(content) {
        if let Some(m) = captures.get(1) {
            return m.as_str().to_string();
        }
    }
    String::new()
}

/// Parse simple YAML key: value content into a HashMap.
///
/// Each line should be in the format `key: value`. Lines starting with `#`
/// are treated as comments and skipped. Values may be optionally quoted
/// with single or double quotes.
///
/// Normalizes line endings to handle `\n` (Unix), `\r\n` (Windows), and `\r` (classic Mac).
pub fn parse_simple_yaml(content: &str) -> std::collections::HashMap<String, String> {
    let mut result = std::collections::HashMap::new();

    // Normalize line endings
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");

    for line in normalized.split('\n') {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            // Strip surrounding quotes if present
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(&value)
                .to_string();
            result.insert(key, value);
        }
    }

    result
}

/// Parse skill metadata from a SKILL.md file path.
///
/// Reads the file, extracts the frontmatter, and returns a `SkillMetadata`
/// with the name and description. If no frontmatter is found, the name
/// defaults to the parent directory name.
///
/// Mirrors the Go `getSkillMetadata()` method.
pub fn get_skill_metadata(skill_path: &Path) -> Option<SkillMetadata> {
    let content = std::fs::read_to_string(skill_path).ok()?;

    let frontmatter = extract_frontmatter(&content);
    if frontmatter.is_empty() {
        // No frontmatter; use directory name
        let name = skill_path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        return Some(SkillMetadata {
            name,
            description: String::new(),
        });
    }

    // Try JSON first (for backward compatibility)
    #[derive(serde::Deserialize)]
    struct JsonMeta {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
    }

    if let Ok(json_meta) = serde_json::from_str::<JsonMeta>(&frontmatter) {
        if json_meta.name.is_some() || json_meta.description.is_some() {
            return Some(SkillMetadata {
                name: json_meta.name.unwrap_or_default(),
                description: json_meta.description.unwrap_or_default(),
            });
        }
    }

    // Fall back to simple YAML parsing
    let yaml_map = parse_simple_yaml(&frontmatter);
    Some(SkillMetadata {
        name: yaml_map.get("name").cloned().unwrap_or_default(),
        description: yaml_map.get("description").cloned().unwrap_or_default(),
    })
}

/// Loads skills from filesystem directories.
///
/// Supports three skill directories with priority:
/// 1. Workspace skills (project-level)
/// 2. Global skills (~/.nemesisbot/skills)
/// 3. Built-in skills
///
/// Higher-priority skills override lower-priority ones by name.
pub struct SkillsLoader {
    #[allow(dead_code)]
    workspace: PathBuf,
    workspace_skills: PathBuf,
    global_skills: PathBuf,
    builtin_skills: PathBuf,
    enable_security: bool,
    linter: Option<SkillLinter>,
}

impl SkillsLoader {
    /// Create a new SkillsLoader with three skill directories.
    pub fn new(workspace: &str, global_skills: &str, builtin_skills: &str) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            workspace_skills: PathBuf::from(workspace).join("skills"),
            global_skills: PathBuf::from(global_skills),
            builtin_skills: PathBuf::from(builtin_skills),
            enable_security: false,
            linter: None,
        }
    }

    /// Enable security scanning during skill listing.
    pub fn enable_security(&mut self) {
        self.enable_security = true;
        self.linter = Some(SkillLinter::new());
    }

    /// Run security scanning on a skill's content.
    ///
    /// If security scanning is not enabled or no linter is configured,
    /// this is a no-op. Otherwise, it reads the skill file and updates
    /// the `SkillInfo` with the lint score and warning status.
    ///
    /// Mirrors the Go `scanSkillSecurity()` method.
    pub fn scan_skill_security(&self, info: &mut SkillInfo, skill_file: &Path) {
        if !self.enable_security {
            return;
        }
        let Some(ref linter) = self.linter else {
            return;
        };
        if let Ok(content) = std::fs::read_to_string(skill_file) {
            let lint_result: LintResult = linter.lint(&content);
            info.lint_score = Some(lint_result.score);
            info.has_warnings = !lint_result.warnings.is_empty();
        }
    }

    /// List all skills from workspace, global, and builtin directories.
    ///
    /// Workspace skills override global, which override builtin (by name).
    pub fn list_skills(&self) -> Vec<SkillInfo> {
        let mut skills = Vec::new();

        // 1. Workspace skills (highest priority)
        self.scan_directory_into(
            &self.workspace_skills,
            "workspace",
            &mut skills,
            &HashSet::new(),
        );

        // 2. Global skills - skipped if already present in workspace
        let workspace_names: HashSet<String> = skills
            .iter()
            .filter(|s| s.source == "workspace")
            .map(|s| s.name.clone())
            .collect();
        self.scan_directory_into(&self.global_skills, "global", &mut skills, &workspace_names);

        // 3. Built-in skills - skipped if already present in workspace or global
        let existing_names: HashSet<String> = skills
            .iter()
            .filter(|s| s.source == "workspace" || s.source == "global")
            .map(|s| s.name.clone())
            .collect();
        self.scan_directory_into(
            &self.builtin_skills,
            "builtin",
            &mut skills,
            &existing_names,
        );

        skills
    }

    /// Load a single skill's content by name, stripping frontmatter.
    ///
    /// Priority order: workspace -> global -> builtin.
    pub fn load_skill(&self, name: &str) -> Option<String> {
        // 1. Workspace
        let path = self.workspace_skills.join(name).join("SKILL.md");
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(Self::strip_frontmatter(&content));
        }

        // 2. Global
        let path = self.global_skills.join(name).join("SKILL.md");
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(Self::strip_frontmatter(&content));
        }

        // 3. Built-in
        let path = self.builtin_skills.join(name).join("SKILL.md");
        if let Ok(content) = std::fs::read_to_string(&path) {
            return Some(Self::strip_frontmatter(&content));
        }

        None
    }

    /// Load multiple skills and format them as context for LLM injection.
    ///
    /// Returns a formatted string with skill contents separated by dividers.
    pub fn load_skills_for_context(&self, skill_names: &[String]) -> String {
        if skill_names.is_empty() {
            return String::new();
        }

        let parts: Vec<String> = skill_names
            .iter()
            .filter_map(|name| {
                self.load_skill(name)
                    .map(|content| format!("### Skill: {}\n\n{}", name, content))
            })
            .collect();

        parts.join("\n\n---\n\n")
    }

    /// Build an XML-formatted summary of all available skills.
    ///
    /// Output format:
    /// ```xml
    /// <skills>
    ///   <skill>
    ///     <name>...</name>
    ///     <description>...</description>
    ///     <location>...</location>
    ///     <source>...</source>
    ///   </skill>
    /// </skills>
    /// ```
    pub fn build_skills_summary(&self) -> String {
        let all_skills = self.list_skills();
        if all_skills.is_empty() {
            return String::new();
        }

        let mut lines = Vec::new();
        lines.push("<skills>".to_string());

        for s in &all_skills {
            let escaped_name = escape_xml(&s.name);
            let escaped_desc = escape_xml(&s.description);
            let escaped_path = escape_xml(&s.path);

            lines.push("  <skill>".to_string());
            lines.push(format!("    <name>{}</name>", escaped_name));
            lines.push(format!("    <description>{}</description>", escaped_desc));
            lines.push(format!("    <location>{}</location>", escaped_path));
            lines.push(format!("    <source>{}</source>", s.source));
            if self.enable_security && (s.lint_score.unwrap_or(0.0) > 0.0 || s.has_warnings) {
                lines.push(format!(
                    "    <security_score>{:.0}</security_score>",
                    s.lint_score.unwrap_or(0.0) * 100.0
                ));
            }
            lines.push("  </skill>".to_string());
        }

        lines.push("</skills>".to_string());
        lines.join("\n")
    }

    /// Scan a single skill directory for its SKILL.md file.
    fn scan_single_skill(&self, skill_dir: &Path) -> Result<SkillInfo> {
        let skill_md = skill_dir.join("SKILL.md");
        let content = std::fs::read_to_string(&skill_md)?;
        let dir_name = skill_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let (frontmatter, _body) = Self::parse_frontmatter(&content);

        let name = frontmatter
            .get("name")
            .cloned()
            .unwrap_or_else(|| dir_name.clone());
        let description = frontmatter.get("description").cloned().unwrap_or_default();

        Ok(SkillInfo {
            name,
            path: skill_dir.to_string_lossy().to_string(),
            source: "local".to_string(),
            description,
            lint_score: None,
            has_warnings: false,
        })
    }

    /// Scan a directory for skills and add them to the skills list,
    /// skipping any names already in the `skip_names` set.
    fn scan_directory_into(
        &self,
        dir: &Path,
        source: &str,
        skills: &mut Vec<SkillInfo>,
        skip_names: &HashSet<String>,
    ) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                debug!("Skipping directory without SKILL.md: {}", path.display());
                continue;
            }

            let mut info = match self.scan_single_skill(&path) {
                Ok(info) => info,
                Err(e) => {
                    warn!("Failed to load skill from {}: {}", path.display(), e);
                    continue;
                }
            };

            // Validate name
            if !is_valid_name(&info.name) {
                warn!("Invalid skill name from {}: {}", source, info.name);
                continue;
            }
            if info.description.is_empty() || info.description.len() > MAX_DESCRIPTION_LENGTH {
                warn!(
                    "Invalid skill description from {} (name: {})",
                    source, info.name
                );
                continue;
            }

            // Skip if already loaded from a higher-priority source
            if skip_names.contains(&info.name) {
                continue;
            }

            // Override source
            info.source = source.to_string();

            // Run security scan if enabled
            if self.enable_security {
                if let Some(ref linter) = self.linter {
                    if let Ok(content) = std::fs::read_to_string(&skill_md) {
                        let lint_result: LintResult = linter.lint(&content);
                        info.lint_score = Some(lint_result.score);
                        info.has_warnings = !lint_result.warnings.is_empty();
                    }
                }
            }

            debug!(
                "Loaded skill: {} from {} ({})",
                info.name, info.path, source
            );
            skills.push(info);
        }
    }

    /// Parse frontmatter from SKILL.md content.
    ///
    /// Expects content to optionally start with `---` delimiters. Supports two
    /// frontmatter formats:
    ///
    /// **YAML** (simple key: value):
    /// ```markdown
    /// ---
    /// name: My Skill
    /// description: Does things
    /// ---
    /// ```
    ///
    /// **JSON** (object with "name" and "description"):
    /// ```markdown
    /// ---
    /// {"name": "My Skill", "description": "Does things"}
    /// ---
    /// ```
    ///
    /// JSON frontmatter is tried first (for backward compatibility with the Go
    /// implementation), then falls back to simple YAML key: value parsing.
    ///
    /// Returns a map of key-value pairs and the remaining body text.
    fn parse_frontmatter(content: &str) -> (std::collections::HashMap<String, String>, String) {
        let mut map = std::collections::HashMap::new();
        let trimmed = content.trim_start();

        if !trimmed.starts_with("---") {
            return (map, content.to_string());
        }

        // Find the closing ---
        let after_first = &trimmed[3..];
        if let Some(end_idx) = after_first.find("---") {
            let frontmatter_str = after_first[..end_idx].trim();
            let body = after_first[end_idx + 3..].trim().to_string();

            // Try JSON first (for backward compatibility with Go version)
            if let Ok(json_map) = Self::parse_json_frontmatter(frontmatter_str) {
                return (json_map, body);
            }

            // Fall back to simple YAML parsing
            for line in frontmatter_str.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim().to_string();
                    let value = value.trim().to_string();
                    // Strip surrounding quotes if present
                    let value = value
                        .strip_prefix('"')
                        .and_then(|v| v.strip_suffix('"'))
                        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                        .unwrap_or(&value)
                        .to_string();
                    map.insert(key, value);
                }
            }

            return (map, body);
        }

        (map, content.to_string())
    }

    /// Attempt to parse frontmatter as JSON.
    ///
    /// Returns `Ok(map)` if the frontmatter is valid JSON with "name" and/or
    /// "description" fields, or `Err` if parsing fails (not valid JSON or
    /// neither field present).
    fn parse_json_frontmatter(
        frontmatter: &str,
    ) -> std::result::Result<std::collections::HashMap<String, String>, ()> {
        #[derive(serde::Deserialize)]
        struct JsonFrontmatter {
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            description: Option<String>,
        }

        let parsed: JsonFrontmatter = serde_json::from_str(frontmatter).map_err(|_| ())?;

        // Only use JSON result if at least one field was present
        if parsed.name.is_none() && parsed.description.is_none() {
            return Err(());
        }

        let mut map = std::collections::HashMap::new();
        if let Some(name) = parsed.name {
            map.insert("name".to_string(), name);
        }
        if let Some(description) = parsed.description {
            map.insert("description".to_string(), description);
        }
        Ok(map)
    }

    /// Strip frontmatter from SKILL.md content, returning only the body.
    fn strip_frontmatter(content: &str) -> String {
        let re = Regex::new(r"(?s)^---[\r\n]+(.*?)[\r\n]+---[\r\n]*").unwrap();
        re.replace_all(content, "").to_string()
    }

    /// Scans the given root directory for skill directories.
    ///
    /// Each skill is expected to be a subdirectory containing a `SKILL.md` file.
    /// The frontmatter of `SKILL.md` is parsed for `name` and `description` fields.
    /// If frontmatter is absent, the directory name is used as the skill name.
    pub fn scan_directory(root: &Path) -> Result<Vec<SkillInfo>> {
        if !root.exists() {
            return Err(NemesisError::Config(format!(
                "Skills directory does not exist: {}",
                root.display()
            )));
        }

        if !root.is_dir() {
            return Err(NemesisError::Config(format!(
                "Path is not a directory: {}",
                root.display()
            )));
        }

        let mut skills = Vec::new();

        let entries = std::fs::read_dir(root).map_err(|e| {
            NemesisError::Config(format!(
                "Failed to read directory {}: {}",
                root.display(),
                e
            ))
        })?;

        for entry in entries {
            let entry = entry.map_err(NemesisError::Io)?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                debug!("Skipping directory without SKILL.md: {}", path.display());
                continue;
            }

            match Self::scan_single_skill_static(&path, &skill_md) {
                Ok(skill) => {
                    debug!("Loaded skill: {} from {}", skill.name, skill.path);
                    skills.push(skill);
                }
                Err(e) => {
                    warn!("Failed to load skill from {}: {}", path.display(), e);
                }
            }
        }

        Ok(skills)
    }

    /// Static helper to load a single skill (no &self needed).
    fn scan_single_skill_static(skill_dir: &Path, skill_md: &Path) -> Result<SkillInfo> {
        let content = std::fs::read_to_string(skill_md)?;
        let dir_name = skill_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let (frontmatter, _body) = Self::parse_frontmatter(&content);

        let name = frontmatter.get("name").cloned().unwrap_or(dir_name.clone());
        let description = frontmatter.get("description").cloned().unwrap_or_default();

        Ok(SkillInfo {
            name,
            path: skill_dir.to_string_lossy().to_string(),
            source: "local".to_string(),
            description,
            lint_score: None,
            has_warnings: false,
        })
    }
}

#[cfg(test)]
mod tests;
