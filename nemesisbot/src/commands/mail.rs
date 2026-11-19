//! Mail command — 邮件基础设施 CLI（B3；feature=mail 门控）。
//!
//! 四族：`status`（三态只读）/ `list`（列信）/ `send`（发信）/ `addr`
//! （地址注册制 add|remove|list）。读写信面在 CLI 进程内自装插件
//! （load_mail_plugin——**不调 server_start**，CLI 不绑端口；gateway 重启
//! 后服务端面照常）。总闸/名册关 = 诚实报错不静默。

use crate::common;
use crate::mail_plugin_loader::{self, MailLoadOutcome};
use anyhow::{Result, anyhow};
use serde_json::json;

#[derive(clap::Subcommand)]
pub enum MailAction {
    /// Status: DLL presence / master switch / roster / handle (read-only)
    Status,
    /// List messages (optionally filtered)
    List {
        /// Mailbox (registered address); empty = default mailbox
        #[arg(long, default_value = "")]
        mailbox: String,
        /// Unread only
        #[arg(long, default_value_t = false)]
        unread: bool,
        /// Max items
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// Skip offset
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// Send mail (body from --body or stdin with --stdin)
    Send {
        /// Recipient address
        #[arg(long)]
        to: String,
        /// Subject
        #[arg(long, default_value = "")]
        subject: String,
        /// Body (ignored with --stdin)
        #[arg(long, default_value = "")]
        body: String,
        /// Read body from stdin instead of --body
        #[arg(long, default_value_t = false)]
        stdin: bool,
    },
    /// Address registry management
    Addr {
        #[command(subcommand)]
        action: AddrAction,
    },
}

#[derive(clap::Subcommand)]
pub enum AddrAction {
    /// Register an address
    Add { address: String },
    /// Unregister an address
    Remove { address: String },
    /// List registered addresses
    List,
}

/// CLI 进程内自装插件（不 server_start——CLI 不绑端口）。总闸/名册关/
/// 缺席/拒载 = 诚实报错。
fn ensure_loaded(local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    if !mail_plugin_loader::mail_enabled(&home) {
        return Err(anyhow!(
            "邮件总闸未开启：config.mail.json 写 \"enabled\": true 后重试"
        ));
    }
    let config_dir = nemesis_path::workspace_config_dir(&common::workspace_path(&home));
    match mail_plugin_loader::load_mail_plugin(&home, &config_dir) {
        MailLoadOutcome::Loaded => Ok(()),
        MailLoadOutcome::Absent => Err(anyhow!(
            "plugin_mail 缺席（exe 旁 plugins/ 下无 plugin_mail 库文件）"
        )),
        MailLoadOutcome::DisabledBySwitch => Err(anyhow!(
            "plugin_mail 已停用（config.plugin.native.json 名册开关）"
        )),
        MailLoadOutcome::Rejected(r) => Err(anyhow!("plugin_mail 拒载：{r}")),
    }
}

pub async fn run(action: MailAction, local: bool) -> Result<()> {
    match action {
        MailAction::Status => {
            let home = common::resolve_home(local);
            let present = mail_plugin_loader::mail_plugin_present();
            let enabled = mail_plugin_loader::mail_enabled(&home);
            let config_dir = nemesis_path::workspace_config_dir(&common::workspace_path(&home));
            let roster = nemesis_config::plugins_native::load_enabled(
                &config_dir,
                nemesis_config::plugins_native::NativePluginId::PluginMail,
            );
            println!("插件文件在场: {present}");
            println!("总闸 (config.mail.json enabled): {enabled}");
            println!("名册开关 (config.plugin.native.json): {roster}");
            if enabled {
                match mail_plugin_loader::load_mail_plugin(&home, &config_dir) {
                    MailLoadOutcome::Loaded => {
                        let st = mail_plugin_loader::call_mail(
                            "mail_server_status",
                            nemesis_plugin::mail::MailPluginHandle::server_status,
                        )
                        .await
                        .unwrap_or_else(|e| json!({ "error": e }));
                        println!("服务端: {st}");
                    }
                    MailLoadOutcome::Absent => println!("服务端: 未装配（插件缺席）"),
                    MailLoadOutcome::DisabledBySwitch => println!("服务端: 未装配（名册开关停用）"),
                    MailLoadOutcome::Rejected(r) => println!("服务端: 未装配（拒载：{r}）"),
                }
            } else {
                println!("服务端: 未装配（总闸关）");
            }
        }
        MailAction::List {
            mailbox,
            unread,
            limit,
            offset,
        } => {
            ensure_loaded(local)?;
            let q = json!({
                "mailbox": mailbox,
                "unread_only": unread,
                "limit": limit,
                "offset": offset,
            });
            let v = mail_plugin_loader::call_mail("mail.list", move |h| h.list(&q.to_string()))
                .await
                .map_err(|e| anyhow!("{e}"))?;
            let items = v
                .get("items")
                .and_then(|i| i.as_array())
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                println!("（空）");
                return Ok(());
            }
            for it in items {
                let id = it.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let from = it.get("from").and_then(|v| v.as_str()).unwrap_or("?");
                let subject = it.get("subject").and_then(|v| v.as_str()).unwrap_or("");
                let date = it.get("date").and_then(|v| v.as_str()).unwrap_or("");
                let seen = it.get("seen").and_then(|v| v.as_bool()).unwrap_or(false);
                let flag = if seen { "  " } else { "● " };
                println!("{flag}[{id}] {from} | {date} | {subject}");
            }
        }
        MailAction::Send {
            to,
            subject,
            body,
            stdin,
        } => {
            ensure_loaded(local)?;
            let body = if stdin {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf
            } else {
                body
            };
            let v =
                mail_plugin_loader::call_mail("mail.send", move |h| h.send(&to, &subject, &body))
                    .await
                    .map_err(|e| anyhow!("{e}"))?;
            println!("{v}");
        }
        MailAction::Addr { action } => match action {
            AddrAction::Add { address } => {
                ensure_loaded(local)?;
                let a = address.clone();
                let v = mail_plugin_loader::call_mail("mail.addr_add", move |h| h.addr_add(&a))
                    .await
                    .map_err(|e| anyhow!("{e}"))?;
                println!("{v}");
            }
            AddrAction::Remove { address } => {
                ensure_loaded(local)?;
                let a = address.clone();
                let v =
                    mail_plugin_loader::call_mail("mail.addr_remove", move |h| h.addr_remove(&a))
                        .await
                        .map_err(|e| anyhow!("{e}"))?;
                println!("{v}");
            }
            AddrAction::List => {
                ensure_loaded(local)?;
                let v = mail_plugin_loader::call_mail("mail.addr_list", |h| h.addr_list())
                    .await
                    .map_err(|e| anyhow!("{e}"))?;
                let addrs = v
                    .get("addresses")
                    .and_then(|i| i.as_array())
                    .cloned()
                    .unwrap_or_default();
                let default_addr = v
                    .get("default")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string();
                if addrs.is_empty() {
                    println!("（未注册任何地址）");
                }
                for a in addrs {
                    let s = a.as_str().unwrap_or("?");
                    let mark = if s == default_addr {
                        "（默认）"
                    } else {
                        ""
                    };
                    println!("{s}{mark}");
                }
            }
        },
    }
    Ok(())
}
