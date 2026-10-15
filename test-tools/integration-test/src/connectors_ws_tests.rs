//! Connectors WSAPI 数据面契约套件（AI 原生行动代理 P0，2026-10-14）。
//!
//! 单节点范围：PLAN §6.2 冻结契约三命令（connectors.list /
//! connectors.receipts.list / paperbroker.ledger）走真 gateway WS 链路。
//! 断言面与 handlers/connectors/tests.rs 同一投影（单测覆盖 handler 逻辑，
//! 本套件覆盖「真 gateway 装配 + WS 路由 + 实时扫描」整链）：
//! - 注册表实时扫描（运行中 gateway 放置 connector.toml 立即可见，无重启）；
//! - 损坏条目诚实投影（id=目录名、enabled=false、error 非空）；
//! - 回执新→旧排序 + limit 截断 + connector 过滤；
//! - 账本缺文件 = 默认纸面资金 1_000_000（不创建文件）；
//! - 未知命令诚实报错；
//! - E3 undo 全链（live loop）：域 C 反向 op 经真 loop 瀑布执行成功 +
//!   撤销动作留痕新凭条 + read 类诚实说明 + 非 executed 拒撤。
//!
//! 运行位置：main.rs 排尾（vault 套件之后、cleanup 之前）——写工作区
//! `connectors/`（demo fixture，套件结束删除）+ `data/receipts/` +
//! `data/paper_broker.json`，套件结束删除恢复原状。

use serde_json::json;
use std::path::Path;
use test_harness::*;

use crate::ui_batch_series::WsApi;

/// 合法 connector fixture（demo-quotes：行情 GET low read + 下单 POST high
/// write confirm required budget finance）。
const QUOTES_TOML: &str = r#"
[connector]
id = "demo-quotes"
name = "Demo 行情连接器"
version = "1.0.0"
kind = "rest"
description = "IT 表面契约 fixture"

[connector.http]
base = "http://127.0.0.1:1"

[[op]]
id = "quote"
intent = ["查询行情", "查股价"]
risk = "low"
class = "read"

[op.http]
method = "GET"
path = "/quote/{symbol}"

[[op]]
id = "buy"
intent = ["纸面买入"]
risk = "high"
class = "write"
confirm = "required"
budget = "finance"

[op.http]
method = "POST"
path = "/order"
"#;

/// undo 全链 fixture（undo-broker：place 声明 undo→void）。机制测试用
/// `builtin.paper_positions` 当反向动作替身——v1 paper_cancel 恒 Err（即时
/// 成交无挂单，demo 幕三契约），成功路径需要一个能真实执行成功的动作；
/// class 仍声明 write 走域 C 闸，语义见套件内注释。
const UNDO_TOML: &str = r#"
[connector]
id = "undo-broker"
name = "Undo 机制 fixture"
version = "1.0.0"
kind = "rest"
description = "IT undo 全链 fixture"

[connector.http]
base = "http://127.0.0.1:1"

[[op]]
id = "place"
intent = ["纸面下单"]
risk = "medium"
class = "write"
receipt = "order_id"
undo = { op = "void", arg = "order_id" }
action = "builtin.paper_order"

[[op]]
id = "void"
intent = ["撤销留痕"]
risk = "medium"
class = "write"
action = "builtin.paper_positions"
"#;

/// 手写一条回执 jsonl 行（绕开执行面，只验数据面读路径）。
fn receipt_line(receipt_id: &str, connector: &str, op: &str, outcome: &str) -> String {
    json!({
        "receipt_id": receipt_id,
        "ts": "2026-10-14T08:00:00Z",
        "connector": connector,
        "op": op,
        "risk": "low",
        "class": "read",
        "outcome": outcome,
        "args_digest": "abcdef0123456789",
        "summary": format!("IT fixture {receipt_id}"),
        "receipt_key": "order_id",
        "receipt_value": "NB-ORD-1",
    })
    .to_string()
}

pub async fn test_connectors_data_plane(ws: &TestWorkspace, _bin: &Path) -> Vec<TestResult> {
    let suite = "connectors_ws/data_plane";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // ── 0. 前置清理：确保注册表/回执/账本从空态开始（幂等重跑）。
    let ws_dir = ws.workspace();
    let _ = std::fs::remove_dir_all(ws_dir.join("connectors"));
    let _ = std::fs::remove_dir_all(ws_dir.join("data").join("receipts"));
    let _ = std::fs::remove_file(ws_dir.join("data").join("paper_broker.json"));

    // 1. 空注册表：connectors=[]。
    let (dat, err) = api.call("connectors", "list", None).await;
    match &dat {
        Some(d)
            if err.is_none()
                && d["connectors"]
                    .as_array()
                    .map(|a| a.is_empty())
                    .unwrap_or(false) =>
        {
            results.push(pass(
                &format!("{suite}/list_empty"),
                "空注册表 connectors=[]",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/list_empty"),
            format!("意外: {dat:?} {err:?}"),
        )),
    }

    // 2. 运行中 gateway 放置合法 connector.toml → list 实时反映（无重启）。
    let conn_dir = ws_dir.join("connectors").join("demo-quotes");
    if std::fs::create_dir_all(&conn_dir).is_err()
        || std::fs::write(conn_dir.join("connector.toml"), QUOTES_TOML).is_err()
    {
        results.push(fail(&format!("{suite}/place_fixture"), "写 fixture 失败"));
    } else {
        let (dat, err) = api.call("connectors", "list", None).await;
        match &dat {
            Some(d) if err.is_none() => {
                let arr = d["connectors"].as_array().cloned().unwrap_or_default();
                let ok = arr.len() == 1
                    && arr[0]["id"] == "demo-quotes"
                    && arr[0]["kind"] == "rest"
                    && arr[0]["enabled"] == true
                    && arr[0]["error"].is_null()
                    && arr[0]["ops"]
                        .as_array()
                        .map(|o| o.len() == 2)
                        .unwrap_or(false)
                    && arr[0]["ops"][0]["id"] == "quote"
                    && arr[0]["ops"][0]["binding"]["method"] == "GET"
                    && arr[0]["ops"][1]["confirm_required"] == true
                    && arr[0]["ops"][1]["finance_budget"] == true;
                if ok {
                    results.push(pass(
                        &format!("{suite}/list_live_scan"),
                        "运行中放置即时可见（id/kind/ops/confirm/budget 投影）",
                    ));
                } else {
                    results.push(fail(
                        &format!("{suite}/list_live_scan"),
                        format!("投影不符: {arr:?}"),
                    ));
                }
            }
            _ => results.push(fail(&format!("{suite}/list_live_scan"), format!("{err:?}"))),
        }
    }

    // 3. 损坏 connector.toml → 条目 id=目录名、enabled=false、error 非空。
    let bad_dir = ws_dir.join("connectors").join("bad-conn");
    let _ = std::fs::create_dir_all(&bad_dir);
    let _ = std::fs::write(bad_dir.join("connector.toml"), "not [ valid toml {{{{");
    let (dat, err) = api.call("connectors", "list", None).await;
    match &dat {
        Some(d) if err.is_none() => {
            let arr = d["connectors"].as_array().cloned().unwrap_or_default();
            let bad = arr.iter().find(|c| c["id"] == "bad-conn").cloned();
            match bad {
                Some(b)
                    if b["enabled"] == false
                        && b["error"].as_str().map(|s| !s.is_empty()).unwrap_or(false) =>
                {
                    results.push(pass(
                        &format!("{suite}/list_corrupt_entry"),
                        "损坏条目诚实投影（id=目录名 + error 原文）",
                    ));
                }
                Some(b) => results.push(fail(
                    &format!("{suite}/list_corrupt_entry"),
                    format!("损坏条目形态不符: {b}"),
                )),
                None => results.push(fail(
                    &format!("{suite}/list_corrupt_entry"),
                    format!("bad-conn 不在列表: {arr:?}"),
                )),
            }
        }
        _ => results.push(fail(
            &format!("{suite}/list_corrupt_entry"),
            format!("{err:?}"),
        )),
    }

    // 4. 回执空态：receipts=[]。
    let (dat, err) = api
        .call("connectors", "receipts.list", Some(json!({ "limit": 20 })))
        .await;
    match &dat {
        Some(d)
            if err.is_none()
                && d["receipts"]
                    .as_array()
                    .map(|a| a.is_empty())
                    .unwrap_or(false) =>
        {
            results.push(pass(
                &format!("{suite}/receipts_empty"),
                "无回执 receipts=[]",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/receipts_empty"),
            format!("意外: {dat:?} {err:?}"),
        )),
    }

    // 5. 手写两条回执（旧→新）→ list 新→旧；limit=1 只取最新；connector 过滤。
    let today = chrono::Utc::now().format("%Y%m%d");
    let receipts_dir = ws_dir.join("data").join("receipts");
    let _ = std::fs::create_dir_all(&receipts_dir);
    let jsonl = receipts_dir.join(format!("receipts-{today}.jsonl"));
    let two_lines = format!(
        "{}\n{}\n",
        receipt_line("RCPT-1000-1", "demo-quotes", "quote", "executed"),
        receipt_line("RCPT-3000-2", "demo-quotes", "buy", "budget_blocked"),
    );
    if std::fs::write(&jsonl, two_lines).is_err() {
        results.push(fail(
            &format!("{suite}/write_receipts"),
            "写回执 jsonl 失败",
        ));
    } else {
        // 5a. 全量：新→旧。
        let (dat, err) = api
            .call("connectors", "receipts.list", Some(json!({ "limit": 20 })))
            .await;
        match &dat {
            Some(d) if err.is_none() => {
                let ids: Vec<String> = d["receipts"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|r| r["receipt_id"].as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                if ids == vec!["RCPT-3000-2".to_string(), "RCPT-1000-1".to_string()] {
                    results.push(pass(&format!("{suite}/receipts_order"), "新→旧排序"));
                } else {
                    results.push(fail(
                        &format!("{suite}/receipts_order"),
                        format!("ids={ids:?}"),
                    ));
                }
            }
            _ => results.push(fail(&format!("{suite}/receipts_order"), format!("{err:?}"))),
        }
        // 5b. limit=1 截断到最新。
        let (dat, err) = api
            .call("connectors", "receipts.list", Some(json!({ "limit": 1 })))
            .await;
        match &dat {
            Some(d) if err.is_none() => {
                let ids: Vec<String> = d["receipts"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|r| r["receipt_id"].as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                if ids == vec!["RCPT-3000-2".to_string()] {
                    results.push(pass(&format!("{suite}/receipts_limit"), "limit=1 取最新"));
                } else {
                    results.push(fail(
                        &format!("{suite}/receipts_limit"),
                        format!("ids={ids:?}"),
                    ));
                }
            }
            _ => results.push(fail(&format!("{suite}/receipts_limit"), format!("{err:?}"))),
        }
        // 5c. connector 过滤：无该 connector 回执 → 空。
        let (dat, err) = api
            .call(
                "connectors",
                "receipts.list",
                Some(json!({ "limit": 20, "connector": "no-such-conn" })),
            )
            .await;
        match &dat {
            Some(d)
                if err.is_none()
                    && d["receipts"]
                        .as_array()
                        .map(|a| a.is_empty())
                        .unwrap_or(false) =>
            {
                results.push(pass(
                    &format!("{suite}/receipts_filter"),
                    "connector 过滤生效",
                ));
            }
            _ => results.push(fail(
                &format!("{suite}/receipts_filter"),
                format!("意外: {dat:?} {err:?}"),
            )),
        }
    }

    // 6. 账本缺文件 = 默认纸面资金 1_000_000（读路径不创建文件）。
    let (dat, err) = api.call("paperbroker", "ledger", None).await;
    match &dat {
        Some(d) if err.is_none() => {
            let no_file = !ws_dir.join("data").join("paper_broker.json").exists();
            let empty = d["positions"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(false)
                && d["orders"]
                    .as_array()
                    .map(|a| a.is_empty())
                    .unwrap_or(false);
            if d["cash"] == 1_000_000.0 && no_file && empty {
                results.push(pass(
                    &format!("{suite}/ledger_default"),
                    "缺文件默认 cash=1_000_000 且不落盘",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/ledger_default"),
                    format!("cash={:?} no_file={no_file} empty={empty}", d["cash"]),
                ));
            }
        }
        _ => results.push(fail(&format!("{suite}/ledger_default"), format!("{err:?}"))),
    }

    // 7. 账本有状态 → 投影反映（买入后 cash 减少 + 持仓 + 订单）。
    let ledger_json = json!({
        "cash": 850_000.0,
        "positions": { "600519": { "symbol": "600519", "qty": 100.0, "avg_cost": 1500.0 } },
        "orders": [ {
            "order_id": "NB-ORD-1",
            "ts": "2026-10-14T08:00:00Z",
            "side": "buy",
            "symbol": "600519",
            "qty": 100.0,
            "price": 1500.0,
            "status": "filled"
        } ],
    });
    let _ = std::fs::create_dir_all(ws_dir.join("data"));
    let ledger_path = ws_dir.join("data").join("paper_broker.json");
    if std::fs::write(
        &ledger_path,
        serde_json::to_string_pretty(&ledger_json).unwrap_or_default(),
    )
    .is_err()
    {
        results.push(fail(&format!("{suite}/write_ledger"), "写账本失败"));
    } else {
        let (dat, err) = api.call("paperbroker", "ledger", None).await;
        match &dat {
            Some(d) if err.is_none() => {
                let pos = d["positions"].as_array().cloned().unwrap_or_default();
                let orders = d["orders"].as_array().cloned().unwrap_or_default();
                let ok = d["cash"] == 850_000.0
                    && pos.len() == 1
                    && pos[0]["symbol"] == "600519"
                    && pos[0]["qty"] == 100.0
                    && orders.len() == 1
                    && orders[0]["side"] == "buy"
                    && orders[0]["status"] == "filled";
                if ok {
                    results.push(pass(
                        &format!("{suite}/ledger_state"),
                        "账本状态投影（cash/持仓/订单）",
                    ));
                } else {
                    results.push(fail(
                        &format!("{suite}/ledger_state"),
                        format!("投影不符: {d}"),
                    ));
                }
            }
            _ => results.push(fail(&format!("{suite}/ledger_state"), format!("{err:?}"))),
        }
    }

    // 8. 未知命令诚实报错。
    let (_, err) = api.call("connectors", "no_such_command", None).await;
    match err {
        Some(e) if !e.is_empty() => {
            results.push(pass(&format!("{suite}/unknown_command"), "未知命令报错"));
        }
        _ => results.push(fail(&format!("{suite}/unknown_command"), "未知命令未报错")),
    }

    // 9. E3 undo 全链（live loop）：域 C 反向 op 经真 gateway loop 的
    //    handle_tool_call 完整瀑布执行成功 + 撤销动作自身落新凭条（留痕）。
    //    反向动作用 paper_positions 替身（可真实执行成功；v1 paper_cancel
    //    恒 Err 的诚实边界归 demo 幕三契约）。
    let undo_dir = ws_dir.join("connectors").join("undo-broker");
    let mut undo_ok = std::fs::create_dir_all(&undo_dir).is_ok()
        && std::fs::write(undo_dir.join("connector.toml"), UNDO_TOML).is_ok();
    let undo_receipt = json!({
        "receipt_id": "RCPT-UNDO-1",
        "ts": "2026-10-14T08:00:00Z",
        "connector": "undo-broker",
        "op": "place",
        "risk": "medium",
        "class": "write",
        "outcome": "executed",
        "args_digest": "abcdef0123456789",
        "summary": "IT undo fixture 纸面下单",
        "receipt_key": "order_id",
        "receipt_value": "NB-ORD-1",
        "undoable": true,
    })
    .to_string();
    undo_ok = undo_ok
        && std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&jsonl)
            .and_then(|mut f| {
                std::io::Write::write_all(&mut f, format!("{undo_receipt}\n").as_bytes())
            })
            .is_ok();
    if !undo_ok {
        results.push(fail(
            &format!("{suite}/undo_setup"),
            "写 undo fixture/凭条失败",
        ));
    } else {
        // 9a. 域 C：反向 op 执行成功（凭条主键值回填 order_id，真 loop 瀑布）。
        let (dat, err) = api
            .call(
                "connectors",
                "undo",
                Some(json!({ "receipt_id": "RCPT-UNDO-1" })),
            )
            .await;
        match &dat {
            Some(d) if err.is_none() => {
                let rt = d["result_text"].as_str().unwrap_or_default();
                let ok = d["undone"] == true
                    && d["via"] == "reverse_op"
                    && d["undo_op"] == "void"
                    && rt.contains("850000");
                if ok {
                    results.push(pass(
                        &format!("{suite}/undo_reverse_op"),
                        "域 C 撤销：反向 op 经真 loop 执行成功（undone=true/via=reverse_op）",
                    ));
                } else {
                    results.push(fail(
                        &format!("{suite}/undo_reverse_op"),
                        format!("投影不符: {d}"),
                    ));
                }
            }
            _ => results.push(fail(
                &format!("{suite}/undo_reverse_op"),
                format!("{err:?}"),
            )),
        }
        // 9b. 撤销动作自身留痕：void 凭条落账（executed）。
        let (dat, err) = api
            .call(
                "connectors",
                "receipts.list",
                Some(json!({ "limit": 50, "connector": "undo-broker" })),
            )
            .await;
        match &dat {
            Some(d) if err.is_none() => {
                let arr = d["receipts"].as_array().cloned().unwrap_or_default();
                let voided = arr.iter().any(|r| {
                    r["op"] == "void"
                        && r["outcome"] == "executed"
                        && r["args_digest"]
                            .as_str()
                            .map(|s| !s.is_empty())
                            .unwrap_or(false)
                });
                if voided {
                    results.push(pass(
                        &format!("{suite}/undo_receipt_trail"),
                        "撤销动作留痕：反向 op 凭条落账（executed + args_digest）",
                    ));
                } else {
                    results.push(fail(
                        &format!("{suite}/undo_receipt_trail"),
                        format!("void 凭条未落账: {arr:?}"),
                    ));
                }
            }
            _ => results.push(fail(
                &format!("{suite}/undo_receipt_trail"),
                format!("{err:?}"),
            )),
        }
    }

    // 10. read 类凭条撤销 = 诚实说明（undone=false，不算错）。
    let (dat, err) = api
        .call(
            "connectors",
            "undo",
            Some(json!({ "receipt_id": "RCPT-1000-1" })),
        )
        .await;
    match &dat {
        Some(d) if err.is_none() && d["undone"] == false && d["via"] == "none" => {
            results.push(pass(
                &format!("{suite}/undo_read_class"),
                "read 类凭条诚实说明（undone=false/via=none）",
            ));
        }
        _ => results.push(fail(
            &format!("{suite}/undo_read_class"),
            format!("意外: {dat:?} {err:?}"),
        )),
    }

    // 11. 非 executed 凭条拒撤（budget_blocked 未真实生效）。
    let (_, err) = api
        .call(
            "connectors",
            "undo",
            Some(json!({ "receipt_id": "RCPT-3000-2" })),
        )
        .await;
    match err {
        Some(e) if e.contains("未真实生效") => {
            results.push(pass(
                &format!("{suite}/undo_non_executed"),
                "非 executed 凭条拒撤（诚实报错）",
            ));
        }
        other => results.push(fail(
            &format!("{suite}/undo_non_executed"),
            format!("意外: {other:?}"),
        )),
    }

    // ── 清理：恢复工作区原状（fixture/回执/账本全是本套件写入）。
    let _ = std::fs::remove_dir_all(ws_dir.join("connectors"));
    let _ = std::fs::remove_dir_all(ws_dir.join("data").join("receipts"));
    let _ = std::fs::remove_file(ws_dir.join("data").join("paper_broker.json"));

    results
}
