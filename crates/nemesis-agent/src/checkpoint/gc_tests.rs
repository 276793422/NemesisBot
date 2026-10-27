//! M6（2026-10-27 整改批次五）：checkpoint gc 回归。
//!
//! - **纯函数**：保留集三合一（turn 窗 ∪ 时间窗 ∪ extras ∪ 保护集）、
//!   零维全清、不可解析时间无时间窗豁免。
//! - **JSON 形态**：窗口外 turn-N.json 直删 + 内存索引对齐；未装配 /
//!   enabled=false 全路径零触碰；全保留早退。
//! - **git 影子形态**（checkpoint-git）：重建式 gc 后保留 tree restore 全
//!   通过、窗口外 tree 不可达；undo/redo 栈触达 tree（extras）跨窗保留；
//!   交付分支 refs 闭包存活；enabled=false 零触碰（对象集/镜像不动）。
//!
//! 触发点回归：truncate_from_scoped 尾部挂 gc_now——未装配时零触碰由
//! 既有 truncate 测试矩阵全量兜底（默认态 gc_now 首行跳过）。

use super::*;
use std::sync::Arc;

fn gc_hooks(
    enabled: bool,
    keep_turns: usize,
    keep_days: u64,
    extras: Vec<String>,
) -> (
    nemesis_config::CheckpointGcConfig,
    Arc<dyn Fn() -> Vec<String> + Send + Sync>,
) {
    let cfg = nemesis_config::CheckpointGcConfig {
        enabled,
        keep_turns,
        keep_days,
    };
    (cfg, Arc::new(move || extras.clone()))
}

fn no_extras() -> Arc<dyn Fn() -> Vec<String> + Send + Sync> {
    Arc::new(Vec::new)
}

// ---------------------------------------------------------------------------
// 纯函数：保留集
// ---------------------------------------------------------------------------

fn entry(turn: usize, time: &str, tree: Option<&str>) -> crate::checkpoint::gc::RetentionEntry {
    crate::checkpoint::gc::RetentionEntry {
        turn,
        time: time.to_string(),
        tree: tree.map(str::to_string),
    }
}

#[test]
fn retention_turn_window_and_time_window() {
    // turn 窗：最近 N 个；时间窗：7 天内豁免（即使掉出 turn 窗）。
    let now_unix = chrono::Utc::now().timestamp();
    let now = chrono::Utc::now();
    let old = (now - chrono::Duration::days(40)).to_rfc3339();
    let fresh = (now - chrono::Duration::days(1)).to_rfc3339();
    let entries: Vec<_> = (1..=10)
        .map(|t| {
            if t <= 7 {
                entry(t, &old, Some(&format!("tree-{t}")))
            } else {
                entry(t, &fresh, Some(&format!("tree-{t}")))
            }
        })
        .collect();
    // keep_turns=3 且时间窗关：8/9/10。
    let d =
        crate::checkpoint::gc::compute_retention(&entries, &HashSet::new(), &[], now_unix, 3, 0);
    assert_eq!(d.kept_turns, HashSet::from([8, 9, 10]));
    assert_eq!(
        d.kept_trees,
        HashSet::from([
            "tree-8".to_string(),
            "tree-9".to_string(),
            "tree-10".to_string()
        ])
    );
    // keep_turns=3 + 时间窗 7 天：1..=5 是 40 天前（窗外），6..=10 fresh
    //（1 天前）→ 时间窗豁免全保。
    let entries2: Vec<_> = (1..=10)
        .map(|t| {
            if t <= 5 {
                entry(t, &old, Some(&format!("tree-{t}")))
            } else {
                entry(t, &fresh, Some(&format!("tree-{t}")))
            }
        })
        .collect();
    let d2 =
        crate::checkpoint::gc::compute_retention(&entries2, &HashSet::new(), &[], now_unix, 3, 7);
    assert_eq!(d2.kept_turns, (6..=10).collect::<HashSet<_>>());
}

#[test]
fn retention_extras_and_protected_merge() {
    let now = chrono::Utc::now();
    let old = (now - chrono::Duration::days(90)).to_rfc3339();
    let entries = vec![
        entry(1, &old, Some("tree-1")),
        entry(2, &old, Some("tree-2")),
    ];
    // extras 触达 tree-1：条目 1 连 tree 保留（双窗全关也会保）。
    let d = crate::checkpoint::gc::compute_retention(
        &entries,
        &HashSet::new(),
        &["tree-1".to_string()],
        0,
        0,
        0,
    );
    assert!(d.kept_turns.contains(&1));
    assert!(!d.kept_turns.contains(&2));
    assert!(d.kept_trees.contains("tree-1"));
    assert!(!d.kept_trees.contains("tree-2"));
    // extras 不对应任何索引条目（undo 时刻实时盘面 tree）也进对象保留集。
    let d2 = crate::checkpoint::gc::compute_retention(
        &entries,
        &HashSet::new(),
        &["ghost-tree".to_string()],
        0,
        0,
        0,
    );
    assert!(d2.kept_trees.contains("ghost-tree"));
    assert!(d2.kept_turns.is_empty());
    // protected（进行中 cur turn）恒保留。
    let d3 = crate::checkpoint::gc::compute_retention(&entries, &HashSet::from([2]), &[], 0, 0, 0);
    assert_eq!(d3.kept_turns, HashSet::from([2]));
}

#[test]
fn retention_unparseable_time_no_day_exemption() {
    let entries = vec![entry(1, "not-a-timestamp", Some("tree-1"))];
    // 时间窗开着也救不了不可解析时间（turn 窗/extras/protected 之外即清）。
    let d = crate::checkpoint::gc::compute_retention(&entries, &HashSet::new(), &[], 0, 0, 7);
    assert!(d.kept_turns.is_empty());
}

// ---------------------------------------------------------------------------
// JSON 形态
// ---------------------------------------------------------------------------

/// 无 .git 工作区 + 直接落 turn-N.json 镜像（load 回载进索引）。
fn json_fixture(mirrors: &[(usize, &str)]) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let cp_dir = dir.path().join("cps");
    std::fs::create_dir_all(&cp_dir).unwrap();
    for (turn, time) in mirrors {
        let cp = Checkpoint {
            turn: *turn,
            time: time.to_string(),
            prompt: format!("turn {turn}"),
            files: Vec::new(),
            paths: vec![format!("f{turn}.txt")],
            tree: None,
            after: Vec::new(),
            sealed: true,
            session: None,
        };
        std::fs::write(
            cp_dir.join(format!("turn-{turn}.json")),
            serde_json::to_vec_pretty(&cp).unwrap(),
        )
        .unwrap();
    }
    (dir, root, cp_dir)
}

#[test]
fn gc_json_deletes_expired_keeps_window() {
    let now = chrono::Utc::now();
    let old = (now - chrono::Duration::days(40)).to_rfc3339();
    let (_d, root, cp_dir) =
        json_fixture(&[(1, old.as_str()), (2, old.as_str()), (3, old.as_str())]);
    let store = CheckpointStore::new(Some(cp_dir.clone()), root);
    assert_eq!(store.backend(), CheckpointBackend::Json);
    assert_eq!(store.list_meta().len(), 3);

    store.set_gc(
        nemesis_config::CheckpointGcConfig {
            enabled: true,
            keep_turns: 1,
            keep_days: 0,
        },
        no_extras(),
    );
    let report = store.gc_now();
    assert_eq!(report.backend, "json");
    assert_eq!(report.dropped_turns, 2);
    assert_eq!(report.deleted_mirrors, 2, "turn-1/2 镜像删除");
    assert!(report.bytes_freed > 0);
    assert!(report.skipped.is_none());
    // 磁盘对齐（内容断言，不断言目录存在性）。
    let remain: Vec<String> = std::fs::read_dir(&cp_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(remain, vec!["turn-3.json".to_string()]);
    // 内存索引对齐。
    let metas = store.list_meta();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].turn, 3);
}

#[test]
fn gc_json_early_exit_when_all_kept() {
    let now = chrono::Utc::now();
    let fresh = (now - chrono::Duration::days(1)).to_rfc3339();
    let (_d, root, cp_dir) = json_fixture(&[(1, fresh.as_str()), (2, fresh.as_str())]);
    let store = CheckpointStore::new(Some(cp_dir.clone()), root);
    store.set_gc(
        nemesis_config::CheckpointGcConfig {
            enabled: true,
            keep_turns: 200,
            keep_days: 7,
        },
        no_extras(),
    );
    let report = store.gc_now();
    assert_eq!(
        report.skipped.as_deref(),
        Some("保留集覆盖全部条目，无需清扫")
    );
    assert_eq!(report.dropped_turns, 0);
    assert_eq!(std::fs::read_dir(&cp_dir).unwrap().count(), 2);
}

#[test]
fn gc_disabled_and_unassembled_zero_touch() {
    let now = chrono::Utc::now();
    let old = (now - chrono::Duration::days(400)).to_rfc3339();
    let (_d, root, cp_dir) = json_fixture(&[(1, old.as_str()), (2, old.as_str())]);
    let before: Vec<String> = std::fs::read_dir(&cp_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    // 未装配：gc_now 诚实跳过，零触碰。
    let store = CheckpointStore::new(Some(cp_dir.clone()), root.clone());
    let r0 = store.gc_now();
    assert!(r0.skipped.as_deref().unwrap().contains("未装配"));
    assert_eq!(store.list_meta().len(), 2);

    // 装配但 enabled=false（默认形态）：同样零触碰。
    let (cfg, extras) = gc_hooks(false, 1, 0, vec![]);
    store.set_gc(cfg, extras);
    let r1 = store.gc_now();
    assert!(r1.skipped.as_deref().unwrap().contains("enabled=false"));
    assert_eq!(store.list_meta().len(), 2);
    let after: Vec<String> = std::fs::read_dir(&cp_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(before, after);
}

// BUG-95 配套：gc_armed 自省面（factory 集群闸测试消费）。
#[test]
fn gc_armed_reflects_hooks_and_enabled() {
    let d = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(None, d.path().to_path_buf());

    // 未装配 = 未武装。
    assert!(!store.gc_armed());

    // 装配但 enabled=false（默认形态）= 仍未武装。
    let (cfg_off, extras) = gc_hooks(false, 1, 0, vec![]);
    store.set_gc(cfg_off, extras);
    assert!(!store.gc_armed());

    // 装配且 enabled=true = 武装。
    let (cfg_on, extras_on) = gc_hooks(true, 1, 0, vec![]);
    store.set_gc(cfg_on, extras_on);
    assert!(store.gc_armed());
}

// ---------------------------------------------------------------------------
// git 影子形态（checkpoint-git）
// ---------------------------------------------------------------------------

#[cfg(feature = "checkpoint-git")]
mod git_gc {
    use super::*;
    use crate::checkpoint::CheckpointBackend;

    /// 真 git 仓 workspace + 持久化 cp 目录；四连 turn 各写 f.txt 一版
    /// （begin 即写 tree，tree 逐 turn 变化 → 四份镜像全落盘）。
    fn fixture() -> (
        tempfile::TempDir,
        PathBuf,
        Arc<CheckpointStore>,
        Vec<String>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        git2::Repository::init(&root).unwrap();
        let cp_dir = dir.path().join("cps");
        let store = Arc::new(CheckpointStore::new(Some(cp_dir.clone()), root.clone()));
        assert_eq!(store.backend(), CheckpointBackend::Git);
        let mut trees = Vec::new();
        for turn in 1..=4u32 {
            let f = root.join("f.txt");
            std::fs::write(&f, format!("v{turn}")).unwrap();
            store.begin(turn as usize, format!("turn {turn}"));
            // 从落盘镜像取该 turn 的影子 tree hex。
            let bytes = std::fs::read(cp_dir.join(format!("turn-{turn}.json"))).unwrap();
            let cp: Checkpoint = serde_json::from_slice(&bytes).unwrap();
            trees.push(cp.tree.expect("git 形态 begin 必有 tree"));
        }
        (dir, root, store, trees)
    }

    fn enabled_cfg(keep_turns: usize) -> nemesis_config::CheckpointGcConfig {
        nemesis_config::CheckpointGcConfig {
            enabled: true,
            keep_turns,
            keep_days: 0,
        }
    }

    #[test]
    fn gc_git_rebuild_keeps_retained_restorable_drops_window() {
        let (_d, root, store, trees) = fixture();
        store.set_gc(enabled_cfg(2), no_extras());
        let report = store.gc_now();

        assert_eq!(report.backend, "git");
        assert_eq!(report.entries_total, 4);
        assert_eq!(report.kept_turns, 2);
        assert_eq!(report.dropped_turns, 2);
        assert_eq!(report.deleted_mirrors, 2);
        assert_eq!(report.trees_kept, 2);
        assert!(
            report.skipped.is_none(),
            "重建不应放弃：{:?}",
            report.skipped
        );
        // 镜像对齐：1/2 删，3/4 留。
        for turn in [1usize, 2] {
            assert!(
                !root
                    .parent()
                    .unwrap()
                    .join("cps")
                    .join(format!("turn-{turn}.json"))
                    .exists(),
                "turn-{turn} 镜像应已删"
            );
        }
        // 保留 tree：restore 全通过 + 内容正确（rewind 文件路径回归）。
        let report3 = store.restore_to_tree(&trees[2]).unwrap();
        assert!(report3.ok(), "tree3 restore 失败: {:?}", report3.failures);
        assert_eq!(std::fs::read(root.join("f.txt")).unwrap(), b"v3");
        let report4 = store.restore_to_tree(&trees[3]).unwrap();
        assert!(report4.ok(), "tree4 restore 失败: {:?}", report4.failures);
        assert_eq!(std::fs::read(root.join("f.txt")).unwrap(), b"v4");
        // redo/diff 读路径：read_file_from_tree 对保留 tree 正常。
        assert_eq!(
            store.read_file_from_tree(&trees[2], "f.txt").unwrap(),
            Some("v3".to_string())
        );
        // 窗口外 tree：对象集已清，restore 诚实失败（不可达）。
        let dropped = store.restore_to_tree(&trees[0]).unwrap();
        assert!(
            !dropped.ok(),
            "gc 掉的 tree 必须不可 restore（否则白扫）: {:?}",
            dropped.failures
        );
    }

    #[test]
    fn gc_git_extra_trees_keep_redo_target_across_window() {
        let (_d, _root, store, trees) = fixture();
        // tree1 掉出 keep_turns=2 窗口，但 undo/redo 栈触达 → 保留。
        let extras = vec![trees[0].clone()];
        store.set_gc(
            enabled_cfg(2),
            Arc::new(move || extras.clone()) as Arc<dyn Fn() -> Vec<String> + Send + Sync>,
        );
        let report = store.gc_now();
        assert_eq!(report.extra_trees, 1);
        assert_eq!(report.trees_kept, 3, "窗口内 2 + extras 1");
        assert!(report.skipped.is_none());
        assert_eq!(
            store.read_file_from_tree(&trees[0], "f.txt").unwrap(),
            Some("v1".to_string()),
            "redo 前向恢复目标必须跨窗存活"
        );
    }

    #[test]
    fn gc_git_delivery_branch_refs_survive() {
        let (_d, root, store, trees) = fixture();
        // 交付分支捕获（refs/heads/nemesis/* → commit → tree）：gc 闭包保留。
        // commit tree = 捕获时工作区盘面（v4，在保留窗内）；base 只做 diff 基线。
        store
            .capture_delivery_branch(Some(&trees[0]), "review-a")
            .expect("交付分支捕获");
        store.set_gc(enabled_cfg(2), no_extras());
        let report = store.gc_now();
        assert!(report.skipped.is_none(), "{:?}", report.skipped);
        // ref 仍解析到原 commit（refs 闭包 + commit 对象存活）。
        // delivery_branch_refs 返回短名（去 refs/heads/nemesis/ 前缀）。
        let refs = store.delivery_branch_refs().unwrap();
        assert!(refs.iter().any(|r| r == "review-a"));
        // 交付 patch 重算面在 gc 后仍可用——空树基线（恒可构造）出全量补丁。
        let (patch, truncated) = store
            .delivery_branch_patch("review-a", None)
            .expect("gc 后交付 patch 重算必须成功");
        assert!(!truncated);
        assert!(!patch.is_empty());
        assert!(patch.contains("f.txt"));
        // 诚实边界：调用方传入的窗外 base tree（非 refs 成员）patch 重算诚实
        // 失败——caller-supplied base 不属闭包保护面，近窗交付不受影响。
        assert!(
            store
                .delivery_branch_patch("review-a", Some(&trees[0]))
                .is_err()
        );
        let _ = root;
    }

    #[test]
    fn gc_git_disabled_zero_touch() {
        let (_d, root, store, trees) = fixture();
        let (cfg, extras) = gc_hooks(false, 2, 0, vec![]);
        store.set_gc(cfg, extras);
        let report = store.gc_now();
        assert!(report.skipped.as_deref().unwrap().contains("enabled=false"));
        // 全部 tree 仍可 restore（对象集零触碰）。
        for (i, t) in trees.iter().enumerate() {
            assert!(
                store.read_file_from_tree(t, "f.txt").unwrap().is_some(),
                "tree{} 在 gc 关闭态必须存活",
                i + 1
            );
        }
        assert_eq!(
            std::fs::read(root.join("f.txt")).unwrap(),
            b"v4",
            "零触碰 = 工作区不动"
        );
        // 镜像零触碰。
        let cps = root.parent().unwrap().join("cps");
        assert_eq!(std::fs::read_dir(&cps).unwrap().count(), 4);
    }

    #[test]
    fn gc_git_pack_form_fail_closed() {
        let (_d, root, store, _trees) = fixture();
        // 影子库塞一个假 pack 文件 → 重建式 fail-closed 诚实放弃。
        let shadow = nemesis_path::logs_dir_in_workspace(&root).join("checkpoints.git");
        let pack_dir = shadow.join("objects").join("pack");
        std::fs::create_dir_all(&pack_dir).unwrap();
        std::fs::write(pack_dir.join("fake.pack"), b"not a real pack").unwrap();
        store.set_gc(enabled_cfg(2), no_extras());
        let report = store.gc_now();
        assert!(
            report.skipped.as_deref().unwrap().contains("pack 文件"),
            "pack 形态必须诚实放弃: {:?}",
            report.skipped
        );
        // 零破坏：索引/镜像原样。
        assert_eq!(store.list_meta().len(), 4);
    }

    // BUG-96（2026-10-28 二次复查）：残留 .gc-old 两分支回归。
    // 常态残留：上轮收尾删除重试耗尽的遗留不阻塞本轮 gc（修复前 rename#1
    // 目标非空必败 → gc 永久卡死），且本轮顺手清走。
    #[test]
    fn gc_git_stale_gc_old_residual_does_not_brick() {
        let (_d, root, store, trees) = fixture();
        let shadow = nemesis_path::logs_dir_in_workspace(&root).join("checkpoints.git");
        let stale = shadow.with_file_name("checkpoints.git.gc-old");
        let stale_obj = stale.join("objects").join("ab").join("junk");
        std::fs::create_dir_all(stale_obj.parent().unwrap()).unwrap();
        std::fs::write(&stale_obj, b"residual from a previous round").unwrap();

        store.set_gc(enabled_cfg(2), no_extras());
        let report = store.gc_now();
        assert!(
            report.skipped.is_none(),
            "残留 .gc-old 不得阻塞 gc（修复前在此被换名失败卡死）: {:?}",
            report.skipped
        );
        assert!(!stale.exists(), "残留 .gc-old 应被清走");
        // 本轮清扫本身照常：保留窗 tree 可 restore。
        assert!(
            store
                .read_file_from_tree(&trees[3], "f.txt")
                .unwrap()
                .is_some(),
            "gc 后保留 tree 必须存活"
        );
    }

    // 破损态自愈：objects 缺席 + .gc-old 在场（上轮 rename#2 失败且回滚也
    // 失败）→ 搬回自愈后 gc 照常完成（修复前 odb 打不开，永久报错）。
    #[test]
    fn gc_git_broken_state_self_heals_from_gc_old() {
        let (_d, root, store, trees) = fixture();
        let shadow = nemesis_path::logs_dir_in_workspace(&root).join("checkpoints.git");
        // 精确复刻破损态：objects 原样搬进 .gc-old 名下（= 回滚失败现场）。
        std::fs::rename(
            shadow.join("objects"),
            shadow.with_file_name("checkpoints.git.gc-old"),
        )
        .unwrap();
        assert!(!shadow.join("objects").exists());

        store.set_gc(enabled_cfg(2), no_extras());
        let report = store.gc_now();
        assert!(
            report.skipped.is_none(),
            "破损态应自愈后完成 gc: {:?}",
            report.skipped
        );
        assert!(shadow.join("objects").exists(), "自愈 = objects 回到原位");
        // gc 真做完了：窗口外 tree 已清（对象缺失 → find_tree Err）、
        // 保留窗 tree 存活。
        assert!(
            store.read_file_from_tree(&trees[0], "f.txt").is_err(),
            "窗外 tree 对象应已清（对象缺失 → Err）"
        );
        assert!(
            store
                .read_file_from_tree(&trees[3], "f.txt")
                .unwrap()
                .is_some(),
            "保留窗 tree 必须存活"
        );
    }
}
