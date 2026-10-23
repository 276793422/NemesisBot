//! scheduler —— poll 调度器（T2.1/T2.3）：bot 侧 interval 轮询 + on_change
//! 快照比对 + notify DSL 报警（限频）+ capture 落盘。
//!
//! 生命周期：attach 成功后按 profile.tools[] 的 `poll_report` 工具逐个
//! spawn（一工具一任务）；detach / estop 时 abort（Session.poll_tasks）。
//! payload 无状态——所有调度都在 bot 侧（T2.1 触点注释）。
//!
//! 报警限频（对齐集群 WARN 限频惯例）：同一工具在窗口内只发一次
//! （默认 60s，窗口常量 [`ALERT_RATE_LIMIT`] 单点定义）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nemesis_ziwei_profile::{DataStructure, PollMode, Primitive, Profile, ToolDef};

use crate::capture::CaptureStore;
use crate::session::{AttachError, Session, SessionManager};

/// 同一工具报警最小间隔（限频窗口）。
pub const ALERT_RATE_LIMIT: Duration = Duration::from_secs(60);

/// 紫微事件（sink 消费；gateway 侧转 bus outbound）。
#[derive(Debug, Clone)]
pub enum ZiweiEvent {
    /// notify 条件命中（on_change 触发）。
    Alert {
        profile_id: String,
        profile_name: String,
        tool: String,
        expr: String,
        snapshot: serde_json::Value,
    },
}

/// 事件汇（gateway 装配时注入；bus 出站在 gateway 侧——本 crate 不依赖 bus）。
pub type EventSink = Arc<dyn Fn(ZiweiEvent) + Send + Sync>;

/// attach + 按需调度的一站式入口（WSAPI/测试同源）。
///
/// 结构地址解析：`struct:Name` → 会话 `struct_addrs`（attach 时注入；
/// M3 起由锚自动解析）。解析失败 = 该工具诚实停用（warn 一次，不炸会话）。
pub async fn attach_and_schedule(
    manager: &Arc<SessionManager>,
    profile: &Profile,
    pid: u32,
    struct_addrs: HashMap<String, u64>,
) -> Result<Arc<Session>, AttachError> {
    let session = manager
        .attach_with_struct_addrs(profile, pid, struct_addrs)
        .await?;
    if let Some(sink) = manager.event_sink() {
        let handles = spawn_poll_tasks(manager.clone(), profile, &session, sink, manager.capture());
        let count = handles.len();
        session.set_poll_tasks(handles);
        tracing::info!(
            "[ziwei] poll 调度已启动（profile={} 任务数={count}）",
            profile.profile_id
        );
    }
    Ok(session)
}

/// 为会话按 profile spawn 全部 poll 任务（attach 成功后调用一次）。
pub fn spawn_poll_tasks(
    manager: Arc<SessionManager>,
    profile: &Profile,
    session: &Arc<Session>,
    sink: EventSink,
    capture: Arc<CaptureStore>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let _ = session; // 会话身份由 manager 按 profile_id 路由；参数保留以明确语义
    let mut handles = Vec::new();
    for tool in &profile.tools {
        if tool.action.primitive != Primitive::PollReport {
            continue;
        }
        let ds_name = match tool.action.addr.strip_prefix("struct:") {
            Some(n) => n.to_string(),
            None => continue, // validate 已拦；防御
        };
        let Some(ds) = profile
            .data_structures
            .iter()
            .find(|s| s.name == ds_name)
            .cloned()
        else {
            tracing::warn!(
                "[ziwei] 工具 {} 引用结构 {ds_name:?} 不存在——停用",
                tool.tool_name
            );
            continue;
        };
        let interval = Duration::from_secs(tool.action.interval_secs.unwrap_or(2).max(1));
        let ctx = PollCtx {
            manager: manager.clone(),
            profile_id: profile.profile_id.clone(),
            profile_name: profile.name.clone(),
            tool: tool.clone(),
            ds,
            interval,
            sink: sink.clone(),
            capture: capture.clone(),
        };
        handles.push(tokio::spawn(poll_loop(ctx)));
    }
    handles
}

/// 单工具轮询上下文（任务内自有；无共享可变态）。
struct PollCtx {
    manager: Arc<SessionManager>,
    profile_id: String,
    profile_name: String,
    tool: ToolDef,
    ds: DataStructure,
    interval: Duration,
    sink: EventSink,
    capture: Arc<CaptureStore>,
}

/// poll 主循环（on_change / every 两模式；出错退避一个 interval 续跑）。
async fn poll_loop(ctx: PollCtx) {
    let PollCtx {
        manager,
        profile_id,
        profile_name,
        tool,
        ds,
        interval,
        sink,
        capture,
    } = ctx;
    let Some(addr) = resolve_struct_addr(&manager, &profile_id, &tool) else {
        return; // 停用：warn 已发
    };
    let on_change = tool.action.mode.unwrap_or(PollMode::OnChange) == PollMode::OnChange;
    let mut last: Option<serde_json::Value> = None;
    let mut last_alert: Option<Instant> = None;
    loop {
        tokio::time::sleep(interval).await;
        let v = match manager.read_struct(&profile_id, addr, &ds).await {
            Ok(v) => v,
            Err(e) => {
                // 会话没了（detach 中）→ 静默退场；瞬时读错 → 退避续跑
                if manager.get(&profile_id).is_none() {
                    return;
                }
                tracing::debug!("[ziwei] poll {} 读失败（退避续跑）：{e}", tool.tool_name);
                continue;
            }
        };
        // 首读 = 基线（不触发）；之后比对
        let baseline = last.is_none();
        let changed = !baseline && last.as_ref() != Some(&v);
        last = Some(v.clone());
        if on_change && !changed {
            continue;
        }
        // 变化（或 every 模式）→ capture 回流
        let _ = capture.append(
            &profile_id,
            serde_json::json!({ "tool": tool.tool_name, "kind": "poll", "data": v }),
        );
        // 报警（限频：窗口内同工具只发一次）
        if let Some(notify) = &tool.action.notify {
            let hit = crate::notify::matches(&notify.on, &v);
            let rate_ok = last_alert
                .map(|t| t.elapsed() >= ALERT_RATE_LIMIT)
                .unwrap_or(true);
            if hit && rate_ok {
                last_alert = Some(Instant::now());
                sink(ZiweiEvent::Alert {
                    profile_id: profile_id.clone(),
                    profile_name: profile_name.clone(),
                    tool: tool.tool_name.clone(),
                    expr: notify.on.clone(),
                    snapshot: v.clone(),
                });
            }
        }
    }
}

/// 解析工具的结构地址（会话注入映射；缺失/为零 = None + 一次性 warn）。
fn resolve_struct_addr(manager: &SessionManager, profile_id: &str, tool: &ToolDef) -> Option<u64> {
    let name = tool.action.addr.strip_prefix("struct:")?;
    let session = manager.get(profile_id)?;
    let addr = session.struct_addr(name)?;
    if addr == 0 {
        tracing::warn!(
            "[ziwei] 工具 {} 结构 {name:?} 地址未解析（attach 未提供）——停用",
            tool.tool_name
        );
        return None;
    }
    Some(addr)
}

/// 解析 profile 全部 poll 工具引用的结构名集合（attach 入参校验用）。
pub fn referenced_structs(profile: &Profile) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tool in &profile.tools {
        if tool.action.primitive != Primitive::PollReport {
            continue;
        }
        if let Some(n) = tool.action.addr.strip_prefix("struct:")
            && !out.iter().any(|e| e == n)
        {
            out.push(n.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests;
