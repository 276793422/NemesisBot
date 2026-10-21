//! A4 `security_calibrate` 单测：样本归一/过滤、judge 映射、报告统计。
//! （纯函数面——管线重放与 LLM 调用路径不在此测：管线行为已有
//! nemesis-security 全套单测，judge 请求形态与 GatewayLlmJudge 同源。）

use super::*;

// actual 归一：大小写 + 常见同义词；其他值原样（由收集侧过滤）。
#[test]
fn normalize_actual_maps_synonyms() {
    assert_eq!(normalize_actual("block"), "block");
    assert_eq!(normalize_actual(" BLOCK "), "block");
    assert_eq!(normalize_actual("Blocked"), "block");
    assert_eq!(normalize_actual("deny"), "block");
    assert_eq!(normalize_actual("denied"), "block");
    assert_eq!(normalize_actual("allow"), "allow");
    assert_eq!(normalize_actual(" Allowed "), "allow");
    assert_eq!(normalize_actual("pass"), "allow");
    // 未知值不猜——原样返回，交给收集侧跳过。
    assert_eq!(normalize_actual("maybe"), "maybe");
    assert_eq!(normalize_actual(""), "");
}

// 样本收集：行号保真（跳行存在时报告「行」= 文件真实行号）+ 脏行计数。
#[test]
fn parse_samples_preserves_file_line_numbers() {
    let input = r#"{"tool":"exec","args":{"cmd":"rm"},"actual":"block"}
# comment line
{"tool":"exec","args":{},"actual":"maybe"}
{"tool":"write_file","args":{},"actual":"Allow"}
"#;
    let (samples, skipped) = parse_samples(input.as_bytes()).unwrap();
    // 第 1、4 行合法；第 2 行注释（不计）；第 3 行脏标注（计 1）。
    // 注意空行不存在于此样例——行号断言锚定「合法样本的 src_line =
    // 文件行号」，与跳行后的向量下标（1、2）刻意不同。
    assert_eq!(samples.len(), 2);
    assert_eq!(skipped, 1);
    assert_eq!(samples[0].src_line, 1);
    assert_eq!(samples[1].src_line, 4);
    // 归一在收集侧完成。
    assert_eq!(samples[1].actual, "allow");
}

// JudgeVerdict → 二值映射：deny/allow 归一，ask 与未知值单列。
#[test]
fn map_verdict_maps_recommendations() {
    let v = |rec: &str| JudgeVerdict {
        intent: "测试意图".into(),
        matches_rules: false,
        risk_level: "medium".into(),
        recommendation: rec.into(),
        rationale: "测试理由".into(),
    };
    assert_eq!(map_verdict(&v("deny")).mapped, "block");
    assert_eq!(map_verdict(&v("DENY")).mapped, "block");
    assert_eq!(map_verdict(&v("allow")).mapped, "allow");
    // ask = 交人工，不猜 → 单列。
    assert_eq!(map_verdict(&v("ask")).mapped, "excluded");
    assert_eq!(map_verdict(&v("")).mapped, "excluded");
    assert_eq!(map_verdict(&v("whatever")).mapped, "excluded");
    // 原样保留 raw，报告可回查。
    assert_eq!(map_verdict(&v("ask")).raw_recommendation, "ask");
}

// 混淆矩阵统计与 precision/recall/agreement 的闭式核对。
#[test]
fn report_confusion_matrix_closed_form() {
    let mut r = CalibrateReport {
        generated_at: "t".into(),
        total_samples: 6,
        skipped_lines: 1,
        rule_true_positive: 2,
        rule_false_positive: 1,
        rule_true_negative: 2,
        rule_false_negative: 1,
        judge: None,
        samples: vec![],
    };
    // precision = 2/(2+1) = 0.667；recall = 2/(2+1) = 0.667；
    // agreement = 4/6 = 0.667。
    let p = r.rule_precision().unwrap();
    let rec = r.rule_recall().unwrap();
    let a = r.rule_agreement().unwrap();
    assert!((p - 2.0 / 3.0).abs() < 1e-9);
    assert!((rec - 2.0 / 3.0).abs() < 1e-9);
    assert!((a - 4.0 / 6.0).abs() < 1e-9);

    // 分母 0 → None（不除零、不造 0.0 假象）。
    r.rule_true_positive = 0;
    r.rule_false_positive = 0;
    assert!(r.rule_precision().is_none());
    r.rule_true_negative = 0;
    r.rule_false_negative = 0;
    assert!(r.rule_agreement().is_none());
}

// judge 一致率：excluded（ask/失败）不进分母。
#[test]
fn judge_agreement_excludes_from_denominator() {
    let j = JudgeAgreement {
        both_block: 3,
        both_allow: 1,
        rule_stricter: 1,
        judge_stricter: 0,
        excluded: 5, // ask/parse_error 等——单列
    };
    // 分母 = 3+1+1+0 = 5（不含 excluded 5）；一致率 = 4/5。
    let rate = j.agreement_rate().unwrap();
    assert!((rate - 4.0 / 5.0).abs() < 1e-9);

    // 全单列 → None（无可比面，不造 100% 假象）。
    let empty = JudgeAgreement {
        both_block: 0,
        both_allow: 0,
        rule_stricter: 0,
        judge_stricter: 0,
        excluded: 3,
    };
    assert!(empty.agreement_rate().is_none());
}

// Markdown 渲染：混淆矩阵四格 + judge 段 + 逐样本明细都出现。
#[test]
fn markdown_renders_all_sections() {
    let r = CalibrateReport {
        generated_at: "2026-10-21T00:00:00+08:00".into(),
        total_samples: 2,
        skipped_lines: 0,
        rule_true_positive: 1,
        rule_false_positive: 0,
        rule_true_negative: 1,
        rule_false_negative: 0,
        judge: Some(JudgeAgreement {
            both_block: 1,
            both_allow: 1,
            rule_stricter: 0,
            judge_stricter: 0,
            excluded: 0,
        }),
        samples: vec![SampleOutcome {
            line: 1,
            tool: "exec".into(),
            actual: "block".into(),
            note: "含|竖线".into(),
            rule: RuleOutcome {
                blocked: true,
                approval_required: false,
                layer: Some("command".into()),
                policy: Some("cmd_guard".into()),
                summary: Some("危险命令".into()),
            },
            judge: Some(JudgeOutcome {
                raw_recommendation: "deny".into(),
                mapped: "block".into(),
                intent: "删库".into(),
                rationale: "高危".into(),
                error: None,
            }),
        }],
    };
    let md = render_markdown(&r);
    assert!(md.contains("规则层 vs 人工标注"));
    assert!(md.contains("TP = 1"));
    assert!(md.contains("规则层 × guardian 二审"));
    assert!(md.contains("逐样本明细"));
    assert!(md.contains("cmd_guard"));
    // 竖线转义（不破表格）。
    assert!(md.contains("含\\|竖线"));
}

// 三轮回查：规则「需审批」（ask）在明细中以 ask 标出、不与硬拦混同；
// 矩阵口径行出现在报告里（ask 按 block 计入的立场明示）。
#[test]
fn markdown_marks_approval_required_as_ask() {
    let r = CalibrateReport {
        generated_at: "t".into(),
        total_samples: 2,
        skipped_lines: 0,
        rule_true_positive: 1, // ask 样本按 block 口径计入 TP
        rule_false_positive: 0,
        rule_true_negative: 1,
        rule_false_negative: 0,
        judge: None,
        samples: vec![
            SampleOutcome {
                line: 1,
                tool: "process_kill".into(),
                actual: "block".into(),
                note: String::new(),
                rule: RuleOutcome {
                    blocked: true,
                    approval_required: true,
                    layer: Some("abac".into()),
                    policy: Some("abac".into()),
                    summary: Some("approval required: ... (request ID: x)".into()),
                },
                judge: None,
            },
            SampleOutcome {
                line: 2,
                tool: "exec".into(),
                actual: "block".into(),
                note: String::new(),
                rule: RuleOutcome {
                    blocked: true,
                    approval_required: false,
                    layer: Some("command".into()),
                    policy: Some("cmd_guard".into()),
                    summary: Some("危险命令".into()),
                },
                judge: None,
            },
        ],
    };
    let md = render_markdown(&r);
    // ask 样本标出、硬拦保持 block 标记。
    assert!(md.contains("ask（需审批）"), "{md}");
    assert!(md.contains("block（cmd_guard）"), "{md}");
    // 口径行明示。
    assert!(md.contains("按 block 计入"), "{md}");
}

// 六轮回查：首行 BOM（PowerShell utf8BOM 产物）不吞第一份样本。
#[test]
fn parse_samples_strips_leading_bom() {
    let input = "\u{feff}{\"tool\":\"exec\",\"args\":{},\"actual\":\"block\"}\n{\"tool\":\"exec\",\"args\":{},\"actual\":\"allow\"}\n";
    let (samples, skipped) = parse_samples(input.as_bytes()).unwrap();
    assert_eq!(samples.len(), 2, "BOM 不得吞掉首样本");
    assert_eq!(skipped, 0);
    assert_eq!(samples[0].src_line, 1);
}

// 七轮回查：exec 族样本缺 `command` 键 → 备注列诚实提示（target 空 =
// 规则匹配失真，不让人把失真矩阵误读成规则盲区）；非 exec 族/键齐全不动。
#[test]
fn annotate_note_flags_missing_command_on_exec_family() {
    let args_cmd = serde_json::json!({ "command": "ls" });
    let args_typo = serde_json::json!({ "cmd": "rm -rf /" });
    // exec + 错键 → 提示。
    assert!(annotate_sample_note("exec", &args_typo, "").contains("command 字段"));
    // exec + 键齐 → 原样。
    assert_eq!(annotate_sample_note("exec", &args_cmd, ""), "");
    // exec + 错键 + 既有备注 → 分号拼接不覆盖。
    let joined = annotate_sample_note("exec", &args_typo, "人工备注");
    assert!(joined.starts_with("人工备注；"));
    assert!(joined.contains("command 字段"));
    // 非 exec 族缺 command 不提示（read_file 的参数键本就不是 command）。
    assert_eq!(annotate_sample_note("read_file", &args_typo, ""), "");
    // exec 族全名单抽两个（同走 tool_to_operation 单一真相源）。
    assert!(annotate_sample_note("background_start", &args_typo, "").contains("command"));
    assert!(annotate_sample_note("codex_delegate", &args_typo, "").contains("command"));
}
