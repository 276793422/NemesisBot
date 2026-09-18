//! Board anchor 拆解锚点行落库套件（全自动流转 P2 B1 IT）。
//!
//! 单节点范围：TestAIServer 组合桩（testai-board-1.0，按 system prompt
//! 特征词分流 planner/review/讨论）按 `<PLAN_ANCHOR>` 标记产出带 [CHECK]
//! 锚点行的子任务 AC → 两段式 plan（auto_confirm 自动确认）→ 子单 AC
//! 落库含锚点行断言。跨机全链（派发 → B 端 delivery 锚点实核 → FAIL
//! 短路重派 → 预算耗尽转人工）由 cluster-uat T30 双节点覆盖——本套件的
//! 网关 `cluster.enabled=false`，dispatch 无候选节点，验收链无法在此驱动。
//!
//! 运行位置：main.rs 末位（projects series 之后）——本套件热切换默认模型
//! （models.set_default）+ 改 board config + 写 board.db，沿「改共享状态
//! 套件排尾」纪律；结束时恢复 testai-1.1 默认与关闭的开关。

use serde_json::{Value, json};
use std::path::Path;
use test_harness::*;

use crate::ui_batch_series::WsApi;

pub async fn test_board_anchor_plan_chain(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "board_ws/anchor_plan_chain";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // 0. 组合模型入列（CLI 写 model_name 别名 = testai-board-1.0）+
    //    WSAPI 热切换默认（运行时 provider swap，无需重启）。
    let add = ws
        .run_cli(
            bin,
            &[
                "model",
                "add",
                "--model",
                "test/testai-board-1.0",
                "--base",
                &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                "--key",
                "test-key",
            ],
        )
        .await;
    if add.success() {
        results.push(pass(
            &format!("{suite}/model_add"),
            format!("exit={}", add.exit_code),
        ));
    } else {
        results.push(fail(
            &format!("{suite}/model_add"),
            format!("exit={} stderr={}", add.exit_code, snip(&add.stderr)),
        ));
        return results;
    }
    let (dat, err) = api
        .call(
            "models",
            "set_default",
            Some(json!({ "name": "testai-board-1.0" })),
        )
        .await;
    if err.is_none() {
        results.push(pass(&format!("{suite}/set_default"), "runtime swap ok"));
    } else {
        results.push(fail(
            &format!("{suite}/set_default"),
            format!("set_default failed: {err:?} / {dat:?}"),
        ));
        return results;
    }

    // 1. auto_confirm 开（一段 issue.plan 即自动确认发车）。
    let (_, err) = api
        .call(
            "board",
            "config.set",
            Some(json!({ "key": "plan.auto_confirm", "value": true })),
        )
        .await;
    if err.is_none() {
        results.push(pass(&format!("{suite}/auto_confirm"), "flag set"));
    } else {
        results.push(fail(
            &format!("{suite}/auto_confirm"),
            format!("config.set failed: {err:?}"),
        ));
        return results;
    }

    // 2. 建父单（标题带 planner 标记）+ 一段拆解。
    let (dat, err) = api
        .call(
            "board",
            "issue.create",
            Some(json!({
                "title": "IT锚点拆解 <PLAN_ANCHOR> 集成断言",
                "description": "integration-test board anchor 套件：拆解锚点行落库。",
                "acceptance_criteria": "全部子任务完成。",
                "priority": 2,
            })),
        )
        .await;
    let Some(pid) = dat
        .as_ref()
        .and_then(|d| d.pointer("/issue/id"))
        .and_then(|v| v.as_i64())
    else {
        results.push(fail(
            &format!("{suite}/create"),
            format!("issue.create failed: {err:?} / {dat:?}"),
        ));
        return results;
    };
    results.push(pass(&format!("{suite}/create"), format!("id={pid}")));

    let (_, err) = api
        .call("board", "issue.plan", Some(json!({ "id": pid })))
        .await;
    if err.is_none() {
        results.push(pass(&format!("{suite}/plan"), "planning accepted"));
    } else {
        results.push(fail(
            &format!("{suite}/plan"),
            format!("issue.plan failed: {err:?}"),
        ));
        return results;
    }

    // 3. 轮询 ≤90s 等 auto_confirm 发车建 3 子单。
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    let mut last_err: Option<String>;
    let subs: Vec<Value> = loop {
        // issue.list handler 要求 data 存在（`data.ok_or("missing data")`），
        // 必须传空对象；错误不能吞——吞了会永远轮到 0 假超时。
        let (dat, err) = api.call("board", "issue.list", Some(json!({}))).await;
        last_err = err.map(|e| e.to_string());
        let subs: Vec<Value> = dat
            .as_ref()
            .and_then(|d| d.get("issues"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|i| i.get("parent_issue_id").and_then(|v| v.as_i64()) == Some(pid))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if subs.len() == 3 {
            break subs;
        }
        if tokio::time::Instant::now() >= deadline {
            results.push(fail(
                &format!("{suite}/subs_created"),
                format!(
                    "90s 内未发车建 3 子单（实际 {}，最后错误 {last_err:?}）",
                    subs.len()
                ),
            ));
            return results;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    };
    results.push(pass(&format!("{suite}/subs_created"), "3 subs dispatched"));

    // 4. 子单 AC 落库含 [CHECK] 锚点行（planner <PLAN_ANCHOR> 输出原样落库）。
    let mut ids = Vec::new();
    for s in &subs {
        if let Some(id) = s.get("id").and_then(|v| v.as_i64()) {
            ids.push(id);
        }
    }
    ids.sort();
    let mut ac_text = String::new();
    for id in &ids {
        let (dat, err) = api
            .call("board", "issue.get", Some(json!({ "id": id })))
            .await;
        match dat
            .as_ref()
            .and_then(|d| d.pointer("/issue/acceptance_criteria"))
            .and_then(|v| v.as_str())
        {
            Some(ac) => ac_text.push_str(ac),
            None => {
                results.push(fail(
                    &format!("{suite}/sub_get"),
                    format!("sub {id} 无 acceptance_criteria: {err:?}"),
                ));
            }
        }
    }
    let expect = [
        "[CHECK] file:uat-t2/pass/sub1.md exists",
        "[CHECK] file:uat-t2/pass/sub1.md contains:锚点测试",
        "[CHECK] re:集群协作状态正常",
        "[CHECK] file:uat-t2/pass/sub2.md exists",
        "[CHECK] file:uat-t2/pass/sub3.md exists",
    ];
    let mut all_hit = true;
    for e in expect {
        if !ac_text.contains(e) {
            results.push(fail(
                &format!("{suite}/anchor_ac"),
                format!("子单 AC 缺锚点行「{e}」: {}", trunc_str(&ac_text, 300)),
            ));
            all_hit = false;
        }
    }
    if all_hit {
        results.push(pass(
            &format!("{suite}/anchor_ac"),
            "5 条锚点行全部落库（exists/contains/re 三形态）",
        ));
    }

    // 5. 清理：取消子单 + 父单；恢复默认模型与开关（尾位纪律）。
    for id in &ids {
        let _ = api
            .call("board", "issue.cancel", Some(json!({ "id": id })))
            .await;
    }
    let _ = api
        .call("board", "issue.cancel", Some(json!({ "id": pid })))
        .await;
    let _ = api
        .call(
            "board",
            "config.set",
            Some(json!({ "key": "plan.auto_confirm", "value": false })),
        )
        .await;
    let (_, err) = api
        .call(
            "models",
            "set_default",
            Some(json!({ "name": "testai-1.1" })),
        )
        .await;
    if err.is_none() {
        results.push(pass(&format!("{suite}/restore"), "默认模型恢复 testai-1.1"));
    } else {
        results.push(fail(
            &format!("{suite}/restore"),
            format!("默认模型恢复失败（不影响本套件断言，但污染后续套件）: {err:?}"),
        ));
    }
    results
}

/// char-boundary 安全截断（中文 panic 家族纪律）。
fn trunc_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// stderr 首行截断（harness 只有 stdout_first_line）。
fn snip(s: &str) -> String {
    trunc_str(s.lines().next().unwrap_or("").trim(), 120)
}

// ---------------------------------------------------------------------------
// P3 D1: board_issue agent 工具路径（全自动流转 P3 IT）
// ---------------------------------------------------------------------------

/// 「对 master 说一句话建单 → 父单出现 → plan 发车」全链（agent 工具路径）。
///
/// 驱动方式：普通 chat 消息带 `<BOARD_ISSUE>标题</BOARD_ISSUE>` 标记 →
/// TestAIServer 组合桩三步对话机（create 工具调用 → plan 工具调用 →
/// BOARD_ISSUE_FLOW_DONE 收尾）→ 断言权威 board.db 出现父单 + 3 子单
/// （auto_confirm 发车，cluster.enabled=false 下派发诚实降级落系统评论，
/// 子单照建——与 WSAPI issue.plan 单节点语义一致）。
///
/// 与 anchor 套件的差别：anchor 走 WSAPI issue.plan 入口（P2 链路）；
/// 本套件走 **主 agent 工具 dispatch**（P3 D1 新入口，board_issue 工具
/// 注册 + tier 闸 + moderator 槽的运行时证明）。
pub async fn test_board_agent_tool_issue_flow(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "board_ws/agent_tool_issue_flow";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // 0. 组合模型入列（幂等：anchor 套件可能已加）+ 热切换默认。
    let (dat, _err) = api.call("models", "list", None).await;
    let listed = dat
        .as_ref()
        .and_then(|d| d.get("models"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().any(|m| {
                m.get("model_name").and_then(|n| n.as_str()) == Some("testai-board-1.0")
                    || m.get("model").and_then(|n| n.as_str()) == Some("test/testai-board-1.0")
            })
        })
        .unwrap_or(false);
    if listed {
        results.push(pass(&format!("{suite}/model_add"), "already registered"));
    } else {
        let add = ws
            .run_cli(
                bin,
                &[
                    "model",
                    "add",
                    "--model",
                    "test/testai-board-1.0",
                    "--base",
                    &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                    "--key",
                    "test-key",
                ],
            )
            .await;
        if add.success() {
            results.push(pass(
                &format!("{suite}/model_add"),
                format!("exit={}", add.exit_code),
            ));
        } else {
            results.push(fail(
                &format!("{suite}/model_add"),
                format!("exit={} stderr={}", add.exit_code, snip(&add.stderr)),
            ));
            return results;
        }
    }
    let (_, err) = api
        .call(
            "models",
            "set_default",
            Some(json!({ "name": "testai-board-1.0" })),
        )
        .await;
    if err.is_none() {
        results.push(pass(&format!("{suite}/set_default"), "runtime swap ok"));
    } else {
        results.push(fail(
            &format!("{suite}/set_default"),
            format!("set_default failed: {err:?}"),
        ));
        return results;
    }

    // 1. auto_confirm 开（工具里的 plan 链会直接发车建子单）。
    let (_, err) = api
        .call(
            "board",
            "config.set",
            Some(json!({ "key": "plan.auto_confirm", "value": true })),
        )
        .await;
    if err.is_none() {
        results.push(pass(&format!("{suite}/auto_confirm"), "flag set"));
    } else {
        results.push(fail(
            &format!("{suite}/auto_confirm"),
            format!("config.set failed: {err:?}"),
        ));
        return results;
    }

    // 2. 对 master 说一句话（标记驱动）：建单 → 拆解 → 收尾标记。
    const TITLE: &str = "P3IT 标记建单";
    let chat_mark = format!("<BOARD_ISSUE>{TITLE}</BOARD_ISSUE>");
    let mut stream = match ws_connect(WS_PORT, AUTH_TOKEN).await {
        Ok(s) => s,
        Err(e) => {
            results.push(fail(
                &format!("{suite}/chat_connect"),
                format!("ws connect failed: {e}"),
            ));
            return results;
        }
    };
    let reply = match ws_send_and_recv(&mut stream, &chat_mark, 90).await {
        Ok(r) => r,
        Err(e) => {
            results.push(fail(
                &format!("{suite}/chat_round"),
                format!("chat round failed: {e}"),
            ));
            return results;
        }
    };
    if reply.contains("BOARD_ISSUE_FLOW_DONE") {
        results.push(pass(
            &format!("{suite}/chat_round"),
            "收尾标记 BOARD_ISSUE_FLOW_DONE（建单+plan 两工具调用全过）",
        ));
    } else if reply.contains("BOARD_ISSUE_TOOL_UNEXPECTED") {
        results.push(fail(
            &format!("{suite}/chat_round"),
            format!("工具链异常落 UNEXPECTED: {}", trunc_str(&reply, 200)),
        ));
        return results;
    } else {
        results.push(fail(
            &format!("{suite}/chat_round"),
            format!("回复缺收尾标记: {}", trunc_str(&reply, 200)),
        ));
        return results;
    }

    // 3. 权威看板断言：父单出现（标题=标记标题，agent 工具建单）。
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    let parent: Value = loop {
        let (dat, err) = api.call("board", "issue.list", Some(json!({}))).await;
        let hit = dat
            .as_ref()
            .and_then(|d| d.get("issues"))
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|i| {
                        i.get("title").and_then(|t| t.as_str()) == Some(TITLE)
                            && i.get("parent_issue_id").and_then(|v| v.as_i64()).is_none()
                    })
                    .cloned()
            });
        if let Some(p) = hit {
            break p;
        }
        if tokio::time::Instant::now() >= deadline {
            results.push(fail(
                &format!("{suite}/parent_issue"),
                format!("看板未出现父单（title={TITLE}）: {err:?}"),
            ));
            return results;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    };
    let pid = parent.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
    let pnumber = parent
        .get("number")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    results.push(pass(
        &format!("{suite}/parent_issue"),
        format!("{pnumber} (id={pid})"),
    ));

    // 4. plan 发车：3 子单挂到父单（auto_confirm；派发无候选诚实降级）。
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    let mut last_err: Option<String>;
    let subs: Vec<Value> = loop {
        let (dat, err) = api.call("board", "issue.list", Some(json!({}))).await;
        last_err = err.map(|e| e.to_string());
        let subs: Vec<Value> = dat
            .as_ref()
            .and_then(|d| d.get("issues"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|i| i.get("parent_issue_id").and_then(|v| v.as_i64()) == Some(pid))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if subs.len() == 3 {
            break subs;
        }
        if tokio::time::Instant::now() >= deadline {
            results.push(fail(
                &format!("{suite}/subs_created"),
                format!(
                    "90s 内未发车建 3 子单（实际 {}，最后错误 {last_err:?}）",
                    subs.len()
                ),
            ));
            return results;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    };
    results.push(pass(&format!("{suite}/subs_created"), "3 subs dispatched"));

    // 5. 父单有 auto_confirm 系统评论（发车痕迹）。
    let (cdat, cerr) = api
        .call("board", "comment.list", Some(json!({ "issue_id": pid })))
        .await;
    let comments: Vec<Value> = cdat
        .as_ref()
        .and_then(|d| d.get("comments"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let text: String = comments
        .iter()
        .filter_map(|c| c.get("content").and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    if text.contains("auto_confirm") {
        results.push(pass(&format!("{suite}/dispatch_comment"), "系统评论在"));
    } else {
        results.push(fail(
            &format!("{suite}/dispatch_comment"),
            format!(
                "父单系统评论缺 auto_confirm 发车痕迹（{} 条评论）: {} / {cerr:?}",
                comments.len(),
                trunc_str(&text, 160)
            ),
        ));
    }

    // 6. 清理：取消子单 + 父单；恢复默认模型与开关（尾位纪律）。
    for s in &subs {
        if let Some(id) = s.get("id").and_then(|v| v.as_i64()) {
            let _ = api
                .call("board", "issue.cancel", Some(json!({ "id": id })))
                .await;
        }
    }
    let _ = api
        .call("board", "issue.cancel", Some(json!({ "id": pid })))
        .await;
    let (_, err) = api
        .call(
            "board",
            "config.set",
            Some(json!({ "key": "plan.auto_confirm", "value": false })),
        )
        .await;
    if err.is_some() {
        results.push(fail(
            &format!("{suite}/restore"),
            format!("auto_confirm 恢复失败（污染后续）: {err:?}"),
        ));
    }
    let (_, err) = api
        .call(
            "models",
            "set_default",
            Some(json!({ "name": "testai-1.1" })),
        )
        .await;
    if err.is_none() {
        results.push(pass(&format!("{suite}/restore"), "默认模型恢复 testai-1.1"));
    } else {
        results.push(fail(
            &format!("{suite}/restore"),
            format!("默认模型恢复失败（不影响本套件断言，但污染后续套件）: {err:?}"),
        ));
    }
    let _ = stream.close(None).await;
    results
}

/// P4 配置面 IT（全自动流转 E1/F3 单机可达部分）：board.config.set/get 六个
/// 新键（review.max_turns / review.selfcheck / review.auto_close_project +
/// budget 三键）round-trip + 未知键 loud 拒绝 + 默认值恢复。
///
/// 预算冻结/换节点重派等**行为**断言需要跨节点验收链（写回→评审→重派），
/// 单机 IT 网关 cluster.enabled=false 无候选节点无法驱动——归 cluster-uat
/// T34/T35 全保真覆盖；本套件钉的是 E1/F3 的配置面在真实 gateway WSAPI
/// 栈上的契约。
///
/// 运行位置：main.rs 末位（anchor/agent-tool 套件之后）——改 board config，
/// 沿「改共享状态套件排尾」纪律；结束恢复默认值。
pub async fn test_board_p4_config_surface(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "board_ws/p4_config_surface";
    let mut results = Vec::new();
    print_suite_header(suite);
    let _ = (ws, bin); // 接口对齐同文件其他套件（本套件纯 WSAPI）

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // 1. P4 键 + 兜底开关两键（集群完备性加固 2026-09-11）逐一 config.set。
    let cases: &[(&str, serde_json::Value)] = &[
        ("review.max_turns", json!(3)),
        ("review.selfcheck", json!(true)),
        ("review.auto_close_project", json!(true)),
        ("budget.max_subissues_per_parent", json!(10)),
        ("budget.max_total_redispatch", json!(5)),
        ("budget.wall_clock_budget_secs", json!(3600)),
        ("dispatch_fallback", json!(true)),
        ("dispatch_fallback_target", json!("Alex")),
    ];
    for (key, value) in cases {
        let (_, err) = api
            .call(
                "board",
                "config.set",
                Some(json!({ "key": key, "value": value })),
            )
            .await;
        if err.is_none() {
            results.push(pass(&format!("{suite}/set_{key}"), "accepted"));
        } else {
            results.push(fail(
                &format!("{suite}/set_{key}"),
                format!("config.set failed: {err:?}"),
            ));
            return results;
        }
    }

    // 2. config.get 回读断言（round-trip 契约）。
    let (dat, err) = api.call("board", "config.get", None).await;
    match (&dat, &err) {
        (Some(d), None) => {
            let checks = [
                ("/review/max_turns", json!(3)),
                ("/review/selfcheck", json!(true)),
                ("/review/auto_close_project", json!(true)),
                ("/budget/max_subissues_per_parent", json!(10)),
                ("/budget/max_total_redispatch", json!(5)),
                ("/budget/wall_clock_budget_secs", json!(3600)),
                ("/dispatch_fallback", json!(true)),
                ("/dispatch_fallback_target", json!("Alex")),
            ];
            for (pointer, want) in checks {
                if d.pointer(pointer) == Some(&want) {
                    results.push(pass(&format!("{suite}/get{pointer}"), "round-trip ok"));
                } else {
                    results.push(fail(
                        &format!("{suite}/get{pointer}"),
                        format!("期望 {want}，实际 {:?}", d.pointer(pointer)),
                    ));
                }
            }
        }
        _ => {
            results.push(fail(
                &format!("{suite}/config_get"),
                format!("config.get failed: {err:?}"),
            ));
        }
    }

    // 3. 未知键 loud 拒绝（白名单外 → 报错含「允许」清单提示）。
    let (_, err) = api
        .call(
            "board",
            "config.set",
            Some(json!({ "key": "review.bogus", "value": true })),
        )
        .await;
    match &err {
        Some(e) if e.contains("允许") => {
            results.push(pass(&format!("{suite}/reject_unknown"), "loud reject ok"));
        }
        other => {
            results.push(fail(
                &format!("{suite}/reject_unknown"),
                format!("未知键应被 loud 拒绝（错误含「允许」清单），实际 {other:?}"),
            ));
        }
    }

    // 4. 恢复默认值（尾位纪律：不污染后续套件）。
    let defaults: &[(&str, serde_json::Value)] = &[
        ("review.max_turns", json!(1)),
        ("review.selfcheck", json!(false)),
        ("review.auto_close_project", json!(false)),
        ("budget.max_subissues_per_parent", json!(20)),
        ("budget.max_total_redispatch", json!(0)),
        ("budget.wall_clock_budget_secs", json!(0)),
        ("dispatch_fallback", json!(false)),
        ("dispatch_fallback_target", serde_json::Value::Null),
    ];
    let mut restore_ok = true;
    for (key, value) in defaults {
        let (_, err) = api
            .call(
                "board",
                "config.set",
                Some(json!({ "key": key, "value": value })),
            )
            .await;
        if err.is_some() {
            restore_ok = false;
        }
    }
    if restore_ok {
        results.push(pass(&format!("{suite}/restore"), "P4 键恢复默认"));
    } else {
        results.push(fail(
            &format!("{suite}/restore"),
            "P4 键恢复默认失败（污染后续套件）",
        ));
    }

    results
}

/// 全自动流转 P5/E2：审计（决策流）WSAPI 表面套件。
///
/// 单节点边界（同 anchor 套件）：网关 cluster.enabled=false → 自动验收链
/// 不会在此产生真实 `auto_decide` 决策（回滚 happy path 由 cluster-uat
/// T37 双节点覆盖）。本套件钉死端点契约：
/// 1. P5 新 config 键 round-trip（review.checkers / budget.max_tokens_per_parent）；
/// 2. audit.list 全量/按 action 过滤 + id 倒序 + JOIN 单据编号/标题；
/// 3. audit.rollback 诚实拒绝：非 auto_decide 活动 / 不存在的活动。
pub async fn test_board_p5_audit_surface(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "board_ws/p5_audit_surface";
    let mut results = Vec::new();
    print_suite_header(suite);
    let _ = (ws, bin);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // ---- 1. P5 新 config 键 round-trip（与 nemesis-web 单测同一契约的
    //        黑盒面：set → get 断言 → 恢复默认）。
    for (key, value) in [
        ("review.checkers", json!(3)),
        ("budget.max_tokens_per_parent", json!(250_000)),
    ] {
        let (_, err) = api
            .call(
                "board",
                "config.set",
                Some(json!({ "key": key, "value": value })),
            )
            .await;
        if err.is_some() {
            results.push(fail(
                &format!("{suite}/set_{key}"),
                format!("config.set failed: {err:?}"),
            ));
            return results;
        }
    }
    let (dat, err) = api.call("board", "config.get", None).await;
    match &dat {
        Some(d) if err.is_none() => {
            let checks = [
                ("/review/checkers", json!(3)),
                ("/budget/max_tokens_per_parent", json!(250_000)),
            ];
            for (pointer, want) in checks {
                if d.pointer(pointer) == Some(&want) {
                    results.push(pass(&format!("{suite}/get{pointer}"), "round-trip ok"));
                } else {
                    results.push(fail(
                        &format!("{suite}/get{pointer}"),
                        format!("期望 {want}，实际 {:?}", d.pointer(pointer)),
                    ));
                }
            }
        }
        _ => {
            results.push(fail(
                &format!("{suite}/config_get"),
                format!("config.get failed: {err:?}"),
            ));
        }
    }
    for (key, value) in [
        ("review.checkers", json!(1)),
        ("budget.max_tokens_per_parent", json!(0)),
    ] {
        let _ = api
            .call(
                "board",
                "config.set",
                Some(json!({ "key": key, "value": value })),
            )
            .await;
    }
    results.push(pass(&format!("{suite}/restore_p5_keys"), "P5 键恢复默认"));

    // ---- 2. audit.list：建单走三步状态转移 → status_change 活动落库。
    let created = match api
        .call(
            "board",
            "issue.create",
            Some(json!({
                "title": "P5AUDIT 审计表面",
                "description": "integration-test P5/E2：状态转移活动进决策流，回滚诚实拒绝。"
            })),
        )
        .await
    {
        (Some(v), None) => v,
        (_, Some(e)) => {
            results.push(fail(suite, format!("issue.create failed: {e}")));
            return results;
        }
        _ => {
            results.push(fail(suite, "issue.create 异常响应"));
            return results;
        }
    };
    let audit_issue = created
        .pointer("/issue/id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if audit_issue == 0 {
        results.push(fail(suite, format!("issue.create 无 id: {created}")));
        return results;
    }
    for to in ["todo", "in_progress", "in_review"] {
        let (_, err) = api
            .call(
                "board",
                "issue.status",
                Some(json!({ "id": audit_issue, "status": to })),
            )
            .await;
        if err.is_some() {
            results.push(fail(
                &format!("{suite}/status_{to}"),
                format!("issue.status → {to} failed: {err:?}"),
            ));
            return results;
        }
    }

    // 全量列表：最新活动（我们的 in_review 转移）在顶部（id 倒序），行带
    // JOIN 出的单据编号/标题。
    let (list_dat, list_err) = api
        .call("board", "audit.list", Some(json!({ "limit": 50 })))
        .await;
    match &list_dat {
        Some(d) if list_err.is_none() => {
            let rows = d
                .get("decisions")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if rows.is_empty() {
                results.push(fail(&format!("{suite}/list_rows"), "audit.list 空"));
                return results;
            }
            let first = &rows[0];
            let top_is_ours = first.get("issue_id").and_then(|v| v.as_i64()) == Some(audit_issue)
                && first.get("action").and_then(|v| v.as_str()) == Some("status_changed");
            let joined = first
                .get("issue_number")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
                && first.get("issue_title").is_some();
            let id_of = |r: &Value| r.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let desc_order = rows.windows(2).all(|w| id_of(&w[0]) >= id_of(&w[1]));
            if top_is_ours && desc_order {
                results.push(pass(
                    &format!("{suite}/list_top_and_order"),
                    "最新活动置顶 + id 倒序",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/list_top_and_order"),
                    format!("首行={first}, desc={desc_order}"),
                ));
            }
            if joined {
                results.push(pass(&format!("{suite}/list_join"), "JOIN 编号/标题"));
            } else {
                results.push(fail(
                    &format!("{suite}/list_join"),
                    format!("行缺 issue_number/issue_title: {first}"),
                ));
            }
            let our_rows = rows
                .iter()
                .filter(|r| r.get("issue_id").and_then(|v| v.as_i64()) == Some(audit_issue))
                .count();
            if our_rows >= 3 {
                results.push(pass(
                    &format!("{suite}/list_our_rows"),
                    format!("本单 {our_rows} 条 status_change"),
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/list_our_rows"),
                    format!("本单 status_change 应 ≥3（三次转移），实际 {our_rows}"),
                ));
            }
        }
        _ => {
            results.push(fail(
                &format!("{suite}/list_rows"),
                format!("audit.list failed: {list_err:?}"),
            ));
            return results;
        }
    }

    // 按 action 过滤：全部行都是 status_change。
    let (fdat, ferr) = api
        .call(
            "board",
            "audit.list",
            Some(json!({ "limit": 50, "action": "status_changed" })),
        )
        .await;
    match &fdat {
        Some(d) if ferr.is_none() => {
            let rows = d
                .get("decisions")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let all_filtered = rows
                .iter()
                .all(|r| r.get("action").and_then(|v| v.as_str()) == Some("status_changed"));
            if !rows.is_empty() && all_filtered {
                results.push(pass(
                    &format!("{suite}/list_action_filter"),
                    format!("action 过滤 {} 行全命中", rows.len()),
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/list_action_filter"),
                    format!("过滤失效或空（rows={}）", rows.len()),
                ));
            }
        }
        _ => {
            results.push(fail(
                &format!("{suite}/list_action_filter"),
                format!("audit.list(action) failed: {ferr:?}"),
            ));
        }
    }

    // ---- 3. audit.rollback 诚实拒绝：status_change（非 auto_decide）+
    //        不存在的活动 id。
    let our_activity = fdat
        .as_ref()
        .and_then(|d| d.get("decisions"))
        .and_then(|v| v.as_array())
        .and_then(|rows| {
            rows.iter()
                .find(|r| r.get("issue_id").and_then(|v| v.as_i64()) == Some(audit_issue))
        })
        .and_then(|r| r.get("id").and_then(|v| v.as_i64()))
        .unwrap_or(0);
    let (_, err) = api
        .call(
            "board",
            "audit.rollback",
            Some(json!({ "activity_id": our_activity })),
        )
        .await;
    match &err {
        Some(e) if e.contains("不是自动决策记录") => {
            results.push(pass(
                &format!("{suite}/rollback_reject_non_auto"),
                "status_change 活动拒绝回滚",
            ));
        }
        other => {
            results.push(fail(
                &format!("{suite}/rollback_reject_non_auto"),
                format!("非 auto_decide 应拒绝（含「不是自动决策记录」），实际 {other:?}"),
            ));
        }
    }
    let (_, err) = api
        .call(
            "board",
            "audit.rollback",
            Some(json!({ "activity_id": 999_999_999 })),
        )
        .await;
    match &err {
        Some(e) if e.contains("不存在") => {
            results.push(pass(
                &format!("{suite}/rollback_reject_missing"),
                "不存在活动诚实报错",
            ));
        }
        other => {
            results.push(fail(
                &format!("{suite}/rollback_reject_missing"),
                format!("不存在 id 应拒绝（含「不存在」），实际 {other:?}"),
            ));
        }
    }

    results
}

// ---------------------------------------------------------------------------
// goal P1（可观测批）：board.project.progress 聚合投影 + issue.list stage 字段。
// 实机验收：goal §四 T-obs-2/3/4 的后端数据面（前端渲染另有 vitest/人工）。
// ---------------------------------------------------------------------------

pub async fn test_board_p1_progress_surface(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "board_ws/p1_progress_surface";
    let mut results = Vec::new();
    print_suite_header(suite);
    let _ = (ws, bin);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // ---- 1. 建项目 + 三单：done / in_progress（无在途=待重派）/ backlog。
    let proj = match api
        .call(
            "board",
            "project.create",
            Some(json!({ "name": "P1PROG 进度聚合", "description": "IT" })),
        )
        .await
    {
        (Some(d), None) => d["project"]["id"].as_i64().unwrap_or(0),
        _ => {
            results.push(fail(
                &format!("{suite}/create_project"),
                "project.create failed",
            ));
            return results;
        }
    };
    if proj == 0 {
        results.push(fail(&format!("{suite}/create_project"), "project id=0"));
        return results;
    }

    let mut ids = Vec::new();
    for title in ["P1P done 单", "P1P 待重派单", "P1P backlog 单"] {
        let (d, err) = api
            .call(
                "board",
                "issue.create",
                Some(json!({ "title": title, "project_id": proj })),
            )
            .await;
        match (d, err) {
            (Some(d), None) => ids.push(d["issue"]["id"].as_i64().unwrap_or(0)),
            _ => {
                results.push(fail(
                    &format!("{suite}/create {title}"),
                    "issue.create failed",
                ));
                return results;
            }
        }
    }
    // done 单：backlog → done（合法直达边）。
    let (_, err) = api
        .call(
            "board",
            "issue.status",
            Some(json!({ "id": ids[0], "status": "done" })),
        )
        .await;
    if err.is_some() {
        results.push(fail(
            &format!("{suite}/done transition"),
            format!("{err:?}"),
        ));
    }
    // 待重派单：backlog → in_progress（不派发 → 无在途）。
    let (_, err) = api
        .call(
            "board",
            "issue.status",
            Some(json!({ "id": ids[1], "status": "in_progress" })),
        )
        .await;
    if err.is_some() {
        results.push(fail(
            &format!("{suite}/redo transition"),
            format!("{err:?}"),
        ));
    }

    // ---- 2. 全项目摘要模式（无 project_id）：含本项目行且字段齐。
    let (d, err) = api.call("board", "project.progress", None).await;
    match (&d, &err) {
        (Some(d), None) => {
            let rows = d["projects"].as_array().cloned().unwrap_or_default();
            results.push(pass(
                &format!("{suite}/all_mode"),
                format!("projects={}", rows.len()),
            ));
            let mine = rows.iter().find(|r| r["project_id"].as_i64() == Some(proj));
            match mine {
                Some(r) => {
                    if r["stage"] == "待重派"
                        && r["counts"]["done"] == 1
                        && r["counts"]["in_progress"] == 1
                        && r["counts"]["backlog"] == 1
                    {
                        results.push(pass(&format!("{suite}/all_mode_row"), "counts+stage 正确"));
                    } else {
                        results.push(fail(
                            &format!("{suite}/all_mode_row"),
                            format!("unexpected row: {r}"),
                        ));
                    }
                }
                None => results.push(fail(
                    &format!("{suite}/all_mode_row"),
                    format!("project {proj} 不在摘要中"),
                )),
            }
        }
        _ => results.push(fail(
            &format!("{suite}/all_mode"),
            "project.progress (all) failed",
        )),
    }

    // ---- 3. 单项目模式：逐单 stage（done=已完成 / in_progress 无在途=待重派 / backlog=待派发）。
    let (d, err) = api
        .call(
            "board",
            "project.progress",
            Some(json!({ "project_id": proj })),
        )
        .await;
    match (&d, err) {
        (Some(d), None) => {
            let expect = [(ids[0], "已完成"), (ids[1], "待重派"), (ids[2], "待派发")];
            let mut all_ok = true;
            for (id, want) in expect {
                let row = d["issues"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|r| r["id"].as_i64() == Some(id));
                match row {
                    Some(r) if r["stage"] == json!(want) => {}
                    other => {
                        all_ok = false;
                        results.push(fail(
                            &format!("{suite}/stage {id}"),
                            format!("期望 {want}，实际 {other:?}"),
                        ));
                    }
                }
            }
            if all_ok {
                results.push(pass(&format!("{suite}/per_issue_stage"), "逐单环节正确"));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/single_mode"),
            "project.progress failed",
        )),
    }

    // ---- 4. issue.list 行带 stage 字段（C2 前端徽标数据源）。
    let (d, err) = api
        .call("board", "issue.list", Some(json!({ "project_id": proj })))
        .await;
    match (&d, err) {
        (Some(d), None) => {
            let rows = d["issues"].as_array().cloned().unwrap_or_default();
            let with_stage = rows.iter().filter(|r| r.get("stage").is_some()).count();
            if with_stage == rows.len() && !rows.is_empty() {
                results.push(pass(&format!("{suite}/issue_list_stage"), "全部行带 stage"));
            } else {
                results.push(fail(
                    &format!("{suite}/issue_list_stage"),
                    format!("{with_stage}/{} 行带 stage", rows.len()),
                ));
            }
        }
        _ => results.push(fail(&format!("{suite}/issue_list"), "issue.list failed")),
    }

    results
}

/// A2 + A2b（看板项目档案 goal P1）表面契约：issue.list 默认排除归档项目
/// 子单/已取消单/hidden 单（include_* 显式放行；hidden 无放行口）+
/// issue.bulk_archive 批量清理（非取消单整体拒绝）+ 逐单审计。
/// 单节点表面契约——真决策流归 cluster-uat 双节点（沿 p5 同款边界）。
pub async fn test_board_archive_filter_surface(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "board_ws/archive_filter_surface";
    let mut results = Vec::new();
    print_suite_header(suite);
    let _ = (ws, bin);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // ---- 1. 建项目 + 项目子单 ×2 + 独立单 ×1。
    let proj = match api
        .call(
            "board",
            "project.create",
            Some(json!({ "name": "ARCHF 归档过滤", "description": "IT" })),
        )
        .await
    {
        (Some(d), None) => d["project"]["id"].as_i64().unwrap_or(0),
        _ => {
            results.push(fail(
                &format!("{suite}/create_project"),
                "project.create failed",
            ));
            return results;
        }
    };
    let mut in_proj_ids = Vec::new();
    for title in ["ARCHF 项目子单甲", "ARCHF 项目子单乙"] {
        let (d, err) = api
            .call(
                "board",
                "issue.create",
                Some(json!({ "title": title, "project_id": proj })),
            )
            .await;
        match (d, err) {
            (Some(d), None) => in_proj_ids.push(d["issue"]["id"].as_i64().unwrap_or(0)),
            _ => {
                results.push(fail(
                    &format!("{suite}/create {title}"),
                    "issue.create failed",
                ));
                return results;
            }
        }
    }
    let (d, err) = api
        .call(
            "board",
            "issue.create",
            Some(json!({ "title": "ARCHF 独立单" })),
        )
        .await;
    let standalone_id = match (d, err) {
        (Some(d), None) => d["issue"]["id"].as_i64().unwrap_or(0),
        _ => {
            results.push(fail(
                &format!("{suite}/create standalone"),
                "issue.create failed",
            ));
            return results;
        }
    };

    // ---- 2. 项目活跃期：默认列表三单全见。
    let (d, err) = api.call("board", "issue.list", Some(json!({}))).await;
    match (&d, err) {
        (Some(d), None) => {
            let ids: Vec<i64> = d["issues"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|r| r["id"].as_i64())
                .collect();
            if in_proj_ids.iter().all(|i| ids.contains(i)) && ids.contains(&standalone_id) {
                results.push(pass(
                    &format!("{suite}/active_all_visible"),
                    "项目活跃期三单全见",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/active_all_visible"),
                    format!("缺单：ids={ids:?}"),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/active_all_visible"),
            "issue.list failed",
        )),
    }

    // ---- 3. 归档项目：默认排除子单；include_archived_projects=true 放行。
    let (_, err) = api
        .call(
            "board",
            "project.update",
            Some(json!({ "id": proj, "status": "archived" })),
        )
        .await;
    if err.is_some() {
        results.push(fail(
            &format!("{suite}/archive_project"),
            format!("{err:?}"),
        ));
    }
    let (d, err) = api.call("board", "issue.list", Some(json!({}))).await;
    match (&d, err) {
        (Some(d), None) => {
            let ids: Vec<i64> = d["issues"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|r| r["id"].as_i64())
                .collect();
            if ids.contains(&standalone_id) && in_proj_ids.iter().all(|i| !ids.contains(i)) {
                results.push(pass(
                    &format!("{suite}/archived_excluded"),
                    "归档项目子单默认排除，独立单保留",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/archived_excluded"),
                    format!("期望只余独立单：ids={ids:?}"),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/archived_excluded"),
            "issue.list failed",
        )),
    }
    let (d, err) = api
        .call(
            "board",
            "issue.list",
            Some(json!({ "include_archived_projects": true })),
        )
        .await;
    match (&d, err) {
        (Some(d), None) => {
            let ids: Vec<i64> = d["issues"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|r| r["id"].as_i64())
                .collect();
            if in_proj_ids.iter().all(|i| ids.contains(i)) {
                results.push(pass(
                    &format!("{suite}/archived_include"),
                    "include_archived_projects=true 放行",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/archived_include"),
                    format!("放行后仍缺子单：ids={ids:?}"),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/archived_include"),
            "issue.list failed",
        )),
    }

    // ---- 4. 取消独立单：默认排除；include_cancelled=true 放行。
    let (_, err) = api
        .call(
            "board",
            "issue.status",
            Some(json!({ "id": standalone_id, "status": "cancelled" })),
        )
        .await;
    if err.is_some() {
        results.push(fail(
            &format!("{suite}/cancel_standalone"),
            format!("{err:?}"),
        ));
    }
    let (d, err) = api.call("board", "issue.list", Some(json!({}))).await;
    match (&d, err) {
        (Some(d), None) => {
            let ids: Vec<i64> = d["issues"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|r| r["id"].as_i64())
                .collect();
            // 共享网关上有其他套件的单——只断言本套件三单全被排除。
            let mine_clean =
                !ids.contains(&standalone_id) && in_proj_ids.iter().all(|i| !ids.contains(i));
            if mine_clean {
                results.push(pass(
                    &format!("{suite}/cancelled_excluded"),
                    "取消单默认排除（本套件三单全不在默认面）",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/cancelled_excluded"),
                    format!("取消/归档单未被排除：ids={ids:?}"),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/cancelled_excluded"),
            "issue.list failed",
        )),
    }
    let (d, err) = api
        .call(
            "board",
            "issue.list",
            Some(json!({ "include_cancelled": true })),
        )
        .await;
    match (&d, err) {
        (Some(d), None) => {
            let ids: Vec<i64> = d["issues"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|r| r["id"].as_i64())
                .collect();
            if ids.contains(&standalone_id) && in_proj_ids.iter().all(|i| !ids.contains(i)) {
                results.push(pass(
                    &format!("{suite}/cancelled_include"),
                    "include_cancelled=true 放行取消单（归档项目子单仍排除）",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/cancelled_include"),
                    format!("期望只余独立取消单：ids={ids:?}"),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/cancelled_include"),
            "issue.list failed",
        )),
    }

    // ---- 5. 清理：bulk_archive 取消单 → 默认面 + include_cancelled 面都消失。
    let (d, err) = api
        .call(
            "board",
            "issue.bulk_archive",
            Some(json!({ "ids": [standalone_id] })),
        )
        .await;
    match (&d, err) {
        (Some(d), None) => {
            if d["archived"].as_i64() == Some(1) {
                results.push(pass(&format!("{suite}/bulk_archive_ok"), "archived=1"));
            } else {
                results.push(fail(
                    &format!("{suite}/bulk_archive_ok"),
                    format!("unexpected: {d}"),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/bulk_archive_ok"),
            "issue.bulk_archive failed",
        )),
    }
    let (d, err) = api
        .call(
            "board",
            "issue.list",
            Some(json!({ "include_cancelled": true })),
        )
        .await;
    match (&d, err) {
        (Some(d), None) => {
            let ids: Vec<i64> = d["issues"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|r| r["id"].as_i64())
                .collect();
            if !ids.contains(&standalone_id) {
                results.push(pass(
                    &format!("{suite}/hidden_no_bypass"),
                    "hidden 无放行口：include_cancelled 面也不再可见",
                ));
            } else {
                results.push(fail(
                    &format!("{suite}/hidden_no_bypass"),
                    "hidden 单仍在 include_cancelled 面",
                ));
            }
        }
        _ => results.push(fail(
            &format!("{suite}/hidden_no_bypass"),
            "issue.list failed",
        )),
    }

    // ---- 6. 非取消单整体拒绝（用项目子单——非 cancelled 状态）。
    let (_, err) = api
        .call(
            "board",
            "issue.bulk_archive",
            Some(json!({ "ids": [in_proj_ids[0]] })),
        )
        .await;
    match err {
        Some(e) if e.contains("不是已取消单") => {
            results.push(pass(&format!("{suite}/reject_non_cancelled"), e));
        }
        other => results.push(fail(
            &format!("{suite}/reject_non_cancelled"),
            format!("期望「不是已取消单」拒绝，实际 {other:?}"),
        )),
    }

    // ---- 7. 审计：action=issue_bulk_archive 的活动行存在且指向被清单。
    let (d, err) = api
        .call(
            "board",
            "audit.list",
            Some(json!({ "limit": 100, "action": "issue_bulk_archive" })),
        )
        .await;
    match (&d, err) {
        (Some(d), None) => {
            let rows = d["decisions"].as_array().cloned().unwrap_or_default();
            let hit = rows
                .iter()
                .any(|r| r["issue_id"].as_i64() == Some(standalone_id));
            if hit {
                results.push(pass(&format!("{suite}/audit_row"), "逐单审计已落"));
            } else {
                results.push(fail(
                    &format!("{suite}/audit_row"),
                    format!("{} 行审计中无独立单记录", rows.len()),
                ));
            }
        }
        _ => results.push(fail(&format!("{suite}/audit_row"), "audit.list failed")),
    }

    results
}

/// 看板项目档案 P2/B+C 套件：目录绑定（显式/自动）+ 四件套脚手架 + 防绕过
/// 拒绝面 + plan.md 拆解里程碑（代码触发）+ project.json 状态投影。
///
/// 运行位置：末位（改共享状态纪律）——热切默认模型（models.set_default）+
/// 改 board config + 写 board.db + 落盘档案目录；结束时恢复默认模型与开关。
pub async fn test_board_project_archive(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "board_ws/project_archive";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut api = match WsApi::connect().await {
        Ok(a) => a,
        Err(e) => {
            results.push(fail(suite, format!("WsApi connect failed: {e}")));
            return results;
        }
    };

    // ---- 1. 显式目录建项目：绝对路径 → 落盘四件套脚手架 ----
    let base = std::env::temp_dir().join(format!("nb-it-bproj-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dir_a = base.join("PROJ-A");
    let (d, err) = api
        .call(
            "board",
            "project.create",
            Some(json!({
                "name": "ITP2 显式档案",
                "description": "IT project archive suite",
                "directory": dir_a.to_string_lossy(),
            })),
        )
        .await;
    let Some(pa) = d.as_ref().and_then(|d| d["project"]["id"].as_i64()) else {
        results.push(fail(
            &format!("{suite}/create_explicit"),
            format!("project.create failed: {err:?} / {d:?}"),
        ));
        return results;
    };
    let resp_dir = d
        .as_ref()
        .and_then(|d| d["directory"].as_str())
        .unwrap_or_default()
        .to_string();
    let root_a = std::path::PathBuf::from(&resp_dir);
    let mut scaffold_ok = root_a.join("project.json").is_file()
        && root_a.join("timeline.jsonl").is_file()
        && root_a.join("docs").is_dir()
        && root_a.join("records").is_dir();
    if let Ok(gi) = std::fs::read_to_string(root_a.join(".gitignore")) {
        scaffold_ok = scaffold_ok && gi.contains("/project.json") && gi.contains("/records/");
    } else {
        scaffold_ok = false;
    }
    if scaffold_ok {
        results.push(pass(
            &format!("{suite}/create_explicit"),
            format!("显式目录绑定 + 四件套：{resp_dir}"),
        ));
    } else {
        results.push(fail(
            &format!("{suite}/create_explicit"),
            format!("脚手架缺失 @ {resp_dir}"),
        ));
    }
    // manifest 落库：id 回填 + status 投影（sync_project_manifest 创建即跑）。
    match std::fs::read_to_string(root_a.join("project.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
    {
        Some(m) if m["project_id"].as_i64() == Some(pa) && m["status"] == "active" => {
            results.push(pass(
                &format!("{suite}/manifest"),
                "project.json id 回填+status",
            ));
        }
        other => results.push(fail(
            &format!("{suite}/manifest"),
            format!("project.json 异常: {other:?}"),
        )),
    }

    // ---- 2. 自动分配目录：落到 <workspace>/board-projects/ 下 ----
    let (d, err) = api
        .call(
            "board",
            "project.create",
            Some(json!({ "name": "ITP2 自动档案", "description": "IT" })),
        )
        .await;
    let Some(pb) = d.as_ref().and_then(|d| d["project"]["id"].as_i64()) else {
        results.push(fail(
            &format!("{suite}/create_auto"),
            format!("project.create(无目录) failed: {err:?}"),
        ));
        return results;
    };
    let resp_dir_b = d
        .as_ref()
        .and_then(|d| d["directory"].as_str())
        .unwrap_or_default()
        .to_string();
    let ws_prefix = ws.workspace().to_string_lossy().to_string();
    // CI Windows runner 的 %TEMP% 是 8.3 短名形态（`C:\Users\RUNNER~1\...`，
    // 真名 runneradmin），而服务端返回的 directory 是 canonical 展开后的
    // 真名路径——裸字符串前缀比对在短名环境必假红（2026-09-18 CI 实录）。
    // 两侧都过 std::fs::canonicalize 归一（verbatim 前缀 + 真名，两侧一致）
    // 后再做组件级 starts_with；canonicalize 失败（目录不存在等）回退原值。
    let canonical_str = |p: &std::path::Path| -> String {
        std::fs::canonicalize(p)
            .map(|c| c.to_string_lossy().to_string())
            .unwrap_or_else(|_| p.to_string_lossy().to_string())
    };
    let in_ws = std::path::Path::new(&canonical_str(std::path::Path::new(&resp_dir_b)))
        .starts_with(std::path::Path::new(&canonical_str(
            ws.workspace().as_path(),
        )));
    if resp_dir_b.contains("board-projects") && in_ws {
        results.push(pass(
            &format!("{suite}/create_auto"),
            format!("自动目录落 workspace 内：{resp_dir_b}"),
        ));
    } else {
        results.push(fail(
            &format!("{suite}/create_auto"),
            format!("自动目录越界/未归位：{resp_dir_b}（workspace={ws_prefix}）"),
        ));
    }

    // ---- 3. 防绕过拒绝面（workspace 重叠双向 / 既有项目重叠 / 相对路径 / 8.3）----
    let rejects: Vec<(&str, Value)> = vec![
        (
            "ws_self",
            json!({ "name": "ITP2 拒绝甲", "directory": ws.workspace().to_string_lossy() }),
        ),
        (
            "ws_inside",
            json!({
                "name": "ITP2 拒绝乙",
                "directory": ws
                    .workspace()
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
            }),
        ),
        (
            "proj_overlap",
            json!({ "name": "ITP2 拒绝丙", "directory": root_a.join("docs").to_string_lossy() }),
        ),
        (
            "relative",
            json!({ "name": "ITP2 拒绝丁", "directory": "./rel-proj" }),
        ),
        (
            "shortname83",
            json!({ "name": "ITP2 拒绝戊", "directory": "X:/nb-it-8dot3~1" }),
        ),
    ];
    for (tag, payload) in rejects {
        let (_, err) = api.call("board", "project.create", Some(payload)).await;
        if err.is_some() {
            results.push(pass(&format!("{suite}/reject_{tag}"), "诚实拒绝"));
        } else {
            results.push(fail(
                &format!("{suite}/reject_{tag}"),
                format!("{tag} 未被拒绝（应拒）"),
            ));
        }
    }

    // ---- 4. 拆解里程碑：plan 链 → docs/plan.md 代码触发落盘 ----
    let add = ws
        .run_cli(
            bin,
            &[
                "model",
                "add",
                "--model",
                "test/testai-board-1.0",
                "--base",
                &format!("http://127.0.0.1:{}/v1", ai_server_port()),
                "--key",
                "test-key",
            ],
        )
        .await;
    if !add.success() {
        results.push(fail(
            &format!("{suite}/model_add"),
            format!("exit={} stderr={}", add.exit_code, snip(&add.stderr)),
        ));
        return results;
    }
    let (_, err) = api
        .call(
            "models",
            "set_default",
            Some(json!({ "name": "testai-board-1.0" })),
        )
        .await;
    if err.is_some() {
        results.push(fail(&format!("{suite}/set_default"), format!("{err:?}")));
        return results;
    }
    let (_, err) = api
        .call(
            "board",
            "config.set",
            Some(json!({ "key": "plan.auto_confirm", "value": true })),
        )
        .await;
    if err.is_some() {
        results.push(fail(&format!("{suite}/auto_confirm"), format!("{err:?}")));
        return results;
    }
    let (d, err) = api
        .call(
            "board",
            "issue.create",
            Some(json!({
                "title": "IT项目档案 <PLAN_ANCHOR> 拆解里程碑",
                "description": "IT project archive：拆解里程碑落盘。",
                "project_id": pa,
            })),
        )
        .await;
    let Some(parent_id) = d.as_ref().and_then(|d| d["issue"]["id"].as_i64()) else {
        results.push(fail(
            &format!("{suite}/create_parent"),
            format!("issue.create failed: {err:?}"),
        ));
        return results;
    };
    let parent_number = d
        .as_ref()
        .and_then(|d| d["issue"]["number"].as_str())
        .unwrap_or_default()
        .to_string();
    let (_, err) = api
        .call("board", "issue.plan", Some(json!({ "id": parent_id })))
        .await;
    if err.is_some() {
        results.push(fail(&format!("{suite}/plan"), format!("{err:?}")));
        return results;
    }
    // 轮询 ≤90s 等 auto_confirm 建 3 子单（issue.list 必须传空对象）。
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
    let mut last_err: Option<String>;
    let sub_ids: Vec<i64> = loop {
        let (dat, err) = api.call("board", "issue.list", Some(json!({}))).await;
        last_err = err.map(|e| e.to_string());
        let subs: Vec<i64> = dat
            .as_ref()
            .and_then(|d| d.get("issues"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|i| {
                        i.get("parent_issue_id").and_then(|v| v.as_i64()) == Some(parent_id)
                    })
                    .filter_map(|i| i.get("id").and_then(|v| v.as_i64()))
                    .collect()
            })
            .unwrap_or_default();
        if subs.len() == 3 {
            break subs;
        }
        if tokio::time::Instant::now() >= deadline {
            results.push(fail(
                &format!("{suite}/subs_created"),
                format!(
                    "90s 内未建 3 子单（实际 {}，最后错误 {last_err:?}）",
                    subs.len()
                ),
            ));
            return results;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    };
    results.push(pass(&format!("{suite}/subs_created"), "3 subs created"));
    // docs/plan.md：代码触发（confirm_plan 挂接点），含子单清单 + 依赖图。
    let plan_md = std::fs::read_to_string(root_a.join("docs").join("plan.md")).unwrap_or_default();
    if plan_md.contains("# 拆解计划")
        && plan_md.contains("依赖：子0")
        && plan_md.contains("### 子0：")
    {
        results.push(pass(
            &format!("{suite}/plan_md"),
            "docs/plan.md 落盘（子单清单+依赖图）",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/plan_md"),
            format!("plan.md 内容缺失：{}", trunc_str(&plan_md, 200)),
        ));
    }
    // timeline.jsonl：kind=plan 事件 + 父单编号（number 已含 NB- 前缀）。
    let timeline = std::fs::read_to_string(root_a.join("timeline.jsonl")).unwrap_or_default();
    if timeline.contains("\"kind\":\"plan\"") && timeline.contains(&parent_number) {
        results.push(pass(&format!("{suite}/timeline"), "plan 事件落 timeline"));
    } else {
        results.push(fail(
            &format!("{suite}/timeline"),
            format!("timeline 缺 plan 事件：{}", trunc_str(&timeline, 200)),
        ));
    }

    // ---- 5. 状态投影：project.update → project.json.status 同步刷新 ----
    let (_, err) = api
        .call(
            "board",
            "project.update",
            Some(json!({ "id": pa, "status": "archived" })),
        )
        .await;
    let proj_status = std::fs::read_to_string(root_a.join("project.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|m| m["status"].as_str().map(|s| s.to_string()));
    if err.is_none() && proj_status.as_deref() == Some("archived") {
        results.push(pass(&format!("{suite}/manifest_sync"), "status 投影同步"));
    } else {
        results.push(fail(
            &format!("{suite}/manifest_sync"),
            format!("update err={err:?} status={proj_status:?}"),
        ));
    }
    let _ = api
        .call(
            "board",
            "project.update",
            Some(json!({ "id": pa, "status": "active" })),
        )
        .await;

    // ---- 6. 清理：取消单 + 恢复默认模型与开关（尾位纪律）+ 删临时目录 ----
    for id in &sub_ids {
        let _ = api
            .call("board", "issue.cancel", Some(json!({ "id": id })))
            .await;
    }
    let _ = api
        .call("board", "issue.cancel", Some(json!({ "id": parent_id })))
        .await;
    let _ = api
        .call(
            "board",
            "config.set",
            Some(json!({ "key": "plan.auto_confirm", "value": false })),
        )
        .await;
    let _ = api
        .call(
            "board",
            "project.update",
            Some(json!({ "id": pb, "status": "archived" })),
        )
        .await;
    let (_, err) = api
        .call(
            "models",
            "set_default",
            Some(json!({ "name": "testai-1.1" })),
        )
        .await;
    if err.is_none() {
        results.push(pass(&format!("{suite}/restore"), "默认模型恢复 testai-1.1"));
    } else {
        results.push(fail(
            &format!("{suite}/restore"),
            format!("默认模型恢复失败（污染后续套件）: {err:?}"),
        ));
    }
    let _ = std::fs::remove_dir_all(&base);

    results
}
