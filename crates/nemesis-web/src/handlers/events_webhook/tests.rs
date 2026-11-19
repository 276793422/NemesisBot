use super::*;
use std::sync::{Arc, Mutex};

/// 组装一个带 webhook 规则的真泵（rules 文件 + EventWatchManager + 收集
/// 注入器），返回 (TempDir, submitter, 收集到的注入内容, JoinHandle)。
/// TempDir 返还给调用方持有——提前 drop 会在泵装载规则前删掉 rules 文件。
async fn pump_with_rule(
    token: &str,
    merge_window_secs: u64,
    cooldown_secs: u64,
) -> (
    tempfile::TempDir,
    nemesis_events::WebhookSubmitter,
    Arc<Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    let dir = tempfile::tempdir().unwrap();
    let rules_path = dir.path().join("events.json");
    let rule = nemesis_events::EventRule {
        id: "wh1".to_string(),
        watch: nemesis_events::WatchSpec {
            kind: "webhook".to_string(),
            token: Some(token.to_string()),
            ..Default::default()
        },
        deliver: nemesis_events::DeliverSpec {
            session_key: "agent:main:session:t".to_string(),
            prompt: "外部推送：{payload}".to_string(),
            cooldown_secs: Some(cooldown_secs),
            merge_window_secs: Some(merge_window_secs),
            max_per_hour: None,
            enabled: None,
        },
    };
    nemesis_events::save_rules(&rules_path, &[rule]).unwrap();
    let contents = Arc::new(Mutex::new(Vec::new()));
    let sink = contents.clone();
    let injector: nemesis_events::InjectorFn = Arc::new(move |_s: &str, c: &str| -> bool {
        sink.lock().unwrap().push(c.to_string());
        true
    });
    let mgr = nemesis_events::EventWatchManager::new(rules_path, injector, Arc::new(|| false));
    let submitter = mgr.webhook_handle();
    let handle = mgr.start();
    (dir, submitter, contents, handle)
}

async fn wait_for(contents: &Mutex<Vec<String>>, needle: &str) -> bool {
    for _ in 0..40 {
        if contents.lock().unwrap().iter().any(|c| c.contains(needle)) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn matched_token_returns_200_and_delivers_payload() {
    let (_dir, submitter, contents, handle) = pump_with_rule("s3cret", 0, 0).await;
    let resp = respond(
        Some(&submitter),
        "s3cret",
        axum::body::Bytes::from("deploy finished"),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    assert!(
        wait_for(&contents, "deploy finished").await,
        "载荷必须经泵投递到注入器"
    );
    let logged = contents.lock().unwrap()[0].clone();
    assert!(logged.starts_with("[事件:wh1] "), "投递带规则徽标");
    handle.abort();
}

#[tokio::test]
async fn wrong_token_is_honest_404() {
    let (_dir, submitter, contents, handle) = pump_with_rule("right", 0, 0).await;
    let resp = respond(
        Some(&submitter),
        "wrong-token",
        axum::body::Bytes::from("x"),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    // 404 不投递
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(contents.lock().unwrap().is_empty());
    handle.abort();
}

#[tokio::test]
async fn oversize_body_is_413_without_queueing() {
    let (_dir, submitter, contents, handle) = pump_with_rule("tk", 0, 0).await;
    let big = vec![b'x'; 8 * 1024 + 1];
    let resp = respond(Some(&submitter), "tk", axum::body::Bytes::from(big)).await;
    assert_eq!(resp.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(contents.lock().unwrap().is_empty(), "超限载荷不入队");
    handle.abort();
}

#[tokio::test]
async fn unwired_submitter_is_honest_503() {
    let resp = respond(None, "any", axum::body::Bytes::from("x")).await;
    assert_eq!(resp.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn payload_whitespace_trimmed_and_merge_window_folds_burst() {
    let (_dir, submitter, contents, handle) = pump_with_rule("tk", 1, 0).await;
    // 两次推送（带首尾空白）在同一静默窗内折叠为一次投递。
    let r1 = respond(
        Some(&submitter),
        "tk",
        axum::body::Bytes::from("  event one\n"),
    )
    .await;
    let r2 = respond(
        Some(&submitter),
        "tk",
        axum::body::Bytes::from("event two  "),
    )
    .await;
    assert_eq!(r1.status(), axum::http::StatusCode::OK);
    assert_eq!(r2.status(), axum::http::StatusCode::OK);
    assert!(wait_for(&contents, "event two").await, "合并投递必含载荷");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let logged = contents.lock().unwrap();
    assert_eq!(
        logged.len(),
        1,
        "同静默窗两次推送折叠为一次投递（count=2 注记）"
    );
    assert!(logged[0].contains("event one"), "两载荷合并列明");
    handle.abort();
}
