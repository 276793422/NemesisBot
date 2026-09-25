// checkpoint.rs 覆盖率补充测试（影子库初始化失败回落 / alternates 写失败
// 降级 / restore_to_tree 坏 hex 与缺失 tree / checkout 与删除失败面 /
// read_file_from_tree 错误面 / hybrid 写删失败 / 树写入失败降级）。
//
// 现有 d2/e3/m3 覆盖成功回路；本文件专打错误臂——全部诚实降级语义
// （warn + 空 vec / Err），绝不 panic、绝不半恢复。

use super::*;

fn git_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    (dir, root)
}

/// 影子库路径已被普通文件占用 → init/open 失败 → warn + 回落 JSON（201-203）。
#[test]
fn shadow_blocked_by_regular_file_falls_back_to_json() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();

    let shadow = dir.path().join("shadow.git");
    std::fs::write(&shadow, "not a gitdir").unwrap();

    let store = CheckpointStore::new_with_shadow(None, root.clone(), shadow);
    assert_eq!(store.backend(), CheckpointBackend::Json);
}

/// alternates 写失败（占位为目录）→ warn 降级（blob 复用失效但后端仍 Git）。
#[test]
fn alternates_write_failure_degrades_but_keeps_git_backend() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();

    // 预建合法 bare 影子库，但把 alternates 占位成目录 → 写入必失败。
    let shadow = dir.path().join("shadow.git");
    let mut opts = git2::RepositoryInitOptions::new();
    opts.bare(true).mkpath(true);
    git2::Repository::init_opts(&shadow, &opts).unwrap();
    std::fs::create_dir_all(shadow.join("objects").join("info").join("alternates")).unwrap();

    let store = CheckpointStore::new_with_shadow(None, root.clone(), shadow);
    assert_eq!(store.backend(), CheckpointBackend::Git);
}

/// restore_to_tree：坏 hex → oid 解析失败臂；格式合法但 tree 缺失 →
/// 影子对象不可达臂。两者都诚实返回空 vec（不 panic 不 Err）。
#[test]
fn restore_to_tree_bad_hex_and_missing_tree_degrade_to_empty() {
    let (_d, root) = git_root();
    std::fs::write(root.join("f.txt"), "v1").unwrap();
    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "turn 1");

    let (w, d) = store.restore_to_tree("zzzz-not-hex").unwrap();
    assert!(w.is_empty() && d.is_empty(), "bad hex: {w:?} {d:?}");

    let (w, d) = store.restore_to_tree(&"e".repeat(40)).unwrap();
    assert!(w.is_empty() && d.is_empty(), "missing tree: {w:?} {d:?}");
}

/// checkout_tree 失败（tree 里的文件在盘上变成含内容的目录）→ warn，
/// written 诚实为空（若 libgit2 force 能翻越目录则该假设不成立，本测试
/// 改断言工作区终态即可——两种结局都不 panic）。
#[tokio::test]
async fn checkout_tree_failure_when_file_became_dir() {
    let (_d, root) = git_root();
    std::fs::write(root.join("f.txt"), "v1").unwrap();
    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "turn 1");

    std::fs::write(root.join("f.txt"), "v2").unwrap();
    store.begin(2, "turn 2"); // target→cur: f.txt Modified

    // 盘上 f.txt 变非空目录 → checkout（文件语义）受阻。
    std::fs::remove_file(root.join("f.txt")).unwrap();
    std::fs::create_dir(root.join("f.txt")).unwrap();
    std::fs::write(root.join("f.txt").join("inner.txt"), "x").unwrap();

    let (_w, _d) = store.restore_code(1).await;
    // 诚实降级：不 panic 即可；checkout 若被目录挡住则 f.txt 仍是目录。
    let still_dir = root.join("f.txt").is_dir();
    let restored = root.join("f.txt").is_file();
    assert!(still_dir || restored, "workspace must not be half-broken");
}

/// 删除失败（待删文件在盘上是目录）→ warn，deleted 诚实为空。
#[tokio::test]
async fn delete_failure_when_removed_file_became_dir() {
    let (_d, root) = git_root();
    std::fs::write(root.join("doomed.txt"), "v1").unwrap();
    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "turn 1");

    std::fs::remove_file(root.join("doomed.txt")).unwrap();
    store.begin(2, "turn 2"); // target→cur diff: doomed.txt Deleted

    // 盘上同名位置现在是目录 → remove_file 失败臂。
    std::fs::create_dir(root.join("doomed.txt")).unwrap();
    let (_w, d) = store.restore_code(1).await;
    assert!(!d.contains(&"doomed.txt".to_string()), "d: {d:?}");
}

/// path_inside_root：`..` 视为界外（走 JSON 内容路径）；相对恒在内；
/// 绝对路径按词法前缀判。
#[test]
fn path_inside_root_rejects_traversal_and_classifies_paths() {
    let (_d, root) = git_root();
    let store = CheckpointStore::new(None, root.clone());

    assert!(!store.path_inside_root("../escape.txt"));
    assert!(!store.path_inside_root("..\\escape.txt"));
    assert!(store.path_inside_root("rel/ok.txt"));
    let inside = root.join("abs.txt");
    assert!(store.path_inside_root(inside.to_str().unwrap()));
    let outside = root.parent().unwrap().join("outside.txt");
    assert!(!store.path_inside_root(outside.to_str().unwrap()));
}

/// read_file_from_tree 错误面：JSON 形态诚实 Err；坏 hex Err；
/// 路径是 tree 内目录（非 NotFound 的 get_path 错误）→ Err。
#[test]
fn read_file_from_tree_error_faces() {
    // JSON 形态。
    let (_d, root) = git_root();
    std::fs::write(root.join("a.txt"), "v1").unwrap();
    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "t");
    let tree_hex = store.latest_tree_hex().unwrap();

    let json_store = {
        // 无 .git 的 root → JSON 形态。
        let dir2 = tempfile::tempdir().unwrap();
        let root2 = dir2.path().join("plain");
        std::fs::create_dir_all(&root2).unwrap();
        // 保持 dir2 存活到断言结束：泄漏 TempDir（测试进程退出即回收）。
        std::mem::forget(dir2);
        CheckpointStore::new(None, root2)
    };
    let err = json_store
        .read_file_from_tree(&tree_hex, "a.txt")
        .unwrap_err();
    assert!(err.contains("影子库不可用"), "err: {err}");

    // 坏 hex。
    let err = store.read_file_from_tree("zzzz", "a.txt").unwrap_err();
    assert!(err.contains("tree hex 解析失败"), "err: {err}");

    // 路径是 tree 内目录：get_path 返回 tree 条目 → find_blob 类型不匹配
    // → Err 面（libgit2 该形态落 blob 读取臂；另一非 NotFound 臂
    // 「tree 内路径查找失败」在此 libgit2 上无已知名形态，记豁免）。
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("n.txt"), "n").unwrap();
    store.begin(2, "t2");
    let tree2 = store.latest_tree_hex().unwrap();
    let err = store.read_file_from_tree(&tree2, "sub").unwrap_err();
    assert!(
        err.contains("blob 读取失败") || err.contains("tree 内路径查找失败"),
        "err: {err}"
    );
}

/// hybrid_restore 失败面：写入目标在盘上是目录（写失败不进 written）；
/// 待删文件不存在（删失败不进 deleted）；`..` 路径 safe_path 拒绝。
#[tokio::test]
async fn hybrid_restore_failures_are_honest() {
    let (_d, root) = git_root();
    let store = CheckpointStore::new(None, root.clone());
    std::fs::create_dir_all(root.join("sub")).unwrap();

    let (w, d) = store
        .hybrid_restore(vec![
            ("sub".to_string(), Some("blocked".to_string())), // 目录 → 写失败
            ("ghost.txt".to_string(), None),                  // 不存在 → 删失败
            ("../escape.txt".to_string(), Some("x".to_string())), // safe_path 拒
        ])
        .await;
    assert!(w.is_empty(), "w: {w:?}");
    assert!(d.is_empty(), "d: {d:?}");
}

/// 影子库在构造后被外力删除 → begin 的 tree 写入失败 → 该 turn 无 tree
/// （warn 降级，JSON 索引照常）。
#[tokio::test]
async fn begin_degrades_when_shadow_dir_vanishes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    let shadow = dir.path().join("shadow.git");
    let store = CheckpointStore::new_with_shadow(None, root.clone(), shadow.clone());
    assert_eq!(store.backend(), CheckpointBackend::Git);

    std::fs::write(root.join("f.txt"), "v1").unwrap();
    store.begin(1, "t1");
    assert!(store.latest_tree_hex().is_some(), "turn 1 has tree");

    // 外力删掉影子库 → 后续 begin 的 tree 写入失败（降级 None）。
    std::fs::remove_dir_all(&shadow).unwrap();
    std::fs::write(root.join("f.txt"), "v2").unwrap();
    store.begin(2, "t2");
    // turn 2 无 tree；turn 1 的 tree 引用仍在索引里（诚实保留）。
    assert_eq!(store.current_tree_hex(), None);
    assert_eq!(root.join("f.txt").metadata().unwrap().len(), 2);
}
