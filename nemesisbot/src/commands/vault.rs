//! `nemesisbot vault` —— 凭据 vault CLI（P0 安全升级）。
//!
//! 与 `nemesisbot credentials`（模型 API key 的 yaml 引用存储，U15）和
//! `nemesisbot auth`（OAuth 凭据存储）语义不同：vault 是**通用加密秘密存储**
//! （`workspace/config/vault.enc`，AES-256-GCM + DPAPI/Argon2id）。
//!
//! 铁律：`list` 永不显示值；secret/口令经隐藏输入，不进 shell history。

use crate::common;
use anyhow::{Context, Result};
use clap::Subcommand;
use nemesis_security::vault::{VaultMode, VaultStore};

#[derive(Subcommand)]
pub enum VaultAction {
    /// 写入（或轮换）一个别名。secret 经隐藏输入，不回显。
    Set {
        /// 别名（如 `openai-main`、`telegram-main`）。
        alias: String,
        /// 可选域标注（如 `openai`、`telegram`）。
        #[arg(long)]
        domain: Option<String>,
        /// 可选描述。
        #[arg(long)]
        description: Option<String>,
        /// 覆盖已有别名前要求确认（防误轮换）。
        #[arg(long)]
        force: bool,
        /// 从 stdin 读一行 secret（脚本/自动化；交互模式默认隐藏输入）。
        #[arg(long)]
        stdin: bool,
    },
    /// 列出全部别名与元数据（**永不显示值**）。
    List,
    /// 删除一个别名（需确认）。
    Remove { alias: String },
    /// 迁移模型 API key 到 vault（P0 B2）：config.json 明文 key 与
    /// credentials.yaml 条目 → vault 别名，配置回写 `vault:<alias>`。
    /// 幂等——已迁移的条目跳过；同值复用别名，异值加后缀不覆盖。
    Migrate,
}

/// 读取 vault 模式对应口令：argon2 模式取 `NEMESISBOT_VAULT_PASSPHRASE`
/// 环境变量，缺失则隐藏输入；dpapi 模式无需口令。
fn passphrase_for_mode(mode: VaultMode) -> Result<Option<String>> {
    match mode {
        VaultMode::Dpapi => Ok(None),
        VaultMode::Argon2id => match std::env::var("NEMESISBOT_VAULT_PASSPHRASE") {
            Ok(p) if !p.is_empty() => Ok(Some(p)),
            _ => Ok(Some(rpassword::prompt_password("vault 口令: ")?)),
        },
    }
}

/// 打开（必要时创建）vault，argon2 模式完成解锁。
fn open_or_create(vault_path: &std::path::Path) -> Result<VaultStore> {
    let store = if vault_path.exists() {
        let mut s = VaultStore::open(vault_path)
            .with_context(|| format!("打开 vault 失败: {}", vault_path.display()))?;
        if !s.is_unlocked() {
            let pw = passphrase_for_mode(s.mode())?;
            if let Some(pw) = pw {
                s.unlock(&pw).context("vault 解锁失败（口令错误？）")?;
            }
        }
        s
    } else {
        let mode = VaultStore::default_mode();
        let pw = passphrase_for_mode(mode)?;
        VaultStore::create(vault_path, mode, pw.as_deref())
            .with_context(|| format!("创建 vault 失败: {}", vault_path.display()))?
    };
    Ok(store)
}

pub async fn run(action: VaultAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let vault_path = nemesis_path::resolve_vault_path_in_workspace(&common::workspace_path(&home));

    match action {
        VaultAction::Set {
            alias,
            domain,
            description,
            force,
            stdin: read_stdin,
        } => {
            let mut store = open_or_create(&vault_path)?;
            let existed = store.aliases().iter().any(|a| a == &alias);
            if existed && !force {
                let confirm = prompt_confirm(&format!(
                    "别名「{alias}」已存在，覆盖（轮换）？值不可回显，操作不可撤销"
                ))?;
                if !confirm {
                    println!("已取消。");
                    return Ok(());
                }
            }
            let secret = if read_stdin {
                use std::io::BufRead as _;
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line)?;
                while line.ends_with('\n') || line.ends_with('\r') {
                    line.pop();
                }
                line
            } else {
                rpassword::prompt_password(format!("输入 {alias} 的 secret: "))?
            };
            if secret.is_empty() {
                anyhow::bail!("secret 为空，已取消");
            }
            store
                .set(
                    &alias,
                    &secret,
                    &domain.unwrap_or_default(),
                    &description.unwrap_or_default(),
                )
                .context("写入 vault 失败")?;
            store.save().context("保存 vault 失败")?;
            if existed {
                println!("已轮换别名「{alias}」（created_at 保留，rotated_at 更新）。");
            } else {
                println!("已写入别名「{alias}」。配置中引用写作 vault:{alias}");
            }
        }
        VaultAction::List => {
            let store = open_or_create(&vault_path)?;
            let listings = store.list();
            if listings.is_empty() {
                println!("vault 为空（{}）。", vault_path.display());
                return Ok(());
            }
            println!(
                "{:<28} {:<14} {:<24} {:<24} {}",
                "ALIAS", "DOMAIN", "CREATED", "ROTATED", "DESCRIPTION"
            );
            for l in &listings {
                println!(
                    "{:<28} {:<14} {:<24} {:<24} {}",
                    l.alias,
                    if l.meta.domain.is_empty() {
                        "-"
                    } else {
                        &l.meta.domain
                    },
                    &l.meta.created_at,
                    l.meta.rotated_at.as_deref().unwrap_or("-"),
                    if l.meta.description.is_empty() {
                        "-"
                    } else {
                        &l.meta.description
                    },
                );
            }
            println!();
            println!(
                "共 {} 条。值永不显示；配置引用写作 vault:<alias>",
                listings.len()
            );
        }
        VaultAction::Remove { alias } => {
            let mut store = open_or_create(&vault_path)?;
            if !store.aliases().iter().any(|a| a == &alias) {
                anyhow::bail!("别名不存在: {alias}");
            }
            let confirm = prompt_confirm(&format!("确认删除别名「{alias}」？"))?;
            if !confirm {
                println!("已取消。");
                return Ok(());
            }
            store.remove(&alias)?;
            store.save()?;
            println!("已删除「{alias}」。注意：引用该别名的配置会开始报解析错误。");
        }
        VaultAction::Migrate => {
            let mut store = open_or_create(&vault_path)?;
            let config_path = home.join("config.json");
            let cred_path = nemesis_config::credentials::credentials_path_for_home(&home);
            let report = migrate_model_keys(&config_path, &cred_path, &mut store)?;
            store.save().context("保存 vault 失败")?;

            println!(
                "模型 API key 迁移到加密 vault（{}，{}）",
                vault_path.display(),
                store.mode()
            );
            println!("  config.json : {}", config_path.display());
            if !report.yaml_aliases_removed.is_empty() {
                println!(
                    "  credentials : {}（已迁移条目将移除）",
                    cred_path.display()
                );
            }
            if report.is_noop() {
                println!();
                println!("没有需要迁移的 key（缺失或全部已是 env:/yaml:/vault: 引用/空值）。");
                return Ok(());
            }
            println!();
            if !report.migrated.is_empty() {
                println!("迁移 {} 个：", report.migrated.len());
                for (name, alias, src) in &report.migrated {
                    println!("  [{src}] {name} -> vault:{alias}");
                }
            }
            if report.reused > 0 {
                println!("复用已有同值别名：{} 个", report.reused);
            }
            for (want, used) in &report.conflicts {
                println!("警告：别名「{want}」已存在且值不同，改用「{used}」（原值未覆盖）");
            }
            if !report.yaml_broken.is_empty() {
                for (name, alias) in &report.yaml_broken {
                    println!(
                        "警告：{name} 引用 yaml:{alias}，但 credentials.yaml 中无此条目——未迁移，请检查"
                    );
                }
            }
            if report.skipped_env > 0 || report.already_vault > 0 {
                println!(
                    "跳过：{} 个 env: 引用（非明文落盘，保持原样）、{} 个已是 vault:",
                    report.skipped_env, report.already_vault
                );
            }
            println!();
            println!("迁移期兼容说明：明文 key 在迁移前继续工作；每次解析进程会提示一次。");
        }
    }
    Ok(())
}

/// P0 B2（2026-09-22 计划 §2.4）：模型 key 迁移核心。
///
/// 三个来源一次处理：
/// 1. config.json 内联明文 key → vault（别名 = sanitize(model_name)）
/// 2. `yaml:<alias>` 引用 → 值从 credentials.yaml 搬进 vault（别名不变），
///    配置回写 `vault:<alias>`，credentials.yaml 对应条目移除（明文彻底
///    离盘）
/// 3. `env:` 引用保持原样（非明文落盘）；`vault:` 已迁移跳过
///
/// 冲突策略与 `credentials::run_import` 一致：已有别名同值 → 复用；异值 →
/// `__2`/`__3` 后缀，绝不覆盖。config 用 typed round-trip 保存（ModelConfig
/// extra flatten 保未类型化键，与 credentials 迁移同路线）。
fn migrate_model_keys(
    config_path: &std::path::Path,
    cred_path: &std::path::Path,
    store: &mut VaultStore,
) -> Result<MigrateReport> {
    let mut report = MigrateReport::default();
    if !config_path.exists() {
        return Ok(report);
    }
    let mut config = nemesis_config::load_config(config_path)
        .map_err(|e| anyhow::anyhow!("读取 config.json 失败: {e}"))?;
    let mut creds = if cred_path.exists() {
        Some(nemesis_config::credentials::load_credentials_file(
            cred_path,
        )?)
    } else {
        None
    };

    let mut config_changed = false;
    let mut creds_changed = false;
    for mc in config.model_list.iter_mut() {
        let display_name = if mc.model_name.is_empty() {
            mc.model.clone()
        } else {
            mc.model_name.clone()
        };
        if mc.api_key.is_empty() {
            continue;
        }
        if mc.api_key.starts_with("vault:") {
            report.already_vault += 1;
            continue;
        }
        if mc.api_key.starts_with("env:") {
            report.skipped_env += 1;
            continue;
        }

        // 取出明文值：yaml 引用查表，字面量直取。
        let (literal, yaml_alias) = if let Some(alias) = mc.api_key.strip_prefix("yaml:") {
            match creds.as_ref().and_then(|c| c.keys.get(alias)) {
                Some(v) => (v.clone(), Some(alias.to_string())),
                None => {
                    // 断引用：不动配置，报给用户（fail loud 而非静默丢引用）。
                    report.yaml_broken.push((display_name, alias.to_string()));
                    continue;
                }
            }
        } else {
            (mc.api_key.clone(), None)
        };

        let base_alias = yaml_alias
            .clone()
            .unwrap_or_else(|| nemesis_config::credentials::sanitize_alias(&display_name));
        let alias = vault_alias_for(store, &base_alias, &literal, &mut report);
        store
            .set(&alias, &literal, "model", "")
            .map_err(|e| anyhow::anyhow!("写入 vault 失败（{alias}）: {e}"))?;
        mc.api_key = format!("vault:{alias}");
        config_changed = true;
        report.migrated.push((display_name, alias, "model".into()));

        if let Some(removed) = yaml_alias {
            if let Some(creds) = creds.as_mut() {
                creds.keys.remove(&removed);
                creds_changed = true;
                report.yaml_aliases_removed.push(removed);
            }
        }
    }

    if config_changed {
        nemesis_config::save_config(config_path, &mut config)
            .map_err(|e| anyhow::anyhow!("写回 config.json 失败: {e}"))?;
    }
    if creds_changed {
        if let Some(creds) = creds.as_ref() {
            nemesis_config::credentials::save_credentials_file(cred_path, creds)?;
        }
    }
    Ok(report)
}

/// vault 别名冲突裁决（run_import 同策略）：同值复用；异值 `__N` 后缀。
fn vault_alias_for(
    store: &VaultStore,
    base_alias: &str,
    literal: &str,
    report: &mut MigrateReport,
) -> String {
    if let Ok(existing) = store.get(base_alias) {
        if existing == literal {
            report.reused += 1;
            return base_alias.to_string();
        }
        let mut n = 2;
        let mut candidate = format!("{base_alias}__{n}");
        loop {
            match store.get(&candidate) {
                Ok(v) if v == literal => {
                    report.reused += 1;
                    return candidate;
                }
                Ok(_) => {
                    n += 1;
                    candidate = format!("{base_alias}__{n}");
                }
                Err(_) => {
                    report
                        .conflicts
                        .push((base_alias.to_string(), candidate.clone()));
                    return candidate;
                }
            }
        }
    }
    base_alias.to_string()
}

/// 迁移报告（CLI 输出与测试断言）。
#[derive(Debug, Default)]
struct MigrateReport {
    /// (模型名, 别名, 来源)——来源 "model"。
    migrated: Vec<(String, String, String)>,
    reused: usize,
    /// (想要的别名, 实际用的别名)——同名异值加后缀。
    conflicts: Vec<(String, String)>,
    /// (模型名, 断引别名)。
    yaml_broken: Vec<(String, String)>,
    yaml_aliases_removed: Vec<String>,
    skipped_env: usize,
    already_vault: usize,
}

impl MigrateReport {
    fn is_noop(&self) -> bool {
        self.migrated.is_empty() && self.yaml_broken.is_empty() && self.reused == 0
    }
}

/// y/N 确认（stdin，回显无妨——确认类答案非秘密）。
fn prompt_confirm(prompt: &str) -> Result<bool> {
    use std::io::Write as _;
    print!("{prompt} [y/N]: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

#[cfg(test)]
mod tests;
