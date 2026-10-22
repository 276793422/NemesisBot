//! L9 版本自切换通道工具面：`selfupdate_check` / `selfupdate_apply`。
//!
//! 职责（设计文档 docs/INFO/2026-10-12_LLM自更新通道设计方案.md §四 工具面；
//! 2026-10-21 两段式改版——审批卡退役）：
//! - `selfupdate_check`：只读查询最新 Release vs 本进程 commit 身份——LLM 可
//!   自主调用（NetworkRequest 档，与 web_fetch 同源管线，默认无审批摩擦）；
//! - `selfupdate_apply`：触发两段式 apply——本工具受理即返回，安全闸是编排
//!   内部的 v4 签名验签（Valid-only，编译期根锚）+ sequence 单调闸 + estop +
//!   单飞锁；Dashboard 确认框 / CLI 受理即主体意志表达。无待装包时编排自动
//!   先下载。
//!
//! 安全形态：
//! - check 声明 `NetworkRequest`、apply 声明 `SystemInstall`（诚实分类——
//!   装/切二进制 + 受控重启网关；不声明会走未知名 fail-closed，行为等价但
//!   每调一次 warn 一次，显式声明去噪 + 分类明示，codemode 同款理由）；
//! - apply 经 manager 槽（`handlers::selfupdate::manager()`，gateway 装配点
//!   注入）——None = 未装配诚实报错；estop/journal/验签全在编排内
//!   重跑，工具层不复制判据（单一真相源）；
//! - 主 loop 独占：项目 loop（L6++）不该重启整个 gateway 杀掉兄弟项目；
//!   集群 B 端 worker 是任务执行器，不参与版本治理。
//!
//! 注册面：`agent_factory.rs::build_agent_loop`（主 loop 专属，codemode 块
//! 同位）；编译闸 = `selfupdate` feature。tier 供给面走白名单天然闸
//! （mini/normal 白名单不含本工具，仅 big 全量可见）。

use nemesis_agent::context::RequestContext;

/// 只读检查工具注册名（loop::Tool 无 name()；注册键即 LLM 可见名）。
pub const CHECK_TOOL_NAME: &str = "selfupdate_check";
/// apply 提案工具注册名。
pub const APPLY_TOOL_NAME: &str = "selfupdate_apply";

/// 当前构建身份（version + 短 commit，编译期注入）。
fn identity() -> (&'static str, &'static str) {
    (
        crate::common::VERSION_INFO.version,
        crate::common::VERSION_INFO.git_commit,
    )
}

// ---------------------------------------------------------------------------

/// `selfupdate_check` — 只读：最新 Release 产物 vs 当前 commit 身份。
pub struct SelfupdateCheckTool;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for SelfupdateCheckTool {
    fn description(&self) -> String {
        "Check whether a newer NemesisBot build is available on the release \
         channel. Read-only: compares the latest release artifact's commit \
         identity against the current build. No download, no restart."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        // 容忍空 args / {}（无参工具：LLM 有时传 "{}" 或 ""）。
        let _ = serde_json::from_str::<serde_json::Value>(args)
            .or_else(|_| serde_json::from_str::<serde_json::Value>("{}"));
        let (version, commit) = identity();
        let c = nemesis_web::selfupdate::check(version, commit).await?;
        let summary = if !c.asset_present {
            format!(
                "No usable artifact yet: target asset '{}' is absent from the latest release.",
                c.asset
            )
        } else if c.update_available {
            if c.current_commit_known && !c.latest_commit.is_empty() {
                format!(
                    "Update available: {} -> {}.",
                    c.current_commit, c.latest_commit
                )
            } else {
                "Update likely available (current identity unknown or release body lacks commit)."
                    .to_string()
            }
        } else {
            "Up to date: current build matches the latest release.".to_string()
        };
        let mut out = serde_json::to_value(&c).map_err(|e| e.to_string())?;
        out["summary"] = serde_json::json!(summary);
        Ok(out.to_string())
    }
}

// ---------------------------------------------------------------------------

/// `selfupdate_apply` — 触发两段式 apply（v4 验签是安全闸；受理即执行）。
pub struct SelfupdateApplyTool;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for SelfupdateApplyTool {
    fn description(&self) -> String {
        "Trigger a version self-update: runs the two-phase update \
         orchestration (download if no pending package, v4 signature \
         verification, staging, controlled restart). Accepted immediately \
         and executed in the background — signature verification is the \
         security gate, no interactive approval happens gateway-side. Use \
         selfupdate_check first to see whether an update exists; the update \
         restarts the gateway, which interrupts this session."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "allow_downgrade": {
                    "type": "boolean",
                    "description": "Allow switching to an OLDER commit than the current one (default false; updates refuse downgrade without this)."
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let allow_downgrade = serde_json::from_str::<serde_json::Value>(args)
            .unwrap_or(serde_json::json!({}))
            .get("allow_downgrade")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mgr = nemesis_web::handlers::selfupdate::manager().ok_or_else(|| {
            "selfupdate 编排器未装配（gateway 装配未完成或构建未带 selfupdate feature）".to_string()
        })?;
        let (version, commit) = identity();
        tokio::spawn(async move {
            match mgr.apply(version, commit, allow_downgrade).await {
                Ok(msg) => tracing::info!("[SelfUpdate] agent-tool apply: {msg}"),
                Err(e) => tracing::warn!("[SelfUpdate] agent-tool apply 失败：{e}"),
            }
        });
        Ok(serde_json::json!({
            "accepted": true,
            "note": "apply 已受理（后台编排；无待装包自动先下载）：\
                     v4 签名验签是安全闸，结果看 gateway 日志与审计链。\
                     成功后 gateway 受控重启为新版本——\
                     本会话会随重启中断，重启后 journal 启动收口自动确认。"
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests;
