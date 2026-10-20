//! P4（2026-10-17 交付门）：交付分支捕获测试。
//!
//! - `capture_delivery_branch`：git 形态 happy path（base 指定 / 空树
//!   基线）/ 同名 force 覆盖 / 危险分支名拒绝 / JSON 形态诚实 Err。
//! - `delivery_branch_patch`：从 ref 重算 patch 与 capture 时一致；
//!   base 缺省回落空树。
//! - `delivery_branch_refs`：列举 `refs/heads/nemesis/*`（字典序）。
//! - 捕获不打扰工作区：capture 后盘上文件原样（影子库操作不落工作区）。

use super::*;

/// 建一个带真 git 仓的临时 workspace（同 e3_tests 夹具语义）。
fn p4_git_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    (dir, root)
}

#[test]
fn capture_with_base_tree_produces_diff_and_patch() {
    let (_d, root) = p4_git_root();
    std::fs::write(root.join("a.txt"), "v1\n").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "任务开始");
    let base = store.current_tree_hex().expect("git 形态有实时 tree");

    // 任务执行：改旧文件 + 增新文件（含子目录）。
    std::fs::write(root.join("a.txt"), "v2\nmore\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("new.rs"), "fn main() {}\n").unwrap();

    let cap = store
        .capture_delivery_branch(Some(&base), "run-t1")
        .expect("capture");
    assert_eq!(cap.ref_name, "refs/heads/nemesis/run-t1");
    assert_eq!(cap.base_tree, base);
    assert_eq!(cap.files_changed, 2, "1 改 1 增");
    assert!(
        cap.insertions >= 2,
        "v2+new 至少 2 行增：{}",
        cap.insertions
    );
    assert!(cap.deletions >= 1, "v1 行计删：{}", cap.deletions);
    assert!(!cap.patch_truncated);
    assert!(cap.patch.contains("a.txt"), "patch 含旧文件");
    assert!(cap.patch.contains("src/new.rs"), "patch 含新文件（子目录）");
    assert!(cap.patch.contains("fn main()"), "patch 含新内容");
    assert_eq!(
        cap.tree,
        store.current_tree_hex().unwrap(),
        "捕获 tree=实时 tree"
    );

    // ref 可解到 commit，commit tree == capture.tree。
    let shadow = nemesis_path::logs_dir_in_workspace(&root).join("checkpoints.git");
    let repo = git2::Repository::open_ext(
        &shadow,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::path::Path::new("/"),
    )
    .unwrap();
    let oid = git2::Oid::from_str(&cap.commit).unwrap();
    let commit = repo.find_commit(oid).unwrap();
    assert_eq!(commit.tree_id().to_string(), cap.tree);
}

#[test]
fn capture_with_none_base_uses_empty_tree_all_added() {
    let (_d, root) = p4_git_root();
    std::fs::write(root.join("only.txt"), "hello\n").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    let cap = store
        .capture_delivery_branch(None, "run-t2")
        .expect("capture");
    // canonical 空树基线：全部文件计新增，deletions=0。
    assert_eq!(cap.files_changed, 1);
    assert_eq!(cap.deletions, 0);
    assert!(cap.insertions >= 1);
    assert!(cap.patch.contains("only.txt"));
}

#[test]
fn capture_same_branch_force_overwrites_ref() {
    let (_d, root) = p4_git_root();
    std::fs::write(root.join("f.txt"), "v1\n").unwrap();
    let store = CheckpointStore::new(None, root.clone());

    let c1 = store
        .capture_delivery_branch(None, "run-t3")
        .expect("capture 1");
    std::fs::write(root.join("f.txt"), "v2\n").unwrap();
    let c2 = store
        .capture_delivery_branch(None, "run-t3")
        .expect("capture 2 (重放覆盖)");

    assert_ne!(c1.commit, c2.commit, "重捕获 = 新 commit");
    assert_eq!(c1.ref_name, c2.ref_name);
    // ref 只剩一个且指向新 commit（force 覆盖不堆引用）。
    let refs = store.delivery_branch_refs().expect("refs");
    assert_eq!(refs, vec!["run-t3".to_string()]);
    let (patch, _) = store.delivery_branch_patch("run-t3", None).expect("patch");
    assert!(patch.contains("v2"), "重算 patch 反映最新内容");
}

#[test]
fn capture_rejects_unsafe_branch_names() {
    let (_d, root) = p4_git_root();
    let store = CheckpointStore::new(None, root);

    for bad in [
        "",
        "../escape",
        "-leading-dash",
        "a..b",
        "has space",
        "中文",
        &"x".repeat(81),
    ] {
        let err = store
            .capture_delivery_branch(None, bad)
            .expect_err(&format!("分支名 {bad:?} 必须拒绝"));
        assert!(
            !err.contains("影子库不可用"),
            "校验先于后端检查（JSON 形态下也能给出名字错误而非后端错误）: {err}"
        );
    }
}

#[test]
fn capture_on_json_backend_errs_honestly() {
    let dir = tempfile::tempdir().unwrap();
    // 无 .git 的工作区 → JSON 回落 → 诚实 Err（不为无 .git 工作区造
    // standalone 影子——可审分支只有在有真实 .git 可合入时才有行动意义）。
    let store = CheckpointStore::new(None, dir.path().to_path_buf());
    assert_eq!(store.backend(), CheckpointBackend::Json);
    let err = store
        .capture_delivery_branch(None, "run-t4")
        .expect_err("JSON 形态必须诚实报错");
    assert!(err.contains("影子库不可用"), "{err}");
    assert!(
        store.delivery_branch_patch("run-t4", None).is_err(),
        "patch 重算同面诚实"
    );
    assert!(store.delivery_branch_refs().is_err());
}

#[test]
fn delivery_branch_patch_recomputes_and_base_matters() {
    let (_d, root) = p4_git_root();
    std::fs::write(root.join("code.txt"), "v1\n").unwrap();
    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "begin");
    let base = store.current_tree_hex().unwrap();

    std::fs::write(root.join("code.txt"), "v2\n").unwrap();
    let cap = store
        .capture_delivery_branch(Some(&base), "run-t5")
        .expect("capture");

    // 显式 base：与 capture 时一致。
    let (p_base, trunc) = store
        .delivery_branch_patch("run-t5", Some(&base))
        .expect("patch with base");
    assert_eq!(p_base, cap.patch);
    assert_eq!(trunc, cap.patch_truncated);

    // 缺省 base（空树）：同文件内容但 delta 形态不同（全部计新增）。
    let (p_empty, _) = store
        .delivery_branch_patch("run-t5", None)
        .expect("patch empty");
    assert!(p_empty.contains("code.txt"));
    assert!(p_empty.contains("v2"));
    assert_ne!(p_empty, p_base, "基线不同 → patch 不同");
}

#[test]
fn delivery_branch_refs_lists_sorted() {
    let (_d, root) = p4_git_root();
    std::fs::write(root.join("f.txt"), "x\n").unwrap();
    let store = CheckpointStore::new(None, root);

    store.capture_delivery_branch(None, "run-b").unwrap();
    store.capture_delivery_branch(None, "run-a").unwrap();
    let refs = store.delivery_branch_refs().expect("refs");
    assert_eq!(refs, vec!["run-a".to_string(), "run-b".to_string()]);
}

#[test]
fn capture_does_not_disturb_workspace() {
    let (_d, root) = p4_git_root();
    std::fs::write(root.join("keep.txt"), "untouched\n").unwrap();
    let store = CheckpointStore::new(None, root.clone());

    store.capture_delivery_branch(None, "run-t6").unwrap();
    // 工作区文件原样；真实 .git 未被影子写入污染（影子在 logs/ 下）。
    assert_eq!(
        std::fs::read_to_string(root.join("keep.txt")).unwrap(),
        "untouched\n"
    );
    assert!(root.join(".git").is_dir());
    assert!(
        nemesis_path::logs_dir_in_workspace(&root)
            .join("checkpoints.git")
            .is_dir(),
        "影子库独立于真实 .git"
    );
}

// ---------------------------------------------------------------------------
// ②typed 证据块（诚实边界收口 2026-10-19）：delivery.files 清单
// ---------------------------------------------------------------------------

#[test]
fn capture_files_entries_cover_all_kinds_with_old_sha() {
    let (_d, root) = p4_git_root();
    std::fs::write(root.join("a.txt"), "v1\n").unwrap();
    std::fs::write(root.join("old.txt"), "doomed\n").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "任务开始");
    let base = store.current_tree_hex().unwrap();

    // 任务：改 a.txt + 增 src/new.rs + 删 old.txt（三形态各一）。
    std::fs::write(root.join("a.txt"), "v2\nmore\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("new.rs"), "fn main() {}\n").unwrap();
    std::fs::remove_file(root.join("old.txt")).unwrap();

    let cap = store
        .capture_delivery_branch(Some(&base), "run-tf1")
        .unwrap();
    // 计数含删除（3 形态全计），R5 清单同样含全部形态（删除入清单，
    // old_sha256 承载证据）。
    assert_eq!(cap.files_changed, 3);
    assert_eq!(cap.files.len(), 3, "add/modify/delete 三条目");
    assert!(!cap.files_truncated);

    let a = cap
        .files
        .iter()
        .find(|f| f.path == "a.txt")
        .expect("a.txt 在清单");
    let new = cap
        .files
        .iter()
        .find(|f| f.path == "src/new.rs")
        .expect("src/new.rs 在清单（正斜杠形态）");
    let old = cap
        .files
        .iter()
        .find(|f| f.path == "old.txt")
        .expect("old.txt（删除）在清单（R5：删除也是交付事实）");
    assert!(
        cap.files.iter().all(|f| !f.path.contains('\\')),
        "路径全正斜杠"
    );

    // sha256 = 交付 tree 的 blob 内容摘要（与磁盘新内容一致）。
    let expect_a = {
        let mut h = Sha256::new();
        h.update(b"v2\nmore\n");
        format!("{:x}", h.finalize())
    };
    let expect_old = {
        let mut h = Sha256::new();
        h.update(b"doomed\n");
        format!("{:x}", h.finalize())
    };
    let expect_new = {
        let mut h = Sha256::new();
        h.update(b"fn main() {}\n");
        format!("{:x}", h.finalize())
    };
    assert_eq!(a.kind, DeliveryFileKind::Modify);
    assert_eq!(a.sha256, expect_a);
    assert_eq!(
        a.old_sha256,
        {
            let mut h = Sha256::new();
            h.update(b"v1\n");
            format!("{:x}", h.finalize())
        },
        "modify 携带变更前摘要"
    );
    assert_eq!(new.kind, DeliveryFileKind::Add);
    assert_eq!(new.sha256, expect_new);
    assert!(new.old_sha256.is_empty(), "add 无变更前");
    assert_eq!(old.kind, DeliveryFileKind::Delete);
    assert!(old.sha256.is_empty(), "delete 无新内容可哈希");
    assert_eq!(
        old.old_sha256, expect_old,
        "delete 的 old_sha256 = 变更前 blob 摘要"
    );

    // wire 契约：kind 小写；delete 条目无 sha256 键、add 条目无 old_sha256 键。
    let json = serde_json::to_value(&cap).unwrap();
    let jold = json["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "old.txt")
        .unwrap();
    assert_eq!(jold["kind"], "delete");
    assert!(jold.get("sha256").is_none());
    assert_eq!(jold["old_sha256"].as_str().unwrap().len(), 64);
}

#[test]
fn capture_files_empty_serializes_as_empty_array() {
    let (_d, root) = p4_git_root();
    std::fs::write(root.join("same.txt"), "v1\n").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "任务开始");
    let base = store.current_tree_hex().unwrap();
    // 任务零改动。
    let cap = store
        .capture_delivery_branch(Some(&base), "run-tf2")
        .unwrap();
    assert!(cap.files.is_empty());
    assert!(!cap.files_truncated);

    // R5：files 恒序列化——空清单 = 「零文件变更」的派生事实（与「字段
    // 缺席」语义不同）；files_truncated 仍只 true 时序列化。
    let json = serde_json::to_value(&cap).unwrap();
    assert_eq!(
        json.get("files").and_then(|v| v.as_array()),
        Some(&Vec::new()),
        "空 files 序列化为空数组"
    );
    assert!(json.get("files_truncated").is_none());
}

#[test]
fn capture_files_truncates_at_max_with_honest_flag() {
    let (_d, root) = p4_git_root();
    std::fs::write(root.join("seed.txt"), "seed\n").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "任务开始");
    let base = store.current_tree_hex().unwrap();

    // FILES_MAX + 30 个新增文件 → 清单截断到上限 + 诚实注记位。
    for i in 0..FILES_MAX + 30 {
        let p = root.join(format!("gen/f{i:04}.txt"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("content {i}\n")).unwrap();
    }
    let cap = store
        .capture_delivery_branch(Some(&base), "run-tf3")
        .unwrap();
    assert_eq!(cap.files.len(), FILES_MAX, "清单截断到上限");
    assert!(cap.files_truncated, "超限必须带截断注记位");
    assert_eq!(cap.files_changed, FILES_MAX + 30, "计数不受清单截断影响");
}
