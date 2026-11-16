//! 唤醒计算 + 预算判定 + sweep 纯逻辑（驱动器的全部裁决）。
//!
//! [`sweep_loop`] 是唯一裁决入口：单条 loop 在给定时刻的动作
//! （Wake / Park / None）。宿主（gateway 驱动器）消费返回值执行注入与
//! 落盘——本模块零 IO，全可单测。

use crate::state::{LoopState, LoopStatus};

/// 唤醒消息标记（注入内容前缀——脚本模型/测试按此识别唤醒轮）。
pub const WAKE_MARKER: &str = "[循环:";
/// 唤醒消息引用的进度尾条数。
const PROGRESS_TAKE: usize = 3;

/// 预算耗尽维度名（None = 未耗尽）。
pub fn budget_exhausted(st: &LoopState, now_ms: i64) -> Option<&'static str> {
    let b = &st.budget;
    if b.max_turns_total > 0 && st.spent.turns >= b.max_turns_total {
        return Some("max_turns_total");
    }
    if b.wall_clock_budget_secs > 0 {
        let elapsed = ((now_ms - st.started_at_ms).max(0) / 1000) as u64;
        if elapsed >= b.wall_clock_budget_secs {
            return Some("wall_clock_budget_secs");
        }
    }
    if b.max_tokens > 0 && st.spent.tokens >= b.max_tokens {
        return Some("max_tokens");
    }
    None
}

/// 计算下次唤醒。三态：`delay_secs`（一次性，epoch = now + delay）/
/// `cron`（周期，croner 取下一次发生）/ 两者互斥校验。返回
/// `(next_wake_ms, cron)` ——cron 形态第二元 Some（唤醒后重算依据）。
pub fn compute_next_wake(
    now_ms: i64,
    delay_secs: Option<u64>,
    cron: Option<&str>,
) -> Result<(Option<i64>, Option<String>), String> {
    match (delay_secs, cron) {
        (Some(_), Some(_)) => Err("delay_secs 与 cron 互斥，只能给一个".to_string()),
        (None, None) => Err("缺少 delay_secs 或 cron（必给其一）".to_string()),
        (Some(d), None) => {
            if d == 0 {
                return Err("delay_secs 必须大于 0".to_string());
            }
            Ok((Some(now_ms + (d as i64) * 1000), None))
        }
        (None, Some(expr)) => {
            let next = cron_next_ms(now_ms, expr)?;
            Ok((Some(next), Some(expr.to_string())))
        }
    }
}

/// croner 解析 + 下一次发生（本地时区语义，nemesis-cron 同口径）。
fn cron_next_ms(now_ms: i64, expr: &str) -> Result<i64, String> {
    use std::str::FromStr;
    let cron = croner::Cron::from_str(expr).map_err(|e| format!("cron 表达式无效: {e}"))?;
    let now = chrono::DateTime::from_timestamp_millis(now_ms)
        .map(|dt| dt.with_timezone(&chrono::Local))
        .unwrap_or_else(chrono::Local::now);
    cron.find_next_occurrence(&now, false)
        .map(|dt| dt.with_timezone(&chrono::Local).timestamp_millis())
        .map_err(|e| format!("计算下次唤醒失败: {e}"))
}

/// epoch ms → 人读本地时间（工具回执/WSAPI 展示面）。
pub fn format_wake_time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %Z")
                .to_string()
        })
        .unwrap_or_else(|| format!("<epoch {ms}>"))
}

/// 单条 loop 的 sweep 裁决。返回：
/// - [`SweepOutcome::Wake`]：到期且预算未耗尽——宿主注入消息后必须落盘
///   （spent/next_wake 已在 st 上更新）；
/// - [`SweepOutcome::Park`]：预算耗尽——宿主落盘留痕（status 已置 parked）；
/// - None：无动作（非 active / 未排程 / 未到期 / estop 冻结）。
///
/// estop 冻结语义：true 时恒 None（零动作——wake 留到期，release 后下一
/// 个 sweep 补投；预算停车同样冻结，不借急停窗口偷改状态）。
pub fn sweep_loop(st: &mut LoopState, now_ms: i64, estop: bool) -> Option<SweepOutcome> {
    if estop || st.status != LoopStatus::Active {
        return None;
    }
    // 预算闸优先于唤醒：耗尽即停车（哪怕本无到期唤醒——状态诚实优先）。
    if let Some(dim) = budget_exhausted(st, now_ms) {
        st.status = LoopStatus::Parked;
        st.next_wake_ms = None;
        st.cron = None;
        st.push_progress(
            now_ms,
            format!("[parked] 预算耗尽（{dim}），已停车转人工——resume 前请先调整 budget"),
        );
        return Some(SweepOutcome::Park { dim });
    }
    let due = st.next_wake_ms.map(|w| w <= now_ms).unwrap_or(false);
    if !due {
        return None;
    }
    st.spent.turns += 1;
    // cron 周期 → 重算下次；一次性 → 清排程（agent 须再 loop_wakeup——
    // 自配速核心：下次何时醒来由 agent 自己决定）。
    if let Some(expr) = st.cron.clone() {
        match cron_next_ms(now_ms, &expr) {
            Ok(next) => st.next_wake_ms = Some(next),
            Err(e) => {
                // 表达式失效（历史遗留坏值）→ 诚实停车，不带病排程。
                st.status = LoopStatus::Parked;
                st.next_wake_ms = None;
                st.cron = None;
                st.push_progress(now_ms, format!("[parked] cron 重算失败（{e}），已停车"));
                return Some(SweepOutcome::Park {
                    dim: "cron_invalid",
                });
            }
        }
    } else {
        st.next_wake_ms = None;
    }
    let message = wake_message(st, now_ms);
    Some(SweepOutcome::Wake { message })
}

/// 组装唤醒注入消息（计划钦定文案骨架：目标 + 进度尾部 + 自主决策提示）。
fn wake_message(st: &LoopState, _now_ms: i64) -> String {
    let mut msg = format!("{WAKE_MARKER}{}] 继续你的目标：{}\n\n", st.id, st.goal);
    let tail = st.progress_tail(PROGRESS_TAKE);
    if tail.is_empty() {
        msg.push_str("（尚无进度记录）\n\n");
    } else {
        msg.push_str("进度尾部：\n");
        for e in tail {
            msg.push_str(&format!("- {}\n", e.text));
        }
        msg.push('\n');
    }
    msg.push_str("你来决定继续/调整/收尾：继续则 loop_wakeup 安排下次唤醒（可带 note 记录进度）；完成则 loop_finish 总结收尾。");
    msg
}

/// sweep 裁决结果（宿主消费：注入 + 落盘 / 仅落盘）。
#[derive(Debug, Clone, PartialEq)]
pub enum SweepOutcome {
    /// 注入 message 到绑定会话，然后落盘。
    Wake { message: String },
    /// 仅落盘（status=parked + progress 留痕已写入 st）。
    Park { dim: &'static str },
}
