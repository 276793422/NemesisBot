//! M3（devtool-upgrade 阶段 5）：会话级 diff 的 checkpoint 基线原语测试。
//!
//! - `base_for_path`：首个声明过该文件的条目（git 形态=tree 锚；JSON
//!   形态=content 锚；无 `paths` 的老 JSON 条目按 `files` 命中——D2 前
//!   兼容）；未声明过的路径 → None。
//! - `read_file_from_tree`：按影子 tree 读单文件内容（NotFound → None，
//!   不误报错误）。

use super::*;
use crate::r#loop::{FileChange, FileChangeKind};

fn m3_git_root() -> (tempfile::TempDir, std::path::PathBuf) {
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

/// git 形态：base_for_path 取首个声明条目的 turn+tree；read_file_from_
/// tree 读出该时刻内容；不在 tree 的路径 → Ok(None)。
#[tokio::test]
async fn git_mode_base_for_path_and_read_file_from_tree() {
    let (_d, root) = m3_git_root();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "turn 1");
    store.snapshot(&modify("code.txt")).await;
    std::fs::write(root.join("code.txt"), "v2").unwrap();

    store.begin(2, "turn 2");
    store.snapshot(&modify("code.txt")).await;
    std::fs::write(root.join("code.txt"), "v3").unwrap();

    let base = store.base_for_path("code.txt").expect("已声明的文件有基线");
    assert_eq!(base.turn, 1, "首个声明条目");
    let tree = base.tree.expect("git 形态带 tree");

    let content = store.read_file_from_tree(&tree, "code.txt").unwrap();
    assert_eq!(content.as_deref(), Some("v1"), "基线=pre-edit 内容");

    let missing = store.read_file_from_tree(&tree, "nope.txt").unwrap();
    assert_eq!(missing, None, "tree 里没有 → None（新增文件语义）");

    assert!(store.base_for_path("untouched.txt").is_none());
}

/// JSON 形态：base 带 content 锚（Modify 的 pre-edit 快照）；未声明路径
/// → None。
#[tokio::test]
async fn json_mode_base_for_path_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(None, dir.path().to_path_buf());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    store.begin(1, "turn 1");
    store.snapshot(&modify("a.txt")).await;

    let base = store
        .base_for_path("a.txt")
        .expect("JSON 形态有 content 基线");
    assert_eq!(base.turn, 1);
    assert_eq!(base.tree, None, "JSON 形态无 tree");
    assert_eq!(
        base.content.as_deref(),
        Some("v1"),
        "Modify 的 pre-edit 快照"
    );

    assert!(store.base_for_path("b.txt").is_none());
}

/// BUG-20261103-347：`repo_relative_path` 形态归一单元钉死（树接口单一
/// 入口）——绝对剥根 / `\\?\` 前缀 / 相对原样 / 界外与 `..` → None /
/// 形态不齐 canonicalize 兜底。
#[tokio::test]
async fn repo_relative_path_form_normalization() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("code.txt"), "v").unwrap();
    std::fs::write(dir.path().join("outside.txt"), "o").unwrap();
    let store = CheckpointStore::new(None, root.clone());
    let root_str = root.to_string_lossy().to_string();

    // 绝对路径 → repo 相对（词法剥根）。
    assert_eq!(
        store.repo_relative_path(&format!("{root_str}\\code.txt")),
        Some("code.txt".to_string())
    );
    // 子目录 + 反斜杠归一为正斜杠。
    assert_eq!(
        store.repo_relative_path(&format!("{root_str}\\sub\\a.rs")),
        Some("sub/a.rs".to_string())
    );
    // `\\?\` verbose 前缀剥除。
    assert_eq!(
        store.repo_relative_path(&format!(r"\\?\{root_str}\code.txt")),
        Some("code.txt".to_string())
    );
    // 相对输入原样归一（前导 ./ 剥除）。
    assert_eq!(
        store.repo_relative_path("./code.txt"),
        Some("code.txt".to_string())
    );
    assert_eq!(
        store.repo_relative_path("sub\\a.rs"),
        Some("sub/a.rs".to_string())
    );
    // 根本身 / 空 → None。
    assert_eq!(store.repo_relative_path(&root_str), None);

    // 工作区外绝对路径 → None。
    let outside = dir.path().join("outside.txt");
    assert_eq!(store.repo_relative_path(&outside.to_string_lossy()), None);
    // `..` 保守拒绝（与 path_inside_root 同语义）。
    assert_eq!(store.repo_relative_path("a/../../b.txt"), None);
    assert_eq!(
        store.repo_relative_path(&format!("{root_str}\\..\\x")),
        None
    );

    // 词法形态不齐（大小写差异，Windows 大小写不敏感卷专属）：canonicalize
    // 双方兜底（文件在盘）。`\\?\` verbose 前缀已被 norm 词法剥除，走不到
    // canonicalize 支路——真正测兜底要用大小写错位。
    #[cfg(windows)]
    {
        let lower_root = CheckpointStore::new(None, PathBuf::from(root_str.to_lowercase()));
        assert_eq!(
            lower_root.repo_relative_path(&format!("{root_str}\\code.txt")),
            Some("code.txt".to_string()),
            "词法剥根失败（大小写）时 canonicalize 双方兜底"
        );
    }

    let _ = dir;
}

/// BUG-20261103-347：`base_for_path` 跨形态命中——声明侧绝对路径、查询
/// 侧相对（及反向），comparable 归一后命中同一基线。
#[tokio::test]
async fn base_for_path_cross_form_absolute_declared() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let abs = root.join("code.txt");
    std::fs::write(&abs, "v1").unwrap();

    let store = CheckpointStore::new(None, root);
    store.begin(1, "turn 1");
    store
        .snapshot(&FileChange {
            path: abs.to_string_lossy().to_string(),
            kind: FileChangeKind::Modify,
        })
        .await;

    // 相对查询命中绝对声明。
    let base = store.base_for_path("code.txt").expect("跨形态命中");
    assert_eq!(base.turn, 1);
    assert_eq!(base.content.as_deref(), Some("v1"));

    // `\\?\` 前缀查询同样命中。
    let verbose = format!(r"\\?\{}", abs.to_string_lossy());
    assert!(store.base_for_path(&verbose).is_some());

    // 未声明路径不误命中。
    assert!(store.base_for_path("other.txt").is_none());

    let _ = dir;
}
