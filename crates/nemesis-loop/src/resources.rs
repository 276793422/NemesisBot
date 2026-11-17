//! 会话资源盘点 provider——durable_loops（资源看板类目 #15「目标」）。
//!
//! 设计真相源：`docs/PLAN/2026-11-17_会话资源盘点看板计划.md` §3.4。
//! 数据源 = `<workspace>/loops/{id}.json`（store 真相源），collect 现场读盘
//! 按 `session_key` 过滤——loops 是文件存储，provider 无需任何注入状态
//! （构造零参，装配点更干净）。⚠ loops 自带 `cron` 字段（自配速唤醒）与
//! cron 服务的定时器是两个子系统，看板分开呈现（计划 §二 #15 注记）。

use crate::store::{list_loops, loops_dir};
use nemesis_types::resources::{CategoryView, GlobalResourceProvider, ProviderMeta, ResourceCtx};
use serde_json::json;

/// 目标（durable loops）provider——零状态，读盘即所得。
pub struct DurableLoopsResourceProvider;

impl DurableLoopsResourceProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DurableLoopsResourceProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalResourceProvider for DurableLoopsResourceProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("durable_loops", "目标", 80, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let dir = loops_dir(ctx.workspace_root);
        if !dir.is_dir() {
            // 目录不存在 = 该 workspace 从未建过 loop——合法无。
            return CategoryView::empty();
        }
        // list_loops：按 id 字典序；坏文件 warn+跳过（单条损坏不炸整表），
        // 无 Result——读盘路径没有 Unavailable 分支。
        let loops = list_loops(&dir);
        let mut rows = Vec::new();
        let mut total = 0usize;
        let mut active = 0usize;
        for lp in loops {
            if lp.session_key != ctx.session_key {
                continue;
            }
            total += 1;
            // active 对**全部**匹配条目统计（rows 只是展示截断窗——截断后
            // 再数会把窗口外活跃目标漏成 0，≥6 条时计数失真）。
            if !lp.status.is_terminal() {
                active += 1;
            }
            if rows.len() < 5 {
                rows.push(json!({
                    "id": lp.id,
                    "goal": lp.goal,
                    "status": lp.status.as_str(),
                    "finished": lp.status.is_terminal(),
                    "cron": lp.cron,
                    "next_wake_ms": lp.next_wake_ms,
                    "spent_turns": lp.spent.turns,
                    "budget_max_turns_total": lp.budget.max_turns_total,
                    "finish_summary": lp.finish_summary,
                }));
            }
        }
        if total == 0 {
            return CategoryView::empty();
        }
        CategoryView::ok(
            total,
            json!({ "loops": rows, "omitted": total.saturating_sub(rows.len()), "active": active }),
        )
    }
}
