//! MCP handler 补测（Phase 3 覆盖率，2026-08-25）。
//!
//! 既有 `handlers/tests.rs` 已盖 server.add/delete、config.get/save、
//! unknown cmd。这里补两块显示/更新语义：
//! ① `servers` 列表的 legacy 归一化显示（transport_type 空 → "stdio"、
//!    url 空 → 显示 command）——`McpServerConfig::normalize` 只在 add 时跑，
//!    手写的存量配置文件里可以没有这两个字段，展示层必须兜底；
//! ② `server.update` 的全部可选字段 patch 臂（transport_type/description/
//!    headers/provider_name/provider_url/tags——旧清单只盖了 url/args/env/
//!    timeout）。

use super::McpHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 手写一份存量形态的 config.mcp.json（transport_type/url 缺省）。
fn seed_mcp_config(dir: &tempfile::TempDir, servers: serde_json::Value) {
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.mcp.json"),
        serde_json::json!({ "enabled": true, "servers": servers }).to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn servers_display_normalizes_legacy_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    // 存量配置：只有 command，transport_type/url 全空。
    seed_mcp_config(
        &dir,
        serde_json::json!([{ "name": "legacy", "command": "run-server.sh" }]),
    );

    let h = McpHandler::new();
    let out = h.handle_cmd("servers", None, &ctx).await.unwrap().unwrap();
    let servers = out["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    // 展示层兜底：空 transport_type 显示 "stdio"，空 url 显示 command。
    assert_eq!(servers[0]["transport_type"], "stdio");
    assert_eq!(servers[0]["url"], "run-server.sh");
}

#[tokio::test]
async fn server_update_patches_all_optional_fields_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    seed_mcp_config(
        &dir,
        serde_json::json!([{ "name": "s1", "url": "http://old" }]),
    );

    let h = McpHandler::new();
    let out = h
        .handle_cmd(
            "server.update",
            Some(serde_json::json!({
                "name": "s1",
                "transport_type": "http",
                "description": "patched desc",
                "headers": ["Authorization: b"],
                "provider_name": "prov",
                "provider_url": "https://prov.example",
                "tags": ["a", "b"],
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);

    // 落盘回读（config.get）验证全部字段持久化。headers 是敏感键：
    // config.get 回显为脱敏形态（凭据回显脱敏批次），原始值只落盘不回显。
    let cfg = h
        .handle_cmd("config.get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let s = &cfg["servers"][0];
    assert_eq!(s["transport_type"], "http");
    assert_eq!(s["url"], "http://old", "未 patch 的字段必须保持原值");
    assert_eq!(s["description"], "patched desc");
    assert_eq!(
        s["headers"],
        serde_json::json!(["Authorization: ****"]),
        "config.get 回显必须脱敏"
    );
    assert_eq!(s["provider_name"], "prov");
    assert_eq!(s["provider_url"], "https://prov.example");
    assert_eq!(s["tags"], serde_json::json!(["a", "b"]));

    // 回程：UI 把脱敏形态原样存回 → server.update 按键还原存量原值，
    // 落盘仍是完整原值（绝不能把掩码当真值写进配置）。
    h.handle_cmd(
        "server.update",
        Some(serde_json::json!({
            "name": "s1",
            "headers": ["Authorization: ****"],
        })),
        &ctx,
    )
    .await
    .unwrap()
    .unwrap();
    let raw = std::fs::read_to_string(dir.path().join("config/config.mcp.json")).unwrap();
    assert!(
        raw.contains("Authorization: b"),
        "掩码回存必须还原为存量原值: {raw}"
    );
}

// ---------------------------------------------------------------------------
// 事务入口并发回归钉（2026-10-13 横扫四期）：config.mcp.json 此前四个写臂
// 全部裸 RMW（横扫清单漏网），现收敛到 update_mcp_config（锁在入口内）。
// 两线程并发改不同 server，两个变更都必须存活。
// ---------------------------------------------------------------------------

#[test]
fn mcp_update_entries_serialized_both_mutations_survive() {
    let dir = tempfile::tempdir().unwrap();
    seed_mcp_config(
        &dir,
        serde_json::json!([
            { "name": "a", "transport_type": "stdio" },
            { "name": "b", "transport_type": "stdio" }
        ]),
    );
    let ws = dir.path().to_string_lossy().to_string();

    let ws_a = ws.clone();
    let ws_b = ws.clone();
    std::thread::scope(|s| {
        let ra = s.spawn(move || {
            super::update_mcp_config(&ws_a, |cfg| {
                cfg.servers.iter_mut().find(|s| s.name == "a").unwrap().url = "http://a".into();
                Ok(())
            })
        });
        let rb = s.spawn(move || {
            super::update_mcp_config(&ws_b, |cfg| {
                cfg.servers.iter_mut().find(|s| s.name == "b").unwrap().url = "http://b".into();
                Ok(())
            })
        });
        ra.join().unwrap().expect("tx a ok");
        rb.join().unwrap().expect("tx b ok");
    });

    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config/config.mcp.json")).unwrap(),
    )
    .unwrap();
    let servers = raw["servers"].as_array().unwrap();
    let a = servers.iter().find(|s| s["name"] == "a").unwrap();
    let b = servers.iter().find(|s| s["name"] == "b").unwrap();
    assert_eq!(a["url"], "http://a", "并发事务 a 的变更存活");
    assert_eq!(b["url"], "http://b", "并发事务 b 的变更存活");
}

// abort 语义钉：闭包 Err ⇒ 不落盘（与 config 家族入口同语义）。
#[test]
fn mcp_update_entry_closure_err_leaves_disk_untouched() {
    let dir = tempfile::tempdir().unwrap();
    seed_mcp_config(
        &dir,
        serde_json::json!([{ "name": "a", "transport_type": "stdio", "url": "" }]),
    );
    let ws = dir.path().to_string_lossy().to_string();

    let err = super::update_mcp_config(&ws, |cfg| {
        cfg.servers[0].url = "http://should-not-persist".into();
        Err::<(), String>("deliberate abort".to_string())
    })
    .unwrap_err();
    assert_eq!(err, "deliberate abort");

    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config/config.mcp.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(raw["servers"][0]["url"], "", "abort 后盘上保持事务前状态");
}

// ---------------------------------------------------------------------------
// R9（2026-10-15）：transport_type 入参值集闸——runtime 不支持的值（如
// "sse"）此前 server.add/server.update 原样落盘，挂载时才由分派真相源
// 拒绝。API 输入面收紧为 loud 拒绝，语义与 CLI `mcp add` 同源。
// R10（2026-10-15）：值集闸升级为归一闸——大小写变体（"HTTP"）此前原样
// 落盘，而分派真相源别名匹配大小写敏感，挂载期同样 loud 拒绝。现在规范
// 小写落盘（CLI `mcp add` 写盘同款语义）。
// ---------------------------------------------------------------------------

#[test]
fn r9_canonical_transport_type_value_set() {
    use super::canonical_transport_type;
    // 合法值返回规范小写形态
    assert_eq!(canonical_transport_type("http").unwrap(), "http");
    assert_eq!(canonical_transport_type("stdio").unwrap(), "stdio");
    // 大小写变体归一（与 CLI build 同源匹配、规范落盘）
    assert_eq!(canonical_transport_type("HTTP").unwrap(), "http");
    assert_eq!(canonical_transport_type("Stdio").unwrap(), "stdio");
    // 未知值 loud 拒绝，错误信息保留用户原值
    for bad in ["sse", "SSE", "websocket", ""] {
        let err = canonical_transport_type(bad).unwrap_err();
        assert!(err.contains("不支持的 transport_type"), "actual: {err}");
        assert!(err.contains(bad), "错误信息应含用户原值 '{bad}': {err}");
    }
}

#[tokio::test]
async fn r9_server_add_rejects_unsupported_transport_without_disk_write() {
    let handler = McpHandler::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    let cfg = nemesis_config::McpConfig::default();
    std::fs::write(
        dir.path().join("config/config.mcp.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "sse-srv", "transport_type": "sse", "url": "http://x/sse"
    });
    let err = match handler.handle_cmd("server.add", Some(data), &ctx).await {
        Ok(v) => panic!("sse add must be loudly rejected, got: {v:?}"),
        Err(e) => e,
    };
    assert!(err.contains("不支持的 transport_type"), "actual: {err}");

    // 拒绝后盘上无残留条目
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config/config.mcp.json")).unwrap(),
    )
    .unwrap();
    assert!(
        raw["servers"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "rejected entry must not persist: {raw}"
    );
}

// R10（2026-10-15）：大小写变体规范小写落盘——分派真相源别名匹配大小写
// 敏感，"HTTP" 原样落盘会在挂载期被 loud 拒绝（API 接受 runtime 消费不
// 了的值，失败延迟到挂载）。server.add 与 server.update 两臂都归一。
#[tokio::test]
async fn r10_transport_type_case_variants_canonicalized_on_disk() {
    let handler = McpHandler::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/config.mcp.json"),
        serde_json::to_string_pretty(&nemesis_config::McpConfig::default()).unwrap(),
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    // server.add "HTTP" → 盘上 "http"
    let add = serde_json::json!({
        "name": "case-srv", "transport_type": "HTTP", "url": "http://x/mcp"
    });
    handler
        .handle_cmd("server.add", Some(add), &ctx)
        .await
        .expect("HTTP variant must be accepted and canonicalized");

    // server.update 补一个 stdio 条目再改 "Stdio" → 盘上 "stdio"
    let add2 = serde_json::json!({
        "name": "case-stdio", "transport_type": "http", "url": "http://y/mcp"
    });
    handler
        .handle_cmd("server.add", Some(add2), &ctx)
        .await
        .expect("canonical http add must succeed");
    let upd = serde_json::json!({ "name": "case-stdio", "transport_type": "Stdio" });
    handler
        .handle_cmd("server.update", Some(upd), &ctx)
        .await
        .expect("Stdio variant must be accepted and canonicalized");

    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config/config.mcp.json")).unwrap(),
    )
    .unwrap();
    let servers = raw["servers"].as_array().expect("servers array");
    let get_type = |name: &str| {
        servers
            .iter()
            .find(|s| s["name"] == name)
            .map(|s| s["transport_type"].as_str().unwrap_or("?"))
            .unwrap_or("missing")
    };
    assert_eq!(get_type("case-srv"), "http", "HTTP → http 盘上归一");
    assert_eq!(get_type("case-stdio"), "stdio", "Stdio → stdio 盘上归一");
}

// R12（2026-10-15）：头行语法闸——server.add / server.update 写入面与 CLI
// `mcp add` 同源（单一真相源 nemesis_config::validate_mcp_header_line）。
// 此前 Dashboard 放行运行期必跳过/必失败的行（缺冒号、保留头、引用名带
// 空格），失败延迟到挂载期 401 且 WARN 对用户不可见。

fn r12_bad_header_lines() -> Vec<&'static str> {
    vec![
        "Authorization",          // 缺冒号
        "Mcp-Session-Id: forged", // 协议保留头
        "X-Key: vault: name",     // 引用名带首尾空格
        "X-Key: vault:",          // 引用名为空
    ]
}

#[tokio::test]
async fn r12_server_add_rejects_illegal_header_line_without_disk_write() {
    let handler = McpHandler::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/config.mcp.json"),
        serde_json::to_string_pretty(&nemesis_config::McpConfig::default()).unwrap(),
    )
    .unwrap();
    let ctx = make_ctx(&dir);

    for bad in r12_bad_header_lines() {
        let data = serde_json::json!({
            "name": "h-srv", "transport_type": "http", "url": "http://x/mcp",
            "headers": ["X-Ok: 1", bad],
        });
        let err = match handler.handle_cmd("server.add", Some(data), &ctx).await {
            Ok(v) => panic!("bad header '{bad}' must be rejected, got: {v:?}"),
            Err(e) => e,
        };
        assert!(
            err.contains("headers 校验失败"),
            "'{bad}' → wrong error: {err}"
        );
    }

    // 拒绝后盘上无残留条目
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config/config.mcp.json")).unwrap(),
    )
    .unwrap();
    assert!(
        raw["servers"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "rejected entries must not persist: {raw}"
    );
}

#[tokio::test]
async fn r12_server_update_rejects_new_illegal_header_line() {
    let handler = McpHandler::new();
    let dir = tempfile::tempdir().unwrap();
    seed_mcp_config(
        &dir,
        serde_json::json!([{
            "name": "s1", "transport_type": "http", "url": "http://x",
            "headers": ["Authorization: b"],
        }]),
    );
    let ctx = make_ctx(&dir);

    let data = serde_json::json!({
        "name": "s1",
        "headers": ["Authorization: b", "X-Bad: vault: x"],
    });
    let err = match handler.handle_cmd("server.update", Some(data), &ctx).await {
        Ok(v) => panic!("new bad header must be rejected, got: {v:?}"),
        Err(e) => e,
    };
    assert!(err.contains("headers 校验失败"), "actual: {err}");

    // 拒绝后盘上 headers 保持原值（事务 abort 语义）
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config/config.mcp.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        raw["servers"][0]["headers"],
        serde_json::json!(["Authorization: b"]),
        "rejected update must not persist: {raw}"
    );
}

#[tokio::test]
async fn r12_server_update_tolerates_masked_legacy_plus_new_good_line() {
    let handler = McpHandler::new();
    let dir = tempfile::tempdir().unwrap();
    seed_mcp_config(
        &dir,
        serde_json::json!([{
            "name": "s1", "transport_type": "http", "url": "http://x",
            "headers": ["Authorization: b"],
        }]),
    );
    let ctx = make_ctx(&dir);

    // UI 回显形态：掩码行（透传存量）+ 新增合法行——掩码行不进校验面，
    // 更新必须成功且盘上 = 存量原值 + 新行。
    let data = serde_json::json!({
        "name": "s1",
        "headers": ["Authorization: ****", "X-Extra: ok"],
    });
    let out = handler
        .handle_cmd("server.update", Some(data), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);

    let raw = std::fs::read_to_string(dir.path().join("config/config.mcp.json")).unwrap();
    assert!(
        raw.contains("Authorization: b"),
        "掩码行须还原存量原值: {raw}"
    );
    assert!(raw.contains("X-Extra: ok"), "新合法行须落盘: {raw}");
}
