//! T5 统一附加流程测试（goal 2026-09-03）。
//!
//! 覆盖三态安全语义 + 审计链 path+hash + 水合降级 + 跨来源去重：
//! 1. **点名路径放行**：无 file_rules 时附加**不走**文件工具的
//!    `restrict_to_workspace`——工作区外的绝对路径照样附加（聊天用户点名
//!    路径 = 主体意志，安全 8 层管线照常全跑）。
//! 2. **file_rules 收紧**：`*.png` deny 规则经管线 Layer 3（ABAC）拦截
//!    附加，诚实注明 `[图片未附加: ...]`；同消息的 .jpg 不受牵连。
//! 3. **文件工具 restrict=true**：同一工作区外路径，agent 自主 `read_file`
//!    工具仍拒（工具层开关继续管 agent，不受附加语义影响）。
//!
//! 另：审计链事件记 target=path + reason=sha256（P4.2 hash-only 语义）、
//! 水合成功/失效占位、URL 占位注记（T9 前诚实降级）。
//!
//! 注意：走真实 SecurityPlugin 的测试必须 multi_thread flavor——管线
//! Layer 7（病毒扫描）无条件 `tokio::task::block_in_place`，current_thread
//! runtime 会 panic。

use std::path::PathBuf;

use super::{AttachOutcome, attach_turn_images, hydrate_image_refs};
use nemesis_types::channel::MediaAttachment;

// ---------------------------------------------------------------------------
// 测试素材
// ---------------------------------------------------------------------------

/// 最小 PNG（8 字节签名 + IHDR 长度前缀；检测器只嗅探 magic 不解析结构）。
fn png_bytes() -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
    v
}

/// 最小 JPEG（FF D8 FF 前缀）。
fn jpg_bytes() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F']
}

/// 建临时目录 + 写入图片文件，返回 (目录守卫, 图片路径)。
fn temp_image(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).expect("write image");
    (dir, path)
}

fn media_ref(url: &str) -> MediaAttachment {
    MediaAttachment {
        media_type: String::new(),
        url: url.to_string(),
        data: None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

// ---------------------------------------------------------------------------
// SecurityPlugin 构造（真实管线；Layer 7 block_in_place → multi_thread）
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
fn allow_all_plugin(audit_path: &std::path::Path) -> nemesis_security::pipeline::SecurityPlugin {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        default_action: "allow".to_string(),
        audit_chain_enabled: true,
        audit_chain_path: Some(audit_path.to_string_lossy().into_owned()),
        ..Default::default()
    })
}

#[cfg(feature = "security")]
fn deny_png_plugin() -> nemesis_security::pipeline::SecurityPlugin {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    use nemesis_security::types::SecurityRule;
    SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        default_action: "allow".to_string(),
        file_rules: vec![SecurityRule {
            pattern: "*.png".to_string(),
            action: "deny".to_string(),
            comment: "T5 test: deny png attach".to_string(),
        }],
        ..Default::default()
    })
}

/// multi_thread runtime（真实管线测试用；见文件头注释）。
#[cfg(feature = "security")]
fn multithread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("multi-thread runtime")
}

// ---------------------------------------------------------------------------
// 状态 1：点名路径放行（附加不走工具层 restrict_to_workspace）
// ---------------------------------------------------------------------------

#[test]
#[cfg(feature = "security")]
fn named_path_outside_workspace_attaches_via_pipeline() {
    let rt = multithread_runtime();
    rt.block_on(async {
        let (dir, img) = temp_image("nb_attach_ok.png", &png_bytes());
        // workspace 基准指向另一个（不存在的）子目录：图片在工作区外
        let ws = dir.path().join("workspace");
        let plugin = allow_all_plugin(&dir.path().join("audit.jsonl"));

        let text = format!("看这张图 {}", img.display());
        let outcome = attach_turn_images(&text, &[], Some(&ws), "web", Some(&plugin));

        assert!(
            outcome.notes.is_empty(),
            "unexpected notes: {:?}",
            outcome.notes
        );
        assert_eq!(
            outcome.ref_strings(),
            vec![img.to_string_lossy().into_owned()],
            "工作区外点名路径应放行并产出引用"
        );
    });
}

#[test]
fn named_path_attaches_without_pipeline() {
    // security=None（security.enabled=false / feature 裁剪）：闸门直通语义。
    let (dir, img) = temp_image("nb_attach_nosec.jpg", &jpg_bytes());
    let ws = dir.path().join("workspace");

    let text = format!("图 {}", img.display());
    let outcome = attach_turn_images(&text, &[], Some(&ws), "web", None);

    assert!(outcome.notes.is_empty(), "notes: {:?}", outcome.notes);
    assert_eq!(outcome.ref_strings().len(), 1);
}

// ---------------------------------------------------------------------------
// 状态 2：file_rules 收紧（管线 Layer 3 拦截附加，诚实注明）
// ---------------------------------------------------------------------------

#[test]
#[cfg(feature = "security")]
fn file_rules_deny_png_blocks_attach_but_jpg_passes() {
    let rt = multithread_runtime();
    rt.block_on(async {
        let (dir, png) = temp_image("nb_deny.png", &png_bytes());
        let jpg = dir.path().join("nb_allow.jpg");
        std::fs::write(&jpg, jpg_bytes()).unwrap();
        let ws = dir.path().join("workspace");
        let plugin = deny_png_plugin();

        let text = format!("两张图 {} 和 {}", png.display(), jpg.display());
        let outcome = attach_turn_images(&text, &[], Some(&ws), "web", Some(&plugin));

        // png 被 Layer 3 拦 → 诚实注明；jpg 不受牵连照常附加。
        assert_eq!(outcome.attached.len(), 1, "只有 jpg 应附加");
        assert_eq!(outcome.attached[0].resolved, jpg, "附加的应是 jpg");
        assert_eq!(outcome.notes.len(), 1, "png 应有一条诚实注记");
        assert!(
            outcome.notes[0].starts_with("[图片未附加:"),
            "note 应为诚实未附加格式，got: {}",
            outcome.notes[0]
        );
        assert!(
            outcome.notes[0].contains(&png.to_string_lossy().to_string()),
            "note 应指明被拒路径，got: {}",
            outcome.notes[0]
        );
    });
}

// ---------------------------------------------------------------------------
// 状态 3：文件工具 restrict=true 对同一工作区外路径仍拒（语义对比）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_file_tool_restrict_still_rejects_outside_path() {
    use nemesis_tools::registry::Tool;

    let (dir, img) = temp_image("nb_tool_restrict.png", &png_bytes());
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();

    let tool = nemesis_tools::filesystem::ReadFileTool::new(ws.to_string_lossy().as_ref(), true);
    let result = tool
        .execute(&serde_json::json!({ "path": img.to_string_lossy() }))
        .await;

    assert!(
        result.is_error,
        "restrict=true 时工作区外路径必须被文件工具拒绝"
    );
    assert!(
        result.for_llm.contains("outside workspace"),
        "应报 outside workspace，got: {}",
        result.for_llm
    );
}

// ---------------------------------------------------------------------------
// 审计链：path + sha256（P4.2 hash-only 语义）
// ---------------------------------------------------------------------------

#[test]
#[cfg(feature = "security")]
fn audit_chain_records_path_and_sha256() {
    let rt = multithread_runtime();
    rt.block_on(async {
        let (dir, img) = temp_image("nb_audit.png", &png_bytes());
        let plugin = allow_all_plugin(&dir.path().join("audit.jsonl"));

        let text = format!("审计图 {}", img.display());
        let outcome = attach_turn_images(
            &text,
            &[],
            Some(&dir.path().join("ws")),
            "web",
            Some(&plugin),
        );
        assert!(outcome.notes.is_empty());

        let chain = plugin.audit_chain().expect("audit chain enabled");
        let expected_hash = sha256_hex(&png_bytes());
        let total = chain.total_event_count() as usize;

        let mut found = false;
        for i in 0..total {
            let Some(ev) = chain.get_event(i) else {
                continue;
            };
            if ev.operation != "image_attach" {
                continue;
            }
            assert_eq!(ev.tool_name, "read_file");
            assert_eq!(ev.decision, "allowed");
            assert_eq!(
                ev.target,
                img.to_string_lossy().into_owned(),
                "审计事件 target 应记路径"
            );
            assert!(
                ev.reason.contains(&format!("sha256={}", expected_hash)),
                "审计事件 reason 应记 sha256（hash-only），got: {}",
                ev.reason
            );
            found = true;
        }
        assert!(found, "审计链中应有 image_attach 事件（共 {} 条）", total);
    });
}

// ---------------------------------------------------------------------------
// media 引用 / 跨来源去重 / 诚实降级注记
// ---------------------------------------------------------------------------

#[test]
fn media_path_reference_attaches_and_dedups_with_text() {
    let (dir, img) = temp_image("nb_media.png", &png_bytes());
    let ws = dir.path().join("workspace");

    // 文本和 media 指同一张图 → 去重后只附加一次。
    let text = format!("同图 {}", img.display());
    let media = vec![media_ref(&img.to_string_lossy())];
    let outcome = attach_turn_images(&text, &media, Some(&ws), "web", None);

    assert!(outcome.notes.is_empty(), "notes: {:?}", outcome.notes);
    assert_eq!(outcome.ref_strings().len(), 1, "跨来源同图应去重");
}

#[test]
fn media_missing_path_gets_honest_note() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("gone.png");

    let outcome = attach_turn_images(
        "hi",
        &[media_ref(&missing.to_string_lossy())],
        None,
        "web",
        None,
    );

    assert!(outcome.attached.is_empty());
    assert_eq!(outcome.notes.len(), 1);
    assert!(
        outcome.notes[0].contains("文件不存在"),
        "应注明文件不存在，got: {}",
        outcome.notes[0]
    );
}

#[test]
fn media_disguised_text_file_gets_not_image_note() {
    let (dir, fake) = temp_image("nb_fake.png", b"this is not an image at all");
    let _ = dir;

    let outcome = attach_turn_images(
        "hi",
        &[media_ref(&fake.to_string_lossy())],
        None,
        "web",
        None,
    );

    assert!(outcome.attached.is_empty());
    assert!(
        outcome.notes[0].contains("不是图片"),
        "应注明内容不是图片，got: {}",
        outcome.notes[0]
    );
}

#[test]
fn media_url_without_prefetch_gets_honest_note() {
    // T9 后同步入口不再拉网络：直接收到 URL 属调用方误用（loop.rs 先经
    // fetch_url_media 预取），诚实注明而非静默。
    let outcome = attach_turn_images(
        "hi",
        &[media_ref("https://example.com/pic.png")],
        None,
        "web",
        None,
    );

    assert!(outcome.attached.is_empty());
    assert_eq!(outcome.notes.len(), 1);
    assert!(
        outcome.notes[0].contains("fetch_url_media"),
        "应注明需先预取，got: {}",
        outcome.notes[0]
    );
}

// ---------------------------------------------------------------------------
// T9：URL 预取（fetch_url_media）——SSRF 闸 / 下载落盘 / 验真 / 同批去重
// ---------------------------------------------------------------------------

/// 本地 mock 图床：循环 accept（至多 `max_hits` 个连接），每个请求回一段
/// PNG/文本字节。返回 base url（http://127.0.0.1:{port}）。
fn spawn_mock_image_server(
    body: Vec<u8>,
    content_type: &'static str,
    hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    max_hits: usize,
) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for _ in 0..max_hits {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // 读掉请求头（到空行为止）。
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                content_type,
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    format!("http://{}", addr)
}

use super::fetch_url_media;

#[tokio::test]
#[cfg(feature = "security")]
async fn t9_loopback_url_rejected_by_ssrf_guard() {
    {
        // 默认配置 block_localhost=true：回环 URL 在连接前即被拦截。
        let guard = nemesis_security::ssrf::Guard::from_enabled(true);
        let (kept, notes) = fetch_url_media(
            &[media_ref("http://127.0.0.1:9/secret.png")],
            std::path::Path::new("unused_uploads"),
            Some(&guard),
        )
        .await;
        assert!(kept.is_empty(), "SSRF 拒绝项不应保留: {:?}", kept);
        assert_eq!(notes.len(), 1, "notes: {:?}", notes);
        assert!(
            notes[0].contains("SSRF 拦截"),
            "应注明 SSRF 拦截，got: {}",
            notes[0]
        );
    };
}

#[tokio::test]
async fn t9_url_download_lands_and_attaches_to_base64() {
    let png = {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(b"t9-fetched-bytes");
        v
    };
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let base = spawn_mock_image_server(png.clone(), "image/png", hits, 2);

    let dir = tempfile::tempdir().unwrap();
    let uploads = dir.path().join("uploads");
    let (kept, notes) =
        fetch_url_media(&[media_ref(&format!("{}/pic.png", base))], &uploads, None).await;

    assert!(notes.is_empty(), "notes: {:?}", notes);
    assert_eq!(kept.len(), 1);
    // 改写为 uploads 下的本地路径引用（url_ 前缀 + .png）。
    let path = &kept[0].url;
    assert!(
        path.contains("uploads") && path.contains("url_") && path.ends_with(".png"),
        "应改写为 uploads 落盘路径，got: {}",
        path
    );

    // 落盘字节与源一致（写后即落盘）。
    let stored = std::fs::read(path).unwrap();
    assert_eq!(stored, png);

    // 改写后的引用走同步附加链 → 水合出同字节 base64（goal 验证口径）。
    let outcome = attach_turn_images("看图", &kept, None, "web", None);
    assert!(outcome.notes.is_empty(), "notes: {:?}", outcome.notes);
    assert_eq!(outcome.ref_strings().len(), 1);
    let refs = outcome.ref_strings();
    let (images, placeholders) = hydrate_image_refs(&refs);
    assert!(placeholders.is_empty(), "placeholders: {:?}", placeholders);
    assert_eq!(images.len(), 1);
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&images[0].data)
        .unwrap();
    assert_eq!(decoded, png);
}

#[tokio::test]
async fn t9_url_non_image_content_rejected() {
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let base = spawn_mock_image_server(b"definitely not an image".to_vec(), "image/png", hits, 2);

    let dir = tempfile::tempdir().unwrap();
    let uploads = dir.path().join("uploads");
    let (kept, notes) =
        fetch_url_media(&[media_ref(&format!("{}/fake.png", base))], &uploads, None).await;

    assert!(kept.is_empty(), "伪装图片不应保留: {:?}", kept);
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].contains("不是图片"),
        "应注明内容不是图片，got: {}",
        notes[0]
    );
    // 拒绝路径不落盘（sniff 在写之前）。
    assert!(
        !uploads.exists() || uploads.read_dir().unwrap().next().is_none(),
        "拒绝项不应落盘"
    );
}

#[tokio::test]
async fn t9_same_url_fetched_once_and_deduped() {
    let png = {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(b"t9-dedup");
        v
    };
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let base = spawn_mock_image_server(png, "image/png", hits.clone(), 4);

    let dir = tempfile::tempdir().unwrap();
    let uploads = dir.path().join("uploads");
    let url = format!("{}/same.png", base);
    let (kept, notes) = fetch_url_media(&[media_ref(&url), media_ref(&url)], &uploads, None).await;

    assert!(notes.is_empty(), "notes: {:?}", notes);
    assert_eq!(kept.len(), 2);
    // 同批同 URL 只拉一次，两条引用指向同一落盘路径。
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "同 URL 应只请求一次"
    );
    assert_eq!(kept[0].url, kept[1].url);

    // 附加层跨来源去重：两条相同路径只附加一次。
    let outcome = attach_turn_images("hi", &kept, None, "web", None);
    assert!(outcome.notes.is_empty(), "notes: {:?}", outcome.notes);
    assert_eq!(outcome.ref_strings().len(), 1);
}

#[tokio::test]
async fn t9_local_media_passthrough_untouched() {
    let (dir, img) = temp_image("nb_t9_local.png", &png_bytes());
    let uploads = dir.path().join("uploads");
    let local = img.to_string_lossy().into_owned();

    let (kept, notes) = fetch_url_media(&[media_ref(&local)], &uploads, None).await;

    assert!(notes.is_empty());
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].url, local, "本地路径引用应原样透传");
    assert!(!uploads.exists(), "本地路径不应触发 uploads 目录创建");
}

// ---------------------------------------------------------------------------
// S1（2026-09-04 四轮盲审）：下载大小硬上限——Content-Length 预检 + 流式中止
// ---------------------------------------------------------------------------

/// 裸响应 mock：对每个连接原样回放 `response` 字节（测试自拼 HTTP 报文）。
fn spawn_mock_raw_response(response: Vec<u8>, max_hits: usize) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for _ in 0..max_hits {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            // 读掉请求头（到空行为止）；客户端中止后写端可能断管——忽略。
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(&response);
        }
    });
    format!("http://{}", addr)
}

fn uploads_file_count(uploads: &std::path::Path) -> usize {
    uploads.read_dir().map(|rd| rd.count()).unwrap_or(0)
}

#[tokio::test]
async fn s1_content_length_precheck_rejects_before_download() {
    // 谎报超大 Content-Length：预检在下载前即拒（旧实现 resp.bytes() 会
    // 先把整个 body 读进内存才看大小）。
    let huge: u64 = 999_999_999;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {huge}\r\nConnection: close\r\n\r\n"
    );
    let mut response = head.into_bytes();
    response.extend_from_slice(b"tiny");
    let base = spawn_mock_raw_response(response, 2);

    let dir = tempfile::tempdir().unwrap();
    let uploads = dir.path().join("uploads");
    let (kept, notes) =
        fetch_url_media(&[media_ref(&format!("{}/big.png", base))], &uploads, None).await;

    assert!(kept.is_empty(), "超限项不应保留: {:?}", kept);
    assert_eq!(notes.len(), 1, "notes: {:?}", notes);
    assert!(
        notes[0].contains("25MB") && notes[0].contains("Content-Length"),
        "应注明 Content-Length 预检超限，got: {}",
        notes[0]
    );
    assert_eq!(uploads_file_count(&uploads), 0, "拒绝路径不落盘");
}

#[tokio::test]
async fn s1_streaming_read_aborts_when_body_exceeds_cap() {
    // chunked 分块传输（无 Content-Length，预检无从判定）→ 流式累计
    // 超 MAX_IMAGE_BYTES 立即中止，内存占用恒 ≤ 上限 + 单块。
    use super::super::image_path_detector::MAX_IMAGE_BYTES;
    let oversized: Vec<u8> = vec![0u8; (MAX_IMAGE_BYTES + 1024) as usize];
    let mut response = Vec::new();
    response.extend_from_slice(
        b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
    );
    response.extend_from_slice(format!("{:X}\r\n", oversized.len()).as_bytes());
    response.extend_from_slice(&oversized);
    response.extend_from_slice(b"\r\n0\r\n\r\n");
    let base = spawn_mock_raw_response(response, 2);

    let dir = tempfile::tempdir().unwrap();
    let uploads = dir.path().join("uploads");
    let (kept, notes) =
        fetch_url_media(&[media_ref(&format!("{}/flood.png", base))], &uploads, None).await;

    assert!(kept.is_empty(), "流式超限项不应保留: {:?}", kept);
    assert_eq!(notes.len(), 1, "notes: {:?}", notes);
    assert!(
        notes[0].contains("25MB") && notes[0].contains("流式读取中止"),
        "应注明流式中止，got: {}",
        notes[0]
    );
    assert_eq!(uploads_file_count(&uploads), 0, "中止路径不落盘");
}

// ---------------------------------------------------------------------------
// 水合（build_messages 每轮重读的 T6 语义）
// ---------------------------------------------------------------------------

#[test]
fn hydrate_reads_bytes_to_base64() {
    let bytes = png_bytes();
    let (dir, img) = temp_image("nb_hydrate.png", &bytes);
    let refs = vec![img.to_string_lossy().into_owned()];
    let _ = &dir; // 保持目录存活

    let (images, placeholders) = hydrate_image_refs(&refs);

    assert!(placeholders.is_empty(), "placeholders: {:?}", placeholders);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].path, refs[0]);
    assert_eq!(images[0].media_type, "image/png");
    use base64::Engine as _;
    assert_eq!(
        images[0].data,
        base64::engine::general_purpose::STANDARD.encode(&bytes),
        "水合字节应与文件内容 base64 一致"
    );
}

#[test]
fn hydrate_missing_ref_degrades_to_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("vanished.png");
    let refs = vec![missing.to_string_lossy().into_owned()];

    let (images, placeholders) = hydrate_image_refs(&refs);

    assert!(images.is_empty(), "失效引用不得产出图片字节");
    assert_eq!(
        placeholders,
        vec![format!("[图片已失效: {}]", refs[0])],
        "失效引用降级为占位文本行，不静默"
    );
}

#[test]
fn hydrate_disguised_ref_degrades_to_placeholder() {
    let (dir, fake) = temp_image("nb_hydrate_fake.jpg", b"plain text");
    let _ = dir;
    let refs = vec![fake.to_string_lossy().into_owned()];

    let (images, placeholders) = hydrate_image_refs(&refs);

    assert!(images.is_empty());
    assert_eq!(placeholders.len(), 1);
    assert!(placeholders[0].starts_with("[图片已失效:"));
}

// ---------------------------------------------------------------------------
// 注记合并（AttachOutcome::merge_into_text）
// ---------------------------------------------------------------------------

#[test]
fn merge_into_text_appends_notes_without_trailing_blank() {
    let outcome = AttachOutcome {
        attached: Vec::new(),
        notes: vec!["[图片未附加: a]".into(), "[图片未附加: b]".into()],
    };
    assert_eq!(
        outcome.merge_into_text("hello".into()),
        "hello\n[图片未附加: a]\n[图片未附加: b]"
    );
}

#[test]
fn merge_into_text_no_notes_returns_text_unchanged() {
    let outcome = AttachOutcome::default();
    assert_eq!(outcome.merge_into_text("原文".into()), "原文");
}

// ---------------------------------------------------------------------------
// D6 每消息张数上限（二次回归 2026-09-03 补齐：两来源合计 ≤8，超出聚合注记）
// ---------------------------------------------------------------------------

#[test]
fn d6_per_message_cap_drops_overflow_with_aggregated_note() {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for i in 0..10 {
        let p = dir.path().join(format!("nb_cap_{i}.png"));
        std::fs::write(&p, png_bytes()).unwrap();
        paths.push(p);
    }
    let text = paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" 和 ");

    let outcome = attach_turn_images(&text, &[], None, "web", None);

    assert_eq!(
        outcome.attached.len(),
        super::image_path_detector::MAX_IMAGES_PER_MESSAGE,
        "应只附加前 8 张"
    );
    assert_eq!(outcome.notes.len(), 1, "溢出应聚合成一条注记");
    let note = &outcome.notes[0];
    assert!(
        note.contains(&format!(
            "超过每消息 {} 张上限",
            super::image_path_detector::MAX_IMAGES_PER_MESSAGE
        )) && note.contains("已忽略 2 张"),
        "注记应说明上限与忽略数，got: {note}"
    );
    // 先到先得：被忽略的是第 9、10 张。
    for dropped in paths.iter().skip(8) {
        let name = dropped.file_name().unwrap().to_string_lossy();
        assert!(note.contains(name.as_ref()), "注记应列出被忽略文件: {note}");
    }
    for kept in paths.iter().take(8) {
        let name = kept.file_name().unwrap().to_string_lossy();
        assert!(
            !note.contains(name.as_ref()),
            "已附加的文件不应进上限注记: {note}"
        );
    }
}

#[test]
fn d6_cap_covers_media_source_after_text_source() {
    let dir = tempfile::tempdir().unwrap();
    let mut txt_paths = Vec::new();
    for i in 0..8 {
        let p = dir.path().join(format!("nb_txt_{i}.png"));
        std::fs::write(&p, png_bytes()).unwrap();
        txt_paths.push(p);
    }
    // 第 9~16 张走 media 引用（T8 上传形态）：必须不同文件（重复会被去重
    // 静默跳过，到不了上限分支）。
    let mut media_paths = Vec::new();
    for i in 0..8 {
        let p = dir.path().join(format!("nb_med_{i}.png"));
        std::fs::write(&p, png_bytes()).unwrap();
        media_paths.push(p);
    }
    let media: Vec<MediaAttachment> = media_paths
        .iter()
        .map(|p| media_ref(&p.to_string_lossy()))
        .collect();

    let text = txt_paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" 和 ");
    let outcome = attach_turn_images(&text, &media, None, "web", None);

    assert_eq!(
        outcome.attached.len(),
        super::image_path_detector::MAX_IMAGES_PER_MESSAGE,
        "文本来源 8 张已占满，media 来源应全被上限挡住"
    );
    // 先到先得：附加的应全是文本来源。
    for a in &outcome.attached {
        assert!(
            a.raw.contains("nb_txt_"),
            "附加应全是文本来源，got: {}",
            a.raw
        );
    }
    assert_eq!(outcome.notes.len(), 1, "notes: {:?}", outcome.notes);
    assert!(
        outcome.notes[0].contains("已忽略 8 张"),
        "注记应列 media 来源被忽略的 8 张: {}",
        outcome.notes[0]
    );
    for m in &media_paths {
        let name = m.file_name().unwrap().to_string_lossy();
        assert!(
            outcome.notes[0].contains(name.as_ref()),
            "注记应列被忽略的 media 文件: {}",
            outcome.notes[0]
        );
    }
}

#[test]
fn d6_under_cap_no_note() {
    let dir = tempfile::tempdir().unwrap();
    let mut text_parts = Vec::new();
    for i in 0..8 {
        let p = dir.path().join(format!("nb_ok_{i}.png"));
        std::fs::write(&p, png_bytes()).unwrap();
        text_parts.push(p.display().to_string());
    }

    let outcome = attach_turn_images(&text_parts.join(" 和 "), &[], None, "web", None);

    assert!(
        outcome.notes.is_empty(),
        "恰好在上限内不应有注记: {:?}",
        outcome.notes
    );
    assert_eq!(outcome.attached.len(), 8);
}

// ---------------------------------------------------------------------------
// A1（2026-09-03 二次回归）：重定向不跟随——SSRF 闸只校验首跳 URL，
// 302 → 内网地址曾借共享池默认跟随策略绕过闸直打内网。现在 Policy::none()
// + 显式 is_redirection 拦截：诚实注记，重定向目标零连接。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn t9_redirect_response_not_followed() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits2 = hits.clone();
    let _server = std::thread::spawn(move || {
        // 循环 accept（避免 accept-一次-return 与 backlog RST 竞争）。
        // 不 join：第 2 个 accept 永远等不到连接（客户端不跟随重定向），
        // join 会死锁——线程随测试进程消亡（同 spawn_mock_image_server 惯例）。
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            // 302 指向链路本地元数据端点（客户端 Policy::none 不会真连）。
            let resp = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let uploads = dir.path().join("uploads");
    // addr 是 SocketAddr（Display 无 scheme）；fetch_url_media 只预取 http(s)，
    // 缺 scheme 会被门控跳过原样保留——必须显式加 http://。
    let (kept, notes) = fetch_url_media(
        &[media_ref(&format!("http://{addr}/jump.png"))],
        &uploads,
        None,
    )
    .await;

    assert!(kept.is_empty(), "重定向响应不应保留: {:?}", kept);
    assert_eq!(notes.len(), 1, "notes: {:?}", notes);
    assert!(
        notes[0].contains("重定向未跟随"),
        "3xx 应诚实注明未跟随，got: {}",
        notes[0]
    );
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "只应有一次首跳连接（无跟随）"
    );
}

// C3（2026-09-03 二次回归）：扩展名按**内容签名表**定——URL 以 .png 结尾但
// 实际是 JPEG 时，落盘扩展名应为 jpg（Content-Type 同理可伪造）。
#[tokio::test]
async fn t9_extension_from_magic_not_url_or_content_type() {
    // 最小 JPEG 签名（FF D8 FF）开头 + 可识别尾注。
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
    jpeg.extend_from_slice(b"jpeg-bytes-from-mock");
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // URL 叫 .png、Content-Type 谎报 image/png。
    let base = spawn_mock_image_server(jpeg, "image/png", hits, 2);

    let dir = tempfile::tempdir().unwrap();
    let uploads = dir.path().join("uploads");
    let (kept, notes) = fetch_url_media(
        &[media_ref(&format!("{}/fake-name.png", base))],
        &uploads,
        None,
    )
    .await;

    assert!(notes.is_empty(), "notes: {:?}", notes);
    assert_eq!(kept.len(), 1);
    let path = &kept[0].url;
    assert!(
        path.ends_with(".jpg"),
        "落盘扩展名应按 magic 定为 jpg，got: {}",
        path
    );
}
