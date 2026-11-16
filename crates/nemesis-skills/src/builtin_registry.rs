//! Builtin skills registry - 仓库自带 `Skills/` 目录作为第一方技能源。
//!
//! 双臂形态的**离线兜底臂**（复刻 nemesis-web persona builtin 内嵌先例，
//! 2026-11-15 拍板②同款降级链）：live GitHub 臂见 config.skills 出厂第三源
//! （仓库 `Skills/{slug}/SKILL.md`，加技能 = 提交目录，部署实例不重编可见
//! 可装）；本内嵌臂让断网/纯内网实例与未升级旧工作区也能搜到、装到仓库
//! 自带技能。
//!
//! **安装走统一漏斗**（用户拍板 2026-11-16，不跳过不特例）：
//! `download_and_install` 只负责把内嵌树写进 staging；验签四态 → enforce_trust
//! （无 `.signature` → `allow_unsigned` 裁决，缺省 ReviewRequired 走审批卡）
//! → `check_skill_security_dir` 整目录 lint → InstallPlan 审批 → lockfile/
//! origin 记账，全部由 `SkillInstaller::install` 共享路径承接。
//!
//! 内容说明：仓库 SKILL.md 无 YAML frontmatter（`# 标题` 起头），展示名取
//! 首个标题行、摘要取首个正文段（blockquote 剥 `>` 前缀）截断。
//! include_dir 路径语义：`get_file`/entry `path()` 均相对**内嵌根**的完整
//! 路径（nemesis-web persona 先例同款结论），子树释放按 slug 前缀剥除。

use std::path::Path;

use async_trait::async_trait;

use nemesis_types::error::{NemesisError, Result};

use crate::registry::SkillRegistry;
use crate::types::{
    BrowseResult, BrowseSort, InstallResult, SkillContent, SkillMeta, SkillSearchResult,
    contains_ci,
};

/// 编译期内嵌的仓库技能目录（repo 根 `Skills/`；与 nemesis-web 的
/// `BUILTIN_PERSONAS_DIR` 同形态）。运行期配置无法触达该目录的内容——
/// 它随二进制发行。
static BUILTIN_SKILLS_DIR: include_dir::Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../Skills");

/// 内嵌技能目录名（registry name；WSAPI/前端按此展示与路由安装）。
const REGISTRY_NAME: &str = "builtin";

/// 摘要截断长度（首段往往是一整段文字；市场卡片展示用）。
const SUMMARY_MAX_CHARS: usize = 120;

/// Builtin 技能注册表（unit 结构，数据面全在编译期内嵌目录）。
pub struct BuiltinSkillsRegistry;

impl BuiltinSkillsRegistry {
    pub fn new() -> Self {
        Self
    }

    /// 内嵌技能 slug 列表（顶层技能目录名 = 根下含 `{slug}/SKILL.md` 的
    /// 目录；显式排序——目录顺序是平台实现细节，消费侧不依赖字典序以外
    /// 的次序）。
    pub fn skill_slugs() -> Vec<String> {
        let mut slugs: Vec<String> = BUILTIN_SKILLS_DIR
            .dirs()
            .filter_map(|d| {
                let name = d.path().file_name()?.to_string_lossy().to_string();
                if BUILTIN_SKILLS_DIR
                    .get_file(format!("{name}/SKILL.md"))
                    .is_some()
                {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        slugs.sort();
        slugs
    }

    /// 按 slug 取内嵌技能子目录（精确匹配顶层目录名——slug 结构上不可能
    /// 越出 `Skills/` 树，路径穿越无从谈起；仍做显式形态检查快速失败）。
    /// 返回类型显式 `Dir<'static>`——裸 `Dir` 的省略生命周期会绑定到入参
    /// `slug`（include_dir::Dir 对 lifetime 不变），把入参抬成 'static。
    fn find_skill(slug: &str) -> Result<&'static include_dir::Dir<'static>> {
        if slug.is_empty() || slug.contains('/') || slug.contains('\\') || slug.contains("..") {
            return Err(NemesisError::Validation(format!(
                "invalid builtin skill slug '{slug}'"
            )));
        }
        BUILTIN_SKILLS_DIR
            .get_dir(slug)
            .ok_or_else(|| NemesisError::NotFound(format!("builtin skill '{slug}' not found")))
    }

    /// 读 SKILL.md 正文（owned——生命周期不上穿 trait 方法签名）。
    fn skill_md(slug: &str) -> Result<String> {
        let file = BUILTIN_SKILLS_DIR
            .get_file(format!("{slug}/SKILL.md"))
            .ok_or_else(|| {
                NemesisError::Validation(format!("builtin skill '{slug}' has no SKILL.md"))
            })?;
        let text = std::str::from_utf8(file.contents()).map_err(|e| {
            NemesisError::Validation(format!("builtin skill '{slug}' SKILL.md is not UTF-8: {e}"))
        })?;
        Ok(text.to_string())
    }

    /// 展示名：首个 `# ` 标题行文本；无标题回落 slug。
    fn extract_display_name(content: &str, slug: &str) -> String {
        content
            .lines()
            .find_map(|l| l.strip_prefix("# ").map(str::trim))
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| slug.to_string())
    }

    /// 摘要：首个非空、非标题行（blockquote 剥 `>` 前缀），截断。
    fn extract_summary(content: &str) -> String {
        let raw = content
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .unwrap_or("")
            .trim_start_matches('>')
            .trim();
        let mut out: String = raw.chars().take(SUMMARY_MAX_CHARS).collect();
        if raw.chars().count() > SUMMARY_MAX_CHARS {
            out.push('…');
        }
        out
    }

    fn search_result(slug: &str) -> Result<SkillSearchResult> {
        let content = Self::skill_md(slug)?;
        Ok(SkillSearchResult {
            score: 1.0,
            slug: slug.to_string(),
            display_name: Self::extract_display_name(&content, slug),
            summary: Self::extract_summary(&content),
            version: "latest".to_string(),
            registry_name: REGISTRY_NAME.to_string(),
            source_repo: "Skills/".to_string(),
            download_path: format!("Skills/{slug}/SKILL.md"),
            downloads: 0,
            truncated: false,
        })
    }

    /// 递归释放内嵌子树（文件 + 嵌套目录；多文件技能如 automated-testing
    /// 的 examples/phases/scripts 必须整树落位，不能只拷 SKILL.md）。
    /// include_dir 0.7 的 `files()`/`dirs()` 都只返回**直接子条目**（源码
    /// `entries().iter().filter_map`，非递归——初版只落 SKILL.md 的根因），
    /// 因此逐层下钻；子条目路径相对内嵌根（带 slug 前缀），落位名取
    /// `file_name`（slug 已在 target 解析，不再重复拼前缀）。
    fn extract_tree(dir: &'static include_dir::Dir<'static>, target: &Path) -> Result<()> {
        std::fs::create_dir_all(target).map_err(NemesisError::Io)?;
        for f in dir.files() {
            let name = f.path().file_name().ok_or_else(|| {
                NemesisError::Validation(format!(
                    "builtin skill tree has bad file path: {}",
                    f.path().display()
                ))
            })?;
            std::fs::write(target.join(name), f.contents()).map_err(NemesisError::Io)?;
        }
        for sub in dir.dirs() {
            let name = sub.path().file_name().ok_or_else(|| {
                NemesisError::Validation(format!(
                    "builtin skill tree has bad dir path: {}",
                    sub.path().display()
                ))
            })?;
            Self::extract_tree(sub, &target.join(name))?;
        }
        Ok(())
    }
}

impl Default for BuiltinSkillsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SkillRegistry for BuiltinSkillsRegistry {
    fn name(&self) -> &str {
        REGISTRY_NAME
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        let mut results = Vec::new();
        for slug in Self::skill_slugs() {
            if results.len() >= limit {
                break;
            }
            // 命中面：slug 或展示名/摘要任一含查询串（空查询 = 全量列出，
            // contains_ci 对空 needle 恒真）。
            let content = Self::skill_md(&slug)?;
            let display = Self::extract_display_name(&content, &slug);
            let summary = Self::extract_summary(&content);
            if contains_ci(&slug, query)
                || contains_ci(&display, query)
                || contains_ci(&summary, query)
            {
                results.push(Self::search_result(&slug)?);
            }
        }
        Ok(results)
    }

    async fn get_skill_meta(&self, slug: &str) -> Result<SkillMeta> {
        Self::find_skill(slug)?;
        let content = Self::skill_md(slug)?;
        Ok(SkillMeta {
            slug: slug.to_string(),
            display_name: Self::extract_display_name(&content, slug),
            summary: Self::extract_summary(&content),
            latest_version: "latest".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: REGISTRY_NAME.to_string(),
            author: String::new(),
            downloads: 0,
            published_at: None,
        })
    }

    async fn download_and_install(
        &self,
        slug: &str,
        version: &str,
        target_dir: &str,
    ) -> Result<InstallResult> {
        let dir = Self::find_skill(slug)?;
        let summary = Self::extract_summary(&Self::skill_md(slug)?);
        Self::extract_tree(dir, Path::new(target_dir))?;
        Ok(InstallResult {
            version: if version.is_empty() {
                "latest".to_string()
            } else {
                version.to_string()
            },
            is_malware_blocked: false,
            is_suspicious: false,
            summary,
        })
    }

    async fn get_skill_content(&self, slug: &str) -> Result<SkillContent> {
        Self::find_skill(slug)?;
        let content = Self::skill_md(slug)?;
        Ok(SkillContent {
            slug: slug.to_string(),
            filename: "SKILL.md".to_string(),
            content,
        })
    }

    async fn browse(&self, _sort: &BrowseSort, limit: usize, cursor: &str) -> Result<BrowseResult> {
        // 内置技能无热度/时间统计面——所有排序键同一稳定 slug 序（文档注记）。
        let slugs = Self::skill_slugs();
        let offset: usize = cursor.parse().unwrap_or(0);
        let start = offset.min(slugs.len());
        let end = (start + limit).min(slugs.len());
        let mut items = Vec::new();
        for slug in &slugs[start..end] {
            items.push(Self::search_result(slug)?);
        }
        let next_cursor = if end < slugs.len() {
            Some(end.to_string())
        } else {
            None
        };
        Ok(BrowseResult { items, next_cursor })
    }
}

#[cfg(test)]
mod tests;
