//! Persona command — manage AI personas (list, search, install, activate, remove).

use anyhow::Result;

#[derive(clap::Subcommand)]
pub enum PersonaAction {
    /// List installed personas
    List,
    /// Search persona marketplace
    Search {
        /// Search query (optional - shows all if omitted)
        query: Option<String>,
    },
    /// Install a persona from marketplace
    Install {
        /// Persona ID (e.g. "engineering-code-reviewer")
        id: String,
    },
    /// Activate a persona
    Activate {
        /// Persona directory name
        name: String,
    },
    /// Remove an installed persona
    Remove {
        /// Persona directory name
        name: String,
    },
    /// Show current active persona
    Current,
    /// Restore default persona
    Restore,
}

pub async fn run(action: PersonaAction, _home: &str, workspace: &str) -> Result<()> {
    let ws = std::path::Path::new(workspace);
    match action {
        PersonaAction::List => cmd_list(ws),
        PersonaAction::Search { query } => cmd_search(ws, query.as_deref()),
        PersonaAction::Install { id } => cmd_install(ws, &id),
        PersonaAction::Activate { name } => cmd_activate(ws, &name),
        PersonaAction::Remove { name } => cmd_remove(ws, &name),
        PersonaAction::Current => cmd_current(ws),
        PersonaAction::Restore => cmd_restore(ws),
    }
}

// ---------------------------------------------------------------------------
// Local commands (synchronous)
// ---------------------------------------------------------------------------

fn cmd_current(workspace: &std::path::Path) -> Result<()> {

    // Read active persona
    let active_path = workspace.join("personas/_active.json");
    if !active_path.exists() {
        println!("No persona initialized yet.");
        return Ok(());
    }
    let content = std::fs::read_to_string(&active_path)?;
    let v: serde_json::Value = serde_json::from_str(&content)?;
    let active = v["name"].as_str().unwrap_or("default");

    // Read persona info
    let persona_path = workspace.join(format!("personas/{}/PERSONA.json", active));
    if persona_path.exists() {
        let pc = std::fs::read_to_string(&persona_path)?;
        let pv: serde_json::Value = serde_json::from_str(&pc)?;
        println!(
            "当前人格: {} {} ({})",
            pv["emoji"].as_str().unwrap_or(""),
            pv["name"].as_str().unwrap_or(active),
            active,
        );
        if let Some(desc) = pv["description"].as_str() {
            if !desc.is_empty() {
                println!("  描述: {}", desc);
            }
        }
    } else {
        println!("当前人格: {}", active);
    }

    Ok(())
}

fn cmd_list(workspace: &std::path::Path) -> Result<()> {
    let personas_dir = workspace.join("personas");

    if !personas_dir.exists() {
        println!("尚未安装任何人格。使用 'nemesisbot persona search' 浏览超市。");
        return Ok(());
    }

    // Read active
    let active = if personas_dir.join("_active.json").exists() {
        let c = std::fs::read_to_string(personas_dir.join("_active.json"))?;
        let v: serde_json::Value = serde_json::from_str(&c)?;
        v["name"].as_str().unwrap_or("default").to_string()
    } else {
        "default".to_string()
    };

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&personas_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let pj_path = path.join("PERSONA.json");
        let (name, emoji) = if pj_path.exists() {
            let pc = std::fs::read_to_string(&pj_path)?;
            let pv: serde_json::Value = serde_json::from_str(&pc)?;
            (
                pv["name"].as_str().unwrap_or(&dir_name).to_string(),
                pv["emoji"].as_str().unwrap_or("").to_string(),
            )
        } else {
            (dir_name.clone(), String::new())
        };
        let is_active = dir_name == active;
        let is_default = dir_name == "default";
        entries.push((dir_name, name, emoji, is_active, is_default));
    }

    // Sort: default first, then alphabetically
    entries.sort_by(|a, b| {
        if a.4 { return std::cmp::Ordering::Less; }
        if b.4 { return std::cmp::Ordering::Greater; }
        a.1.cmp(&b.1)
    });

    if entries.is_empty() {
        println!("尚未安装任何人格。");
        return Ok(());
    }

    println!("已安装人格 ({} 个):", entries.len());
    println!();
    for (dir, name, emoji, is_active, is_default) in &entries {
        let active_marker = if *is_active { " [使用中]" } else { "" };
        let default_marker = if *is_default { " [默认]" } else { "" };
        println!("  {} {} ({}){}{}", emoji, name, dir, active_marker, default_marker);
    }

    Ok(())
}

fn cmd_activate(workspace: &std::path::Path, name: &str) -> Result<()> {
    let src_dir = workspace.join(format!("personas/{}", name));
    if !src_dir.exists() {
        anyhow::bail!("人格 '{}' 不存在。使用 'nemesisbot persona list' 查看已安装的人格。", name);
    }

    // Copy persona files to workspace root
    let persona_files = ["IDENTITY.md", "SOUL.md", "AGENT.md", "TOOLS.md"];
    for file in &persona_files {
        let src = src_dir.join(file);
        if src.exists() {
            let content = std::fs::read_to_string(&src)?;
            std::fs::write(workspace.join(file), &content)?;
        }
    }

    // Update active marker
    let personas_dir = workspace.join("personas");
    std::fs::create_dir_all(&personas_dir)?;
    let active = serde_json::json!({"name": name});
    std::fs::write(
        personas_dir.join("_active.json"),
        serde_json::to_string_pretty(&active)?,
    )?;

    println!("已激活人格: {}", name);
    Ok(())
}

fn cmd_remove(workspace: &std::path::Path, name: &str) -> Result<()> {
    if name == "default" {
        anyhow::bail!("不能删除默认人格。");
    }
    let dir = workspace.join(format!("personas/{}", name));
    if !dir.exists() {
        anyhow::bail!("人格 '{}' 不存在。", name);
    }

    // If this is the active persona, restore default first
    let active_path = workspace.join("personas/_active.json");
    if active_path.exists() {
        let c = std::fs::read_to_string(&active_path)?;
        let v: serde_json::Value = serde_json::from_str(&c)?;
        if v["name"].as_str() == Some(name) {
            // Restore default
            let default_dir = workspace.join("personas/default");
            if default_dir.exists() {
                let persona_files = ["IDENTITY.md", "SOUL.md", "AGENT.md", "TOOLS.md"];
                for file in &persona_files {
                    let src = default_dir.join(file);
                    if src.exists() {
                        let content = std::fs::read_to_string(&src)?;
                        std::fs::write(workspace.join(file), &content)?;
                    }
                }
                let active = serde_json::json!({"name": "default"});
                std::fs::write(&active_path, serde_json::to_string_pretty(&active)?)?;
            }
        }
    }

    std::fs::remove_dir_all(&dir)?;
    println!("已删除人格: {}", name);
    Ok(())
}

fn cmd_restore(workspace: &std::path::Path) -> Result<()> {
    cmd_activate(workspace, "default")
}

// ---------------------------------------------------------------------------
// Remote commands (async, GitHub API)
// ---------------------------------------------------------------------------

fn cmd_search(workspace: &std::path::Path, query: Option<&str>) -> Result<()> {
    let query = query.unwrap_or("");
    println!("正在搜索人格超市...");

    let rt = tokio::runtime::Handle::current();
    let ws = workspace.to_path_buf();
    let result = tokio::task::block_in_place(|| {
        rt.block_on(async { search_personas(query, &ws).await })
    })?;

    if result.is_empty() {
        if query.is_empty() {
            println!("没有找到可用的人格。");
        } else {
            println!("搜索 '{}' 没有找到匹配的人格。", query);
        }
        return Ok(());
    }

    println!("找到 {} 个人格:", result.len());
    println!();
    for (id, name, emoji, division, installed) in &result {
        let installed_marker = if *installed { " [已安装]" } else { "" };
        println!("  {} {} ({}) - {}{}", emoji, name, id, division, installed_marker);
    }

    if !query.is_empty() {
        println!("\n使用 'nemesisbot persona install <id>' 安装人格。");
    }

    Ok(())
}

fn cmd_install(workspace: &std::path::Path, id: &str) -> Result<()> {
    // Check not already installed
    let persona_dir = workspace.join(format!("personas/{}", id));
    if persona_dir.exists() {
        anyhow::bail!("人格 '{}' 已经安装。使用 'nemesisbot persona activate {}' 激活。", id, id);
    }

    println!("正在下载人格 '{}'...", id);

    let ws_str = workspace.to_string_lossy().to_string();
    let rt = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| {
        rt.block_on(async { fetch_and_convert(&ws_str, id).await })
    })??;

    println!("已安装人格: {}。", id);
    println!("使用 'nemesisbot persona activate {}' 激活。", id);

    Ok(())
}

// ---------------------------------------------------------------------------
// GitHub API helpers (reuse persona handler logic)
// ---------------------------------------------------------------------------

async fn search_personas(query: &str, workspace: &std::path::Path) -> Result<Vec<(String, String, String, String, bool)>> {
    // Fetch tree
    let url = format!(
        "https://api.github.com/repos/msitarzewski/agency-agents/git/trees/main?recursive=1"
    );
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0")
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("GitHub API error: {}", resp.status());
    }
    let json: serde_json::Value = resp.json().await?;

    let mut results = Vec::new();
    if let Some(tree) = json["tree"].as_array() {
        for item in tree {
            let path = item["path"].as_str().unwrap_or("");
            if !is_agent_file_simple(path) {
                continue;
            }
            let parts: Vec<&str> = path.trim_end_matches(".md").split('/').collect();
            let id = parts.last().unwrap_or(&"").to_string();
            let cat_dir = parts.first().unwrap_or(&"").to_string();
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
            let category = map_category_simple(&cat_dir);

            let query_lower = query.to_lowercase();
            if !query_lower.is_empty() {
                let haystack = format!("{} {} {}", name, category, id).to_lowercase();
                if !haystack.contains(&query_lower) {
                    continue;
                }
            }

            let installed = workspace.join(format!("personas/{}", id)).exists();
            results.push((id, name, "🤖".to_string(), category.to_string(), installed));
        }
    }

    Ok(results)
}

async fn fetch_and_convert(workspace: &str, id: &str) -> Result<Result<()>> {
    // Find path from tree
    let url = format!(
        "https://api.github.com/repos/msitarzewski/agency-agents/git/trees/main?recursive=1"
    );
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0")
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Ok(Err(anyhow::anyhow!("GitHub API error: {}", resp.status())));
    }
    let json: serde_json::Value = resp.json().await?;

    let mut agent_path = None;
    if let Some(tree) = json["tree"].as_array() {
        for item in tree {
            let p = item["path"].as_str().unwrap_or("");
            let filename = p.rsplit('/').next().unwrap_or("");
            if filename.trim_end_matches(".md") == id && p.ends_with(".md") && is_agent_file_simple(p) {
                agent_path = Some(p.to_string());
                break;
            }
        }
    }

    let path = match agent_path {
        Some(p) => p,
        None => return Ok(Err(anyhow::anyhow!("人格 '{}' 在仓库中未找到", id))),
    };

    // Fetch file content
    let content_url = format!(
        "https://api.github.com/repos/msitarzewski/agency-agents/contents/{}?ref=main",
        path
    );
    let resp = client.get(&content_url).send().await?;
    if !resp.status().is_success() {
        return Ok(Err(anyhow::anyhow!("下载失败: {}", resp.status())));
    }
    let cjson: serde_json::Value = resp.json().await?;
    let content_b64 = cjson["content"].as_str().ok_or_else(|| anyhow::anyhow!("missing content"))?;
    let cleaned: String = content_b64.chars().filter(|c| !c.is_whitespace()).collect();
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &cleaned)
        .map_err(|e| anyhow::anyhow!("base64 decode error: {}", e))?;
    let content = String::from_utf8(decoded)?;

    // Use handler's conversion logic (duplicated here for CLI independence)
    // Parse frontmatter
    let fm = parse_frontmatter_simple(&content);
    let (preamble, sections) = parse_sections_simple(&content);

    let name = fm.as_ref().map(|f| f.0.as_str()).unwrap_or(id);
    let emoji = fm.as_ref().map(|f| f.1.as_str()).unwrap_or("🤖");
    let description = fm.as_ref().map(|f| f.2.as_str()).unwrap_or("");
    let vibe = fm.as_ref().map(|f| f.3.as_str()).unwrap_or("");

    // Convert sections
    let identity_secs: Vec<_> = sections.iter().filter(|(t, _)| {
        let n = strip_emoji_simple(t);
        n.contains("identity") || n.contains("memory")
    }).collect();
    let soul_secs: Vec<_> = sections.iter().filter(|(t, _)| {
        let n = strip_emoji_simple(t);
        n.contains("critical rule") || n.contains("communication") || n.contains("checklist")
    }).collect();
    let agent_secs: Vec<_> = sections.iter().filter(|(t, _)| {
        let n = strip_emoji_simple(t);
        !(n.contains("identity") || n.contains("memory") || n.contains("critical rule")
          || n.contains("communication") || n.contains("checklist")
          || n.contains("tool") || n.contains("integration"))
    }).collect();
    let tools_secs: Vec<_> = sections.iter().filter(|(t, _)| {
        let n = strip_emoji_simple(t);
        n.contains("tool") || n.contains("integration")
    }).collect();

    // Build files
    let mut identity = format!(
        "# {}\n\n{}{}\n\n## 基本信息\n\n- 姓名：{}\n- Emoji：{}\n- 风格：{}\n- 描述：{}\n",
        name,
        if vibe.is_empty() { String::new() } else { format!("{}\n\n", vibe) },
        if description.is_empty() { String::new() } else { format!("> {}\n\n", description) },
        name, emoji, vibe, description,
    );
    if !preamble.is_empty() {
        identity.push_str(&format!("\n---\n\n{}\n", preamble));
    }
    let mut soul = format!("# {} — 行为规则\n\n", name);
    for (title, content) in &soul_secs {
        soul.push_str(&format!("## {}\n\n{}\n\n", title, content));
    }
    let mut agent_extra = format!("\n---\n\n# {} 专用工作流\n", name);
    for (title, content) in &agent_secs {
        agent_extra.push_str(&format!("\n## {}\n\n{}\n", title, content));
    }
    let tools_extra = if tools_secs.is_empty() {
        String::new()
    } else {
        let mut extra = format!("\n---\n\n# {} 工具使用\n", name);
        for (title, content) in &tools_secs {
            extra.push_str(&format!("\n## {}\n\n{}\n", title, content));
        }
        extra
    };

    // Write to disk
    let dir = std::path::Path::new(workspace).join(format!("personas/{}", id));
    std::fs::create_dir_all(&dir)?;

    let persona_json = serde_json::json!({"name": name, "emoji": emoji, "description": description});
    std::fs::write(dir.join("PERSONA.json"), serde_json::to_string_pretty(&persona_json)?)?;

    // Add identity sections
    let mut identity_full = identity;
    for (title, content) in &identity_secs {
        identity_full.push_str(&format!("\n---\n\n## {}\n\n{}\n", title, content));
    }
    std::fs::write(dir.join("IDENTITY.md"), &identity_full)?;
    std::fs::write(dir.join("SOUL.md"), &soul)?;

    // AGENT.md = system default + extra
    let default_agent = include_str!("../../workspace/AGENT.md");
    std::fs::write(dir.join("AGENT.md"), format!("{}\n{}", default_agent, agent_extra))?;

    // TOOLS.md = system default + extra
    let default_tools = include_str!("../../workspace/TOOLS.md");
    let tools_content = if tools_extra.is_empty() {
        default_tools.to_string()
    } else {
        format!("{}\n{}", default_tools, tools_extra)
    };
    std::fs::write(dir.join("TOOLS.md"), &tools_content)?;

    Ok(Ok(()))
}

// Simple standalone parsing helpers for CLI (avoids dependency on handler)

fn is_agent_file_simple(path: &str) -> bool {
    if !path.ends_with(".md") || !path.contains('/') { return false; }
    let first_dir = path.split('/').next().unwrap_or("");
    if ["scripts", "examples", "integrations", "strategy", ".github"].contains(&first_dir) { return false; }
    let filename = path.rsplit('/').next().unwrap_or("");
    if ["README.md", "CONTRIBUTING.md", "LICENSE.md", "SECURITY.md", "CONTRIBUTING_zh-CN.md"].contains(&filename) { return false; }
    if filename.starts_with("QUICKSTART") || filename.starts_with("EXECUTIVE") { return false; }
    true
}

fn map_category_simple(dir: &str) -> &'static str {
    match dir {
        "engineering" => "开发", "marketing" => "营销", "security" => "安全",
        "design" => "创意", "academic" => "学术", "product" => "产品",
        "paid-media" => "付费媒体", "sales" => "销售",
        "game-development" => "游戏开发", "finance" => "金融",
        "gis" => "GIS", "spatial-computing" => "空间计算",
        "specialized" => "专业", "testing" => "测试",
        "support" => "客服", "project-management" => "项目管理",
        _ => "通用",
    }
}

fn strip_emoji_simple(s: &str) -> String {
    s.chars().filter(|c| !matches!(c,
        '\u{1F300}'..='\u{1F9FF}' | '\u{2600}'..='\u{26FF}' |
        '\u{2700}'..='\u{27BF}' | '\u{FE00}'..='\u{FE0F}' |
        '\u{1F000}'..='\u{1FFFF}' | '\u{200D}'
    )).collect::<String>().trim().to_lowercase()
}

fn parse_frontmatter_simple(content: &str) -> Option<(String, String, String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") { return None; }
    let after = &trimmed[3..];
    let end = after.find("---")?;
    let yaml = &after[..end];
    let mut name = String::new(); let mut emoji = String::new();
    let mut desc = String::new(); let mut vibe = String::new();
    for line in yaml.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("name:") { name = v.trim().trim_matches('"').to_string(); }
        else if let Some(v) = l.strip_prefix("emoji:") { emoji = v.trim().trim_matches('"').to_string(); }
        else if let Some(v) = l.strip_prefix("description:") { desc = v.trim().trim_matches('"').to_string(); }
        else if let Some(v) = l.strip_prefix("vibe:") { vibe = v.trim().trim_matches('"').to_string(); }
    }
    if name.is_empty() { return None; }
    Some((name, emoji, desc, vibe))
}

fn parse_sections_simple(content: &str) -> (String, Vec<(String, String)>) {
    let body = if content.trim_start().starts_with("---") {
        let after = &content.trim_start()[3..];
        if let Some(end) = after.find("---") { &after[end + 3..] } else { content }
    } else { content };

    let mut preamble_lines: Vec<String> = Vec::new();
    let mut sections = Vec::new();
    let mut title = String::new();
    let mut lines = Vec::new();
    let mut found = false;

    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("## ") && !t.starts_with("### ") {
            if found { sections.push((title.clone(), lines.join("\n").trim().to_string())); lines.clear(); }
            found = true;
            title = t[3..].trim().to_string();
        } else if found {
            lines.push(line.to_string());
        } else {
            let is_h1 = t.starts_with("# ") && !t.starts_with("## ");
            if !is_h1 {
                preamble_lines.push(line.to_string());
            }
        }
    }
    if found && !title.is_empty() {
        sections.push((title, lines.join("\n").trim().to_string()));
    }
    (preamble_lines.join("\n").trim().to_string(), sections)
}
