// 机械拆分：本文件内容由 board.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

/// issue → worker 任务提示词（`issue.dispatch` 的 peer_chat content）。
/// worker 端按普通任务 prompt 走自己的 agent（工具/安全层照常生效），
/// 结尾固定要求结果汇报——最终回复是唯一回传渠道（经 callback 写回看板）。
#[cfg(feature = "cluster")]
/// 汇报格式五段以 nemesis-board::report 为准（交付线程首评 / 验收
/// agent 同源解析；此处只引用）。
use nemesis_board::report::REPORT_FORMAT_SECTION;

/// 派发提示词（impl-plan 结构化 PRD 模板）：背景/验收/交付物要求/
/// 汇报格式五段。description 是自由文本（人写）或结构化分段（planner 写），
/// 原文放「背景」段不做二次加工。唯一调用方 dispatch_issue_core（cluster），
/// 同 cfg 门控（REPORT_FORMAT_SECTION 是 cluster-only const）。
#[cfg(feature = "cluster")]
/// `review_feedback`（Swarm）：FAIL 重派时验收 agent 的差距意见，渲染
/// 成「上轮验收意见」段——worker 必须知道为什么被打回，否则重派就是掷
/// 骰子（检视 2）。首派传 `None`。
/// `experience_section`（Swarm.5）：团队过往经验注入段（注入端已渲染），
/// 无匹配传 `None`——不塞空段，保持提示词字节与无经验时一致。
/// `attempt`（机制加固）：本次派发序号（1 起；首派 `None`
/// 不渲染，prompt 字节与历史一致）。重派任务卡告知轮次 + 循环升级契约：
/// worker 不知道自己是第几回挨打，就无法判断「同一问题反复重派无解」，
/// 只会静默再写一轮（无限重写循环的 worker 侧根因）。
pub(super) fn build_dispatch_prompt(
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
    // Swarm （检视 2）：重派上下文延续——上轮验收差距原文随包走。
    if let Some(feedback) = review_feedback
        && !feedback.trim().is_empty()
    {
        p.push_str(&format!(
            "\n## 上轮验收意见（本次重派原因，必须针对性整改）\n{}\n",
            feedback.trim()
        ));
    }
    // （机制加固）：重派轮次 + 循环升级通道（worker 侧编码）。
    // 只在重派时渲染（首派 prompt 字节与历史一致）。
    if let Some(n) = attempt {
        p.push_str(&format!(
            "\n## 重派轮次（第 {n} 次派发）\n本单此前已被派发 {} 次、验收未通过。若你判断同一问题按已有方法继续整改仍无法通过验收（连续多轮重派无解），不要再静默重写一轮：在最终回复正文里**单独一行**写行首标记 `REDISPATCH_LOOP:` 加一句话说明卡点，并在汇报的「风险与未尽事项」节列出已尝试方案清单（每项一行：试了什么、结果如何）。主控端识别该标记后会停止自动重派、转人工裁决——这是升级通道，不是失败污点；诚实升级好过无限重写。\n",
            n - 1
        ));
    }
    // Swarm （层 1）：issue 名下资产随包走（引用+签名 token；
    // 内容对端按需 HTTP 拉）。
    if let Some(section) = assets_section {
        p.push_str(section);
    }
    // Swarm.5：scope 匹配的团队经验随包走（经验段是参考，
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

/// 急停发车护栏（F-， 真机实证）：estop 生效中一切看板
/// 发车入口一律拒绝。与 `project.resume` 的「护栏三不变：estop 急停中
/// 拒绝恢复发车」同族——手动发车（issue.dispatch / issue.plan /
/// autopilot.run / project.create auto_start）都是发车面，此前唯独它们
/// 漏网（真机实证 estop ENGAGED 时 issue.dispatch 仍成功派出）。评审/
/// 清扫走保险丝挂起（board_review 检查点），语义不同不在此拦。
pub(super) fn refuse_dispatch_when_estopped(ctx: &RequestContext) -> Result<(), String> {
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
pub(super) async fn issue_dispatch(
    store: &Arc<BoardStore>,
    actor: Actor,
    ctx: &RequestContext,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    // F-：急停中拒绝发车（护栏，最便宜也最根本的一闸先行）。
    refuse_dispatch_when_estopped(ctx)?;
    let data = data.ok_or("missing data")?;
    let id = data
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or("missing field: id")?;
    let issue = store.get_issue(id)?;

    // 目标解析：显式 target 优先，否则取 worker 指派；manager_self 由
    // coordinator 本机执行（autopilot），不支持远端派发。
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
    // D：手动重派 = 人类明确意图 → 失败账清零（「人工处理后计数作废」
    // 在手动路径成真；熔断停车后 reopen 重派不再因存量 count 秒熔断）。与评
    // 审链同源同一 JSON（以 nemesis-board fail_streak 为准）；清账失败不
    // 阻断派发结果（护栏非主路径）。workspace 未注入 = 跳过（测试形态）。
    if let Some(ws) = ctx.state.workspace.as_deref() {
        nemesis_board::fail_streak::FailStreakLedger::load(std::path::Path::new(ws)).clear(id);
    }
    Ok(Some(out))
}

/// dispatch prompt 的「## 任务资产」段渲染（Swarm 层 1）。签发
/// 上下文挂 store（gateway 装配 set 一次）——未注入 / issue 无资产逐级
/// 诚实降级为 None。
#[cfg(feature = "cluster")]
pub(super) fn render_dispatch_assets_section(
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

/// 派发经验注入的检索（.5）：查库（滤 deprecated）→ scope 对
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

/// 派发硬闸（双端真机 拓扑误杀根修）：远端目标 +
/// `file:` 锚点 = 拒绝派发。`file:` 锚点在派发端（看板权威端）workspace
/// 解析实核，而任务在远端节点 workspace 执行——执行者真实交付成功也会被
/// 假 FAIL（实证： 烧光 2 轮重派预算转人工，B 端交付物早已存在）。
/// 本闸是**模型无关**的确定性拦截（planner 提示词拓扑纪律是软防线，这是
/// 硬防线；手动派发 / FAIL 重派 / autopilot 全走 dispatch_issue_core =
/// 单一漏斗全覆盖）。返回 Some(拒绝理由) = 拦截。
/// 纯函数（无 store/cluster 依赖），单测直测。
#[cfg(feature = "cluster")]
pub(super) fn reject_remote_file_anchors(
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

// /：基线下发（看板项目档案 goal 合并批）
//
// BaselinePusher 走模块级 OnceLock 而非 AppState 字段（PARENT_REVIEW_HOOK
// 同款理由：AppState 全库字面构造测试点太多，加字段是断点级改动）。
// gateway 装配时注册；未注册（单测/极简装配）= 派发走既有无基线管线，
// 存量项目（无 directory）同样不参与——两条边界与 goal §/ 一致。

/// 派发基线推送器：既有分块传输通路（transfer.begin/chunk/end，AEAD 鉴权）
/// + 护栏现读 provider（gateway 30s 热刷新循环注入的 cell）。
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

// EST-01/02（横扫加固）：看板派发族的 estop 闸。
// estop 生效时全部自动/手动派发诚实冻结，而不是照常发车（此前 board 侧
// 完全不感知 estop——急停只冻结 agent loop，看板 cron/补派/停车场 sweep
// 会继续向 worker 派新任务）。同 BASELINE_PUSHER 理由：AppState 全库字面
// 构造测试点太多，加字段是断点级改动，故走模块级 OnceLock 注入。gateway
// 装配时注册；未注册（单测/极简装配）= 闸不生效（等价旧行为）。

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
pub(super) fn board_estop_engaged() -> bool {
    estop_dispatch_frozen(BOARD_ESTOP.get().map(|e| e.as_ref()))
}

// U-A（停车现场工程）：派发/拆解侧停车档案的工作区根。
// handoff 落盘需要 `<workspace>/board/handoffs/`，而派发/拆解族全是
// store+cluster 纯函数（AppState 字面量测试点太多，加参数是断点级改动）
// 同 BASELINE_PUSHER/BOARD_ESTOP 理由走模块级 OnceLock 注入，gateway
// 装配时注册；未装配（单测/极简装配）= 停车档案缺席但评论照常（诚实降级）。

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
/// 调用方全在 cluster-gated 函数内——default-feature 单独编译时放行死代码。
#[cfg_attr(not(feature = "cluster"), allow(dead_code))]
pub(super) fn write_side_handoff(
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

/// 恢复类动作成功后的停车档案归档（决策可见性-15）：移入
/// `handoffs/archive/` 保留追溯、清空「在等人」集合（issue.list 的
/// needs_user 徽标随之熄灭）。best-effort：失败只 WARN 不阻塞动作——档案
/// 残留只影响徽标时效，status 闸（done/cancelled/backlog 不亮牌）兜底正确性。
pub(super) fn archive_handoff_best_effort(issue_number: &str) {
    if let Some(ws) = handoff_workspace_root()
        && let Err(e) = nemesis_board::handoff::archive_handoff(ws, issue_number)
    {
        tracing::warn!("[Board] {} 停车档案归档失败（动作照常）：{e}", issue_number);
    }
}

/// 活跃停车档案扫描（issue.list needs_user 投影 / issue.decision 共用）：
/// handoffs 目录一次性读（每单一文件小 JSON、最新停车覆盖写，条目数 ≤
/// 停车单数），返回 issue_number → 档案 JSON。`archive/` 子目录不扫（归档
/// 即「不在等人」）；未装配工作区根 / 目录缺席 / 单文件损坏 = 跳过（诚实
/// 无信号，不报错——投影面不为脏文件炸列表）。read_dir 结果不按序消费
/// （按 issue_number 建 map，每单至多一文件），无需排序。
pub(super) fn scan_live_handoffs() -> std::collections::HashMap<String, serde_json::Value> {
    match handoff_workspace_root() {
        Some(ws) => scan_live_handoffs_in(ws),
        None => Default::default(),
    }
}

/// [`scan_live_handoffs`] 的可测内核（HANDOFF_WORKSPACE 是进程级
/// OnceLock——判定逻辑走纯函数，全局接线由 gateway 装配保证，同
/// estop_dispatch_frozen「单测不装配零波及」先例）。
pub(super) fn scan_live_handoffs_in(
    workspace: &std::path::Path,
) -> std::collections::HashMap<String, serde_json::Value> {
    let Ok(entries) = std::fs::read_dir(workspace.join("board").join("handoffs")) else {
        return Default::default();
    };
    let mut out = std::collections::HashMap::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        if let Some(number) = v.get("issue").and_then(|x| x.as_str()) {
            out.insert(number.to_string(), v);
        }
    }
    out
}
/// estop 闸判定可测内核（BOARD_ESTOP 是进程级 OnceLock——engaged 态若在
/// 单测内联装配会与并行测试的派发路径互踩，故判定逻辑走纯函数，全局接线
/// 仅一行由 gateway 装配保证，同 BASELINE_PUSHER「单测不装配零波及」先例）。
#[cfg(feature = "cluster")]
pub(super) fn estop_dispatch_frozen(estop: Option<&nemesis_agent::estop::EstopState>) -> bool {
    estop.is_some_and(|e| e.is_engaged())
}

/// estop 冻结拒绝文案（EST 族统一出口）。
#[cfg(feature = "cluster")]
pub(super) fn estop_frozen_error() -> String {
    "⛔ 急停（estop）生效中，派发已冻结：`estop --release` 释放后恢复（定时/补派触发器会自动重派）"
        .to_string()
}

/// 派发基线下发。项目档案仓库幂等 ensure → HEAD 树导出 staging →
/// 分块推给 worker（对端收件箱 `<workspace>/cluster/inbox/<task_id>/`）。
/// 返回 `Ok(Some(commit))` = 档案管线派发（基线已落地并记账）；
/// `Ok(None)` = 非档案管线（推送器未装配 / 单未绑项目 / 存量项目无
/// directory），走既有无基线路径；`Err` = 诚实弃派（调用方清理）。
///
/// **版本混布停车**（goal 边界②）：旧 worker 无 transfer handler，begin
/// RPC 得 "no handler" error 帧 → 本函数 Err → 派发诚实停车，提示升级。
///
/// 同步桥：dispatch_issue_core 是 sync（fire_autopilot/auto_dispatch_* /
/// dispatch_subissue_auto 同步链直通 cron，async 化是断点级改动），推送
/// 用 block_in_place 桥（adapters 先例；仅推送器已装配的生产路径到达，
/// 单测不装配零波及）。
#[cfg(feature = "cluster")]
pub(super) fn push_dispatch_baseline(
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
    // 幂等 ensure（部署后首次派发补 init + 对现有内容首 commit）。
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
        nemesis_utils::fsutil::remove_dir_all_resilient(&staging);
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
    nemesis_utils::fsutil::remove_dir_all_resilient(&staging);
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
/// 触发共用。`target="self"` 在入口即委托本地执行面
/// [`dispatch_issue_local`]（BUG 单点下沉，全部调用方生效）。
/// `cluster` 传 `None` 时本地闸先行、集群缺失
/// 最后报（校验矩阵不依赖集群实例，可单测）。`review_feedback`（Swarm
/// ）为 FAIL 重派专用——验收差距渲染进 dispatch prompt「上轮验收意见」
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
    // target="self" 单机本地执行面（一等公民，BUG 单点下沉）：
    // "self" 不是集群概念（canonical_peer_id 必然查无此节点），恒委托
    // [`dispatch_issue_local`]（自带五闸镜像，estop/冲突冻结/状态/查重/
    // 计划全部重查）。分臂放在入口最前 = WSAPI 手动派发 / conflict
    // resume 重派 / 送达失败 sweep 重试 / auto_dispatch（本地单
    // assignee_id="self"）/ autopilot / plan 依赖波 / board_bus 认领等
    // 全部调用方一处语义生效，不再逐调用方补分臂。集群未装配（None）
    // 也照常本地执行。
    if target.eq_ignore_ascii_case("self") {
        return dispatch_issue_local(store, issue_id, actor, review_feedback);
    }
    // EST-01/02（派发单一入口 = 唯一落点）：estop 生效中一律诚实拒绝——
    // 释放后既有触发器（autopilot cron / 补派 / 停车场 sweep / 评审 fuse）
    // 自然覆盖恢复，无新增停车机制。
    if board_estop_engaged() {
        return Err(estop_frozen_error());
    }
    let issue = store.get_issue(issue_id)?;

    // / 冲突冻结闸（派发单一入口 = 唯一落点）：项目冲突冻结中一律
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

    // BUG-②（事故）：挂有未确认计划的父单不可直接派发
    // 子单只在 confirm_plan 时刻物化，直接派发 = 父单本体当单任务
    // 派出（绕过拆解）。running（拆解进行中）同拦——等确认或等失败，
    // 不与拆解链赛跑。过期/失败计划不拦（TTL 惰性清理与 plan.list 同
    // 判据；confirm 成功即摘除，都不在缓存里）。
    #[cfg(feature = "cluster")]
    {
        let now = std::time::Instant::now();
        let running = RUNNING_PLANS
            .lock()
            .values()
            .any(|r| r.issue_id == issue.id);
        let ready_ids: Vec<String> = {
            let mut cache = PLAN_CACHE.lock();
            cache.retain(|_, p| now.duration_since(p.created_at) <= PLAN_TTL);
            cache
                .iter()
                .filter(|(_, p)| p.issue_id == issue.id)
                .map(|(id, _)| id.clone())
                .collect()
        };
        if running || !ready_ids.is_empty() {
            let state_word = if running {
                "拆解进行中"
            } else {
                "待确认"
            };
            let id_hint = ready_ids
                .first()
                .map(|id| format!("（plan {id}）"))
                .unwrap_or_default();
            return Err(format!(
                "该单挂有未确认的拆解计划（{state_word}{id_hint}）：到任务详情确认该计划，或等其过期（{}s）后重试",
                PLAN_TTL.as_secs()
            ));
        }
    }

    // 派发 = 给远端 worker 发 peer_chat，集群必需。
    let cluster = cluster.ok_or("集群未运行，无法派发（issue.dispatch 需要集群）")?;

    // RPC client 前置检查（根修）：此前检查在 try_claim_dispatch /
    // 状态推进 / 指派回填之后——「集群在册但 RPC client 槽位未填」的启动窗口
    // 内派发会 Err 上抛且不清理，留下幽灵 dispatched 行 + in_progress 卡死
    // （该窗口内 sweep 尚未 armed，需人工恢复）。前置到一切状态变更之前：
    // 无 client = 无副作用诚实拒绝。
    if cluster.rpc_client_arc().is_none() {
        return Err("集群 RPC client 未就绪（启动窗口），请稍后重试派发".to_string());
    }

    // 身份口径统一（双身份失配第三处落点）：账本 worker_id
    // 必须与 worker 上报的传输层身份（_rpc.from = 运行时节点 id）同形态
    // 人工指派/兜底可能给 peer 名（"Alex"），不归一化则 task.started /
    // delivery.files 的 worker 校验永远失配。matcher 产出已是节点 id，此
    // 步幂等。
    // （派发目标校验前置）：解析失败 = 注册表无此节点，直接
    // 拒绝——旧行为放行原名去 RPC 撞墙，API 层却返回 dispatched=true（行为
    // 分裂，真机幽灵 target 实证）。归一化（含大小写回落）见
    // Cluster::canonical_peer_id。
    let target = cluster
        .canonical_peer_id(target)
        .ok_or_else(|| format!("未知节点：{target}（不在集群注册表中，请核对节点名或 id）"))?;

    // 拓扑硬闸（模型无关）：远端目标 + file: 锚点 = 拒绝（软防线是
    // planner 提示词拓扑纪律；手动派发/重派/autopilot 都过这道闸）。
    if let Some(reason) = reject_remote_file_anchors(
        issue.acceptance_criteria.as_deref(),
        &target,
        cluster.node_id(),
        &cluster.node_name(),
    ) {
        return Err(reason);
    }

    // .5：经验注入段在 prompt 组装前检索（命中计数待发车成功后落）。
    let (experience_section, experience_ids) = render_issue_experience_section(store, &issue);

    // （机制加固）：重派轮次 = 历史（已完结）派发行数 + 1。
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
    // 职能 slug 随派发载荷下行（顶层键；旧 B 端忽略未知键 = 优雅
    // 降级）。A 端只传 slug，契约文本由 B 端本地渲染（跨 RPC 不传文本）。
    let mut chat_payload = serde_json::json!({ "content": prompt, "_source": source_payload });
    if let Some(p) = issue.required_profession.as_deref().map(str::trim)
        && !p.is_empty()
    {
        chat_payload["required_profession"] = serde_json::json!(p);
    }
    let task_id =
        cluster.submit_peer_chat(&target, "peer_chat", chat_payload, "board", &chat_id)?;

    // 派发账本开账（确定性 id=board_dispatch:{task_id}；账本未安装=
    // 零变化，best-effort 不阻断派发）。收口点：写回终态（nemesisbot
    // board_dispatch.rs）、本函数三处派发中止、RPC 送达失败分支、超时
    // sweep（autopilot.rs）。
    nemesis_taskstore::ledger::begin_run(
        &nemesis_taskstore::ledger::board_dispatch_run_id(&task_id),
        "board_dispatch",
        &issue.title,
        &target,
        serde_json::json!({ "issue_id": issue.id, "issue_number": issue.number }),
    );

    // .5：注入计数只认实际发车的派发（提交失败不计 use_count）。
    if !experience_ids.is_empty()
        && let Err(e) = store.mark_team_memory_used(&experience_ids)
    {
        tracing::warn!("[Board] team_memory use_count 更新失败（不影响派发）：{e}");
    }

    // /（看板项目档案 goal 合并批）：项目档案基线下发。推送器未装配 /
    // 单未绑项目 / 存量项目无 directory = Ok(None)（走既有无基线路径）；
    // Err = **诚实弃派**——此刻 insert_dispatch 还没执行，fail_task 清理
    // 本地 task + 系统评论留痕即可，不留悬挂 dispatched 态（版本混布时旧
    // worker 无 transfer handler，在这里免费获得停车）。
    let baseline_commit = match push_dispatch_baseline(store, &issue, &task_id, &target) {
        Ok(v) => v,
        Err(e) => {
            cluster.fail_task(&task_id, &e);
            // 派发中止收账（诚实失败，可重派=新 task 新账行）。
            nemesis_taskstore::ledger::settle_run(
                &nemesis_taskstore::ledger::board_dispatch_run_id(&task_id),
                "failed",
                Some(&e),
            );
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
    //    原子占用闸（BUG-2 根修）：claim 返回 None = 并发路径（依赖闸
    //    补派 × 停车场 sweep 等）已占用该 issue → 本次 submit 的 task 诚实
    //    取消，不写行、不留幽灵 dispatched（幽灵行会永久占满 worker
    //    inflight，后续派发全部静默 deferred）。Some(generation) = 占用
    //    成功，代际号随下行 payload 带给 worker、写回时
    //    原样回声（写回闸据此拒绝旧代迟到回调覆盖新一代派发）。
    let dispatch_generation = match store.try_claim_dispatch(&task_id, issue.id, &target, actor)? {
        Some(generation) => generation,
        None => {
            cluster.fail_task(&task_id, "并发派发竞态：该 issue 已有进行中的派发");
            // 并发竞态取消收账。
            nemesis_taskstore::ledger::settle_run(
                &nemesis_taskstore::ledger::board_dispatch_run_id(&task_id),
                "cancelled",
                Some("并发派发竞态：该 issue 已有进行中的派发"),
            );
            return Err("该 issue 已有进行中的派发（并发派发竞态，本次派发已取消）".to_string());
        }
    };

    // C 里程碑 2（看板项目档案 goal）：派发落定 → records/NB-xx/
    // dispatch.md + timeline（起第 5 参填基线 commit——执行档案 merge
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
    //    幽灵 dispatched（BUG-2 对称彻底）。
    //    派发前快照（状态+指派）随 RPC 闭包下行——送达
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
                // 状态转移失败收账。
                nemesis_taskstore::ledger::settle_run(
                    &nemesis_taskstore::ledger::board_dispatch_run_id(&task_id),
                    "failed",
                    Some(&format!("状态转移失败：{e}")),
                );
                return Err(format!("派发中止：状态转移失败：{e}"));
            }
        }
    } else {
        issue
    };
    // 项目状态联动：项目绑定的单实际派出 → 项目 active → in_progress
    // （首派语义；已在 in_progress/completed 的项目不回退不重复触发）。
    link_project_on_dispatch(store, issue.project_id);

    // 派发即指派：派发目标落 assignee 字段。显式 target /
    // 匹配器 / 兜底 / 换节点重派四路都汇经本单一入口——此前派发只路由
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
    //    （callback 不会再来，不留悬挂 dispatched 态）。（RPC client 槽位
    //    已在函数头前置检查；此处 unwrap 失败理论上不可达，防御性保留。）
    let issue_id = issue.id;
    let rpc_client = cluster.rpc_client_arc().ok_or("RPC client not available")?;
    // 档案管线派发带基线 commit（worker 侧以收件箱基线为起点开跑）。
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
    // 派发代际号下行（worker 写回原样回声；旧 B 端不认识
    // 该键 = 回声缺席 = 写回闸放行，向后兼容）。
    payload["dispatch_generation"] = serde_json::json!(dispatch_generation);
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
        // 统一失败收尾：传输失败与 B 端拒单（BUG-1，——ACK
        // status=error = 工作队列满，任务从未开始）同走 路径
        // （finish FAILED + 回退 + ⛔ 留痕，sweep 按 MARK 前缀识别重试候选）。
        let failure: Option<(&'static str, String)> = match rpc_client
            .call_with_timeout(&target_for_rpc, request, timeout)
            .await
        {
            Ok(resp) => {
                if nemesis_cluster::rpc::peer_chat_handler::peer_chat_ack_rejected(
                    resp.result.as_ref(),
                ) {
                    // b：ACK error 字段（B 端 mandate 拒绝原因等）优先
                    // 透传——缺失回落队列满文案（旧 B 端 wire 无 error 键）。
                    let ack_reason = resp
                        .result
                        .as_ref()
                        .and_then(|r| r.get("error"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| {
                            "B 端工作队列满，任务未被接受（ACK status=error）".into()
                        });
                    Some(("worker 拒单", ack_reason))
                } else {
                    tracing::info!("[Board] dispatch RPC ACK received (task_id={task_id_for_rpc})");
                    None
                }
            }
            Err(e) => Some((e.failure_category(), e.to_string())),
        };
        if let Some((category, detail)) = failure {
            tracing::warn!(
                "[Board] dispatch send failed (task_id={task_id_for_rpc}): [{category}] {detail}"
            );
            // 送达失败收账（此分支不落 fail_task——本地 task 由 stale
            // 安全网兜底回收，账本面这里直接诚实收口）。
            nemesis_taskstore::ledger::settle_run(
                &nemesis_taskstore::ledger::board_dispatch_run_id(&task_id_for_rpc),
                "failed",
                Some(&format!("[{category}] {detail}")),
            );
            let _ = store_for_rpc.finish_dispatch(
                &task_id_for_rpc,
                nemesis_board::models::dispatch_state::FAILED,
            );
            // 送达失败 = 任务从未开始 → 状态回退到
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
                    // 前缀走统一常量（sweep 重试候选按前缀识别，
                    // 字面量漂移 = 重试静默失效）。
                    "{}: [{}] RPC 送达失败（{detail}）",
                    nemesis_board::SEND_FAILURE_MARK,
                    category
                ),
                parent_id: None,
                ctype: nemesis_board::CommentType::System,
            });
        }
    });

    // 派发成功 = 单据回到执行轨道（恢复动作之一）：归档停车档案
    // （重派场景的旧「待决策」徽标随之熄灭）。
    archive_handoff_best_effort(&issue.number);
    Ok(serde_json::json!({
        "dispatched": true,
        "task_id": task_id,
        "issue": issue_to_view(store, &issue)?,
    }))
}

/// `issue.dispatch`（未编译 cluster）：派发即发集群 RPC，无从谈起。
#[cfg(not(feature = "cluster"))]
pub(super) async fn issue_dispatch(
    _store: &Arc<BoardStore>,
    _actor: Actor,
    _ctx: &RequestContext,
    _data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    Err("issue.dispatch 需要集群支持（cluster feature 未编译）".to_string())
}
/// 项目状态联动：项目绑定的单实际派出 → 项目 `active → in_progress`
/// （首派语义）。项目不在 active（已 in_progress/completed/archived）或
/// 绑定缺失 → 不动。联动是派发的伴生效果——失败只 WARN 不回滚派发。
#[cfg(feature = "cluster")]
pub(super) fn link_project_on_dispatch(store: &Arc<BoardStore>, project_id: Option<i64>) {
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

/// 三路 sweep 触发源（announce 回调 / 派发落定重估波 / 周期兜底 ticker，
/// F）的进程级串行锁：sweep 体是 check-then-dispatch 序列，两个
/// wave 并发时可同时通过 `has_active_dispatch` 闸对同一单双派。同步锁
/// 即可——sweep 体全同步（SQLite + fire-and-forget RPC spawn），常态
/// 持锁毫秒级；**例外**：档案基线转移（write_side_handoff 内 block_in_place
/// 打包）可在持锁下跑秒级~分钟级，此窗口内其余触发源排队等待（正确性
/// 优先，档案导出另有独立线程，不放大此锁）；poison 恢复沿用
/// `unwrap_or_else(into_inner)` 惯例。
#[cfg(feature = "cluster")]
static PARK_SWEEP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// 准入停车评论标记（与 PARK_NOTICE_MARK 分开：原因不同、去重不互斥）。
/// pub：gateway sweep 监察环（nemesisbot）自治重派前置闸（B37 人优先
/// 槽位）同款去重标记——两处共用一个标记，互见即去重（同一「目标满载」
/// 事实不双写评论），跨 crate。
pub const INFLIGHT_NOTICE_MARK: &str = "在途派发已达上限";

/// 准入判定（纯函数，单测直测）：目标 worker 在途派发数达到上限（上限
/// 大于 0 时）= 满，自动派发不派、留 backlog 等承接。pub：gateway sweep
/// 监察环（nemesisbot）自治重派闸同源消费（B37 人优先槽位），跨 crate。
pub fn inflight_full(
    load: &std::collections::HashMap<String, usize>,
    target: &str,
    cap: i64,
) -> bool {
    cap > 0 && load.get(target).copied().unwrap_or(0) as i64 >= cap
}

/// 子单自动派发（依赖闸 + 匹配器选节点；confirm 派发波、补派触发器与
/// 停车场 sweep 共用的统一定义）。返回：`Ok(Some(out))` 已派出；
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
    // 父单存活闸（F）：父单已取消 = 任务线已死，本单不再派出——
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

    // R-9（goal）touch_paths 互斥：本单声明的写路径与「同父在途单」的
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
                    // （goal）：匹配失败明细——要求 vs 每个在线候选差什么
                    // （与 rank_peers 同语义：role 精确、tags 交集非空）。
                    // 候选取 matcher_candidates 单一入口（BUG-20261115-001：
                    // 明细与决策面同源，含授予标签合并）。
                    let candidates = matcher_candidates(store, cluster);
                    let detail = match_failure_detail(&issue, &candidates); // 兜底开关（集群完备性）：无人匹配但任务必须
                    // 做下去——按配置兜底派给在线节点（派发前 ⚠ 评论留痕；
                    // 与 ⏸ 停车评论是不同语义，不受 去重约束）。
                    if let Some((fb_target, reason)) =
                        pick_fallback_target(board_cfg, store, cluster, &issue)
                    {
                        // E（goal）：标签授予——把 relaxed 掉的缺失标签
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
                        // 不落评论不动父单。：评论附匹配明细（差什么一眼可）。
                        if notify_park {
                            // 去重：最后一条 system 评论已是暂缓说明 → 不重复落。
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
                                        "⏸ {PARK_NOTICE_MARK}：当前在线节点无可匹配（角色/标签）节点；节点上线或要求调整后系统会自动重试，也可手动指派节点后派发。匹配明细：{detail}"
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
                            // 可见性：整条链都在等节点 → 父单转 blocked 显形。
                            mark_parent_blocked_if_stalled(store, &issue, actor);
                        }
                        return Ok(None);
                    }
                }
            }
        }
    };

    // 身份口径统一：target 归一化为节点 id（与账本 worker_id / worker 上报
    // 身份 _rpc.from 同形态； 负载表键随之统一，防同一 worker 名字/id
    // 双键分裂计数）。 双身份失配第三处落点。
    let target = cluster
        .and_then(|c| c.canonical_peer_id(&target))
        .unwrap_or(target);

    // （goal）派发准入控制：目标 worker 在途派发数达上限 → 本单不派、
    // 留 backlog——新节点上线后 sweep 自然承接（防「同波多就绪单全挤单
    // worker、后上线的设备接不到活」）。cap=0 = 不限（旧行为）。 同款
    // 去重（独立 INFLIGHT_NOTICE_MARK）+ 父单受阻联动。
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
    //（blocked 来自 停车联动，状态机 blocked→in_progress 合法）。
    if let Some(pid) = issue.parent_issue_id
        && let Ok(parent) = store.get_issue(pid)
        && matches!(
            parent.status,
            IssueStatus::Backlog | IssueStatus::Todo | IssueStatus::Blocked
        )
    {
        // 父单联动 best-effort：并发双派窗口内两路都读到 Todo、都过闸，
        // 第二个 transition（InProgress→InProgress）被状态机拒绝——此时
        // 本单派发已成功，联动失败不得用 `?` 反噬整个派发结果（否则
        // 调用方按派发失败重试=同单双派风险）。降级 warn 留痕。
        if let Err(e) = store.transition_issue(pid, IssueStatus::InProgress, actor) {
            tracing::warn!(
                "[Board] 父单联动转 in_progress 失败（并发窗口/状态机拒绝）parent={pid} issue={} err={e}",
                issue.id
            );
        }
    }
    Ok(Some(out))
}

/// 停车场父单受阻联动：停车单的父单尚未开工（backlog/todo）且
/// 所有兄弟子单都没派出过（无在途派发）→ 父单转 blocked + 系统评论。
/// 「整条链都在等节点」在看板受阻列显形，而不是停在 backlog 假装没发生
/// （生产实测：用户建任务后"然后就没了"）。后续任一子单
/// 派出时由派发侧父单联动（bump 条件含 blocked）推回 in_progress。
#[cfg(feature = "cluster")]
pub(super) fn mark_parent_blocked_if_stalled(
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

/// 停车场 sweep：对「待派态 + planner 来源或曾被自动派发暂缓」的
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

/// 周期兜底 ticker（F）专用入口：`notify_park=true`——首次停车落
/// ⏸ 评论 + 父单 blocked 显形（与直派路径同语义； 去重保证重复 tick
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
    // EST-02（横扫加固）：estop 生效中 sweep 短路——派发核心
    // 的 estop 闸本会逐单拒绝，这里提前收敛，避免 20s 周期对全部停车候选
    // 的无效报错 churn。release 后（resume watcher / 定时器）自然恢复。
    if board_estop_engaged() {
        return (0, 0, 0);
    }
    // U-B：LLM 端点不可用封存中——派发重试 sweep 短路（worker 与
    // master 共享端点，发车必然烧在不可用端点上；恢复后 sweep 自然续跑，
    // 停车候选语义不变）。
    if super::super::board_unavailable::frozen_now() {
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
    // 送达失败自动重试——待派态或 in_review 且最后一条
    // system 评论是 ⛔ 送达失败标记且最新派发行 state=failed 的单进入重试
    // 池（评论 + 派发行账本双闸，gateway 重启不丢退避进度）。池含 in_review：
    // 评审 FAIL 自动重派绕过 backlog/todo 状态闸，issue 带 in_review 直进
    // dispatch_issue_core，送达失败回滚也回 in_review——只捞 backlog/todo
    // 会漏掉该形态（复检批补： 原始事故形状）。账本闸（多角度审计批）：
    // 成功交付只写 Delivery/Comment 评论不写 system 评论——无账本闸则旧 ⛔
    // 标记仍是最后一条 system 评论，交付后 in_review 立即重新入池，评审进行
    // 中被重派（乒乓循环）。
    // 闸序：重试预算（board.budget.max_send_retries 默认 5；超限 → 🛑 转人工
    // 评论，落盘即出池）→ 退避（1m→2m→4m…封顶 30m，从末次失败时刻起算）→
    // 执行器按状态分流：backlog/todo 走 dispatch_subissue_auto（完整松弛
    // 阶梯+停车路径）；in_review 走 dispatch_issue_core 直派（重派目标=末次
    // worker 在线则原地重试，否则在线候选首选；无可用节点——file: 锚点拓扑
    // 死锁 ⏸ 出池转人工，全员离线 debug 留池）。上一轮 park 候选已先跑完
    // （顺序执行）：派出的单已 in_progress，不会在本池重复出现。
    let mut considered = candidates.len();
    match store.list_send_failure_candidates(nemesis_board::SEND_FAILURE_MARK) {
        Ok(send_fails) => {
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // 重试预算（多角度审计-13，替换旧 max_total_redispatch
            // 挪用）：专用键 board.budget.max_send_retries，默认 5，按本单
            // ⛔ 标记评论计数；0 = 关闭保险丝；unlimited_mode = true 视为 0
            //（操作者显式放弃护栏）。与父单全链派发行口径（max_total_redispatch）
            // 互不掺和——两种计数语义之前混在一个闸里，🛑 文案与审计口径双失真。
            let cfg = board_cfg.cloned().unwrap_or_default();
            let cap = if cfg.unlimited_mode || cfg.budget.max_send_retries == 0 {
                0
            } else {
                cfg.budget.max_send_retries
            };
            for (id, attempts, last_fail, _snapshot_status) in send_fails {
                considered += 1;
                // 快照复核（并发审计-13）：池 SELECT 与本臂执行间
                // 可隔基线转移等秒级窗口——单子可能已被并发链收口（done/
                // cancelled）或改道（评审回 todo 等）。终态单不派发也不落
                // 评论（⏸/🛑 落在收口单上是对事实撒谎）；分流一律用新鲜
                // 状态（快照 status 弃用），直派臂不再凭快照绕过 todo 的
                // 依赖闸。
                let fresh = match store.get_issue(id) {
                    Ok(i) => i,
                    Err(_) => continue,
                };
                if matches!(fresh.status, IssueStatus::Done | IssueStatus::Cancelled) {
                    tracing::debug!("[Board] 送达失败重试 issue={id} 已收口，跳过");
                    continue;
                }
                let status = fresh.status.as_str().to_string();
                // 重试预算闸：超限 → 🛑 转人工。🛑 评论落盘 = 最后一条
                // system 评论不再是 ⛔ 标记 → 出池。
                if cap > 0 && attempts > cap {
                    tracing::info!(
                        "[Board] 送达失败重试超预算（issue={id} {attempts}>{cap}）→ 转人工"
                    );
                    // 🛑 评论是熔断出池的载体（落盘 = 最后一条 system 评论
                    // 不再是 ⛔）——写入失败必须可见，静默吞掉 = 熔断永不
                    // 落盘 + 每 tick 静默重写（并发审计-13）。
                    if let Err(e) = store.add_comment(nemesis_board::models::NewComment {
                        issue_id: id,
                        author: actor.clone(),
                        content: format!(
                            "🛑 送达失败自动重试已达预算上限（本单 {attempts} 次 > \
                             board.budget.max_send_retries={cap}）：转人工处理。\
                             可人工指派节点后重新派发，或调大预算后重新派发恢复自动重试。"
                        ),
                        parent_id: None,
                        ctype: nemesis_board::CommentType::System,
                    }) {
                        tracing::warn!(
                            "[Board] 🛑 熔断评论写入失败（issue={id}，下 tick 重写）：{e}"
                        );
                    }
                    continue;
                }
                // 退避闸：未到重试时刻 → 本 tick 跳过。
                if now_secs
                    < last_fail.unsigned_abs() + nemesis_board::send_failure_backoff_secs(attempts)
                {
                    continue;
                }
                let dispatched_ok = if status == "in_review" {
                    // 评审 FAIL 重派回流形态：直派单点（不做松弛阶梯——
                    // 评审已经认可原目标的能力面，先原地重试）。差距意见
                    // 从评论找回随包重发（worker 必须知道为什么被打回，
                    // 否则重派就是掷骰子—— 检视 2 同源纪律）。
                    // 在途闸（闭环-13 补齐：backlog 臂经
                    // dispatch_subissue_auto_with_config 自带此闸，本直派
                    // 臂此前绕过——目标 worker 在途满载仍硬塞， 的
                    // 「同波多就绪单不挤单 worker」语义对重试臂失效）。
                    // **只 debug 延后，不落任何 system 评论**—— 候选的
                    // 身份锚 = 最后一条 system 评论是 ⛔ 标记，此处写评论
                    // 会把单子自己旋出重试池（自败）。
                    let target = pick_send_failure_redispatch_target(store, cluster, id);
                    match target {
                        Some(target) => {
                            let target = cluster.canonical_peer_id(&target).unwrap_or(target);
                            // 拓扑预检（并发审计-13）：file: 锚点 +
                            // 远端目标 = 确定性拓扑死锁，dispatch_issue_core
                            // 的拓扑闸必拒——先于派发尝试落 ⏸（成因精确）。
                            // 此前在 Err 臂事后判形态，冲突冻结/基线中止等
                            // 先序闸的瞬态 Err 也会被误诊成拓扑死锁而停车
                            // 出池（自动重试永久丢失）。
                            if let Some(reason) = reject_remote_file_anchors(
                                fresh.acceptance_criteria.as_deref(),
                                &target,
                                cluster.node_id(),
                                &cluster.node_name(),
                            ) {
                                tracing::info!(
                                    "[Board] 送达失败重试 issue={id} 含 file: 锚点（远端重派拓扑不可行）→ ⏸ 暂停转人工"
                                );
                                if let Err(e) =
                                    store.add_comment(nemesis_board::models::NewComment {
                                        issue_id: id,
                                        author: actor.clone(),
                                        content: format!(
                                            "⏸ 送达失败自动重试暂停：{reason}。\
                                         请将 file: 锚点改为 re: 型，或人工将任务派给本节点后恢复。"
                                        ),
                                        parent_id: None,
                                        ctype: nemesis_board::CommentType::System,
                                    })
                                {
                                    tracing::warn!(
                                        "[Board] ⏸ 出池评论写入失败（issue={id}，下 tick 重写）：{e}"
                                    );
                                }
                                continue;
                            }
                            let cap = board_cfg.map(|c| c.worker_max_inflight).unwrap_or(1);
                            let load = store.count_active_dispatch_by_worker().unwrap_or_default();
                            if inflight_full(&load, &target, cap) {
                                tracing::debug!(
                                    "[Board] 送达失败重试 issue={id} 目标 {target} 在途满载\
                                    （{}/{}），本 tick 延后",
                                    load.get(&target).copied().unwrap_or(0),
                                    cap
                                );
                                Ok(None)
                            } else {
                                let feedback = recover_review_feedback(store, id);
                                dispatch_issue_core(
                                    store,
                                    Some(cluster),
                                    id,
                                    &target,
                                    actor,
                                    feedback.as_deref(),
                                )
                                .map(|_| Some(()))
                            }
                        }
                        None => {
                            // 无可用目标分两类（多角度审计-13）：
                            // file: 锚点单 = 拓扑死锁（只能本节点执行，而送达
                            // 失败的目标是远端）——自动重试永不可能成功，⏸
                            // 出池转人工；其余（全员离线等）= 瞬态，debug 留池
                            // 等节点回来（此前 INFO 每 tick 刷屏）。
                            let file_anchor = store
                                .get_issue(id)
                                .ok()
                                .map(|i| {
                                    nemesis_board::anchor::has_file_anchor(
                                        i.acceptance_criteria.as_deref().unwrap_or(""),
                                    )
                                })
                                .unwrap_or(false);
                            if file_anchor {
                                tracing::info!(
                                    "[Board] 送达失败重试 issue={id} 含 file: 锚点（远端重派拓扑不可行）→ ⏸ 暂停转人工"
                                );
                                if let Err(e) =
                                    store.add_comment(nemesis_board::models::NewComment {
                                        issue_id: id,
                                        author: actor.clone(),
                                        content: "⏸ 送达失败自动重试暂停：本单验收标准含 file: 锚点，\
                                                  只能在派发端节点实核，而末次送达失败的派发目标是远端节点——\
                                                  自动重派在拓扑上不可行。请将 file: 锚点改为 re: 型，\
                                                  或人工将任务派给本节点后恢复。"
                                            .to_string(),
                                        parent_id: None,
                                        ctype: nemesis_board::CommentType::System,
                                    })
                                {
                                    tracing::warn!(
                                        "[Board] ⏸ 出池评论写入失败（issue={id}，下 tick 重写）：{e}"
                                    );
                                }
                            } else {
                                tracing::debug!(
                                    "[Board] 送达失败重试 issue={id}（in_review）暂无在线可用节点，留池"
                                );
                            }
                            Ok(None)
                        }
                    }
                } else {
                    dispatch_subissue_auto_with_config(
                        board_cfg,
                        store,
                        Some(cluster),
                        id,
                        actor,
                        notify_park,
                    )
                    .map(|opt| opt.map(|_| ()))
                };
                match dispatched_ok {
                    Ok(Some(_)) => {
                        dispatched += 1;
                        tracing::info!(
                            "[Board] 送达失败重试发车（issue={id} 第 {} 次）",
                            attempts + 1
                        );
                    }
                    Ok(None) => {}
                    Err(e) => {
                        failed += 1;
                        // estop 中途拉闸 → 尊重冻结留池（release 后自然续跑），
                        // 不落评论——冻结期对全部候选写 ⏸ 是纯噪声。
                        if board_estop_engaged() {
                            tracing::debug!(
                                "[Board] 送达失败重试 issue={id} 派发受阻（estop 生效，留池）：{e}"
                            );
                            continue;
                        }
                        // 已有新派发在途（并发触发源抢到 claim）→ 账本闸自然
                        // 出池，不落评论污染交付线程。
                        if store.has_active_dispatch(id).unwrap_or(false) {
                            tracing::debug!(
                                "[Board] 送达失败重试 issue={id} 已有在途派发接管，让位"
                            );
                            continue;
                        }
                        // 其余 Err（RPC 提交瞬断 / 项目冲突冻结 / 基线中止 /
                        // 状态竞态等）：留池按退避重试（1m→30m 限频，不是每
                        // tick），冻结类由 project.resume 自愈后自然续跑。
                        // 拓扑死锁已在派发前预检出池——此处不再事后判 file:
                        // 形态（并发审计-13 修：冻结等先序闸的瞬态
                        // Err 曾被误诊成拓扑死锁而永久停车）。
                        tracing::warn!(
                            "[Board] 送达失败重试派发 issue {id} 失败（留池按退避重试）：{e}"
                        );
                    }
                }
            }
        }
        Err(e) => tracing::warn!("[Board] 送达失败重试候选查询失败：{e}"),
    }
    (considered, dispatched, failed)
}

/// 送达失败重试的 in_review 重派目标：末次派发 worker 在线则原地重试
/// （送达失败 = 可达性本身是失败项，与 评审重派的 Same(last) 不同——
/// 那里 worker 刚交付过、可达性成立）；末次 worker 离线/无派发记录 →
/// 在线候选首选（[`rank_dispatch_candidates`] 匹配序）。无在线可用节点
/// 返回 None（调用方本 tick 跳过留池，退避闸继续生效）。
#[cfg(feature = "cluster")]
pub(super) fn pick_send_failure_redispatch_target(
    store: &Arc<BoardStore>,
    cluster: &Arc<nemesis_cluster::cluster::Cluster>,
    issue_id: i64,
) -> Option<String> {
    if let Ok(dispatches) = store.list_dispatches(issue_id)
        && let Some(last) = dispatches.last()
        && let Some(peer) = cluster.get_peer(&last.worker_id)
        && peer.is_online()
    {
        return Some(last.worker_id.clone());
    }
    let issue = store.get_issue(issue_id).ok()?;
    rank_dispatch_candidates(store, cluster, &issue)
        .into_iter()
        .find(|name| cluster.get_peer(name).is_some_and(|p| p.is_online()))
}

/// 送达失败重试（in_review）的验收差距找回：评审 FAIL 臂把差距以
/// 「❌ agent 验收未通过（第 N/M 次重派）## 差距…」用户评论落库
/// （board_review.rs 唯一写入方），取最后一条以「❌ agent 验收未通过」
/// **开头**的用户评论**整段原文**作 review_feedback——不解析差距/reasons
/// 边界，整段随「上轮验收意见」段重发（信息只多不少）。前缀锚定而非
/// contains（多角度审计-13）：人工评审意见若引用「## 差距」
/// 字样（引用 worker 自述/粘贴旧意），contains 会把人工评论误当自动
/// 重派差距重发。无此类评论（人工 in_review 直派送达失败等形态）返回
/// None——不造空意见。
#[cfg(feature = "cluster")]
pub(super) fn recover_review_feedback(store: &Arc<BoardStore>, issue_id: i64) -> Option<String> {
    store
        .list_comments(issue_id)
        .ok()?
        .into_iter()
        .rev()
        .find(|c| {
            c.ctype == nemesis_board::CommentType::Comment
                && c.content.starts_with("❌ agent 验收未通过")
        })
        .map(|c| c.content)
}

/// （goal）重平衡：节点 announce（上线/刷新）时，把其他 worker 手里
/// 「已派未开跑」（dispatched 且未 running）的排队单，按空闲 slot 挪给本
/// 节点——只从超载者（活跃 = 在途+排队 > cap，排队单存在即满载证据）偷
/// 队首；running 单不可迁移。新 worker 需满足该单角色/标签要求（同匹配器
/// 语义）。**file: 锚点单不偷**（[`nemesis_board::anchor:has_file_anchor`]，
/// 拓扑约束：转派远端必被派发拓扑闸拒绝——真机 / 实证取消后无法
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
    // 可执行性校验：新 worker 需满足该单角色/标签要求（同匹配器语义，
    // 候选走 matcher_candidates 单一入口——授予标签参与判定）。
    let candidates: Vec<nemesis_board::PeerCandidate> = matcher_candidates(store, cluster)
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
        // 拓扑约束（/ 根）：file: 锚点单只能在派发端
        // workspace 实核，转派远端必被 dispatch_issue_core 拓扑闸拒绝——
        // 而本函数 cancel 先行、失败不可逆。跳过不偷（留在原队列，原节点
        // 空闲后自然出队），谓词统一定义 = has_file_anchor。
        if nemesis_board::anchor::has_file_anchor(
            issue.acceptance_criteria.as_deref().unwrap_or(""),
        ) {
            continue;
        }
        let (required_role, required_tags) =
            normalize_dispatch_role(issue.required_role.as_deref(), &issue.required_tags);
        let input = nemesis_board::MatchInput {
            required_role: required_role.as_deref(),
            required_tags: &required_tags,
            required_profession: issue.required_profession.as_deref(),
            required_capabilities: &issue.required_capabilities,
            required_backend: issue.required_backend.as_deref(),
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
                // 补偿（/ 根）：取消已落库且不可逆，
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
/// 松弛排序 [`pick_fallback_target`] 共用的统一定义）：数据源 = 注册表
/// 真实 tags ∪ category（后 announce/静态 peers/get_info 的 tags 全链
/// 落地；曾长期降级为 `tags:= [category]`，planner 的 required_tags 天然
/// 不可满足 → 全部停车）。
#[cfg(feature = "cluster")]
pub(super) fn project_dispatch_candidates(
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
            // announce 携带的 tags 已落注册表——投影 = 真实 tags ∪
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
            // announce/静态 peers 携带的职能与档位已落注册表——同
            // tags 先例投影进 matcher（tier None = 闸 fail-open）。
            professions: p.professions.clone(),
            tier: p.tier.clone(),
            // announce/静态 peers 携带的 backend 宣告已落注册表——
            // 同 tags/tier 先例投影进 matcher（None = 闸 fail-closed）。
            runtime_backend: p.runtime_backend.clone(),
        })
        .collect()
}

/// required_role 归一：角色词表（worker/coordinator）外的
/// 非空词转标签语义（追加进 required_tags）——词表内原样保留。四个派发
/// 匹配面共用（rank_dispatch_candidates / rebalance_queued_to_worker /
/// pick_fallback_target / match_failure_detail）。此前 pick_fallback_target
/// 独家丢弃词表外角色词（其余三处转 tags），兜底松弛丢标签后零约束、
/// 任意在线节点可接单（根修）。
/// （cfg 随调用面走：四个调用方全部 cluster-gated，本函数不裸挂——此前
/// cfg 错位挂到本函数、rank_dispatch_candidates 反而裸奔，IoT 裁剪构建
/// （无 cluster）四处 /；修正后两者同闸，裁剪构建零死代码。）
#[cfg(feature = "cluster")]
fn normalize_dispatch_role(
    required_role: Option<&str>,
    required_tags: &[String],
) -> (Option<String>, Vec<String>) {
    match required_role.map(str::trim) {
        Some(r) if !r.is_empty() && r != "worker" && r != "coordinator" => {
            let mut tags = required_tags.to_vec();
            tags.push(r.to_string());
            (None, tags)
        }
        Some(r) if !r.is_empty() => (Some(r.to_string()), required_tags.to_vec()),
        _ => (None, required_tags.to_vec()),
    }
}

/// 匹配器全量排序（换节点重派的候选来源 + [`pick_target_by_matcher`]
/// 的统一定义）：在线节点（不含本机）→ 投影 →
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
    let (required_role, required_tags) =
        normalize_dispatch_role(issue.required_role.as_deref(), &issue.required_tags);

    let load = store.count_active_dispatch_by_worker().unwrap_or_default();
    // 拓扑约束（// 根）：file: 锚点单只能本节点
    // 执行，而候选投影全是远端节点（excluding_self）——返回空候选，让
    // 消费方走各自的「无候选」路径（换节点决策回落 Same(本节点) / 匹配器
    // 停车 / 冲突解决同节点重派），不产出注定被拓扑闸拒绝的非法目标。
    // 谓词统一定义 = has_file_anchor。
    if nemesis_board::anchor::has_file_anchor(issue.acceptance_criteria.as_deref().unwrap_or("")) {
        return Vec::new();
    }
    let input = nemesis_board::MatchInput {
        required_role: required_role.as_deref(),
        required_tags: &required_tags,
        required_profession: issue.required_profession.as_deref(),
        required_capabilities: &issue.required_capabilities,
        required_backend: issue.required_backend.as_deref(),
        description: &issue.description,
    };
    // E（goal）：候选 tags 合并 master 授予标签（granted_tags 台账）。
    let candidates = matcher_candidates(store, cluster);
    nemesis_board::rank_peers(&input, &candidates, &load)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// 匹配器选节点：取 [`rank_dispatch_candidates`] 榜首（先过 耗时
/// 次级重排——见 [`duration_weighted_candidates`]）。
#[cfg(feature = "cluster")]
pub(super) fn pick_target_by_matcher(
    store: &BoardStore,
    cluster: &nemesis_cluster::cluster::Cluster,
    issue: &nemesis_board::Issue,
) -> Option<String> {
    duration_weighted_candidates(
        store,
        issue,
        rank_dispatch_candidates(store, cluster, issue),
    )
    .into_iter()
    .next()
}

/// （引入）：耗时估计次级重排。匹配器硬条件
/// 排序（分数↓负载↑）之后，按 (worker, required_profession) 历史耗时
/// 中位数把「同类任务跑得快」的节点提前——借鉴前代实现运行时估计器
/// （滑窗中位数 + 欠采样回落全局 + 无数据零干预），数据面 = 本仓现成的
/// `issue_dispatch` 完结派发表（master 侧单钟），无需新计时埋点。
/// 单候选 / 无样本 / 查询失败 → 原序返回（零行为变化红线）。与
/// 指纹稳定加权（成功率，board_review 侧）正交：本函数管速度。
#[cfg(feature = "cluster")]
pub(super) fn duration_weighted_candidates(
    store: &BoardStore,
    issue: &nemesis_board::Issue,
    ranked: Vec<String>,
) -> Vec<String> {
    if ranked.len() < 2 {
        return ranked;
    }
    let category = nemesis_board::normalize_category(issue.required_profession.as_deref());
    match store.recent_completed_dispatch_durations((nemesis_board::WORKER_SAMPLE_CAP * 8) as i64) {
        Ok(samples) if !samples.is_empty() => {
            let ordered = nemesis_board::reorder_by_predicted_duration(ranked, &samples, &category);
            tracing::debug!(
                "[board] duration weighting (category={category}) reordered candidates: {ordered:?}"
            );
            ordered
        }
        Ok(_) => ranked,
        Err(e) => {
            tracing::warn!("[board] duration sample query failed (original order kept): {e}");
            ranked
        }
    }
}

/// 停车场兜底目标（集群完备性加固； 三档化）。自动
/// 派发匹配不到节点时，为了任务做下去由「一个客户端」推进。返回
/// `(节点 id, 兜底说明)`；None = 不兜底（调用方走原停车路径）。
///
/// 语义（用户裁决 钉住规则 / 三档默认 Role）：
/// - `Off` / 无在线节点 → None（兜底造不出客户端，诚实停车）。
/// - `dispatch_fallback_target` 钉住目标：按 name 或 id 精确匹配在线节点
///   （大小写不敏感）；**钉住的不在线 = None，不悄悄换人**（钉住即点名）。
/// - 未钉住：按档位走 松弛阶梯——`Role` 档 ②保职能丢标签 → ③丢职能
///   保角色标签（worker/coordinator 纪律保留）；`Full` 档阶梯全开 ②③④。
///   rank_peers 同分按负载↑、id 字典序，确定性。
#[cfg(feature = "cluster")]
pub(super) fn pick_fallback_target(
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    store: &BoardStore,
    cluster: &nemesis_cluster::cluster::Cluster,
    issue: &nemesis_board::Issue,
) -> Option<(String, &'static str)> {
    use nemesis_config::DispatchFallbackMode;
    // 拓扑约束（根）：file: 锚点单只能本节点执行，兜底候选
    // 全是远端节点——兜底产物必被拓扑闸拒绝，直接 None 走停车路径（与
    // 匹配器无候选同语义）。谓词统一定义 = has_file_anchor。
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

    // 2. 松弛排序（阶梯 = matcher::pick_relaxed：②保职能丢标签 →
    //    ③丢职能（保角色/标签）→ ④全松弛；无职能子单 ② 蜕化为旧「保
    //    角色丢标签」，行为不变）。角色归一沿用严格匹配（词表外角色词
    //    已在严格匹配里转 tags，这里只需认 worker/coordinator）。
    let candidates = matcher_candidates(store, cluster);
    let load = store.count_active_dispatch_by_worker().unwrap_or_default();
    // 2. 松弛排序（阶梯 = matcher::pick_relaxed：②保职能丢标签 →
    //    ③丢职能（保角色/标签）→ ④全松弛；无职能子单 ② 蜕化为旧「保
    //    角色丢标签」，行为不变）。角色归一走统一定义
    //    normalize_dispatch_role（词表外角色词转 tags——此前这里独家丢词，
    //    松弛丢标签后零约束，任意在线节点可接单）。
    let (role, tags) =
        normalize_dispatch_role(issue.required_role.as_deref(), &issue.required_tags);
    let base = nemesis_board::MatchInput {
        required_role: role.as_deref(),
        required_tags: &tags,
        required_profession: issue.required_profession.as_deref(),
        required_capabilities: &issue.required_capabilities,
        required_backend: issue.required_backend.as_deref(),
        description: &issue.description,
    };
    nemesis_board::pick_relaxed(&base, &candidates, &load, depth)
}

/// E（goal）：候选投影合并 master 授予标签（tags ∪ granted_tags）。
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

/// 匹配器候选投影的单一入口（BUG-20261115-001 横扫收敛）：在线节点投影
/// ∪ master 授予标签合并。此前 rank 侧带标签合并、停车明细/兜底取候选侧
/// 用裸投影——明细会把已授予标签误报成缺口（反向撒谎）。决策面
/// （[`rank_dispatch_candidates`]）与观测面（明细）从此吃同一份候选。
#[cfg(feature = "cluster")]
pub(super) fn matcher_candidates(
    store: &BoardStore,
    cluster: &nemesis_cluster::cluster::Cluster,
) -> Vec<nemesis_board::PeerCandidate> {
    merge_granted_tags(store, project_dispatch_candidates(cluster))
}

/// （goal）匹配失败明细（纯函数，单测直测）：要求 vs 每个在线候选
/// 差什么（与 rank_peers 同语义：required_role 精确、required_tags 交集
/// 非空；词表外角色词已在 rank_dispatch_candidates 归一进 tags）。
/// 输出人读行集，直接拼进停车评论——「差什么」一眼可见。
///
/// 判定面必须与决策面同构（BUG-20261115-001）：rank_peers 的 6 个硬条件
/// （role/tags/prof/tier/caps/backend）逐项在此有镜像——backend 闸是
/// fail-closed（节点未宣告 = 排除且不参与松弛），明细不查就会产出
/// 「全 ✓ 却无可匹配」的矛盾停车评论（真机 NB-3 实证）。
#[cfg(feature = "cluster")]
pub(super) fn match_failure_detail(
    issue: &nemesis_board::Issue,
    candidates: &[nemesis_board::PeerCandidate],
) -> String {
    // 拓扑闸镜像（与 rank_dispatch_candidates 同源谓词 has_file_anchor）：
    // file: 锚点单的候选投影被决策面整体清空（锚点在派发端 workspace 实核，
    // 只能本节点执行），明细不得伪装成「无在线候选节点」误导诊断。
    if nemesis_board::anchor::has_file_anchor(issue.acceptance_criteria.as_deref().unwrap_or("")) {
        return "单子含 file: 类锚点（拓扑约束：锚点在派发端 workspace 实核，只能本节点执行），不参与远端节点匹配".to_string();
    }
    // 角色归一（统一定义 normalize_dispatch_role）：词表外角色词转标签。
    let (required_role, required_tags) =
        normalize_dispatch_role(issue.required_role.as_deref(), &issue.required_tags);
    if candidates.is_empty() {
        return "无在线候选节点".to_string();
    }
    // 职能需求：精确匹配语义 + tier 闸差值都要在停车评论里可见。
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
    // backend 需求归一（与 rank_peers 硬条件 6 同源：trim + 小写；空 = 不过闸）。
    let backend_need = issue
        .required_backend
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);
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
        // 环境能力硬条件（批）：逐项查缺口（大小写不敏感精确词）。
        for need in &issue.required_capabilities {
            let need = need.trim();
            let hit = !need.is_empty()
                && c.capabilities
                    .iter()
                    .any(|x| x.trim().eq_ignore_ascii_case(need));
            if !hit {
                missing.push(format!("缺能力 {need}"));
            }
        }
        // backend 硬条件 6 镜像（fail-closed：节点未宣告 = 排除，与
        // rank_peers 同构；宣告值大小写不敏感精确比对）。
        if let Some(need) = &backend_need {
            let declared = c
                .runtime_backend
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let hit = declared.is_some_and(|b| b.to_lowercase() == *need);
            if !hit {
                match declared {
                    Some(b) => missing.push(format!("backend {b}≠{need}")),
                    None => missing.push(format!(
                        "未宣告 backend（需求 {need}；闸 fail-closed，需节点宣告同值 backend）"
                    )),
                }
            }
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
        let cap_str = if c.capabilities.is_empty() {
            "—".to_string()
        } else {
            c.capabilities.join("/")
        };
        let backend_str = c
            .runtime_backend
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("—");
        lines.push(format!(
            "{} {}[role={},tags={},prof={},caps={},backend={}]{}",
            if missing.is_empty() { "✓" } else { "✗" },
            c.name,
            c.role,
            tag_str,
            prof_str,
            cap_str,
            backend_str,
            if missing.is_empty() {
                String::new()
            } else {
                format!(" 缺:{}", missing.join(","))
            }
        ));
    }
    lines.join("；")
}
/// 自动派发判定（纯函数，可单测）：config 开关开 **且** 指派对象是 worker。
/// 开关 = `config.json` 的 `board.auto_dispatch`（.5 接口预留，
/// 现阶段不做自动派发，默认 false；置 true 即激活）。
pub(super) fn should_auto_dispatch(
    board_cfg: Option<&nemesis_config::BoardFlagConfig>,
    assignee: Option<AssignmentType>,
) -> bool {
    board_cfg.map(|b| b.auto_dispatch).unwrap_or(false) && assignee == Some(AssignmentType::Worker)
}

/// 读 live config 的 board 段（全局 ConfigStore 单例；测试/CLI 无单例 →
/// None → 判定恒 false，与「默认关」语义一致）。
pub(super) fn live_board_config() -> Option<nemesis_config::BoardFlagConfig> {
    nemesis_config::load_live().and_then(|c| c.board)
}

/// 指派成功后的自动派发（`issue.assign` 末尾调用；.5 接口预留）。走
/// [`dispatch_issue_core`] 单一派发入口。失败**不回滚指派**：warn + ⛔ 系统
/// 评论留痕（与派发 RPC 送达失败同语义）。返回是否实际派发（调用方据此
/// 重取 issue 保证响应反映派发后的 in_progress 态）。
#[cfg(feature = "cluster")]
pub(super) fn auto_dispatch_after_assign(
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
pub(super) fn auto_dispatch_with_config(
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
pub(super) fn auto_dispatch_after_assign(
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

// flow 导入单的自动派发（外循环域剥离后唯一消费方 = flow 引擎 board_land 步）

/// flow 导入单的派发（flow 引擎模板 land 步建单后的自动派发）。目标解析
/// 阶梯 = 显式 `target`（模板钉住，不在线诚实停车不换人）→ 匹配器硬条件 →
/// 兜底松弛（board.dispatch_fallback）→ None = ⏸ 系统评论停车（sweep/
/// 停车场/announce 既有触发器接管恢复）。派发失败不回滚导入：warn + ⛔
/// 评论留痕（与 auto_dispatch_with_config 同语义）。返回是否实际派发。
///
/// pub 供 nemesisbot 跨 crate 复用（install_baseline_pusher 同先例）——
/// 派发单一入口仍是 [`dispatch_issue_core`]，本 helper 只包目标解析。
#[cfg(feature = "cluster")]
pub fn dispatch_issue_for_flow(
    store: &Arc<BoardStore>,
    cluster: Option<&Arc<nemesis_cluster::cluster::Cluster>>,
    issue: &nemesis_board::Issue,
    explicit_target: Option<&str>,
    actor: &Actor,
) -> bool {
    let pinned = explicit_target
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string());
    // target="self" 单机本地执行面（一等公民，无回退语义）：恒走本地执行
    // 器直驱本机 AgentLoop，与集群是否在无关（不走 RPC 回环；集群
    // RPC-loopback-to-self 仍以显式节点名/id 达成）。分支置于集群检查
    // 之前——"self" 不是集群概念，集群未启用也照常本地执行。字面量大小
    // 写不敏感（词法层与集群同款）。
    if pinned.as_deref().map(|t| t.eq_ignore_ascii_case("self")) == Some(true) {
        return match dispatch_issue_local(store, issue.id, actor, None) {
            Ok(_) => {
                tracing::info!(
                    "[Board] flow auto-dispatch: issue #{} → self（本地执行）",
                    issue.number
                );
                true
            }
            Err(e) => {
                tracing::warn!(
                    "[Board] flow auto-dispatch failed for issue #{}: {e}",
                    issue.number
                );
                let _ = store.add_comment(nemesis_board::models::NewComment {
                    issue_id: issue.id,
                    author: nemesis_board::Actor::system("board"),
                    content: format!("⛔ flow 导入自动派发失败：{e}"),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::System,
                });
                false
            }
        };
    }
    let Some(cluster) = cluster else {
        tracing::warn!(
            "[Board] flow: 集群未启用，issue #{} 留 backlog 等手动处理",
            issue.number
        );
        return false;
    };
    let target = pinned.or_else(|| {
        pick_target_by_matcher(store, cluster, issue).or_else(|| {
            pick_fallback_target(live_board_config().as_ref(), store, cluster, issue)
                .map(|(t, _)| t)
        })
    });
    let Some(target) = target else {
        let msg =
            "⏸ flow 导入暂缓派发：无匹配节点（职能/能力/兜底均未命中），等待节点上线由 sweep 承接";
        tracing::info!("[Board] flow issue #{}: {msg}", issue.number);
        let _ = store.add_comment(nemesis_board::models::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::system("board"),
            content: msg.to_string(),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        });
        return false;
    };
    match dispatch_issue_core(store, Some(cluster), issue.id, &target, actor, None) {
        Ok(_) => {
            tracing::info!(
                "[Board] flow auto-dispatch: issue #{} → {target}",
                issue.number
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                "[Board] flow auto-dispatch failed for issue #{}: {e}",
                issue.number
            );
            let _ = store.add_comment(nemesis_board::models::NewComment {
                issue_id: issue.id,
                author: nemesis_board::Actor::system("board"),
                content: format!("⛔ flow 导入自动派发失败：{e}"),
                parent_id: None,
                ctype: nemesis_board::CommentType::System,
            });
            false
        }
    }
}

/// 非 cluster 编译形态：诚实零操作 + warn（与 auto_dispatch_after_assign
/// 非 cluster 臂同款纪律）。无调用方（flow_engine 的 dispatch_issue 恒在
/// cluster 形态）——桩仅为跨形态可装配而存在；cluster 槽位类型是
/// `Option<&()>`（AppState.cluster 的 not(cluster) 镜像，收下不用），
/// 不得引用 nemesis_cluster 类型（该形态下 crate 不编译）。
#[cfg(not(feature = "cluster"))]
pub fn dispatch_issue_for_flow(
    _store: &Arc<BoardStore>,
    _cluster: Option<&()>,
    _issue: &nemesis_board::Issue,
    _explicit_target: Option<&str>,
    _actor: &Actor,
) -> bool {
    false
}
// target="self" 单机本地执行面（一等公民，与集群是否启用无关）

/// 本地执行请求（target="self" 派发经 [`dispatch_issue_local`] 组装，交由
/// 网关注册的执行器上总线驱动本机 AgentLoop）。
#[cfg(feature = "cluster")]
pub struct LocalDispatchRequest {
    pub task_id: String,
    pub issue_id: i64,
    /// 展示编号（"" 形态， nemesis_board::Issue.number 原样透传，
    /// 日志/评审上下文用）。
    pub issue_number: String,
    pub prompt: String,
    /// 派发代际号：claim 成功的 generation 原样下行，
    /// 执行器写回时经 `write_back_board_dispatch` 回声——写回闸据此
    /// 拒绝旧代迟到回调覆盖新一代派发。None = 老调用方未带（回声闸
    /// 放行，兼容存量）。
    pub generation: Option<i64>,
}

/// 本地执行器（gateway 侧装配，见 nemesisbot local_dispatch.rs）。`submit`
/// 同步面只做校验/消息入总线（Err = 派发未成行，调用方走 同款回滚）；
/// 回复捕获/写回/评审触发在执行器内部异步完成（镜像 RPC ACK 后异步回报
/// 语义——dispatched 行存续到执行器写回终态为止）。
#[cfg(feature = "cluster")]
pub trait LocalDispatchExecutor: Send + Sync {
    fn submit(&self, req: LocalDispatchRequest) -> Result<(), String>;
}

#[cfg(feature = "cluster")]
static LOCAL_DISPATCH_EXECUTOR: std::sync::OnceLock<std::sync::Arc<dyn LocalDispatchExecutor>> =
    std::sync::OnceLock::new();

/// 网关装配点（post_agent projects_manager 装配后调用；OnceLock 语义 =
/// 首装胜出，网关单装配无竞态面）。
#[cfg(feature = "cluster")]
pub fn install_local_dispatch_executor(executor: std::sync::Arc<dyn LocalDispatchExecutor>) {
    if LOCAL_DISPATCH_EXECUTOR.set(executor).is_err() {
        tracing::warn!("[Board] 本地执行器重复安装，忽略（首装胜出）");
    }
}

/// target="self" 单机本地派发：完整镜像 [`dispatch_issue_core`] 的闸门与
/// 账务链（estop/冲突冻结/状态/查重/计划五闸 → 经验注入 → attempt →
/// prompt → 账本开账 → claim("self") → 派发里程碑 → in_progress → 指派
/// 回填），只把「集群 RPC 发车」换成「本地执行器上总线」。submit 失败 =
/// 任务从未开始，走 同款回滚（finish FAILED + 账本收口 + 状态/指派
/// 回退 + ⛔ SEND_FAILURE_MARK 评论，sweep 按 MARK 前缀识别重试候选）。
/// 本地面无 push_dispatch_baseline（对端收件箱协议）/无 canonical_peer_id
/// （目标恒字面 "self"）/无 remote file 锚点闸（执行根在本机，file: 锚
/// 可达）——三处省略是本地形态的结构性差异，不是闸门缺口。
#[cfg(feature = "cluster")]
pub fn dispatch_issue_local(
    store: &Arc<BoardStore>,
    issue_id: i64,
    actor: &Actor,
    review_feedback: Option<&str>,
) -> Result<serde_json::Value, String> {
    let executor = LOCAL_DISPATCH_EXECUTOR
        .get()
        .ok_or("本地执行器未装配（网关未安装本地派发面），target=\"self\" 不可用")?;

    // EST-01/02：estop 生效中一律诚实拒绝（镜像 core，派发单一落点纪律）。
    if board_estop_engaged() {
        return Err(estop_frozen_error());
    }
    let issue = store.get_issue(issue_id)?;

    // / 冲突冻结闸（镜像 core）。
    if let Some(pid) = issue.project_id
        && let Ok(project) = store.get_project(pid)
        && project.conflict_frozen
    {
        return Err(format!(
            "项目「{}」冲突冻结中，禁止派发（含重派/兜底/autopilot）：先 project.resume 人工落定冲突并补合并冻结期交付",
            project.name
        ));
    }

    // 状态闸（镜像 core）。
    match issue.status {
        IssueStatus::Backlog
        | IssueStatus::Todo
        | IssueStatus::InProgress
        | IssueStatus::InReview => {}
        other => return Err(format!("issue 处于 {other} 状态，不可派发")),
    }

    // 已有未完结派发 → 拒绝重复派发（镜像 core）。
    if store.has_active_dispatch(issue.id)? {
        return Err("该 issue 已有进行中的派发（等 worker 回报或超时后再试）".to_string());
    }

    // BUG-②：挂有未确认计划的父单不可直接派发（镜像 core；本函数 cfg
    // cluster，RUNNING_PLANS/PLAN_CACHE 静态量直接可用）。
    let now = std::time::Instant::now();
    let running = RUNNING_PLANS
        .lock()
        .values()
        .any(|r| r.issue_id == issue.id);
    let ready_ids: Vec<String> = {
        let mut cache = PLAN_CACHE.lock();
        cache.retain(|_, p| now.duration_since(p.created_at) <= PLAN_TTL);
        cache
            .iter()
            .filter(|(_, p)| p.issue_id == issue.id)
            .map(|(id, _)| id.clone())
            .collect()
    };
    if running || !ready_ids.is_empty() {
        let state_word = if running {
            "拆解进行中"
        } else {
            "待确认"
        };
        let id_hint = ready_ids
            .first()
            .map(|id| format!("（plan {id}）"))
            .unwrap_or_default();
        return Err(format!(
            "该单挂有未确认的拆解计划（{state_word}{id_hint}）：到任务详情确认该计划，或等其过期（{}s）后重试",
            PLAN_TTL.as_secs()
        ));
    }

    let target = "self";
    // .5：经验注入段在 prompt 组装前检索（镜像 core）。
    let (experience_section, experience_ids) = render_issue_experience_section(store, &issue);
    // 重派轮次 = 历史派发行数 + 1（镜像 core）。
    let attempt_count = store.list_dispatches(issue.id)?.len() as u32;
    let attempt = (attempt_count > 0).then_some(attempt_count + 1);
    let prompt = build_dispatch_prompt(
        &issue,
        render_dispatch_assets_section(store, issue.id)?.as_deref(),
        review_feedback,
        experience_section.as_deref(),
        attempt,
    );

    // 本地 task id：`local-` 前缀 + uuid（写回路由按 issue_dispatch 表的
    // task_id 匹配，前缀无路由语义，纯人读可辨）。
    let task_id = format!("local-{}", uuid::Uuid::new_v4());

    // 派发账本开账（镜像 core；收口点 = 执行器写回终态 + 本函数
    // 两处派发中止）。
    nemesis_taskstore::ledger::begin_run(
        &nemesis_taskstore::ledger::board_dispatch_run_id(&task_id),
        "board_dispatch",
        &issue.title,
        target,
        serde_json::json!({ "issue_id": issue.id, "issue_number": issue.number }),
    );

    // .5：注入计数只认实际发车的派发（镜像 core）。
    if !experience_ids.is_empty()
        && let Err(e) = store.mark_team_memory_used(&experience_ids)
    {
        tracing::warn!("[Board] team_memory use_count 更新失败（不影响派发）：{e}");
    }

    // 2. 原子占用闸（BUG-2 镜像 core；claim 目标 = "self"）。
    //    远端签名升级：bool → Option<i64>（代际号）；None = 已被占用
    //    → 竞态取消；Some(generation) 随执行器请求下行、写回时原样
    //    回声（写回闸拒绝旧代迟到回调——本地形态同一语义）。
    let dispatch_generation = match store.try_claim_dispatch(&task_id, issue.id, target, actor)? {
        Some(generation) => generation,
        None => {
            nemesis_taskstore::ledger::settle_run(
                &nemesis_taskstore::ledger::board_dispatch_run_id(&task_id),
                "cancelled",
                Some("并发派发竞态：该 issue 已有进行中的派发"),
            );
            return Err("该 issue 已有进行中的派发（并发派发竞态，本次派发已取消）".to_string());
        }
    };

    // C 里程碑 2（镜像 core；本地形态无基线下发 → 第 5 参 None）。
    nemesis_board::archive_writer::write_dispatch_milestone(store, &issue, target, &task_id, None);

    // 3. 状态推进 + 指派回填（镜像 core；转移失败回滚 claim 的 dispatch 行）。
    let dispatch_prev_status = issue.status;
    let dispatch_prev_assignee = issue.assignee;
    let dispatch_prev_assignee_id = issue.assignee_id.clone();
    let issue = if issue.status != IssueStatus::InProgress {
        match store.transition_issue(issue.id, IssueStatus::InProgress, actor) {
            Ok(i) => i,
            Err(e) => {
                let _ =
                    store.finish_dispatch(&task_id, nemesis_board::models::dispatch_state::FAILED);
                nemesis_taskstore::ledger::settle_run(
                    &nemesis_taskstore::ledger::board_dispatch_run_id(&task_id),
                    "failed",
                    Some(&format!("状态转移失败：{e}")),
                );
                return Err(format!("派发中止：状态转移失败：{e}"));
            }
        }
    } else {
        issue
    };
    link_project_on_dispatch(store, issue.project_id);
    if (issue.assignee != Some(AssignmentType::Worker)
        || issue.assignee_id.as_deref() != Some(target))
        && let Err(e) = store.assign_issue(
            issue.id,
            Some(AssignmentType::Worker),
            Some(target.to_string()),
            actor,
        )
    {
        tracing::warn!("[Board] 派发回填指派失败（不阻断派发）：{e}");
    }

    // 4. 发车：执行器同步面失败 = 任务从未开始 → 同款回滚（诚实失败，
    // ⛔ SEND_FAILURE_MARK 评论 = sweep 重试候选识别前缀）。
    let submit_result = executor.submit(LocalDispatchRequest {
        task_id: task_id.clone(),
        issue_id: issue.id,
        issue_number: issue.number.clone(),
        prompt,
        generation: Some(dispatch_generation),
    });
    if let Err(e) = submit_result {
        let _ = store.finish_dispatch(&task_id, nemesis_board::models::dispatch_state::FAILED);
        nemesis_taskstore::ledger::settle_run(
            &nemesis_taskstore::ledger::board_dispatch_run_id(&task_id),
            "failed",
            Some(&e),
        );
        match store.rollback_dispatch_failure(
            issue.id,
            dispatch_prev_status,
            dispatch_prev_assignee,
            dispatch_prev_assignee_id,
            &nemesis_board::Actor::system("board"),
        ) {
            Ok(true) => tracing::info!("[Board] 本地发车失败回退完成（issue={}）", issue.id),
            Ok(false) => {}
            Err(re) => tracing::warn!("[Board] 本地发车失败回退未执行（让位或异常）：{re}"),
        }
        let _ = store.add_comment(nemesis_board::models::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::system("board"),
            content: format!(
                "{}: [local] 本地发车失败（{e}）",
                nemesis_board::SEND_FAILURE_MARK
            ),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        });
        return Err(format!("本地发车失败：{e}"));
    }

    // 派发成功 = 单据回到执行轨道（镜像 core）。
    archive_handoff_best_effort(&issue.number);
    tracing::info!(
        "[Board] 本地派发落定：issue #{} → self（task {task_id}）",
        issue.number
    );
    Ok(serde_json::json!({
        "dispatched": true,
        "task_id": task_id,
        "issue": issue_to_view(store, &issue)?,
    }))
}
