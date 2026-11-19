//! git_repo 测试。
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
    std::fs::write(root.join(".gitignore"), crate::archive::GITIGNORE_CONTENT).unwrap();
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
// ensure_repo + commit_worktree
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
    // .gitignore 自排除（publish 内容等值臂的前提）：未跟踪 + 命中自身
    // 规则 → 首 commit 不得把它带进跟踪树（否则 add_all 每次 pre-merge
    // 落定都会把它入树，master 树 = 交付树 +.gitignore → 漂移误拒）。
    assert!(
        tree.get_path(Path::new(".gitignore")).is_err(),
        ".gitignore 必须自排除，不入跟踪树"
    );
    // 工作集无新增（.gitignore 在盘不入库）→ 后续 commit 仍 no-op。
    assert!(commit_worktree(&root, "gitignore only").unwrap().is_none());
}

#[test]
fn ensure_repo_adopts_existing_files_as_initial_baseline() {
    // 用户选已有目录 → 现有文件作为初始基线录入（首 commit）。
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
// HEAD 树导出
// ---------------------------------------------------------------------------

#[test]
fn export_head_tree_writes_tracked_files_only() {
    let root = seeded_repo("export");
    // 未提交的脏文件不进导出（导出的是 HEAD 树，不是工作区）。
    write(&root, "src/dirty.rs", "// not committed\n");

    let dest = temp_dir("export-dest");
    let (count, bytes) = export_head_tree(&root, &dest).unwrap();
    assert_eq!(count, 1, "HEAD 树 = src/main.rs（.gitignore 自排除不入树）");
    assert!(bytes > 0);
    assert_eq!(read(&dest, "src/main.rs"), "fn main() {}\n");
    assert!(!dest.join("project.json").exists(), "投影件不入基线");
    assert!(!dest.join("src/dirty.rs").exists(), "未提交不导出");
}

// ---------------------------------------------------------------------------
// 三方合并矩阵
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
    // 二进制基线（含 NUL）；双方写不同字节 → 冲突 + binary 标记。
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

/// BUG-20261112-104 回归锁：删除补刀是提交落库后的卫生动作——删除被
/// 挡（打开句柄无 FILE_SHARE_DELETE = ERROR_SHARING_VIOLATION，AV/索引
/// 器瞬时持有的确定性形态）只允许 warn 残留，绝不向上传播（签名返回
/// ()，合并结果不受影响）。直调补刀函数——实合并流里 reset --hard 先
/// 行删除目标件，补刀失败态在真实流上探不到，只能以直调钉住韧性纪律。
/// 注：新 Rust std 的 Windows 删除走 POSIX 语义，只读位不再拦
/// remove_file——句柄占用才是跨版本稳定的注入形态。
#[cfg(windows)]
#[test]
fn sweep_deleted_files_resilient_to_permission_failures() {
    let (root, _baseline) = baseline_of("rosweep", "old\n");
    write(&root, "src/stale.h", "will-be-stale\n");
    commit_worktree(&root, "add stale").unwrap().unwrap();

    // 构造 old（含 stale.h）/ new（无 stale.h）两棵树。
    let repo = Repository::open(&root).unwrap();
    let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
    let old_tree = head_commit.tree().unwrap();
    let mut index = repo.index().unwrap();
    index.read_tree(&old_tree).unwrap();
    index
        .remove(std::path::Path::new("src/stale.h"), 0)
        .unwrap();
    let new_oid = index.write_tree_to(&repo).unwrap();
    let new_tree = repo.find_tree(new_oid).unwrap();

    // 无 FILE_SHARE_DELETE 的打开句柄 = 删除在句柄存续期间恒
    // ERROR_SHARING_VIOLATION——韧性重试 10 次耗尽后 warn 残留。
    // （Rust std File::open 自带 SHARE_DELETE 不拦删除，必须显式收窄。）
    let stale_path = root.join("src/stale.h");
    let _blocking_handle = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2) // FILE_SHARE_READ | FILE_SHARE_WRITE（无 DELETE）
            .open(&stale_path)
            .unwrap()
    };

    // 补刀不得 Err/panic（签名即契约：返回 ()）；失败只落 warn。
    sweep_deleted_workdir_files(&repo, &root, &old_tree, &new_tree);

    // 残留语义：句柄存续期间文件必须在场（交清扫兜底，不丢内容）。
    assert!(
        stale_path.exists(),
        "删除被句柄挡住时文件应残留（warn + 兜底语义）"
    );
    // 收尾：释放句柄后文件可正常清理（tempdir 可删）。
    drop(_blocking_handle);
    std::fs::remove_file(&stale_path).unwrap();
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

/// 复原（压测⑤）：变更集带工作区前缀 `board-projects/<项目名>/…` 时
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
    // 正确项目相对路径不动（对照组）。
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
    // 但不等档案目录名（别的项目路径）不剥。
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
// /：commit_resolution（AI 硬解落盘单一出口）
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
// 评审证据：commit_changed_files（F- 根修）
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
// F-：commit_blob_text（评审证据段内容注入的数据源）
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
// 外循环：clone_repo 幂等 + 钉 HEAD；push_branch 快进与非快进
// ---------------------------------------------------------------------------

#[test]
fn clone_repo_local_path_lands_head_and_content() {
    let origin = seeded_repo("clone-origin");
    let want_head = head_commit_hex(&origin).unwrap().unwrap();
    let dest = temp_dir("clone-dest");
    // 本地路径克隆（file 传输恒可用，不依赖 TLS）。
    let got = clone_repo(origin.to_str().unwrap(), &dest, None, None).unwrap();
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
    let first = clone_repo(origin.to_str().unwrap(), &dest, None, None).unwrap();
    // 副本里继续产出一笔 commit（模拟 worker 合并历史）。
    std::fs::write(dest.join("src/extra.rs"), "pub fn extra() {}\n").unwrap();
    let merged = commit_worktree(&dest, "local change").unwrap();
    assert!(merged.is_some());
    // 幂等重入：不 fetch 不重置，返回**副本当前** HEAD（不是远端旧 HEAD）。
    let again = clone_repo(origin.to_str().unwrap(), &dest, None, None).unwrap();
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
    clone_repo(origin.to_str().unwrap(), &dest, None, None).unwrap();
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
    clone_repo(origin.to_str().unwrap(), &dest, None, None).unwrap();
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
        None,
    )
    .unwrap_err();
    // libgit2 文案：`src refspec... does not match any`——诚实透传即可，
    // 断言只钉「失败 + 原因可读」。
    assert!(err.contains("推送"), "{err}");
    assert!(!err.trim().ends_with("失败"), "{err}");
}

// ---------------------------------------------------------------------------
// 外循环：branch_at_head（第三臂——交付已并入副本，发布分支直取 HEAD）
// ---------------------------------------------------------------------------

#[test]
fn branch_at_head_points_head_and_keeps_worktree() {
    let root = seeded_repo("bah-seed");
    let before = head(&root);
    let got = branch_at_head(&root, "nemesis/task-1").unwrap();
    assert_eq!(got, before, "返回值必须等值 HEAD commit");
    let repo = Repository::open(&root).unwrap();
    let b = repo
        .find_branch("nemesis/task-1", git2::BranchType::Local)
        .unwrap();
    assert_eq!(
        b.get().peel_to_commit().unwrap().id().to_string(),
        before,
        "分支必须指向 HEAD"
    );
    assert_eq!(head(&root), before, "工作区 HEAD 不得移动");
}

#[test]
fn branch_at_head_existing_branch_is_honest_error() {
    let root = seeded_repo("bah-dup");
    branch_at_head(&root, "nemesis/task-1").unwrap();
    let err = branch_at_head(&root, "nemesis/task-1").unwrap_err();
    assert!(err.contains("已存在"), "{err}");
}

#[test]
fn branch_at_head_unborn_head_is_err() {
    let root = temp_dir("bah-unborn");
    std::fs::create_dir_all(&root).unwrap();
    Repository::init(&root).unwrap();
    let err = branch_at_head(&root, "nemesis/task-1").unwrap_err();
    assert!(err.contains("HEAD"), "{err}");
}

// ---------------------------------------------------------------------------
// 外循环：branch_tree_if_exists（分支指向查询——发布重试幂等判断用）
// ---------------------------------------------------------------------------

#[test]
fn branch_tree_if_exists_absent_branch_is_none() {
    let root = seeded_repo("btie-absent");
    assert_eq!(
        branch_tree_if_exists(&root, "nemesis/task-1").unwrap(),
        None,
        "分支缺席 → None（走常规建分支路径）"
    );
}

#[test]
fn branch_tree_if_exists_present_reports_commit_and_tree() {
    let root = seeded_repo("btie-present");
    branch_at_head(&root, "nemesis/task-1").unwrap();
    let (commit, tree) = branch_tree_if_exists(&root, "nemesis/task-1")
        .unwrap()
        .expect("分支在场 → Some");
    assert_eq!(commit, head(&root), "commit 等值分支指向");
    let repo = Repository::open(&root).unwrap();
    let head_tree = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap()
        .id()
        .to_string();
    assert_eq!(tree, head_tree, "tree 等值 HEAD 树（branch_at_head 语义）");
}

// ---------------------------------------------------------------------------
// 外循环：apply_patch_new_branch（patch → 副本内交付分支，工作区回原位）
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
fn apply_patch_new_branch_existing_same_content_reuses_commit() {
    let root = seeded_repo("apply-reuse");
    write(&root, "src/main.rs", "fn main() { a }\n");
    let patch = workdir_patch(&root);
    reset_worktree_to_head(&root);
    let first = apply_patch_new_branch(&root, "nemesis/task-x", &patch, "m").unwrap();
    // 续作：同基线 + 同内容 → 幂等复用既有 commit（push/PR 阶段失败后
    // 重试不被自己上次的分支挡死）。
    let again = apply_patch_new_branch(&root, "nemesis/task-x", &patch, "m").unwrap();
    assert_eq!(again, first, "同内容重放必须复用同一 commit，不新建");
    // 复用路径同样不遗留脏工作区。
    let on_head = std::fs::read_to_string(root.join("src/main.rs")).unwrap();
    assert_eq!(on_head, "fn main() {}\n", "工作区必须回基线形态");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn apply_patch_new_branch_existing_mismatch_refuses_overwrite() {
    let root = seeded_repo("apply-mismatch");
    write(&root, "src/main.rs", "fn main() { a }\n");
    let patch = workdir_patch(&root);
    reset_worktree_to_head(&root);
    apply_patch_new_branch(&root, "nemesis/task-x", &patch, "m").unwrap();
    // 内容不同（换 patch）→ 拒绝覆盖，维持人工核查语义。
    write(&root, "src/main.rs", "fn main() { b }\n");
    let other = workdir_patch(&root);
    reset_worktree_to_head(&root);
    let err = apply_patch_new_branch(&root, "nemesis/task-x", &other, "m").unwrap_err();
    assert!(err.contains("拒绝覆盖"), "{err}");
    // 原交付分支未被第二笔内容污染。
    let repo = Repository::open(&root).unwrap();
    let delivered = repo
        .find_branch("nemesis/task-x", git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_commit()
        .unwrap();
    let entry = delivered
        .tree()
        .unwrap()
        .get_path(Path::new("src/main.rs"))
        .unwrap();
    let blob = repo.find_blob(entry.id()).unwrap();
    assert_eq!(
        std::str::from_utf8(blob.content()).unwrap(),
        "fn main() { a }\n",
        "拒绝覆盖路径原分支内容必须原样保留"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn apply_patch_new_branch_existing_new_baseline_refuses_reuse() {
    let root = seeded_repo("apply-newbase");
    write(&root, "src/main.rs", "fn main() { a }\n");
    let patch = workdir_patch(&root);
    reset_worktree_to_head(&root);
    apply_patch_new_branch(&root, "nemesis/task-x", &patch, "m").unwrap();
    // 基线前移后同 patch 重放：父基线不一致 → 不复用、拒绝覆盖（旧分支
    // 基于旧基线，续作推送会形成错误谱系）。
    write(&root, "src/other.rs", "pub fn b() {}\n");
    {
        // index 必须挂在存活的 repo 上（临时表达式 drop 后 libgit2 报
        // "Index is not backed up by an existing repository"）。
        let repo = Repository::open(&root).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("src/other.rs")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "advance base", &tree, &[&parent])
            .unwrap();
    }
    let err = apply_patch_new_branch(&root, "nemesis/task-x", &patch, "m").unwrap_err();
    assert!(err.contains("拒绝覆盖"), "{err}");
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
    clone_repo(origin.to_str().unwrap(), &copy, None, None).unwrap();
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

/// 克隆产物工作树必须与 HEAD blob 字节一致（回归锁： 全文件
/// 伪冲突案例——clone 自带的 checkout 先于 autocrlf 钉落执行，local config
/// 尚无键时 libgit2 落到系统级配置（本机 autocrlf=true），LF blob 被
/// smudge 成 CRLF 工作树；合并三方 base(blob LF) 与 ours/theirs(工作树
/// CRLF) 行尾不同源，未触及行全部伪冲突）。修复 = 钉完 config 对 HEAD
/// 强制 reset --hard 撤销 smudge。在全局 autocrlf=true 的机器上修复前必
/// 红；全局 false 的机器上本锁平凡通过，但「工作树=仓库字节」契约恒被钉住。
#[test]
fn clone_worktree_matches_blob_bytes_no_smudge() {
    let origin = seeded_repo("clone-nosmudge-origin");
    let copy = temp_dir("clone-nosmudge-copy");
    let head = clone_repo(origin.to_str().unwrap(), &copy, None, None).unwrap();
    {
        let repo = Repository::open(&copy).unwrap();
        let commit = repo
            .find_commit(git2::Oid::from_str(&head).unwrap())
            .unwrap();
        let entry = commit
            .tree()
            .unwrap()
            .get_path(Path::new("src/main.rs"))
            .unwrap();
        let blob = repo.find_blob(entry.id()).unwrap();
        let blob_bytes = blob.content();
        let worktree_bytes = std::fs::read(copy.join("src/main.rs")).unwrap();
        assert_eq!(
            worktree_bytes, blob_bytes,
            "克隆产物工作树字节被 checkout 过滤器改写（smudge 未撤销，合并三方行尾将不同源）"
        );
        assert!(!blob_bytes.contains(&b'\r'), "源 blob 应为 LF 原始字节");
    }
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&copy);
}

// ---------------------------------------------------------------------------
// sync_repo_remote（jira-gitlab-flow 批：确定性 clone-or-pull）
// ---------------------------------------------------------------------------

/// bare 推送靶（libgit2 本地传输不支持推非 bare，同 push_branch 测试纪律）。
fn bare_origin(tag: &str) -> (PathBuf, PathBuf) {
    let origin = seeded_repo(&format!("sync-{tag}-origin"));
    let bare = temp_dir(&format!("sync-{tag}-bare"));
    {
        let mut builder = git2::build::RepoBuilder::new();
        builder.bare(true);
        builder.clone(origin.to_str().unwrap(), &bare).unwrap();
    }
    (origin, bare)
}

/// bare 远端 HEAD 分支推一笔新 commit（工作克隆 + commit_worktree + push）。
/// 返回新 commit hex。
fn advance_bare(bare: &Path, tag: &str, rel: &str, content: &str) -> String {
    let work = temp_dir(&format!("sync-advance-{tag}"));
    clone_repo(bare.to_str().unwrap(), &work, None, None).unwrap();
    write(&work, rel, content);
    commit_worktree(&work, &format!("remote advance {tag}"))
        .unwrap()
        .expect("advance 必须产出 commit");
    let repo = Repository::open(&work).unwrap();
    let branch = repo.head().unwrap().shorthand().unwrap().to_string();
    push_branch(
        &work,
        &branch,
        "origin",
        &format!("refs/heads/{branch}"),
        None,
        None,
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(&work);
    head_of_commit(bare, "HEAD")
}

/// 任意仓库/引用的 commit hex（断言辅助）。
fn head_of_commit(root: &Path, refname: &str) -> String {
    Repository::open(root)
        .unwrap()
        .find_reference(refname)
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string()
}

/// 本地建分支并推到 bare 靶（声明分支用例的远端侧准备）。
fn push_named_branch(bare: &Path, name: &str, tag: &str) {
    let work = temp_dir(&format!("sync-named-{tag}"));
    clone_repo(bare.to_str().unwrap(), &work, None, None).unwrap();
    {
        let repo = Repository::open(&work).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch(name, &commit, false).unwrap();
    }
    push_branch(
        &work,
        name,
        "origin",
        &format!("refs/heads/{name}"),
        None,
        None,
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn sync_repo_fresh_clone_matches_origin_head() {
    let (origin, bare) = bare_origin("fresh");
    let dest = temp_dir("sync-fresh-dest");
    let (outcome, head_hex) =
        sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap();
    assert_eq!(outcome, SyncOutcome::Cloned);
    assert_eq!(head_hex, head_of_commit(&bare, "HEAD"));
    // 工作树字节契约：clone 产物与源文件一致 + autocrlf 已钉死。
    assert_eq!(read(&dest, "src/main.rs"), read(&origin, "src/main.rs"));
    let repo = Repository::open(&dest).unwrap();
    assert_eq!(repo.config().unwrap().get_bool("core.autocrlf"), Ok(false));
    assert!(workdir_is_clean(&dest).unwrap());
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn sync_repo_fresh_clone_declared_branch_checks_out() {
    let (origin, bare) = bare_origin("freshdev");
    push_named_branch(&bare, "dev", "freshdev");
    let dest = temp_dir("sync-freshdev-dest");
    let (outcome, head_hex) =
        sync_repo_remote(bare.to_str().unwrap(), &dest, Some("dev"), None, None).unwrap();
    assert_eq!(outcome, SyncOutcome::Checkout);
    assert_eq!(head_hex, head_of_commit(&bare, "refs/heads/dev"));
    let repo = Repository::open(&dest).unwrap();
    assert_eq!(repo.head().unwrap().shorthand().unwrap(), "dev");
    assert!(workdir_is_clean(&dest).unwrap());
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&dest);
}

/// scaffold 形态目录（board.project.create ensure_scaffold 预置档案在场）。
fn scaffold_dir(name: &str) -> PathBuf {
    let dest = temp_dir(name);
    std::fs::create_dir_all(dest.join("docs/review")).unwrap();
    std::fs::create_dir_all(dest.join("artifacts")).unwrap();
    std::fs::create_dir_all(dest.join("records")).unwrap();
    std::fs::write(
        dest.join("project.json"),
        b"{\"id\":1,\"name\":\"demo\",\"directory\":null}",
    )
    .unwrap();
    std::fs::write(dest.join("timeline.jsonl"), b"").unwrap();
    std::fs::write(dest.join(".gitignore"), crate::archive::GITIGNORE_CONTENT).unwrap();
    dest
}

#[test]
fn sync_repo_clones_into_scaffolded_dir_preserving_files() {
    let (origin, bare) = bare_origin("scaffold");
    // 目标目录已有 scaffold 档案（非空）——libgit2 clone 直接 clone 会拒绝，
    // 必须走就地物化臂。
    let dest = scaffold_dir("sync-scaffold-dest");
    let (outcome, head_hex) =
        sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap();
    assert_eq!(outcome, SyncOutcome::Cloned);
    assert_eq!(head_hex, head_of_commit(&bare, "HEAD"));
    // scaffold 档案逐字节保留（init/checkout 不触碰未跟踪内容）。
    assert_eq!(
        std::fs::read(dest.join("project.json")).unwrap(),
        b"{\"id\":1,\"name\":\"demo\",\"directory\":null}"
    );
    assert_eq!(std::fs::read(dest.join("timeline.jsonl")).unwrap(), b"");
    assert!(dest.join("docs/review").is_dir());
    assert!(dest.join("artifacts").is_dir());
    assert!(dest.join("records").is_dir());
    // 仓库内容 + 字节契约 + 洁净态（scaffold.gitignore 恰好豁免投影件，
    // artifacts/ 空目录对 git 不可）。
    assert_eq!(read(&dest, "src/main.rs"), read(&origin, "src/main.rs"));
    let repo = Repository::open(&dest).unwrap();
    assert_eq!(repo.config().unwrap().get_bool("core.autocrlf"), Ok(false));
    assert!(workdir_is_clean(&dest).unwrap());
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn sync_repo_scaffolded_dir_declared_branch_checks_out() {
    let (origin, bare) = bare_origin("scaffolddev");
    push_named_branch(&bare, "dev", "scaffolddev");
    let dest = scaffold_dir("sync-scaffolddev-dest");
    let (outcome, head_hex) =
        sync_repo_remote(bare.to_str().unwrap(), &dest, Some("dev"), None, None).unwrap();
    assert_eq!(outcome, SyncOutcome::Checkout);
    assert_eq!(head_hex, head_of_commit(&bare, "refs/heads/dev"));
    let repo = Repository::open(&dest).unwrap();
    assert_eq!(repo.head().unwrap().shorthand().unwrap(), "dev");
    // scaffold 档案在场 + 仓库字节契约。
    assert!(dest.join("project.json").is_file());
    assert_eq!(read(&dest, "src/main.rs"), read(&origin, "src/main.rs"));
    assert!(workdir_is_clean(&dest).unwrap());
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn sync_repo_existing_repo_declared_branch_missing_materializes() {
    let (origin, bare) = bare_origin("matdev");
    let dest = temp_dir("sync-matdev-dest");
    sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap();
    // 远端后补 dev 分支；本地仓库再次 sync 声明分支 → 从 fetch 物化。
    push_named_branch(&bare, "dev", "matdev");
    let (outcome, head_hex) =
        sync_repo_remote(bare.to_str().unwrap(), &dest, Some("dev"), None, None).unwrap();
    assert_eq!(outcome, SyncOutcome::Checkout);
    assert_eq!(head_hex, head_of_commit(&bare, "refs/heads/dev"));
    let repo = Repository::open(&dest).unwrap();
    assert_eq!(repo.head().unwrap().shorthand().unwrap(), "dev");
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn sync_repo_up_to_date_is_noop_same_head() {
    let (origin, bare) = bare_origin("uptodate");
    let dest = temp_dir("sync-uptodate-dest");
    sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap();
    let before = head(&dest);
    let (outcome, head_hex) =
        sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap();
    assert_eq!(outcome, SyncOutcome::UpToDate);
    assert_eq!(head_hex, before);
    assert_eq!(head(&dest), before, "UpToDate 不得移动 HEAD");
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn sync_repo_fast_forward_advances_local_branch() {
    let (origin, bare) = bare_origin("ff");
    let dest = temp_dir("sync-ff-dest");
    let before = sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None)
        .unwrap()
        .1;
    let remote_hex = advance_bare(&bare, "ff", "src/remote.txt", "remote change\n");
    let (outcome, head_hex) =
        sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap();
    assert_eq!(outcome, SyncOutcome::FastForward);
    assert_eq!(head_hex, remote_hex);
    assert_eq!(read(&dest, "src/remote.txt"), "remote change\n");
    // 快进 = 线性推进：新 commit 单父 == 推进前 HEAD。
    {
        let repo = Repository::open(&dest).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_count(), 1);
        assert_eq!(commit.parent(0).unwrap().id().to_string(), before);
    }
    assert!(workdir_is_clean(&dest).unwrap());
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn sync_repo_diverged_creates_two_parent_merge_commit() {
    let (origin, bare) = bare_origin("merged");
    let dest = temp_dir("sync-merged-dest");
    let _before = sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None)
        .unwrap()
        .1;
    let remote_hex = advance_bare(&bare, "merged", "src/remote.txt", "remote change\n");
    // 本地独立 commit → 与远端分叉。
    write(&dest, "src/local.txt", "local change\n");
    let local_hex = commit_worktree(&dest, "local change")
        .unwrap()
        .expect("local commit 必须产出");
    let (outcome, _head_hex) =
        sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap();
    assert_eq!(outcome, SyncOutcome::Merged);
    // 双亲 merge commit：parent[0] = 本地头、parent[1] = 远端头。
    {
        let repo = Repository::open(&dest).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(
            commit.parent_count(),
            2,
            "分叉同步必须产出双亲 merge commit"
        );
        assert_eq!(commit.parent(0).unwrap().id().to_string(), local_hex);
        assert_eq!(commit.parent(1).unwrap().id().to_string(), remote_hex);
    }
    // 工作区同时含两侧改动（合并树已检出）。
    assert_eq!(read(&dest, "src/local.txt"), "local change\n");
    assert_eq!(read(&dest, "src/remote.txt"), "remote change\n");
    assert!(workdir_is_clean(&dest).unwrap());
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn sync_repo_conflict_lists_paths_without_moving_branch() {
    let (origin, bare) = bare_origin("conflict");
    let dest = temp_dir("sync-conflict-dest");
    sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap();
    let _ = advance_bare(
        &bare,
        "conflict",
        "src/main.rs",
        "fn main() { remote_edit(); }\n",
    );
    // 本地同文件同区域改动 → 三方合并冲突。
    write(&dest, "src/main.rs", "fn main() { local_edit(); }\n");
    commit_worktree(&dest, "local same-line edit")
        .unwrap()
        .expect("local commit");
    let before = head(&dest);
    let err = sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap_err();
    assert!(err.contains("冲突"), "{err}");
    assert!(err.contains("src/main.rs"), "错误必须列冲突路径: {err}");
    // 冲突 = 停车走人：分支头停在本地 commit 不动、工作区干净（合并结果零落地）。
    assert_eq!(head(&dest), before);
    assert!(workdir_is_clean(&dest).unwrap());
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn sync_repo_dirty_workdir_rejected_with_sample() {
    let (origin, bare) = bare_origin("dirty");
    let dest = temp_dir("sync-dirty-dest");
    sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap();
    let before = head(&dest);
    advance_bare(&bare, "dirty", "src/remote.txt", "remote change\n");
    // 未跟踪脏文件 → 拒绝同步且错误携带样本。
    write(&dest, "src/wip.txt", "human work in progress\n");
    let err = sync_repo_remote(bare.to_str().unwrap(), &dest, None, None, None).unwrap_err();
    assert!(err.contains("脏"), "{err}");
    assert!(err.contains("src/wip.txt"), "错误必须列脏项样本: {err}");
    assert_eq!(head(&dest), before);
    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&bare);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn sync_repo_swaps_stale_origin_to_declared_url() {
    // 步骤 url 是单一意图表达：既有 origin 指向别处（形态切换/迁移）必须
    // 改写后 fetch——回归锁（repo_sync 沿用旧 origin 缺陷）。
    let (origin_a, bare_a) = bare_origin("swap-a");
    // B 直接裸克隆 A：两处 bare_origin 各自 ensure_repo 的首 commit 带
    // Signature:now 墙钟（秒级粒度），两次建仓跨秒则 seed commit id 不同、
    // A/B 历史分叉 → FF 判定误落 Merged（CI coverage
    // 「B 的 commit 应快进进来 left: Merged right: FastForward」）。裸克隆
    // 保证 seed commit id 逐位一致，测试不再吃建仓速度竞态。
    let bare_b = temp_dir("sync-swap-b-bare");
    {
        let mut builder = git2::build::RepoBuilder::new();
        builder.bare(true);
        builder.clone(origin_a.to_str().unwrap(), &bare_b).unwrap();
    }
    let dest = temp_dir("sync-swap-dest");
    sync_repo_remote(bare_a.to_str().unwrap(), &dest, None, None, None).unwrap();
    let origin_before = {
        let repo = Repository::open(&dest).unwrap();
        repo.find_remote("origin")
            .unwrap()
            .url()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        origin_before.trim_end_matches('/'),
        bare_a.to_str().unwrap(),
        "夹具前置：origin 初始指向 A"
    );

    // B 推新 commit；A 也推一个兄弟 commit（用于验证 fetch 没打回 A）。
    let remote_b = advance_bare(&bare_b, "swap", "src/remote.txt", "from B\n");
    advance_bare(&bare_a, "swap", "src/other.txt", "from A\n");

    // 以 B 的 url（带尾斜杠形态，顺带验证 trim 对账不误写）再同步。
    let url_b = format!("{}/", bare_b.to_str().unwrap());
    let (outcome, head_hex) = sync_repo_remote(&url_b, &dest, None, None, None).unwrap();
    assert_eq!(outcome, SyncOutcome::FastForward, "B 的 commit 应快进进来");
    assert_eq!(head_hex, remote_b, "HEAD 必须落在 B 的 commit 上");
    assert_ne!(
        head(&dest),
        head_of_commit(&bare_a, "HEAD"),
        "A 的兄弟 commit 不得混入（fetch 必须打 B）"
    );
    let origin_after = {
        let repo = Repository::open(&dest).unwrap();
        repo.find_remote("origin")
            .unwrap()
            .url()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        origin_after.trim_end_matches('/'),
        bare_b.to_str().unwrap(),
        "origin 必须已改写为步骤 url"
    );

    // 再跑一轮：url 一致（trim 后）= UpToDate 不空转。
    let (outcome2, head2) =
        sync_repo_remote(bare_b.to_str().unwrap(), &dest, None, None, None).unwrap();
    assert_eq!(outcome2, SyncOutcome::UpToDate);
    assert_eq!(head2, remote_b);

    let _ = std::fs::remove_dir_all(&origin_a);
    let _ = std::fs::remove_dir_all(&bare_a);
    let _ = std::fs::remove_dir_all(&bare_b);
}
