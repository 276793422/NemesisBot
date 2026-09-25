// pending.rs 覆盖率补充（wave 5）：box 路径映射四臂、walk 枚举 + 错误臂、
// MAX_BOX_FILES 停止臂、commit_file 拷贝、pending_workspace 安全过滤。

use super::*;

/// real_path_for_box：user / drive / 无关前缀 / 非盒路径 四臂（30-56）。
#[test]
fn real_path_for_box_maps_user_drive_and_rejects_others() {
    let box_root = std::path::PathBuf::from(r"C:\box");
    let user_profile = std::path::PathBuf::from(r"C:\Users\u");

    let user_file = box_root.join(r"user\current\docs\note.md");
    assert_eq!(
        real_path_for_box(&user_file, &box_root, &user_profile),
        Some(user_profile.join(r"docs\note.md"))
    );

    let drive_file = box_root.join(r"drive\c\temp\x.bin");
    assert_eq!(
        real_path_for_box(&drive_file, &box_root, &user_profile),
        Some(std::path::PathBuf::from(r"C:\").join(r"temp\x.bin"))
    );

    // 盒元数据（RegHive 等）不映射。
    let hive = box_root.join("RegHive.dat");
    assert_eq!(real_path_for_box(&hive, &box_root, &user_profile), None);

    // 根本不在盒内 → strip_prefix 失败 → None。
    let outside = std::path::PathBuf::from(r"D:\elsewhere\f.txt");
    assert_eq!(real_path_for_box(&outside, &box_root, &user_profile), None);
}

/// walk + enumerate：镜像 user/drive 的文件被枚举、盒元数据被排除；
/// real_path / size 正确（58-116 主体）。
#[test]
fn enumerate_box_walks_mirror_trees_and_skips_metadata() {
    let home = tempfile::tempdir().unwrap();
    let box_root = home.path().join("box");
    let user_profile = home.path().join("profile");
    std::fs::create_dir_all(user_profile.join("docs")).unwrap();

    let f1 = box_root.join(r"user\current\docs\a.txt");
    std::fs::create_dir_all(f1.parent().unwrap()).unwrap();
    std::fs::write(&f1, b"12345").unwrap();

    let f2 = box_root.join(r"drive\c\data\b.bin");
    std::fs::create_dir_all(f2.parent().unwrap()).unwrap();
    std::fs::write(&f2, b"xy").unwrap();

    // 盒元数据：不在 user/drive 下 → 不进列表。
    std::fs::write(box_root.join("DONT-USE.TXT"), b"meta").unwrap();

    let all = enumerate_box(&box_root, &user_profile).expect("walk succeeds");
    assert_eq!(all.len(), 2, "{all:?}");

    let mut got: Vec<(String, u64)> = all
        .iter()
        .map(|p| (p.real_path.display().to_string(), p.size))
        .collect();
    got.sort();
    // real_path 排序：home 在 %TEMP%（C:\Users\...）下，'U' < 'd' → user 文件在前。
    assert_eq!(got[0].1, 5, "user file size");
    assert_eq!(got[1].1, 2, "drive file size");
    assert!(
        got.iter().all(|(p, _)| !p.contains("DONT-USE")),
        "metadata excluded: {got:?}"
    );
}

/// enumerate：盒根不存在 → 空列表不是错误（65-69）。
#[test]
fn enumerate_box_on_missing_root_returns_empty() {
    let missing = std::env::temp_dir().join(format!("nb_missing_box_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    let all = enumerate_box(&missing, std::path::Path::new(r"C:\Users\u"))
        .expect("missing root is empty, not an error");
    assert!(all.is_empty());
}

/// walk 错误臂：不可读目录 → 带 context 的 Err（73-75）。
#[test]
fn walk_surfaces_read_dir_errors_with_context() {
    let bogus = std::env::temp_dir().join(format!("nb_no_such_dir_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&bogus);
    let mut out = Vec::new();
    let err = walk(
        &bogus,
        &bogus,
        std::path::Path::new(r"C:\Users\u"),
        &mut out,
    )
    .expect_err("read_dir on missing root must fail");
    assert!(err.to_string().contains("read_dir"), "{err}");
}

/// commit_file：盒内文件拷回真路径（父目录自动创建），返回字节数（119-140）。
#[test]
fn commit_file_copies_box_content_to_real_path() {
    let home = tempfile::tempdir().unwrap();
    let box_file = home.path().join("box").join(r"user\current\out.txt");
    std::fs::create_dir_all(box_file.parent().unwrap()).unwrap();
    std::fs::write(&box_file, b"committed content").unwrap();

    let real = home.path().join("workspace").join("deep").join("out.txt");
    let pending = PendingFile {
        box_path: box_file,
        real_path: real.clone(),
        size: 17,
    };
    let n = commit_file(&pending).expect("commit copies");
    assert_eq!(n, 17);
    assert_eq!(std::fs::read(&real).unwrap(), b"committed content");
}

/// pending_workspace：只返回工作区子树的安全过滤（145-160）。
#[test]
fn pending_workspace_filters_to_workspace_subtree() {
    let home = tempfile::tempdir().unwrap();
    let box_root = home.path().join("box");
    let user_profile = home.path().join("profile");
    let workspace = user_profile.join("workspace");
    std::fs::create_dir_all(workspace.join("src")).unwrap();

    let inside = box_root.join(r"user\current\workspace\src\in.rs");
    std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
    std::fs::write(&inside, b"in").unwrap();

    let outside = box_root.join(r"user\current\elsewhere\out.rs");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(&outside, b"out").unwrap();

    let scoped = pending_workspace(&box_root, &workspace, &user_profile).expect("scoped listing");
    assert_eq!(scoped.len(), 1, "{scoped:?}");
    assert!(
        scoped[0].real_path.starts_with(&workspace),
        "only workspace files: {scoped:?}"
    );
}
