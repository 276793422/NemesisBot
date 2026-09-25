//! image_attach 覆盖率补充测试（URL 预取失败面 / 钉死 DNS client /
//! 降采样闸接入两来源 / D6 溢出与 data: 注记 / 水合降级）。

use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{
    attach_turn_images, build_pinned_no_redirect_client, fetch_url_media, hydrate_image_refs,
    pick_url_ext,
};
use nemesis_types::channel::MediaAttachment;

fn media_ref(url: &str) -> MediaAttachment {
    MediaAttachment {
        media_type: String::new(),
        url: url.to_string(),
        data: None,
    }
}

fn png_bytes() -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
    v
}

/// 编码一张纯色 PNG（真实可解码，供降采样闸走 Replaced 路径）。
fn solid_png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([120, 130, 140]));
    let mut cursor = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .expect("encode png");
    cursor.into_inner()
}

/// 每请求回一段预置原始 HTTP 字节的 mock 服务器（至多 max_hits 个连接）。
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

// ---------------------------------------------------------------------------
// 文本来源：candidate_note 诚实注记（不可附加候选）
// ---------------------------------------------------------------------------

#[test]
fn text_candidate_not_found_gets_honest_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("ghost.png");
    let text = format!("看图 {}", missing.display());
    let outcome = attach_turn_images(&text, &[], Some(dir.path()), None, "web", None);
    assert!(outcome.attached.is_empty());
    assert_eq!(outcome.notes.len(), 1, "notes: {:?}", outcome.notes);
    assert!(outcome.notes[0].contains("图片未附加"));
    assert!(outcome.notes[0].contains("文件不存在"));
}

// ---------------------------------------------------------------------------
// 钉死 DNS client 构建器（S2；security feature）
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
#[test]
fn pinned_client_builder_resolves_for_valid_input() {
    let ip: std::net::IpAddr = "93.184.216.34".parse().unwrap();
    let client = build_pinned_no_redirect_client(
        "http://example.com/img.png",
        &[ip],
        "cov-ua",
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(2),
    );
    assert!(client.is_some(), "valid url + ip must build");
}

#[cfg(feature = "security")]
#[test]
fn pinned_client_builder_none_on_bad_url() {
    let ip: std::net::IpAddr = "93.184.216.34".parse().unwrap();
    let client = build_pinned_no_redirect_client(
        "not a url",
        &[ip],
        "cov-ua",
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(2),
    );
    assert!(client.is_none(), "unparseable url must yield None");
}

#[cfg(feature = "security")]
#[test]
fn pinned_client_builder_none_on_hostless_url() {
    let ip: std::net::IpAddr = "93.184.216.34".parse().unwrap();
    let client = build_pinned_no_redirect_client(
        "http://",
        &[ip],
        "cov-ua",
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(2),
    );
    assert!(client.is_none(), "hostless url must yield None");
}

// ---------------------------------------------------------------------------
// fetch_url_media：F-K 溢出 / data: 注记 / 下载失败面
// ---------------------------------------------------------------------------

/// F-K：media 引用超过每消息上限 → 截断 + 聚合注记（下载前生效）。
#[tokio::test]
async fn fk_media_overflow_truncates_before_download() {
    let media: Vec<MediaAttachment> = (0..crate::image_path_detector::MAX_IMAGES_PER_MESSAGE + 2)
        .map(|i| media_ref(&format!("data:image/png;base64,AAAA{i}")))
        .collect();
    let uploads = tempfile::tempdir().expect("uploads");
    let (kept, notes) = fetch_url_media(&media, uploads.path(), None).await;
    let max = crate::image_path_detector::MAX_IMAGES_PER_MESSAGE;
    let overflow = notes
        .iter()
        .find(|n| n.contains(&format!("仅处理前 {max} 条")))
        .expect("overflow note present");
    assert!(overflow.contains("图片未附加"));
    // data: 项不落盘
    assert!(kept.is_empty());
}

/// data: URI 与内联 data 引用 → 诚实注明不支持（不误报文件不存在）。
#[tokio::test]
async fn data_uri_gets_unsupported_note() {
    let uploads = tempfile::tempdir().expect("uploads");
    let (kept, notes) = fetch_url_media(
        &[media_ref("data:image/png;base64,AAAA"), {
            let mut m = media_ref("");
            m.data = Some("aGVsbG8=".to_string());
            m
        }],
        uploads.path(),
        None,
    )
    .await;
    assert!(kept.is_empty());
    assert_eq!(notes.len(), 2, "notes: {:?}", notes);
    assert!(notes[0].contains("内联 data 图片引用"));
    assert!(notes[1].contains("内联 data 图片引用"));
}

/// 连接拒绝（无监听端口）→ 拉取失败诚实注记（send Err 分支）。
#[tokio::test]
async fn connection_refused_gets_fetch_failed_note() {
    // 先拿一个 OS 分配端口再放手 → 该端口当前无监听。
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let uploads = tempfile::tempdir().expect("uploads");
    let (kept, notes) = fetch_url_media(
        &[media_ref(&format!("http://{addr}/x.png"))],
        uploads.path(),
        None,
    )
    .await;
    assert!(kept.is_empty());
    assert_eq!(notes.len(), 1, "notes: {:?}", notes);
    assert!(notes[0].contains("URL 拉取失败"), "got: {}", notes[0]);
}

/// 非 2xx（404）→ error_for_status 拒绝 + 诚实注记。
#[tokio::test]
async fn http_404_gets_fetch_failed_note() {
    let resp: Vec<u8> =
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    let (base, _counter) = spawn_raw_http(vec![resp], 1);
    let uploads = tempfile::tempdir().expect("uploads");
    let (kept, notes) = fetch_url_media(
        &[media_ref(&format!("{base}/ghost.png"))],
        uploads.path(),
        None,
    )
    .await;
    assert!(kept.is_empty());
    assert_eq!(notes.len(), 1, "notes: {:?}", notes);
    assert!(notes[0].contains("URL 拉取失败"), "got: {}", notes[0]);
}

/// 响应体在 Content-Length 之前中断 → chunk 读取错误 + 诚实注记。
#[tokio::test]
async fn truncated_body_gets_read_failed_note() {
    // Content-Length 宣称 1000 字节，实发 8 字节即断开。
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n".to_vec();
    let mut resp = head;
    resp.extend_from_slice(&png_bytes());
    let (base, _counter) = spawn_raw_http(vec![resp], 1);
    let uploads = tempfile::tempdir().expect("uploads");
    let (kept, notes) = fetch_url_media(
        &[media_ref(&format!("{base}/cut.png"))],
        uploads.path(),
        None,
    )
    .await;
    assert!(kept.is_empty());
    assert_eq!(notes.len(), 1, "notes: {:?}", notes);
    assert!(
        notes[0].contains("URL 响应读取失败") || notes[0].contains("URL 拉取失败"),
        "got: {}",
        notes[0]
    );
}

/// 空响应体（200 + Content-Length: 0）→ 验真失败诚实注记。
#[tokio::test]
async fn empty_body_gets_empty_note() {
    let (base, _counter) = spawn_raw_http(vec![http_200("image/png", b"")], 1);
    let uploads = tempfile::tempdir().expect("uploads");
    let (kept, notes) = fetch_url_media(
        &[media_ref(&format!("{base}/empty.png"))],
        uploads.path(),
        None,
    )
    .await;
    assert!(kept.is_empty());
    assert_eq!(notes.len(), 1, "notes: {:?}", notes);
    assert!(notes[0].contains("URL 响应为空"), "got: {}", notes[0]);
}

/// uploads 目录创建失败（父级是文件）→ 诚实注记（create_dir_all 错误分支）。
#[tokio::test]
async fn uploads_dir_creation_failure_gets_note() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // 把 uploads 的父位置占成文件 → create_dir_all 必败。
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, b"file").expect("write blocker");
    let uploads_dir = blocker.join("uploads");

    let (base, _counter) = spawn_raw_http(vec![http_200("image/png", &png_bytes())], 1);
    let (kept, notes) =
        fetch_url_media(&[media_ref(&format!("{base}/x.png"))], &uploads_dir, None).await;
    assert!(kept.is_empty());
    assert_eq!(notes.len(), 1, "notes: {:?}", notes);
    assert!(
        notes[0].contains("创建 uploads 目录失败"),
        "got: {}",
        notes[0]
    );
}

// ---------------------------------------------------------------------------
// J6 降采样闸接入统一附加（两来源）
// ---------------------------------------------------------------------------

/// 文本来源：超分辨率图先降采样 → 注记 + 产物（uploads down_*.jpg）附加。
#[test]
fn text_source_oversize_image_downsampled_then_attached() {
    let dir = tempfile::tempdir().expect("dir");
    let uploads = tempfile::tempdir().expect("uploads");
    let big = dir.path().join("big.png");
    std::fs::write(&big, solid_png(9000, 100)).expect("write big png");

    let text = format!("大图 {}", big.display());
    let outcome = attach_turn_images(
        &text,
        &[],
        Some(dir.path()),
        Some(uploads.path()),
        "web",
        None,
    );
    assert!(
        outcome.notes.iter().any(|n| n.contains("图片已降采样")),
        "notes: {:?}",
        outcome.notes
    );
    assert_eq!(outcome.attached.len(), 1);
    let attached = &outcome.attached[0].resolved;
    let name = attached.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("down_"), "product name: {name}");
    assert!(name.ends_with(".jpg"), "product name: {name}");
}

/// media 来源：同型降采样路径（Replaced → 产物替代原图走附加链）。
#[test]
fn media_source_oversize_image_downsampled_then_attached() {
    let dir = tempfile::tempdir().expect("dir");
    let uploads = tempfile::tempdir().expect("uploads");
    let big = dir.path().join("big.png");
    std::fs::write(&big, solid_png(100, 9000)).expect("write big png");

    let outcome = attach_turn_images(
        "no path in text",
        &[media_ref(&big.to_string_lossy())],
        Some(dir.path()),
        Some(uploads.path()),
        "web",
        None,
    );
    assert!(
        outcome.notes.iter().any(|n| n.contains("图片已降采样")),
        "notes: {:?}",
        outcome.notes
    );
    assert_eq!(outcome.attached.len(), 1);
    assert_eq!(outcome.attached[0].raw, big.to_string_lossy().into_owned());
}

/// media 来源：超 25MB（稀疏写全零 body）→ TooLarge 诚实注记。
#[test]
fn media_source_too_large_gets_honest_note() {
    let dir = tempfile::tempdir().expect("dir");
    let big = dir.path().join("huge.png");
    let mut bytes = png_bytes();
    bytes.resize(26 * 1024 * 1024, 0);
    std::fs::write(&big, &bytes).expect("write huge");

    let outcome = attach_turn_images(
        "",
        &[media_ref(&big.to_string_lossy())],
        Some(dir.path()),
        None,
        "web",
        None,
    );
    assert!(outcome.attached.is_empty());
    assert_eq!(outcome.notes.len(), 1, "notes: {:?}", outcome.notes);
    assert!(
        outcome.notes[0].contains("图片超过 25MB 上限"),
        "got: {}",
        outcome.notes[0]
    );
}

// ---------------------------------------------------------------------------
// 水合降级：magic 合法但扩展名不在白名单 → 占位文本
// ---------------------------------------------------------------------------

#[test]
fn hydrate_extensionless_magic_file_degrades_to_placeholder() {
    let dir = tempfile::tempdir().expect("dir");
    // PNG magic 但无扩展名 → verify 过（嗅 magic），media_type_for_path None。
    let path = dir.path().join("noext");
    std::fs::write(&path, png_bytes()).expect("write");
    let (images, placeholders) = hydrate_image_refs(&[path.to_string_lossy().into_owned()]);
    assert!(images.is_empty());
    assert_eq!(placeholders.len(), 1);
    assert!(placeholders[0].contains("图片已失效"));
}

/// 水合成功路径仍产出 base64（回归锚）。
#[test]
fn hydrate_png_ref_produces_base64() {
    let dir = tempfile::tempdir().expect("dir");
    let path: PathBuf = dir.path().join("ok.png");
    std::fs::write(&path, png_bytes()).expect("write");
    let (images, placeholders) = hydrate_image_refs(&[path.to_string_lossy().into_owned()]);
    assert!(placeholders.is_empty());
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].media_type, "image/png");
    assert!(!images[0].data.is_empty());
}

// ===========================================================================
// Wave4 覆盖批次：pick_url_ext 四分派臂（URL 扩展名白名单 / 白名单外回落
// content-type / 无扩展名 URL 回落 / 兜底 jpg）。
// ===========================================================================

#[test]
fn pick_url_ext_prefers_whitelisted_url_extension() {
    assert_eq!(pick_url_ext("https://x.test/pic.PNG?dl=1", ""), "png");
    assert_eq!(pick_url_ext("https://x.test/a/b/photo.jpeg", ""), "jpeg");
    assert_eq!(pick_url_ext("https://x.test/anim.webp", ""), "webp");
    assert_eq!(pick_url_ext("https://x.test/graph.gif", ""), "gif");
}

#[test]
fn pick_url_ext_falls_back_to_content_type() {
    // 扩展名不在白名单 → content-type 决定。
    assert_eq!(
        pick_url_ext(
            "https://x.test/download.aspx?id=1",
            "image/png; charset=binary"
        ),
        "png"
    );
    assert_eq!(
        pick_url_ext("https://x.test/get?file=1", "image/jpeg"),
        "jpg"
    );
    assert_eq!(
        pick_url_ext("https://x.test/get?file=2", "image/webp"),
        "webp"
    );
    assert_eq!(
        pick_url_ext("https://x.test/get?file=3", "image/gif"),
        "gif"
    );
}

#[test]
fn pick_url_ext_no_dotted_segment_falls_to_content_type_then_jpg() {
    // 无含点段（只有裸 host）→ content-type 臂；未知类型 → 兜底 jpg。
    assert_eq!(pick_url_ext("https://x.test/", "image/png"), "png");
    assert_eq!(
        pick_url_ext("https://x.test/", "application/octet-stream"),
        "jpg"
    );
}

// ---------------------------------------------------------------------------
// wave5c：Keep 臂 + 同文件去重 continue + 溢出 MAX + media URL 防御注记 +
// 钉死 client 薄包装
// ---------------------------------------------------------------------------

/// 文本来源：小图（Keep 判定）附加一次；同一路径第二次出现走 dedup continue。
#[test]
fn text_source_duplicate_file_dedups_and_small_image_keeps() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("keep.png");
    std::fs::write(&img, solid_png(32, 32)).unwrap();
    let text = format!("看图 {} 和再一次 {}", img.display(), img.display());
    let outcome = attach_turn_images(&text, &[], Some(dir.path()), None, "web", None);
    assert_eq!(
        outcome.attached.len(),
        1,
        "dedup keeps one: {:?} / notes {:?}",
        outcome.attached,
        outcome.notes
    );
    assert!(
        outcome.notes.iter().all(|n| !n.contains("图片未附加")),
        "no failure notes expected: {:?}",
        outcome.notes
    );
}

/// 文本来源：超过 MAX_IMAGES_PER_MESSAGE 的候选进溢出（不再附加）。
#[test]
fn text_source_overflow_beyond_max_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let max = crate::image_path_detector::MAX_IMAGES_PER_MESSAGE;
    let mut text = String::new();
    for i in 0..(max + 2) {
        let p = dir.path().join(format!("ov{i}.png"));
        std::fs::write(&p, solid_png(16, 16)).unwrap();
        text.push_str(&format!(" {} ", p.display()));
    }
    let outcome = attach_turn_images(&text, &[], Some(dir.path()), None, "web", None);
    assert_eq!(outcome.attached.len(), max, "attached capped at MAX");
}

/// media 来源：URL 引用不经预取 → 防御性诚实注记（不拉网络）；空 URL 跳过。
#[test]
fn media_url_and_empty_url_get_deferred_notes() {
    let dir = tempfile::tempdir().unwrap();
    let media = vec![media_ref(""), media_ref("https://example.com/pic.png")];
    let outcome = attach_turn_images("", &media, Some(dir.path()), None, "web", None);
    assert!(outcome.attached.is_empty());
    assert!(
        outcome
            .notes
            .iter()
            .any(|n| n.contains("URL 引用未经 fetch_url_media 预取")),
        "notes: {:?}",
        outcome.notes
    );
}

/// 钉死 DNS client 薄包装：合法输入构建成功（转发默认 UA/超时）。
#[cfg(feature = "security")]
#[test]
fn pinned_no_redirect_wrapper_builds_client() {
    let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
    let client = super::pinned_no_redirect_client("http://example.com/a.png", &[ip]);
    assert!(client.is_some(), "valid url+ip builds a client");
}
