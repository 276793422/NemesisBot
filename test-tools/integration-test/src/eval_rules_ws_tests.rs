//! 条目级规则管理 WSAPI 表面契约（2026-11-08 配置页补全批次 3 接真 IT）。
//!
//! 两个套件钉**运行相 gateway 真链路**：security.approvals.add/remove
//! （nemesis-security upsert/remove_manual_rule 同源闸：死条目 loud 拒）
//! 与 security.eval_rules.*（数据半边 nemesis-config::eval_rules 单一真相
//! 源，语义镜像 CLI `eval rules *`），写操作落盘 = 与 CLI 同一文件的盘面
//! 真相（approval_rules.json 数组形态 / eval_rules.json {version,rules}）。
//!
//! 结构级校验细节（未知 source/坏 regex/缺 rule 字段）由 nemesis-web 单测
//! `eval_rules_add_rejects_invalid_structure` 钉住，此处不重复。

use serde_json::json;
use std::path::Path;
use test_harness::*;

use crate::ui_batch_series::WsApi;

fn approval_rules_json_path(ws: &TestWorkspace) -> std::path::PathBuf {
    ws.workspace().join("config").join("approval_rules.json")
}

fn eval_rules_json_path(ws: &TestWorkspace) -> std::path::PathBuf {
    ws.workspace().join("config").join("eval_rules.json")
}

const IT_PATTERN: &str = "it-rules-cargo-check *";

pub async fn test_rules_mgmt_ws_surface(ws: &TestWorkspace, _bin: &Path) -> Vec<TestResult> {
    let suite = "rules_ws/surface";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // ═══ approvals.add / remove（审批记忆规则条目级管理）═══

    // add happy：trim 落盘 + list 回读
    let (dat, err) = api
        .call(
            "security",
            "approvals.add",
            Some(json!({ "op": "process_exec", "pattern": format!("  {IT_PATTERN}  ") })),
        )
        .await;
    match &dat {
        Some(v) if v["added"] == json!(true) => {
            results.push(pass(
                &format!("{suite}/approvals_add"),
                "approvals.add added=true（pattern trim）",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/approvals_add"),
            format!("approvals.add 应成功，dat={dat:?} err={err:?}"),
        )),
    }

    let (dat, err) = api.call("security", "approvals.list", None).await;
    let in_list = dat
        .as_ref()
        .and_then(|v| v["rules"].as_array())
        .map(|a| a.iter().any(|r| r["pattern"].as_str() == Some(IT_PATTERN)))
        .unwrap_or(false);
    if in_list {
        results.push(pass(
            &format!("{suite}/approvals_list_visible"),
            "approvals.list 含新规则（trim 后 pattern）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/approvals_list_visible"),
            format!("approvals.list 应含 {IT_PATTERN}，dat={dat:?} err={err:?}"),
        ));
    }

    // 落盘真相：approval_rules.json（数组形态，与 CLI 同源写穿）
    let pj = approval_rules_json_path(ws);
    let on_disk = std::fs::read_to_string(&pj)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let persisted = on_disk
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().any(|r| r["pattern"].as_str() == Some(IT_PATTERN)))
        .unwrap_or(false);
    if persisted {
        results.push(pass(
            &format!("{suite}/approvals_persisted"),
            "WSAPI add 落盘 approval_rules.json（数组形态，与 CLI 同源）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/approvals_persisted"),
            format!("approval_rules.json 应含 {IT_PATTERN}，实得 {:?}", on_disk),
        ));
    }

    // 闸：安装类永不入规则 + 静态 CRITICAL 非 exec 拒绝（死条目 loud 拒）
    for (op, needle) in [
        ("plugins.install", "never enters the rule table"),
        ("system_shutdown", "always stay manual"),
    ] {
        let (dat, err) = api
            .call(
                "security",
                "approvals.add",
                Some(json!({ "op": op, "pattern": "whatever" })),
            )
            .await;
        let rejected = dat.is_none() && err.as_ref().map(|e| e.contains(needle)).unwrap_or(false);
        if rejected {
            results.push(pass(
                &format!("{suite}/approvals_gate_{op}"),
                format!("install/critical 非 exec 死条目 loud 拒绝（{op}）"),
            ));
        } else {
            results.push(fail(
                &format!("{suite}/approvals_gate_{op}"),
                format!("approvals.add({op}) 应拒绝，dat={dat:?} err={err:?}"),
            ));
        }
    }

    // remove：计数诚实（1 → 再删 0）
    let (dat, err) = api
        .call(
            "security",
            "approvals.remove",
            Some(json!({ "op": "process_exec", "pattern": IT_PATTERN })),
        )
        .await;
    match &dat {
        Some(v) if v["removed"] == json!(1) => {
            results.push(pass(
                &format!("{suite}/approvals_remove"),
                "approvals.remove removed=1",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/approvals_remove"),
            format!("approvals.remove 应 removed=1，dat={dat:?} err={err:?}"),
        )),
    }
    let (dat, _err) = api
        .call(
            "security",
            "approvals.remove",
            Some(json!({ "op": "process_exec", "pattern": IT_PATTERN })),
        )
        .await;
    if dat.as_ref().and_then(|v| v["removed"].as_i64()) == Some(0) {
        results.push(pass(
            &format!("{suite}/approvals_remove_ghost"),
            "重复 remove removed=0（诚实上报不报错）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/approvals_remove_ghost"),
            format!("重复 remove 应 removed=0，dat={dat:?}"),
        ));
    }

    // ═══ eval_rules CRUD（评估规则，数据半边单一真相源）═══

    // list 种子：首访 seed 默认集 + 落盘
    let (dat, err) = api.call("security", "eval_rules.list", None).await;
    let seed = dat
        .as_ref()
        .and_then(|v| v["rules"].as_array().map(|a| a.to_vec()));
    match (&dat, &seed) {
        (Some(v), Some(rules)) if v["version"] == json!(1) && !rules.is_empty() => {
            results.push(pass(
                &format!("{suite}/eval_list_seeds"),
                format!("eval_rules.list 首访种子默认集（{} 条）", rules.len()),
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/eval_list_seeds"),
            format!("eval_rules.list 应种子默认集，dat={dat:?} err={err:?}"),
        )),
    }
    if !eval_rules_json_path(ws).exists() {
        results.push(fail(
            &format!("{suite}/eval_seed_persisted"),
            "种子应落盘 eval_rules.json".to_string(),
        ));
    } else {
        results.push(pass(
            &format!("{suite}/eval_seed_persisted"),
            "种子落盘 eval_rules.json（与 CLI 同一文件）",
        ));
    }
    let seed_len = seed.as_ref().map(|a| a.len()).unwrap_or(0);

    // add 自定义规则
    let rule = json!({
        "id": "it-custom-reg",
        "description": "IT 套件临时规则",
        "level": "high",
        "source": "tool_trace",
        "conditions": [
            { "field": "arguments.command", "op": "contains", "value": "it-rules-marker" }
        ],
        "min_count": 1,
    });
    let (dat, err) = api
        .call("security", "eval_rules.add", Some(json!({ "rule": rule })))
        .await;
    match &dat {
        Some(v) if v["added"] == json!(true) => {
            results.push(pass(
                &format!("{suite}/eval_add"),
                "eval_rules.add added=true",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/eval_add"),
            format!("eval_rules.add 应成功，dat={dat:?} err={err:?}"),
        )),
    }

    // add 同 id 冲突拒绝（镜像 CLI）
    let (dat, err) = api
        .call("security", "eval_rules.add", Some(json!({ "rule": rule })))
        .await;
    let conflict = dat.is_none()
        && err
            .as_ref()
            .map(|e| e.contains("already exists"))
            .unwrap_or(false);
    if conflict {
        results.push(pass(
            &format!("{suite}/eval_add_conflict"),
            "重复 add 同 id 拒绝（already exists）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/eval_add_conflict"),
            format!("重复 add 应拒绝，dat={dat:?} err={err:?}"),
        ));
    }

    // update 现有条目 level → critical
    let mut edited = rule.clone();
    edited["level"] = json!("critical");
    let (dat, err) = api
        .call(
            "security",
            "eval_rules.update",
            Some(json!({ "rule": edited })),
        )
        .await;
    match &dat {
        Some(v) if v["updated"] == json!(true) => {
            results.push(pass(
                &format!("{suite}/eval_update"),
                "eval_rules.update updated=true",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/eval_update"),
            format!("eval_rules.update 应成功，dat={dat:?} err={err:?}"),
        )),
    }
    // update ghost 报错
    let mut ghost = rule.clone();
    ghost["id"] = json!("it-ghost");
    let (dat, err) = api
        .call(
            "security",
            "eval_rules.update",
            Some(json!({ "rule": ghost })),
        )
        .await;
    let ghost_err = dat.is_none()
        && err
            .as_ref()
            .map(|e| e.contains("not found"))
            .unwrap_or(false);
    if ghost_err {
        results.push(pass(
            &format!("{suite}/eval_update_ghost"),
            "update 不存在 id 报错（not found）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/eval_update_ghost"),
            format!("update ghost 应报错，dat={dat:?} err={err:?}"),
        ));
    }

    // list 验证替换生效 + set_enabled 关
    let (dat, _err) = api.call("security", "eval_rules.list", None).await;
    let found_critical = dat
        .as_ref()
        .and_then(|v| v["rules"].as_array())
        .map(|a| {
            a.iter()
                .any(|r| r["id"] == json!("it-custom-reg") && r["level"] == json!("critical"))
        })
        .unwrap_or(false);
    if found_critical {
        results.push(pass(
            &format!("{suite}/eval_update_visible"),
            "list 里 it-custom-reg level=critical（替换生效）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/eval_update_visible"),
            format!("list 应含 critical 版 it-custom-reg，dat={dat:?}"),
        ));
    }
    let (dat, err) = api
        .call(
            "security",
            "eval_rules.set_enabled",
            Some(json!({ "id": "it-custom-reg", "enabled": false })),
        )
        .await;
    match &dat {
        Some(v) if v["enabled"] == json!(false) => {
            results.push(pass(
                &format!("{suite}/eval_set_enabled"),
                "set_enabled false 生效",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/eval_set_enabled"),
            format!("eval_rules.set_enabled 应成功，dat={dat:?} err={err:?}"),
        )),
    }

    // reset：replaced = 种子+自定义，count = 种子数，自定义消失
    let (dat, err) = api.call("security", "eval_rules.reset", None).await;
    let reset_ok = dat
        .as_ref()
        .map(|v| {
            v["reset"] == json!(true)
                && v["replaced"].as_u64() == Some((seed_len + 1) as u64)
                && v["count"].as_u64() == Some(seed_len as u64)
        })
        .unwrap_or(false);
    if reset_ok {
        results.push(pass(
            &format!("{suite}/eval_reset"),
            format!(
                "eval_rules.reset replaced={} count={}",
                seed_len + 1,
                seed_len
            ),
        ));
    } else {
        results.push(fail(
            &format!("{suite}/eval_reset"),
            format!(
                "eval_rules.reset 应 replaced={} count={}，dat={dat:?} err={err:?}",
                seed_len + 1,
                seed_len
            ),
        ));
    }
    let (dat, _err) = api.call("security", "eval_rules.list", None).await;
    let custom_gone = dat
        .as_ref()
        .and_then(|v| v["rules"].as_array())
        .map(|a| !a.iter().any(|r| r["id"] == json!("it-custom-reg")))
        .unwrap_or(false);
    if custom_gone {
        results.push(pass(
            &format!("{suite}/eval_reset_visible"),
            "reset 后自定义规则消失（恢复默认集）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/eval_reset_visible"),
            format!("reset 后 list 应不含 it-custom-reg，dat={dat:?}"),
        ));
    }

    // remove ghost 报错（套件尾：it-custom-reg 已被 reset 冲掉）
    let (dat, err) = api
        .call(
            "security",
            "eval_rules.remove",
            Some(json!({ "id": "it-custom-reg" })),
        )
        .await;
    let gone_err = dat.is_none()
        && err
            .as_ref()
            .map(|e| e.contains("not found"))
            .unwrap_or(false);
    if gone_err {
        results.push(pass(
            &format!("{suite}/eval_remove_ghost"),
            "remove 不存在 id 报错（not found）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/eval_remove_ghost"),
            format!("remove ghost 应报错，dat={dat:?} err={err:?}"),
        ));
    }

    results
}
