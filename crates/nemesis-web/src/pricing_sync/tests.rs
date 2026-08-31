//! [`super::fetch_and_replace`] 测试：本机起临时 axum 服务器（受限网络下
//! 不打外网）覆盖完整 HTTP 路径——200+ETag 替换 / 304 增量 / 失败保留旧表。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::{State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

use super::fetch_and_replace;
use nemesis_data::PricingStore;

const TABLE_JSON_V1: &str = r#"{
  "test-model-a": {
    "max_tokens": 4096,
    "input_cost_per_token": 1e-06,
    "output_cost_per_token": 3e-06,
    "litellm_provider": "openai",
    "mode": "chat"
  },
  "test-model-b": {
    "input_cost_per_token": 5e-07,
    "output_cost_per_token": 2e-06,
    "litellm_provider": "zhipu",
    "mode": "chat"
  }
}"#;

const TABLE_JSON_V2: &str = r#"{
  "test-model-a": {
    "max_tokens": 8192,
    "input_cost_per_token": 2e-06,
    "output_cost_per_token": 6e-06,
    "litellm_provider": "openai",
    "mode": "chat"
  }
}"#;

const ETAG_V1: &str = "\"table-v1\"";

#[derive(Clone, Default)]
struct FixtureState {
    table_hits: Arc<AtomicUsize>,
}

async fn table_handler(headers: HeaderMap, State(st): State<FixtureState>) -> impl IntoResponse {
    st.table_hits.fetch_add(1, Ordering::SeqCst);
    // 第二次起 v2 内容 + v2 etag（内容变了，304 分支要走 /cached 路由测）。
    if headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == ETAG_V1)
    {
        return (
            StatusCode::NOT_MODIFIED,
            [(axum::http::header::ETAG, ETAG_V1)],
            String::new(),
        );
    }
    (
        StatusCode::OK,
        [(axum::http::header::ETAG, ETAG_V1)],
        TABLE_JSON_V1.to_string(),
    )
}

async fn updated_handler(headers: HeaderMap) -> impl IntoResponse {
    if headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == ETAG_V1)
    {
        // 客户端以为还拿着 v1，但服务器已换 v2 → 返回新内容 + 新 etag。
        return (
            StatusCode::OK,
            [(axum::http::header::ETAG, "\"table-v2\"")],
            TABLE_JSON_V2.to_string(),
        );
    }
    (
        StatusCode::OK,
        [(axum::http::header::ETAG, ETAG_V1)],
        TABLE_JSON_V1.to_string(),
    )
}

async fn broken_handler() -> impl IntoResponse {
    (StatusCode::OK, "{not json".to_string())
}

async fn err_handler() -> impl IntoResponse {
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn spawn_server() -> String {
    let state = FixtureState::default();
    let app = Router::new()
        .route("/table", get(table_handler))
        .route("/updated", get(updated_handler))
        .route("/broken", get(broken_handler))
        .route("/err", get(err_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn tmp_store(name: &str) -> PricingStore {
    let dir = std::env::temp_dir().join(format!(
        "nb-pricing-sync-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    PricingStore::open(&dir).unwrap()
}

#[tokio::test]
async fn fetch_replaces_downloaded_layer_with_etag() {
    let base = spawn_server().await;
    let store = tmp_store("replace");

    let r = fetch_and_replace(&store, Some(&format!("{base}/table"))).await.unwrap();
    assert!(r.updated);
    assert_eq!(r.entry_count, 2);
    assert_eq!(r.etag.as_deref(), Some(ETAG_V1));

    let p = store.lookup("test-model-a").unwrap();
    assert!((p.input_cost_per_million - 1.0).abs() < 1e-9);
    // provider/model 配置名 → bare-suffix 命中。
    assert_eq!(store.lookup("zhipu/test-model-b").unwrap().model_id, "test-model-b");

    let meta = store.meta();
    assert_eq!(meta.etag.as_deref(), Some(ETAG_V1));
    assert_eq!(meta.entry_count, 2);
    assert!(meta.fetched_at.is_some());

    // 同一 etag 再拉 → 304 NotModified。
    let r2 = fetch_and_replace(&store, Some(&format!("{base}/table"))).await.unwrap();
    assert!(!r2.updated, "second fetch with same etag should be 304");
}

#[tokio::test]
async fn server_side_update_bumps_table() {
    let base = spawn_server().await;
    let store = tmp_store("update");
    fetch_and_replace(&store, Some(&format!("{base}/updated"))).await.unwrap();
    // 服务器换 v2 → 本次拉到新内容 + 新 etag。
    let r = fetch_and_replace(&store, Some(&format!("{base}/updated"))).await.unwrap();
    assert!(r.updated);
    assert_eq!(store.lookup("test-model-a").unwrap().max_output_tokens, Some(8192));
    assert!((store.lookup("test-model-a").unwrap().input_cost_per_million - 2.0).abs() < 1e-9);
    assert_eq!(store.meta().etag.as_deref(), Some("\"table-v2\""));
}

#[tokio::test]
async fn http_error_keeps_old_table() {
    let base = spawn_server().await;
    let store = tmp_store("httperr");
    fetch_and_replace(&store, Some(&format!("{base}/table"))).await.unwrap();
    let before = store.lookup("test-model-a").unwrap().input_cost_per_million;

    let err = fetch_and_replace(&store, Some(&format!("{base}/err"))).await;
    assert!(err.is_err());
    // 降级契约：旧表原封不动，meta 记录失败来源。
    assert_eq!(store.lookup("test-model-a").unwrap().input_cost_per_million, before);
    assert_eq!(store.meta().entry_count, 2, "meta entry_count still from last success");
    assert!(store.meta().source_url.as_deref().unwrap_or("").ends_with("/err"));
}

#[tokio::test]
async fn malformed_payload_keeps_old_table() {
    let base = spawn_server().await;
    let store = tmp_store("malformed");
    fetch_and_replace(&store, Some(&format!("{base}/table"))).await.unwrap();

    let err = fetch_and_replace(&store, Some(&format!("{base}/broken"))).await;
    assert!(err.is_err(), "malformed payload must error, not blank the table");
    assert_eq!(store.lookup("test-model-a").unwrap().input_cost_per_million, 1.0);
}
