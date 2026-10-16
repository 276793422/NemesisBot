//! 验收标准自动生成（2026-10-17 用户裁决）：issue 创建时未填验收标准，
//! 系统按标题/背景自动推导判定基准——「我说了有 BUG，验收标准就是 BUG
//! 被解决」，评审不得对着「未提供」装瞎（NB-125 实录：worker 两轮交付
//! 均正确，评审因 AC 缺失连续 UNSURE 无限重派，纯烧 worker 轮次）。
//!
//! 确定性模板、零 LLM：创建路径（WSAPI/CLI/autopilot/planner 子单）全部
//! 经 [`crate::store::BoardStore::create_issue`] 单点生效；planner 子单自带
//! AC 不触发。用户事后可在详情页编辑（issue.update 照常覆盖）。评审侧
//! 对存量缺 AC 单据用同一函数兜底推导（[`crate::review`] prompt 渲染），
//! 口径一致。

/// BUG 信号词（标题+背景拼接后大小写不敏感子串匹配；命中 = BUG 形态，
/// 未命中 = 任务形态）。宁滥勿缺：误命中只是措辞从「目标已达成」换成
/// 「问题已修复」，证据要求两侧相同。
const BUG_SIGNALS: [&str; 18] = [
    "bug",
    "缺陷",
    "错误",
    "报错",
    "崩溃",
    "异常",
    "失败",
    "无法",
    "不能",
    "不生效",
    "不起作用",
    "不对",
    "修复",
    "fix",
    "error",
    "crash",
    "broken",
    "fail",
];

/// 按标题/背景推导验收标准。两者全空 → None（无从推导，维持「未提供」
/// 诚实降级）。产物带「（系统自动生成）」前缀——创建者与评审都能一眼
/// 分辨这是推导判据而非人工逐条写的验收标准。
pub fn generate_acceptance_criteria(title: &str, description: &str) -> Option<String> {
    let title = title.trim();
    let desc = description.trim();
    if title.is_empty() && desc.is_empty() {
        return None;
    }
    let lower = format!("{title}\n{desc}").to_lowercase();
    let bug_shaped = BUG_SIGNALS.iter().any(|s| lower.contains(s));
    Some(if bug_shaped {
        "（系统自动生成）任务标题/背景中报告的问题已被修复并验证：\
         1) 按背景描述的原场景操作，原问题不再出现，相关功能恢复正常；\
         2) 附可重放的客观证据（自动化测试/脚本输出/实测记录）证明修复效果。\
         仅定位或分析问题不算完成——必须有修复落地与验证证据。"
            .to_string()
    } else {
        "（系统自动生成）任务标题/背景中描述的目标已达成并验证：\
         1) 交付物按背景描述可正常使用/运行，行为符合描述预期；\
         2) 附客观证据（测试输出/运行结果/交付物核验）证明。\
         仅分析或给出方案不算完成——必须有可验证的落地结果。"
            .to_string()
    })
}

#[cfg(test)]
mod tests;
