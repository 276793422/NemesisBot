//! UI 批次（P1-P5）新端点 integration-test 断言（goal §九.3 合规）。
//!
//! 各批交付时的测试都在 crate 级（nemesis-web handler 单测）；本套件补
//! **真进程链路**断言：Phase 2 起的 gateway（`--local`，TestWorkspace home）
//! + WebSocket WSAPI / HTTP 端点，覆盖 §九.3 要求的三类断言：
//! 字段透传落盘、值校验拒绝非法值、get/set 往返。
//!
//! 覆盖矩阵（对应 docs/PLAN/2026-08-24_ui-batch-acceptance-plan.md）：
//! - P1-1..4  memory config.get/set（auto_inject / auto_inject_top_k）
//! - P1-5     tasks cron.add/update/list（max_rounds 三态 + jobs.json 落盘）
//! - P2-1..2  coding lsp_status / config
//! - P2-3     SDK HTTP export/pip（公开 GET 200 + zip 魔数）
//! - P3-1..2  HTTP turns/fork（401/404/轮次表/前缀分叉/原会话不动；
//!            session 在 gateway 启动**前**种入——live store 构造时
//!            load_from_disk 才能看到，这也是生产里「重启后仍可见」路径）
//! - P3-3..6  models add/update_field/list/delete（合法+非法值）+
//!            catalog_update（HOME parent() 修复的运行时证明）
//! - P4-1..4  hooks get/set（模板/原文 roundtrip/非法拒且盘上不破坏）
//! - P5-7..10 sandbox overview/set_config/status（形状/字段级合并/非法拒）
//!
//! 注意：本套件**末位**运行（main.rs 里放在所有既有 gateway 测试之后）——
//! 它会改 config.json（sandbox set_config / models add），不能污染前面
//! 对默认配置的断言（test_config_defaults 等）。

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use test_harness::*;
use tokio_tungstenite::tungstenite::Message;

// ---------------------------------------------------------------------------
// WSAPI 泛化 helper（三级协议 request/response，reqId 关联）
// ---------------------------------------------------------------------------

/// 一条 WS 连接上的顺序 WSAPI 调用器。每次调用自增 reqId。
struct WsApi {
    stream: WsStream,
    next_id: u32,
}

impl WsApi {
    async fn connect() -> Result<Self, String> {
        Ok(Self {
            stream: ws_connect(WS_PORT, AUTH_TOKEN).await.map_err(|e| e.to_string())?,
            next_id: 0,
        })
    }

    /// 发一个 WSAPI 请求，等它的响应（按 reqId 匹配；忽略 push/ping）。
    /// 返回 `(data, error)` —— 恰有一个是 Some。
    async fn call(
        &mut self,
        module: &str,
        cmd: &str,
        data: Option<Value>,
    ) -> (Option<Value>, Option<String>) {
        self.next_id += 1;
        let req_id = format!("it-ui-{}", self.next_id);
        let msg = json!({
            "type": "request",
            "module": module,
            "cmd": cmd,
            "reqId": req_id,
            "data": data,
        });
        if let Err(e) = self.stream.send(Message::Text(msg.to_string().into())).await {
            return (None, Some(format!("ws send failed: {e}")));
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            match tokio::time::timeout_at(deadline, self.stream.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => {
                    let Ok(v) = serde_json::from_str::<Value>(&text.to_string()) else {
                        continue;
                    };
                    if v.get("type").and_then(|t| t.as_str()) == Some("response")
                        && v.get("reqId").and_then(|r| r.as_str()) == Some(req_id.as_str())
                    {
                        let err = v.get("error").and_then(|e| e.as_str()).map(String::from);
                        let dat = v.get("data").cloned().filter(|d| !d.is_null());
                        return (dat, err);
                    }
                    continue; // 别的 reqId / push
                }
                Ok(Some(Ok(_))) => continue, // ping/pong/binary
                Ok(Some(Err(e))) => return (None, Some(format!("ws error: {e}"))),
                Ok(None) => return (None, Some("ws closed".into())),
                Err(_) => return (None, Some("ws timeout (60s)".into())),
            }
        }
    }
}

/// HTTP GET，带/不带 X-Auth-Token。返回 (status, body bytes)。
async fn http_get(path: &str, with_token: bool) -> (u16, Vec<u8>) {
    let client = http_client();
    let mut req = client.get(format!("http://127.0.0.1:{}{}", WEB_PORT, path));
    if with_token {
        req = req.header("X-Auth-Token", AUTH_TOKEN);
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            (status, body)
        }
        Err(e) => (0, format!("http error: {e}").into_bytes()),
    }
}

/// HTTP POST JSON，带/不带 token。返回 (status, body)。
async fn http_post_json(path: &str, body: &str, with_token: bool) -> (u16, String) {
    let client = http_client();
    let mut req = client
        .post(format!("http://127.0.0.1:{}{}", WEB_PORT, path))
        .header("Content-Type", "application/json")
        .body(body.to_string());
    if with_token {
        req = req.header("X-Auth-Token", AUTH_TOKEN);
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            (status, text)
        }
        Err(e) => (0, format!("http error: {e}")),
    }
}

/// 读 TestWorkspace home 的 config.json（raw）。
fn read_home_config(ws: &TestWorkspace) -> Value {
    let path = ws.home().join("config.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

/// 镜像 `SessionStore::sanitize_session_id`（session 文件名规则）。
fn sanitize_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// P3 session 种子（gateway 启动前调用——live store 构造时 load_from_disk）
// ---------------------------------------------------------------------------

/// 种入 2 轮会话 `agent:main:session:itui1`（dsh_series 同款格式）。
/// 在 gateway spawn 之前调（保持与旧契约相同的种子时机）。2026-08-25
/// 第三轮起 turns/fork 直接读 chat_log jsonl（不再走 live store 的
/// get_history），但 store json 仍种着——fork 的 store 写路径与
/// `session list` 依旧要看到它。
pub fn seed_fork_sessions(ws: &TestWorkspace) {
    let key = "agent:main:session:itui1";
    let safe = sanitize_key(key);
    let sess_path = ws
        .workspace()
        .join("sessions")
        .join(format!("{safe}.json"));
    std::fs::create_dir_all(sess_path.parent().unwrap()).unwrap();
    let session_json = json!({
        "key": key,
        "messages": [
            {"role": "user", "content": "ITUI turn one q", "tool_calls": [], "tool_call_id": null, "timestamp": "2026-08-24T10:00:00+08:00"},
            {"role": "assistant", "content": "ITUI turn one a", "tool_calls": [], "tool_call_id": null, "timestamp": "2026-08-24T10:00:05+08:00"},
            {"role": "user", "content": "ITUI turn two q", "tool_calls": [], "tool_call_id": null, "timestamp": "2026-08-24T10:01:00+08:00"},
            {"role": "assistant", "content": "ITUI turn two a", "tool_calls": [], "tool_call_id": null, "timestamp": "2026-08-24T10:01:05+08:00"}
        ],
        "summary": "",
        "created": "2026-08-24T10:00:00+08:00",
        "updated": "2026-08-24T10:01:05+08:00"
    });
    std::fs::write(&sess_path, session_json.to_string()).unwrap();
    let log_path = ws
        .workspace()
        .join("logs")
        .join("session_logs")
        .join(format!("{safe}.jsonl"));
    std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
    let log_lines: Vec<String> = [
        "user|ITUI turn one q",
        "assistant|ITUI turn one a",
        "user|ITUI turn two q",
        "assistant|ITUI turn two a",
    ]
    .iter()
    .map(|l| {
        let (role, content) = l.split_once('|').unwrap();
        json!({"role": role, "content": content, "timestamp": "2026-08-24T10:00:00+08:00"})
            .to_string()
    })
    .collect();
    std::fs::write(&log_path, log_lines.join("\n") + "\n").unwrap();
}

// ---------------------------------------------------------------------------
// P1-1..4：memory auto_inject / auto_inject_top_k
// ---------------------------------------------------------------------------
pub async fn test_ui_p1_memory_auto_inject(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "ui/p1_memory_auto_inject";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, &format!("ws connect: {e}")));
            return results;
        }
    };
    let mem_cfg_path = ws
        .workspace()
        .join("config")
        .join("config.enhanced_memory.json");

    // P1-1 默认回显：无 config.enhanced_memory.json → false / 3
    let (data, err) = api.call("memory", "config.get", None).await;
    let ok = err.is_none()
        && data.as_ref().map_or(false, |d| {
            d.get("auto_inject").and_then(|v| v.as_bool()) == Some(false)
                && d.get("auto_inject_top_k").and_then(|v| v.as_u64()) == Some(3)
        });
    results.push(if ok {
        pass(&format!("{}/get_defaults", suite), "auto_inject=false top_k=3")
    } else {
        fail(
            &format!("{}/get_defaults", suite),
            &format!("err={err:?} data={data:?}"),
        )
    });

    // P1-2 set 落盘 + 往返：true / 5
    let (_, err) = api
        .call(
            "memory",
            "config.set",
            Some(json!({"auto_inject": true, "auto_inject_top_k": 5})),
        )
        .await;
    let disk = std::fs::read_to_string(&mem_cfg_path).unwrap_or_default();
    let disk_ok = disk.contains("\"auto_inject\": true") || disk.contains("\"auto_inject\":true");
    let disk_k =
        disk.contains("\"auto_inject_top_k\": 5") || disk.contains("\"auto_inject_top_k\":5");
    results.push(if err.is_none() && disk_ok && disk_k {
        pass(&format!("{}/set_persist", suite), "盘上 true/5")
    } else {
        fail(
            &format!("{}/set_persist", suite),
            &format!("err={err:?} disk='{disk}'"),
        )
    });

    let (data, err) = api.call("memory", "config.get", None).await;
    let rt_ok = data.as_ref().map_or(false, |d| {
        d.get("auto_inject").and_then(|v| v.as_bool()) == Some(true)
            && d.get("auto_inject_top_k").and_then(|v| v.as_u64()) == Some(5)
    });
    results.push(if err.is_none() && rt_ok {
        pass(&format!("{}/set_roundtrip", suite), "get 回显 true/5")
    } else {
        fail(
            &format!("{}/set_roundtrip", suite),
            &format!("err={err:?} data={data:?}"),
        )
    });

    // P1-3 越界：0 / 11 → 拒（含 1-10 提示）
    for bad in [0u64, 11u64] {
        let (_, err) = api
            .call("memory", "config.set", Some(json!({"auto_inject_top_k": bad})))
            .await;
        let refused = err.as_deref().map_or(false, |e| e.contains("1-10"));
        results.push(if refused {
            pass(&format!("{}/reject_{bad}", suite), "越界拒")
        } else {
            fail(&format!("{}/reject_{bad}", suite), &format!("err={err:?}"))
        });
    }

    // P1-3 边界：1 / 10 合法（顺序执行，后写覆盖前写，各自断言成功）
    for good in [1u64, 10u64] {
        let (_, err) = api
            .call("memory", "config.set", Some(json!({"auto_inject_top_k": good})))
            .await;
        results.push(if err.is_none() {
            pass(&format!("{}/boundary_{good}", suite), "边界值过")
        } else {
            fail(&format!("{}/boundary_{good}", suite), &format!("err={err:?}"))
        });
    }

    // P1-4 非法类型（字符串）：2026-08-24 复检修复——config.set 现按 sandbox
    // set_config 惯例对「出现但类型错」loud 拒绝（验收预案 P1-4 记录的
    // 静默忽略债已核销）。断言：报错指名字段 + 盘上保持 10 不落盘。
    let (_, err) = api
        .call("memory", "config.set", Some(json!({"auto_inject_top_k": "5"})))
        .await;
    let disk = std::fs::read_to_string(&mem_cfg_path).unwrap_or_default();
    let rejected = err
        .as_deref()
        .map_or(false, |e| e.contains("auto_inject_top_k"))
        && (disk.contains("\"auto_inject_top_k\": 10") || disk.contains("\"auto_inject_top_k\":10"));
    results.push(if rejected {
        pass(&format!("{}/string_type_rejected", suite), "拒且不落盘")
    } else {
        fail(
            &format!("{}/string_type_rejected", suite),
            &format!("err={err:?} disk='{disk}'"),
        )
    });

    results
}

// ---------------------------------------------------------------------------
// P1-5：cron max_rounds 三态（add → update → list 回显 + jobs.json 落盘）
// ---------------------------------------------------------------------------
pub async fn test_ui_p1_cron_max_rounds(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "ui/p1_cron_max_rounds";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, &format!("ws connect: {e}")));
            return results;
        }
    };

    let find_job = |data: &Option<Value>| -> Option<Value> {
        data.as_ref()?
            .get("jobs")?
            .as_array()?
            .iter()
            .find(|j| j.get("name").and_then(|n| n.as_str()) == Some("ui-p1-cron"))
            .cloned()
    };

    // add（带 max_rounds 5）
    let (_, err) = api
        .call(
            "tasks",
            "cron.add",
            Some(json!({
                "name": "ui-p1-cron",
                "cron": "*/5 * * * *",
                "prompt": "ui batch p1 acceptance",
                "max_rounds": 5,
            })),
        )
        .await;
    results.push(if err.is_none() {
        pass(&format!("{}/add", suite), "cron.add ok")
    } else {
        fail(&format!("{}/add", suite), &format!("err={err:?}"))
    });

    // list 找到 job id + max_rounds 回显
    let (data, err) = api.call("tasks", "cron.list", None).await;
    let job = find_job(&data);
    let (job_id, echoed) = match &job {
        Some(j) => (
            j.get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string(),
            j.get("max_rounds").and_then(|m| m.as_u64()),
        ),
        None => (String::new(), None),
    };
    results.push(if err.is_none() && job.is_some() && echoed == Some(5) {
        pass(&format!("{}/list_echo", suite), "回显 max_rounds=5")
    } else {
        fail(
            &format!("{}/list_echo", suite),
            &format!("err={err:?} job={job:?}"),
        )
    });

    // jobs.json 落盘对账（gateway 的 cron store = <home>/workspace/cron/jobs.json）
    let disk = std::fs::read_to_string(ws.workspace().join("cron").join("jobs.json"))
        .unwrap_or_default();
    results.push(if disk.contains("ui-p1-cron") && disk.contains("max_rounds") {
        pass(&format!("{}/jobs_json", suite), "jobs.json 含 job+max_rounds")
    } else {
        fail(&format!("{}/jobs_json", suite), &format!("disk='{disk}'"))
    });

    // 三态之 set：update 成 10 → 回显 10
    let (_, err) = api
        .call(
            "tasks",
            "cron.update",
            Some(json!({"id": job_id, "max_rounds": 10})),
        )
        .await;
    let (data, _) = api.call("tasks", "cron.list", None).await;
    let now = find_job(&data).and_then(|j| j.get("max_rounds").and_then(|m| m.as_u64()));
    results.push(if err.is_none() && now == Some(10) {
        pass(&format!("{}/update_set", suite), "10 回显")
    } else {
        fail(
            &format!("{}/update_set", suite),
            &format!("err={err:?} max_rounds={now:?}"),
        )
    });

    // 三态之 clear：null → 回显 null/缺省
    let (_, err) = api
        .call(
            "tasks",
            "cron.update",
            Some(json!({"id": job_id, "max_rounds": null})),
        )
        .await;
    let (data, _) = api.call("tasks", "cron.list", None).await;
    let cleared = find_job(&data).map(|j| j.get("max_rounds").map_or(true, |m| m.is_null()));
    results.push(if err.is_none() && cleared == Some(true) {
        pass(&format!("{}/update_clear", suite), "null 清除")
    } else {
        fail(
            &format!("{}/update_clear", suite),
            &format!("err={err:?} cleared={cleared:?}"),
        )
    });

    // 三态之 absent：不带 key → 不动 max_rounds（patch 其他字段验证不误伤）
    let (_, err) = api
        .call(
            "tasks",
            "cron.update",
            Some(json!({"id": job_id, "prompt": "ui-p1-patched"})),
        )
        .await;
    let (data, _) = api.call("tasks", "cron.list", None).await;
    let name_kept = find_job(&data).map_or(false, |j| {
        j.get("prompt").and_then(|p| p.as_str()) == Some("ui-p1-patched")
            && j.get("max_rounds").map_or(true, |m| m.is_null())
    });
    results.push(if err.is_none() && name_kept {
        pass(&format!("{}/update_absent_key", suite), "缺省 key 不动")
    } else {
        fail(
            &format!("{}/update_absent_key", suite),
            &format!("err={err:?} kept={name_kept}"),
        )
    });

    // 清理
    let _ = api
        .call("tasks", "cron.delete", Some(json!({"id": job_id})))
        .await;
    results
}

// ---------------------------------------------------------------------------
// P2-1..2：coding lsp_status / config
// ---------------------------------------------------------------------------
pub async fn test_ui_p2_coding() -> Vec<TestResult> {
    let suite = "ui/p2_coding";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, &format!("ws connect: {e}")));
            return results;
        }
    };

    // lsp_status：形状断言（不依赖机器装没装——布尔值随机器，字段形状固定）
    let (data, err) = api.call("coding", "lsp_status", None).await;
    let langs = data
        .as_ref()
        .and_then(|d| d.get("languages").and_then(|l| l.as_array()))
        .cloned()
        .unwrap_or_default();
    let shape_ok = err.is_none()
        && langs.len() >= 5
        && langs.iter().all(|l| {
            l.get("lang").map_or(false, |v| v.is_string())
                && l.get("command").map_or(false, |v| v.is_string())
                && l.get("available").map_or(false, |v| v.is_boolean())
        });
    results.push(if shape_ok {
        pass(
            &format!("{}/lsp_status_shape", suite),
            &format!("{} 语言形状齐", langs.len()),
        )
    } else {
        fail(
            &format!("{}/lsp_status_shape", suite),
            &format!("err={err:?} langs={langs:?}"),
        )
    });

    // config：三段回显（lsp / claude_code / codex——默认 off）
    let (data, err) = api.call("coding", "config", None).await;
    let cfg_ok = err.is_none()
        && data.as_ref().map_or(false, |d| {
            d.get("lsp")
                .and_then(|v| v.get("enabled"))
                .and_then(|v| v.as_bool())
                == Some(false)
                && d.get("claude_code").is_some()
                && d.get("codex").is_some()
        });
    results.push(if cfg_ok {
        pass(&format!("{}/config_sections", suite), "三段齐 + lsp 默认 off")
    } else {
        fail(
            &format!("{}/config_sections", suite),
            &format!("err={err:?} data={data:?}"),
        )
    });

    results
}

// ---------------------------------------------------------------------------
// P2-3：SDK HTTP export/pip（公开 GET 200 / 带 token 200 + zip 魔数）
// ---------------------------------------------------------------------------
pub async fn test_ui_p2_sdk_http() -> Vec<TestResult> {
    let suite = "ui/p2_sdk_http";
    let mut results = Vec::new();
    print_suite_header(suite);

    for path in ["/api/sdk/export", "/api/sdk/pip"] {
        let tag = path.replace('/', "_");
        // 无 token → 200（端点按设计公开：整个 /api/* GET 面无全局鉴权层，
        // auth 在 WS 层/个别 handler；SDK 两路由是无状态静态产物下载，与
        // /api/version、/api/config 同级。首跑曾误断言 401 —— 预期写错，
        // 非产品 bug，2026-08-24 修正。）
        let (status, _) = http_get(path, false).await;
        results.push(if status == 200 {
            pass(&format!("{}/public_200{tag}", suite), "200")
        } else {
            fail(&format!("{}/public_200{tag}", suite), &format!("status={status}"))
        });

        // 带 token → 200 + zip 魔数 PK\x03\x04
        let (status, body) = http_get(path, true).await;
        let zip_ok = body.len() > 4 && &body[0..2] == b"PK";
        results.push(if status == 200 && zip_ok {
            pass(
                &format!("{}/zip{tag}", suite),
                &format!("200 + zip ({}B)", body.len()),
            )
        } else {
            fail(
                &format!("{}/zip{tag}", suite),
                &format!(
                    "status={status} len={} head={:?}",
                    body.len(),
                    &body.get(0..4.min(body.len()))
                ),
            )
        });
    }

    results
}

// ---------------------------------------------------------------------------
// P3-3..6：models add/update_field/list/delete/catalog
// ---------------------------------------------------------------------------
pub async fn test_ui_p3_models_update_field(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "ui/p3_models_update_field";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, &format!("ws connect: {e}")));
            return results;
        }
    };

    // 自备模型（不依赖 Phase 1 的 model 测试状态）
    let (_, err) = api
        .call(
            "models",
            "add",
            Some(json!({
                "name": "ui-p3-model",
                "model": "test/ui-p3-model",
                "key": "test-key",
                "base_url": "http://127.0.0.1:8080/v1",
            })),
        )
        .await;
    if err.is_some() {
        results.push(fail(&format!("{}/add", suite), &format!("err={err:?}")));
        return results;
    }
    results.push(pass(&format!("{}/add", suite), "ok"));

    let uf = |field: &str, value: Value| {
        json!({
            "name": "ui-p3-model",
            "field": field,
            "value": value,
        })
    };

    // 合法：tier / effort 大小写归一 / size 字符串数字 / real_name
    let cases: &[(&str, Value, &str)] = &[
        ("model_tier", json!("mini"), "tier=mini"),
        ("reasoning_effort", json!("LOW"), "effort 大小写归一"),
        ("model_size_b", json!("30"), "size 字符串转数字"),
        ("real_name", json!("UiP3-Test"), "real_name"),
    ];
    for (field, value, label) in cases {
        let (data, err) = api
            .call("models", "update_field", Some(uf(field, value.clone())))
            .await;
        let ok = err.is_none()
            && data
                .as_ref()
                .and_then(|d| d.get("updated"))
                .and_then(|v| v.as_bool())
                == Some(true);
        results.push(if ok {
            pass(&format!("{}/set_{field}", suite), *label)
        } else {
            fail(
                &format!("{}/set_{field}", suite),
                &format!("err={err:?} data={data:?}"),
            )
        });
    }
    // effort=off → 清空（值为 ""）
    let (data, err) = api
        .call(
            "models",
            "update_field",
            Some(uf("reasoning_effort", json!("off"))),
        )
        .await;
    let off_ok = err.is_none()
        && data
            .as_ref()
            .and_then(|d| d.get("value"))
            .and_then(|v| v.as_str())
            == Some("");
    results.push(if off_ok {
        pass(&format!("{}/set_effort_off", suite), "off → \"\"")
    } else {
        fail(
            &format!("{}/set_effort_off", suite),
            &format!("err={err:?} data={data:?}"),
        )
    });

    // 非法：tier 枚举外 / effort 枚举外 / size 0 / real_name 空 / 未知字段 / 缺模型
    let bad: &[(&str, Value, &str)] = &[
        ("model_tier", json!("bogus"), "tier 拒"),
        ("reasoning_effort", json!("turbo"), "effort 拒"),
        ("model_size_b", json!(0), "size=0 拒"),
        ("real_name", json!("  "), "real_name 空拒"),
        ("no_such_field", json!(1), "未知字段拒"),
    ];
    for (field, value, label) in bad {
        let (_, err) = api
            .call("models", "update_field", Some(uf(field, value.clone())))
            .await;
        results.push(if err.is_some() {
            pass(&format!("{}/reject_{field}", suite), *label)
        } else {
            fail(&format!("{}/reject_{field}", suite), "未拒")
        });
    }
    let (_, err) = api
        .call(
            "models",
            "update_field",
            Some(json!({"name": "ui-p3-nonexistent", "field": "model_tier", "value": "mini"})),
        )
        .await;
    results.push(if err.is_some() {
        pass(&format!("{}/reject_missing_model", suite), "缺模型拒")
    } else {
        fail(&format!("{}/reject_missing_model", suite), "未拒")
    });

    // P3-4 拒后盘上不变：非法值全被拒后，config.json 里 ui-p3-model 的
    // tier 仍是 mini（拒绝路径从未触盘）。
    let entry_disk = read_home_config(ws)
        .get("model_list")
        .and_then(|l| l.as_array())
        .and_then(|a| {
            a.iter()
                .find(|m| m.get("model_name").and_then(|n| n.as_str()) == Some("ui-p3-model"))
                .cloned()
        })
        .unwrap_or(Value::Null);
    results.push(
        if entry_disk.get("model_tier").and_then(|v| v.as_str()) == Some("mini") {
            pass(&format!("{}/reject_no_disk_change", suite), "盘上 tier 仍 mini")
        } else {
            fail(
                &format!("{}/reject_no_disk_change", suite),
                &format!("entry={entry_disk}"),
            )
        },
    );

    // list 回读 extras（raw-RMW 的对外证明；list 条目键是 model_name）
    let find_model = |data: &Option<Value>| -> Option<Value> {
        data.as_ref()?
            .get("models")?
            .as_array()?
            .iter()
            .find(|m| m.get("model_name").and_then(|n| n.as_str()) == Some("ui-p3-model"))
            .cloned()
    };
    let (data, err) = api.call("models", "list", None).await;
    let entry = find_model(&data);
    let echo_ok = err.is_none()
        && entry.as_ref().map_or(false, |e| {
            e.get("model_tier").and_then(|v| v.as_str()) == Some("mini")
                && e.get("model_size_b").and_then(|v| v.as_u64()) == Some(30)
                && e.get("real_name").and_then(|v| v.as_str()) == Some("UiP3-Test")
        });
    results.push(if echo_ok {
        pass(&format!("{}/list_echo", suite), "extras 回读齐")
    } else {
        fail(
            &format!("{}/list_echo", suite),
            &format!("err={err:?} entry={entry:?}"),
        )
    });

    // P3-6 catalog_update 端到端证明：子进程（env 传 home.parent()，CLI join
    // 回同一 home）写 <home>/models_catalog.json，gateway 的 catalog_info 读同
    // 一路径 → exists:true。首跑（2026-08-24）在此抓到真 bug：handler 读点拼了
    // config/ 子目录与 CLI 写盘分叉 → 永远 exists:false（已修 models.rs 两处
    // 读点 + 夹具 3 处）。依赖网络（models.dev / 镜像）——网络失败按 skip 记录。
    let (data, err) = api.call("models", "catalog_update", None).await;
    let updated_ok = err.is_none()
        && data
            .as_ref()
            .and_then(|d| d.get("exists"))
            .and_then(|v| v.as_bool())
            == Some(true);
    if updated_ok {
        results.push(pass(
            &format!("{}/catalog_update", suite),
            "子进程落盘 + gateway 回读同路径",
        ));
    } else if err.is_some() {
        results.push(skip(
            &format!("{}/catalog_update", suite),
            &format!("网络不可达？err={err:?}"),
        ));
    } else {
        results.push(fail(
            &format!("{}/catalog_update", suite),
            &format!("子进程成功但 gateway 读不到 catalog（读写路径分叉？）data={data:?}"),
        ));
    }

    // 清理（delete 后 list 里 ui-p3-model 消失）
    let (_, err) = api
        .call("models", "delete", Some(json!({"name": "ui-p3-model"})))
        .await;
    let (data, _) = api.call("models", "list", None).await;
    let gone = find_model(&data).is_none();
    results.push(if err.is_none() && gone {
        pass(&format!("{}/cleanup", suite), "delete ok")
    } else {
        fail(&format!("{}/cleanup", suite), &format!("err={err:?} gone={gone}"))
    });

    results
}

// ---------------------------------------------------------------------------
// P3-1..2：HTTP turns/fork（401/404/轮次表/前缀分叉/原会话不动）
// ---------------------------------------------------------------------------
pub async fn test_ui_p3_fork_http(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "ui/p3_fork_http";
    let mut results = Vec::new();
    print_suite_header(suite);

    // 种子在 seed_fork_sessions（gateway 启动前）完成；这里只断言 HTTP 行为。
    // URL 用 dashboard session id（itui1），handler 映射到 agent:main:session:itui1。
    let sess_path = ws
        .workspace()
        .join("sessions")
        .join("agent_main_session_itui1.json");
    let source_before = std::fs::read_to_string(&sess_path).unwrap_or_default();

    // 401：无 token
    let (status, _) = http_get("/api/chat/sessions/itui1/turns", false).await;
    results.push(if status == 401 {
        pass(&format!("{}/turns_401", suite), "401")
    } else {
        fail(&format!("{}/turns_401", suite), &format!("status={status}"))
    });

    // 404：不存在的 session
    let (status, _) = http_get("/api/chat/sessions/nope/turns", true).await;
    results.push(if status == 404 {
        pass(&format!("{}/turns_404", suite), "404")
    } else {
        fail(&format!("{}/turns_404", suite), &format!("status={status}"))
    });

    // 200：轮次表（2 轮，turn 2 预览含 turn two q）
    let (status, raw) = http_get("/api/chat/sessions/itui1/turns", true).await;
    let body = String::from_utf8_lossy(&raw).to_string();
    let turns_ok = status == 200 && body.contains("ITUI turn two q") && body.contains("\"turn\":2");
    results.push(if turns_ok {
        pass(&format!("{}/turns_200", suite), "轮次表 2 轮")
    } else {
        fail(
            &format!("{}/turns_200", suite),
            &format!("status={status} body='{body}'"),
        )
    });

    // fork 401 / 404
    let (status, _) = http_post_json("/api/chat/sessions/itui1/fork", "{}", false).await;
    results.push(if status == 401 {
        pass(&format!("{}/fork_401", suite), "401")
    } else {
        fail(&format!("{}/fork_401", suite), &format!("status={status}"))
    });
    let (status, _) = http_post_json("/api/chat/sessions/nope/fork", "{}", true).await;
    results.push(if status == 404 {
        pass(&format!("{}/fork_404", suite), "404")
    } else {
        fail(&format!("{}/fork_404", suite), &format!("status={status}"))
    });

    // fork at_turn=1：新 session 文件只含第 1 轮（2 条），原会话字节不动。
    // new_key 由服务端生成（响应回传），文件名 = sanitize(new_key).json。
    let (status, body) = http_post_json("/api/chat/sessions/itui1/fork", r#"{"at_turn":1}"#, true).await;
    let resp: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let new_key = resp
        .get("new_key")
        .and_then(|k| k.as_str())
        .unwrap_or("")
        .to_string();
    let fork_path = ws
        .workspace()
        .join("sessions")
        .join(format!("{}.json", sanitize_key(&new_key)));
    let fork_json: Value = std::fs::read_to_string(&fork_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let msg_count = fork_json
        .get("messages")
        .and_then(|m| m.as_array())
        .map(|a| a.len());
    let source_after = std::fs::read_to_string(&sess_path).unwrap_or_default();
    let log_lines = resp
        .get("chat_log_lines")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let fork_ok = status == 200
        && !new_key.is_empty()
        && msg_count == Some(2)
        && fork_json
            .get("messages")
            .and_then(|m| m.as_array())
            .and_then(|a| a.first())
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            == Some("ITUI turn one q")
        && log_lines >= 1
        && source_after == source_before;
    results.push(if fork_ok {
        pass(&format!("{}/fork_at1", suite), "前缀 2 条 + 原会话不动")
    } else {
        fail(
            &format!("{}/fork_at1", suite),
            &format!("status={status} new_key='{new_key}' msgs={msg_count:?} log={log_lines}"),
        )
    });

    // fork 缺省 at_turn：全量（4 条）
    let (status, body) = http_post_json("/api/chat/sessions/itui1/fork", "{}", true).await;
    let resp: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let new_key = resp
        .get("new_key")
        .and_then(|k| k.as_str())
        .unwrap_or("")
        .to_string();
    let full_path = ws
        .workspace()
        .join("sessions")
        .join(format!("{}.json", sanitize_key(&new_key)));
    let full_json: Value = std::fs::read_to_string(&full_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let full_count = full_json
        .get("messages")
        .and_then(|m| m.as_array())
        .map(|a| a.len());
    results.push(if status == 200 && full_count == Some(4) {
        pass(&format!("{}/fork_full", suite), "全量 4 条")
    } else {
        fail(
            &format!("{}/fork_full", suite),
            &format!("status={status} new_key='{new_key}' msgs={full_count:?}"),
        )
    });

    results
}

// ---------------------------------------------------------------------------
// P4-1..4：hooks get/set
// ---------------------------------------------------------------------------
pub async fn test_ui_p4_hooks(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "ui/p4_hooks";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, &format!("ws connect: {e}")));
            return results;
        }
    };
    let hooks_path = ws.home().join("config").join("hooks.json");

    // P4-1 缺文件 → 模板（exists=false，模板自身可解析 + valid:true）
    let (data, err) = api.call("hooks", "get", None).await;
    let tmpl_ok = err.is_none()
        && data.as_ref().map_or(false, |d| {
            d.get("exists").and_then(|v| v.as_bool()) == Some(false)
                && d.get("valid").and_then(|v| v.as_bool()) == Some(true)
                && d.get("content").and_then(|c| c.as_str()).map_or(false, |c| {
                    serde_json::from_str::<Value>(c).is_ok()
                })
        });
    results.push(if tmpl_ok {
        pass(&format!("{}/get_template", suite), "模板可解析")
    } else {
        fail(
            &format!("{}/get_template", suite),
            &format!("err={err:?} data={data:?}"),
        )
    });

    // P4-3 set 合法 → 原文照写（非 pretty 重排：紧凑原文逐字节落盘）
    let original = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"exit 0"}]}]}}"#;
    let (_, err) = api
        .call("hooks", "set", Some(json!({"content": original})))
        .await;
    let disk = std::fs::read_to_string(&hooks_path).unwrap_or_default();
    results.push(if err.is_none() && disk == original {
        pass(&format!("{}/set_verbatim", suite), "原文逐字节")
    } else {
        fail(
            &format!("{}/set_verbatim", suite),
            &format!("err={err:?} disk='{disk}'"),
        )
    });

    // P4-2 get 有文件 → 原文 roundtrip + summary 计数（1 个脚本）
    let (data, err) = api.call("hooks", "get", None).await;
    let rt_ok = err.is_none()
        && data
            .as_ref()
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_str())
            == Some(original)
        && data
            .as_ref()
            .and_then(|d| d.get("exists"))
            .and_then(|v| v.as_bool())
            == Some(true)
        && data
            .as_ref()
            .and_then(|d| d.get("summary"))
            .and_then(|s| s.get("total"))
            .and_then(|t| t.as_u64())
            == Some(1);
    results.push(if rt_ok {
        pass(&format!("{}/get_roundtrip", suite), "原文 + summary=1")
    } else {
        fail(
            &format!("{}/get_roundtrip", suite),
            &format!("err={err:?} data={data:?}"),
        )
    });

    // P4-4 set 非法 → 拒且盘上文件不被破坏（保持上一版原文）
    let (_, err) = api
        .call("hooks", "set", Some(json!({"content": "{ not json"})))
        .await;
    let disk = std::fs::read_to_string(&hooks_path).unwrap_or_default();
    results.push(if err.is_some() && disk == original {
        pass(&format!("{}/set_invalid_kept", suite), "拒 + 盘上未破坏")
    } else {
        fail(
            &format!("{}/set_invalid_kept", suite),
            &format!("err={err:?} disk='{disk}'"),
        )
    });

    // P4 附加：盘上损坏文件 → get 返回 valid:false + 原文
    std::fs::write(&hooks_path, "corrupt {{{").unwrap();
    let (data, err) = api.call("hooks", "get", None).await;
    let corrupt_ok = err.is_none()
        && data
            .as_ref()
            .and_then(|d| d.get("valid"))
            .and_then(|v| v.as_bool())
            == Some(false)
        && data
            .as_ref()
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_str())
            == Some("corrupt {{{");
    results.push(if corrupt_ok {
        pass(&format!("{}/get_corrupt", suite), "valid:false + 原文")
    } else {
        fail(
            &format!("{}/get_corrupt", suite),
            &format!("err={err:?} data={data:?}"),
        )
    });

    // 清理（后续批次不需要 hooks.json）
    let _ = std::fs::remove_file(&hooks_path);
    results
}

// ---------------------------------------------------------------------------
// P5-7..10：sandbox overview / set_config / status
// ---------------------------------------------------------------------------
pub async fn test_ui_p5_sandbox(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "ui/p5_sandbox";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, &format!("ws connect: {e}")));
            return results;
        }
    };

    // P5-7 overview 形状（Windows：kind=sandboxie + 三布尔 + ready；其他平台
    // 分支本机不跑，由 handler 单测 + WSL 侧覆盖）
    let (data, err) = api.call("sandbox", "overview", None).await;
    let d = data.clone().unwrap_or(Value::Null);
    let platform = d
        .get("platform")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();
    let exec_ok = d.get("executor").map_or(false, |e| {
        ["enabled", "sandbox", "allow_network", "strict"]
            .iter()
            .all(|k| e.get(k).and_then(|v| v.as_bool()).is_some())
    });
    let probe_ok = if platform == "windows" {
        d.get("backend_probe")
            .and_then(|b| b.get("kind"))
            .and_then(|k| k.as_str())
            == Some("sandboxie")
            && ["start_exe_present", "sbiesvc_running", "engine_owned"].iter().all(|k| {
                d.get("backend_probe")
                    .and_then(|b| b.get(k))
                    .and_then(|v| v.as_bool())
                    .is_some()
            })
    } else {
        d.get("backend_probe")
            .and_then(|b| b.get("kind"))
            .and_then(|k| k.as_str())
            == Some("userland")
            && d
                .get("backend_probe")
                .and_then(|b| b.get("backends"))
                .map_or(false, |v| v.is_array())
    };
    let shape_ok = err.is_none()
        && ["windows", "linux", "macos", "other"].contains(&platform.as_str())
        && exec_ok
        && probe_ok
        && d.get("ready").and_then(|v| v.as_bool()).is_some();
    results.push(if shape_ok {
        pass(
            &format!("{}/overview_shape", suite),
            &format!("platform={platform}"),
        )
    } else {
        fail(
            &format!("{}/overview_shape", suite),
            &format!("err={err:?} data={data:?}"),
        )
    });

    // P5-8 联动落盘：{enabled:true,sandbox:true} → 盘上 executor 段 + restart_hint
    let (data, err) = api
        .call(
            "sandbox",
            "set_config",
            Some(json!({"enabled": true, "sandbox": true})),
        )
        .await;
    let cfg = read_home_config(ws);
    let disk_exec = cfg.get("executor").cloned().unwrap_or(Value::Null);
    let step1 = err.is_none()
        && disk_exec.get("enabled").and_then(|v| v.as_bool()) == Some(true)
        && disk_exec.get("sandbox").and_then(|v| v.as_bool()) == Some(true)
        && data
            .as_ref()
            .and_then(|d| d.get("restart_hint"))
            .map_or(false, |h| h.is_string());
    results.push(if step1 {
        pass(&format!("{}/set_enabled_sandbox", suite), "联动落盘 + restart_hint")
    } else {
        fail(
            &format!("{}/set_enabled_sandbox", suite),
            &format!("err={err:?} disk={disk_exec}"),
        )
    });

    // P5-8 字段级合并：{strict:true} 只动 strict，保住 enabled/sandbox
    let (_, err) = api
        .call("sandbox", "set_config", Some(json!({"strict": true})))
        .await;
    let cfg = read_home_config(ws);
    let disk_exec = cfg.get("executor").cloned().unwrap_or(Value::Null);
    let merged = err.is_none()
        && disk_exec.get("strict").and_then(|v| v.as_bool()) == Some(true)
        && disk_exec.get("enabled").and_then(|v| v.as_bool()) == Some(true)
        && disk_exec.get("sandbox").and_then(|v| v.as_bool()) == Some(true);
    results.push(if merged {
        pass(&format!("{}/set_merge", suite), "strict 落盘 + 兄弟保留")
    } else {
        fail(
            &format!("{}/set_merge", suite),
            &format!("err={err:?} disk={disk_exec}"),
        )
    });

    // P5-9 非法：非 bool（拒并指名字段）/ 空 payload / 拒后盘上不变
    let (_, err) = api
        .call("sandbox", "set_config", Some(json!({"sandbox": "yes"})))
        .await;
    let refused_field = err.as_deref().map_or(false, |e| e.contains("sandbox"));
    results.push(if refused_field {
        pass(&format!("{}/reject_non_bool", suite), "拒并指名字段")
    } else {
        fail(&format!("{}/reject_non_bool", suite), &format!("err={err:?}"))
    });
    let (_, err) = api.call("sandbox", "set_config", Some(json!({}))).await;
    results.push(if err.is_some() {
        pass(&format!("{}/reject_empty", suite), "空拒")
    } else {
        fail(&format!("{}/reject_empty", suite), "未拒")
    });
    let cfg = read_home_config(ws);
    results.push(
        if cfg.pointer("/executor/strict").and_then(|v| v.as_bool()) == Some(true) {
            pass(&format!("{}/reject_no_disk_change", suite), "盘上未变")
        } else {
            fail(&format!("{}/reject_no_disk_change", suite), "盘上被改")
        },
    );

    // P5-10 status 端点可用（直接 SCM/文件探测，不走子进程——run_cli_subcmd
    // 只被 sandbox start/stop 用，属破坏性不可测；HOME parent() 修复的
    // 运行时证明由 P3-6 catalog_update 承担，同一模式同一坑）
    let (data, err) = api.call("sandbox", "status", None).await;
    results.push(if err.is_none() && data.is_some() {
        pass(&format!("{}/status_ok", suite), "status 可用")
    } else {
        fail(&format!("{}/status_ok", suite), &format!("err={err:?}"))
    });

    results
}
