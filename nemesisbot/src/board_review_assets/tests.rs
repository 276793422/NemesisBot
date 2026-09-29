//! board_review_assets 单测（M5）：bundle 提取 / 本地命中免网络 / HTTP
//! 取回 + sha256 校验 / 张数与单张上限 / 视觉评审裸调用（wiremock +
//! http-compat 真管线：image part 上行断言 + 解析回灌重试 + 3 轮失败）。

use super::*;
use nemesis_board::Actor;
use std::sync::Arc;
use wiremock::matchers::{method, path};

/// 1×1 红 PNG（合法魔数；本模块不做 magic 校验，真实字节利于 realism）。
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn make_bundle(ref_name: &str, sha: &str, node_url: &str) -> AssetTokenBundle {
    AssetTokenBundle {
        asset_ref: ref_name.to_string(),
        asset_token: "tok".to_string(),
        expires_at: 4102444800,
        node_url: node_url.to_string(),
        node_id: "node-a".to_string(),
        sha256: sha.to_string(),
        size: 0,
    }
}

fn bundle_text(ref_name: &str, sha: &str, node_url: &str) -> String {
    serde_json::to_string(&make_bundle(ref_name, sha, node_url)).unwrap()
}

fn comment(ctype: CommentType, content: String) -> Comment {
    Comment {
        id: 1,
        issue_id: 1,
        author: Actor::agent("w"),
        content,
        parent_id: None,
        ctype,
        created_at: 0,
    }
}

fn test_cluster() -> Arc<nemesis_cluster::cluster::Cluster> {
    Arc::new(nemesis_cluster::cluster::Cluster::new(
        nemesis_cluster::types::ClusterConfig::default(),
    ))
}

fn seed_assets(tmp: &std::path::Path, name: &str, bytes: &[u8]) {
    let assets = tmp.join("board").join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join(name), bytes).unwrap();
}

// ---------------------------------------------------------------------------
// bundle 提取
// ---------------------------------------------------------------------------

#[test]
fn extract_bundles_filter_by_image_ext_and_shape() {
    let sha = nemesis_board::sha256_bytes(b"x");
    let good = bundle_text("mock.png", &sha, "http://127.0.0.1:1");
    let pdf = bundle_text("report.pdf", &sha, "http://127.0.0.1:1");
    let text = format!(
        "交付如下：\n{good}\n另有文档：{pdf}\n坏 JSON：{{broken\n普通对象：{{\"verdict\":\"PASS\"}}"
    );
    let out = extract_image_bundles_from(&text);
    assert_eq!(out.len(), 1, "只收图像扩展名的 bundle: {out:?}");
    assert_eq!(out[0].asset_ref, "mock.png");
}

#[test]
fn extract_bundles_multiple_in_order() {
    let sha = nemesis_board::sha256_bytes(b"x");
    let text = format!(
        "{} {}",
        bundle_text("a.png", &sha, "http://127.0.0.1:1"),
        bundle_text("b.jpg", &sha, "http://127.0.0.1:1")
    );
    let out = extract_image_bundles_from(&text);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].asset_ref, "a.png");
    assert_eq!(out[1].asset_ref, "b.jpg");
}

// ---------------------------------------------------------------------------
// collect_review_images：本地命中 / HTTP 取回 / 过滤 / 上限
// ---------------------------------------------------------------------------

#[tokio::test]
async fn collect_local_hit_skips_network() {
    let tmp = tempfile::tempdir().unwrap();
    seed_assets(tmp.path(), "shot.png", TINY_PNG);
    let sha = nemesis_board::sha256_bytes(TINY_PNG);
    // node_url 指向不可达端口：若误走网络会失败，本地命中必须零网络。
    let c = comment(
        CommentType::Delivery,
        bundle_text("shot.png", &sha, "http://127.0.0.1:1"),
    );
    let imgs = collect_review_images(tmp.path(), &test_cluster(), &[c]).await;
    assert_eq!(imgs.len(), 1);
    assert_eq!(imgs[0].bytes, TINY_PNG);
    assert!(imgs[0].path.ends_with("shot.png"));
}

#[tokio::test]
async fn collect_sha_mismatch_local_refetches_and_unreachable_skips() {
    let tmp = tempfile::tempdir().unwrap();
    seed_assets(tmp.path(), "shot.png", TINY_PNG);
    // 本地实体与 bundle sha 不符（上次取回后文件被改）→ 必须重取；
    // 端点不可达 → 诚实跳过，不拿脏文件凑数。
    let wrong_sha = nemesis_board::sha256_bytes(b"stale");
    let c = comment(
        CommentType::Delivery,
        bundle_text("shot.png", &wrong_sha, "http://127.0.0.1:1"),
    );
    let imgs = collect_review_images(tmp.path(), &test_cluster(), &[c]).await;
    assert!(imgs.is_empty());
}

#[tokio::test]
async fn collect_fetches_via_http_and_verifies_sha() {
    let tmp = tempfile::tempdir().unwrap();
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/api/board/asset/art.png"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(TINY_PNG))
        .mount(&server)
        .await;
    let sha = nemesis_board::sha256_bytes(TINY_PNG);
    let c = comment(
        CommentType::Delivery,
        bundle_text("art.png", &sha, &server.uri()),
    );
    let imgs = collect_review_images(tmp.path(), &test_cluster(), &[c]).await;
    assert_eq!(imgs.len(), 1);
    assert_eq!(imgs[0].bytes, TINY_PNG);
    // 落盘断言：实体在 assets 目录（评审降级注记里给人工看的路径真实可用）。
    assert!(
        tmp.path()
            .join("board")
            .join("assets")
            .join("art.png")
            .is_file()
    );
}

#[tokio::test]
async fn collect_skips_status_and_system_comments() {
    let tmp = tempfile::tempdir().unwrap();
    seed_assets(tmp.path(), "s.png", TINY_PNG);
    let sha = nemesis_board::sha256_bytes(TINY_PNG);
    let text = bundle_text("s.png", &sha, "http://127.0.0.1:1");
    // 机械评论里的 bundle 不当交付数据；若误扫，本地命中会返回 1 张。
    let imgs = collect_review_images(
        tmp.path(),
        &test_cluster(),
        &[
            comment(CommentType::StatusChange, text.clone()),
            comment(CommentType::System, text),
        ],
    )
    .await;
    assert!(imgs.is_empty());
}

#[tokio::test]
async fn collect_caps_at_max_images() {
    let tmp = tempfile::tempdir().unwrap();
    let sha = nemesis_board::sha256_bytes(TINY_PNG);
    let mut comments = Vec::new();
    for i in 0..6 {
        let name = format!("f{i}.png");
        seed_assets(tmp.path(), &name, TINY_PNG);
        comments.push(comment(
            CommentType::Delivery,
            bundle_text(&name, &sha, "http://127.0.0.1:1"),
        ));
    }
    let imgs = collect_review_images(tmp.path(), &test_cluster(), &comments).await;
    assert_eq!(imgs.len(), MAX_REVIEW_IMAGES);
    assert_eq!(imgs[0].ref_name, "f0.png", "按评论序先到先得");
}

#[tokio::test]
async fn collect_skips_oversize_image() {
    let tmp = tempfile::tempdir().unwrap();
    let big = vec![0u8; MAX_IMAGE_BYTES as usize + 1];
    seed_assets(tmp.path(), "big.png", &big);
    let sha = nemesis_board::sha256_bytes(&big);
    let c = comment(
        CommentType::Delivery,
        bundle_text("big.png", &sha, "http://127.0.0.1:1"),
    );
    let imgs = collect_review_images(tmp.path(), &test_cluster(), &[c]).await;
    assert!(imgs.is_empty(), "超单张上限诚实跳过");
}

// ---------------------------------------------------------------------------
// run_review_llm_vision：http-compat 真管线
// ---------------------------------------------------------------------------

fn vision_provider(base: &str) -> Arc<dyn nemesis_providers::router::LLMProvider> {
    let cfg = nemesis_providers::factory::FactoryConfig {
        llm_ref: "test/vision-m".to_string(),
        api_key: "k".to_string(),
        api_base: base.to_string(),
        ..Default::default()
    };
    nemesis_providers::factory::create_provider(&cfg).unwrap()
}

const PASS_JSON: &str = "{\"verdict\":\"PASS\",\"reasons\":[\"图面符合验收\"]}";
const GARBAGE: &str = "我不会输出 JSON";

fn chat_mock(body: serde_json::Value) -> wiremock::Mock {
    wiremock::Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": body}, "finish_reason": "stop"}]
            })),
        )
}

#[tokio::test]
async fn vision_review_attaches_image_parts_and_parses() {
    let server = wiremock::MockServer::start().await;
    chat_mock(serde_json::json!(PASS_JSON)).mount(&server).await;
    let provider = vision_provider(&server.uri());
    let img = ReviewImage {
        ref_name: "mock.png".to_string(),
        bytes: TINY_PNG.to_vec(),
        path: std::path::PathBuf::from("x/mock.png"),
    };
    let mut prompt = "评审这张图".to_string();
    let out = run_review_llm_vision(provider, "vision-m", "sys", &mut prompt, &[img])
        .await
        .unwrap();
    assert_eq!(out.verdict, nemesis_board::ReviewVerdict::Pass);
    // 上行断言：image_url data URI part 与文本 prompt 都在请求体里。
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body = String::from_utf8_lossy(&reqs[0].body);
    assert!(body.contains("image_url"), "got: {body}");
    assert!(body.contains("data:image/png;base64,"));
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, TINY_PNG);
    assert!(body.contains(&b64));
    assert!(body.contains("评审这张图"));
    assert!(prompt.contains("评审这张图"), "成功路径不改写 prompt");
}

#[tokio::test]
async fn vision_review_retry_recovers_with_replay_note() {
    let server = wiremock::MockServer::start().await;
    // 挂载序：先挂 garbage（up_to 2，wiremock 按挂载序先匹配）耗尽后
    // 落到 valid → 前两轮 garbage、第三轮 valid。
    wiremock::Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": GARBAGE}, "finish_reason": "stop"}]
            })),
        )
        .up_to_n_times(2)
        .mount(&server)
        .await;
    chat_mock(serde_json::json!(PASS_JSON)).mount(&server).await;
    let provider = vision_provider(&server.uri());
    let img = ReviewImage {
        ref_name: "a.jpg".to_string(),
        bytes: TINY_PNG.to_vec(),
        path: std::path::PathBuf::from("x/a.jpg"),
    };
    let mut prompt = "评审".to_string();
    let out = run_review_llm_vision(provider, "vision-m", "sys", &mut prompt, &[img])
        .await
        .unwrap();
    assert_eq!(out.verdict, nemesis_board::ReviewVerdict::Pass);
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 3, "解析失败回灌重试 ≤2（共 3 轮）");
    // 回灌语义：第三轮请求体带前轮原输出（build_retry_prompt）。
    let last = String::from_utf8_lossy(&reqs[2].body);
    assert!(last.contains(GARBAGE), "回灌带前轮输出: {last}");
    // 图 parts 在重试轮原样保留。
    assert!(last.contains("data:image/jpeg;base64,"));
}

#[tokio::test]
async fn vision_review_three_garbage_rounds_fail_honest() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": GARBAGE}, "finish_reason": "stop"}]
            })),
        )
        .mount(&server)
        .await;
    let provider = vision_provider(&server.uri());
    let img = ReviewImage {
        ref_name: "a.webp".to_string(),
        bytes: TINY_PNG.to_vec(),
        path: std::path::PathBuf::from("x/a.webp"),
    };
    let mut prompt = "评审".to_string();
    let err = run_review_llm_vision(provider, "vision-m", "sys", &mut prompt, &[img])
        .await
        .unwrap_err();
    assert!(err.contains("3 轮"), "{err}");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}
