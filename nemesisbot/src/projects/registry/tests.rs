//! （2026-09-08）：项目注册表契约测试——缺席/损坏/名称与路径校验/
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

    // rename 共用同一校验权威定义；trim 生效；path/id 不动。
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

    // 与已有 extra_dirs：精确相等 = 共享放行（建项顺序无关——先加目录后建
    // 项目与先建项目后加目录同一终态，2026-11-25 单一规则）；嵌套拒绝。
    let shared = mk("shared-assets");
    add_project_dir(&reg, &ok.id, shared.to_str().unwrap(), &ws).unwrap();
    let twin = create_project(&reg, &ws, "twin", shared.to_str().unwrap(), max).unwrap();
    assert_eq!(
        twin.path,
        canonicalize_for_compare(&shared),
        "项目主根 = 别人附加目录（精确相等）必须共享放行"
    );
    let shared_child = mk("shared-assets/inner");
    let err = create_project(&reg, &ws, "y2", shared_child.to_str().unwrap(), max)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("嵌套重叠") && err.contains("sibling"),
        "项目目录与别人附加目录嵌套必须拒绝（报错带归属项目名）: {err}"
    );
    assert_eq!(list_projects(&reg).len(), 3);
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
// 指令信任位——serde 缺省 true（老条目零迁移）+ set_project_trust RMW。
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
// 写并发串行化（同类横扫补钉）：create/remove/rename/set_trust 都
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

#[test]
fn test_registry_accepts_utf8_bom_file() {
    // BUG-20261109-036：注册表是用户可编辑资产（save 落 pretty JSON）——
    // Windows 记事本「带 BOM 的 UTF-8」保存形态曾被裸 read_to_string 解析
    // 失败，触发「已损坏请手动修复或删除」误导文案（文件完好，按提示删除
    // = 静默清空用户数据）。读侧走 BOM 剥离 helper 后 BOM 文件正常解析；
    // 真 BOM 损坏对照臂确认 loud 拒绝语义未被稀释。
    let ws = tempfile::tempdir().unwrap();
    let reg = registry_path(ws.path());
    std::fs::create_dir_all(reg.parent().unwrap()).unwrap();
    std::fs::write(
        &reg,
        format!("\u{feff}{}", serde_json::json!({"projects": []})),
    )
    .unwrap();
    let loaded = try_load_projects(&reg).expect("BOM 前缀的合法注册表必须可读");
    assert!(loaded.is_some(), "BOM 文件存在 → Some（非缺席）");

    // 对照：BOM + 真损坏 JSON 仍 loud 拒绝（helper 只剥 BOM，不吞错误）。
    std::fs::write(&reg, "\u{feff}{not-json").unwrap();
    assert!(try_load_projects(&reg).is_err(), "真损坏仍必须 loud 拒绝");
}

// ---------------------------------------------------------------------------
// 附加放行目录（extra_dirs，2026-11-19 D 批）：add/remove 是读-校验-写 RMW，
// 校验链单一规则（2026-11-25）：主 workspace 任何重叠拒绝；项目主根/
// 已有 extra_dir 精确相等 = 共享放行、嵌套拒绝（报错带归属项目名）。
// remove 摘列表不动磁盘。canonical 表示与 path 同源（剥 verbatim + 归一
// 8.3），serde default 零迁移。
// ---------------------------------------------------------------------------

#[test]
fn test_entry_extra_dirs_serde_default_empty() {
    // 老形态 JSON（无 extra_dirs 键）→ 空 vec（零迁移）。
    let legacy: ProjectEntry = serde_json::from_str(
        r#"{"id":"p-00000002","name":"老条目","path":"C:/p","created_at":"2026-09-08T00:00:00Z"}"#,
    )
    .unwrap();
    assert!(legacy.extra_dirs.is_empty());

    // 显式非空 round-trip 保留。
    let e = ProjectEntry {
        extra_dirs: vec!["D:/assets".to_string()],
        ..legacy.clone()
    };
    let back: ProjectEntry = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
    assert_eq!(back, e);
}

#[test]
fn test_add_remove_project_dir_happy_path_and_canonical_form() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let proj = parent.path().join("demo");
    let assets = parent.path().join("assets");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&assets).unwrap();
    let reg = registry_path(ws.path());

    let e = create_project(&reg, ws.path(), "演示项目", proj.to_str().unwrap(), 4).unwrap();
    let updated = add_project_dir(&reg, &e.id, assets.to_str().unwrap(), ws.path()).unwrap();
    let want = canonicalize_for_compare(&assets)
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        updated.extra_dirs,
        vec![want.clone()],
        "烧入 canonical 表示"
    );

    // 盘上 roundtrip。
    assert_eq!(
        find_project(&reg, &e.id).unwrap().extra_dirs,
        vec![want.clone()]
    );

    // 重复添加 = 诚实拒绝。
    let err = add_project_dir(&reg, &e.id, assets.to_str().unwrap(), ws.path())
        .unwrap_err()
        .to_string();
    assert!(err.contains("已存在"), "unexpected: {err}");

    // remove：出列表；目录与其内容原样（注册不拥有）。
    let after = remove_project_dir(&reg, &e.id, assets.to_str().unwrap()).unwrap();
    assert!(after.extra_dirs.is_empty());
    assert!(assets.is_dir(), "remove 只摘列表，绝不动磁盘");
    assert!(
        find_project(&reg, &e.id).unwrap().extra_dirs.is_empty(),
        "盘上状态必须与返回值一致"
    );
}

// ===== B38 孤儿组老化（2026-11-19）=====

/// 落一条指定 missing_since 的条目（直写 jsonl——走 save_projects 同一
/// 序列化面；项目目录可选创建）。
fn seed_entry(reg: &Path, dir: Option<&Path>, missing_since: Option<&str>) -> ProjectEntry {
    let ws_tmp = tempfile::tempdir().unwrap();
    let proj = match dir {
        Some(d) => d.to_path_buf(),
        None => ws_tmp.path().join("gone-project"),
    };
    if let Some(d) = dir {
        std::fs::create_dir_all(d).unwrap();
    }
    let entry = ProjectEntry {
        id: new_project_id(),
        name: "seed".into(),
        path: canonicalize_for_compare(&proj),
        created_at: chrono::Local::now().to_rfc3339(),
        instructions_trusted: true,
        extra_dirs: Vec::new(),
        missing_since: missing_since.map(str::to_string),
    };
    let mut file = try_load_projects(reg).unwrap().unwrap_or_default();
    file.projects.push(entry.clone());
    save_projects(reg, &file).unwrap();
    entry
}

#[test]
fn test_sweep_marks_first_observation_and_persists() {
    // 目录缺失 + 无戳 → 首次观测只记时（条目保留、不解除分组）。
    let ws = tempfile::tempdir().unwrap();
    let reg = registry_path(ws.path());
    let e = seed_entry(&reg, None, None);

    let (marked, aged) = sweep_missing_projects(&reg, 7 * 24 * 3600).unwrap();
    assert_eq!(marked.len(), 1, "无戳缺失条目必须进 marked：{marked:?}");
    assert!(aged.is_empty(), "首次观测绝不老化：{aged:?}");
    let stored = find_project(&reg, &e.id).unwrap();
    assert!(
        stored.missing_since.is_some(),
        "missing_since 必须落盘（下次启动才可判超期）"
    );
}

#[test]
fn test_add_dir_validation_and_overlap_matrix() {
    let root = tempfile::tempdir().unwrap();
    let ws = root.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let reg = registry_path(&ws);
    let mk = |name: &str| {
        let p = root.path().join(name);
        std::fs::create_dir_all(&p).unwrap();
        p
    };

    let proj_a = mk("proj-a");
    let e = create_project(&reg, &ws, "A", proj_a.to_str().unwrap(), 8).unwrap();
    let proj_b = mk("proj-b");
    let eb = create_project(&reg, &ws, "B", proj_b.to_str().unwrap(), 8).unwrap();

    // 入参校验：空 / 相对 / 不存在。
    let nope = root.path().join("nope");
    for (bad, needle) in [
        ("", "不能为空"),
        ("relative/path", "绝对路径"),
        (nope.to_str().unwrap(), "不存在"),
    ] {
        let err = add_project_dir(&reg, &e.id, bad, &ws)
            .unwrap_err()
            .to_string();
        assert!(err.contains(needle), "unexpected: {err}");
    }

    // 与主 workspace 重叠：相等 / 子目录。
    let sub_of_ws = mk("ws/sub");
    for cand in [ws.to_str().unwrap(), sub_of_ws.to_str().unwrap()] {
        let err = add_project_dir(&reg, &e.id, cand, &ws)
            .unwrap_err()
            .to_string();
        assert!(err.contains("主工作区"), "unexpected: {err}");
    }

    // 与项目主根嵌套重叠：嵌套拒绝（报错带归属项目名）。
    let child_of_b = mk("proj-b/child");
    let err = add_project_dir(&reg, &e.id, child_of_b.to_str().unwrap(), &ws)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("嵌套重叠") && err.contains("B"),
        "extra_dir 与项目主根嵌套必须拒绝: {err}"
    );

    // 精确相等 = 共享放行（2026-11-25 单一规则）：别的项目的主根（B 的
    // proj-b）挂为自己的附加目录；自己的主根（A 的 proj-a）同样放行
    // （无妨 no-op，单规则不特判）。
    add_project_dir(&reg, &e.id, proj_b.to_str().unwrap(), &ws).unwrap();
    add_project_dir(&reg, &e.id, proj_a.to_str().unwrap(), &ws).unwrap();

    // 与已有 extra_dir：嵌套拒绝（带归属，此时只有 B 拥有）；精确相等 =
    // 跨项目共享放行。
    let b_assets = mk("b-assets");
    add_project_dir(&reg, &eb.id, b_assets.to_str().unwrap(), &ws).unwrap();
    let b_assets_child = mk("b-assets/inner");
    let err = add_project_dir(&reg, &e.id, b_assets_child.to_str().unwrap(), &ws)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("嵌套重叠") && err.contains("「B」"),
        "与别人附加目录嵌套必须拒绝（报错带归属项目名）: {err}"
    );
    let shared = add_project_dir(&reg, &e.id, b_assets.to_str().unwrap(), &ws).unwrap();
    assert!(
        shared.extra_dirs.iter().any(|d| d.ends_with("b-assets")),
        "跨项目精确相等必须共享放行: {:?}",
        shared.extra_dirs
    );

    // ws 的兄弟目录合法（不在任何禁区）。
    let ok_dir = mk("ok-assets");
    let updated = add_project_dir(&reg, &e.id, ok_dir.to_str().unwrap(), &ws).unwrap();
    assert!(
        updated.extra_dirs.iter().any(|d| d.ends_with("ok-assets")),
        "合法目录入列: {:?}",
        updated.extra_dirs
    );
}

#[test]
fn test_add_dir_cap_and_unknown_project_and_corrupt() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let proj = parent.path().join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    let reg = registry_path(ws.path());

    let e = create_project(&reg, ws.path(), "demo", proj.to_str().unwrap(), 4).unwrap();

    // 上限 EXTRA_DIRS_MAX：第 max+1 个拒绝且不落盘。
    for i in 0..EXTRA_DIRS_MAX {
        let d = parent.path().join(format!("d{i}"));
        std::fs::create_dir_all(&d).unwrap();
        add_project_dir(&reg, &e.id, d.to_str().unwrap(), ws.path())
            .unwrap_or_else(|err| panic!("add {i} must succeed: {err}"));
    }
    let over = parent.path().join("d-over");
    std::fs::create_dir_all(&over).unwrap();
    let err = add_project_dir(&reg, &e.id, over.to_str().unwrap(), ws.path())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("上限") && err.contains('8'),
        "unexpected: {err}"
    );
    assert_eq!(
        find_project(&reg, &e.id).unwrap().extra_dirs.len(),
        EXTRA_DIRS_MAX,
        "被拒条目不落盘"
    );

    // 未知项目：与 remove/rename 同报错语义。
    let some = parent.path().join("some");
    std::fs::create_dir_all(&some).unwrap();
    let err = add_project_dir(&reg, "p-ffffffff", some.to_str().unwrap(), ws.path())
        .unwrap_err()
        .to_string();
    assert!(err.contains("项目不存在"), "unexpected: {err}");
    let err = remove_project_dir(&reg, "p-ffffffff", some.to_str().unwrap())
        .unwrap_err()
        .to_string();
    assert!(err.contains("项目不存在"), "unexpected: {err}");

    // remove 不存在的附加目录：诚实报错。
    let err = remove_project_dir(&reg, &e.id, some.to_str().unwrap())
        .unwrap_err()
        .to_string();
    assert!(err.contains("附加目录不存在"), "unexpected: {err}");

    // 损坏注册表：写路径 loud 拒绝。
    std::fs::write(&reg, "{not json").unwrap();
    let err = add_project_dir(&reg, &e.id, some.to_str().unwrap(), ws.path())
        .unwrap_err()
        .to_string();
    assert!(err.contains("损坏"), "unexpected: {err}");
}

#[test]
fn test_sweep_revival_clears_missing_since() {
    // TTL 内目录恢复 = 复活（判定基于目录存在性，非一次性快照）。
    let ws = tempfile::tempdir().unwrap();
    let proj = ws.path().join("revived");
    std::fs::create_dir_all(&proj).unwrap();
    let reg = registry_path(ws.path());
    let e = seed_entry(&reg, Some(&proj), Some("2020-01-01T00:00:00+08:00"));

    let (marked, aged) = sweep_missing_projects(&reg, 7 * 24 * 3600).unwrap();
    assert!(marked.is_empty() && aged.is_empty());
    let stored = find_project(&reg, &e.id).unwrap();
    assert!(
        stored.missing_since.is_none(),
        "目录存在必须清戳（超期戳也不例外——存在性是唯一判据）"
    );
}

#[test]
fn test_sweep_ages_out_after_ttl_and_zero_disables() {
    // 缺失 + 戳超 TTL → 条目移除（解除分组）；ttl=0 全程直通零改动。
    let ws = tempfile::tempdir().unwrap();
    let reg = registry_path(ws.path());
    let old = (chrono::Local::now() - chrono::Duration::days(8)).to_rfc3339();
    let e = seed_entry(&reg, None, Some(&old));

    // 对照臂：0 = 关，什么都不动。
    let (marked, aged) = sweep_missing_projects(&reg, 0).unwrap();
    assert!(marked.is_empty() && aged.is_empty(), "ttl=0 必须直通");
    assert!(find_project(&reg, &e.id).is_some(), "关闸时条目必须还在");

    // 7 天 TTL：8 天前的戳 → 老化移除。
    let (marked, aged) = sweep_missing_projects(&reg, 7 * 24 * 3600).unwrap();
    assert!(marked.is_empty());
    assert_eq!(aged.len(), 1);
    assert_eq!(aged[0].id, e.id);
    assert!(
        find_project(&reg, &e.id).is_none(),
        "老化条目必须从注册表移除（解除分组）"
    );
    // 零删除纪律：会话/项目文件不在 sweep 触达面（本测试无任何文件删除
    // 调用；sweep 只写 projects.json）。
}

#[test]
fn test_sweep_huge_ttl_clamped_no_panic_no_mass_ageout() {
    // BUG-20261127-021 回归钉：orphan_ttl_secs 是 u64 且无上界校验，此前
    // sweep 内 `chrono::Duration::seconds(ttl_secs as i64)` 裸转换双缺陷面：
    // ① ≥2^63 的值 `as i64` 回绕成负 TTL → `age >= ttl` 恒真 → 全部缺失
    //   条目立即老化（语义反转：用户想要「永不老化」得到「马上全解除分
    //   组」——session.rs u64::MAX as i64 = -1 陷阱同族，BUG-20261102-326
    //   / R735 有根）；
    // ② 正值超出 chrono 毫秒 i64 表示域（>~9.2e15 秒）→ Duration::seconds
    //   panic 语义 → gateway start_all 装配期崩溃（网关砖化）。
    // 修复 = time_bounds::MAX_AGE_SECS（100 年）钳制：对一切 sane 值恒等，
    // 巨值映射到「实际等于永不」，保用户意图不反转。
    let ws = tempfile::tempdir().unwrap();
    let reg = registry_path(ws.path());
    let old = (chrono::Local::now() - chrono::Duration::days(8)).to_rfc3339();
    let e = seed_entry(&reg, None, Some(&old));

    // u64::MAX（「永不老化」意图）→ 钳到 100 年：8 天前的戳不老化、不回绕。
    let (marked, aged) = sweep_missing_projects(&reg, u64::MAX).unwrap();
    assert!(marked.is_empty(), "已有戳条目不进首次观测：{marked:?}");
    assert!(aged.is_empty(), "回绕负 TTL 不得造成立即全量老化：{aged:?}");
    assert!(find_project(&reg, &e.id).is_some(), "条目必须保留");

    // chrono 表示域外的正值同样钳制，不 panic、不老化。
    let (marked, aged) = sweep_missing_projects(&reg, 10_000_000_000_000_000).unwrap();
    assert!(marked.is_empty() && aged.is_empty());
    assert!(find_project(&reg, &e.id).is_some());
}

#[test]
fn test_sweep_within_ttl_keeps_entry() {
    // 缺失 + 戳在 TTL 内 → 保持现状（条目在、戳不变、无新标记）。
    let ws = tempfile::tempdir().unwrap();
    let reg = registry_path(ws.path());
    let recent = (chrono::Local::now() - chrono::Duration::days(1)).to_rfc3339();
    let e = seed_entry(&reg, None, Some(&recent));

    let (marked, aged) = sweep_missing_projects(&reg, 7 * 24 * 3600).unwrap();
    assert!(marked.is_empty() && aged.is_empty());
    let stored = find_project(&reg, &e.id).unwrap();
    assert_eq!(
        stored.missing_since.as_deref(),
        Some(recent.as_str()),
        "TTL 内戳必须原样保留（不重置计时）"
    );
}

#[test]
fn test_sweep_corrupt_stamp_reanchors_not_ages() {
    // 戳损坏：re-anchor 到现在（从零重数）——绝不因坏戳直接老化（从严）。
    let ws = tempfile::tempdir().unwrap();
    let reg = registry_path(ws.path());
    let e = seed_entry(&reg, None, Some("not-a-timestamp"));

    let (marked, aged) = sweep_missing_projects(&reg, 7 * 24 * 3600).unwrap();
    assert!(aged.is_empty(), "坏戳不得触发老化：{aged:?}");
    assert_eq!(marked.len(), 1, "坏戳条目按首次观测重记");
    let stored = find_project(&reg, &e.id).unwrap();
    let ts = stored.missing_since.expect("re-anchor 必须落新戳");
    assert!(
        chrono::DateTime::parse_from_rfc3339(&ts).is_ok(),
        "新戳必须是合法 RFC3339: {ts}"
    );
}

#[test]
fn test_sweep_corrupt_registry_loud_rejects() {
    // 注册表损坏 = Err loud 拒绝（绝不拿 sweep 覆盖损坏文件——同写路径
    // 纪律），文件原样保留。
    let ws = tempfile::tempdir().unwrap();
    let reg = registry_path(ws.path());
    seed_entry(&reg, None, None);
    std::fs::write(&reg, "{broken").unwrap();
    let before = std::fs::read_to_string(&reg).unwrap();

    assert!(sweep_missing_projects(&reg, 7 * 24 * 3600).is_err());
    assert_eq!(
        std::fs::read_to_string(&reg).unwrap(),
        before,
        "损坏文件必须原样保留（防静默清空）"
    );
}

#[test]
fn test_old_shape_entry_without_missing_since_is_none() {
    // 老形态兼容：手写 JSON 缺 missing_since 键 → None（serde default）；
    // roundtrip 保真（有戳条目保存后再读不丢）。
    let ws = tempfile::tempdir().unwrap();
    let reg = registry_path(ws.path());
    let proj = ws.path().join("legacy");
    std::fs::create_dir_all(&proj).unwrap();
    let body = serde_json::json!({
        "projects": [{
            "id": "p-deadbeef",
            "name": "legacy",
            "path": proj.to_string_lossy(),
            "created_at": "2026-01-01T00:00:00+08:00",
            "instructions_trusted": true
        }]
    });
    std::fs::create_dir_all(reg.parent().unwrap()).unwrap();
    std::fs::write(&reg, body.to_string()).unwrap();

    let stored = find_project(&reg, "p-deadbeef").expect("老形态条目必须可读");
    assert!(stored.missing_since.is_none(), "缺键必须落 None");

    // roundtrip：带戳条目保存后重读保真。
    let mut file = try_load_projects(&reg).unwrap().unwrap();
    file.projects[0].missing_since = Some("2026-01-02T00:00:00+08:00".into());
    save_projects(&reg, &file).unwrap();
    let reloaded = find_project(&reg, "p-deadbeef").unwrap();
    assert_eq!(
        reloaded.missing_since.as_deref(),
        Some("2026-01-02T00:00:00+08:00"),
        "带戳 roundtrip 必须保真"
    );
}
