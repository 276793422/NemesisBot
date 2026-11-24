//! userprofile::mining 单测：统计规则随迁断言 + 仪式序列状态机（例 1）+
//! todo 收尾挖掘 + 遥测前因挖掘（例 2）。全部纯函数确定性断言。

use super::*;
use crate::userprofile::collection::DialogueRow;

fn now() -> &'static str {
    "2026-11-24T00:00:00+08:00"
}

fn row(ts_ms: i64, role: &str, session: &str, content: &str) -> DialogueRow {
    DialogueRow::new(ts_ms, role, content, session, Vec::new())
}

// ---- 仪式序列状态机（session_ritual_matched 经 ritual_candidates 公开面
//      断言 + 直接单测状态机本身）----

#[test]
fn ritual_full_chain_matches() {
    // 用户例 1 全链：计划 → 多轮回顾确认 → 步骤清单 → 测试。
    let msgs = [
        "去设计文档里做个重构计划",
        "回顾计划确认是否正确",
        "再确认一遍",
        "将计划做成步骤流程，每个流程做成一个复选框",
        "做好测试了么",
    ];
    assert!(session_ritual_matched(&msgs), "完整仪式链必须命中");
}

#[test]
fn ritual_missing_phase_does_not_match() {
    // 缺 confirm 二次出现（只有一轮确认）→ 不算（「好几遍」是用户例的
    // 显著特征）。
    let msgs = [
        "做个计划",
        "回顾计划确认是否正确",
        "做成复选框流程",
        "做好测试了么",
    ];
    assert!(!session_ritual_matched(&msgs), "确认仅一轮不构成仪式");
    // 缺测试收尾。
    let msgs = ["做个计划", "回顾确认", "再次确认", "做成复选框流程"];
    assert!(!session_ritual_matched(&msgs));
    // 无计划起点（闲聊里提到测试不算）。
    let msgs = ["今天天气不错", "确认一下", "再确认", "做个清单", "做测试"];
    assert!(!session_ritual_matched(&msgs), "无 plan 起点不推进");
}

#[test]
fn ritual_requires_two_sessions_in_candidates() {
    // 单会话命中 = 任务偶然形态，不上榜；两个会话都命中 → 一条 workflow
    // 条目（confidence ≥0.75 过 0.7 注入闸、带 4 步清单与溯源）。
    let good = [
        "做个计划",
        "回顾计划确认是否正确",
        "再确认一遍",
        "将计划做成步骤流程，每个流程做成一个复选框",
        "做好测试了么",
    ];
    let mut rows: Vec<DialogueRow> = good
        .iter()
        .enumerate()
        .map(|(i, c)| row(i as i64, "user", "s1", c))
        .collect();
    rows.push(row(100, "user", "s2", "今天聊聊天气"));
    assert!(ritual_candidates(&rows, now()).is_empty(), "单会话不上榜");

    for (i, c) in good.iter().enumerate() {
        rows.push(row(200 + i as i64, "user", "s2", c));
    }
    let out = ritual_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "workflow");
    assert!(out[0].confidence >= 0.7, "两会话 confidence ≥0.75");
    assert_eq!(out[0].steps.len(), 4, "四阶段步骤清单");
    assert!(out[0].auto, "挖掘器产物 = auto 桶");
    assert_eq!(out[0].evidence, vec!["s1", "s2"]);
    assert_eq!(out[0].hits, 2);
    // assistant 行不参与仪式判定。
    let mut with_bot = rows.clone();
    with_bot.insert(2, row(1, "assistant", "s1", "计划做好了包含测试复选框"));
    assert_eq!(ritual_candidates(&with_bot, now()).len(), 1);
}

// ---- todo 收尾挖掘 ----

fn todo(file: &str, items: &[(&str, &str)]) -> TodoSnapshot {
    TodoSnapshot {
        file: file.into(),
        items: items
            .iter()
            .map(|(c, s)| (c.to_string(), s.to_string()))
            .collect(),
    }
}

#[test]
fn todo_test_last_candidates() {
    let snaps = vec![
        todo(
            "todo_a.json",
            &[
                ("实现功能", "completed"),
                ("写文档", "completed"),
                ("跑测试", "completed"),
                ("验证结果", "completed"),
            ],
        ),
        todo(
            "todo_b.json",
            &[
                ("改样式", "pending"),
                ("检查布局", "pending"),
                ("测试浏览器", "pending"),
            ],
        ),
    ];
    let out = todo_candidates(&snaps, now());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "quality_bar");
    assert!(out[0].confidence >= 0.7);
    assert!(out[0].auto);
    assert_eq!(out[0].evidence, vec!["todo_a.json", "todo_b.json"]);
    // steps = 收尾测试项内容（最后两步）。
    assert!(out[0].steps.contains(&"跑测试".to_string()));
    assert!(out[0].steps.contains(&"验证结果".to_string()));
}

#[test]
fn todo_below_thresholds_no_entry() {
    // <2 个快照不上榜。
    let snaps = vec![todo(
        "a",
        &[
            ("x", "pending"),
            ("y", "pending"),
            ("测试", "pending"),
            ("验证", "pending"),
        ],
    )];
    assert!(todo_candidates(&snaps, now()).is_empty());
    // 占比 <0.7 不上榜（3 中 1）。
    let snaps = vec![
        todo(
            "a",
            &[
                ("x", "pending"),
                ("y", "pending"),
                ("测试", "pending"),
                ("验证", "pending"),
            ],
        ),
        todo(
            "b",
            &[
                ("x", "pending"),
                ("y", "pending"),
                ("z", "pending"),
                ("w", "pending"),
            ],
        ),
        todo(
            "c",
            &[
                ("x", "pending"),
                ("y", "pending"),
                ("z", "pending"),
                ("w", "pending"),
            ],
        ),
    ];
    assert!(todo_candidates(&snaps, now()).is_empty());
    // 少于 3 步的清单不算。
    let snaps = vec![
        todo("a", &[("测试", "pending"), ("验证", "pending")]),
        todo("b", &[("检查", "pending"), ("test", "pending")]),
    ];
    assert!(todo_candidates(&snaps, now()).is_empty());
}

// ---- 遥测前因挖掘（例 2）----

fn usage(ts_ms: i64, module: &str, cmd: &str) -> UsageRow {
    UsageRow {
        ts_ms,
        module: module.into(),
        cmd: cmd.into(),
        ok: true,
    }
}

#[test]
fn antecedent_diag_then_cluster_matches() {
    let day0 = 1_792_922_400_000i64; // 2026-10-25T10:00:00Z 基准
    let rows = vec![
        usage(day0, "cluster", "diagnostics.run"),
        usage(day0 + 5 * 60 * 1000, "cluster", "nodes.list"), // 窗内后果
        usage(day0 + 86_400_000, "cluster", "diagnostics.run"),
        usage(
            day0 + 86_400_000 + 10 * 60 * 1000,
            "board",
            "board.issue.list",
        ), // board 也算后果
        usage(day0 + 2 * 86_400_000, "cluster", "diagnostics.run"),
        usage(
            day0 + 2 * 86_400_000 + 60 * 1000,
            "cluster",
            "cluster.status",
        ), // 窗内后果
    ];
    let out = antecedent_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "workflow");
    assert!(out[0].auto);
    assert_eq!(out[0].evidence.len(), 3, "三个不同日期");
    assert!(out[0].content.contains("诊断"));
    assert_eq!(out[0].steps.len(), 3);
}

#[test]
fn antecedent_below_days_or_window_no_entry() {
    // 只有 2 个不同日期 → 不上榜。
    let day0 = 1_792_922_400_000i64;
    let rows = vec![
        usage(day0, "cluster", "diagnostics.run"),
        usage(day0 + 60 * 1000, "cluster", "nodes.list"),
        usage(day0 + 86_400_000, "cluster", "diagnostics.run"),
        usage(day0 + 86_400_000 + 60 * 1000, "cluster", "nodes.list"),
    ];
    assert!(antecedent_candidates(&rows, now()).is_empty());
    // 窗外（>60min）后果不计。
    let rows = vec![
        usage(day0, "cluster", "diagnostics.run"),
        usage(day0 + 61 * 60 * 1000, "cluster", "nodes.list"),
        usage(day0 + 86_400_000, "cluster", "diagnostics.run"),
        usage(day0 + 86_400_000 + 61 * 60 * 1000, "cluster", "nodes.list"),
        usage(day0 + 2 * 86_400_000, "cluster", "diagnostics.run"),
        usage(
            day0 + 2 * 86_400_000 + 61 * 60 * 1000,
            "cluster",
            "nodes.list",
        ),
    ];
    assert!(antecedent_candidates(&rows, now()).is_empty());
    // 无诊断标记（直接 cluster 操作）不构成前因。
    let rows = vec![usage(day0, "cluster", "nodes.list")];
    assert!(antecedent_candidates(&rows, now()).is_empty());
}

// ---- 统计规则（随迁断言在 store/tests.rs；这里补 auto 标记面）----

#[test]
fn stat_candidates_marked_auto() {
    let msgs: Vec<String> = (0..12).map(|_| "继续".into()).collect();
    let out = stat_candidates(&msgs, now());
    assert!(!out.is_empty());
    assert!(out.iter().all(|h| h.auto), "统计规则产物恒 auto 桶（G8）");
}

// ---- 批 B：stat 增量（提问粒度长文 / 中英混用）----

#[test]
fn stat_longform_and_mixed_language() {
    // 长文：10 条 250 字消息 → 长需求书条目；简洁规则（avg<40）不该同时出。
    let long: Vec<String> = (0..10).map(|_| "帮我看一下这个需求，".repeat(25)).collect();
    let out = stat_candidates(&long, now());
    let longform = out
        .iter()
        .find(|h| h.content.contains("长文详述"))
        .expect("长文粒度应上榜");
    assert!(longform.confidence >= 0.7);
    assert!(
        !out.iter().any(|h| h.content.contains("简短直接")),
        "长文与简洁互斥"
    );
    // 短消息（avg<40）→ 只有简洁，没有长文。
    let short: Vec<String> = (0..10).map(|i| format!("改第{i}处")).collect();
    let out = stat_candidates(&short, now());
    assert!(out.iter().any(|h| h.content.contains("简短直接")));
    assert!(!out.iter().any(|h| h.content.contains("长文详述")));
    // 中英混用：10 条 CJK+ASCII 消息 → 术语保留英文条目。
    let mixed: Vec<String> = (0..10)
        .map(|i| format!("把模块{i}的 API 参数改一下"))
        .collect();
    let out = stat_candidates(&mixed, now());
    assert!(
        out.iter().any(|h| h.content.contains("术语保留英文原词")),
        "混用条目应上榜：{out:?}"
    );
    // 纯中文（无 ASCII 词）→ 不上榜。
    let pure: Vec<String> = (0..10).map(|i| format!("把模块{i}的参数改一下")).collect();
    let out = stat_candidates(&pure, now());
    assert!(!out.iter().any(|h| h.content.contains("术语保留英文原词")));
}

// ---- 批 B：页面足迹链 ----

#[test]
fn route_chain_three_days_matches() {
    let day0 = 1_792_922_400_000i64;
    let day = |n: i64| day0 + n * 86_400_000;
    let mut rows = Vec::new();
    for d in 0..3i64 {
        rows.push(usage(day(d), "route", "/board"));
        rows.push(usage(day(d) + 60_000, "route", "/cluster"));
    }
    // 非 route 行不参与（cluster 行混入不破坏链）。
    rows.push(usage(day(0) + 30_000, "cluster", "nodes.list"));
    let out = route_chain_candidates(&rows, now());
    assert_eq!(out.len(), 1, "同转移跨 3 日 → 一条");
    assert_eq!(out[0].category, "workflow");
    assert!(out[0].content.contains("/board"));
    assert!(out[0].content.contains("/cluster"));
    assert_eq!(out[0].steps.len(), 2, "steps=页面序列");
    assert_eq!(out[0].evidence.len(), 3, "evidence=日期");
    // 2 日重现不上榜。
    let mut rows = Vec::new();
    for d in 0..2i64 {
        rows.push(usage(day(d), "route", "/board"));
        rows.push(usage(day(d) + 60_000, "route", "/cluster"));
    }
    assert!(route_chain_candidates(&rows, now()).is_empty());
    // 单日内同页反复切换只算一个序列（相邻去重），单日 1 次出现不上榜。
    let rows = vec![
        usage(day(0), "route", "/a"),
        usage(day(0) + 1_000, "route", "/b"),
        usage(day(0) + 2_000, "route", "/a"),
        usage(day(0) + 3_000, "route", "/b"),
    ];
    assert!(route_chain_candidates(&rows, now()).is_empty());
}

// ---- 批 B：todo 完成/中弃统计 ----

#[test]
fn todo_completion_stats_counts() {
    let stats = todo_completion_stats(&[
        todo("a", &[("x", "completed"), ("y", "completed")]),
        todo(
            "b",
            &[("x", "pending"), ("y", "completed"), ("z", "in_progress")],
        ),
    ]);
    assert_eq!(stats.snapshot_count, 2);
    assert_eq!(stats.item_count, 5);
    assert_eq!(stats.completed_count, 3);
    assert!((stats.completion_ratio - 0.6).abs() < 1e-9);
    assert_eq!(stats.snapshots_with_pending, 1);
    // 空输入 = 零值，ratio 不除零。
    let stats = todo_completion_stats(&[]);
    assert_eq!(stats.completion_ratio, 0.0);
    let stats = todo_completion_stats(&[todo("a", &[])]);
    assert_eq!(stats.completion_ratio, 0.0);
    assert_eq!(stats.snapshots_with_pending, 0, "空快照不算中弃");
}

// ---- 批 B：todo 步骤类别序列（部署收尾）----

#[test]
fn todo_sequence_deploy_tail_matches() {
    // 3 个快照都含部署步且收尾 → quality_bar 条目。
    let snaps = vec![
        todo(
            "a",
            &[
                ("实现功能", "completed"),
                ("跑测试", "completed"),
                ("部署上线", "completed"),
            ],
        ),
        todo(
            "b",
            &[
                ("写代码", "pending"),
                ("部署发布", "pending"),
                ("验证部署", "pending"),
            ],
        ),
        todo(
            "c",
            &[
                ("自测", "pending"),
                ("复核", "pending"),
                ("deploy 到服务器", "pending"),
            ],
        ),
    ];
    let out = todo_sequence_candidates(&snaps, now());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "quality_bar");
    assert!(out[0].auto);
    assert_eq!(out[0].evidence, vec!["a", "b", "c"]);
    assert!(out[0].steps.iter().any(|s| s.contains("部署上线")));
    // 部署步在中间（非收尾）→ 不上榜。
    let snaps = vec![
        todo(
            "a",
            &[
                ("部署准备", "pending"),
                ("实现功能", "pending"),
                ("跑测试", "pending"),
            ],
        ),
        todo(
            "b",
            &[
                ("发布前检查", "pending"),
                ("写代码", "pending"),
                ("自测", "pending"),
            ],
        ),
        todo(
            "c",
            &[
                ("上线演练", "pending"),
                ("复核", "pending"),
                ("收尾", "pending"),
            ],
        ),
    ];
    assert!(todo_sequence_candidates(&snaps, now()).is_empty());
    // 只有 2 个含部署快照 → 不上榜。
    let snaps = vec![
        todo(
            "a",
            &[
                ("实现", "pending"),
                ("自测", "pending"),
                ("部署", "pending"),
            ],
        ),
        todo(
            "b",
            &[
                ("实现", "pending"),
                ("自测", "pending"),
                ("发布", "pending"),
            ],
        ),
    ];
    assert!(todo_sequence_candidates(&snaps, now()).is_empty());
}

// ---- 批 B：开场白模式 ----

#[test]
fn opener_task_dominance_matches() {
    let rows = vec![
        row(1, "user", "s1", "帮我把登录页改成暗色主题"),
        row(2, "assistant", "s1", "好的"),
        row(3, "user", "s2", "看一下这个模块的依赖问题"),
        row(4, "user", "s3", "给看板加一个导出按钮"),
    ];
    let out = opener_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "style");
    assert!(out[0].content.contains("直接派活"));
    assert_eq!(out[0].evidence, vec!["s1", "s2", "s3"]);
    // 2 会话 → 不上榜。
    let rows = vec![
        row(1, "user", "s1", "帮我把登录页改成暗色主题"),
        row(2, "user", "s2", "看一下这个模块的依赖问题"),
    ];
    assert!(opener_candidates(&rows, now()).is_empty());
    // 主流形态 <70%（2 派活 + 1 寒暄）→ 不上榜。
    let rows = vec![
        row(1, "user", "s1", "帮我把登录页改成暗色主题"),
        row(2, "user", "s2", "看一下这个模块的依赖问题"),
        row(3, "user", "s3", "你好，在吗"),
    ];
    assert!(opener_candidates(&rows, now()).is_empty());
}

#[test]
fn opener_classification_order() {
    // 继续/接着前缀优先于报错关键词（继续昨天的报错排查 = Resume）。
    let rows = vec![
        row(1, "user", "s1", "继续昨天的报错排查"),
        row(2, "user", "s2", "接着上次的重构往下做"),
        row(3, "user", "s3", "之前那个性能问题再看看"),
    ];
    let out = opener_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("接着之前的进度"));
    // 贴报错开场。
    let rows = vec![
        row(1, "user", "s1", "报错了：NullPointerException\nat Foo.bar"),
        row(2, "user", "s2", "集群又失败了，看下"),
        row(3, "user", "s3", "这个 error 怎么回事"),
    ];
    let out = opener_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("贴报错"));
}

// ---- 批 B：催促/复述 ----

#[test]
fn urge_bursts_match() {
    let t0 = 1_792_922_400_000i64;
    let mut rows = Vec::new();
    // 簇 1：30s 间隔三连发。
    for i in 0..3i64 {
        rows.push(row(t0 + i * 30_000, "user", "s1", "跑完了吗"));
    }
    // 簇 2：另一内容/另一时段（>URGE_WINDOW 之后）。
    for i in 0..3i64 {
        rows.push(row(t0 + 600_000 + i * 30_000, "user", "s1", "好了没"));
    }
    rows.push(row(t0 + 700_000, "assistant", "s1", "还在跑"));
    let out = urge_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("复述/催促"));
    assert_eq!(out[0].hits, 6, "两簇共 6 条发送");
    assert_eq!(out[0].evidence.len(), 2);
    // 单簇 → 不上榜。
    let rows: Vec<DialogueRow> = (0..3i64)
        .map(|i| row(t0 + i * 30_000, "user", "s1", "跑完了吗"))
        .collect();
    assert!(urge_candidates(&rows, now()).is_empty());
    // 同文三连但最后一发超过 2min（拆成 2+1）→ 不上榜。
    let rows = vec![
        row(t0, "user", "s1", "跑完了吗"),
        row(t0 + 30_000, "user", "s1", "跑完了吗"),
        row(t0 + 200_000, "user", "s1", "跑完了吗"),
    ];
    assert!(urge_candidates(&rows, now()).is_empty());
}

// ---- 批 B：分工风格 ----

#[test]
fn delegation_micro_management() {
    // 两个会话各：user→assistant→user→…（fast 插话 4/4，静默段 0）。
    let t0 = 1_792_922_400_000i64;
    let mut rows = Vec::new();
    for (sid, base) in [("s1", t0), ("s2", t0 + 3_600_000)] {
        for i in 0..4i64 {
            let t = base + i * 40_000;
            rows.push(row(t, "user", sid, &format!("这里改一下第{i}处")));
            rows.push(row(t + 10_000, "assistant", sid, "好的"));
        }
    }
    let out = delegation_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("微观管理"));
    assert_eq!(out[0].evidence, vec!["s1", "s2"]);
}

#[test]
fn delegation_dispatch_style() {
    // 两个会话各 2 个静默段（user→assistant…15min→user），user 消息数
    // <4 → 微观侧不参与；4 段跨 2 会话 → 派单型。
    let t0 = 1_792_922_400_000i64;
    let silent = 15 * 60 * 1000i64;
    let mut rows = Vec::new();
    for (sid, base) in [("s1", t0), ("s2", t0 + 3_600_000)] {
        for i in 0..2i64 {
            let t = base + i * (silent + 60_000);
            rows.push(row(t, "user", sid, &format!("任务{i}，开工")));
            rows.push(row(t + 60_000, "assistant", sid, "收到，在做了"));
            rows.push(row(t + silent, "user", sid, "进度如何"));
        }
    }
    let out = delegation_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("派单型"));
    assert_eq!(out[0].hits, 4, "总静默段数");
    // 混合形态（微观会话 + 静默段并存）→ 不上榜。
    let mut mixed = rows.clone();
    for i in 0..4i64 {
        let t = t0 + 7_200_000 + i * 40_000;
        mixed.push(row(t, "user", "s3", &format!("改第{i}处")));
        mixed.push(row(t + 10_000, "assistant", "s3", "好的"));
    }
    assert!(delegation_candidates(&mixed, now()).is_empty());
}

// ---- 批 B：返工节奏 ----

#[test]
fn rework_iterative_and_oneshot() {
    let now_s = now();
    // ≥4 次 rewind → 迭代型。
    let out = rework_candidates(4, 2, now_s);
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("迭代式"));
    assert_eq!(out[0].hits, 4);
    // 多会话零 rewind → 一次成型型。
    let out = rework_candidates(0, 6, now_s);
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("一次成型"));
    // 少会话零 rewind → 不上榜（证据不足）。
    assert!(rework_candidates(0, 3, now_s).is_empty());
    // 中间带（1-3 次 rewind）→ 不上榜。
    assert!(rework_candidates(2, 6, now_s).is_empty());
}

// ---- 批 B：障碍处置顺序 ----

fn usage_ok(ts_ms: i64, module: &str, cmd: &str, ok: bool) -> UsageRow {
    UsageRow {
        ts_ms,
        module: module.into(),
        cmd: cmd.into(),
        ok,
    }
}

#[test]
fn retry_self_retry_three_days() {
    let day0 = 1_792_922_400_000i64;
    let mut rows = Vec::new();
    for d in 0..3i64 {
        let t = day0 + d * 86_400_000;
        rows.push(usage_ok(t, "cluster", "nodes.ping", false));
        rows.push(usage_ok(t + 20_000, "cluster", "nodes.ping", true)); // 同 cmd 窗内重发成功
    }
    let out = retry_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("自己重试"));
    assert_eq!(out[0].evidence.len(), 3);
    // 2 日 → 不上榜。
    let mut rows = Vec::new();
    for d in 0..2i64 {
        let t = day0 + d * 86_400_000;
        rows.push(usage_ok(t, "cluster", "nodes.ping", false));
        rows.push(usage_ok(t + 20_000, "cluster", "nodes.ping", true));
    }
    assert!(retry_candidates(&rows, now()).is_empty());
    // 重发超窗（>5min）不算。
    let rows = (0..3i64)
        .map(|d| {
            let t = day0 + d * 86_400_000;
            vec![
                usage_ok(t, "cluster", "nodes.ping", false),
                usage_ok(t + 6 * 60_000, "cluster", "nodes.ping", true),
            ]
        })
        .collect::<Vec<_>>()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert!(retry_candidates(&rows, now()).is_empty());
    // 不同 cmd 的成功不算重试解决。
    let rows = (0..3i64)
        .map(|d| {
            let t = day0 + d * 86_400_000;
            vec![
                usage_ok(t, "cluster", "nodes.ping", false),
                usage_ok(t + 20_000, "cluster", "other.cmd", true),
            ]
        })
        .collect::<Vec<_>>()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert!(retry_candidates(&rows, now()).is_empty());
}

// ---- 批 B：审批交互弱信号 ----

#[test]
fn approval_signal_friction_and_smooth() {
    let t0 = 1_792_922_400_000i64;
    // 8 respond，3 失败（37.5% ≥30%）→ 拉锯姿态（workflow）。
    let mut rows: Vec<UsageRow> = (0..8)
        .map(|i| usage_ok(t0 + i * 1000, "approval", "respond", i >= 3))
        .collect();
    let out = approval_signal_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("拉锯"));
    // 10 respond，1 失败（10% ≤10%）→ 顺畅姿态（trust）。
    rows = (0..10)
        .map(|i| usage_ok(t0 + i * 1000, "approval", "respond", i > 0))
        .collect();
    let out = approval_signal_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "trust");
    assert!(out[0].content.contains("快而稳"));
    // 中间带（4/10=40% 不算；改 2/10=20% 在 0.1~0.3 之间）→ 不上榜。
    rows = (0..10)
        .map(|i| usage_ok(t0 + i * 1000, "approval", "respond", i >= 2))
        .collect();
    assert!(approval_signal_candidates(&rows, now()).is_empty());
    // 样本不足（4 条）→ 不上榜。
    rows = (0..4)
        .map(|i| usage_ok(t0 + i * 1000, "approval", "respond", i > 0))
        .collect();
    assert!(approval_signal_candidates(&rows, now()).is_empty());
}

// ---- 遥测行读取 ----

#[test]
fn read_usage_rows_parses_and_sorts() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let logs = ws.join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let body = concat!(
        "{\"ts_ms\": 200, \"module\": \"cluster\", \"cmd\": \"nodes.list\", \"ok\": true}\n",
        "not json\n",
        "{\"ts_ms\": 100, \"module\": \"chat\", \"cmd\": \"send\", \"ok\": false}\n",
        "{\"module\": \"cluster\"}\n",
    );
    std::fs::write(logs.join("wsapi_usage.jsonl"), body).unwrap();
    let out = read_usage_rows(ws);
    assert_eq!(out.len(), 2, "坏行/缺字段行跳过");
    assert_eq!(out[0].ts_ms, 100, "时间升序");
    assert_eq!(out[1].ts_ms, 200);
    assert!(!out[0].ok);
    // 缺文件 = 空。
    assert!(read_usage_rows(&dir.path().join("nope")).is_empty());
}

// ---- 批 C 挖掘面（2026-11-24）：工作域 / 模型切换 / 审批强信号 ----

/// 批 C ①：带 file_kinds 的 assistant 行构造器。
fn file_row(ts_ms: i64, kinds: &[&str]) -> DialogueRow {
    let mut r = row(ts_ms, "assistant", "s1", "done");
    r.file_kinds = kinds.iter().map(|k| k.to_string()).collect();
    r
}

#[test]
fn work_domain_code_dominance() {
    // 10 行带文件接触，7 行 code（70% ≥ 60%）→ workflow 条目。
    let mut rows = Vec::new();
    for i in 0..7i64 {
        rows.push(file_row(i, &["code"]));
    }
    rows.push(file_row(100, &["doc"]));
    rows.push(file_row(101, &["doc"]));
    rows.push(file_row(102, &["code", "config"]));
    let out = work_domain_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "workflow");
    assert!(out[0].content.contains("代码工作为主"));
    assert_eq!(out[0].hits, 8, "code 计数=主导种类行数");
    assert!(out[0].evidence[0].contains("files code=8/10"));
    // auto 条目（batch B 纪律沿用）。
    assert!(out[0].auto);
}

#[test]
fn work_domain_below_thresholds_no_entry() {
    // 样本不足（<10 行）→ 不上榜。
    let rows: Vec<_> = (0..9i64).map(|i| file_row(i, &["code"])).collect();
    assert!(work_domain_candidates(&rows, now()).is_empty());
    // 样本够但占比不足 60% → 不上榜。
    let mut rows = Vec::new();
    for i in 0..5i64 {
        rows.push(file_row(i, &["code"]));
    }
    for i in 10..15i64 {
        rows.push(file_row(i, &["doc"]));
    }
    assert!(work_domain_candidates(&rows, now()).is_empty());
    // 主导种类是 other → 不上榜（未知扩展名无可执行含义）。
    let rows: Vec<_> = (0..10i64).map(|i| file_row(i, &["other"])).collect();
    assert!(work_domain_candidates(&rows, now()).is_empty());
    // user 行带 file_kinds 不参与（恒 assistant 行）。
    let mut u = row(0, "user", "s1", "继续");
    u.file_kinds = vec!["code".into()];
    let rows = (1..11i64)
        .map(|i| file_row(i, &["doc"]))
        .chain(std::iter::once(u))
        .collect::<Vec<_>>();
    let out = work_domain_candidates(&rows, now());
    assert_eq!(out.len(), 1);
    assert!(out[0].content.contains("文档写作"));
}

#[test]
fn model_switch_counts_transitions_and_slash() {
    let now_s = now();
    // 3 次模型变化（glm→kimi→glm→deepseek）→ tool_preference 条目。
    let mut m1 = row(0, "assistant", "s1", "a");
    m1.model = "glm-5.3-flash".into();
    let mut m2 = row(1, "assistant", "s1", "b");
    m2.model = "kimi-k3".into();
    let mut m3 = row(2, "assistant", "s1", "c");
    m3.model = "glm-5.3-flash".into();
    let mut m4 = row(3, "assistant", "s1", "d");
    m4.model = "deepseek-v4".into();
    let out = model_switch_candidates(&[m1, m2, m3, m4], now_s);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "tool_preference");
    assert_eq!(out[0].hits, 3);
    assert!(out[0].evidence[0].contains("changes=3"));
    // 2 次模型变化（glm→kimi→glm）+ 1 次 /model slash → 混合计数过阈。
    let mut m1 = row(0, "assistant", "s1", "a");
    m1.model = "glm-5.3-flash".into();
    let mut m2 = row(1, "assistant", "s1", "b");
    m2.model = "kimi-k3".into();
    let mut m3 = row(2, "assistant", "s1", "c");
    m3.model = "glm-5.3-flash".into();
    let slash = row(3, "user", "s1", "  /model glm-5.3-flash");
    let out = model_switch_candidates(&[m1.clone(), m2.clone(), m3, slash], now_s);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].hits, 3);
    assert!(out[0].evidence[1].contains("/model=1"));
    // 不足 3 → 不上榜。
    assert!(model_switch_candidates(&[m1.clone(), m2.clone()], now_s).is_empty());
    // 空模型行不算变化（旧条目缺 model 徽标）。
    let rows = vec![m1, m2, row(2, "assistant", "s1", "c")];
    assert!(model_switch_candidates(&rows, now_s).is_empty());
    // 单模型恒定（无变化）→ 不上榜。
    let rows = vec![
        {
            let mut r = row(0, "assistant", "s1", "a");
            r.model = "glm-5.3-flash".into();
            r
        },
        {
            let mut r = row(1, "assistant", "s1", "b");
            r.model = "glm-5.3-flash".into();
            r
        },
    ];
    assert!(model_switch_candidates(&rows, now_s).is_empty());
}

/// 批 C ④：审批卡信号构造器（只带挖掘器读的键）。
fn approval_signal(approved: bool, duration_ms: u64) -> serde_json::Value {
    serde_json::json!({
        "ts_ms": 0, "kind": "approval", "operation": "exec",
        "risk_level": "HIGH", "approved": approved,
        "duration_ms": duration_ms, "session_key": "s1"
    })
}

#[test]
fn approval_strong_fast_and_slow() {
    let now_s = now();
    // 5 张批准卡平均 3s（≤5s）→ trust 条目。
    let signals: Vec<_> = [2000, 3000, 4000, 3000, 3000]
        .iter()
        .map(|&d| approval_signal(true, d))
        .collect();
    let out = approval_strong_candidates(&signals, now_s);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "trust");
    assert!(out[0].content.contains("快而稳"));
    assert_eq!(out[0].hits, 5);
    assert!(out[0].evidence.iter().any(|e| e.contains("avg_ms=3000")));
    // 平均 70s（≥60s）→ workflow 条目。
    let signals: Vec<_> = [60_000, 70_000, 80_000, 70_000, 70_000]
        .iter()
        .map(|&d| approval_signal(true, d))
        .collect();
    let out = approval_strong_candidates(&signals, now_s);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "workflow");
    assert!(out[0].content.contains("审批响应慢"));
    // 中间带（平均 30s）→ 不上榜。
    let signals: Vec<_> = (0..5).map(|_| approval_signal(true, 30_000)).collect();
    assert!(approval_strong_candidates(&signals, now_s).is_empty());
    // 样本不足 5 张 → 不上榜。
    let signals: Vec<_> = (0..4).map(|_| approval_signal(true, 2000)).collect();
    assert!(approval_strong_candidates(&signals, now_s).is_empty());
    // 拒绝卡不混入「决策快」平均（5 拒 0 批 → 无批准样本）。
    let signals: Vec<_> = (0..5).map(|_| approval_signal(false, 1000)).collect();
    assert!(approval_strong_candidates(&signals, now_s).is_empty());
    // estop 信号不参与审批挖掘。
    let mut signals: Vec<_> = [2000, 3000, 4000, 3000, 3000]
        .iter()
        .map(|&d| approval_signal(true, d))
        .collect();
    signals.push(serde_json::json!({"ts_ms": 9, "kind": "estop", "action": "engage"}));
    assert_eq!(approval_strong_candidates(&signals, now_s).len(), 1);
}

// ---------- 终端会话挖掘（2026-11-24 T 批） ----------

fn term(shell: &str, cmds: &[&str]) -> crate::userprofile::collection::TerminalSession {
    crate::userprofile::collection::TerminalSession {
        file: format!("pty_{shell}.log"),
        shell: shell.to_string(),
        ts_ms: 0,
        commands: cmds.iter().map(|c| c.to_string()).collect(),
    }
}

/// shell 偏好：≥3 会话且主导占比 ≥70% → environment 条目；缺头会话不进
/// 分母；不足 3 会话 / 占比不足 → 空。
#[test]
fn shell_preference_candidates_gates() {
    let sessions = vec![
        term("pwsh", &["a"]),
        term("pwsh", &["b"]),
        term("pwsh", &["c"]),
        term("", &["d"]), // 缺 shell 头：不进分母
    ];
    let out = shell_preference_candidates(&sessions, now());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].category, "environment");
    assert!(out[0].content.contains("pwsh"), "{}", out[0].content);

    // 不足 3 个已知 shell 会话 → 不挖。
    let few = vec![term("pwsh", &["a"]), term("", &["b"]), term("", &["c"])];
    assert!(shell_preference_candidates(&few, now()).is_empty());

    // 占比不足 70%（2/3 pwsh + 1/3 bash = 0.67 < 0.7）→ 不挖。
    let mixed = vec![term("pwsh", &[]), term("pwsh", &[]), term("bash", &[])];
    assert!(shell_preference_candidates(&mixed, now()).is_empty());
}

/// 命令族：首 token 总数 ≥8 且跨 ≥3 会话且非噪声 → tool_preference top-3；
/// 噪声命令 / 单会话命令 / 参数不参与。
#[test]
fn terminal_command_candidates_gates() {
    // cargo 总 8 次、跨 3 会话 → 上榜；ls/cd/exit 噪声不参与；参数不采集。
    let s1 = term(
        "pwsh",
        &["cargo build --release", "cargo test", "cargo run", "ls"],
    );
    let s2 = term(
        "pwsh",
        &["cargo build", "cargo clippy --tests", "cargo check"],
    );
    let s3 = term("pwsh", &["cargo fmt --all --check", "cargo doc", "cd .."]);
    let out = terminal_command_candidates(&[s1, s2, s3], now());
    assert_eq!(out.len(), 1, "cargo 8 次/3 会话达标，其余命令未达标或噪声");
    assert_eq!(out[0].category, "tool_preference");
    assert!(out[0].content.contains("cargo"), "{}", out[0].content);
    assert!(
        !out.iter().any(|e| e.content.contains("release")),
        "参数不采集：条目内容只提命令名"
    );

    // 总数够但只跨 1 个会话（8 连敲同命令）→ 不构成习惯。
    let spammy = vec![
        term("pwsh", &["vec x"; 8]),
        term("pwsh", &[]),
        term("pwsh", &[]),
    ];
    assert!(terminal_command_candidates(&spammy, now()).is_empty());
}

/// top-K 排序：次数多的命令排前 + 未达标命令不出（确定性断言）。
/// 每个命令都跨 3 会话（spread 达标），按总数区分高低。
#[test]
fn terminal_command_candidates_orders_by_count() {
    // gamma：9 次（3+3+3）；delta：8 次（3+3+2）；epsilon：7 次（3+2+2，
    // 不达标）；ls 噪声不参与。
    let s1 = term(
        "pwsh",
        &[
            "gamma --a1",
            "gamma --a2",
            "gamma --a3",
            "delta --b1",
            "delta --b2",
            "delta --b3",
            "epsilon --c1",
            "epsilon --c2",
            "epsilon --c3",
        ],
    );
    let s2 = term(
        "pwsh",
        &[
            "gamma --d1",
            "gamma --d2",
            "gamma --d3",
            "delta --e1",
            "delta --e2",
            "delta --e3",
            "epsilon --f1",
            "epsilon --f2",
        ],
    );
    let s3 = term(
        "pwsh",
        &[
            "gamma --g1",
            "gamma --g2",
            "gamma --g3",
            "delta --h1",
            "delta --h2",
            "epsilon --i1",
            "epsilon --i2",
            "ls",
        ],
    );
    let out = terminal_command_candidates(&[s1, s2, s3], now());
    assert_eq!(out.len(), 2, "epsilon 7 次不达标、ls 噪声");
    assert!(out[0].content.contains("gamma"), "{}", out[0].content);
    assert!(out[1].content.contains("delta"), "{}", out[1].content);
}
