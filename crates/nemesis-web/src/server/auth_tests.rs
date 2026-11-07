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
    // 豁免：relay 自有信任边界（设备 hello token / 设备访问令牌 cookie /
    // admin cookie 各自把关；BUG-20261107-006：全量形态同样挂 route_layer，
    // 此前漏列 = dashboard token 非空时 relay 整体 401）
    assert!(auth_exempt_path("/bridge"));
    assert!(auth_exempt_path("/d/dev1"));
    assert!(auth_exempt_path("/d/dev1/"));
    assert!(auth_exempt_path("/d/dev1/any/path"));
    assert!(auth_exempt_path("/relay"));
    assert!(auth_exempt_path("/relay/login"));
    assert!(auth_exempt_path("/api/relay/status"));

    // 不豁免：控制面 REST
    assert!(!auth_exempt_path("/api/status"));
    assert!(!auth_exempt_path("/api/config"));
    assert!(!auth_exempt_path("/api/chat/stream"));
    // 不豁免：批次三【中继通道】dashboard 端点（与 /api/status 同信任边界，
    // relay 未注入时也要能诚实报错——绝不进豁免清单）
    assert!(!auth_exempt_path("/api/relay/overview"));
    assert!(!auth_exempt_path("/api/relay/enabled"));
    assert!(!auth_exempt_path("/api/relay/client/reconnect"));
    // S5（2026-10-09）：OpenAI 兼容端点触发 agent 执行——与 dashboard
    // 同一信任边界，绝不进豁免清单（Bearer 头本就被中间件接受）。
    assert!(!auth_exempt_path("/v1/chat/completions"));
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
    // relay 路径近邻不误豁免（前缀边界）
    assert!(!auth_exempt_path("/bridgeX"));
    assert!(!auth_exempt_path("/bridgehax"));
    assert!(!auth_exempt_path("/relayx"));
    assert!(!auth_exempt_path("/relay/loginx"));
    assert!(!auth_exempt_path("/api/relay/statusx"));
    assert!(!auth_exempt_path("/api/relay/status/extra"));
    assert!(!auth_exempt_path("/dxx"));
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

// ============================================================
// relay 自有信任边界（BUG-20261107-006）
// ============================================================

/// 非空 dashboard token + **注入 relay** 的 router（全量 gateway hub 形态，
/// web_init.rs set_relay 路径）+ oneshot 请求（可选带 token 头）。
async fn relay_router_oneshot(uri: &str, token_header: Option<&str>) -> axum::response::Response {
    let config = WebServerConfig {
        auth_token: "sekret".to_string(),
        ..Default::default()
    };
    let mut server = WebServer::new(config);
    server.set_relay(std::sync::Arc::new(crate::relay::RelayServer::new(
        "bridge-token".to_string(),
        true,
    )));
    let app = server.build_router();
    let mut builder = axum::http::Request::builder().uri(uri);
    if let Some(t) = token_header {
        builder = builder.header("x-auth-token", t);
    }
    use tower::ServiceExt;
    app.oneshot(builder.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn test_auth_relay_out_of_band_paths_not_dashboard_gated() {
    // 设备/管理员只持 relay 自有凭据（hello token / 设备 cookie / admin
    // cookie），不持 dashboard token——relay 自有信任边界的路由面不得被
    // 统一鉴权 401（server.rs build_router 注释：挂上会把 relay 整体 401）。
    // - /relay：无 admin cookie → 登录页 200；
    // - /d/dev1/：设备未桥入 → relay 语义 503（设备离线页）；
    // - /bridge：普通 GET（无 Upgrade 头）→ WebSocketUpgrade 提取器 400，
    //   同 test_auth_workflow_chat_upgrade_on_ws_path_bypasses 判别法。
    for (label, uri, expected) in [
        ("relay 状态页登录", "/relay", 200),
        ("设备隧道入口", "/d/dev1/", 503),
        ("桥 WS 升级（提取器拒裸 GET）", "/bridge", 400),
    ] {
        let resp = relay_router_oneshot(uri, None).await;
        assert_eq!(
            resp.status(),
            expected,
            "{label} 必须过 auth 闸到达 relay 自有语义（{expected}），401 = 漏豁免"
        );
    }
}

#[tokio::test]
async fn test_auth_relay_status_api_reaches_own_admin_gate() {
    // /api/relay/status 豁免 dashboard 闸后，无 admin cookie 由 relay 自己的
    // admin gate 出 401（响应体是 relay 语义而非 auth middleware 的
    // {"error":"unauthorized"} JSON——判别法：带 dashboard token 仍 401，
    // 因为 handler 只认 admin cookie 不认 dashboard token）。
    let resp = relay_router_oneshot("/api/relay/status", Some("sekret")).await;
    assert_eq!(
        resp.status(),
        401,
        "带 dashboard token 也必须 401（admin cookie 才是 /api/relay/status 的凭据）"
    );
}

#[tokio::test]
async fn test_auth_relay_dashboard_family_stays_gated() {
    // 批次三端点（overview/enabled/client/reconnect）是 dashboard 信任边界
    // （build_router 注释：与 /api/status 同语义）——不持 dashboard token
    // 必须 401（回归锁：不得随 relay 豁免一起放开）。
    for (label, uri) in [
        ("overview", "/api/relay/overview"),
        ("client/reconnect", "/api/relay/client/reconnect"),
    ] {
        let resp = relay_router_oneshot(uri, None).await;
        assert_eq!(resp.status(), 401, "{label} 必须留在 dashboard 鉴权下");
    }
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

// ============================================================
// LLM 代理调用方 key 回落臂（2026-11-03 LLM代理调用方key管理 P3）
// ============================================================

/// 带代理 key 表的 router + oneshot（workspace 指向临时目录，key 文件由
/// 调用方先落盘； TempDir 句柄返回给调用方持有防提前清理）。
async fn proxy_router_oneshot(
    dir: &tempfile::TempDir,
    uri: &str,
    bearer: Option<&str>,
) -> axum::response::Response {
    let config = WebServerConfig {
        auth_token: "sekret".to_string(),
        workspace: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let app = WebServer::new(config).build_router();
    let mut builder = axum::http::Request::builder().uri(uri);
    if let Some(b) = bearer {
        builder = builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {b}"));
    }
    use tower::ServiceExt;
    app.oneshot(builder.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap()
}

fn seed_proxy_key(dir: &tempfile::TempDir) {
    nemesis_config::proxy_keys_file::add_key(
        &dir.path().join("config"),
        nemesis_config::proxy_keys_file::ProxyKeyEntry {
            name: "bot-a".to_string(),
            key: "nbk_aaa".to_string(),
            enabled: true,
            created_at: 0,
            note: String::new(),
        },
    )
    .unwrap();
}

#[tokio::test]
async fn test_proxy_key_hit_passes_proxy_lane() {
    let dir = tempfile::tempdir().unwrap();
    seed_proxy_key(&dir);
    // 代理 key（Bearer）打代理 lane：非 401 即过了鉴权闸（空 body 进
    // completions handler 按 JSON 解析失败回 4xx 业务错，非鉴权错）。
    let resp = proxy_router_oneshot(&dir, "/v1/chat/completions", Some("nbk_aaa")).await;
    assert_ne!(resp.status(), 401, "代理 key 命中必须放行（非 401）");
}

#[tokio::test]
async fn test_proxy_key_rejected_on_dashboard_path() {
    let dir = tempfile::tempdir().unwrap();
    seed_proxy_key(&dir);
    // 代理 key 打 Dashboard API（白名单外）：match 恒 None → 落回主 token
    // 校验（key ≠ sekret）→ 必 401，防借道提权。
    let resp = proxy_router_oneshot(&dir, "/api/status", Some("nbk_aaa")).await;
    assert_eq!(resp.status(), 401, "代理 key 打白名单外路径必须 401");
}

#[tokio::test]
async fn test_proxy_key_master_token_unaffected() {
    let dir = tempfile::tempdir().unwrap();
    seed_proxy_key(&dir);
    // 共享主 token 老行为完全不变（回归钉子）。
    let resp = proxy_router_oneshot(&dir, "/api/status", Some("sekret")).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_proxy_key_disabled_rejected_without_restart() {
    let dir = tempfile::tempdir().unwrap();
    seed_proxy_key(&dir);
    let resp = proxy_router_oneshot(&dir, "/v1/chat/completions", Some("nbk_aaa")).await;
    assert_ne!(resp.status(), 401);
    // 禁用即失效（热生效：同一 router 不重建）。
    nemesis_config::proxy_keys_file::set_enabled(&dir.path().join("config"), "bot-a", false)
        .unwrap();
    // BUG-20261103：原 sleep(20ms)「不赌 mtime 粒度」仍是赌——Windows CI 实录
    // 20ms 内文件时间可同 tick（shard9 禁用未生效 → 放行 → GET 打 POST-only
    // completions 回 405 顶掉 401）。缓存已改内容比对（proxy_keys.rs），写盘
    // 即刻生效不依赖任何钟，sleep 撤除让测试同时钉住零延迟热生效语义。
    let resp = proxy_router_oneshot(&dir, "/v1/chat/completions", Some("nbk_aaa")).await;
    assert_eq!(
        resp.status(),
        401,
        "禁用的代理 key 必须 401（回落主 token 校验）"
    );
}
