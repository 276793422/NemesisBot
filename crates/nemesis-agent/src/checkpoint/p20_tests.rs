//! P20（2026-09-25 能力扩展 WS7）：turn 收尾封印 + rewind 冲突预检 + 影子库
//! 自愈单测。
//!
//! 覆盖：seal_turn 指纹封印与幂等 / conflict_scan 的 last-declared 规则
//! （外部修改报冲突、无外部修改零冲突、trailing 未封印诚实 unchecked、
//! 外部删除报冲突）/ heal_shadow_repo 清扫与幂等。

use super::*;
use crate::r#loop::{FileChange, FileChangeKind};

/// 建一个带真 git 仓的临时 workspace（`ws/.git` 目录存在 → 影子库形态）。
fn p20_git_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    (dir, root)
}

fn modify(rel: &str) -> FileChange {
    FileChange {
        path: rel.to_string(),
        kind: FileChangeKind::Modify,
    }
}

/// sha256 hex（与 `hash_on_disk` 同实现口径的测试侧参照——断言期望指纹用）。
fn p20_hash(content: &str) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(content.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// 三 turn 声明链 staging：turn1 声明改 code.txt 到 v2；turn2 → v3；turn3 →
/// v4。每次 `begin` 的兜底封印把上一 turn 以「当时盘面」指纹封存（生产中
/// 该动作由 process_admitted 收尾的 seal_turn 完成，begin 只是崩溃兜底——
/// 测试里走兜底路径等价覆盖同一 seal_turn 实现）。返回（store, root, 临时
/// 目录守护）；此时 turn 4 已开启、cur 未声明未封印。
async fn p20_stage_three_turns() -> (CheckpointStore, std::path::PathBuf, tempfile::TempDir) {
    let (d, root) = p20_git_root();
    std::fs::write(root.join("code.txt"), "v1").unwrap();
    let store = CheckpointStore::new(None, root.clone());
    assert_eq!(store.backend(), CheckpointBackend::Git);

    store.begin(1, "t1");
    std::fs::write(root.join("code.txt"), "v2").unwrap();
    store.snapshot(&modify("code.txt")).await;

    store.begin(2, "t2");
    std::fs::write(root.join("code.txt"), "v3").unwrap();
    store.snapshot(&modify("code.txt")).await;

    store.begin(3, "t3");
    std::fs::write(root.join("code.txt"), "v4").unwrap();
    store.snapshot(&modify("code.txt")).await;

    store.begin(4, "t4");
    (store, root, d)
}

// ---------------------------------------------------------------------------
// seal_turn：指纹封印 + 幂等
// ---------------------------------------------------------------------------

/// 落盘纪律保留：空 turn（无声明路径）封印不凭空造 turn-N.json（「翻页
/// 请求不留壳」纪律对封印路径同样成立）。
#[tokio::test]
async fn p20_seal_empty_turn_does_not_litter_persist_dir() {
    let (_d, root) = p20_git_root();
    let cp_dir = root.join("logs").join("checkpoints");
    let store = CheckpointStore::new(Some(cp_dir.clone()), root.clone());

    std::fs::write(root.join("f.txt"), "v1").unwrap();
    store.begin(1, "t1"); // 首 turn tree 首次已知 → 落盘
    assert!(cp_dir.join("turn-1.json").exists());

    store.begin(2, "t2 empty"); // 空 turn 不落盘（落盘纪律）
    assert!(!cp_dir.join("turn-2.json").exists());
    store.seal_turn(2); // 封印空 turn → 也不该凭空落壳
    assert!(!cp_dir.join("turn-2.json").exists(), "空 turn 封印不得留壳");
}

/// 封印指纹 = 封印时刻盘面；重复 seal 不覆盖（幂等）——外部改盘后二次
/// seal，期望指纹仍是第一次的。
#[tokio::test]
async fn p20_seal_turn_records_after_fingerprint_idempotent() {
    let (_d, root) = p20_git_root();
    std::fs::write(root.join("f.txt"), "v1").unwrap();
    let store = CheckpointStore::new(None, root.clone());

    store.begin(1, "t1");
    std::fs::write(root.join("f.txt"), "v2").unwrap();
    store.snapshot(&modify("f.txt")).await;

    store.seal_turn(1);
    // 外部改盘后再 seal：幂等——不重采指纹。
    std::fs::write(root.join("f.txt"), "v2-external").unwrap();
    store.seal_turn(1);

    let report = store.conflict_scan(1);
    assert_eq!(
        report.conflicts.len(),
        1,
        "期望指纹应停在第一次 seal 的盘面: {:?}",
        report.conflicts
    );
    assert_eq!(report.conflicts[0].path, "f.txt");
    assert_eq!(report.conflicts[0].expected_hash, p20_hash("v2"));
    assert_eq!(report.conflicts[0].current_hash, p20_hash("v2-external"));
}

/// 无外部修改时扫描干净（零冲突零 unchecked）。
#[tokio::test]
async fn p20_conflict_scan_clean_when_no_external_change() {
    let (store, root, _d) = p20_stage_three_turns().await;
    let _ = root;
    let report = store.conflict_scan(1);
    assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
    assert!(
        report.unchecked_paths.is_empty(),
        "{:?}",
        report.unchecked_paths
    );
}

// ---------------------------------------------------------------------------
// conflict_scan：last-declared 规则 + unchecked 诚实跳过
// ---------------------------------------------------------------------------

/// 外部修改 v5 后：conflict_scan(2) 认 **turn3**（turn >= 2 里最后声明
/// code.txt 的）封印指纹 hash(v4) 对比 hash(v5)——绝不回退到 turn1/2 的
/// 更旧 seal（那是更早状态，会把 agent 自己的 v2→v3 变更误报）。
#[tokio::test]
async fn p20_conflict_scan_uses_last_declared_seal_not_older() {
    let (store, root, _d) = p20_stage_three_turns().await;
    std::fs::write(root.join("code.txt"), "v5-external").unwrap();

    let report = store.conflict_scan(2);
    assert_eq!(report.conflicts.len(), 1, "{:?}", report.conflicts);
    assert_eq!(report.conflicts[0].path, "code.txt");
    assert_eq!(report.conflicts[0].expected_hash, p20_hash("v4"));
    assert_eq!(report.conflicts[0].current_hash, p20_hash("v5-external"));
    assert!(report.unchecked_paths.is_empty());
}

/// trailing turn 声明了路径但未封印 → unchecked 诚实跳过，不误报。
#[tokio::test]
async fn p20_conflict_scan_unchecked_for_unsealed_trailing_turn() {
    let (_d, root) = p20_git_root();
    std::fs::write(root.join("a.txt"), "v1").unwrap();
    let store = CheckpointStore::new(None, root.clone());

    store.begin(1, "t1");
    std::fs::write(root.join("a.txt"), "v2").unwrap();
    store.snapshot(&modify("a.txt")).await;
    std::fs::write(root.join("a.txt"), "v3-external").unwrap();

    // turn 1 是 cur 且未封印 → 扫描诚实跳过。
    let report = store.conflict_scan(1);
    assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
    assert_eq!(report.unchecked_paths, vec!["a.txt".to_string()]);
}

/// 封印时刻在盘、之后被外部删除 → 报冲突（current_hash 空串 = 不在盘）。
#[tokio::test]
async fn p20_conflict_scan_reports_externally_deleted_file() {
    let (_d, root) = p20_git_root();
    std::fs::write(root.join("keep.txt"), "precious").unwrap();
    let store = CheckpointStore::new(None, root.clone());

    store.begin(1, "t1");
    store.snapshot(&modify("keep.txt")).await;
    store.seal_turn(1);
    store.begin(2, "t2");

    std::fs::remove_file(root.join("keep.txt")).unwrap(); // 外部删除

    let report = store.conflict_scan(1);
    assert_eq!(report.conflicts.len(), 1, "{:?}", report.conflicts);
    assert_eq!(report.conflicts[0].path, "keep.txt");
    assert_eq!(report.conflicts[0].expected_hash, p20_hash("precious"));
    assert_eq!(report.conflicts[0].current_hash, "", "空串 = 已不在盘");
}

/// begin 的崩溃兜底封印：上一 turn 没走到收尾封印（生产崩溃模拟）时，
/// 下一个 begin 把它以当时盘面补封。
#[tokio::test]
async fn p20_begin_fallback_seals_previous_turn() {
    let (_d, root) = p20_git_root();
    std::fs::write(root.join("f.txt"), "v1").unwrap();
    let store = CheckpointStore::new(None, root.clone());

    store.begin(1, "t1");
    std::fs::write(root.join("f.txt"), "v2").unwrap();
    store.snapshot(&modify("f.txt")).await;
    // 注意：没有显式 seal_turn(1)——直接 begin(2)，模拟崩溃后重启的首个
    // turn：兜底封印应把 turn1 以 hash(v2) 封存。
    store.begin(2, "t2");

    std::fs::write(root.join("f.txt"), "v3-external").unwrap();
    let report = store.conflict_scan(1);
    assert_eq!(report.conflicts.len(), 1, "{:?}", report.conflicts);
    assert_eq!(report.conflicts[0].expected_hash, p20_hash("v2"));
}

// ---------------------------------------------------------------------------
// heal_shadow_repo：清扫 + 幂等
// ---------------------------------------------------------------------------

/// 清扫 tmp_obj_/tmp_pack_/*.lock 残留；合法文件（packed-refs、对象、
/// alternates）不动；二次清扫幂等返回 0；清扫后 store 仍可正常构造。
#[test]
fn p20_heal_shadow_repo_sweeps_temp_and_locks_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let shadow = dir.path().join("shadow.git");
    // 真实 bare 库打底（open_shadow 需要合法 git 形态——HEAD/config 等），
    // 残留文件再叠加上去。
    let mut opts = git2::RepositoryInitOptions::new();
    opts.bare(true).mkpath(true);
    git2::Repository::init_opts(&shadow, &opts).unwrap();
    std::fs::create_dir_all(shadow.join("objects").join("ab")).unwrap();
    std::fs::create_dir_all(shadow.join("refs").join("heads")).unwrap();

    // 崩溃残留四形态。
    std::fs::write(shadow.join("objects").join("tmp_obj_1234"), "x").unwrap();
    std::fs::write(shadow.join("objects").join("tmp_pack_5678"), "y").unwrap();
    std::fs::write(shadow.join("index.lock"), "z").unwrap();
    std::fs::write(shadow.join("refs").join("heads").join("main.lock"), "w").unwrap();
    // 合法文件——绝不误删。
    std::fs::write(shadow.join("packed-refs"), "ref stuff").unwrap();
    std::fs::write(shadow.join("objects").join("ab").join("cdef"), "obj").unwrap();
    std::fs::create_dir_all(shadow.join("objects").join("info")).unwrap();
    std::fs::write(
        shadow.join("objects").join("info").join("alternates"),
        "../x",
    )
    .unwrap();

    let removed = CheckpointStore::heal_shadow_repo(&shadow);
    assert_eq!(removed, 4, "四个残留全清扫");
    assert!(!shadow.join("objects").join("tmp_obj_1234").exists());
    assert!(!shadow.join("objects").join("tmp_pack_5678").exists());
    assert!(!shadow.join("index.lock").exists());
    assert!(!shadow.join("refs").join("heads").join("main.lock").exists());
    assert!(shadow.join("packed-refs").exists());
    assert!(shadow.join("objects").join("ab").join("cdef").exists());
    assert!(
        shadow
            .join("objects")
            .join("info")
            .join("alternates")
            .exists()
    );

    // 幂等：无残留二次清扫 = 0。
    assert_eq!(CheckpointStore::heal_shadow_repo(&shadow), 0);

    // 清扫后的影子库仍可被 store 正常打开（git 形态）。
    let (d, root) = p20_git_root();
    let _ = d;
    let store = CheckpointStore::new_with_shadow(None, root.clone(), shadow.clone());
    assert_eq!(store.backend(), CheckpointBackend::Git);
    store.begin(1, "after heal");
    assert_eq!(store.list_meta().len(), 1);
}
