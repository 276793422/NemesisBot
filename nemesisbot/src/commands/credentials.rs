//! `nemesisbot credentials` — model API key 迁移（U15）。
//!
//! 注意：这里管理的是 **模型 API key** 的 yaml 引用存储
//! （`workspace/config/credentials.yaml`），与 `nemesisbot auth` 的
//! OAuth 凭据存储（auth.rs "stored credentials"）语义不同，勿混淆。

use crate::common;
use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum CredentialsAction {
    /// Migrate inline plaintext model API keys from config.json into
    /// workspace/config/credentials.yaml (0600) and rewrite them as
    /// `yaml:<alias>` references. Idempotent — existing aliases are never
    /// overwritten.
    Import,
}

pub async fn run(action: CredentialsAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let config_path = home.join("config.json");
    let cred_path = nemesis_config::credentials::credentials_path_for_home(&home);

    match action {
        CredentialsAction::Import => {
            // Point this process's `yaml:` resolution at the same file so a
            // follow-up `model` command in the same shell already resolves.
            nemesis_config::credentials::set_global_credentials_path(cred_path.clone());

            let report = nemesis_config::credentials::run_import(&config_path, &cred_path)?;

            println!("模型 API key 迁移（注意：这是模型 API key 的 yaml 引用迁移，");
            println!("与 `nemesisbot auth` 的 OAuth 凭据存储无关）");
            println!("  config.json : {}", config_path.display());
            println!("  credentials : {}", cred_path.display());
            if report.is_noop() {
                println!();
                println!("没有需要迁移的明文 key（config.json 缺失或全部已是引用/空值）。");
                return Ok(());
            }
            println!();
            println!("迁移 {} 个明文 key：", report.migrated.len());
            for (name, alias) in &report.migrated {
                println!("  {} -> yaml:{}", name, alias);
            }
            if report.reused > 0 {
                println!("复用已有同值 alias：{} 个", report.reused);
            }
            for (want, used) in &report.conflicts {
                println!(
                    "警告：alias「{}」已存在且值不同，改用「{}」（原值未覆盖）",
                    want, used
                );
            }
            if report.skipped_reference > 0 || report.skipped_empty > 0 {
                println!(
                    "跳过：{} 个已是引用、{} 个空 key",
                    report.skipped_reference, report.skipped_empty
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
