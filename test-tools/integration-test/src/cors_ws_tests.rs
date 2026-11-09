//! CORS WSAPI 连接态契约（2026-11-08 配置页补全 C-1 接真 IT）。
//!
//! IT 运行相（Phase 2 起）gateway 启动前 Phase 1 的 cli/cors 套件已把
//! cors.json 落在盘上（web_init：cors.json 存在才构造 + 注入 manager）→
//! 运行相 manager 恒连接。本套件钉**连接态真语义**：WSAPI 四命令与
//! gateway 持有的同一 CORSManager 实例交互（list 形状 / add→list+落盘 /
//! remove / toggle 往返），写操作落盘 cors.json = 「改后需重启 gateway
//! 才影响 CorsLayer」语义的盘面真相。
//!
//! not-connected（None）路径由 nemesis-web 单测
//! `config_cors_without_manager_honest_errors` 钉住（handler 层诚实报错）。

use serde_json::json;
use std::path::Path;
use test_harness::*;

use crate::ui_batch_series::WsApi;

fn cors_json_path(ws: &TestWorkspace) -> std::path::PathBuf {
    ws.workspace().join("config").join("cors.json")
}

const TEST_ORIGIN: &str = "https://it-cors-ws.example.com";

pub async fn test_cors_wsapi_surface(ws: &TestWorkspace, _bin: &Path) -> Vec<TestResult> {
    let suite = "cors_ws/surface";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // ── 连接态：cors.list 返回真实配置形状（manager 已注入）──
    let (dat, err) = api.call("config", "cors.list", None).await;
    match &dat {
        Some(v) => {
            let shape_ok = v.get("origins").is_some()
                && v.get("development_mode").is_some()
                && v.get("allowed_origins").is_some()
                && v.get("allow_localhost").is_some();
            if shape_ok {
                results.push(pass(
                    &format!("{suite}/list_shape"),
                    "cors.list 返回真实管理器配置形状",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/list_shape"),
                    format!("cors.list 缺关键字段：{v}"),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/list_shape"),
            format!("cors.list 应成功（运行相 manager 恒连接），err={err:?}"),
        )),
    }

    // ── add：幂等唯一 origin → list 含 → 盘上 cors.json 含（落盘真相）──
    let (dat, err) = api
        .call("config", "cors.add", Some(json!({ "origin": TEST_ORIGIN })))
        .await;
    match &dat {
        Some(v) if v["added"] == json!(true) => {
            results.push(pass(&format!("{suite}/add"), "cors.add added=true"));
        }
        _ => results.push(fail(
            &format!("{suite}/add"),
            format!("cors.add 应成功，dat={dat:?} err={err:?}"),
        )),
    }

    let (dat, err) = api.call("config", "cors.list", None).await;
    let in_list = dat
        .as_ref()
        .and_then(|v| v["origins"].as_array())
        .map(|a| a.iter().any(|o| o.as_str() == Some(TEST_ORIGIN)))
        .unwrap_or(false);
    if in_list {
        results.push(pass(&format!("{suite}/add_visible"), "list 含新 origin"));
    } else {
        results.push(fail(
            &format!("{suite}/add_visible"),
            format!("cors.list 应含 {TEST_ORIGIN}，dat={dat:?} err={err:?}"),
        ));
    }

    let pj = cors_json_path(ws);
    let on_disk = std::fs::read_to_string(&pj)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let persisted = on_disk
        .as_ref()
        .map(|v| {
            v["allowed_origins"]
                .as_array()
                .map(|a| a.iter().any(|o| o.as_str() == Some(TEST_ORIGIN)))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if persisted {
        results.push(pass(
            &format!("{suite}/add_persisted"),
            "WSAPI add 落盘 cors.json（与 CLI 同源写穿）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/add_persisted"),
            format!("cors.json 应含 {TEST_ORIGIN}，实得 {:?}", on_disk),
        ));
    }

    // ── remove：list 不再含 → 盘上同步 ──
    let (dat, err) = api
        .call(
            "config",
            "cors.remove",
            Some(json!({ "origin": TEST_ORIGIN })),
        )
        .await;
    match &dat {
        Some(v) if v["removed"] == json!(true) => {
            results.push(pass(&format!("{suite}/remove"), "cors.remove removed=true"));
        }
        _ => results.push(fail(
            &format!("{suite}/remove"),
            format!("cors.remove 应成功，dat={dat:?} err={err:?}"),
        )),
    }

    let (dat, _err) = api.call("config", "cors.list", None).await;
    let gone = dat
        .as_ref()
        .and_then(|v| v["origins"].as_array())
        .map(|a| !a.iter().any(|o| o.as_str() == Some(TEST_ORIGIN)))
        .unwrap_or(false);
    if gone {
        results.push(pass(
            &format!("{suite}/remove_visible"),
            "list 已不含该 origin",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/remove_visible"),
            format!("cors.list 应不含 {TEST_ORIGIN}，dat={dat:?}"),
        ));
    }

    // ── toggle 往返：true → development_mode=true → 还原 false ──
    let (dat, err) = api
        .call("config", "cors.toggle", Some(json!({ "enabled": true })))
        .await;
    match &dat {
        Some(v) if v["development_mode"] == json!(true) => {
            results.push(pass(&format!("{suite}/toggle_on"), "dev-mode 开"));
        }
        _ => results.push(fail(
            &format!("{suite}/toggle_on"),
            format!("cors.toggle(true) 应成功，dat={dat:?} err={err:?}"),
        )),
    }
    let (dat, err) = api
        .call("config", "cors.toggle", Some(json!({ "enabled": false })))
        .await;
    match &dat {
        Some(v) if v["development_mode"] == json!(false) => {
            results.push(pass(
                &format!("{suite}/toggle_off_restore"),
                "dev-mode 关（还原）",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/toggle_off_restore"),
            format!("cors.toggle(false) 应成功，dat={dat:?} err={err:?}"),
        )),
    }

    // ── 非法 origin 拒绝（镜像 CLI 校验）──
    let (dat, err) = api
        .call("config", "cors.add", Some(json!({ "origin": "not-a-url" })))
        .await;
    match (&dat, &err) {
        (None, Some(e)) if e.contains("http") => {
            results.push(pass(
                &format!("{suite}/reject_invalid"),
                "无 scheme origin 诚实拒绝",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/reject_invalid"),
            format!("非法 origin 应被拒，dat={dat:?} err={err:?}"),
        )),
    }

    results
}
