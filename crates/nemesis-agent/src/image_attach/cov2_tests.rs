//! image_attach 覆盖率收尾批次（Wave6B）：SSRF Guard 三态接入 fetch_url_media
//! （闸关直通 / 钉死 client）/ pick_url_ext 决策表 / 降采样闸错误注记
//! （文本 + media 两来源）/ media 闸拒绝与不可读注记。

use super::*;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 本地 raw HTTP mock：按序吐预组响应（复用 cov_tests 同款形态）。
fn spawn_raw_http(responses: Vec<Vec<u8>>, max_hits: usize) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();
    std::thread::spawn(move || {
        for resp in responses.into_iter().take(max_hits) {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            c2.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(&resp);
            let _ = stream.flush();
        }
    });
    (format!("http://{}", addr), counter)
}

fn http_200(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut v = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        content_type,
        body.len()
    )
    .into_bytes();
    v.extend_from_slice(body);
    v
}

/// 最小合法 PNG（magic 8 字节 + 尾量），过 sniff_magic 验真。
fn png_bytes() -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend_from_slice(&[0u8; 16]);
    v
}

fn media_ref(url: &str) -> nemesis_types::channel::MediaAttachment {
    nemesis_types::channel::MediaAttachment {
        media_type: "image".to_string(),
        url: url.to_string(),
        data: None,
    }
}

/// 闸开但放行回环/内网（本机 mock 下载用）。
#[cfg(feature = "security")]
fn permissive_local_guard() -> nemesis_security::ssrf::Guard {
    nemesis_security::ssrf::Guard::new(nemesis_security::ssrf::SsrfConfig {
        enabled: true,
        block_localhost: false,
        block_private_ips: false,
        ..Default::default()
    })
    .expect("valid guard config")
}

// ---------------------------------------------------------------------------
// fetch_url_media × SSRF Guard 三态
// ---------------------------------------------------------------------------

/// 闸关（from_enabled(false)）→ Ok(空) → plain client 直通（316 臂）。
#[cfg(feature = "security")]
#[tokio::test]
async fn fetch_url_media_disabled_guard_takes_plain_client() {
    let _logs = crate::test_support::capture_logs();
    let resp = http_200("image/png", &png_bytes());
    let (base, _counter) = spawn_raw_http(vec![resp], 1);
    let uploads = tempfile::tempdir().unwrap();
    let guard = nemesis_security::ssrf::Guard::from_enabled(false);

    let (kept, notes) = fetch_url_media(
        &[media_ref(&format!("{base}/plain.png"))],
        uploads.path(),
        Some(&guard),
    )
    .await;
    assert!(notes.is_empty(), "notes: {:?}", notes);
    assert_eq!(kept.len(), 1);
    assert!(kept[0].url.starts_with(uploads.path().to_str().unwrap()));
}

/// 闸开 + 回环放行 → Ok(非空 ips) → 钉死 client（317/329 臂）→ 下载落盘
/// 成功（dir_ready 459 + info! 466 参数行）。
#[cfg(feature = "security")]
#[tokio::test]
async fn fetch_url_media_pinned_client_downloads_and_persists() {
    let _logs = crate::test_support::capture_logs();
    let resp = http_200("image/png", &png_bytes());
    let (base, counter) = spawn_raw_http(vec![resp], 1);
    let uploads = tempfile::tempdir().unwrap();
    let guard = permissive_local_guard();

    let (kept, notes) = fetch_url_media(
        &[media_ref(&format!("{base}/pinned.png"))],
        uploads.path(),
        Some(&guard),
    )
    .await;
    assert!(notes.is_empty(), "notes: {:?}", notes);
    assert_eq!(kept.len(), 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    // 落盘文件真实存在（dir_ready 臂 + std::fs::write 成功路径）。
    let stored = Path::new(kept[0].url.as_str());
    assert!(stored.exists());
    assert!(stored.starts_with(uploads.path()));
}

// ---------------------------------------------------------------------------
// pick_url_ext 决策表
// ---------------------------------------------------------------------------

#[test]
fn pick_url_ext_decision_table() {
    // 白名单内 URL 后缀优先（大小写归一）。
    assert_eq!(pick_url_ext("http://h/pic.PNG?a=1", ""), "png");
    assert_eq!(pick_url_ext("http://h/a/b.webp", ""), "webp");
    // 白名单外后缀 → Content-Type 映射。
    assert_eq!(pick_url_ext("http://h/pic.bin?ver=2", "image/webp"), "webp");
    assert_eq!(pick_url_ext("http://h/pic.bin", "image/gif"), "gif");
    // 都没有 → 兜底 jpg。
    assert_eq!(pick_url_ext("http://h/noext", "text/plain"), "jpg");
    assert_eq!(pick_url_ext("", ""), "jpg");
}

// ---------------------------------------------------------------------------
// attach_turn_images：降采样闸 / 闸拒绝 / 不可读
// ---------------------------------------------------------------------------

/// 文本来源 + 降采样开 + 候选不存在 → downscale_gate Err → 诚实注记。
#[test]
fn text_source_downscale_error_gets_honest_note() {
    let dir = tempfile::tempdir().unwrap();
    let uploads = tempfile::tempdir().unwrap();
    let missing = dir.path().join("ghost.png");
    let text = format!("看图 {}", missing.display());

    let outcome = attach_turn_images(
        &text,
        &[],
        Some(dir.path()),
        Some(uploads.path()),
        "web",
        None,
    );
    assert!(outcome.attached.is_empty());
    assert_eq!(outcome.notes.len(), 1, "notes: {:?}", outcome.notes);
    assert!(
        outcome.notes[0].contains("图片未附加"),
        "got: {}",
        outcome.notes[0]
    );
    assert!(
        outcome.notes[0].contains("ghost.png"),
        "got: {}",
        outcome.notes[0]
    );
}

/// media 来源 + 降采样开 + 路径不存在 → 同一 Err 注记形态。
#[test]
fn media_source_downscale_error_gets_honest_note() {
    let dir = tempfile::tempdir().unwrap();
    let uploads = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.png");
    let outcome = attach_turn_images(
        "",
        &[media_ref(&missing.to_string_lossy())],
        Some(dir.path()),
        Some(uploads.path()),
        "web",
        None,
    );
    assert!(outcome.attached.is_empty());
    assert_eq!(outcome.notes.len(), 1, "notes: {:?}", outcome.notes);
    // downscale_gate 对元数据读取失败的诚实理由。
    assert!(
        outcome.notes[0].contains("无法读取图片元数据"),
        "got: {}",
        outcome.notes[0]
    );
}

/// media 来源 + 真实 png + 安全闸拒绝（*.png deny 规则）→ gate Err 注记。
#[cfg(feature = "security")]
#[test]
fn media_source_gate_denied_gets_honest_note() {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    use nemesis_security::types::SecurityRule;
    let plugin = SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        default_action: "allow".to_string(),
        file_rules: vec![SecurityRule {
            pattern: "*.png".to_string(),
            action: "deny".to_string(),
            comment: "covw6: deny png attach".to_string(),
        }],
        ..Default::default()
    });

    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("real.png");
    std::fs::write(&png, png_bytes()).unwrap();

    let outcome = attach_turn_images(
        "",
        &[media_ref(&png.to_string_lossy())],
        Some(dir.path()),
        None,
        "web",
        Some(&plugin),
    );
    assert!(outcome.attached.is_empty());
    assert_eq!(outcome.notes.len(), 1, "notes: {:?}", outcome.notes);
    assert!(
        outcome.notes[0].contains("图片未附加"),
        "got: {}",
        outcome.notes[0]
    );
}

/// media 来源 + verify=Unreadable（句柄锁致 open 失败）→ `_` 兜底臂注记。
#[cfg(all(windows, feature = "security"))]
#[test]
fn media_source_unreadable_gets_fallback_note() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("locked.png");
    std::fs::write(&png, png_bytes()).unwrap();
    let _handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&png)
        .unwrap();

    let outcome = attach_turn_images(
        "",
        &[media_ref(&png.to_string_lossy())],
        Some(dir.path()),
        None,
        "web",
        None,
    );
    assert!(outcome.attached.is_empty());
    assert_eq!(outcome.notes.len(), 1, "notes: {:?}", outcome.notes);
    assert!(
        outcome.notes[0].contains("文件无法读取"),
        "got: {}",
        outcome.notes[0]
    );
}
