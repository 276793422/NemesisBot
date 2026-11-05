// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

// 跨子文件引用（机械拆分）：extract_name_arg 定义在 web.rs
use super::web::extract_name_arg;

/// Fetch a memory tool's canonical definition from nemesis-memory.
///
/// Single source of truth for the agent-facing memory tool schemas: the
/// wrappers' `parameters()`/`description()` below delegate to
/// [`memory_tool_definitions`] so they can never drift from what
/// `MemoryToolExecutor` actually consumes. (e2e found exactly that
/// drift: the hand-copied `memory_store` schema demanded `key` +
/// tags-as-string while the executor reads `memory_type`/`content` +
/// tags-as-array — args_validator then rejected executor-valid calls.)
///
/// [`memory_tool_definitions`]: nemesis_memory::memory_tools:memory_tool_definitions
#[cfg(feature = "memory")]
fn memory_tool_def(name: &str) -> nemesis_memory::memory_tools::MemoryTool {
    nemesis_memory::memory_tools::memory_tool_definitions()
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("memory_tool_definitions must cover `{name}`"))
}

/// Memory search tool for searching conversation memory.
///
/// Delegates to `nemesis_memory::memory_tools:MemoryToolExecutor` for the
/// actual search. If no memory manager is configured, returns an error.
#[cfg(feature = "memory")]
pub struct MemorySearchTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

#[cfg(feature = "memory")]
impl MemorySearchTool {
    /// Create a new memory search tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[cfg(feature = "memory")]
#[async_trait]
impl Tool for MemorySearchTool {
    fn description(&self) -> String {
        memory_tool_def("memory_search").description
    }

    fn parameters(&self) -> serde_json::Value {
        memory_tool_def("memory_search").parameters
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let result = executor.execute("memory_search", &val).await;
        if result.success {
            Ok(result.content)
        } else {
            Err(result.content)
        }
    }
}

/// Memory store tool for storing information in long-term memory.
///
/// Delegates to `nemesis_memory::memory_tools:MemoryToolExecutor`.
#[cfg(feature = "memory")]
pub struct MemoryStoreTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

#[cfg(feature = "memory")]
impl MemoryStoreTool {
    /// Create a new memory store tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[cfg(feature = "memory")]
#[async_trait]
impl Tool for MemoryStoreTool {
    fn description(&self) -> String {
        memory_tool_def("memory_store").description
    }

    fn parameters(&self) -> serde_json::Value {
        memory_tool_def("memory_store").parameters
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let result = executor.execute("memory_store", &val).await;
        if result.success {
            Ok(result.content)
        } else {
            Err(result.content)
        }
    }
}

/// Memory forget tool for removing information from long-term memory.
///
/// Delegates to `nemesis_memory::memory_tools:MemoryToolExecutor`.
#[cfg(feature = "memory")]
pub struct MemoryForgetTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

#[cfg(feature = "memory")]
impl MemoryForgetTool {
    /// Create a new memory forget tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[cfg(feature = "memory")]
#[async_trait]
impl Tool for MemoryForgetTool {
    fn description(&self) -> String {
        memory_tool_def("memory_forget").description
    }

    fn parameters(&self) -> serde_json::Value {
        memory_tool_def("memory_forget").parameters
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let result = executor.execute("memory_forget", &val).await;
        if result.success {
            Ok(result.content)
        } else {
            Err(result.content)
        }
    }
}

/// Memory list tool for listing stored memories.
///
/// Delegates to `nemesis_memory::memory_tools:MemoryToolExecutor`.
#[cfg(feature = "memory")]
pub struct MemoryListTool {
    executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>,
}

#[cfg(feature = "memory")]
impl MemoryListTool {
    /// Create a new memory list tool backed by the given executor.
    pub fn new(executor: Option<Arc<nemesis_memory::memory_tools::MemoryToolExecutor>>) -> Self {
        Self { executor }
    }
}

#[cfg(feature = "memory")]
#[async_trait]
impl Tool for MemoryListTool {
    fn description(&self) -> String {
        memory_tool_def("memory_list").description
    }

    fn parameters(&self) -> serde_json::Value {
        memory_tool_def("memory_list").parameters
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let executor = match &self.executor {
            Some(e) => e,
            None => return Err("Memory store is not available".to_string()),
        };

        let val: serde_json::Value =
            serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}));

        let result = executor.execute("memory_list", &val).await;
        if result.success {
            Ok(result.content)
        } else {
            Err(result.content)
        }
    }
}

// ===========================================================================
// Skills tools
// ===========================================================================

/// Skills list tool for listing available local skills.
///
/// Mirrors Go's `SkillsListTool`. Uses a `SkillsLoader` to scan workspace,
/// global, and builtin skill directories. Falls back to a stub message when
/// no loader is configured.
pub struct SkillsListTool {
    loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
}

impl SkillsListTool {
    /// Create a new skills list tool.
    pub fn new(loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillsListTool {
    fn description(&self) -> String {
        "List available skills".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"category":{"type":"string","description":"Filter by category"}}})
    }

    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        match &self.loader {
            Some(loader) => {
                let skills = loader.list_skills();
                if skills.is_empty() {
                    return Ok(
                        "No skills installed. Use find_skills to search for available skills."
                            .to_string(),
                    );
                }

                let mut output = format!("Installed skills ({}):\n", skills.len());
                for (i, skill) in skills.iter().enumerate() {
                    output.push_str(&format!(
                        "\n{}. **{}** (source: {})",
                        i + 1,
                        skill.name,
                        skill.source
                    ));
                    if !skill.description.is_empty() {
                        output.push_str(&format!("\n   Description: {}", skill.description));
                    }
                    if let Some(score) = skill.lint_score {
                        output.push_str(&format!("\n   Security score: {:.0}/100", score * 100.0));
                    }
                    output.push('\n');
                }
                Ok(output)
            }
            None => Ok("[SkillsList] No skills loaded (skills loader not configured)".to_string()),
        }
    }
}

/// Skills info tool for getting detailed info about a specific skill.
///
/// Mirrors Go's `SkillsInfoTool`. Returns the full content of a skill's
/// SKILL.md file (with frontmatter stripped) when available.
pub struct SkillsInfoTool {
    loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
}

impl SkillsInfoTool {
    /// Create a new skills info tool.
    pub fn new(loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillsInfoTool {
    fn description(&self) -> String {
        "Get detailed information about a specific skill".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"name":{"type":"string","description":"Skill name"}},"required":["name"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let skill_name = extract_name_arg(args)?;

        match &self.loader {
            Some(loader) => {
                let skills = loader.list_skills();
                let skill = skills.iter().find(|s| s.name == skill_name);
                match skill {
                    Some(info) => {
                        let content = loader
                            .load_skill(&skill_name)
                            .unwrap_or_else(|| "(no content available)".to_string());
                        Ok(format!(
                            "Skill: **{}**\nSource: {}\nPath: {}\nDescription: {}\n\n{}",
                            info.name, info.source, info.path, info.description, content
                        ))
                    }
                    None => Err(format!(
                        "Skill '{}' not found. Use skills_list to see installed skills.",
                        skill_name
                    )),
                }
            }
            None => Ok(format!(
                "[SkillsInfo] Skill '{}' not found (skills loader not configured)",
                skill_name
            )),
        }
    }
}

/// Find skills tool - searches configured registries for available skills.
///
/// Mirrors Go's `FindSkillsTool`. Uses `RegistryManager` to search across
/// all configured registries (GitHub, ClawHub, etc.) concurrently.
pub struct FindSkillsTool {
    registry: Arc<nemesis_skills::registry::RegistryManager>,
}

impl FindSkillsTool {
    /// Create a new find skills tool.
    pub fn new(registry: Arc<nemesis_skills::registry::RegistryManager>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for FindSkillsTool {
    fn description(&self) -> String {
        "Search for skills in remote registries".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"query":{"type":"string","description":"Search query"},"limit":{"type":"integer","description":"Maximum results"}},"required":["query"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let query = val["query"].as_str().unwrap_or("");
        if query.trim().is_empty() {
            return Err("missing or empty 'query' parameter".to_string());
        }

        let limit = val["limit"].as_u64().unwrap_or(5).clamp(1, 50) as usize;

        let results = self
            .registry
            .search(query, limit)
            .await
            .map_err(|e| format!("failed to search registries: {}", e))?;

        if results.is_empty() {
            return Ok(format!("No skills found for query '{}'", query));
        }

        let mut output = format!("Found {} skill(s) for \"{}\":\n", results.len(), query);
        for (i, result) in results.iter().enumerate() {
            output.push_str(&format!("\n{}. **{}**", i + 1, result.slug));
            if !result.version.is_empty() {
                output.push_str(&format!(" v{}", result.version));
            }
            output.push_str(&format!(
                " (score: {:.2}, registry: {})\n",
                result.score, result.registry_name
            ));
            if !result.display_name.is_empty() {
                output.push_str(&format!("   Display Name: {}\n", result.display_name));
            }
            if !result.summary.is_empty() {
                output.push_str(&format!("   Description: {}\n", result.summary));
            }
            if result.downloads > 0 {
                output.push_str(&format!("   Downloads: {}\n", result.downloads));
            }
        }

        Ok(output)
    }
}

/// Install skill tool - installs a skill from a configured registry.
///
/// Mirrors Go's `InstallSkillTool`. Downloads and installs a skill from the
/// specified registry to the local workspace skills directory.
///
/// 执行面走 [`nemesis_skills::installer:SkillInstaller`]
/// 全漏斗（staging → 验签 → 安全扫描 → 版本龄 → 审批门
/// → 落位 + lockfile）。旧形态裸 `RegistryManager::install` 直落
/// skills/，绕过全部供应链闸——web/CLI 同操作均走漏斗，工具面是唯一旁路
/// （prompt-injected LLM 可装恶意技能 → 重启被 loader 加载 → 持久后门）。
pub struct InstallSkillTool {
    registry: Arc<nemesis_skills::registry::RegistryManager>,
    workspace: String,
    /// 审批门槽（gateway 晚填）。None 槽 / 槽内 None 门 = 无审批面直装。
    install_gate_slot: Option<SkillsInstallGateSlot>,
}

impl InstallSkillTool {
    /// Create a new install skill tool.
    pub fn new(
        registry: Arc<nemesis_skills::registry::RegistryManager>,
        workspace: String,
        install_gate_slot: Option<SkillsInstallGateSlot>,
    ) -> Self {
        Self {
            registry,
            workspace,
            install_gate_slot,
        }
    }
}

#[async_trait]
impl Tool for InstallSkillTool {
    fn description(&self) -> String {
        "Install a skill from a remote registry".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"name":{"type":"string","description":"Skill to install (registry/slug format)"}},"required":["name"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let slug = match val["name"].as_str().or_else(|| val["slug"].as_str()) {
            Some(s) if !s.is_empty() => s,
            _ => {
                return Err("slug parameter is required and must be a non-empty string".to_string());
            }
        };

        // Validate skill identifier (path traversal protection)
        nemesis_skills::types::validate_skill_identifier(slug)
            .map_err(|e| format!("invalid slug: {}", e))?;

        let registry_name = val["registry"].as_str().unwrap_or("github");
        let version = val["version"].as_str().unwrap_or("latest");
        let force = val["force"].as_bool().unwrap_or(false);

        // 漏斗：与 web install_with_ctx 同源走
        // SkillInstaller（staging → 验签 → 安全扫描 → 版本龄 →
        // 审批门 → 落位 + lockfile）。审批门经共享槽晚绑；/
        // 主配置消费点在 gateway 侧（槽同款），工具面无 home 语境按缺省
        // （allow_unsigned=true、age 关）——与改造前工具语义一致，非本轮收缩。
        let mut installer = nemesis_skills::installer::SkillInstaller::new(&self.workspace);
        installer.set_registry_manager_shared(self.registry.clone());
        if let Some(slot) = &self.install_gate_slot
            && let Some(gate) = slot.read().clone()
        {
            installer.set_install_gate(gate);
        }

        // Check if skill already exists locally (unless force)
        let skill_dir = std::path::Path::new(&self.workspace)
            .join("skills")
            .join(slug);
        if skill_dir.exists() {
            if !force {
                return Err(format!(
                    "skill '{}' already exists locally at {}. Use force=true to reinstall.",
                    slug,
                    skill_dir.display()
                ));
            }
            // force = 先卸旧再装。经 installer.uninstall：边界校验 +
            // lockfile 记账同步移除（与 web force 的.skill-rollback 事务化
            // 同族语义；卸失败诚实报错，旧技能不动）。
            installer
                .uninstall(slug)
                .map_err(|e| format!("failed to remove existing skill '{}': {}", slug, e))?;
        }

        // Install through the funnel.
        installer
            .install(registry_name, slug, version)
            .await
            .map_err(|e| format!("failed to install skill '{}': {}", slug, e))?;

        Ok(format!(
            "Skill '{}' installed successfully from registry '{}'",
            slug, registry_name
        ))
    }
}

// ===========================================================================
// Skill manage tool (agent-authored skills / procedural memory)
// ===========================================================================

/// Skill manage tool — the agent's procedural memory.
///
/// Lets the agent author reusable SKILL.md skills under `workspace/skills/<name>/`,
/// so a workflow it just learned (or distilled from reference material via the
/// `learn` skill) becomes a reusable slash command. Mirrors Hermes Agent's
/// `skill_manage`. Actions: `create` (full SKILL.md incl. frontmatter), `patch`/
/// `edit` (replace `old`->`new` text), `write_file`/`remove_file` (supporting
/// files inside the skill dir), `delete` (remove whole skill). All writes are
/// security-checked before landing.
/// Shareable slot holding an optional approval manager, filled in later by the
/// gateway (after the agent loop is built). Lets `skill_manage` request
/// interactive approval for writes when `skills.manage_approval` is enabled.
#[cfg(feature = "security")]
pub type ApprovalManagerSlot =
    Arc<parking_lot::RwLock<Option<Arc<dyn nemesis_security::auditor::ApprovalManager>>>>;
#[cfg(not(feature = "security"))]
/// Placeholder when the security feature is off — `skill_manage` auto-allows
/// writes (no approval middleware). All `Option<ApprovalManagerSlot>` fields
/// stay `None`-compatible via this `` alias.
pub type ApprovalManagerSlot = ();

/// `install_skill` 工具的装前审批门共享槽——gateway 在 agent
/// loop 建好后填 `LateWebSkillsGate`（approval_slot/question_slot 同款
/// 「先建槽后填」；重启重建的 loop 共享同一槽 Arc，晚填也可）。
/// 槽缺席（`None`）或槽内为空 = 无审批面直装（headless / 测试装配 /
/// `skills.install_approval=false` 显式关卡——三种形态同语义）；漏斗其余
/// 闸（staging / 验签 / 安全扫描 / lockfile）与门无关，恒生效。
pub type SkillsInstallGateSlot =
    Arc<parking_lot::RwLock<Option<nemesis_skills::install_gate::SharedInstallGate>>>;

#[cfg_attr(not(feature = "security"), allow(dead_code))]
pub struct SkillManageTool {
    workspace: String,
    /// Optional approval manager slot (filled later by the gateway). When
    /// `require_approval` is true and this slot is populated, writes prompt.
    approval_manager: Option<ApprovalManagerSlot>,
    /// Whether writes require interactive approval (config: skills.manage_approval).
    require_approval: bool,
}

impl SkillManageTool {
    /// Create a new skill manage tool writing under `<workspace>/skills/`.
    pub fn new(
        workspace: String,
        approval_manager: Option<ApprovalManagerSlot>,
        require_approval: bool,
    ) -> Self {
        Self {
            workspace,
            approval_manager,
            require_approval,
        }
    }

    fn skill_dir(&self, name: &str) -> PathBuf {
        Path::new(&self.workspace).join("skills").join(name)
    }

    /// Request interactive approval for a write action. No-op when approval is
    /// disabled. When enabled but no manager is running, refuse the write (safe
    /// default). Runs the blocking popup on a `spawn_blocking` thread.
    async fn check_approval(
        &self,
        operation: &str,
        target: &str,
        reason: &str,
    ) -> Result<(), String> {
        #[cfg(not(feature = "security"))]
        {
            // Security feature trimmed — no approval middleware; auto-allow skill writes.
            let _ = (operation, target, reason);
            // 尾表达式形式（非 `return Ok`）：not(security) 单臂编译时
            // 本块即函数尾，clippy::needless_return 会判 return 多余
            // （minimal-feature 形态 远端 clippy）。
            Ok(())
        }
        #[cfg(feature = "security")]
        {
            if !self.require_approval {
                return Ok(());
            }
            let slot = match &self.approval_manager {
                Some(s) => s.clone(),
                None => {
                    return Err(
                        "skill write requires approval but no approval manager is configured"
                            .to_string(),
                    );
                }
            };
            let am = {
                let guard = slot.read();
                match guard.as_ref() {
                    Some(m) if m.is_running() => m.clone(),
                    _ => {
                        return Err(
                            "skill write requires approval but no approval manager is running"
                                .to_string(),
                        );
                    }
                }
            };
            let req_id = format!("skill_manage:{}:{}", operation, target);
            let op = format!("skill_manage.{}", operation);
            let target = target.to_string();
            let reason = reason.to_string();
            match tokio::task::spawn_blocking(move || {
                am.request_approval_sync(&req_id, &op, &target, "MEDIUM", &reason, 30)
            })
            .await
            {
                Ok(Ok(v)) if v.approved => Ok(()),
                Ok(Ok(_)) => Err(format!("skill write '{}' denied by user", operation)),
                Ok(Err(e)) => Err(format!("approval request failed: {}", e)),
                Err(e) => Err(format!("approval task failed: {}", e)),
            }
        }
    }

    fn do_create(
        &self,
        skill_dir: &Path,
        name: &str,
        v: &serde_json::Value,
    ) -> Result<String, String> {
        let content = v["content"]
            .as_str()
            .ok_or_else(|| "'content' is required for create".to_string())?;
        let overwrite = v["overwrite"].as_bool().unwrap_or(false);
        let skill_md = skill_dir.join("SKILL.md");
        if skill_md.exists() && !overwrite {
            return Err(format!(
                "skill '{}' already exists at {}. Set overwrite=true to replace.",
                name,
                skill_md.display()
            ));
        }
        let check = nemesis_skills::security_check::check_skill_security(content, name, "");
        if check.blocked {
            return Err(format!(
                "skill content blocked by security check: {}",
                check.block_reason
            ));
        }
        std::fs::create_dir_all(skill_dir)
            .map_err(|e| format!("failed to create skill dir: {}", e))?;
        if let Err(e) = std::fs::write(&skill_md, content) {
            // 回滚清理走韧性包装（残留空目录无害，尽力而为）。
            nemesis_utils::fsutil::remove_dir_all_resilient(skill_dir);
            return Err(format!("failed to write SKILL.md: {}", e));
        }
        // 学得技能 manifest 记账（created/overwrite 共用 upsert）。记账失败
        // 不回滚 create——技能本体已落盘，降级为结果附注（账本可后补）。
        let manifest_note = match nemesis_skills::learned::record_learned(
            Path::new(&self.workspace),
            name,
            &nemesis_skills::learned::extract_description(content),
        ) {
            Ok(()) => String::new(),
            Err(e) => format!(
                "\n⚠ learned manifest 记账失败（技能已创建，账目可后补）: {}",
                e
            ),
        };
        Ok(format!(
            "Skill '{}' created at {} (lint {:.0}/100{}).{}",
            name,
            skill_md.display(),
            check.lint_result.score * 100.0,
            check
                .quality_score
                .as_ref()
                .map(|q| format!(", quality {:.0}/100", q.overall))
                .unwrap_or_default(),
            manifest_note
        ))
    }

    fn do_patch(
        &self,
        skill_dir: &Path,
        name: &str,
        v: &serde_json::Value,
    ) -> Result<String, String> {
        let old = v["old"].as_str().unwrap_or("");
        let new = v["new"]
            .as_str()
            .ok_or_else(|| "'new' is required for patch/edit".to_string())?;
        let skill_md = skill_dir.join("SKILL.md");
        let mut content = std::fs::read_to_string(&skill_md)
            .map_err(|e| format!("failed to read SKILL.md for '{}': {}", name, e))?;
        if old.is_empty() {
            content.push_str(new);
        } else if let Some(idx) = content.find(old) {
            content.replace_range(idx..idx + old.len(), new);
        } else {
            return Err(format!(
                "'old' text not found in SKILL.md for skill '{}'",
                name
            ));
        }
        let check = nemesis_skills::security_check::check_skill_security(&content, name, "");
        if check.blocked {
            return Err(format!(
                "edited content blocked by security check: {}",
                check.block_reason
            ));
        }
        std::fs::write(&skill_md, &content)
            .map_err(|e| format!("failed to write SKILL.md: {}", e))?;
        Ok(format!(
            "Skill '{}' updated (lint {:.0}/100).",
            name,
            check.lint_result.score * 100.0
        ))
    }

    fn do_write_file(
        &self,
        skill_dir: &Path,
        name: &str,
        v: &serde_json::Value,
    ) -> Result<String, String> {
        if !skill_dir.exists() {
            return Err(format!(
                "skill '{}' has no directory yet; create it first",
                name
            ));
        }
        let path = v["path"]
            .as_str()
            .ok_or_else(|| "'path' is required for write_file".to_string())?;
        let content = v["content"]
            .as_str()
            .ok_or_else(|| "'content' is required for write_file".to_string())?;
        let overwrite = v["overwrite"].as_bool().unwrap_or(false);
        let target = resolve_within(skill_dir, path)?;
        // 装前安全检查同闸：安装路径（目录形态）
        // 对可执行面（SURFACE_FILE_EXTENSIONS）整体 lint——设计原文「恶意
        // 载荷藏在辅助脚本里无法再绕过检查」；agent 自写路径（write_file）
        // 此前零检查，同一载荷即可绕过。扩展名判定与安装路径同源（单一
        // 真相源），数据文件（非 surface 扩展名）与安装路径同口径放行。
        if nemesis_skills::lint::is_surface_file(&target) {
            let check = nemesis_skills::security_check::check_skill_security(content, name, "");
            if check.blocked {
                return Err(format!(
                    "file content blocked by security check: {}",
                    check.block_reason
                ));
            }
        }
        if target.exists() && !overwrite {
            return Err(format!(
                "file already exists at {}. Set overwrite=true.",
                target.display()
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("failed to create dir: {}", e))?;
        }
        std::fs::write(&target, content).map_err(|e| format!("failed to write file: {}", e))?;
        Ok(format!(
            "Wrote {} ({} bytes) in skill '{}'.",
            target.display(),
            content.len(),
            name
        ))
    }

    fn do_remove_file(
        &self,
        skill_dir: &Path,
        name: &str,
        v: &serde_json::Value,
    ) -> Result<String, String> {
        if !skill_dir.exists() {
            return Err(format!("skill '{}' has no directory yet", name));
        }
        let path = v["path"]
            .as_str()
            .ok_or_else(|| "'path' is required for remove_file".to_string())?;
        let target = resolve_within(skill_dir, path)?;
        if !target.exists() {
            return Err(format!("file not found: {}", target.display()));
        }
        std::fs::remove_file(&target).map_err(|e| format!("failed to remove file: {}", e))?;
        Ok(format!(
            "Removed {} from skill '{}'.",
            target.display(),
            name
        ))
    }

    fn do_delete(&self, skill_dir: &Path, name: &str) -> Result<String, String> {
        if !skill_dir.exists() {
            return Err(format!(
                "skill '{}' not found at {}",
                name,
                skill_dir.display()
            ));
        }
        nemesis_utils::fsutil::remove_dir_all_retry(skill_dir)
            .map_err(|e| format!("failed to delete skill: {}", e))?;
        // 学得技能 manifest 同步移除（不在账 = 无操作）。失败降级附注，
        // 不回滚 delete（目录已删，账目残条可由 learned_remove 清）。
        let manifest_note =
            match nemesis_skills::learned::remove_learned(Path::new(&self.workspace), name) {
                Ok(_) => String::new(),
                Err(e) => format!(
                    "\n⚠ learned manifest 移除失败（可由 skills.learned_remove 清理）: {}",
                    e
                ),
            };
        Ok(format!("Skill '{}' deleted.{}", name, manifest_note))
    }
}

#[async_trait]
impl Tool for SkillManageTool {
    fn description(&self) -> String {
        "Create, update, or delete reusable skills (the agent's procedural memory). \
         Persists a SKILL.md to workspace/skills/<name>/ so a workflow can be reused later \
         as a slash command. Actions: create (full SKILL.md incl. --- frontmatter ---), \
         patch/edit (replace old->new), write_file/remove_file (supporting files under the \
         skill dir), delete (remove whole skill). Security-checked before writing."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "patch", "edit", "write_file", "remove_file", "delete"],
                    "description": "Operation to perform"
                },
                "name": {"type": "string", "description": "Skill slug (lowercase, digits, hyphens, <64 chars)"},
                "content": {"type": "string", "description": "Full SKILL.md (create) or file content (write_file)"},
                "path": {"type": "string", "description": "Sub-path within skill dir, e.g. references/api.md (write_file/remove_file)"},
                "old": {"type": "string", "description": "Exact text to replace (patch/edit)"},
                "new": {"type": "string", "description": "Replacement text (patch/edit)"},
                "overwrite": {"type": "boolean", "default": false, "description": "Allow overwriting an existing file"}
            },
            "required": ["action", "name"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let action = v["action"]
            .as_str()
            .ok_or_else(|| "missing required 'action' field".to_string())?;
        let name = v["name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing required 'name' field".to_string())?;

        // Path-traversal protection on the skill name.
        nemesis_skills::types::validate_skill_identifier(name)
            .map_err(|e| format!("invalid skill name '{}': {}", name, e))?;

        let skill_dir = self.skill_dir(name);

        // Approval gate for any write action (no-op unless manage_approval is on).
        if matches!(
            action,
            "create" | "patch" | "edit" | "write_file" | "remove_file" | "delete"
        ) {
            self.check_approval(
                action,
                &skill_dir.display().to_string(),
                &format!("skill '{}' — {}", name, action),
            )
            .await?;
        }

        match action {
            "create" => self.do_create(&skill_dir, name, &v),
            "patch" | "edit" => self.do_patch(&skill_dir, name, &v),
            "write_file" => self.do_write_file(&skill_dir, name, &v),
            "remove_file" => self.do_remove_file(&skill_dir, name, &v),
            "delete" => self.do_delete(&skill_dir, name),
            other => Err(format!(
                "unknown action '{}' (valid: create, patch, edit, write_file, remove_file, delete)",
                other
            )),
        }
    }
}

/// Resolve a relative path inside `base`, rejecting absolute paths and `..`
/// traversal so write_file/remove_file cannot escape the skill directory.
fn resolve_within(base: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() || rel.contains("..") {
        return Err(format!(
            "path must be a relative path within the skill dir, with no '..': {}",
            rel
        ));
    }
    let canon_base = base
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize skill dir: {}", e))?;
    let canon_target = canon_base.join(rel_path);
    if !canon_target.starts_with(&canon_base) {
        return Err(format!("path escapes skill directory: {}", rel));
    }
    Ok(canon_target)
}

// ===========================================================================
// （devtool-upgrade）：question 工具 —— 结构化提问阻塞等答
// ===========================================================================

/// Shareable slot holding an optional question broker, filled in later by the
/// gateway (after the agent loop is built). Lets the `question` tool surface a
/// structured ask-the-user card on the Dashboard and block on the answer.
/// `None`（headless / exec_worker / 基线注册）= 工具不注册（模型根本看不到，
/// 而不是看到一个只会失败的调用）。
pub type QuestionBrokerSlot =
    Arc<parking_lot::RwLock<Option<Arc<dyn nemesis_types::agent::QuestionAsker>>>>;

/// question 工具默认等待窗口（秒）。超时回灌「按最佳判断继续」。
pub const QUESTION_DEFAULT_TIMEOUT_SECS: u64 = 120;

/// （提问两形态）：表单形态字段数上限。超限诚实拒绝（防模型一卡塞满
/// 屏幕；两形态共用 question_id 发号器，超限消耗一个 ID 无害）。
pub const QUESTION_MAX_FIELDS: usize = 8;

/// 单选卡片选项上限（≤6 radio 列表，>6 前端收纳为下拉 select）。
pub const QUESTION_MAX_SINGLE_OPTIONS: usize = 12;
