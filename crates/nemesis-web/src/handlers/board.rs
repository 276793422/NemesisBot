//! Board handler — managed-agent 看板（W2 P1；P2 加角色门控）。
//!
//! 所有命令经 `AppState.board` 的 [`nemesis_board::BoardService`]（gateway 在
//! `board` feature 开启时注入；未注入时统一报 "board service not available"）。
//! 状态转移 / 指派走 store 的状态机接口（非法转移被拒），普通字段更新走 patch。
//! 操作者身份：dashboard 登录用户 → `Actor::admin(session_id)`。
//!
//! **角色门控（已移除，2026-08-31，见 board/tests.rs 回归钉）**：board.db 是
//! 节点本地数据（无集群同步、CLI 同权直写），worker 对本机看板有完整写权
//! （完整 CRUD；回归钉 `test_worker_role_never_403` 防止 role 403 门控回归）。
//! 集群权威语义只体现在 `issue.dispatch`/`issue.cancel` 的派发链路上
//! （dispatch 把任务发到 coordinator 选定的 worker，回报写回发起方看板）。

use crate::handlers::{get_opt_str, get_str, require_workspace};
use crate::ws_router::{ModuleHandler, RequestContext};
use base64::Engine;
use nemesis_board::BoardStore;
use nemesis_board::assignment::{Actor, AssignmentType};
use nemesis_board::models::{
    CommentType, IssueFilter, IssuePatch, IssueStatus, NewComment, NewIssue, ProjectPatch,
};
// 只被 cluster 门控的 sync/cascade 系函数使用（batch-3 SAN-06/08）。
#[cfg(feature = "cluster")]
use nemesis_board::store::DescendantEdges;
use std::sync::Arc;

pub struct BoardHandler;

/// 附件大小上限（base64 解码后字节数；WS 帧默认 16MB，留余量）。
const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;

/// Get the board service handle from AppState, or error if not injected.
fn require_board(ctx: &RequestContext) -> Result<Arc<BoardStore>, String> {
    ctx.state
        .board
        .as_ref()
        .map(|svc| svc.store().clone())
        .ok_or_else(|| "board service not available".to_string())
}

/// Request author → Actor（dashboard 用户记为 admin）。
fn ctx_actor(ctx: &RequestContext) -> Actor {
    Actor::admin(&ctx.session_id)
}

/// 解析可选指派对：`assignee_type`（"manager_self"/"worker"）+ `assignee_id`。
/// 二者必须同时出现或同时缺失。
fn parse_assignee(data: &serde_json::Value) -> Result<Option<(AssignmentType, String)>, String> {
    let at = get_opt_str(data, "assignee_type");
    let aid = get_opt_str(data, "assignee_id");
    match (at, aid) {
        (None, None) => Ok(None),
        (Some(t), Some(id)) => {
            let at = AssignmentType::from_str(&t)
                .ok_or_else(|| format!("未知 assignee_type: {t}（可选 manager_self/worker）"))?;
            Ok(Some((at, id)))
        }
        (Some(_), None) => Err("assignee_type 与 assignee_id 必须成对提供".to_string()),
        (None, Some(_)) => Err("assignee_type 与 assignee_id 必须成对提供".to_string()),
    }
}

/// 解析必填 status 字符串 → IssueStatus。
fn parse_status(data: &serde_json::Value) -> Result<IssueStatus, String> {
    let s = get_str(data, "status")?;
    IssueStatus::from_str(&s).ok_or_else(|| format!("未知 status: {s}"))
}

/// 附件文件名消毒：取路径最后一段（防目录穿越），拒空/`.`/`..`/控制字符/
/// 超长。只保留基本名，原始名存 attachment.filename 供展示。
fn sanitize_filename(name: &str) -> Result<String, String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() || base == "." || base == ".." {
        return Err("非法附件文件名".to_string());
    }
    if base.chars().any(char::is_control) {
        return Err("附件文件名含控制字符".to_string());
    }
    if base.len() > 200 {
        return Err("附件文件名过长（>200 字节）".to_string());
    }
    Ok(base.to_string())
}

fn issue_to_view(
    store: &BoardStore,
    issue: &nemesis_board::Issue,
) -> Result<serde_json::Value, String> {
    let comments = store.list_comments(issue.id)?;
    let activity = store.list_activity(issue.id)?;
    let subscribers = store.list_subscribers(issue.id)?;
    let mut v = serde_json::to_value(issue).map_err(|e| format!("serialize issue: {e}"))?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "comments".to_string(),
            serde_json::to_value(comments).unwrap_or_default(),
        );
        obj.insert(
            "activity".to_string(),
            serde_json::to_value(activity).unwrap_or_default(),
        );
        obj.insert(
            "subscribers".to_string(),
            serde_json::to_value(subscribers).unwrap_or_default(),
        );
    }
    Ok(v)
}

fn build_filter(data: &serde_json::Value) -> Result<IssueFilter, String> {
    let mut filter = IssueFilter {
        query: get_opt_str(data, "query"),
        ..Default::default()
    };
    if let Some(s) = get_opt_str(data, "status") {
        filter.status = Some(IssueStatus::from_str(&s).ok_or_else(|| format!("未知 status: {s}"))?);
    }
    if let Some((at, aid)) = parse_assignee(data)? {
        filter.assignee = Some((at, aid));
    }
    if let Some(p) = data.get("project_id").and_then(|v| v.as_i64()) {
        filter.project_id = Some(p);
    }
    if let Some(p) = data.get("priority").and_then(|v| v.as_i64()) {
        filter.priority = Some(p as i32);
    }
    // P1/A2+A2b（看板项目档案 goal）：用户面列表默认排除——归档项目子单 /
    // 已取消单 / hidden 单。前端两个页签（列表/看板）同走 issue.list，一处
    // 默认两处生效；显式 `include_*: true` 才放行。hidden 没有放行口
    // （永久收起语义，goal A2b）；详情接口不走这里（单查不受影响）。
    filter.exclude_archived_projects = !data
        .get("include_archived_projects")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    filter.exclude_cancelled = !data
        .get("include_cancelled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    filter.exclude_hidden = true;
    Ok(filter)
}

fn build_patch(data: &serde_json::Value) -> IssuePatch {
    IssuePatch {
        title: get_opt_str(data, "title"),
        description: get_opt_str(data, "description"),
        priority: data
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|p| p as i32),
        project_id: data.get("project_id").and_then(|v| v.as_i64()),
        due_date: data.get("due_date").and_then(|v| v.as_i64()),
        position: data.get("position").and_then(|v| v.as_i64()),
        acceptance_criteria: get_opt_str(data, "acceptance_criteria"),
        parent_issue_id: data.get("parent_issue_id").and_then(|v| v.as_i64()),
    }
}

/// WSAPI issue 载荷 → NewIssue（pub：agent 工具 `board_issue create` 复用
/// 同一份字段解析/默认值/assignee 配对语义——单一真相源）。
pub fn build_new_issue(data: &serde_json::Value, actor: Actor) -> Result<NewIssue, String> {
    let mut ni = NewIssue {
        title: get_str(data, "title")?,
        description: get_opt_str(data, "description").unwrap_or_default(),
        priority: data
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|p| p as i32)
            .unwrap_or(nemesis_board::models::priority::MEDIUM),
        creator: actor,
        acceptance_criteria: get_opt_str(data, "acceptance_criteria"),
        due_date: data.get("due_date").and_then(|v| v.as_i64()),
        parent_issue_id: data.get("parent_issue_id").and_then(|v| v.as_i64()),
        project_id: data.get("project_id").and_then(|v| v.as_i64()),
        ..NewIssue::default()
    };
    if let Some((at, aid)) = parse_assignee(data)? {
        ni.assignee = Some(at);
        ni.assignee_id = Some(aid);
    }
    // M2：职能 slug 直建单面（planner 之外的手工入口；空串/空白 = 不限）。
    if let Some(p) = get_opt_str(data, "required_profession") {
        ni.required_profession = (!p.trim().is_empty()).then_some(p);
    }
    if let Some(o) = get_opt_str(data, "origin_type") {
        ni.origin = Some(nemesis_board::models::TaskOrigin {
            origin_type: o,
            origin_id: get_opt_str(data, "origin_id").unwrap_or_default(),
        });
    }
    Ok(ni)
}

/// issue → worker 任务提示词（`issue.dispatch` 的 peer_chat content）。
/// worker 端按普通任务 prompt 走自己的 agent（工具/安全层照常生效），
/// 结尾固定要求结果汇报——最终回复是唯一回传渠道（经 callback 写回看板）。
#[cfg(feature = "cluster")]
/// 汇报格式五段真相源在 nemesis-board::report（交付线程首评 / M4 验收
/// agent 同源解析；此处只引用）。
use nemesis_board::report::REPORT_FORMAT_SECTION;

/// 派发提示词（impl-plan §3.3 结构化 PRD 模板）：背景/验收/交付物要求/
/// 汇报格式五段。description 是自由文本（人写）或结构化分段（planner 写），
/// 原文放「背景」段不做二次加工。唯一调用方 dispatch_issue_core（cluster），
/// 同 cfg 门控（REPORT_FORMAT_SECTION 是 cluster-only const）。
#[cfg(feature = "cluster")]
/// `review_feedback`（Swarm M4）：FAIL 重派时验收 agent 的差距意见，渲染
/// 成「上轮验收意见」段——worker 必须知道为什么被打回，否则重派就是掷
/// 骰子（§6.1 检视 2）。首派传 `None`。
/// `experience_section`（Swarm M4.5）：团队过往经验注入段（注入端已渲染），
/// 无匹配传 `None`——不塞空段，保持提示词字节与无经验时一致。
/// `attempt`（PR5，2026-10-10 机制加固）：本次派发序号（1 起；首派 `None`
/// 不渲染，prompt 字节与历史一致）。重派任务卡告知轮次 + 循环升级契约：
/// worker 不知道自己是第几回挨打，就无法判断「同一问题反复重派无解」，
/// 只会静默再写一轮（C1 无限重写循环的 worker 侧根因）。
fn build_dispatch_prompt(
    issue: &nemesis_board::Issue,
    assets_section: Option<&str>,
    review_feedback: Option<&str>,
    experience_section: Option<&str>,
    attempt: Option<u32>,
) -> String {
    let mut p = format!(
        "{} {}\n\n## 标题\n{}\n",
        nemesis_board::TASK_CARD_HEADER,
        issue.number,
        issue.title
    );
    p.push_str(&format!(
        "\n## 背景\n{}\n",
        if issue.description.trim().is_empty() {
            "（未提供）"
        } else {
            issue.description.trim()
        }
    ));
    p.push_str(&format!(
        "\n## 验收标准\n{}\n",
        match issue.acceptance_criteria.as_deref().map(str::trim) {
            Some(ac) if !ac.is_empty() => ac,
            _ => "（未提供——完成判据以背景描述为准，无法客观判定时在自检结果中说明）",
        }
    ));
    // Swarm M4（§6.1 检视 2）：重派上下文延续——上轮验收差距原文随包走。
    if let Some(feedback) = review_feedback
        && !feedback.trim().is_empty()
    {
        p.push_str(&format!(
            "\n## 上轮验收意见（本次重派原因，必须针对性整改）\n{}\n",
            feedback.trim()
        ));
    }
    // PR5（2026-10-10 机制加固）：重派轮次 + 循环升级通道（worker 侧编码）。
    // 只在重派时渲染（首派 prompt 字节与历史一致）。
    if let Some(n) = attempt {
        p.push_str(&format!(
            "\n## 重派轮次（第 {n} 次派发）\n本单此前已被派发 {} 次、验收未通过。若你判断同一问题按已有方法继续整改仍无法通过验收（连续多轮重派无解），不要再静默重写一轮：在最终回复正文里**单独一行**写行首标记 `REDISPATCH_LOOP:` 加一句话说明卡点，并在汇报的「偏差与疑点」节列出已尝试方案清单（每项一行：试了什么、结果如何）。主控端识别该标记后会停止自动重派、转人工裁决——这是升级通道，不是失败污点；诚实升级好过无限重写。\n",
            n - 1
        ));
    }
    // Swarm M3（§5.4 层 1）：issue 名下资产随包走（引用+签名 token；
    // 内容对端按需 HTTP 拉）。
    if let Some(section) = assets_section {
        p.push_str(section);
    }
    // Swarm M4.5（§6.5.2）：scope 匹配的团队经验随包走（经验段是参考，
    // 不是验收标准——提示词里写明）。
    if let Some(section) = experience_section {
        p.push_str(section);
    }
    p.push_str(
        "\n## 交付物要求\n\
         - 列出所有改动/新建文件的完整路径\n\
         - 涉及代码产出时给出 branch 名与 commit 列表（如适用）\n",
    );
    p.push_str(&format!(
        "\n## 汇报格式（最终回复必须严格按以下五段组织，标题原样保留）\n{REPORT_FORMAT_SECTION}\n\n\
         最终回复会原样写回看板任务，是唯一回传渠道。\n"
    ));
    p
}

/// 急停发车护栏（F-U4-5，2026-09-15 真机实证）：estop 生效中一切看板
/// 发车入口一律拒绝。与 `project.resume` 的「护栏三不变：estop 急停中
/// 拒绝恢复发车」同族——手动发车（issue.dispatch / issue.plan /
/// autopilot.run / project.create auto_start）都是发车面，此前唯独它们
/// 漏网（真机实证 estop ENGAGED 时 issue.dispatch 仍成功派出）。评审/
/// 清扫走保险丝挂起（board_review 检查点），语义不同不在此拦。
fn refuse_dispatch_when_estopped(ctx: &RequestContext) -> Result<(), String> {
    if ctx
        .state
        .estop
        .as_ref()
        .map(|e| e.is_engaged())
        .unwrap_or(false)
    {
        return Err("⛔ 急停（E-STOP）生效中：派发被拒绝（先释放急停）".to_string());
    }
    Ok(())
}

/// `issue.dispatch` 实现（cluster 编译时）：解析派发目标 →
/// [`dispatch_issue_core`]。先做纯本地校验（目标），状态/重复派发闸在
/// core 内、集群缺失最后报——错误信息更有指向性，且校验矩阵不依赖集群
/// 实例（可单测）。
#[cfg(feature = "cluster")]
async fn issue_dispatch(
    store: &Arc<BoardStore>,
    actor: Actor,
    ctx: &RequestContext,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    // F-U4-5：急停中拒绝发车（护栏，最便宜也最根本的一闸先行）。
    refuse_dispatch_when_estopped(ctx)?;
    let data = data.ok_or("missing data")?;
    let id = data
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("missing field: id")?;
    let issue = store.get_issue(id)?;

    // 目标解析：显式 target 优先，否则取 worker 指派；manager_self 由
    // coordinator 本机执行（P4 autopilot），不支持远端派发。
    let target = match get_opt_str(&data, "target") {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        Some(_) => return Err("target 不能为空".to_string()),
        None => match (&issue.assignee, &issue.assignee_id) {
            (Some(AssignmentType::Worker), Some(wid)) => wid.clone(),
            (Some(AssignmentType::ManagerSelf), _) => {
                return Err("manager_self 指派由 coordinator 本机执行，不支持远端派发".to_string());
            }
            _ => return Err("缺少派发目标：提供 target 或先把 issue 指派给 worker".to_string()),
        },
    };

    let cluster = ctx.state.cluster.clone();
    let out = dispatch_issue_core(store, cluster.as_ref(), id, &target, &actor, None)?;
    // §6.4D：手动重派 = 人类明确意图 → 失败账清零（M3「人工处理后计数作废」
    // 在手动路径成真；熔断停车后 reopen 重派不再因存量 count 秒熔断）。与评
    // 审链同源同一 JSON（nemesis-board fail_streak 单一真相源）；清账失败不
    // 阻断派发结果（护栏非主路径）。workspace 未注入 = 跳过（测试形态）。
    if let Some(ws) = ctx.state.workspace.as_deref() {
        nemesis_board::fail_streak::FailStreakLedger::load(std::path::Path::new(ws)).clear(id);
    }
    Ok(Some(out))
}

/// dispatch prompt 的「## 任务资产」段渲染（Swarm M3 §5.4 层 1）。签发
/// 上下文挂 store（gateway 装配 set 一次）——未注入 / issue 无资产逐级
/// 诚实降级为 None。
#[cfg(feature = "cluster")]
fn render_dispatch_assets_section(
    store: &Arc<BoardStore>,
    issue_id: i64,
) -> Result<Option<String>, String> {
    let Some(signing) = store.asset_signing() else {
        return Ok(None);
    };
    let assets = store.assets_for_issue(issue_id)?;
    Ok(nemesis_board::render_assets_section(
        &signing,
        &assets,
        nemesis_board::DEFAULT_TOKEN_TTL_SECS,
    ))
}

/// 派发经验注入的检索（M4.5 §6.5.2）：查库（滤 deprecated）→ scope 对
/// 任务文本（标题+描述+验收标准）与 required_tags 匹配 → top-N。返回
/// （渲染好的注入段, 命中条目 id——发车成功后由调用方计 use_count）。
/// 查库失败诚实降级为无注入（经验是增值段，任何异常都不阻断派发主线）。
#[cfg(feature = "cluster")]
fn render_issue_experience_section(
    store: &Arc<BoardStore>,
    issue: &nemesis_board::Issue,
) -> (Option<String>, Vec<i64>) {
    let entries = match store.list_team_memory(None, false) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("[Board] team_memory 读取失败，派发跳过经验注入：{e}");
            return (None, Vec::new());
        }
    };
    let issue_text = format!(
        "{}\n{}\n{}",
        issue.title,
        issue.description,
        issue.acceptance_criteria.as_deref().unwrap_or("")
    );
    let hits = nemesis_board::match_experiences(
        &entries,
        &issue_text,
        &issue.required_tags,
        nemesis_board::MAX_MATCHED_EXPERIENCES,
    );
    let ids: Vec<i64> = hits.iter().map(|e| e.id).collect();
    (
        nemesis_board::render_dispatch_experience_section(&hits),
        ids,
    )
}

/// P1 派发硬闸（2026-09-12 双端真机 S2 拓扑误杀根修）：远端目标 +
/// `file:` 锚点 = 拒绝派发。`file:` 锚点在派发端（看板权威端）workspace
/// 解析实核，而任务在远端节点 workspace 执行——执行者真实交付成功也会被
/// 假 FAIL（S2 实证：NB-16 烧光 2 轮重派预算转人工，B 端交付物早已存在）。
/// 本闸是**模型无关**的确定性拦截（planner 提示词拓扑纪律是软防线，这是
/// 硬防线；手动派发 / FAIL 重派 / autopilot 全走 dispatch_issue_core =
/// 单一漏斗全覆盖）。返回 Some(拒绝理由) = 拦截。
/// 纯函数（无 store/cluster 依赖），单测直测。
#[cfg(feature = "cluster")]
fn reject_remote_file_anchors(
    acceptance_criteria: Option<&str>,
    target: &str,
    local_node_id: &str,
    local_node_name: &str,
) -> Option<String> {
    let ac = acceptance_criteria?;
    let (anchors, _semantic, _rejected) = nemesis_board::anchor::parse_anchors(ac);
    let file_anchors: Vec<&str> = anchors
        .iter()
        .filter(|a| a.kind.is_file_anchor())
        .map(|a| a.raw.as_str())
        .collect();
    if file_anchors.is_empty() {
        return None;
    }
    // 目标 = 本节点（id 或名称，大小写不敏感）→ 同一 workspace，file: 锚点合法。
    let t = target.trim().to_lowercase();
    if t == local_node_id.trim().to_lowercase() || t == local_node_name.trim().to_lowercase() {
        return None;
    }
    Some(format!(
        "⛔ 拒绝派发：目标节点「{target}」是远端节点，而验收标准包含 {} 条 file: 锚点。\
file: 锚点在派发端 workspace 解析实核，远端任务的交付文件不在派发端 workspace——真实交付也会被误判 FAIL（拓扑误杀）。\
请把 file: 锚点改为 `re:` 型（对交付汇报文本实核，跨节点安全）后重试，或将任务派给本节点。\n涉及锚点：\n{}",
        file_anchors.len(),
        file_anchors
            .iter()
            .map(|raw| format!("- `{raw}`"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

// ---------------------------------------------------------------------------
// P4/E2：基线下发（看板项目档案 goal 合并批）
//
// BaselinePusher 走模块级 OnceLock 而非 AppState 字段（PARENT_REVIEW_HOOK
// 同款理由：AppState 全库字面构造测试点太多，加字段是断点级改动）。
// gateway 装配时注册；未注册（单测/极简装配）= 派发走既有无基线管线，
// 存量项目（无 directory）同样不参与——两条边界与 goal §P4/E2 一致。
// ---------------------------------------------------------------------------

/// 派发基线推送器：既有分块传输通路（transfer.begin/chunk/end，AEAD 鉴权）
/// + D4 护栏现读 provider（gateway 30s 热刷新循环注入的 cell）。
#[cfg(feature = "cluster")]
pub struct BaselinePusher {
    pub transport: Arc<dyn nemesis_cluster::outbox::TransferTransport>,
    pub source_node_id: String,
    pub max_bytes: Box<dyn Fn() -> u64 + Send + Sync>,
}

#[cfg(feature = "cluster")]
static BASELINE_PUSHER: std::sync::OnceLock<Arc<BaselinePusher>> = std::sync::OnceLock::new();

/// 装配基线推送器（gateway 启动期调用一次；重复装配 = 保留首份并返回
/// false——推送器不可变，语义安全）。
#[cfg(feature = "cluster")]
pub fn install_baseline_pusher(pusher: Arc<BaselinePusher>) -> bool {
    BASELINE_PUSHER.set(pusher).is_ok()
}

// ---------------------------------------------------------------------------
// EST-01/02（2026-09-16 横扫加固）：看板派发族的 estop 闸。
// estop 生效时全部自动/手动派发诚实冻结，而不是照常发车（此前 board 侧
// 完全不感知 estop——急停只冻结 agent loop，看板 cron/补派/停车场 sweep
// 会继续向 worker 派新任务）。同 BASELINE_PUSHER 理由：AppState 全库字面
// 构造测试点太多，加字段是断点级改动，故走模块级 OnceLock 注入。gateway
// 装配时注册；未注册（单测/极简装配）= 闸不生效（等价旧行为）。
// ---------------------------------------------------------------------------

/// 模块级 estop 状态槽（gateway 启动注入，与 AppState.estop 同一实例）。
#[cfg(feature = "cluster")]
static BOARD_ESTOP: std::sync::OnceLock<Arc<nemesis_agent::estop::EstopState>> =
    std::sync::OnceLock::new();

/// 装配看板 estop 闸（gateway 启动期调用一次；重复装配保留首份）。
#[cfg(feature = "cluster")]
pub fn install_board_estop(estop: Arc<nemesis_agent::estop::EstopState>) -> bool {
    BOARD_ESTOP.set(estop).is_ok()
}

/// estop 是否生效中（未装配 = false，等价旧无闸行为）。
#[cfg(feature = "cluster")]
fn board_estop_engaged() -> bool {
    estop_dispatch_frozen(BOARD_ESTOP.get().map(|e| e.as_ref()))
}

// ---------------------------------------------------------------------------
// U-A（2026-10-12 停车现场工程）：派发/拆解侧停车档案的工作区根。
// handoff 落盘需要 `<workspace>/board/handoffs/`，而派发/拆解族全是
// store+cluster 纯函数（AppState 字面量测试点太多，加参数是断点级改动）
// ——同 BASELINE_PUSHER/BOARD_ESTOP 理由走模块级 OnceLock 注入，gateway
// 装配时注册；未装配（单测/极简装配）= 停车档案缺席但评论照常（诚实降级）。
// ---------------------------------------------------------------------------

static HANDOFF_WORKSPACE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// 装配停车档案工作区根（gateway 启动期调用一次；重复装配保留首份）。
pub fn install_handoff_workspace(ws: std::path::PathBuf) -> bool {
    HANDOFF_WORKSPACE.set(ws).is_ok()
}

/// 工作区根访问器（U-B 中断档案 `<workspace>/board/interruptions/` 与
/// handoffs 同根复用；未装配 = None——中断档案静默缺席）。
pub fn handoff_workspace_root() -> Option<&'static std::path::PathBuf> {
    HANDOFF_WORKSPACE.get()
}

/// 派发/拆解侧停车档案落盘（board_review::park_with_handoff 的 web 侧
/// 简化形态：无 deps/评论拼接职责，只写档案；失败 WARN 不阻塞停车流）。
fn write_side_handoff(
    store: &BoardStore,
    issue: &nemesis_board::Issue,
    stop_kind: &'static str,
    suggested_action: &'static str,
    hint: String,
    resume_entry: &str,
    environment: serde_json::Value,
    attempted: Vec<serde_json::Value>,
) {
    let Some(ws) = HANDOFF_WORKSPACE.get() else {
        return; // 未装配（单测/极简装配）：档案缺席，评论照常
    };
    let spec = nemesis_board::handoff::HandoffSpec {
        stop_kind,
        suggested_action,
        hint,
        resume_entry: resume_entry.to_string(),
        environment,
        attempted,
    };
    let chain = nemesis_board::handoff::verdict_chain_from(store, issue.id);
    if let Err(e) = nemesis_board::handoff::write_handoff(ws, &issue.number, issue.id, &spec, chain)
    {
        tracing::warn!("[Board] {} 停车档案写失败（评论照发）：{e}", issue.number);
    }
}

/// estop 闸判定可测内核（BOARD_ESTOP 是进程级 OnceLock——engaged 态若在
/// 单测内联装配会与并行测试的派发路径互踩，故判定逻辑走纯函数，全局接线
/// 仅一行由 gateway 装配保证，同 BASELINE_PUSHER「单测不装配零波及」先例）。
#[cfg(feature = "cluster")]
fn estop_dispatch_frozen(estop: Option<&nemesis_agent::estop::EstopState>) -> bool {
    estop.is_some_and(|e| e.is_engaged())
}

/// estop 冻结拒绝文案（EST 族统一出口）。
#[cfg(feature = "cluster")]
fn estop_frozen_error() -> String {
    "⛔ 急停（estop）生效中，派发已冻结：`estop --release` 释放后恢复（定时/补派触发器会自动重派）"
        .to_string()
}

/// 派发基线下发（E2）。项目档案仓库幂等 ensure → HEAD 树导出 staging →
/// 分块推给 worker（对端收件箱 `<workspace>/cluster/inbox/<task_id>/`）。
/// 返回 `Ok(Some(commit))` = 档案管线派发（基线已落地并记账）；
/// `Ok(None)` = 非档案管线（推送器未装配 / 单未绑项目 / 存量项目无
/// directory），走既有无基线路径；`Err` = 诚实弃派（调用方清理）。
///
/// **版本混布停车**（goal E2 边界②）：旧 worker 无 transfer handler，begin
/// RPC 得 "no handler" error 帧 → 本函数 Err → 派发诚实停车，提示升级。
///
/// 同步桥：dispatch_issue_core 是 sync（fire_autopilot/auto_dispatch_* /
/// dispatch_subissue_auto 同步链直通 cron，async 化是断点级改动），推送
/// 用 block_in_place 桥（adapters 先例；仅推送器已装配的生产路径到达，
/// 单测不装配零波及）。
#[cfg(feature = "cluster")]
fn push_dispatch_baseline(
    store: &Arc<BoardStore>,
    issue: &nemesis_board::Issue,
    task_id: &str,
    target: &str,
) -> Result<Option<String>, String> {
    let Some(pusher) = BASELINE_PUSHER.get() else {
        return Ok(None);
    };
    let Some(project_id) = issue.project_id else {
        return Ok(None);
    };
    let project = store.get_project(project_id)?;
    let Some(dir) = project.directory.as_ref() else {
        return Ok(None);
    };
    let root = std::path::PathBuf::from(dir);
    // 幂等 ensure（P4 部署后首次派发补 init + 对现有内容首 commit）。
    nemesis_board::git_repo::ensure_repo(&root)?;
    let commit = nemesis_board::git_repo::head_commit_hex(&root)?
        .ok_or_else(|| "项目仓库无 HEAD commit（空仓库且 init commit 失败？）".to_string())?;
    // HEAD 树导出临时 staging（推送完即删；task 级一次性目录）。
    let staging = std::env::temp_dir().join(format!(
        "nmb-baseline-{}",
        nemesis_cluster::transfer::sanitize_transfer_id(task_id)
    ));
    let export = nemesis_board::git_repo::export_head_tree(&root, &staging);
    if let Err(e) = export {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("基线导出失败: {e}"));
    }
    tracing::info!(
        task_id = %task_id,
        target = %target,
        baseline = %commit,
        "[Board] 基线下发开始（E2 分块传输）"
    );
    let pushed = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            nemesis_cluster::outbox::push_transfer_dir(
                pusher.transport.as_ref(),
                target,
                task_id,
                nemesis_cluster::transfer::TRANSFER_KIND_PROJECT_BASELINE,
                &pusher.source_node_id,
                &staging,
                (pusher.max_bytes)(),
            )
            .await
        })
    });
    let _ = std::fs::remove_dir_all(&staging);
    match pushed {
        // 空仓库/空树 = NothingToSend：worker 收 `_baseline_commit` 后按空
        // 基线开跑（goal 空集宽容），基线行照记（管线归属不变）。
        Ok(nemesis_cluster::outbox::PushDirOutcome::Delivered)
        | Ok(nemesis_cluster::outbox::PushDirOutcome::NothingToSend) => {
            store.set_dispatch_baseline(task_id, &commit)?;
            tracing::info!(
                task_id = %task_id,
                target = %target,
                baseline = %commit,
                "[Board] 基线下发完成"
            );
            Ok(Some(commit))
        }
        Ok(nemesis_cluster::outbox::PushDirOutcome::Overlimit { total_bytes, limit }) => {
            Err(format!(
                "基线快照 {total_bytes} 字节超护栏 {limit}（board.archive.max_transfer_bytes）——诚实弃派，不截断"
            ))
        }
        Err(e) => Err(if e.contains("no handler") || e.contains("未知") {
            format!("{e}（对端不支持档案基线传输——worker 版本过旧，请升级 worker 后重派）")
        } else {
            e
        }),
    }
}

/// 派发闸 → 登记 task + 派发绑定 → 推进 in_progress → fire-and-forget 发
/// peer_chat RPC。`issue.dispatch`（WSAPI）与 gateway 的 autopilot 定时
/// 触发共用（单一真相源）。`cluster` 传 `None` 时本地闸先行、集群缺失
/// 最后报（校验矩阵不依赖集群实例，可单测）。`review_feedback`（Swarm
/// M4）为 FAIL 重派专用——验收差距渲染进 dispatch prompt「上轮验收意见」
/// 段；常规派发传 `None`。
#[cfg(feature = "cluster")]
pub fn dispatch_issue_core(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue_id: i64,
    target: &str,
    actor: &Actor,
    review_feedback: Option<&str>,
) -> Result<serde_json::Value, String> {
    // EST-01/02（派发单一入口 = 唯一落点）：estop 生效中一律诚实拒绝——
    // 释放后既有触发器（autopilot cron / 补派 / 停车场 sweep / 评审 fuse）
    // 自然覆盖恢复，无新增停车机制。
    if board_estop_engaged() {
        return Err(estop_frozen_error());
    }
    let issue = store.get_issue(issue_id)?;

    // P5/F2 冲突冻结闸（派发单一入口 = 唯一落点）：项目冲突冻结中一律
    // 诚实拒绝——解冻只有一条路（project.resume 补合并回放），不存在旁路。
    if let Some(pid) = issue.project_id
        && let Ok(project) = store.get_project(pid)
        && project.conflict_frozen
    {
        return Err(format!(
            "项目「{}」冲突冻结中，禁止派发（含重派/兜底/autopilot）：先 project.resume 人工落定冲突并补合并冻结期交付",
            project.name
        ));
    }

    // 状态闸：blocked/终态不可派发；backlog/todo/in_review 派发即转
    // in_progress（都合法）。
    match issue.status {
        IssueStatus::Backlog
        | IssueStatus::Todo
        | IssueStatus::InProgress
        | IssueStatus::InReview => {}
        other => return Err(format!("issue 处于 {other} 状态，不可派发")),
    }

    // 已有未完结派发 → 拒绝重复派发。
    if store.has_active_dispatch(issue.id)? {
        return Err("该 issue 已有进行中的派发（等 worker 回报或超时后再试）".to_string());
    }

    // 派发 = 给远端 worker 发 peer_chat，集群必需。
    let cluster = cluster.ok_or("集群未运行，无法派发（issue.dispatch 需要集群）")?;

    // 单一真相源（T37 双身份失配第三处落点，2026-09-13）：账本 worker_id
    // 必须与 worker 上报的传输层身份（_rpc.from = 运行时节点 id）同形态
    // ——人工指派/兜底可能给 peer 名（"Alex"），不归一化则 task.started /
    // delivery.files 的 worker 校验永远失配。matcher 产出已是节点 id，此
    // 步幂等。
    // F1（2026-09-30 派发目标校验前置）：解析失败 = 注册表无此节点，直接
    // 拒绝——旧行为放行原名去 RPC 撞墙，API 层却返回 dispatched=true（行为
    // 分裂，真机幽灵 target 实证）。归一化（含大小写回落）见
    // Cluster::canonical_peer_id。
    let target = cluster
        .canonical_peer_id(target)
        .ok_or_else(|| format!("未知节点：{target}（不在集群注册表中，请核对节点名或 id）"))?;

    // P1 拓扑硬闸（模型无关）：远端目标 + file: 锚点 = 拒绝（软防线是
    // planner 提示词拓扑纪律；手动派发/重派/autopilot 都过这道闸）。
    if let Some(reason) = reject_remote_file_anchors(
        issue.acceptance_criteria.as_deref(),
        &target,
        cluster.node_id(),
        &cluster.node_name(),
    ) {
        return Err(reason);
    }

    // M4.5：经验注入段在 prompt 组装前检索（命中计数待发车成功后落）。
    let (experience_section, experience_ids) = render_issue_experience_section(store, &issue);

    // PR5（2026-10-10 机制加固）：重派轮次 = 历史（已完结）派发行数 + 1。
    // 首派 0 行 → None（不渲染重派段）；重派 → Some(N) 渲染轮次 + 循环
    // 升级契约。行数口径含失败派发——worker 需要的是「这是第几回」的完整
    // 感知，UNAVAILABLE 等未跑完的尝试同样计入挨打次数。
    let attempt_count = store.list_dispatches(issue.id)?.len() as u32;
    let attempt = (attempt_count > 0).then_some(attempt_count + 1);

    let prompt = build_dispatch_prompt(
        &issue,
        render_dispatch_assets_section(store, issue.id)?.as_deref(),
        review_feedback,
        experience_section.as_deref(),
        attempt,
    );

    // 1. 登记本地 task（与 dashboard peer_chat 同契约），拿 task_id。
    //    worker 端会话键 = cluster_rpc:{source}/board:{number}：同 issue 多次
    //    派发收敛到同一 worker 会话（上下文延续）。
    let source_node_id = cluster.node_id().to_string();
    let chat_id = format!("board:{}", issue.number);
    let source_payload = serde_json::json!({
        "node_id": source_node_id,
        "channel": "board",
        "chat_id": chat_id,
    });
    // M2：职能 slug 随派发载荷下行（顶层键；旧 B 端忽略未知键 = 优雅
    // 降级）。A 端只传 slug，契约文本由 B 端本地渲染（跨 RPC 不传文本）。
    let mut chat_payload = serde_json::json!({ "content": prompt, "_source": source_payload });
    if let Some(p) = issue.required_profession.as_deref().map(str::trim)
        && !p.is_empty()
    {
        chat_payload["required_profession"] = serde_json::json!(p);
    }
    let task_id =
        cluster.submit_peer_chat(&target, "peer_chat", chat_payload, "board", &chat_id)?;

    // M4.5：注入计数只认实际发车的派发（提交失败不计 use_count）。
    if !experience_ids.is_empty()
        && let Err(e) = store.mark_team_memory_used(&experience_ids)
    {
        tracing::warn!("[Board] team_memory use_count 更新失败（不影响派发）：{e}");
    }

    // P4/E2（看板项目档案 goal 合并批）：项目档案基线下发。推送器未装配 /
    // 单未绑项目 / 存量项目无 directory = Ok(None)（走既有无基线路径）；
    // Err = **诚实弃派**——此刻 insert_dispatch 还没执行，fail_task 清理
    // 本地 task + 系统评论留痕即可，不留悬挂 dispatched 态（版本混布时旧
    // worker 无 transfer handler，在这里免费获得停车）。
    let baseline_commit = match push_dispatch_baseline(store, &issue, &task_id, &target) {
        Ok(v) => v,
        Err(e) => {
            cluster.fail_task(&task_id, &e);
            let _ = store.add_comment(nemesis_board::models::NewComment {
                issue_id: issue.id,
                author: nemesis_board::Actor::system("board"),
                content: format!("⛔ 派发中止：{e}"),
                parent_id: None,
                ctype: nemesis_board::CommentType::System,
            });
            return Err(format!("派发中止：{e}"));
        }
    };

    // 2. 登记派发绑定（peer_chat_callback 的写回路由键）+ 审计活动。
    //    原子占用闸（R5-BUG-2 根修）：claim 失败 = 并发路径（依赖闸补派
    //    × 停车场 sweep 等）已占用该 issue → 本次 submit 的 task 诚实
    //    取消，不写行、不留幽灵 dispatched（幽灵行会永久占满 worker
    //    inflight，后续派发全部静默 deferred）。
    if !store.try_claim_dispatch(&task_id, issue.id, &target, actor)? {
        cluster.fail_task(&task_id, "并发派发竞态：该 issue 已有进行中的派发");
        return Err("该 issue 已有进行中的派发（并发派发竞态，本次派发已取消）".to_string());
    }

    // C 里程碑 2（看板项目档案 goal P2）：派发落定 → records/NB-xx/
    // dispatch.md + timeline（P4 起第 5 参填基线 commit——执行档案 merge
    // 的 base 对照锚）。失败不阻塞派发（writer 内部 WARN+审计）；存量
    // 项目静默跳过。
    nemesis_board::archive_writer::write_dispatch_milestone(
        store,
        &issue,
        &target,
        &task_id,
        baseline_commit.as_deref(),
    );

    // 3. 状态推进：→ in_progress（状态机转移，写 status_change 审计）。
    //    转移失败（人工同刻挪状态 / 竞态余波）→ 回滚本次 claim 的 dispatch
    //    行（置 failed）+ 取消 task——行生命周期与派发决策同生共死，不留
    //    幽灵 dispatched（R5-BUG-2 对称彻底）。
    //    F2（2026-09-30）：派发前快照（状态+指派）随 RPC 闭包下行——送达
    //    失败（任务从未开始）时把推进出的 in_progress 回退到派发前状态、
    //    指派恢复派发前值（rollback_dispatch_failure 守卫竞态）。
    let dispatch_prev_status = issue.status;
    let dispatch_prev_assignee = issue.assignee;
    let dispatch_prev_assignee_id = issue.assignee_id.clone();
    let issue = if issue.status != IssueStatus::InProgress {
        match store.transition_issue(issue.id, IssueStatus::InProgress, actor) {
            Ok(i) => i,
            Err(e) => {
                let _ =
                    store.finish_dispatch(&task_id, nemesis_board::models::dispatch_state::FAILED);
                cluster.fail_task(&task_id, &format!("派发中止：状态转移失败（{e}）"));
                return Err(format!("派发中止：状态转移失败：{e}"));
            }
        }
    } else {
        issue
    };
    // F2 项目状态联动：项目绑定的单实际派出 → 项目 active → in_progress
    // （首派语义；已在 in_progress/completed 的项目不回退不重复触发）。
    link_project_on_dispatch(store, issue.project_id);

    // 派发即指派（2026-09-17）：派发目标落 assignee 字段。显式 target /
    // 匹配器 / 兜底 / D3 换节点重派四路都汇经本单一入口——此前派发只路由
    // 不回填，看板「指派」列在任务执行完仍显示未指派（字段与事实脱节）。
    // 已指向同一 worker 时跳过（幂等，防重复 activity）；回填失败不阻断
    // 派发（此刻派发已落定，字段只是显示层事实）——WARN 留痕。显式
    // issue.assign 语义不变，仍是人工指派入口。
    if (issue.assignee != Some(AssignmentType::Worker)
        || issue.assignee_id.as_deref() != Some(target.as_str()))
        && let Err(e) = store.assign_issue(
            issue.id,
            Some(AssignmentType::Worker),
            Some(target.to_string()),
            actor,
        )
    {
        tracing::warn!("[Board] 派发回填指派失败（不阻断派发）：{e}");
    }

    // 4. 发 RPC（fire-and-forget）：ACK 后 worker 异步处理，回报走
    //    peer_chat_callback。目标不可达时立刻终结派发 + 系统评论留痕
    //    （callback 不会再来，不留悬挂 dispatched 态）。
    let issue_id = issue.id;
    let rpc_client = cluster.rpc_client_arc().ok_or("RPC client not available")?;
    // E2：档案管线派发带基线 commit（worker 侧以收件箱基线为起点开跑）。
    let mut payload = serde_json::json!({
        "content": prompt,
        "task_id": task_id,
        "_source": source_payload,
        // B 端首次收到本节点 peer_chat 时按此端口注册回调地址
        // （gateway peer_chat handler；缺省回落 DEFAULT_RPC_PORT=21949，
        // 回调会打到死端口——与 ClusterRpcTool 的 wire 契约对齐）。
        "_source_rpc_port": cluster.rpc_port(),
    });
    if let Some(commit) = &baseline_commit {
        payload["_baseline_commit"] = serde_json::json!(commit);
    }
    if let Some(p) = issue
        .required_profession
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        payload["required_profession"] = serde_json::json!(p);
    }
    let request = nemesis_cluster::rpc_types::RPCRequest {
        id: task_id.clone(),
        action: nemesis_cluster::rpc_types::ActionType::Known(
            nemesis_cluster::rpc_types::KnownAction::PeerChat,
        ),
        payload,
        source: source_node_id,
        target: Some(target.to_string()),
    };
    let store_for_rpc = store.clone();
    let task_id_for_rpc = task_id.clone();
    let target_for_rpc = target.to_string();
    let prev_status_for_rpc = dispatch_prev_status;
    let prev_assignee_for_rpc = dispatch_prev_assignee;
    let prev_assignee_id_for_rpc = dispatch_prev_assignee_id.clone();
    tokio::spawn(async move {
        let timeout = std::time::Duration::from_secs(30);
        match rpc_client
            .call_with_timeout(&target_for_rpc, request, timeout)
            .await
        {
            Ok(_) => {
                tracing::info!("[Board] dispatch RPC ACK received (task_id={task_id_for_rpc})");
            }
            Err(e) => {
                tracing::warn!("[Board] dispatch RPC send failed (task_id={task_id_for_rpc}): {e}");
                let _ = store_for_rpc.finish_dispatch(
                    &task_id_for_rpc,
                    nemesis_board::models::dispatch_state::FAILED,
                );
                // F2（2026-09-30）：送达失败 = 任务从未开始 → 状态回退到
                // 派发前值 + 指派恢复（守卫内部自含：人工同刻挪过状态就让
                // 位）。失败仅留日志（⛔ 评论与审计已在下面/rollback 内落）。
                match store_for_rpc.rollback_dispatch_failure(
                    issue_id,
                    prev_status_for_rpc,
                    prev_assignee_for_rpc,
                    prev_assignee_id_for_rpc,
                    &nemesis_board::Actor::system("board"),
                ) {
                    Ok(true) => tracing::info!(
                        "[Board] 送达失败回退完成（issue={issue_id} → {}）",
                        prev_status_for_rpc
                    ),
                    Ok(false) => {}
                    Err(re) => tracing::warn!("[Board] 送达失败回退未执行（让位或异常）：{re}"),
                }
                let _ = store_for_rpc.add_comment(nemesis_board::models::NewComment {
                    issue_id,
                    author: nemesis_board::Actor::system("board"),
                    content: format!(
                        "⛔ 派发失败：[{}] RPC 送达失败（{e}）",
                        e.failure_category()
                    ),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
            }
        }
    });

    Ok(serde_json::json!({
        "dispatched": true,
        "task_id": task_id,
        "issue": issue_to_view(store, &issue)?,
    }))
}

/// `issue.dispatch`（未编译 cluster）：派发即发集群 RPC，无从谈起。
#[cfg(not(feature = "cluster"))]
async fn issue_dispatch(
    _store: &Arc<BoardStore>,
    _actor: Actor,
    _ctx: &RequestContext,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("issue.dispatch 需要集群支持（cluster feature 未编译）".to_string())
}

// ---------------------------------------------------------------------------
// Swarm M1：issue.plan 两段式 AI 拆解 + 依赖补派（2026-09-09 swarm-goal §1）
//
// 两段式：`issue.plan {id}`（confirm 缺省/false）异步跑 planner（裸提示词
// detached LLM 调用，无人格/零工具），产出推送 `board.plan_ready` /
// `board.plan_failed`（SSE/WS push），返回 plan_id；`issue.plan {id,
// plan_id, confirm:true}` 取缓存落库（parent/required_*/origin=planner）
// + 批内依赖边（序号→真实 id 映射）+ 依赖闸派发波 + 父单联动。
// ---------------------------------------------------------------------------

/// plan 预览缓存条目（一段产出 → 二段确认的中间态）。
#[cfg(feature = "cluster")]
struct PlanPreview {
    issue_id: i64,
    subs: Vec<nemesis_board::PlannedSubIssue>,
    /// D9 末轮宽和降级明细：`(子单序号, 原始非法值)`——人工确认发车时
    /// 随 confirm_plan 落系统评论（auto_confirm 直发路径同源）。
    downgrades: Vec<(usize, String)>,
    created_at: std::time::Instant,
}

/// 全自动流转 P1（A3）：父单收口验收钩子。模块级 static 而非 AppState
/// 字段——AppState 全库字面构造测试点太多，加字段是断点级改动（与下方
/// PLAN_CACHE 同款理由）。gateway 装配时注册；hook 内部自读
/// `board.auto_close_parent` 旗标（false = no-op 等价现行为），本 crate
/// 只负责在子单全部落定路径上触发。
static PARENT_REVIEW_HOOK: std::sync::OnceLock<std::sync::Arc<dyn Fn(i64) + Send + Sync>> =
    std::sync::OnceLock::new();

/// 注册父单收口验收钩子（gateway 启动装配调用一次；重复注册拒绝）。
pub fn set_parent_review_hook(
    hook: std::sync::Arc<dyn Fn(i64) + Send + Sync>,
) -> Result<(), String> {
    PARENT_REVIEW_HOOK
        .set(hook)
        .map_err(|_| "parent review hook already set".to_string())
}

/// 全自动流转 P4（F3）：项目收口验收钩子。同 PARENT_REVIEW_HOOK 的模块级
/// static 理由（AppState 字面构造测试点太多）；gateway 装配时注册，hook
/// 内部自读 `board.review.auto_close_project` 旗标（false = no-op）。
static PROJECT_REVIEW_HOOK: std::sync::OnceLock<std::sync::Arc<dyn Fn(i64) + Send + Sync>> =
    std::sync::OnceLock::new();

/// 注册项目收口验收钩子（gateway 启动装配调用一次；重复注册拒绝）。
pub fn set_project_review_hook(
    hook: std::sync::Arc<dyn Fn(i64) + Send + Sync>,
) -> Result<(), String> {
    PROJECT_REVIEW_HOOK
        .set(hook)
        .map_err(|_| "project review hook already set".to_string())
}

/// 看板项目档案 P6（F9）：项目收口**总结**钩子——人工路径（project.update
/// status=completed）触发；gateway 装配时注册（→ nemesisbot
/// `spawn_project_summary`，内部自守门：estop/tier/档案目录缺失诚实跳过，
/// 生成失败不阻塞收口状态机）。与上方评审钩子分离：总结是锦上添花，
/// 不做验收判定，也不读 auto_close_project 旗标。
static PROJECT_SUMMARY_HOOK: std::sync::OnceLock<std::sync::Arc<dyn Fn(i64) + Send + Sync>> =
    std::sync::OnceLock::new();

/// 注册项目收口总结钩子（gateway 启动装配调用一次；重复注册拒绝）。
pub fn set_project_summary_hook(
    hook: std::sync::Arc<dyn Fn(i64) + Send + Sync>,
) -> Result<(), String> {
    PROJECT_SUMMARY_HOOK
        .set(hook)
        .map_err(|_| "project summary hook already set".to_string())
}

/// M5-1（2026-10-10 机制加固）：评审重触发钩子——卡 in_review 的单子人工
/// 一键重入验收评审（WSAPI `review.retrigger`，替代 estop-pulse hack）。
/// gateway 装配时注册（spawn_board_review + master 角色闸在闭包内）；
/// 未安装时 WSAPI 诚实报错。
static REVIEW_RETRIGGER_HOOK: std::sync::OnceLock<std::sync::Arc<dyn Fn(i64) + Send + Sync>> =
    std::sync::OnceLock::new();

/// 注册评审重触发钩子（gateway 启动装配调用一次；重复注册拒绝）。
pub fn set_review_retrigger_hook(
    hook: std::sync::Arc<dyn Fn(i64) + Send + Sync>,
) -> Result<(), String> {
    REVIEW_RETRIGGER_HOOK
        .set(hook)
        .map_err(|_| "review retrigger hook already set".to_string())
}

/// F3 项目收口触发的聚合前置检查（纯 store 查询；单测直测）：项目存在、
/// 状态 active/in_progress、非空、全部顶层父单 done。cancelled 子单的
/// 范围缺口闸在评审侧（review_project_completion）复核，此处不做子孙
/// 遍历（触发面保持便宜；误触发由评审侧诚实跳过兜底）。
#[cfg(feature = "cluster")]
pub fn project_completion_eligible(store: &Arc<BoardStore>, project_id: i64) -> bool {
    let Ok(project) = store.get_project(project_id) else {
        return false;
    };
    if !matches!(project.status.as_str(), "active" | "in_progress") {
        return false;
    }
    let Ok(top) = store.list_issues(&nemesis_board::models::IssueFilter {
        project_id: Some(project_id),
        ..Default::default()
    }) else {
        return false;
    };
    let parents: Vec<_> = top
        .into_iter()
        .filter(|i| i.parent_issue_id.is_none())
        .collect();
    !parents.is_empty() && parents.iter().all(|p| p.status == IssueStatus::Done)
}

/// F3 项目收口触发面：顶层父单落 done 时调用（on_issue_settled 与
/// review_parent_issue PASS→done 两条路径）。前置检查不过 = no-op；
/// 过了才 fire hook（旗标判定在 hook 内）。
#[cfg(feature = "cluster")]
pub fn notify_project_review_on_parent_done(store: &Arc<BoardStore>, issue_id: i64) {
    let Ok(issue) = store.get_issue(issue_id) else {
        return;
    };
    // 只有顶层父单是项目收口的触发面（子单 done 走父单聚合）。
    let Some(project_id) = (issue.parent_issue_id.is_none())
        .then_some(issue.project_id)
        .flatten()
    else {
        return;
    };
    if !project_completion_eligible(store, project_id) {
        return;
    }
    if let Some(hook) = PROJECT_REVIEW_HOOK.get() {
        hook(project_id);
    }
}

/// 单子单环节推导（goal P1/C2 徽标 + C1 聚合共用的单一真相源）。
/// 停车判定与 sweep 同源（PARK_NOTICE_MARK 系统评论）；「执行中/待重派」
/// 以在途派发区分（无在途的 in_progress = 打回重做单，属 D 恢复候选）。
fn issue_stage(store: &BoardStore, issue: &nemesis_board::Issue) -> Result<&'static str, String> {
    Ok(match issue.status {
        IssueStatus::Done => "已完成",
        IssueStatus::InReview => "验收中",
        IssueStatus::Blocked => "受阻",
        IssueStatus::InProgress => {
            if store.has_active_dispatch(issue.id)? {
                "执行中"
            } else {
                "待重派"
            }
        }
        IssueStatus::Backlog | IssueStatus::Todo => {
            let parked = store
                .last_system_comment(issue.id)?
                .map(|c| c.contains(PARK_NOTICE_MARK))
                .unwrap_or(false);
            if parked { "已停车" } else { "待派发" }
        }
        IssueStatus::Cancelled => "已取消",
    })
}

/// 项目进度聚合（goal P1/C1+C2）：单项目子单按环节计数 + 当前卡点环节 +
/// 逐单环节数据（前端 ProjectPanel 进度行/环节徽标的数据源）。环节推导
/// 规则见 goal §三 P1/C2——停车判定与 sweep 同源（PARK_NOTICE_MARK 系统
/// 评论）；「执行中/待重派」以在途派发区分（无在途的 in_progress = 打回
/// 重做单，属 D 恢复候选，拍板①2026-09-13 精化）。
/// 看板项目档案 P6（F11）：档案 manifest 完整性投影（directory 下的
/// project.json）。(None, []) = 未绑定目录/manifest 不可读——诚实缺省，
/// 前端隐藏完整性徽章（不臆造 ok）。
fn archive_integrity_of(project: &nemesis_board::models::Project) -> (Option<String>, Vec<String>) {
    let Some(dir) = project.directory.as_deref() else {
        return (None, Vec::new());
    };
    match nemesis_board::archive::read_manifest(std::path::Path::new(dir)) {
        Some(m) => (Some(m.integrity), m.missing_blocks),
        None => (None, Vec::new()),
    }
}

pub(crate) fn project_progress(
    store: &BoardStore,
    project_id: i64,
) -> Result<serde_json::Value, String> {
    let issues = store.list_issues(&nemesis_board::models::IssueFilter {
        project_id: Some(project_id),
        ..Default::default()
    })?;

    let (
        mut n_done,
        mut n_dispatched,
        mut n_redo,
        mut n_review,
        mut n_parked,
        mut n_backlog,
        mut n_blocked,
        mut n_cancelled,
    ) = (0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
    let mut rows = Vec::with_capacity(issues.len());
    for issue in &issues {
        let stage = issue_stage(store, issue)?;
        match stage {
            "已完成" => n_done += 1,
            "执行中" => n_dispatched += 1,
            "待重派" => n_redo += 1,
            "验收中" => n_review += 1,
            "已停车" => n_parked += 1,
            "待派发" => n_backlog += 1,
            "受阻" => n_blocked += 1,
            _ => n_cancelled += 1,
        }
        rows.push(serde_json::json!({
            "id": issue.id,
            "number": issue.number,
            "title": issue.title,
            "status": issue.status.as_str(),
            "stage": stage,
        }));
    }

    // 卡点环节推导（优先级：受阻 > 停车 > 执行中 > 待重派 > 验收中 > 待派发）
    let stage = if issues.is_empty() {
        "未拆解"
    } else if n_blocked > 0 {
        "受阻"
    } else if n_parked > 0 {
        "停车待恢复"
    } else if n_dispatched > 0 {
        "执行中"
    } else if n_redo > 0 {
        "待重派"
    } else if n_review > 0 {
        "验收中"
    } else if n_backlog > 0 {
        "待派发"
    } else {
        "已完成"
    };

    // F11（看板项目档案 P6）：档案完整性投影（单项目详情行）。
    let (integrity, missing) = match store.get_project(project_id) {
        Ok(p) => archive_integrity_of(&p),
        Err(_) => (None, Vec::new()),
    };

    Ok(serde_json::json!({
        "project_id": project_id,
        "total": issues.len(),
        "counts": {
            "done": n_done,
            "dispatched": n_dispatched,
            "in_progress": n_redo,
            "in_review": n_review,
            "parked": n_parked,
            "backlog": n_backlog,
            "blocked": n_blocked,
            "cancelled": n_cancelled,
        },
        "stage": stage,
        "archive_integrity": integrity,
        "archive_missing_blocks": missing,
        "issues": rows,
    }))
}

/// 全项目摘要聚合（`project.progress` 不带 project_id）：每项目一行
/// counts+stage，不含逐单 rows——前端项目列表一次拉取全部进度。
pub(crate) fn project_progress_all(store: &BoardStore) -> Result<serde_json::Value, String> {
    let projects = store.list_projects()?;
    let mut rows = Vec::with_capacity(projects.len());
    for p in &projects {
        let full = project_progress(store, p.id)?;
        // F11（看板项目档案 P6）：列表行带完整性投影（⚠ 徽章数据源）。
        let (integrity, missing) = archive_integrity_of(p);
        rows.push(serde_json::json!({
            "project_id": p.id,
            "name": p.name,
            "status": p.status,
            "total": full["total"],
            "counts": full["counts"],
            "stage": full["stage"],
            "archive_integrity": integrity,
            "archive_missing_blocks": missing,
        }));
    }
    Ok(serde_json::json!({ "projects": rows }))
}

/// D（goal P2）项目级恢复发车（拍板①：项目域内重试全部可派未派子单，
/// 派发顺序由依赖闸/AI 自行决定；dry_run=true 只出预览不落任何副作用）。
///
/// 候选定义（拍板①2026-09-13 精化）：非终态 && 无在途派发 && 非验收中 &&
/// 非父单（有子单的单不是派发单元）&& 非 blocked（blocked=人工把关信号，
/// 解除后自然进入候选）。
///
/// B1 明细（goal P2/D 顺序依赖条款，B1 就绪后生效）：无匹配候选在
/// dry_run 预览行与执行失败行附 `match_detail`（要求 vs 每个在线候选差
/// 什么），不再是泛化文案。
#[cfg(feature = "cluster")]
pub async fn project_resume(
    store: &Arc<BoardStore>,
    cluster: &Arc<nemesis_cluster::cluster::Cluster>,
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    project_id: i64,
    dry_run: bool,
    actor: &Actor,
) -> Result<serde_json::Value, String> {
    // P5/F3+F4 冲突冻结先行段：冻结中的项目 resume = 「人工落定冲突 →
    // 清冻结 → commit worktree → 串行补合并冻结期交付」回放链，然后视
    // 结果决定是否继续下方既有派发流。dry_run 只出预览不落任何副作用。
    let mut replay_summary: Option<serde_json::Value> = None;
    if store.get_project(project_id)?.conflict_frozen {
        if dry_run {
            return Ok(serde_json::json!({
                "dry_run": true,
                "frozen": true,
                "note": "项目冲突冻结中：执行 resume 将先人工冲突落定 commit → 补合并冻结期交付，再恢复派发（如补合并再冲突会重新冻结并停车回人工）",
            }));
        }
        let hook = RESUME_REPLAY_HOOK
            .get()
            .ok_or("resume 回放钩子未安装（gateway 未装配档案合并依赖）")?;
        let replay = hook(project_id)?;
        // 补合并再冲突 → 重新冻结：停在这里回人工（resume 是人工在场流程，
        // 不进 auto 硬解），剩余待补队列逐条 pop 语义下天然保留。
        if store.get_project(project_id)?.conflict_frozen {
            return Ok(serde_json::json!({
                "dry_run": false,
                "resumed": false,
                "frozen": true,
                "replay": replay,
                "note": "补合并再次冲突 → 项目重新冻结回人工；解决冲突后再次 project.resume",
            }));
        }
        replay_summary = Some(replay);
    }

    let issues = store.list_issues(&nemesis_board::models::IssueFilter {
        project_id: Some(project_id),
        ..Default::default()
    })?;

    // 候选：非终态、无在途派发、非父单、非验收中。
    let mut candidates: Vec<(&nemesis_board::Issue, Option<String>)> = Vec::new();
    for issue in &issues {
        if matches!(
            issue.status,
            IssueStatus::Done | IssueStatus::Cancelled | IssueStatus::InReview
        ) {
            continue;
        }
        if !store.list_children(issue.id).unwrap_or_default().is_empty() {
            continue; // 父单：恢复作用在叶子派发单元。
        }
        if store.has_active_dispatch(issue.id).unwrap_or(true) {
            continue; // 查询失败保守=视为有在途（不重复派）。
        }
        let target = pick_target_by_matcher(store, cluster, issue)
            .or_else(|| pick_fallback_target(board_cfg, store, cluster, issue).map(|(t, _)| t));
        candidates.push((issue, target));
    }

    // B1 明细数据源：与 dispatch 路径同款在线候选快照（只读，供无匹配
    // 候选附「差什么」明细；goal P2/D 顺序依赖条款，B1 就绪后接入）。
    let peers = project_dispatch_candidates(cluster);

    if dry_run {
        let preview: Vec<serde_json::Value> = candidates
            .iter()
            .map(|(i, t)| {
                let mut row = serde_json::json!({
                    "issue_id": i.id,
                    "number": i.number,
                    "title": i.title,
                    "target": t.clone().unwrap_or_else(|| "无匹配（兜底未开或无在线节点）".into()),
                });
                if t.is_none() {
                    row["match_detail"] =
                        serde_json::Value::String(match_failure_detail(i, &peers));
                }
                row
            })
            .collect();
        return Ok(serde_json::json!({ "dry_run": true, "candidates": preview }));
    }

    // 执行：逐单走既有派发管线（兜底/预算/审计照常）。
    let mut dispatched = 0usize;
    let mut failed: Vec<serde_json::Value> = Vec::new();
    for (issue, target) in &candidates {
        match target {
            Some(t) => match dispatch_issue_core(store, Some(cluster), issue.id, t, actor, None) {
                Ok(_) => dispatched += 1,
                Err(e) => failed.push(
                    serde_json::json!({ "issue_id": issue.id, "number": issue.number, "error": e }),
                ),
            },
            None => {
                let mut row = serde_json::json!({
                    "issue_id": issue.id,
                    "number": issue.number,
                    "error": "无匹配节点（兜底未开或无在线节点）",
                });
                row["match_detail"] =
                    serde_json::Value::String(match_failure_detail(issue, &peers));
                failed.push(row);
            }
        }
    }
    // 恢复动作可见性：成功重派的单由派发链自带痕迹；此处对失败项诚实汇总。
    let mut out = serde_json::json!({
        "dry_run": false,
        "dispatched": dispatched,
        "failed": failed,
        "total_candidates": candidates.len(),
    });
    if let Some(replay) = replay_summary {
        out["conflict_replay"] = replay;
    }
    Ok(out)
}

/// `project.resume`（未编译 cluster）：恢复发车即发集群 RPC，无从谈起。
/// （`_cluster` 在该形态下是 `&()`——AppState.cluster 的 not(cluster) 镜像。）
#[cfg(not(feature = "cluster"))]
async fn project_resume(
    _store: &Arc<BoardStore>,
    _cluster: &(),
    _board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    _project_id: i64,
    _dry_run: bool,
    _actor: &Actor,
) -> Result<serde_json::Value, String> {
    Err("project.resume 需要集群支持（cluster feature 未编译）".to_string())
}

/// P5/F4 resume 补合并回放钩子（依赖倒置：nemesis-web 不反向依赖
/// nemesisbot——回放逻辑真相源在 nemesisbot::board_archive_ingest，由
/// gateway 装配时注入，仿 [`crate::handlers::board`] 的 parent review hook
/// 先例）。入参 project_id，出参回放摘要 JSON（unfrozen/manual_commit/
/// merged/superseded/parked/refrozen）。
pub(crate) type ResumeReplayHook =
    std::sync::Arc<dyn Fn(i64) -> Result<serde_json::Value, String> + Send + Sync>;

pub(crate) static RESUME_REPLAY_HOOK: std::sync::OnceLock<ResumeReplayHook> =
    std::sync::OnceLock::new();

/// gateway 安装回放钩子（幂等；重复安装以首次为准——OnceLock 语义）。
pub fn set_resume_replay_hook(hook: ResumeReplayHook) {
    let _ = RESUME_REPLAY_HOOK.set(hook);
}

/// S-O1 合并停车人工重试钩子（依赖倒置同 [`RESUME_REPLAY_HOOK`] 先例）：
/// 重试真相源在 nemesisbot::board_archive_ingest（`retry_merge_for_issue`，
/// 扫档案树 placement 凭据反查 task → 重新登记 PLACED → 重走合并触发）。
/// 入参 issue 引用，出参逐 task 重试明细 JSON。
pub(crate) type RetryMergeHook = std::sync::Arc<
    dyn Fn(&nemesis_board::models::Issue) -> Result<serde_json::Value, String> + Send + Sync,
>;

pub(crate) static RETRY_MERGE_HOOK: std::sync::OnceLock<RetryMergeHook> =
    std::sync::OnceLock::new();

/// gateway 安装重试钩子（幂等；OnceLock 语义）。
pub fn set_retry_merge_hook(hook: RetryMergeHook) {
    let _ = RETRY_MERGE_HOOK.set(hook);
}

/// plan 预览缓存（plan_id → 预览）。模块级 static 而非 AppState 字段：
/// AppState 在全库有大量字面构造测试点，加字段是断点级改动；缓存是本
/// handler 私有的进程内运行态，单例语义。插入时惰性清过期条目（无后台
/// 清扫任务）。
#[cfg(feature = "cluster")]
static PLAN_CACHE: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<String, PlanPreview>>,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

/// 预览有效期：确认发车须在 10 分钟内，过期重拆（防陈旧 plan 覆盖用户
/// 在预览期间对父单的手工编辑）。
#[cfg(feature = "cluster")]
const PLAN_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// 拆解运行期观测面（2026-10-11「停着」第二次事故）：PLAN_CACHE 只在拆解
/// 完成后写入，planner 运行期（GLM 实测 4~7 分钟）查询返回空、SSE 无事件、
/// 父单无评论——编排者无法区分「拆解中」与「链路死亡」，同一表象连续两次
/// 误判。RUNNING_PLANS 记录 in-flight 拆解，`plan.list/get` 与 ready 条目
/// 并列输出（status 区分）；`board.plan_started` SSE + 父单「已开始拆解」
/// 评论补齐实时与持久两个通道。
#[cfg(feature = "cluster")]
struct RunningPlan {
    pub(super) issue_id: i64,
    pub(super) started_at: std::time::Instant,
    /// 第几次尝试（1 起）。超时重拆链（§1.1）每次 +1；列表投影/横幅
    /// 「第 N 次尝试」文案数据源。
    pub(super) attempt: u32,
}

#[cfg(feature = "cluster")]
static RUNNING_PLANS: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<String, RunningPlan>>,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

/// 在途拆解快照（U-B trip 边沿逐一写中断档案用；也供探针 resume 前的
/// 在途双保险查询）：`[(plan_id, issue_id, attempt)]`。
#[cfg(feature = "cluster")]
pub(crate) fn running_plans_snapshot() -> Vec<(String, i64, u32)> {
    RUNNING_PLANS
        .lock()
        .iter()
        .map(|(plan_id, r)| (plan_id.clone(), r.issue_id, r.attempt))
        .collect()
}

/// RUNNING_PLANS 摘除 guard：Drop 语义保证正常返回 / 错误返回 / panic 展开
/// 三种退出路径都摘条目（watched 壳管留痕，guard 管注册表清洁，各管各的）。
#[cfg(feature = "cluster")]
struct RunningPlanGuard {
    pub(super) plan_id: String,
}

#[cfg(feature = "cluster")]
impl Drop for RunningPlanGuard {
    fn drop(&mut self) {
        RUNNING_PLANS.lock().remove(&self.plan_id);
    }
}

/// P11 观测面（2026-10-10）：plan 预览查询。PLAN_CACHE 此前只进不出且无
/// 查询面（SSE board.plan_ready/plan_failed 只在线可见，错过即盲），编排
/// 者只能盯日志——`board.plan.list/get` 给 Dashboard/编排者列存活预览与
/// 单查能力。TTL 惰性过滤与插入点同判据（过期条目顺带清除）。
#[cfg(feature = "cluster")]
fn plan_list(
    store: &Arc<BoardStore>,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let data = data.unwrap_or(serde_json::json!({}));
    let issue_id = data.get("issue_id").and_then(|v| v.as_i64());
    let now = std::time::Instant::now();
    let mut cache = PLAN_CACHE.lock();
    cache.retain(|_, p| now.duration_since(p.created_at) <= PLAN_TTL);
    let mut plans: Vec<serde_json::Value> = cache
        .iter()
        .filter(|(_, p)| issue_id.is_none_or(|id| p.issue_id == id))
        .map(|(plan_id, p)| plan_preview_json(plan_id, p, now))
        .collect();
    drop(cache);
    // running 条目（拆解中）与 ready 并列：PLAN_CACHE 完成后才写入，缺此
    // 查询「正在跑」返回空——编排者无法区分拆解中与没跑（「停着」事故）。
    // created_ms 置 0 → 排序自然垫底（ready 等确认的在前）。
    for (plan_id, r) in RUNNING_PLANS.lock().iter() {
        if issue_id.is_none_or(|id| r.issue_id == id) {
            plans.push(serde_json::json!({
                "plan_id": plan_id,
                "issue_id": r.issue_id,
                "status": "running",
                "attempt": r.attempt,
                "created_ms": 0u64,
                "elapsed_secs": now.duration_since(r.started_at).as_secs(),
            }));
        }
    }
    // 新拆解在前（编排者关注的是"刚出的计划等确认"）。
    plans.sort_by_key(|v| std::cmp::Reverse(v["created_ms"].as_u64().unwrap_or(0)));
    // issue_number 附属（issue 可能已被删，best-effort 不炸列表）。
    for v in &mut plans {
        if let Some(id) = v["issue_id"].as_i64()
            && let Ok(issue) = store.get_issue(id)
        {
            v["issue_number"] = serde_json::json!(issue.number);
        }
    }
    Ok(Some(serde_json::json!({ "plans": plans })))
}

/// P11 观测面：单查预览。不存在/已消费/已过期 = 诚实报错（消费路径
/// confirm 成功即 remove，缓存里没有就是没有了——语义与缓存一致）。
#[cfg(feature = "cluster")]
fn plan_get(data: Option<serde_json::Value>) -> Result<Option<serde_json::Value>, String> {
    let data = data.ok_or("missing data")?;
    let plan_id = get_str(&data, "plan_id")?;
    let now = std::time::Instant::now();
    // running 优先：拆解中查询得到（status=running + 已等待秒数），不再是
    // 「查不到=死了」的歧义形态。
    if let Some(r) = RUNNING_PLANS.lock().get(plan_id.as_str()) {
        return Ok(Some(serde_json::json!({
            "plan_id": plan_id,
            "issue_id": r.issue_id,
            "status": "running",
            "attempt": r.attempt,
            "elapsed_secs": now.duration_since(r.started_at).as_secs(),
        })));
    }
    let mut cache = PLAN_CACHE.lock();
    let alive = cache
        .get(plan_id.as_str())
        .filter(|p| now.duration_since(p.created_at) <= PLAN_TTL)
        .map(|p| plan_preview_json(&plan_id, p, now));
    if alive.is_none() {
        // 过期条目顺带摘除；不存在的 remove 是 no-op。
        cache.remove(plan_id.as_str());
    }
    match alive {
        Some(v) => Ok(Some(v)),
        None => Err(format!(
            "plan {plan_id} 不存在、已过期或已消费（预览有效期 {}s，过期请重拆）",
            PLAN_TTL.as_secs()
        )),
    }
}

/// PlanPreview → 查询视图投影（plan.list / plan.get 共用）。
#[cfg(feature = "cluster")]
fn plan_preview_json(plan_id: &str, p: &PlanPreview, now: std::time::Instant) -> serde_json::Value {
    let elapsed_ms = now.duration_since(p.created_at).as_millis() as u64;
    serde_json::json!({
        "plan_id": plan_id,
        "issue_id": p.issue_id,
        // status 区分 ready（已完成待确认）与 running（plan.list 并列输出
        // 的 in-flight 条目）——查询方不再需要靠字段缺省猜状态。
        "status": "ready",
        "sub_count": p.subs.len(),
        "subs": p.subs
            .iter()
            .map(|s| {
                serde_json::json!({
                    "title": s.title,
                    "required_role": s.required_role,
                    "required_profession": s.required_profession,
                    "depends_on": s.depends_on,
                })
            })
            .collect::<Vec<_>>(),
        "downgrades": p.downgrades,
        "created_ms": elapsed_ms,
        "ttl_remaining_secs": PLAN_TTL.as_secs().saturating_sub(elapsed_ms / 1000),
    })
}

/// `board.plan.list`（未编译 cluster）：预览缓存与 planner 链同属集群面。
#[cfg(not(feature = "cluster"))]
fn plan_list(
    _store: &Arc<BoardStore>,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("board.plan.list 需要集群支持（cluster feature 未编译）".to_string())
}

/// `board.plan.get`（未编译 cluster）：同上。
#[cfg(not(feature = "cluster"))]
fn plan_get(_data: Option<serde_json::Value>) -> Result<Option<serde_json::Value>, String> {
    Err("board.plan.get 需要集群支持（cluster feature 未编译）".to_string())
}

/// planner LLM 编排（一段）：裸提示词 detached 调用（max_turns=1 单轮出
/// JSON）→ `parse_plan` 失败带 [`nemesis_board::build_retry_prompt`] 回灌
/// 重试 ≤2 次（首跑 + 2 次自纠）。D9 三臂：前两轮严格解析（职能格式非法
/// → 回灌自纠）；**末轮换 [`nemesis_board::parse_plan_lenient`] 宽和臂**
/// ——职能格式非法的子单置空收进计划，降级明细随 Ok 返回（调用方落
/// 系统评论 + WARN），不废整个计划。纯编排：提示词/解析/校验真相源在
/// nemesis-board::planner。`team_experience`（M4.5）是 meta 级经验注入
/// （调用方检索渲染后传入，空 = 不注入）。
///
/// pub（全自动流转 P3/D1）：WSAPI `issue.plan` 与 agent 工具 `board_issue
/// plan` 共用同一实现（单一真相源）。
#[cfg(feature = "cluster")]
pub async fn run_planner(
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    parent: &nemesis_board::Issue,
    team_experience: Vec<String>,
    cluster_profile: Option<String>,
) -> Result<(Vec<nemesis_board::PlannedSubIssue>, Vec<(usize, String)>), String> {
    let mut prompt = nemesis_board::build_planner_user_prompt(
        &parent.title,
        &parent.description,
        parent.acceptance_criteria.as_deref(),
        &team_experience,
        cluster_profile.as_deref(),
    );
    let mut last_err = String::new();
    for round in 0..=2 {
        let raw = match agent_loop
            .run_detached(
                &prompt,
                nemesis_agent::r#loop::DetachedOpts {
                    system_prompt: Some(nemesis_board::planner_system_prompt()),
                    no_tools: true,
                    max_turns: 1,
                    label: Some("board-planner"),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(raw) => raw,
            // 发现 D（2026-09-11 措辞精度）：LLM 调用失败不消耗解析重试轮，
            // 与「解析失败 3 轮」分开归类，下游评论不再误报轮数。
            Err(e) => return Err(format!("planner LLM 调用失败：{e}")),
        };
        // D9 末轮宽和臂：宽和解析是严格解析的放宽超集（唯一差别 = 职能
        // 格式非法降级置空），末轮直接用宽和版——严格可过的计划在宽和版
        // 下零降级原样通过，行为对成功路径无差。
        if round == 2 {
            return match nemesis_board::parse_plan_lenient(&raw) {
                Ok(lp) => {
                    if !lp.downgraded.is_empty() {
                        tracing::warn!(
                            "[Board] planner 末轮宽和降级（职能格式非法→置空）issue={} downgrades={:?}",
                            parent.id,
                            lp.downgraded
                        );
                    }
                    Ok((lp.plan, lp.downgraded))
                }
                Err(e) => {
                    let msg = e.message;
                    Err(format!("planner 输出连续 3 轮无法解析：{msg}"))
                }
            };
        }
        match nemesis_board::parse_plan(&raw) {
            Ok(subs) => return Ok((subs, Vec::new())),
            Err(e) => {
                prompt = nemesis_board::build_retry_prompt(&raw, &e);
                // 重试不丢集群画像约束（R-10：拆解必须与可执行者对齐）。
                if let Some(p) = &cluster_profile {
                    prompt.push_str(&format!(
                        "\n# 可用执行节点（集群画像，拆解必须与此对齐）\n{p}\n"
                    ));
                }
                last_err = e.message;
            }
        }
    }
    Err(format!("planner 输出连续 3 轮无法解析：{last_err}"))
}

/// 全自动流转 P3/D1：plan 链共享编排（WSAPI `issue.plan` 一段、agent 工具
/// `board_issue plan`、autopilot `auto_plan`、项目 `auto_start` 四入口单一
/// 真相源）。编排 = 团队经验检索（M4.5，发起点定格）→ [`run_planner`] →
/// 预览入 [`PLAN_CACHE`] + `board.plan_ready` 事件 → A1 旗标现读（true 则
/// [`confirm_plan`] 发车 + 系统评论；失败保留预览转人工 + `board.plan_failed`）。
///
/// 同步调用（不 spawn）——调用方决定阻塞（agent 工具拿完整结果汇报）还是
/// 丢后台（WSAPI / autopilot / 项目启动）。`plan_id` 由调用方生成传入（用户
/// / agent 手里确认发车的凭据）。错误契约：**所有**失败路径的 SSE
/// `board.plan_failed` 发布在链内完成，调用方只记日志 / 透传错误。
///
/// plan 链后台任务的统一观察壳（panic 留痕闭环）：`execute_plan_chain`
/// 的三个 fire-and-forget 入口共用。任务 panic（JoinError）不再无声——
/// ERROR 日志 + issue 系统评论 + `board.plan_failed` 事件（载荷与链内
/// 失败发布同形）；任务被取消（关机/estop）= 正常流不留痕。2026-10-11
/// 事故固化：planner 截断 panic 杀死规划任务、看板零留痕，用户体感
/// 「建了不动」——无人值守链路的后台任务死亡必须看板可见（日志只有
/// 翻日志的人看得到）。
#[cfg(feature = "cluster")]
fn spawn_plan_chain_watched(
    store: Arc<BoardStore>,
    hub: Option<Arc<crate::events::EventHub>>,
    issue_id: i64,
    plan_id: String,
    fut: impl std::future::Future<Output = ()> + Send + 'static,
) {
    let handle = tokio::spawn(fut);
    tokio::spawn(async move {
        if let Err(je) = handle.await {
            if !je.is_panic() {
                return; // 任务被取消（关机/estop）= 正常流
            }
            let msg = nemesis_utils::string_utils::panic_message(je.into_panic());
            let short: String = msg.chars().take(300).collect();
            tracing::error!("[Board] plan chain PANIC issue={issue_id} plan={plan_id}: {short}");
            let _ = store.add_comment(NewComment {
                issue_id,
                author: Actor::system("board"),
                content: format!(
                    "❌ 拆解链后台任务异常终止（内部错误 panic）：{short}——该单未完成自动流转，请重试拆解或人工处置"
                ),
                parent_id: None,
                ctype: CommentType::System,
            });
            if let Some(hub) = hub {
                hub.publish(
                    "board.plan_failed",
                    serde_json::json!({ "plan_id": plan_id, "issue_id": issue_id, "error": format!("后台任务异常终止（panic）：{short}") }),
                );
            }
        }
    });
}

/// plan 链环境快照（fire-and-forget 三入口 + N1 重启恢复共用）：一次克隆
/// 进 `Arc`，超时重拆监视直接用同一份环境发起新链（递归走
/// [`spawn_plan_chain`]，panic 留痕与超时监视对新链自动续上）。
#[cfg(feature = "cluster")]
struct PlanChainEnv {
    store: Arc<BoardStore>,
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    issue: nemesis_board::Issue,
    actor: Actor,
    /// config.json 读取根（超时阈值/attempt 上限/重启恢复开关现读；None =
    /// 全部走缺省值）。与 auto_confirm 同链路：改配置对下一链即时生效。
    home: Option<std::path::PathBuf>,
    hub: Option<Arc<crate::events::EventHub>>,
}

/// `board.plan` 段现读（home 缺失/读取失败 = 缺省值；不 fail-closed——
/// 超时/重试上限走缺省是安全方向，与 auto_confirm 的 fail-closed 语义
/// 不同：发车是副作用，超时监视只是兜底）。
#[cfg(feature = "cluster")]
fn plan_chain_cfg(home: Option<&std::path::Path>) -> nemesis_config::BoardPlanConfig {
    home.and_then(|h| {
        nemesis_config::load_config(&std::path::Path::new(h).join("config.json")).ok()
    })
    .map(|c| c.board.unwrap_or_default().plan)
    .unwrap_or_default()
}

/// 拆解链 spawn 单点（超时重拆 §1.1 / §5#1 配置化）：= panic 留痕监视壳
///（[`spawn_plan_chain_watched`]）+ 超时重拆监视。attempt 从 1 起；重拆
/// attempt+1 新 plan_id；attempt 耗尽停车（评论 + `board.plan_failed` +
/// RUNNING_PLANS 摘除）——停车而非无限烧 LLM。
///
/// 设计注记（四入口覆盖面）：fire-and-forget 三入口（WSAPI `issue.plan` /
/// autopilot `auto_plan` / 项目 `auto_start`）+ N1 重启恢复走本函数；
/// agent 工具 `board_issue plan` 是同步 await 形态（agent 自盯结果，LLM
/// 调用受 provider 600s 超时 + parse 3 轮上限约束），不挂后台监视。
#[cfg(feature = "cluster")]
fn spawn_plan_chain(
    env: Arc<PlanChainEnv>,
    plan_id: String,
    attempt: u32,
    cfg: nemesis_config::BoardPlanConfig,
) {
    let chain_plan_id = plan_id.clone();
    let fut_env = env.clone();
    let fut = async move {
        if let Err(e) = execute_plan_chain(
            &fut_env.store,
            fut_env.cluster.clone(),
            fut_env.agent_loop.clone(),
            fut_env.issue.clone(),
            fut_env.actor.clone(),
            &chain_plan_id,
            fut_env.home.as_deref(),
            fut_env.hub.as_deref(),
            attempt,
        )
        .await
        {
            tracing::warn!(
                "[Board] plan chain failed issue={} plan={chain_plan_id}: {e}",
                fut_env.issue.id
            );
        }
    };
    spawn_plan_chain_watched(
        env.store.clone(),
        env.hub.clone(),
        env.issue.id,
        plan_id.clone(),
        fut,
    );
    spawn_plan_timeout_watch(env, plan_id, attempt, cfg);
}

/// 超时重拆监视（§1.1 D）：`timeout_secs`（0=不限，不监视）后醒来检查
/// RUNNING_PLANS——条目仍在 = 超时（正常完成 / panic / 错误路径的 guard
/// 都已摘条目）。处置：摘除本 plan_id（旧链 LLM 返回后 superseded 自检
/// 静默放弃——tokio 任务不可取消，双闸防双发车）→ 评论 → 重拆或停车。
/// `cfg` 由调用方注入（生产读 config，测试传秒级短值——§5#1 裁决）。
#[cfg(feature = "cluster")]
fn spawn_plan_timeout_watch(
    env: Arc<PlanChainEnv>,
    plan_id: String,
    attempt: u32,
    cfg: nemesis_config::BoardPlanConfig,
) {
    if cfg.timeout_secs == 0 {
        return; // 0 = 不限：不挂监视
    }
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(cfg.timeout_secs)).await;
        {
            let mut reg = RUNNING_PLANS.lock();
            match reg.get(&plan_id) {
                Some(rp) if rp.issue_id == env.issue.id => {
                    reg.remove(&plan_id);
                }
                _ => return, // 已完成/已失败/已被其它链取代 = 未超时
            }
        }
        tracing::warn!(
            "[Board] plan chain timeout issue={} plan={plan_id} attempt={attempt} ({}s)",
            env.issue.id,
            cfg.timeout_secs
        );
        let minutes = cfg.timeout_secs / 60;
        let can_retry =
            cfg.timeout_reattempt && (cfg.max_attempts == 0 || attempt < cfg.max_attempts);
        if !can_retry {
            // 停车：不重拆（reattempt 关 或 attempt 耗尽），等人工。
            let why = if !cfg.timeout_reattempt {
                "自动重拆未开启（board.plan.timeout_reattempt=false）".to_string()
            } else {
                format!(
                    "已连续超时 {} 次（board.plan.max_attempts={}），已停止自动重试",
                    attempt, cfg.max_attempts
                )
            };
            tracing::warn!("[Board] plan chain 停车 issue={}：{why}", env.issue.id);
            // U-A：拆解停车档案（手动重拆 = issue.plan 入口）。
            write_side_handoff(
                &env.store,
                &env.issue,
                "plan_exhausted",
                "retry",
                format!("拆解超时 {why}：核查模型端点后手动重拆（issue.plan），或人工直接创建子单"),
                "issue.plan（number=NB-x）手动重拆；或人工拆解",
                serde_json::json!({
                    "attempt": attempt,
                    "max_attempts": cfg.max_attempts,
                    "timeout_secs": cfg.timeout_secs,
                }),
                vec![serde_json::json!({
                    "action": "plan_chain",
                    "result": format!("第 {attempt} 次拆解超时（{} 分钟）", minutes),
                })],
            );
            let _ = env.store.add_comment(NewComment {
                issue_id: env.issue.id,
                author: Actor::system("board"),
                content: format!(
                    "❌ 拆解超时（{} 分钟），{why}，请人工处理（可手动重拆）",
                    minutes
                ),
                parent_id: None,
                ctype: CommentType::System,
            });
            if let Some(hub) = env.hub.as_deref() {
                hub.publish(
                    "board.plan_failed",
                    serde_json::json!({
                        "plan_id": plan_id,
                        "issue_id": env.issue.id,
                        "error": format!("拆解超时（{} 分钟），{why}", minutes),
                    }),
                );
            }
            return;
        }
        let next = attempt + 1;
        let _ = env.store.add_comment(NewComment {
            issue_id: env.issue.id,
            author: Actor::system("board"),
            content: format!(
                "⏰ 拆解超时（{} 分钟），自动重试（第 {} 次）",
                minutes, next
            ),
            parent_id: None,
            ctype: CommentType::System,
        });
        // 新链配置现读（热生效：改阈值/上限对重拆链即时生效）。
        let new_cfg = plan_chain_cfg(env.home.as_deref());
        let new_plan_id = format!("plan-{}", uuid::Uuid::new_v4());
        tracing::info!(
            "[Board] plan chain 重新拆解 issue={} new_plan={new_plan_id} attempt={next}",
            env.issue.id
        );
        spawn_plan_chain(env, new_plan_id, next, new_cfg);
    });
}

/// N1 gateway 重启拆解恢复（§1.3 / §7.2 N1，无人值守闭环）：RUNNING_PLANS
/// 是内存态、重启杀拆解任务——旧行为 = 父单停留「已开始自动拆解」评论态
/// 等手动重拆（无人值守语义下 = 流程中断）。启动恢复扫描：状态仍
/// backlog/todo + 最新系统评论为拆解中形态 + 无子单 + 不在 RUNNING_PLANS
/// 的单子 = 中断残留 → 自动重新发起拆解（走 [`spawn_plan_chain`] 同一
/// 超时重拆链，attempt 重计）。手动 cancel 的单子状态非 backlog/todo 天然
/// 排除；恢复拆解同受 `board.plan.max_attempts` 闸。
///
/// 调用点 = gateway 装配期（评审重放扫描同阶段，moderator 槽已装配）。
/// 守卫链：`board.plan.restart_recover`（默认开，关=关回旧行为）→ estop
/// 挂起跳过（急停意志优先，恢复不抢跑；estop release 不自动补拆——用户
/// 释放后可手动重拆或重启）→ 非 master 节点跳过（worker 本地 board.db 是
/// 只读镜像，拆解/派发链跑在 master）。返回恢复发起数。
#[cfg(feature = "cluster")]
pub fn recover_interrupted_plans(
    store: &Arc<BoardStore>,
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    home: Option<std::path::PathBuf>,
    hub: Option<Arc<crate::events::EventHub>>,
    estop: Option<&nemesis_agent::estop::EstopState>,
) -> usize {
    let cfg = plan_chain_cfg(home.as_deref());
    if !cfg.restart_recover {
        tracing::info!("[Board] 拆解恢复扫描跳过：board.plan.restart_recover=false");
        return 0;
    }
    if let Some(estop) = estop
        && estop.is_engaged()
    {
        tracing::warn!("[Board] 拆解恢复扫描跳过：estop 挂起（释放后请手动重拆或重启再试）");
        return 0;
    }
    if let Some(c) = &cluster
        && !matches!(c.role().as_str(), "coordinator" | "master" | "manager")
    {
        return 0; // worker 镜像不跑拆解链（与评审钩子同款角色闸）
    }
    let mut recovered = 0usize;
    for status in [IssueStatus::Backlog, IssueStatus::Todo] {
        let issues = match store.list_issues(&nemesis_board::models::IssueFilter {
            status: Some(status),
            ..Default::default()
        }) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("[Board] 拆解恢复扫描 {status:?} 单据失败：{e}");
                continue;
            }
        };
        for issue in issues {
            // 拆解中形态 = 最新系统评论是「已开始自动拆解」（完成/失败/超时
            // 评论都在其后顶掉它；用户手动评论是普通类型不影响系统序列）。
            let interrupted = store
                .list_comments(issue.id)
                .map(|cs| {
                    cs.iter()
                        .rev()
                        .find(|c| c.ctype == CommentType::System)
                        .is_some_and(|c| c.content.starts_with("🧩 已开始自动拆解"))
                })
                .unwrap_or(false);
            if !interrupted {
                continue;
            }
            // 双保险：已有子单（拆解实际发过车）/ 已有在途链（扫描期间用户
            // 手动触发）→ 让位。
            if store
                .list_children(issue.id)
                .map(|c| !c.is_empty())
                .unwrap_or(true)
            {
                continue;
            }
            if RUNNING_PLANS
                .lock()
                .values()
                .any(|r| r.issue_id == issue.id)
            {
                continue;
            }
            tracing::info!(
                "[Board] 拆解恢复：issue {}（{}）重启中断残留 → 自动重新发起拆解",
                issue.number,
                issue.title
            );
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("board"),
                content: "🔄 检测到上次拆解因重启中断，自动重新发起拆解".to_string(),
                parent_id: None,
                ctype: CommentType::System,
            });
            let env = Arc::new(PlanChainEnv {
                store: store.clone(),
                cluster: cluster.clone(),
                agent_loop: agent_loop.clone(),
                issue,
                actor: Actor::system("board"),
                home: home.clone(),
                hub: hub.clone(),
            });
            let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
            spawn_plan_chain(env, plan_id, 1, cfg.clone());
            recovered += 1;
        }
    }
    if recovered > 0 {
        tracing::info!("[Board] 拆解恢复扫描完成：恢复 {recovered} 个中断拆解");
    }
    recovered
}

/// U-B（§9.2）中断续拆：探针解封后对 `pending=plan` 中断档案重建拆解链
/// （与 N1 恢复同链——走 [`spawn_plan_chain`] 超时重拆链，attempt 重计 1）。
/// 在途双保险：该单已有在途链 = 让位（扫描/探针/手动三方竞态防双发车）。
/// 返回是否实际发车。
#[cfg(feature = "cluster")]
pub fn respawn_plan_chain(
    store: &Arc<BoardStore>,
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    issue: nemesis_board::Issue,
    home: Option<std::path::PathBuf>,
    hub: Option<Arc<crate::events::EventHub>>,
) -> bool {
    if RUNNING_PLANS
        .lock()
        .values()
        .any(|r| r.issue_id == issue.id)
    {
        tracing::info!("[Board] 中断续拆让位：issue {} 已有在途拆解链", issue.id);
        return false;
    }
    let _ = store.add_comment(NewComment {
        issue_id: issue.id,
        author: Actor::system("board"),
        content: "🔄 LLM 端点已恢复，中断的拆解自动续做".to_string(),
        parent_id: None,
        ctype: CommentType::System,
    });
    let env = Arc::new(PlanChainEnv {
        store: store.clone(),
        cluster,
        agent_loop,
        issue,
        actor: Actor::system("board"),
        home,
        hub,
    });
    let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
    let cfg = plan_chain_cfg(env.home.as_deref());
    spawn_plan_chain(env, plan_id, 1, cfg);
    true
}

/// `home` 供 A1 旗标现读（config.json 热改即时生效；None = 视为关闭，
/// fail-closed 回人工确认）。`hub` 供 SSE 事件（cron 路径 None——诚实降级：
/// 无实时推送，评论/落库/发车链路完整）。`attempt` = 第几次尝试（1 起；
/// 超时重拆链每次 +1，随 [`RunningPlan`] 注册与 `board.plan_started`
/// 载荷透出——前端横幅「第 N 次尝试」数据源）。
#[cfg(feature = "cluster")]
pub async fn execute_plan_chain(
    store: &Arc<BoardStore>,
    cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    issue: nemesis_board::Issue,
    actor: Actor,
    plan_id: &str,
    home: Option<&std::path::Path>,
    hub: Option<&crate::events::EventHub>,
    attempt: u32,
) -> Result<serde_json::Value, String> {
    let started = std::time::Instant::now();
    // U-B（§9.2）：LLM 端点不可用封存闸——拆解发起短路。自动/手动同闸：
    // 手动得到诚实挂起说明 + 中断档案（意图不丢，恢复后探针自动续做），
    // 不是静默吞掉。
    if super::board_unavailable::frozen_now() {
        if let Some(ws) = handoff_workspace_root().map(|p| p.as_path()) {
            super::board_unavailable::write_interruption(
                ws,
                store,
                &issue,
                "plan",
                format!("拆解发起时 LLM 端点不可用封存中（attempt {attempt}）：恢复后探针自动续拆"),
                serde_json::json!({ "plan_id": plan_id, "attempt": attempt }),
            );
        }
        tracing::warn!(
            "[Board] plan chain 挂起 issue={}：LLM 端点不可用封存中（恢复后自动续做）",
            issue.id
        );
        return Err(
            "⏸ LLM 端点不可用（封存中）：拆解已挂起，端点恢复后自动续做；也可稍后手动重拆（issue.plan）"
                .to_string(),
        );
    }
    let ub_cfg = super::board_unavailable::unavailable_cfg(home);
    // 拆解运行期可观测（2026-10-11「停着」事故）：running 条目（guard 摘除）
    // + `board.plan_started` SSE + 父单「已开始拆解」评论。planner 单次
    // 4~7 分钟且此前全程零反馈，用户体感=「停着」。
    RUNNING_PLANS.lock().insert(
        plan_id.to_string(),
        RunningPlan {
            issue_id: issue.id,
            started_at: started,
            attempt,
        },
    );
    let _running_guard = RunningPlanGuard {
        plan_id: plan_id.to_string(),
    };
    if let Some(hub) = hub {
        hub.publish(
            "board.plan_started",
            serde_json::json!({
                "plan_id": plan_id,
                "issue_id": issue.id,
                "issue_number": issue.number,
                "attempt": attempt,
            }),
        );
    }
    let _ = store.add_comment(NewComment {
        issue_id: issue.id,
        author: Actor::system("board"),
        content: "🧩 已开始自动拆解（planner 运行中，通常 3~8 分钟；完成后按 auto_confirm 自动发车或转人工确认）"
            .to_string(),
        parent_id: None,
        ctype: CommentType::System,
    });
    // M4.5：planner 的 meta 级经验注入——同步检索渲染（发起点定格，避免长
    // LLM 调用期间的经验变更串进本次拆解）。meta 注入不计 use_count（计数
    // 只认面向 worker 的派发注入）。
    let team_experience: Vec<String> = {
        let entries = store.list_team_memory(None, false).unwrap_or_default();
        let issue_text = format!(
            "{}\n{}\n{}",
            issue.title,
            issue.description,
            issue.acceptance_criteria.as_deref().unwrap_or("")
        );
        let hits = nemesis_board::match_experiences(
            &entries,
            &issue_text,
            &issue.required_tags,
            nemesis_board::MAX_MATCHED_EXPERIENCES,
        );
        nemesis_board::render_planner_experience_strings(&hits)
    };
    // R-10（goal P4）：集群画像注入——planner 拆解即按可执行者对齐
    // （名字/角色/tags/能力；投影与派发匹配器同源）。无在线节点 → None。
    let cluster_profile: Option<String> = cluster.as_ref().and_then(|c| {
        let lines: Vec<String> = project_dispatch_candidates(c)
            .iter()
            .map(|p| {
                let tags = if p.tags.is_empty() {
                    "—".to_string()
                } else {
                    p.tags.join("/")
                };
                let caps = if p.capabilities.is_empty() {
                    String::new()
                } else {
                    format!(" caps={}", p.capabilities.join("/"))
                };
                format!("- {} [role={} tags={tags}]{caps}", p.name, p.role)
            })
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    });
    let (subs, downgrades) = match run_planner(
        &agent_loop,
        &issue,
        team_experience,
        cluster_profile,
    )
    .await
    {
        Ok((subs, downgrades)) => {
            // U-B：planner 成功 = 端点活证据（连击清零/封存解除——生产里
            // 封存期到不了这里：入口闸已拦）。
            super::board_unavailable::record_llm_success(&ub_cfg);
            (subs, downgrades)
        }
        Err(e) => {
            // U-B：planner 失败喂封存判定；恰好触发封存 = 全部在途工作写
            // 中断档案（本单 + 其它在途拆解链）。
            let tripped = super::board_unavailable::record_llm_failure(&ub_cfg, &e);
            if tripped {
                super::board_unavailable::write_trip_interruptions(
                    handoff_workspace_root().map(|p| p.as_path()),
                    store,
                    Some((
                        &issue,
                        "plan",
                        format!(
                            "拆解在途（attempt {attempt}）时 LLM 端点不可用封存：恢复后自动续拆"
                        ),
                        serde_json::json!({ "plan_id": plan_id, "attempt": attempt }),
                    )),
                    &e,
                );
            }
            // superseded 优先于失败面（§1.1 双闸）：被超时重拆取代的旧链
            // 静默放弃——不发失败评论、不发 plan_failed（旧链迟到的失败
            // 事件会撤掉新链横幅），不消费解析结果。
            if !RUNNING_PLANS.lock().contains_key(plan_id) {
                tracing::info!(
                    "[Board] plan chain superseded（已被超时重拆取代）issue={} plan={plan_id}，旧链静默放弃",
                    issue.id
                );
                return Ok(serde_json::json!({
                    "status": "superseded",
                    "plan_id": plan_id,
                    "issue_id": issue.id,
                }));
            }
            tracing::warn!("[Board] planner failed issue={}: {e}", issue.id);
            // P11 观测面：失败评论留痕（SSE 只在线可见，评论持久可回查；
            // 截断 200 字符防异常长报错刷屏单据）。U-A：链内重试已耗尽的
            // 最终失败 = 停车，落现场档案。
            let err_short: String = e.chars().take(200).collect();
            write_side_handoff(
                store,
                &issue,
                "plan_exhausted",
                "retry",
                format!(
                    "planner 拆解失败（链内重试已耗尽）：核查模型端点后手动重拆（issue.plan），或人工直接创建子单。原因：{err_short}"
                ),
                "issue.plan（number=NB-x）手动重拆；或人工拆解",
                serde_json::json!({ "plan_id": plan_id }),
                vec![serde_json::json!({
                    "action": "run_planner",
                    "result": err_short,
                })],
            );
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("board"),
                content: format!("❌ planner 拆解失败：{err_short}"),
                parent_id: None,
                ctype: CommentType::System,
            });
            if let Some(hub) = hub {
                hub.publish(
                        "board.plan_failed",
                        serde_json::json!({ "plan_id": plan_id, "issue_id": issue.id, "error": e.clone() }),
                    );
            }
            return Err(e);
        }
    };
    // superseded 自检（§1.1 双闸之一，成功路径）：超时重拆监视在超时点摘除
    // 本 plan_id 并发起新链——本链的 LLM future 不可取消仍在飞，返回后第一
    // 检查点验证自己仍在 RUNNING_PLANS；不在 = 已被取代 → 静默放弃：不发
    // plan_ready、不落缓存、不评论、不 confirm（防「双计划双发车」）。
    if !RUNNING_PLANS.lock().contains_key(plan_id) {
        tracing::info!(
            "[Board] plan chain superseded（已被超时重拆取代）issue={} plan={plan_id}，旧链静默放弃",
            issue.id
        );
        return Ok(serde_json::json!({
            "status": "superseded",
            "plan_id": plan_id,
            "issue_id": issue.id,
        }));
    }
    let count = subs.len();
    {
        let mut cache = PLAN_CACHE.lock();
        cache.retain(|_, p| p.created_at.elapsed() <= PLAN_TTL);
        cache.insert(
            plan_id.to_string(),
            PlanPreview {
                issue_id: issue.id,
                subs: subs.clone(),
                downgrades: downgrades.clone(),
                created_at: std::time::Instant::now(),
            },
        );
    }
    tracing::info!(
        "[Board] planner done issue={} subs={count} ({}ms)",
        issue.id,
        started.elapsed().as_millis()
    );
    if let Some(hub) = hub {
        hub.publish(
            "board.plan_ready",
            serde_json::json!({
                "plan_id": plan_id,
                "issue_id": issue.id,
                "subs": subs.clone(),
            }),
        );
    }
    // ---- A1：board.plan.auto_confirm=true → 跳过人工确认闸直接发车。
    // 旗标每次拆解现读（config.json 热改即时生效）；读取失败 fail-closed
    // 回人工确认。plan_id 缓存条目留在原处（未消费）——人工此时再点确认
    // 会撞「plan_id 已存在但 issue 已有子单」的重复建单风险，故发车成功
    // 后移除。
    let auto_confirm = home
        .and_then(|h| {
            nemesis_config::load_config(&std::path::Path::new(h).join("config.json")).ok()
        })
        .map(|c| c.board.unwrap_or_default().plan.auto_confirm)
        .unwrap_or(false);
    // P11 观测面：拆解成功评论留痕（含 plan_id；措辞按发车模式分支——
    // 人工模式附 TTL 提示，自动发车臂随后另有发车评论，不重复诱导确认）。
    let _ = store.add_comment(NewComment {
        issue_id: issue.id,
        author: Actor::system("board"),
        content: if auto_confirm {
            format!("🧩 planner 拆解完成：{plan_id}，{count} 个子单（自动发车模式）")
        } else {
            format!(
                "🧩 planner 拆解完成：{plan_id}，{count} 个子单，请在 {}s 内确认派发",
                PLAN_TTL.as_secs()
            )
        },
        parent_id: None,
        ctype: CommentType::System,
    });
    if !auto_confirm {
        return Ok(serde_json::json!({
            "status": "planned",
            "plan_id": plan_id,
            "issue_id": issue.id,
            "subs": count,
            "note": "已生成拆解预览，等待人工确认发车（board.plan.auto_confirm 未开启）",
        }));
    }
    match confirm_plan(store, cluster.as_ref(), &issue, subs, &actor, &downgrades) {
        Ok(mut out) => {
            PLAN_CACHE.lock().remove(plan_id);
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("board"),
                content: format!(
                    "🤖 board.plan.auto_confirm 已开启，拆解自动发车：{} 子单已建，{} 已派",
                    out.get("created")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0),
                    out.get("dispatched").and_then(|v| v.as_u64()).unwrap_or(0),
                ),
                parent_id: None,
                ctype: CommentType::System,
            });
            // E2 决策审计：自动发车入决策流（失败只 warn，不炸发车结果）。
            if let Err(ae) = store.add_activity(
                issue.id,
                &Actor::system("board"),
                "auto_decide",
                Some(
                    &serde_json::json!({
                        "decision": "auto_confirm_dispatch",
                        "verdict": "plan",
                        "subs": count,
                    })
                    .to_string(),
                ),
            ) {
                tracing::warn!("[Board] auto_decide 活动落库失败 issue={}：{ae}", issue.id);
            }
            tracing::info!("[Board] auto_confirm 发车 issue={} subs={count}", issue.id);
            out["status"] = serde_json::json!("dispatched");
            out["plan_id"] = serde_json::json!(plan_id);
            out["subs"] = serde_json::json!(count);
            Ok(out)
        }
        Err(e) => {
            tracing::warn!(
                "[Board] auto_confirm 发车失败 issue={}：{e}（保留预览，转人工确认）",
                issue.id
            );
            // P11 观测面：发车失败评论留痕（预览保留转人工，评论给入口指引）。
            let err_short: String = e.chars().take(200).collect();
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("board"),
                content: format!(
                    "⚠️ 拆解完成但自动发车失败：{err_short}（预览已保留，可人工确认派发）"
                ),
                parent_id: None,
                ctype: CommentType::System,
            });
            if let Some(hub) = hub {
                hub.publish(
                    "board.plan_failed",
                    serde_json::json!({
                        "plan_id": plan_id,
                        "issue_id": issue.id,
                        "error": format!("auto_confirm 发车失败：{e}"),
                    }),
                );
            }
            Err(format!(
                "拆解完成但自动发车失败：{e}（预览已保留，可人工确认）"
            ))
        }
    }
}

/// `issue.plan` 实现（cluster 编译时）：一段异步拆解 / 二段确认落库+派发波。
#[cfg(feature = "cluster")]
async fn issue_plan(
    store: &Arc<BoardStore>,
    actor: Actor,
    ctx: &RequestContext,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    // F-U4-5：急停中拒绝（二段确认=派发波发车；一段拆解的 LLM 走主持人
    // loop 同被急停冻结——两段统一在此诚实拒绝，反馈比自然失败更快更明确）。
    refuse_dispatch_when_estopped(ctx)?;
    let data = data.ok_or("missing data")?;
    let id = data
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("missing field: id")?;
    let issue = store.get_issue(id)?;

    if data
        .get("confirm")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        // ---- 二段：确认发车（缓存一次性消费）----
        let plan_id = get_str(&data, "plan_id")?.to_string();
        let preview = {
            let mut cache = PLAN_CACHE.lock();
            match cache.remove(plan_id.as_str()) {
                Some(p) if p.issue_id == id && p.created_at.elapsed() <= PLAN_TTL => p,
                Some(p) => {
                    // 与该 issue 不匹配 → 放回（可能是用户在别的 issue
                    // 表单里贴错了 plan_id，不应误伤那个在途预览）。
                    cache.insert(plan_id.clone(), p);
                    return Err("plan 与该 issue 不匹配，请重新拆解".to_string());
                }
                None => {
                    return Err("plan_id 不存在或已被消费（每个 plan 只能确认一次）".to_string());
                }
            }
        };
        let cluster = ctx.state.cluster.clone();
        let out = confirm_plan(
            store,
            cluster.as_ref(),
            &issue,
            preview.subs,
            &actor,
            &preview.downgrades,
        )?;
        Ok(Some(out))
    } else {
        // ---- 一段：异步拆解（重复点按钮 = 重复 LLM 消耗，新 plan_id
        //      覆盖旧预览；结果幂等——confirm 以 plan_id 定位）----
        let agent_loop = ctx
            .state
            .agent_loop
            .read()
            .clone()
            .ok_or("agent 未运行，无法执行 AI 拆解")?;
        // M4.5 经验检索 / planner 调用 / 预览缓存 / plan_ready 事件 /
        // A1 auto_confirm 发车——编排全部收敛到 execute_plan_chain（P3/D1：
        // 与 agent 工具 board_issue plan、autopilot auto_plan、项目
        // auto_start 四入口同一实现）。这里只负责丢后台 + 记日志（SSE 错误
        // 事件的发布在链内完成，不重复发）。
        let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
        let hub = ctx.state.event_hub.clone();
        let env = std::sync::Arc::new(PlanChainEnv {
            store: store.clone(),
            cluster: ctx.state.cluster.clone(),
            agent_loop,
            issue: issue.clone(),
            actor: actor.clone(),
            home: ctx.home.clone().map(std::path::PathBuf::from),
            hub: Some(hub),
        });
        let cfg = plan_chain_cfg(env.home.as_deref());
        spawn_plan_chain(env, plan_id.clone(), 1, cfg);
        Ok(Some(
            serde_json::json!({ "status": "planning", "plan_id": plan_id }),
        ))
    }
}

/// `issue.plan`（未编译 cluster）：planner 产物的派发语义（required_* 匹配、
/// 依赖补派波）全依赖集群。
#[cfg(not(feature = "cluster"))]
async fn issue_plan(
    _store: &Arc<BoardStore>,
    _actor: Actor,
    _ctx: &RequestContext,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("issue.plan 需要集群支持（cluster feature 未编译）".to_string())
}

/// 二段确认：预览 → 落库（parent/required_*/origin=planner）→ 依赖边
/// （批内序号 → 真实 id）→ 依赖闸派发波（无依赖的立即派，其余留 backlog
/// 等补派触发器）→ 父单联动。单张派发失败不回滚整批（落库已成事实），
/// 系统评论留痕继续。
///
/// pub（全自动流转 P3/D1）：WSAPI `issue.plan` confirm 与 agent 工具 /
/// autopilot auto_plan / 项目 auto_start 共用（单一真相源）。
#[cfg(feature = "cluster")]
pub fn confirm_plan(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    parent: &nemesis_board::Issue,
    subs: Vec<nemesis_board::PlannedSubIssue>,
    actor: &Actor,
    downgrades: &[(usize, String)],
) -> Result<serde_json::Value, String> {
    // 1) 落库（保持批内顺序；id_by_idx 供依赖映射）。required_profession
    //    随拆解落库（M2）；D9 末轮宽和降级明细逐条落被降级子单的系统评论
    //    「required_profession=`…` 非法，已降级无职能」（审计可回查）。
    //    降级子单**强制置空职能**再落库（防御性单一真相源：清空语义在
    //    confirm_plan 定案，不依赖上游 parse_plan_lenient 是否已剥）。
    let mut id_by_idx: Vec<i64> = Vec::with_capacity(subs.len());
    for (idx, sub) in subs.iter().enumerate() {
        let ac = sub.acceptance_criteria.trim();
        let role = sub.required_role.trim();
        let downgraded = downgrades.iter().any(|(di, _)| *di == idx);
        let prof = if downgraded {
            ""
        } else {
            sub.required_profession.trim()
        };
        let created = store.create_issue(nemesis_board::NewIssue {
            title: sub.title.clone(),
            description: sub.description.clone(),
            acceptance_criteria: (!ac.is_empty()).then(|| ac.to_string()),
            parent_issue_id: Some(parent.id),
            required_role: (!role.is_empty()).then(|| role.to_string()),
            required_tags: sub.required_tags.clone(),
            required_profession: (!prof.is_empty()).then(|| prof.to_string()),
            // 子单归属父单所在项目（F2 首派联动读子单的 project_id 推进
            // 项目 active→in_progress；缺继承则联动早退、项目永远 active）。
            project_id: parent.project_id,
            origin: Some(nemesis_board::TaskOrigin {
                origin_type: "planner".to_string(),
                origin_id: parent.number.clone(),
            }),
            creator: actor.clone(),
            ..nemesis_board::NewIssue::default()
        })?;
        if downgraded && let Some((_, original)) = downgrades.iter().find(|(di, _)| *di == idx) {
            let _ = store.add_comment(nemesis_board::NewComment {
                issue_id: created.id,
                author: nemesis_board::Actor::system("board"),
                content: format!(
                    "⚠ required_profession=`{original}` 非法，已降级无职能（planner 末轮宽和）"
                ),
                parent_id: None,
                ctype: nemesis_board::CommentType::System,
            });
        }
        id_by_idx.push(created.id);
    }

    // 2) 依赖边（批内序号 → 真实 id；越界 parse_plan 已挡，防御再钳一次）。
    for (idx, sub) in subs.iter().enumerate() {
        let deps: Vec<i64> = sub
            .depends_on
            .iter()
            .filter(|&&d| d < id_by_idx.len() && d != idx)
            .map(|&d| id_by_idx[d])
            .collect();
        store.set_issue_dependencies(id_by_idx[idx], &deps)?;
    }

    // C 里程碑 1（看板项目档案 goal P2）：拆解落库 → docs/plan.md +
    // timeline。档案写入失败不阻塞发车（writer 内部 WARN+审计，goal C 纪律）；
    // 存量项目（无 directory）静默跳过。
    nemesis_board::archive_writer::write_plan_milestone(store, parent, &subs);

    // 3) 派发波：依赖闸放行的立即派，其余留 backlog 等补派触发器。
    let mut dispatched = 0usize;
    let mut deferred: Vec<i64> = Vec::new();
    for &cid in &id_by_idx {
        match dispatch_subissue_auto(store, cluster, cid, actor, true) {
            Ok(Some(_)) => dispatched += 1,
            Ok(None) => deferred.push(cid),
            Err(e) => {
                tracing::warn!("[Board] 子单 {cid} 自动派发失败：{e}");
                let _ = store.add_comment(nemesis_board::NewComment {
                    issue_id: cid,
                    author: nemesis_board::Actor::system("board"),
                    content: format!("⛔ 自动派发失败：{e}"),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
            }
        }
    }

    // 4) 父单联动（首派 → in_progress 已在 dispatch_subissue_auto 内处理）。
    let _ = sync_parent_status(store, parent.id, actor);

    // 5) F5 件③：父单锚点下放防御性后置检查——提示词纪律（件②）LLM
    //    不保证遵守：父单 AC 含 `[CHECK] re:` 锚点但没有任何子单继承 →
    //    系统评论提醒人工补（不阻塞发车；评论就是持久记录，详情弹窗可见）。
    if nemesis_board::has_content_regex_anchor(parent.acceptance_criteria.as_deref().unwrap_or(""))
        && !subs
            .iter()
            .any(|s| nemesis_board::has_content_regex_anchor(&s.acceptance_criteria))
    {
        let _ = store.add_comment(nemesis_board::NewComment {
            issue_id: parent.id,
            author: nemesis_board::Actor::system("board"),
            content: "⚠ 锚点下放检查：父单验收标准含 [CHECK] re: 锚点，但拆解出的子单均未继承任何 re: 锚点——父单收口时这些锚点没有交付文本可核验，验收将失败。请给至少一个子单补上可独立核验的锚点行，或修正父单验收标准。".to_string(),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        });
        tracing::warn!(
            "[Board] 父单 {} AC 含 re: 锚点但子单均未继承（已落评论提醒人工）",
            parent.id
        );
    }

    Ok(serde_json::json!({
        "created": id_by_idx,
        "dispatched": dispatched,
        "deferred": deferred,
        "issue": issue_to_view(store, &store.get_issue(parent.id)?)?,
    }))
}

/// F2 项目状态联动：项目绑定的单实际派出 → 项目 `active → in_progress`
/// （首派语义）。项目不在 active（已 in_progress/completed/archived）或
/// 绑定缺失 → 不动。联动是派发的伴生效果——失败只 WARN 不回滚派发。
#[cfg(feature = "cluster")]
fn link_project_on_dispatch(store: &Arc<BoardStore>, project_id: Option<i64>) {
    let Some(pid) = project_id else {
        return;
    };
    let project = match store.get_project(pid) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[Board] 项目状态联动：project {pid} 读取失败（不影响派发）：{e}");
            return;
        }
    };
    if nemesis_board::models::ProjectStatus::from_str(&project.status)
        != Some(nemesis_board::models::ProjectStatus::Active)
    {
        return;
    }
    if let Err(e) = store.update_project(
        pid,
        &ProjectPatch {
            status: Some(
                nemesis_board::models::ProjectStatus::InProgress
                    .as_str()
                    .to_string(),
            ),
            ..ProjectPatch::default()
        },
    ) {
        tracing::warn!("[Board] 项目状态联动失败 project={pid}（不影响派发）：{e}");
    }
}

/// ⏸ 暂缓评论的去重标记：单上最后一条 system 评论含此串 = 停车状态已
/// 留痕，不再重复落同文案评论（B4 防堆叠）。
#[cfg_attr(not(feature = "cluster"), allow(dead_code))]
const PARK_NOTICE_MARK: &str = "自动派发暂缓";

/// 三路 sweep 触发源（announce 回调 / 派发落定重估波 / 周期兜底 ticker，
/// F-U3-4）的进程级串行锁：sweep 体是 check-then-dispatch 序列，两个
/// wave 并发时可同时通过 `has_active_dispatch` 闸对同一单双派。同步锁
/// 即可——sweep 体全同步（SQLite + fire-and-forget RPC spawn），持锁
/// 毫秒级；poison 恢复沿用 `unwrap_or_else(into_inner)` 惯例。
#[cfg(feature = "cluster")]
static PARK_SWEEP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// D0 准入停车评论标记（与 PARK_NOTICE_MARK 分开：原因不同、去重不互斥）。
#[cfg_attr(not(feature = "cluster"), allow(dead_code))]
const INFLIGHT_NOTICE_MARK: &str = "在途派发已达上限";

/// D0 准入判定（纯函数，单测直测）：目标 worker 在途派发数 ≥ 上限（上限
/// >0 时）= 满，自动派发不派、留 backlog 等承接。
#[cfg_attr(not(feature = "cluster"), allow(dead_code))]
fn inflight_full(load: &std::collections::HashMap<String, usize>, target: &str, cap: i64) -> bool {
    cap > 0 && load.get(target).copied().unwrap_or(0) as i64 >= cap
}

/// 子单自动派发（依赖闸 + 匹配器选节点；confirm 派发波、补派触发器与
/// 停车场 sweep 共用——单一真相源）。返回：`Ok(Some(out))` 已派出；
/// `Ok(None)` 暂缓（非待派状态 / 已有在途派发 / 依赖未满足 / 无匹配节点）；
/// `Err` 硬错误（集群缺失、派发核心失败）。
///
/// `notify_park`：无匹配节点停车时是否做通知性动作（⏸ 系统评论去重落 +
/// 父单受阻联动）。发车波/补派触发器 = true；停车场 sweep 周期重试 =
/// false（静默，不因反复重试刷评论）。
#[cfg(feature = "cluster")]
pub fn dispatch_subissue_auto(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue_id: i64,
    actor: &Actor,
    notify_park: bool,
) -> Result<Option<serde_json::Value>, String> {
    dispatch_subissue_auto_with_config(
        live_board_config().as_ref(),
        store,
        cluster,
        issue_id,
        actor,
        notify_park,
    )
}

/// [`dispatch_subissue_auto`] 的可测内核：board 配置显式入参（测试不碰
/// 进程级全局单例），兜底开关判定也走同一入口。
#[cfg(feature = "cluster")]
pub fn dispatch_subissue_auto_with_config(
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue_id: i64,
    actor: &Actor,
    notify_park: bool,
) -> Result<Option<serde_json::Value>, String> {
    let issue = store.get_issue(issue_id)?;

    // 状态闸：只自动派待派态（planner 产物落 backlog；用户预排的 todo
    // 也算）。在途/已收口/阻塞的单不动。
    if !matches!(issue.status, IssueStatus::Backlog | IssueStatus::Todo) {
        return Ok(None);
    }
    // 已有在途派发（人工先派过）→ 不重复。
    if store.has_active_dispatch(issue.id)? {
        return Ok(None);
    }
    // 依赖闸：全部依赖 done 才派；否则留待补派触发器。
    for dep_id in store.dependencies_of(issue.id)? {
        if store.get_issue(dep_id)?.status != IssueStatus::Done {
            return Ok(None);
        }
    }
    // 父单存活闸（F-U4-3）：父单已取消 = 任务线已死，本单不再派出——
    // 否则补派触发器/停车场 sweep 会在父单死后继续派子单空烧 token。
    //（reopen 侧同款校验：父单 cancelled 的子单不可单独复活。）
    if let Some(pid) = issue.parent_issue_id
        && store
            .get_issue(pid)
            .map(|p| p.status == IssueStatus::Cancelled)
            .unwrap_or(false)
    {
        return Ok(None);
    }

    // R-9（goal P4）touch_paths 互斥：本单声明的写路径与「同父在途单」的
    // 写路径相交 → 本波不派（静默延后，下一触发波重估）——防多 worker 并发
    // 写同一路径互相覆盖。无声明（空 [TOUCH]）不拦。
    {
        let my_touch =
            nemesis_board::parse_touch_paths(issue.acceptance_criteria.as_deref().unwrap_or(""));
        if !my_touch.is_empty()
            && let Some(pid) = issue.parent_issue_id
        {
            let conflict = store
                .list_children(pid)
                .unwrap_or_default()
                .iter()
                .any(|s| {
                    s.id != issue.id
                        && store.has_active_dispatch(s.id).unwrap_or(false)
                        && nemesis_board::parse_touch_paths(
                            s.acceptance_criteria.as_deref().unwrap_or(""),
                        )
                        .iter()
                        .any(|p| my_touch.iter().any(|m| m == p))
                });
            if conflict {
                return Ok(None); // 静默延后（与依赖闸同风格）
            }
        }
    }

    // 目标解析：显式 worker 指派 > 匹配器 > 兜底开关。
    let target = match (&issue.assignee, &issue.assignee_id) {
        (Some(AssignmentType::Worker), Some(wid)) => wid.clone(),
        _ => {
            let cluster = cluster.ok_or("集群未运行，无法自动派发")?;
            match pick_target_by_matcher(store, cluster, &issue) {
                Some(t) => t,
                None => {
                    // B1（goal P1）：匹配失败明细——要求 vs 每个在线候选差什么
                    // （与 rank_peers 同语义：role 精确、tags 交集非空）。
                    let candidates = project_dispatch_candidates(cluster);
                    let detail = match_failure_detail(&issue, &candidates); // 兜底开关（集群完备性 2026-09-11）：无人匹配但任务必须
                    // 做下去——按配置兜底派给在线节点（派发前 ⚠ 评论留痕；
                    // 与 ⏸ 停车评论是不同语义，不受 B4 去重约束）。
                    if let Some((fb_target, reason)) =
                        pick_fallback_target(board_cfg, store, cluster, &issue)
                    {
                        // E（goal P2/P5）：标签授予——把 relaxed 掉的缺失标签
                        // 授予目标节点（board_meta 台账；同类后续任务正常匹配，
                        // 降级路径越走越少）。
                        let granted_new = store
                            .grant_tags_to_node(&fb_target, &issue.required_tags)
                            .unwrap_or_default();
                        let grant_note = if granted_new.is_empty() {
                            String::new()
                        } else {
                            format!("；已授予标签 [{}]", granted_new.join(","))
                        };
                        let _ = store.add_comment(nemesis_board::NewComment {
                            issue_id: issue.id,
                            author: nemesis_board::Actor::system("board"),
                            content: format!(
                                "⚠ 无人匹配（角色/标签要求）：{reason}，按兜底策略派给 {fb_target}（board.dispatch.fallback）{grant_note}"
                            ),
                            parent_id: None,
                            ctype: nemesis_board::CommentType::System,
                        });
                        fb_target
                    } else {
                        // 诚实留痕：不悄悄留 backlog。sweep 静默路径（!notify_park）
                        // 不落评论不动父单。B1：评论附匹配明细（差什么一眼可见）。
                        if notify_park {
                            // B4 去重：最后一条 system 评论已是暂缓说明 → 不重复落。
                            let already_noted = store
                                .last_system_comment(issue.id)
                                .ok()
                                .flatten()
                                .map(|c| c.contains(PARK_NOTICE_MARK))
                                .unwrap_or(false);
                            if !already_noted {
                                store.add_comment(nemesis_board::NewComment {
                                    issue_id: issue.id,
                                    author: nemesis_board::Actor::system("board"),
                                    content: format!(
                                        "⏸ 自动派发暂缓：当前在线节点无可匹配（角色/标签）节点；节点上线或要求调整后系统会自动重试，也可手动指派节点后派发。匹配明细：{detail}"
                                    ),
                                    parent_id: None,
                                    ctype: nemesis_board::CommentType::System,
                                })?;
                                // U-A：首次停车落现场档案（去重闸同款——只
                                // 首次写，sweep 重扫不覆写）。
                                write_side_handoff(
                                    store,
                                    &issue,
                                    "no_match",
                                    "retry",
                                    format!(
                                        "无在线节点满足角色/标签要求：上线合格节点后停车场 sweep 自动重派，或手动指派节点后派发。匹配明细：{detail}"
                                    ),
                                    "issue.dispatch（number=NB-x，可带 target 手动指派）；或等 sweep 自动重试",
                                    serde_json::json!({
                                        "required_role": issue.required_role,
                                        "required_tags": issue.required_tags,
                                        "required_profession": issue.required_profession,
                                    }),
                                    vec![serde_json::json!({
                                        "action": "match",
                                        "result": detail,
                                    })],
                                );
                            }
                            // B5 可见性：整条链都在等节点 → 父单转 blocked 显形。
                            mark_parent_blocked_if_stalled(store, &issue, actor);
                        }
                        return Ok(None);
                    }
                }
            }
        }
    };

    // 单一真相源：target 归一化为节点 id（与账本 worker_id / worker 上报
    // 身份 _rpc.from 同形态；D0 负载表键随之统一，防同一 worker 名字/id
    // 双键分裂计数）。T37 双身份失配第三处落点（2026-09-13）。
    let target = cluster
        .and_then(|c| c.canonical_peer_id(&target))
        .unwrap_or(target);

    // D0（goal P2）派发准入控制：目标 worker 在途派发数达上限 → 本单不派、
    // 留 backlog——新节点上线后 sweep 自然承接（防「同波多就绪单全挤单
    // worker、后上线的设备接不到活」）。cap=0 = 不限（旧行为）。B4 同款
    // 去重（独立 INFLIGHT_NOTICE_MARK）+ B5 父单受阻联动。
    let cap = board_cfg.map(|c| c.worker_max_inflight).unwrap_or(1);
    let load = store.count_active_dispatch_by_worker().unwrap_or_default();
    if inflight_full(&load, &target, cap) {
        let already_noted = store
            .last_system_comment(issue.id)
            .ok()
            .flatten()
            .map(|c| c.contains(INFLIGHT_NOTICE_MARK))
            .unwrap_or(false);
        if notify_park && !already_noted {
            let _ = store.add_comment(nemesis_board::NewComment {
                issue_id: issue.id,
                author: nemesis_board::Actor::system("board"),
                content: format!(
                    "⏸ {INFLIGHT_NOTICE_MARK}（{target} 在途 {inflight}/{cap}）：节点空闲或新节点上线后自动重试",
                    inflight = load.get(&target).copied().unwrap_or(0),
                    cap = cap
                ),
                parent_id: None,
                ctype: nemesis_board::CommentType::System,
            });
        }
        mark_parent_blocked_if_stalled(store, &issue, actor);
        return Ok(None);
    }

    let out = dispatch_issue_core(store, cluster, issue.id, &target, actor, None)?;

    // 父单联动：首张子单派出 → 父单 backlog/todo/blocked → in_progress
    //（blocked 来自 B5 停车联动，状态机 blocked→in_progress 合法）。
    if let Some(pid) = issue.parent_issue_id
        && let Ok(parent) = store.get_issue(pid)
        && matches!(
            parent.status,
            IssueStatus::Backlog | IssueStatus::Todo | IssueStatus::Blocked
        )
    {
        store.transition_issue(pid, IssueStatus::InProgress, actor)?;
    }
    Ok(Some(out))
}

/// 停车场父单受阻联动（B5）：停车单的父单尚未开工（backlog/todo）且
/// 所有兄弟子单都没派出过（无在途派发）→ 父单转 blocked + 系统评论。
/// 「整条链都在等节点」在看板受阻列显形，而不是停在 backlog 假装没发生
/// （生产实测 2026-09-10：用户建任务后"然后就没了"）。后续任一子单
/// 派出时由派发侧父单联动（bump 条件含 blocked）推回 in_progress。
#[cfg(feature = "cluster")]
fn mark_parent_blocked_if_stalled(
    store: &Arc<BoardStore>,
    parked: &nemesis_board::Issue,
    actor: &Actor,
) {
    let Some(pid) = parked.parent_issue_id else {
        return;
    };
    let Ok(parent) = store.get_issue(pid) else {
        return;
    };
    if !matches!(parent.status, IssueStatus::Backlog | IssueStatus::Todo) {
        return;
    }
    let Ok(children) = store.list_children(pid) else {
        return;
    };
    // 查询失败按"有在途"处理（保守：不动父单状态）。
    let all_unstarted = children.iter().all(|c| {
        matches!(c.status, IssueStatus::Backlog | IssueStatus::Todo)
            && !store.has_active_dispatch(c.id).unwrap_or(true)
    });
    if !all_unstarted {
        return;
    }
    if store
        .transition_issue(pid, IssueStatus::Blocked, actor)
        .is_ok()
    {
        let _ = store.add_comment(nemesis_board::NewComment {
            issue_id: pid,
            author: nemesis_board::Actor::system("board"),
            content: "⏸ 子任务自动派发暂缓（当前在线节点无可匹配），父任务转受阻；节点上线或要求调整后自动重试".to_string(),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        });
    }
}

/// 停车场 sweep（A2）：对「待派态 + planner 来源或曾被自动派发暂缓」的
/// 候选重试 [`dispatch_subissue_auto`]（其自带状态/在途/依赖/匹配/兜底闸）。
/// 触发点 = 节点发现回调（gateway 装配接线）——节点上线、身份/tags 变更、
/// 周期 announce 刷新都会触发；重试仍不满足时**静默**（notify_park=false）。
/// 返回 `(候选数, 派出数, 失败数)`（观测用）。
#[cfg(feature = "cluster")]
pub fn sweep_parked_dispatches(
    store: &Arc<BoardStore>,
    cluster: &Arc<nemesis_cluster::cluster::Cluster>,
    actor: &Actor,
) -> (usize, usize, usize) {
    sweep_parked_dispatches_notify(store, cluster, actor, false)
}

/// 周期兜底 ticker（F-U3-4）专用入口：`notify_park=true`——首次停车落
/// ⏸ 评论 + 父单 blocked 显形（与直派路径同语义；B4 去重保证重复 tick
/// 不刷屏），其余行为与 [`sweep_parked_dispatches`] 完全一致。
#[cfg(feature = "cluster")]
pub fn sweep_parked_dispatches_notify(
    store: &Arc<BoardStore>,
    cluster: &Arc<nemesis_cluster::cluster::Cluster>,
    actor: &Actor,
    notify_park: bool,
) -> (usize, usize, usize) {
    sweep_parked_dispatches_with_config(
        live_board_config().as_ref(),
        store,
        cluster,
        actor,
        notify_park,
    )
}

/// [`sweep_parked_dispatches`] 的可测内核：board 配置显式入参（测试不碰
/// 进程级全局单例），兜底开关判定也走同一入口。`notify_park` 透传给
/// [`dispatch_subissue_auto_with_config`]。进程级 [`PARK_SWEEP_LOCK`]
/// 串行三路触发源。
#[cfg(feature = "cluster")]
pub fn sweep_parked_dispatches_with_config(
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    store: &Arc<BoardStore>,
    cluster: &Arc<nemesis_cluster::cluster::Cluster>,
    actor: &Actor,
    notify_park: bool,
) -> (usize, usize, usize) {
    let _serial = PARK_SWEEP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // EST-02（2026-09-16 横扫加固）：estop 生效中 sweep 短路——派发核心
    // 的 estop 闸本会逐单拒绝，这里提前收敛，避免 20s 周期对全部停车候选
    // 的无效报错 churn。release 后（resume watcher / 定时器）自然恢复。
    if board_estop_engaged() {
        return (0, 0, 0);
    }
    // U-B（§9.2）：LLM 端点不可用封存中——派发重试 sweep 短路（worker 与
    // master 共享端点，发车必然烧在不可用端点上；恢复后 sweep 自然续跑，
    // 停车候选语义不变）。
    if super::board_unavailable::frozen_now() {
        tracing::debug!("[Board] 停车场 sweep 短路：LLM 端点不可用封存中");
        return (0, 0, 0);
    }
    let candidates = match store.list_dispatch_park_candidates() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[Board] 停车场 sweep 候选查询失败：{e}");
            return (0, 0, 0);
        }
    };
    let mut dispatched = 0usize;
    let mut failed = 0usize;
    for id in &candidates {
        match dispatch_subissue_auto_with_config(
            board_cfg,
            store,
            Some(cluster),
            *id,
            actor,
            notify_park,
        ) {
            Ok(Some(_)) => dispatched += 1,
            Ok(None) => {}
            Err(e) => {
                failed += 1;
                tracing::warn!("[Board] 停车场 sweep 派发 issue {id} 失败：{e}");
            }
        }
    }
    (candidates.len(), dispatched, failed)
}

/// D0b（goal P2）重平衡：节点 announce（上线/刷新）时，把其他 worker 手里
/// 「已派未开跑」（dispatched 且未 running）的排队单，按空闲 slot 挪给本
/// 节点——只从超载者（活跃 = 在途+排队 > cap，排队单存在即满载证据）偷
/// 队首；running 单不可迁移。新 worker 需满足该单角色/标签要求（同匹配器
/// 语义）。**file: 锚点单不偷**（[`nemesis_board::anchor::has_file_anchor`]，
/// 拓扑约束：转派远端必被派发拓扑闸拒绝——真机 NB-5/NB-6 实证取消后无法
/// 重派）。转派失败补偿：原排队派发已被取消，系统评论诚实留痕（issue 保
/// 留可重派状态，人工/autopilot 一发即活），不无声死透。竞态诚实边界：
/// task_cancel 下行到达前旧 worker 已出队开跑 → 任务照跑、其回写因
/// dispatch 已取消被写回幂等早退丢弃、重派新 task_id 正常执行（旧 worker
/// 白跑一轮，结果仍单份）。返回挪动单数。
#[cfg(feature = "cluster")]
pub async fn rebalance_queued_to_worker(
    store: &Arc<BoardStore>,
    cluster: &Arc<nemesis_cluster::cluster::Cluster>,
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    target_worker_id: &str,
    actor: &Actor,
) -> usize {
    let cap = board_cfg.map(|c| c.worker_max_inflight).unwrap_or(1);
    if cap <= 0 {
        return 0; // 0=不限：准入不生效，重平衡无意义。
    }
    let Ok(load) = store.count_active_dispatch_by_worker() else {
        return 0;
    };
    let target_load = load.get(target_worker_id).copied().unwrap_or(0);
    if target_load >= cap as usize {
        return 0; // 目标已满载，无处可挪。
    }
    let free = cap as usize - target_load;
    let Ok(queued) = store.list_queued_dispatches_excluding(target_worker_id) else {
        return 0;
    };
    if queued.is_empty() {
        return 0;
    }
    // 可执行性校验：新 worker 需满足该单角色/标签要求（同匹配器语义）。
    let candidates: Vec<nemesis_board::PeerCandidate> = project_dispatch_candidates(cluster)
        .into_iter()
        .filter(|c| c.id == target_worker_id)
        .collect();
    if candidates.is_empty() {
        return 0;
    }
    let target_candidate = candidates[0].clone();
    let Some(rpc) = cluster.rpc_client_arc() else {
        return 0;
    };

    let mut moved = 0usize;
    for d in queued {
        if moved >= free {
            break;
        }
        let from_load = load.get(&d.worker_id).copied().unwrap_or(0);
        if from_load <= cap as usize {
            continue; // 只从超载者（活跃 = 在途+排队 > cap，见函数注释）偷；普通欠载的排队单不动。
        }
        let Ok(issue) = store.get_issue(d.issue_id) else {
            continue;
        };
        // 拓扑约束（2026-10-08 NB-5/NB-6 根修①）：file: 锚点单只能在派发端
        // workspace 实核，转派远端必被 dispatch_issue_core 拓扑闸拒绝——
        // 而本函数 cancel 先行、失败不可逆。跳过不偷（留在原队列，原节点
        // 空闲后自然出队），谓词单一真相源 = has_file_anchor。
        if nemesis_board::anchor::has_file_anchor(
            issue.acceptance_criteria.as_deref().unwrap_or(""),
        ) {
            continue;
        }
        let (required_role, required_tags) = match issue.required_role.as_deref().map(str::trim) {
            Some(r) if !r.is_empty() && r != "worker" && r != "coordinator" => {
                let mut tags = issue.required_tags.clone();
                tags.push(r.to_string());
                (None, tags)
            }
            Some(r) if !r.is_empty() => (Some(r), issue.required_tags.clone()),
            _ => (None, issue.required_tags.clone()),
        };
        let input = nemesis_board::MatchInput {
            required_role,
            required_tags: &required_tags,
            required_profession: issue.required_profession.as_deref(),
            description: &issue.description,
        };
        // 新 worker 需满足要求（rank 命中才挪——重平衡不是无脑搬运）。
        if nemesis_board::rank_peers(&input, std::slice::from_ref(&target_candidate), &load)
            .is_empty()
        {
            continue;
        }

        // ① 取消旧排队派发：store 终态（dispatched → cancelled）+ task_cancel
        //    下行（fire-and-forget；竞态窗口=cancel 到达前已出队开跑 → 白跑，
        //    其回写被写回幂等早退丢弃）。
        if store.cancel_dispatch(&d.task_id, actor).ok().is_none() {
            continue; // 已被终结（回报/超时先行）→ 跳过。
        }
        let cancel_req = nemesis_cluster::rpc_types::RPCRequest {
            id: format!("rebalance-cancel-{}", d.task_id),
            action: nemesis_cluster::rpc_types::ActionType::Custom("task_cancel".to_string()),
            payload: serde_json::json!({ "task_id": d.task_id }),
            source: cluster.node_id().to_string(),
            target: Some(d.worker_id.clone()),
        };
        let _ = rpc.call(&d.worker_id, cancel_req).await;

        // ② 重派给新 worker（既有派发管线：拓扑硬闸/审计/经验注入照常）。
        match dispatch_issue_core(
            store,
            Some(cluster),
            d.issue_id,
            target_worker_id,
            actor,
            None,
        ) {
            Ok(_) => {
                moved += 1;
                let _ = store.add_comment(nemesis_board::NewComment {
                    issue_id: d.issue_id,
                    author: nemesis_board::Actor::system("board"),
                    content: format!(
                        "↳ D0b 重平衡：排队单从 {from}（在途 {from_load}>{cap}）转派至 {target_worker_id}",
                        from = d.worker_id,
                        from_load = from_load,
                        cap = cap
                    ),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
            }
            Err(e) => {
                tracing::warn!("[Board] D0b 重平衡重派 issue {} 失败：{e}", d.issue_id);
                // 补偿（2026-10-08 NB-5/NB-6 根修②）：取消已落库且不可逆，
                // issue 此时无活跃派发——系统评论诚实留痕，把「无声死透」
                // 变成「可见停车」。状态机 in_progress → todo 非法（不走
                // 回退）；issue 保留 in_progress 可被 dispatch_issue_core
                // 直接重派（人工/autopilot 一发即活）。评论失败不影响主流程
                // （数据已破坏，能留多少痕留多少痕）。
                let _ = store.add_comment(nemesis_board::NewComment {
                    issue_id: d.issue_id,
                    author: nemesis_board::Actor::system("board"),
                    content: format!(
                        "⛔ D0b 重平衡转派失败：{e}\n原排队派发（{worker}）已被取消且无法自动恢复，issue 无活跃派发。\
可重新派发（issue 当前状态允许直派）；若因验收锚点形态导致，请调整锚点后重试。",
                        worker = d.worker_id
                    ),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
            }
        }
    }
    moved
}

/// 在线节点 → PeerCandidate 投影（[`rank_dispatch_candidates`] 与兜底
/// 松弛排序 [`pick_fallback_target`] 共用的单一真相源）：数据源 = 注册表
/// 真实 tags ∪ category（A1 后 announce/静态 peers/get_info 的 tags 全链
/// 落地；曾长期降级为 `tags := [category]`，planner 的 required_tags 天然
/// 不可满足 → 全部停车）。
#[cfg(feature = "cluster")]
fn project_dispatch_candidates(
    cluster: &nemesis_cluster::cluster::Cluster,
) -> Vec<nemesis_board::PeerCandidate> {
    let peers = cluster.get_online_peers_excluding_self();
    peers
        .iter()
        .map(|p| nemesis_board::PeerCandidate {
            id: p.base.id.clone(),
            name: p.base.name.clone(),
            role: match p.base.role {
                nemesis_types::cluster::NodeRole::Coordinator => "coordinator",
                nemesis_types::cluster::NodeRole::Worker => "worker",
            }
            .to_string(),
            // A1：announce 携带的 tags 已落注册表——投影 = 真实 tags ∪
            // category（category 保留兜底：无 tags 的老节点行为不变）。
            tags: {
                let mut t = p.tags.clone();
                let c = p.base.category.trim();
                if !c.is_empty() && !t.iter().any(|x| x.eq_ignore_ascii_case(c)) {
                    t.push(c.to_string());
                }
                t
            },
            capabilities: p.capabilities.clone(),
            // M2：announce/静态 peers 携带的职能与档位已落注册表——同
            // tags 先例投影进 matcher（tier None = 闸 fail-open）。
            professions: p.professions.clone(),
            tier: p.tier.clone(),
        })
        .collect()
}

/// 匹配器全量排序（D3 换节点重派的候选来源 + [`pick_target_by_matcher`]
/// 的单一真相源）：在线节点（不含本机）→ 投影 →
/// [`nemesis_board::rank_peers`] 匹配度降序全量返回。节点角色词表只有
/// worker/coordinator，planner 给的其他角色词（如 "qa"）转标签语义与
/// tags/category 匹配。**file: 锚点单返回空候选**（拓扑约束，见函数内
/// 注释）——消费方按各自语义回落（Same 重派/停车）。
#[cfg(feature = "cluster")]
pub fn rank_dispatch_candidates(
    store: &BoardStore,
    cluster: &nemesis_cluster::cluster::Cluster,
    issue: &nemesis_board::Issue,
) -> Vec<String> {
    // required_role 归一：角色词表外的词转标签语义（进 required_tags）。
    let (required_role, required_tags) = match issue.required_role.as_deref().map(str::trim) {
        Some(r) if !r.is_empty() && r != "worker" && r != "coordinator" => {
            let mut tags = issue.required_tags.clone();
            tags.push(r.to_string());
            (None, tags)
        }
        Some(r) if !r.is_empty() => (Some(r), issue.required_tags.clone()),
        _ => (None, issue.required_tags.clone()),
    };

    let load = store.count_active_dispatch_by_worker().unwrap_or_default();
    // 拓扑约束（2026-10-08 NB-4/NB-5/NB-6 根修③）：file: 锚点单只能本节点
    // 执行，而候选投影全是远端节点（excluding_self）——返回空候选，让
    // 消费方走各自的「无候选」路径（换节点决策回落 Same(本节点) / 匹配器
    // 停车 / 冲突解决同节点重派），不产出注定被拓扑闸拒绝的非法目标。
    // 谓词单一真相源 = has_file_anchor。
    if nemesis_board::anchor::has_file_anchor(issue.acceptance_criteria.as_deref().unwrap_or("")) {
        return Vec::new();
    }
    let input = nemesis_board::MatchInput {
        required_role,
        required_tags: &required_tags,
        required_profession: issue.required_profession.as_deref(),
        description: &issue.description,
    };
    // E（goal P2/P5）：候选 tags 合并 master 授予标签（granted_tags 台账）。
    let candidates = merge_granted_tags(store, project_dispatch_candidates(cluster));
    nemesis_board::rank_peers(&input, &candidates, &load)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// 匹配器选节点：取 [`rank_dispatch_candidates`] 榜首。
#[cfg(feature = "cluster")]
fn pick_target_by_matcher(
    store: &BoardStore,
    cluster: &nemesis_cluster::cluster::Cluster,
    issue: &nemesis_board::Issue,
) -> Option<String> {
    rank_dispatch_candidates(store, cluster, issue)
        .into_iter()
        .next()
}

/// 停车场兜底目标（集群完备性加固 2026-09-11；2026-09-29 三档化）。自动
/// 派发匹配不到节点时，为了任务做下去由「一个客户端」推进。返回
/// `(节点 id, 兜底说明)`；None = 不兜底（调用方走原停车路径）。
///
/// 语义（用户裁决 2026-09-11 钉住规则 / 2026-09-29 三档默认 Role）：
/// - `Off` / 无在线节点 → None（兜底造不出客户端，诚实停车）。
/// - `dispatch_fallback_target` 钉住目标：按 name 或 id 精确匹配在线节点
///   （大小写不敏感）；**钉住的不在线 = None，不悄悄换人**（钉住即点名）。
/// - 未钉住：按档位走 D13 松弛阶梯——`Role` 档 ②保职能丢标签 → ③丢职能
///   保角色标签（worker/coordinator 纪律保留）；`Full` 档阶梯全开 ②③④。
///   rank_peers 同分按负载↑、id 字典序，确定性。
#[cfg(feature = "cluster")]
fn pick_fallback_target(
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    store: &BoardStore,
    cluster: &nemesis_cluster::cluster::Cluster,
    issue: &nemesis_board::Issue,
) -> Option<(String, &'static str)> {
    use nemesis_config::DispatchFallbackMode;
    // 拓扑约束（2026-10-08 根修④）：file: 锚点单只能本节点执行，兜底候选
    // 全是远端节点——兜底产物必被拓扑闸拒绝，直接 None 走停车路径（与
    // 匹配器无候选同语义）。谓词单一真相源 = has_file_anchor。
    if nemesis_board::anchor::has_file_anchor(issue.acceptance_criteria.as_deref().unwrap_or("")) {
        return None;
    }
    let cfg = board_cfg?;
    let depth = match cfg.dispatch_fallback {
        DispatchFallbackMode::Off => return None,
        DispatchFallbackMode::Role => nemesis_board::RelaxDepth::Role,
        DispatchFallbackMode::Full => nemesis_board::RelaxDepth::Full,
    };
    let peers = cluster.get_online_peers_excluding_self();
    if peers.is_empty() {
        return None;
    }

    // 1. 钉住目标：name/id 精确匹配在线节点；不在线诚实停车。
    if let Some(want) = cfg.dispatch_fallback_target.as_deref().map(str::trim)
        && !want.is_empty()
    {
        return peers
            .iter()
            .find(|p| {
                p.base.name.eq_ignore_ascii_case(want) || p.base.id.eq_ignore_ascii_case(want)
            })
            .map(|p| (p.base.id.clone(), "指定兜底节点"));
    }

    // 2. 松弛排序（D13 阶梯 = matcher::pick_relaxed：②保职能丢标签 →
    //    ③丢职能（保角色/标签）→ ④全松弛；无职能子单 ② 蜕化为旧「保
    //    角色丢标签」，行为不变）。角色归一沿用严格匹配（词表外角色词
    //    已在严格匹配里转 tags，这里只需认 worker/coordinator）。
    let candidates = merge_granted_tags(store, project_dispatch_candidates(cluster));
    let load = store.count_active_dispatch_by_worker().unwrap_or_default();
    let role = issue
        .required_role
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty() && (*r == "worker" || *r == "coordinator"));
    let base = nemesis_board::MatchInput {
        required_role: role,
        required_tags: &issue.required_tags,
        required_profession: issue.required_profession.as_deref(),
        description: &issue.description,
    };
    nemesis_board::pick_relaxed(&base, &candidates, &load, depth)
}

/// E（goal P2/P5）：候选投影合并 master 授予标签（tags ∪ granted_tags）。
/// 查询失败按无授予处理（不阻塞派发）。
#[cfg(feature = "cluster")]
fn merge_granted_tags(
    store: &BoardStore,
    mut candidates: Vec<nemesis_board::PeerCandidate>,
) -> Vec<nemesis_board::PeerCandidate> {
    let Ok(granted) = store.granted_tags_map() else {
        return candidates;
    };
    for c in &mut candidates {
        if let Some(g) = granted.get(&c.id) {
            for t in g {
                if !c.tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                    c.tags.push(t.clone());
                }
            }
        }
    }
    candidates
}

/// B1（goal P1）匹配失败明细（纯函数，单测直测）：要求 vs 每个在线候选
/// 差什么（与 rank_peers 同语义：required_role 精确、required_tags 交集
/// 非空；词表外角色词已在 rank_dispatch_candidates 归一进 tags）。
/// 输出人读行集，直接拼进停车评论——「差什么」一眼可见。
#[cfg(feature = "cluster")]
fn match_failure_detail(
    issue: &nemesis_board::Issue,
    candidates: &[nemesis_board::PeerCandidate],
) -> String {
    // 角色归一（与 rank_dispatch_candidates 同一规则）：词表外角色词转标签。
    let (required_role, required_tags) = match issue.required_role.as_deref().map(str::trim) {
        Some(r) if !r.is_empty() && r != "worker" && r != "coordinator" => {
            let mut tags = issue.required_tags.clone();
            tags.push(r.to_string());
            (None, tags)
        }
        Some(r) if !r.is_empty() => (Some(r.to_string()), issue.required_tags.clone()),
        _ => (None, issue.required_tags.clone()),
    };
    if candidates.is_empty() {
        return "无在线候选节点".to_string();
    }
    // 职能需求（M2）：精确匹配语义 + tier 闸差值都要在停车评论里可见。
    let prof_need = issue
        .required_profession
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(nemesis_prompts::professions::meta::normalize_slug);
    let prof_min_tier = prof_need
        .as_deref()
        .and_then(nemesis_prompts::professions::meta::find_builtin)
        .map(|m| m.min_tier);
    let mut lines = Vec::new();
    for c in candidates {
        let mut missing = Vec::new();
        if let Some(r) = &required_role
            && c.role.to_lowercase() != r.to_lowercase()
        {
            missing.push(format!("role≠{r}"));
        }
        for t in &required_tags {
            let hit = c.tags.iter().any(|x| x.eq_ignore_ascii_case(t.as_str()));
            if !hit {
                missing.push(format!("缺标签 {t}"));
            }
        }
        if let Some(need) = &prof_need {
            let hit = c
                .professions
                .iter()
                .any(|p| nemesis_prompts::professions::meta::normalize_slug(p) == *need);
            if !hit {
                missing.push(format!("未宣告职能 {need}"));
            }
        }
        // tier 未知（None）= fail-open，不算缺口。
        if let Some(min_tier) = prof_min_tier
            && let Some(tier) = &c.tier
            && !nemesis_prompts::professions::meta::tier_allows(Some(tier), min_tier)
        {
            missing.push(format!("档位 {tier} 低于门槛 {min_tier}"));
        }
        let tag_str = if c.tags.is_empty() {
            "—".to_string()
        } else {
            c.tags.join("/")
        };
        let prof_str = if c.professions.is_empty() {
            "—".to_string()
        } else {
            c.professions.join("/")
        };
        lines.push(format!(
            "{} {}[role={},tags={},prof={}]{}",
            if missing.is_empty() { "✓" } else { "✗" },
            c.name,
            c.role,
            tag_str,
            prof_str,
            if missing.is_empty() {
                String::new()
            } else {
                format!(" 缺:{}", missing.join(","))
            }
        ));
    }
    lines.join("；")
}

/// 父单状态联动：子树（Subtree 闭包，SAN-06/08）全部 done → 父单
/// in_review；子树任一 cancelled → 父单 in_review + 系统评论标注缺口（人工
/// 裁决重开或取消）。backlog/todo 先推 in_progress（状态机不允许跨列）。
/// **沿祖先链向上传播**（visited 防环）：终态父单不再早退阻断——状态机
/// 拒绝终态转移（不强行推列）但缺口评论照写、上层继续重估。旧实现只动
/// 直接父单且终态早退，中间层 done 之后其下取消叶子对上层永久冻结（收口
/// 汇报声称完成而子树实际存在取消缺口）。
#[cfg(feature = "cluster")]
pub fn sync_parent_status(
    store: &Arc<BoardStore>,
    parent_id: i64,
    actor: &Actor,
) -> Result<(), String> {
    let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut cur = Some(parent_id);
    while let Some(pid) = cur {
        if !visited.insert(pid) {
            break; // 数据异常 parent 自引用——防御性截断
        }
        cur = sync_one_parent(store, pid, actor)?;
    }
    Ok(())
}

/// 单层联动（[`sync_parent_status`] 内部）：评估 parent 的整棵子树并推
/// 进状态 → 返回再上一层父单 id（None = 已到顶）。
#[cfg(feature = "cluster")]
fn sync_one_parent(
    store: &Arc<BoardStore>,
    parent_id: i64,
    actor: &Actor,
) -> Result<Option<i64>, String> {
    let parent = store.get_issue(parent_id)?;
    let next = parent.parent_issue_id;
    let terminal = parent.status.is_terminal();
    // 子树闭包（SAN-08 Subtree）：只走父子边，穿过中间层看全部后代。
    let subtree = store.descendants(&[parent_id], DescendantEdges::Subtree);
    if subtree.is_empty() {
        return Ok(next);
    }
    let all_done = subtree.iter().all(|c| c.status == IssueStatus::Done);
    let any_cancelled = subtree.iter().any(|c| c.status == IssueStatus::Cancelled);
    if !all_done && !any_cancelled {
        return Ok(next);
    }

    // backlog/todo 不能直接进 in_review，先垫 in_progress（blocked 同理：
    // blocked→in_progress 合法而 blocked→in_review 非法——B5 停车联动的
    // 受阻父单全子落定走这里必须先垫）。终态父单跳过一切推列。
    if !terminal
        && matches!(
            parent.status,
            IssueStatus::Backlog | IssueStatus::Todo | IssueStatus::Blocked
        )
    {
        store.transition_issue(parent_id, IssueStatus::InProgress, actor)?;
    }
    if all_done {
        if !terminal && parent.status != IssueStatus::InReview {
            store.transition_issue(parent_id, IssueStatus::InReview, actor)?;
        }
        // 全自动流转 P1（A3）：子单全部落定 → 触发父单收口验收钩子。
        // 旗标判定在 hook 内（未注册 / auto_close_parent=false = no-op，
        // 等价现行为）；cancelled 缺口闸也在 hook 侧。终态父单已收过口，
        // 不重复触发。
        if !terminal && let Some(hook) = PARENT_REVIEW_HOOK.get() {
            hook(parent_id);
        }
    } else if !terminal && parent.status != IssueStatus::InReview {
        store.transition_issue(parent_id, IssueStatus::InReview, actor)?;
    }
    if !all_done {
        // 缺口评论列全子树非 done 项（旧实现只列直接子单——孙层缺口不可
        // 见）。终态父单（无法推列）也写：SAN-06 冻结缺口就此显形而非
        // 永久沉默。非终态父单维持旧防重语义（已在 in_review 不重复写）。
        if terminal || parent.status != IssueStatus::InReview {
            let gap: Vec<String> = subtree
                .iter()
                .filter(|c| c.status != IssueStatus::Done)
                .map(|c| format!("{}（{}）", c.number, c.status))
                .collect();
            let content = format!(
                "⚠ 子任务存在未完成项，父任务收口进评审等待人工裁决：{}",
                gap.join("、")
            );
            // C-F4（复核 2026-09-16）：终态父单缺口评论去重——terminal 分支
            // 没有状态变化可依（旧防重语义「已在 in_review 不重复写」对终态
            // 单恒不命中），此后每次子单落定触发 sync 都会重写同款评论刷屏。
            // 与最近一条同款系统评论逐字比较，内容未变则跳过；缺口集合变化
            // 时仍会补写显形（SAN-06 语义不变）。
            let duplicated = store
                .list_comments(parent_id)
                .map(|cs| {
                    cs.iter()
                        .rev()
                        .find(|cm| {
                            cm.ctype == nemesis_board::CommentType::System
                                && cm.content.starts_with("⚠ 子任务存在未完成项")
                        })
                        .is_some_and(|cm| cm.content == content)
                })
                .unwrap_or(false);
            if !duplicated {
                store.add_comment(nemesis_board::NewComment {
                    issue_id: parent_id,
                    author: nemesis_board::Actor::system("board"),
                    content,
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                })?;
            }
        }
    }
    Ok(next)
}

/// 子单落定（done/cancelled）后的联动 = M1 补派触发器 + 父单收口：扫
/// dependents 中依赖已满足的待派子单补派（`dispatch_subissue_auto` 自带
/// 状态/在途/依赖闸），随后父单状态联动。错误只 warn 不上抛（联动是
/// 附加动作，不阻断落定本身）。接线点：WSAPI `issue.status` / `issue.move`
/// 落到 done/cancelled 时（CLI 直写 store 不经此路径——M1 已知边界）。
/// 返回级联取消的编号列表（C3：`issue.cancel` 响应体披露）。
#[cfg(feature = "cluster")]
pub fn on_issue_settled(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue_id: i64,
    actor: &Actor,
) -> Vec<String> {
    let cancelled = store
        .get_issue(issue_id)
        .map(|i| i.status == IssueStatus::Cancelled)
        .unwrap_or(false);
    if let Ok(issue) = store.get_issue(issue_id)
        && let Some(pid) = issue.parent_issue_id
        && let Err(e) = sync_parent_status(store, pid, actor)
    {
        tracing::warn!("[Board] 父单 {pid} 联动失败：{e}");
    }
    // F3：顶层父单落定是项目收口验收的触发面（前置聚合检查在 notify 内，
    // 不过 = no-op）。
    notify_project_review_on_parent_done(store, issue_id);
    if cancelled {
        // B1：依赖闸只认 done 且状态机无 reopen——依赖取消后 dependents
        // 永远无法派发（死链）。级联取消让死链显形：各自留痕 + 父单经
        // sync_parent_status 进 in_review + 缺口评论。（reopen 已补上
        // cancelled→backlog 出口：级联取消的单可用 issue.reopen 复活。）
        // F-U4-3：级联边 = 依赖边 ∪ 父子边（见 cascade_cancel_dependents）。
        cascade_cancel_dependents(store, cluster, issue_id, actor)
    } else {
        match store.dependents_of(issue_id) {
            Ok(deps) => {
                for dep_id in deps {
                    if let Err(e) = dispatch_subissue_auto(store, cluster, dep_id, actor, true) {
                        tracing::warn!("[Board] 补派子单 {dep_id} 失败：{e}");
                    }
                }
            }
            Err(e) => tracing::warn!("[Board] dependents_of({issue_id}) 查询失败：{e}"),
        }
        Vec::new()
    }
}

/// 下行 task_cancel（fire-and-forget，30s 超时）：`issue.cancel` 在途取消
/// 与级联取消（[`cascade_cancel_dependents`]）共用的单一出口。送达失败不
/// 影响 A 侧终态（worker 回报被写回幂等早退兜住），评论留痕；RPC client
/// 不可用时 warn 放弃（best-effort，不阻断取消本体）。
#[cfg(feature = "cluster")]
fn spawn_task_cancel(
    store: &Arc<BoardStore>,
    cluster: &Arc<nemesis_cluster::cluster::Cluster>,
    issue_id: i64,
    worker_id: &str,
    task_id: &str,
) {
    let Some(rpc_client) = cluster.rpc_client_arc() else {
        tracing::warn!("[Board] task_cancel 未下发（RPC client 不可用）：task_id={task_id}");
        return;
    };
    let request = nemesis_cluster::rpc_types::RPCRequest {
        id: format!("cancel-{task_id}"),
        action: nemesis_cluster::rpc_types::ActionType::Custom("task_cancel".to_string()),
        payload: serde_json::json!({ "task_id": task_id }),
        source: cluster.node_id().to_string(),
        target: Some(worker_id.to_string()),
    };
    let store = store.clone();
    let worker_id = worker_id.to_string();
    let task_id = task_id.to_string();
    tokio::spawn(async move {
        let timeout = std::time::Duration::from_secs(30);
        match rpc_client
            .call_with_timeout(&worker_id, request, timeout)
            .await
        {
            Ok(_) => {
                tracing::info!("[Board] task_cancel delivered (task_id={task_id})");
            }
            Err(e) => {
                tracing::warn!("[Board] task_cancel send failed (task_id={task_id}): {e}");
                let _ = store.add_comment(nemesis_board::models::NewComment {
                    issue_id,
                    author: nemesis_board::Actor::system("board"),
                    content: format!("⛔ 取消指令送达失败（{e}），worker 端可能仍在执行"),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
            }
        }
    });
}

/// 级联取消（B1 + F-U4-3 + SAN-08）：root 落 cancelled 后，两条「挂在
/// 我身上就会陪葬」的边全部级联——**依赖边**（`dependents_of`：依赖我的
/// 单，依赖闸只认 done，死链显形）+ **父子边**（`list_children`：planner
/// 拆解的依赖边是兄弟链、从不指向父单，只走依赖边则 cancel 父单恒零级联，
/// 子单沦为僵尸 backlog 且补派触发器还会继续派它们空烧 token）。遍历统一
/// 走 [`nemesis_board::store::BoardStore::descendants`]（依赖∪父子闭包，
/// visited 防环）——**穿过终态单继续下探**：旧手搓 BFS 在终态单上直接
/// 跳过且不入队，done/cancelled 中间节点之下的死链从此不可见（依赖链
/// 穿不过已完成的环节）。终态过滤在动作侧：只有非终态节点被取消。
/// 在途派发连带取消（竞态守卫赢才动派发行 + 集群在则下行 task_cancel）。
/// 每单系统评论留痕（状态机自带 status_change 评论 + 活动 + 通知）；各
/// 父单经 [`sync_parent_status`] any_cancelled 分支进 in_review + 缺口
/// 评论。传导断路：某节点取消失败（竞态输给 worker 完成）则其下游不再
/// 连带取消（与旧「失败不入队」语义等价）。返回被连带取消的编号列表
/// （C3：cancel 响应体披露，不再无声）。
#[cfg(feature = "cluster")]
fn cascade_cancel_dependents(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    root_id: i64,
    actor: &Actor,
) -> Vec<String> {
    let mut cascaded: Vec<String> = Vec::new();
    let Ok(root_issue) = store.get_issue(root_id) else {
        return cascaded;
    };
    let mut stopped: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for dep in store.descendants(&[root_id], DescendantEdges::CascadeUnion) {
        if dep.status.is_terminal() {
            continue;
        }
        // 传导断路：任一直接上游（父单/依赖）取消失败 → 本单不再连带。
        let mut upstream_blocked = dep.parent_issue_id.is_some_and(|p| stopped.contains(&p));
        if !upstream_blocked {
            match store.dependencies_of(dep.id) {
                Ok(ups) => upstream_blocked |= ups.iter().any(|u| stopped.contains(u)),
                Err(e) => {
                    tracing::warn!("[Board] 级联取消 dependencies_of({}) 查询失败：{e}", dep.id)
                }
            }
        }
        if upstream_blocked {
            continue;
        }
        let _ = store.add_comment(nemesis_board::NewComment {
            issue_id: dep.id,
            author: nemesis_board::Actor::system("board"),
            content: format!(
                "⛔ {} 已取消，本单级联取消（父单或依赖取消即死链；可 issue.reopen 复活）",
                root_issue.number
            ),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        });
        // 在途派发连带取消：竞态守卫（赢 = 终结派发行）→ 集群在则下行
        // task_cancel（worker 不再空烧）。输给 worker 回报/超时 sweep →
        // 不动派发行（随后的 transition 状态机会诚实拒绝）。
        if let Ok(Some(d)) = store.get_active_dispatch(dep.id) {
            match store.cancel_dispatch(&d.task_id, actor) {
                Ok(Some(_)) => {
                    if let Some(c) = cluster {
                        spawn_task_cancel(store, c, dep.id, &d.worker_id, &d.task_id);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("[Board] 级联取消 {} 在途派发失败：{e}", dep.number)
                }
            }
        }
        if let Err(e) = store.transition_issue(dep.id, IssueStatus::Cancelled, actor) {
            tracing::warn!("[Board] 级联取消 {} 失败：{e}", dep.number);
            stopped.insert(dep.id);
            continue;
        }
        cascaded.push(dep.number.clone());
        if let Some(pid) = dep.parent_issue_id
            && let Err(e) = sync_parent_status(store, pid, actor)
        {
            tracing::warn!("[Board] 级联取消后父单 {pid} 联动失败：{e}");
        }
    }
    cascaded
}

/// `issue.cancel` 实现（cluster 编译时，W2 P4 per-task cancel + B2/B3 修复）：
/// 有在途派发 → 竞态守卫（只认 dispatched 态，Some=赢）→ 终态 → 下行
/// task_cancel；无在途派发（停车场单/依赖闸未放行单）→ 直接终态，集群
/// 非必需。两条路径落定后统一走 [`on_issue_settled`] 联动（父单收口 +
/// dependents 级联取消）——取消不再是孤岛操作。
#[cfg(feature = "cluster")]
async fn issue_cancel(
    store: &Arc<BoardStore>,
    actor: Actor,
    ctx: &RequestContext,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let data = data.ok_or("missing data")?;
    let id = data
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("missing field: id")?;
    let issue = store.get_issue(id)?;
    let dispatch = store.get_active_dispatch(id)?;

    // 有在途派发：集群必需 + 竞态守卫（只认 dispatched/running 态——D0b
    // running 同样在途、可取消，worker 侧 token cancel 打断执行中任务；
    // 输掉竞态——worker 恰好回报/超时 sweep 先到——不动 issue，报错让
    // 前端刷新）。
    let mut task_id = String::new();
    if let Some(dispatch) = dispatch {
        let cluster = ctx
            .state
            .cluster
            .clone()
            .ok_or("集群未运行，无法取消（issue.cancel 需要集群）")?;
        let worker_id = dispatch.worker_id.clone();
        let tid = dispatch.task_id.clone();
        if store.cancel_dispatch(&tid, &actor)?.is_none() {
            return Err("派发已终结（worker 回报或超时），取消未生效".to_string());
        }
        task_id = tid.clone();

        // 下行取消（fire-and-forget）：B 端 gateway 收 task_cancel → abort
        // 任务。送达失败不影响 A 侧终态（worker 回报被写回幂等早退兜住），
        // 评论留痕。（与级联取消共用 [`spawn_task_cancel`] 单一出口。）
        spawn_task_cancel(store, &cluster, id, &worker_id, &tid);
    }

    // 状态机：→ cancelled（终态）。无在途派发时直接转（B3：停车场单/
    // 依赖闸未放行单不需要集群参与）。
    let issue = if issue.status != IssueStatus::Cancelled {
        store.transition_issue(id, IssueStatus::Cancelled, &actor)?
    } else {
        issue
    };

    // B2：落定联动（父单收口 + dependents 级联取消）——错误只 warn，
    // 不回滚取消。C3：级联清单随响应披露（被连带取消的子单编号），不再
    // 无声——用户据此知道哪些单被连带、可用 issue.reopen 拉回。
    let cascade_cancelled = on_issue_settled(store, ctx.state.cluster.as_ref(), id, &actor);

    Ok(Some(serde_json::json!({
        "cancelled": true,
        "task_id": (!task_id.is_empty()).then_some(task_id),
        "cascade_cancelled": cascade_cancelled,
        "issue": issue_to_view(store, &issue)?,
    })))
}

/// `issue.cancel`（未编译 cluster）：取消要下行 task_cancel，无从谈起。
#[cfg(not(feature = "cluster"))]
async fn issue_cancel(
    _store: &Arc<BoardStore>,
    _actor: Actor,
    _ctx: &RequestContext,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("issue.cancel 需要集群支持（cluster feature 未编译）".to_string())
}

/// 自动派发判定（纯函数，可单测）：config 开关开 **且** 指派对象是 worker。
/// 开关 = `config.json` 的 `board.auto_dispatch`（W2.5 接口预留，用户拍板
/// 2026-08-31：现阶段不做自动派发，默认 false；置 true 即激活）。
fn should_auto_dispatch(
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    assignee: Option<AssignmentType>,
) -> bool {
    board_cfg.map(|b| b.auto_dispatch).unwrap_or(false) && assignee == Some(AssignmentType::Worker)
}

/// 读 live config 的 board 段（全局 ConfigStore 单例；测试/CLI 无单例 →
/// None → 判定恒 false，与「默认关」语义一致）。
fn live_board_config() -> Option<nemesis_config::BoardFlagConfig> {
    nemesis_config::load_live().and_then(|c| c.board)
}

/// 指派成功后的自动派发（`issue.assign` 末尾调用；W2.5 接口预留）。走
/// [`dispatch_issue_core`] 单一派发入口。失败**不回滚指派**：warn + ⛔ 系统
/// 评论留痕（与派发 RPC 送达失败同语义）。返回是否实际派发（调用方据此
/// 重取 issue 保证响应反映派发后的 in_progress 态）。
#[cfg(feature = "cluster")]
fn auto_dispatch_after_assign(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue: &nemesis_board::Issue,
    actor: &Actor,
) -> bool {
    auto_dispatch_with_config(live_board_config().as_ref(), store, cluster, issue, actor)
}

/// [`auto_dispatch_after_assign`] 的可测内核：config 显式入参（测试不碰
/// 进程级全局单例），判定 + 派发 + 失败留痕全部在此。
#[cfg(feature = "cluster")]
fn auto_dispatch_with_config(
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue: &nemesis_board::Issue,
    actor: &Actor,
) -> bool {
    if !should_auto_dispatch(board_cfg, issue.assignee) {
        return false;
    }
    let Some(target) = issue.assignee_id.clone() else {
        return false;
    };
    match dispatch_issue_core(store, cluster, issue.id, &target, actor, None) {
        Ok(_) => {
            tracing::info!(
                "[Board] auto-dispatch: issue #{} → {target}（board.auto_dispatch）",
                issue.number
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                "[Board] auto-dispatch failed for issue #{}: {e}",
                issue.number
            );
            let _ = store.add_comment(nemesis_board::models::NewComment {
                issue_id: issue.id,
                author: nemesis_board::Actor::system("board"),
                content: format!("⛔ 自动派发失败：{e}"),
                parent_id: None,
                ctype: nemesis_board::CommentType::System,
            });
            false
        }
    }
}

/// 非 cluster 编译：派发无从谈起，接口恒零操作。仍跑同一判定 gate——
/// 开关被误开时 warn 提示（配置生效但能力缺失，行为可观测），并保持
/// helper 在两种 feature 配置下都被使用（零 dead_code）。
#[cfg(not(feature = "cluster"))]
fn auto_dispatch_after_assign(
    _store: &Arc<BoardStore>,
    issue: &nemesis_board::Issue,
    _actor: &Actor,
) -> bool {
    if should_auto_dispatch(live_board_config().as_ref(), issue.assignee) {
        tracing::warn!(
            "[Board] board.auto_dispatch=true 但 cluster feature 未编译，无法自动派发（issue #{})",
            issue.number
        );
    }
    false
}

// ---------------------------------------------------------------------------
// autopilot（W2 P4 定时派活）：模板建单 + 可选派发 + cron 触发簿记
// ---------------------------------------------------------------------------

/// autopilot 模板 → NewIssue：标题 `{date}` 占位符替换为本地日期
/// （YYYY-MM-DD）；origin 记 autopilot（run 历史 =
/// `list_issues_by_origin("autopilot", id)`）；target 非空时预指派 worker。
fn autopilot_new_issue(ap: &nemesis_board::Autopilot, actor: &Actor) -> NewIssue {
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut ni = NewIssue {
        title: ap.title.replace("{date}", &date),
        description: ap.description.clone(),
        // F-U5-1：验收标准透传——定时任务也要能全自动验收（空 = 不填，
        // 评审保持「验收标准（未提供）」保守转人工语义）。
        acceptance_criteria: ap
            .acceptance_criteria
            .clone()
            .filter(|s| !s.trim().is_empty()),
        priority: ap.priority,
        project_id: ap.project_id,
        creator: actor.clone(),
        origin: Some(nemesis_board::models::TaskOrigin {
            origin_type: "autopilot".to_string(),
            origin_id: ap.id.to_string(),
        }),
        ..NewIssue::default()
    };
    let target = ap.target.trim();
    if !target.is_empty() {
        ni.assignee = Some(AssignmentType::Worker);
        ni.assignee_id = Some(target.to_string());
    }
    ni
}

/// 全自动流转 D2：autopilot auto_plan 触发 plan 链所需的运行时上下文。
/// cron 路径的 moderator 槽在 gateway 装配期晚填（OnceLock 模式，照
/// autopilot_cluster_slot 同款）；WSAPI 路径 agent_loop 现成，包一个即抛
/// 槽传入。`hub` cron 路径传 None（web server 尚未装配，诚实降级：无 SSE
/// 推送，评论/落库/发车链路完整）。`cluster` 供拆解后的派发波使用——
/// planner 产物无预指派，`dispatch_subissue_auto` 走匹配器选节点，没有
/// 集群引用时诚实降级（子单留 todo + ⛔ 评论，单节点模式即此形态）。
#[cfg(feature = "cluster")]
pub struct AutoPlanContext {
    /// planner 的 moderator AgentLoop 槽（空槽 = 未就绪，诚实降级见
    /// `fire_autopilot` 内注释）。
    pub moderator_slot: Arc<std::sync::OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
    /// home 目录（A1 auto_confirm 旗标现读）。
    pub home: std::path::PathBuf,
    /// SSE 事件 hub（None = 不推事件）。
    pub hub: Option<Arc<crate::events::EventHub>>,
    /// 集群引用（拆解后派发波的匹配器选节点依赖它；None = 派发诚实失败落
    /// 系统评论，与单节点模式语义一致）。
    pub cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
}

/// autopilot 触发核心（W2 P4；WSAPI `autopilot.run` 与 gateway 的 cron
/// on_job 共用，单一真相源）：按模板建 issue → target 非空时派发 →
/// 记 last_run_at。`cluster` 传 `None` 且 target 非空 → 建单前拒绝
/// （不留半成品）。
///
/// 全自动流转 D2：`auto_plan: Option<&AutoPlanContext>` —— `Some` 且规则
/// `auto_plan=true` 且未预指派 target → 建单后异步触发 plan 链（moderator
/// 槽空时 WARN + 系统评论诚实降级，不炸 cron 回调）。`None`（CLI 手跑等
/// 无运行时上下文的入口）且规则开了 auto_plan → 结果 JSON 标注 skipped。
#[cfg(feature = "cluster")]
pub fn fire_autopilot(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    ap: &nemesis_board::Autopilot,
    actor: &Actor,
    auto_plan: Option<&AutoPlanContext>,
) -> Result<serde_json::Value, String> {
    // EST-03（复核 2026-09-16）：入口前置闸——estop 中 autopilot 不建单。
    // 此前闸只在下游（dispatch_issue_core / execute_plan_chain 派发点），
    // estop 中到点的规则仍先建出单再在派发/拆解处失败，留下孤儿单 + 失败
    // 评论噪音。前置拒绝：cron 下一轮到点自动重试（estop_frozen_error
    // 文案已声明该语义）。非 cluster 版 fire_autopilot 只做纯建单（target
    // 必须空、auto_plan 降级），无 agent 活动链，不设闸。
    if board_estop_engaged() {
        return Err(estop_frozen_error());
    }
    let target = ap.target.trim().to_string();
    if !target.is_empty() && cluster.is_none() {
        return Err(format!(
            "autopilot「{}」配置了派发目标 {target}，但集群未运行，无法派发",
            ap.name
        ));
    }
    let issue = store.create_issue(autopilot_new_issue(ap, actor))?;
    let dispatch = if target.is_empty() {
        None
    } else {
        Some(
            dispatch_issue_core(store, cluster, issue.id, &target, actor, None)
                .map_err(|e| format!("issue #{} 已创建但派发失败: {e}", issue.number))?,
        )
    };
    // D2：无显式 assignee（target 空）+ 规则开 auto_plan → 自动拆解。
    let auto_plan_out = if target.is_empty() && ap.auto_plan {
        fire_autopilot_plan_chain(store, issue.id, actor, auto_plan)
    } else {
        serde_json::json!(null)
    };
    store.mark_autopilot_run(ap.id)?;
    Ok(serde_json::json!({
        "ran": true,
        "issue_id": issue.id,
        "issue_number": issue.number,
        "dispatch": dispatch,
        "auto_plan": auto_plan_out,
    }))
}

/// 全自动流转 P3/F1：项目自动启动——建父单（title=项目名、origin=project、
/// acceptance_criteria 透传、project_id 绑定）并后台触发 plan 链（与 WSAPI
/// `issue.plan` 同一 [`execute_plan_chain`]，D2/F1 共源）。返回创建的父单。
/// agent 未运行时诚实降级：父单照建 + 系统评论注明未拆解（项目已建成是
/// 事实，不因 planner 缺席回滚），不报错。
#[cfg(feature = "cluster")]
fn spawn_project_auto_start(
    store: &Arc<BoardStore>,
    ctx: &RequestContext,
    actor: &Actor,
    project_name: &str,
    project_description: &str,
    acceptance_criteria: Option<String>,
    project_id: i64,
) -> Result<nemesis_board::Issue, String> {
    let issue = store.create_issue(NewIssue {
        title: project_name.to_string(),
        description: project_description.to_string(),
        acceptance_criteria: acceptance_criteria.filter(|s| !s.trim().is_empty()),
        project_id: Some(project_id),
        origin: Some(nemesis_board::models::TaskOrigin {
            origin_type: "project".to_string(),
            origin_id: project_id.to_string(),
        }),
        creator: actor.clone(),
        ..NewIssue::default()
    })?;
    let agent_loop = match ctx.state.agent_loop.read().clone() {
        Some(l) => l,
        None => {
            tracing::warn!(
                "[Board] 项目自动启动 issue={}：agent 未运行，仅建父单不拆解",
                issue.id
            );
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("board"),
                content: "🤖 项目自动启动已建父单，但 agent 未运行，未自动拆解（可在看板手动拆解）"
                    .to_string(),
                parent_id: None,
                ctype: CommentType::System,
            });
            return Ok(issue);
        }
    };
    let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
    let env = std::sync::Arc::new(PlanChainEnv {
        store: store.clone(),
        cluster: ctx.state.cluster.clone(),
        agent_loop,
        issue: issue.clone(),
        actor: actor.clone(),
        home: ctx.home.clone().map(std::path::PathBuf::from),
        hub: Some(ctx.state.event_hub.clone()),
    });
    let cfg = plan_chain_cfg(env.home.as_deref());
    spawn_plan_chain(env, plan_id, 1, cfg);
    Ok(issue)
}

/// D2 内核：给刚建的 autopilot 单触发 plan 链（后台 spawn，不阻塞触发方）。
/// `auto_plan=None`（入口无运行时上下文）→ 返回 skipped 说明；槽空 →
/// WARN + 系统评论（moderator 未装配是可恢复状态——agent 重启间隙 cron
/// 恰好到点不该炸，下一轮规则触发自然恢复）。
#[cfg(feature = "cluster")]
fn fire_autopilot_plan_chain(
    store: &Arc<BoardStore>,
    issue_id: i64,
    actor: &Actor,
    auto_plan: Option<&AutoPlanContext>,
) -> serde_json::Value {
    use nemesis_board::models::{CommentType, NewComment};
    let Some(ap_ctx) = auto_plan else {
        return serde_json::json!({
            "status": "skipped",
            "reason": "auto_plan=true 但本入口无 moderator/事件上下文（CLI 手跑），未触发拆解",
        });
    };
    let Some(agent_loop) = ap_ctx.moderator_slot.get().cloned() else {
        tracing::warn!(
            "[Board] autopilot auto_plan issue={issue_id}：moderator loop 未就绪（agent 未装配完成），本轮跳过拆解",
        );
        let _ = store.add_comment(NewComment {
            issue_id,
            author: Actor::system("board"),
            content: "🤖 autopilot auto_plan 已开启，但 moderator agent 未就绪，本轮未自动拆解（下轮触发自动恢复）".to_string(),
            parent_id: None,
            ctype: CommentType::System,
        });
        return serde_json::json!({
            "status": "skipped",
            "reason": "moderator loop 未就绪",
        });
    };
    let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
    let issue_for_chain = match store.get_issue(issue_id) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("[Board] autopilot auto_plan issue={issue_id}：读取失败 {e}");
            return serde_json::json!({ "status": "failed", "reason": e });
        }
    };
    let env = std::sync::Arc::new(PlanChainEnv {
        store: store.clone(),
        cluster: ap_ctx.cluster.clone(), // 匹配器选节点靠它；None=派发诚实降级
        agent_loop,
        issue: issue_for_chain,
        actor: actor.clone(),
        home: Some(ap_ctx.home.clone()),
        hub: ap_ctx.hub.clone(),
    });
    let cfg = plan_chain_cfg(env.home.as_deref());
    spawn_plan_chain(env, plan_id.clone(), 1, cfg);
    serde_json::json!({ "status": "planning", "plan_id": plan_id })
}

/// 非 cluster 编译：autopilot 只支持 target 为空的周期建单。auto_plan=true
/// 的规则诚实降级：系统评论注明拆解未触发（需要集群编译），不静默吞。
#[cfg(not(feature = "cluster"))]
pub fn fire_autopilot(
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
    actor: &Actor,
) -> Result<serde_json::Value, String> {
    use nemesis_board::models::{CommentType, NewComment};
    if !ap.target.trim().is_empty() {
        return Err(format!(
            "autopilot「{}」配置了派发目标，但 cluster feature 未编译，无法派发",
            ap.name
        ));
    }
    let issue = store.create_issue(autopilot_new_issue(ap, actor))?;
    let auto_plan_skipped = ap.auto_plan;
    if auto_plan_skipped {
        let _ = store.add_comment(NewComment {
            issue_id: issue.id,
            author: Actor::system("board"),
            content: "🤖 autopilot auto_plan 已开启，但本节点未编译集群支持，无法自动拆解"
                .to_string(),
            parent_id: None,
            ctype: CommentType::System,
        });
    }
    store.mark_autopilot_run(ap.id)?;
    Ok(serde_json::json!({
        "ran": true,
        "issue_id": issue.id,
        "issue_number": issue.number,
        "dispatch": Option::<serde_json::Value>::None,
        "auto_plan": if auto_plan_skipped {
            serde_json::json!({
                "status": "skipped",
                "reason": "cluster feature 未编译",
            })
        } else {
            serde_json::json!(null)
        },
    }))
}

/// autopilot 的 cron job 即时登记/跟随更新（W2 P4）。cron 服务未注入
/// （单测/极简构建）时跳过——gateway 启动同步会以 store 为真相源补注册。
/// 已登记过（cron_job_id 命中且 job 还在）→ patch 跟随 store；否则新登记
/// 并回存 job id（`board-ap:{autopilot_id}` 名字约定，启动同步据此清孤儿）。
fn arm_autopilot_job(
    ctx: &RequestContext,
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
) -> Result<(), String> {
    let Some(cron) = ctx.state.cron.as_ref() else {
        return Ok(());
    };
    arm_autopilot_job_with(cron, store, ap)
}

/// arm 的核心（不依赖 RequestContext）：gateway 启动同步
/// （`sync_autopilot_jobs`）与 WSAPI handler 共用这一份构造逻辑——
/// 单一真相源，别在 gateway 里复制 CronSchedule/patch 语义。
fn arm_autopilot_job_with(
    cron: &Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    store: &Arc<BoardStore>,
    ap: &nemesis_board::Autopilot,
) -> Result<(), String> {
    let schedule = nemesis_cron::CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some(ap.cron.clone()),
        tz: None,
    };
    let svc = cron
        .lock()
        .map_err(|_| "cron service lock poisoned".to_string())?;
    if let Some(job_id) = ap.cron_job_id.as_deref()
        && svc.get_job(job_id).is_some()
    {
        svc.patch_job(
            job_id,
            &nemesis_cron::CronJobPatch {
                schedule: Some(schedule),
                enabled: Some(ap.enabled),
                ..Default::default()
            },
        )?;
        return Ok(());
    }
    // add_job_ext 返回随机 id（不支持指定 id），注册后回存映射。
    let job = svc.add_job_ext(
        &format!("board-ap:{}", ap.id),
        schedule,
        "",
        false,
        None,
        None,
        None,
        None,
        ap.enabled,
    )?;
    store.set_autopilot_cron_job(ap.id, Some(&job.id))
}

/// autopilot 删除时顺手摘掉 cron job（best-effort：失败只记日志，不阻断
/// 删除——启动同步的孤儿清理兜底）。
fn disarm_autopilot_job(ctx: &RequestContext, ap: &nemesis_board::Autopilot) {
    if let (Some(cron), Some(job_id)) = (ctx.state.cron.as_ref(), ap.cron_job_id.as_deref()) {
        match cron.lock() {
            Ok(svc) => match svc.remove_job(job_id) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!("[Board] autopilot cron job not found: {job_id}");
                }
                Err(e) => {
                    tracing::warn!("[Board] autopilot cron job {job_id} remove failed: {e}");
                }
            },
            Err(_) => tracing::warn!(
                "[Board] cron service lock poisoned; autopilot job {job_id} left behind"
            ),
        }
    }
}

/// W2 P4: autopilot cron job 启动同步（gateway 在 cron.start 前调用）。
/// store 是唯一真相源，把 cron 服务对齐到 store：
///   1. 删孤儿：cron 里 `board-ap:*` 命名空间的 job 在 store 侧已无对应
///      登记（规则被删/改时 backfill 失败留下的幽灵 job）→ 移除，防到点
///      触发；只动 board-ap: 前缀，不碰用户自己的 cron job。
///   2. 补登记：规则缺 job（上次登记时 cron 未注入、进程崩溃丢内存态）→
///      走 arm 重新登记并回存 job id。
///   3. 跟随：已登记的 job schedule/enabled 跟随 store（与 arm 同一逻辑，
///      幂等）。
///      返回重新登记的规则数（gateway 记日志用）。
pub fn sync_autopilot_jobs(
    cron: &Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    store: &Arc<BoardStore>,
) -> Result<usize, String> {
    let aps = store.list_autopilots()?;
    let valid_job_ids: std::collections::HashSet<&str> = aps
        .iter()
        .filter_map(|a| a.cron_job_id.as_deref())
        .collect();
    // 1) 孤儿清理。锁只在扫描段持有，remove 后即释放，不与 2)3) 嵌套。
    {
        let svc = cron
            .lock()
            .map_err(|_| "cron service lock poisoned".to_string())?;
        for job in svc.list_jobs(true) {
            if job.name.starts_with("board-ap:") && !valid_job_ids.contains(job.id.as_str()) {
                tracing::info!(
                    "[Board] autopilot sync: removing orphan cron job {} ({})",
                    job.id,
                    job.name
                );
                svc.remove_job(&job.id)
                    .map_err(|e| {
                        tracing::warn!("[Board] orphan cron job {} remove failed: {e}", job.id)
                    })
                    .ok();
            }
        }
    }
    // 2)+3) 逐条 arm（内部自判 patch 跟随 or 新登记；均幂等）。
    let mut rearmed = 0;
    for ap in &aps {
        let registered = match ap.cron_job_id.as_deref() {
            Some(jid) => {
                let svc = cron
                    .lock()
                    .map_err(|_| "cron service lock poisoned".to_string())?;
                svc.get_job(jid).is_some()
            }
            None => false,
        };
        arm_autopilot_job_with(cron, store, ap)?;
        if !registered {
            rearmed += 1;
        }
    }
    Ok(rearmed)
}

#[async_trait::async_trait]
impl ModuleHandler for BoardHandler {
    fn module_name(&self) -> &str {
        "board"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "issue.list",
            "issue.get",
            "issue.timeline",
            "issue.create",
            "issue.update",
            "issue.assign",
            "issue.status",
            "issue.move",
            "issue.dispatch",
            "issue.cancel",
            "issue.reopen",
            "issue.plan",
            "plan.list",
            "plan.get",
            "issue.bulk_archive",
            "autopilot.list",
            "autopilot.create",
            "autopilot.update",
            "autopilot.remove",
            "autopilot.run",
            "autopilot.runs",
            "comment.add",
            "comment.list",
            "activity.list",
            "subscriber.add",
            "subscriber.remove",
            "subscriber.list",
            "project.list",
            "project.progress",
            "project.resume",
            "project.create",
            "project.update",
            "project.open_dir",
            "attachment.add",
            "attachment.get",
            "inbox.list",
            "inbox.mark_read",
            "attachment.list",
            "channel.list",
            "channel.messages",
            "channel.post",
            "stats",
            "audit.list",
            "audit.rollback",
            "audit.retry_merge",
            "review.retrigger",
            "unavailable.status",
            "unavailable.release",
            "config.get",
            "config.set",
            "dossier.export",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // 单节点模式下 board 命令不依赖 workspace（store 由 gateway 按已解析
        // home 打开），但保留 workspace 校验与其他 handler 一致。
        require_workspace(ctx)?;
        let store = require_board(ctx)?;
        let actor = ctx_actor(ctx);
        match cmd {
            // --- issue ---
            "issue.list" => {
                let data = data.ok_or("missing data")?;
                let issues = store.list_issues(&build_filter(&data)?)?;
                let total = issues.len();
                // goal P1/C2：逐单附环节（issue_stage 单一真相源）——前端
                // 子单行环节徽标数据源；加法变更，旧前端兼容。
                // §1.1 拆解运行期投影（cluster 编译时）：命中 RUNNING_PLANS
                // 的单附 `planning: {elapsed_secs, attempt}`（纯展示叠加，
                // 不改 issue 状态机——backlog 语义不变）。前端列表徽标/
                // 项目卡片行「🧩 拆解中」数据源。
                #[cfg(feature = "cluster")]
                let planning_now: std::collections::HashMap<i64, (u64, u32)> = {
                    let now = std::time::Instant::now();
                    RUNNING_PLANS
                        .lock()
                        .values()
                        .map(|r| {
                            (
                                r.issue_id,
                                (now.duration_since(r.started_at).as_secs(), r.attempt),
                            )
                        })
                        .collect()
                };
                let mut rows = Vec::with_capacity(issues.len());
                for issue in &issues {
                    let mut v =
                        serde_json::to_value(issue).map_err(|e| format!("serialize issue: {e}"))?;
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert(
                            "stage".to_string(),
                            serde_json::json!(issue_stage(&store, issue)?),
                        );
                        #[cfg(feature = "cluster")]
                        if let Some((elapsed, attempt)) = planning_now.get(&issue.id) {
                            obj.insert(
                                "planning".to_string(),
                                serde_json::json!({
                                    "elapsed_secs": elapsed,
                                    "attempt": attempt,
                                }),
                            );
                        }
                    }
                    rows.push(v);
                }
                Ok(Some(serde_json::json!({ "issues": rows, "total": total })))
            }
            "issue.get" => {
                let data = data.ok_or("missing data")?;
                let issue = if let Ok(id) = data.get("id").and_then(|v| v.as_i64()).ok_or(()) {
                    store.get_issue(id)?
                } else {
                    store.get_issue_by_number(&get_str(&data, "number")?)?
                };
                Ok(Some(
                    serde_json::json!({ "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            // U-C（2026-10-12 §9.3）：单子全链路时间线（可追溯查询面——终结
            // 「人查 4 个库」）。聚合入口与 dossier timeline.md 同源
            // （nemesis-board::timeline）；集群侧事件（任务结果 / 续行快照
            // 在场）在此注入。按 (ts, source_rank) 升序。
            "issue.timeline" => {
                let data = data.ok_or("missing data")?;
                let issue = if let Ok(id) = data.get("id").and_then(|v| v.as_i64()).ok_or(()) {
                    store.get_issue(id)?
                } else {
                    store.get_issue_by_number(&get_str(&data, "number")?)?
                };
                #[allow(unused_mut)] // 非 cluster 编译：extra 恒空仍照常传聚合入口
                let mut extra: Vec<nemesis_board::timeline::TimelineEvent> = Vec::new();
                #[cfg(feature = "cluster")]
                if let Some(cluster) = ctx.state.cluster.as_ref() {
                    for d in store.list_dispatches(issue.id)? {
                        // 已终结任务：结果存储在案（success/failure + stored_at）。
                        if let Some(r) = cluster.result_store().get(&d.task_id) {
                            extra.push(nemesis_board::timeline::TimelineEvent {
                                ts: nemesis_board::timeline::parse_ts_to_epoch(&r.stored_at),
                                source: "cluster_task",
                                kind: if r.success { "success" } else { "failure" }.into(),
                                actor: d.worker_id.clone(),
                                summary: format!(
                                    "任务结果（{}）：{}",
                                    r.action,
                                    if r.success { "成功" } else { "失败" }
                                ),
                                detail: r.result.as_str().map(|s| s.to_string()).or_else(|| {
                                    r.result
                                        .get("error")
                                        .and_then(|e| e.as_str())
                                        .map(String::from)
                                }),
                            });
                        } else if cluster.continuation_store().contains(&d.task_id) {
                            // 在途：续行快照在场（任务未终结）。
                            extra.push(nemesis_board::timeline::TimelineEvent {
                                ts: d.dispatched_at,
                                source: "continuation",
                                kind: "pending".into(),
                                actor: d.worker_id.clone(),
                                summary: format!("任务 {} 在途（续行快照在场）", d.task_id),
                                detail: Some(d.task_id),
                            });
                        }
                    }
                }
                let events =
                    nemesis_board::timeline::collect_issue_timeline(&store, issue.id, extra);
                Ok(Some(serde_json::json!({
                    "issue": issue.number,
                    "issue_id": issue.id,
                    "events": events,
                })))
            }
            "issue.create" => {
                let data = data.ok_or("missing data")?;
                let issue = store.create_issue(build_new_issue(&data, actor)?)?;
                let mut out =
                    serde_json::json!({ "created": true, "issue": issue_to_view(&store, &issue)? });
                // F5 件①：建单警示——验收标准含 `[CHECK] re:` 锚点时提醒
                // 锚点下放纪律（叶子单直接执行不受影响；本单日后被 AI 拆解
                // 成父单则父层无交付文本，锚点必须由子单继承才可核验）。
                if nemesis_board::has_content_regex_anchor(
                    issue.acceptance_criteria.as_deref().unwrap_or(""),
                ) {
                    out["warning"] = serde_json::json!(
                        "验收标准含 [CHECK] re: 锚点：本单若后续被 AI 拆解为父单，父单层没有交付文本可核验该锚点（验收必败）——拆解时请确保至少一个子单继承该锚点。直接执行（不拆解）的单不受影响。"
                    );
                }
                Ok(Some(out))
            }
            "issue.update" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let patch = build_patch(&data);
                let issue = store.update_issue(id, &patch, &actor)?;
                Ok(Some(
                    serde_json::json!({ "updated": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            "issue.assign" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let (assignee, assignee_id) = match parse_assignee(&data)? {
                    Some((at, aid)) => (Some(at), Some(aid)),
                    None => (None, None),
                };
                let issue = store.assign_issue(id, assignee, assignee_id, &actor)?;
                // 自动派发接口（W2.5 预留，默认关；board.auto_dispatch=true
                // 且指派给 worker 时触发）。触发后重取 issue，响应反映派发
                // 推进的 in_progress 态。
                #[cfg(feature = "cluster")]
                let dispatched =
                    auto_dispatch_after_assign(&store, ctx.state.cluster.as_ref(), &issue, &actor);
                #[cfg(not(feature = "cluster"))]
                let dispatched = auto_dispatch_after_assign(&store, &issue, &actor);
                let issue = if dispatched {
                    store.get_issue(id)?
                } else {
                    issue
                };
                Ok(Some(
                    serde_json::json!({ "assigned": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            "issue.status" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let to = parse_status(&data)?;
                let issue = store.transition_issue(id, to, &actor)?;
                // M1 补派触发器 + 父单收口（子单落定后联动；错误只 warn）。
                #[cfg(feature = "cluster")]
                if matches!(to, IssueStatus::Done | IssueStatus::Cancelled) {
                    on_issue_settled(&store, ctx.state.cluster.as_ref(), id, &actor);
                }
                Ok(Some(
                    serde_json::json!({ "changed": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            // 看板拖拽（W2 P3）：状态转移 + 列内排序一个原子操作（同列重排
            // 只改 position；跨列走状态机）。
            "issue.move" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let to = parse_status(&data)?;
                let position = data
                    .get("position")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: position")?;
                let issue = store.move_issue(id, to, position, &actor)?;
                // M1 补派触发器 + 父单收口（跨列拖到 done/cancelled 同样落定）。
                #[cfg(feature = "cluster")]
                if matches!(to, IssueStatus::Done | IssueStatus::Cancelled) {
                    on_issue_settled(&store, ctx.state.cluster.as_ref(), id, &actor);
                }
                Ok(Some(
                    serde_json::json!({ "moved": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            // 派发到远端 worker（W2 P2）：issue → peer_chat，task_id ↔ issue
            // 绑定入 issue_dispatch 表；worker 回报经 peer_chat_callback 由
            // gateway 写回看板（成功 → 结果评论 + in_review，失败 → 失败评论）。
            "issue.dispatch" => issue_dispatch(&store, actor, ctx, data).await,
            // 取消进行中的派发（W2 P4）：A 侧派发/issue 双终态 + 下行
            // task_cancel 让 worker abort。赢竞态才动账（worker 恰好回报则
            // 拒绝取消，issue 保持写回的状态）。响应带 cascade_cancelled
            //（C3：被级联取消的子单编号清单）。
            "issue.cancel" => issue_cancel(&store, actor, ctx, data).await,
            // reopen（发现④ 2026-09-15）：cancelled → backlog 唯一终态
            // 出口，自动追加审计评论 + 解除 hidden；与 CLI `issue reopen`
            // 共用 store.reopen_issue 单一后端。
            "issue.reopen" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let issue = store.reopen_issue(id, &actor)?;
                Ok(Some(
                    serde_json::json!({ "reopened": true, "issue": issue_to_view(&store, &issue)? }),
                ))
            }
            // AI 拆解（Swarm M1）：confirm 缺省/false = 异步跑 planner（返回
            // plan_id，结果经 board.plan_ready/failed push）；confirm:true =
            // 按 plan_id 落库 + 依赖闸派发波 + 父单联动。
            "issue.plan" => issue_plan(&store, actor, ctx, data).await,
            // P11 观测面：plan 预览查询（list 全量存活按 issue_id 可选过滤 /
            // get 单查；TTL 惰性过滤与插入点同判据）。
            "plan.list" => plan_list(&store, data),
            "plan.get" => plan_get(data),
            // 取消单清理（P1/A2b）：批量给已取消单打 hidden 标记（永久收起，
            // 列表面无放行口）。只认 status=cancelled——混入非取消单整体拒绝
            //（不是跳过，语义诚实）；前端二次确认后调用；逐单写
            // issue_bulk_archive 审计。
            "issue.bulk_archive" => {
                let data = data.ok_or("missing data")?;
                let ids: Vec<i64> =
                    serde_json::from_value(data.get("ids").cloned().ok_or("missing field: ids")?)
                        .map_err(|e| format!("ids 应为数字数组: {e}"))?;
                if ids.is_empty() {
                    return Err("ids 不能为空".to_string());
                }
                let issues = store.bulk_archive_cancelled(&ids, &actor)?;
                Ok(Some(serde_json::json!({
                    "archived": issues.len(),
                    "issues": issues,
                })))
            }
            // --- autopilot（W2 P4 定时派活）---
            "autopilot.list" => {
                let autopilots = store.list_autopilots()?;
                Ok(Some(serde_json::json!({ "autopilots": autopilots })))
            }
            "autopilot.create" => {
                let data = data.ok_or("missing data")?;
                let cron_expr = get_str(&data, "cron")?;
                nemesis_cron::CronService::validate_schedule(&cron_expr)?;
                let ap = store.create_autopilot(&nemesis_board::NewAutopilot {
                    name: get_str(&data, "name")?.to_string(),
                    title: get_str(&data, "title")?.to_string(),
                    cron: cron_expr.to_string(),
                    description: get_opt_str(&data, "description").unwrap_or_default(),
                    priority: data
                        .get("priority")
                        .and_then(|v| v.as_i64())
                        .map(|p| p as i32)
                        .unwrap_or(nemesis_board::models::priority::MEDIUM),
                    project_id: data.get("project_id").and_then(|v| v.as_i64()),
                    target: get_opt_str(&data, "target").unwrap_or_default(),
                    enabled: data
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true),
                    // 全自动流转 D2：建单后自动 planner 拆解（默认 false，
                    // 存量前端不传时行为不变）。
                    auto_plan: data
                        .get("auto_plan")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    // F-U5-1：验收标准（触发建单透传；空 = 不填）。
                    acceptance_criteria: get_opt_str(&data, "acceptance_criteria")
                        .filter(|s| !s.trim().is_empty()),
                })?;
                // cron 已注入 → 即时登记并回填 cron_job_id；未注入（单测/
                // 极简构建）→ 启动同步兜底。
                arm_autopilot_job(ctx, &store, &ap)?;
                let ap = store.get_autopilot(ap.id)?;
                Ok(Some(
                    serde_json::json!({ "created": true, "autopilot": ap }),
                ))
            }
            "autopilot.update" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                if let Some(c) = get_opt_str(&data, "cron") {
                    nemesis_cron::CronService::validate_schedule(&c)?;
                }
                let ap = store.update_autopilot(
                    id,
                    &nemesis_board::AutopilotPatch {
                        name: get_opt_str(&data, "name"),
                        cron: get_opt_str(&data, "cron"),
                        title: get_opt_str(&data, "title"),
                        description: get_opt_str(&data, "description"),
                        priority: data
                            .get("priority")
                            .and_then(|v| v.as_i64())
                            .map(|p| p as i32),
                        project_id: data.get("project_id").and_then(|v| v.as_i64()),
                        target: get_opt_str(&data, "target"),
                        enabled: data.get("enabled").and_then(|v| v.as_bool()),
                        auto_plan: data.get("auto_plan").and_then(|v| v.as_bool()),
                        // F-U5-1：验收标准（Some("") = 清空；缺省 = 不改）。
                        acceptance_criteria: get_opt_str(&data, "acceptance_criteria"),
                    },
                )?;
                arm_autopilot_job(ctx, &store, &ap)?;
                let ap = store.get_autopilot(ap.id)?;
                Ok(Some(
                    serde_json::json!({ "updated": true, "autopilot": ap }),
                ))
            }
            "autopilot.remove" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let ap = store.get_autopilot(id)?;
                disarm_autopilot_job(ctx, &ap);
                let removed = store.remove_autopilot(id)?;
                Ok(Some(serde_json::json!({ "removed": removed, "id": id })))
            }
            // 手动触发一次（到点自动触发走 gateway on_job → 同一
            // fire_autopilot 核心）。
            "autopilot.run" => {
                // F-U4-5：急停中拒绝手动触发（target 非空的规则会发车）。
                refuse_dispatch_when_estopped(ctx)?;
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let ap = store.get_autopilot(id)?;
                let out = {
                    #[cfg(feature = "cluster")]
                    {
                        // D2：WSAPI 有现成 moderator/事件上下文——包即抛槽
                        // 传入（槽空 = agent 未运行，auto_plan 走诚实降级）。
                        let auto_plan_ctx = AutoPlanContext {
                            moderator_slot: {
                                let slot: Arc<
                                    std::sync::OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>,
                                > = Arc::new(std::sync::OnceLock::new());
                                if let Some(loop_arc) = ctx.state.agent_loop.read().clone() {
                                    let _ = slot.set(loop_arc);
                                }
                                slot
                            },
                            home: ctx
                                .home
                                .clone()
                                .map(std::path::PathBuf::from)
                                .unwrap_or_default(),
                            hub: Some(ctx.state.event_hub.clone()),
                            cluster: ctx.state.cluster.clone(),
                        };
                        fire_autopilot(
                            &store,
                            ctx.state.cluster.as_ref(),
                            &ap,
                            &actor,
                            Some(&auto_plan_ctx),
                        )?
                    }
                    #[cfg(not(feature = "cluster"))]
                    {
                        fire_autopilot(&store, &ap, &actor)?
                    }
                };
                Ok(Some(out))
            }
            // run 历史：origin=autopilot 的 issue 列表（最新在前）。
            "autopilot.runs" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let limit = data
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(20)
                    .clamp(1, 100) as usize;
                let issues = store.list_issues_by_origin("autopilot", &id.to_string(), limit)?;
                Ok(Some(serde_json::json!({ "issues": issues })))
            }
            // --- comment / activity ---
            "comment.add" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let content = get_str(&data, "content")?;
                let comment = store.add_comment(NewComment {
                    issue_id,
                    author: actor,
                    content: content.clone(),
                    parent_id: data.get("parent_id").and_then(|v| v.as_i64()),
                    ctype: CommentType::Comment,
                })?;
                // M5（2026-10-10 集群无人值守硬化）：审批回执回源——
                // `/approve|/deny <短号>` 评论转发远程审批（短号在注册表才
                // 动作；失败留痕为系统评论）。异步旁路，不阻塞本响应。
                #[cfg(feature = "cluster")]
                if crate::remote_approval::looks_like_approval_reply(&content)
                    && let Some(cluster) = ctx.state.cluster.as_ref()
                {
                    let store = store.clone();
                    let cluster = cluster.clone();
                    tokio::spawn(async move {
                        crate::remote_approval::try_remote_approval_reply(
                            &store, &cluster, issue_id, &content,
                        )
                        .await;
                    });
                }
                Ok(Some(
                    serde_json::json!({ "added": true, "comment": comment }),
                ))
            }
            "comment.list" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let comments = store.list_comments(issue_id)?;
                Ok(Some(serde_json::json!({ "comments": comments })))
            }
            "activity.list" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let activity = store.list_activity(issue_id)?;
                Ok(Some(serde_json::json!({ "activity": activity })))
            }
            // --- subscriber ---
            "subscriber.add" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                store.subscribe(issue_id, &actor, "manual")?;
                Ok(Some(
                    serde_json::json!({ "subscribed": true, "issue_id": issue_id }),
                ))
            }
            "subscriber.remove" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                store.unsubscribe(issue_id, &actor)?;
                Ok(Some(
                    serde_json::json!({ "unsubscribed": true, "issue_id": issue_id }),
                ))
            }
            "subscriber.list" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let subscribers = store.list_subscribers(issue_id)?;
                Ok(Some(serde_json::json!({ "subscribers": subscribers })))
            }
            // --- project / attachment / stats ---
            "project.list" => {
                let projects = store.list_projects()?;
                Ok(Some(serde_json::json!({ "projects": projects })))
            }
            // 项目进度聚合（goal P1/C1+C2）：带 project_id = 单项目（含逐单
            // 环节数据）；不带 = 全项目摘要（前端列表页一次拉取）。
            "project.progress" => {
                let project_id = data
                    .as_ref()
                    .and_then(|d| d.get("project_id"))
                    .and_then(|v| v.as_i64());
                match project_id {
                    Some(pid) => Ok(Some(project_progress(&store, pid)?)),
                    None => Ok(Some(project_progress_all(&store)?)),
                }
            }
            // D（goal P2）项目级恢复发车：入口项目上一键重试全部可派未派子单
            // （拍板①：派发顺序由 AI/依赖闸自行决定）。estop 中拒绝（护栏）。
            "project.resume" => {
                let data = data.ok_or("missing data")?;
                let project_id = data
                    .get("project_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: project_id")?;
                let dry_run = data
                    .get("dry_run")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // 护栏三不变：estop 急停中拒绝恢复发车。
                if ctx
                    .state
                    .estop
                    .as_ref()
                    .map(|e| e.is_engaged())
                    .unwrap_or(false)
                {
                    return Err("⛔ 急停（E-STOP）生效中：恢复发车被拒绝（先释放急停）".to_string());
                }
                // as_ref 借用而非 clone：feature 关时字段退化为 Option<()>（Copy），
                // `.clone()` 会撞 clippy clone_on_copy（裁剪构建 clippy -p 现实告警）；
                // 两个 cfg 臂的 project_resume 形参（&Arc<Cluster> / &()）都恰好收借用。
                let cluster = ctx
                    .state
                    .cluster
                    .as_ref()
                    .ok_or("集群未运行，无法恢复派发")?;
                let board_cfg = live_board_config();
                let out = project_resume(
                    &store,
                    cluster,
                    board_cfg.as_ref(),
                    project_id,
                    dry_run,
                    &actor,
                )
                .await?;
                Ok(Some(out))
            }
            "project.create" => {
                let data = data.ok_or("missing data")?;
                // 全自动流转 P3/F1：可选验收标准 + 自动启动。保守默认
                // （auto_start=false）——存量前端建项目行为不变。
                let acceptance_criteria = get_opt_str(&data, "acceptance_criteria");
                let auto_start = data
                    .get("auto_start")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // 看板项目档案 goal P2/B1-B2：可选目录入参（None = 自动分配
                // `<workspace>/board-projects/<安全化名>/`）。解析/安全化/
                // 重叠拒绝/四件套脚手架都在 `archive::resolve_project_directory`
                // + `ensure_scaffold`（绑定不可变，此后无修改入口）。
                let workspace = require_workspace(ctx)?;
                let name = get_str(&data, "name")?;
                // 同名预检（2026-10-11 用户实测）：项目名有 UNIQUE 约束，
                // 重名此前透传裸 `UNIQUE constraint failed: project.name`
                // ——用户看不懂也无处查。中文提示 + 现状指引；UNIQUE 约束
                // 仍在兜底并发窗口。
                if store.list_projects()?.iter().any(|p| p.name == name) {
                    return Err(format!(
                        "已存在同名项目「{name}」：项目名需唯一。请在项目列表先处理旧项目（归档或改名）再重试，或换一个项目名。"
                    ));
                }
                let requested_dir = get_opt_str(&data, "directory");
                let existing_dirs: Vec<std::path::PathBuf> = store
                    .list_projects()?
                    .into_iter()
                    .filter_map(|p| p.directory.map(std::path::PathBuf::from))
                    .collect();
                let dir_path = nemesis_board::resolve_project_directory(
                    requested_dir.as_deref(),
                    std::path::Path::new(workspace),
                    &name,
                    &existing_dirs,
                )?;
                nemesis_board::ensure_scaffold(
                    &dir_path,
                    // project_id 尚未生成，manifest 在创建后由
                    // sync_project_manifest 回填真实 id。
                    0, &name, "active",
                )?;
                let project = store.create_project(
                    &name,
                    &get_opt_str(&data, "description").unwrap_or_default(),
                    None,
                    &get_opt_str(&data, "icon").unwrap_or_default(),
                    acceptance_criteria.as_deref().unwrap_or(""),
                    Some(&dir_path.to_string_lossy()),
                )?;
                // manifest 回填真实 project_id（脚手架先建，id 后生成）。
                nemesis_board::sync_project_manifest(&store, project.id);
                let mut out = serde_json::json!({
                    "created": true,
                    "project": project,
                    "directory": dir_path.to_string_lossy(),
                });
                // F5 件①：建项目警示——项目层永远没有交付文本（收口评审
                // 的锚点核验对象是汇总摘要），AC 写 `[CHECK] re:` 锚点在
                // 项目层不可自动核验；应下放到具体任务单。
                if nemesis_board::has_content_regex_anchor(
                    acceptance_criteria.as_deref().unwrap_or(""),
                ) {
                    out["warning"] = serde_json::json!(
                        "项目验收标准含 [CHECK] re: 锚点：项目层没有交付文本可核验（项目收口时该锚点必然失败）——请把锚点要求写到具体任务单的验收标准里，而不是项目上。"
                    );
                }
                if auto_start {
                    // F-U4-5：自动开工链终点是拆解发车（plan 链 A1 波），
                    // 急停中拒绝整段。项目照建已是事实——响应体照常返回，
                    // auto_start 字段注明拒绝（与非 cluster 降级路径同形），
                    // 不用 `?` 把已建项目信息吞成裸 Err。
                    if let Err(e) = refuse_dispatch_when_estopped(ctx) {
                        out["auto_start"] = serde_json::json!({ "error": e });
                        return Ok(Some(out));
                    }
                    // 自动开工链依赖集群派发（plan 链→节点执行），cluster
                    // feature 编译期裁掉时诚实降级：项目照建（已是事实），
                    // 注明未拆解，不报错不回滚——与 agent 未运行时的降级
                    // 语义一致。
                    #[cfg(feature = "cluster")]
                    {
                        let issue = spawn_project_auto_start(
                            &store,
                            ctx,
                            &actor,
                            &project.name,
                            &get_opt_str(&data, "description").unwrap_or_default(),
                            acceptance_criteria,
                            project.id,
                        )?;
                        out["auto_start"] = serde_json::json!({
                            "issue_id": issue.id,
                            "issue_number": issue.number,
                        });
                    }
                    #[cfg(not(feature = "cluster"))]
                    {
                        let _ = acceptance_criteria; // no-cluster 路径不消费
                        out["auto_start"] = serde_json::json!({
                            "skipped": "cluster 编译期裁剪未启用，无法自动拆解派发；项目已创建，可手动拆解或重新启用 cluster"
                        });
                    }
                }
                Ok(Some(out))
            }
            // 项目字段级更新（W2 P3）：归档走 status="archived"（软删除）。
            "project.update" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let patch = ProjectPatch {
                    name: get_opt_str(&data, "name"),
                    description: get_opt_str(&data, "description"),
                    status: get_opt_str(&data, "status"),
                    icon: get_opt_str(&data, "icon"),
                    acceptance_criteria: get_opt_str(&data, "acceptance_criteria"),
                };
                let project = store.update_project(id, &patch)?;
                // C 里程碑 5（看板项目档案 goal P2）：状态/名称投影回
                // project.json（失败静默——投影可重建，不阻塞管理操作）。
                nemesis_board::archive_writer::sync_project_manifest(&store, id);
                // F9（看板项目档案 P6）：人工收口 → completed 落定即触发
                // AI 收口总结（异步；hook 未注册/生成失败均不阻塞管理操作）。
                if patch.status.as_deref() == Some("completed")
                    && project.status == "completed"
                    && let Some(hook) = PROJECT_SUMMARY_HOOK.get()
                {
                    hook(id);
                }
                Ok(Some(
                    serde_json::json!({ "updated": true, "project": project }),
                ))
            }
            // 看板项目档案 P6（F10）：打开档案目录。只放行 board 项目注册表
            // （board.db Project.directory——绑定不可变、无修改入口）已知路径，
            // 前端不可传任意路径；系统文件管理器打开（复用 projects 模块的
            // open_in_file_manager：CREATE_NO_WINDOW spawn 不等待）。
            "project.open_dir" => {
                let data = data.ok_or("missing data")?;
                let project_id = data
                    .get("project_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: project_id")?;
                let project = store.get_project(project_id)?;
                let dir = project.directory.as_deref().ok_or("项目未绑定档案目录")?;
                let path = std::path::PathBuf::from(dir);
                if !path.is_dir() {
                    return Err(format!("档案目录不存在: {}", path.display()));
                }
                crate::handlers::projects::open_in_file_manager(&path)?;
                Ok(Some(
                    serde_json::json!({ "opened": true, "directory": dir }),
                ))
            }
            // 附件上传（W2 P3）：base64 内容 → workspace/board/files/ 存文件
            // + 元数据入表。storage_path 记 workspace 相对路径（可移植）。
            "attachment.add" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let filename = sanitize_filename(&get_str(&data, "filename")?)?;
                let content_b64 = get_str(&data, "content")?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(content_b64.trim())
                    .map_err(|e| format!("content 不是合法 base64: {e}"))?;
                if bytes.len() > MAX_ATTACHMENT_BYTES {
                    return Err(format!(
                        "附件过大（{} 字节，上限 {} 字节）",
                        bytes.len(),
                        MAX_ATTACHMENT_BYTES
                    ));
                }
                // 先校验 issue 存在（比 FK 报错信息更友好），再落文件。
                store.get_issue(issue_id)?;
                let workspace = require_workspace(ctx)?;
                let files_dir = std::path::Path::new(workspace)
                    .join("board")
                    .join("files")
                    .join(format!("issue_{issue_id}"));
                std::fs::create_dir_all(&files_dir)
                    .map_err(|e| format!("创建附件目录失败: {e}"))?;
                // 毫秒时间戳前缀防同名覆盖。
                let stored_name = format!("{}_{}", chrono::Utc::now().timestamp_millis(), filename);
                std::fs::write(files_dir.join(&stored_name), &bytes)
                    .map_err(|e| format!("写入附件文件失败: {e}"))?;
                let rel_path = format!("board/files/issue_{issue_id}/{stored_name}");
                let attachment =
                    store.add_attachment(issue_id, &filename, &rel_path, bytes.len() as i64)?;
                Ok(Some(
                    serde_json::json!({ "added": true, "attachment": attachment }),
                ))
            }
            // 附件下载（W2 P3）：读文件回 base64（MVP 经 WS 传小文件；大文件
            // 走 HTTP 静态路由留 P4 评估）。
            "attachment.get" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let attachment = store.get_attachment(id)?;
                let workspace = require_workspace(ctx)?;
                let bytes =
                    std::fs::read(std::path::Path::new(workspace).join(&attachment.storage_path))
                        .map_err(|e| format!("读取附件文件失败: {e}"))?;
                let content = base64::engine::general_purpose::STANDARD.encode(&bytes);
                Ok(Some(
                    serde_json::json!({ "attachment": attachment, "content": content }),
                ))
            }
            // 收件箱（W2 P3）：站内通知列表（store 事件钩子产生；经通道的
            // 站外投递留 P4）。MVP 单管理员语义：admin 通知全员可见。
            "inbox.list" => {
                let data = data.unwrap_or(serde_json::Value::Null);
                let unread_only = data
                    .get("unread_only")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let limit = data
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100)
                    .clamp(1, 500) as usize;
                let notifications = store.list_notifications("admin", None, unread_only, limit)?;
                let unread = store.unread_notification_count("admin", None)?;
                Ok(Some(
                    serde_json::json!({ "notifications": notifications, "unread": unread }),
                ))
            }
            "inbox.mark_read" => {
                let data = data.ok_or("missing data")?;
                let marked: i64 = if data.get("all").and_then(|v| v.as_bool()).unwrap_or(false) {
                    store.mark_all_notifications_read("admin", None)? as i64
                } else {
                    let id = data
                        .get("id")
                        .and_then(|v| v.as_i64())
                        .ok_or("missing field: id（或传 all=true 全部已读）")?;
                    if store.mark_notification_read(id)? {
                        1
                    } else {
                        0
                    }
                };
                let unread = store.unread_notification_count("admin", None)?;
                Ok(Some(
                    serde_json::json!({ "marked": marked, "unread": unread }),
                ))
            }
            "attachment.list" => {
                let data = data.ok_or("missing data")?;
                let issue_id = data
                    .get("issue_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: issue_id")?;
                let attachments = store.list_attachments(issue_id)?;
                Ok(Some(serde_json::json!({ "attachments": attachments })))
            }
            // --- channel（Swarm M3 批次 E：讨论频道）---
            "channel.list" => {
                let channels = store.list_channels()?;
                Ok(Some(serde_json::json!({ "channels": channels })))
            }
            "channel.messages" => {
                let data = data.ok_or("missing data")?;
                let channel_id = data
                    .get("channel_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: channel_id")?;
                let after_id = data.get("after_id").and_then(|v| v.as_i64()).unwrap_or(0);
                let limit = data
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(200)
                    .clamp(1, 500) as i64;
                let messages = store.list_channel_messages(channel_id, after_id, limit)?;
                Ok(Some(serde_json::json!({ "messages": messages })))
            }
            "channel.post" => {
                let data = data.ok_or("missing data")?;
                // 发言走 master 讨论管线（幂等/额度三闸/裁决投递与 worker
                // 上行同源）；桥未装配 = 讨论总线不可用，诚实拒绝。
                let ingress = ctx
                    .state
                    .board
                    .as_ref()
                    .and_then(|svc| svc.discussion())
                    .ok_or("讨论总线未装配（需要 board+cluster feature 且 gateway 运行中）")?;
                let channel_id = data
                    .get("channel_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: channel_id")?;
                let content = get_str(&data, "content")?;
                if content.trim().is_empty() {
                    return Err("content must not be empty".to_string());
                }
                let reply_to = data.get("reply_to").and_then(|v| v.as_i64());
                let kind_tag = get_opt_str(&data, "kind_tag").unwrap_or_else(|| "text".to_string());
                let client_msg_id = format!("dash-{}", uuid::Uuid::new_v4());
                let posted = ingress.post(
                    &actor,
                    nemesis_board::models::thread_kind::CHANNEL,
                    channel_id,
                    &client_msg_id,
                    &content,
                    reply_to,
                    &kind_tag,
                )?;
                Ok(Some(serde_json::json!({ "posted": posted })))
            }
            "stats" => {
                let counts = store.count_by_status()?;
                let map: serde_json::Map<String, serde_json::Value> = counts
                    .into_iter()
                    .map(|(st, n)| (st.as_str().to_string(), serde_json::json!(n)))
                    .collect();
                Ok(Some(serde_json::json!({ "by_status": map })))
            }
            // --- audit（全自动流转 P5/E2：决策流视图）---
            "audit.list" => {
                let data = data.unwrap_or(serde_json::json!({}));
                let limit = data
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50)
                    .clamp(1, 500) as u32;
                // F-U6-1：offset 翻页——刷屏类记录挤占 limit 窗口时仍可翻到最新。
                let offset = data.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let action = data
                    .get("action")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty());
                let rows = store.list_recent_activity_paged(limit, offset, action)?;
                Ok(Some(serde_json::json!({ "decisions": rows })))
            }
            "audit.rollback" => {
                let data = data.ok_or("missing data")?;
                let activity_id = data
                    .get("activity_id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: activity_id")?;
                let issue = store.rollback_decision(activity_id)?;
                tracing::info!(
                    "[Board] audit.rollback activity={activity_id} issue={} → in_review",
                    issue.number
                );
                Ok(Some(
                    serde_json::json!({ "rolled_back": true, "issue": issue }),
                ))
            }
            // S-O1：合并停车人工重试（按 issue 扫档案树未合并 placement 重走合并）。
            "audit.retry_merge" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_i64())
                    .ok_or("missing field: id")?;
                let issue = store.get_issue(id)?;
                let hook = RETRY_MERGE_HOOK
                    .get()
                    .ok_or("合并重试钩子未安装（gateway 未装配档案合并依赖）")?;
                let result = hook(&issue)?;
                tracing::info!(
                    "[Board] audit.retry_merge issue={} → {}",
                    issue.number,
                    result
                );
                Ok(Some(result))
            }
            // M5-1（2026-10-10 机制加固）：评审重触发——卡 in_review 的单子
            // 人工一键重入验收评审。number（NB-N）优先、id 兜底；非
            // in_review 诚实拒绝（评审链内有竞态守卫，这里是入口闸）。
            "review.retrigger" => {
                let data = data.ok_or("missing data")?;
                let issue = if let Ok(number) = get_str(&data, "number") {
                    store.get_issue_by_number(&number)?
                } else if let Some(id) = data.get("id").and_then(|v| v.as_i64()) {
                    store.get_issue(id)?
                } else {
                    return Err("review.retrigger 需要 number（NB-N）或 id 之一".into());
                };
                if issue.status != IssueStatus::InReview {
                    return Err(format!(
                        "issue {} 当前状态为 {}，仅 in_review 单子可重触发评审",
                        issue.number,
                        issue.status.as_str()
                    ));
                }
                let hook = REVIEW_RETRIGGER_HOOK.get().ok_or(
                    "评审重触发钩子未安装（gateway 未装配 board 评审依赖或非 master 节点）",
                )?;
                hook(issue.id);
                // 留痕：人工触发记录进单据（决策流/审计可见）。
                tracing::info!(
                    "[Board] review.retrigger issue={} by {}/{}",
                    issue.number,
                    actor.kind,
                    actor.id
                );
                let _ = store.add_comment(NewComment {
                    issue_id: issue.id,
                    author: actor,
                    content: "🔁 人工触发重新验收评审（review.retrigger）".to_string(),
                    parent_id: None,
                    ctype: CommentType::System,
                });
                Ok(Some(
                    serde_json::json!({ "retriggered": true, "issue": issue.number }),
                ))
            }
            // U-B（2026-10-12 无人值守）：端点不可用封存观测 + 手动放行。
            // status = 状态机快照 + 待续做中断档案清单（不 load 内容）。
            "unavailable.status" => {
                let mut snap = super::board_unavailable::snapshot_json();
                if let Some(ws) = handoff_workspace_root().map(|p| p.as_path()) {
                    let pending: Vec<String> =
                        super::board_unavailable::read_pending_interruptions(ws)
                            .iter()
                            .filter_map(|d| {
                                d.get("issue")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            })
                            .collect();
                    snap["pending_interruptions"] = serde_json::json!(pending);
                }
                Ok(Some(snap))
            }
            "unavailable.release" => {
                let was = super::board_unavailable::manual_release();
                tracing::info!(
                    "[Board] board.unavailable.release by {}/{} (was_frozen={was})",
                    actor.kind,
                    actor.id
                );
                Ok(Some(
                    serde_json::json!({ "released": true, "was_frozen": was }),
                ))
            }
            // --- config（全自动流转 P1/A4：配置 TAB 读写 board 段旗标）---
            "config.get" => {
                let home = ctx.home.as_deref().ok_or("home 未解析")?;
                let cfg =
                    nemesis_config::load_config(&std::path::Path::new(home).join("config.json"))
                        .map_err(|e| format!("config.json 读取失败：{e}"))?;
                let board = cfg.board.unwrap_or_default();
                serde_json::to_value(&board)
                    .map(Some)
                    .map_err(|e| format!("board 段序列化失败：{e}"))
            }
            "config.set" => {
                let data = data.ok_or("missing data")?;
                let key = get_str(&data, "key")?;
                let value = data.get("value").ok_or("missing field: value")?;
                board_config_set(ctx, &key, value)
            }
            // --- 任务档案导出（2026-09-29 goal：统一整合任务全部记录）---
            "dossier.export" => {
                let data = data.ok_or("missing data")?;
                let store = require_board(ctx)?;
                let workspace = require_workspace(ctx)?;
                let out_root = std::path::Path::new(workspace)
                    .join("logs")
                    .join("dossiers");
                // 用量闭包：DataStore 在（gateway 常态）→ per-task 聚合；
                // 不在 → 全部 None（usage.csv 缺行诚实，README 总量按有账计）。
                let ds = ctx.state.data_store.clone();
                let usage_of = |task_id: &str| {
                    let ds = ds.as_ref()?;
                    ds.aggregate_session_usage_by_task(task_id).ok().map(|agg| {
                        nemesis_board::dossier::DossierUsage {
                            input_tokens: agg.input_tokens,
                            output_tokens: agg.output_tokens,
                        }
                    })
                };
                let outcome = if let Ok(number) = get_str(&data, "number") {
                    nemesis_board::dossier::export_issue_tree(
                        &store,
                        &number,
                        std::path::Path::new(workspace),
                        &out_root,
                        &usage_of,
                    )?
                } else if let Some(pid) = data.get("project_id").and_then(|v| v.as_i64()) {
                    nemesis_board::dossier::export_project(
                        &store,
                        pid,
                        std::path::Path::new(workspace),
                        &out_root,
                        &usage_of,
                    )?
                } else {
                    return Err("dossier.export 需要 number（NB-N）或 project_id 之一".into());
                };
                Ok(Some(serde_json::json!({
                    "root": outcome.root.display().to_string(),
                    "issues": outcome.issue_numbers,
                    "notes": outcome.notes,
                })))
            }
            _ => Err(format!("unknown command: board.{}", cmd)),
        }
    }
}

/// 全自动流转 P1（A4）：board 配置段白名单写入。typed 改字段 → 重序列化
/// round-trip 验证（与 `config.set_field` 同契约：字段没活下来就 loud 拒绝，
/// 不谎报 updated）→ live/盘双路保存。白名单外键拒绝（配置页不作为任意
/// 配置写入口）。保存后热生效：消费点（load_board_flags/plan 自动发车/
/// QuotaLedger 探针/sweep）均每次现读。
fn board_config_set(
    ctx: &RequestContext,
    key: &str,
    value: &serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let home = ctx.home.as_deref().ok_or("home 未解析")?;
    let path = std::path::Path::new(home).join("config.json");
    // live 缓存优先（与其他 handler 同一视图），无 live 时回落磁盘读取。
    let mut config = match nemesis_config::load_live() {
        Some(c) => c,
        None => {
            nemesis_config::load_config(&path).map_err(|e| format!("config.json 读取失败：{e}"))?
        }
    };
    let board = config.board.get_or_insert_with(Default::default);

    fn need_bool(v: &serde_json::Value) -> Result<bool, String> {
        v.as_bool().ok_or_else(|| "需要布尔值".to_string())
    }

    match key {
        "auto_review" => board.auto_review = need_bool(value)?,
        "auto_accept" => board.auto_accept = need_bool(value)?,
        "auto_close_parent" => board.auto_close_parent = need_bool(value)?,
        "unlimited_mode" => board.unlimited_mode = need_bool(value)?,
        // P5/F1 冲突漏斗全局开关（false=human 档：冻结转人工）。
        "conflict_auto_resolve" => board.conflict_auto_resolve = need_bool(value)?,
        // P34 重派决策量化（灰度默认关；true = D3 换节点重派叠加
        // worker × 任务类型成功率指纹三档权重）。
        "fingerprint_weighting" => board.fingerprint_weighting = need_bool(value)?,
        // 2026-09-29 三档化：off/role/full（默认 role=丢职能保角色）；
        // 旧布尔 false/true 兼容接收（→off/full），写盘统一为字符串档位。
        "dispatch_fallback" => {
            board.dispatch_fallback = serde_json::from_value(value.clone()).map_err(|e| {
                format!("dispatch_fallback 需要 \"off\"|\"role\"|\"full\"（或旧布尔）: {e}")
            })?;
        }
        // D0（goal P2）派发准入：单 worker 在途派发上限（非负整数；0=不限）。
        "worker_max_inflight" => {
            board.worker_max_inflight = value
                .as_i64()
                .filter(|v| *v >= 0)
                .ok_or("worker_max_inflight 需要非负整数")?;
        }
        "dispatch_fallback_target" => {
            board.dispatch_fallback_target = if value.is_null() {
                None
            } else {
                Some(
                    value
                        .as_str()
                        .ok_or("dispatch_fallback_target 需要字符串或 null")?
                        .trim()
                        .to_string(),
                )
            };
        }
        "review.selfcheck" => board.review.selfcheck = need_bool(value)?,
        // M7（2026-10-10）：exec 锚点开关 + 超时（默认关=诚实 skipped 注记，
        // 不进结果组；开=锚点命令在项目根执行，exit 0=过）。
        "review.exec_anchors_enabled" => board.review.exec_anchors_enabled = need_bool(value)?,
        "review.exec_anchor_timeout_secs" => {
            board.review.exec_anchor_timeout_secs = value
                .as_u64()
                .ok_or("review.exec_anchor_timeout_secs 需要非负整数")?;
        }
        "review.auto_close_project" => board.review.auto_close_project = need_bool(value)?,
        "plan.auto_confirm" => board.plan.auto_confirm = need_bool(value)?,
        // 任务档案（2026-09-29）：项目收口自动导出（默认开）。
        "dossier.auto_on_complete" => board.dossier.auto_on_complete = need_bool(value)?,
        "plan.model" => {
            board.plan.model = if value.is_null() {
                None
            } else {
                Some(
                    value
                        .as_str()
                        .ok_or("plan.model 需要字符串或 null")?
                        .to_string(),
                )
            };
        }
        "max_redispatch" => {
            board.max_redispatch =
                u32::try_from(value.as_u64().ok_or("max_redispatch 需要非负整数")?)
                    .map_err(|_| "max_redispatch 超出 u32 范围")?;
        }
        "review.max_turns" => {
            board.review.max_turns =
                u32::try_from(value.as_u64().ok_or("review.max_turns 需要非负整数")?)
                    .map_err(|_| "review.max_turns 超出 u32 范围")?;
        }
        "review.checkers" => {
            board.review.checkers =
                u32::try_from(value.as_u64().ok_or("review.checkers 需要非负整数")?)
                    .map_err(|_| "review.checkers 超出 u32 范围")?;
            // 消费侧（run_review_panel）同样收敛，这里前置拒绝给出明确文案。
            if board.review.checkers < 1 || board.review.checkers > 5 {
                return Err("review.checkers 需要在 1..=5 范围内".to_string());
            }
        }
        "budget.max_subissues_per_parent" => {
            board.budget.max_subissues_per_parent = u32::try_from(
                value
                    .as_u64()
                    .ok_or("budget.max_subissues_per_parent 需要非负整数")?,
            )
            .map_err(|_| "超出 u32 范围")?;
        }
        "budget.max_total_redispatch" => {
            board.budget.max_total_redispatch = u32::try_from(
                value
                    .as_u64()
                    .ok_or("budget.max_total_redispatch 需要非负整数")?,
            )
            .map_err(|_| "超出 u32 范围")?;
        }
        "budget.wall_clock_budget_secs" => {
            board.budget.wall_clock_budget_secs = value
                .as_u64()
                .ok_or("budget.wall_clock_budget_secs 需要非负整数")?;
        }
        "budget.max_tokens_per_parent" => {
            board.budget.max_tokens_per_parent = value
                .as_u64()
                .ok_or("budget.max_tokens_per_parent 需要非负整数")?;
        }
        "dispatch_timeout_secs" => {
            board.dispatch_timeout_secs =
                value.as_u64().ok_or("dispatch_timeout_secs 需要非负整数")?;
        }
        "discussion.retention_days" => {
            board.discussion.retention_days =
                value.as_i64().ok_or("discussion.retention_days 需要整数")?;
        }
        "discussion.max_agent_turns_per_thread" => {
            board.discussion.max_agent_turns_per_thread = u32::try_from(
                value
                    .as_u64()
                    .ok_or("discussion.max_agent_turns_per_thread 需要非负整数")?,
            )
            .map_err(|_| "超出 u32 范围")?;
        }
        "discussion.hourly_budget_per_node" => {
            board.discussion.hourly_budget_per_node = u32::try_from(
                value
                    .as_u64()
                    .ok_or("discussion.hourly_budget_per_node 需要非负整数")?,
            )
            .map_err(|_| "超出 u32 范围")?;
        }
        "discussion.rate_limit_per_min" => {
            board.discussion.rate_limit_per_min = u32::try_from(
                value
                    .as_u64()
                    .ok_or("discussion.rate_limit_per_min 需要非负整数")?,
            )
            .map_err(|_| "超出 u32 范围")?;
        }
        _ => {
            return Err(format!(
                "未知或不允许的 board 配置键：{key}（允许：auto_review / auto_accept / \
                 auto_close_parent / unlimited_mode / conflict_auto_resolve / dispatch_fallback / \
                 dispatch_fallback_target / fingerprint_weighting / \
                 max_redispatch / dispatch_timeout_secs / worker_max_inflight / \
                 plan.auto_confirm / plan.model / review.max_turns / review.selfcheck / \
                 review.exec_anchors_enabled / review.exec_anchor_timeout_secs / \
                 review.auto_close_project / review.checkers / budget.max_subissues_per_parent / \
                 budget.max_total_redispatch / budget.wall_clock_budget_secs /                  budget.max_tokens_per_parent / discussion.*）"
            ));
        }
    }

    // round-trip 验证：typed 重序列化后字段必须仍在且值一致（防 typed
    // 结构与键清单漂移时谎报 updated——config.set_field G6 契约同款）。
    let reserialized =
        serde_json::to_value(&config).map_err(|e| format!("config 重序列化失败：{e}"))?;
    let Some(board_json) = reserialized.get("board") else {
        return Err("board 段 round-trip 丢失（typed 结构漂移）".to_string());
    };
    let leaf = key.rsplit('.').next().unwrap_or(key);
    let persisted = match leaf {
        "plan" | "discussion" | "backup" | "review" | "budget" => board_json.get(leaf),
        _ => {
            // plan.x / discussion.x 嵌套键到子对象里核。
            if let Some((parent, _)) = key.split_once('.') {
                board_json.get(parent).and_then(|p| p.get(leaf))
            } else {
                board_json.get(leaf)
            }
        }
    };
    match persisted {
        None => return Err(format!("配置键 {key} round-trip 丢失（typed 结构漂移）")),
        // 值不一致且布尔/数值维度也不等价（排除 serde 归一化等价形态）才报错。
        Some(actual)
            if actual != value
                && actual.as_bool() != value.as_bool()
                && actual.as_u64() != value.as_u64() =>
        {
            return Err(format!(
                "配置键 {key} round-trip 不一致（值被拒绝或归一化）"
            ));
        }
        _ => {}
    }

    // M6-2 rebase（2026-10-10 机制加固，P5 clobber 根修）：上面读到的
    // live/磁盘快照与真正落盘之间，其他写者（CLI model add、别的 handler、
    // 手工编辑）可能已改过磁盘 config.json——整份回写会把那些非 board 改动
    // 静默回滚。落盘前以磁盘最新内容为底、只嫁接本次改过的 board 段；
    // 磁盘读失败则诚实放弃（不拿陈旧快照重建磁盘，fail-closed）。
    config = rebase_board_onto_disk(config, &path)?;

    if let Some(r) = nemesis_config::save_live(config.clone()) {
        r.map_err(|e| format!("failed to save config: {e}"))?;
    } else {
        nemesis_config::save_config(&path, &mut config)
            .map_err(|e| format!("failed to save config: {e}"))?;
    }
    tracing::info!("[Board] board.config.set key={key}");
    Ok(Some(serde_json::json!({ "updated": true, "key": key })))
}

/// M6-2 rebase（P5 clobber 根修）：以磁盘最新 config.json 为底、只保留
/// `mutated` 的 board 段，返回合并结果。磁盘读失败 = Err（调用方不落盘，
/// fail-closed——绝不拿陈旧快照重建磁盘）。
fn rebase_board_onto_disk(
    mutated: nemesis_config::Config,
    path: &std::path::Path,
) -> Result<nemesis_config::Config, String> {
    let mut disk_fresh = nemesis_config::load_config(path)
        .map_err(|e| format!("config.json 重读失败（rebase 放弃，磁盘未写）：{e}"))?;
    disk_fresh.board = mutated.board;
    Ok(disk_fresh)
}

#[cfg(test)]
mod tests;
