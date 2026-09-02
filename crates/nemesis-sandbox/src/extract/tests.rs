use super::*;

/// Download 7z.zip from the project GitHub + unzip → 7z.exe/7z.dll appear.
/// This exercises the download path directly (bypasses the system-7z check),
/// validating it for users who don't have 7-Zip pre-installed.
#[tokio::test]
#[ignore = "requires network (downloads 7z from GitHub); run via `cargo test -p nemesis-sandbox download_and_unzip -- --ignored` or the sandbox e2e workflow"]
async fn download_and_unzip_7z_brings_7z_exe_into_runtime() {
    let tmp = tempfile::tempdir().unwrap();
    download_and_unzip_7z(tmp.path())
        .await
        .expect("download + unzip should succeed");
    let exe = tmp.path().join("7z").join("7z.exe");
    let dll = tmp.path().join("7z").join("7z.dll");
    assert!(exe.exists(), "7z.exe missing at {}", exe.display());
    assert!(dll.exists(), "7z.dll missing at {}", dll.display());
    // the zip itself should have been cleaned up.
    assert!(
        !tmp.path().join("7z.zip").exists(),
        "7z.zip should be removed after unzip"
    );
}

// ---------------------------------------------------------------------------
// Phase 3 覆盖率（2026-08-25）：cached 7z 分支 / seven_zip_status 探测 /
// extract 的假 7z 执行（成功 + 失败带 stderr）。下载分支走上面的 ignored
// 网络测试，不在此重复。
// ---------------------------------------------------------------------------

#[test]
fn seven_zip_status_cached_when_runtime_has_exe() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join("7z").join("7z.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    assert_eq!(seven_zip_status(tmp.path()), (true, "cached"));
}

#[test]
fn seven_zip_status_no_cache_reports_system_or_none() {
    let tmp = tempfile::tempdir().unwrap();
    let (ok, src) = seven_zip_status(tmp.path());
    // 机器上可能装有系统 7-Zip——两种结果都合法，但 tuple 必须自洽。
    match src {
        "system" => assert!(ok, "system 分支必须 ok=true"),
        "none" => assert!(!ok, "none 分支必须 ok=false"),
        other => panic!("未知来源 {other}"),
    }
}

#[tokio::test]
async fn resolve_seven_zip_prefers_cached_without_network() {
    let tmp = tempfile::tempdir().unwrap();
    let exe = tmp.path().join("7z").join("7z.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"").unwrap();
    let got = resolve_seven_zip(tmp.path()).await.unwrap();
    assert_eq!(got, exe, "cached 7z 存在时绝不能触发下载分支");
}

/// 造假 7z：把 args 写进 marker，`ok` 决定退出码；ok 时同时产出一个
/// "解压出的" 文件证明调用真发生过。
fn fake_7z(dir: &Path, ok: bool) -> std::path::PathBuf {
    let marker = dir.join("7z_args.txt");
    if cfg!(windows) {
        let p = dir.join("fake7z.bat");
        let m = marker.to_string_lossy().replace('\\', "/");
        std::fs::write(
            &p,
            format!(
                "@echo off\r\necho %* > \"{m}\"\r\necho seven-zip-stderr-noise 1>&2\r\nexit /b {}\r\n",
                if ok { 0 } else { 2 }
            ),
        )
        .unwrap();
        p
    } else {
        let p = dir.join("fake7z.sh");
        let m = marker.to_string_lossy();
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\necho \"$@\" > \"{m}\"\necho seven-zip-stderr-noise 1>&2\nexit {}\n",
                if ok { 0 } else { 2 }
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }
}

#[test]
fn extract_success_returns_ok_and_passes_flags() {
    let tmp = tempfile::tempdir().unwrap();
    let seven_zip = fake_7z(tmp.path(), true);
    let installer = tmp.path().join("Sandboxie.exe");
    std::fs::write(&installer, b"installer").unwrap();
    let out_dir = tmp.path().join("runtime");
    std::fs::create_dir_all(&out_dir).unwrap();

    extract(&installer, &out_dir, &seven_zip).unwrap();
    let args = std::fs::read_to_string(tmp.path().join("7z_args.txt")).unwrap();
    let norm = args.replace('\\', "/");
    assert!(norm.contains("x "), "解压动词：{norm}");
    assert!(norm.contains("Sandboxie.exe"), "{norm}");
    assert!(norm.contains("-o"), "输出目录参数：{norm}");
    assert!(norm.contains("-y"), "yes-to-all：{norm}");
}

#[test]
fn extract_failure_bails_with_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let seven_zip = fake_7z(tmp.path(), false);
    let installer = tmp.path().join("Sandboxie.exe");
    std::fs::write(&installer, b"installer").unwrap();
    let err = extract(&installer, tmp.path(), &seven_zip).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("7z extraction failed"), "{msg}");
    assert!(
        msg.contains("seven-zip-stderr-noise"),
        "stderr 必须带回：{msg}"
    );
}

#[test]
fn extract_missing_7z_binary_is_err() {
    let tmp = tempfile::tempdir().unwrap();
    let installer = tmp.path().join("Sandboxie.exe");
    std::fs::write(&installer, b"installer").unwrap();
    let err = extract(
        &installer,
        tmp.path(),
        Path::new(r"Z:\definitely\missing\7z.exe"),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("spawn"), "{err:#}");
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：extract_release 走
// cached 假 7z（绝不触发下载红线）；subscriber 下让 extract 的 info! 参数行
// 真实求值。
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[tokio::test]
async fn extract_release_with_cached_7z_propagates_extraction_error() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    // runtime/7z/7z.exe 放一个真 PE（where.exe 副本）→ resolve 命中 cached 臂，
    // 绝不进下载分支；where 收到 "x <installer> -o<dir> -y" 找不到文件必然
    // 非零退出 → extract_release 把 extract 的失败原样传播
    // （同时覆盖 resolve→extract 串联）。
    let seven_zip_dir = tmp.path().join("7z");
    std::fs::create_dir_all(&seven_zip_dir).unwrap();
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let real_where = std::path::Path::new(&windir)
        .join("System32")
        .join("where.exe");
    std::fs::copy(&real_where, seven_zip_dir.join("7z.exe")).unwrap();
    let installer = tmp.path().join("Sandboxie-Classic-fake.exe");
    std::fs::write(&installer, b"fake-installer").unwrap();

    let err = extract_release(&installer, tmp.path()).await.unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("7z extraction failed"), "{msg}");
}

#[cfg(unix)]
#[tokio::test]
async fn extract_release_with_cached_7z_propagates_extraction_error() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let seven_zip_dir = tmp.path().join("7z");
    std::fs::create_dir_all(&seven_zip_dir).unwrap();
    std::fs::write(seven_zip_dir.join("7z.exe"), "#!/bin/sh\nexit 3\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            seven_zip_dir.join("7z.exe"),
            // `Permissions` struct lives in std::fs; `from_mode` comes from
            // the PermissionsExt trait imported above. (The re-export under
            // std::os::unix::fs is private — E0603 on Linux.)
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let installer = tmp.path().join("Sandboxie-Classic-fake.exe");
    std::fs::write(&installer, b"fake-installer").unwrap();

    let err = extract_release(&installer, tmp.path()).await.unwrap_err();
    assert!(format!("{err:#}").contains("7z extraction failed"));
}

// ---------------------------------------------------------------------------
// R5 覆盖率批次（2026-08-27）：resolve_seven_zip 的 system 臂（机器依赖
// 守卫：本机装有系统 7-Zip 才测；干净机器会走网络下载分支，本测试绝不
// 联网）。cached 臂已由上面 prefers_cached 测试钉住。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolve_seven_zip_uses_system_7z_when_no_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let (ok, src) = seven_zip_status(tmp.path());
    if src != "system" {
        eprintln!("skip: 本机无系统 7z（ok={ok}, src={src}），system 臂不可离线测");
        return;
    }
    let p = resolve_seven_zip(tmp.path()).await.unwrap();
    assert_ne!(
        p,
        tmp.path().join("7z").join("7z.exe"),
        "无缓存必不返回 cached 路径"
    );
    assert!(p.exists(), "系统 7z 路径必须真实存在: {}", p.display());
}
