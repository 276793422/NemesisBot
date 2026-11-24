//! （BUG-20261124-001/002）：跨世代 turn 撞号回归锁。
//!
//! turn 序号是 loop 内存计数（重启归零），checkpoint 索引持久化跨世代
//! 存活——网关重启后新世代 mint 的低号 turn 与盘上旧世代条目撞号。修复 =
//! 双闸（会话收窄 + begin 时间窗）：撞号条目只要 begin 时间早于锚点就
//! 退出波及面与恢复靶树。本文件用「真重启」形态（store 重建重载盘上
//! 索引）钉死：
//! - 撞号 turn 的冲突预检不把其他会话旧条目泛洪进 other_session_paths；
//! - restore 靶树锚定本会话最新 begin 的 tree（不再 `.next_back()` 取
//!   全局最高 turn 的树——那可能是别的会话/别的世代的）；
//! - `max_turn` 供 loop 自举（`set_checkpoint_store` fetch_max）。

use super::*;
use crate::r#loop::{FileChange, FileChangeKind};

/// 建一个带真 git 仓的临时 workspace（`ws/.git` 目录存在 → 影子库形态）。
fn bug24_git_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    (dir, root)
}

fn create(rel: &str) -> FileChange {
    FileChange {
        path: rel.to_string(),
        kind: FileChangeKind::Create,
    }
}

/// 会话 b 跑满 N 个 turn（b1.txt..bN.txt），全部落盘持久化。
/// 返回后 drop store——模拟上一世代进程退出（盘上索引仍在）。
async fn stage_sibling_generation(cp_dir: &std::path::Path, root: &std::path::Path, n: usize) {
    let store = CheckpointStore::new(Some(cp_dir.to_path_buf()), root.to_path_buf());
    std::fs::write(root.join("shared.txt"), "orig").unwrap();
    for t in 1..=n {
        store.begin_for_session(t, format!("b{t}"), Some("session-b"));
        let name = format!("b{t}.txt");
        std::fs::write(root.join(&name), format!("v{t}")).unwrap();
        // b1..b(t-1) 在本 turn begin 前已存在 → Modify；b{t} 自身 Create。
        store
            .snapshot_for_session(&create(&name), Some("session-b"))
            .await;
        store.seal_turn(t);
    }
}

/// 撞号不泛洪：上一世代 session-b 落盘 turn 1..3，本世代 session-a 以
/// turn 1 重新开局（与 b1 撞号——生产事故形态）。修复后冲突预检的时间
/// 窗把旧世代条目全部挡在波及面之外：other_session_paths 必须为空。
/// （修复前：`turn >= from_turn` 单闸把 b1..b3 全部算进回退波及面，
/// 用户删一轮对话弹出一堆无关文件的确认框。）
#[tokio::test]
async fn bug24_collision_turns_do_not_flood_other_session_paths() {
    let (_d, root) = bug24_git_root();
    let cp_dir = root.join("logs").join("checkpoints");

    // 上一世代：session-b turns 1..3（b1..b3.txt），持久化后进程退出。
    stage_sibling_generation(&cp_dir, &root, 3).await;

    // 本世代重启：session-a turn 1 与盘上 b1 撞号。
    let store = CheckpointStore::new(Some(cp_dir.clone()), root.clone());
    store.begin_for_session(1, "a1", Some("session-a"));
    std::fs::write(root.join("a1.txt"), "va1").unwrap();
    store
        .snapshot_for_session(&create("a1.txt"), Some("session-a"))
        .await;
    // 生产撞号形态：seal_turn 按 turn 查找 done 优先——封印的是 b 的旧
    // t1（已封印 → 幂等 no-op），a 的新条目留在 cur 未封印。照实复刻。
    store.seal_turn(1);

    let report = store.conflict_scan_scoped(1, Some("session-a"));
    assert!(
        report.other_session_paths.is_empty(),
        "撞号旧世代条目不得进波及面: {:?}",
        report.other_session_paths
    );
    assert!(
        report.conflicts.is_empty(),
        "a1.txt 指纹应干净（b 的旧条目被时间窗挡住）: {:?}",
        report.conflicts
    );
}

/// restore 靶树锚定本会话（BUG-20261124-002）：session-b 旧世代 turns
/// 1..3 之后，session-a 重启后跑 turn 1..2（a1/a2.txt，均因撞号未封印）。
/// `restore_code_scoped(2, Some("session-a"))` 的靶树必须锚定 a 自己最新
/// begin（turn 2）的 tree——回退删 a2.txt、保留 a1.txt 与 b1..b3；
/// 绝不 checkout b 的 t3 树（旧全局 `.next_back()` 行为 = 跨会话/跨世代
/// 碾盘）。from_turn 无本会话条目时诚实 no-op，盘面原样。
#[tokio::test]
async fn bug24_restore_target_anchors_own_session_not_global_max() {
    let (_d, root) = bug24_git_root();
    let cp_dir = root.join("logs").join("checkpoints");

    // 上一世代：session-b turns 1..3。
    stage_sibling_generation(&cp_dir, &root, 3).await;

    // 本世代：session-a turns 1..2（撞号 → 两条都留 cur 未封印）。
    let store = CheckpointStore::new(Some(cp_dir.clone()), root.clone());
    store.begin_for_session(1, "a1", Some("session-a"));
    std::fs::write(root.join("a1.txt"), "va1").unwrap();
    store
        .snapshot_for_session(&create("a1.txt"), Some("session-a"))
        .await;
    store.seal_turn(1); // no-op（封到 b 的旧 t1）
    store.begin_for_session(2, "a2", Some("session-a"));
    std::fs::write(root.join("a2.txt"), "va2").unwrap();
    store
        .snapshot_for_session(&create("a2.txt"), Some("session-a"))
        .await;
    store.seal_turn(2); // no-op（封到 b 的旧 t2）

    // 回退 a 的 turn 2：靶树 = a turn 2 begin 时刻（含 b1..b3 + a1，不含
    // a2）→ a2.txt 物理删除，其余原样。
    let report = store.restore_code_scoped(2, Some("session-a")).await;
    assert!(report.ok(), "恢复不得失败: {:?}", report.failures);
    assert!(
        !report.no_checkpoint_from_turn,
        "a 自己有 turn>=2 条目，不得报零条目"
    );
    assert!(
        report.deleted.iter().any(|p| p.ends_with("a2.txt")),
        "a2.txt 应被回退删除: deleted={:?}",
        report.deleted
    );
    assert!(
        root.join("a1.txt").exists(),
        "a1.txt 属于 turn 1（回退线之前），必须保留"
    );
    for t in 1..=3 {
        assert!(
            root.join(format!("b{t}.txt")).exists(),
            "b{t}.txt 是别的会话的文件，不得被碾"
        );
    }
    assert_eq!(
        std::fs::read_to_string(root.join("shared.txt")).unwrap(),
        "orig",
        "共享文件保持初值"
    );

    // from_turn=4：a 无 turn>=4 条目 → 诚实零条目（盘面原样，旧全局代码
    // 会 checkout b 的 t3 树——b1..b3 删除 + b2/b3 内容回退 = 灾难）。
    // a2.txt 维持上一臂的合法删除态（零条目臂不得复活任何东西）。
    let report4 = store.restore_code_scoped(4, Some("session-a")).await;
    assert!(
        report4.no_checkpoint_from_turn,
        "无本会话条目必须零条目信号"
    );
    assert!(report4.written.is_empty() && report4.deleted.is_empty());
    assert!(root.join("a1.txt").exists());
    assert!(!root.join("a2.txt").exists(), "零条目臂不得复活已回退文件");
    for t in 1..=3 {
        assert!(root.join(format!("b{t}.txt")).exists());
    }
}

/// max_turn 单元：空索引 None；内存+盘上载入并存取 max；重载后仍读到
/// 存量（loop 自举依赖）。
#[tokio::test]
async fn bug24_max_turn_covers_disk_and_memory() {
    let (_d, root) = bug24_git_root();
    let cp_dir = root.join("logs").join("checkpoints");

    let fresh = CheckpointStore::new(Some(cp_dir.clone()), root.clone());
    assert_eq!(fresh.max_turn(), None, "空索引 = None");

    // git 形态 begin 即落盘（tree 在场 + 与 prev 不同 → 持久化，落盘纪律
    // 与 JSON 形态不同——见 begin_for_session 的 changed_since_prev）：
    // t9 的 cur 条目同样有壳。断言双桶取 max。
    std::fs::write(root.join("f.txt"), "v0").unwrap();
    for t in [3usize, 7] {
        fresh.begin_for_session(t, format!("t{t}"), Some("s"));
        std::fs::write(root.join(format!("f{t}.txt")), format!("v{t}")).unwrap();
        fresh
            .snapshot_for_session(&create(&format!("f{t}.txt")), Some("s"))
            .await;
        fresh.seal_turn(t);
    }
    fresh.begin_for_session(9, "t9", Some("s"));
    std::fs::write(root.join("f9.txt"), "v9").unwrap();
    fresh
        .snapshot_for_session(&create("f9.txt"), Some("s"))
        .await;
    assert_eq!(
        fresh.max_turn(),
        Some(9),
        "done + cur 双桶取 max（t9 压过 t7）"
    );
    drop(fresh);

    // 重载：盘上条目（含未封印的 t9 壳）全部回载 → max 仍 9。loop 自举
    // 依据正是这条盘上真相——计数器顶到 10，新 mint 不与任何旧条目撞号。
    let reloaded = CheckpointStore::new(Some(cp_dir), root);
    assert_eq!(
        reloaded.max_turn(),
        Some(9),
        "重载后从盘上条目恢复 max（自举依据）"
    );
}
