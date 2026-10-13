//! L6++（2026-09-08）：项目注册表契约测试——缺席/损坏/名称与路径校验/
//! 上限/重叠矩阵/remove/rename。纯文件操作：每测独立 tempdir，无全局态，
//! 无跨测竞争。损坏文件 loud 拒绝（写路径）+ lenient 空表（读路径）的
//! 双姿态是本模块的核心防呆，单独钉死。

use super::*;

#[test]
fn test_missing_registry_is_ok_none_and_empty_list() {
    let ws = tempfile::tempdir().unwrap();
    let reg = registry_path(ws.path());
    assert_eq!(
        reg,
        ws.path().join("config").join("projects.json"),
        "注册表固定在 <workspace>/config/projects.json"
    );
    assert!(try_load_projects(&reg).unwrap().is_none(), "缺席 ≠ 损坏");
    assert!(list_projects(&reg).is_empty());
}

#[test]
fn test_create_roundtrip_and_duplicate_path_rejected() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let proj = parent.path().join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    let reg = registry_path(ws.path());

    let e = create_project(&reg, ws.path(), "演示项目", proj.to_str().unwrap(), 4).unwrap();
    assert!(
        e.id.starts_with("p-") && e.id.len() == 10,
        "id 形态 p-{{8hex}}: {}",
        e.id
    );
    assert!(e.id[2..].chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(e.name, "演示项目");
    assert_eq!(
        e.path,
        canonicalize_for_compare(&proj),
        "烧入 canonical（剥 verbatim）路径"
    );
    assert!(!e.created_at.is_empty());

    // 磁盘 roundtrip。
    let file = try_load_projects(&reg).unwrap().unwrap();
    assert_eq!(file.projects, vec![e.clone()]);

    // 同一路径重复注册 = 重叠，拒绝。
    let err = create_project(&reg, ws.path(), "另一个名", proj.to_str().unwrap(), 4)
        .unwrap_err()
        .to_string();
    assert!(err.contains("重叠"), "unexpected: {err}");

    assert_eq!(find_project(&reg, &e.id).unwrap().id, e.id);
    assert_eq!(list_projects(&reg).len(), 1);
}

#[test]
fn test_corrupt_registry_loud_rejects_writes_lenient_reads() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let proj = parent.path().join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    let reg = registry_path(ws.path());

    // 先落一条合法数据，再人为损坏。
    create_project(&reg, ws.path(), "demo", proj.to_str().unwrap(), 4).unwrap();
    std::fs::write(&reg, "{not json").unwrap();

    // 读：try_load = Err（区别于缺席的 Ok(None)）；lenient = 空表不炸。
    assert!(try_load_projects(&reg).is_err());
    assert!(load_projects_lenient(&reg).projects.is_empty());

    // 写：loud 拒绝，且不覆盖损坏文件（防静默清空用户数据）。
    let err = create_project(&reg, ws.path(), "x", proj.to_str().unwrap(), 4)
        .unwrap_err()
        .to_string();
    assert!(err.contains("损坏"), "unexpected: {err}");
    assert!(remove_project(&reg, "p-00000000").is_err());
    assert!(rename_project(&reg, "p-00000000", "y").is_err());
    assert_eq!(
        std::fs::read_to_string(&reg).unwrap(),
        "{not json",
        "损坏文件必须原样保留"
    );
}

#[test]
fn test_name_validation_shared_by_create_and_rename() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let proj = parent.path().join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    let reg = registry_path(ws.path());

    for bad in ["", "   "] {
        let err = create_project(&reg, ws.path(), bad, proj.to_str().unwrap(), 4)
            .unwrap_err()
            .to_string();
        assert!(err.contains("名称不能为空"), "unexpected: {err}");
    }
    let long = "名".repeat(101);
    let err = create_project(&reg, ws.path(), &long, proj.to_str().unwrap(), 4)
        .unwrap_err()
        .to_string();
    assert!(err.contains("过长"), "unexpected: {err}");

    // rename 共用同一校验真相源；trim 生效；path/id 不动。
    let e = create_project(&reg, ws.path(), "demo", proj.to_str().unwrap(), 4).unwrap();
    let err = rename_project(&reg, &e.id, "  ").unwrap_err().to_string();
    assert!(err.contains("名称不能为空"));
    let renamed = rename_project(&reg, &e.id, "  新名字  ").unwrap();
    assert_eq!(renamed.name, "新名字");
    assert_eq!(
        renamed.path, e.path,
        "rename 不改 path（改路径=移除后重建）"
    );
    assert_eq!(renamed.id, e.id);
}

#[test]
fn test_cap_rejects_with_actionable_message() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let p1 = parent.path().join("p1");
    let p2 = parent.path().join("p2");
    std::fs::create_dir_all(&p1).unwrap();
    std::fs::create_dir_all(&p2).unwrap();
    let reg = registry_path(ws.path());

    create_project(&reg, ws.path(), "one", p1.to_str().unwrap(), 1).unwrap();
    let err = create_project(&reg, ws.path(), "two", p2.to_str().unwrap(), 1)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("上限") && err.contains('1'),
        "unexpected: {err}"
    );
    assert_eq!(list_projects(&reg).len(), 1, "被拒条目不落盘");
}

#[test]
fn test_path_validation_and_overlap_matrix() {
    let root = tempfile::tempdir().unwrap();
    let ws = root.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let reg = registry_path(&ws);
    let max = 8;
    let mk = |name: &str| {
        let p = root.path().join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    };

    // 相对路径 / 空路径 / 不存在的目录。
    let nope = root.path().join("nope");
    for (bad, needle) in [
        ("relative/path", "绝对路径"),
        ("", "不能为空"),
        (nope.to_str().unwrap(), "不存在"),
    ] {
        let err = create_project(&reg, &ws, "x", bad, max)
            .unwrap_err()
            .to_string();
        assert!(err.contains(needle), "unexpected: {err}");
    }

    // 与主 workspace 重叠：相等 / ws 的子目录 / ws 的祖先（双向）。
    let sub_of_ws = mk("ws/sub");
    for cand in [ws.to_str().unwrap(), sub_of_ws.to_str().unwrap()] {
        let err = create_project(&reg, &ws, "x", cand, max)
            .unwrap_err()
            .to_string();
        assert!(err.contains("主工作区"), "unexpected: {err}");
    }
    let err = create_project(&reg, &ws, "x", root.path().to_str().unwrap(), max)
        .unwrap_err()
        .to_string();
    assert!(err.contains("重叠"), "ws 的祖先同样拒绝: {err}");

    // 合法：ws 的兄弟目录。
    let sibling = mk("sibling_proj");
    let ok = create_project(&reg, &ws, "sibling", sibling.to_str().unwrap(), max).unwrap();
    assert_eq!(ok.path, canonicalize_for_compare(&sibling));

    // 项目间重叠：已有项目的子目录拒绝；兄弟合法。
    let child = mk("sibling_proj/child");
    let err = create_project(&reg, &ws, "y", child.to_str().unwrap(), max)
        .unwrap_err()
        .to_string();
    assert!(err.contains("重叠"), "unexpected: {err}");
    mk("sibling_proj2");
    create_project(
        &reg,
        &ws,
        "z",
        root.path().join("sibling_proj2").to_str().unwrap(),
        max,
    )
    .unwrap();
    assert_eq!(list_projects(&reg).len(), 2);
}

#[test]
fn test_remove_unbinds_only_and_missing_errors() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let p1 = parent.path().join("p1");
    let p2 = parent.path().join("p2");
    std::fs::create_dir_all(&p1).unwrap();
    std::fs::create_dir_all(&p2).unwrap();
    let reg = registry_path(ws.path());

    let e1 = create_project(&reg, ws.path(), "one", p1.to_str().unwrap(), 4).unwrap();
    let e2 = create_project(&reg, ws.path(), "two", p2.to_str().unwrap(), 4).unwrap();

    // 移除 = 只解除分组：条目出表，目录与其内容原样（注册不拥有）。
    let removed = remove_project(&reg, &e1.id).unwrap();
    assert_eq!(removed, e1);
    assert_eq!(list_projects(&reg), vec![e2.clone()]);
    assert!(p1.is_dir());

    // 不存在：remove/rename 同报错语义（对齐 eval_rules 家族）。
    for err in [
        remove_project(&reg, "p-ffffffff").unwrap_err().to_string(),
        rename_project(&reg, "p-ffffffff", "n")
            .unwrap_err()
            .to_string(),
    ] {
        assert!(err.contains("项目不存在"), "unexpected: {err}");
    }

    // 被移除的路径可重新注册（围栏释放），但拿新 id（不复活旧条目）。
    let again = create_project(&reg, ws.path(), "one-again", p1.to_str().unwrap(), 4).unwrap();
    assert_ne!(again.id, e1.id);
}

// ---------------------------------------------------------------------------
// N7（2026-10-13）：指令信任位——serde 缺省 true（老条目零迁移）+ set_project_trust RMW。
// ---------------------------------------------------------------------------

#[test]
fn test_entry_instructions_trusted_defaults_true_and_roundtrips() {
    // 老形态 JSON（无 instructions_trusted 键）→ 反序列化为 true（注册即
    // 信任行为，零迁移）。
    let legacy: ProjectEntry = serde_json::from_str(
        r#"{"id":"p-00000001","name":"老条目","path":"C:/p","created_at":"2026-09-08T00:00:00Z"}"#,
    )
    .unwrap();
    assert!(legacy.instructions_trusted);

    // 显式 false round-trip 保留。
    let e = ProjectEntry {
        instructions_trusted: false,
        ..legacy.clone()
    };
    let json = serde_json::to_string(&e).unwrap();
    let back: ProjectEntry = serde_json::from_str(&json).unwrap();
    assert!(!back.instructions_trusted);
    assert_eq!(back, e);
}

#[test]
fn test_set_project_trust_rmw_and_not_found() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let proj = parent.path().join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    let reg = registry_path(ws.path());

    let e = create_project(&reg, ws.path(), "演示项目", proj.to_str().unwrap(), 4).unwrap();
    assert!(e.instructions_trusted, "新条目缺省受信");

    // 翻转为 false：返回值 + 盘上状态一致。
    let updated = set_project_trust(&reg, &e.id, false).unwrap();
    assert!(!updated.instructions_trusted);
    assert!(
        !find_project(&reg, &e.id).unwrap().instructions_trusted,
        "盘上状态必须与返回值一致（翻转为 false）"
    );

    // 再翻回 true。
    let back = set_project_trust(&reg, &e.id, true).unwrap();
    assert!(back.instructions_trusted);

    // 不存在：与 remove/rename 同报错语义。
    let err = set_project_trust(&reg, "p-ffffffff", false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("项目不存在"), "unexpected: {err}");
}

// ---------------------------------------------------------------------------
// 写并发串行化（2026-10-13 同类横扫补钉）：create/remove/rename/set_trust 都
// 是 load→改→save 的 RMW，WSAPI 各臂可并发到达。无锁时后写者以自己的快照
// 整体覆盖先写者的更新（save 原子救不了丢更新）——trust 翻转被 create 的
// 旧快照冲回 true = 指令信任闸静默失效。REGISTRY_WRITE_LOCK 串行化后，
// 任意交错下两笔更新都必须同时落盘。
// ---------------------------------------------------------------------------

#[test]
fn test_concurrent_create_and_trust_flip_no_lost_update() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let reg = registry_path(ws.path());

    // 先注册 A（trust 翻转的靶子），再并发：N 路 create 新项目 + M 路把 A
    // 翻成 false。串行化保证最终态=某种串行序：全部新项目在场且 A=false。
    let proj_a = parent.path().join("proj-a");
    std::fs::create_dir_all(&proj_a).unwrap();
    let ea = create_project(&reg, ws.path(), "A", proj_a.to_str().unwrap(), 8).unwrap();

    const N: usize = 6;
    std::thread::scope(|s| {
        for i in 0..N {
            let reg = &reg;
            let ws = ws.path();
            let parent = parent.path();
            s.spawn(move || {
                let p = parent.join(format!("proj-new-{i}"));
                std::fs::create_dir_all(&p).unwrap();
                create_project(reg, ws, &format!("新项目{i}"), p.to_str().unwrap(), 8)
                    .unwrap_or_else(|e| panic!("并发 create {i} 失败: {e}"));
            });
        }
        for _ in 0..N {
            let reg = &reg;
            let id = ea.id.clone();
            s.spawn(move || {
                set_project_trust(reg, &id, false)
                    .unwrap_or_else(|e| panic!("并发 trust 翻转失败: {e}"));
            });
        }
    });

    let entries = list_projects(&reg);
    assert_eq!(
        entries.len(),
        1 + N,
        "并发 create 不得互相冲掉（丢更新即少条目）: {}",
        entries.len()
    );
    assert!(
        !find_project(&reg, &ea.id).unwrap().instructions_trusted,
        "trust 翻转不得被并发 create 的快照覆盖回 true"
    );
    for i in 0..N {
        let p = parent.path().join(format!("proj-new-{i}"));
        assert!(
            entries
                .iter()
                .any(|e| e.path == canonicalize_for_compare(&p)),
            "新项目 {i} 必须在场"
        );
    }
}
