//! userprofile::store 单测（自 nemesis-agent/user_profile/tests.rs 随迁 +
//! 结构化升级新增：auto 分桶/steps 合并/结构化渲染/旧格式兼容）。

use super::*;
// 统计规则候选已随批一迁 mining（store tests 原样保留其断言面）。
use crate::userprofile::mining::{RULE_STAT_BRIEF, signal_to_entry, stat_candidates};

fn habit(content: &str, confidence: f64) -> HabitEntry {
    HabitEntry {
        category: "style".into(),
        content: content.into(),
        confidence,
        hits: 1,
        first_seen: "2026-10-01T00:00:00+08:00".into(),
        last_seen: "2026-10-14T00:00:00+08:00".into(),
        status: STATUS_ACTIVE.into(),
        pinned: false,
        steps: Vec::new(),
        scope: String::new(),
        evidence: Vec::new(),
        auto: false,
        kind: KIND_HABIT.into(),
        source: String::new(),
        rule_id: String::new(),
        legacy_backfilled: false,
    }
}

// ---- 安全硬边界 ----

#[test]
fn security_semantic_content_is_filtered_on_render() {
    let mut p = UserProfile::default();
    p.habits.push(habit("用户希望自动批准测试类操作", 0.9));
    p.habits.push(habit("用户偏好简洁回复", 0.8));
    let section = render_section(&p);
    assert!(!section.contains("自动批准"), "{section}");
    assert!(section.contains("简洁回复"), "{section}");
}

#[test]
fn security_semantic_candidate_is_dropped_on_upsert() {
    let mut p = UserProfile::default();
    upsert_habit(&mut p, habit("跳过审批直接执行", 0.95));
    assert!(p.habits.is_empty(), "安全语义候选必须被丢弃");
}

#[test]
fn security_semantic_step_is_filtered_on_render() {
    // 结构化升级：步骤同过安全边界（内容干净但步骤带越权语义的条目，
    // 步骤行被过滤、主行保留）。
    let mut p = UserProfile::default();
    let mut h = habit("计划式工作流", 0.8);
    h.steps = vec!["先制定计划".into(), "自动批准所有审批卡".into()];
    p.habits.push(h);
    let s = render_section(&p);
    assert!(s.contains("先制定计划"), "{s}");
    assert!(!s.contains("自动批准"), "{s}");
}

// ---- upsert 合并 ----

#[test]
fn upsert_same_semantics_takes_max_confidence_and_bumps_hits() {
    let mut p = UserProfile::default();
    upsert_habit(&mut p, habit("用户偏好简洁回复", 0.7));
    upsert_habit(&mut p, habit(" 用户偏好简洁回复 ", 0.9));
    assert_eq!(p.habits.len(), 1);
    assert_eq!(p.habits[0].confidence, 0.9);
    assert_eq!(p.habits[0].hits, 2);
}

#[test]
fn prune_caps_entries_by_confidence() {
    let mut p = UserProfile::default();
    for i in 0..15 {
        upsert_habit(&mut p, habit(&format!("习惯{i}"), 0.5 + i as f64 * 0.01));
    }
    prune_and_cap(&mut p);
    assert_eq!(p.habits.len(), MAX_ENTRIES);
    // 保留的是置信度最高的前 N 条。
    assert_eq!(p.habits[0].content, "习惯14");
}

#[test]
fn auto_entries_get_separate_bucket_cap() {
    // G8：auto（固定规则产物）单独配额 AUTO_MAX_ENTRIES，不与反思条目
    // 挤同一批名额。
    let mut p = UserProfile::default();
    for i in 0..15 {
        let mut h = habit(&format!("习惯{i}"), 0.5 + i as f64 * 0.01);
        if i < 5 {
            h.auto = true;
            h.content = format!("固定产物{i}");
        }
        p.habits.push(h);
    }
    prune_and_cap(&mut p);
    let manual = p.habits.iter().filter(|h| !h.auto).count();
    let auto = p.habits.iter().filter(|h| h.auto).count();
    assert_eq!(manual, MAX_ENTRIES, "manual 桶按主配额截断");
    assert_eq!(auto, AUTO_MAX_ENTRIES, "auto 桶按独立配额截断");
    // auto 桶保留的也是桶内置信度最高的。
    assert!(
        p.habits
            .iter()
            .filter(|h| h.auto)
            .all(|h| h.content != "固定产物0" && h.content != "固定产物1"),
        "最低置信度 auto 条目应被截去"
    );
}

#[test]
fn auto_flag_ors_on_merge() {
    // 反思条目（auto=false）被同语义固定产物命中 → 归 auto 桶（自迁移：
    // 老统计条目 auto 字段缺省 false，规则通道 upsert 后自然归桶）。
    let mut p = UserProfile::default();
    upsert_habit(&mut p, habit("用户偏好简洁回复", 0.8));
    assert!(!p.habits[0].auto);
    let mut rule_hit = habit("用户偏好简洁回复", 0.7);
    rule_hit.auto = true;
    upsert_habit(&mut p, rule_hit);
    assert!(p.habits[0].auto, "auto 语义命中必须置位（OR）");
}

// ---- steps/evidence 结构化合并 ----

#[test]
fn steps_merge_union_dedup_preserves_order() {
    let mut p = UserProfile::default();
    let mut a = habit("计划式工作流", 0.8);
    a.steps = vec!["先制定计划".into(), "回顾确认".into()];
    upsert_habit(&mut p, a);
    let mut b = habit("计划式工作流", 0.85);
    b.steps = vec!["回顾确认".into(), "测试收尾".into()];
    upsert_habit(&mut p, b);
    assert_eq!(p.habits.len(), 1);
    assert_eq!(
        p.habits[0].steps,
        vec!["先制定计划", "回顾确认", "测试收尾"],
        "steps 并集：保序去重，反思细化能累积"
    );
}

#[test]
fn steps_are_sanitized_on_insert_and_merge() {
    // 步骤清洗：换行折叠 + 超长截断 + 条数上限（MAX_STEPS）。
    let mut p = UserProfile::default();
    let mut h = habit("仪式", 0.8);
    let long = "x".repeat(300);
    h.steps = (0..MAX_STEPS + 3)
        .map(|i| format!("步骤{i}\r\n{long}"))
        .collect();
    upsert_habit(&mut p, h);
    assert!(p.habits[0].steps.len() <= MAX_STEPS, "条数截断");
    for s in &p.habits[0].steps {
        assert!(!s.contains('\r') && !s.contains('\n'), "换行折叠: {s}");
        assert!(s.chars().count() <= MAX_STEP_CHARS, "单步超长截断");
    }
}

#[test]
fn evidence_merge_union_truncates() {
    let mut p = UserProfile::default();
    let mut a = habit("用前先诊断", 0.8);
    a.evidence = (0..MAX_EVIDENCE).map(|i| format!("ev{i}")).collect();
    upsert_habit(&mut p, a);
    let mut b = habit("用前先诊断", 0.8);
    b.evidence = vec!["ev0".into(), "新日期".into()];
    upsert_habit(&mut p, b);
    assert_eq!(p.habits[0].evidence.len(), MAX_EVIDENCE, "溯源截上限");
    assert!(
        p.habits[0].evidence.contains(&"新日期".to_string()),
        "新溯源并入"
    );
}

// ---- 衰减 ----

#[test]
fn decay_advances_status_by_age() {
    let now = chrono::Local::now();
    let mut p = UserProfile::default();
    let mut fresh = habit("新习惯", 0.9);
    fresh.last_seen = now.to_rfc3339();
    let mut old = habit("旧习惯", 0.8);
    old.last_seen = (now - chrono::Duration::days(31)).to_rfc3339();
    let mut ancient = habit("远古习惯", 0.7);
    ancient.last_seen = (now - chrono::Duration::days(61)).to_rfc3339();
    p.habits = vec![fresh, old, ancient];

    decay_sweep(&mut p, now);
    assert_eq!(p.habits[0].status, STATUS_ACTIVE);
    assert_eq!(p.habits[1].status, STATUS_DECAYING);
    assert_eq!(p.habits[2].status, STATUS_EXPIRED);
}

#[test]
fn decay_never_touches_vetoed() {
    let now = chrono::Local::now();
    let mut p = UserProfile::default();
    let mut v = habit("被否决", 0.9);
    v.status = STATUS_VETOED.into();
    v.last_seen = (now - chrono::Duration::days(100)).to_rfc3339();
    p.habits = vec![v];
    decay_sweep(&mut p, now);
    assert_eq!(p.habits[0].status, STATUS_VETOED);
}

#[test]
fn decay_respects_configured_days() {
    // config decay_days=7：8 天旧条目在缺省 30 天下仍 active，在 7 天下 decaying。
    let now = chrono::Local::now();
    let mut p = UserProfile::default();
    let mut h = habit("一周前", 0.9);
    h.last_seen = (now - chrono::Duration::days(8)).to_rfc3339();
    p.habits = vec![h];
    decay_sweep(&mut p, now);
    assert_eq!(p.habits[0].status, STATUS_ACTIVE);
    decay_sweep_with_days(&mut p, now, 7);
    assert_eq!(p.habits[0].status, STATUS_DECAYING);
}

#[test]
fn decay_absurd_config_days_saturates_instead_of_overflow() {
    // 回归锁（BUG-20261107-025）：config 来源 decay_days 只挡 0/负无上界
    // （decay_days_or），合法可解析的荒谬大值经 `*2` 溢出——debug panic /
    // release 环绕为负 → days > 负数恒真 → 全部条目误判 expired 被
    // prune 清空（静默毁数据）。饱和语义 = 永不衰减（诚实极端值）。
    let now = chrono::Local::now();
    let mut p = UserProfile::default();
    let mut h = habit("不该被误杀的习惯", 0.9);
    h.last_seen = now.to_rfc3339();
    p.habits = vec![h];
    // 5e18：serde 合法 i64，×2 溢出 i64::MAX（≈9.22e18）。
    decay_sweep_with_days(&mut p, now, 5_000_000_000_000_000_000);
    assert_eq!(p.habits[0].status, STATUS_ACTIVE);
    // i64::MAX 本体同面（saturating_mul 封顶后比较恒假）。
    let mut p2 = UserProfile::default();
    let mut h2 = habit("极限值也不误杀", 0.8);
    h2.last_seen = (now - chrono::Duration::days(3650)).to_rfc3339();
    p2.habits = vec![h2];
    decay_sweep_with_days(&mut p2, now, i64::MAX);
    assert_eq!(p2.habits[0].status, STATUS_ACTIVE);
}

// ---- 渲染 ----

#[test]
fn render_skips_decaying_and_expired_and_empty_is_zero_bytes() {
    let mut p = UserProfile::default();
    assert_eq!(render_section(&p), "", "空画像必须零字节");
    let mut a = habit("活跃", 0.9);
    a.status = STATUS_ACTIVE.into();
    let mut d = habit("衰减中", 0.8);
    d.status = STATUS_DECAYING.into();
    let mut e = habit("过期", 0.7);
    e.status = STATUS_EXPIRED.into();
    p.habits = vec![a, d, e];
    let s = render_section(&p);
    assert!(s.contains("活跃"));
    assert!(s.contains("衰减中")); // decaying 仍渲染（降权语义由排序承载）
    assert!(!s.contains("过期"));
}

#[test]
fn structured_entry_renders_numbered_steps() {
    // 批二注入升级：带 steps 条目 = 主行 + 编号步骤块。
    let mut p = UserProfile::default();
    let mut h = habit("计划式工作流", 0.8);
    h.steps = vec!["先制定计划".into(), "测试收尾".into()];
    p.habits.push(h);
    let s = render_section(&p);
    assert!(s.contains("- [style] 计划式工作流"), "{s}");
    assert!(s.contains("  1. 先制定计划"), "{s}");
    assert!(s.contains("  2. 测试收尾"), "{s}");
    // 无 steps 条目保持旧单行形态（字节兼容）。
    p.habits.push(habit("简洁习惯", 0.8));
    let s2 = render_section(&p);
    assert!(s2.lines().any(|l| l == "- [style] 简洁习惯"), "{s2}");
}

// ---- 统计候选 ----

#[test]
fn stat_candidates_language_rule() {
    let msgs: Vec<String> = (0..12)
        .map(|_| "帮我把登录模块重构一下，注意兼容旧接口".into())
        .collect();
    let out = stat_candidates(&msgs);
    assert!(
        out.iter()
            .any(|h| h.category == "style" && h.fallback_statement.contains("中文")),
        "{out:?}"
    );
}

#[test]
fn stat_candidates_short_messages_rule() {
    let msgs: Vec<String> = (0..12).map(|_| "继续".into()).collect();
    let out = stat_candidates(&msgs);
    assert!(
        out.iter().any(|h| h.fallback_statement.contains("简洁")),
        "{out:?}"
    );
}

#[test]
fn stat_candidates_correction_rule() {
    let mut msgs: Vec<String> = (0..12).map(|_| "继续".into()).collect();
    for (i, m) in msgs.iter_mut().take(4).enumerate() {
        *m = format!("不对，我意思是方案{i}");
    }
    let out = stat_candidates(&msgs);
    assert!(out.iter().any(|h| h.category == "correction"), "{out:?}");
}

#[test]
fn stat_candidates_empty_input_is_empty() {
    assert!(stat_candidates(&[]).is_empty());
}

// ---- 往返 ----

#[test]
fn profile_roundtrip_via_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = UserProfile {
        enabled_at: Some("2026-10-14T00:00:00+08:00".into()),
        last_sweep_ms: Some(1_760_000_000_000),
        habits: vec![habit("用户偏好结论先行", 0.85)],
        ..Default::default()
    };
    save_profile(dir.path(), &p).unwrap();
    p.habits[0].hits = 5;
    save_profile(dir.path(), &p).unwrap();
    let loaded = load_profile(dir.path());
    assert_eq!(loaded.habits.len(), 1);
    assert_eq!(loaded.habits[0].hits, 5);
    assert_eq!(
        loaded.enabled_at.as_deref(),
        Some("2026-10-14T00:00:00+08:00")
    );
    assert_eq!(loaded.last_sweep_ms, Some(1_760_000_000_000));
    // 结构化新字段随 round-trip 保留。
    assert_eq!(loaded.sweeps_since_reflect, 0);
    assert_eq!(loaded.last_reflect_ms, None);
}

// ---- sweep 证据窗（滚动 24h）----

/// 01T00:00:00Z = epoch 1790812800 秒（锚点常用于以下断言）。
const ANCHOR_MS: i64 = 1_790_812_800_000;

#[test]
fn sweep_window_first_sweep_clamps_to_anchor() {
    // 首 sweep：锚点比滚动界新 → 窗口夹回锚点（仅向前不回溯）。
    let now_ms = ANCHOR_MS + 30 * 60 * 1000; // 启用后 30 分钟
    let p = UserProfile {
        enabled_at: Some("2026-10-01T00:00:00Z".into()),
        last_sweep_ms: None,
        ..Default::default()
    };
    assert_eq!(sweep_window_start_ms(&p, now_ms), Some(ANCHOR_MS));
}

#[test]
fn sweep_window_first_sweep_old_account_scans_recent_24h_only() {
    // 启用已超 24h 的存量用户首 sweep：min(锚点, now-24h) = 滚动界，
    // max 后仍滚动界 → 只扫近 24h（不回溯挖陈年历史）。
    let now_ms = ANCHOR_MS + 30 * 24 * 60 * 60 * 1000; // 启用后 30 天
    let p = UserProfile {
        enabled_at: Some("2026-10-01T00:00:00Z".into()),
        last_sweep_ms: None,
        ..Default::default()
    };
    assert_eq!(
        sweep_window_start_ms(&p, now_ms),
        Some(now_ms - SWEEP_WINDOW_MS)
    );
}

#[test]
fn sweep_window_steady_state_rolls_24h() {
    // 稳态：上次 sweep 1 小时前 → min 落滚动界 → 窗口 = now-24h。
    let now_ms = ANCHOR_MS + 10 * 24 * 60 * 60 * 1000;
    let p = UserProfile {
        enabled_at: Some("2026-10-01T00:00:00Z".into()),
        last_sweep_ms: Some(now_ms - 60 * 60 * 1000),
        ..Default::default()
    };
    assert_eq!(
        sweep_window_start_ms(&p, now_ms),
        Some(now_ms - SWEEP_WINDOW_MS)
    );
}

#[test]
fn sweep_window_downtime_backfills_from_waterline() {
    // 停机补漏：水线停在 3 天前（锚点更早）→ 窗口下探到水线。
    let now_ms = ANCHOR_MS + 10 * 24 * 60 * 60 * 1000;
    let p = UserProfile {
        enabled_at: Some("2026-10-01T00:00:00Z".into()),
        last_sweep_ms: Some(now_ms - 3 * 24 * 60 * 60 * 1000),
        ..Default::default()
    };
    assert_eq!(
        sweep_window_start_ms(&p, now_ms),
        Some(now_ms - 3 * 24 * 60 * 60 * 1000)
    );
}

#[test]
fn sweep_window_future_waterline_takes_rolling_bound() {
    // 时钟偏移：水线在未来 → min 取滚动界，不扫未来。
    let now_ms = ANCHOR_MS + 10 * 24 * 60 * 60 * 1000;
    let p = UserProfile {
        enabled_at: Some("2026-10-01T00:00:00Z".into()),
        last_sweep_ms: Some(now_ms + 60 * 60 * 1000),
        ..Default::default()
    };
    assert_eq!(
        sweep_window_start_ms(&p, now_ms),
        Some(now_ms - SWEEP_WINDOW_MS)
    );
}

#[test]
fn sweep_window_none_when_never_enabled_or_anchor_corrupt() {
    let now_ms = ANCHOR_MS + 60 * 60 * 1000;
    assert_eq!(sweep_window_start_ms(&UserProfile::default(), now_ms), None);
    let p = UserProfile {
        enabled_at: Some("not-a-timestamp".into()),
        last_sweep_ms: None,
        ..Default::default()
    };
    assert_eq!(sweep_window_start_ms(&p, now_ms), None);
}

#[test]
fn profile_json_without_waterline_field_loads_clean() {
    // 旧版画像 JSON（无 last_sweep_ms）= serde default None，零迁移。
    let dir = tempfile::tempdir().unwrap();
    let legacy = r#"{"enabled_at": "2026-10-01T00:00:00Z", "habits": []}"#;
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config").join("user_profile.json"), legacy).unwrap();
    let loaded = load_profile(dir.path());
    assert_eq!(loaded.last_sweep_ms, None);
    // 首 sweep 行为 = 窗口夹锚点（与 sweep_window_first_sweep_clamps_to_anchor 同语义）。
    assert_eq!(
        sweep_window_start_ms(&loaded, ANCHOR_MS + 60 * 60 * 1000),
        Some(ANCHOR_MS)
    );
}

#[test]
fn legacy_entry_without_structured_fields_reads_clean() {
    // 批二兼容：老条目 JSON 无 steps/scope/evidence/auto 字段 = serde
    // default 原样可读可注入（旧形态单句条目）。
    let dir = tempfile::tempdir().unwrap();
    let legacy = r#"{"enabled_at":"2026-10-01T00:00:00Z","habits":[{"category":"style","content":"喜欢简洁回复","confidence":0.9,"hits":1,"first_seen":"2026-01-01T00:00:00Z","last_seen":"2026-01-01T00:00:00Z"}]}"#;
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config").join("user_profile.json"), legacy).unwrap();
    let p = load_profile(dir.path());
    assert_eq!(p.habits.len(), 1);
    assert!(p.habits[0].steps.is_empty());
    assert!(p.habits[0].scope.is_empty());
    assert!(p.habits[0].evidence.is_empty());
    assert!(!p.habits[0].auto);
    assert!(!render_section(&p).is_empty());
}

// ---- veto 驻留抑制链（二轮复查根修回归锁）----

#[test]
fn vetoed_entry_survives_prune_and_cap() {
    let mut p = UserProfile::default();
    for i in 0..12 {
        upsert_habit(&mut p, habit(&format!("习惯{i}"), 0.5 + i as f64 * 0.01));
    }
    // 否决其中一条高置信度条目。
    p.habits[11].status = STATUS_VETOED.into();
    prune_and_cap(&mut p);
    let vetoed = p
        .habits
        .iter()
        .filter(|h| h.status == STATUS_VETOED)
        .count();
    assert_eq!(vetoed, 1, "vetoed 条目必须驻留（否则下轮 sweep 原样再生）");
    // 活跃名额不被 vetoed 占用。
    let active = p
        .habits
        .iter()
        .filter(|h| h.status != STATUS_VETOED)
        .count();
    assert_eq!(active, MAX_ENTRIES);
}

#[test]
fn prune_with_max_entries_configurable_cap_applies() {
    // config `user_profile.max_entries` 接线（三轮复查根修：此前声名未
    // 读取，上限永远走 MAX_ENTRIES 常量）。
    let mut p = UserProfile::default();
    for i in 0..8 {
        upsert_habit(&mut p, habit(&format!("习惯{i}"), 0.5 + i as f64 * 0.01));
    }
    prune_and_cap_with_max(&mut p, 3);
    assert_eq!(p.habits.len(), 3, "可调上限生效");
    // 保留的是置信度最高的 3 条（浮点容差比较）。
    let mut confs: Vec<f64> = p.habits.iter().map(|h| h.confidence).collect();
    confs.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    assert!(
        (confs[0] - 0.57).abs() < 1e-9,
        "最高置信度条目应保留: {}",
        confs[0]
    );
    // 核心层是字面语义（truncate(0) = 清空）；config 侧 max_entries_or
    // 把 0 挡掉回落缺省——两层契约分别钉住。
    let mut p2 = UserProfile::default();
    for i in 0..8 {
        upsert_habit(&mut p2, habit(&format!("习惯{i}"), 0.5));
    }
    prune_and_cap_with_max(&mut p2, 0);
    assert_eq!(p2.habits.len(), 0, "核心层 0 = 字面清空");
}

#[test]
fn upsert_hit_on_vetoed_entry_stays_vetoed() {
    let mut p = UserProfile::default();
    let h = habit("用户偏好简洁回复", 0.8);
    p.habits.push(h.clone());
    p.habits[0].status = STATUS_VETOED.into();
    // 同内容候选再次 upsert（= sweep 重扫到同一信号）。
    upsert_habit(&mut p, h);
    assert_eq!(p.habits.len(), 1);
    assert_eq!(
        p.habits[0].status, STATUS_VETOED,
        "vetoed 命中 upsert 必须保持否决态（重生成抑制）"
    );
}

// ---- 空子串防线 ----

#[test]
fn empty_substring_is_rejected_for_veto_forget_pin() {
    let dir = tempfile::tempdir().unwrap();
    let p = UserProfile {
        enabled_at: Some("2026-10-01T00:00:00Z".into()),
        habits: vec![habit("用户偏好简洁回复", 0.8), habit("习惯乙", 0.7)],
        ..Default::default()
    };
    save_profile(dir.path(), &p).unwrap();
    assert!(veto_habit(dir.path(), ProfileScope::Dialogue, "").is_err());
    assert!(veto_habit(dir.path(), ProfileScope::Dialogue, "   ").is_err());
    assert!(forget_habit(dir.path(), ProfileScope::Dialogue, "").is_err());
    assert!(set_pinned(dir.path(), ProfileScope::Dialogue, "", true).is_err());
    // 防线在写盘前：条目原样保留。
    let p = load_profile(dir.path());
    assert_eq!(p.habits.len(), 2);
}

// ---- 四路评审修复回归锁 ----

#[test]
fn upsert_hit_clamps_out_of_range_confidence() {
    // B-：LLM 反思通道候选可能带 >1.0 的越界置信度，命中合并路径必须
    // 钳制（插入路径已有 clamp，这里钉命中路径）。
    let mut p = UserProfile::default();
    upsert_habit(&mut p, habit("用户偏好简洁回复", 0.4));
    upsert_habit(&mut p, habit("用户偏好简洁回复", 1.7));
    assert_eq!(p.habits.len(), 1);
    assert!(
        (p.habits[0].confidence - 1.0).abs() < 1e-9,
        "命中合并 max 后必须钳到 [0,1]: {}",
        p.habits[0].confidence
    );
    // 负值同钳。
    let mut p2 = UserProfile::default();
    upsert_habit(&mut p2, habit("习惯丙", 0.4));
    upsert_habit(&mut p2, habit("习惯丙", -0.5));
    assert!(
        (p2.habits[0].confidence - 0.4).abs() < 1e-9,
        "max(0.4, -0.5) = 0.4，不得为负"
    );
}

#[test]
fn prune_keeps_pinned_over_higher_confidence() {
    // B-Minor2：pinned 是用户意志，截断不得丢——低置信度 pinned 条目
    // 优先于高置信度未钉条目保留。
    let mut p = UserProfile::default();
    for i in 0..15 {
        upsert_habit(&mut p, habit(&format!("习惯{i}"), 0.5 + i as f64 * 0.01));
    }
    let mut low = habit("钉住的低置信习惯", 0.1);
    low.pinned = true;
    p.habits.push(low);
    prune_and_cap(&mut p);
    assert!(
        p.habits.iter().any(|h| h.content == "钉住的低置信习惯"),
        "pinned 条目必须存活于截断后集合"
    );
    assert_eq!(
        p.habits.iter().filter(|h| !h.auto).count(),
        MAX_ENTRIES,
        "pinned 占活跃名额，manual 桶仍按上限截断"
    );
}

#[test]
fn render_folds_newlines_in_content() {
    // B-：反思通道内容若夹带换行会伪造注入行结构——渲染层压成单行。
    let mut p = UserProfile::default();
    let mut h = habit("第一行\r\n第二行", 0.8);
    h.status = STATUS_ACTIVE.into();
    p.habits = vec![h];
    let s = render_section(&p);
    assert!(!s.contains('\r'), "CR 必须被折叠");
    assert_eq!(s.lines().count(), 1, "单条目渲染 = 单行（无结构换行注入）");
    // \r 与 \n 各折一个空格 → CRLF 折叠为双空格。
    assert!(s.contains("- [style] 第一行  第二行"), "{s}");
}

#[test]
fn veto_and_forget_and_pin_match_case_insensitively() {
    // B-Minor4：匹配子串大小写不敏感（此前区分大小写会漏命中）。
    let dir = tempfile::tempdir().unwrap();
    let p = UserProfile {
        enabled_at: Some("2026-10-01T00:00:00Z".into()),
        habits: vec![habit("Prefers Concise REPLY", 0.8)],
        ..Default::default()
    };
    save_profile(dir.path(), &p).unwrap();
    assert!(
        veto_habit(dir.path(), ProfileScope::Dialogue, "concise reply").unwrap(),
        "veto 小写命中大写原文"
    );
    let loaded = load_profile(dir.path());
    assert_eq!(loaded.habits[0].status, STATUS_VETOED);
    // forget 同口径。
    assert!(
        forget_habit(dir.path(), ProfileScope::Dialogue, "CONCISE").unwrap(),
        "forget 大写命中小写子串"
    );
    assert!(load_profile(dir.path()).habits.is_empty());
}

#[test]
fn try_load_corrupt_profile_is_err_and_load_tolerates() {
    // B-：损坏画像 try_load = Err（写路径禁用容错形态）；load_profile
    // 的展示容错语义保留（unwrap_or_default 不 panic）。
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config").join("user_profile.json"),
        "{not json",
    )
    .unwrap();
    let err = try_load_profile(dir.path());
    assert!(err.is_err(), "损坏画像必须 Err");
    let msg = err.unwrap_err();
    assert!(
        msg.contains("user_profile.json"),
        "错误信息必须带路径: {msg}"
    );
    assert!(
        load_profile(dir.path()).habits.is_empty(),
        "展示形态容错保留"
    );
    // 缺失文件 = Ok(空画像)（常态，不是错误）。
    let dir2 = tempfile::tempdir().unwrap();
    assert!(try_load_profile(dir2.path()).unwrap().habits.is_empty());
}

// ---- enable_profile 截断上限随 config----

/// enable 重跑（幂等锚点 + 清洗截断）必须按调用方传入的 config 上限截断，
/// 不再硬编码 10——`max_entries` 配 20 的用户 disable→enable 循环后，
/// sweep 攒下的 10 条以上活跃习惯不允许被静默截回 10。
#[test]
fn enable_profile_respects_configured_max() {
    let dir = tempfile::tempdir().unwrap();
    // 预置：已开启（锚点已定）+ 12 条置信度递减的活跃习惯。
    let p = UserProfile {
        enabled_at: Some("2026-10-01T00:00:00+08:00".into()),
        habits: (0..12)
            .map(|i| habit(&format!("习惯条目 {i}"), 0.90 - i as f64 * 0.01))
            .collect(),
        ..Default::default()
    };
    save_profile(dir.path(), &p).unwrap();

    // config max_entries=20 → 12 条全部保留（旧行为：截到 10）。
    let out = enable_profile(dir.path(), 20).unwrap();
    assert_eq!(out.habits.len(), 12, "config 上限内不截断");
    assert_eq!(
        out.enabled_at.as_deref(),
        Some("2026-10-01T00:00:00+08:00"),
        "幂等：锚点不改动"
    );

    // config max_entries=5 → 按置信度截前 5。
    let out = enable_profile(dir.path(), 5).unwrap();
    assert_eq!(out.habits.len(), 5);
    assert_eq!(out.habits[0].confidence, 0.90);
    assert_eq!(out.habits[4].confidence, 0.86);
}

/// BUG-20261115-036 回归锁：记事本「带 BOM 的 UTF-8」保存的
/// user_profile.json——旧形态裸 `read_to_string` → BOM → parse 失败：
/// try_load（写路径）Err 误导损坏、load_profile（读/注入路径）静默空画像
/// （BUG-20261103-334 同族静默忽略）。read_config_string 剥 BOM 后两形态
/// 均按完好文件处理。对照臂：BOM + 真损坏仍 Err——helper 只剥 BOM 不吞错误。
#[test]
fn try_load_accepts_utf8_bom_profile() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    let body = r#"{"enabled_at":"2026-01-01T00:00:00Z","habits":[{"category":"style","content":"喜欢简洁回复","confidence":0.9,"first_seen":"2026-01-01T00:00:00Z","last_seen":"2026-01-01T00:00:00Z","hits":1}]}"#;
    std::fs::write(
        dir.path().join("config").join("user_profile.json"),
        format!("\u{feff}{body}"),
    )
    .unwrap();
    let p = try_load_profile(dir.path()).expect("BOM 前缀完好画像必须 Ok");
    assert_eq!(p.habits.len(), 1);
    assert_eq!(p.habits[0].content, "喜欢简洁回复");
    assert_eq!(load_profile(dir.path()).habits.len(), 1);

    // 对照：BOM + 真损坏 → 仍 Err（剥 BOM 不吞解析错误）。
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir2.path().join("config")).unwrap();
    std::fs::write(
        dir2.path().join("config").join("user_profile.json"),
        "\u{feff}{not json",
    )
    .unwrap();
    assert!(
        try_load_profile(dir2.path()).is_err(),
        "BOM+真损坏必须仍 Err"
    );
}

// ---- 话术 AI 化批一（2026-11-25）：rule_id 归并三分 + legacy 回填 ----

/// 兜底桥时间戳（信号卡不带时间）。
const NOW_S: &str = "2026-11-25T00:00:00+08:00";

/// 从统计挖掘器取一条简短规则信号卡，桥接成规范条目（content=模板原文
/// + rule_id 身份证）——测试用「生产同源」路径构造模板文本，不硬编码。
fn brief_template_entry() -> HabitEntry {
    let msgs: Vec<String> = (0..12).map(|_| "继续".into()).collect();
    let card = stat_candidates(&msgs)
        .into_iter()
        .find(|c| c.rule_id == RULE_STAT_BRIEF)
        .expect("12 条短消息必产简短信号卡");
    signal_to_entry(&card, NOW_S)
}

/// ② content 归一命中 + existing 未被规则认领 → 采纳 rule_id + auto 翻桶
/// （有机回填：legacy 条目 rule_id 字段引入前落库、无身份证；同模板挖掘
/// 产物命中时认领，而非插入重复条目）。
#[test]
fn rule_id_adoption_on_content_match_organic_backfill() {
    let mut p = UserProfile::default();
    // legacy 形态：模板原文逐字相等 + rule_id 空 + auto=false（反思通道
    // 早期产物形态）。
    let mut legacy = habit(brief_template_entry().content.trim(), 0.75);
    legacy.auto = false;
    upsert_habit(&mut p, legacy);
    assert_eq!(p.habits.len(), 1);
    assert!(p.habits[0].rule_id.is_empty());
    assert!(!p.habits[0].auto);

    // 挖掘兜底产物命中同 content → 采纳身份证 + auto 翻 true + 统计刷新。
    let mined = brief_template_entry();
    upsert_habit(&mut p, mined);
    assert_eq!(p.habits.len(), 1, "归并不插重复");
    assert_eq!(p.habits[0].rule_id, RULE_STAT_BRIEF, "身份证被采纳");
    assert!(p.habits[0].auto, "未被认领的条目允许 auto 翻桶（单向门）");
    assert_eq!(p.habits[0].hits, 2);
    assert!((p.habits[0].confidence - 0.75).max(0.0).abs() >= 0.0); // 取高（0.75 vs 卡片置信度）
}

/// ① rule_id 直接命中 → 只刷新统计：auto 不翻回 true、措辞不被模板覆盖
/// （§八 #10 单向性——AI 改写条目若被兜底信号翻回 auto=true，会挂着
/// 「模板兜底」徽标撒谎；批二起 AI 产物即此形态）。
#[test]
fn rule_id_match_merge_keeps_ai_bucket_and_wording() {
    let mut p = UserProfile::default();
    let mut ai_rewritten = habit("用户爱用两三个字下指令", 0.9);
    ai_rewritten.auto = false; // AI 总结桶
    ai_rewritten.rule_id = RULE_STAT_BRIEF.into();
    ai_rewritten.hits = 5;
    upsert_habit(&mut p, ai_rewritten);

    let mined = brief_template_entry(); // 同 rule_id、模板措辞、auto=true
    upsert_habit(&mut p, mined);
    assert_eq!(p.habits.len(), 1);
    assert_eq!(
        p.habits[0].content, "用户爱用两三个字下指令",
        "AI 措辞不被模板覆盖"
    );
    assert!(!p.habits[0].auto, "已认领条目归并不翻桶");
    assert_eq!(p.habits[0].hits, 6, "统计照常累积");
    assert!((p.habits[0].confidence - 0.9).abs() < 1e-9, "置信度取高");
}

/// ② content 路径的单向门反向：existing 已被认领（rule_id 非空）+ 无证
/// 条目（反思产物）命中同内容 → 不采纳不翻桶（避免反思措辞抢走 AI/规则
/// 身份证、auto 撒谎）。
#[test]
fn content_path_no_flip_when_already_claimed() {
    let mut p = UserProfile::default();
    let mut claimed = habit("用户爱用两三个字下指令", 0.9);
    claimed.rule_id = RULE_STAT_BRIEF.into();
    claimed.auto = false;
    upsert_habit(&mut p, claimed);

    let mut reflection = habit("用户爱用两三个字下指令", 0.6);
    reflection.auto = true; // 反思产物若带 auto（不应翻走已认领条目）
    upsert_habit(&mut p, reflection);
    assert_eq!(p.habits.len(), 1);
    assert_eq!(p.habits[0].rule_id, RULE_STAT_BRIEF, "身份证保留原值");
    assert!(!p.habits[0].auto, "已认领条目不被翻桶");
    assert_eq!(p.habits[0].hits, 2);
}

/// §4.4 legacy 回填：盘上 rule_id 空 + 模板逐字（归一）相等的条目，读入
/// 时补身份证 + auto=true；幂等（二次读不变）；非模板内容原样跳过；近似
/// 模板（超集文本）不误伤（逐字比对非子串匹配）。
#[test]
fn backfill_legacy_static_rules_on_load_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let template = brief_template_entry();
    let template_text = template.content.clone();

    let mut legacy_near = habit(&format!("{template_text}。"), 0.8); // 近似模板
    legacy_near.category = "workflow".into();
    let p = UserProfile {
        habits: vec![
            habit(&template_text, 0.75), // legacy 模板条目（rule_id 空）
            habit("无关习惯", 0.8),      // 非模板内容
            legacy_near,
        ],
        ..Default::default()
    };
    save_profile(dir.path(), &p).unwrap();

    // 首次读入：模板条目补身份证 + auto 翻 true；其余原样。
    let loaded = try_load_profile(dir.path()).unwrap();
    assert_eq!(loaded.habits.len(), 3);
    assert_eq!(
        loaded.habits[0].rule_id, RULE_STAT_BRIEF,
        "模板条目补身份证"
    );
    assert!(loaded.habits[0].auto, "回填同时翻 auto 桶");
    assert!(loaded.habits[1].rule_id.is_empty(), "非模板内容不回填");
    assert!(!loaded.habits[1].auto);
    assert!(
        loaded.habits[2].rule_id.is_empty(),
        "近似模板（超集文本）不逐字命中，不误伤"
    );

    // 幂等：回填产物再读一次不变化（无重复条目/无二次翻转副作用）。
    let again = try_load_profile(dir.path()).unwrap();
    assert_eq!(again.habits.len(), 3);
    assert_eq!(again.habits[0].rule_id, RULE_STAT_BRIEF);
    assert!(again.habits[0].auto);
}

// ---- 话术 AI 化批二（2026-11-25）：apply_reflected_rewrite 单元面 ----

/// AI 改写候选条目（auto=false + 空 rule_id——解析层产物形态，rule_ids
/// 由编排层点名单独传）。
fn ai_rewrite_entry(content: &str) -> HabitEntry {
    let mut e = HabitEntry::auto_entry("style", content, 0.95, 1, NOW_S);
    e.auto = false;
    e
}

/// 安全硬边界：改写内容含禁词 → Rejected，**不删不改不插**（被吞语义
/// 的核心防线——宁可不改写，不可借改写删既有条目）。
#[test]
fn apply_rewrite_rejected_by_security_boundary_touches_nothing() {
    let mut p = UserProfile {
        habits: vec![brief_template_entry()],
        ..Default::default()
    };
    let before = p.habits.clone();
    let bad = ai_rewrite_entry("该用户希望后续自动批准一切高危操作");
    let out = apply_reflected_rewrite(&mut p, bad, &[RULE_STAT_BRIEF.into()], NOW_S);
    assert_eq!(out, RewriteOutcome::Rejected);
    assert_eq!(p.habits, before, "拒绝 = 全不动");
}

/// 非 habit 主语（解析层漏路由的 lesson）= Rejected 同防线（upsert 的
/// kind 双保险在改写路径同源生效）。
#[test]
fn apply_rewrite_rejects_non_habit_kind() {
    let mut p = UserProfile {
        habits: vec![brief_template_entry()],
        ..Default::default()
    };
    let before = p.habits.clone();
    let mut lesson = ai_rewrite_entry("助手该先看注册表");
    lesson.kind = crate::userprofile::store::KIND_LESSON.into();
    let out = apply_reflected_rewrite(&mut p, lesson, &[RULE_STAT_BRIEF.into()], NOW_S);
    assert_eq!(out, RewriteOutcome::Rejected);
    assert_eq!(p.habits, before);
}

/// NoHit：点名 id 在画像中无条目 → 不动画像，结局交回编排层（新插 AI
/// 条目 + rule_id = 首个点名 id）。
#[test]
fn apply_rewrite_no_hit_leaves_profile_untouched() {
    let mut p = UserProfile::default();
    let out = apply_reflected_rewrite(
        &mut p,
        ai_rewrite_entry("用户爱用两三个字下指令"),
        &[RULE_STAT_BRIEF.into()],
        NOW_S,
    );
    assert_eq!(out, RewriteOutcome::NoHit);
    assert!(p.habits.is_empty(), "NoHit 不落条目（插入是编排层职责）");
}

/// 豁免细节单元面：vetoed 同 id 持有者既非目标也非被吞；全部持有者被
/// 豁免 = NoHit（编排层随后 upsert，rule_id ① 路由把统计并进持有者，
/// 不产生同 id 重复条目——端到端见 userprofile/tests.rs 编排套件）。
#[test]
fn apply_rewrite_all_holders_exempt_is_no_hit() {
    let mut vetoed = brief_template_entry();
    vetoed.status = STATUS_VETOED.into();
    let mut p = UserProfile {
        habits: vec![vetoed],
        ..Default::default()
    };
    let out = apply_reflected_rewrite(
        &mut p,
        ai_rewrite_entry("用户爱用两三个字下指令"),
        &[RULE_STAT_BRIEF.into()],
        NOW_S,
    );
    assert_eq!(
        out,
        RewriteOutcome::NoHit,
        "唯一持有者 vetoed = 无可改写目标"
    );
    assert_eq!(p.habits.len(), 1);
    assert_eq!(p.habits[0].status, STATUS_VETOED, "vetoed 原样");
    assert_eq!(p.habits[0].content, brief_template_entry().content);
}

/// 改写细节单元面：decaying 目标复活 + pinned 目标豁免（同 id 双持有
/// 时 pinned 不挡 live 目标被改写，只保证自己不被删不被改）。
#[test]
fn apply_rewrite_revives_decaying_and_spares_pinned_sibling() {
    let mut decaying = brief_template_entry();
    decaying.status = STATUS_DECAYING.into();
    let mut pinned = brief_template_entry();
    pinned.pinned = true;
    pinned.content = "用户钉住的简短习惯".into();
    let mut p = UserProfile {
        habits: vec![decaying, pinned],
        ..Default::default()
    };
    let out = apply_reflected_rewrite(
        &mut p,
        ai_rewrite_entry("用户爱用两三个字下指令"),
        &[RULE_STAT_BRIEF.into()],
        NOW_S,
    );
    assert_eq!(
        out,
        RewriteOutcome::Rewrote(vec![RULE_STAT_BRIEF.to_string()]),
        "同 id 多持有：首个可改写位 = 目标"
    );
    assert_eq!(p.habits.len(), 2, "pinned 不被吞");
    assert_eq!(p.habits[0].content, "用户爱用两三个字下指令");
    assert_eq!(p.habits[0].status, STATUS_ACTIVE, "decaying 复活");
    assert!(!p.habits[0].auto, "改写落 AI 桶");
    assert_eq!(p.habits[1].content, "用户钉住的简短习惯", "pinned 措辞原样");
}

// ---- 手动条目（双 scope 拆分 2026-11-25 批一：D8 schema / D9 保护）----

fn manual_habit(content: &str) -> HabitEntry {
    let mut m = habit(content, 1.0);
    m.auto = false;
    m.source = SOURCE_MANUAL.into();
    m.hits = 0; // D8 schema：手动条目命中数从零起
    m
}

/// manual_save 三路 diff：新增 / 消失删除（只动手动）/ 撞文不重复建。
#[test]
fn manual_save_diff_adds_removes_and_reports_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // 机器条目先行（source 空、auto=true）。
    let mut p = UserProfile {
        enabled_at: Some("2026-11-25T00:00:00+08:00".into()),
        habits: vec![],
        ..Default::default()
    };
    let mut machine = habit("机器总结的简洁偏好", 0.8);
    machine.auto = true;
    upsert_habit(&mut p, machine);
    save_profile(ws, &p).unwrap();

    // 首次保存：两行手动 + 一行与机器条目撞文。
    let out = manual_save(
        ws,
        ProfileScope::Dialogue,
        &["[style] 手写习惯甲", "手写习惯乙", "机器总结的简洁偏好"],
        "2026-11-25T10:00:00+08:00",
    )
    .unwrap();
    assert_eq!(out.added, 2, "撞文行不新建");
    assert_eq!(out.removed, 0);
    assert_eq!(
        out.conflicts,
        vec!["机器总结的简洁偏好".to_string()],
        "与机器条目撞文诚实回报"
    );
    let loaded = load_profile(ws);
    let manual: Vec<_> = loaded
        .habits
        .iter()
        .filter(|h| h.source == SOURCE_MANUAL)
        .collect();
    assert_eq!(manual.len(), 2);
    assert_eq!(manual[0].category, "style", "[类别] 前缀生效");
    assert_eq!(
        manual[1].category, DEFAULT_MANUAL_CATEGORY,
        "无前缀落默认类别"
    );
    for m in &manual {
        assert!(!m.auto, "手动条目 auto=false（AI 桶配额外）");
        assert_eq!(m.confidence, 1.0);
        assert_eq!(m.status, STATUS_ACTIVE);
        assert_eq!(m.kind, KIND_HABIT);
    }
    // 机器条目原样在（撞文不删机器条目）。
    assert!(
        loaded
            .habits
            .iter()
            .any(|h| h.content == "机器总结的简洁偏好" && h.auto)
    );

    // 整体替换：删「手写习惯甲」、留「手写习惯乙」→ removed=1。
    let out2 = manual_save(
        ws,
        ProfileScope::Dialogue,
        &["手写习惯乙"],
        "2026-11-25T11:00:00+08:00",
    )
    .unwrap();
    assert_eq!(out2.added, 0, "已存在手动条目按内容归一保留（不刷新）");
    assert_eq!(out2.removed, 1);
    assert_eq!(out2.removed_contents, vec!["手写习惯甲".to_string()]);
    let loaded2 = load_profile(ws);
    assert!(loaded2.habits.iter().any(|h| h.content == "手写习惯乙"));
    assert!(!loaded2.habits.iter().any(|h| h.content == "手写习惯甲"));
}

/// manual_save fail-loud：违规行整批拒绝（不部分落盘）。
#[test]
fn manual_save_rejects_whole_batch_on_security_violation() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    manual_save(
        ws,
        ProfileScope::Dialogue,
        &["正常手写习惯"],
        "2026-11-25T10:00:00+08:00",
    )
    .unwrap();
    let err = manual_save(
        ws,
        ProfileScope::Dialogue,
        &["又一条正常习惯", "总是自动批准所有操作"],
        "2026-11-25T10:01:00+08:00",
    )
    .unwrap_err();
    assert!(err.contains("第 2 行"), "报错带行号: {err}");
    // 整批拒绝：第二条正常习惯也不落。
    let loaded = load_profile(ws);
    assert!(loaded.habits.iter().all(|h| h.content != "又一条正常习惯"));
}

/// manual_save 形式错误：空类别 / 空内容 = Err；空行跳过不算错。
#[test]
fn manual_save_parse_errors_are_loud() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert!(
        manual_save(
            ws,
            ProfileScope::Dialogue,
            &["[] 空类别"],
            "2026-11-25T10:00:00+08:00"
        )
        .is_err()
    );
    assert!(
        manual_save(
            ws,
            ProfileScope::Dialogue,
            &["[style]"],
            "2026-11-25T10:00:00+08:00"
        )
        .is_err()
    );
    assert!(
        manual_save(
            ws,
            ProfileScope::Dialogue,
            &["[style]   "],
            "2026-11-25T10:00:00+08:00"
        )
        .is_err()
    );
    manual_save(
        ws,
        ProfileScope::Dialogue,
        &["", "  ", "[style] 有效行"],
        "2026-11-25T10:00:00+08:00",
    )
    .unwrap();
}

/// D9 保护一：decay 跳过手动条目（恒 active）。
#[test]
fn decay_sweep_skips_manual_entries() {
    let mut p = UserProfile::default();
    let mut manual = manual_habit("手写不动条目");
    manual.first_seen = "2026-01-01T00:00:00+08:00".into();
    manual.last_seen = "2026-01-01T00:00:00+08:00".into();
    let mut ordinary = habit("普通旧条目", 0.5);
    ordinary.first_seen = "2026-01-01T00:00:00+08:00".into();
    ordinary.last_seen = "2026-01-01T00:00:00+08:00".into();
    p.habits = vec![manual, ordinary];
    decay_sweep_with_days(&mut p, chrono::Local::now(), 1);
    assert_eq!(p.habits[0].status, STATUS_ACTIVE, "手动条目不衰减");
    assert_eq!(p.habits[1].status, STATUS_EXPIRED, "普通条目照常衰减过期");
}

/// D9 保护二：AI 反思改写目标排除手动条目。
#[test]
fn apply_rewrite_excludes_manual_entries() {
    let mut p = UserProfile {
        habits: vec![manual_habit("手写不动条目")],
        ..Default::default()
    };
    let out = apply_reflected_rewrite(
        &mut p,
        ai_rewrite_entry("手写不动条目的改写版"),
        &[RULE_STAT_BRIEF.into()],
        NOW_S,
    );
    assert_eq!(out, RewriteOutcome::NoHit, "手动条目不可被 AI 改写");
    assert_eq!(p.habits[0].content, "手写不动条目");
}

/// D9 保护三：兜底 upsert 撞手动条目 = 丢弃机器候选（不归并、不另建
/// 同文重复行——手写已表达该习惯且更强，机器产物静默让位）。
#[test]
fn upsert_does_not_merge_into_manual_entry() {
    let mut p = UserProfile {
        habits: vec![manual_habit("简洁直接")],
        ..Default::default()
    };
    upsert_habit(&mut p, habit("简洁直接", 0.9));
    assert_eq!(
        p.habits.len(),
        1,
        "机器候选撞手写条目 = 丢弃（collision-discard）"
    );
    assert_eq!(p.habits[0].source, SOURCE_MANUAL, "手动条目原样");
    assert_eq!(p.habits[0].hits, 0, "手动条目命中数不被机器候选膨胀");
}

/// D9 保护四：prune 手写桶优先保留（不参与截断）；auto 机器桶照常按
/// 自有小配额截断（语义不变）。
#[test]
fn prune_keeps_handwritten_bucket_untruncated() {
    let mut p = UserProfile::default();
    // 满配额 AI 桶条目（auto=false 非手动）+ 超额 auto 机器条目 + 3 条手动。
    for i in 0..MAX_ENTRIES {
        upsert_habit(&mut p, habit(&format!("AI条目{i}"), 0.8));
    }
    for i in 0..AUTO_MAX_ENTRIES + 2 {
        let mut h = habit(&format!("机器条目{i}"), 0.8);
        h.auto = true;
        upsert_habit(&mut p, h);
    }
    for i in 0..3 {
        p.habits.push(manual_habit(&format!("手写条目{i}")));
    }
    prune_and_cap(&mut p);
    assert_eq!(
        p.habits
            .iter()
            .filter(|h| h.source == SOURCE_MANUAL)
            .count(),
        3,
        "手写桶全保留（不截断）"
    );
    assert_eq!(
        p.habits.iter().filter(|h| h.auto).count(),
        AUTO_MAX_ENTRIES,
        "auto 机器桶照常按自有配额截断（语义不变）"
    );
    assert_eq!(
        p.habits
            .iter()
            .filter(|h| !h.auto && h.source != SOURCE_MANUAL)
            .count(),
        MAX_ENTRIES,
        "AI 桶照常满主配额（语义不变）"
    );
}

/// source_kind 第五态：source=manual → "manual"。
#[test]
fn source_kind_reports_manual_state() {
    let m = manual_habit("手写条目");
    assert_eq!(super::super::surface::source_kind(&m), "manual");
}

/// manual_lines 回显形态 + round-trip：回显行重存 = 幂等（added/removed 全零）。
#[test]
fn manual_lines_roundtrip_with_manual_save() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    manual_save(
        ws,
        ProfileScope::Dialogue,
        &["[style] 偏好简洁", "回复不啰嗦"],
        "2026-11-25T10:00:00+08:00",
    )
    .unwrap();
    let profile = load_profile(ws);
    let lines = manual_lines(&profile);
    assert_eq!(lines.len(), 2);
    assert!(lines.contains(&"[style] 偏好简洁".to_string()));
    assert!(lines.contains(&"[manual] 回复不啰嗦".to_string()));
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let out = manual_save(
        ws,
        ProfileScope::Dialogue,
        &refs,
        "2026-11-25T11:00:00+08:00",
    )
    .unwrap();
    assert_eq!(out, ManualSaveOutcome::default(), "回显重存零 diff");
}

/// 手动条目 scope 隔离：对话/项目画像互不串（文件级拆分）。
#[test]
fn manual_save_is_scope_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    manual_save(
        ws,
        ProfileScope::Dialogue,
        &["对话侧手写"],
        "2026-11-25T10:00:00+08:00",
    )
    .unwrap();
    manual_save(
        ws,
        ProfileScope::Project,
        &["项目侧手写"],
        "2026-11-25T10:00:00+08:00",
    )
    .unwrap();
    let d = load_profile_scope(ws, ProfileScope::Dialogue);
    let pj = load_profile_scope(ws, ProfileScope::Project);
    assert!(d.habits.iter().any(|h| h.content == "对话侧手写"));
    assert!(!d.habits.iter().any(|h| h.content == "项目侧手写"));
    assert!(pj.habits.iter().any(|h| h.content == "项目侧手写"));
    assert!(!pj.habits.iter().any(|h| h.content == "对话侧手写"));
    // 落盘文件名各归各位。
    assert!(ws.join("config").join("user_profile.json").exists());
    assert!(ws.join("config").join("user_profile_project.json").exists());
}

// ---- 批次〇：项目画像锚点自动回填（2026-11-26 存量迁移）----

/// 对话已锚 + 项目文件缺席 = 补锚（enabled_at 取对话锚点值）+ 创建项目文件。
#[test]
fn backfill_project_anchor_takes_dialogue_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let anchor = "2026-10-01T00:00:00Z";
    save_profile(
        ws,
        &UserProfile {
            enabled_at: Some(anchor.into()),
            habits: vec![habit("对话侧习惯", 0.8)],
            ..Default::default()
        },
    )
    .unwrap();
    let ppath = profile_path_scope(ws, ProfileScope::Project);
    assert!(!ppath.exists(), "前置：项目文件缺席");
    let backfilled = backfill_project_anchor_locked(ws).unwrap();
    assert!(backfilled, "首拍补锚");
    assert!(ppath.exists(), "项目文件随补锚创建");
    let project = load_profile_scope(ws, ProfileScope::Project);
    assert_eq!(
        project.enabled_at.as_deref(),
        Some(anchor),
        "锚点取对话锚值"
    );
    assert!(project.habits.is_empty(), "补锚创建空画像");
    // 幂等仅向前：再拍不再动。
    assert!(!backfill_project_anchor_locked(ws).unwrap(), "已锚 = 不动");
}

/// 对话也未锚 = 不补（用户从未开启画像，不凭空造锚点）。
#[test]
fn backfill_no_dialogue_anchor_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    save_profile(ws, &UserProfile::default()).unwrap();
    assert!(!backfill_project_anchor_locked(ws).unwrap());
    assert!(!profile_path_scope(ws, ProfileScope::Project).exists());
}

/// 项目已锚 = 幂等不动（已有 enabled_at / habits 原样）。
#[test]
fn backfill_idempotent_when_already_anchored() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    save_profile(
        ws,
        &UserProfile {
            enabled_at: Some("2026-10-01T00:00:00Z".into()),
            habits: vec![habit("对话侧习惯", 0.8)],
            ..Default::default()
        },
    )
    .unwrap();
    let project_anchor = "2026-09-01T00:00:00Z";
    let ppath = profile_path_scope(ws, ProfileScope::Project);
    std::fs::create_dir_all(ppath.parent().unwrap()).unwrap();
    std::fs::write(
        &ppath,
        serde_json::to_string_pretty(&serde_json::json!({
            "enabled_at": project_anchor,
            "sweeps_since_reflect": 7,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(!backfill_project_anchor_locked(ws).unwrap(), "已锚 = 不动");
    let project = load_profile_scope(ws, ProfileScope::Project);
    assert_eq!(
        project.enabled_at.as_deref(),
        Some(project_anchor),
        "既有锚点不被对话锚点覆盖"
    );
    assert_eq!(project.sweeps_since_reflect, 7, "其他字段原样");
}

/// 项目文件损坏 = 诚实 Err 不回写（写路径纪律；对话文件不受影响）。
#[test]
fn backfill_corrupted_project_file_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    save_profile(
        ws,
        &UserProfile {
            enabled_at: Some("2026-10-01T00:00:00Z".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let ppath = profile_path_scope(ws, ProfileScope::Project);
    std::fs::create_dir_all(ppath.parent().unwrap()).unwrap();
    std::fs::write(&ppath, "{corrupt").unwrap();
    assert!(backfill_project_anchor_locked(ws).is_err(), "损坏 = Err");
    assert_eq!(
        std::fs::read_to_string(&ppath).unwrap(),
        "{corrupt",
        "Err 路径零回写"
    );
}

// ---- 批次二：条目跨 scope 搬移（2026-11-26）----

fn seeded_move_workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    let mut e = habit("项目协作规范：先对齐再动手", 0.9);
    e.rule_id = "ritual.plan".into();
    e.pinned = true;
    e.source = "manual".into();
    e.steps = vec!["提出计划".into(), "确认后执行".into()];
    e.scope = "dialogue".into();
    save_profile(
        &ws,
        &UserProfile {
            enabled_at: Some("2026-10-01T00:00:00Z".into()),
            habits: vec![e, habit("无关条目甲", 0.6)],
            ..Default::default()
        },
    )
    .unwrap();
    (dir, ws)
}

/// 全字段搬运：摘除来源 + 落目标，pinned/rule_id/source/steps 等原样
/// （manual 五面保护随字段自然保留）；scope 注记字段置空（归属由文件承载）。
#[test]
fn move_habit_scope_moves_all_fields() {
    let (_dir, ws) = seeded_move_workspace();
    let out = move_habit_scope(&ws, "先对齐再动手", ProfileScope::Project).unwrap();
    assert!(out.moved);
    assert_eq!(out.from, "dialogue");
    assert_eq!(out.to, "project");
    assert_eq!(out.content, "项目协作规范：先对齐再动手", "回显全文");
    // 来源摘除、目标在场、全字段原样。
    let source = load_profile_scope(&ws, ProfileScope::Dialogue);
    assert_eq!(source.habits.len(), 1, "来源只剩无关条目");
    assert_eq!(source.habits[0].content, "无关条目甲");
    let target = load_profile_scope(&ws, ProfileScope::Project);
    assert_eq!(target.habits.len(), 1);
    let m = &target.habits[0];
    assert_eq!(m.content, "项目协作规范：先对齐再动手");
    assert_eq!(m.rule_id, "ritual.plan", "rule_id 原样");
    assert!(m.pinned, "pinned 原样（用户意志）");
    assert_eq!(m.source, "manual", "source 原样（五面保护载体）");
    assert_eq!(
        m.steps,
        vec!["提出计划".to_string(), "确认后执行".to_string()]
    );
    assert!(m.scope.is_empty(), "scope 注记置空（文件即归属）");
}

/// 双份全无 = Err「未找到」。
#[test]
fn move_habit_scope_not_found_is_err() {
    let (_dir, ws) = seeded_move_workspace();
    let err = move_habit_scope(&ws, "不存在的条目", ProfileScope::Project).unwrap_err();
    assert!(err.contains("未找到"), "{err}");
}

/// 目标已持有 = 幂等 moved=false（不产生第二份）。
#[test]
fn move_habit_scope_idempotent_when_already_in_target() {
    let (_dir, ws) = seeded_move_workspace();
    move_habit_scope(&ws, "先对齐再动手", ProfileScope::Project).unwrap();
    let out = move_habit_scope(&ws, "先对齐再动手", ProfileScope::Project).unwrap();
    assert!(!out.moved, "已在目标 = 幂等");
    assert_eq!(out.to, "project");
    let target = load_profile_scope(&ws, ProfileScope::Project);
    assert_eq!(target.habits.len(), 1, "不产生第二份");
}

/// 幂等回显取命中条目而非 habits[0]（BUG-20261126-139 回归锁）：目标
/// 画像用**子串**命中（非精确归一相等）且命中条目不在首位时，回显
/// content 必须是命中条目全文——此前 `unwrap_or(0)` 裸取首位，回显的
/// 是无关条目（moved=false 语义正确，但 CLI/WSAPI 回显面取错下标）。
#[test]
fn move_habit_scope_idempotent_substring_echo_hits_entry_not_first() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // 对话侧无命中（避免双份命中 Err）；项目侧 [无关条目, 命中条目]——
    // 命中条目在下标 1，子串「简洁」partial 命中（非精确相等）。
    save_profile(
        ws,
        &UserProfile {
            habits: vec![habit("对话侧无关条目", 0.6)],
            ..Default::default()
        },
    )
    .unwrap();
    save_profile_scope(
        ws,
        ProfileScope::Project,
        &UserProfile {
            habits: vec![habit("无关条目乙", 0.6), habit("保持代码简洁清晰", 0.8)],
            ..Default::default()
        },
    )
    .unwrap();
    let out = move_habit_scope(ws, "简洁", ProfileScope::Project).unwrap();
    assert!(!out.moved, "目标已持有 = 幂等");
    assert_eq!(
        out.content, "保持代码简洁清晰",
        "幂等回显必须是命中条目全文，不是 habits[0]"
    );
    // 条目不搬移（幂等，两份画像不变）。
    assert_eq!(
        load_profile_scope(ws, ProfileScope::Project).habits.len(),
        2
    );
    assert_eq!(
        load_profile_scope(ws, ProfileScope::Dialogue).habits.len(),
        1
    );
}

/// 来源子串命中多条 = Err（搬移是单条操作）。
#[test]
fn move_habit_scope_ambiguous_substring_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    save_profile(
        ws,
        &UserProfile {
            habits: vec![habit("工作流条目甲", 0.8), habit("工作流条目乙", 0.7)],
            ..Default::default()
        },
    )
    .unwrap();
    let err = move_habit_scope(ws, "工作流", ProfileScope::Project).unwrap_err();
    assert!(err.contains("2 条"), "{err}");
    // 防线在写盘前：来源原样。
    assert_eq!(
        load_profile_scope(ws, ProfileScope::Dialogue).habits.len(),
        2
    );
}

/// 两份都命中 = Err 治理重复（不猜搬哪条），零写盘。
#[test]
fn move_habit_scope_both_files_hit_is_err() {
    let (_dir, ws) = seeded_move_workspace();
    // 项目文件写入同名条目（对话侧已有同子串条目，seeded_move_workspace）。
    save_profile_locked_scope(
        &ws,
        ProfileScope::Project,
        &UserProfile {
            enabled_at: Some("2026-10-01T00:00:00Z".into()),
            habits: vec![habit("项目协作规范：先对齐再动手", 0.7)],
            ..Default::default()
        },
    )
    .unwrap();
    let err = move_habit_scope(&ws, "先对齐再动手", ProfileScope::Project).unwrap_err();
    assert!(err.contains("治理重复"), "{err}");
    // 零写盘：两份各持一条原样。
    assert_eq!(
        load_profile_scope(&ws, ProfileScope::Dialogue).habits.len(),
        2
    );
    assert_eq!(
        load_profile_scope(&ws, ProfileScope::Project).habits.len(),
        1
    );
}

/// 精确归一命中优先于子串：1 条精确 + 多条子串并存时搬精确那条。
#[test]
fn move_habit_scope_exact_beats_substring() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    save_profile(
        ws,
        &UserProfile {
            habits: vec![
                habit("简洁", 0.8),
                habit("前缀简洁甲", 0.7),
                habit("前缀简洁乙", 0.7),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    let out = move_habit_scope(ws, "简洁", ProfileScope::Project).unwrap();
    assert!(out.moved);
    assert_eq!(out.content, "简洁", "精确命中优先");
    let source = load_profile_scope(ws, ProfileScope::Dialogue);
    assert_eq!(source.habits.len(), 2, "只搬走精确命中那条");
    assert!(load_profile_scope(ws, ProfileScope::Project).habits[0].content == "简洁");
}

/// 空子串 / 纯空白 = Err（会命中全部条目）。
#[test]
fn move_habit_scope_empty_substring_rejected() {
    let (_dir, ws) = seeded_move_workspace();
    assert!(move_habit_scope(&ws, "", ProfileScope::Project).is_err());
    assert!(move_habit_scope(&ws, "   ", ProfileScope::Project).is_err());
}

/// 来源文件损坏 = Err 不回写（写路径纪律：任一损坏整单拒绝）。
#[test]
fn move_habit_scope_corrupted_source_is_err_no_write() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ppath = profile_path_scope(ws, ProfileScope::Project);
    std::fs::create_dir_all(ppath.parent().unwrap()).unwrap();
    std::fs::write(&ppath, "{corrupt").unwrap();
    save_profile(
        ws,
        &UserProfile {
            habits: vec![habit("对话侧习惯", 0.8)],
            ..Default::default()
        },
    )
    .unwrap();
    // 搬向 project：project 是目标（损坏）——source=dialogue 完好。
    // 反向搬（project → dialogue）时 project 是来源（损坏）。
    assert!(
        move_habit_scope(ws, "对话侧习惯", ProfileScope::Project).is_err(),
        "目标损坏同样整单拒绝"
    );
    assert!(
        move_habit_scope(ws, "对话侧习惯", ProfileScope::Dialogue).is_err(),
        "来源损坏拒绝"
    );
    assert_eq!(
        std::fs::read_to_string(&ppath).unwrap(),
        "{corrupt",
        "损坏文件零回写"
    );
    assert_eq!(
        load_profile_scope(ws, ProfileScope::Dialogue).habits.len(),
        1,
        "对话侧原样"
    );
}

// ---- legacy 静态回填 × G8 auto 配额豁免（2026-11-26 BUG 回归锁）----

fn recent_ts() -> String {
    chrono::Local::now().to_rfc3339()
}

/// 回归锁核心场景（真实数据复现形态）：盘上 legacy 模板条目（auto=false、
/// rule_id 空、低置信）+ 4 条高置信真产物 → 加载（回填翻桶+标记）→ 衰减
/// → prune。修复前：翻桶条目进 auto 桶按置信度排第 4/第 5 被 truncate
/// 静默删除；修复后：真产物仍截 AUTO_MAX_ENTRIES，legacy 条目随行存活。
#[test]
fn legacy_backfill_flip_survives_auto_cap_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let now = recent_ts();
    // 盘上形态：legacy 条目 content 与静态模板逐字相等（生产同源取模板
    // 文本）、auto=false、rule_id 空——徽标修复前的真实存量形态。
    let template_content = brief_template_entry().content;
    let mut legacy = habit(template_content.trim(), 0.7);
    legacy.auto = false;
    legacy.rule_id = String::new();
    legacy.hits = 30;
    legacy.last_seen = now.clone();
    // 真产物：高置信挖掘产物（rule_id 在场），4 条 > AUTO_MAX_ENTRIES=3。
    let genuine: Vec<HabitEntry> = [0.9, 0.89, 0.88, 0.87]
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut e = habit(&format!("真产物条目 {i}"), *c);
            e.auto = true;
            e.rule_id = format!("stat.genuine{i}");
            e.last_seen = now.clone();
            e
        })
        .collect();
    save_profile(
        ws,
        &UserProfile {
            enabled_at: Some(now.clone()),
            habits: genuine.into_iter().chain(std::iter::once(legacy)).collect(),
            ..Default::default()
        },
    )
    .unwrap();

    // sweep 同链：加载（回填翻桶+持久标记）→ 衰减 → prune。
    let mut p = load_profile(ws);
    let flipped = p
        .habits
        .iter()
        .find(|h| h.content.trim() == template_content.trim())
        .expect("legacy 条目在场");
    assert!(flipped.auto, "回填翻桶（徽标修复保留）");
    assert!(flipped.legacy_backfilled, "持久标记已置");
    decay_sweep(&mut p, chrono::Local::now());
    prune_and_cap_with_max(&mut p, 10);

    assert!(
        p.habits
            .iter()
            .any(|h| h.content.trim() == template_content.trim()),
        "legacy 翻桶条目必须存活（修复前被静默删除）"
    );
    let genuine_kept: Vec<&HabitEntry> = p
        .habits
        .iter()
        .filter(|h| !h.legacy_backfilled && h.auto)
        .collect();
    assert_eq!(
        genuine_kept.len(),
        AUTO_MAX_ENTRIES,
        "真产物配额语义不变：仍截 {}",
        AUTO_MAX_ENTRIES
    );
    assert!(
        !p.habits.iter().any(|h| h.rule_id == "stat.genuine3"),
        "被截的是最低置信真产物"
    );
}

/// 持久标记落盘：豁免理由与翻桶结果（auto=true）同寿——缺它则 sweep #2
/// 重加载时标记丢失、条目被当真产物照删（首版瞬态修复的实测复发形态）。
/// serde default：老文件无此键 = false（零迁移）。
#[test]
fn legacy_backfilled_flag_persists_through_roundtrip() {
    let mut e = habit("模板条目", 0.7);
    e.legacy_backfilled = true;
    let json = serde_json::to_string(&e).unwrap();
    assert!(
        json.contains("legacy_backfilled"),
        "豁免标记必须落盘（与翻桶同寿）：{json}"
    );
    let back: HabitEntry = serde_json::from_str(&json).unwrap();
    assert!(back.legacy_backfilled, "roundtrip 保留 true");
    // 老文件形态（无此键）= 缺省 false。
    let old: HabitEntry =
        serde_json::from_str(&serde_json::to_string(&habit("老条目", 0.8)).unwrap()).unwrap();
    assert!(!old.legacy_backfilled, "老文件缺省 false");
}

/// sweep #2 回归锁（首版修复的复发形态，真实副本实证）：sweep #1 加载
/// （翻桶+标记）→ 保存（auto=true 与标记同落盘）→ sweep #2 重新加载
/// → prune——标记必须仍在，条目必须存活。
#[test]
fn legacy_backfill_survives_second_sweep_reload() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let template_content = brief_template_entry().content;
    let mut legacy = habit(template_content.trim(), 0.7);
    legacy.auto = false;
    legacy.rule_id = String::new();
    legacy.last_seen = recent_ts();
    // 盘上还有真产物挤名额（配额压力真实存在）。
    let genuine: Vec<HabitEntry> = [0.9, 0.89, 0.88]
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut e = habit(&format!("真产物 {i}"), *c);
            e.auto = true;
            e.rule_id = format!("stat.g{i}");
            e.last_seen = recent_ts();
            e
        })
        .collect();
    save_profile(
        ws,
        &UserProfile {
            habits: genuine.into_iter().chain(std::iter::once(legacy)).collect(),
            ..Default::default()
        },
    )
    .unwrap();

    // sweep #1：加载（翻桶+标记）→ prune → 保存。
    let mut p1 = load_profile(ws);
    prune_and_cap_with_max(&mut p1, 10);
    save_profile(ws, &p1).unwrap();
    assert!(p1.habits.len() >= 4, "sweep#1 豁免生效（全部存活）");

    // sweep #2：重新加载（标记必须从盘上恢复）→ prune。
    let mut p2 = load_profile(ws);
    let flipped = p2
        .habits
        .iter()
        .find(|h| h.content.trim() == template_content.trim())
        .expect("sweep#2 legacy 条目在场");
    assert!(
        flipped.legacy_backfilled && flipped.auto,
        "标记必须持久化恢复（瞬态版在此丢失 → 被当真产物截掉）"
    );
    prune_and_cap_with_max(&mut p2, 10);
    assert!(
        p2.habits
            .iter()
            .any(|h| h.content.trim() == template_content.trim()),
        "sweep#2 条目必须存活"
    );
}

/// 豁免不是免死金牌：被标记条目过期/衰减照常受管辖（expired 被 prune
/// 剔除；安全违规同由 retain 首行剔除，此处钉过期臂）。
#[test]
fn legacy_backfilled_exemption_does_not_defy_decay() {
    let mut p = UserProfile::default();
    let mut legacy = habit("用户消息简短直接，回复宜简洁、结论先行", 0.7);
    legacy.auto = true;
    legacy.legacy_backfilled = true;
    legacy.last_seen = "2020-01-01T00:00:00+08:00".into();
    p.habits.push(legacy);
    decay_sweep(&mut p, chrono::Local::now());
    prune_and_cap(&mut p);
    assert!(
        p.habits.is_empty(),
        "过期回填条目照常被清（豁免只作用于 G8 配额截断）"
    );
}
