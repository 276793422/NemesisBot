//! `inject_chat_id` 回归锁（BUG-20261117-goals-echo）。
//!
//! 背景：goals 引擎 CONTINUE 注入消息的 chat_id 曾直接填裸 session_key
//! （`agent:main:session:<id>`），agent 回复出站同 chat_id 被 WebChannel
//! 拒投（只认 `web:<conn>` 形态）——bench `goal_gate` 流 B 的效果面断言
//! 90s 超时。修复 = 注入 chat_id 经 conv_router 解析活跃 web 连接
//! （委托 `post_agent::resolve_inject_chat_id` 单一真相源，events/loops
//! 注入器同款），无绑定回落 `web:<会话id>` 通道原生形态（死会话软失败
//! 语义，回复仍落历史）。
//!
//! 三义钉死：
//! 1. 有绑定 → target 解析出绑定值（`web:<conn>` 实时形态）；
//! 2. 无绑定 → 回落 `web:<会话id>` 通道原生形态（历史投递，不 panic 不空串）；
//! 3. 绑定更新（latest-wins）→ target 跟进新值（tab 重开场景）。

use super::{inject_chat_id, note_outbound_lagged};

/// 会话键形态（与生产注入路径一致的裸 session_key 输入）。
const SESSION_KEY: &str = "agent:main:session:goal-echo-test";

#[test]
fn bound_session_resolves_to_web_chat_id() {
    let router = std::sync::Arc::new(nemesis_web::ConvRouter::new());
    router.bind(SESSION_KEY, "web:conn-abc");
    let chat_id = inject_chat_id(&router, SESSION_KEY);
    assert_eq!(chat_id, "web:conn-abc", "有绑定必须解析出 web:<conn> 形态");
}

#[test]
fn unbound_session_falls_back_to_channel_native_form() {
    let router = std::sync::Arc::new(nemesis_web::ConvRouter::new());
    let chat_id = inject_chat_id(&router, SESSION_KEY);
    assert_eq!(
        chat_id, "web:goal-echo-test",
        "无绑定回落 web:<会话id> 通道原生形态（死会话软失败：回复仍落历史）"
    );
}

#[test]
fn rebinding_follows_latest_bind() {
    let router = std::sync::Arc::new(nemesis_web::ConvRouter::new());
    router.bind(SESSION_KEY, "web:conn-old");
    assert_eq!(inject_chat_id(&router, SESSION_KEY), "web:conn-old");
    // tab 关闭重开：新连接重新 bind（latest-wins），注入必须跟进新值。
    router.bind(SESSION_KEY, "web:conn-new");
    assert_eq!(inject_chat_id(&router, SESSION_KEY), "web:conn-new");
}

#[test]
fn other_session_binding_does_not_leak() {
    let router = std::sync::Arc::new(nemesis_web::ConvRouter::new());
    router.bind("agent:main:session:other", "web:conn-other");
    let chat_id = inject_chat_id(&router, SESSION_KEY);
    assert_eq!(
        chat_id, "web:goal-echo-test",
        "其他会话的绑定不得串到本会话（会话键隔离）"
    );
}

// ============================================================
// 出站 Lagged 记账（BUG-20261117-076）
// ============================================================

/// Lagged 丢帧必须先入 bus 出站丢失账本再 warn——`lagged_outbound()`
/// 指标完整性（bus 记账协议要求全部订阅端入账；本 watcher 此前只 warn
/// 是家族横扫的唯一漏网点）。突变验证：摘掉 helper 内 record 调用本测
/// 必红。
#[tokio::test]
async fn outbound_lagged_records_into_bus_accounting() {
    let bus = std::sync::Arc::new(nemesis_bus::MessageBus::with_capacity(2));
    let mut rx = bus.subscribe_outbound();
    // 容量 2 灌 4 条（订阅在先，发布不丢）→ 接收端首帧必报 Lagged(2)。
    for i in 0..4 {
        bus.publish_outbound(nemesis_types::channel::OutboundMessage::new(
            "web",
            "chat-lag",
            &format!("m{i}"),
        ));
    }
    let n = match rx.recv().await {
        Ok(_) => panic!("超容量灌入 + 零消费的首帧 recv 必报 Lagged"),
        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => n,
        Err(e) => panic!("unexpected recv error: {e:?}"),
    };
    assert_eq!(n, 2, "容量 2 灌 4 条：首帧 recv 必报 Lagged(2)");
    assert_eq!(bus.lagged_outbound(), 0, "处理前账本必须还是 0");

    note_outbound_lagged(&bus, n);
    assert_eq!(
        bus.lagged_outbound(),
        2,
        "Lagged(n) 必须入 bus 出站丢失账本（不只 warn）"
    );
}

// ============================================================
// BUG-20261118-154 回归锁：工具记录消费点 + 引擎转移状态机闸
// ============================================================

use std::collections::HashMap;
use std::sync::Arc;

use super::GoalsEngine;

/// 最小测试引擎（无 LLM 依赖：evaluate 的无 goal / md 缺失路径在
/// loop_for_session 之前返回；apply_verdict / block_goal 不调 LLM）。
fn test_engine(ws: &std::path::Path) -> Arc<GoalsEngine> {
    Arc::new(GoalsEngine {
        bus: Arc::new(nemesis_bus::MessageBus::with_capacity(16)),
        main_loop_ref: Arc::new(parking_lot::RwLock::new(None)),
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        workspace: ws.to_path_buf(),
        conv_router: Arc::new(nemesis_web::ConvRouter::new()),
        eval_lock: tokio::sync::Mutex::new(()),
        cursors: tokio::sync::Mutex::new(HashMap::new()),
        tool_calls: std::sync::Mutex::new(HashMap::new()),
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 工具记录消费点必须在评估周期入口（BUG-20261118-154 主发现）：无 goal
/// 绑定会话（生产常态）的记录曾永远不消费——登记面对全部会话累积、唯一
/// 消费点深在材料收集步，每会话 ≤30 条 KB 级预览只进不出 = 长驻进程
/// 无界增长。突变验证：把 take 移回材料收集步本测必红。
#[tokio::test]
async fn tool_records_consumed_even_without_goal() {
    let dir = tempfile::tempdir().unwrap();
    let engine = test_engine(dir.path());
    engine.record_tool_started("agent:main:session:s1", "c1", "exec", "ls");
    engine.record_tool_finished("agent:main:session:s1", "c1", "exec", true, "ok");
    assert_eq!(engine.tool_calls.lock().unwrap().len(), 1);
    // 空注册表 → 无 Active goal 早退；消费必须已发生。
    engine.evaluate("agent:main:session:s1").await;
    assert!(
        engine.tool_calls.lock().unwrap().is_empty(),
        "无 goal 会话的评估周期必须消费（清空）工具记录——只进不出即无界增长"
    );
}

/// goal.md 缺失早退路径同样消费（BUG-20261118-154 污染面半边）：记录
/// 曾跨 turn 滞留，陈旧材料混入下一轮评估。
#[tokio::test]
async fn tool_records_consumed_when_goal_md_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let sk = "agent:main:session:s2";
    nemesis_goals::update_registry(ws, |entries| {
        entries.push(nemesis_goals::GoalEntry::new(
            "G1".to_string(),
            sk.to_string(),
            nemesis_goals::GoalCreatedBy::Agent,
            None,
            None,
            now_ms(),
        ));
        Ok(())
    })
    .unwrap();
    let engine = test_engine(ws);
    engine.record_tool_started(sk, "c1", "write_file", "x");
    // goal.md 文件不存在 → md 缺失早退；消费必须已发生（入口 take）。
    engine.evaluate(sk).await;
    assert!(
        engine.tool_calls.lock().unwrap().is_empty(),
        "goal.md 缺失早退不得让工具记录跨 turn 滞留"
    );
}

/// 评估 LLM 窗口内人已 disable：done 写入必须被状态机闸跳过——
/// paused→done 是转移表禁止的非法态，引擎不得覆盖人操作。
#[test]
fn done_write_skipped_when_goal_no_longer_active() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let sk = "agent:main:session:s3";
    let entry = nemesis_goals::GoalEntry::new(
        "G1".to_string(),
        sk.to_string(),
        nemesis_goals::GoalCreatedBy::Human,
        None,
        None,
        now_ms(),
    );
    // 注册表预置 Paused（模拟评估窗口内人 disable）；评估快照仍是 Active。
    let mut paused = entry.clone();
    paused.status = nemesis_goals::GoalStatus::Paused;
    nemesis_goals::update_registry(ws, |entries| {
        entries.push(paused);
        Ok(())
    })
    .unwrap();
    let engine = test_engine(ws);
    engine.apply_verdict(
        &entry,
        nemesis_goals::EvalVerdict::Done,
        None,
        "main",
        std::time::Instant::now(),
        "{\"verdict\":\"DONE\"}",
    );
    let entries = nemesis_goals::load_registry(ws).unwrap();
    assert_eq!(
        entries[0].status,
        nemesis_goals::GoalStatus::Paused,
        "评估窗口内人 pause 的 goal 不得被引擎写成 done（转移表 Paused→Done 禁止）"
    );
}

/// Continue 臂同闸：paused goal 不占轮数预算。
#[test]
fn continue_round_bump_skipped_when_goal_no_longer_active() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let sk = "agent:main:session:s4";
    let entry = nemesis_goals::GoalEntry::new(
        "G1".to_string(),
        sk.to_string(),
        nemesis_goals::GoalCreatedBy::Human,
        None,
        None,
        now_ms(),
    );
    let mut paused = entry.clone();
    paused.status = nemesis_goals::GoalStatus::Paused;
    nemesis_goals::update_registry(ws, |entries| {
        entries.push(paused);
        Ok(())
    })
    .unwrap();
    let engine = test_engine(ws);
    engine.apply_verdict(
        &entry,
        nemesis_goals::EvalVerdict::Continue,
        Some("缺口".to_string()),
        "main",
        std::time::Instant::now(),
        "{\"verdict\":\"CONTINUE\"}",
    );
    let entries = nemesis_goals::load_registry(ws).unwrap();
    assert_eq!(entries[0].status, nemesis_goals::GoalStatus::Paused);
    assert_eq!(
        entries[0].rounds_used, 0,
        "评估窗口内人 pause 的 goal 不得被引擎无声 rounds+1（偷扣预算）"
    );
}

/// block_goal 状态机闸：paused goal 不被引擎覆盖成 blocked。
#[test]
fn block_goal_skipped_when_goal_no_longer_active() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let sk = "agent:main:session:s5";
    let mut paused = nemesis_goals::GoalEntry::new(
        "G1".to_string(),
        sk.to_string(),
        nemesis_goals::GoalCreatedBy::Human,
        None,
        None,
        now_ms(),
    );
    paused.status = nemesis_goals::GoalStatus::Paused;
    nemesis_goals::update_registry(ws, |entries| {
        entries.push(paused);
        Ok(())
    })
    .unwrap();
    let engine = test_engine(ws);
    engine.block_goal("G1", "round-limit", "预算耗尽");
    let entries = nemesis_goals::load_registry(ws).unwrap();
    assert_eq!(
        entries[0].status,
        nemesis_goals::GoalStatus::Paused,
        "评估窗口内人 pause 的 goal 不得被引擎写成 blocked"
    );
    assert_eq!(entries[0].blocked_code, None);
}

/// 正向对照：Active goal 的 done 判定照常落地（闸不误伤正常路径）。
#[test]
fn done_write_applies_when_still_active() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let sk = "agent:main:session:s6";
    let entry = nemesis_goals::GoalEntry::new(
        "G1".to_string(),
        sk.to_string(),
        nemesis_goals::GoalCreatedBy::Agent,
        None,
        None,
        now_ms(),
    );
    nemesis_goals::update_registry(ws, |entries| {
        entries.push(entry.clone());
        Ok(())
    })
    .unwrap();
    let engine = test_engine(ws);
    engine.apply_verdict(
        &entry,
        nemesis_goals::EvalVerdict::Done,
        None,
        "main",
        std::time::Instant::now(),
        "{\"verdict\":\"DONE\"}",
    );
    let entries = nemesis_goals::load_registry(ws).unwrap();
    assert_eq!(
        entries[0].status,
        nemesis_goals::GoalStatus::Done,
        "Active goal 的 done 判定必须照常落地"
    );
}
