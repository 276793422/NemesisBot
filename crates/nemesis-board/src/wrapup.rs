//! 项目汇报包渲染（计划 §8）：项目收口后机器自动产出单一 markdown
//! 报告——①做了什么 ②未确认规格假设清单（「我替你决定了这些」）
//! ③红队迷路报告（按单号排序）④行为闸结果 ⑤预算消耗 vs 估算（诚实
//! 注记）⑥人工复核清单（「值得你看一眼」，永不阻塞）。
//!
//! 落点三处同源：顶层父单系统评论（8KB 截断）、inbox `report_ready`
//! 通知、dossier 归档 `report.md`（导出核心按项目身份嵌入）。纯 store
//! 聚合渲染（LLM 零参与，不受 estop/tier 闸）；触点事实从触点行 payload
//! 解析（损坏 = 诚实注记，不猜不阻塞）。人工复核清单是标记面**不是
//! 闸门**——收口状态不因清单非空而回退（公理②关门不锁门）。

use std::fmt::Write as _;

use crate::models::{CommentType, Issue, IssueFilter, TouchpointKind, TouchpointStatus};
use crate::store::BoardStore;

/// 表格内备注截断长度（字符）。
const NOTE_TRUNC: usize = 120;
/// 迷路报告摘要截断长度（字符）。
const SUMMARY_TRUNC: usize = 500;

/// 渲染项目汇报包。`usage_rows` = (task_id, worker, input, output)
/// 聚合行——调用方按与 dossier 相同口径从账本聚合（本侧只汇总不查账，
/// 保持纯渲染可测）。Err = 项目不存在 / store 读取失败。
pub fn render_project_report(
    store: &BoardStore,
    project_id: i64,
    usage_rows: &[(String, String, i64, i64)],
) -> Result<String, String> {
    let project = store.get_project(project_id)?;
    let mut issues = store.list_issues(&IssueFilter {
        project_id: Some(project_id),
        ..Default::default()
    })?;
    // 确定性顺序：编号字典序（与 dossier 同纪律——报告可 diff）。
    issues.sort_by(|a, b| a.number.cmp(&b.number).then(a.id.cmp(&b.id)));

    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M");
    let mut out = String::new();
    let _ = writeln!(out, "# 项目汇报包：{}", project.name);
    let _ = writeln!(out, "\n> 生成于 {ts}（系统自动聚合，LLM 零参与）");

    render_done_section(store, &issues, &mut out);
    render_assumptions_section(store, &issues, &mut out);
    render_lost_section(store, &issues, &mut out);
    render_gate_section(store, &issues, &mut out);
    render_budget_section(usage_rows, &mut out);
    render_review_section(store, &issues, &mut out);
    Ok(out)
}

/// 单据的触点行（id 升序——store 契约；读失败 = 空，渲染层诚实缺席，
/// 不因 per-issue 读失败炸整份报告）。
fn issue_touchpoints(store: &BoardStore, issue_id: i64) -> Vec<crate::models::Touchpoint> {
    store
        .list_touchpoints_by_issue(issue_id)
        .unwrap_or_default()
}

/// 最近的 (kind, status) 触点（id 升序 → 反向首个即最新）。
fn latest_tp(
    tps: &[crate::models::Touchpoint],
    kind: TouchpointKind,
    status: TouchpointStatus,
) -> Option<&crate::models::Touchpoint> {
    tps.iter()
        .rev()
        .find(|t| t.kind == kind && t.status == status)
}

// ---------------------------------------------------------------------------
// ① 做了什么
// ---------------------------------------------------------------------------

fn render_done_section(store: &BoardStore, issues: &[Issue], out: &mut String) {
    let _ = writeln!(out, "\n## 一、做了什么\n");
    if issues.is_empty() {
        let _ = writeln!(out, "（项目下无单据）");
        return;
    }
    let _ = writeln!(out, "| 单据 | 状态 | 标题 | 最新系统备注 |");
    let _ = writeln!(out, "|---|---|---|---|");
    for i in issues {
        let note = latest_system_note(store, i.id).unwrap_or_default();
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            i.number,
            i.status.as_str(),
            cell(&i.title),
            cell(&note)
        );
    }
}

/// 最近一条系统评论截断（None = 无系统备注；读失败 = 空白不阻塞表）。
fn latest_system_note(store: &BoardStore, issue_id: i64) -> Option<String> {
    let comments = store.list_comments(issue_id).ok()?;
    comments
        .iter()
        .rev()
        .find(|c| c.ctype == CommentType::System)
        .map(|c| trunc(&c.content, NOTE_TRUNC))
}

// ---------------------------------------------------------------------------
// ② 未确认规格假设清单（「我替你决定了这些」）
// ---------------------------------------------------------------------------

fn render_assumptions_section(store: &BoardStore, issues: &[Issue], out: &mut String) {
    let _ = writeln!(out, "\n## 二、未确认规格假设清单（我替你决定了这些）\n");
    let mut any = false;
    for i in issues {
        let tps = issue_touchpoints(store, i.id);
        // 只有「超时按默认」的规格触点算「我替你决定」——人工确认过的
        // 规格是用户意志，不进替决清单。
        let Some(tp) = latest_tp(
            &tps,
            TouchpointKind::Spec,
            TouchpointStatus::TimeoutDefaulted,
        ) else {
            continue;
        };
        any = true;
        let _ = writeln!(out, "**{} {}**（超时按草案直通发车）", i.number, i.title);
        let assumptions = payload_strings(&tp.payload, "assumptions");
        if assumptions.is_empty() {
            let _ = writeln!(out, "- （蒸馏无显式假设——草案即全部决定）");
        } else {
            for (n, a) in assumptions.iter().enumerate() {
                let _ = writeln!(out, "{}. {}", n + 1, a.trim());
            }
        }
        let _ = writeln!(out);
    }
    if !any {
        let _ = writeln!(out, "无——本项目的规格都经人工确认（或无规格触点）。");
    }
}

// ---------------------------------------------------------------------------
// ③ 红队迷路报告（按单号排序）
// ---------------------------------------------------------------------------

fn render_lost_section(store: &BoardStore, issues: &[Issue], out: &mut String) {
    let _ = writeln!(out, "\n## 三、红队迷路报告\n");
    let mut any = false;
    for i in issues {
        let tps = issue_touchpoints(store, i.id);
        // 最新原型触点（任意状态——迷路叙事以最后一次原型评审为准）。
        let Some(tp) = tps
            .iter()
            .rev()
            .find(|t| t.kind == TouchpointKind::Prototype)
        else {
            continue;
        };
        any = true;
        let _ = writeln!(out, "**{} {}**", i.number, i.title);
        let vetoes = tps
            .iter()
            .filter(|t| t.kind == TouchpointKind::Prototype)
            .filter(|t| {
                matches!(
                    t.status,
                    TouchpointStatus::VetoedLocal | TouchpointStatus::VetoedSpec
                )
            })
            .count();
        if vetoes > 0 {
            let _ = writeln!(
                out,
                "- 否决 {} 次（该卡累计否决计数 {} 轮）",
                vetoes, tp.veto_rounds
            );
        } else {
            let _ = writeln!(out, "- 否决 0 次（一次通过）");
        }
        match payload_str(&tp.payload, "summary") {
            Some(s) if !s.trim().is_empty() => {
                let _ = writeln!(out, "- 评审摘要：{}", trunc(&s, SUMMARY_TRUNC));
            }
            _ => {
                let _ = writeln!(out, "- 评审摘要：触点载荷无摘要（诚实缺席）");
            }
        }
        let _ = writeln!(out);
    }
    if !any {
        let _ = writeln!(
            out,
            "无——本项目无原型触点（红队迷路报告只覆盖原型评审链）。"
        );
    }
}

// ---------------------------------------------------------------------------
// ④ 行为闸结果
// ---------------------------------------------------------------------------

fn render_gate_section(store: &BoardStore, issues: &[Issue], out: &mut String) {
    let _ = writeln!(out, "\n## 四、行为闸结果\n");
    let mut any = false;
    for i in issues {
        let tps = issue_touchpoints(store, i.id);
        if tps.is_empty() {
            continue;
        }
        any = true;
        let mut parts: Vec<String> = Vec::new();
        if let Some(spec) = tps.iter().rev().find(|t| t.kind == TouchpointKind::Spec) {
            let version = payload_str(&spec.payload, "version").unwrap_or_else(|| "v1".to_string());
            if payload_bool(&spec.payload, "distilled") {
                parts.push(format!("规格蒸馏 ✅（{version}）"));
            } else {
                parts.push("规格蒸馏 ⚠ 回退草案".to_string());
            }
        }
        if let Some(proto) = tps
            .iter()
            .rev()
            .find(|t| t.kind == TouchpointKind::Prototype)
        {
            let gate =
                payload_str(&proto.payload, "machine_gate").unwrap_or_else(|| "—".to_string());
            parts.push(format!("原型机器闸 {gate}"));
        }
        let vetoes = tps
            .iter()
            .filter(|t| {
                matches!(
                    t.status,
                    TouchpointStatus::VetoedLocal | TouchpointStatus::VetoedSpec
                )
            })
            .count();
        if vetoes > 0 {
            parts.push(format!("否决 {vetoes} 次"));
        }
        let _ = writeln!(out, "- {} {}：{}", i.number, i.title, parts.join("；"));
    }
    if !any {
        let _ = writeln!(out, "（本项目的单据无行为闸记录——未启用规格/原型触点。）");
    }
}

// ---------------------------------------------------------------------------
// ⑤ 预算消耗 vs 估算
// ---------------------------------------------------------------------------

fn render_budget_section(usage_rows: &[(String, String, i64, i64)], out: &mut String) {
    let _ = writeln!(out, "\n## 五、预算消耗 vs 估算\n");
    if usage_rows.is_empty() {
        let _ = writeln!(out, "（无已记账任务——账本缺席或任务未产生 token 记录。）");
        return;
    }
    let total_in: i64 = usage_rows.iter().map(|(_, _, i, _)| *i).sum();
    let total_out: i64 = usage_rows.iter().map(|(_, _, _, o)| *o).sum();
    let _ = writeln!(
        out,
        "记录到账的任务 {} 个：input {total_in} + output {total_out} = {} tokens（明细见 dossier `usage.csv`）",
        usage_rows.len(),
        total_in + total_out
    );
    // 诚实边界：单据无估算字段（建单不要求携带估算），「估算 vs 实际」
    // 对账缺一半——只列实际，不造估算假象。
    let _ = writeln!(
        out,
        "\n估算对账：单据未携带估算基线，此处只列实际消耗（不造估算假象）。"
    );
}

// ---------------------------------------------------------------------------
// ⑥ 人工复核清单（值得你看一眼——永不阻塞）
// ---------------------------------------------------------------------------

fn render_review_section(store: &BoardStore, issues: &[Issue], out: &mut String) {
    let _ = writeln!(out, "\n## 六、人工复核清单（值得你看一眼——永不阻塞）\n");
    let mut entries: Vec<String> = Vec::new();
    for i in issues {
        let tps = issue_touchpoints(store, i.id);
        for tp in &tps {
            match tp.status {
                TouchpointStatus::TimeoutDefaulted => {
                    let origin = payload_str(&tp.human_input, "origin")
                        .unwrap_or_else(|| "timeout".to_string());
                    entries.push(format!(
                        "- {}：{}触点超时按默认流转（origin={origin}）",
                        i.number,
                        kind_label(tp.kind)
                    ));
                }
                TouchpointStatus::VetoedLocal | TouchpointStatus::VetoedSpec => {
                    entries.push(format!(
                        "- {}：{}触点否决待续（累计 {} 轮）",
                        i.number,
                        kind_label(tp.kind),
                        tp.veto_rounds
                    ));
                }
                TouchpointStatus::ParkedSync => {
                    entries.push(format!(
                        "- {}：{}触点已停车标「需同步沟通」（否决 {} 轮熔断）",
                        i.number,
                        kind_label(tp.kind),
                        tp.veto_rounds
                    ));
                }
                // 等人中/已确认/已取消：不是复核信号面。
                TouchpointStatus::Pending
                | TouchpointStatus::Confirmed
                | TouchpointStatus::Cancelled => {}
            }
        }
        let escalations = store
            .list_activity(i.id)
            .unwrap_or_default()
            .iter()
            .filter(|a| a.action == "auto_decide")
            .filter(|a| {
                a.details
                    .as_deref()
                    .is_some_and(|d| d.contains("escalate_human"))
            })
            .count();
        if escalations > 0 {
            entries.push(format!(
                "- {}：自动决策链 {escalations} 次转人工（escalate_human）",
                i.number
            ));
        }
    }
    if entries.is_empty() {
        let _ = writeln!(out, "无。");
    } else {
        for e in &entries {
            let _ = writeln!(out, "{e}");
        }
    }
    let _ = writeln!(
        out,
        "\n> 以上是标记面，不是闸门：收口状态不因清单非空而回退。"
    );
}

fn kind_label(kind: TouchpointKind) -> &'static str {
    match kind {
        TouchpointKind::Spec => "规格",
        TouchpointKind::Prototype => "原型",
    }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 表格单元格消毒（`|` 与换行会破表）。
fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// rune 安全截断（先 trim 再按字符数截——中文/emoji 不劈半）。
fn trunc(s: &str, max_chars: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max_chars {
        return t.to_string();
    }
    let cut: String = t.chars().take(max_chars).collect();
    format!("{}…", cut.trim_end())
}

/// payload JSON 顶层字符串字段。
fn payload_str(payload: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// payload JSON 顶层布尔字段（缺失 = false）。
fn payload_bool(payload: &str, key: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|v| v.get(key).and_then(|x| x.as_bool()))
        .unwrap_or(false)
}

/// payload JSON 顶层字符串数组字段。
fn payload_strings(payload: &str, key: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|v| v.get(key).and_then(|x| x.as_array()).cloned())
        .map(|a| {
            a.into_iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
