//! M3（2026-10-10 机制加固）/ §6.4（2026-10-12 加固）：同因失败签名熔断账本。
//!
//! unlimited_mode 放大的是「重派次数不设限」，不是「无收敛重派授权」：
//! 同一失败签名（`{verdict}|{排序失败锚点 raw 集合}`；无失败锚点 =
//! `{verdict}|no-anchors`）连续 ≥[`FAIL_STREAK_THRESHOLD`] 次 → 停止自动
//! 重派、转人工（评论留痕），**优先于 unlimited_mode**。签名翻转即清零
//! 重计（worker 换了病就不算旧账）；非重派终态（自动收货/待人工/转人工）
//! 与 estop 停车清账（世界被外力改变，旧计数作废）。
//!
//! §6.4A 签名带 verdict 维度：锚点全过/无锚点时 FAIL 与 UNSURE 不再坍缩成
//! 同一签名（「缺浏览器证据」「守卫拦截」「已补证据仍不确定」本就是不同
//! 因，verdict 是评审产出的最权威因分类；FAIL=有差距可修 vs
//! UNSURE=无证据可判，处置路径本就不同）。存量账本走既有「签名翻转清零」
//! 机制天然兼容（旧格式签名遇新格式 = 翻转清零重计），零迁移。
//!
//! 持久化：`<workspace>/board/fail_streaks.json`（gateway 重启不丢账）。
//! 文件缺失/损坏 = 诚实从空账本起（WARN 留痕），不为记账阻塞决策链。
//!
//! 本模块住在 nemesis-board（§6.4D）：评审链（nemesisbot board_review）与
//! WSAPI 手动重派清账（nemesis-web issue_dispatch）两个消费方共用同一
//! JSON 文件，跨 crate 公开避免 nemesisbot 内部模块被 web 层反向依赖。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::anchor::AnchorResult;
use crate::review::ReviewVerdict;

/// 熔断阈值：同一 issue 连续同签名失败达到该次数即停止自动重派。
pub const FAIL_STREAK_THRESHOLD: u32 = 3;

/// UNSURE 恒定签名：锚点 FAIL 已短路成 FAIL verdict，UNSURE 只在锚点全过
/// 或无锚点时出现 → 失败锚点集恒空（§6.4B 独立收敛闸的判定键）。
pub const UNSURE_SIGNATURE: &str = "unsure|no-anchors";

/// 单 issue 的在账条目。
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct StreakEntry {
    pub signature: String,
    pub count: u32,
}

/// 磁盘形态（issue_id → 条目；JSON 对象键必须是字符串）。
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct LedgerFile {
    issues: HashMap<String, StreakEntry>,
}

/// [`FailStreakLedger::record_and_check`] 的结局。
#[derive(Debug, PartialEq, Eq)]
pub enum StreakOutcome {
    /// 未达阈值（携带当前连续次数，1 起）。
    Continue(u32),
    /// 达到阈值——调用方必须停止自动重派转人工。
    Tripped(u32),
}

/// 同因失败签名账本（随 workspace 走，按次落盘）。
pub struct FailStreakLedger {
    path: PathBuf,
    inner: LedgerFile,
}

/// 失败签名（M3 + §6.4A）：`{verdict 小写}|{排序失败锚点 raw 集合 join}`；
/// 无失败锚点（锚点全过/无锚点）= `{verdict 小写}|no-anchors`。
pub fn failure_signature(anchor_results: &[AnchorResult], verdict: &ReviewVerdict) -> String {
    let mut failed: Vec<&str> = anchor_results
        .iter()
        .filter(|r| !r.passed)
        .map(|r| r.anchor.raw.as_str())
        .collect();
    let anchor_part = if failed.is_empty() {
        "no-anchors".to_string()
    } else {
        failed.sort_unstable();
        failed.join("|")
    };
    format!("{}|{}", verdict.as_str().to_ascii_lowercase(), anchor_part)
}

/// §6.4B UNSURE 独立收敛闸判定（纯函数）：verdict 为 UNSURE 且阈值 > 0 且
/// 记账后连续次数已达阈值 → 转人工（给一次 worker 自愈机会，超出即认定
/// 「只有人能解」，NB-8 第 4 轮实证）。`max_unsure = 0` = 不限（完全旧行
/// 为）；FAIL 轮不受此闸（M3 熔断管辖）；`Tripped` 结局由 M3 熔断先行处理
/// （调用方先查 Tripped 再查本闸——默认阈值 2 < M3 阈值 3，UNSURE 场景
/// 本闸先停）。
pub fn unsure_gate_trips(
    verdict: &ReviewVerdict,
    outcome: &StreakOutcome,
    max_unsure: u32,
) -> bool {
    if *verdict != ReviewVerdict::Unsure || max_unsure == 0 {
        return false;
    }
    matches!(outcome, StreakOutcome::Continue(n) if *n >= max_unsure)
}

impl FailStreakLedger {
    /// 从 `<workspace>/board/fail_streaks.json` 加载；缺失/损坏 = 空账本。
    pub fn load(workspace: &Path) -> Self {
        let path = workspace.join("board").join("fail_streaks.json");
        let inner = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| {
                let parsed: LedgerFile = serde_json::from_str(&s).ok()?;
                Some(parsed)
            })
            .unwrap_or_default();
        Self { path, inner }
    }

    /// 记一笔失败并判定。同签名 → 次数 +1；签名翻转 → 清零重计为 1。
    pub fn record_and_check(&mut self, issue_id: i64, signature: &str) -> StreakOutcome {
        let entry = self
            .inner
            .issues
            .entry(issue_id.to_string())
            .or_insert_with(|| StreakEntry {
                signature: signature.to_string(),
                count: 0,
            });
        if entry.signature != signature {
            // 签名翻转：worker 换了失败形态（含 §6.4A 旧格式存量条目遇新
            // 格式签名），旧账清零重计。
            entry.signature = signature.to_string();
            entry.count = 0;
        }
        entry.count += 1;
        let count = entry.count;
        self.save();
        if count >= FAIL_STREAK_THRESHOLD {
            StreakOutcome::Tripped(count)
        } else {
            StreakOutcome::Continue(count)
        }
    }

    /// 清账（非重派终态 / estop 停车 / 人工介入 / 手动重派）。
    pub fn clear(&mut self, issue_id: i64) {
        if self.inner.issues.remove(&issue_id.to_string()).is_some() {
            self.save();
        }
    }

    /// 落盘；失败只 WARN 不炸决策链（记账是护栏不是主路径）。
    fn save(&self) {
        if let Some(dir) = self.path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("[BoardReview] fail_streaks 目录创建失败（账本未持久化）：{e}");
            return;
        }
        match serde_json::to_string_pretty(&self.inner) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.path, json) {
                    tracing::warn!("[BoardReview] fail_streaks.json 写盘失败：{e}");
                }
            }
            Err(e) => tracing::warn!("[BoardReview] fail_streaks.json 序列化失败：{e}"),
        }
    }
}

#[cfg(test)]
#[path = "fail_streak/tests.rs"]
mod tests;
