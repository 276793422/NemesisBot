//! M3（2026-10-10 机制加固）：同因失败签名熔断账本。
//!
//! unlimited_mode 放大的是「重派次数不设限」，不是「无收敛重派授权」：
//! 同一失败签名（排序失败锚点 raw 集合；无锚点 = `fail:no-anchors`）连续
//! ≥[`FAIL_STREAK_THRESHOLD`] 次 → 停止自动重派、转人工（评论留痕），
//! **优先于 unlimited_mode**。签名翻转即清零重计（worker 换了病就不算旧
//! 账）；非重派终态（自动收货/待人工/转人工）与 estop 停车清账（世界被
//! 外力改变，旧计数作废）。
//!
//! 持久化：`<workspace>/board/fail_streaks.json`（gateway 重启不丢账）。
//! 文件缺失/损坏 = 诚实从空账本起（WARN 留痕），不为记账阻塞决策链。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 熔断阈值：同一 issue 连续同签名失败达到该次数即停止自动重派。
pub(crate) const FAIL_STREAK_THRESHOLD: u32 = 3;

/// 单 issue 的在账条目。
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct StreakEntry {
    signature: String,
    count: u32,
}

/// 磁盘形态（issue_id → 条目；JSON 对象键必须是字符串）。
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct LedgerFile {
    issues: HashMap<String, StreakEntry>,
}

/// [`FailStreakLedger::record_and_check`] 的结局。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StreakOutcome {
    /// 未达阈值（携带当前连续次数，1 起）。
    Continue(u32),
    /// 达到阈值——调用方必须停止自动重派转人工。
    Tripped(u32),
}

/// 同因失败签名账本（随 workspace 走，按次落盘）。
pub(crate) struct FailStreakLedger {
    path: PathBuf,
    inner: LedgerFile,
}

/// 失败签名（M3）：排序失败锚点 raw 集合 join；无失败锚点（LLM 语义评审
/// FAIL 路径）= `fail:no-anchors`——连续多次无客观信号的 FAIL 同样收敛到
/// 转人工，这是无锚点单据在 unlimited 下唯一的收敛闸。
pub(crate) fn failure_signature(anchor_results: &[nemesis_board::AnchorResult]) -> String {
    let mut failed: Vec<&str> = anchor_results
        .iter()
        .filter(|r| !r.passed)
        .map(|r| r.anchor.raw.as_str())
        .collect();
    if failed.is_empty() {
        return "fail:no-anchors".to_string();
    }
    failed.sort_unstable();
    failed.join("|")
}

impl FailStreakLedger {
    /// 从 `<workspace>/board/fail_streaks.json` 加载；缺失/损坏 = 空账本。
    pub(crate) fn load(workspace: &Path) -> Self {
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
    pub(crate) fn record_and_check(&mut self, issue_id: i64, signature: &str) -> StreakOutcome {
        let entry = self
            .inner
            .issues
            .entry(issue_id.to_string())
            .or_insert_with(|| StreakEntry {
                signature: signature.to_string(),
                count: 0,
            });
        if entry.signature != signature {
            // 签名翻转：worker 换了失败形态，旧账清零重计。
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

    /// 清账（非重派终态 / estop 停车 / 人工介入）。
    pub(crate) fn clear(&mut self, issue_id: i64) {
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
#[path = "board_fail_streak_tests.rs"]
mod tests;
