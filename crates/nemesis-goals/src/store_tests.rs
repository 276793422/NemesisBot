//! store 存取测试（注册表原子写/update Err 不落盘/goal.md/evals/id/三清）。

use crate::state::{EvalRecord, GoalCreatedBy, GoalEntry, GoalStatus};
use crate::store::{
    alloc_id, append_eval, goal_md_path, goals_dir, load_registry, read_evals_tail, read_goal_md,
    registry_path, remove_goal, save_registry, update_registry, write_goal_md,
};

fn tmp_ws() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    (dir, root)
}

fn mk_entry(id: &str, status: GoalStatus) -> GoalEntry {
    let mut e = GoalEntry::new(
        id.to_string(),
        format!("agent:main:session:{id}"),
        GoalCreatedBy::Human,
        Some(10),
        None,
        1_000,
    );
    e.status = status;
    e
}

#[test]
fn registry_roundtrip_and_update() {
    let (_d, ws) = tmp_ws();
    // 空表（文件不存在）
    assert!(load_registry(&ws).unwrap().is_empty());
    // 写两条 → 读回
    save_registry(
        &ws,
        &[
            mk_entry("G1", GoalStatus::Active),
            mk_entry("G2", GoalStatus::Done),
        ],
    )
    .unwrap();
    let loaded = load_registry(&ws).unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].id, "G1");
    assert_eq!(loaded[0].status, GoalStatus::Active);
    assert_eq!(loaded[0].max_rounds, 10);
    assert_eq!(loaded[1].status, GoalStatus::Done);
    // update：状态转移 + 字段更新
    update_registry(&ws, |entries| {
        let e = entries.iter_mut().find(|e| e.id == "G1").unwrap();
        e.status = GoalStatus::Blocked;
        e.blocked_code = Some("round-limit".to_string());
        Ok(())
    })
    .unwrap();
    let loaded = load_registry(&ws).unwrap();
    assert_eq!(loaded[0].status, GoalStatus::Blocked);
    assert_eq!(loaded[0].blocked_code.as_deref(), Some("round-limit"));
}

#[test]
fn update_registry_err_leaves_file_untouched() {
    let (_d, ws) = tmp_ws();
    save_registry(&ws, &[mk_entry("G1", GoalStatus::Active)]).unwrap();
    // f 返回 Err → 整表放弃，内存中的修改不落盘
    let err: String = update_registry::<()>(&ws, |entries| {
        entries[0].status = GoalStatus::Done;
        Err("拒绝本次变更".to_string())
    })
    .unwrap_err();
    assert!(err.contains("拒绝本次变更"));
    let loaded = load_registry(&ws).unwrap();
    assert_eq!(loaded[0].status, GoalStatus::Active);
}

#[test]
fn goal_md_write_read_missing() {
    let (_d, ws) = tmp_ws();
    assert!(read_goal_md(&ws, "G1").unwrap().is_none());
    write_goal_md(&ws, "G1", "# 目标\n修复登录 bug").unwrap();
    assert_eq!(
        read_goal_md(&ws, "G1").unwrap().unwrap(),
        "# 目标\n修复登录 bug"
    );
    assert!(goal_md_path(&ws, "G1").ends_with("goals/G1.goal.md"));
}

#[test]
fn evals_append_and_tail() {
    let (_d, ws) = tmp_ws();
    assert!(read_evals_tail(&ws, "G1", 10).is_empty());
    for i in 0..5u64 {
        append_eval(
            &ws,
            "G1",
            &EvalRecord {
                at_ms: 1_000 + i as i64,
                round: i + 1,
                verdict: if i == 4 { "done" } else { "continue" }.to_string(),
                gap: (i != 4).then(|| format!("缺口{i}")),
                model: "test-model".to_string(),
                duration_ms: 100,
                raw_excerpt: String::new(),
            },
        )
        .unwrap();
    }
    let tail = read_evals_tail(&ws, "G1", 3);
    assert_eq!(tail.len(), 3);
    assert_eq!(tail[0].round, 3); // 原序旧→新
    assert_eq!(tail[2].verdict, "done");
    // 全量
    assert_eq!(read_evals_tail(&ws, "G1", 100).len(), 5);
}

#[test]
fn alloc_id_monotonic() {
    let (_d, ws) = tmp_ws();
    assert_eq!(alloc_id(&ws), "G1");
    std::fs::create_dir_all(goals_dir(&ws)).unwrap();
    write_goal_md(&ws, "G1", "a").unwrap();
    append_eval(
        &ws,
        "G3",
        &EvalRecord {
            at_ms: 1,
            round: 1,
            verdict: "continue".to_string(),
            gap: None,
            model: "m".to_string(),
            duration_ms: 1,
            raw_excerpt: String::new(),
        },
    )
    .unwrap();
    // 扫 G1.goal.md 与 G3.evals.jsonl → max+1
    assert_eq!(alloc_id(&ws), "G4");
}

/// 回归锁：alloc_id 必须并入注册表条目取 max——手删文件/部分恢复造成的
/// 文件-注册表错位下，不与残留条目撞 id（撞 id 会让按 id 查找的更新/删除
/// 命中错行，评估闸把 done 写到孤儿条目上、新目标永不能关闭）。
#[test]
fn alloc_id_considers_registry_not_just_files() {
    let (_d, ws) = tmp_ws();
    // 错位态：注册表有 G7，goals/ 目录文件全被手删 → 不得回退分配 G1
    save_registry(&ws, &[mk_entry("G7", GoalStatus::Active)]).unwrap();
    assert_eq!(alloc_id(&ws), "G8");
    // 混合态：文件 G2 + 注册表 G7 → 两者都算
    std::fs::create_dir_all(goals_dir(&ws)).unwrap();
    write_goal_md(&ws, "G2", "a").unwrap();
    assert_eq!(alloc_id(&ws), "G8");
    // 损坏注册表 = 读不出 → 不 panic，退回纯文件扫描语义
    std::fs::create_dir_all(ws.join("config")).unwrap();
    std::fs::write(ws.join("config").join("goals.json"), "{broken").unwrap();
    std::fs::remove_file(goal_md_path(&ws, "G2")).unwrap();
    assert_eq!(alloc_id(&ws), "G1");
}

#[test]
fn remove_goal_three_way_cleanup() {
    let (_d, ws) = tmp_ws();
    save_registry(
        &ws,
        &[
            mk_entry("G1", GoalStatus::Active),
            mk_entry("G2", GoalStatus::Done),
        ],
    )
    .unwrap();
    write_goal_md(&ws, "G1", "g").unwrap();
    append_eval(
        &ws,
        "G1",
        &EvalRecord {
            at_ms: 1,
            round: 1,
            verdict: "continue".to_string(),
            gap: None,
            model: "m".to_string(),
            duration_ms: 1,
            raw_excerpt: String::new(),
        },
    )
    .unwrap();
    remove_goal(&ws, "G1").unwrap();
    // 注册表行删除、文件两清、G2 不受累
    let loaded = load_registry(&ws).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "G2");
    assert!(read_goal_md(&ws, "G1").unwrap().is_none());
    assert!(read_evals_tail(&ws, "G1", 10).is_empty());
    // 不存在的 id = Err
    assert!(remove_goal(&ws, "G9").is_err());
}

// BUG-20261120-025 横向同族回归锁：记事本「带 BOM 的 UTF-8」保存形态——
// 旧形态裸 read_to_string 的前导 \u{FEFF} 会让 serde_json 报错（registry
// 全灭）/BOM 杂质进 goal.md 内容（LLM 上下文）/jsonl 首行误判损坏静默跳过。
// 三个读取面都必须对 BOM 前缀文件照常工作（read_config_string/strip_bom
// 单一真相源，BUG-20261115-036 同族纪律）。
#[test]
fn bom_prefixed_registry_loads() {
    let (_d, ws) = tmp_ws();
    save_registry(&ws, &[mk_entry("G1", GoalStatus::Active)]).unwrap();
    let path = registry_path(&ws);
    let plain = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("\u{feff}{plain}")).unwrap();
    let loaded = load_registry(&ws).expect("BOM 前缀注册表必须照常加载");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "G1");
}

#[test]
fn bom_prefixed_goal_md_content_is_stripped() {
    let (_d, ws) = tmp_ws();
    std::fs::create_dir_all(goals_dir(&ws)).unwrap();
    let path = goal_md_path(&ws, "G1");
    std::fs::write(&path, "\u{feff}目标正文").unwrap();
    let md = read_goal_md(&ws, "G1").unwrap().expect("goal.md 存在");
    assert!(!md.contains('\u{feff}'), "BOM 不得进 goal.md 内容");
    assert_eq!(md, "目标正文");
}

#[test]
fn bom_prefixed_evals_first_line_not_dropped() {
    let (_d, ws) = tmp_ws();
    for i in 0..2u64 {
        append_eval(
            &ws,
            "G1",
            &EvalRecord {
                at_ms: 1_000 + i as i64,
                round: i + 1,
                verdict: "continue".to_string(),
                gap: None,
                model: "test-model".to_string(),
                duration_ms: 100,
                raw_excerpt: String::new(),
            },
        )
        .unwrap();
    }
    let path = goals_dir(&ws).join("G1.evals.jsonl");
    let plain = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("\u{feff}{plain}")).unwrap();
    let tail = read_evals_tail(&ws, "G1", 10);
    assert_eq!(tail.len(), 2, "BOM 前缀不得吞掉 jsonl 首条评估记录");
    assert_eq!(tail[0].round, 1);
}
