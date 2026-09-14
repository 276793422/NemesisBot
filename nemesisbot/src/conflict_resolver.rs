//! P5/F5 冲突硬解执行体（auto 档，`board.config.conflict_auto_resolve`）。
//!
//! 项目不停摆为第一原则，永远有下一步动作（goal 钦定链路）：
//!
//! 1. **AI 强制硬解**（复用评审 LLM 通道 detached 调用 + tier 闸拒 mini）：
//!    输入=冲突三阶段内容（我方/对方/基线）+ 子单任务意图；输出只允许改
//!    冲突文件（范围钉死，越界拒绝）——「以当前仓库内容为主，不乱搞」；
//!    无置信回落；产出经 [`nemesis_board::git_repo::commit_resolution`]
//!    合入 + commit（message 带 `conflict_auto_resolve` 标记+解题理由）。
//! 2. **机械失败路径**（产出无法应用/重试 `RESOLVE_MAX_ATTEMPTS` 轮仍败，
//!    N=3 含错误反馈重试——非置信问题，是不造假的诚实失败）→ 立刻放弃
//!    本次合并 → 重派【原 worker】：带最新基线+原任务+冲突说明。
//! 3. **原 worker 离线** → 三轮主动接触（t0/+60s/+120s，[`Cluster::probe_peer`]
//!    帧级真探）→ 任一轮应答照发原 worker；三轮全无 → 直接换人
//!    （`rank_dispatch_candidates` 选历史未用在线候选——探针已证不可达，
//!    D3 的「同 worker 连败≥2 才换」规则不适用于接触性失败）。否决
//!    「排队等原 worker 回来」（停摆风险，用户拍板）。
//! 4. 换人后原 worker 归来 → 派发记录已被新 target supersede（既有语义），
//!    旧变更集按 E8 归属校验作废，不会双版本打架。
//! 5. 重派循环套既有预算保险丝（`budget_breach`），打满转人工——兜底，
//!    非常态（回落 human 档：项目冻结 + 停车）。
//!
//! F7 审计：`conflict_auto_resolve`（硬解成功）/ `conflict_redispatch`
//!（硬解败重派原 worker）/ `conflict_switch_worker`（换人）全走
//! `record_auto_decide` 单一漏斗；彻底失败回落 human 档记 `conflict`。

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use nemesis_board::assignment::Actor;
use nemesis_board::git_repo::ConflictFile;
use nemesis_board::models::{CommentType, Issue, NewComment};
use tracing::{info, warn};

/// 硬解 LLM 重试轮数（含错误反馈重试；共 3 轮，与评审/规划器回灌预算同量级）。
pub(crate) const RESOLVE_MAX_ATTEMPTS: u32 = 3;
/// 原 worker 三轮主动接触时刻表（秒，自 t0 起的绝对时刻；具名常量）。
pub(crate) const PROBE_SCHEDULE_SECS: &[u64] = &[0, 60, 120];
/// 单文件单侧内容注入 prompt 的字节上限（超出截断+注记；防超长文件撑爆
/// 上下文——硬解只看冲突邻域也够）。
const PROMPT_SIDE_CAP_BYTES: usize = 16 * 1024;

/// lockfile 清单（E5/F6：清单类文件 AI 解时理由必须含「建议重新生成」
/// ——锁文件正确解法是重新生成而不是手工缝合）。
const LOCKFILE_NAMES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "poetry.lock",
    "Pipfile.lock",
    "Gemfile.lock",
    "composer.lock",
    "packages.lock.json",
];

/// 硬解产出单文件处置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolutionAction {
    /// 文本级三方缝合（content = 完整新文件内容）。
    Merge,
    /// 采信我方（当前仓库内容）。
    Ours,
    /// 采信对方（worker 变更集）。
    Theirs,
}

impl ResolutionAction {
    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "merge" => Some(Self::Merge),
            "ours" => Some(Self::Ours),
            "theirs" => Some(Self::Theirs),
            _ => None,
        }
    }
}

/// 校验后的单文件硬解产出。
#[derive(Debug, Clone)]
pub(crate) struct Resolution {
    pub path: String,
    pub action: ResolutionAction,
    pub content: Option<Vec<u8>>,
    pub reason: String,
}

/// 冲突硬解执行体入口（fire-and-forget；由 board_archive_ingest 冲突分支
/// 在 auto 档调用）。异步接管：硬解 → 落盘 → 收口，或机械失败 → 重派链。
pub(crate) fn spawn_conflict_resolver(
    deps: crate::board_review::BoardReviewDeps,
    issue: Issue,
    worker_id: String,
    task_id: String,
    conflicts: Vec<ConflictFile>,
) {
    tokio::spawn(async move {
        run_resolver(deps, issue, worker_id, task_id, conflicts).await;
    });
}

async fn run_resolver(
    deps: crate::board_review::BoardReviewDeps,
    issue: Issue,
    worker_id: String,
    task_id: String,
    conflicts: Vec<ConflictFile>,
) {
    // estop 保险丝：急停中不跑硬解（与评审/重派同语义）→ human 档兜底。
    if deps.estop.is_engaged() {
        fallback_freeze(&deps, &issue, &task_id, "急停中，冲突硬解挂起转人工");
        return;
    }
    // LLM 通道就绪闸：主 agent 未装配无法硬解 → human 档兜底（诚实）。
    let Some(agent_loop) = deps.moderator_loop.get().cloned() else {
        fallback_freeze(&deps, &issue, &task_id, "主 agent 未就绪，无法硬解，转人工");
        return;
    };
    // F5 tier 闸拒 mini：小模型硬解合并冲突是造假重灾区——human 档兜底。
    if matches!(
        agent_loop.tier(),
        nemesis_types::capability::ModelTier::Mini
    ) {
        fallback_freeze(
            &deps,
            &issue,
            &task_id,
            "当前模型能力档为 mini，冲突硬解需要 normal/big 档模型（board.conflict_auto_resolve 保持开启时请切换模型），转人工",
        );
        return;
    }

    // ---- 硬解（≤3 轮，错误反馈回灌）----
    let mut prompt = build_user_prompt(&issue, &task_id, &conflicts);
    let mut resolved: Option<Vec<Resolution>> = None;
    let mut last_err = String::new();
    for round in 1..=RESOLVE_MAX_ATTEMPTS {
        let opts = nemesis_agent::r#loop::DetachedOpts {
            system_prompt: Some(CONFLICT_RESOLVER_SYSTEM_PROMPT),
            no_tools: true,
            max_turns: 1,
            label: Some("conflict-resolver"),
            ..Default::default()
        };
        let raw = match agent_loop.run_detached(&prompt, opts).await {
            Ok(raw) => raw,
            Err(e) => {
                // LLM 调用失败 ≠ 产出不合规，不消耗解析轮（与评审同语义）。
                last_err = format!("LLM 调用失败：{e}");
                break;
            }
        };
        match parse_resolutions(&raw, &conflicts) {
            Ok(r) => {
                resolved = Some(r);
                break;
            }
            Err(e) => {
                info!(
                    issue = %issue.number,
                    round,
                    "[ConflictResolver] 硬解产出不合规，回灌重试：{e}"
                );
                last_err = format!("第 {round} 轮产出不合规：{e}");
                prompt.push_str(&format!(
                    "\n\n## 上一轮产出被拒绝（{last_err}）\n\n上一轮原文：\n{raw}\n\n请修正后重新只输出 JSON。"
                ));
            }
        }
    }
    let Some(resolutions) = resolved else {
        // 机械失败 → 立刻放弃本次合并，重派原 worker（带冲突说明）。
        warn!(
            issue = %issue.number,
            "[ConflictResolver] 硬解 {RESOLVE_MAX_ATTEMPTS} 轮未产出可应用方案：{last_err}"
        );
        redispatch_after_failed_resolve(deps, issue, worker_id, task_id, &last_err).await;
        return;
    };

    // ---- 落盘（commit_resolution；与在途合并互斥）----
    let root = match project_root(&deps, &issue) {
        Some(r) => r,
        None => {
            fallback_freeze(
                &deps,
                &issue,
                &task_id,
                "项目档案目录缺失，硬解产物无处落盘",
            );
            return;
        }
    };
    let overrides = match resolutions
        .iter()
        .map(|r| override_content(r, &conflicts))
        .collect::<Result<Vec<_>, String>>()
    {
        Ok(o) => o,
        Err(e) => {
            // 校验闸已挡住全部已知不可表达形态；此处防御兜底仍走机械失败途。
            warn!(issue = %issue.number, "[ConflictResolver] 硬解产物装配失败：{e}");
            redispatch_after_failed_resolve(deps, issue, worker_id, task_id, &e).await;
            return;
        }
    };
    let reasons = resolutions
        .iter()
        .map(|r| format!("- {}: {}", r.path, r.reason.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    let message = format!(
        "merge: 冲突 AI 硬解（conflict_auto_resolve，task {task_id}）\n\n解题理由：\n{reasons}"
    );
    // E4 合并串行闸：落盘 commit 与在途合并互斥（std Mutex 持锁不跨 await
    // ——本段无 await，同步持锁安全）。
    let commit = {
        let _serial = crate::board_archive_ingest::MERGE_SERIAL
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        nemesis_board::git_repo::commit_resolution(&root, overrides, &message)
    };
    match commit {
        Ok(commit_oid) => {
            info!(
                issue = %issue.number,
                commit = %commit_oid,
                "[ConflictResolver] 冲突硬解已落盘合入"
            );
            let store = deps.store.as_ref();
            crate::board_review::record_auto_decide(
                store,
                issue.id,
                deps.cluster.node_id(),
                "conflict_auto_resolve",
                "resolved",
                serde_json::json!({
                    "task_id": task_id,
                    "commit": commit_oid,
                    "model": agent_loop.active_model(),
                    "resolutions": resolutions.iter().map(|r| serde_json::json!({
                        "path": r.path,
                        "action": match r.action {
                            ResolutionAction::Merge => "merge",
                            ResolutionAction::Ours => "ours",
                            ResolutionAction::Theirs => "theirs",
                        },
                        "reason": r.reason.trim(),
                    })).collect::<Vec<_>>(),
                }),
            );
            crate::board_archive_ingest::finish_merged_review(
                store,
                &issue,
                &format!(
                    "🤖 合并冲突已由 AI 硬解（board.conflict_auto_resolve，commit {}）。\n\n解题理由：\n{reasons}",
                    &commit_oid[..12.min(commit_oid.len())]
                ),
                Some(&root),
            );
        }
        Err(e) => {
            // 落盘失败 = 机械失败（与产出不合规同途）：重派原 worker。
            warn!(issue = %issue.number, "[ConflictResolver] 硬解落盘失败：{e}");
            redispatch_after_failed_resolve(
                deps,
                issue,
                worker_id,
                task_id,
                &format!("硬解产物落盘失败：{e}"),
            )
            .await;
        }
    }
}

/// 机械失败后的重派链：原 worker 三轮主动接触（任一应答照发）→ 三轮全无
/// 换人（D3 决策表）→ 预算保险丝打满回落 human 档。
async fn redispatch_after_failed_resolve(
    deps: crate::board_review::BoardReviewDeps,
    issue: Issue,
    worker_id: String,
    task_id: String,
    failure: &str,
) {
    // estop 检查点：重派链发车前。
    if deps.estop.is_engaged() {
        fallback_freeze(&deps, &issue, &task_id, "急停中，冲突重派挂起转人工");
        return;
    }
    let store = deps.store.as_ref();
    // E1 预算保险丝：打满转人工（兜底，非常态）。
    match crate::board_review::load_board_flags(&deps.home) {
        Ok(flags) => {
            if let Some(breach) = crate::board_review::budget_breach(&deps, &issue, &flags) {
                fallback_freeze(
                    &deps,
                    &issue,
                    &task_id,
                    &format!("冲突硬解失败且重派预算超限（{breach}），转人工"),
                );
                return;
            }
        }
        Err(e) => {
            fallback_freeze(
                &deps,
                &issue,
                &task_id,
                &format!("board 旗标读取失败（{e}），冲突处置转人工"),
            );
            return;
        }
    }
    let conflict_note = format!(
        "⚠ 上次交付与仓库当前内容合并冲突，AI 硬解未能安全处置（{failure}）。\
请基于最新基线重新实现本单改动（你的工作副本上下文仍在，按你当时的思路改到新基线上），\
完成后照常回报。",
    );

    // 三轮主动接触原 worker（t0/+60s/+120s；帧级真探）。
    let mut online = false;
    for (i, &at) in PROBE_SCHEDULE_SECS.iter().enumerate() {
        if i > 0 {
            let wait = at - crate::conflict_resolver::PROBE_SCHEDULE_SECS[i - 1];
            tokio::time::sleep(Duration::from_secs(wait)).await;
            // 接触等待中途触发急停 → 停车等释放（不排队发车）。
            if deps.estop.is_engaged() {
                fallback_freeze(&deps, &issue, &task_id, "急停中，冲突重派挂起转人工");
                return;
            }
        }
        if deps.cluster.probe_peer(&worker_id).await {
            info!(
                issue = %issue.number,
                worker = %worker_id,
                at = at,
                "[ConflictResolver] 原 worker 在线（第 {} 轮接触）", i + 1
            );
            online = true;
            break;
        }
    }
    let target = if online {
        crate::board_review::record_auto_decide(
            store,
            issue.id,
            deps.cluster.node_id(),
            "conflict_redispatch",
            "redispatch",
            serde_json::json!({
                "task_id": task_id,
                "worker": worker_id,
                "failure": failure,
            }),
        );
        let _ = store.add_comment(NewComment {
            issue_id: issue.id,
            author: Actor::system("conflict-resolver"),
            content: format!(
                "🔁 合并冲突 AI 硬解未果，已重派原 worker {worker_id}（task {task_id}）。\n\n{conflict_note}"
            ),
            parent_id: None,
            ctype: CommentType::System,
        });
        worker_id.clone()
    } else {
        // 三轮全无 → 换人：探针已证原 worker 不可达，直接选历史未用在线
        // 候选（D3 的「同 worker 连败≥2 才换」规则不适用——接触性失败即
        // 刻换人；用户拍板否决排队等）。无未用候选才回落原 worker（照发，
        // dispatch_issue_core 对不可达目标诚实 fail_task）。
        let dispatches = match store.list_dispatches(issue.id) {
            Ok(d) => d,
            Err(e) => {
                fallback_freeze(
                    &deps,
                    &issue,
                    &task_id,
                    &format!("派发记录读取失败（{e}），转人工"),
                );
                return;
            }
        };
        let historical: std::collections::HashSet<&str> =
            dispatches.iter().map(|d| d.worker_id.as_str()).collect();
        let next = nemesis_web::handlers::board::rank_dispatch_candidates(
            &deps.store,
            &deps.cluster,
            &issue,
        )
        .into_iter()
        .find(|w| !historical.contains(w.as_str()));
        let (switched, target) = match next {
            Some(t) => (true, t),
            None => (false, worker_id.clone()),
        };
        if switched {
            crate::board_review::record_auto_decide(
                store,
                issue.id,
                deps.cluster.node_id(),
                "conflict_switch_worker",
                "redispatch",
                serde_json::json!({
                    "task_id": task_id,
                    "worker": worker_id,
                    "new_target": target,
                    "failure": failure,
                }),
            );
            let _ = store.add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::system("conflict-resolver"),
                content: format!(
                    "🔁 原 worker {worker_id} 三轮接触无应答（t0/+60s/+120s），本单换节点执行 → {target}。原 worker 归来后旧交付按归属校验作废。"
                ),
                parent_id: None,
                ctype: CommentType::System,
            });
        } else {
            crate::board_review::record_auto_decide(
                store,
                issue.id,
                deps.cluster.node_id(),
                "conflict_redispatch",
                "redispatch",
                serde_json::json!({
                    "task_id": task_id,
                    "worker": worker_id,
                    "note": "三轮接触无应答且无次优候选，回落原 worker",
                    "failure": failure,
                }),
            );
        }
        target
    };
    // 发车（新派发 = 新 task_id = 新基线下发；旧变更集由 E8 归属校验作废）。
    match nemesis_web::handlers::board::dispatch_issue_core(
        &deps.store,
        Some(&deps.cluster),
        issue.id,
        &target,
        &Actor::system("conflict-resolver"),
        Some(&conflict_note),
    ) {
        Ok(_) => info!(
            issue = %issue.number,
            target = %target,
            "[ConflictResolver] 冲突重派发车"
        ),
        Err(e) => {
            fallback_freeze(
                &deps,
                &issue,
                &task_id,
                &format!("冲突重派发车失败（{e}），转人工"),
            );
        }
    }
}

/// auto 档彻底失败的兜底：回落 human 档（项目域冻结 + `conflict` 审计 +
/// 停车）。与冲突入口 human 档同出口——「打满转人工」落点。
fn fallback_freeze(
    deps: &crate::board_review::BoardReviewDeps,
    issue: &Issue,
    task_id: &str,
    reason: &str,
) {
    warn!(issue = %issue.number, "[ConflictResolver] 回落 human 档：{reason}");
    let store = deps.store.as_ref();
    if let Some(pid) = issue.project_id
        && let Err(e) = store.set_project_conflict_frozen(pid, true)
    {
        warn!("[ConflictResolver] 项目 {pid} 冻结置位失败: {e}");
    }
    crate::board_review::record_auto_decide(
        store,
        issue.id,
        deps.cluster.node_id(),
        "conflict",
        "parked",
        serde_json::json!({
            "task_id": task_id,
            "mode": "auto_fallback",
            "reason": reason,
        }),
    );
    crate::board_archive_ingest::park_merge(
        store,
        issue.id,
        task_id,
        &format!("AI 硬解未果，项目已冻结待人工解：{reason}"),
    );
}

fn project_root(deps: &crate::board_review::BoardReviewDeps, issue: &Issue) -> Option<PathBuf> {
    issue
        .project_id
        .and_then(|pid| deps.store.get_project(pid).ok())
        .and_then(|p| p.directory)
        .map(PathBuf::from)
}

/// 单条硬解产出 → commit_resolution override 条目（merge 用产出内容；
/// 择边从冲突三阶段取对应侧 blob——AI 永不发明二进制内容）。
fn override_content(
    r: &Resolution,
    conflicts: &[ConflictFile],
) -> Result<(String, Vec<u8>), String> {
    let cf = conflicts
        .iter()
        .find(|f| f.path == r.path)
        .ok_or_else(|| format!("{} 不在冲突集（防御兜底）", r.path))?;
    let content = match &r.action {
        ResolutionAction::Merge => r
            .content
            .clone()
            .ok_or_else(|| format!("{} action=merge 缺 content（防御兜底）", r.path))?,
        ResolutionAction::Ours => cf
            .ours
            .clone()
            .ok_or_else(|| format!("{} 我方侧无文件，择边不可表达", r.path))?,
        ResolutionAction::Theirs => cf
            .theirs
            .clone()
            .ok_or_else(|| format!("{} 对方侧无文件，择边不可表达", r.path))?,
    };
    Ok((r.path.clone(), content))
}

// ---------------------------------------------------------------------------
// prompt + 产出契约
// ---------------------------------------------------------------------------

/// 硬解 system prompt（范围钉死：只允许改冲突文件；以当前仓库内容为主）。
pub(crate) const CONFLICT_RESOLVER_SYSTEM_PROMPT: &str = r#"你是看板项目的合并冲突解决专家。两个改动在同一文件上冲突，你要产出确定性的合并结果。

铁律：
1. 以当前仓库内容为主干，把对方改动尽量并入；不发明双方都没有的内容，不删改无关代码。
2. 只允许处置列出的冲突文件，禁止提及任何其他文件。
3. 二进制文件（图片/编译产物等）只能择边（"ours" 或 "theirs"），绝不发明内容。
4. 锁文件（package-lock.json / Cargo.lock 等）无法安全手工缝合——任何处置的理由里必须包含「建议重新生成」。
5. 无法安全缝合时，宁可择边（保留一方完整内容），不要产出猜测的混合体。

只输出一个 JSON 对象（可包在 ```json 代码块里），不要任何其他文字：
{"resolutions": [{"path": "<冲突文件路径>", "action": "merge|ours|theirs", "content": "<action=merge 时的完整新文件内容>", "reason": "<一句话理由>"}]}

要求：每个冲突文件恰有一条；action=merge 时 content 必须是文件完整新内容（不是 diff、不是片段）；reason 必填。"#;

/// 组装硬解 user prompt（任务意图 + 冲突三阶段内容）。
fn build_user_prompt(issue: &Issue, task_id: &str, conflicts: &[ConflictFile]) -> String {
    let mut p = String::new();
    p.push_str("## 任务上下文\n\n");
    p.push_str(&format!("看板单：{} {}\n", issue.number, issue.title));
    if !issue.description.trim().is_empty() {
        p.push_str(&format!(
            "任务说明（对方 worker 的改动意图）：\n{}\n",
            issue.description.trim()
        ));
    }
    if let Some(ac) = issue
        .acceptance_criteria
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        p.push_str(&format!("验收标准：\n{ac}\n"));
    }
    p.push_str(&format!(
        "\n我方 = 当前仓库内容（含此前其他子单已合并的改动）；对方 = 本单 worker 变更集（task {task_id}）；基线 = worker 开工时的共同祖先。\n\n## 冲突文件\n"
    ));
    for f in conflicts {
        p.push_str(&format!("\n### {}\n", f.path));
        if f.binary {
            let size = |d: &Option<Vec<u8>>| d.as_ref().map(|v| v.len()).unwrap_or(0);
            p.push_str(&format!(
                "（二进制文件：我方 {} 字节 / 对方 {} 字节。只能择边。）\n",
                size(&f.ours),
                size(&f.theirs),
            ));
            continue;
        }
        for (side, data) in [
            ("我方（当前仓库）", &f.ours),
            ("对方（worker 变更集）", &f.theirs),
            ("基线（共同祖先）", &f.ancestor),
        ] {
            let Some(data) = data else {
                p.push_str(&format!("\n{side}：该侧无此文件（新增/删除型冲突）。\n"));
                continue;
            };
            if data.len() > PROMPT_SIDE_CAP_BYTES {
                let head = String::from_utf8_lossy(&data[..PROMPT_SIDE_CAP_BYTES]);
                p.push_str(&format!(
                    "\n{side}（前 {} 字节，共 {} 字节已截断）：\n{head}\n…（截断）\n",
                    PROMPT_SIDE_CAP_BYTES,
                    data.len()
                ));
            } else {
                p.push_str(&format!("\n{side}：\n{}\n", String::from_utf8_lossy(data)));
            }
        }
    }
    p
}

/// 解析 + 校验硬解产出（范围钉死在这里强制：路径必须 ⊆ 冲突集；二进制
/// 只能择边；择边侧必须在场；锁文件理由必须含「建议重新生成」）。
pub(crate) fn parse_resolutions(
    raw: &str,
    conflicts: &[ConflictFile],
) -> Result<Vec<Resolution>, String> {
    // 容错提取 JSON（剥 ``` 围栏 / 前后杂文）：取首个 '{' 到末个 '}'。
    let start = raw.find('{').ok_or("输出中没有 JSON 对象")?;
    let end = raw.rfind('}').ok_or("输出中没有 JSON 对象")?;
    let json_text = raw
        .get(start..=end)
        .ok_or("输出中 JSON 对象提取失败（多字节截断）")?;
    let v: serde_json::Value =
        serde_json::from_str(json_text).map_err(|e| format!("JSON 解析失败: {e}"))?;
    let items = v
        .get("resolutions")
        .and_then(|r| r.as_array())
        .ok_or("缺少 resolutions 数组")?;

    let conflict_paths: HashSet<&str> = conflicts.iter().map(|f| f.path.as_str()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let path = item
            .get("path")
            .and_then(|p| p.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("第 {} 条缺 path", i + 1))?;
        if !conflict_paths.contains(path) {
            return Err(format!(
                "越界产出：{path} 不在冲突文件清单内（只允许改冲突文件）"
            ));
        }
        if !seen.insert(path.to_string()) {
            return Err(format!("重复处置：{path} 出现多条"));
        }
        let action_s = item
            .get("action")
            .and_then(|a| a.as_str())
            .ok_or_else(|| format!("{path} 缺 action"))?;
        let action = ResolutionAction::parse(action_s)
            .ok_or_else(|| format!("{path} action 非法（{action_s}，只允许 merge/ours/theirs）"))?;
        let reason = item
            .get("reason")
            .and_then(|r| r.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{path} 缺 reason"))?;
        if LOCKFILE_NAMES.contains(&path.rsplit('/').next().unwrap_or(path))
            && !reason.contains("建议重新生成")
        {
            return Err(format!(
                "{path} 是锁文件：理由必须包含「建议重新生成」（锁文件正确解法是重新生成）"
            ));
        }
        let cf = conflicts
            .iter()
            .find(|f| f.path == path)
            .expect("path 已验证在冲突集内");
        let content = match (&action, cf.binary) {
            (ResolutionAction::Merge, true) => {
                return Err(format!("{path} 是二进制文件，只能择边（ours/theirs）"));
            }
            (ResolutionAction::Merge, false) => {
                let s = item
                    .get("content")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| format!("{path} action=merge 需要 content（完整新文件内容）"))?;
                Some(s.as_bytes().to_vec())
            }
            (ResolutionAction::Ours, _) => {
                if cf.ours.is_none() {
                    return Err(format!(
                        "{path} 我方侧无此文件（新增/删除型冲突），不能择边 ours；请用 theirs 或 merge"
                    ));
                }
                None
            }
            (ResolutionAction::Theirs, _) => {
                if cf.theirs.is_none() {
                    return Err(format!(
                        "{path} 对方侧无此文件（新增/删除型冲突），不能择边 theirs；请用 ours 或 merge"
                    ));
                }
                None
            }
        };
        out.push(Resolution {
            path: path.to_string(),
            action,
            content,
            reason: reason.to_string(),
        });
    }
    // 覆盖完备闸：漏一个冲突文件就是假绿（未处置的冲突会带病合入）。
    let missing: Vec<String> = conflicts
        .iter()
        .filter(|f| !seen.contains(&f.path))
        .map(|f| f.path.clone())
        .collect();
    if !missing.is_empty() {
        return Err(format!("缺少冲突文件处置：{}", missing.join(", ")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
