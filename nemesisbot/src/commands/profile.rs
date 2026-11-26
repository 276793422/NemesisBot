//! 用户习惯画像 CLI（Forge 化重构，2026-11-24：API 面迁
//! `nemesis-forge::userprofile`）。
//!
//! `nemesisbot profile <action>`：list / enable / disable / veto / forget /
//! pin。与 WSAPI `profile.*` 同源（nemesis-forge::userprofile::{store,
//! surface}）——CLI 直读直写盘上真相；运行中 gateway 的 sweep 下轮 tick
//! 感知。

use anyhow::Result;
use clap::Subcommand;

use crate::common;
use nemesis_forge::userprofile::store::ProfileScope;
use nemesis_forge::userprofile::{store, surface};

#[derive(Debug, Subcommand)]
pub enum ProfileAction {
    /// Show habit entries + switch state
    List,
    /// Enable profiling (records the enabled_at anchor; forward-only)
    Enable,
    /// Disable profiling (existing entries kept, no further collection)
    Disable,
    /// Veto habits whose content contains the substring (negative samples)
    Veto {
        /// Content substring to match
        content: String,
    },
    /// Forget habits whose content contains the substring (removable)
    Forget {
        /// Content substring to match
        content: String,
    },
    /// Pin / unpin habits whose content contains the substring
    Pin {
        /// Content substring to match
        content: String,
        /// Unpin instead of pin
        #[arg(long)]
        unpin: bool,
    },
    /// Move a habit between dialogue/project profiles (batch 2, 2026-11-26)
    Move {
        /// Content substring to match (exact normalized match preferred)
        content: String,
        /// Target profile scope (dialogue|project)
        #[arg(value_parser = ["dialogue", "project"])]
        to_scope: String,
    },
}

pub fn run(action: ProfileAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let workspace = common::workspace_path(&home);
    match action {
        ProfileAction::List => {
            let profile = surface::list(&workspace, ProfileScope::Dialogue);
            let cfg = nemesis_config::load_config(&common::config_path(&home))
                .ok()
                .and_then(|cfg| cfg.user_profile);
            let enabled = cfg.as_ref().map(|u| u.enabled).unwrap_or(false);
            println!(
                "enabled: {} (enabled_at: {})",
                enabled,
                profile.enabled_at.as_deref().unwrap_or("(never)")
            );
            println!("habits ({}):", profile.habits.len());
            for h in &profile.habits {
                println!(
                    "  - [{}] {} (confidence {:.2}, hits {}, status {}, {}, {})",
                    h.category,
                    h.content,
                    h.confidence,
                    h.hits,
                    h.status,
                    if h.pinned { "pinned" } else { "unpinned" },
                    if h.auto { "auto" } else { "manual" },
                );
                for (i, s) in h.steps.iter().enumerate() {
                    println!("      {}. {}", i + 1, s);
                }
                if !h.evidence.is_empty() {
                    println!("      溯源: {}", h.evidence.join(", "));
                }
            }
        }
        ProfileAction::Enable => {
            // 开关先落 config（写锁在 save_config 内），锚点后置——
            // 失败顺序安全：开关失败 = 锚点不动；锚点成功后开关必已开。
            set_config_enabled(&home, true)?;
            // 截断上限与 sweep 同源：config 现读 max_entries。
            let max_entries = nemesis_config::load_config(&common::config_path(&home))
                .ok()
                .and_then(|cfg| cfg.user_profile)
                .map(|u| u.max_entries_or(store::MAX_ENTRIES))
                .unwrap_or(store::MAX_ENTRIES);
            let profile = surface::enable(&workspace, max_entries).map_err(anyhow::Error::msg)?;
            println!(
                "✓ profile enabled (enabled_at: {})",
                profile.enabled_at.as_deref().unwrap_or("(now)")
            );
        }
        ProfileAction::Disable => {
            set_config_enabled(&home, false)?;
            println!("✓ profile disabled (config user_profile.enabled=false)");
        }
        ProfileAction::Veto { content } => {
            // home = append_event 总闸（复查 2026-11-25 批 F3：学习关闭不落）。
            let hit = surface::veto(&workspace, ProfileScope::Dialogue, home.to_str(), &content)
                .map_err(anyhow::Error::msg)?;
            println!(
                "{}",
                if hit {
                    "✓ vetoed"
                } else {
                    "○ no matching habit"
                }
            );
        }
        ProfileAction::Forget { content } => {
            let hit = surface::forget(&workspace, ProfileScope::Dialogue, home.to_str(), &content)
                .map_err(anyhow::Error::msg)?;
            println!(
                "{}",
                if hit {
                    "✓ forgotten"
                } else {
                    "○ no matching habit"
                }
            );
        }
        ProfileAction::Pin { content, unpin } => {
            let hit = surface::pin(&workspace, ProfileScope::Dialogue, &content, !unpin)
                .map_err(anyhow::Error::msg)?;
            println!(
                "{}",
                if hit {
                    if unpin { "✓ unpinned" } else { "✓ pinned" }
                } else {
                    "○ no matching habit"
                }
            );
        }
        ProfileAction::Move { content, to_scope } => {
            let to = parse_scope_wire(&to_scope).map_err(anyhow::Error::msg)?;
            let out =
                store::move_habit_scope(&workspace, &content, to).map_err(anyhow::Error::msg)?;
            if out.moved {
                println!("✓ moved {} → {}", out.content, out.to);
            } else {
                println!("○ already in {} (no-op): {}", out.to, out.content);
            }
        }
    }
    Ok(())
}

/// scope wire 名解析（CLI 边缘本地形态；web 侧 `required_scope` 同构）：
/// dialogue|project 大小写不敏感，非法值 loud 报错带合法值提示。
fn parse_scope_wire(s: &str) -> std::result::Result<ProfileScope, String> {
    match s.trim().to_lowercase().as_str() {
        "dialogue" => Ok(ProfileScope::Dialogue),
        "project" => Ok(ProfileScope::Project),
        other => Err(format!(
            "unknown scope '{other}'（合法值：dialogue|project）"
        )),
    }
}

/// config.json `user_profile.enabled` 开关（typed load/save——flatten 透传
/// 保 untyped per-model 键；写锁在 save_config 内）。
fn set_config_enabled(home: &std::path::Path, enabled: bool) -> Result<()> {
    let cfg_path = common::config_path(home);
    let mut cfg = nemesis_config::load_config(&cfg_path)?;
    let entry = cfg
        .user_profile
        .get_or_insert_with(nemesis_config::UserProfileConfig::default);
    entry.enabled = enabled;
    nemesis_config::save_config(&cfg_path, &mut cfg)?;
    Ok(())
}
