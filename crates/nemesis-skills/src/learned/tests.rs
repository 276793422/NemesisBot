//! N3 learned manifest 单元测试。
//!
//! 覆盖：load 缺失/损坏(.bak 备份)/正常、save 往返 + BTreeMap 顺序稳定、
//! upsert 新建/覆盖保留 created_at、remove 存在/不存在、便捷函数
//! record_learned / remove_learned、extract_description（带引号 / 无
//! frontmatter / description 在 frontmatter 外不认 / 多余键不干扰）。

use super::*;

fn temp_ws() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// ---------------------------------------------------------------------------
// load
// ---------------------------------------------------------------------------

#[test]
fn load_missing_returns_empty() {
    let ws = temp_ws();
    let m = LearnedManifest::load(ws.path());
    assert_eq!(m.version, 1);
    assert!(m.entries.is_empty());
}

#[test]
fn load_corrupt_backs_up_and_continues() {
    let ws = temp_ws();
    let path = learned_manifest_path(ws.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not json").unwrap();
    let m = LearnedManifest::load(ws.path());
    assert!(m.entries.is_empty());
    assert!(path.with_extension("json.bak").exists(), "必须留 .bak 现场");
}

// ---------------------------------------------------------------------------
// save 往返 + 顺序稳定
// ---------------------------------------------------------------------------

#[test]
fn save_roundtrip_and_stable_order() {
    let ws = temp_ws();
    let mut m = LearnedManifest::new();
    m.upsert("zeta", "z desc", 1000);
    m.upsert("alpha", "a desc", 2000);
    m.upsert("mid", "m desc", 3000);
    m.save(ws.path()).unwrap();

    let reloaded = LearnedManifest::load(ws.path());
    let names: Vec<&str> = reloaded.entries.keys().map(|s| s.as_str()).collect();
    assert_eq!(names, vec!["alpha", "mid", "zeta"], "BTreeMap 键序");
    assert_eq!(reloaded.entries["zeta"].created_at_ms, 1000);
    assert_eq!(reloaded.entries["alpha"].description, "a desc");
}

#[test]
fn save_creates_missing_skills_dir() {
    // skills/ 目录不存在（agent 直接 record 的边界）→ save 建目录。
    let ws = temp_ws();
    let mut m = LearnedManifest::new();
    m.upsert("solo", "d", 42);
    m.save(ws.path()).unwrap();
    assert!(learned_manifest_path(ws.path()).exists());
    assert!(
        !learned_manifest_path(ws.path())
            .with_extension("json.tmp")
            .exists(),
        "tmp 必须已被 rename 消费"
    );
}

// ---------------------------------------------------------------------------
// upsert / remove / get
// ---------------------------------------------------------------------------

#[test]
fn upsert_overwrite_keeps_created_at() {
    let mut m = LearnedManifest::new();
    m.upsert("keep-ts", "old desc", 111);
    m.upsert("keep-ts", "new desc", 999);
    let e = m.get("keep-ts").unwrap();
    assert_eq!(e.description, "new desc");
    assert_eq!(e.created_at_ms, 111, "覆盖不得刷新创建时间");
}

#[test]
fn remove_returns_membership() {
    let mut m = LearnedManifest::new();
    m.upsert("x", "d", 1);
    assert!(m.remove("x"));
    assert!(!m.remove("x"), "二次移除 = 不存在");
    assert!(m.get("x").is_none());
}

// ---------------------------------------------------------------------------
// 便捷函数
// ---------------------------------------------------------------------------

#[test]
fn record_then_remove_learned_roundtrip() {
    let ws = temp_ws();
    record_learned(ws.path(), "flow", "one-shot flow").unwrap();
    let m = LearnedManifest::load(ws.path());
    assert_eq!(m.entries["flow"].description, "one-shot flow");

    let existed = remove_learned(ws.path(), "flow").unwrap();
    assert!(existed);
    assert!(LearnedManifest::load(ws.path()).entries.is_empty());

    let existed2 = remove_learned(ws.path(), "flow").unwrap();
    assert!(!existed2, "不存在 = false 且不写盘");
}

// ---------------------------------------------------------------------------
// 写并发串行化（2026-10-13 同类横扫补钉）：record/remove 都是 load→改→save
// 的 RMW，agent 工具路径（跨会话并发 turn）与 WSAPI 管理面可并发到达。
// 无锁时后写者以自己的快照整体覆盖先写者的更新（save 原子救不了丢更新）。
// MANIFEST_WRITE_LOCK 串行化后，任意交错下两笔更新都必须同时落盘。
// ---------------------------------------------------------------------------

#[test]
fn concurrent_record_and_remove_no_lost_update() {
    let ws = temp_ws();
    // 预置一条 remove 的靶子。
    record_learned(ws.path(), "seed", "to be removed").unwrap();

    std::thread::scope(|s| {
        // 并发 record 一批新条目。
        for i in 0..6 {
            let ws = ws.path();
            s.spawn(move || {
                record_learned(ws, &format!("skill-{i}"), &format!("desc {i}"))
                    .unwrap_or_else(|e| panic!("并发 record {i} 失败: {e}"));
            });
        }
        // 同批并发 remove 靶子。
        let ws = ws.path();
        s.spawn(move || {
            remove_learned(ws, "seed").unwrap();
        });
    });

    let m = LearnedManifest::load(ws.path());
    assert!(!m.entries.contains_key("seed"), "seed 必须已被移除");
    for i in 0..6 {
        assert!(
            m.entries.contains_key(&format!("skill-{i}")),
            "skill-{i} 不得被并发 remove 的快照冲掉（丢更新即缺条目）"
        );
    }
}

// ---------------------------------------------------------------------------
// extract_description
// ---------------------------------------------------------------------------

#[test]
fn extract_description_basic_and_quoted() {
    let md = "---\nname: demo\ndescription: A demo skill\n---\n# Body\n";
    assert_eq!(extract_description(md), "A demo skill");

    let md_q = "---\ndescription: \"quoted desc\"\n---\nbody";
    assert_eq!(extract_description(md_q), "quoted desc");

    let md_sq = "---\ndescription: 'single'\n---\nbody";
    assert_eq!(extract_description(md_sq), "single");
}

#[test]
fn extract_description_missing_or_outside_frontmatter() {
    assert_eq!(extract_description("# no frontmatter"), "");
    assert_eq!(extract_description("---\nname: x\n---\n"), "");
    // frontmatter 外的 description: 行不认（正文可能是代码示例）。
    let md = "---\nname: x\n---\ndescription: not-meta\n";
    assert_eq!(extract_description(md), "");
    // description 前有其他键也不干扰。
    let md2 = "---\ntitle: t\ndescription: real one\n---\n";
    assert_eq!(extract_description(md2), "real one");
}
