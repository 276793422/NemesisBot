//! [`crate::session_group_sweeper`] 单测：灰组 / inactive 组 / 活跃组 /
//! 部分活跃组 / 对话组五形态判定 + 报告内容 + 零删除断言。
//!
//! 手法（与 logs_sweeper tests 同形）：夹具 mtime = 真实当下、不可回拨
//! （std 无稳定 API，不引 filetime）——把 sweep 的 `now` 推进到未来
//! 40 天，夹具相对它就是「40 天前」，超 30 天闸。混合 mtime 的组级形态
//! （部分活跃不打扰 / judgeable 阻断）在 fs 层造不出（mtime 不可回拨），
//! 由拆出的判定纯函数 [`super::classify_groups`] 直测（内存构造组）。

use super::*;
use std::path::Path;

/// 推进到未来 40 天的时钟：全部夹具按 mtime 相对它超 30 天闸。
fn future_now() -> SystemTime {
    SystemTime::now() + Duration::from_secs(40 * DAY_SECS)
}

/// temp workspace 夹具：注册表（P1 活跃目录在 / P2 目录消失）+ 四个带
/// 归属会话 + 一个对话组会话。返回 workspace 路径。
///
/// - s_p1（project_id=P1，活跃组）→ 不入报告
/// - s_p2a / s_p2b（project_id=P2，目录消失 = inactive，全部超龄）→ 入报告
/// - s_gone（project_id="gone"，注册表无条目 = 灰组）→ 入报告
/// - s_none（无归属 = 对话组）→ 不入报告
fn fixture_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    // 注册表：P1（目录真实存在）/ P2（路径指到不存在的目录）。
    let cfg_dir = ws.join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let p1_dir = ws.join("proj-one");
    std::fs::create_dir_all(&p1_dir).unwrap(); // P1 目录在 = 活跃
    let projects = serde_json::json!({
        "projects": [
            { "id": "P1", "name": "one", "path": p1_dir.to_string_lossy(),
              "created_at": "2026-01-01T00:00:00Z" },
            { "id": "P2", "name": "two",
              "path": ws.join("gone-dir-two").to_string_lossy(),
              "created_at": "2026-01-01T00:00:00Z" }
        ]
    });
    std::fs::write(
        cfg_dir.join("projects.json"),
        serde_json::to_string(&projects).unwrap(),
    )
    .unwrap();

    // 会话 sidecar + 正文。
    let logs = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&logs).unwrap();
    let mk = |name: &str, project_id: Option<&str>| {
        let mut meta = serde_json::json!({ "title": name });
        if let Some(pid) = project_id {
            meta["project_id"] = serde_json::json!(pid);
            meta["project_path"] = serde_json::json!("/x");
        }
        std::fs::write(logs.join(format!("{name}.meta.json")), meta.to_string()).unwrap();
        std::fs::write(logs.join(format!("{name}.jsonl")), "t\n").unwrap();
    };
    mk("s_p1", Some("P1"));
    mk("s_p2a", Some("P2"));
    mk("s_p2b", Some("P2"));
    mk("s_gone", Some("gone"));
    mk("s_none", None);
    dir
}

fn ws_of(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().to_path_buf()
}

/// classify 级内存构造器：注册表同 fixture 形态（P1 目录在 / P2 目录
/// 消失 / gone 无条目），组内会话按传入 mtime 秒造。P1 的「目录在」用
/// 独立 tempdir 承载（不写 CWD，并行测试无碰撞）。
fn classify_fixture(
    groups: Vec<(&str, Vec<(&str, u64, bool)>)>,
    now: SystemTime,
) -> (Vec<GroupFinding>, Vec<GroupFinding>) {
    let p1_dir = tempfile::tempdir().unwrap();
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        "P1".to_string(),
        ("one".to_string(), p1_dir.path().to_path_buf()),
    );
    entries.insert(
        "P2".to_string(),
        ("two".to_string(), p1_dir.path().join("no-such-sub")),
    );
    let mut map = std::collections::BTreeMap::new();
    for (pid, sessions) in groups {
        map.insert(
            pid.to_string(),
            sessions
                .into_iter()
                .map(|(k, m, j)| SessionAge {
                    key: k.to_string(),
                    mtime_secs: m,
                    judgeable: j,
                })
                .collect(),
        );
    }
    classify_groups(map, &entries, now)
}

#[test]
fn orphan_and_inactive_reported_active_and_dialogue_skipped() {
    let dir = fixture_workspace();
    let report = sweep_session_groups(&ws_of(&dir), future_now());

    assert_eq!(report.scanned_grouped_sessions, 4, "带归属 meta 总数");

    // 灰组：s_gone（注册表无条目）。
    assert_eq!(report.orphan_groups.len(), 1, "orphan: {report:?}");
    let g = &report.orphan_groups[0];
    assert_eq!(g.kind, "orphan");
    assert_eq!(g.project_id, "gone");
    assert_eq!(g.sessions, vec!["s_gone"]);
    assert_eq!(g.session_count, 1);
    assert!(g.project_name.is_none() && g.project_path.is_none());
    assert!(g.latest_mtime_secs > 0);

    // inactive 组：s_p2a + s_p2b（注册表有条目、目录消失）。
    assert_eq!(report.inactive_groups.len(), 1, "inactive: {report:?}");
    let g = &report.inactive_groups[0];
    assert_eq!(g.kind, "inactive");
    assert_eq!(g.project_id, "P2");
    assert_eq!(g.project_name.as_deref(), Some("two"));
    assert!(g.project_path.is_some());
    assert_eq!(g.session_count, 2);
    assert_eq!(g.sessions, vec!["s_p2a", "s_p2b"]);

    // 活跃组（P1 目录在）与对话组（无归属）都不在报告。
    let s = serde_json::to_string(&report).unwrap();
    assert!(!s.contains("s_p1"), "活跃组不入报告");
    assert!(!s.contains("s_none"), "对话组不入报告");
}

#[test]
fn classify_partially_active_group_not_reported() {
    // 组内一老一「新」（mtime 相对 now 仅 1 天）：非全部超龄 → 不入报告。
    let now = SystemTime::now();
    let old = unix_secs(now) - 40 * DAY_SECS;
    let fresh = unix_secs(now) - DAY_SECS;
    let (orphans, inactives) = classify_fixture(
        vec![("P2", vec![("s_old", old, true), ("s_fresh", fresh, true)])],
        now,
    );
    assert!(orphans.is_empty() && inactives.is_empty(), "部分活跃不打扰");

    // 同组全部超龄 → 照常入报告（同一夹具换 mtime，钉住判定对称性）。
    let (orphans, inactives) = classify_fixture(
        vec![("P2", vec![("s_a", old, true), ("s_b", old, true)])],
        now,
    );
    assert!(orphans.is_empty());
    assert_eq!(inactives.len(), 1);
    assert_eq!(inactives[0].sessions, vec!["s_a", "s_b"]);
}

#[test]
fn classify_unjudgeable_session_blocks_group() {
    // judgeable=false（mtime 不可读）按不超龄处理：哪怕其余会话全超龄，
    // 组也不入报告（判不了就不提示，保守不误报）。
    let now = SystemTime::now();
    let old = unix_secs(now) - 40 * DAY_SECS;
    let (orphans, inactives) = classify_fixture(
        vec![("gone", vec![("s_old", old, true), ("s_broken", old, false)])],
        now,
    );
    assert!(orphans.is_empty() && inactives.is_empty());
}

#[test]
fn classify_young_group_not_reported() {
    // 全组 1 天龄：不超 30 天闸 → 不打扰（新移除的项目不被立即提示）。
    let now = SystemTime::now();
    let young = unix_secs(now) - DAY_SECS;
    let (orphans, inactives) = classify_fixture(
        vec![
            ("gone", vec![("s1", young, true)]),
            ("P2", vec![("s2", young, true)]),
        ],
        now,
    );
    assert!(orphans.is_empty() && inactives.is_empty());
}

#[test]
fn now_gap_clock_skew_is_not_overaged() {
    // 时钟回拨形态（mtime 在未来）：负差落 0 = 不超龄（保守方向钉死）。
    let now = SystemTime::now();
    let future_mtime = unix_secs(now) + 3600;
    assert_eq!(now_secs_gap(now, future_mtime), 0);
    let past = now - Duration::from_secs(31 * DAY_SECS);
    assert_eq!(
        now_secs_gap(now, unix_secs(past)),
        31 * DAY_SECS,
        "正常超龄形态照常计距"
    );
}

#[test]
fn report_written_and_zero_deletion() {
    let dir = fixture_workspace();
    let ws = ws_of(&dir);
    let report = sweep_session_groups(&ws, future_now());
    assert!(report.has_findings());

    // 报告滚动覆盖写：logs/maintenance/session-groups.json。
    write_report(&ws, &report);
    let path = ws
        .join("logs")
        .join("maintenance")
        .join("session-groups.json");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk["orphan_groups"][0]["project_id"], "gone");
    assert_eq!(on_disk["inactive_groups"][0]["project_id"], "P2");
    assert_eq!(on_disk["scanned_grouped_sessions"], 4);

    // 零删除：全部夹具文件原样在盘（纯扫描 + 报告，钦定不删任何字节）。
    let logs = ws.join("logs").join("session_logs");
    for n in ["s_p1", "s_p2a", "s_p2b", "s_gone", "s_none"] {
        assert!(logs.join(format!("{n}.meta.json")).exists(), "{n} meta 在");
        assert!(logs.join(format!("{n}.jsonl")).exists(), "{n} jsonl 在");
    }
    assert!(ws.join("config").join("projects.json").exists());
}

#[test]
fn empty_workspace_is_empty_report_no_panic() {
    let dir = tempfile::tempdir().unwrap();
    let report = sweep_session_groups(dir.path(), future_now());
    assert_eq!(report.scanned_grouped_sessions, 0);
    assert!(!report.has_findings());
    // 空 workspace 写报告路径也能建目录落盘（滚动覆盖语义：无残留也写，
    // 「上次有、这次没有」可见）。
    write_report(dir.path(), &report);
    assert!(
        dir.path()
            .join("logs")
            .join("maintenance")
            .join("session-groups.json")
            .exists()
    );
}

#[test]
fn corrupted_meta_and_empty_owner_skipped() {
    let dir = fixture_workspace();
    let logs = dir.path().join("logs").join("session_logs");
    std::fs::write(logs.join("s_bad.meta.json"), "not-json{{").unwrap();
    std::fs::write(logs.join("s_bad.jsonl"), "t\n").unwrap();
    // project_id 空串 = 无归属（对话组口径）。
    std::fs::write(
        logs.join("s_empty.meta.json"),
        r#"{"project_id":"", "project_path":"/x"}"#,
    )
    .unwrap();
    std::fs::write(logs.join("s_empty.jsonl"), "t\n").unwrap();

    let report = sweep_session_groups(&ws_of(&dir), future_now());
    assert_eq!(report.scanned_grouped_sessions, 4, "损坏/空归属不入计数");
    let s = serde_json::to_string(&report).unwrap();
    assert!(!s.contains("s_bad") && !s.contains("s_empty"));
}

/// Path 引用形态点检（trait 导入消费）。
#[test]
fn path_import_smoke() {
    let p: &Path = Path::new("x");
    assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("x"));
}
