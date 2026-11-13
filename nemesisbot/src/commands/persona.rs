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

/// persona id 路径安全闸——复用 技能 slug 验证器单一真相源（拒路径
/// 分隔符/`..`/`:`/Windows 保留名/尾点尾空格）。CLI install/remove 对
/// `personas/{id}` 是裸 `join`（web 面同路径有 `resolve_path` 兜底，CLI
/// 此前无任何闸）：远端树里合法存在的 `..\x.md` 文件名（git/Linux 允许
/// `\`）经 `is_agent_file` 放行（只查 `.md`/含 `/`/跳过表），会让
/// create_dir_all + 六个固定名文件写出工作区。必须在触碰文件系统与网络
/// 之前拒绝。
fn validate_persona_id(id: &str) -> Result<()> {
    nemesis_skills::types::validate_skill_identifier(id)
        .map_err(|e| anyhow::anyhow!("人格 id '{}' 非法：{}", id, e))
}

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
        if let Some(desc) = pv["description"].as_str()
            && !desc.is_empty()
        {
            println!("  描述: {}", desc);
        }
    } else {
        println!("当前人格: {}", active);
    }

    Ok(())
}

fn cmd_list(workspace: &std::path::Path) -> Result<()> {
    // §4.4 快照降级面注记：list 列的是**本地已装**人格（读 workspace/personas，
    // 零网络）——上游不可达降级臂只属于拉取型命令（search/install），本命令
    // 无上游依赖，快照注记不适用。
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
        let dir_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
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
        if a.4 {
            return std::cmp::Ordering::Less;
        }
        if b.4 {
            return std::cmp::Ordering::Greater;
        }
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
        println!(
            "  {} {} ({}){}{}",
            emoji, name, dir, active_marker, default_marker
        );
    }

    Ok(())
}

fn cmd_activate(workspace: &std::path::Path, name: &str) -> Result<()> {
    // slug 闸与 install/remove 对齐（activate 此前漏闸，
    // 空串/路径形态 name 命中 personas 目录本身——激活生命周期清空工作区
    // 人格文件后把激活标记写成非法名）。
    validate_persona_id(name)?;
    // 与 web 面同源的完整生命周期（归档当前人格 → 清理根 → 恢复目标 →
    // 原子写标记； 前 CLI 简化版只拷 4 个.md 不归档，
    // 运行期 memory 滞留根目录，下次 web 切换时被归档进错误人格目录）。
    nemesis_web::handlers::persona::activate_persona_workspace(&workspace.to_string_lossy(), name)
        .map_err(anyhow::Error::msg)?;

    println!("已激活人格: {}", name);
    Ok(())
}

fn cmd_remove(workspace: &std::path::Path, name: &str) -> Result<()> {
    if name == "default" {
        anyhow::bail!("不能删除默认人格。");
    }
    validate_persona_id(name)?;
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
            // 与 web 面同源的完整恢复（归档/清理/恢复 default + 原子写标记）。
            // fail-closed：default 目录缺失时诚实报错中止，不静默删出悬空
            // 激活标记（前 CLI 行为，见 wave_b 测试）。
            nemesis_web::handlers::persona::activate_persona_workspace(
                &workspace.to_string_lossy(),
                "default",
            )
            .map_err(anyhow::Error::msg)?;
        }
    }

    // 韧性删除：用户点名的 remove 操作，瞬时句柄单发会假失败报错。
    nemesis_utils::fsutil::remove_dir_all_retry(&dir)?;
    println!("已删除人格: {}", name);
    Ok(())
}

fn cmd_restore(workspace: &std::path::Path) -> Result<()> {
    cmd_activate(workspace, "default")
}

// ---------------------------------------------------------------------------
// Remote commands (async, GitHub API)
// ---------------------------------------------------------------------------

/// 快照插件按需装载 + 诚实注记（幂等；缺席 = 静默走纯在线；名册开关关 =
/// 用户意志，注记一行；被拒 = stderr 留痕——headless CLI 无 tracing 面板，
/// 用户需要知道为什么没有快照兜底）。
fn ensure_snapshot_plugin(workspace: &std::path::Path) {
    use crate::resources_plugin_loader::ResourcesLoadOutcome;
    let config_dir = nemesis_path::workspace_config_dir(workspace);
    match crate::resources_plugin_loader::load_resources_plugin(&config_dir) {
        ResourcesLoadOutcome::Rejected(reason) => {
            eprintln!("[plugin-resources] 快照插件装载被拒（{reason}）——超市将走纯在线");
        }
        ResourcesLoadOutcome::DisabledBySwitch => {
            eprintln!("[plugin-resources] 快照插件已被本地插件库开关停用——超市将走纯在线");
        }
        _ => {}
    }
}

fn cmd_search(workspace: &std::path::Path, query: Option<&str>) -> Result<()> {
    let query = query.unwrap_or("");
    println!("正在搜索人格超市...");
    // 快照插件按需装载（缺席/拒绝/开关关 = 纯在线，装载结果不影响流程）。
    ensure_snapshot_plugin(workspace);

    let rt = tokio::runtime::Handle::current();
    let ws = workspace.to_path_buf();
    let result =
        tokio::task::block_in_place(|| rt.block_on(async { search_personas(query, &ws).await }))?;

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
        println!(
            "  {} {} ({}) - {}{}",
            emoji, name, id, division, installed_marker
        );
    }

    if !query.is_empty() {
        println!("\n使用 'nemesisbot persona install <id>' 安装人格。");
    }

    Ok(())
}

fn cmd_install(workspace: &std::path::Path, id: &str) -> Result<()> {
    // slug 闸先行（对齐 skills installer「校验必须在任何网络语句之前」）。
    validate_persona_id(id)?;

    // Check not already installed
    let persona_dir = workspace.join(format!("personas/{}", id));
    if persona_dir.exists() {
        anyhow::bail!(
            "人格 '{}' 已经安装。使用 'nemesisbot persona activate {}' 激活。",
            id,
            id
        );
    }

    println!("正在下载人格 '{}'...", id);
    // 快照插件按需装载（上游不可达时 install 走内置快照兜底）。
    ensure_snapshot_plugin(workspace);

    let ws_str = workspace.to_string_lossy().to_string();
    let rt = tokio::runtime::Handle::current();
    tokio::task::block_in_place(|| rt.block_on(async { fetch_and_convert(&ws_str, id).await }))?;

    println!("已安装人格: {}。", id);
    println!("使用 'nemesisbot persona activate {}' 激活。", id);

    Ok(())
}

// ---------------------------------------------------------------------------
// GitHub API helpers (reuse persona handler logic)
// ---------------------------------------------------------------------------

/// 路径 → 搜索结果行（在线/快照两臂共用：id/名/类别推导 + query 过滤 +
/// installed 探测）。
fn path_to_result(
    path: &str,
    query: &str,
    workspace: &std::path::Path,
) -> Option<(String, String, String, String, bool)> {
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
    let category = nemesis_web::handlers::persona::map_category(&cat_dir);

    let query_lower = query.to_lowercase();
    if !query_lower.is_empty() {
        let haystack = format!("{} {} {}", name, category, id).to_lowercase();
        if !haystack.contains(&query_lower) {
            return None;
        }
    }

    let installed = workspace.join(format!("personas/{}", id)).exists();
    Some((id, name, "🤖".to_string(), category.to_string(), installed))
}

/// 路径清单 → 搜索结果行（快照臂与在线臂的 json 解包分叉点之后共用）。
fn paths_to_results(
    paths: &[String],
    query: &str,
    workspace: &std::path::Path,
) -> Vec<(String, String, String, String, bool)> {
    paths
        .iter()
        .filter_map(|p| path_to_result(p, query, workspace))
        .collect()
}

/// 快照树（插件缺席 / manifest 无该 kind / 过滤后空 = None → 纯在线）。
/// 语义过滤与 web 面同源（is_agent_file 单一真相源）。
fn snapshot_persona_tree() -> Option<(Vec<String>, nemesis_plugin::resources::ResourceEntry)> {
    nemesis_plugin::resources::snapshot_tree_filtered(
        nemesis_plugin::resources::RESOURCES_KIND_PERSONA_CATALOG,
        nemesis_web::handlers::persona::is_agent_file,
    )
}

/// 在线臂故障注入钩子（cfg(test) 专供：2 = 强制上游失败，快照降级臂离线
/// 回归用；生产恒 0 走真实网络。web 面同款 TEST_REFRESH_MODE 先例）。
#[cfg(test)]
static TEST_ONLINE_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

async fn search_personas(
    query: &str,
    workspace: &std::path::Path,
) -> Result<Vec<(String, String, String, String, bool)>> {
    match search_personas_online(query, workspace).await {
        Ok(results) => Ok(results),
        Err(e) => {
            let Some((paths, entry)) = snapshot_persona_tree() else {
                return Err(e);
            };
            let class = nemesis_web::handlers::persona::classify_github_error(&e.to_string());
            eprintln!(
                "GitHub 上游不可达（{}），搜索结果来自内置快照（{}）",
                class,
                entry.short_label()
            );
            Ok(paths_to_results(&paths, query, workspace))
        }
    }
}

async fn search_personas_online(
    query: &str,
    workspace: &std::path::Path,
) -> Result<Vec<(String, String, String, String, bool)>> {
    // Fetch tree
    let url = "https://api.github.com/repos/msitarzewski/agency-agents/git/trees/main?recursive=1"
        .to_string();
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
            if !nemesis_web::handlers::persona::is_agent_file(path) {
                continue;
            }
            if let Some(row) = path_to_result(path, query, workspace) {
                results.push(row);
            }
        }
    }

    Ok(results)
}

/// 内容 → 转换 → 落盘（在线/快照两臂共用；转换与 web shop.download 同源：
/// convert_agent_content 的 AGENT/TOOLS 拼系统模板、PERSONA.json 含扩展字段
/// ——前 CLI 简化转换缺字段，web 语义消费方读到空值）。
fn write_converted_persona(workspace: &str, id: &str, content: &str) -> Result<()> {
    let converted = nemesis_web::handlers::persona::convert_agent_content(content);

    // Write to disk
    let dir = std::path::Path::new(workspace).join(format!("personas/{}", id));
    std::fs::create_dir_all(&dir)?;

    std::fs::write(dir.join("PERSONA.json"), &converted.persona_json)?;
    std::fs::write(dir.join("IDENTITY.md"), &converted.identity)?;
    std::fs::write(dir.join("SOUL.md"), &converted.soul)?;
    std::fs::write(dir.join("AGENT.md"), &converted.agent_full)?;
    std::fs::write(dir.join("TOOLS.md"), &converted.tools_full)?;

    // 空心跳文件与 web shop.download 对齐：缺失/空 HEARTBEAT.md 会让心跳
    // 服务跳过 LLM，之后 web activate 本人格时根 HEARTBEAT.md 被清掉不补。
    std::fs::write(dir.join("HEARTBEAT.md"), "")?;

    Ok(())
}

/// install 在线臂 + 快照降级（拍板 2026-11-12：GitHub 上游错误分类只影响
/// stderr 措辞，不分支降级行为——有快照且快照含该 id 时**全部错误类别一律
/// 降级**，快照无该 id 或无快照时维持原始错误）。生产入口。
async fn fetch_and_convert(workspace: &str, id: &str) -> Result<()> {
    fetch_and_convert_with(
        workspace,
        id,
        snapshot_persona_tree().map(|(paths, entry)| {
            (paths, entry, |p: &str| {
                nemesis_plugin::resources::snapshot_content(
                    nemesis_plugin::resources::RESOURCES_KIND_PERSONA_CATALOG,
                    p,
                )
            })
        }),
    )
    .await
}

/// 快照源参数化的降级臂本体。在线臂所有失败路径（HTTP 状态/网络/内容异常/
/// 未找到）统一 `Err` 上抛——前形态 `Ok(Err(..))` 内层 Result 会被
/// `Ok(r) => Ok(r)` 透传**绕过快照臂**（BUG-20261113-146 根因：GitHub 未认
/// 证限流 403 这类最常见不可达形态下 install 无快照兜底），已坍缩为单层
/// `Result<()>` 杜绝再犯。
///
/// 快照三元组走参数而非 BRIDGE 全局注入：BRIDGE 是进程级 OnceLock，测试
/// 注入会永久污染同 binary 的 r10 系列（其前提 = 无快照插件 → 死上游必须
/// Err），桥不可作为测试 seam。
async fn fetch_and_convert_with<C>(
    workspace: &str,
    id: &str,
    snapshot: Option<(Vec<String>, nemesis_plugin::resources::ResourceEntry, C)>,
) -> Result<()>
where
    C: Fn(&str) -> Result<String, String>,
{
    match fetch_and_convert_online(workspace, id).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let Some((paths, entry, content_of)) = snapshot else {
                return Err(e);
            };
            // 匹配口径与 web 面同源（stem == id || filename == id，且 .md +
            // is_agent_file——在线臂同款四条件）。
            let path = paths.iter().find(|p| {
                let filename = p.rsplit('/').next().unwrap_or("");
                (filename.trim_end_matches(".md") == id || filename == id)
                    && p.ends_with(".md")
                    && nemesis_web::handlers::persona::is_agent_file(p)
            });
            let Some(path) = path else {
                // 快照里也没有该 id（快照较旧或 id 真缺失）→ 维持原始错误。
                return Err(e);
            };
            let class = nemesis_web::handlers::persona::classify_github_error(&e.to_string());
            eprintln!(
                "GitHub 上游不可达（{}），'{}' 改读内置快照（{}）",
                class,
                id,
                entry.short_label()
            );
            let content = content_of(path).map_err(anyhow::Error::msg)?;
            write_converted_persona(workspace, id, &content)?;
            Ok(())
        }
    }
}

async fn fetch_and_convert_online(workspace: &str, id: &str) -> Result<()> {
    // 测试注入点：强制上游失败（快照降级臂离线回归；生产恒 0 不可达）。
    #[cfg(test)]
    if TEST_ONLINE_MODE.load(std::sync::atomic::Ordering::SeqCst) == 2 {
        anyhow::bail!("GitHub API error: 403 Forbidden");
    }

    // Find path from tree
    let url = "https://api.github.com/repos/msitarzewski/agency-agents/git/trees/main?recursive=1"
        .to_string();
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0")
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("GitHub API error: {}", resp.status());
    }
    let json: serde_json::Value = resp.json().await?;

    let mut agent_path = None;
    if let Some(tree) = json["tree"].as_array() {
        for item in tree {
            let p = item["path"].as_str().unwrap_or("");
            let filename = p.rsplit('/').next().unwrap_or("");
            // 匹配口径与 web 面同源（`stem == id || filename == id`，
            // 前 CLI 漏第二臂）。
            if (filename.trim_end_matches(".md") == id || filename == id)
                && p.ends_with(".md")
                && nemesis_web::handlers::persona::is_agent_file(p)
            {
                agent_path = Some(p.to_string());
                break;
            }
        }
    }

    let path = match agent_path {
        Some(p) => p,
        None => anyhow::bail!("人格 '{}' 在仓库中未找到", id),
    };

    // Fetch file content
    let content_url = format!(
        "https://api.github.com/repos/msitarzewski/agency-agents/contents/{}?ref=main",
        path
    );
    let resp = client.get(&content_url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("下载失败: {}", resp.status());
    }
    let cjson: serde_json::Value = resp.json().await?;
    let content_b64 = cjson["content"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing content"))?;
    let cleaned: String = content_b64.chars().filter(|c| !c.is_whitespace()).collect();
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &cleaned)
        .map_err(|e| anyhow::anyhow!("base64 decode error: {}", e))?;
    let content = String::from_utf8(decoded)?;

    write_converted_persona(workspace, id, &content)?;

    Ok(())
}

#[cfg(test)]
mod tests;
