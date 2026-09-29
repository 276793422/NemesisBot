//! generate_image 工具单测（集群专业职能框架 M4）。
//!
//! 落盘链路用 wiremock 假端点跑真实 IO（临时目录）；路径 sanitization
//! 纯函数直测。注册闸（resolve_image_model）两态也在本文件。

use super::*;
use crate::agent_factory::resolve_image_model;
use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::Tool as _;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ctx() -> RequestContext {
    RequestContext::new("test", "chat", "tester", "sess")
}

/// 一张 1×1 红色 PNG（最小合法载荷，b64 编码）。
const TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn tool(api_base: &str, tmp: &std::path::Path) -> GenerateImageTool {
    GenerateImageTool::new(
        api_base.to_string(),
        "sk-test".to_string(),
        "img-model".to_string(),
        30,
        tmp.join("images"),
    )
}

#[tokio::test]
async fn generates_writes_file_and_returns_path_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"b64_json": TINY_PNG_B64}],
        })))
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let t = tool(&format!("{}/v1", server.uri()), tmp.path());

    let out = t
        .execute(
            r#"{"prompt": "一只猫", "output": "mockups/login.png"}"#,
            &ctx(),
        )
        .await
        .expect("成功臂");
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    let written = tmp.path().join("images").join("mockups").join("login.png");
    assert_eq!(parsed["path"].as_str().unwrap(), written.to_string_lossy());
    assert_eq!(parsed["model"].as_str().unwrap(), "img-model");
    let bytes = std::fs::read(&written).expect("产物落盘");
    assert_eq!(&bytes[..4], b"\x89PNG");
}

#[tokio::test]
async fn default_output_name_is_generated_under_output_dir() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"b64_json": TINY_PNG_B64}],
        })))
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let t = tool(&server.uri(), tmp.path());

    let out = t
        .execute(r#"{"prompt": "图"}"#, &ctx())
        .await
        .expect("成功臂");
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    let p = std::path::Path::new(parsed["path"].as_str().unwrap());
    assert!(
        p.starts_with(tmp.path().join("images")),
        "产物必须在 images/ 下: {p:?}"
    );
    assert_eq!(p.extension().and_then(|e| e.to_str()), Some("png"));
}

#[tokio::test]
async fn rejects_path_traversal_and_absolute_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tool("http://127.0.0.1:9", tmp.path());
    for bad in [
        "../evil.png",
        "..\\evil.png",
        "C:\\abs\\x.png",
        "/etc/x.png",
        "con.png",
        "sub/../x.png",
        "",
        "   ",
        "just-a-dir/",
        "x.png:hidden",
    ] {
        let err = t
            .execute(&format!(r#"{{"prompt": "x", "output": {bad:?}}}"#), &ctx())
            .await
            .expect_err(&format!("output={bad:?} 应被拒绝"));
        assert!(err.contains("非法 output"), "output={bad:?}: {err}");
    }
    // 拒绝发生在任何网络/落盘之前——images/ 目录不应被创建。
    assert!(!tmp.path().join("images").exists());
}

#[tokio::test]
async fn upstream_error_is_surfaced_without_file_writes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let t = tool(&server.uri(), tmp.path());

    let err = t
        .execute(r#"{"prompt": "x"}"#, &ctx())
        .await
        .expect_err("5xx → Err");
    assert!(err.contains("500"), "{err}");
    assert!(!tmp.path().join("images").exists());
}

#[tokio::test]
async fn missing_prompt_is_honest_error() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tool("http://127.0.0.1:9", tmp.path());
    let err = t
        .execute(r#"{"size": "1024x1024"}"#, &ctx())
        .await
        .expect_err("缺 prompt → Err");
    assert!(err.contains("prompt"), "{err}");
}

#[test]
fn sanitize_rel_accepts_normal_relative_paths() {
    assert_eq!(
        GenerateImageTool::sanitize_rel("a/b.png"),
        Some(PathBuf::from("a/b.png"))
    );
    assert_eq!(
        GenerateImageTool::sanitize_rel(" x.png "),
        Some(PathBuf::from("x.png"))
    );
    // Windows 反斜杠习惯输入归一为分隔符后接受。
    assert_eq!(
        GenerateImageTool::sanitize_rel("mockups\\login.png"),
        Some(PathBuf::from("mockups/login.png"))
    );
    assert_eq!(GenerateImageTool::sanitize_rel("../x.png"), None);
    assert_eq!(GenerateImageTool::sanitize_rel("a/../x.png"), None);
    assert_eq!(GenerateImageTool::sanitize_rel("dir/"), None);
    assert_eq!(GenerateImageTool::sanitize_rel("."), None);
}

// ---------------------------------------------------------------------------
// 注册闸 resolve_image_model 两态
// ---------------------------------------------------------------------------

fn cfg_with(entries: serde_json::Value, image_gen_model: Option<&str>) -> nemesis_config::Config {
    let mut raw = serde_json::json!({
        "model_list": entries,
        "tools": { "image_gen": { "model": image_gen_model } },
    });
    if image_gen_model.is_none() {
        raw["tools"]["image_gen"] = serde_json::json!({});
    }
    serde_json::from_value(raw).expect("测试 config 反序列化")
}

#[test]
fn resolve_prefers_unique_images_openai_entry() {
    let cfg = cfg_with(
        serde_json::json!([
            {"model_name": "chat", "model": "glm-5", "api_base": "http://a", "api_key": "k"},
            {"model_name": "img", "model": "dall-e", "api_base": "http://b/v1", "api_key": "k2", "protocol": "images-openai"}
        ]),
        None,
    );
    let (base, key, model) = resolve_image_model(&cfg).expect("唯一 images-openai 条目");
    assert_eq!(base, "http://b/v1");
    assert_eq!(key, "k2");
    assert_eq!(model, "dall-e");
}

#[test]
fn resolve_alias_and_reject_arms() {
    let img = serde_json::json!([
        {"model_name": "chat", "model": "glm-5", "api_base": "http://a", "api_key": "k"},
        {"model_name": "img-a", "model": "dall-e", "api_base": "http://a/v1", "api_key": "k", "protocol": "images-openai"},
        {"model_name": "img-b", "model": "flux", "api_base": "http://b/v1", "api_key": "k", "protocol": "images-openai"}
    ]);

    // 歧义（两条 images-openai 无别名）→ 诚实报错。
    let cfg = cfg_with(img.clone(), None);
    assert!(resolve_image_model(&cfg).unwrap_err().contains("歧义"));

    // 别名命中 → 精确取该条。
    let cfg = cfg_with(img.clone(), Some("img-b"));
    assert_eq!(resolve_image_model(&cfg).unwrap().2, "flux");

    // 别名不存在 → 诚实报错。
    let cfg = cfg_with(img.clone(), Some("nope"));
    assert!(resolve_image_model(&cfg).unwrap_err().contains("不存在"));

    // 别名指向非图像协议条目 → 诚实报错（工具只说 images/generations wire）。
    let cfg = cfg_with(img.clone(), Some("chat"));
    assert!(
        resolve_image_model(&cfg)
            .unwrap_err()
            .contains("images-openai")
    );

    // 无图像条目 → 诚实报错。
    let cfg = cfg_with(
        serde_json::json!([{"model_name": "chat", "model": "glm-5", "api_base": "http://a", "api_key": "k"}]),
        None,
    );
    assert!(
        resolve_image_model(&cfg)
            .unwrap_err()
            .contains("未配置图像模型")
    );

    // 缺 api_base → 诚实报错。
    let cfg = cfg_with(
        serde_json::json!([{"model_name": "img", "model": "dall-e", "api_key": "k", "protocol": "images-openai"}]),
        None,
    );
    assert!(resolve_image_model(&cfg).unwrap_err().contains("api_base"));
}
