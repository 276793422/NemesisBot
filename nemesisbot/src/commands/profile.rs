//! 用户习惯画像 CLI（自进化升级 B 线，2026-10-14）。
//!
//! `nemesisbot profile <action>`：list / enable / disable / veto / forget /
//! pin。与 WSAPI `profile.*` 同源（nemesis-agent::user_profile）——CLI 直读
//! 直写盘上真相；运行中 gateway 的 sweep 下轮 tick 感知。

use anyhow::Result;
use clap::Subcommand;

use crate::common;

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
}

pub fn run(action: ProfileAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let workspace = common::workspace_path(&home);
    match action {
        ProfileAction::List => {
            let profile = nemesis_agent::user_profile::load_profile(&workspace);
            let enabled = nemesis_config::load_config(&common::config_path(&home))
                .ok()
                .and_then(|cfg| cfg.user_profile)
                .map(|u| u.enabled)
                .unwrap_or(false);
            println!(
                "enabled: {} (enabled_at: {})",
                enabled,
                profile.enabled_at.as_deref().unwrap_or("(never)")
            );
            println!("habits ({}):", profile.habits.len());
            for h in &profile.habits {
                println!(
                    "  - [{}] {} (confidence {:.2}, hits {}, status {}, {})",
                    h.category,
                    h.content,
                    h.confidence,
                    h.hits,
                    h.status,
                    if h.pinned { "pinned" } else { "unpinned" }
                );
            }
        }
        ProfileAction::Enable => {
            // 开关先落 config（写锁在 set_config_value 内），锚点后置——
            // 失败顺序安全：开关失败 = 锚点不动；锚点成功后开关必已开。
            set_config_enabled(&home, true)?;
            // 截断上限与 sweep 同源：config 现读 max_entries（BUG-20261025-20：
            // 此前硬编码 10，max_entries>10 的存量习惯在重跑 enable 时被截）。
            let max_entries = nemesis_config::load_config(&common::config_path(&home))
                .ok()
                .and_then(|cfg| cfg.user_profile)
                .map(|u| u.max_entries_or(nemesis_agent::user_profile::MAX_ENTRIES))
                .unwrap_or(nemesis_agent::user_profile::MAX_ENTRIES);
            let profile = nemesis_agent::user_profile::enable_profile(&workspace, max_entries)
                .map_err(anyhow::Error::msg)?;
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
            let hit = nemesis_agent::user_profile::veto_habit(&workspace, &content)
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
            let hit = nemesis_agent::user_profile::forget_habit(&workspace, &content)
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
            let hit = nemesis_agent::user_profile::set_pinned(&workspace, &content, !unpin)
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
    }
    Ok(())
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
