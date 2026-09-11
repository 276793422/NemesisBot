//! [`super::parse_delivery_report`] 单测：五段全齐 / 风险段与经验段缺失 /
//! 核心段缺失降级 / 前后散文从宽 / 前缀歧义（「## 交付物清单」含
//! 「## 交付物」前缀）不误切。

use super::*;

#[test]
fn parses_full_five_section_report() {
    let text = REPORT_FORMAT_SECTION.to_string();
    let r = parse_delivery_report(&text).expect("canonical format parses");
    assert_eq!(r.conclusion, "（一句话：完成 / 部分完成 / 失败）");
    assert_eq!(
        r.deliverables,
        "（branch、commits、改动/新建文件路径，逐条列出；没有则写\"无\"）"
    );
    assert_eq!(r.self_check, "（对照上面的验收标准逐条自检）");
    assert_eq!(r.risks, "（没有则写\"无\"）");
    assert_eq!(
        r.experience,
        "（本次踩过的坑/可复用的做法，写清适用范围；没有则写\"无\"）"
    );
}

#[test]
fn parses_filled_report_with_prose_around() {
    let text = "\
好的，任务完成，以下是我的汇报：

## 结论
完成。修复了登录超时的 bug。

## 交付物清单
- branch: fix/login-timeout
- commits: abc1234
- crates/auth/src/session.rs（修改）

## 自检结果
1. 超时用例通过 ✅
2. 回归 30 分钟无超时 ✅

## 风险与未尽事项
旧 session 未迁移，会强制重新登录。

（以上）";
    let r = parse_delivery_report(text).expect("filled report parses");
    assert_eq!(r.conclusion, "完成。修复了登录超时的 bug。");
    assert!(r.deliverables.contains("fix/login-timeout"));
    assert!(r.deliverables.contains("crates/auth/src/session.rs"));
    assert!(r.self_check.contains("回归 30 分钟无超时"));
    // 尾部散文并入末段（从宽——无法区分正文与人格后缀）。
    assert!(r.risks.starts_with("旧 session 未迁移，会强制重新登录。"));
}

#[test]
fn missing_optional_risks_section_is_empty_string() {
    let text = "\
## 结论
部分完成。

## 交付物清单
- docs/a.md（新建）

## 自检结果
两条验收过一条。";
    let r = parse_delivery_report(text).expect("three core sections suffice");
    assert_eq!(r.risks, "");
    assert_eq!(r.conclusion, "部分完成。");
}

#[test]
fn missing_experience_section_is_empty_string_backward_compatible() {
    // 旧四段汇报（M4.5 之前的 worker 输出）照常解析，experience 空串。
    let text = "\
## 结论
完成。

## 交付物清单
- a.rs

## 自检结果
ok

## 风险与未尽事项
无";
    let r = parse_delivery_report(text).expect("four-section report still parses");
    assert_eq!(r.experience, "");
}

#[test]
fn experience_section_parsed_with_content() {
    let text = "\
## 结论
完成。

## 交付物清单
无

## 自检结果
ok

## 风险与未尽事项
无

## 经验与坑
- 坑（rust）：改 trait 后必须 touch main.rs 强制重链，否则 cargo 可能跳过重链接。";
    let r = parse_delivery_report(text).expect("parses");
    assert!(r.experience.contains("touch main.rs"));
    // 经验段不吞尾部散文之外的内容：最后一段吸收尾部。
}

#[test]
fn missing_any_core_section_degrades_to_none() {
    // 缺结论。
    assert!(parse_delivery_report("## 交付物清单\nx\n## 自检结果\ny").is_none());
    // 缺交付物清单。
    assert!(parse_delivery_report("## 结论\nx\n## 自检结果\ny").is_none());
    // 缺自检结果。
    assert!(parse_delivery_report("## 结论\nx\n## 交付物清单\ny").is_none());
    // 全没有。
    assert!(parse_delivery_report("任务做完了，都挺好。").is_none());
    assert!(parse_delivery_report("").is_none());
}

#[test]
fn section_split_does_not_confuse_prefix_headers() {
    // 正文里出现「## 交付物」子串（不是词表标题）时按完整标题切分，
    // 「结论」段不被误切短。
    let text = "\
## 结论
见下方 ## 交付物 一节说明，结论：完成。

## 交付物清单
- a.rs

## 自检结果
ok";
    let r = parse_delivery_report(text).expect("parses");
    assert_eq!(r.conclusion, "见下方 ## 交付物 一节说明，结论：完成。");
    assert_eq!(r.deliverables, "- a.rs");
}

#[test]
fn duplicated_header_keeps_first_occurrence_section() {
    let text = "\
## 结论
第一次结论。

## 交付物清单
- a.rs

## 交付物清单
- 重复段

## 自检结果
ok";
    let r = parse_delivery_report(text).expect("parses");
    assert_eq!(r.deliverables, "- a.rs", "first occurrence wins");
}

#[test]
fn delivery_report_serde_roundtrip() {
    let r = DeliveryReport {
        conclusion: "完成".to_string(),
        deliverables: "无".to_string(),
        self_check: "全部通过".to_string(),
        risks: String::new(),
        experience: "坑（db）：WAL 并发读安全".to_string(),
    };
    let json = serde_json::to_string(&r).unwrap();
    let back: DeliveryReport = serde_json::from_str(&json).unwrap();
    assert_eq!(back, r);
    // 旧 JSON（无 experience 键）反序列化宽容为空串。
    let legacy = serde_json::from_str::<DeliveryReport>(
        r#"{"conclusion":"完成","deliverables":"无","self_check":"ok","risks":""}"#,
    )
    .expect("legacy json parses");
    assert_eq!(legacy.experience, "");
}
