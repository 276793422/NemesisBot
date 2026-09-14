//! git_repo 测试（P4/E1+E4）。
//!
//! 覆盖：幂等 init/首 commit/.gitignore 边界、工作集 commit 变更检测、
//! HEAD 树导出（只含跟踪树）、三方合并矩阵（不同区域自动合/同行真冲突/
//! 二进制冲突/删除 vs 修改/空集 no-op/新文件 upsert/旧基线仍可合）、
//! 变更集路径自防御。

use super::*;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn temp_dir(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-gitrepo-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 建一个带首次 commit 的仓库：src/main.rs + ignored 投影件。
fn seeded_repo(name: &str) -> PathBuf {
    let root = temp_dir(name);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("project.json"), b"{\"projection\":true}").unwrap();
    std::fs::write(
        root.join(".gitignore"),
        "/project.json\n/timeline.jsonl\n/docs/\n/records/\n",
    )
    .unwrap();
    let fresh = ensure_repo(&root).unwrap();
    assert!(fresh, "seeded_repo 首次 ensure 必须新建");
    root
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, content).unwrap();
}

fn head(root: &Path) -> String {
    head_commit_hex(root).unwrap().expect("HEAD 必须存在")
}

/// 提交数（历史可查断言用）。
fn commit_count(root: &Path) -> usize {
    let repo = Repository::open(root).unwrap();
    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push_head().unwrap();
    revwalk.count()
}

// ---------------------------------------------------------------------------
// E1：ensure_repo + commit_worktree
// ---------------------------------------------------------------------------

#[test]
fn ensure_repo_is_idempotent_and_respects_gitignore() {
    let root = seeded_repo("idempotent");

    // 幂等：再次 ensure 不新建（首 commit 不重复）。
    assert!(!ensure_repo(&root).unwrap());
    assert_eq!(commit_count(&root), 1, "幂等 ensure 不得加 commit");

    // 首 commit 只含跟踪树：main.rs 在库，投影件 project.json 不在。
    let repo = Repository::open(&root).unwrap();
    let tree = head_commit(&repo).unwrap().tree().unwrap();
    assert!(tree.get_path(Path::new("src/main.rs")).is_ok());
    assert!(
        tree.get_path(Path::new("project.json")).is_err(),
        "投影文件必须被 .gitignore 排除（B6）"
    );
    assert!(tree.get_path(Path::new(".gitignore")).is_ok());
}

#[test]
fn ensure_repo_adopts_existing_files_as_initial_baseline() {
    // B5：用户选已有目录 → 现有文件作为初始基线录入（首 commit）。
    let root = temp_dir("existing");
    write(&root, "docs/notes.md", "# 用户已有笔记\n");
    ensure_repo(&root).unwrap();
    assert_eq!(
        read(&root, "docs/notes.md"),
        "# 用户已有笔记\n",
        "已有文件必须原样保留"
    );
    assert!(commit_count(&root) >= 1, "已有内容必须有首 commit");
}

#[test]
fn commit_worktree_detects_changes_and_skips_noop() {
    let root = seeded_repo("worktree");
    let base = head(&root);

    // 无变化 → no-op。
    assert!(commit_worktree(&root, "noop").unwrap().is_none());
    assert_eq!(head(&root), base, "no-op 不得移动 HEAD");

    // 修改跟踪文件 → commit。
    write(&root, "src/main.rs", "fn main() { println!(\"hi\"); }\n");
    let oid = commit_worktree(&root, "edit main")
        .unwrap()
        .expect("变更必须 commit");
    assert_ne!(oid, base);

    // 新增 ignored 投影件 → 不触发 commit（tree 无变化）。
    write(&root, "timeline.jsonl", "{\"ts\":1}\n");
    assert!(commit_worktree(&root, "ignored only").unwrap().is_none());
}

// ---------------------------------------------------------------------------
// E2：HEAD 树导出
// ---------------------------------------------------------------------------

#[test]
fn export_head_tree_writes_tracked_files_only() {
    let root = seeded_repo("export");
    // 未提交的脏文件不进导出（导出的是 HEAD 树，不是工作区）。
    write(&root, "src/dirty.rs", "// not committed\n");

    let dest = temp_dir("export-dest");
    let (count, bytes) = export_head_tree(&root, &dest).unwrap();
    assert_eq!(count, 2, "HEAD 树 = src/main.rs + .gitignore");
    assert!(bytes > 0);
    assert_eq!(read(&dest, "src/main.rs"), "fn main() {}\n");
    assert!(!dest.join("project.json").exists(), "投影件不入基线");
    assert!(!dest.join("src/dirty.rs").exists(), "未提交不导出");
}

// ---------------------------------------------------------------------------
// E4：三方合并矩阵
// ---------------------------------------------------------------------------

/// 建基线 commit → 返回 (root, baseline_oid)。
fn baseline_of(name: &str, content: &str) -> (PathBuf, String) {
    let root = seeded_repo(name);
    write(&root, "src/common.h", content);
    let oid = commit_worktree(&root, "add common.h").unwrap().unwrap();
    (root, oid)
}

#[test]
fn merge_different_regions_auto_combines() {
    // 经典 common.h 场景：基线 10 行；theirs 改头部、ours 改尾部 → 自动合。
    let base_content: String = (1..=10).map(|i| format!("line-{i}\n")).collect();
    let (root, baseline) = baseline_of("auto", &base_content);

    // theirs：worker 在基线上改头部（line-1 → line-1-edited）。
    let theirs_content = base_content.replacen("line-1\n", "line-1-edited\n", 1);
    // ours：master HEAD 前进——改尾部（line-10 → line-10-master）。
    write(
        &root,
        "src/common.h",
        &base_content.replacen("line-10\n", "line-10-master\n", 1),
    );
    commit_worktree(&root, "master edit tail").unwrap().unwrap();
    let before = head(&root);

    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: baseline.clone(),
            upserts: vec![ChangesetFile {
                path: "src/common.h".into(),
                content: theirs_content.into_bytes(),
                executable: false,
            }],
            deletions: vec![],
        },
    )
    .unwrap();
    let MergeOutcome::Merged { commit_oid } = outcome else {
        panic!("不同区域必须自动合并，实得 {outcome:?}");
    };
    assert_ne!(commit_oid, before, "合并必须产新 commit");

    // 工作区 = 合并结果：双方改动都在。
    let merged = read(&root, "src/common.h");
    assert!(
        merged.contains("line-1-edited"),
        "worker 改动必须在：{merged}"
    );
    assert!(
        merged.contains("line-10-master"),
        "master 改动必须在：{merged}"
    );
    // 历史可查：新 commit 的 parent = 合并前的 HEAD。
    assert!(commit_count(&root) >= 3);
}

#[test]
fn merge_same_line_conflict_detected_without_mutation() {
    let base_content: String = (1..=5).map(|i| format!("line-{i}\n")).collect();
    let (root, baseline) = baseline_of("conflict", &base_content);

    // 双方都改 line-3，内容不同 → 真冲突。
    write(
        &root,
        "src/common.h",
        &base_content.replacen("line-3\n", "master-three\n", 1),
    );
    commit_worktree(&root, "master edit line3")
        .unwrap()
        .unwrap();
    let before = head(&root);
    let worktree_before = read(&root, "src/common.h");

    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: baseline,
            upserts: vec![ChangesetFile {
                path: "src/common.h".into(),
                content: base_content
                    .replacen("line-3\n", "worker-three\n", 1)
                    .into_bytes(),
                executable: false,
            }],
            deletions: vec![],
        },
    )
    .unwrap();
    let MergeOutcome::Conflict { files } = outcome else {
        panic!("同行不同改必须冲突，实得 {outcome:?}");
    };
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/common.h");
    assert!(!files[0].binary, "文本冲突不得标二进制");

    // 冲突 = 仓库零改动：HEAD 不动、工作区不动。
    assert_eq!(head(&root), before, "冲突不得移动 HEAD");
    assert_eq!(
        read(&root, "src/common.h"),
        worktree_before,
        "冲突不得动工作区"
    );
}

#[test]
fn merge_binary_conflict_flagged() {
    // 二进制基线（含 NUL）；双方写不同字节 → 冲突 + binary 标记（E5）。
    let root = seeded_repo("binary");
    let blob_a: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x00, 0x01];
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("assets/logo.bin"), &blob_a).unwrap();
    let oid = commit_worktree(&root, "add binary").unwrap().unwrap();

    let ours: Vec<u8> = vec![0xDE, 0x00, 0xAD, 0x01];
    std::fs::write(root.join("assets/logo.bin"), &ours).unwrap();
    commit_worktree(&root, "master binary edit")
        .unwrap()
        .unwrap();

    let theirs: Vec<u8> = vec![0xBE, 0x02, 0xEF, 0x00];
    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: oid,
            upserts: vec![ChangesetFile {
                path: "assets/logo.bin".into(),
                content: theirs,
                executable: false,
            }],
            deletions: vec![],
        },
    )
    .unwrap();
    let MergeOutcome::Conflict { files } = outcome else {
        panic!("二进制双改必须冲突，实得 {outcome:?}");
    };
    assert_eq!(files.len(), 1);
    assert!(files[0].binary, "内容级判定必须标 binary（E5）");
}

#[test]
fn merge_delete_vs_modify_conflicts() {
    let (root, baseline) = baseline_of("delmod", "keep\n");

    // ours 改文件；theirs 删文件 → 删除 vs 修改冲突。
    write(&root, "src/common.h", "master-modified\n");
    commit_worktree(&root, "master modify").unwrap().unwrap();

    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: baseline,
            upserts: vec![],
            deletions: vec!["src/common.h".into()],
        },
    )
    .unwrap();
    assert!(
        matches!(outcome, MergeOutcome::Conflict { .. }),
        "删除 vs 修改必须冲突，实得 {outcome:?}"
    );
}

#[test]
fn merge_empty_changeset_is_noop() {
    let (root, baseline) = baseline_of("empty", "content\n");

    // 空变更集：theirs 树 == ours 树 → no-op（不产空 commit）。
    let before = head(&root);
    let count = commit_count(&root);
    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: baseline,
            upserts: vec![],
            deletions: vec![],
        },
    )
    .unwrap();
    let MergeOutcome::Merged { commit_oid } = outcome else {
        panic!("空变更集必须宽容通过，实得 {outcome:?}");
    };
    assert_eq!(commit_oid, before, "空变更集不得产新 commit");
    assert_eq!(commit_count(&root), count);
}

#[test]
fn merge_upserts_new_file_and_deletes_existing() {
    let (root, _baseline) = baseline_of("addel", "old\n");

    // theirs：新增 artifacts/out.txt + 删除基线里的 src/other.h。
    write(&root, "src/other.h", "to-be-deleted\n");
    commit_worktree(&root, "add other").unwrap().unwrap();
    let baseline2 = head(&root);

    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: baseline2,
            upserts: vec![ChangesetFile {
                path: "artifacts/out.txt".into(),
                content: b"worker output\n".to_vec(),
                executable: false,
            }],
            deletions: vec!["src/other.h".into()],
        },
    )
    .unwrap();
    assert!(
        matches!(outcome, MergeOutcome::Merged { .. }),
        "实得 {outcome:?}"
    );
    assert_eq!(read(&root, "artifacts/out.txt"), "worker output\n");
    assert!(!root.join("src/other.h").exists(), "删除必须同步到工作区");
}

#[test]
fn merge_against_stale_baseline_still_works() {
    // 基线落后两代（master 自己前进了两次）仍可三方合并——3-way 的本职。
    let base_content: String = (1..=6).map(|i| format!("line-{i}\n")).collect();
    let (root, baseline) = baseline_of("stale", &base_content);

    // master 前进两次（改 line-5、加 README）。
    write(
        &root,
        "src/common.h",
        &base_content.replacen("line-5\n", "line-5-a\n", 1),
    );
    commit_worktree(&root, "advance 1").unwrap().unwrap();
    write(&root, "README.md", "# project\n");
    commit_worktree(&root, "advance 2").unwrap().unwrap();

    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: baseline,
            upserts: vec![ChangesetFile {
                path: "src/common.h".into(),
                content: base_content
                    .replacen("line-2\n", "line-2-worker\n", 1)
                    .into_bytes(),
                executable: false,
            }],
            deletions: vec![],
        },
    )
    .unwrap();
    assert!(
        matches!(outcome, MergeOutcome::Merged { .. }),
        "实得 {outcome:?}"
    );
    let merged = read(&root, "src/common.h");
    assert!(merged.contains("line-2-worker"));
    assert!(merged.contains("line-5-a"));
    assert_eq!(read(&root, "README.md"), "# project\n");
}

// ---------------------------------------------------------------------------
// 路径自防御
// ---------------------------------------------------------------------------

#[test]
fn merge_rejects_malformed_paths() {
    let (root, baseline) = baseline_of("malformed", "x\n");
    let cases: Vec<ChangesetFile> = ["../escape.txt", "a\\b.txt", "C:/abs.txt", "doc~1.tmp"]
        .into_iter()
        .map(|p| ChangesetFile {
            path: p.into(),
            content: b"evil".to_vec(),
            executable: false,
        })
        .collect();
    for f in cases {
        let r = merge_changeset(
            &root,
            &MergeInput {
                baseline_commit: baseline.clone(),
                upserts: vec![f.clone()],
                deletions: vec![],
            },
        );
        assert!(r.is_err(), "畸形路径必须整体拒绝：{}", f.path);
    }
    // 删除清单同样过闸。
    let r = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: baseline,
            upserts: vec![],
            deletions: vec!["../escape".into()],
        },
    );
    assert!(r.is_err(), "畸形删除路径必须拒绝");
}

#[test]
fn looks_binary_matches_git_heuristic() {
    assert!(!looks_binary(b"plain text\nwith lines\n"));
    assert!(looks_binary(&[0x00, 0x01, 0x02]));
    // NUL 在前 8000 字节之外 → 文本。
    let mut late = vec![b'a'; 8100];
    late[8050] = 0x00;
    assert!(!looks_binary(&late));
}

// ---------------------------------------------------------------------------
// P5/F5：commit_resolution（AI 硬解落盘单一出口）
// ---------------------------------------------------------------------------

#[test]
fn commit_resolution_writes_overrides_and_commits() {
    let root = seeded_repo("commit-resolution");
    let base = head(&root);

    let oid = commit_resolution(
        &root,
        vec![(
            "src/main.rs".into(),
            b"fn main() { /* resolved */ }\n".to_vec(),
        )],
        "AI 硬解落盘",
    )
    .unwrap();
    assert_ne!(oid, base, "有实际落盘必须产生新 commit");
    assert_eq!(head(&root), oid, "HEAD 前移到落盘 commit");
    assert_eq!(
        read(&root, "src/main.rs"),
        "fn main() { /* resolved */ }\n",
        "override 必须落真盘（commit 后 reset --hard）"
    );
}

#[test]
fn commit_resolution_noop_returns_parent_without_new_commit() {
    // 树 == parent 树（override 内容与现值一致）→ no-op 返回 parent id，
    // 不产生空 commit（与 commit_worktree no-op 语义对齐）。
    let root = seeded_repo("commit-resolution-noop");
    let base = head(&root);
    let oid = commit_resolution(
        &root,
        vec![("src/main.rs".into(), b"fn main() {}\n".to_vec())],
        "no-op",
    )
    .unwrap();
    assert_eq!(oid, base, "no-op 返回 parent id");
    assert_eq!(head(&root), base, "no-op 不移动 HEAD");
}

#[test]
fn commit_resolution_rejects_traversal_paths() {
    let root = seeded_repo("commit-resolution-evil");
    assert!(commit_resolution(&root, vec![("../evil.txt".into(), b"x".to_vec())], "m").is_err());
    assert!(
        commit_resolution(&root, vec![("a\\b.txt".into(), b"x".to_vec())], "m").is_err(),
        "反斜杠路径必须拒绝"
    );
    assert!(commit_resolution(&root, vec![("".into(), b"x".to_vec())], "m").is_err());
}
