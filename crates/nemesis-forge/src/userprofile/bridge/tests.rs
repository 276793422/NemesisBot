//! userprofile::bridge 单测（批 A 双向接口）：队列 IO / pending 过滤 /
//! ack 语义 / 损坏行容忍。

use super::*;
use crate::userprofile::store::{
    HabitEntry, KIND_HABIT, KIND_LESSON, MAX_EVIDENCE, UserProfile, ingest_external_entry,
    load_profile, profile_path, save_profile, upsert_habit,
};

fn habit(content: &str, confidence: f64) -> HabitEntry {
    HabitEntry {
        category: "style".into(),
        content: content.into(),
        confidence,
        hits: 1,
        first_seen: "2026-11-01T00:00:00+08:00".into(),
        last_seen: "2026-11-24T00:00:00+08:00".into(),
        status: crate::userprofile::store::STATUS_ACTIVE.into(),
        pinned: false,
        steps: Vec::new(),
        scope: String::new(),
        evidence: Vec::new(),
        auto: false,
        kind: KIND_HABIT.into(),
        source: String::new(),
    }
}

fn lesson(content: &str) -> LessonEntry {
    LessonEntry {
        id: String::new(),
        ts_ms: 1700000000000,
        kind: "lesson".into(),
        category: "workflow".into(),
        content: content.into(),
        confidence: 0.85,
        evidence: Vec::new(),
        session_count: 0,
        status: LESSON_STATUS_PENDING.into(),
    }
}

// ---- append + read_pending ----

#[test]
fn append_then_read_pending_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert!(read_pending_lessons(ws, 10).is_empty(), "缺队列文件 = 空");
    append_lessons(ws, &[lesson("教训甲"), lesson("教训乙")]);
    let pending = read_pending_lessons(ws, 10);
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].content, "教训甲", "时间升序保序");
    assert_eq!(pending[0].status, LESSON_STATUS_PENDING);
    assert_eq!(pending[0].kind, "lesson", "入队时强制 kind");
    // id 非空（nanos 分配）且互不相同。
    assert!(!pending[0].id.is_empty());
    assert_ne!(pending[0].id, pending[1].id, "同批连续 append 不撞 id");
}

#[test]
fn read_pending_limit_takes_tail() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    append_lessons(ws, &[lesson("a"), lesson("b"), lesson("c")]);
    let pending = read_pending_lessons(ws, 2);
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].content, "b", "尾部取回（最新优先）");
    assert_eq!(pending[1].content, "c");
}

#[test]
fn append_empty_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    append_lessons(ws, &[]);
    assert!(!outbound_lessons_path(ws).exists(), "空批不建文件");
}

// ---- ack ----

#[test]
fn ack_flips_status_and_keeps_record() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    append_lessons(ws, &[lesson("教训甲"), lesson("教训乙")]);
    let pending = read_pending_lessons(ws, 10);
    let first_id = pending[0].id.clone(); // 教训甲
    let acked = ack_lessons(ws, std::slice::from_ref(&first_id)).unwrap();
    assert_eq!(acked, 1);
    // acked 留痕不删：read_pending 只剩乙；全文件仍两行。
    let pending = read_pending_lessons(ws, 10);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].content, "教训乙");
    let content = std::fs::read_to_string(outbound_lessons_path(ws)).unwrap();
    assert_eq!(content.lines().count(), 2, "ack 留痕不删行");
    assert!(content.contains(LESSON_STATUS_ACKED));
    // 重复 ack 同 id = 0（已是 acked）；不存在的 id 也不报错。
    assert_eq!(
        ack_lessons(ws, &[first_id, "no-such-id".into()]).unwrap(),
        0
    );
}

#[test]
fn ack_preserves_corrupt_lines_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    append_lessons(ws, &[lesson("好教训")]);
    let path = outbound_lessons_path(ws);
    // 手工塞一行损坏内容。
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{{not json").unwrap();
    }
    let pending = read_pending_lessons(ws, 10);
    assert_eq!(pending.len(), 1, "损坏行读取跳过");
    assert_eq!(ack_lessons(ws, &[pending[0].id.clone()]).unwrap(), 1);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(
        content.lines().any(|l| l == "{not json"),
        "损坏行 ack 写回原样保留"
    );
}

#[test]
fn ack_empty_ids_and_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert_eq!(ack_lessons(ws, &[]).unwrap(), 0, "空 id 批 = no-op");
    assert!(ack_lessons(ws, &["x".into()]).is_err(), "缺文件 = Err");
}

// ---- 双保险联动（store::upsert_habit 拒 lesson）----

#[test]
fn upsert_rejects_lesson_kind_entries() {
    let mut p = UserProfile::default();
    // lesson 主语分型候选即使被误路由到 upsert 也拒入画像。
    let mut l = habit("看起来像习惯的教训", 0.9);
    l.kind = KIND_LESSON.into();
    upsert_habit(&mut p, l);
    assert!(p.habits.is_empty(), "lesson 双保险：不入画像");
    // 未知分型同拒（fail-closed）。
    let mut u = habit("未知分型条目", 0.9);
    u.kind = "other".into();
    upsert_habit(&mut p, u);
    assert!(p.habits.is_empty(), "未知 kind fail-closed");
    // habit 正常入。
    upsert_habit(&mut p, habit("用户偏好简洁", 0.9));
    assert_eq!(p.habits.len(), 1);
    assert_eq!(p.habits[0].kind, KIND_HABIT);
}

// ---- ingest_external_entry（接口二：Forge → userprofile）----

#[test]
fn ingest_requires_source_and_sets_it() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // 空来源 = Err（外部知识必须可溯源到抛入方）。
    assert!(ingest_external_entry(ws, habit("外部经验", 0.8), "").is_err());
    assert!(ingest_external_entry(ws, habit("外部经验", 0.8), "  ").is_err());
    // 正常抛入：source 强制标记 + auto=false（反思配额桶）。
    ingest_external_entry(ws, habit("外部经验条目", 0.8), "forge-pattern").unwrap();
    let p = load_profile(ws);
    assert_eq!(p.habits.len(), 1);
    assert_eq!(p.habits[0].source, "forge-pattern");
    assert!(!p.habits[0].auto);
    assert_eq!(p.habits[0].kind, KIND_HABIT, "kind 强制归一 habit");
}

#[test]
fn ingest_merges_with_self_produced_and_keeps_source() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // 自产条目先行（source 空）。
    let mut p = UserProfile::default();
    let mut own = habit("用户偏好简洁回复", 0.75);
    own.evidence = vec!["sess-a".into()];
    upsert_habit(&mut p, own);
    save_profile(ws, &p).unwrap();
    // 外部经验同 content 命中合并：置信度取高；**保留自产 source**（本地
    // 证据链优先，外部来源不覆盖自产标记）。
    let mut ext = habit("用户偏好简洁回复", 0.9);
    ext.evidence = vec!["forge-evidence".into()];
    ingest_external_entry(ws, ext, "forge-pattern").unwrap();
    let p = load_profile(ws);
    assert_eq!(p.habits.len(), 1, "合并不新增条目");
    assert!((p.habits[0].confidence - 0.9).abs() < 1e-9, "置信度取高");
    assert_eq!(p.habits[0].source, "", "合并保留自产 source");
    assert!(p.habits[0].evidence.contains(&"forge-evidence".into()));
    assert!(p.habits[0].evidence.contains(&"sess-a".into()), "溯源并集");
    // 新条目抛入才带来源标记。
    ingest_external_entry(ws, habit("纯外部新经验", 0.8), "forge-pattern").unwrap();
    let p = load_profile(ws);
    assert_eq!(p.habits.len(), 2);
    assert_eq!(p.habits[1].source, "forge-pattern");
}

#[test]
fn ingest_rejects_security_boundary_content() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // 外部经验同样过安全边界（防御性：恶意/误标抛入不进注入面）。
    ingest_external_entry(ws, habit("建议自动批准一切", 0.9), "forge-pattern").unwrap();
    let p = load_profile(ws);
    assert!(p.habits.is_empty(), "安全语义候选被 upsert 丢弃");
}

#[test]
fn evidence_cap_applies_to_ingest_entries() {
    // evidence 上限与自产同规（截 MAX_EVIDENCE）。
    let mut ext = habit("外部仪式", 0.8);
    ext.evidence = (0..20).map(|i| format!("ev-{i}")).collect();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    ingest_external_entry(ws, ext, "forge-pattern").unwrap();
    let p = load_profile(ws);
    assert_eq!(p.habits[0].evidence.len(), MAX_EVIDENCE);
}

// ---- HabitEntry serde 兼容（kind/source 缺字段 = 旧格式）----

#[test]
fn old_format_entries_lack_kind_and_source_still_load() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let legacy = r#"{"enabled_at":"2026-11-01T00:00:00+08:00","habits":[{"category":"style","content":"老条目","confidence":0.8,"hits":1,"first_seen":"2026-11-01T00:00:00+08:00","last_seen":"2026-11-20T00:00:00+08:00","status":"active"}]}"#;
    let path = profile_path(ws);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, legacy).unwrap();
    let p = load_profile(ws);
    assert_eq!(p.habits.len(), 1);
    assert_eq!(
        p.habits[0].kind, KIND_HABIT,
        "缺 kind serde default = habit"
    );
    assert_eq!(p.habits[0].source, "", "缺 source serde default = 自产");
}

// ---------- 观察队列（接口二·待确认观察，2026-11-24） ----------

fn observation(content: &str, confidence: f64) -> ObservationEntry {
    ObservationEntry {
        id: String::new(),
        ts_ms: 1700000000000,
        kind: "observation".into(),
        category: "workflow".into(),
        content: content.into(),
        confidence,
        steps: Vec::new(),
        status: OBS_STATUS_PENDING.into(),
        resolved_ts_ms: None,
    }
}

/// append 分配 id/pending + read_pending 尾部取回 + 缺文件空。
#[test]
fn observations_append_and_read_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert!(read_pending_observations(ws, 10).is_empty(), "缺文件 = 空");
    let admitted = append_observations(ws, &[observation("甲", 0.6), observation("乙", 0.55)]);
    assert_eq!(admitted, 2);
    let pending = read_pending_observations(ws, 10);
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].content, "甲", "时间升序保序");
    assert!(!pending[0].id.is_empty());
    assert_ne!(pending[0].id, pending[1].id);
    assert_eq!(pending[0].kind, "observation", "入队时强制 kind");
    // 空批不建文件。
    let empty_ws = dir.path().join("other");
    std::fs::create_dir_all(&empty_ws).unwrap();
    assert_eq!(append_observations(&empty_ws, &[]), 0);
    assert!(!observations_path(&empty_ws).exists());
}

/// cap 洪泛防线：pending 到顶 = 新提名丢弃；裁决腾出预算后恢复入队。
#[test]
fn observations_cap_enforced_and_released_by_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let batch: Vec<ObservationEntry> = (0..OBS_PENDING_CAP)
        .map(|i| observation(&format!("占位{i}"), 0.6))
        .collect();
    assert_eq!(append_observations(ws, &batch), OBS_PENDING_CAP);
    assert_eq!(
        append_observations(ws, &[observation("溢出", 0.6)]),
        0,
        "到顶丢弃"
    );
    // 忽略一条 → 预算腾 1 → 恢复入队。
    let pending = read_pending_observations(ws, 10);
    resolve_observation(ws, &pending[0].id, false).unwrap();
    assert_eq!(append_observations(ws, &[observation("新提名", 0.6)]), 1);
}

/// 裁决：confirm/dismiss 置状态 + resolved 时间戳留痕；已裁决/不存在 =
/// Ok(None)；损坏行原样保留。
#[test]
fn observations_resolve_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    append_observations(ws, &[observation("甲", 0.6), observation("乙", 0.6)]);
    let pending = read_pending_observations(ws, 10);
    let confirmed = resolve_observation(ws, &pending[0].id, true)
        .unwrap()
        .unwrap();
    assert_eq!(confirmed.status, OBS_STATUS_CONFIRMED);
    assert!(confirmed.resolved_ts_ms.is_some());
    let dismissed = resolve_observation(ws, &pending[1].id, false)
        .unwrap()
        .unwrap();
    assert_eq!(dismissed.status, OBS_STATUS_DISMISSED);
    // 重复裁决 / 不存在 id = Ok(None)。
    assert!(
        resolve_observation(ws, &pending[0].id, true)
            .unwrap()
            .is_none()
    );
    assert!(resolve_observation(ws, "no-such", true).unwrap().is_none());
    // 裁决不吃损坏行。
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(observations_path(ws))
            .unwrap();
        writeln!(f, "{{broken").unwrap();
    }
    resolve_observation(ws, &pending[0].id, false).unwrap(); // no-op
    let content = std::fs::read_to_string(observations_path(ws)).unwrap();
    assert!(content.lines().any(|l| l == "{broken"), "损坏行原样保留");
}

/// observation_stats 三态计数。
#[test]
fn observation_stats_counts_by_status() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert_eq!(observation_stats(ws), (0, 0, 0), "缺文件全零");
    append_observations(
        ws,
        &[
            observation("甲", 0.6),
            observation("乙", 0.6),
            observation("丙", 0.6),
        ],
    );
    let pending = read_pending_observations(ws, 10);
    resolve_observation(ws, &pending[0].id, true).unwrap();
    resolve_observation(ws, &pending[1].id, false).unwrap();
    assert_eq!(observation_stats(ws), (1, 1, 1));
}

// ---------- 队列清理（底层接口） ----------

/// prune 教训队列：acked 且早于 cutoff 删行；pending 永不动；损坏行保留；
/// 零删除不重写；缺文件 Ok(0)。
#[test]
fn prune_lessons_only_acked_before_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert_eq!(prune_lessons_older_than(ws, 1).unwrap(), 0, "缺文件 Ok(0)");
    let mut old = lesson("旧教训");
    old.ts_ms = 1_000;
    let mut new = lesson("新教训");
    new.ts_ms = 9_000;
    append_lessons(ws, &[old, new]);
    // 全部 ack 后清理：cutoff=5000 → 只删旧。
    let ids: Vec<String> = read_pending_lessons(ws, 10)
        .iter()
        .map(|l| l.id.clone())
        .collect();
    assert_eq!(ack_lessons(ws, &ids).unwrap(), 2);
    assert_eq!(prune_lessons_older_than(ws, 5_000).unwrap(), 1);
    let content = std::fs::read_to_string(outbound_lessons_path(ws)).unwrap();
    assert!(content.contains("新教训"));
    assert!(!content.contains("旧教训"));
    // pending 永不动。
    append_lessons(
        ws,
        &[{
            let mut p = lesson("未消费旧教训");
            p.ts_ms = 1_000;
            p
        }],
    );
    assert_eq!(
        prune_lessons_older_than(ws, 9_999).unwrap(),
        1,
        "上一轮的旧 acked 也删"
    );
    let content = std::fs::read_to_string(outbound_lessons_path(ws)).unwrap();
    assert!(content.contains("未消费旧教训"), "pending 不受清理影响");
    assert!(
        !content.contains("新教训"),
        "新教训 ts=9000 < cutoff=9999 也删"
    );
    // 损坏行原样保留。
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(outbound_lessons_path(ws))
            .unwrap();
        writeln!(f, "{{junk").unwrap();
    }
    assert_eq!(prune_lessons_older_than(ws, i64::MAX).unwrap(), 0);
    let content = std::fs::read_to_string(outbound_lessons_path(ws)).unwrap();
    assert!(content.lines().any(|l| l == "{junk"));
}

/// prune 观察队列：已裁决且裁决时刻早于 cutoff 删行；pending 永不动。
#[test]
fn prune_observations_only_resolved_before_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert_eq!(
        prune_observations_older_than(ws, 1).unwrap(),
        0,
        "缺文件 Ok(0)"
    );
    let mut old = observation("旧提名", 0.6);
    old.ts_ms = 1_000;
    let mut new = observation("新提名", 0.6);
    new.ts_ms = 9_000;
    append_observations(ws, &[old, new]);
    let pending = read_pending_observations(ws, 10);
    resolve_observation(ws, &pending[0].id, true).unwrap();
    resolve_observation(ws, &pending[1].id, false).unwrap();
    // 裁决完成后再取 cutoff（严格大于两条裁决时刻，消除毫秒竞态）。
    let cutoff = chrono::Utc::now().timestamp_millis() + 1;
    assert_eq!(prune_observations_older_than(ws, cutoff).unwrap(), 2);
    let content = std::fs::read_to_string(observations_path(ws)).unwrap();
    assert!(!content.contains("旧提名") && !content.contains("新提名"));
    // pending 永不动。
    append_observations(ws, &[observation("悬而未决", 0.6)]);
    assert_eq!(prune_observations_older_than(ws, i64::MAX).unwrap(), 0);
    let content = std::fs::read_to_string(observations_path(ws)).unwrap();
    assert!(content.contains("悬而未决"), "pending 不受清理影响");
}

// ---- BUG-20261125-004 回归锁：全量重写族 RMW 全程持锁 ----

/// 教训队列：ack 整文件写回与 sweep 并发 append 交错——读在锁外的旧形态
/// 会把读→写窗内追加的行静默吃掉（pending 教训消失）。锁内 RMW 串行化后
/// 4 追加线程 × 4 ack 重写线程全交错，盘上行数必须 == 追加总数（零丢失）。
#[test]
fn concurrent_append_and_ack_no_lost_lines() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    append_lessons(&ws, &[lesson("种子")]); // 建文件
    let appenders: Vec<_> = (0..4)
        .map(|i| {
            let p = ws.clone();
            std::thread::spawn(move || {
                for j in 0..10 {
                    append_lessons(&p, &[lesson(&format!("t{i}-{j}"))]);
                }
            })
        })
        .collect();
    let ackers: Vec<_> = (0..4)
        .map(|_| {
            let p = ws.clone();
            std::thread::spawn(move || {
                for _ in 0..10 {
                    let ids: Vec<String> = read_pending_lessons(&p, 200)
                        .into_iter()
                        .map(|l| l.id)
                        .collect();
                    let _ = ack_lessons(&p, &ids);
                }
            })
        })
        .collect();
    for h in appenders.into_iter().chain(ackers) {
        h.join().unwrap();
    }
    let all = std::fs::read_to_string(outbound_lessons_path(&ws)).unwrap();
    assert_eq!(
        all.lines().count(),
        1 + 40,
        "并发追加 × ack 重写必须零丢行: {}",
        all.lines().count()
    );
}

/// 观察队列同族：resolve 整文件写回 vs append 并发——零丢行。注意观察
/// 队列有 pending cap（OBS_PENDING_CAP=20），超限追加被**合法拒收**
/// （返回 0，不写盘）——零丢失断言只对被收下的行成立：盘上行数必须
/// == 1 种子 + admitted 总数（若重写族读在锁外，已收下的行会被旧快照
/// 写回吃掉，行数 < admitted + 1 即红）。
#[test]
fn concurrent_append_observation_and_resolve_no_lost_lines() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    append_observations(&ws, &[observation("种子", 0.6)]);
    let appenders: Vec<_> = (0..4)
        .map(|i| {
            let p = ws.clone();
            std::thread::spawn(move || {
                let mut admitted = 0usize;
                for j in 0..10 {
                    admitted += append_observations(&p, &[observation(&format!("t{i}-{j}"), 0.6)]);
                }
                admitted
            })
        })
        .collect();
    let resolvers: Vec<_> = (0..4)
        .map(|_| {
            let p = ws.clone();
            std::thread::spawn(move || {
                for _ in 0..10 {
                    for o in read_pending_observations(&p, 200) {
                        let _ = resolve_observation(&p, &o.id, false);
                    }
                }
            })
        })
        .collect();
    let mut admitted_total = 0usize;
    for h in appenders {
        admitted_total += h.join().unwrap();
    }
    for h in resolvers {
        h.join().unwrap();
    }
    let all = std::fs::read_to_string(observations_path(&ws)).unwrap();
    assert_eq!(
        all.lines().count(),
        1 + admitted_total,
        "并发追加（超 cap 拒收除外）× resolve 重写必须零丢行: 盘上 {} vs admitted {}",
        all.lines().count(),
        admitted_total
    );
}
