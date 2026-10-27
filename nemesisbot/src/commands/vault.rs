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
                "{:<28} {:<14} {:<24} {:<24} DESCRIPTION",
                "ALIAS", "DOMAIN", "CREATED", "ROTATED"
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

/// P0 B2（2026-09-22 计划 §2.4）：模型 key 迁移——CLI 壳。
///
/// 迁移核心已下沉 `nemesis_security::vault::migrate_model_keys_to_vault`
/// （raw Value 单一真相源，2026-10-27 与 WSAPI `models.migrate_keys` 共用）；
/// 本壳只负责定位文件、装配 creds、按顺序不变量落盘：
///
/// 持久化顺序不变量：**vault 落盘先于 config 回写**——迁移的本质是
/// 「明文 key 换 `vault:<alias>`」，若先剥离明文再落 vault，「config 回写
/// 成功 → vault 落盘失败/进程中断」窗口内盘上就是断链态（配置指
/// `vault:<alias>` 但 vault 文件没有该别名，明文已离盘）。与
/// credential_broker 的「先落 vault 再送裁决」、chat_secrets 的
/// persist-then-swap 是同一条纪律。
fn migrate_model_keys(
    config_path: &std::path::Path,
    cred_path: &std::path::Path,
    store: &mut VaultStore,
) -> anyhow::Result<nemesis_security::vault::VaultMigrateReport> {
    if !config_path.exists() {
        return Ok(Default::default());
    }
    // Raw JSON round-trip（非 typed）：未类型化 extras（tier/protocol/stream/
    // …）按字节保真，迁移只动 api_key 一个字段——与 nemesis-web
    // update_raw_config 同语义。
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| anyhow::anyhow!("读取 config.json 失败: {e}"))?;
    let mut config: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| anyhow::anyhow!("解析 config.json 失败: {e}"))?;
    let mut creds = if cred_path.exists() {
        Some(nemesis_config::credentials::load_credentials_file(
            cred_path,
        )?)
    } else {
        None
    };

    let report =
        nemesis_security::vault::migrate_model_keys_to_vault(&mut config, store, creds.as_mut())
            .map_err(|e| anyhow::anyhow!("vault 写入失败: {e}"))?;

    if report.config_changed() {
        // 先落 vault 再回写配置（顺序不变量见函数头注）——vault 落盘失败
        // 时明文 key 还在 config/yaml 原地，重跑迁移即可，零损失。
        store.save().map_err(|e| {
            anyhow::anyhow!("保存 vault 失败（明文 key 未离盘，修复后重跑即可）: {e}")
        })?;
        let out = serde_json::to_string_pretty(&config)
            .map_err(|e| anyhow::anyhow!("序列化 config.json 失败: {e}"))?;
        nemesis_utils::write_file_atomic(&config_path.to_string_lossy(), out.as_bytes(), 0o600)
            .map_err(|e| anyhow::anyhow!("写回 config.json 失败: {e}"))?;
    }
    if report.creds_changed {
        nemesis_config::credentials::save_credentials_file(
            cred_path,
            creds.as_ref().expect("creds_changed 蕴含 creds 在场"),
        )?;
    }
    Ok(report)
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
