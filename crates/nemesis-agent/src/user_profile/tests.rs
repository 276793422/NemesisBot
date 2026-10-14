//! user_profile 单测：安全边界过滤 / upsert 合并 / 衰减状态机 / 渲染 / 统计候选。

use super::*;

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

// ---- 统计候选 ----

#[test]
fn stat_candidates_language_rule() {
    let msgs: Vec<String> = (0..12)
        .map(|_| "帮我把登录模块重构一下，注意兼容旧接口".into())
        .collect();
    let out = stat_candidates(&msgs, "2026-10-14T00:00:00+08:00");
    assert!(
        out.iter()
            .any(|h| h.category == "style" && h.content.contains("中文")),
        "{out:?}"
    );
}

#[test]
fn stat_candidates_short_messages_rule() {
    let msgs: Vec<String> = (0..12).map(|_| "继续".into()).collect();
    let out = stat_candidates(&msgs, "2026-10-14T00:00:00+08:00");
    assert!(out.iter().any(|h| h.content.contains("简洁")), "{out:?}");
}

#[test]
fn stat_candidates_correction_rule() {
    let mut msgs: Vec<String> = (0..12).map(|_| "继续".into()).collect();
    for (i, m) in msgs.iter_mut().take(4).enumerate() {
        *m = format!("不对，我意思是方案{i}");
    }
    let out = stat_candidates(&msgs, "2026-10-14T00:00:00+08:00");
    assert!(out.iter().any(|h| h.category == "correction"), "{out:?}");
}

#[test]
fn stat_candidates_empty_input_is_empty() {
    assert!(stat_candidates(&[], "x").is_empty());
}

// ---- 往返 ----

#[test]
fn profile_roundtrip_via_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = UserProfile {
        enabled_at: Some("2026-10-14T00:00:00+08:00".into()),
        last_sweep_ms: Some(1_760_000_000_000),
        habits: vec![habit("用户偏好结论先行", 0.85)],
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
}

// ---- sweep 证据窗（滚动 24h）----

/// 2026-10-01T00:00:00Z = epoch 1790812800 秒（锚点常用于以下断言）。
const ANCHOR_MS: i64 = 1_790_812_800_000;

#[test]
fn sweep_window_first_sweep_clamps_to_anchor() {
    // 首 sweep：锚点比滚动界新 → 窗口夹回锚点（仅向前不回溯）。
    let now_ms = ANCHOR_MS + 30 * 60 * 1000; // 启用后 30 分钟
    let p = UserProfile {
        enabled_at: Some("2026-10-01T00:00:00Z".into()),
        last_sweep_ms: None,
        habits: vec![],
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
        habits: vec![],
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
        habits: vec![],
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
        habits: vec![],
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
        habits: vec![],
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
        habits: vec![],
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
    let mut p = UserProfile {
        enabled_at: Some("2026-10-01T00:00:00Z".into()),
        last_sweep_ms: None,
        habits: vec![habit("用户偏好简洁回复", 0.8), habit("习惯乙", 0.7)],
    };
    save_profile(dir.path(), &p).unwrap();
    assert!(veto_habit(dir.path(), "").is_err());
    assert!(veto_habit(dir.path(), "   ").is_err());
    assert!(forget_habit(dir.path(), "").is_err());
    assert!(set_pinned(dir.path(), "", true).is_err());
    // 防线在写盘前：条目原样保留。
    p = load_profile(dir.path());
    assert_eq!(p.habits.len(), 2);
}

// ---- 四路评审修复回归锁 ----

#[test]
fn upsert_hit_clamps_out_of_range_confidence() {
    // B-F2：LLM 反思通道候选可能带 >1.0 的越界置信度，命中合并路径必须
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
        p.habits.len(),
        MAX_ENTRIES,
        "pinned 占活跃名额，总数仍按上限截断"
    );
}

#[test]
fn render_folds_newlines_in_content() {
    // B-F2：反思通道内容若夹带换行会伪造注入行结构——渲染层压成单行。
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
        last_sweep_ms: None,
        habits: vec![habit("Prefers Concise REPLY", 0.8)],
    };
    save_profile(dir.path(), &p).unwrap();
    assert!(
        veto_habit(dir.path(), "concise reply").unwrap(),
        "veto 小写命中大写原文"
    );
    let loaded = load_profile(dir.path());
    assert_eq!(loaded.habits[0].status, STATUS_VETOED);
    // forget 同口径。
    assert!(
        forget_habit(dir.path(), "CONCISE").unwrap(),
        "forget 大写命中小写子串"
    );
    assert!(load_profile(dir.path()).habits.is_empty());
}

#[test]
fn try_load_corrupt_profile_is_err_and_load_tolerates() {
    // B-F3：损坏画像 try_load = Err（写路径禁用容错形态）；load_profile
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
