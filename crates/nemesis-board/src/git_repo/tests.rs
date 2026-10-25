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

/// NB-54 复原（压测⑤）：变更集带工作区前缀 `board-projects/<项目名>/…` 时
/// 合并落点必须是项目根相对（剥前缀），不得双重嵌套。
#[test]
fn merge_strips_workspace_prefix_from_changeset_paths() {
    // 档案目录名与真实布局同形：<tmp>/board-projects/压测C1-周报模板。
    let projects_root = temp_dir("wsproj");
    let root = projects_root
        .join(crate::archive::BOARD_PROJECTS_DIR)
        .join("压测C1-周报模板");
    std::fs::create_dir_all(&root).unwrap();
    write(&root, "docs/plan.md", "# plan\n");
    ensure_repo(&root).unwrap();
    let baseline = head(&root);

    // worker 误按 master 工作区视角申报路径（双重嵌套现场）。
    let outcome = merge_changeset(
        &root,
        &MergeInput {
            baseline_commit: baseline,
            upserts: vec![ChangesetFile {
                path: "board-projects/压测C1-周报模板/周报模板.md".into(),
                content: "# 周报\n".as_bytes().to_vec(),
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
    // 正确落点 = 项目根下；双重嵌套路径不得存在。
    assert_eq!(read(&root, "周报模板.md"), "# 周报\n");
    assert!(
        !root.join("board-projects").exists(),
        "不得双重嵌套 board-projects/"
    );
}

/// 归一化纯函数矩阵：只剥 `board-projects/<档案目录名>/` 完整前缀，
/// 其余路径（正确的项目相对 / 同名子目录深路径 / 目录名自身）逐字节不动。
#[test]
fn normalize_repo_path_matrix() {
    let root = Path::new("/ws/board-projects/demo");
    let strip = |p: &str| normalize_repo_path(root, p);
    assert_eq!(strip("board-projects/demo/周报模板.md"), "周报模板.md");
    assert_eq!(strip("board-projects/demo/docs/plan.md"), "docs/plan.md");
    // 正确项目相对路径不动（NB-62 对照组）。
    assert_eq!(strip("docs/plan.md"), "docs/plan.md");
    // 同首段但项目名不同（别的项目的路径）不动——宁嵌套不误剥。
    assert_eq!(
        strip("board-projects/other/file.md"),
        "board-projects/other/file.md"
    );
    // 项目档案目录名自身（无后段）不动。
    assert_eq!(strip("board-projects/demo"), "board-projects/demo");
    // 深层同名嵌套（前缀已剥过一次的幂等性）只剥一层。
    assert_eq!(
        strip("board-projects/demo/board-projects/demo/x.md"),
        "board-projects/demo/x.md"
    );
    // 规则是纯词法的（首两段 == board-projects + 档案目录名）：root 在
    // board-projects 之外时同形前缀照剥——worker LLM 幻觉标准布局写的
    // 路径，剥回项目根相对仍是正确恢复。
    let ext = Path::new("/data/myproj");
    assert_eq!(
        normalize_repo_path(ext, "board-projects/myproj/a.md"),
        "a.md"
    );
    // 但第二段不等档案目录名（别的项目路径）不剥。
    assert_eq!(
        normalize_repo_path(ext, "board-projects/other/a.md"),
        "board-projects/other/a.md"
    );
}

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

// ---------------------------------------------------------------------------
// 评审证据：commit_changed_files（F-U3-1 根修）
// ---------------------------------------------------------------------------

#[test]
fn commit_changed_files_lists_merge_diff_vs_parent() {
    let root = seeded_repo("changed-files");
    let base = head(&root);
    // 变更集合并：新增 calculator.py + 修改 src/main.rs（三方合并路径）。
    let input = MergeInput {
        baseline_commit: base.clone(),
        upserts: vec![
            ChangesetFile {
                path: "calculator.py".into(),
                content: b"print('calc')\n".to_vec(),
                executable: false,
            },
            ChangesetFile {
                path: "src/main.rs".into(),
                content: b"fn main() { println!(1); }\n".to_vec(),
                executable: false,
            },
        ],
        deletions: vec![],
    };
    let merged = match merge_changeset(&root, &input).unwrap() {
        MergeOutcome::Merged { commit_oid } => commit_oid,
        MergeOutcome::Conflict { files } => panic!("不应冲突: {files:?}"),
    };
    let mut files = commit_changed_files(&root, &merged).unwrap();
    files.sort();
    assert_eq!(
        files,
        vec![
            ("calculator.py".to_string(), "新增".to_string()),
            ("src/main.rs".to_string(), "修改".to_string()),
        ],
        "合并 commit 的变更清单 = 相对父提交的 diff"
    );
    // 基线 commit 本身（根提交）= 全量清单（初始种子文件）。
    let root_files = commit_changed_files(&root, &base).unwrap();
    assert!(
        root_files
            .iter()
            .any(|(p, s)| p == "src/main.rs" && s == "新增"),
        "根提交对空树 diff = 全量新增：{root_files:?}"
    );
}

#[test]
fn commit_changed_files_rejects_bad_oid_and_missing_commit() {
    let root = seeded_repo("changed-files-bad-oid");
    assert!(
        commit_changed_files(&root, "not-an-oid").is_err(),
        "非法 oid 诚实报错"
    );
    assert!(
        commit_changed_files(&root, &"0".repeat(40)).is_err(),
        "不存在的 commit 诚实报错"
    );
}

// ---------------------------------------------------------------------------
// F-U3-7：commit_blob_text（评审证据段内容注入的数据源）
// ---------------------------------------------------------------------------

#[test]
fn commit_blob_text_reads_text_and_degrades_honestly() {
    let root = seeded_repo("blob-text");
    write(&root, "report.md", "# 复核报告\n20 passed");
    write(&root, "src/bin.dat", "\u{0}\u{1}\u{2}");
    commit_worktree(&root, "add files for blob read").unwrap();
    let oid = head(&root);

    // 文本文件 → 内容原文。
    let text = commit_blob_text(&root, &oid, "report.md", 64 * 1024).unwrap();
    assert_eq!(text.as_deref(), Some("# 复核报告\n20 passed"));

    // 二进制 → Ok(None) 诚实降级。
    let bin = commit_blob_text(&root, &oid, "src/bin.dat", 64 * 1024).unwrap();
    assert!(bin.is_none(), "二进制 blob 应返回 None");

    // 超限 → Ok(None)。
    let big = commit_blob_text(&root, &oid, "report.md", 4).unwrap();
    assert!(big.is_none(), "超限 blob 应返回 None");

    // 路径不存在 → Ok(None)。
    let missing = commit_blob_text(&root, &oid, "nope/none.txt", 64 * 1024).unwrap();
    assert!(missing.is_none(), "缺失路径应返回 None");

    // 非法 commit oid → Err。
    assert!(commit_blob_text(&root, "deadbeef", "report.md", 1024).is_err());

    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// P1 外循环：clone_repo 幂等 + 钉 HEAD；push_branch 快进与非快进
// ---------------------------------------------------------------------------

#[test]
fn clone_repo_local_path_lands_head_and_content() {
    let origin = seeded_repo("clone-origin");
    let want_head = head_commit_hex(&origin).unwrap().unwrap();
    let dest = temp_dir("clone-dest");
    // 本地路径克隆（file 传输恒可用，不依赖 TLS）。
    let got = clone_repo(origin.to_str().unwrap(), &dest, None).unwrap();
    assert_eq!(got, want_head, "克隆 HEAD 必须等于源 HEAD（钉 base SHA）");
    assert!(dest.join("src/main.rs").exists(), "工作集内容必须落地");
    // autocrlf 钉死（字节精确契约）。
    let repo = Repository::open(&dest).unwrap();
    let autocrlf = repo.config().unwrap().get_bool("core.autocrlf").unwrap();
    assert!(!autocrlf);
}

#[test]
fn clone_repo_idempotent_reuse_keeps_existing_head() {
    let origin = seeded_repo("clone-reuse-origin");
    let dest = temp_dir("clone-reuse-dest");
    let first = clone_repo(origin.to_str().unwrap(), &dest, None).unwrap();
    // 副本里继续产出一笔 commit（模拟 worker 合并历史）。
    std::fs::write(dest.join("src/extra.rs"), "pub fn extra() {}\n").unwrap();
    let merged = commit_worktree(&dest, "local change").unwrap();
    assert!(merged.is_some());
    // 幂等重入：不 fetch 不重置，返回**副本当前** HEAD（不是远端旧 HEAD）。
    let again = clone_repo(origin.to_str().unwrap(), &dest, None).unwrap();
    assert_ne!(again, first, "重入必须返回副本自身 HEAD");
    assert!(dest.join("src/extra.rs").exists(), "历史不得被外部重写");
}

#[test]
fn push_branch_fast_forward_and_local_branch_creation() {
    let origin = seeded_repo("push-origin");
    // libgit2 本地传输不支持 push 到非 bare 仓库（生产推 http 远端不受此限
    // ）——测试用 bare 镜像当推送靶。
    let bare = temp_dir("push-bare");
    {
        let mut builder = git2::build::RepoBuilder::new();
        builder.bare(true);
        builder.clone(origin.to_str().unwrap(), &bare).unwrap();
    }
    let dest = temp_dir("push-dest");
    clone_repo(origin.to_str().unwrap(), &dest, None).unwrap();
    {
        let repo = Repository::open(&dest).unwrap();
        repo.remote("bare", bare.to_str().unwrap()).unwrap();
    }
    std::fs::write(dest.join("src/delivery.rs"), "pub fn delivery() {}\n").unwrap();
    // 先落本地分支再推（交付分支形态：refs/heads/nemesis/task-N）。
    let repo = Repository::open(&dest).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("nemesis/task-1", &head, false).unwrap();
    push_branch(
        &dest,
        "nemesis/task-1",
        "bare",
        "refs/heads/nemesis/task-1",
        None,
    )
    .unwrap();
    // bare 靶必须出现同名分支且指向交付 commit。
    let bare_repo = Repository::open(&bare).unwrap();
    let remote_branch = bare_repo
        .find_branch("nemesis/task-1", git2::BranchType::Local)
        .unwrap();
    let delivered = repo
        .find_branch("nemesis/task-1", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap();
    assert_eq!(
        remote_branch.get().peel_to_commit().unwrap().id(),
        delivered.id(),
        "远端分支必须指向交付 commit"
    );
}

#[test]
fn push_branch_missing_local_branch_is_honest_error() {
    let origin = seeded_repo("push-missing-origin");
    let bare = temp_dir("push-missing-bare");
    {
        let mut builder = git2::build::RepoBuilder::new();
        builder.bare(true);
        builder.clone(origin.to_str().unwrap(), &bare).unwrap();
    }
    let dest = temp_dir("push-missing-dest");
    clone_repo(origin.to_str().unwrap(), &dest, None).unwrap();
    {
        let repo = Repository::open(&dest).unwrap();
        repo.remote("bare", bare.to_str().unwrap()).unwrap();
    }
    let err = push_branch(
        &dest,
        "nemesis/ghost",
        "bare",
        "refs/heads/nemesis/ghost",
        None,
    )
    .unwrap_err();
    // libgit2 文案：`src refspec ... does not match any`——诚实透传即可，
    // 断言只钉「失败 + 原因可读」。
    assert!(err.contains("推送"), "{err}");
    assert!(!err.trim().ends_with("失败"), "{err}");
}

// ---------------------------------------------------------------------------
// P3 外循环：apply_patch_new_branch（patch → 副本内交付分支，工作区回原位）
// ---------------------------------------------------------------------------

/// 从「已修改的工作区」产 unified diff 文本（复刻 worker 侧交付捕获形态：
/// nemesis-agent checkpoint::diff_patch 同款 print 配方——context/add/del
/// 三类必须手动补 origin 前缀字节，否则 patch 无法被 Diff::from_buffer 回读）。
fn workdir_patch(root: &Path) -> Vec<u8> {
    let repo = Repository::open(root).unwrap();
    let head_tree = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap();
    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&head_tree), None)
        .unwrap();
    let mut buf = Vec::new();
    diff.print(git2::DiffFormat::Patch, |_d, _h, line| {
        match line.origin() {
            '+' | '-' | ' ' => buf.push(line.origin() as u8),
            _ => {}
        }
        buf.extend_from_slice(line.content());
        true
    })
    .unwrap();
    buf
}

#[test]
fn apply_patch_new_branch_lands_commit_and_restores_worktree() {
    let root = seeded_repo("apply-happy");
    let base_sha = head(&root);
    // 造交付变更 → 产 patch → 还原工作区（副本基线形态）。
    write(&root, "src/main.rs", "fn main() {\n    delivered();\n}\n");
    let patch = workdir_patch(&root);
    assert!(!patch.is_empty());
    reset_worktree_to_head(&root);

    let branch = "nemesis/task-42";
    let commit_hex =
        apply_patch_new_branch(&root, branch, &patch, "NemesisBot task 42: 交付").unwrap();

    // base HEAD 未动。
    assert_eq!(
        head(&root),
        base_sha,
        "当前分支 HEAD 不得被交付 commit 移动"
    );
    // 交付分支在场、父 = base、提交信息原文。
    let repo = Repository::open(&root).unwrap();
    let delivered = repo
        .find_branch(branch, git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap();
    assert_eq!(
        delivered.parent(0).unwrap().id().to_string(),
        base_sha,
        "交付 commit 必须以基线 HEAD 为父"
    );
    assert_eq!(delivered.message().unwrap(), "NemesisBot task 42: 交付");
    // 工作区回原位：当前分支仍是原分支、交付变更不在工作区。
    assert_ne!(repo.head().unwrap().shorthand().unwrap(), branch);
    let on_head = std::fs::read_to_string(root.join("src/main.rs")).unwrap();
    assert_eq!(on_head, "fn main() {}\n", "工作区必须 reset 回基线形态");
    // 交付分支的 blob 内容是 patch 后形态。
    let entry = delivered
        .tree()
        .unwrap()
        .get_path(Path::new("src/main.rs"))
        .unwrap();
    let blob = repo.find_blob(entry.id()).unwrap();
    assert_eq!(
        std::str::from_utf8(blob.content()).unwrap(),
        "fn main() {\n    delivered();\n}\n"
    );
    assert_ne!(commit_hex, base_sha);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn apply_patch_new_branch_rejects_dirty_worktree() {
    let root = seeded_repo("apply-dirty");
    let patch = b"diff --git a/x b/x\n".to_vec();
    // 未跟踪文件 = 脏（副本被中途触碰 → 拒绝，防糊到漂移现场）。
    write(&root, "stray.txt", "mid-flight touch\n");
    let err = apply_patch_new_branch(&root, "nemesis/task-d", &patch, "m").unwrap_err();
    assert!(err.contains("未提交/未跟踪改动"), "{err}");
    assert!(
        !repo_find_branch(&root, "nemesis/task-d"),
        "失败路径必须零残留（分支不落）"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn apply_patch_new_branch_rejects_existing_branch() {
    let root = seeded_repo("apply-exists");
    write(&root, "src/main.rs", "fn main() { a }\n");
    let patch = workdir_patch(&root);
    reset_worktree_to_head(&root);
    apply_patch_new_branch(&root, "nemesis/task-x", &patch, "m").unwrap();
    // 幂等撞 git 层：分支已存在 = Err（诚实人工核查，不静默复用）。
    let err = apply_patch_new_branch(&root, "nemesis/task-x", &patch, "m").unwrap_err();
    assert!(err.contains("已存在"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn apply_patch_new_branch_empty_patch_is_honest_error_no_residue() {
    let root = seeded_repo("apply-empty");
    // 空 diff = 交付 commit 树与基线一致 = 空交付 → Err 且分支删除。
    let err = apply_patch_new_branch(&root, "nemesis/task-e", b"", "m").unwrap_err();
    assert!(err.contains("空") || err.contains("无变更"), "{err}");
    assert!(!repo_find_branch(&root, "nemesis/task-e"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn apply_patch_new_branch_then_push_full_chain() {
    let origin = seeded_repo("apply-chain-origin");
    // bare 推送靶（libgit2 本地传输不支持推非 bare，同 push_branch 测试纪律）。
    let bare = temp_dir("apply-chain-bare");
    {
        let mut builder = git2::build::RepoBuilder::new();
        builder.bare(true);
        builder.clone(origin.to_str().unwrap(), &bare).unwrap();
    }
    let copy = temp_dir("apply-chain-copy");
    clone_repo(origin.to_str().unwrap(), &copy, None).unwrap();
    {
        // clone_repo 已带 origin（指向源仓）——只改 URL 指向 bare 推送靶。
        let repo = Repository::open(&copy).unwrap();
        repo.remote_set_url("origin", bare.to_str().unwrap())
            .unwrap();
    }
    write(&copy, "src/main.rs", "fn main() { delivered_chain(); }\n");
    let patch = workdir_patch(&copy);
    reset_worktree_to_head(&copy);

    let commit_hex =
        apply_patch_new_branch(&copy, "nemesis/task-7", &patch, "chain commit").unwrap();
    push_branch(
        &copy,
        "nemesis/task-7",
        "origin",
        "refs/heads/nemesis/task-7",
        None,
    )
    .unwrap();
    // bare 靶上出现交付分支且指向交付 commit。
    let bare_repo = Repository::open(&bare).unwrap();
    let remote_branch = bare_repo
        .find_branch("nemesis/task-7", git2::BranchType::Local)
        .unwrap();
    assert_eq!(
        remote_branch
            .get()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        commit_hex
    );
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&copy);
}

/// 分支在场探测（失败路径零残留断言用）。
fn repo_find_branch(root: &Path, name: &str) -> bool {
    Repository::open(root)
        .map(|r| r.find_branch(name, git2::BranchType::Local).is_ok())
        .unwrap_or(false)
}

/// 工作区还原（测试夹具语义：checkout head force，等价副本 reset --hard）。
fn reset_worktree_to_head(root: &Path) {
    let repo = Repository::open(root).unwrap();
    let head_obj = repo.head().unwrap().peel_to_commit().unwrap();
    let mut opts = git2::build::CheckoutBuilder::new();
    opts.force();
    repo.checkout_tree(head_obj.as_object(), Some(&mut opts))
        .unwrap();
}
