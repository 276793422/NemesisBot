//! D2：checkpoint git 影子库双形态回归（D4 约束的验收测试）。
//!
//! - **git 形态**：fixture 是真 git 仓（`git2::Repository::init`），store 构造
//!   期自动选影子库后端；turn 边界写 tree → **shell/exec 副作用**（不经
//!   `snapshot()` 声明的纯 fs 写、新建、删除）也在 rewind 覆盖内（D2 的
//!   核心价值）；`logs/` 运行时属主目录不进影子树（影子 gitdir 自身就在
//!   `logs/` 下——自引用陷阱）；重启后从 JSON 索引加载仍可 restore。
//! - **JSON 形态**（D4）：无 `.git` 的仓走原 JSON 快照路径（存量 15 测试即
//!   回归主体），这里补 `backend()` 断言 + `.git` 文件形态（linked
//!   worktree）回落。
//! - **跨模式安全**：git 形态落盘的 JSON 索引，在 `.git` 消失后以 JSON 形态
//!   加载 → restore 对工作区内文件是诚实 no-op（`content=None` 绝不误删）。

use super::*;
use crate::r#loop::{FileChange, FileChangeKind};

/// 建一个带真 git 仓的临时 workspace（`ws/.git` 目录存在 → 影子库形态）。
fn git_root() -> (tempfile::TempDir, std::path::PathBuf) {
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

// ---------------------------------------------------------------------------
// 后端选择（D4 两形态）
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// libgit2 坑回归：空 pathspec + 回调 = 空指针段错误（2026-09-06 实测）
// ---------------------------------------------------------------------------

/// libgit2 `git_index_add_all` 空 pathspec + 回调组合 = STATUS_ACCESS_VIOLATION：
/// `git_pathspec__match` 对空 spec 直接 `return true` 但不填 `matched_pathspec`
/// （pathspec.c:209-211）→ 回调收到 NULL → git2-rs shim `CStr::from_ptr(NULL)`
/// 段错误。workaround = 非空全匹配 spec（`**`，递归含子目录）。此测试在
/// libgit2 升级时守卫该坑（若上游修复可考虑换回空 spec）。
#[test]
fn add_all_glob_pathspec_with_callback_is_safe() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    let shadow = dir.path().join("shadow.git");
    let mut opts = git2::RepositoryInitOptions::new();
    opts.bare(true).mkpath(true);
    let repo = git2::Repository::init_opts(&shadow, &opts).unwrap();
    repo.set_workdir(&root, false).unwrap();

    std::fs::write(root.join("tracked.txt"), "v1").unwrap();
    std::fs::create_dir_all(root.join("sub").join("deep")).unwrap();
    std::fs::write(root.join("sub").join("deep").join("n.txt"), "n").unwrap();
    std::fs::create_dir_all(root.join("logs")).unwrap();
    std::fs::write(root.join("logs").join("skip-me.log"), "s").unwrap();

    let mut idx = repo.index().unwrap();
    idx.add_all(
        ["**"],
        git2::IndexAddOption::DEFAULT,
        Some(&mut |p: &std::path::Path, _s: &[u8]| {
            if p.starts_with(std::path::Path::new("logs")) {
                1 // skip（运行时属主目录不进影子树）
            } else {
                0
            }
        }),
    )
    .unwrap();
    idx.write().unwrap();
    // skip 生效：logs/ 不进 index，其余（含深层嵌套）进了。
    assert!(idx.get_path(Path::new("tracked.txt"), 0).is_some());
    assert!(idx.get_path(Path::new("sub/deep/n.txt"), 0).is_some());
    assert!(idx.get_path(Path::new("logs/skip-me.log"), 0).is_none());
    // write_tree 正常出非空 tree。
    assert_ne!(
        idx.write_tree().unwrap().to_string(),
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904" // 空树
    );
}

#[test]
fn backend_selection_git_vs_json() {
    let (_d, root) = git_root();
    let store = CheckpointStore::new(None, root);
    assert_eq!(store.backend(), CheckpointBackend::Git);

    let dir = tempfile::tempdir().unwrap();
    let store2 = CheckpointStore::new(None, dir.path().to_path_buf());
    assert_eq!(store2.backend(), CheckpointBackend::Json);
}

#[test]
fn git_file_form_falls_back_to_json() {
    // linked worktree / submodule 的 .git 是文件不是目录 → JSON 回落。
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".git"), "gitdir: /elsewhere").unwrap();
    let store = CheckpointStore::new(None, root);
    assert_eq!(store.backend(), CheckpointBackend::Json);
}

// ---------------------------------------------------------------------------
// git 形态：rewind 覆盖 shell 副作用（D2 核心验收）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn git_mode_rewinds_edits_and_shell_products() {
    let (_d, root) = git_root();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    assert_eq!(store.backend(), CheckpointBackend::Git);

    store.begin(1, "turn 1");
    // shell 副作用：不经 snapshot 声明的纯 fs 改写 + 新建文件
    std::fs::write(root.join("code.txt"), "v1-edited-by-shell").unwrap();
    std::fs::write(root.join("shell-product.txt"), "made by exec").unwrap();

    store.begin(2, "turn 2");
    // turn 2 里的声明式工具改动（走 snapshot）+ 又一次 shell 写
    std::fs::write(root.join("code.txt"), "v2-final").unwrap();
    store.snapshot(&modify("code.txt")).await;

    // rewind 到 turn 1 开始时：code.txt=v1，shell 产物消失
    let (written, deleted) = store.restore_code(1).await;
    assert!(
        written.contains(&"code.txt".to_string()),
        "written: {written:?}"
    );
    assert!(
        deleted.contains(&"shell-product.txt".to_string()),
        "deleted: {deleted:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("code.txt")).unwrap(),
        "v1"
    );
    assert!(!root.join("shell-product.txt").exists());
}

#[tokio::test]
async fn git_mode_restores_shell_deleted_file() {
    let (_d, root) = git_root();
    std::fs::write(root.join("doomed.txt"), "precious").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "turn 1");
    std::fs::remove_file(root.join("doomed.txt")).unwrap(); // shell rm

    store.begin(2, "turn 2");
    let (written, _) = store.restore_code(1).await;
    assert!(
        written.contains(&"doomed.txt".to_string()),
        "written: {written:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("doomed.txt")).unwrap(),
        "precious"
    );
}

// ---------------------------------------------------------------------------
// git 形态：logs/ 自引用陷阱 + 落盘纪律
// ---------------------------------------------------------------------------

#[tokio::test]
async fn git_mode_excludes_runtime_owned_dirs_from_tree() {
    let (_d, root) = git_root();
    let store = CheckpointStore::new(None, root.clone());
    assert!(store.backend() == CheckpointBackend::Git);

    // logs/ 下放运行时文件（影子 gitdir 自身就在 logs/checkpoints.git）。
    std::fs::create_dir_all(root.join("logs")).unwrap();
    std::fs::write(root.join("logs").join("junk.log"), "x").unwrap();

    store.begin(1, "turn 1");
    std::fs::write(root.join("real.txt"), "v").unwrap();
    store.begin(2, "turn 2");

    // tree 写入成功即证明 add_all 正确跳过 logs/**（否则会把影子 gitdir
    // 自身加进 index——自引用）。rewind 不触碰 logs/。
    let (w, d) = store.restore_code(1).await;
    assert!(!w.iter().any(|p| p.starts_with("logs")), "w: {w:?}");
    assert!(!d.iter().any(|p| p.starts_with("logs")), "d: {d:?}");
    assert!(root.join("logs").join("junk.log").exists());
    // real.txt 是 turn 1 之后新增的 → 被回滚删除（shell 产物覆盖的又一例）。
    assert!(!root.join("real.txt").exists());
}

#[tokio::test]
async fn git_mode_empty_turns_do_not_persist_but_tree_turns_do() {
    let (_d, root) = git_root();
    let cp_dir = root.join("logs").join("checkpoints");
    let store = CheckpointStore::new(Some(cp_dir.clone()), root.clone());

    // 首 turn：tree 首次已知 → 落盘（restore 的基线）。
    store.begin(1, "turn 1");
    assert!(cp_dir.join("turn-1.json").exists());

    // tree 与上一 turn 一致 → 空 turn 不落盘（落盘纪律：翻页不留壳）。
    store.begin(2, "turn 2 (no changes)");
    assert!(!cp_dir.join("turn-2.json").exists());

    // 改动后再 begin → 落盘。
    std::fs::write(root.join("f.txt"), "x").unwrap();
    store.begin(3, "turn 3 (after change)");
    assert!(cp_dir.join("turn-3.json").exists());

    store.begin(4, "turn 4 (no changes again)");
    assert!(!cp_dir.join("turn-4.json").exists());
}

// ---------------------------------------------------------------------------
// git 形态：持久化 / picker 元数据
// ---------------------------------------------------------------------------

#[tokio::test]
async fn git_mode_persistence_reloads_and_restores() {
    let (_d, root) = git_root();
    let cp_dir = root.join("logs").join("checkpoints");
    std::fs::write(root.join("doc.txt"), "orig").unwrap();

    {
        let store = CheckpointStore::new(Some(cp_dir.clone()), root.clone());
        store.begin(1, "persisted");
        std::fs::write(root.join("doc.txt"), "changed").unwrap();
    }
    // 新实例（模拟 gateway 重启）：从磁盘 JSON 索引加载 turn-1 → restore。
    let store2 = CheckpointStore::new(Some(cp_dir), root.clone());
    let metas = store2.list_meta();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].turn, 1);
    let (written, _) = store2.restore_code(1).await;
    assert!(
        written.contains(&"doc.txt".to_string()),
        "written: {written:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("doc.txt")).unwrap(),
        "orig"
    );
}

#[tokio::test]
async fn git_mode_list_meta_reports_declared_paths() {
    let (_d, root) = git_root();
    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "turn 1");
    store.snapshot(&modify("a.txt")).await;

    let metas = store.list_meta();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].paths, vec!["a.txt".to_string()]);
}

// ---------------------------------------------------------------------------
// git 形态：hybrid（工作区外声明路径仍走内容快照）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn git_mode_hybrid_restores_out_of_workspace_paths() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    let outside_dir = d.path().join("outside");
    std::fs::create_dir_all(&outside_dir).unwrap();
    let outside = outside_dir.join("o.txt");
    std::fs::write(&outside, "outside-orig").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "hybrid");
    // 声明式工具改工作区外文件（restrict 关闭时合法）→ snapshot 读全文。
    store
        .snapshot(&FileChange {
            path: outside.to_string_lossy().to_string(),
            kind: FileChangeKind::Modify,
        })
        .await;
    std::fs::write(&outside, "clobbered").unwrap();

    let (w, _) = store.restore_code(1).await;
    assert!(w.iter().any(|p| p.ends_with("o.txt")), "w: {w:?}");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside-orig");
}

// ---------------------------------------------------------------------------
// 跨模式安全（.git 消失 → JSON 形态加载 git 时代的索引）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cross_mode_git_index_loaded_as_json_never_false_deletes() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    let cp_dir = root.join("logs").join("checkpoints");
    let real_git = root.join(".git");
    let moved_git = d.path().join("git-away");

    {
        let store = CheckpointStore::new(Some(cp_dir.clone()), root.clone());
        store.begin(1, "git era");
        std::fs::write(root.join("tracked.txt"), "v1").unwrap(); // shell 写
        store.begin(2, "turn 2"); // tree 变化 → turn-1 落盘
        // git 形态下 in-root 快照 content=None（内容在 tree 里）
        store.snapshot(&modify("tracked.txt")).await;
    }

    // .git 消失（仓被移动/删除）→ 新实例回落 JSON 形态。
    std::fs::rename(&real_git, &moved_git).unwrap();
    let store2 = CheckpointStore::new(Some(cp_dir), root.clone());
    assert_eq!(store2.backend(), CheckpointBackend::Json);

    let (w, dd) = store2.restore_code(2).await;
    assert!(
        w.is_empty() && dd.is_empty(),
        "cross-mode restore must be honest no-op: {w:?} {dd:?}"
    );
    assert!(
        root.join("tracked.txt").exists(),
        "in-root file must NOT be false-deleted (content=None is tree semantics, not delete)"
    );
}
