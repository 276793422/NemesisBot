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
    assert!(
        libs.contains(&"onnxruntime_providers_shared.dll"),
        "{libs:?}"
    );
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

// ===========================================================================
// R6 覆盖率批次（2026-08-27）：init_sherpa 哑 DLL 臂 + run() 组装路径
// ===========================================================================

#[test]
fn init_sherpa_dummy_dll_fails_past_existence_check() {
    // 90-91 行：主库在场（哑字节）→ 走到 sherpa::init → 加载失败 Err
    let tmp = tempfile::tempdir().unwrap();
    touch(&tmp.path().join(REQUIRED_LIBS[0]), b"not-a-real-dll");
    let err = format!("{:#}", init_sherpa(tmp.path()).unwrap_err());
    assert!(!err.contains("Voice runtime not found"), "{err}");
}

#[test]
fn run_with_prepopulated_dummy_libs_reaches_init_and_fails() {
    // 76-79 行：run() = exe_dir() + run_in_dir。测试二进制目录预置哑 DLL
    // → all_present → 跳过下载（不碰网络）→ init 失败于哑 DLL → Err。
    // Drop 守卫确保哑文件清理，不长期污染 target 目录。
    struct Cleanup(Vec<PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for p in &self.0 {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    let exe = exe_dir().unwrap();
    let mut guard = Cleanup(Vec::new());
    for lib in REQUIRED_LIBS {
        let p = exe.join(lib);
        if !p.exists() {
            std::fs::write(&p, b"dummy-for-run-test").unwrap();
            guard.0.push(p);
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.toml");
    let err = format!("{:#}", run(&config_path).unwrap_err());
    assert!(!err.contains("Voice runtime not found"), "{err}");
    assert!(!err.contains("Downloading"), "{err}");
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
    std::fs::write(
        &config_path,
        "# custom marker config\n[models]\nauto_download = false\n",
    )
    .unwrap();

    let _ = run_in_dir(&config_path, &lib_dir);
    let after = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        after.contains("# custom marker config"),
        "config overwritten: {after}"
    );
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
    touch(
        &tmp.path().join("pkg").join("lib").join("unrelated.dll"),
        b"x",
    );
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
    assert!(
        err.contains("Required library not found in archive"),
        "{err}"
    );
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

// ===========================================================================
// Wave4 覆盖批次（2026-09-25）：下载路径真覆盖。
//
// 上一版把 download_runtime_libs / try_download_and_extract / try_download_aec
// / download_to 全列结构性豁免（URL 硬编码真网络）。但这些函数的 URL 都是
// **参数**或可被 proxy 打偏——用两条确定性通路即可真覆盖，不碰真网络：
// 1. wiremock 本地假源（127.0.0.1 随机端口）：喂真 tar/bz2 字节 → 全成功臂。
// 2. 不可达代理 http://127.0.0.1:9（连接拒绝，立即失败）：覆盖失败臂 + bail。
//
// tar 归档在测试里用 PATH 上的 tar 现场打包（生产解压同样依赖 PATH 上的
// tar——对称：创建能行解压就能行）。bz2 打包失败的环境（无 bz2 支持）跳过
// 成功臂测试（SKIP 约定：eprintln + return），失败臂测试不受影响。
// ===========================================================================

#[cfg(all(target_os = "windows", feature = "download"))]
mod download_paths {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 共享临时下载目录（%TEMP%/nemesis-voice-setup、nemesis-voice-aec-setup）
    /// 是全局路径——并行测试互相踩，必须串行 + 前后清理。
    static DOWNLOAD_TMP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn voice_setup_tmp() -> std::path::PathBuf {
        std::env::temp_dir().join("nemesis-voice-setup")
    }

    fn aec_setup_tmp() -> std::path::PathBuf {
        std::env::temp_dir().join("nemesis-voice-aec-setup")
    }

    fn clean_all_setup_tmps() {
        let _ = std::fs::remove_dir_all(voice_setup_tmp());
        let _ = std::fs::remove_dir_all(aec_setup_tmp());
    }

    /// current_thread runtime 上 block_on（wiremock server 与被测 async fn
    /// 同一驱动——照抄 model/tests.rs 的工程约束）。
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fut)
    }

    /// 用 PATH 上的 tar 现场打归档（生产 `try_download_*` 解压用的就是同一个
    /// tar）。返回 false = 该环境打不出归档（如无 bz2 支持）→ 调用方 SKIP。
    fn make_tar(dst: &Path, src_dir: &Path, entries: &[&str], bz2: bool) -> bool {
        let _ = std::fs::remove_file(dst);
        let mut cmd = std::process::Command::new("tar");
        cmd.arg(if bz2 { "-cjf" } else { "-cf" })
            .arg(dst)
            .arg("-C")
            .arg(src_dir);
        for e in entries {
            cmd.arg(e);
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        matches!(
            cmd.output(),
            Ok(out)
                if out.status.success()
                    && dst.exists()
                    && std::fs::metadata(dst).map(|m| m.len() > 0).unwrap_or(false)
        )
    }

    // -------------------------------------------------------------------
    // download_runtime_libs —— 不可达代理 → 双 URL 全失败 → bail
    // -------------------------------------------------------------------

    #[test]
    fn download_runtime_libs_unreachable_proxy_bails_with_sources_failed() {
        let tmp = tempfile::tempdir().unwrap();
        // 127.0.0.1:9（discard 端口，连接拒绝，立即失败不超时）
        let err = format!(
            "{:#}",
            download_runtime_libs(tmp.path(), "http://127.0.0.1:9").unwrap_err()
        );
        assert!(err.contains("All download sources failed"), "{err}");
        // 提示文案带必需库清单
        assert!(err.contains(REQUIRED_LIBS[0]), "{err}");
        assert!(err.contains("manually"), "{err}");
    }

    // -------------------------------------------------------------------
    // download_aec_lib —— 不可达代理 → bail（带手动安装提示）
    // -------------------------------------------------------------------

    #[test]
    fn download_aec_lib_unreachable_proxy_bails_with_hint() {
        let _g = DOWNLOAD_TMP_LOCK.lock().unwrap();
        clean_all_setup_tmps();
        let tmp = tempfile::tempdir().unwrap();
        let err = format!(
            "{:#}",
            download_aec_lib(tmp.path(), "http://127.0.0.1:9").unwrap_err()
        );
        assert!(err.contains("[aec] All download sources failed"), "{err}");
        assert!(err.contains(AEC_WIN_ARTIFACT), "{err}");
        assert!(err.contains(AEC_LIB_FILENAME), "{err}");
    }

    // -------------------------------------------------------------------
    // try_download_aec —— wiremock 全成功臂（下载 → .part 改名 → tar 解压
    // → find_file → 拷贝 aec.dll）
    // -------------------------------------------------------------------

    #[test]
    fn try_download_aec_full_success_installs_dll() {
        let _g = DOWNLOAD_TMP_LOCK.lock().unwrap();
        clean_all_setup_tmps();

        // 现场打包：libaec-win-x86-64/aec.dll
        let src = tempfile::tempdir().unwrap();
        let pkg = src.path().join("libaec-win-x86-64");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("aec.dll"), b"dummy-aec-bytes").unwrap();

        let aec_tmp = aec_setup_tmp();
        std::fs::create_dir_all(&aec_tmp).unwrap();
        let archive = aec_tmp.join(AEC_WIN_ARTIFACT);
        if !make_tar(&archive, src.path(), &["libaec-win-x86-64"], false) {
            eprintln!("SKIP: tar 不可用（本环境打不出归档）");
            return;
        }
        let serve_bytes = std::fs::read(&archive).unwrap();

        let dst = tempfile::tempdir().unwrap();
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_raw(serve_bytes, "application/octet-stream"),
                )
                .mount(&server)
                .await;
            let url = format!("{}/{}", server.uri(), AEC_WIN_ARTIFACT);

            let got = try_download_aec(&url, dst.path(), "").await.unwrap();
            assert_eq!(got, dst.path().join(AEC_LIB_FILENAME));
            assert_eq!(std::fs::read(&got).unwrap(), b"dummy-aec-bytes");
        });

        clean_all_setup_tmps();
    }

    // -------------------------------------------------------------------
    // try_download_aec —— 缓存命中臂（归档已在 → 不下载，直接解压）
    // -------------------------------------------------------------------

    #[test]
    fn try_download_aec_cached_archive_skips_download() {
        let _g = DOWNLOAD_TMP_LOCK.lock().unwrap();
        clean_all_setup_tmps();

        let src = tempfile::tempdir().unwrap();
        let pkg = src.path().join("libaec-win-x86-64");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("aec.dll"), b"cached-aec-bytes").unwrap();

        let aec_tmp = aec_setup_tmp();
        std::fs::create_dir_all(&aec_tmp).unwrap();
        let archive = aec_tmp.join(AEC_WIN_ARTIFACT);
        if !make_tar(&archive, src.path(), &["libaec-win-x86-64"], false) {
            eprintln!("SKIP: tar 不可用（本环境打不出归档）");
            return;
        }

        let dst = tempfile::tempdir().unwrap();
        block_on(async {
            // URL 指向不存在的服务器也没关系——走缓存臂根本不会发请求
            let got = try_download_aec("http://127.0.0.1:9/x.zip", dst.path(), "")
                .await
                .unwrap();
            assert_eq!(got, dst.path().join(AEC_LIB_FILENAME));
            assert_eq!(std::fs::read(&got).unwrap(), b"cached-aec-bytes");
        });

        clean_all_setup_tmps();
    }

    // -------------------------------------------------------------------
    // try_download_aec —— HTTP 错误臂（404 → bail，.part 不残留改名）
    // -------------------------------------------------------------------

    #[test]
    fn try_download_aec_http_404_bails() {
        let _g = DOWNLOAD_TMP_LOCK.lock().unwrap();
        clean_all_setup_tmps();

        let dst = tempfile::tempdir().unwrap();
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            let url = format!("{}/{}.zip", server.uri(), AEC_WIN_ARTIFACT);
            let err = format!(
                "{:#}",
                try_download_aec(&url, dst.path(), "").await.unwrap_err()
            );
            assert!(err.contains("HTTP 404"), "{err}");
            // 失败后归档不该被改名成最终名（还在 .part 或不存在）
            assert!(!aec_tmp_guard().join(AEC_WIN_ARTIFACT).exists());
        });

        clean_all_setup_tmps();
    }

    /// 测试内访问共享临时目录（只在 cfg(test) 下载臂测试里用）。
    fn aec_tmp_guard() -> std::path::PathBuf {
        aec_setup_tmp()
    }

    // -------------------------------------------------------------------
    // try_download_and_extract —— wiremock 全成功臂（sherpa 布局）
    // 下载 → .part 改名 → tar -xjf → find_lib_dir → copy_libs_from
    // -------------------------------------------------------------------

    #[test]
    fn try_download_and_extract_full_success_installs_runtime_libs() {
        let _g = DOWNLOAD_TMP_LOCK.lock().unwrap();
        clean_all_setup_tmps();

        // 现场 bz2 打包：{SHERPA_RELEASE_NAME}/lib/{3 DLL}
        let src = tempfile::tempdir().unwrap();
        let lib = src.path().join(SHERPA_RELEASE_NAME).join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join(REQUIRED_LIBS[0]), b"dummy-c-api").unwrap();
        std::fs::write(lib.join(REQUIRED_LIBS[1]), b"dummy-onnxrt").unwrap();
        std::fs::write(lib.join(REQUIRED_LIBS[2]), b"dummy-providers").unwrap();

        let setup_tmp = voice_setup_tmp();
        std::fs::create_dir_all(&setup_tmp).unwrap();
        let archive = setup_tmp.join(format!("{}.tar.bz2", SHERPA_RELEASE_NAME));
        if !make_tar(
            &archive,
            src.path(),
            &[&format!("{}/lib", SHERPA_RELEASE_NAME)],
            true,
        ) {
            eprintln!("SKIP: tar 无 bz2 支持（本环境打不出 bz2 归档）");
            return;
        }
        let serve_bytes = std::fs::read(&archive).unwrap();
        // 走下载臂：先清掉预置归档（wiremock 会重新喂）
        std::fs::remove_file(&archive).unwrap();

        let exe_dir = tempfile::tempdir().unwrap();
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_raw(serve_bytes, "application/octet-stream"),
                )
                .mount(&server)
                .await;
            let url = format!("{}/{}.tar.bz2", server.uri(), SHERPA_RELEASE_NAME);

            try_download_and_extract(&url, exe_dir.path(), "")
                .await
                .unwrap();
            for lib in REQUIRED_LIBS {
                let p = exe_dir.path().join(lib);
                assert!(p.exists(), "{lib} must be installed to exe dir");
            }
        });

        clean_all_setup_tmps();
    }

    // -------------------------------------------------------------------
    // try_download_and_extract —— 缓存命中臂（归档已在 → 跳过下载）
    // -------------------------------------------------------------------

    #[test]
    fn try_download_and_extract_cached_archive_skips_download() {
        let _g = DOWNLOAD_TMP_LOCK.lock().unwrap();
        clean_all_setup_tmps();

        let src = tempfile::tempdir().unwrap();
        let lib = src.path().join(SHERPA_RELEASE_NAME).join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join(REQUIRED_LIBS[0]), b"cached-c-api").unwrap();
        std::fs::write(lib.join(REQUIRED_LIBS[1]), b"cached-onnxrt").unwrap();
        std::fs::write(lib.join(REQUIRED_LIBS[2]), b"cached-providers").unwrap();

        let setup_tmp = voice_setup_tmp();
        std::fs::create_dir_all(&setup_tmp).unwrap();
        let archive = setup_tmp.join(format!("{}.tar.bz2", SHERPA_RELEASE_NAME));
        if !make_tar(
            &archive,
            src.path(),
            &[&format!("{}/lib", SHERPA_RELEASE_NAME)],
            true,
        ) {
            eprintln!("SKIP: tar 无 bz2 支持（本环境打不出 bz2 归档）");
            return;
        }

        let exe_dir = tempfile::tempdir().unwrap();
        // URL 指向不可达端口——走缓存臂不会发请求
        block_on(async {
            try_download_and_extract("http://127.0.0.1:9/x.tar.bz2", exe_dir.path(), "")
                .await
                .unwrap();
            assert!(exe_dir.path().join(REQUIRED_LIBS[0]).exists());
        });

        clean_all_setup_tmps();
    }

    // -------------------------------------------------------------------
    // try_download_and_extract —— 坏归档臂（下载成功但 tar 解压失败 →
    // 清理归档 + bail "tar extraction failed"）
    // -------------------------------------------------------------------

    #[test]
    fn try_download_and_extract_bad_archive_bails_and_cleans_up() {
        let _g = DOWNLOAD_TMP_LOCK.lock().unwrap();
        clean_all_setup_tmps();

        let exe_dir = tempfile::tempdir().unwrap();
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_raw(b"not-a-tar-at-all", "application/octet-stream"),
                )
                .mount(&server)
                .await;
            let url = format!("{}/{}.tar.bz2", server.uri(), SHERPA_RELEASE_NAME);
            let err = format!(
                "{:#}",
                try_download_and_extract(&url, exe_dir.path(), "")
                    .await
                    .unwrap_err()
            );
            assert!(err.contains("tar extraction failed"), "{err}");
            // bail 前删了归档 + .part
            let archive = voice_setup_tmp().join(format!("{}.tar.bz2", SHERPA_RELEASE_NAME));
            assert!(
                !archive.exists(),
                "bad archive must be removed after failure"
            );
        });

        clean_all_setup_tmps();
    }

    // -------------------------------------------------------------------
    // try_download_and_extract —— HTTP 错误臂（500 → bail，不进解压）
    // -------------------------------------------------------------------

    #[test]
    fn try_download_and_extract_http_500_bails() {
        let _g = DOWNLOAD_TMP_LOCK.lock().unwrap();
        clean_all_setup_tmps();

        let exe_dir = tempfile::tempdir().unwrap();
        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(500))
                .mount(&server)
                .await;
            let url = format!("{}/{}.tar.bz2", server.uri(), SHERPA_RELEASE_NAME);
            let err = format!(
                "{:#}",
                try_download_and_extract(&url, exe_dir.path(), "")
                    .await
                    .unwrap_err()
            );
            assert!(err.contains("HTTP 500"), "{err}");
            assert!(!exe_dir.path().join(REQUIRED_LIBS[0]).exists());
        });

        clean_all_setup_tmps();
    }
}
