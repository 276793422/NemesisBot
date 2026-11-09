//! 人工触点 responder 漏斗 + 超时 sweep + 恢复钩子槽（看板人工触点体系 P1）。
//!
//! 设计文档：`docs/PLAN/2026-11-07_board-human-touchpoints.md` §4/§5/§6.4/§6.5。
//! 三公理落地：
//! ① **机器永不死锁**——恢复钩子未装配时漏斗诚实降级（收口 + warn，不留
//!    pending 死锁面）；sweep 锚定绝对 deadline（estop 冻结不白等）。
//! ② **关门不锁门**——unattended 超时按默认动作收口 + 系统评论，迟到
//!    输入走评论进变更管理（触点终态不重开）。
//! ③ **否决 = 注入方向**——同范围否决轮数达 `veto_max_rounds` 熔断
//!    `parked_sync`（「需同步沟通」），veto_max_rounds<=0 = 熔断关闭。
//!
//! 本模块只做**漏斗**（状态机/审计/通知/单据落账）。挂卡方（P2 规格蒸馏
//! / P3 原型里程碑）经 [`create_touchpoint_card`] 进场；确认/超时后的
//! 机器续跑经 [`install_touchpoint_resume_hook`] 接管（P2/P3 装配）。
//!
//! checkpoint 模式 = 死等语义（计划 §4 唯一例外）：等待窗到期后不默认
//! 发车、转入同步沟通等待（通报一次），硬顶 `blocking_cap_secs` 到顶才
//! 按默认动作收口。unattended 模式（默认）：等待窗到期即按默认收口。

use std::sync::{Arc, OnceLock};

use nemesis_board::assignment::Actor;
use nemesis_board::models::{
    CommentType, IssueStatus, NewComment, NewNotification, Touchpoint, TouchpointKind,
    TouchpointStatus, notification_kind,
};
use nemesis_board::touchpoint::{TouchpointAuditEvent, default_action_for};
use nemesis_config::{AttendanceMode, BoardAttendanceConfig};

use super::*;

/// T+10min 提醒拍（挂卡后仍未处理触发一次；audit `remind` 幂等去重）。
const REMIND_AFTER_MS: i64 = 600_000;

// ---------------------------------------------------------------------------
// 恢复钩子槽（P2/P3 装配；未装配 = 诚实降级，机器不停摆）
// ---------------------------------------------------------------------------

/// 恢复载荷（确认/超时默认后触发一次；挂卡方经 hook 续跑机器链）。
#[derive(Debug, Clone)]
pub struct TouchpointResume {
    /// 承载触点单的 issue id（P2 恢复规划链的锚点）。
    pub issue_id: i64,
    pub touchpoint_id: i64,
    pub kind: TouchpointKind,
    /// `confirm`（人工确认）| `timeout_default`（超时/静音按默认动作）|
    /// `veto_local`（原型局部返工否决 → 证据回灌重派）。
    pub action: &'static str,
    /// 人工输入 JSON（确认 = 落账原文；超时 = `{"default_action":...}`）。
    pub human_input: String,
}

pub type TouchpointResumeFn = dyn Fn(&TouchpointResume) -> Result<(), String> + Send + Sync;

static TOUCHPOINT_RESUME_HOOK: OnceLock<Arc<TouchpointResumeFn>> = OnceLock::new();

/// 装配恢复钩子（install-once；P2/P3 在 gateway 装配点注入。重复装配
/// = false 不覆盖——先装者赢，防双写）。
pub fn install_touchpoint_resume_hook(hook: Arc<TouchpointResumeFn>) -> bool {
    TOUCHPOINT_RESUME_HOOK.set(hook).is_ok()
}

/// 触发恢复钩子（fire-and-forget 语义由钩子实现方负责；本侧只透传）。
/// 未装配 = warn 降级：触点已收口、单据已流转，机器链续跑缺席是 P1
/// 底座的诚实形态，不留等待死锁面。
fn fire_resume(r: &TouchpointResume) {
    match TOUCHPOINT_RESUME_HOOK.get() {
        Some(hook) => {
            if let Err(e) = hook(r) {
                // 钩子同步失败不回滚已提交的触点状态（确认已落账）；
                // 续跑链自己的失败路径自兜底（P2 接线点负责）。
                tracing::warn!(
                    "[Touchpoint] resume hook 同步失败 tp={} issue={} action={}: {e}",
                    r.touchpoint_id,
                    r.issue_id,
                    r.action
                );
            }
        }
        None => tracing::warn!(
            "[Touchpoint] resume hook 未装配（P2/P3 接线），tp={} issue={} action={} 收口但不续跑",
            r.touchpoint_id,
            r.issue_id,
            r.action
        ),
    }
}

// ---------------------------------------------------------------------------
// 时钟与换算（触点族时间统一毫秒口径，见 store::now_ms 注释）
// ---------------------------------------------------------------------------

/// epoch 毫秒（漏斗侧时钟；store 触点行同口径）。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// live config → `board.attendance` 段（全局 ConfigStore 单例；测试/CLI
/// 无单例 → 回落全默认——onboard 前零配置形态即用，与「无人值守默认态」
/// 语义一致）。工作台 handler 与 gateway sweep ticker 共用此读取口径；
/// ticker 侧另带 mtime 缓存（同派发 sweep 模式），本函数保持无状态。
pub fn live_attendance_config() -> BoardAttendanceConfig {
    super::dispatch::live_board_config()
        .map(|b| b.attendance)
        .unwrap_or_default()
}

/// 按 kind 换算绝对 deadline（毫秒；`timeout_secs_for` 未知 kind 回落
/// spec 窗——值域在类型层已保证，此处纵深防御）。
///
/// 0 值语义（配置声明「0 = 不超时」）：返回 `0`，交给消费侧既有的
/// `deadline_ms <= 0 = 不设限` 惯例闸（timeout sweep / overdue 判定）。
/// 此前裸加法把 0 吞成 `now_ms`（非零）——触点创建即超时，与声明
/// 方向完全相反。
///
/// 溢出安全：秒值钳到 `i64::MAX/1000` 后 saturating——裸 `as i64 * 1000`
/// 在巨值配置下环绕成负数，deadline 反向变「已过期」。
pub fn deadline_from_cfg(now_ms: i64, cfg: &BoardAttendanceConfig, kind: TouchpointKind) -> i64 {
    let secs = cfg.timeout_secs_for(kind.as_str());
    if secs == 0 {
        return 0;
    }
    let ms = (secs.min(i64::MAX as u64 / 1000) as i64).saturating_mul(1000);
    now_ms.saturating_add(ms)
}

/// `blocking_cap_secs` → 毫秒（溢出安全，同 [`deadline_from_cfg`] 钳制
/// 纪律）：裸 `as i64 * 1000` 在巨值下环绕成负——checkpoint 硬顶比较
/// `deadline_ms + 负值 < now` 恒真 →「巨大兜底窗」反向变立即收口；
/// parked_sync 滞留通报同病。钳制后巨大值 ≈ 永不到顶，与「0 = 关」
/// 的极端值方向一致（0 值由调用方的 `> 0` / `== 0` 闸先拦，不进这里）。
pub(crate) fn blocking_cap_ms(cfg: &BoardAttendanceConfig) -> i64 {
    (cfg.blocking_cap_secs.min(i64::MAX as u64 / 1000) as i64).saturating_mul(1000)
}

/// 无人值守三态解析（计划 §4）：任务级 `Some` > 项目级 `Some` > 全局
/// mode；None = 未表态逐级继承。任务级覆盖是建单时的一次性意志，项目级
/// 是「别再问我」的就地翻转落点。
pub fn resolve_unattended(
    store: &BoardStore,
    issue: &nemesis_board::models::Issue,
    cfg: &BoardAttendanceConfig,
) -> Result<bool, String> {
    if let Some(u) = issue.unattended {
        return Ok(u);
    }
    if let Some(pid) = issue.project_id {
        let project = store.get_project(pid)?; // 行缺失 = Err（诚实失败）
        if let Some(u) = project.unattended {
            return Ok(u);
        }
    }
    Ok(cfg.normalized_mode()? == AttendanceMode::Unattended)
}

fn audit_has(audit_json: &str, action: &str) -> bool {
    serde_json::from_str::<Vec<TouchpointAuditEvent>>(if audit_json.trim().is_empty() {
        "[]"
    } else {
        audit_json
    })
    .map(|events| events.iter().any(|e| e.action == action))
    .unwrap_or(false)
}

fn human_audit_actor(actor: &Actor) -> String {
    format!("human:{}:{}", actor.kind, actor.id)
}

fn system_actor() -> Actor {
    Actor::system("touchpoint-sweep")
}

fn default_action_note(action: &str) -> &'static str {
    match action {
        "proceed_current_version" => "按当前版本继续",
        _ => "按草案继续发车（假设清单已留档）",
    }
}

// ---------------------------------------------------------------------------
// 挂卡（三拍之一：挂卡拍；创建方 P2/P3 与测试共用入口）
// ---------------------------------------------------------------------------

/// 挂卡统一入口：建触点行 + audit `hang` + 触点单系统评论 + inbox 挂卡
/// 通知。承载单由调用方建好（in_review——评审中列内置顶徽标的载体）；
/// deadline 由调用方按 [`deadline_from_cfg`] 换算。
pub fn create_touchpoint_card(
    store: &BoardStore,
    actor: &Actor,
    issue_id: i64,
    project_id: Option<i64>,
    kind: TouchpointKind,
    payload: &str,
    deadline_ms: i64,
) -> Result<Touchpoint, String> {
    let tp = store.create_touchpoint(&nemesis_board::models::NewTouchpoint {
        issue_id,
        project_id,
        kind,
        deadline_ms,
        payload: payload.to_string(),
    })?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(now_ms(), human_audit_actor(actor), "hang", kind.as_str()),
    )?;
    let label = kind_label(kind);
    store.add_comment(NewComment {
        issue_id,
        author: actor.clone(),
        content: format!(
            "🎯 {label}触点已挂卡，请在截止前到工作台处理（超时按默认动作流转，意见可随时补评论）"
        ),
        parent_id: None,
        ctype: CommentType::System,
    })?;
    let issue = store.get_issue(issue_id)?;
    store.notify(NewNotification {
        recipient: Actor::admin("dashboard"),
        kind: notification_kind::TOUCHPOINT_PENDING.to_string(),
        title: format!("{} {}", issue.number, issue.title),
        content: format!("🎯 等你处理：{label}触点已挂卡"),
        issue_id: Some(issue_id),
    })?;
    Ok(tp)
}

fn kind_label(kind: TouchpointKind) -> &'static str {
    match kind {
        TouchpointKind::Spec => "规格确认",
        TouchpointKind::Prototype => "原型评审",
    }
}

/// 无人值守直通挂卡（无人值守是默认态——卡照建、照按默认收口，全程
/// 留痕，汇报包的行为闸/迷路章节照读）：建触点行 + `hang` 审计 + 直接
/// 按默认动作收口（TimeoutDefaulted + `timeout_default` 审计）。**不
/// 评论/不通知/不转移承载单/不触发续跑钩子**——调用方（无人值守直通
/// 链）自己走评论与收货/续跑语义，这里重复触发会造成双评论/双续跑。
/// 挂卡失败由调用方决定是否照常（触点记档是可观测面，不是收货闸）。
pub fn create_and_default_touchpoint(
    store: &BoardStore,
    actor: &Actor,
    issue_id: i64,
    project_id: Option<i64>,
    kind: TouchpointKind,
    payload: &str,
) -> Result<Touchpoint, String> {
    let tp = store.create_touchpoint(&nemesis_board::models::NewTouchpoint {
        issue_id,
        project_id,
        kind,
        deadline_ms: 0,
        payload: payload.to_string(),
    })?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(
            now_ms(),
            format!("{}:{}", actor.kind, actor.id),
            "hang",
            "unattended-direct",
        ),
    )?;
    let action = default_action_for(kind);
    let human_input =
        serde_json::json!({ "default_action": action, "origin": "unattended" }).to_string();
    store.transition_touchpoint(tp.id, TouchpointStatus::TimeoutDefaulted, &human_input)?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(now_ms(), "timeout-sweep", "timeout_default", "unattended"),
    )?;
    Ok(tp)
}

// ---------------------------------------------------------------------------
// responder 漏斗（workbench.respond 的业务侧；全部动作显式审计）
// ---------------------------------------------------------------------------

/// 人工动作词表（`workbench.respond` 的 `action`）：
/// `confirm` / `revise_spec` / `rework_local` / `cancel` / `mute`。
/// 返回收口后的触点 + 承载单状态。
pub fn respond_touchpoint(
    store: &BoardStore,
    cfg: &BoardAttendanceConfig,
    actor: &Actor,
    touchpoint_id: i64,
    action: &str,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let tp = store
        .get_touchpoint(touchpoint_id)?
        .ok_or_else(|| format!("触点 {touchpoint_id} 不存在"))?;
    if tp.status != TouchpointStatus::Pending {
        return Err(format!(
            "触点 {touchpoint_id} 当前状态 {} 不接受人工动作（迟到意见请走单据评论进变更管理）",
            tp.status
        ));
    }
    let issue = store.get_issue(tp.issue_id)?; // 行缺失 = Err（诚实失败）
    match action {
        "confirm" => respond_confirm(store, actor, &tp),
        "revise_spec" => respond_revise_spec(store, cfg, actor, &tp, &issue, payload),
        "rework_local" => respond_rework_local(store, cfg, actor, &tp, &issue, payload),
        "cancel" => respond_cancel(store, actor, &tp, payload),
        "mute" => respond_mute(store, actor, &tp, &issue),
        other => Err(format!(
            "未知触点动作 \"{other}\"（可选：confirm / revise_spec / rework_local / cancel / mute）"
        )),
    }
}

/// 确认：触点 Confirmed + 承载单按 kind 落位 + 恢复钩子（未装配 = warn 收口）。
fn respond_confirm(
    store: &BoardStore,
    actor: &Actor,
    tp: &Touchpoint,
) -> Result<serde_json::Value, String> {
    let human_input = serde_json::json!({ "action": "confirm" }).to_string();
    store.transition_touchpoint(tp.id, TouchpointStatus::Confirmed, &human_input)?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(now_ms(), human_audit_actor(actor), "confirm", ""),
    )?;
    store.add_comment(NewComment {
        issue_id: tp.issue_id,
        author: actor.clone(),
        content: format!("✅ {}确认：继续执行", kind_label(tp.kind)),
        parent_id: None,
        ctype: CommentType::System,
    })?;
    store.transition_issue(tp.issue_id, settled_issue_status(tp.kind), actor)?;
    fire_resume(&TouchpointResume {
        issue_id: tp.issue_id,
        touchpoint_id: tp.id,
        kind: tp.kind,
        action: "confirm",
        human_input,
    });
    respond_result(store, tp.id)
}

/// 改规格：否决计数 → 熔断闸 → v2 重挂（倒计时重置，承载单留评审中）。
fn respond_revise_spec(
    store: &BoardStore,
    cfg: &BoardAttendanceConfig,
    actor: &Actor,
    tp: &Touchpoint,
    issue: &nemesis_board::models::Issue,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let new_spec = payload
        .get("spec")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("改规格需要 spec（修订后的规格正文）")?;
    if veto_meltdown(tp.veto_rounds + 1, cfg) {
        return park_for_meltdown(store, cfg, actor, tp, issue, "revise_spec");
    }
    let human_input = serde_json::json!({
        "action": "revise_spec",
        "spec_len": new_spec.len(),
    })
    .to_string();
    store.transition_touchpoint(tp.id, TouchpointStatus::VetoedSpec, &human_input)?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(now_ms(), human_audit_actor(actor), "veto_spec", ""),
    )?;
    // v2 重挂：版本自增 + spec 替换，deadline 重置（计划 §6.4）。
    let next_payload = bump_payload_version(&tp.payload, Some(new_spec));
    let rehung = store.rehang_touchpoint(
        tp.id,
        deadline_from_cfg(now_ms(), cfg, tp.kind),
        &next_payload,
    )?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(
            now_ms(),
            human_audit_actor(actor),
            "rehang",
            payload_version(&next_payload),
        ),
    )?;
    let version = payload_version(&next_payload);
    store.add_comment(NewComment {
        issue_id: tp.issue_id,
        author: actor.clone(),
        content: format!("📝 改规格：{version} 已挂出，倒计时重置"),
        parent_id: None,
        ctype: CommentType::System,
    })?;
    let _ = rehung;
    respond_result(store, tp.id)
}

/// 局部返工（kind 分臂）：spec = 否决计数 → 熔断闸 → 同触点原载荷重挂
/// （行内累计，复验即重问人工——无机械进展的第二次否决自然触发熔断）；
/// prototype = 否决落账**不重挂**（每次机器周期挂新卡，卡内计数恒从 0
/// 起）→ 恢复钩子证据回灌重派 → 机器闸复验取代人工复验（P3 语义），熔
/// 断计数跨卡求和。钩子未装配 = 诚实降级：状态已落账、重派不发车。
fn respond_rework_local(
    store: &BoardStore,
    cfg: &BoardAttendanceConfig,
    actor: &Actor,
    tp: &Touchpoint,
    issue: &nemesis_board::models::Issue,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("局部返工需要 reason（否决原因）")?;
    let recheck = payload
        .get("recheck")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if tp.kind == TouchpointKind::Prototype {
        // 原型臂：不重挂本卡——落账后交恢复钩子证据回灌重派（机器闸
        // 复验取代人工复验，P3 语义），熔断计数跨卡求和。
        return respond_rework_local_prototype(store, cfg, actor, tp, issue, reason, recheck);
    }
    // spec 臂：同触点行内重挂（原载荷原版本），否决计数行内累计。
    if veto_meltdown(tp.veto_rounds + 1, cfg) {
        return park_for_meltdown(store, cfg, actor, tp, issue, "rework_local");
    }
    let human_input = serde_json::json!({
        "action": "rework_local",
        "reason": reason,
        "recheck": recheck,
    })
    .to_string();
    store.transition_touchpoint(tp.id, TouchpointStatus::VetoedLocal, &human_input)?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(now_ms(), human_audit_actor(actor), "veto_local", reason),
    )?;
    store.rehang_touchpoint(
        tp.id,
        deadline_from_cfg(now_ms(), cfg, tp.kind),
        &tp.payload,
    )?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(
            now_ms(),
            human_audit_actor(actor),
            "rehang",
            payload_version(&tp.payload),
        ),
    )?;
    let look = if recheck {
        "；复验通过后追加一次人工复验"
    } else {
        ""
    };
    store.add_comment(NewComment {
        issue_id: tp.issue_id,
        author: actor.clone(),
        content: format!("↩️ 局部返工已记录：{reason}{look}；触点重挂等待处理"),
        parent_id: None,
        ctype: CommentType::System,
    })?;
    respond_result(store, tp.id)
}

/// prototype 局部返工臂：熔断计数 = 本单全部原型触点的 veto_rounds 求
/// 和 +1（每次机器周期挂新卡、卡内计数恒从 0 起，跨卡求和才反映真实
/// 否决轮数——「改完再看」复验诉求随否决落账，机器闸复验时经
/// [`latest_rework_recheck`] 读取）；落账 VetoedLocal 后触发恢复钩子
/// 证据回灌重派，**不重挂本卡**。
fn respond_rework_local_prototype(
    store: &BoardStore,
    cfg: &BoardAttendanceConfig,
    actor: &Actor,
    tp: &Touchpoint,
    issue: &nemesis_board::models::Issue,
    reason: &str,
    recheck: bool,
) -> Result<serde_json::Value, String> {
    let rounds_before: i64 = store
        .list_touchpoints_by_issue(tp.issue_id)
        .unwrap_or_default()
        .iter()
        .filter(|t| t.kind == TouchpointKind::Prototype)
        .map(|t| t.veto_rounds)
        .sum();
    if veto_meltdown(rounds_before + 1, cfg) {
        return park_for_meltdown(store, cfg, actor, tp, issue, "rework_local");
    }
    let human_input = serde_json::json!({
        "action": "rework_local",
        "reason": reason,
        "recheck": recheck,
    })
    .to_string();
    store.transition_touchpoint(tp.id, TouchpointStatus::VetoedLocal, &human_input)?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(now_ms(), human_audit_actor(actor), "veto_local", reason),
    )?;
    let look = if recheck {
        "；复验通过后追加一次人工复验"
    } else {
        ""
    };
    // 续跑去向如实注记：钩子在场 = 证据回灌重派（机器闸复验）；缺席 =
    // 诚实降级（触点状态已落账，重派不发车，需人工派发）。
    let tail = if TOUCHPOINT_RESUME_HOOK.get().is_some() {
        "证据回灌重派，机器闸复验"
    } else {
        "恢复钩子未装配，需人工派发（重派未发车）"
    };
    store.add_comment(NewComment {
        issue_id: tp.issue_id,
        author: actor.clone(),
        content: format!("↩️ 局部返工已记录：{reason}{look}；{tail}"),
        parent_id: None,
        ctype: CommentType::System,
    })?;
    fire_resume(&TouchpointResume {
        issue_id: tp.issue_id,
        touchpoint_id: tp.id,
        kind: tp.kind,
        action: "veto_local",
        human_input,
    });
    respond_result(store, tp.id)
}

/// 取消：整单取消（承载单 → cancelled；触点终态不重开）。
fn respond_cancel(
    store: &BoardStore,
    actor: &Actor,
    tp: &Touchpoint,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let human_input = serde_json::json!({ "action": "cancel", "reason": reason }).to_string();
    store.transition_touchpoint(tp.id, TouchpointStatus::Cancelled, &human_input)?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(now_ms(), human_audit_actor(actor), "cancel", &reason),
    )?;
    let note = if reason.is_empty() {
        "✖️ 触点取消".to_string()
    } else {
        format!("✖️ 触点取消：{reason}")
    };
    store.add_comment(NewComment {
        issue_id: tp.issue_id,
        author: actor.clone(),
        content: note,
        parent_id: None,
        ctype: CommentType::System,
    })?;
    store.transition_issue(tp.issue_id, IssueStatus::Cancelled, actor)?;
    respond_result(store, tp.id)
}

/// 「别再问我」：项目翻转为无人值守（就地翻转，计划 §4）+ 当前触点按
/// 默认动作立即收口（关门不锁门）。触点不挂项目 = 诚实报错（mute 需要
/// 项目落点，散单没有可翻转的开关面）。
fn respond_mute(
    store: &BoardStore,
    actor: &Actor,
    tp: &Touchpoint,
    issue: &nemesis_board::models::Issue,
) -> Result<serde_json::Value, String> {
    let pid = tp
        .project_id
        .or(issue.project_id)
        .ok_or("「别再问我」需要触点归属项目（散单无项目开关面；可改用取消）")?;
    store.set_project_unattended(pid, Some(true))?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(
            now_ms(),
            human_audit_actor(actor),
            "mute",
            format!("project:{pid}"),
        ),
    )?;
    store.add_comment(NewComment {
        issue_id: tp.issue_id,
        author: actor.clone(),
        content: "🔇 别再问我：项目已翻转无人值守，本触点按默认动作收口".to_string(),
        parent_id: None,
        ctype: CommentType::System,
    })?;
    timeout_default(store, tp, "mute");
    respond_result(store, tp.id)
}

/// 否决熔断判定（纯比较）：`veto_max_rounds<=0` = 熔断关闭（0=关惯例）。
fn veto_meltdown(veto_rounds_after: i64, cfg: &BoardAttendanceConfig) -> bool {
    cfg.veto_max_rounds > 0 && veto_rounds_after >= i64::from(cfg.veto_max_rounds)
}

/// 最近一次原型局部否决是否带「改完再看」复验诉求（latest-veto-wins 纯
/// 查询，不消费）：读该单最新一张 VetoedLocal **原型**触点的
/// human_input.recheck；无否决记录 = false。复验诉求无需核销——之后一
/// 次不带 recheck 的否决自然覆盖（最新否决说了算）。消费点：原型机器
/// 行为闸 PASS 臂无人值守分支（复验诉求 = 不直接收货，重挂复验卡）。
pub fn latest_rework_recheck(store: &BoardStore, issue_id: i64) -> bool {
    let Ok(tps) = store.list_touchpoints_by_issue(issue_id) else {
        return false;
    };
    tps.into_iter()
        .filter(|t| {
            t.kind == TouchpointKind::Prototype && t.status == TouchpointStatus::VetoedLocal
        })
        .max_by_key(|t| t.id)
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t.human_input).ok())
        .and_then(|v| v.get("recheck").and_then(|x| x.as_bool()))
        .unwrap_or(false)
}

/// 熔断停车：触点 parked_sync + 承载单 blocked + 「需同步沟通」通报。
fn park_for_meltdown(
    store: &BoardStore,
    cfg: &BoardAttendanceConfig,
    actor: &Actor,
    tp: &Touchpoint,
    issue: &nemesis_board::models::Issue,
    from_action: &str,
) -> Result<serde_json::Value, String> {
    let human_input = serde_json::json!({ "action": from_action, "meltdown": true }).to_string();
    store.transition_touchpoint(tp.id, TouchpointStatus::ParkedSync, &human_input)?;
    store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(
            now_ms(),
            human_audit_actor(actor),
            "parked_sync",
            format!("veto_max_rounds={} 熔断", cfg.veto_max_rounds),
        ),
    )?;
    store.add_comment(NewComment {
        issue_id: tp.issue_id,
        author: system_actor(),
        content: format!(
            "🛑 同范围否决已达上限（veto_max_rounds={}）：触点停车，标记「需同步沟通」。异步工单不是同步沟通的替代品——请人工介入；迟到意见可走评论",
            cfg.veto_max_rounds
        ),
        parent_id: None,
        ctype: CommentType::System,
    })?;
    store.transition_issue(tp.issue_id, IssueStatus::Blocked, actor)?;
    store.notify(NewNotification {
        recipient: Actor::admin("dashboard"),
        kind: notification_kind::TOUCHPOINT_TIMEOUT.to_string(),
        title: format!("{} {}", issue.number, issue.title),
        content: "🛑 触点已停车待同步沟通（否决熔断）".to_string(),
        issue_id: Some(tp.issue_id),
    })?;
    respond_result(store, tp.id)
}

/// 收口结果（fresh 触点 + 承载单状态）。
fn respond_result(store: &BoardStore, touchpoint_id: i64) -> Result<serde_json::Value, String> {
    let tp = store
        .get_touchpoint(touchpoint_id)?
        .ok_or_else(|| format!("触点 {touchpoint_id} 收口后回读丢失"))?;
    let issue_status = store
        .get_issue(tp.issue_id)
        .map(|i| i.status.as_str().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    Ok(serde_json::json!({
        "touchpoint": serde_json::to_value(&tp).map_err(|e| format!("serialize touchpoint: {e}"))?,
        "issue_status": issue_status,
    }))
}

// ---------------------------------------------------------------------------
// 超时 sweep（estop 感知；cfg 显式入参——测试不碰全局单例）
// ---------------------------------------------------------------------------

/// sweep 统计（gateway ticker 日志面）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TouchpointSweepStats {
    pub checked: usize,
    pub reminded: usize,
    pub defaulted: usize,
    pub parked: usize,
    pub cap_noticed: usize,
}

fn sweep_estop_engaged() -> bool {
    // estop 装配面在 cluster feature 内（dispatch.rs BOARD_ESTOP）；
    // 非 cluster 形态无装配 = 无闸（等价旧无闸行为）。
    #[cfg(feature = "cluster")]
    return super::dispatch::board_estop_engaged();
    #[cfg(not(feature = "cluster"))]
    return false;
}

/// 超时 sweep 内核（计划 §5：复用派发 sweep 模式）。cfg 显式入参；
/// estop 短路最前（release 后按绝对 deadline 判定，停机不白等）。
/// - 提醒拍：挂卡 T+10min 未处理 → audit `remind` + inbox（幂等）。
/// - unattended：等待窗到 → 按默认动作收口（承载单 done + 续跑钩子）。
/// - checkpoint：等待窗到 → 通报转入同步沟通等待；硬顶
///   `blocking_cap_secs` 到顶才按默认收口（死等有界，机器不停摆）。
/// - parked_sync 滞留超 `blocking_cap_secs` → 通报一次（人工升级，
///   触点终态不重开——熔断是诚实认输）。
pub fn sweep_touchpoint_timeouts_with_config(
    cfg: &BoardAttendanceConfig,
    store: &BoardStore,
    now_ms: i64,
) -> TouchpointSweepStats {
    let mut stats = TouchpointSweepStats::default();
    if sweep_estop_engaged() {
        return stats;
    }
    let pendings = match store.list_pending_touchpoints() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("[Touchpoint] sweep pending 列表失败: {e}");
            return stats;
        }
    };
    for tp in pendings {
        stats.checked += 1;
        if tp.deadline_ms <= 0 {
            continue; // 0 = 不设限（惯例口径）
        }
        // 提醒拍（T+10min；audit 幂等去重）。
        if now_ms < tp.deadline_ms
            && now_ms - tp.created_at >= REMIND_AFTER_MS
            && !audit_has(&tp.audit, "remind")
        {
            if let Err(e) = store.append_touchpoint_audit(
                tp.id,
                TouchpointAuditEvent::new(now_ms, "timeout-sweep", "remind", ""),
            ) {
                tracing::warn!("[Touchpoint] remind 审计失败 tp={}: {e}", tp.id);
                continue;
            }
            notify_touchpoint(
                store,
                tp.issue_id,
                notification_kind::TOUCHPOINT_REMIND,
                "⏰ 触点等你处理已 10 分钟（超时按默认动作流转）",
            );
            stats.reminded += 1;
        }
        if now_ms < tp.deadline_ms {
            continue;
        }
        let issue = match store.get_issue(tp.issue_id) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("[Touchpoint] sweep: 承载单 {} 读取失败: {e}", tp.issue_id);
                continue;
            }
        };
        let unattended = match resolve_unattended(store, &issue, cfg) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!("[Touchpoint] sweep: 无人值守解析失败 tp={}: {e}", tp.id);
                continue;
            }
        };
        if unattended {
            timeout_default(store, &tp, "timeout");
            stats.defaulted += 1;
            continue;
        }
        // checkpoint 死等语义：窗口到 → 通报转同步沟通等待；硬顶到 → 默认收口。
        let cap_ms = blocking_cap_ms(cfg);
        if cfg.blocking_cap_secs > 0 && now_ms < tp.deadline_ms + cap_ms {
            if !audit_has(&tp.audit, "checkpoint_wait") {
                if let Err(e) = store.append_touchpoint_audit(
                    tp.id,
                    TouchpointAuditEvent::new(now_ms, "timeout-sweep", "checkpoint_wait", ""),
                ) {
                    tracing::warn!("[Touchpoint] checkpoint_wait 审计失败 tp={}: {e}", tp.id);
                    continue;
                }
                store.add_comment(NewComment {
                    issue_id: tp.issue_id,
                    author: system_actor(),
                    content: "⏳ 人工等待窗已到（checkpoint 模式）：转入同步沟通等待，硬顶到顶后自动按默认动作收口".to_string(),
                    parent_id: None,
                    ctype: CommentType::System,
                })
                .ok();
                notify_touchpoint(
                    store,
                    tp.issue_id,
                    notification_kind::TOUCHPOINT_TIMEOUT,
                    "⏳ 触点等待窗已到：checkpoint 模式同步沟通等待中（硬顶后自动继续）",
                );
                stats.parked += 1;
            }
            continue;
        }
        timeout_default(store, &tp, "blocking_cap");
        stats.defaulted += 1;
    }
    // parked_sync 滞留通报（熔断是诚实认输，机器不代答；人工升级留痕）。
    if let Ok(parked_list) = store.list_touchpoints_by_status(TouchpointStatus::ParkedSync) {
        for tp in parked_list {
            if cfg.blocking_cap_secs == 0 {
                break; // 0 = 关（不通报）
            }
            let cap_ms = blocking_cap_ms(cfg);
            if now_ms - tp.updated_at < cap_ms || audit_has(&tp.audit, "park_cap_notice") {
                continue;
            }
            if let Err(e) = store.append_touchpoint_audit(
                tp.id,
                TouchpointAuditEvent::new(now_ms, "timeout-sweep", "park_cap_notice", ""),
            ) {
                tracing::warn!("[Touchpoint] park_cap_notice 审计失败 tp={}: {e}", tp.id);
                continue;
            }
            store
                .add_comment(NewComment {
                    issue_id: tp.issue_id,
                    author: system_actor(),
                    content: "🛑 停车滞留已超硬顶：请人工同步沟通（触点保持停车，机器不代答）"
                        .to_string(),
                    parent_id: None,
                    ctype: CommentType::System,
                })
                .ok();
            notify_touchpoint(
                store,
                tp.issue_id,
                notification_kind::TOUCHPOINT_TIMEOUT,
                "🛑 停车滞留超硬顶，请人工同步沟通",
            );
            stats.cap_noticed += 1;
        }
    }
    stats
}

/// 超时/静音/硬顶共用收口：按默认动作收口 + 系统评论 + inbox 通报 +
/// 续跑钩子 + 承载单按 kind 落位。`origin` 进审计 detail（timeout/mute/blocking_cap）。
fn timeout_default(store: &BoardStore, tp: &Touchpoint, origin: &str) -> bool {
    let action = default_action_for(tp.kind);
    let human_input = serde_json::json!({ "default_action": action, "origin": origin }).to_string();
    if let Err(e) =
        store.transition_touchpoint(tp.id, TouchpointStatus::TimeoutDefaulted, &human_input)
    {
        tracing::warn!("[Touchpoint] 默认收口转移失败 tp={}: {e}", tp.id);
        return false;
    }
    if let Err(e) = store.append_touchpoint_audit(
        tp.id,
        TouchpointAuditEvent::new(now_ms(), "timeout-sweep", "timeout_default", origin),
    ) {
        tracing::warn!("[Touchpoint] 默认收口审计失败 tp={}: {e}", tp.id);
    }
    store
        .add_comment(NewComment {
            issue_id: tp.issue_id,
            author: system_actor(),
            content: format!(
                "⏱ 已按默认继续：{}（{}自动流转；迟到意见走评论进变更管理）",
                default_action_note(action),
                origin_note(origin)
            ),
            parent_id: None,
            ctype: CommentType::System,
        })
        .ok();
    notify_touchpoint(
        store,
        tp.issue_id,
        notification_kind::TOUCHPOINT_TIMEOUT,
        &format!("⏱ 触点已按默认继续：{}", default_action_note(action)),
    );
    // 承载单按 kind 落位（规格单回 in_progress 续拆解链、原型单 done；
    // 系统操作者不触发 NB-CODE-012 审计——InReview 来源本来就在验收链语义内）。
    if let Err(e) =
        store.transition_issue(tp.issue_id, settled_issue_status(tp.kind), &system_actor())
    {
        tracing::warn!("[Touchpoint] 承载单落位失败 issue={}: {e}", tp.issue_id);
    }
    fire_resume(&TouchpointResume {
        issue_id: tp.issue_id,
        touchpoint_id: tp.id,
        kind: tp.kind,
        action: "timeout_default",
        human_input,
    });
    true
}

fn origin_note(origin: &str) -> &'static str {
    match origin {
        "mute" => "「别再问我」就地",
        "blocking_cap" => "硬顶到顶",
        _ => "超时",
    }
}

/// 确认/默认收口后承载单的落位（按 kind，状态机合法边 InReview→InProgress
/// / InReview→Done 都在场）：
/// - **原型里程碑单**：机器闸已过、人工方向已定 → 单真的做完了 → done。
/// - **规格点火单**：方向已定、流水线还要走 → 回 in_progress（规划入口
///   回位），由恢复钩子续跑 planner → confirm → 派发——提前 done 会把
///   自动验收/自动收父单链骗成「已完工」。
fn settled_issue_status(kind: TouchpointKind) -> IssueStatus {
    match kind {
        TouchpointKind::Spec => IssueStatus::InProgress,
        TouchpointKind::Prototype => IssueStatus::Done,
    }
}

// ---------------------------------------------------------------------------
// P2 规格蒸馏闸（execute_plan_chain 前置闸）：体验型任务的规划入口在
// 拆解之前先过这里——有人值守先蒸馏规格挂卡等人确认（确认/超时默认经
// 恢复钩子回到拆解链，规格从触点 payload 烧进单据）；无人值守照常蒸馏
// 但不等人（假设清单落账直通拆解）。非体验型零开销直通。
// 消费方 plan.rs（LLM 调用 + 分支落账）；本侧只放纯函数与 store 操作。
// ---------------------------------------------------------------------------

/// 蒸馏产物（触点一挂卡 payload 的结构化形态）。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DistilledSpec {
    /// 规格版本（v1 初版；revise_spec 自增 v2+）。
    pub version: String,
    /// 规格正文（markdown；蒸馏失败回退 = 原始描述原文）。
    pub spec: String,
    /// 关键假设清单（超时默认发车时落审计；计划 §2「假设清单进审计」）。
    pub assumptions: Vec<String>,
    /// 体验验收条款（确认后进 issue.acceptance_criteria 成 `[CHECK]` 锚点源）。
    pub acceptance_checks: Vec<String>,
    /// false = 蒸馏失败的回退草案（原文即草案，审计可辨）。
    pub distilled: bool,
}

/// 规格触点闸的判定结果（纯判定，无副作用；plan.rs 按臂落账）。
#[derive(Debug)]
pub(super) enum SpecGateAction {
    /// 非体验型：照旧直通（gate 零开销路径）。
    Skip,
    /// 已有 pending 规格卡：幂等早退（重复触发不重复挂卡）。
    WaitPending(Touchpoint),
    /// 体验型 + 有人值守 + 无触点：蒸馏 → 挂卡 → 承载单 in_review →
    /// 提前返回（不拆解）。
    DistillWait,
    /// 体验型 + 无人值守：蒸馏 → 假设清单落账 → 规格烧进单据 → 直通拆解。
    DistillProceed,
    /// 已有终态规格触点（confirm / timeout_default）：规格烧进单据 →
    /// 直通拆解（恢复钩子续跑的落点臂）。
    ProceedWithSpec(Touchpoint),
}

/// 闸判定（同步、只读）：quality 归一（空串/未知值按标准档，计划 §5）
/// → 无人值守三态解析 → 规格触点在场性检查。任何 Err 向上透传（诚实
/// 失败，不静默放行）。
pub(super) fn spec_gate_decision(
    store: &BoardStore,
    issue: &nemesis_board::models::Issue,
) -> Result<SpecGateAction, String> {
    if !issue.quality.trim().eq_ignore_ascii_case("experience") {
        return Ok(SpecGateAction::Skip);
    }
    let attended = !resolve_unattended(store, issue, &live_attendance_config())?;
    let tps = store.list_touchpoints_by_issue(issue.id)?;
    let settled = tps.iter().find(|t| {
        t.kind == TouchpointKind::Spec
            && matches!(
                t.status,
                TouchpointStatus::Confirmed | TouchpointStatus::TimeoutDefaulted
            )
    });
    if let Some(tp) = settled {
        // 终态规格在场（确认过或超时默认过）：无论有人无人都不重复蒸馏。
        return Ok(SpecGateAction::ProceedWithSpec(tp.clone()));
    }
    if !attended {
        return Ok(SpecGateAction::DistillProceed);
    }
    if let Some(tp) = tps
        .iter()
        .find(|t| t.kind == TouchpointKind::Spec && t.status == TouchpointStatus::Pending)
    {
        return Ok(SpecGateAction::WaitPending(tp.clone()));
    }
    Ok(SpecGateAction::DistillWait)
}

/// 规格正文在 issue.description 里的所属小节标题（幂等替换锚点）。
const SPEC_DESC_HEADER: &str = "## 📐 规格";

/// 规格正文烧进描述：无标记 = 追加小节；有标记 = 整节替换（版本升级
/// revise_spec 后幂等重烧）。标记前正文原样保留。
pub(super) fn apply_spec_to_description(description: &str, version: &str, spec: &str) -> String {
    let block = format!("{SPEC_DESC_HEADER}（触点 {version}）\n\n{spec}");
    match description.find(SPEC_DESC_HEADER) {
        Some(idx) => format!("{}\n\n{block}", description[..idx].trim_end()),
        None => format!("{}\n\n{block}", description.trim_end()),
    }
}

/// 验收条款并进 acceptance_criteria：每条加 `[CHECK]` 前缀（锚点管线
/// 单一真相源 nemesis-board::anchor 同款行形态），逐条去重（幂等重烧
/// 不重复追加）；已有条目原样保留。全部已存在 = None（无需 patch）。
pub(super) fn apply_spec_checks_to_ac(existing: Option<&str>, checks: &[String]) -> Option<String> {
    if checks.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = existing
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let mut added = 0usize;
    for c in checks {
        let line = format!("[CHECK] {c}");
        if !lines.iter().any(|l| l.trim() == line) {
            lines.push(line);
            added += 1;
        }
    }
    if added == 0 {
        return None;
    }
    Some(lines.join("\n"))
}

/// 规格烧进承载单（描述小节 + [CHECK] 锚点源）。patch 无可写内容时
/// 跳过（幂等重入零副作用）。
pub(super) fn apply_spec_to_issue(
    store: &BoardStore,
    issue: &nemesis_board::models::Issue,
    version: &str,
    spec: &DistilledSpec,
) -> Result<(), String> {
    let description = apply_spec_to_description(&issue.description, version, &spec.spec);
    let acceptance_criteria = apply_spec_checks_to_ac(
        issue.acceptance_criteria.as_deref(),
        &spec.acceptance_checks,
    );
    if description == issue.description && acceptance_criteria.is_none() {
        return Ok(());
    }
    store.update_issue(
        issue.id,
        &nemesis_board::models::IssuePatch {
            description: Some(description),
            acceptance_criteria,
            ..Default::default()
        },
        &Actor::system("board-touchpoint"),
    )?;
    Ok(())
}

/// 从终态触点 payload 提取规格（version/spec/acceptance_checks；payload
/// 损坏或缺 spec = None → 调用方诚实注记后照旧拆解，不阻塞链）。
pub(super) fn spec_from_payload(payload: &str) -> Option<DistilledSpec> {
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let spec = v.get("spec")?.as_str()?.trim().to_string();
    if spec.is_empty() {
        return None;
    }
    let arr = |k: &str| -> Vec<String> {
        v.get(k)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(DistilledSpec {
        version: version_of(&v),
        spec,
        assumptions: arr("assumptions"),
        acceptance_checks: arr("acceptance_checks"),
        distilled: true,
    })
}

/// 蒸馏输出解析（宽容：剥围栏、取首 `{` 到末 `}`；缺 spec/空 spec =
/// None → 调用方回退草案）。与 sanitize_ac_draft 同纪律：宽容剥围栏，
/// 结构校验从严。
pub(super) fn parse_distill_output(raw: &str) -> Option<DistilledSpec> {
    let t = raw.trim();
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    if end < start {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&t[start..=end]).ok()?;
    let spec = v.get("spec")?.as_str()?.trim().to_string();
    if spec.is_empty() {
        return None;
    }
    let arr = |k: &str| -> Vec<String> {
        v.get(k)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    Some(DistilledSpec {
        version: version_of(&v),
        spec,
        assumptions: arr("assumptions"),
        acceptance_checks: arr("acceptance_checks"),
        distilled: true,
    })
}

fn version_of(v: &serde_json::Value) -> String {
    v.get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("v1")
        .trim()
        .to_string()
}

/// 蒸馏失败回退草案：原始描述原文即规格（无人值守不因蒸馏失败停摆；
/// 审计可辨 distilled=false）。
pub(super) fn fallback_spec(issue: &nemesis_board::models::Issue) -> DistilledSpec {
    DistilledSpec {
        version: "v1".to_string(),
        spec: format!(
            "（蒸馏失败回退：原始愿望原文即草案）\n\n{}",
            issue.description.trim()
        ),
        assumptions: Vec::new(),
        acceptance_checks: Vec::new(),
        distilled: false,
    }
}

/// 规格挂卡 payload（version + spec + assumptions + acceptance_checks +
/// distilled 五键；revise_spec 的 bump_payload_version 只动 version/spec，
/// 其余键原样透传）。
pub(super) fn spec_payload(spec: &DistilledSpec) -> String {
    serde_json::json!({
        "version": spec.version,
        "spec": spec.spec,
        "assumptions": spec.assumptions,
        "acceptance_checks": spec.acceptance_checks,
        "distilled": spec.distilled,
    })
    .to_string()
}

/// 装配触点恢复钩子（install-once；首个 board WSAPI 命令即装，之后
/// 零开销）。闭包捕获 `Arc<AppState>` 快照、fire 时新鲜读——agent 重启
/// （RwLock 新实例）/board 服务/集群装配变化自动跟进。spec = 续跑规划链
/// （规格注入由闸 ProceedWithSpec 臂统一接管，钩子只负责「重新进链」）；
/// prototype = P3 机器闸接线（confirm/timeout_default 补级联收口；
/// veto_local 证据回灌重派）。
#[cfg(feature = "cluster")]
pub(super) fn ensure_touchpoint_resume_hook_installed(state: Arc<crate::api_handlers::AppState>) {
    if TOUCHPOINT_RESUME_HOOK.get().is_some() {
        return;
    }
    let ok = install_touchpoint_resume_hook(Arc::new(move |r| {
        let store = match state.board.as_ref().map(|svc| svc.store().clone()) {
            Some(s) => s,
            None => return Err("board 服务未装配，无法续跑触点链".to_string()),
        };
        if r.kind == TouchpointKind::Prototype {
            // P3 机器闸接线：confirm/timeout_default 补级联收口；
            // veto_local 证据回灌重派（机器闸复验取代人工复验）。
            return prototype_resume(&state, &store, r);
        }
        // 新鲜读单据：确认/默认已把承载单回 in_progress；人工已挪走
        // （cancel/done 等）→ 尊重人工意图，不续跑。
        let issue = match store.get_issue(r.issue_id) {
            Ok(i) => i,
            Err(e) => return Err(format!("读承载单失败: {e}")),
        };
        if issue.status != IssueStatus::InProgress {
            tracing::info!(
                "[Touchpoint] 规格收口续跑跳过：承载单状态已非 in_progress（{}）",
                issue.status.as_str()
            );
            return Ok(());
        }
        let agent_loop = match state.agent_loop.read().clone() {
            Some(l) => l,
            None => return Err("agent loop 未运行，无法续跑拆解链".to_string()),
        };
        let env = Arc::new(super::plan::PlanChainEnv {
            store: store.clone(),
            cluster: state.cluster.clone(),
            agent_loop,
            issue,
            actor: Actor::system("board-touchpoint"),
            home: state.home.clone().map(std::path::PathBuf::from),
            hub: Some(state.event_hub.clone()),
        });
        let plan_id = format!("tp-resume-{}-{}", r.touchpoint_id, now_ms());
        super::plan::spawn_plan_chain(
            env,
            plan_id,
            1,
            super::plan::plan_chain_cfg(state.home.as_deref().map(std::path::Path::new)),
        );
        Ok(())
    }));
    if ok {
        tracing::info!(
            "[Touchpoint] 恢复钩子已装配（spec→规划链续跑；prototype→级联收口/证据回灌重派）"
        );
    }
}

/// prototype 触点续跑（P3 机器闸接线）：按收口动作分派——
/// - `confirm`/`timeout_default`：承载单已按 [`settled_issue_status`] 落账
///   （原型 = done），补级联收口（父单同步/依赖单补派/级联取消）。
/// - `veto_local`：局部返工否决 → 证据回灌重派（沿用最后一次派发的
///   worker——改的是它交付的原型；无派发记录回落 self 本机），否决原因
///   经 `review_feedback` 渲染进 dispatch prompt → 机器闸复验取代人工
///   复验（gate 重跑，PASS 再挂卡/收货）。
///
/// 任何失败 = Err 上抛（fire_resume warn-only，不回滚已落账触点状态）；
/// 重派发车失败追加系统评论留痕（诚实降级，需人工派发）。
#[cfg(feature = "cluster")]
fn prototype_resume(
    state: &Arc<crate::api_handlers::AppState>,
    store: &Arc<nemesis_board::BoardStore>,
    r: &TouchpointResume,
) -> Result<(), String> {
    let actor = Actor::system("board-touchpoint");
    match r.action {
        "confirm" | "timeout_default" => {
            let cascaded = on_issue_settled(store, state.cluster.as_ref(), r.issue_id, &actor);
            tracing::info!(
                "[Touchpoint] prototype 收口级联完成 issue={} cascades={}",
                r.issue_id,
                cascaded.len()
            );
            Ok(())
        }
        "veto_local" => {
            let target = store
                .list_dispatches(r.issue_id)
                .unwrap_or_default()
                .into_iter()
                .rev()
                .map(|d| d.worker_id)
                .find(|w| !w.trim().is_empty())
                .unwrap_or_else(|| "self".to_string());
            let reason = serde_json::from_str::<serde_json::Value>(&r.human_input)
                .ok()
                .and_then(|v| v.get("reason").and_then(|x| x.as_str()).map(String::from))
                .unwrap_or_default();
            let gap = format!("[触点局部返工] {reason}");
            match dispatch_issue_core(
                store,
                state.cluster.as_ref(),
                r.issue_id,
                &target,
                &actor,
                Some(&gap),
            ) {
                Ok(_) => {
                    tracing::info!(
                        "[Touchpoint] prototype 局部返工重派发车 issue={} target={target} tp={}",
                        r.issue_id,
                        r.touchpoint_id
                    );
                    Ok(())
                }
                Err(e) => {
                    let _ = store.add_comment(NewComment {
                        issue_id: r.issue_id,
                        author: actor.clone(),
                        content: format!(
                            "⚠️ 局部返工重派发车失败（target={target}）：{e}——需人工派发"
                        ),
                        parent_id: None,
                        ctype: CommentType::System,
                    });
                    Err(format!("prototype 局部返工重派失败: {e}"))
                }
            }
        }
        other => {
            tracing::warn!(
                "[Touchpoint] prototype resume 未知 action={other}，tp={} issue={} 收口但不续跑",
                r.touchpoint_id,
                r.issue_id
            );
            Ok(())
        }
    }
}

fn notify_touchpoint(store: &BoardStore, issue_id: i64, kind: &str, content: &str) {
    let Ok(issue) = store.get_issue(issue_id) else {
        return;
    };
    if let Err(e) = store.notify(NewNotification {
        recipient: Actor::admin("dashboard"),
        kind: kind.to_string(),
        title: format!("{} {}", issue.number, issue.title),
        content: content.to_string(),
        issue_id: Some(issue_id),
    }) {
        tracing::warn!("[Touchpoint] inbox 通知失败 issue={issue_id}: {e}");
    }
}

// ---------------------------------------------------------------------------
// payload 版本辅助（v1 → v2 自增；挂卡方结构之上只动 version/spec 两键）
// ---------------------------------------------------------------------------

fn payload_version(payload: &str) -> String {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|v| v.get("version").and_then(|x| x.as_str()).map(String::from))
        .unwrap_or_else(|| "v1".to_string())
}

fn bump_payload_version(payload: &str, new_spec: Option<&str>) -> String {
    let mut obj = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(payload)
        .unwrap_or_default();
    let cur = obj
        .get("version")
        .and_then(|v| v.as_str())
        .and_then(|s| s.strip_prefix('v'))
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(1);
    obj.insert(
        "version".to_string(),
        serde_json::Value::String(format!("v{}", cur + 1)),
    );
    if let Some(s) = new_spec {
        obj.insert("spec".to_string(), serde_json::Value::String(s.to_string()));
    }
    serde_json::to_string(&obj).unwrap_or_else(|_| payload.to_string())
}

// ---------------------------------------------------------------------------
// 取数面（board.touchpoint.list / workbench.get 的 provider）
// ---------------------------------------------------------------------------

/// 看板触点列表（看板触点卡徽标/置顶排序/「⏸ 等我处理 (N)」过滤片
/// 计数的数据源；计划 §6.5：列表归看板，页面归工作台）。
/// `status` 缺省 = pending（活卡）；`all=true` = 全量。
pub fn list_touchpoints(
    store: &BoardStore,
    data: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let all = data.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
    let project_id = data.get("project_id").and_then(|v| v.as_i64());
    let items: Vec<Touchpoint> = if all {
        match data.get("status").and_then(|v| v.as_str()) {
            Some(s) => {
                let st = TouchpointStatus::parse(s).ok_or_else(|| format!("未知触点状态: {s}"))?;
                store.list_touchpoints_by_status(st)?
            }
            None => {
                let mut v = Vec::new();
                for st in [
                    TouchpointStatus::Pending,
                    TouchpointStatus::VetoedLocal,
                    TouchpointStatus::VetoedSpec,
                    TouchpointStatus::Confirmed,
                    TouchpointStatus::TimeoutDefaulted,
                    TouchpointStatus::Cancelled,
                    TouchpointStatus::ParkedSync,
                ] {
                    v.extend(store.list_touchpoints_by_status(st)?);
                }
                v
            }
        }
    } else {
        store.list_pending_touchpoints()?
    };
    let now = now_ms();
    let mut rows = Vec::with_capacity(items.len());
    let mut pending_count = 0i64;
    for tp in items {
        if let Some(pid) = project_id
            && tp.project_id != Some(pid)
        {
            continue;
        }
        if tp.status == TouchpointStatus::Pending {
            pending_count += 1;
        }
        let (number, title, issue_status) = match store.get_issue(tp.issue_id) {
            Ok(i) => (
                i.number.clone(),
                i.title.clone(),
                i.status.as_str().to_string(),
            ),
            Err(_) => ("?".to_string(), String::new(), "unknown".to_string()),
        };
        let overdue =
            tp.status == TouchpointStatus::Pending && tp.deadline_ms > 0 && now >= tp.deadline_ms;
        rows.push(serde_json::json!({
            "touchpoint": serde_json::to_value(&tp).map_err(|e| format!("serialize touchpoint: {e}"))?,
            "issue_number": number,
            "issue_title": title,
            "issue_status": issue_status,
            "overdue": overdue,
        }));
    }
    // deadline 升序（最早到期的先处理；终态行按 id 降序垫底由前端排序）。
    Ok(serde_json::json!({ "touchpoints": rows, "pending_count": pending_count }))
}

/// workbench.get 的触点 provider（页面完整载荷：触点语义 + 承载单全
/// 视图 + 默认动作 + 无人值守解析；kind 前缀 touchpoint-* 两值）。
pub fn touchpoint_workbench_payload(
    store: &BoardStore,
    cfg: &BoardAttendanceConfig,
    touchpoint_id: i64,
) -> Result<serde_json::Value, String> {
    let tp = store
        .get_touchpoint(touchpoint_id)?
        .ok_or_else(|| format!("触点 {touchpoint_id} 不存在"))?;
    let issue = store.get_issue(tp.issue_id)?; // 行缺失 = Err（诚实失败）
    let unattended = resolve_unattended(store, &issue, cfg)?;
    let now = now_ms();
    Ok(serde_json::json!({
        "touchpoint": serde_json::to_value(&tp).map_err(|e| format!("serialize touchpoint: {e}"))?,
        "issue": super::views::issue_to_view(store, &issue)?,
        "overdue": tp.status == TouchpointStatus::Pending
            && tp.deadline_ms > 0
            && now >= tp.deadline_ms,
        "default_action": default_action_for(tp.kind),
        "unattended": unattended,
        "timeout_secs": cfg.timeout_secs_for(tp.kind.as_str()),
    }))
}
