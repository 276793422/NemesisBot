//! vm_sync 宿主侧单测（纯宿主可跑：选源/打包/账本/安全闸；带真实 VM 的
//! 全链路归 B9-2/B9-3 真机臂——本文件不假装测 VM 内行为）。

use std::path::PathBuf;

use super::{
    BaselineManifest, DivergedEntry, FileSync, SyncExclusion, baseline_dir, case_gate_pairs,
    diverged_verdict, host_fs_case_insensitive, load_baseline, pack_tar_blocking,
    parse_index_modes, safe_rel, select_files,
};

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
}

fn admits(rel: &str) -> bool {
    SyncExclusion { is_git: false }.admits(rel)
}

// ---------------------------------------------------------------------------
// 排除规则 admit 闸
// ---------------------------------------------------------------------------

#[test]
fn admits_prefix_and_exact_dir_match() {
    for rel in [
        ".git/HEAD",
        ".git", // 与排除目录同名的边缘形态（worktree gitfile 等，宁缺勿滥）
        "target",
        "target/debug/foo.o",
        "node_modules/left-pad/index.js",
        "__pycache__/m.pyc",
    ] {
        assert!(!admits(rel), "应排除: {rel}");
    }
    for rel in [
        "src/main.rs",
        "gitignore-note.md",
        "targeting.rs", // 前缀是目录形态（含 /），不能误伤同名前缀文件
    ] {
        assert!(admits(rel), "应保留: {rel}");
    }
}

#[test]
fn exclusion_detect_marks_git_root() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(!SyncExclusion::detect(tmp.path()).is_git);
    std::fs::create_dir(tmp.path().join(".git")).unwrap();
    assert!(SyncExclusion::detect(tmp.path()).is_git);
}

// ---------------------------------------------------------------------------
// walk 选源：排序 + 排除整枝剪
// ---------------------------------------------------------------------------

#[test]
fn walk_selection_sorted_and_excluded() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("README.md"), b"r").unwrap();
    std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
    std::fs::write(root.join("target/debug/x.o"), b"obj").unwrap();
    std::fs::write(root.join("node_modules/pkg/i.js"), b"i").unwrap();
    std::fs::write(root.join(".git/HEAD"), b"ref").unwrap();

    let files = super::select_files_walk(root).unwrap();
    let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, vec!["README.md", "src/main.rs"]);
}

// ---------------------------------------------------------------------------
// git index 模式解析
// ---------------------------------------------------------------------------

#[test]
fn parse_index_modes_extract_mode_and_path() {
    // git ls-files -z 的真实形态：条目 NUL 分隔，meta 与 path 以 \t 分界。
    let raw = b"100644 0123456789012345678901234567890123456789 0\tsrc/a.rs\x00\
                100755 0123456789012345678901234567890123456789 0\tsh/run.sh\x00";
    let m = parse_index_modes(raw);
    assert_eq!(m.get("src/a.rs"), Some(&0o644));
    assert_eq!(m.get("sh/run.sh"), Some(&0o755));
    assert_eq!(m.get("missing"), None);
}

// ---------------------------------------------------------------------------
// git 集成选源（探针守卫：git 缺席 = 诚实 SKIP，不伪装失败）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn select_files_git_respects_gitignore() {
    // 工具在场 ≠ 可用：先以 git 自身实跑探针定夺（环境纪律）。
    let probe = std::process::Command::new("git").arg("--version").output();
    let Ok(out) = probe else {
        eprintln!("SKIP: git 无法启动（环境不满足）");
        return;
    };
    if !out.status.success() {
        eprintln!("SKIP: git --version 失败（环境不满足）");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "git init 失败: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    std::fs::write(root.join(".gitignore"), b"ignored.txt\ntarget/\n").unwrap();
    std::fs::write(root.join("a.rs"), b"fn a() {}").unwrap();
    std::fs::write(root.join("ignored.txt"), b"no").unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target/o.o"), b"o").unwrap();

    let (files, is_git, selection) = select_files(root).await.unwrap();
    assert!(is_git);
    assert_eq!(selection, "git");
    let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(
        rels,
        vec![".gitignore", "a.rs"],
        "忽略文件/排除目录不应入选"
    );
}

/// 盘面是唯一真相：`--cached` 会列出已从盘面删除但未 stage 的 index 幽灵
/// 条目——修前带进打包清单 = pack_tar「清单与盘面失配」诚实报错，一个
/// 未 stage 的普通删除把首次全量同步卡死开工。
#[tokio::test]
async fn select_files_git_filters_ghost_cached_entries() {
    let probe = std::process::Command::new("git").arg("--version").output();
    let Ok(out) = probe else {
        eprintln!("SKIP: git 无法启动（环境不满足）");
        return;
    };
    if !out.status.success() {
        eprintln!("SKIP: git --version 失败（环境不满足）");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // 提交身份 -c 注入：CI 容器可能没配全局 user.email/name。
    let git = |args: &[&str]| {
        let mut c = std::process::Command::new("git");
        c.arg("-c")
            .arg("user.email=t@t")
            .arg("-c")
            .arg("user.name=t");
        c.args(args).current_dir(root).output().unwrap()
    };
    assert!(git(&["init", "-q"]).status.success());
    std::fs::write(root.join("ghost.rs"), b"fn g() {}").unwrap();
    std::fs::write(root.join("alive.rs"), b"fn a() {}").unwrap();
    assert!(git(&["add", "."]).status.success());
    assert!(git(&["commit", "-q", "-m", "init"]).status.success());

    // 盘面删除但不 stage：index 幽灵条目形态（修前此态选源即失败）。
    std::fs::remove_file(root.join("ghost.rs")).unwrap();

    let files = super::select_files_git(root).await.unwrap();
    let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
    assert_eq!(rels, vec!["alive.rs"], "幽灵条目应被盘面过滤剔除: {rels:?}");
}

// ---------------------------------------------------------------------------
// 打包：mode 覆写 + 单趟哈希 + 内容一致
// ---------------------------------------------------------------------------

#[test]
fn pack_tar_writes_content_mode_and_hashes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.txt"), b"hello tar").unwrap();
    std::fs::write(root.join("sub/b.bin"), [1u8, 2, 3, 4, 5]).unwrap();

    let files = vec![
        FileSync {
            rel: "a.txt".into(),
            mode: 0o755, // Windows 宿主读不到 exec 位，必须按条目覆写
        },
        FileSync {
            rel: "sub/b.bin".into(),
            mode: 0o644,
        },
    ];
    let out: PathBuf = tmp.path().join("out.tar");
    let (hashes, payload_bytes) = pack_tar_blocking(root, &files, &out).unwrap();

    assert_eq!(hashes.get("a.txt").unwrap(), &sha256_hex(b"hello tar"));
    assert_eq!(payload_bytes, 9 + 5);

    let f = std::fs::File::open(&out).unwrap();
    let mut ar = tar::Archive::new(std::io::BufReader::new(f));
    let mut seen = std::collections::HashMap::new();
    for ent in ar.entries().unwrap() {
        let mut ent = ent.unwrap();
        let name = ent.path().unwrap().to_string_lossy().replace('\\', "/");
        let mode = ent.header().mode().unwrap();
        let mut body = Vec::new();
        std::io::Read::read_to_end(&mut ent, &mut body).unwrap();
        seen.insert(name, (mode, body));
    }
    let (mode_a, body_a) = seen.get("a.txt").expect("缺 a.txt");
    assert_eq!(
        *mode_a, 0o755,
        "header mode 必须按 FileSync.mode 覆写（NTFS 无 exec 位）"
    );
    assert_eq!(body_a, b"hello tar");
    let (mode_b, body_b) = seen.get("sub/b.bin").expect("缺 sub/b.bin");
    assert_eq!(*mode_b, 0o644);
    assert_eq!(body_b, &[1u8, 2, 3, 4, 5]);
}

// ---------------------------------------------------------------------------
// 基线账本：读写 roundtrip + 坏账本诚实降级
// ---------------------------------------------------------------------------

#[test]
fn baseline_roundtrip_and_corrupt_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let dir = baseline_dir(ws, "NB-project/1");
    std::fs::create_dir_all(&dir).unwrap();
    let mut files = std::collections::BTreeMap::new();
    files.insert("a.txt".to_string(), "aa".repeat(32));
    let m = BaselineManifest {
        project_id: "NB-project/1".into(),
        synced_at_ms: 1234,
        is_git: true,
        selection: "git".into(),
        bytes: 42,
        skipped_symlinks: 0,
        files,
        host_meta: std::collections::BTreeMap::new(),
    };
    std::fs::write(
        dir.join("baseline.json"),
        serde_json::to_string(&m).unwrap(),
    )
    .unwrap();
    let loaded = load_baseline(ws, "NB-project/1").expect("应读到基线");
    assert_eq!(loaded.project_id, "NB-project/1");
    assert_eq!(
        loaded.files.get("a.txt").map(String::as_str),
        Some("a".repeat(64).as_str())
    );

    // 坏基线 = 无基线（下次开工重全量），不 panic 不静默成功。
    std::fs::write(dir.join("baseline.json"), "{not json").unwrap();
    assert!(load_baseline(ws, "NB-project/1").is_none());
    assert!(load_baseline(ws, "不存在的项目").is_none());
}

/// 原子写：内容落位 + 唯一临时名纪律（成功后不残留 tmp；旧固定
/// `*.json.tmp` 依赖「调用方持 INFLIGHT 闸」隐式不变式，唯一名把安全
/// 性收进函数自身）。
#[tokio::test]
async fn write_json_atomic_writes_content_and_leaves_no_tmp() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("nested/dir/x.json");
    super::write_json_atomic(&p, "{\"a\":1}").await.unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"a\":1}");
    let leftovers: Vec<String> = std::fs::read_dir(p.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, vec!["x.json"], "不得残留临时文件: {leftovers:?}");
}

// ---------------------------------------------------------------------------
// rel 安全闸（B9-2 commit 回读的防御闸先备）
// ---------------------------------------------------------------------------

#[test]
fn safe_rel_rejects_traversal_and_absolute() {
    assert!(safe_rel("src/main.rs").is_ok());
    assert!(safe_rel("a/b/c.txt").is_ok());
    for bad in ["../evil", "a/../../b", "/abs/path", ""] {
        assert!(safe_rel(bad).is_err(), "应拒绝: {bad}");
    }
    // Windows 盘符前缀在 unix 上只是普通字符，平台相关断言挂 cfg。
    #[cfg(windows)]
    assert!(safe_rel("C:\\win").is_err(), "应拒绝盘符绝对路径");
}

// ---------------------------------------------------------------------------
// B9-2 开工闸：git status -z 解析 / 宿主 meta diff / 三方裁决真值表
// ---------------------------------------------------------------------------

#[test]
fn parse_status_z_collects_rename_delete_untracked() {
    let ex = SyncExclusion { is_git: true };
    // porcelain v1 -z 形态：每条目 `XY <path>\0`；R/C 条目再跟第二段 old\0。
    let mut raw: Vec<u8> = Vec::new();
    let push = |raw: &mut Vec<u8>, segs: &[&[u8]]| {
        for s in segs {
            raw.extend_from_slice(s);
            raw.push(0);
        }
    };
    push(&mut raw, &[b" M src/modified.rs"]);
    push(&mut raw, &[b"?? new/file.txt"]);
    push(&mut raw, &[b" D gone/old.txt"]);
    push(&mut raw, &[b"R  renamed.txt", b"original.txt"]);
    push(&mut raw, &[b"?? target/x.o"]); // 排除表应滤掉
    push(&mut raw, &[b"?? "]); // 残条（len<4）静默跳过
    push(&mut raw, &[b"C  copied.txt", b"renamed.txt"]); // C 同款双段（C 消费 old 段）

    let mut got = super::parse_status_z(&raw, &ex);
    got.sort();
    assert_eq!(
        got,
        vec![
            "copied.txt".to_string(),
            "gone/old.txt".to_string(),
            "new/file.txt".to_string(),
            "original.txt".to_string(),
            "renamed.txt".to_string(),
            "src/modified.rs".to_string(),
        ]
    );
}

#[test]
fn diff_host_meta_upsert_delete_and_sort() {
    use super::HostChangeKind;
    use super::HostFileMeta as M;
    let meta = |m: u64, s: u64| M {
        mtime_ms: m,
        size: s,
    };
    let mut baseline = std::collections::BTreeMap::new();
    baseline.insert("a.txt".to_string(), meta(1, 1));
    baseline.insert("b.txt".to_string(), meta(2, 2));
    baseline.insert("c.txt".to_string(), meta(3, 3));
    let mut current = std::collections::BTreeMap::new();
    current.insert("a.txt".to_string(), meta(1, 1)); // 未变
    current.insert("b.txt".to_string(), meta(9, 2)); // mtime 变 → Upsert
    current.insert("d.txt".to_string(), meta(4, 4)); // 新增 → Upsert
    // c 缺席 → Delete

    let changes = super::diff_host_meta(&baseline, &current);
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0].rel, "b.txt");
    assert_eq!(changes[0].kind, HostChangeKind::Upsert);
    assert_eq!(changes[1].rel, "c.txt");
    assert_eq!(changes[1].kind, HostChangeKind::Delete);
    assert_eq!(changes[2].rel, "d.txt");
    assert_eq!(changes[2].kind, HostChangeKind::Upsert);
}

#[test]
fn classify_host_change_truth_table() {
    use super::{ChangeKind, HostChangeKind, classify_host_change};
    const SHA_B: &str = "base";
    const SHA_V: &str = "vmmod";

    // Upsert：VM 等于基线 = 推；双侧皆无 = 纯新增推；VM 缺失基线条
    // （宿主改 + VM 无此文件 = 删改冲突）或偏离基线 = 冲突。
    assert!(matches!(
        classify_host_change(&HostChangeKind::Upsert, Some(SHA_B), Some(SHA_B)),
        ChangeKind::Push
    ));
    assert!(matches!(
        classify_host_change(&HostChangeKind::Upsert, None, None),
        ChangeKind::Push
    ));
    assert!(matches!(
        classify_host_change(&HostChangeKind::Upsert, Some(SHA_B), None),
        ChangeKind::Conflict
    ));
    assert!(matches!(
        classify_host_change(&HostChangeKind::Upsert, Some(SHA_B), Some(SHA_V)),
        ChangeKind::Conflict
    ));
    assert!(matches!(
        classify_host_change(&HostChangeKind::Upsert, None, Some(SHA_V)),
        ChangeKind::Conflict
    ));

    // Delete：基线无 = Skip；VM 未改/也无 = DeleteOnVm；VM 改过 = 冲突。
    assert!(matches!(
        classify_host_change(&HostChangeKind::Delete, None, Some(SHA_B)),
        ChangeKind::Skip
    ));
    assert!(matches!(
        classify_host_change(&HostChangeKind::Delete, Some(SHA_B), Some(SHA_B)),
        ChangeKind::DeleteOnVm
    ));
    assert!(matches!(
        classify_host_change(&HostChangeKind::Delete, Some(SHA_B), None),
        ChangeKind::DeleteOnVm
    ));
    assert!(matches!(
        classify_host_change(&HostChangeKind::Delete, Some(SHA_B), Some(SHA_V)),
        ChangeKind::Conflict
    ));
}

#[test]
fn diff_trees_changed_and_deleted() {
    let mut baseline = std::collections::BTreeMap::new();
    baseline.insert("same.txt".to_string(), "s".repeat(64));
    baseline.insert("gone.txt".to_string(), "g".repeat(64));
    baseline.insert("mod.txt".to_string(), "old".repeat(64));
    let mut current = std::collections::BTreeMap::new();
    current.insert("same.txt".to_string(), "s".repeat(64));
    current.insert("mod.txt".to_string(), "new".repeat(64));
    current.insert("added.txt".to_string(), "a".repeat(64));

    let (changed, deleted) = super::diff_trees(&baseline, &current);
    assert_eq!(
        changed,
        vec!["added.txt".to_string(), "mod.txt".to_string()]
    );
    assert_eq!(deleted, vec!["gone.txt".to_string()]);
}

// ---------------------------------------------------------------------------
// B9-2 commit 闸：tar 回读宿主（safe_rel 防御 + 内容 + unix exec 位）
// ---------------------------------------------------------------------------

fn build_tar(entries: &[(&str, Option<&[u8]>, Option<u32>)]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (path, body, mode) in entries {
        match body {
            None => {
                let mut h = tar::Header::new_gnu();
                h.set_size(0);
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(mode.unwrap_or(0o755));
                h.set_mtime(0);
                b.append_data(&mut h, path, std::io::empty()).unwrap();
            }
            Some(data) => {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(mode.unwrap_or(0o644));
                h.set_mtime(0);
                b.append_data(&mut h, path, *data).unwrap();
            }
        }
    }
    b.into_inner().unwrap()
}

#[test]
fn extract_tar_to_host_writes_content_and_creates_parents() {
    let tar_bytes = build_tar(&[
        ("sub", None, None),
        ("sub/a.txt", Some(b"hello host"), None),
        ("run.sh", Some(b"#!/bin/sh"), Some(0o755)),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    super::extract_tar_to_host(&tar_bytes, root).expect("解包应成功");
    assert_eq!(
        std::fs::read(root.join("sub/a.txt")).unwrap(),
        b"hello host"
    );
    assert_eq!(std::fs::read(root.join("run.sh")).unwrap(), b"#!/bin/sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(root.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m & 0o111, 0o111, "header exec 位应归一保留");
        let m2 = std::fs::metadata(root.join("sub/a.txt"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m2 & 0o111, 0, "普通文件不应带 exec 位");
    }
}

#[test]
fn extract_tar_to_host_rejects_traversal_entry() {
    // Builder::append_data 会拒绝 `..` 路径（构造期即拦），恶意归档只能
    // 直接改写 header 原始 name 字段后走无校验的 append 造出来。
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_size(4);
    h.set_mode(0o644);
    h.set_mtime(0);
    h.as_mut_bytes()[0..11].copy_from_slice(b"../evil.txt");
    h.set_cksum();
    b.append(&h, b"boom".as_slice()).unwrap();
    let tar_bytes = b.into_inner().unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let r = super::extract_tar_to_host(&tar_bytes, tmp.path());
    assert!(r.is_err(), "越界条目应拒绝");
}

// ---------------------------------------------------------------------------
// BUG-20261101-262：tar 条目读走 read_entry_capped 单一真相源
// （extract_tar_to_host_with_caps 可注入小 cap 钉行为）
// ---------------------------------------------------------------------------

#[test]
fn extract_tar_to_host_entry_cap_rejects_oversize() {
    let tar_bytes = build_tar(&[("big.bin", Some(&[0u8; 32]), None)]);
    let tmp = tempfile::tempdir().unwrap();
    let err = super::extract_tar_to_host_with_caps(&tar_bytes, tmp.path(), 16).unwrap_err();
    assert!(err.contains("上限"), "超 cap 条目应诚实报错，实际: {err}");
    assert!(
        !tmp.path().join("big.bin").exists(),
        "超 cap 条目不得落半份文件"
    );
}

#[test]
fn extract_tar_to_host_entry_cap_allows_within_cap() {
    let tar_bytes = build_tar(&[
        ("ok.txt", Some(b"within cap"), None),
        ("sub/deep.txt", Some(b"also ok"), None),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    super::extract_tar_to_host_with_caps(&tar_bytes, tmp.path(), 64).expect("cap 内应成功");
    assert_eq!(
        std::fs::read(tmp.path().join("ok.txt")).unwrap(),
        b"within cap"
    );
    assert_eq!(
        std::fs::read(tmp.path().join("sub/deep.txt")).unwrap(),
        b"also ok"
    );
}

#[test]
fn extract_tar_to_host_default_cap_constant_present() {
    // 生产包装必须带 256MB 条目上限（对齐 selfupdate 先例）——防有人把
    // 包装改成 cap=0 / usize::MAX 等无防御形态（哨兵断言）。
    assert_eq!(
        super::PULL_ENTRY_MAX_BYTES,
        256 * 1024 * 1024,
        "PULL_ENTRY_MAX_BYTES 应保持 256MB 先例值"
    );
    // 哨兵断言走 const 块：对 `const` 断言在编译期验证（零运行时成本，
    // 违约直接编译失败），也满足 clippy 1.95 `assertions_on_constants`
    //（-D warnings，CI 44a5e913 inline 实录两红）。
    const _: () = assert!(super::PULL_TOTAL_MAX_BYTES > 0);
    const _: () = assert!(super::PULL_CHUNK_MAX_BYTES > 0);
}

// ---------------------------------------------------------------------------
// BUG-20261101-262：pull wire 闸纯函数（chunk 计数 / 单块与总量预算）
// ---------------------------------------------------------------------------

#[test]
fn validate_pull_chunk_count_rejects_zero_and_oversize() {
    // chunks=0：非空清单的诚实 tar 至少 1 条目，0 = wire 谎报。
    let err = super::validate_pull_chunk_count(0).unwrap_err();
    assert!(err.contains("chunks=0"), "实际: {err}");
    // 超镜像上限（guest MAX_TOTAL_CHUNKS=200_000）。
    let err = super::validate_pull_chunk_count(200_001).unwrap_err();
    assert!(err.contains("超上限"), "实际: {err}");
    // 边界内放行。
    super::validate_pull_chunk_count(1).unwrap();
    super::validate_pull_chunk_count(200_000).unwrap();
}

#[test]
fn check_pull_chunk_budget_rejects_single_and_total() {
    // 单块超 8MiB 镜像（guest BULK_CHUNK_MAX_BYTES）。
    let err = super::check_pull_chunk_budget(0, super::PULL_CHUNK_MAX_BYTES + 1).unwrap_err();
    assert!(err.contains("单块"), "实际: {err}");
    // 累计超 2GiB：多块各不超限、合计越界。
    let err = super::check_pull_chunk_budget(
        super::PULL_TOTAL_MAX_BYTES as usize,
        super::PULL_CHUNK_MAX_BYTES,
    )
    .unwrap_err();
    assert!(err.contains("累计"), "实际: {err}");
    // 边界内放行（恰抵上限）。
    super::check_pull_chunk_budget(
        super::PULL_TOTAL_MAX_BYTES as usize - super::PULL_CHUNK_MAX_BYTES,
        super::PULL_CHUNK_MAX_BYTES,
    )
    .unwrap();
    super::check_pull_chunk_budget(0, super::PULL_CHUNK_MAX_BYTES).unwrap();
}

// ---------------------------------------------------------------------------
// 组件 symlink 闸（2026-10-20 复查批；与 guest 侧 extract_tar_safe 同族）
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn extract_tar_to_host_refuses_symlink_component() {
    // 宿主工作区（真实开发目录）存在链接组件：pnpm node_modules 形态。
    // VM 树回拉同名真实文件时不得顺着链接写出工作区。
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let tar_bytes = build_tar(&[("link/x.txt", Some(b"pwn"), None)]);
    let err = super::extract_tar_to_host(&tar_bytes, &root).unwrap_err();
    assert!(
        err.contains("symlink"),
        "必须 fail-closed 拒绝，实际错误: {err}"
    );
    assert!(
        !outside.join("x.txt").exists(),
        "回拉不得顺着链接写出工作区"
    );
}

#[cfg(unix)]
#[test]
fn extract_tar_to_host_refuses_final_symlink_no_clobber() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("store.bin"), b"original").unwrap();
    std::os::unix::fs::symlink(outside.join("store.bin"), root.join("victim")).unwrap();
    let tar_bytes = build_tar(&[("victim", Some(b"overwritten"), None)]);
    let err = super::extract_tar_to_host(&tar_bytes, &root).unwrap_err();
    assert!(err.contains("symlink"), "实际错误: {err}");
    assert_eq!(
        std::fs::read(outside.join("store.bin")).unwrap(),
        b"original",
        "链接目标不得被覆盖"
    );
}

// ---------------------------------------------------------------------------
// B9-2 旧基线兼容：缺 host_meta 键 = 空表（全量嫌疑 + 阈值兜底）
// ---------------------------------------------------------------------------

#[test]
fn baseline_without_host_meta_loads_empty_map() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = baseline_dir(tmp.path(), "p1");
    std::fs::create_dir_all(&dir).unwrap();
    let legacy = serde_json::json!({
        "project_id": "p1",
        "synced_at_ms": 1,
        "is_git": false,
        "selection": "walk",
        "bytes": 0,
        "skipped_symlinks": 0,
        "files": {},
    });
    std::fs::write(dir.join("baseline.json"), legacy.to_string()).unwrap();
    let loaded = load_baseline(tmp.path(), "p1").expect("B9-1 旧基线应可读");
    assert!(loaded.host_meta.is_empty(), "缺键应落空表（serde default）");
}

// ---------------------------------------------------------------------------
// B9-2 跨平台 commit 校验闸：大小写碰撞（大小写不敏感宿主回传互覆防线）
// ---------------------------------------------------------------------------

#[test]
fn case_pairs_detect_collision_within_changed() {
    let pairs = case_gate_pairs(
        &["src/Foo.rs".to_string(), "src/foo.rs".to_string()],
        &[],
        &[],
    );
    assert_eq!(
        pairs,
        vec![("src/Foo.rs".to_string(), "src/foo.rs".to_string())]
    );
}

#[test]
fn case_pairs_detect_changed_vs_existing_collision() {
    let pairs = case_gate_pairs(&["README.md".to_string()], &["readme.md".to_string()], &[]);
    assert_eq!(
        pairs,
        vec![("README.md".to_string(), "readme.md".to_string())]
    );
}

#[test]
fn case_pairs_ignore_historical_collision_not_touched_by_commit() {
    // 两端都是既有文件（不在 changed）——历史碰撞不由本笔提交停车。
    let pairs = case_gate_pairs(
        &["other.rs".to_string()],
        &["Bar.rs".to_string(), "bar.rs".to_string()],
        &[],
    );
    assert!(pairs.is_empty(), "{pairs:?}");
}

#[test]
fn case_pairs_case_only_rename_via_delete_is_not_collision() {
    // VM 删 foo.rs + 加 Foo.rs = 大小写改名——deleted 参与剔除，不误停车。
    let pairs = case_gate_pairs(
        &["Foo.rs".to_string()],
        &["foo.rs".to_string()],
        &["foo.rs".to_string()],
    );
    assert!(pairs.is_empty(), "{pairs:?}");
}

#[test]
fn case_pairs_identical_bytes_are_not_collision() {
    let pairs = case_gate_pairs(&["x.md".to_string()], &["x.md".to_string()], &[]);
    assert!(pairs.is_empty(), "{pairs:?}");
}

#[test]
fn case_pairs_empty_inputs_are_clean() {
    assert!(case_gate_pairs(&[], &[], &[]).is_empty());
}

#[test]
fn case_gate_enforcement_follows_host_fs() {
    // 平台可移植断言：检测层恒出对；强制层（Err/warn）随宿主文件系统语义。
    let pairs = case_gate_pairs(&["A".to_string()], &["a".to_string()], &[]);
    assert_eq!(pairs, vec![("A".to_string(), "a".to_string())]);
    assert_eq!(
        host_fs_case_insensitive(),
        cfg!(windows) || cfg!(target_os = "macos")
    );
}

// ---------------------------------------------------------------------------
// 挂起表裁决（H3 停车语义：diverged_verdict 纯函数）
// ---------------------------------------------------------------------------

fn parked_entry(host_sha: &str, vm_sha: &str) -> DivergedEntry {
    DivergedEntry {
        host_sha: host_sha.to_string(),
        vm_sha: vm_sha.to_string(),
        parked_at_ms: 0,
    }
}

#[test]
fn diverged_host_reedited_is_release_merged() {
    // 宿主重新改动（指纹既非挂起时宿主版、也非 VM 版）= 手动合并完成。
    let e = parked_entry("HOST_OLD", "VM1");
    assert!(matches!(
        diverged_verdict(&e, Some("HOST_NEW"), Some("VM1")),
        super::DivergedVerdict::ReleaseMerged
    ));
}

#[test]
fn diverged_host_untouched_stays_parked() {
    // 宿主没再动 = 继续挂起（VM 侧怎么漂都不影响——合并由宿主侧发起）。
    let e = parked_entry("HOST_OLD", "VM1");
    assert!(matches!(
        diverged_verdict(&e, Some("HOST_OLD"), Some("VM9")),
        super::DivergedVerdict::KeepParked
    ));
    // VM 侧文件消失（worker 清理）同样不解挂：宿主没动就没有合并意志。
    assert!(matches!(
        diverged_verdict(&e, Some("HOST_OLD"), None),
        super::DivergedVerdict::KeepParked
    ));
}

#[test]
fn diverged_host_converged_to_vm_releases() {
    // 宿主改成了 VM 的内容（或被外部同步）= 无事解挂。
    let e = parked_entry("HOST_OLD", "VM1");
    assert!(matches!(
        diverged_verdict(&e, Some("VM1"), Some("VM1")),
        super::DivergedVerdict::ReleaseConverged
    ));
}

#[test]
fn diverged_host_deleted_is_release_merged() {
    // 宿主删掉挂起文件 = 合并意志（采纳删除）——解挂后走 Delete 分类：
    // VM 仍在 → DeleteOnVm；VM 也没了 → Skip，两条路都收敛。
    let e = parked_entry("HOST_OLD", "VM1");
    assert!(matches!(
        diverged_verdict(&e, None, Some("VM1")),
        super::DivergedVerdict::ReleaseMerged
    ));
}

#[test]
fn diverged_both_gone_releases_clean() {
    // 挂起时宿主就缺文件、现在仍缺，且 VM 侧也没了 = 无事解挂。
    let e = parked_entry("", "VM1");
    assert!(matches!(
        diverged_verdict(&e, None, None),
        super::DivergedVerdict::ReleaseConverged
    ));
}

#[test]
fn diverged_parked_host_missing_then_host_appears_is_merge() {
    // 挂起时宿主缺文件（空 host_sha），之后宿主出现了该文件（且 ≠ VM 版）
    // = 新内容 = 合并完成解挂推进。
    let e = parked_entry("", "VM1");
    assert!(matches!(
        diverged_verdict(&e, Some("HOST_NEW"), Some("VM1")),
        super::DivergedVerdict::ReleaseMerged
    ));
    // 宿主出现了文件但内容恰为 VM 版 = 收敛。
    assert!(matches!(
        diverged_verdict(&e, Some("VM1"), Some("VM1")),
        super::DivergedVerdict::ReleaseConverged
    ));
    // 宿主仍缺 = 继续挂起。
    assert!(matches!(
        diverged_verdict(&e, None, Some("VM1")),
        super::DivergedVerdict::KeepParked
    ));
}

// ---------------------------------------------------------------------------
// P5-R1：.browser-profile 排除（登录态 VM 本地；D-P6-1 可迁移形态预埋）
// ---------------------------------------------------------------------------

#[test]
fn browser_profile_excluded_uploads_admitted() {
    // guest chromium user-data-dir：cookies/缓存不进项目工作副本（体积与
    // 正确性双考虑）——登录态经 VM overlay 持久，不走同步。
    assert!(!admits(".browser-profile/Default/Cookies"));
    assert!(!admits(".browser-profile")); // 精确目录名边缘形态
    // uploads/ 刻意**不**排除：browser 截图落 <workspace>/uploads/，经
    // vm_sync 回灌宿主 → vision 评审 M5 同源。
    assert!(admits("uploads/browser-1730000000000.png"));
}

// ---------------------------------------------------------------------------
// pack_tar 错误清理臂：打包中途失败不留半截 tar 孤儿（系统 temp 唯一名会累积）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pack_tar_failure_leaves_no_orphan_tar() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("ok.txt"), b"hello").unwrap();

    // 系统 temp 中本前缀 tar 计数差值断言（pack_tar 仅生产两处调用，无并发
    // 测试竞争此前缀；绝对计数会被历史残留干扰，差值不受影响）。
    let tar_count = || {
        std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("nemesis-vm-sync-")
            })
            .count()
    };
    let before = tar_count();

    // 第二条目是目录：stat 通过后 is_file 闸失败——此时 File::create 已经
    // 落盘，修复前半截 tar 留系统 temp 成孤儿，修复后错误清理臂即删。
    let files = vec![
        FileSync {
            rel: "ok.txt".to_string(),
            mode: 0o644,
        },
        FileSync {
            rel: "sub".to_string(),
            mode: 0o755,
        },
    ];
    let result = super::pack_tar(&root, &files).await;
    assert!(result.is_err());

    assert_eq!(
        tar_count(),
        before,
        "打包失败后系统 temp 不应残留 nemesis-vm-sync-* tar"
    );
}
