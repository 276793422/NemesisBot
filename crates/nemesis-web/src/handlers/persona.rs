//! Persona handler — manage AI persona profiles with GitHub marketplace integration.
//!
//! Supports browsing, searching, previewing, and downloading personas from the
//! agency-agents GitHub repository. Downloaded agents are converted from single
//! .md files into the NemesisBot persona format (IDENTITY.md, SOUL.md, AGENT.md,
//! TOOLS.md).

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::handlers::{get_str, require_workspace, resolve_path};
use crate::ws_router::{ModuleHandler, RequestContext};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const PERSONA_FILES: &[&str] = &["IDENTITY.md", "SOUL.md", "AGENT.md", "TOOLS.md"];

/// Files that are archived/restored during persona switch (in addition to PERSONA_FILES).
const PERSONA_ARCHIVE_FILES: &[&str] = &["HEARTBEAT.md"];

/// Directories that are archived/restored during persona switch.
const PERSONA_ARCHIVE_DIRS: &[&str] = &["memory", "sessions", "logs/session_logs"];
const GITHUB_API: &str = "https://api.github.com";
const AGENCY_REPO: &str = "msitarzewski/agency-agents";
const AGENCY_BRANCH: &str = "main";

/// System default AGENT.md content — embedded at compile time.
const DEFAULT_AGENT_MD: &str = include_str!("../../../../nemesisbot/workspace/AGENT.md");
/// System default TOOLS.md content — embedded at compile time.
const DEFAULT_TOOLS_MD: &str = include_str!("../../../../nemesisbot/workspace/TOOLS.md");

/// Directories in agency-agents that are NOT agent categories (skip entirely).
const SKIP_DIRS: &[&str] = &[
    "scripts",
    "examples",
    "integrations",
    "strategy",
    ".github",
];

/// Filenames to skip even inside valid category directories.
const SKIP_FILENAMES: &[&str] = &[
    "README.md",
    "CONTRIBUTING.md",
    "LICENSE.md",
    "SECURITY.md",
    "CONTRIBUTING_zh-CN.md",
];

/// Browser-like User-Agent for GitHub API requests.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Clone)]
#[allow(dead_code)]
struct AgentEntry {
    id: String,
    name: String,
    emoji: String,
    description: String,
    category: String,
    path: String,
}

#[derive(Clone)]
struct Frontmatter {
    name: String,
    emoji: String,
    description: String,
    vibe: String,
    color: String,
    tools: String,
    raw_yaml: String,
}

struct Section {
    title: String,
    content: String,
}

// ---------------------------------------------------------------------------
// Category mapping
// ---------------------------------------------------------------------------

fn map_category(dir: &str) -> &'static str {
    match dir {
        "engineering" => "开发",
        "marketing" => "营销",
        "security" => "安全",
        "design" => "创意",
        "academic" => "学术",
        "product" => "产品",
        "paid-media" => "付费媒体",
        "sales" => "销售",
        "finance" => "金融",
        "game-development" => "游戏开发",
        "gis" => "GIS",
        "spatial-computing" => "空间计算",
        "specialized" => "专业",
        "testing" => "测试",
        "support" => "客服",
        "project-management" => "项目管理",
        _ => "通用",
    }
}

// ---------------------------------------------------------------------------
// File filtering & parsing
// ---------------------------------------------------------------------------

fn is_agent_file(path: &str) -> bool {
    if !path.ends_with(".md") {
        return false;
    }
    if !path.contains('/') {
        return false;
    }
    let first_dir = path.split('/').next().unwrap_or("");
    if SKIP_DIRS.contains(&first_dir) {
        return false;
    }
    let filename = path.rsplit('/').next().unwrap_or("");
    if SKIP_FILENAMES.contains(&filename) {
        return false;
    }
    if filename.starts_with("QUICKSTART") || filename.starts_with("EXECUTIVE") {
        return false;
    }
    true
}

fn parse_agent_from_path(path: &str) -> AgentEntry {
    let parts: Vec<&str> = path.trim_end_matches(".md").split('/').collect();
    let id = parts.last().unwrap_or(&"").to_string();
    let category_dir = parts.first().unwrap_or(&"").to_string();
    let name = id
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    AgentEntry {
        id,
        name,
        emoji: "🤖".to_string(),
        description: String::new(),
        category: map_category(&category_dir).to_string(),
        path: path.to_string(),
    }
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("{}", e))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("{}", e))? {
        let entry = entry.map_err(|e| format!("{}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).map_err(|e| format!("{}", e))?;
        }
    }
    Ok(())
}

/// Extract name and emoji from NemesisBot IDENTITY.md format.
fn extract_identity_info(content: &str) -> (String, String) {
    let mut name = "default".to_string();
    let mut emoji = "🤖".to_string();
    for line in content.lines() {
        let line = line.trim();
        // Match "**姓名：** xxx" or "**姓名:** xxx"
        if line.contains("姓名") && (line.contains("：") || line.contains(":")) {
            if let Some(idx) = line.find("：").or_else(|| line.find(": **")).or_else(|| line.find(":**")) {
                let rest = &line[idx + "：".len()..].trim_start_matches('*').trim_start_matches(' ').trim_start_matches('*').trim();
                if !rest.is_empty() {
                    name = rest.to_string();
                }
            } else if let Some(rest) = line.splitn(2, |c: char| c == '：' || c == ':').nth(1) {
                let rest = rest.trim().trim_start_matches('*').trim();
                if !rest.is_empty() {
                    name = rest.to_string();
                }
            }
        }
        if line.contains("表情符号") && (line.contains("：") || line.contains(":")) {
            for ch in line.chars() {
                if is_emoji_char(ch) {
                    emoji = ch.to_string();
                    break;
                }
            }
        }
    }
    (name, emoji)
}


// ---------------------------------------------------------------------------
// GitHub API helpers
// ---------------------------------------------------------------------------

fn get_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client")
    })
}

/// L1 cache: file tree from GitHub Trees API.
static TREE_CACHE: std::sync::LazyLock<std::sync::Mutex<Option<Vec<(String, i64)>>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

fn invalidate_cache() {
    if let Ok(mut tree) = TREE_CACHE.lock() {
        *tree = None;
    }
    FM_CACHE.lock().unwrap().clear();
    CONTENT_CACHE.lock().unwrap().clear();
}

async fn fetch_tree() -> Result<Vec<(String, i64)>, String> {
    {
        let cache = TREE_CACHE.lock().unwrap();
        if let Some(ref tree) = *cache {
            return Ok(tree.clone());
        }
    }
    let url = format!(
        "{}/repos/{}/git/trees/{}?recursive=1",
        GITHUB_API, AGENCY_REPO, AGENCY_BRANCH
    );
    let resp = get_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("GitHub tree API error: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub tree API returned {}: {}", status, body));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse tree response: {}", e))?;
    let mut entries = Vec::new();
    if let Some(tree) = json["tree"].as_array() {
        for item in tree {
            let path = item["path"].as_str().unwrap_or("");
            let size = item["size"].as_i64().unwrap_or(0);
            if is_agent_file(path) {
                entries.push((path.to_string(), size));
            }
        }
    }
    let mut cache = TREE_CACHE.lock().unwrap();
    *cache = Some(entries);
    Ok(cache.as_ref().unwrap().clone())
}

/// L2 cache: frontmatter extracted from downloaded files.
static FM_CACHE: std::sync::LazyLock<std::sync::Mutex<HashMap<String, Frontmatter>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

/// L3 cache: full .md content downloaded from GitHub.
static CONTENT_CACHE: std::sync::LazyLock<std::sync::Mutex<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

async fn fetch_agent_content(id: &str) -> Result<String, String> {
    // Check L3 cache
    {
        let cache = CONTENT_CACHE.lock().unwrap();
        if let Some(content) = cache.get(id) {
            return Ok(content.clone());
        }
    }

    // Find path from tree
    let tree = fetch_tree().await?;
    let path = tree
        .iter()
        .find(|(p, _)| {
            let filename = p.rsplit('/').next().unwrap_or("");
            filename.trim_end_matches(".md") == id || filename == id
        })
        .map(|(p, _)| p.as_str())
        .ok_or_else(|| format!("agent '{}' not found in repository", id))?;

    let url = format!(
        "{}/repos/{}/contents/{}?ref={}",
        GITHUB_API, AGENCY_REPO, path, AGENCY_BRANCH
    );
    let resp = get_client()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("GitHub contents API error: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub contents API returned {}: {}", status, body));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse contents response: {}", e))?;

    let encoding = json["encoding"].as_str().unwrap_or("");
    let content_b64 = json["content"]
        .as_str()
        .ok_or("missing content field in GitHub response")?;

    let content = if encoding == "base64" {
        // GitHub returns base64 with newlines
        let cleaned: String = content_b64.chars().filter(|c| !c.is_whitespace()).collect();
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &cleaned)
            .map_err(|e| format!("base64 decode error: {}", e))?
    } else {
        content_b64.as_bytes().to_vec()
    };

    let text = String::from_utf8(content).map_err(|e| format!("utf8 error: {}", e))?;

    // Parse frontmatter and update L2 cache
    if let Some(fm) = parse_frontmatter(&text) {
        let mut cache = FM_CACHE.lock().unwrap();
        cache.insert(id.to_string(), fm);
    }

    // Update L3 cache
    {
        let mut cache = CONTENT_CACHE.lock().unwrap();
        cache.insert(id.to_string(), text.clone());
    }

    Ok(text)
}

// ---------------------------------------------------------------------------
// Frontmatter parsing (simple, no serde_yaml dependency)
// ---------------------------------------------------------------------------

fn parse_frontmatter(content: &str) -> Option<Frontmatter> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_first = &trimmed[3..];
    let end = after_first.find("---")?;
    let yaml = &after_first[..end];

    let mut name = String::new();
    let mut emoji = String::new();
    let mut description = String::new();
    let mut vibe = String::new();
    let mut color = String::new();
    let mut tools = String::new();

    for line in yaml.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("emoji:") {
            emoji = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("description:") {
            description = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("vibe:") {
            vibe = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("color:") {
            color = val.trim().trim_matches('"').to_string();
        } else if let Some(val) = line.strip_prefix("tools:") {
            tools = val.trim().trim_matches('"').to_string();
        }
    }

    if name.is_empty() {
        return None;
    }

    Some(Frontmatter {
        name,
        emoji,
        description,
        vibe,
        color,
        tools,
        raw_yaml: yaml.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Section parsing
// ---------------------------------------------------------------------------

fn strip_emoji(s: &str) -> String {
    s.chars()
        .filter(|c| !is_emoji_char(*c))
        .collect::<String>()
        .trim()
        .to_lowercase()
}

fn is_emoji_char(c: char) -> bool {
    // Common emoji ranges
    matches!(
        c,
        '\u{1F300}'..='\u{1F9FF}'
            | '\u{2600}'..='\u{26FF}'
            | '\u{2700}'..='\u{27BF}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{1F000}'..='\u{1FFFF}'
            | '\u{200D}' // ZWJ
    )
}

struct ParsedContent {
    preamble: String,
    sections: Vec<Section>,
}

fn parse_sections(content: &str) -> ParsedContent {
    let mut sections = Vec::new();

    // Find body after frontmatter
    let body = if content.trim_start().starts_with("---") {
        let after_first = &content.trim_start()[3..];
        if let Some(end) = after_first.find("---") {
            &after_first[end + 3..]
        } else {
            content
        }
    } else {
        content
    };

    let mut preamble_lines: Vec<String> = Vec::new();
    let mut current_title = String::new();
    let mut current_lines: Vec<String> = Vec::new();
    let mut found_first_h2 = false;

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") && !trimmed.starts_with("### ") {
            if found_first_h2 {
                sections.push(Section {
                    title: current_title.clone(),
                    content: current_lines.join("\n").trim().to_string(),
                });
                current_lines.clear();
            }
            found_first_h2 = true;
            current_title = trimmed[3..].trim().to_string();
        } else if found_first_h2 {
            current_lines.push(line.to_string());
        } else {
            // Before first ## — collect as preamble (skip H1 title)
            let is_h1 = trimmed.starts_with("# ") && !trimmed.starts_with("## ");
            if !is_h1 {
                preamble_lines.push(line.to_string());
            }
        }
    }

    if found_first_h2 && !current_title.is_empty() {
        sections.push(Section {
            title: current_title.clone(),
            content: current_lines.join("\n").trim().to_string(),
        });
    }

    ParsedContent {
        preamble: preamble_lines.join("\n").trim().to_string(),
        sections,
    }
}

// ---------------------------------------------------------------------------
// Conversion engine
// ---------------------------------------------------------------------------

enum SectionTarget {
    Identity,
    Soul,
    Agent,
    Tools,
}

fn classify_section(title: &str) -> SectionTarget {
    let norm = strip_emoji(title);
    if norm.contains("identity") || norm.contains("memory") {
        return SectionTarget::Identity;
    }
    if norm.contains("critical rule")
        || norm.contains("communication style")
        || norm.contains("checklist")
    {
        return SectionTarget::Soul;
    }
    if norm.contains("tool") || norm.contains("integration") {
        return SectionTarget::Tools;
    }
    // Everything else goes to Agent (including core mission, workflow, etc.)
    SectionTarget::Agent
}

fn build_persona_json(fm: Option<&Frontmatter>) -> serde_json::Value {
    let name = fm.map(|f| f.name.as_str()).unwrap_or("Unknown");
    let emoji = fm.map(|f| f.emoji.as_str()).unwrap_or("🤖");
    let description = fm.map(|f| f.description.as_str()).unwrap_or("");
    let mut obj = serde_json::json!({
        "name": name,
        "emoji": emoji,
        "description": description,
    });
    if let Some(f) = fm {
        if !f.color.is_empty() {
            obj["color"] = serde_json::Value::String(f.color.clone());
        }
        if !f.tools.is_empty() {
            obj["tools"] = serde_json::Value::String(f.tools.clone());
        }
        if !f.vibe.is_empty() {
            obj["vibe"] = serde_json::Value::String(f.vibe.clone());
        }
        if !f.raw_yaml.is_empty() {
            obj["frontmatter"] = serde_json::Value::String(f.raw_yaml.clone());
        }
    }
    obj
}

struct PersonaFiles {
    identity: String,
    soul: String,
    agent_extra: String, // to be appended to system default AGENT.md
    tools_extra: String, // to be appended to system default TOOLS.md (may be empty)
}

fn convert_agent_md(content: &str) -> PersonaFiles {
    let fm = parse_frontmatter(content);
    let parsed = parse_sections(content);

    let name = fm.as_ref().map(|f| f.name.as_str()).unwrap_or("Unknown");
    let emoji = fm.as_ref().map(|f| f.emoji.as_str()).unwrap_or("🤖");
    let vibe = fm.as_ref().map(|f| f.vibe.as_str()).unwrap_or("");
    let description = fm
        .as_ref()
        .map(|f| f.description.as_str())
        .unwrap_or("");

    // Build IDENTITY.md (full replacement)
    let identity_sections: Vec<&Section> = parsed.sections
        .iter()
        .filter(|s| matches!(classify_section(&s.title), SectionTarget::Identity))
        .collect();

    let mut identity = format!(
        "# {}\n\n{}{}\n\n## 基本信息\n\n- 姓名：{}\n- Emoji：{}\n- 风格：{}\n- 描述：{}\n",
        name,
        if vibe.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", vibe)
        },
        if description.is_empty() {
            String::new()
        } else {
            format!("> {}\n\n", description)
        },
        name,
        emoji,
        vibe,
        description,
    );
    // Include preamble (role description between frontmatter and first ##)
    if !parsed.preamble.is_empty() {
        identity.push_str(&format!("\n---\n\n{}\n", parsed.preamble));
    }
    for sec in &identity_sections {
        identity.push_str(&format!("\n---\n\n## {}\n\n{}\n", sec.title, sec.content));
    }

    // Build SOUL.md (full replacement)
    let soul_sections: Vec<&Section> = parsed.sections
        .iter()
        .filter(|s| matches!(classify_section(&s.title), SectionTarget::Soul))
        .collect();

    let mut soul = format!("# {} — 行为规则\n\n", name);
    for sec in &soul_sections {
        soul.push_str(&format!("## {}\n\n{}\n\n", sec.title, sec.content));
    }

    // Build AGENT.md extra (to be appended to system default)
    let agent_sections: Vec<&Section> = parsed.sections
        .iter()
        .filter(|s| matches!(classify_section(&s.title), SectionTarget::Agent))
        .collect();

    let mut agent_extra = format!("\n---\n\n# {} 专用工作流\n", name);
    for sec in &agent_sections {
        agent_extra.push_str(&format!("\n## {}\n\n{}\n", sec.title, sec.content));
    }

    // Build TOOLS.md extra (to be appended to system default)
    let tools_sections: Vec<&Section> = parsed.sections
        .iter()
        .filter(|s| matches!(classify_section(&s.title), SectionTarget::Tools))
        .collect();

    let tools_extra = if tools_sections.is_empty() {
        String::new()
    } else {
        let mut extra = format!("\n---\n\n# {} 工具使用\n", name);
        for sec in &tools_sections {
            extra.push_str(&format!("\n## {}\n\n{}\n", sec.title, sec.content));
        }
        extra
    };

    PersonaFiles {
        identity,
        soul,
        agent_extra,
        tools_extra,
    }
}

// ---------------------------------------------------------------------------
// PersonaHandler
// ---------------------------------------------------------------------------

pub struct PersonaHandler;

#[async_trait::async_trait]
impl ModuleHandler for PersonaHandler {
    fn module_name(&self) -> &str {
        "persona"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "current" => self.cmd_current(workspace),
            "list" => self.cmd_list(workspace),
            "activate" => {
                let d = data.ok_or("missing data")?;
                let name = get_str(&d, "name")?;
                let result = self.cmd_activate(workspace, &name)?;
                restart_agent(ctx)?;
                Ok(result)
            }
            "restore" => {
                let result = self.cmd_activate(workspace, "default")?;
                restart_agent(ctx)?;
                Ok(result)
            }
            "remove" => {
                let d = data.ok_or("missing data")?;
                let name = get_str(&d, "name")?;
                self.cmd_remove(workspace, &name)
            }
            "file.get" => {
                let d = data.ok_or("missing data")?;
                let name = get_str(&d, "name")?;
                let file = get_str(&d, "file")?;
                self.cmd_file_get(workspace, &name, &file)
            }
            "file.save" => {
                let d = data.ok_or("missing data")?;
                let name = get_str(&d, "name")?;
                let file = get_str(&d, "file")?;
                let content = get_str(&d, "content")?;
                self.cmd_file_save(workspace, &name, &file, &content)
            }
            "shop.browse" => self.cmd_shop_browse(workspace, data.as_ref()).await,
            "shop.search" => self.cmd_shop_search(workspace, data.as_ref()).await,
            "shop.refresh" => {
                invalidate_cache();
                Ok(Some(serde_json::json!({"refreshed": true})))
            }
            "shop.preview" => {
                let d = data.ok_or("missing data")?;
                let id = get_str(&d, "id")?;
                self.cmd_shop_preview(workspace, &id).await
            }
            "shop.download" => {
                let d = data.ok_or("missing data")?;
                let id = get_str(&d, "id")?;
                self.cmd_shop_download(workspace, &id).await
            }
            _ => Err(format!("unknown command: persona.{}", cmd)),
        }
    }
}

fn restart_agent(ctx: &crate::ws_router::RequestContext) -> Result<(), String> {
    let svc = ctx.state.agent_service.as_ref()
        .ok_or("Agent not available")?;
    svc.stop()?;
    tracing::info!("[Persona] Agent stopped for persona switch");
    svc.start()?;
    tracing::info!("[Persona] Agent restarted with new persona files");
    Ok(())
}

impl PersonaHandler {
    pub fn new() -> Self {
        Self
    }

    // -----------------------------------------------------------------------
    // Auto-migration
    // -----------------------------------------------------------------------

    fn ensure_initialized(&self, workspace: &str) -> Result<(), String> {
        let personas_dir = resolve_path(workspace, "personas")?;
        if personas_dir.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&personas_dir)
            .map_err(|e| format!("failed to create personas dir: {}", e))?;

        let default_dir = personas_dir.join("default");
        std::fs::create_dir_all(&default_dir)
            .map_err(|e| format!("failed to create default dir: {}", e))?;

        for &file in PERSONA_FILES.iter().chain(PERSONA_ARCHIVE_FILES.iter()) {
            let src = resolve_path(workspace, file)?;
            if src.exists() {
                let content = std::fs::read_to_string(&src)
                    .map_err(|e| format!("failed to read {}: {}", file, e))?;
                std::fs::write(default_dir.join(file), &content)
                    .map_err(|e| format!("failed to write {}: {}", file, e))?;
            }
        }

        // Archive memory/ directory if it exists
        for &dir_name in PERSONA_ARCHIVE_DIRS {
            let src_dir = resolve_path(workspace, dir_name)?;
            if src_dir.exists() {
                let dst_dir = default_dir.join(dir_name);
                copy_dir_recursive(&src_dir, &dst_dir)
                    .map_err(|e| format!("failed to archive {}: {}", dir_name, e))?;
            }
        }

        // Build PERSONA.json from actual workspace files
        let identity_path = resolve_path(workspace, "IDENTITY.md")?;
        let (name, emoji) = if identity_path.exists() {
            let content = std::fs::read_to_string(&identity_path)
                .unwrap_or_default();
            extract_identity_info(&content)
        } else {
            ("default".to_string(), "🤖".to_string())
        };
        let persona_json = serde_json::json!({
            "name": name,
            "emoji": emoji,
            "description": "",
        });
        let json_str = serde_json::to_string_pretty(&persona_json)
            .map_err(|e| format!("failed to serialize PERSONA.json: {}", e))?;
        std::fs::write(default_dir.join("PERSONA.json"), json_str)
            .map_err(|e| format!("failed to write PERSONA.json: {}", e))?;

        let active = serde_json::json!({"name": "default"});
        let active_str = serde_json::to_string_pretty(&active)
            .map_err(|e| format!("failed to serialize _active.json: {}", e))?;
        std::fs::write(personas_dir.join("_active.json"), active_str)
            .map_err(|e| format!("failed to write _active.json: {}", e))?;

        Ok(())
    }

    fn read_active(&self, workspace: &str) -> Result<String, String> {
        let path = resolve_path(workspace, "personas/_active.json")?;
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read _active.json: {}", e))?;
        let v: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse _active.json: {}", e))?;
        v.get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "invalid _active.json: missing name".to_string())
    }

    fn write_active(&self, workspace: &str, name: &str) -> Result<(), String> {
        let path = resolve_path(workspace, "personas/_active.json")?;
        let v = serde_json::json!({"name": name});
        let s = serde_json::to_string_pretty(&v)
            .map_err(|e| format!("serialize error: {}", e))?;
        std::fs::write(&path, s)
            .map_err(|e| format!("failed to write _active.json: {}", e))
    }

    fn read_persona_json(
        &self,
        workspace: &str,
        dir_name: &str,
    ) -> Result<serde_json::Value, String> {
        let path = resolve_path(workspace, &format!("personas/{}/PERSONA.json", dir_name))?;
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read personas/{}/PERSONA.json: {}", dir_name, e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse personas/{}/PERSONA.json: {}", dir_name, e))
    }

    // -----------------------------------------------------------------------
    // Local management commands (synchronous)
    // -----------------------------------------------------------------------

    fn cmd_current(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        self.ensure_initialized(workspace)?;
        let active = self.read_active(workspace)?;
        let pj = self.read_persona_json(workspace, &active)?;

        let files: Vec<String> = PERSONA_FILES.iter().map(|f| f.to_string()).collect();

        Ok(Some(serde_json::json!({
            "name": pj["name"],
            "emoji": pj["emoji"],
            "description": pj["description"],
            "active_dir": active,
            "files": files,
        })))
    }

    fn cmd_list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        self.ensure_initialized(workspace)?;
        let active = self.read_active(workspace)?;

        let personas_dir = resolve_path(workspace, "personas")?;
        let entries = std::fs::read_dir(&personas_dir)
            .map_err(|e| format!("failed to read personas dir: {}", e))?;

        let mut personas = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            let pj = match self.read_persona_json(workspace, &dir_name) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let is_active = dir_name == active;
            let is_default = dir_name == "default";

            let mut files = Vec::new();
            for &f in PERSONA_FILES {
                if resolve_path(workspace, &format!("personas/{}/{}", dir_name, f))?.exists() {
                    files.push(f.to_string());
                }
            }

            personas.push(serde_json::json!({
                "dir": dir_name,
                "name": pj["name"],
                "emoji": pj["emoji"],
                "description": pj["description"],
                "is_default": is_default,
                "is_active": is_active,
                "files": files,
            }));
        }

        personas.sort_by(|a, b| {
            let a_default = a["is_default"].as_bool().unwrap_or(false);
            let b_default = b["is_default"].as_bool().unwrap_or(false);
            if a_default {
                return std::cmp::Ordering::Less;
            }
            if b_default {
                return std::cmp::Ordering::Greater;
            }
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

        Ok(Some(serde_json::json!({
            "active": active,
            "personas": personas,
        })))
    }

    fn cmd_activate(
        &self,
        workspace: &str,
        name: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        self.ensure_initialized(workspace)?;

        let target_dir = resolve_path(workspace, &format!("personas/{}", name))?;
        if !target_dir.exists() {
            return Err(format!("persona '{}' not found", name));
        }

        // Step 1: Archive current persona data
        let current = self.read_active(workspace)?;
        if current != name {
            let archive_dir = resolve_path(workspace, &format!("personas/{}", current))?;
            std::fs::create_dir_all(&archive_dir)
                .map_err(|e| format!("failed to create archive dir: {}", e))?;
            let all_files: Vec<&&str> = PERSONA_FILES.iter()
                .chain(PERSONA_ARCHIVE_FILES.iter())
                .collect();
            for &file in &all_files {
                let src = resolve_path(workspace, file)?;
                if src.exists() {
                    let content = std::fs::read_to_string(&src)
                        .map_err(|e| format!("archive: failed to read {}: {}", file, e))?;
                    std::fs::write(archive_dir.join(file), &content)
                        .map_err(|e| format!("archive: failed to write {}: {}", file, e))?;
                }
            }
            // Archive directories (memory/)
            for &dir_name in PERSONA_ARCHIVE_DIRS {
                let src_dir = resolve_path(workspace, dir_name)?;
                if src_dir.exists() {
                    let dst_dir = archive_dir.join(dir_name);
                    if dst_dir.exists() {
                        std::fs::remove_dir_all(&dst_dir)
                            .map_err(|e| format!("archive: failed to clean {}: {}", dir_name, e))?;
                    }
                    copy_dir_recursive(&src_dir, &dst_dir)
                        .map_err(|e| format!("archive: failed to copy {}: {}", dir_name, e))?;
                }
            }
        }

        // Step 2: Clean workspace root of all persona-managed files/dirs
        let all_files: Vec<&&str> = PERSONA_FILES.iter()
            .chain(PERSONA_ARCHIVE_FILES.iter())
            .collect();
        for &file in &all_files {
            let dst = resolve_path(workspace, file)?;
            if dst.exists() {
                let _ = std::fs::remove_file(&dst);
            }
        }
        for &dir_name in PERSONA_ARCHIVE_DIRS {
            let dst_dir = resolve_path(workspace, dir_name)?;
            if dst_dir.exists() {
                std::fs::remove_dir_all(&dst_dir)
                    .map_err(|e| format!("clean: failed to remove {}: {}", dir_name, e))?;
            }
        }

        // Step 3: Restore target persona data
        for &file in &all_files {
            let src = target_dir.join(file);
            if src.exists() {
                let content = std::fs::read_to_string(&src)
                    .map_err(|e| format!("restore: failed to read {}: {}", file, e))?;
                let dst = resolve_path(workspace, file)?;
                if let Some(parent) = dst.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&dst, &content)
                    .map_err(|e| format!("restore: failed to write {}: {}", file, e))?;
            }
        }
        // Restore directories
        for &dir_name in PERSONA_ARCHIVE_DIRS {
            let src_dir = target_dir.join(dir_name);
            if src_dir.exists() {
                let dst_dir = resolve_path(workspace, dir_name)?;
                std::fs::create_dir_all(&dst_dir)
                    .map_err(|e| format!("restore: failed to create {}: {}", dir_name, e))?;
                copy_dir_recursive(&src_dir, &dst_dir)
                    .map_err(|e| format!("restore: failed to copy {}: {}", dir_name, e))?;
            }
        }

        // Ensure memory/ dir and default MEMORY.md exist for new personas
        let memory_dir = resolve_path(workspace, "memory")?;
        let memory_file = memory_dir.join("MEMORY.md");
        if !memory_file.exists() {
            let _ = std::fs::create_dir_all(&memory_dir);
            let default = "# 长期记忆\n\n（此文件存储应该跨会话持久化保存的重要信息。）\n\n## 用户信息\n\n（关于用户的重要事实）\n\n## 偏好\n\n（随时间了解到的用户偏好）\n\n## 重要笔记\n\n（需要记住的事情）\n";
            let _ = std::fs::write(&memory_file, default);
        }

        self.write_active(workspace, name)?;

        Ok(Some(serde_json::json!({
            "activated": true,
            "name": name,
        })))
    }

    fn cmd_remove(
        &self,
        workspace: &str,
        name: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        if name == "default" {
            return Err("cannot remove default persona".to_string());
        }

        let active = self.read_active(workspace)?;
        if active == name {
            self.cmd_activate(workspace, "default")?;
        }

        let dir = resolve_path(workspace, &format!("personas/{}", name))?;
        if !dir.exists() {
            return Err(format!("persona '{}' not found", name));
        }
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("failed to remove persona '{}': {}", name, e))?;

        Ok(Some(serde_json::json!({"removed": true, "name": name})))
    }

    fn cmd_file_get(
        &self,
        workspace: &str,
        name: &str,
        file: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = resolve_path(workspace, &format!("personas/{}/{}", name, file))?;
        if !path.exists() {
            return Ok(Some(serde_json::json!({"name": file, "content": ""})));
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {}", file, e))?;
        Ok(Some(serde_json::json!({"name": file, "content": content})))
    }

    fn cmd_file_save(
        &self,
        workspace: &str,
        name: &str,
        file: &str,
        content: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = resolve_path(workspace, &format!("personas/{}/{}", name, file))?;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, content)
            .map_err(|e| format!("failed to write {}: {}", file, e))?;

        let active = self.read_active(workspace).unwrap_or_default();
        if active == name {
            let root_path = resolve_path(workspace, file)?;
            std::fs::write(&root_path, content)
                .map_err(|e| format!("failed to sync to root: {}", e))?;
        }

        Ok(Some(serde_json::json!({"saved": true, "name": file})))
    }

    // -----------------------------------------------------------------------
    // Shop commands (async, GitHub API)
    // -----------------------------------------------------------------------

    async fn cmd_shop_browse(
        &self,
        workspace: &str,
        data: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        let division_filter = data
            .and_then(|d| d.get("division"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let tree = fetch_tree().await?;
        let fm_cache = FM_CACHE.lock().unwrap();

        let mut items = Vec::new();
        for (path, _size) in &tree {
            let mut entry = parse_agent_from_path(path);
            // Override with cached frontmatter if available
            if let Some(fm) = fm_cache.get(&entry.id) {
                entry.name = fm.name.clone();
                entry.emoji = fm.emoji.clone();
                entry.description = fm.description.clone();
            }
            if !division_filter.is_empty() && entry.category != division_filter {
                continue;
            }
            let installed = resolve_path(workspace, &format!("personas/{}", entry.id))
                .map(|p| p.exists())
                .unwrap_or(false);
            items.push(serde_json::json!({
                "id": entry.id,
                "name": entry.name,
                "emoji": entry.emoji,
                "division": entry.category,
                "description": entry.description,
                "installed": installed,
            }));
        }

        Ok(Some(serde_json::json!({"items": items})))
    }

    async fn cmd_shop_search(
        &self,
        workspace: &str,
        data: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        let query = data
            .and_then(|d| d.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();

        let tree = fetch_tree().await?;
        let fm_cache = FM_CACHE.lock().unwrap();

        let mut items = Vec::new();
        for (path, _size) in &tree {
            let mut entry = parse_agent_from_path(path);
            if let Some(fm) = fm_cache.get(&entry.id) {
                entry.name = fm.name.clone();
                entry.emoji = fm.emoji.clone();
                entry.description = fm.description.clone();
            }
            let haystack = format!("{} {} {}", entry.name, entry.category, entry.id).to_lowercase();
            if !query.is_empty() && !haystack.contains(&query) {
                continue;
            }
            let installed = resolve_path(workspace, &format!("personas/{}", entry.id))
                .map(|p| p.exists())
                .unwrap_or(false);
            items.push(serde_json::json!({
                "id": entry.id,
                "name": entry.name,
                "emoji": entry.emoji,
                "division": entry.category,
                "description": entry.description,
                "installed": installed,
            }));
        }

        Ok(Some(serde_json::json!({"items": items})))
    }

    async fn cmd_shop_preview(
        &self,
        workspace: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let content = fetch_agent_content(id).await?;
        let fm = parse_frontmatter(&content);
        let files = convert_agent_md(&content);

        let name = fm.as_ref().map(|f| f.name.as_str()).unwrap_or(id);
        let emoji = fm.as_ref().map(|f| f.emoji.as_str()).unwrap_or("🤖");

        let installed = resolve_path(workspace, &format!("personas/{}", id))
            .map(|p| p.exists())
            .unwrap_or(false);

        // Build converted files preview
        let agent_full = format!("{}\n{}", DEFAULT_AGENT_MD, files.agent_extra);
        let tools_full = if files.tools_extra.is_empty() {
            DEFAULT_TOOLS_MD.to_string()
        } else {
            format!("{}\n{}", DEFAULT_TOOLS_MD, files.tools_extra)
        };
        let persona_json = build_persona_json(fm.as_ref());

        Ok(Some(serde_json::json!({
            "id": id,
            "name": name,
            "emoji": emoji,
            "installed": installed,
            "raw": content,
            "converted": {
                "IDENTITY.md": files.identity,
                "SOUL.md": files.soul,
                "AGENT.md": agent_full,
                "TOOLS.md": tools_full,
                "PERSONA.json": serde_json::to_string_pretty(&persona_json).unwrap_or_default(),
            },
        })))
    }

    async fn cmd_shop_download(
        &self,
        workspace: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        // Check not already installed
        let dir = resolve_path(workspace, &format!("personas/{}", id))?;
        if dir.exists() {
            return Err(format!("persona '{}' already installed", id));
        }

        let content = fetch_agent_content(id).await?;
        let fm = parse_frontmatter(&content);
        let files = convert_agent_md(&content);

        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("failed to create dir: {}", e))?;

        // Write PERSONA.json
        let persona_json = build_persona_json(fm.as_ref());
        std::fs::write(
            dir.join("PERSONA.json"),
            serde_json::to_string_pretty(&persona_json).unwrap(),
        )
        .map_err(|e| format!("failed to write PERSONA.json: {}", e))?;

        // Write IDENTITY.md (full replacement)
        std::fs::write(dir.join("IDENTITY.md"), &files.identity)
            .map_err(|e| format!("failed to write IDENTITY.md: {}", e))?;

        // Write SOUL.md (full replacement)
        std::fs::write(dir.join("SOUL.md"), &files.soul)
            .map_err(|e| format!("failed to write SOUL.md: {}", e))?;

        // Write AGENT.md (system default + persona workflow)
        let agent_content = format!("{}\n{}", DEFAULT_AGENT_MD, files.agent_extra);
        std::fs::write(dir.join("AGENT.md"), &agent_content)
            .map_err(|e| format!("failed to write AGENT.md: {}", e))?;

        // Write TOOLS.md (system default + persona tools if any)
        let tools_content = if files.tools_extra.is_empty() {
            DEFAULT_TOOLS_MD.to_string()
        } else {
            format!("{}\n{}", DEFAULT_TOOLS_MD, files.tools_extra)
        };
        std::fs::write(dir.join("TOOLS.md"), &tools_content)
            .map_err(|e| format!("failed to write TOOLS.md: {}", e))?;

        // Write empty HEARTBEAT.md for new persona
        std::fs::write(dir.join("HEARTBEAT.md"), "")
            .map_err(|e| format!("failed to write HEARTBEAT.md: {}", e))?;

        let display_name = fm.as_ref().map(|f| f.name.as_str()).unwrap_or(id);
        Ok(Some(serde_json::json!({
            "downloaded": true,
            "name": display_name,
            "id": id,
        })))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
