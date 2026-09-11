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

    // 1. 六个 P4 键逐一 config.set。
    let cases: &[(&str, serde_json::Value)] = &[
        ("review.max_turns", json!(3)),
        ("review.selfcheck", json!(true)),
        ("review.auto_close_project", json!(true)),
        ("budget.max_subissues_per_parent", json!(10)),
        ("budget.max_total_redispatch", json!(5)),
        ("budget.wall_clock_budget_secs", json!(3600)),
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
