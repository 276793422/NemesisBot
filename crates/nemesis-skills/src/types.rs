//! Skill types and data structures.

use serde::{Deserialize, Serialize};

/// Information about a loaded skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    /// Skill name (from frontmatter or directory name).
    pub name: String,
    /// Filesystem path to the skill directory.
    pub path: String,
    /// Source of the skill (e.g., "local", "registry-name/slug").
    pub source: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Lint score (0.0-1.0, where 1.0 is safest). None if not linted yet.
    pub lint_score: Option<f64>,
    /// Whether any warnings were found during linting.
    #[serde(default)]
    pub has_warnings: bool,
}

/// Maximum skill name length.
pub const MAX_SKILL_NAME_LENGTH: usize = 64;
/// Maximum skill description length.
pub const MAX_SKILL_DESCRIPTION_LENGTH: usize = 1024;

impl SkillInfo {
    /// Validate the skill info, checking name and description constraints.
    ///
    /// Mirrors Go `SkillInfo.validate()`. Returns a list of validation errors
    /// (empty if valid).
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        if self.name.is_empty() {
            errors.push("name is required".to_string());
        } else {
            if self.name.len() > MAX_SKILL_NAME_LENGTH {
                errors.push(format!("name exceeds {} characters", MAX_SKILL_NAME_LENGTH));
            }
            let re = regex::Regex::new(r"^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$").unwrap();
            if !re.is_match(&self.name) {
                errors.push("name must be alphanumeric with hyphens".to_string());
            }
        }

        if self.description.is_empty() {
            errors.push("description is required".to_string());
        } else if self.description.len() > MAX_SKILL_DESCRIPTION_LENGTH {
            errors.push(format!(
                "description exceeds {} characters",
                MAX_SKILL_DESCRIPTION_LENGTH
            ));
        }

        errors
    }
}

/// A single search result from registry queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSearchResult {
    /// Relevance score (0.0-1.0).
    pub score: f64,
    /// Skill slug (URL-friendly identifier).
    pub slug: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Short summary of the skill.
    pub summary: String,
    /// Skill version string.
    #[serde(default)]
    pub version: String,
    /// Name of the registry this result came from.
    pub registry_name: String,
    /// Source repository (e.g., "anthropics/skills").
    #[serde(default)]
    pub source_repo: String,
    /// Download path within the repository (e.g., "skills/pdf/SKILL.md").
    #[serde(default)]
    pub download_path: String,
    /// Download count.
    #[serde(default)]
    pub downloads: i64,
    /// Hint that more results may exist beyond this entry.
    #[serde(default)]
    pub truncated: bool,
}

/// Search results grouped by registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySearchResult {
    /// Registry name (e.g., "clawhub", "anthropics").
    pub registry_name: String,
    /// Results from this registry (sorted by score descending).
    pub results: Vec<SkillSearchResult>,
    /// True if the registry may have more results than returned.
    #[serde(default)]
    pub truncated: bool,
}

/// Skill metadata from a registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    /// Skill slug.
    pub slug: String,
    /// Human-readable display name.
    #[serde(default)]
    pub display_name: String,
    /// Summary.
    #[serde(default)]
    pub summary: String,
    /// Latest version string.
    #[serde(default)]
    pub latest_version: String,
    /// Whether the skill was blocked as malware.
    #[serde(default)]
    pub is_malware_blocked: bool,
    /// Whether the skill is marked as suspicious.
    #[serde(default)]
    pub is_suspicious: bool,
    /// Registry name.
    #[serde(default)]
    pub registry_name: String,
    /// Author handle (e.g. GitHub username). Empty when unavailable.
    #[serde(default)]
    pub author: String,
    /// Download count. 0 when unavailable.
    #[serde(default)]
    pub downloads: i64,
}

/// Result from a DownloadAndInstall operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallResult {
    /// Installed version.
    pub version: String,
    /// Whether the skill was blocked as malware.
    #[serde(default)]
    pub is_malware_blocked: bool,
    /// Whether the skill is marked as suspicious.
    #[serde(default)]
    pub is_suspicious: bool,
    /// Summary of the skill.
    #[serde(default)]
    pub summary: String,
}

/// Skill content (SKILL.md text) fetched from a remote registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillContent {
    /// Skill slug.
    pub slug: String,
    /// Filename (typically "SKILL.md").
    pub filename: String,
    /// File content as UTF-8 text.
    pub content: String,
}

/// Sort mode for browsing skills.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowseSort {
    Trending,
    Downloads,
    Stars,
    Updated,
    Rating,
}

impl BrowseSort {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Trending => "trending",
            Self::Downloads => "downloads",
            Self::Stars => "stars",
            Self::Updated => "updated",
            Self::Rating => "rating",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "downloads" => Self::Downloads,
            "stars" => Self::Stars,
            "updated" => Self::Updated,
            "rating" => Self::Rating,
            _ => Self::Trending,
        }
    }
}

/// Result from a browse operation with cursor-based pagination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseResult {
    /// Skills on the current page.
    pub items: Vec<SkillSearchResult>,
    /// Cursor to fetch the next page. None if no more results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Origin metadata for an installed skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillOrigin {
    /// Format version.
    #[serde(default = "default_origin_version")]
    pub version: i32,
    /// Registry name (e.g., "github", "clawhub").
    pub registry: String,
    /// Skill slug.
    pub slug: String,
    /// Installed version.
    pub installed_version: String,
    /// Unix timestamp of installation.
    pub installed_at: i64,
}

fn default_origin_version() -> i32 {
    1
}

/// Configuration for all skill registries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    /// Search cache configuration.
    #[serde(default)]
    pub search_cache: SearchCacheConfig,
    /// ClawHub registry configuration.
    #[serde(default)]
    pub clawhub: ClawHubConfig,
    /// ModelScope registry configuration.
    #[serde(default)]
    pub modelscope: ModelScopeConfig,
    /// Legacy single-source GitHub config.
    #[serde(default)]
    pub github: GitHubConfig,
    /// New multi-source GitHub configs.
    #[serde(default)]
    pub github_sources: Vec<GitHubSourceConfig>,
    /// Maximum concurrent registry searches.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_searches: usize,
    /// List of GitHub source URLs for skill registries (legacy).
    #[serde(default)]
    pub github_sources_legacy: Vec<GithubSource>,
}

/// Search cache configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCacheConfig {
    /// Whether the cache is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Maximum number of cache entries (default: 50).
    #[serde(default = "default_cache_max_size")]
    pub max_size: usize,
    /// Time-to-live in seconds (default: 300 = 5 minutes).
    #[serde(default = "default_cache_ttl", alias = "ttl_seconds")]
    pub ttl_secs: u64,
}

impl Default for SearchCacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_size: 50,
            ttl_secs: 300,
        }
    }
}

fn default_cache_max_size() -> usize {
    50
}

fn default_cache_ttl() -> u64 {
    300
}

/// ModelScope registry configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelScopeConfig {
    /// Whether the ModelScope registry is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Timeout in seconds (0 = default 30s).
    #[serde(default)]
    pub timeout_secs: u64,
}

impl Default for ModelScopeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout_secs: 0,
        }
    }
}

/// ClawHub registry configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClawHubConfig {
    /// Whether the ClawHub registry is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// ClawHub website URL (e.g., "https://clawhub.ai").
    #[serde(default = "default_clawhub_url")]
    pub base_url: String,
    /// Convex deployment URL.
    #[serde(default = "default_convex_url")]
    pub convex_url: String,
    /// Convex site URL override (for ZIP downloads).
    #[serde(default)]
    pub convex_site_url: String,
    /// Timeout in seconds (0 = default 30s).
    #[serde(default, alias = "timeout")]
    pub timeout_secs: u64,
}

impl Default for ClawHubConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: default_clawhub_url(),
            convex_url: default_convex_url(),
            convex_site_url: String::new(),
            timeout_secs: 0,
        }
    }
}

fn default_clawhub_url() -> String {
    "https://clawhub.ai".to_string()
}

fn default_convex_url() -> String {
    "https://wry-manatee-359.convex.cloud".to_string()
}

/// Legacy single-source GitHub config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubConfig {
    /// Whether the GitHub registry is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Base URL (defaults to raw.githubusercontent.com).
    #[serde(default = "default_github_url")]
    pub base_url: String,
    /// Timeout in seconds (0 = default 30s).
    #[serde(default)]
    pub timeout_secs: u64,
    /// Max response size in bytes (0 = default 1MB).
    #[serde(default)]
    pub max_size: u64,
}

impl Default for GitHubConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: default_github_url(),
            timeout_secs: 0,
            max_size: 0,
        }
    }
}

fn default_github_url() -> String {
    "https://raw.githubusercontent.com".to_string()
}

/// Per-source GitHub configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitHubSourceConfig {
    /// Source name (e.g., "anthropics").
    pub name: String,
    /// Repository in "owner/repo" format.
    pub repo: String,
    /// Whether the source is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Branch (default "main").
    #[serde(default = "default_branch")]
    pub branch: String,
    /// Index type: "skills_json" or "github_api".
    #[serde(default = "default_index_type")]
    pub index_type: String,
    /// Index path (e.g., "skills.json").
    #[serde(default)]
    pub index_path: String,
    /// Skill path pattern (e.g., "skills/{slug}/SKILL.md").
    #[serde(default)]
    pub skill_path_pattern: String,
    /// Timeout in seconds.
    #[serde(default)]
    pub timeout_secs: u64,
    /// Max response size in bytes.
    #[serde(default)]
    pub max_size: u64,
}

fn default_true() -> bool {
    true
}

fn default_branch() -> String {
    "main".to_string()
}

fn default_index_type() -> String {
    "skills_json".to_string()
}

/// A GitHub-based skill source (legacy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubSource {
    /// Display name for this registry source.
    pub name: String,
    /// GitHub repository URL (e.g., "https://github.com/org/skills").
    pub url: String,
    /// Branch to use (defaults to "main").
    #[serde(default = "default_branch")]
    pub branch: String,
}

/// Available skill from a remote listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableSkill {
    /// Skill name.
    pub name: String,
    /// Repository.
    #[serde(default)]
    pub repository: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Author.
    #[serde(default)]
    pub author: String,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Security check result combining lint + quality + signature checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityCheckResult {
    /// Lint result.
    pub lint_result: crate::lint::LintResult,
    /// Quality score.
    pub quality_score: Option<crate::quality::QualityScore>,
    /// Whether the skill is blocked.
    pub blocked: bool,
    /// Reason for blocking.
    #[serde(default)]
    pub block_reason: String,
}

fn default_max_concurrent() -> usize {
    2
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            search_cache: SearchCacheConfig::default(),
            clawhub: ClawHubConfig::default(),
            modelscope: ModelScopeConfig::default(),
            github: GitHubConfig::default(),
            github_sources: Vec::new(),
            max_concurrent_searches: 2,
            github_sources_legacy: Vec::new(),
        }
    }
}

/// Validate a skill identifier to prevent path traversal attacks.
pub fn validate_skill_identifier(slug: &str) -> std::result::Result<(), String> {
    let trimmed = slug.trim();
    if trimmed.is_empty() {
        return Err("skill identifier cannot be empty".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err("skill identifier cannot contain path separators".to_string());
    }
    if trimmed.contains("..") {
        return Err("skill identifier cannot contain '..'".to_string());
    }
    if trimmed.len() > 64 {
        return Err("skill identifier too long (max 64 characters)".to_string());
    }
    Ok(())
}

/// Case-insensitive substring match.
pub fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Convert string to lowercase (ASCII-only, efficient).
pub fn to_lower(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
