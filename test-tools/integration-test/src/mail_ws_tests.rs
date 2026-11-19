//! mail WSAPI 表面契约套件（邮件基础设施 B1-B3 IT，2026-11-19）。
//!
//! IT 环境无 plugin_mail DLL（构建产物只随发行包部署，不进 target/<profile>/），
//! 恰好钉**空态契约**三层语义：
//! 1. `mail.status` 槽空 = `{loaded:false, hint}` **非报错**（状态查询的职责
//!    就是暴露装载状态，前端空态卡数据源）；
//! 2. `mail.config_get`/`config_set` 先于句柄闸（插件缺席也要能配——空态卡
//!    「先配好再装载」引导闭环）：缺文件读 `{}`、写整体落盘 + 重启生效 note、
//!    非 object 配置 loud 拒；
//! 3. 全部业务命令（send/list/read/mark_read/delete/stats/record/addr_*）
//!    与未知命令槽空统一诚实报错「mail 插件未装载…」——不给探测面信号差
//!    （未知命令的区分信号在 handle 闸之后，槽空时同样吃装载报错）。
//!
//! 装载态（SMTP 收发全链）由 agent-bench mail_roundtrip 场景覆盖（LLM e2e，
//! 真实 DLL + TLS sink + 明文投递双腿），IT 单节点不重复。
//!
//! 写盘面：config_set 落 `<home>/workspace/config/config.mail.json`（IT 临时
//! 工作区，套件结束删除恢复原状），沿「改共享状态套件排尾」纪律。

use serde_json::{Value, json};
use std::path::Path;
use test_harness::*;

use crate::ui_batch_series::WsApi;

/// 槽空统一报错关键词（mail.rs 单一文案，改动即响）。
const UNLOADED_ERR: &str = "未装载";

pub async fn test_mail_wsapi_surface(ws: &TestWorkspace, _bin: &Path) -> Vec<TestResult> {
    let suite = "mail_ws/surface";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // ── 1. status 空态：loaded:false + hint，非报错 ──
    let (dat, err) = api.call("mail", "status", None).await;
    match (&dat, &err) {
        (Some(v), None) => {
            let loaded = v.get("loaded").and_then(|x| x.as_bool());
            let hint = v.get("hint").and_then(|x| x.as_str()).unwrap_or("");
            if loaded == Some(false) && hint.contains(UNLOADED_ERR) {
                results.push(pass(
                    &format!("{suite}/status_empty_state"),
                    "mail.status 空态 = loaded:false + 诚实 hint（非报错）",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/status_empty_state"),
                    format!(
                        "mail.status 空态形状不符（loaded={loaded:?}，hint 含未装载={}）: {v}",
                        hint.contains(UNLOADED_ERR)
                    ),
                ));
            }
        }
        (None, Some(e)) => results.push(fail(
            &format!("{suite}/status_empty_state"),
            format!("mail.status 空态不应报错，得 error: {e}"),
        )),
        _ => results.push(fail(
            &format!("{suite}/status_empty_state"),
            "mail.status 空态返回缺 data",
        )),
    }

    // ── 2. config_get 缺文件：{}（全默认档）+ path ──
    let cfg_path = ws.workspace().join("config").join("config.mail.json");
    let _ = std::fs::remove_file(&cfg_path); // 确保缺文件前提
    let (dat, err) = api.call("mail", "config_get", None).await;
    match (&dat, &err) {
        (Some(v), None) => {
            let config_empty = v
                .get("config")
                .map(|c| c.as_object().map(|o| o.is_empty()).unwrap_or(false));
            let has_path = v.get("path").and_then(|x| x.as_str()).is_some();
            if config_empty == Some(true) && has_path {
                results.push(pass(
                    &format!("{suite}/config_get_missing_file"),
                    "mail.config_get 缺文件 = {}（全默认档）+ path",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/config_get_missing_file"),
                    format!(
                        "mail.config_get 形状不符（config 空={}，有 path={}）: {v}",
                        config_empty.unwrap_or(false),
                        has_path
                    ),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/config_get_missing_file"),
            format!("mail.config_get 缺文件应返回空对象，得 error: {err:?}"),
        )),
    }

    // ── 3. config_set 非 object：loud 拒 ──
    let (dat, err) = api
        .call(
            "mail",
            "config_set",
            Some(json!({"config": "not-an-object"})),
        )
        .await;
    match (&dat, &err) {
        (None, Some(e)) if e.contains("object") => results.push(pass(
            &format!("{suite}/config_set_reject_non_object"),
            "mail.config_set 非 object 配置 loud 拒",
        )),
        _ => results.push(fail(
            &format!("{suite}/config_set_reject_non_object"),
            format!("mail.config_set 非 object 应报错含 object，得 dat={dat:?} err={err:?}"),
        )),
    }

    // ── 4. config_set 写入 + note（重启生效）+ 盘面 + config_get 往返 ──
    let probe = json!({"enabled": true, "smtp": {"enabled": false}});
    let (dat, err) = api
        .call("mail", "config_set", Some(json!({"config": probe})))
        .await;
    let wrote_ok = matches!(&dat, Some(v) if v.get("ok").and_then(|x| x.as_bool()) == Some(true)
        && v.get("note").and_then(|x| x.as_str()).unwrap_or("").contains("重启"));
    if wrote_ok {
        results.push(pass(
            &format!("{suite}/config_set_write"),
            "mail.config_set 写入成功 + 重启生效 note",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/config_set_write"),
            format!("mail.config_set 应 ok+重启 note，得 dat={dat:?} err={err:?}"),
        ));
    }
    // 盘面真相（pretty JSON 落盘）。
    let disk = std::fs::read_to_string(&cfg_path).ok().and_then(|raw| {
        serde_json::from_str::<Value>(&raw)
            .ok()
            .map(|v| v.get("enabled").and_then(|x| x.as_bool()) == Some(true))
    });
    if disk == Some(true) {
        results.push(pass(
            &format!("{suite}/config_set_disk_truth"),
            "config_set 盘面真相：config.mail.json 落盘含写入键",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/config_set_disk_truth"),
            format!("config.mail.json 盘面不符（{:?}）", cfg_path.display()),
        ));
    }
    // config_get 往返。
    let (dat, err) = api.call("mail", "config_get", None).await;
    let roundtrip = matches!(&dat, Some(v)
        if v.get("config").and_then(|c| c.get("enabled")).and_then(|x| x.as_bool()) == Some(true));
    if roundtrip {
        results.push(pass(
            &format!("{suite}/config_roundtrip"),
            "config_get 读回写入键（写读往返成立）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/config_roundtrip"),
            format!("config_get 往返不符，得 dat={dat:?} err={err:?}"),
        ));
    }

    // ── 5. 业务命令槽空统一诚实报错（不给探测面信号差）──
    let probes: Vec<(&str, Option<Value>)> = vec![
        (
            "send",
            Some(json!({"to": "x@y.local", "subject": "s", "body": "b"})),
        ),
        ("list", Some(json!({}))),
        ("read", Some(json!({"id": "whatever"}))),
        ("mark_read", Some(json!({"id": "whatever"}))),
        ("delete", Some(json!({"id": "whatever"}))),
        ("stats", None),
        ("record", Some(json!({"message_id": "m"}))),
        ("addr_add", Some(json!({"address": "a@bench.local"}))),
        ("addr_remove", Some(json!({"address": "a@bench.local"}))),
        ("addr_list", None),
        // 未知命令同样吃装载报错（handle 闸先于命令分派——探测面无信号差）。
        ("totally_unknown_cmd", None),
    ];
    let mut uniform_ok = 0usize;
    let mut uniform_details = Vec::new();
    for (cmd, data) in &probes {
        let (_dat, err) = api.call("mail", cmd, data.clone()).await;
        match &err {
            Some(e) if e.contains(UNLOADED_ERR) => uniform_ok += 1,
            other => uniform_details.push(format!("{cmd} → {other:?}")),
        }
    }
    if uniform_ok == probes.len() {
        results.push(pass(
            &format!("{suite}/unified_unloaded_error"),
            format!(
                "{} 个业务+未知命令槽空统一报「未装载」（无探测面信号差）",
                probes.len()
            ),
        ));
    } else {
        results.push(fail(
            &format!("{suite}/unified_unloaded_error"),
            format!(
                "槽空统一报错 {uniform_ok}/{}，偏差: {}",
                probes.len(),
                uniform_details.join("; ")
            ),
        ));
    }

    // ── Cleanup：删探针配置恢复原状 ──
    let _ = std::fs::remove_file(&cfg_path);
    results
}
