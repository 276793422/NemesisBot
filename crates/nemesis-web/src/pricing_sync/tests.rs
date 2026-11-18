//! [`super::fetch_and_replace`] 测试：本机起临时 axum 服务器（受限网络下
//! 不打外网）覆盖完整 HTTP 路径——200+ETag 替换 / 304 增量 / 失败保留旧表。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;

use super::fetch_and_replace;
use nemesis_data::PricingStore;

/// 合成夹具表：2 个具名锚条目（既有 lookup 断言消费 test-model-a/b，
/// 价格与原 const 夹具一致）+ 填充条目凑足 ≥ MIN_FILTERED_ENTRIES——
/// BUG-20261118-157 起运行时下载路径拒绝小表，夹具必须过闸才能测
/// HTTP/ETag 机制本身。
fn fixture_json(model_a_in: f64, model_a_out: f64, model_a_max: i64) -> String {
    const FILLERS: usize = 118; // 2 锚 + 118 填充 = 120 ≥ 100
    let mut entries = vec![
        format!(
            r#""test-model-a": {{"max_tokens": {model_a_max}, "input_cost_per_token": {model_a_in}, "output_cost_per_token": {model_a_out}, "litellm_provider": "openai", "mode": "chat"}}"#
        ),
        r#""test-model-b": {"input_cost_per_token": 5e-07, "output_cost_per_token": 2e-06, "litellm_provider": "zhipu", "mode": "chat"}"#.to_string(),
    ];
    for i in 0..FILLERS {
        entries.push(format!(
            r#""filler-{i:03}": {{"input_cost_per_token": 1e-07, "output_cost_per_token": 2e-07, "litellm_provider": "openai", "mode": "chat"}}"#
        ));
    }
    format!("{{{}}}", entries.join(","))
}

fn table_v1() -> String {
    fixture_json(1e-06, 3e-06, 4096)
}

fn table_v2() -> String {
    fixture_json(2e-06, 6e-06, 8192)
}

/// 小表（1 条合法 chat 条目，能过 parse_litellm_json）：BUG-20261118-157
/// 下载闸的阳性对照。
fn table_tiny() -> String {
    r#"{"tiny-a": {"input_cost_per_token": 9e-06, "output_cost_per_token": 9e-06, "litellm_provider": "openai", "mode": "chat"}}"#.to_string()
}

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
        table_v1(),
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
            table_v2(),
        );
    }
    (
        StatusCode::OK,
        [(axum::http::header::ETAG, ETAG_V1)],
        table_v1(),
    )
}

async fn broken_handler() -> impl IntoResponse {
    (StatusCode::OK, "{not json".to_string())
}

async fn err_handler() -> impl IntoResponse {
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn huge_handler() -> impl IntoResponse {
    // 仅比 MAX_PRICE_TABLE_BYTES 多 1 字节：客户端必须在读入中途断开
    // （BUG-20261107-005），不会读完全量。loopback 传 64MB 亚秒级。
    (
        StatusCode::OK,
        vec![b'x'; super::MAX_PRICE_TABLE_BYTES as usize + 1],
    )
}

async fn tiny_handler() -> impl IntoResponse {
    // 1 条合法 chat 条目（过 parse_litellm_json）+ 条目数不足
    // （BUG-20261118-157 下载闸阳性对照）。
    (StatusCode::OK, table_tiny())
}

async fn spawn_server() -> String {
    let state = FixtureState::default();
    let app = Router::new()
        .route("/table", get(table_handler))
        // 镜像链测试的第二 URL：与 /table 同 handler（304 语义可复用）。
        .route("/mirror", get(table_handler))
        .route("/updated", get(updated_handler))
        .route("/broken", get(broken_handler))
        .route("/err", get(err_handler))
        .route("/huge", get(huge_handler))
        .route("/tiny", get(tiny_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn tmp_store(name: &str) -> PricingStore {
    let dir = std::env::temp_dir().join(format!("nb-pricing-sync-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    PricingStore::open(&dir).unwrap()
}

#[tokio::test]
async fn fetch_replaces_downloaded_layer_with_etag() {
    let base = spawn_server().await;
    let store = tmp_store("replace");

    let r = fetch_and_replace(&store, Some(&format!("{base}/table")))
        .await
        .unwrap();
    assert!(r.updated);
    assert_eq!(r.entry_count, 120);
    assert_eq!(r.etag.as_deref(), Some(ETAG_V1));

    let p = store.lookup("test-model-a").unwrap();
    assert!((p.input_cost_per_million - 1.0).abs() < 1e-9);
    // provider/model 配置名 → bare-suffix 命中。
    assert_eq!(
        store.lookup("zhipu/test-model-b").unwrap().model_id,
        "test-model-b"
    );

    let meta = store.meta();
    assert_eq!(meta.etag.as_deref(), Some(ETAG_V1));
    assert_eq!(meta.entry_count, 120);
    assert!(meta.fetched_at.is_some());

    // 同一 etag 再拉 → 304 NotModified。
    let r2 = fetch_and_replace(&store, Some(&format!("{base}/table")))
        .await
        .unwrap();
    assert!(!r2.updated, "second fetch with same etag should be 304");
}

#[tokio::test]
async fn server_side_update_bumps_table() {
    let base = spawn_server().await;
    let store = tmp_store("update");
    fetch_and_replace(&store, Some(&format!("{base}/updated")))
        .await
        .unwrap();
    // 服务器换 v2 → 本次拉到新内容 + 新 etag。
    let r = fetch_and_replace(&store, Some(&format!("{base}/updated")))
        .await
        .unwrap();
    assert!(r.updated);
    assert_eq!(
        store.lookup("test-model-a").unwrap().max_output_tokens,
        Some(8192)
    );
    assert!((store.lookup("test-model-a").unwrap().input_cost_per_million - 2.0).abs() < 1e-9);
    assert_eq!(store.meta().etag.as_deref(), Some("\"table-v2\""));
}

#[tokio::test]
async fn http_error_keeps_old_table() {
    let base = spawn_server().await;
    let store = tmp_store("httperr");
    fetch_and_replace(&store, Some(&format!("{base}/table")))
        .await
        .unwrap();
    let before = store.lookup("test-model-a").unwrap().input_cost_per_million;

    let err = fetch_and_replace(&store, Some(&format!("{base}/err"))).await;
    assert!(err.is_err());
    // 降级契约：旧表原封不动，meta 记录失败来源。
    assert_eq!(
        store.lookup("test-model-a").unwrap().input_cost_per_million,
        before
    );
    assert_eq!(
        store.meta().entry_count,
        120,
        "meta entry_count still from last success"
    );
    assert!(
        store
            .meta()
            .source_url
            .as_deref()
            .unwrap_or("")
            .ends_with("/err")
    );
}

#[tokio::test]
async fn malformed_payload_keeps_old_table() {
    let base = spawn_server().await;
    let store = tmp_store("malformed");
    fetch_and_replace(&store, Some(&format!("{base}/table")))
        .await
        .unwrap();

    let err = fetch_and_replace(&store, Some(&format!("{base}/broken"))).await;
    assert!(
        err.is_err(),
        "malformed payload must error, not blank the table"
    );
    assert_eq!(
        store.lookup("test-model-a").unwrap().input_cost_per_million,
        1.0
    );
    // BUG-20261025-62 回归锁（旧形态必红：解析失败不刷 meta，source_url
    // 仍指上次成功的 /table）：解析失败同样要留下尝试痕迹。
    let meta = store.meta();
    assert!(
        meta.source_url
            .as_deref()
            .unwrap_or("")
            .ends_with("/broken"),
        "parse failure must record source_url, got {:?}",
        meta.source_url
    );
    assert!(
        meta.fetched_at.is_some(),
        "parse failure must bump fetched_at"
    );
}

// BUG-20261025-62 回归锁（旧形态必红：网络级失败零留痕，meta 全 None）：
// 连接拒绝（send Err 路径）也要 record_failed_fetch——受限网络下最常见的
// 失败形态（连接被重置/超时）此前完全不留尝试痕迹，「上次尝试时间」恒旧。
#[tokio::test]
async fn network_failure_records_failed_fetch() {
    let store = tmp_store("net-fail");
    // 127.0.0.1:1 端口拒绝（无需起服务器，离线稳定）。
    let err = fetch_and_replace(&store, Some("http://127.0.0.1:1/prices")).await;
    assert!(err.is_err(), "connection refused must error");
    let meta = store.meta();
    assert!(
        meta.source_url.as_deref() == Some("http://127.0.0.1:1/prices"),
        "network failure must record source_url, got {:?}",
        meta.source_url
    );
    assert!(
        meta.fetched_at.is_some(),
        "network failure must bump fetched_at"
    );
}

// BUG-20261107-005 回归锁（旧 `bytes()` 形态必红：64MB+1 全量灌进内存
// 后才核对上限；新形态在读入中途即断）。降级契约同步钉死：旧表不动 +
// meta 留痕。
#[tokio::test]
async fn oversized_body_aborts_and_keeps_old_table() {
    let base = spawn_server().await;
    let store = tmp_store("oversize");
    fetch_and_replace(&store, Some(&format!("{base}/table")))
        .await
        .unwrap();

    let err = fetch_and_replace(&store, Some(&format!("{base}/huge")))
        .await
        .unwrap_err();
    assert!(err.contains("exceeds limit"), "err: {err}");
    assert!(err.contains("aborted mid-read"), "err: {err}");
    assert_eq!(
        store.lookup("test-model-a").unwrap().input_cost_per_million,
        1.0,
        "oversize failure must keep old table"
    );
    assert!(
        store
            .meta()
            .source_url
            .as_deref()
            .unwrap_or("")
            .ends_with("/huge"),
        "oversize failure must record source_url, got {:?}",
        store.meta().source_url
    );
}

// ---------------------------------------------------------------------------
// 镜像链（2026-09-07）：None → 链式回落；ETag 不跨镜像附带
// （测试注入本地 URL，受限网络下不打外网）
// ---------------------------------------------------------------------------

use super::fetch_chain;

#[tokio::test]
async fn mirror_chain_falls_back_to_next_mirror() {
    let base = spawn_server().await;
    let store = tmp_store("chain-fallback");

    // 主镜像 500 → 回落到备用镜像 → 成功替换。
    let r = fetch_chain(&store, &[&format!("{base}/err"), &format!("{base}/table")])
        .await
        .unwrap();
    assert!(r.updated);
    assert_eq!(r.entry_count, 120);
    assert!(r.source_url.ends_with("/table"), "source = winning mirror");
    assert!(store.lookup("test-model-a").is_some());
}

#[tokio::test]
async fn mirror_chain_all_fail_reports_every_mirror() {
    let base = spawn_server().await;
    let store = tmp_store("chain-allfail");

    let err = fetch_chain(&store, &[&format!("{base}/err"), &format!("{base}/broken")])
        .await
        .unwrap_err();
    // 聚合报错必须点名每一条镜像（诊断友好），旧表保持未动。
    assert!(err.contains("/err"), "err: {err}");
    assert!(err.contains("/broken"), "err: {err}");
    assert!(store.list_downloaded().is_none());
}

#[tokio::test]
async fn etag_not_sent_across_mirrors() {
    let base = spawn_server().await;
    let store = tmp_store("chain-etag");

    // 先从 /table 成功拿到 etag V1。
    fetch_and_replace(&store, Some(&format!("{base}/table")))
        .await
        .unwrap();

    // /mirror 挂的也是 table_handler：若错误地附带跨源 etag → 304；
    // 正确语义（不带）→ 200 全量 → updated=true。
    let r = fetch_chain(&store, &[&format!("{base}/mirror")])
        .await
        .unwrap();
    assert!(
        r.updated,
        "different mirror must be fetched in full, not answered with 304"
    );
}

#[tokio::test]
async fn same_url_etag_still_gives_304() {
    let base = spawn_server().await;
    let store = tmp_store("chain-etag-same");

    fetch_and_replace(&store, Some(&format!("{base}/table")))
        .await
        .unwrap();
    // 同一 URL 链式拉取：etag 附带 → 304 NotModified（增量语义不回退）。
    let r = fetch_chain(&store, &[&format!("{base}/table")])
        .await
        .unwrap();
    assert!(!r.updated, "same-url refetch with etag should be 304");
}

// ---------------------------------------------------------------------------
// 下载闸（BUG-20261118-157）：条目数不足的劫持页/截断 payload 不落下载层
// ---------------------------------------------------------------------------

// BUG-20261118-157 回归锁（旧形态必红：1 条合法 chat 条目静默替换下载层
// + 刷写 meta）：MIN_FILTERED_ENTRIES 闸此前只在编译期内嵌路径接线，运行
// 时下载路径必须同样拒小表——诚实报错 + 旧表原封不动 + meta 留尝试痕迹
// （留痕纪律与解析失败/HTTP 失败同款）。
#[tokio::test]
async fn tiny_table_rejected_keeps_old_table() {
    let base = spawn_server().await;
    let store = tmp_store("tiny-gate");
    fetch_and_replace(&store, Some(&format!("{base}/table")))
        .await
        .unwrap();
    let before = store.lookup("test-model-a").unwrap().input_cost_per_million;

    let err = fetch_and_replace(&store, Some(&format!("{base}/tiny")))
        .await
        .unwrap_err();
    assert!(err.contains("条目数过少"), "err: {err}");
    assert!(err.contains("保留旧表"), "err: {err}");
    assert_eq!(
        store.lookup("test-model-a").unwrap().input_cost_per_million,
        before,
        "tiny table must keep old table"
    );
    assert!(
        store.lookup("tiny-a").is_none(),
        "rejected entry must not enter the downloaded layer"
    );
    assert!(
        store
            .meta()
            .source_url
            .as_deref()
            .unwrap_or("")
            .ends_with("/tiny"),
        "gate rejection must record source_url, got {:?}",
        store.meta().source_url
    );
    assert!(
        store.meta().fetched_at.is_some(),
        "gate rejection must bump fetched_at"
    );
    // 闸拒绝不 clobber 已存 etag（record_failed_fetch 保留语义）——注：
    // 下一次拉 /table 不会走 304（source_url 已被失败留痕改写为 /tiny，
    // etag 附带按「待拉 URL == meta.source_url」门控，全量 200 是既有
    // 正确行为），这里只钉 etag 本身存活。
    assert_eq!(
        store.meta().etag.as_deref(),
        Some(ETAG_V1),
        "gate rejection must not clobber the stored etag"
    );
}
