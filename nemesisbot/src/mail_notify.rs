//! 邮件通知钩子（B3 消费点静态槽；plugins_wasm `set_tool_sync_hook` 同款
//! 静态槽形态——消费点与插件装载解耦，gateway 装配期安装一次）。
//!
//! **两消费点**（计划 §6 notify 面）：
//! - **estop 急停通知**：gateway 装配期 spawn estop watch 订阅任务——
//!   `estop.trigger` 触发发信（estop 中照发——邮件基础设施不是 agent 活动
//!   面，契约注释同文；四入口 CLI/托盘/Dashboard/WSAPI 全走同一
//!   `EstopState`，watch 订阅即全覆盖）；
//! - **board 预算熔断通知**：`board_review` 停车路径（同步上下文）——
//!   [`try_notify`] 异步发信不阻塞停车主流程。
//!
//! **配置**（config.mail.json `notify` 段）：
//! ```json
//! "notify": { "on": ["estop", "board_budget"], "to": "ops@corp.example" }
//! ```
//! - `notify` 缺段 / `on` 空数组 = 消费点全关（缺省静默，零行为差异）；
//! - `to` 空 = 安装时 warn 跳过（发信需收件人——不知发给谁的通知是噪音）；
//! - `on` 白名单语义：kind 不在 `on` 内 = 该消费点静默跳过。
//!
//! 发信失败 = tracing warn（通知是尽力而为——急停/熔断主流程不被邮件
//! 阻塞；审计链另有记录）。插件未装载时 `call_mail` 诚实报错 → 同款 warn。

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use parking_lot::RwLock;

/// 通知钩子槽：参数 = (kind, subject, body)。
type MailNotifyHook = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;
static HOOK: RwLock<Option<MailNotifyHook>> = RwLock::new(None);

/// gateway 装配期安装（post_agent 注入块调用；幂等——OnceLock 风格首写，
/// 重复安装覆盖旧钩子，gateway 进程内只调一次）。
pub fn install(home: &Path) {
    let Some((kinds, to)) = crate::mail_plugin_loader::mail_notify_section(home) else {
        return; // notify 缺段 = 全关（缺省静默）
    };
    if to.is_empty() {
        if !kinds.is_empty() {
            tracing::warn!(
                "[mail_notify] notify.on 已配置但 notify.to 为空——通知跳过（发信需收件人）"
            );
        }
        return;
    }
    let enabled: HashSet<String> = kinds.into_iter().collect();
    if enabled.is_empty() {
        return;
    }
    tracing::info!("[mail_notify] 已安装（on={:?} → {to}）", {
        let mut v: Vec<_> = enabled.iter().map(String::as_str).collect();
        v.sort_unstable();
        v
    });
    *HOOK.write() = Some(Arc::new(move |kind: &str, subject: &str, body: &str| {
        if !enabled.contains(kind) {
            return; // 白名单外静默跳过
        }
        let kind = kind.to_string();
        let to = to.clone();
        let subject = subject.to_string();
        let body = body.to_string();
        // 异步发信：消费点可能是同步上下文（board 停车路径）——经
        // Handle::try_current 兜底（无 runtime = 诚实 warn，不炸调用方）。
        let spawn_result = match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                let kind_for_task = kind.clone();
                rt.spawn(async move {
                    let kind = kind_for_task;
                    // 副本进调用闭包（FnOnce 消费后正本仍可进日志）。
                    let (to2, subj2, body2) = (to.clone(), subject.clone(), body.clone());
                    match crate::mail_plugin_loader::call_mail("mail_notify", move |h| {
                        h.send(&to2, &subj2, &body2)
                    })
                    .await
                    {
                        Ok(_) => tracing::info!("[mail_notify] [{kind}] 通知已发送 → {to}"),
                        Err(e) => tracing::warn!("[mail_notify] [{kind}] 发送失败: {e}"),
                    }
                });
                Ok(())
            }
            Err(_) => Err(()),
        };
        if spawn_result.is_err() {
            tracing::warn!("[mail_notify] [{kind}] 无 tokio runtime 上下文，通知跳过");
        }
    }));
}

/// 消费点入口（estop watch 任务 / board 熔断路径）：钩子未安装或 kind 不在
/// `on` 白名单 = 静默 no-op（零配置形态零行为差异）。
pub fn try_notify(kind: &str, subject: &str, body: &str) {
    let hook = HOOK.read().clone();
    if let Some(h) = hook {
        h(kind, subject, body);
    }
}

#[cfg(test)]
mod tests;
