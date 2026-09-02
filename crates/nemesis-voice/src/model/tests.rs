//! Tests for `model`（S12 覆盖率冲刺，2026-08-26）。
//!
//! 覆盖面：
//! - `ensure_*_model` 五个函数的本地命中 / auto_download 关 / 无 source 三分支
//!   （temp 目录造文件布局夹具，不碰网络）。
//! - `check_model_files` / `build_url` 纯函数。
//! - 下载机械（`download_model_files` / `download_file_resume` / `stream_to_file` /
//!   `get_remote_size`）—— 用本地 wiremock 假镜像（127.0.0.1，非真网络）：
//!   全量下载 / 已存在跳过 / 续传完整直接改名 / Range 206 续传 / 服务端不支持
//!   Range 回退全量 / HTTP 错误 bail / direct URL 覆盖 / 子目录文件建父目录 /
//!   进度回调（thread-local，必须在下载线程上 set）。
//!
//! 两个工程约束：
//! 1. reqwest::blocking 不能进 tokio 上下文 → 下载放独立 std 线程，结果经
//!    tokio oneshot 送回；等待用 `.await`（不能用 rx.recv 阻塞——那会卡死
//!    current_thread runtime，wiremock server 就没人驱动了）。
//! 2. 测试手动建 current_thread runtime 挂 block_on，server 与 await 同一驱动。

use super::*;
use crate::config::{ModelFile, ModelSource};
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn source_with(name: &str, repo: &str, locals: &[&str]) -> ModelSource {
    ModelSource {
        name: name.to_string(),
        category: "test".to_string(),
        repo: repo.to_string(),
        files: locals
            .iter()
            .map(|l| ModelFile {
                local: l.to_string(),
                remote: l.to_string(),
                url: String::new(),
            })
            .collect(),
    }
}

fn cfg_with_model_dir(tmp: &Path, sources: Vec<ModelSource>) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.base_dir = tmp.to_path_buf();
    cfg.models.dir = "./data".to_string();
    cfg.models.sources = sources;
    cfg
}

/// 在独立 std 线程上执行阻塞下载（reqwest::blocking 不能进 tokio 上下文），
/// 结果经 oneshot 回送；调用侧 `.await`（保持 runtime 被驱动，wiremock 才能响应）。
async fn run_blocking<T, F>(f: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.await {
        Ok(v) => v,
        Err(_) => panic!("download thread panicked before sending result"),
    }
}

fn files(local: &str, remote: &str) -> Vec<ModelFile> {
    vec![ModelFile {
        local: local.to_string(),
        remote: remote.to_string(),
        url: String::new(),
    }]
}

/// 手动 current_thread runtime 上跑整个 async 测试体。
fn with_runtime<F, Fut>(f: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    rt.block_on(f());
}

// ---------------------------------------------------------------------------
// check_model_files / build_url 纯函数
// ---------------------------------------------------------------------------

#[test]
fn check_model_files_missing_dir_is_false() {
    assert!(!check_model_files(
        Path::new("Z:/definitely/not/here"),
        &[("a.onnx", "a.onnx")]
    ));
}

#[test]
fn check_model_files_partial_files_is_false_all_required() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.onnx"), b"x").unwrap();
    // b.onnx 缺失 → all() = false
    assert!(!check_model_files(
        tmp.path(),
        &[("a.onnx", "a.onnx"), ("b.onnx", "b.onnx")]
    ));
}

#[test]
fn check_model_files_all_present_is_true() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.onnx"), b"x").unwrap();
    std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
    std::fs::write(tmp.path().join("sub").join("b.onnx"), b"y").unwrap();
    assert!(check_model_files(
        tmp.path(),
        &[("a.onnx", "a.onnx"), ("sub/b.onnx", "b.onnx")]
    ));
}

#[test]
fn check_model_files_empty_list_on_existing_dir_is_true() {
    // 空期望列表 vacuously true（调用方先判 is_empty，这里钉语义）
    let tmp = tempfile::tempdir().unwrap();
    assert!(check_model_files(tmp.path(), &[]));
}

#[test]
fn build_url_trims_trailing_slash_and_joins() {
    assert_eq!(
        build_url("https://hf-mirror.com/", "user/repo", "model.onnx"),
        "https://hf-mirror.com/user/repo/resolve/main/model.onnx"
    );
    assert_eq!(
        build_url("https://hf-mirror.com//", "r", "f"),
        "https://hf-mirror.com/r/resolve/main/f"
    );
}

// ---------------------------------------------------------------------------
// ensure_stt_model / ensure_vad_model / ensure_tts_model / ensure_punct_model /
// ensure_speaker_model —— 三分支（本地命中 / 禁下载 / 无 source）
// ---------------------------------------------------------------------------

#[test]
fn ensure_stt_model_local_files_hit_returns_dir_without_network() {
    crate::test_util::ensure_global_subscriber();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data").join("stt").join("sensevoice-small");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model_sherpa.onnx"), b"m").unwrap();
    std::fs::write(dir.join("tokens.txt"), b"t").unwrap();

    let cfg = cfg_with_model_dir(
        tmp.path(),
        vec![source_with(
            "sensevoice-small",
            "user/sv",
            &["model_sherpa.onnx", "tokens.txt"],
        )],
    );
    let got = ensure_stt_model(&cfg).unwrap();
    assert_eq!(got, dir);
}

#[test]
fn ensure_stt_model_auto_download_disabled_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = cfg_with_model_dir(
        tmp.path(),
        vec![source_with(
            "sensevoice-small",
            "user/sv",
            &["model_sherpa.onnx"],
        )],
    );
    cfg.models.auto_download = false;

    let err = format!("{:#}", ensure_stt_model(&cfg).unwrap_err());
    assert!(err.contains("auto_download is disabled"), "{err}");
    assert!(err.contains("sensevoice-small"), "{err}");
}

#[test]
fn ensure_stt_model_no_source_bails_even_with_autodownload() {
    let tmp = tempfile::tempdir().unwrap();
    // sources 为空 → files 为空 → check 不通过 → context 失败
    let cfg = cfg_with_model_dir(tmp.path(), vec![]);
    let err = format!("{:#}", ensure_stt_model(&cfg).unwrap_err());
    assert!(
        err.contains("not found in config [models.sources]"),
        "{err}"
    );
}

#[test]
fn ensure_vad_model_local_files_hit_returns_first_file_path() {
    crate::test_util::ensure_global_subscriber(); // 122 行 info! 参数求值需 subscriber
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data").join("vad").join("silero_vad");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("silero_vad.onnx"), b"v").unwrap();

    let cfg = cfg_with_model_dir(
        tmp.path(),
        vec![source_with("silero_vad", "user/vad", &["silero_vad.onnx"])],
    );
    let got = ensure_vad_model(&cfg).unwrap();
    // VAD 返回的是模型**文件**路径（不是目录）
    assert_eq!(got, dir.join("silero_vad.onnx"));
}

#[test]
fn ensure_vad_model_auto_download_disabled_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = cfg_with_model_dir(tmp.path(), vec![]);
    cfg.models.auto_download = false;
    let err = format!("{:#}", ensure_vad_model(&cfg).unwrap_err());
    assert!(err.contains("auto_download is disabled"), "{err}");
}

#[test]
fn ensure_tts_model_local_files_hit_returns_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data").join("tts").join("kokoro");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.onnx"), b"m").unwrap();

    let mut cfg = cfg_with_model_dir(
        tmp.path(),
        vec![source_with("kokoro", "user/k", &["model.onnx"])],
    );
    cfg.tts.model_name = "kokoro".to_string();
    assert_eq!(ensure_tts_model(&cfg).unwrap(), dir);
}

#[test]
fn ensure_tts_model_auto_download_disabled_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = cfg_with_model_dir(tmp.path(), vec![]);
    cfg.models.auto_download = false;
    let err = format!("{:#}", ensure_tts_model(&cfg).unwrap_err());
    assert!(err.contains("auto_download is disabled"), "{err}");
}

#[test]
fn ensure_punct_model_local_files_hit_returns_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp
        .path()
        .join("data")
        .join("punct")
        .join("ct-transformer-zh-en");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.onnx"), b"p").unwrap();

    let cfg = cfg_with_model_dir(
        tmp.path(),
        vec![source_with(
            "ct-transformer-zh-en",
            "user/p",
            &["model.onnx"],
        )],
    );
    assert_eq!(ensure_punct_model(&cfg).unwrap(), dir);
}

#[test]
fn ensure_punct_model_no_source_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_with_model_dir(tmp.path(), vec![]);
    let err = format!("{:#}", ensure_punct_model(&cfg).unwrap_err());
    assert!(
        err.contains("not found in config [models.sources]"),
        "{err}"
    );
}

#[test]
fn ensure_speaker_model_local_files_hit_returns_dir() {
    crate::test_util::ensure_global_subscriber(); // 259 行 info! 参数求值需 subscriber
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data").join("speaker").join("3dspeaker");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("campplus.onnx"), b"s").unwrap();

    let mut cfg = cfg_with_model_dir(
        tmp.path(),
        vec![source_with("3dspeaker", "user/sp", &["campplus.onnx"])],
    );
    cfg.speaker.model_name = "3dspeaker".to_string();
    assert_eq!(ensure_speaker_model(&cfg).unwrap(), dir);
}

#[test]
fn ensure_speaker_model_auto_download_disabled_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = cfg_with_model_dir(tmp.path(), vec![]);
    cfg.models.auto_download = false;
    let err = format!("{:#}", ensure_speaker_model(&cfg).unwrap_err());
    assert!(err.contains("auto_download is disabled"), "{err}");
}

#[test]
fn ensure_speaker_model_no_source_bails_even_with_autodownload() {
    // 274 行：sources 为空（auto_download 开着）→ context bail
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_with_model_dir(tmp.path(), vec![]);
    let err = format!("{:#}", ensure_speaker_model(&cfg).unwrap_err());
    assert!(
        err.contains("not found in config [models.sources]"),
        "{err}"
    );
}

// ---------------------------------------------------------------------------
// 下载机械 —— wiremock 本地假镜像（127.0.0.1，非真网络）
// ---------------------------------------------------------------------------

#[test]
fn download_model_files_full_download_writes_final_file_and_clears_part() {
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repo/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_string("MODEL-BYTES"))
            .mount(&server)
            .await;
        // HEAD 未匹配 → 404 → get_remote_size = None（走无 total 的进度分支）

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("stt").join("m");
        let target_check = target.clone();
        let f = files("model.onnx", "model.onnx");
        run_blocking(move || download_model_files(&server_uri, "m", "repo", &f, &target, ""))
            .await
            .unwrap();

        let final_file = target_check.join("model.onnx");
        assert_eq!(std::fs::read(&final_file).unwrap(), b"MODEL-BYTES");
        // .part 成功后必须改名清掉
        assert!(!target_check.join("model.onnx.part").exists());
    });
}

#[test]
fn download_model_files_existing_nonempty_final_file_is_skipped_without_http() {
    with_runtime(|| async {
        // 不挂任何 mock：若代码发请求 → 404 → 下载失败 → 测试失败
        let server = MockServer::start().await;
        let server_uri = server.uri();

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("stt").join("m");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("model.onnx"), b"ALREADY-THERE").unwrap();
        let target_check = target.clone();

        let f = files("model.onnx", "model.onnx");
        run_blocking(move || download_model_files(&server_uri, "m", "repo", &f, &target, ""))
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(target_check.join("model.onnx")).unwrap(),
            b"ALREADY-THERE"
        );
    });
}

#[test]
fn download_model_files_creates_parent_dirs_for_subdir_files() {
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repo/resolve/main/dict/a.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("DICT-A"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("stt").join("m");
        let target_check = target.clone();
        let f = files("dict/a.txt", "dict/a.txt");
        run_blocking(move || download_model_files(&server_uri, "m", "repo", &f, &target, ""))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(target_check.join("dict").join("a.txt")).unwrap(),
            b"DICT-A"
        );
    });
}

#[test]
fn download_model_files_http_error_bails_with_status() {
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repo/resolve/main/missing.onnx"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("stt").join("m");
        let f = files("missing.onnx", "missing.onnx");
        let res =
            run_blocking(move || download_model_files(&server_uri, "m", "repo", &f, &target, ""))
                .await;
        let err = format!("{:#}", res.unwrap_err());
        assert!(err.contains("HTTP 404"), "{err}");
    });
}

#[test]
fn download_model_files_direct_url_overrides_mirror() {
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/direct/absolute/model.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_string("DIRECT"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("stt").join("m");
        let url = format!("{server_uri}/direct/absolute/model.onnx");
        let target_check = target.clone();
        run_blocking(move || {
            let f = vec![ModelFile {
                local: "model.onnx".to_string(),
                remote: "unused-remote.onnx".to_string(),
                url,
            }];
            // repo 留空 → source 标签走 "direct URL" 分支
            download_model_files(&server_uri, "m", "", &f, &target, "")
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(target_check.join("model.onnx")).unwrap(),
            b"DIRECT"
        );
    });
}

#[test]
fn download_file_resume_complete_part_is_renamed_without_get() {
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/repo/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-length", "4"))
            .mount(&server)
            .await;
        // 不挂 GET mock：若走了 GET → 404 → bail → 测试失败

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("final.onnx");
        let part = tmp.path().join("final.onnx.part");
        std::fs::write(&part, b"ABCD").unwrap(); // 与远端等长 → 已完整
        let target_check = target.clone();
        let part_check = part.clone();

        run_blocking(move || {
            let client = reqwest::blocking::Client::new();
            download_file_resume(
                &client,
                &format!("{server_uri}/repo/resolve/main/model.onnx"),
                &target,
                &part,
                "final.onnx",
            )
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&target_check).unwrap(), b"ABCD");
        assert!(!part_check.exists());
    });
}

#[test]
fn download_file_resume_partial_gets_206_and_appends() {
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/repo/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(200).insert_header("content-length", "4"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repo/resolve/main/model.onnx"))
            .and(wiremock::matchers::header("Range", "bytes=2-"))
            .respond_with(ResponseTemplate::new(206).set_body_string("CD"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("final.onnx");
        let part = tmp.path().join("final.onnx.part");
        std::fs::write(&part, b"AB").unwrap(); // 已有 2 字节
        let target_check = target.clone();

        crate::test_util::ensure_global_subscriber(); // 433 行续传 info! 参数求值
        run_blocking(move || {
            let client = reqwest::blocking::Client::new();
            download_file_resume(
                &client,
                &format!("{server_uri}/repo/resolve/main/model.onnx"),
                &target,
                &part,
                "final.onnx",
            )
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read(&target_check).unwrap(), b"ABCD");
    });
}

#[test]
fn download_file_resume_server_ignores_range_falls_back_to_full() {
    with_runtime(|| async {
        let server = MockServer::start().await;
        // HEAD 404 → total None；GET（带不带 Range 都 200 全量）→ 非 206 → 全量重下
        Mock::given(method("GET"))
            .and(path("/repo/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_string("WXYZ"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("final.onnx");
        let part = tmp.path().join("final.onnx.part");
        std::fs::write(&part, b"AB").unwrap();
        let target_check = target.clone();

        run_blocking(move || {
            let client = reqwest::blocking::Client::new();
            download_file_resume(
                &client,
                &format!("{server_uri}/repo/resolve/main/model.onnx"),
                &target,
                &part,
                "final.onnx",
            )
        })
        .await
        .unwrap();
        // .part 被全量覆盖，不是拼接
        assert_eq!(std::fs::read(&target_check).unwrap(), b"WXYZ");
    });
}

#[test]
fn download_model_files_reports_progress_on_download_thread() {
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repo/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_string("BODY"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("stt").join("m");
        let msgs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = msgs.clone();
        let f = files("model.onnx", "model.onnx");
        run_blocking(move || {
            // PROGRESS_CB 是 thread-local → 必须在执行下载的线程上 set
            set_progress(Some(Box::new(move |m: &str| {
                sink.lock().unwrap().push(m.to_string());
            })));
            let r = download_model_files(&server_uri, "m", "repo", &f, &target, "");
            set_progress(None); // 清线程本地回调
            r
        })
        .await
        .unwrap();

        let got = msgs.lock().unwrap();
        assert!(
            got.iter().any(|m| m.contains("开始下载")),
            "progress messages missing '开始下载': {got:?}"
        );
        assert!(
            got.iter().any(|m| m.contains("下载完成")),
            "progress messages missing '下载完成': {got:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// ensure_stt_model 端到端（wiremock 假镜像 + 真下载管线）
// ---------------------------------------------------------------------------

#[test]
fn ensure_stt_model_downloads_when_missing_then_returns_dir() {
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/sv/resolve/main/model_sherpa.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_string("SV"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_model_dir(
            tmp.path(),
            vec![source_with(
                "sensevoice-small",
                "user/sv",
                &["model_sherpa.onnx"],
            )],
        );
        cfg.models.mirror.base = server_uri;

        let dir = run_blocking(move || ensure_stt_model(&cfg)).await.unwrap();
        assert!(dir.ends_with("sensevoice-small"));
        assert_eq!(std::fs::read(dir.join("model_sherpa.onnx")).unwrap(), b"SV");
    });
}

#[test]
fn get_remote_size_head_error_returns_none() {
    with_runtime(|| async {
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let res = run_blocking(move || {
            let client = reqwest::blocking::Client::new();
            get_remote_size(&client, &format!("{server_uri}/no-such-path"))
        })
        .await;
        assert!(res.is_none(), "404 HEAD must map to None, got {res:?}");
    });
}

// ===========================================================================
// R6 覆盖率批次（2026-08-27）：ensure_* 下载臂（成功 + `)?` 失败传播）、
// proxy 两臂、>4MB flush 臂、慢体 2s 进度臂（Some/None 两变体）、截断体
// interrupted 臂、0 字节已存在文件重下。
// ===========================================================================

#[test]
fn ensure_stt_model_download_failure_propagates_err() {
    // 97 行 `)?` 错误传播臂：下载 HTTP 500 → Err 带 status
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/sv/resolve/main/model_sherpa.onnx"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_model_dir(
            tmp.path(),
            vec![source_with(
                "sensevoice-small",
                "user/sv",
                &["model_sherpa.onnx"],
            )],
        );
        cfg.models.mirror.base = server_uri;

        let err = format!(
            "{:#}",
            run_blocking(move || ensure_stt_model(&cfg))
                .await
                .unwrap_err()
        );
        assert!(err.contains("HTTP 500"), "{err}");
    });
}

#[test]
fn ensure_vad_model_downloads_when_missing_then_returns_file_path() {
    with_runtime(|| async {
        crate::test_util::ensure_global_subscriber();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/vad/resolve/main/silero_vad.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_string("VAD"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_model_dir(
            tmp.path(),
            vec![source_with("silero_vad", "user/vad", &["silero_vad.onnx"])],
        );
        cfg.models.mirror.base = server_uri;

        let path = run_blocking(move || ensure_vad_model(&cfg)).await.unwrap();
        assert!(path.ends_with("silero_vad.onnx"), "{path:?}");
        assert_eq!(std::fs::read(&path).unwrap(), b"VAD");
    });
}

#[test]
fn ensure_vad_model_download_failure_propagates_err() {
    // 149 行 `)?` 错误传播臂
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/vad/resolve/main/silero_vad.onnx"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_model_dir(
            tmp.path(),
            vec![source_with("silero_vad", "user/vad", &["silero_vad.onnx"])],
        );
        cfg.models.mirror.base = server_uri;

        let err = format!(
            "{:#}",
            run_blocking(move || ensure_vad_model(&cfg))
                .await
                .unwrap_err()
        );
        assert!(err.contains("HTTP 500"), "{err}");
    });
}

#[test]
fn ensure_tts_model_no_source_bails() {
    // 179-182 行 context 参数（TTS 此前无 no-source 测试）
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = cfg_with_model_dir(tmp.path(), vec![]);
    cfg.tts.model_name = "kokoro".to_string();
    let err = format!("{:#}", ensure_tts_model(&cfg).unwrap_err());
    assert!(
        err.contains("not found in config [models.sources]"),
        "{err}"
    );
}

#[test]
fn ensure_tts_model_downloads_when_missing_then_returns_dir() {
    with_runtime(|| async {
        crate::test_util::ensure_global_subscriber();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/k/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_string("TTS"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_model_dir(
            tmp.path(),
            vec![source_with("kokoro", "user/k", &["model.onnx"])],
        );
        cfg.tts.model_name = "kokoro".to_string();
        cfg.models.mirror.base = server_uri;

        let dir = run_blocking(move || ensure_tts_model(&cfg)).await.unwrap();
        assert!(dir.ends_with("kokoro"), "{dir:?}");
        assert_eq!(std::fs::read(dir.join("model.onnx")).unwrap(), b"TTS");
    });
}

#[test]
fn ensure_tts_model_download_failure_propagates_err() {
    // 191 行 `)?` 错误传播臂
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/k/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_model_dir(
            tmp.path(),
            vec![source_with("kokoro", "user/k", &["model.onnx"])],
        );
        cfg.tts.model_name = "kokoro".to_string();
        cfg.models.mirror.base = server_uri;

        let err = format!(
            "{:#}",
            run_blocking(move || ensure_tts_model(&cfg))
                .await
                .unwrap_err()
        );
        assert!(err.contains("HTTP 500"), "{err}");
    });
}

#[test]
fn ensure_punct_model_auto_download_disabled_bails() {
    // 217-221 行（punct 此前无 auto-disabled 测试）
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = cfg_with_model_dir(
        tmp.path(),
        vec![source_with(
            "ct-transformer-zh-en",
            "user/p",
            &["model.onnx"],
        )],
    );
    cfg.models.auto_download = false;
    let err = format!("{:#}", ensure_punct_model(&cfg).unwrap_err());
    assert!(err.contains("auto_download is disabled"), "{err}");
}

#[test]
fn ensure_punct_model_downloads_when_missing_then_returns_dir() {
    with_runtime(|| async {
        crate::test_util::ensure_global_subscriber();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/p/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_string("PUNCT"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_model_dir(
            tmp.path(),
            vec![source_with(
                "ct-transformer-zh-en",
                "user/p",
                &["model.onnx"],
            )],
        );
        cfg.models.mirror.base = server_uri;

        let dir = run_blocking(move || ensure_punct_model(&cfg))
            .await
            .unwrap();
        assert!(dir.ends_with("ct-transformer-zh-en"), "{dir:?}");
        assert_eq!(std::fs::read(dir.join("model.onnx")).unwrap(), b"PUNCT");
    });
}

#[test]
fn ensure_punct_model_download_failure_propagates_err() {
    // 235 行 `)?` 错误传播臂
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/p/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_model_dir(
            tmp.path(),
            vec![source_with(
                "ct-transformer-zh-en",
                "user/p",
                &["model.onnx"],
            )],
        );
        cfg.models.mirror.base = server_uri;

        let err = format!(
            "{:#}",
            run_blocking(move || ensure_punct_model(&cfg))
                .await
                .unwrap_err()
        );
        assert!(err.contains("HTTP 500"), "{err}");
    });
}

#[test]
fn ensure_speaker_model_downloads_when_missing_then_returns_dir() {
    with_runtime(|| async {
        crate::test_util::ensure_global_subscriber();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/sp/resolve/main/campplus.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_string("SPK"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_model_dir(
            tmp.path(),
            vec![source_with("3dspeaker", "user/sp", &["campplus.onnx"])],
        );
        cfg.speaker.model_name = "3dspeaker".to_string();
        cfg.models.mirror.base = server_uri;

        let dir = run_blocking(move || ensure_speaker_model(&cfg))
            .await
            .unwrap();
        assert!(dir.ends_with("3dspeaker"), "{dir:?}");
        assert_eq!(std::fs::read(dir.join("campplus.onnx")).unwrap(), b"SPK");
    });
}

#[test]
fn ensure_speaker_model_download_failure_propagates_err() {
    // 283 行 `)?` 错误传播臂
    with_runtime(|| async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user/sp/resolve/main/campplus.onnx"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_with_model_dir(
            tmp.path(),
            vec![source_with("3dspeaker", "user/sp", &["campplus.onnx"])],
        );
        cfg.speaker.model_name = "3dspeaker".to_string();
        cfg.models.mirror.base = server_uri;

        let err = format!(
            "{:#}",
            run_blocking(move || ensure_speaker_model(&cfg))
                .await
                .unwrap_err()
        );
        assert!(err.contains("HTTP 500"), "{err}");
    });
}

// ---------------------------------------------------------------------------
// R6：proxy 两臂 / flush 臂 / 慢体进度两变体 / 截断体 / 0 字节重下
// ---------------------------------------------------------------------------

#[test]
fn download_model_files_invalid_proxy_url_bails() {
    // 313 行 Proxy::all 解析失败臂
    with_runtime(|| async {
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("stt").join("m");
        let f = files("model.onnx", "model.onnx");
        let res = run_blocking(move || {
            download_model_files(&server_uri, "m", "repo", &f, &target, "http://[::1")
        })
        .await;
        let err = format!("{:#}", res.unwrap_err());
        assert!(err.contains("Invalid proxy URL"), "{err}");
    });
}

#[test]
fn download_model_files_dead_proxy_fails_to_connect() {
    // 314-315 行 proxy 装上 builder 的成功臂：指向死端口 → 下载连接失败
    with_runtime(|| async {
        crate::test_util::ensure_global_subscriber(); // 314 行 info! 参数
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("stt").join("m");
        let f = files("model.onnx", "model.onnx");
        let res = run_blocking(move || {
            download_model_files(&server_uri, "m", "repo", &f, &target, "http://127.0.0.1:1")
        })
        .await;
        let err = format!("{:#}", res.unwrap_err());
        assert!(err.contains("Failed to download"), "{err}");
    });
}

#[test]
fn download_model_files_body_over_4mb_hits_midstream_flush() {
    // 502-503 行：written−last_flush ≥ 4MB → sync_data 臂
    with_runtime(|| async {
        crate::test_util::ensure_global_subscriber();
        let server = MockServer::start().await;
        let body = vec![b'x'; 5 * 1024 * 1024];
        Mock::given(method("GET"))
            .and(path("/repo/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("stt").join("m");
        let target_check = target.clone();
        let f = files("model.onnx", "model.onnx");
        run_blocking(move || download_model_files(&server_uri, "m", "repo", &f, &target, ""))
            .await
            .unwrap();
        assert_eq!(
            std::fs::metadata(target_check.join("model.onnx"))
                .unwrap()
                .len(),
            5 * 1024 * 1024
        );
    });
}

/// 慢滴服务器：HEAD 按参数回 200(CL:10) 或 404；GET 头+首块立即到，
/// 停 3s 再发尾块——制造「两次 read 间隔 ≥2s」翻 stream_to_file 进度臂。
/// （wiremock 的 set_delay 只能延迟整个响应=头+体同批到达，last_print 从
/// 拿到 response 才起算，进度臂永远不触发——必须裸 TCP 分块慢滴。）
fn spawn_drip_server(head_ok: bool) -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        for _ in 0..2 {
            // HEAD（get_remote_size）与 GET 各一条连接（Connection: close 不复用）
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let mut n = 0;
            loop {
                let r = stream.read(&mut buf[n..]).unwrap();
                n += r;
                if r == 0 || buf[..n].ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let is_get = buf.starts_with(b"GET");
            if is_get {
                let (head, c1, c2) = if head_ok {
                    // 带总长：Content-Length 10
                    (
                        "HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n",
                        "01234",
                        "56789",
                    )
                } else {
                    // 不带总长：读到 EOF 为止
                    (
                        "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n",
                        "SLOW-",
                        "BODY",
                    )
                };
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(c1.as_bytes()).unwrap();
                stream.flush().unwrap();
                std::thread::sleep(std::time::Duration::from_secs(3));
                stream.write_all(c2.as_bytes()).unwrap();
                stream.flush().unwrap();
                // drop(stream) 关连接
            } else if head_ok {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
                stream.flush().unwrap();
            } else {
                stream
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
                stream.flush().unwrap();
            }
        }
    });
    addr
}

#[test]
fn download_file_resume_slow_body_with_total_reports_percentage() {
    // 508-522 行：首读距 last_print ≥2s → Some(total) 百分比进度臂
    with_runtime(|| async {
        crate::test_util::ensure_global_subscriber();
        let addr = spawn_drip_server(true);
        let url = format!("http://{addr}/repo/resolve/main/model.onnx");

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("final.onnx");
        let part = tmp.path().join("final.onnx.part");
        let msgs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = msgs.clone();
        let target_check = target.clone();
        let part_check = part.clone();

        run_blocking(move || {
            set_progress(Some(Box::new(move |m: &str| {
                sink.lock().unwrap().push(m.to_string());
            })));
            let client = reqwest::blocking::Client::new();
            let r = download_file_resume(&client, &url, &target, &part, "final.onnx");
            set_progress(None);
            r
        })
        .await
        .unwrap();

        assert_eq!(std::fs::read(&target_check).unwrap(), b"0123456789");
        assert!(!part_check.exists());
        let got = msgs.lock().unwrap();
        assert!(
            got.iter().any(|m| m.contains('%')),
            "progress messages missing percentage line: {got:?}"
        );
    });
}

#[test]
fn download_file_resume_slow_body_unknown_total_reports_mb_downloaded() {
    // 524-533 行：HEAD 404 → total None → "MB downloaded" 进度臂
    with_runtime(|| async {
        crate::test_util::ensure_global_subscriber();
        let addr = spawn_drip_server(false);
        let url = format!("http://{addr}/repo/resolve/main/model.onnx");

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("final.onnx");
        let part = tmp.path().join("final.onnx.part");
        let msgs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = msgs.clone();
        let target_check = target.clone();

        run_blocking(move || {
            set_progress(Some(Box::new(move |m: &str| {
                sink.lock().unwrap().push(m.to_string());
            })));
            let client = reqwest::blocking::Client::new();
            let r = download_file_resume(&client, &url, &target, &part, "final.onnx");
            set_progress(None);
            r
        })
        .await
        .unwrap();

        assert_eq!(std::fs::read(&target_check).unwrap(), b"SLOW-BODY");
        let got = msgs.lock().unwrap();
        assert!(
            got.iter().any(|m| m.contains("MB downloaded")),
            "progress messages missing MB-downloaded line: {got:?}"
        );
    });
}

#[test]
fn download_file_resume_truncated_body_bails_interrupted() {
    // 536-538 行：Content-Length 虚报 + 提前断连 → read Err → interrupted bail
    with_runtime(|| async {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};
            for _ in 0..2 {
                // HEAD（get_remote_size）与 GET 各一条连接（Connection: close 不复用）
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 2048];
                let mut n = 0;
                loop {
                    let r = stream.read(&mut buf[n..]).unwrap();
                    n += r;
                    if r == 0 || buf[..n].ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let is_get = buf.starts_with(b"GET");
                // GET 虚报 CL=100 只发 5 字节即断连；HEAD 只回头
                let resp = if is_get {
                    "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nSHORT"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n"
                };
                stream.write_all(resp.as_bytes()).unwrap();
                stream.flush().unwrap();
                // drop(stream) 关连接 → 客户端读侧 UnexpectedEof
            }
        });

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("final.onnx");
        let part = tmp.path().join("final.onnx.part");
        let url = format!("http://{addr}/repo/resolve/main/model.onnx");

        let err = format!(
            "{:#}",
            run_blocking(move || {
                let client = reqwest::blocking::Client::new();
                download_file_resume(&client, &url, &target, &part, "final.onnx")
            })
            .await
            .unwrap_err()
        );
        assert!(err.contains("Download interrupted"), "{err}");
        // 失败后 .part 保留（供续传），target 不落
        assert!(tmp.path().join("final.onnx.part").exists());
        assert!(!tmp.path().join("final.onnx").exists());
    });
}

#[test]
fn download_model_files_zero_byte_existing_file_is_redownloaded() {
    // 341 行：已存在但 size == 0 → 不跳过，走下载
    with_runtime(|| async {
        crate::test_util::ensure_global_subscriber();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repo/resolve/main/model.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_string("NEW"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("stt").join("m");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("model.onnx"), b"").unwrap(); // 0 字节占位
        let target_check = target.clone();

        let f = files("model.onnx", "model.onnx");
        run_blocking(move || download_model_files(&server_uri, "m", "repo", &f, &target, ""))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(target_check.join("model.onnx")).unwrap(),
            b"NEW"
        );
    });
}
