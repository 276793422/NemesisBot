//! F1（2026-09-22 审计修复）：统一 REST 鉴权中间件测试。
//!
//! 覆盖三块：
//! 1. `auth_exempt_path` 豁免清单真值表（含近邻前缀不误豁免）；
//! 2. `extract_request_token` 三载体（头 → 查询参数 → Bearer）与空值跳过；
//! 3. `build_router` + `tower::ServiceExt::oneshot` 端到端：非空 token 下
//!    `/api/status` 无/错 token 401、三载体正确 token 200、豁免路径不过闸、
//!    `workflow_chat=` 只豁免 WS 路径（其余路径照常 401）、空 expected token
//!    恒放行（默认部署零影响）。

use super::*;

// ============================================================
// auth_exempt_path 真值表
// ============================================================

#[test]
fn test_auth_exempt_path_truth_table() {
    // 豁免：探活
    assert!(auth_exempt_path("/health"));
    assert!(auth_exempt_path("/api/health"));
    // 豁免：token 即凭据的公开端点（前缀形态）
    assert!(auth_exempt_path("/api/share/abc123"));
    assert!(auth_exempt_path("/api/board/asset/some-ref"));
    // 豁免：workflow-chat 公共元数据 + 密码校验（per-workflow 密码即凭据）
    assert!(auth_exempt_path("/api/workflow/chat/wf-001"));
    // 豁免：SDK 公开静态产物下载（外部消费者无 dashboard token；
    // integration-test ui/p2_sdk_http 钉住公开契约）
    assert!(auth_exempt_path("/api/sdk/export"));
    assert!(auth_exempt_path("/api/sdk/pip"));
    // 豁免：webhook 回调（外部服务 GitHub/Slack 等裸调；自有安全层 =
    // 可选 HMAC + per-IP 限流 + 审计；缺陷 11 修复 2026-09-23）
    assert!(auth_exempt_path("/api/workflow/webhook/webhook_flow"));
    assert!(auth_exempt_path("/api/workflow/webhook/a/b"));

    // 不豁免：控制面 REST
    assert!(!auth_exempt_path("/api/status"));
    assert!(!auth_exempt_path("/api/config"));
    assert!(!auth_exempt_path("/api/chat/stream"));
    assert!(!auth_exempt_path("/"));
    assert!(!auth_exempt_path("/dashboard/overview"));
}

#[test]
fn test_auth_exempt_path_near_miss_prefixes_not_exempt() {
    // 近邻前缀不误豁免：前缀匹配必须带边界（`/` 或精确串）
    assert!(!auth_exempt_path("/api/sharefoo"));
    assert!(!auth_exempt_path("/api/healthz"));
    assert!(!auth_exempt_path("/api/board/assets/ref"));
    assert!(!auth_exempt_path("/api/workflow/chatty"));
    assert!(!auth_exempt_path("/api/workflow/webhooks/wf"));
    assert!(!auth_exempt_path("/api/sdkfoo/x"));
    // 精确匹配不认子路径/尾随斜杠变体
    assert!(!auth_exempt_path("/health/"));
}

// ============================================================
// extract_request_token 三载体
// ============================================================

fn req_with(uri: &str, headers: &[(&str, &str)]) -> axum::extract::Request {
    let mut builder = axum::http::Request::builder().uri(uri);
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    builder.body(axum::body::Body::empty()).unwrap()
}

#[test]
fn test_extract_request_token_header_first() {
    let req = req_with(
        "/api/status?token=from-query",
        &[
            ("x-auth-token", "from-header"),
            ("authorization", "Bearer from-bearer"),
        ],
    );
    assert_eq!(
        extract_request_token(&req).as_deref(),
        Some("from-header"),
        "X-Auth-Token 头优先级最高"
    );
}

#[test]
fn test_extract_request_token_query_fallback() {
    let req = req_with("/api/status?token=from-query", &[]);
    assert_eq!(extract_request_token(&req).as_deref(), Some("from-query"));
}

#[test]
fn test_extract_request_token_bearer_fallback() {
    let req = req_with("/api/status", &[("authorization", "Bearer from-bearer")]);
    assert_eq!(extract_request_token(&req).as_deref(), Some("from-bearer"));
}

#[test]
fn test_extract_request_token_empty_query_skipped() {
    // 空 token 查询参数视同缺省——继续找 Bearer，找不到则 None
    let req = req_with(
        "/api/status?token=",
        &[("authorization", "Bearer from-bearer")],
    );
    assert_eq!(extract_request_token(&req).as_deref(), Some("from-bearer"));
    let req = req_with("/api/status?token=", &[]);
    assert_eq!(extract_request_token(&req), None);
}

#[test]
fn test_extract_request_token_none() {
    let req = req_with("/api/status", &[]);
    assert_eq!(extract_request_token(&req), None);
    // 非 Bearer 的 Authorization 不认
    let req = req_with("/api/status", &[("authorization", "Basic dXNlcjpwYXNz")]);
    assert_eq!(extract_request_token(&req), None);
}

// ============================================================
// build_router 端到端（oneshot）
// ============================================================

/// 非空 token 的 router + oneshot 请求（可选带 token 头/查询参数）。
async fn router_oneshot(
    uri: &str,
    token_header: Option<&str>,
    bearer: Option<&str>,
) -> axum::response::Response {
    let config = WebServerConfig {
        auth_token: "sekret".to_string(),
        ..Default::default()
    };
    let app = WebServer::new(config).build_router();
    let mut builder = axum::http::Request::builder().uri(uri);
    if let Some(t) = token_header {
        builder = builder.header("x-auth-token", t);
    }
    if let Some(b) = bearer {
        builder = builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {b}"));
    }
    use tower::ServiceExt;
    app.oneshot(builder.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn test_auth_missing_token_rejected_401() {
    let resp = router_oneshot("/api/status", None, None).await;
    assert_eq!(resp.status(), 401, "无 token 的控制面请求必须 401");
}

#[tokio::test]
async fn test_auth_wrong_token_rejected_401() {
    for (label, resp) in [
        (
            "查询参数错 token",
            router_oneshot("/api/status?token=wrong", None, None).await,
        ),
        (
            "头错 token",
            router_oneshot("/api/status", Some("wrong"), None).await,
        ),
        (
            "Bearer 错 token",
            router_oneshot("/api/status", None, Some("wrong")).await,
        ),
    ] {
        assert_eq!(resp.status(), 401, "{label} 必须 401");
    }
}

#[tokio::test]
async fn test_auth_valid_token_three_carriers_pass() {
    for (label, resp) in [
        (
            "头 token",
            router_oneshot("/api/status", Some("sekret"), None).await,
        ),
        (
            "查询参数 token",
            router_oneshot("/api/status?token=sekret", None, None).await,
        ),
        (
            "Bearer token",
            router_oneshot("/api/status", None, Some("sekret")).await,
        ),
    ] {
        assert_eq!(resp.status(), 200, "{label} 正确 token 必须放行");
    }
}

#[tokio::test]
async fn test_auth_exempt_health_passes_without_token() {
    let resp = router_oneshot("/health", None, None).await;
    assert_eq!(resp.status(), 200);
    let resp = router_oneshot("/api/health", None, None).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_auth_share_endpoint_passes_auth_layer() {
    // L4 分享：token 即凭据——不持 dashboard token 也不该 401；
    // 未知分享号按 share 语义 404（非 401 即证明过了鉴权闸）。
    let resp = router_oneshot("/api/share/nonexistent-token", None, None).await;
    assert_ne!(resp.status(), 401);
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_auth_workflow_chat_query_does_not_bypass_other_paths() {
    // workflow_chat= 豁免只对 WS 路径成立（复核 2026-09-22 收敛）——其余
    // 路径拼上该查询键**不得**绕过 auth 闸（否则任何端点皆可旁路）。
    let resp = router_oneshot("/api/status?workflow_chat=1", None, None).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_auth_workflow_chat_upgrade_on_ws_path_bypasses() {
    // WS 路径本身：workflow-chat 升级（?workflow_chat=&pwd=）不带 dashboard
    // token，靠 handler 内 per-workflow 密码闸——middleware 放行。普通 GET
    // （无 Upgrade 头）会被 WebSocketUpgrade 提取器 400 拒掉，但那已说明
    // 过了 auth 闸（否则是 401）。
    let resp = router_oneshot("/ws?workflow_chat=0&pwd=x", None, None).await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_auth_empty_expected_token_allows_all() {
    // 空 expected token（默认部署）恒放行——verify_token 文档化约定，
    // integration-test / test-harness 全绿的前提。
    let config = WebServerConfig::default();
    let app = WebServer::new(config).build_router();
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200, "空 expected token 不设闸");
}
