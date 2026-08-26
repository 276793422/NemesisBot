use super::*;
use tempfile::TempDir;

/// Build a temp box tree with one mirrored file: `<box>/drive/C/tmp/a.txt`.
fn one_pending() -> (TempDir, PendingFile) {
    let tmp = TempDir::new().unwrap();
    let box_root = tmp.path().to_path_buf();
    let file = box_root.join("drive").join("C").join("tmp").join("a.txt");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"hello").unwrap();
    let pf = PendingFile {
        box_path: file,
        real_path: PathBuf::from(r"C:\tmp\a.txt"),
        size: 5,
    };
    (tmp, pf)
}

#[test]
fn delete_file_removes_box_file() {
    let (_tmp, pf) = one_pending();
    assert!(pf.box_path.exists());
    assert_eq!(delete_file(&pf).unwrap(), true);
    assert!(!pf.box_path.exists());
}

#[test]
fn delete_file_already_gone_is_false() {
    let (_tmp, pf) = one_pending();
    std::fs::remove_file(&pf.box_path).unwrap();
    // Already absent → Ok(false), NOT an error.
    assert_eq!(delete_file(&pf).unwrap(), false);
}

#[test]
fn delete_file_never_touches_real_path() {
    // Real path doesn't even exist in this temp setup — deleting the box
    // file must not create or modify anything at the real path.
    let (_tmp, pf) = one_pending();
    assert!(!pf.real_path.exists());
    assert_eq!(delete_file(&pf).unwrap(), true);
    assert!(!pf.real_path.exists());
}

// ---------------------------------------------------------------------------
// Phase 3 覆盖率（2026-08-25）：real_path_for_box 映射 / enumerate_box 遍历 /
// pending_workspace **安全过滤器**（工作区外的破坏永不进列表 = 沙盒防泄漏
// 命门）/ commit_file 落盘。全部 tempdir 内构造假盒布局，不依赖 Sandboxie。
// ---------------------------------------------------------------------------

fn make_box(tmp: &TempDir) -> std::path::PathBuf {
    let box_root = tmp.path().join("box");
    std::fs::create_dir_all(box_root.join("user").join("current").join("ws")).unwrap();
    std::fs::create_dir_all(box_root.join("drive").join("C").join("Windows")).unwrap();
    std::fs::create_dir_all(box_root.join("drive").join("C").join("proj").join("src")).unwrap();
    std::fs::write(box_root.join("user").join("current").join("ws").join("a.txt"), b"aaa").unwrap();
    std::fs::write(
        box_root.join("drive").join("C").join("Windows").join("evil.dll"),
        b"pwn",
    )
    .unwrap();
    std::fs::write(
        box_root.join("drive").join("C").join("proj").join("src").join("m.rs"),
        b"fn main(){}",
    )
    .unwrap();
    // 盒元数据：必须被自然排除（不在 user/drive 下）。
    std::fs::write(box_root.join("RegHive"), b"hive").unwrap();
    std::fs::write(box_root.join("DONT-USE.TXT"), b"meta").unwrap();
    box_root
}

#[test]
fn real_path_for_box_maps_user_drive_and_rejects_metadata() {
    let tmp = TempDir::new().unwrap();
    let box_root = make_box(&tmp);
    let up = Path::new(r"C:\Users\zoo");

    // user/<marker>/<rest> → %USERPROFILE%\<rest>
    assert_eq!(
        real_path_for_box(&box_root.join("user").join("current").join("ws").join("a.txt"), &box_root, up),
        Some(up.join("ws").join("a.txt"))
    );
    // drive/<L>/<rest> → <L>:\<rest>
    assert_eq!(
        real_path_for_box(&box_root.join("drive").join("C").join("proj").join("src").join("m.rs"), &box_root, up),
        Some(PathBuf::from(r"C:\proj\src\m.rs"))
    );
    // 盒元数据 → None。
    assert_eq!(real_path_for_box(&box_root.join("RegHive"), &box_root, up), None);
    assert_eq!(real_path_for_box(&box_root.join("DONT-USE.TXT"), &box_root, up), None);
    // 前缀不在 box_root 内 → None。
    assert_eq!(real_path_for_box(Path::new(r"C:\elsewhere\a"), &box_root, up), None);
    // user 下没有 marker 段 → None。
    assert_eq!(real_path_for_box(&box_root.join("user"), &box_root, up), None);
}

#[test]
fn enumerate_box_walks_all_mirrored_files_and_skips_metadata() {
    let tmp = TempDir::new().unwrap();
    let box_root = make_box(&tmp);
    let up = Path::new(r"C:\Users\zoo");
    let files = enumerate_box(&box_root, up).unwrap();
    let paths: Vec<String> = files.iter().map(|f| f.real_path.to_string_lossy().to_string()).collect();
    assert_eq!(files.len(), 3, "{paths:?}");
    assert!(paths.iter().any(|p| p.ends_with("a.txt")));
    assert!(paths.iter().any(|p| p.contains("evil.dll")), "enumerate 不过滤（过滤在 pending_workspace）");
    assert!(paths.iter().any(|p| p.contains("m.rs")));
    // 元数据绝不出现。
    assert!(!paths.iter().any(|p| p.contains("RegHive")));
    assert!(!paths.iter().any(|p| p.contains("DONT-USE")));
    // size 记录了。
    assert_eq!(files.iter().find(|f| f.real_path.ends_with("a.txt")).unwrap().size, 3);
}

#[test]
fn enumerate_box_missing_root_returns_empty() {
    let tmp = TempDir::new().unwrap();
    let files = enumerate_box(&tmp.path().join("nope"), Path::new(r"C:\Users\x")).unwrap();
    assert!(files.is_empty());
}

#[test]
fn pending_workspace_scopes_to_workspace_subtree_only() {
    // 安全命门：盒内工作区外写入（C:\Windows\evil.dll）绝不能出现在
    // 可提交列表里——用户永远只看到自己工作区的写回。
    let tmp = TempDir::new().unwrap();
    let box_root = make_box(&tmp);
    let up = Path::new(r"C:\Users\zoo");
    let ws = PathBuf::from(r"C:\proj");
    let files = pending_workspace(&box_root, &ws, up).unwrap();
    assert_eq!(files.len(), 1, "只有工作区子树内的文件");
    assert!(files[0].real_path.ends_with("m.rs"));
    // user/current/ws 在 %USERPROFILE% 下，不在 C:\proj 下 → 排除。
}

#[test]
fn pending_workspace_results_sorted_by_real_path() {
    let tmp = TempDir::new().unwrap();
    let box_root = tmp.path().join("box");
    for name in ["c.txt", "a.txt", "b.txt"] {
        let p = box_root.join("drive").join("D").join("ws").join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
    }
    let files = pending_workspace(&box_root, &PathBuf::from(r"D:\ws"), Path::new(r"C:\Users\x")).unwrap();
    let names: Vec<String> = files.iter().map(|f| f.real_path.file_name().unwrap().to_string_lossy().to_string()).collect();
    assert_eq!(names, vec!["a.txt", "b.txt", "c.txt"]);
}

#[test]
fn commit_file_copies_content_and_creates_parents() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("box_file.bin");
    std::fs::write(&src, b"payload-123").unwrap();
    let dest = tmp.path().join("deep").join("nested").join("out.bin");
    let pf = PendingFile { box_path: src, real_path: dest.clone(), size: 11 };
    let n = commit_file(&pf).unwrap();
    assert_eq!(n, 11);
    assert_eq!(std::fs::read(&dest).unwrap(), b"payload-123");
}

#[test]
fn commit_file_missing_box_source_errors_with_context() {
    let tmp = TempDir::new().unwrap();
    let pf = PendingFile {
        box_path: tmp.path().join("gone.bin"),
        real_path: tmp.path().join("out.bin"),
        size: 0,
    };
    let err = commit_file(&pf).unwrap_err();
    assert!(format!("{err:#}").contains("commit"), "{err:#}");
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：walk 的 MAX 上限、
// read_dir 错误臂、delete_file 的目录错误臂、delete_box_contents 的
// 假 Start.exe 成功/失败/缺失三臂。
// ---------------------------------------------------------------------------

#[test]
fn walk_caps_output_at_max_box_files() {
    let tmp = TempDir::new().unwrap();
    let box_root = tmp.path().to_path_buf();
    let ws = box_root.join("drive").join("C").join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    // MAX_BOX_FILES=5000：多造 50 个，断言恰好停在 5000（上限防大盒卡顿）
    for i in 0..5050 {
        std::fs::write(ws.join(format!("f{i:05}.txt")), b"x").unwrap();
    }
    let mut out = Vec::new();
    walk(&box_root, &box_root, Path::new(r"C:\Users\x"), &mut out).unwrap();
    assert_eq!(out.len(), 5000, "必须恰好在 MAX_BOX_FILES 截断");
}

#[test]
fn walk_read_dir_error_propagates_with_context() {
    // dir 是文件而非目录 → read_dir Err（非 PermissionDenied）→ 带 context 传播
    let tmp = TempDir::new().unwrap();
    let not_a_dir = tmp.path().join("file.txt");
    std::fs::write(&not_a_dir, b"x").unwrap();
    let mut out = Vec::new();
    let err = walk(&not_a_dir, tmp.path(), Path::new(r"C:\Users\x"), &mut out).unwrap_err();
    assert!(format!("{err:#}").contains("read_dir"), "{err:#}");
}

#[test]
fn walk_permission_denied_dir_is_skipped_not_fatal() {
    // Windows：icacls 给 Everyone 加 deny ACE（SID S-1-1-0 免本地化）。
    // 任何一步 icacls 失败（容器/精简系统）就跳过——该臂按机器依赖分类。
    let tmp = TempDir::new().unwrap();
    let denied = tmp.path().join("denied");
    std::fs::create_dir_all(&denied).unwrap();
    std::fs::write(denied.join("inside.txt"), b"x").unwrap();
    let deny = std::process::Command::new("icacls")
        .arg(&denied)
        .args(["/deny", "*S-1-1-0:(OI)(CI)F"])
        .output();
    let applied = matches!(&deny, Ok(o) if o.status.success());
    if !applied {
        eprintln!("skip: icacls deny 不可用");
        return;
    }
    // 正常兄弟目录可枚举，deny 目录被跳过（Ok 而非 Err）
    let sibling = tmp.path().join("drive").join("C").join("ok.txt");
    std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
    std::fs::write(&sibling, b"x").unwrap();
    let mut out = Vec::new();
    let r = walk(tmp.path(), tmp.path(), Path::new(r"C:\Users\x"), &mut out);
    // 清理：owner 恒可改 DACL——移除 deny ACE 再删
    let _ = std::process::Command::new("icacls").arg(&denied).args(["/remove:d", "*S-1-1-0"]).output();
    let _ = std::process::Command::new("icacls").arg(&denied).args(["/reset"]).output();
    let _ = std::fs::remove_dir_all(&denied);

    assert!(r.is_ok(), "PermissionDenied 目录必须跳过而非 Err: {:?}", r.map_err(|e| format!("{e:#}")));
    assert!(out.iter().any(|f| f.box_path.ends_with("ok.txt")), "兄弟文件仍要枚举到");
    assert!(!out.iter().any(|f| f.box_path.ends_with("inside.txt")), "deny 目录内的文件不出现");
}

#[test]
fn delete_file_box_path_is_directory_errors() {
    // remove_file 指向目录 → Windows ERROR_ACCESS_DENIED → 非 NotFound 的
    // Err 臂（带 context），而非 panic / false
    let tmp = TempDir::new().unwrap();
    let dir_path = tmp.path().join("drive").join("C").join("subdir");
    std::fs::create_dir_all(&dir_path).unwrap();
    let pf = PendingFile {
        box_path: dir_path,
        real_path: PathBuf::from(r"C:\subdir"),
        size: 0,
    };
    let err = delete_file(&pf).unwrap_err();
    assert!(format!("{err:#}").contains("delete box file"), "{err:#}");
}

#[cfg(windows)]
fn fake_start_exe(dir: &std::path::Path, exit_code: u32) -> std::path::PathBuf {
    let p = dir.join("Start.exe.bat");
    std::fs::write(
        &p,
        format!("@echo off\r\necho start-exe-noise 1>&2\r\nexit /b {exit_code}\r\n"),
    )
    .unwrap();
    p
}

#[cfg(windows)]
#[test]
fn delete_box_contents_fake_start_exe_all_three_outcomes() {
    let tmp = TempDir::new().unwrap();
    // ① exit 0 → Ok
    assert!(delete_box_contents(&fake_start_exe(tmp.path(), 0), "NemesisBox").is_ok());
    // ② exit 2 → bail 且带 stderr
    let err = delete_box_contents(&fake_start_exe(tmp.path(), 2), "NemesisBox").unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("delete_sandbox failed"), "{msg}");
    assert!(msg.contains("start-exe-noise"), "stderr 必须带回: {msg}");
    // ③ Start.exe 缺失 → spawn Err 带 context
    let err2 = delete_box_contents(&tmp.path().join("missing").join("Start.exe"), "NemesisBox").unwrap_err();
    assert!(format!("{err2:#}").contains("spawn Start.exe delete_sandbox"), "{err2:#}");
}
