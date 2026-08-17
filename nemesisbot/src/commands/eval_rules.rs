//! `nemesisbot eval rules ...` —— 规则管理命令组（plan Step 3）。
//!
//! 纯文件操作（读改写 `workspace/config/eval_rules.json`），无平台门控、
//! 无沙盒依赖——run() 里 Rules 分支前置独立处理（A1 修订）。
//!
//! add/edit 校验 JSON 结构 + 正则合法性 + id 规则（add 冲突拒绝、
//! edit 要求存在、remove/enable/disable 不存在报错）。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Subcommand;

use crate::common;
use crate::eval_assessor;

#[derive(Subcommand)]
pub enum RulesAction {
    /// List all rules (disabled ones marked).
    List {
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Show one rule's full definition.
    Show {
        id: String,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Add rule(s) from a JSON file (single rule object or {"rules":[...]}).
    Add {
        /// Path to the rule JSON file.
        #[arg(long)]
        file: PathBuf,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Replace an existing rule from a JSON file (single rule object; id in file wins).
    Edit {
        id: String,
        /// Path to the new rule JSON file.
        #[arg(long)]
        file: PathBuf,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Remove a rule.
    Remove {
        id: String,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Enable a rule.
    Enable {
        id: String,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Disable a rule.
    Disable {
        id: String,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
    /// Reset to the built-in default rule set (confirmation prompt; --force skips).
    Reset {
        /// Skip the confirmation prompt.
        #[arg(long)]
        force: bool,
        /// Use ./.nemesisbot as home (same as the global --local).
        #[arg(long)]
        local: bool,
    },
}

pub async fn run(action: RulesAction, cli_local: bool) -> Result<()> {
    use RulesAction as A;
    // Z1 修复：每个子命令带 --local（与 eval prompt/skill 的 EvalCommon.local
    // 对齐——全局 --local 也仍然有效，两者取或）。缺它时用户按 eval 其他
    // 子命令的肌肉记忆写 `eval rules list --local` 会 clap 报错。
    let (local, act, writes): (bool, A, bool) = match action {
        A::List { local } => (local, A::List { local }, false),
        A::Show { id, local } => (local, A::Show { id, local }, false),
        A::Add { file, local } => (local, A::Add { file, local }, true),
        A::Edit { id, file, local } => (local, A::Edit { id, file, local }, true),
        A::Remove { id, local } => (local, A::Remove { id, local }, true),
        A::Enable { id, local } => (local, A::Enable { id, local }, true),
        A::Disable { id, local } => (local, A::Disable { id, local }, true),
        A::Reset { force, local } => (local, A::Reset { force, local }, true),
    };
    let home = common::resolve_home(cli_local || local);

    // BB4（第十轮）防静默建家：--local 指向的 {cwd}/.nemesisbot 不存在时，
    // resolve_home 仍返回该路径，load_rules 会在那里**静默创建**完整 home
    //（实测：在仓库根跑 `eval rules add --local` 就地长出一棵 .nemesisbot/）。
    // 写命令拒绝；只读命令直接展示内置默认集（不落盘、不建目录）。
    let local_missing = (cli_local || local) && !home.exists();
    if local_missing {
        if writes {
            bail!(
                "--local home 不存在：{}（写命令拒绝静默创建；先在目标目录 onboard，或去掉 --local 用默认 home）",
                home.display()
            );
        }
        // 只读命令（list/show）：从内置默认集展示，不落盘不建目录。
        println!("--local home 不存在：{}（以下为内置默认规则，未读写任何文件）", home.display());
        let defaults = eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON)?;
        match &act {
            A::Show { id, .. } => {
                let r = defaults
                    .rules
                    .iter()
                    .find(|r| r.id == *id)
                    .with_context(|| format!("rule '{id}' not found in built-in defaults"))?;
                println!("{}", serde_json::to_string_pretty(r)?);
            }
            _ => print_rule_table(&defaults),
        }
        return Ok(());
    }

    let path = eval_assessor::rules_file_path(&home);

    match act {
        A::List { .. } => cmd_list(&path),
        A::Show { id, .. } => cmd_show(&path, &id),
        A::Add { file, .. } => cmd_add(&path, &file),
        A::Edit { id, file, .. } => cmd_edit(&path, &id, &file),
        A::Remove { id, .. } => cmd_remove(&path, &id),
        A::Enable { id, .. } => cmd_toggle(&path, &id, true),
        A::Disable { id, .. } => cmd_toggle(&path, &id, false),
        A::Reset { force, .. } => cmd_reset(&path, force),
    }
}

fn cmd_list(path: &std::path::Path) -> Result<()> {
    let file = eval_assessor::load_rules(path)?;
    println!("Rules file: {}", path.display());
    print_rule_table(&file);
    Ok(())
}

/// 规则表打印（cmd_list 与 --local-home-missing 的降级展示共用）。
fn print_rule_table(file: &eval_assessor::RulesFile) {
    if file.rules.is_empty() {
        println!("(no rules)");
        return;
    }
    println!("{:<3} {:<28} {:<8} {:<7} {:<14} {}", "#", "ID", "LEVEL", "ON", "SOURCE", "DESCRIPTION");
    for (i, r) in file.rules.iter().enumerate() {
        let on = if r.enabled { "yes" } else { "no" };
        println!(
            "{:<3} {:<28} {:<8} {:<7} {:<14} {}",
            i + 1,
            r.id,
            r.level,
            on,
            r.source,
            r.description
        );
    }
    let enabled = file.rules.iter().filter(|r| r.enabled).count();
    println!("\n{} rule(s), {} enabled", file.rules.len(), enabled);
}

fn cmd_show(path: &std::path::Path, id: &str) -> Result<()> {
    let file = eval_assessor::load_rules(path)?;
    let r = file
        .rules
        .iter()
        .find(|r| r.id == id)
        .with_context(|| format!("rule '{id}' not found (see: eval rules list)"))?;
    println!("{}", serde_json::to_string_pretty(r)?);
    Ok(())
}

fn cmd_add(path: &std::path::Path, file_in: &std::path::Path) -> Result<()> {
    let content = std::fs::read_to_string(file_in)
        .with_context(|| format!("read rules file {}", file_in.display()))?;
    let incoming = eval_assessor::parse_rules_lenient(&content)
        .with_context(|| format!("invalid rules file {}", file_in.display()))?;
    if incoming.rules.is_empty() {
        bail!("input file contains no rules");
    }

    let mut file = eval_assessor::load_rules(path)?;
    for r in &incoming.rules {
        if file.rules.iter().any(|x| x.id == r.id) {
            bail!("rule '{}' already exists (use: eval rules edit {} --file ...)", r.id, r.id);
        }
    }
    file.rules.extend(incoming.rules.clone());
    eval_assessor::save_rules(path, &file)?;
    for r in &incoming.rules {
        println!("added rule '{}' ({}, {})", r.id, r.level, r.source);
    }
    Ok(())
}

fn cmd_edit(path: &std::path::Path, id: &str, file_in: &std::path::Path) -> Result<()> {
    let content = std::fs::read_to_string(file_in)
        .with_context(|| format!("read rules file {}", file_in.display()))?;
    let incoming = eval_assessor::parse_rules_lenient(&content)
        .with_context(|| format!("invalid rules file {}", file_in.display()))?;
    // edit 是单规则语义：多条规则静默取第一条丢其余是坑——明确拒绝，
    // 想批量加用 add（add 天然支持 {"rules":[...]})。
    if incoming.rules.len() != 1 {
        bail!(
            "edit expects exactly one rule, got {} (use `eval rules add --file` for batch)",
            incoming.rules.len()
        );
    }
    let new_rule = incoming.rules.into_iter().next().unwrap();
    if new_rule.id != id {
        bail!("rule id mismatch: edit target is '{id}' but the file defines '{}'", new_rule.id);
    }

    let mut file = eval_assessor::load_rules(path)?;
    let idx = file
        .rules
        .iter()
        .position(|r| r.id == id)
        .with_context(|| format!("rule '{id}' not found (see: eval rules list)"))?;
    file.rules[idx] = new_rule;
    eval_assessor::save_rules(path, &file)?;
    println!("updated rule '{id}'");
    Ok(())
}

fn cmd_remove(path: &std::path::Path, id: &str) -> Result<()> {
    let mut file = eval_assessor::load_rules(path)?;
    let before = file.rules.len();
    file.rules.retain(|r| r.id != id);
    if file.rules.len() == before {
        bail!("rule '{id}' not found (see: eval rules list)");
    }
    eval_assessor::save_rules(path, &file)?;
    println!("removed rule '{id}'");
    Ok(())
}

fn cmd_toggle(path: &std::path::Path, id: &str, enable: bool) -> Result<()> {
    let mut file = eval_assessor::load_rules(path)?;
    let r = file
        .rules
        .iter_mut()
        .find(|r| r.id == id)
        .with_context(|| format!("rule '{id}' not found (see: eval rules list)"))?;
    r.enabled = enable;
    let state = if enable { "enabled" } else { "disabled" };
    eval_assessor::save_rules(path, &file)?;
    println!("{state} rule '{id}'");
    Ok(())
}

fn cmd_reset(path: &std::path::Path, force: bool) -> Result<()> {
    let existing = eval_assessor::load_rules(path)?; // also seeds if missing
    let custom_count = existing.rules.len();

    if !force {
        print!(
            "Reset will replace {custom_count} rule(s) in {} with the built-in defaults. Continue? (y/N): ",
            path.display()
        );
        use std::io::{self as std_io, Write as StdWrite};
        std_io::stdout().flush().ok();
        let mut answer = String::new();
        std_io::stdin().read_line(&mut answer).ok();
        if answer.trim().to_lowercase() != "y" {
            println!("Aborted (use --force to skip confirmation).");
            return Ok(());
        }
    }

    let defaults = eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON)?;
    eval_assessor::save_rules(path, &defaults)?;
    println!("reset {custom_count} → {} rule(s) from built-in defaults", defaults.rules.len());
    Ok(())
}

#[cfg(test)]
mod tests;
