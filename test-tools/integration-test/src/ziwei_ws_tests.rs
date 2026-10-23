//! Ziwei WSAPI 表面契约套件（M2，2026-10-23）。
//!
//! 单节点范围：门关拒绝面（默认 enabled=false → 全命令诚实拒绝）→ 热开闸
//! （写 config.json ziwei.enabled=true，handler 每请求现读）→ 只读面 +
//! 未装配诚实报（manager 随 gateway 启动装配，闸后开的会话面不装配）。
//! 真实附加/报警链路在 nemesis-ziwei/tests/ziwei_m2_live.rs（实弹），
//! 本套件只验 WSAPI 表面契约。
//!
//! 运行位置：main.rs 排尾（connectors 之后、cleanup 之前）——改工作区
//! config.json，套件结束删除 ziwei 键恢复原状。

use serde_json::json;
use std::path::Path;
use test_harness::*;

use crate::ui_batch_series::WsApi;

/// IT 契约断言串（与 `handlers/ziwei.rs` GATE_CLOSED_MSG 同步）。
const GATE_CLOSED: &str = "紫微系统未启用（config ziwei.enabled=false）";

fn config_path(ws: &TestWorkspace) -> std::path::PathBuf {
    ws.config_path()
}

/// 热开/关闸（handler 每请求现读 config——与 tier 同语义）。
fn set_gate(ws: &TestWorkspace, enabled: bool) -> Result<(), String> {
    let p = config_path(ws);
    let raw = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let mut v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if enabled {
        v["ziwei"]["enabled"] = json!(true);
    } else {
        // 恢复原状：删除 ziwei 键（默认 false 语义不变）。
        if let Some(obj) = v.as_object_mut() {
            obj.remove("ziwei");
        }
    }
    std::fs::write(&p, serde_json::to_string(&v).unwrap()).map_err(|e| e.to_string())
}

pub async fn test_ziwei_surface(ws: &TestWorkspace, _bin: &Path) -> Vec<TestResult> {
    let suite = "ziwei_ws/surface";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // ── 门关面（默认配置 ziwei.enabled 缺席 = false）──
    for cmd in ["list", "attach", "invoke", "refresh", "setup.select_target"] {
        let (dat, err) = api.call("ziwei", cmd, None).await;
        match (&dat, &err) {
            (None, Some(e)) if e.contains(GATE_CLOSED) => {
                results.push(pass(&format!("{suite}/gate_closed_{cmd}"), "门关诚实拒绝"));
            }
            _ => results.push(fail(
                &format!("{suite}/gate_closed_{cmd}"),
                format!("应门关拒绝，实得 dat={dat:?} err={err:?}"),
            )),
        }
    }

    // ── L1 注册表：ziwei 模块命令清单全量在册（commands() 是裸名，模块内聚合）──
    let (dat, err) = api.call("system", "commands", None).await;
    match (&dat, &err) {
        (Some(d), None) => {
            let zw_cmds: Vec<String> = d["modules"]
                .as_array()
                .map(|mods| {
                    mods.iter()
                        .filter(|m| m["module"] == "ziwei")
                        .flat_map(|m| m["commands"].as_array().cloned().unwrap_or_default())
                        .filter_map(|c| c.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let mut missing = Vec::new();
            for c in [
                "list",
                "show",
                "attach",
                "detach",
                "invoke",
                "status",
                "remove",
                "refresh",
                "verify",
                "recover",
                "recover_confirm",
                "setup.select_target",
            ] {
                if !zw_cmds.iter().any(|n| n == c) {
                    missing.push(c);
                }
            }
            if missing.is_empty() {
                results.push(pass(&format!("{suite}/l1_registry"), "20 命令全量在册"));
            } else {
                results.push(fail(
                    &format!("{suite}/l1_registry"),
                    format!("缺失: {missing:?}"),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/l1_registry"),
            "system.commands 失败",
        )),
    }

    // ── 热开闸（handler 每请求现读）→ 只读面 + 未装配诚实报 ──
    if let Err(e) = set_gate(ws, true) {
        results.push(fail(
            &format!("{suite}/gate_open"),
            format!("config 写失败: {e}"),
        ));
        return results;
    }

    let (dat, err) = api.call("ziwei", "list", None).await;
    match (&dat, &err) {
        (Some(d), None)
            if d["profiles"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(false) =>
        {
            results.push(pass(&format!("{suite}/list_empty"), "开闸后空表"));
        }
        _ => results.push(fail(
            &format!("{suite}/list_empty"),
            format!("应空 profiles，实得 dat={dat:?} err={err:?}"),
        )),
    }

    let (dat, err) = api.call("ziwei", "status", None).await;
    match (&dat, &err) {
        (Some(d), None) if d["enabled"] == true => {
            // manager 随 gateway 启动装配——启动时闸关 = 不装配（诚实面）。
            let manager_ok = d["manager_present"] == false
                && d["sessions"]
                    .as_array()
                    .map(|a| a.is_empty())
                    .unwrap_or(false);
            if manager_ok {
                results.push(pass(
                    &format!("{suite}/status_unassembled_honest"),
                    "闸后开：manager 未装配诚实可见",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/status_unassembled_honest"),
                    format!("manager_present/sessions 面异常: {d}"),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/status_unassembled_honest"),
            format!("status 应 enabled=true，实得 dat={dat:?} err={err:?}"),
        )),
    }

    let (dat, err) = api
        .call(
            "ziwei",
            "invoke",
            Some(json!({ "profile_id": "ghost", "tool": "t" })),
        )
        .await;
    match (&dat, &err) {
        (None, Some(e)) if e.contains("未装配") => {
            results.push(pass(
                &format!("{suite}/invoke_unassembled_honest"),
                "invoke 未装配诚实报",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/invoke_unassembled_honest"),
            format!("应报未装配，实得 dat={dat:?} err={err:?}"),
        )),
    }

    // ── 恢复原状（删 ziwei 键）──
    if let Err(e) = set_gate(ws, false) {
        results.push(fail(
            &format!("{suite}/restore"),
            format!("config 恢复失败: {e}"),
        ));
    } else {
        results.push(pass(&format!("{suite}/restore"), "config.json 已恢复"));
    }

    results
}
