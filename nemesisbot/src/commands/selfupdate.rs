//! `nemesisbot selfupdate` — L9 版本自切换通道 CLI。
//!
//! 三臂：
//! - `check`  ：直接查 Release 最新产物 vs 本进程 commit 身份（不需要 gateway
//!   在跑——版本身份 = git commit，编译期注入）。
//! - `status` ：本进程身份 + workspace journal 现场 + 本地待装包（不需要
//!   gateway）。
//! - `apply`  ：触发**正在跑的 gateway** 走两段式 apply（POST /api/internal
//!   `selfupdate_apply`）——待装包就位/v4 验签/staging/受控重启全在 gateway
//!   侧同源执行（无待装包自动先下载），CLI 只受理；成功后 gateway 自行走
//!   优雅停机，新版本进程已在等它退场。`--allow-downgrade` 放行回退（默认拒）。
//!
//! apply 的本地快检（journal 未收口）与 gateway 同判据（同一组 pub fn），只
//! 挡明显打空枪——真闸在 gateway 侧重跑一遍。

use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Subcommand;

use crate::common;

#[derive(Subcommand, Debug)]
pub enum SelfupdateCommand {
    /// Check the latest Release asset against the current build's commit.
    Check,
    /// Apply the update via a RUNNING gateway (downloads if no pending
    /// package; v4 signature verification happens gateway-side).
    Apply {
        /// Allow switching to an OLDER commit than the current one.
        #[arg(long)]
        allow_downgrade: bool,
    },
    /// Show current identity + pending journal / local pending package.
    Status,
}

pub async fn run(cmd: SelfupdateCommand, local: bool) -> Result<()> {
    match cmd {
        SelfupdateCommand::Check => run_check().await,
        SelfupdateCommand::Apply { allow_downgrade } => {
            let home = common::resolve_home(local);
            run_apply(&common::workspace_path(&home), allow_downgrade).await
        }
        SelfupdateCommand::Status => {
            let home = common::resolve_home(local);
            run_status(&common::workspace_path(&home))
        }
    }
}

async fn run_check() -> Result<()> {
    let version = common::VERSION_INFO.version;
    let commit = common::VERSION_INFO.git_commit;
    if commit.is_empty() {
        println!("Current build: {version} (dev build — commit unknown)");
    } else {
        println!("Current build: {version} (commit {commit})");
    }
    let c = nemesis_web::selfupdate::check(version, commit)
        .await
        .map_err(anyhow::Error::msg)
        .context("查询 Release 元数据失败")?;
    println!("Latest release: {} ({})", c.latest_name, c.latest_tag);
    println!("Target asset:   {}", c.asset);
    if !c.asset_present {
        bail!(
            "目标产物 {} 尚未出现在最新 Release——等下一轮 nightly。",
            c.asset
        );
    }
    let lc = c.latest_commit.as_str();
    if !lc.is_empty() && c.current_commit_known {
        if nemesis_web::selfupdate::commit_matches(commit, lc) {
            println!("Up to date: 当前 commit 已是最新。");
        } else {
            println!("Update available: {commit} → {lc}");
        }
    } else if !lc.is_empty() {
        println!("Update target commit: {lc}（当前身份未知，apply 可行）。");
    } else {
        println!("Release 元数据不含 commit 身份——apply 在解包后按 version.txt 实核。");
    }
    Ok(())
}

fn run_status(workspace: &Path) -> Result<()> {
    let version = common::VERSION_INFO.version;
    let commit = common::VERSION_INFO.git_commit;
    println!(
        "Current build: {version} (commit {})",
        if commit.is_empty() { "<dev>" } else { commit }
    );
    println!("Target asset:  {}", nemesis_web::selfupdate::TARGET_ASSET);
    match nemesis_web::selfupdate::load_journal(workspace) {
        Some(j) => {
            println!(
                "Update journal: phase={} target={}",
                j.phase, j.target_commit
            );
            println!("  ⚠ 有未收口的更新事务——下次 gateway 启动会自动收口（成功确认或回滚）。");
            println!(
                "  现场：{}",
                nemesis_web::selfupdate::journal_path(workspace).display()
            );
        }
        None => println!("Update journal: 无未竟事务。"),
    }
    match std::env::current_exe()
        .ok()
        .and_then(|exe| nemesis_web::selfupdate::load_pending(&exe))
    {
        Some(p) => {
            println!(
                "Pending package: {}（commit {}，{}MB，下载于 {}）",
                p.release_tag,
                if p.target_commit.is_empty() {
                    "<unknown>"
                } else {
                    &p.target_commit[..p.target_commit.len().min(12)]
                },
                p.size_bytes / (1024 * 1024),
                chrono_fmt_ms(p.downloaded_at_ms),
            );
            println!("  就绪可装——gateway 侧 apply（或 Dashboard【开始更新】）即执行。");
        }
        None => println!("Pending package: 无待装包（updates/ 干净）。"),
    }
    Ok(())
}

async fn run_apply(workspace: &Path, allow_downgrade: bool) -> Result<()> {
    // 前置快检（与 gateway 同判据）：journal 未收口，本地直接拒——不占用
    // gateway 的单飞闸。
    if let Some(j) = nemesis_web::selfupdate::load_journal(workspace) {
        bail!(
            "有未收口的更新事务（phase={}）——先重启 gateway 让启动收口跑完（成功确认或回滚），再 apply。",
            j.phase
        );
    }

    // 找到正在跑的 gateway（gateway state 文件 + 健康检查，estop 同款）。
    let state_path = nemesis_path::resolve_gateway_state_path_in_workspace(workspace);
    let info = crate::commands::dashboard::read_gateway_state(&state_path).ok_or_else(|| {
        anyhow::anyhow!("Gateway 未运行（找不到 state 文件）。先用 `nemesisbot gateway` 启动它。")
    })?;
    if info.web_port <= 0 {
        anyhow::bail!("Gateway state 无效（web_port=0）。");
    }
    let base_url = format!("http://{}:{}", info.web_host, info.web_port);
    crate::commands::dashboard::check_health(&base_url)
        .await
        .map_err(|e| anyhow::anyhow!("Gateway 在 {base_url} 不可达：{e}"))?;

    let auth_token = read_auth_token(workspace)?;
    let mut body = serde_json::json!({ "cmd": "selfupdate_apply" });
    if allow_downgrade {
        body["allow_downgrade"] = serde_json::json!(true);
    }
    post_internal(&base_url, &auth_token, &body).await?;
    println!("✓ apply 已受理（gateway 侧两段式编排已启动；无待装包会自动先下载）。");
    println!("  执行进度看 gateway 日志与审计链；v4 签名验签是安全闸。");
    println!("  成功后 gateway 自动优雅重启为新版本。");
    Ok(())
}

/// /api/internal 的 X-Auth-Token：读与 gateway 同一 home 的 config.json
/// （workspace 的父目录即 home——apply 需要与 gateway 相同的 token 视图）。
fn read_auth_token(workspace: &Path) -> Result<String> {
    let home = workspace
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("无法从 workspace 反推 home：{}", workspace.display()))?;
    let cfg_path = common::config_path(&home);
    let data = std::fs::read_to_string(&cfg_path)
        .with_context(|| format!("读取 {}", cfg_path.display()))?;
    let cfg: serde_json::Value =
        serde_json::from_str(&data).with_context(|| format!("解析 {}", cfg_path.display()))?;
    Ok(cfg
        .pointer("/channels/web/auth_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

async fn post_internal(base_url: &str, token: &str, body: &serde_json::Value) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("HTTP client 构建失败")?;
    let url = format!("{base_url}/api/internal");
    let mut req = client.post(&url).json(body);
    // 未配置 token（空）→ 不发 header；服务端对空 auth_token 放行（既有约定）。
    if !token.is_empty() {
        req = req.header("X-Auth-Token", token);
    }
    let resp = req.send().await.context("POST /api/internal 失败")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("gateway 拒绝（HTTP {status}）：{text}");
    }
    Ok(())
}

/// epoch ms → 本地时间短格式（status 展示用）。
fn chrono_fmt_ms(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| format!("{ms}ms"))
}
