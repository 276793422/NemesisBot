// planner.rs 覆盖率补充测试（PlanParseError Display 83-85 / extract_json_array
// 的 `]` 在前 `[` 在后形态 194）。

use super::*;

/// Display 直接透出 message（83-85）。
#[test]
fn plan_parse_error_display_returns_message() {
    let e = PlanParseError {
        message: "子单超过数量上限".to_string(),
    };
    assert_eq!(format!("{e}"), "子单超过数量上限");
}

/// `]` 出现在 `[` 之前 → 提取失败 →「找不到 JSON 数组」（194）。
#[test]
fn reversed_brackets_fall_to_parse_error() {
    let err = parse_plan("说明文本 ] [ 而已").unwrap_err();
    assert!(
        err.message.contains("找不到 JSON 数组"),
        "实际文案：{}",
        err.message
    );
}

/// build_planner_user_prompt：空描述 / 空验收 / 空经验 / 无画像 → 全部
/// 「（未提供）」降级且不渲染空段（259-270 / 271 / 279-288 的否臂）。
#[test]
fn planner_user_prompt_empty_fields_degrade_honestly() {
    let p = build_planner_user_prompt("父任务", "   ", None, &[], None);
    assert!(p.contains("描述：\n（未提供）"));
    assert!(p.contains("# 整体验收标准\n（未提供）"));
    assert!(!p.contains("团队经验"), "空经验不得渲染经验段");
    assert!(!p.contains("可用执行节点"), "None 画像不得渲染画像段");
    assert!(p.contains("请拆解上述父任务"));
}
