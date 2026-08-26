//! Tests for `bootstrap`（S12 覆盖率冲刺，2026-08-26）。
//!
//! 可达面：常量/纯函数（format_speed）、目录布局探测（find_file / find_lib_dir
//! 四种布局 + bail）、文件操作（copy_libs_from / dir_has_any_target_lib）、
//! fail-fast（init_sherpa 缺主库 bail、run_in_dir 全在场→init 失败于哑 DLL）、
//! 幂等早退（download_aec_lib 已存在 aec.dll → 不碰网络）。
//!
//! 结构性豁免（本文件不测，最终报告逐条列）：
//! - download_runtime_libs / try_download_and_extract：URL 硬编码
//!   github.com / hf-mirror.com（bootstrap 无镜像 seam，不像 model.rs 走
//!   cfg.models.mirror.base），真网络禁。
//! - try_download_aec / download_to 的下载臂：同理，URL 硬编码。
//! - run()（76-79）：exe 目录 = 测试二进制目录，必缺 DLL → 触发真下载。

use super::*;

fn touch(p: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// 造一个所有必需库都在场的目录（内容是哑字节——sherpa::init 会加载失败，
/// 这正好覆盖「在场→走到 init→init 报错」路径，不碰网络）。
fn dir_with_all_libs(tmp: &Path) -> PathBuf {
    for lib in REQUIRED_LIBS {
        touch(&tmp.join(lib), b"not-a-real-dll");
    }
    tmp.to_path_buf()
}

// ---------------------------------------------------------------------------
// required_lib_names / default_config_toml
// ---------------------------------------------------------------------------

#[test]
fn required_lib_names_lists_windows_runtime_dlls() {
    let libs = required_lib_names();
    assert!(libs.contains(&"sherpa-onnx-c-api.dll"), "{libs:?}");
    assert!(libs.contains(&"onnxruntime.dll"), "{libs:?}");
    assert!(libs.contains(&"onnxruntime_providers_shared.dll"), "{libs:?}");
}

#[test]
fn default_config_toml_contains_expected_sections() {
    let toml = default_config_toml();
    assert!(toml.contains("[stt]"), "missing [stt]");
    assert!(toml.contains("[tts]"), "missing [tts]");
    assert!(toml.contains("[models]"), "missing [models]");
}

// ---------------------------------------------------------------------------
// init_sherpa —— 缺主库 fail-fast
// ---------------------------------------------------------------------------

#[test]
fn init_sherpa_missing_main_lib_bails_with_hint() {
    let tmp = tempfile::tempdir().unwrap();
    // 空目录 → sherpa-onnx-c-api.dll 不存在
    let err = format!("{:#}", init_sherpa(tmp.path()).unwrap_err());
    assert!(err.contains("Voice runtime not found"), "{err}");
    assert!(err.contains("voice setup"), "{err}");
}

// ---------------------------------------------------------------------------
// run_in_dir —— 全在场（哑 DLL）→ config 创建 + init 失败于哑 DLL
// ---------------------------------------------------------------------------

#[test]
fn run_in_dir_creates_config_then_init_fails_on_dummy_dll() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_dir = dir_with_all_libs(&tmp.path().join("libs"));
    let config_path = tmp.path().join("config.toml");

    let res = run_in_dir(&config_path, &lib_dir);
    // 哑 DLL 不是合法 PE → sherpa::init 必失败；但证明走到了 init（=库全在场分支）
    let err = format!("{:#}", res.unwrap_err());
    assert!(!err.contains("Voice runtime not found"), "{err}");

    // config 被创建且内容是默认模板
    let written = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(written, DEFAULT_CONFIG);
}

#[test]
fn run_in_dir_existing_config_is_not_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_dir = dir_with_all_libs(&tmp.path().join("libs"));
    let config_path = tmp.path().join("config.toml");
    // 预置自定义 config（合法 TOML——run_in_dir 会 load_or_default 读它取 proxy）
    std::fs::write(&config_path, "# custom marker config\n[models]\nauto_download = false\n")
        .unwrap();

    let _ = run_in_dir(&config_path, &lib_dir);
    let after = std::fs::read_to_string(&config_path).unwrap();
    assert!(after.contains("# custom marker config"), "config overwritten: {after}");
}

// ---------------------------------------------------------------------------
// format_speed —— 纯函数三分支
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "windows", feature = "download"))]
mod speed {
    use super::super::format_speed;

    #[test]
    fn mb_scale() {
        assert_eq!(format_speed(2.5 * 1024.0 * 1024.0), "2.5 MB");
    }

    #[test]
    fn kb_scale() {
        assert_eq!(format_speed(2048.0), "2 KB");
        // 边界：1 MiB - 1 → KB 档
        assert_eq!(format_speed(1024.0 * 1024.0 - 1.0), "1024 KB");
    }

    #[test]
    fn byte_scale() {
        assert_eq!(format_speed(512.0), "512 B");
        assert_eq!(format_speed(0.0), "0 B");
    }
}

// ---------------------------------------------------------------------------
// find_file —— 递归查找
// ---------------------------------------------------------------------------

#[test]
fn find_file_locates_nested_file() {
    let tmp = tempfile::tempdir().unwrap();
    let nested = tmp.path().join("a").join("b").join("c");
    touch(&nested.join("aec.dll"), b"x");

    let got = find_file(tmp.path(), "aec.dll").unwrap();
    assert_eq!(got, Some(nested.join("aec.dll")));
}

#[test]
fn find_file_missing_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("other.txt"), b"x");
    assert_eq!(find_file(tmp.path(), "aec.dll").unwrap(), None);
}

#[test]
fn find_file_empty_root_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(find_file(tmp.path(), "anything.dll").unwrap(), None);
}

// ---------------------------------------------------------------------------
// find_lib_dir —— 四种解压布局 + bail
// ---------------------------------------------------------------------------

#[test]
fn find_lib_dir_primary_layout() {
    // {extract}/{SHERPA_RELEASE_NAME}/lib
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join(SHERPA_RELEASE_NAME).join("lib");
    touch(&lib.join("sherpa-onnx-c-api.dll"), b"x");
    assert_eq!(find_lib_dir(tmp.path()).unwrap(), lib);
}

#[test]
fn find_lib_dir_secondary_layout() {
    // {extract}/lib（无版本子目录）
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    touch(&lib.join("onnxruntime.dll"), b"x");
    assert_eq!(find_lib_dir(tmp.path()).unwrap(), lib);
}

#[test]
fn find_lib_dir_nested_one_level_layout() {
    // {extract}/{任意目录}/lib（含目标库）
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("whatever-pkg").join("lib");
    touch(&lib.join("sherpa-onnx-c-api.dll"), b"x");
    assert_eq!(find_lib_dir(tmp.path()).unwrap(), lib);
}

#[test]
fn find_lib_dir_nested_two_level_layout() {
    // {extract}/{目录}/{子目录}/lib（含目标库）
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("outer").join("inner").join("lib");
    touch(&lib.join("onnxruntime_providers_shared.dll"), b"x");
    assert_eq!(find_lib_dir(tmp.path()).unwrap(), lib);
}

#[test]
fn find_lib_dir_no_lib_anywhere_bails() {
    let tmp = tempfile::tempdir().unwrap();
    // 有目录但没有 lib/ 子目录、也没有目标库
    touch(&tmp.path().join("junk").join("readme.txt"), b"x");
    let err = format!("{:#}", find_lib_dir(tmp.path()).unwrap_err());
    assert!(err.contains("Could not find lib/"), "{err}");
}

#[test]
fn find_lib_dir_lib_dir_without_target_libs_is_skipped() {
    // nested lib/ 存在但不含任何目标库 → 不采纳，继续扫 → bail
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join("pkg").join("lib").join("unrelated.dll"), b"x");
    let err = format!("{:#}", find_lib_dir(tmp.path()).unwrap_err());
    assert!(err.contains("Could not find lib/"), "{err}");
}

// ---------------------------------------------------------------------------
// dir_has_any_target_lib / copy_libs_from
// ---------------------------------------------------------------------------

#[test]
fn dir_has_any_target_lib_detects_any_required_lib() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(!dir_has_any_target_lib(tmp.path()));
    touch(&tmp.path().join("onnxruntime.dll"), b"x");
    assert!(dir_has_any_target_lib(tmp.path()));
}

#[test]
fn copy_libs_from_copies_all_required_libs() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    for lib in REQUIRED_LIBS {
        touch(&src.join(lib), b"dll-bytes");
    }
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    copy_libs_from(&src, &dst).unwrap();
    for lib in REQUIRED_LIBS {
        assert_eq!(std::fs::read(dst.join(lib)).unwrap(), b"dll-bytes");
    }
}

#[test]
fn copy_libs_from_missing_lib_bails_with_name() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    touch(&src.join(REQUIRED_LIBS[0]), b"x");
    // 其余必需库缺失
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&dst).unwrap();
    let err = format!("{:#}", copy_libs_from(&src, &dst).unwrap_err());
    assert!(err.contains("Required library not found in archive"), "{err}");
    assert!(err.contains(REQUIRED_LIBS[1]), "{err}");
}

// ---------------------------------------------------------------------------
// exe_dir
// ---------------------------------------------------------------------------

#[test]
fn exe_dir_returns_existing_directory() {
    let dir = exe_dir().unwrap();
    assert!(dir.is_dir(), "exe_dir not a dir: {}", dir.display());
}

// ---------------------------------------------------------------------------
// download_aec_lib —— 幂等早退（aec.dll 已在场 → 不碰网络）
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "windows", feature = "download"))]
#[test]
fn download_aec_lib_existing_dll_returns_early_without_network() {
    let tmp = tempfile::tempdir().unwrap();
    let existing = tmp.path().join("aec.dll");
    std::fs::write(&existing, b"already-there").unwrap();

    let got = download_aec_lib(tmp.path(), "").unwrap();
    assert_eq!(got, existing);
    // 内容原样（没被重下覆盖）
    assert_eq!(std::fs::read(&existing).unwrap(), b"already-there");
}
