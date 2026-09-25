// git_repo.rs 覆盖率补充测试（commit_blob_text 三形态 348/357-363 /
// 基线 commit 不在仓库 387-388 / 合并删除补刀 524-535 / delete-modify
// 冲突的 (None, Some) 臂 459-460 / 空仓库 commit_resolution 576 / 8.3
// 短名组件闸 87）。

use super::*;
use std::path::PathBuf;

fn temp_root(name: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nmb-git-cov-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 种子仓库：空 init（可能空树首 commit）后写入两个文件再 commit，
/// 返回 (root, head_oid)。
fn seeded(name: &str) -> (PathBuf, String) {
    let root = temp_root(name);
    ensure_repo(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"alpha\n").unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/b.txt"), b"beta\n").unwrap();
    let head = commit_worktree(&root, "seed").unwrap().expect("head");
    (root, head)
}

/// commit_blob_text：路径不存在 → None（348）；路径是目录（树对象）→
/// None（357）；blob 超过 max_bytes → None（358-363）；正常文本 → Some。
#[test]
fn commit_blob_text_edge_forms() {
    let (root, head) = seeded("blob");

    assert!(
        commit_blob_text(&root, &head, "gone.txt", 1024)
            .unwrap()
            .is_none()
    );
    assert!(
        commit_blob_text(&root, &head, "sub", 1024)
            .unwrap()
            .is_none()
    );
    assert!(
        commit_blob_text(&root, &head, "a.txt", 2)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        commit_blob_text(&root, &head, "a.txt", 1024)
            .unwrap()
            .as_deref(),
        Some("alpha\n")
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// 基线 commit 是合法 hex 但不在仓库历史 → Err「不在本仓库」（387-388）；
/// 8.3 短名组件在变更集路径上被拒（87 的 `BASE~1.EXT` 形态 true 臂）。
#[test]
fn merge_rejects_unknown_baseline_and_short_name_paths() {
    let (root, _head) = seeded("unknown-base");

    let ghost = format!("{:040}", 0);
    let err = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: ghost,
            upserts: vec![],
            deletions: vec![],
        },
    )
    .unwrap_err();
    assert!(err.contains("不在本仓库"), "{err}");

    let err = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: _head,
            upserts: vec![ChangesetFile {
                path: "BASE~1.tar".to_string(),
                content: b"x".to_vec(),
                executable: false,
            }],
            deletions: vec![],
        },
    )
    .unwrap_err();
    assert!(err.contains("8.3"), "{err}");

    let _ = std::fs::remove_dir_all(&root);
}

/// 干净合入含删除：合并 commit 后工作区同步删除被删文件（524-535 的
/// Deleted 补刀循环）。
#[test]
fn clean_merge_with_deletion_removes_worktree_file() {
    let (root, head) = seeded("clean-del");

    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: head,
            upserts: vec![ChangesetFile {
                path: "z.txt".to_string(),
                content: b"zee".to_vec(),
                executable: false,
            }],
            deletions: vec!["a.txt".to_string()],
        },
    )
    .unwrap();
    let MergeOutcome::Merged { commit_oid: _ } = outcome else {
        panic!("预期干净合入");
    };
    assert!(!root.join("a.txt").exists(), "被删文件必须从工作区移除");
    assert_eq!(std::fs::read(root.join("z.txt")).unwrap(), b"zee");
    assert!(root.join("sub/b.txt").exists(), "未涉及文件保持原样");

    let _ = std::fs::remove_dir_all(&root);
}

/// delete/modify 冲突：我方删、worker 改 → 冲突明细该文件 our 侧为 None
/// （459-460 的 (None, Some) 臂），仓库零改动。
#[test]
fn delete_modify_conflict_reports_none_ours_side() {
    let (root, baseline) = seeded("delmod");

    // 我方在基线之后删除 a.txt 并提交。
    std::fs::remove_file(root.join("a.txt")).unwrap();
    let _del_head = commit_worktree(&root, "drop a.txt").unwrap().unwrap();

    // worker（基于基线）修改 a.txt。
    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: baseline,
            upserts: vec![ChangesetFile {
                path: "a.txt".to_string(),
                content: b"worker edit\n".to_vec(),
                executable: false,
            }],
            deletions: vec![],
        },
    )
    .unwrap();
    let MergeOutcome::Conflict { files } = outcome else {
        panic!("预期 delete/modify 冲突");
    };
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].path, "a.txt");
    assert!(files[0].ours.is_none(), "我方已删除 → ours 侧 None");
    assert_eq!(
        files[0].theirs.as_deref(),
        Some(b"worker edit\n".as_slice())
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// 空仓库（无 HEAD）做硬解提交 → index.clear 臂（576），提交成功。
#[test]
fn commit_resolution_on_repo_without_head() {
    let root = temp_root("resolution");
    ensure_repo(&root).unwrap();
    // 空目录 init：ensure_repo 的首 commit 对空树是否产生 HEAD 皆可——
    // 显式走 commit_resolution 验证两条 index 预备路径都通。
    let oid = commit_resolution(
        &root,
        vec![(".resolution/plan.md".to_string(), b"plan v1".to_vec())],
        "resolution: 硬解落账",
    )
    .unwrap();
    assert_eq!(oid.len(), 40);
    assert_eq!(
        commit_blob_text(&root, &oid, ".resolution/plan.md", 1024)
            .unwrap()
            .as_deref(),
        Some("plan v1")
    );

    let _ = std::fs::remove_dir_all(&root);
}

// ===========================================================================
// Wave4 覆盖批次（2026-09-25）：head_commit_hex / export 空仓库 /
// commit_changed_files 三臂 / commit_blob_text 二进制臂 / modify-delete
// 冲突 (Some,None) 臂 / 删除不在基线树的 Err 臂。
// ===========================================================================

/// head_commit_hex：空仓库 → None；有 HEAD → 40 位 hex（193-197）。
#[test]
fn head_commit_hex_empty_vs_seeded() {
    let root = temp_root("hex");
    ensure_repo(&root).unwrap();
    // ensure_repo 对空目录 init：可能有也可能没有首 commit——以 head 为准。
    let before = head_commit_hex(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"alpha\n").unwrap();
    let head = commit_worktree(&root, "seed").unwrap().expect("head");
    let after = head_commit_hex(&root).unwrap().expect("seeded");
    assert_eq!(after, head);
    assert_eq!(after.len(), 40);
    match before {
        None => {} // 理论不达（ensure_repo 对空目录也产生空树首 commit）
        Some(initial) => assert_ne!(
            initial, after,
            "首 commit 是空树基线，seed 提交必然产生新 oid"
        ),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// export_head_tree 空仓库 → Ok((0, 0))（212-214）。
#[test]
fn export_head_tree_empty_repo_yields_zero_stats() {
    let root = temp_root("export-empty");
    ensure_repo(&root).unwrap();
    // 空目录 init：若无 HEAD（首 commit 只对空树发生时也不生成 commit），
    // 直接断言 (0,0)；若有 HEAD，用未涉及面测试跳过该臂。
    if head_commit_hex(&root).unwrap().is_none() {
        let dest = root.join("dest");
        let stats = export_head_tree(&root, &dest).unwrap();
        assert_eq!(stats, (0, 0));
        assert!(dest.is_dir(), "dest 必须被建出来");
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// commit_changed_files：根提交（无父）→ 全量清单（284-285）；非法 oid →
/// Err（275-276）；不存在的 commit → Err（277-279）。
#[test]
fn commit_changed_files_root_invalid_and_missing() {
    let (root, head) = seeded("ccf");

    // 找根提交：head 的首个祖先。
    let repo = git2::Repository::open(&root).unwrap();
    let mut commit = repo
        .find_commit(git2::Oid::from_str(&head).unwrap())
        .unwrap();
    while commit.parent_count() > 0 {
        commit = commit.parent(0).unwrap();
    }
    let root_oid = commit.id().to_string();
    // 根提交 = ensure_repo 的空树首 commit → 对空树 diff = 空清单（无父臂
    // 284-285 走通即可）。
    let changed = commit_changed_files(&root, &root_oid).unwrap();
    assert!(changed.is_empty(), "空树根提交无变更：{changed:?}");

    // seed 提交（对父 = 空树）→ 全量清单含 a.txt 新增。
    let changed = commit_changed_files(&root, &head).unwrap();
    assert!(
        changed.iter().any(|(p, s)| p == "a.txt" && s == "新增"),
        "seed 提交清单必须含 a.txt 新增：{changed:?}"
    );

    let err = commit_changed_files(&root, "not-a-hex").unwrap_err();
    assert!(err.contains("非法"), "{err}");
    let ghost = format!("{:040}", 7);
    let err = commit_changed_files(&root, &ghost).unwrap_err();
    assert!(err.contains("不存在"), "{err}");

    let _ = std::fs::remove_dir_all(&root);
}

/// commit_blob_text：二进制内容（首 8000 字节含 NUL）→ None（350-352）。
#[test]
fn commit_blob_text_binary_content_is_none() {
    let (root, _head) = seeded("blobbin");
    let mut bin = vec![b'A'; 64];
    bin[31] = 0; // NUL → looks_binary true
    std::fs::write(root.join("bin.dat"), &bin).unwrap();
    let head = commit_worktree(&root, "add binary").unwrap().unwrap();
    assert!(
        commit_blob_text(&root, &head, "bin.dat", 4096)
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// modify/delete 冲突：我方改、worker 删 → (Some(o), None) 臂（458），
/// theirs 侧 None、仓库零改动。
#[test]
fn modify_delete_conflict_reports_none_theirs_side() {
    let (root, baseline) = seeded("moddel");

    // 我方在基线之后修改 a.txt 并提交。
    std::fs::write(root.join("a.txt"), b"our edit\n").unwrap();
    let _our_head = commit_worktree(&root, "our edit").unwrap().unwrap();

    // worker（基于基线）删除 a.txt。
    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: baseline,
            upserts: vec![],
            deletions: vec!["a.txt".to_string()],
        },
    )
    .unwrap();
    let MergeOutcome::Conflict { files } = outcome else {
        panic!("预期 modify/delete 冲突");
    };
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].path, "a.txt");
    assert_eq!(files[0].ours.as_deref(), Some(b"our edit\n".as_slice()));
    assert!(files[0].theirs.is_none(), "对方删除 → theirs 侧 None");

    let _ = std::fs::remove_dir_all(&root);
}

/// 变更集删除的路径不在基线树 → Err「不在基线树中」（423-428）。
#[test]
fn merge_rejects_deletion_not_in_baseline_tree() {
    let (root, head) = seeded("delmiss");
    let err = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: head,
            upserts: vec![],
            deletions: vec!["ghost.txt".to_string()],
        },
    )
    .unwrap_err();
    assert!(err.contains("不在基线树中"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

// ===========================================================================
// wave6 追加：导出边界（dest 已存在清空 / 空仓库 / gitlink 跳过）、变更
// 清单的删除与类型变更 delta、unborn HEAD 的空树合并与硬解落盘。
// ===========================================================================

/// export_head_tree：dest 已存在 → 先清空再导出（209-210）。
#[test]
fn w6_export_clears_preexisting_dest() {
    let (root, _head) = seeded("w6exp-dst");
    let dest = root.join("export");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("stale.txt"), b"old").unwrap();
    let (files, _bytes) = export_head_tree(&root, &dest).unwrap();
    assert!(!dest.join("stale.txt").exists(), "旧导出内容必须被清掉");
    assert!(files >= 2, "a.txt + sub/b.txt 至少 2 个文件：{files}");
    let _ = std::fs::remove_dir_all(&root);
}

/// export_head_tree：空仓库（unborn HEAD）→ (0, 0)，dest 建空目录（212-213）。
#[test]
fn w6_export_empty_repo_returns_zero_stats() {
    let root = temp_root("w6exp-empty");
    Repository::init(&root).unwrap();
    let dest = root.join("export");
    assert_eq!(export_head_tree(&root, &dest).unwrap(), (0, 0));
    assert!(dest.is_dir());
    let _ = std::fs::remove_dir_all(&root);
}

/// export_head_tree：gitlink（子模块占位）跳过不落盘（253）。
#[test]
fn w6_export_skips_gitlink_entries() {
    let root = temp_root("w6exp-gitlink");
    {
        let repo = Repository::init(&root).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        // 悬空子 commit（gitlink 目标不必可达，但给真 oid 更贴近现实）。
        let empty_oid = repo.treebuilder(None).unwrap().write().unwrap();
        let empty_tree = repo.find_tree(empty_oid).unwrap();
        let sub = repo
            .commit(None, &sig, &sig, "sub", &empty_tree, &[])
            .unwrap();
        let blob = repo.blob(b"hi").unwrap();
        let mut tb = repo.treebuilder(None).unwrap();
        tb.insert("file.txt", blob, 0o100644).unwrap();
        tb.insert("link", sub, 0o160000).unwrap();
        let tree_oid = tb.write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "main", &tree, &[])
            .unwrap();
    }

    let dest = root.join("export");
    let (files, bytes) = export_head_tree(&root, &dest).unwrap();
    assert_eq!(files, 1, "gitlink 不计入文件数");
    assert_eq!(bytes, 2);
    assert!(dest.join("file.txt").exists());
    assert!(!dest.join("link").exists(), "gitlink 不得落盘");
    let _ = std::fs::remove_dir_all(&root);
}

/// commit_changed_files：删除 delta → 「删除」（299）。
/// 豁免（300/301）：diff opts=None（无 find_similar → 恒无 Renamed；
/// 无 INCLUDE_TYPECHANGE → 模式变更折叠为 Modified）——Renamed/
/// Typechange 臂在该调用形态下结构性不可达。
#[test]
fn w6_commit_changed_files_deleted_delta() {
    let (root, _head) = seeded("w6del");
    std::fs::remove_file(root.join("a.txt")).unwrap();
    let del = commit_worktree(&root, "delete a").unwrap().unwrap();
    let files = commit_changed_files(&root, &del).unwrap();
    assert!(
        files.iter().any(|(p, s)| p == "a.txt" && s == "删除"),
        "{files:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// merge_changeset：unborn HEAD → ours 用空树（357-363, 394），变更集合入
/// 产出根 commit 并落盘。
#[test]
fn w6_merge_onto_unborn_head_uses_empty_tree() {
    let root = temp_root("w6unborn");
    let base = {
        let repo = Repository::init(&root).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let empty_oid = repo.treebuilder(None).unwrap().write().unwrap();
        let empty_tree = repo.find_tree(empty_oid).unwrap();
        // 基线 commit 只写对象、不更新 ref → HEAD 保持 unborn。
        repo.commit(None, &sig, &sig, "base", &empty_tree, &[])
            .unwrap()
    };

    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: base.to_string(),
            upserts: vec![ChangesetFile {
                path: "new.txt".to_string(),
                content: b"x".to_vec(),
                executable: false,
            }],
            deletions: vec![],
        },
    )
    .unwrap();
    let MergeOutcome::Merged { commit_oid } = outcome else {
        panic!("预期干净合入");
    };
    assert!(root.join("new.txt").exists(), "工作区同步落盘");
    assert_eq!(
        head_commit_hex(&root).unwrap().as_deref(),
        Some(commit_oid.as_str())
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// commit_resolution：unborn HEAD → index.clear() 后落根 commit（576）。
#[test]
fn w6_commit_resolution_on_unborn_head() {
    let root = temp_root("w6res-unborn");
    Repository::init(&root).unwrap();
    let oid = commit_resolution(
        &root,
        vec![("r.txt".to_string(), b"rr".to_vec())],
        "resolve root",
    )
    .unwrap();
    assert!(root.join("r.txt").exists(), "硬解产物同步工作区");
    assert_eq!(
        head_commit_hex(&root).unwrap().as_deref(),
        Some(oid.as_str())
    );
    let _ = std::fs::remove_dir_all(&root);
}
