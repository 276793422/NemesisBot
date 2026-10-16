//! ac_gen 单测：BUG/任务形态判别 + 空输入诚实降级。

use super::*;

#[test]
fn bug_shaped_title_generates_fix_and_evidence_criteria() {
    // NB-125 原单形态：标题陈述故障现象。
    let ac = generate_acceptance_criteria(
        "项目代码无法正确启动，能看到初始界面无法看到游戏主界面",
        "项目代码无法正确启动。你自己打开游戏看看。",
    )
    .expect("bug title should generate");
    assert!(ac.contains("（系统自动生成）"));
    assert!(ac.contains("已被修复并验证"), "{ac}");
    assert!(ac.contains("原问题不再出现"), "{ac}");
    assert!(ac.contains("可重放的客观证据"), "{ac}");
    // 用户裁决核心：分析完不算验收。
    assert!(ac.contains("仅定位或分析问题不算完成"), "{ac}");
}

#[test]
fn task_shaped_generates_goal_criteria() {
    let ac = generate_acceptance_criteria("编写登录页 PRD", "输出保守范围变体，含验收清单")
        .expect("task title should generate");
    assert!(ac.contains("目标已达成并验证"), "{ac}");
    assert!(ac.contains("仅分析或给出方案不算完成"), "{ac}");
    assert!(!ac.contains("已被修复"), "{ac}");
}

#[test]
fn bug_signal_matches_case_insensitive_and_desc_only() {
    // 信号在背景里、标题干净；英文大小写不敏感。
    let ac = generate_acceptance_criteria("会员中心", "用户反馈 Redeem 按钮点击后报 error 无响应")
        .expect("desc bug signal should generate");
    assert!(ac.contains("已被修复并验证"), "{ac}");
}

#[test]
fn empty_title_and_description_degrade_to_none() {
    assert!(generate_acceptance_criteria("", "").is_none());
    assert!(generate_acceptance_criteria("   ", "\n\t ").is_none());
}
