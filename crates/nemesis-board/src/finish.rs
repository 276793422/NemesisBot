//! b（⑥ FINISH 谓词）：父单收口前置谓词（计划
//! `can_enter_parent_review`）的单一实现。**两级闸面**：
//!
//! - **入口面** [`evaluate_parent_entry_finish`]——`review_parent_issue`
//!   开闸前的环境干净三条件（③ 活跃派发 / ④ 未决 unknown / ⑤ cancelled
//!   缺口）。①②（终态闭包/fail_class）刻意不在入口：in-flight 子单在场
//!   时缺陷闸（停车泊车）/ review_issue 父单分流 / FAIL 返工承接链必须
//!   照常跑——这是既有收口链测试钉住的行为（零回归合同），谓词在入口挡
//!   ①② 会让 缺陷漏停车、返工链哑火。⑤ 在入口保留旧 cancelled 检查
//!   语义（不烧 LLM token 提前让位）。
//! - **收口面** [`evaluate_parent_finish`]（完整五条件）——web
//!   `sync_one_parent` 全 done 垫升闸 + `review_parent_issue` Pass 臂落
//!   done 前的终检（评审耗时数分钟，子树状态可能变化，落盘前以新鲜子树
//!   重验全部条件，宁阻塞不盲收口）。两处共用同一实现，杜绝口径漂移。
//!
//! 五条件（[`evaluate_parent_finish`] 返回未满足项的人读描述，空 = 全部
//! 满足）：
//! ① 子树闭包全终态——每单 ∈ {done, cancelled}；
//! ② 无未收尾失败单——非 done 单必须有 cancelled 终态或 `fail_class: `
//!    标记行（⛔ 失败评论的结构化分类行，gateway 写回唯一写入点）；注：
//!    在 ① 在场的收口面里 ② 恒空（非终态已被 ① 计），保留以对齐计划公式
//!    与谓词矩阵测试；
//! ③ 无活跃派发——逐单 `get_active_dispatch` = None（queued/running 在途
//!    即阻塞，防「单已落定、新代派发又起飞」的窗口期收口）；
//! ④ 未决 unknown 清单为空或已裁决——`unknown: ` 标记行（c ③
//!    `persist_unknowns` 落盘，system 评论）之后同单存在 admin 评论 =
//!    该批已裁决；agent/system 评论不裁决，admin 评论在标记之前不算
//!    （评论按 id 升序 = 插入序比较，created_at 秒级粒度同秒不可靠）；
//! ⑤ cancelled 缺口闸——子树含 cancelled = 不满足（范围缺口要人裁决；
//!    原 review_parent_issue 的 cancelled 检查并入谓词，语义不变）。
//!
//! 谓词在正常路径恒真（全 done 子树、无在途派发、无未裁决 unknown），
//! 不改变既有收口行为。不满足 → 调用方阻塞 + [`write_finish_gap_comment`]
//! 缺口评论显式列出清单（C- 去重：与最近一条同前缀系统评论逐字比较）。

use crate::assignment::Actor;
use crate::models::{CommentType, Issue, IssueStatus, NewComment};
use crate::store::BoardStore;

/// `unknown: ` 标记行前缀（`persist_unknowns` 落盘形态，bot 侧单一生产者）。
const UNKNOWN_LINE_PREFIX: &str = "unknown: ";
/// `fail_class: ` 标记行前缀（写回失败评论的结构化分类行）。
const FAIL_CLASS_LINE_PREFIX: &str = "fail_class: ";
/// 缺口评论前缀（C- 去重的同款识别锚）。
pub const FINISH_GAP_PREFIX: &str = "⚠ FINISH 收口闸未满足：";

/// 父单收口前置谓词（**收口面**，完整五条件）：对子树（Subtree 闭包，
/// SAN-08）逐条件评估，返回未满足条件的人读描述行（空 = 全部满足，可
/// 收口）。只读不写——落缺口评论由调用方经 [`write_finish_gap_comment`]
/// 决定（评论失败不改变阻塞判定）。
pub fn evaluate_parent_finish(store: &BoardStore, subtree: &[Issue]) -> Vec<String> {
    let mut gaps = closure_gaps(store, subtree);
    gaps.extend(evaluate_parent_entry_finish(store, subtree));
    gaps
}

/// 收口面私有段：① 终态闭包 + ② fail_class 收尾（计划公式前两条）。
fn closure_gaps(store: &BoardStore, subtree: &[Issue]) -> Vec<String> {
    let mut gaps: Vec<String> = Vec::new();

    let fmt_nums = |items: &[&Issue]| -> String {
        items
            .iter()
            .map(|c| format!("{}（{}）", c.number, c.status))
            .collect::<Vec<_>>()
            .join("、")
    };

    // ① 子树闭包全终态。
    let non_terminal: Vec<&Issue> = subtree
        .iter()
        .filter(|c| !matches!(c.status, IssueStatus::Done | IssueStatus::Cancelled))
        .collect();
    if !non_terminal.is_empty() {
        gaps.push(format!("① 子树存在未终态单据：{}", fmt_nums(&non_terminal)));
    }

    // ② 未收尾失败单：非 done 单有 cancelled 终态**或** fail_class 标记行
    //    即收尾（cancelled 视作已收尾，范围缺口归 ⑤ 管）；其余须带
    //    fail_class 标记。评论读失败按无标记处理 = fail-closed，宁阻塞
    //    不盲收口。注：收口面里 ① 在场时 ② 恒空（非终态已被 ① 计），
    //    保留以对齐计划公式与谓词矩阵测试。
    let unwrapped: Vec<String> = non_terminal
        .iter()
        .filter(|c| {
            !store
                .list_comments(c.id)
                .map(|cs| {
                    cs.iter().any(|cm| {
                        cm.content
                            .lines()
                            .any(|l| l.trim().starts_with(FAIL_CLASS_LINE_PREFIX))
                    })
                })
                .unwrap_or(false)
        })
        .map(|c| format!("{}（{}）", c.number, c.status))
        .collect();
    if !unwrapped.is_empty() {
        gaps.push(format!(
            "② 子树存在未收尾失败单（无 cancelled 终态也无 fail_class 标记）：{}",
            unwrapped.join("、")
        ));
    }

    gaps
}

/// 父单收口前置谓词（**入口面**，环境干净三条件）：review_parent_issue
/// 开闸前的 ③④⑤ 检查。①② 刻意不在入口——in-flight 子单在场时缺陷闸
/// （停车泊车）/ 父单分流 / FAIL 返工承接链必须照常跑（零回归合同）；
/// ⑤ 在入口保留旧 cancelled 检查语义（不烧 LLM token 提前让位）。
pub fn evaluate_parent_entry_finish(store: &BoardStore, subtree: &[Issue]) -> Vec<String> {
    let mut gaps: Vec<String> = Vec::new();

    // ③ 无活跃派发：queued/running 在途即阻塞（终态历史派发不算——重派
    //    预算结余由既有 round 语义管，这里只看「还有没有人在干」）。
    let inflight: Vec<String> = subtree
        .iter()
        .filter_map(|c| {
            store
                .get_active_dispatch(c.id)
                .ok()
                .flatten()
                .map(|d| format!("{}（task {} {}）", c.number, d.task_id, d.state))
        })
        .collect();
    if !inflight.is_empty() {
        gaps.push(format!(
            "③ 子树存在活跃派发（等写回或取消后再收口）：{}",
            inflight.join("、")
        ));
    }

    // ④ 未决 unknown 清单：system 评论里的 `unknown: ` 行是待裁决批次，
    //    同单其后出现 admin 评论 = 该批已裁决（人工评论回应即裁决，最小
    //    形态）；agent/system 评论不裁决，admin 评论在标记之前不算。
    let mut pending: Vec<String> = Vec::new();
    for c in subtree {
        let comments = match store.list_comments(c.id) {
            Ok(v) => v,
            Err(_) => continue, // 读失败不造 ④ 假缺口（保守面由调用方兜底）
        };
        let mut issue_pending: Vec<String> = Vec::new();
        for (idx, cm) in comments.iter().enumerate() {
            if cm.author.kind != "system" {
                continue;
            }
            let unknowns: Vec<&str> = cm
                .content
                .lines()
                .map(|l| l.trim())
                .filter_map(|l| l.strip_prefix(UNKNOWN_LINE_PREFIX))
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if unknowns.is_empty() {
                continue;
            }
            let adjudicated = comments[idx + 1..]
                .iter()
                .any(|later| later.author.kind == "admin");
            if !adjudicated {
                issue_pending.push(unknowns.join("；"));
            }
        }
        if !issue_pending.is_empty() {
            pending.push(format!("{}：{}", c.number, issue_pending.join("；")));
        }
    }
    if !pending.is_empty() {
        gaps.push(format!(
            "④ 存在未裁决 unknown 清单（admin 评论回应即裁决）：{}",
            pending.join(" | ")
        ));
    }

    // ⑤ cancelled 缺口闸：子树含 cancelled = 范围缺口需人工裁决。
    let cancelled: Vec<&Issue> = subtree
        .iter()
        .filter(|c| c.status == IssueStatus::Cancelled)
        .collect();
    if !cancelled.is_empty() {
        let listing = cancelled
            .iter()
            .map(|c| format!("{}（{}）", c.number, c.status))
            .collect::<Vec<_>>()
            .join("、");
        gaps.push(format!(
            "⑤ 子树存在 cancelled 缺口（范围缺口需人工裁决）：{}",
            listing
        ));
    }

    gaps
}

/// 单条 issue 的待裁决 unknown 批次（触发点④素材）。
#[derive(Debug, Clone, PartialEq)]
pub struct PendingUnknownBatch {
    pub issue_id: i64,
    pub number: String,
    /// 未裁决批次的 `unknown: ` 条目（跨批聚合，去重保序）。
    pub items: Vec<String>,
    /// 最早一批未裁决 unknown 的 created_at（Unix 秒）——超时年龄按它算。
    pub oldest_created_at: i64,
}

/// 扫描给定 issue 列表中的待裁决 unknown 批次（与
/// [`evaluate_parent_entry_finish`] ④ 段同一裁决语义的**结构化投影**）：
/// system 评论带 `unknown: ` 行 = 待裁决批；同单其后（按评论序）出现
/// admin 评论 = 该批已裁决。返回非空批次列表；读评论失败的 issue 跳过
/// （与谓词同款保守降级，不造假阳性）。
pub fn pending_unknown_batches(store: &BoardStore, issues: &[Issue]) -> Vec<PendingUnknownBatch> {
    let mut out: Vec<PendingUnknownBatch> = Vec::new();
    for c in issues {
        let comments = match store.list_comments(c.id) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mut items: Vec<String> = Vec::new();
        let mut oldest: Option<i64> = None;
        for (idx, cm) in comments.iter().enumerate() {
            if cm.author.kind != "system" {
                continue;
            }
            let unknowns: Vec<&str> = cm
                .content
                .lines()
                .map(|l| l.trim())
                .filter_map(|l| l.strip_prefix(UNKNOWN_LINE_PREFIX))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            if unknowns.is_empty() {
                continue;
            }
            let adjudicated = comments[idx + 1..]
                .iter()
                .any(|later| later.author.kind == "admin");
            if adjudicated {
                continue;
            }
            oldest = Some(oldest.map_or(cm.created_at, |prev| prev.min(cm.created_at)));
            for u in unknowns {
                if !items.iter().any(|x| x == u) {
                    items.push(u.to_string());
                }
            }
        }
        if !items.is_empty() {
            out.push(PendingUnknownBatch {
                issue_id: c.id,
                number: c.number.clone(),
                items,
                oldest_created_at: oldest.unwrap_or_default(),
            });
        }
    }
    out
}

/// FINISH 缺口评论落盘（C- 去重）：与最近一条同前缀系统评论逐字比较，
/// 内容未变则跳过（谓词每轮收口检查都会跑，不去重会刷屏）。清单变化仍会
/// 补写显形。空清单不写。
pub fn write_finish_gap_comment(
    store: &BoardStore,
    issue_id: i64,
    unsatisfied: &[String],
) -> Result<(), String> {
    if unsatisfied.is_empty() {
        return Ok(());
    }
    let content = format!("{}\n- {}", FINISH_GAP_PREFIX, unsatisfied.join("\n- "));
    let duplicated = store
        .list_comments(issue_id)
        .map(|cs| {
            cs.iter()
                .rev()
                .find(|cm| {
                    cm.ctype == CommentType::System && cm.content.starts_with(FINISH_GAP_PREFIX)
                })
                .is_some_and(|cm| cm.content == content)
        })
        .unwrap_or(false);
    if duplicated {
        return Ok(());
    }
    store
        .add_comment(NewComment {
            issue_id,
            author: Actor::system("board"),
            content,
            parent_id: None,
            ctype: CommentType::System,
        })
        .map(|_| ())
}

#[cfg(test)]
mod tests;
