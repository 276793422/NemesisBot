//! userprofile::sweep_once 编排级测试（批 A 主语分型路由）。
//!
//! 单元层已各自覆盖（parse 分流 / bridge IO / upsert 双保险）；这里补
//! 编排层：mock LLMCaller 返回混合 kind 结论 → 跑一遍 sweep_once →
//! 断言三面分流（画像只收 habit / bridge 队列收 lesson / ledger 两类
//! 都留痕）。

use std::path::Path;

use super::bridge;
use super::reflection;
use super::store;
use super::sweep_once;
use crate::reflector_llm::LLMCaller;

/// 固定响应 mock caller（编排测试不测 LLM 本身，测路由）。
struct FixedCaller {
    resp: String,
}

#[async_trait::async_trait]
impl LLMCaller for FixedCaller {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        Ok(self.resp.clone())
    }
}

/// 种 workspace：config.json（画像开 + 反思消息闸降到 2）/ 画像文件
/// （enabled_at 锚点 1h 前，计数器 3——反思成功后应归零）/ session_logs
/// 两 user + 一 assistant（30 分钟前，窗内）。
fn seed_workspace(ws: &Path) {
    let cfg_path = ws.join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"user_profile": {"enabled": true, "reflect_min_new_messages": 2}}"#,
    )
    .unwrap();

    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let profile = serde_json::json!({
        "enabled_at": enabled_at,
        "sweeps_since_reflect": 3,
    });
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, serde_json::to_string_pretty(&profile).unwrap()).unwrap();

    let ts = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
    let dir = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for (role, content) in [
        ("user", "帮我把登录页改成暗色主题"),
        ("assistant", "已修改完成，请查看"),
        ("user", "顺便把标题字号调大一点"),
    ] {
        let row = serde_json::json!({"role": role, "content": content, "timestamp": ts});
        body.push_str(&serde_json::to_string(&row).unwrap());
        body.push('\n');
    }
    std::fs::write(dir.join("sess-a.jsonl"), body).unwrap();
}

#[test]
fn sweep_once_routes_habits_and_lessons() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    seed_workspace(ws);
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FixedCaller {
        resp: r#"[
            {"kind": "habit", "category": "workflow", "content": "用户偏好先小步修改再迭代",
             "confidence": 0.9, "steps": ["提出小改动", "确认后追加"]},
            {"kind": "lesson", "category": "workflow", "content": "助手曾未确认需求范围就直接动手",
             "confidence": 0.85}
        ]"#
        .to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let processed = sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    assert_eq!(processed, 2, "窗内 2 条用户消息");

    // 画像面：habit 入画像（反思桶 auto=false），lesson 绝不入（路由 +
    // upsert 双保险端到端）。
    let profile = store::load_profile(ws);
    assert_eq!(profile.habits.len(), 1, "只有 habit 进画像");
    assert_eq!(profile.habits[0].content, "用户偏好先小步修改再迭代");
    assert!(!profile.habits[0].auto, "反思产物 auto=false");
    assert_eq!(
        profile.habits[0].steps,
        vec!["提出小改动".to_string(), "确认后追加".to_string()]
    );
    // 反思成功：计数器归零 + 反思水线推进。
    assert_eq!(profile.sweeps_since_reflect, 0);
    assert!(profile.last_reflect_ms.is_some());

    // bridge 队列面：lesson pending 落盘（入队时分配 id）。
    let pending = bridge::read_pending_lessons(ws, 10);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].content, "助手曾未确认需求范围就直接动手");
    assert_eq!(pending[0].status, bridge::LESSON_STATUS_PENDING);
    assert!(!pending[0].id.is_empty());
    assert!((pending[0].confidence - 0.85).abs() < 1e-9);

    // ledger 面：两类结论都留痕（知识复利回灌，教训不每轮重提）。
    let conclusions = reflection::read_recent_conclusions(ws, 10);
    assert!(conclusions.contains(&"用户偏好先小步修改再迭代".to_string()));
    assert!(conclusions.contains(&"助手曾未确认需求范围就直接动手".to_string()));
}

#[test]
fn sweep_once_disabled_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // config 无 user_profile 节 / enabled=false 都不走后续（哑 caller
    // 也不该被调用——若被调用返回 Err 会让反思 warn，但 no-op 断言的是
    // 画像/队列零写入）。
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": false}}"#,
    )
    .unwrap();
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FixedCaller {
        resp: "not json at all".to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let processed = sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    assert_eq!(processed, 0);
    assert!(store::load_profile(ws).habits.is_empty());
    assert!(bridge::read_pending_lessons(ws, 10).is_empty());
}

/// 置信度收口闸（复查 2026-11-24）：全部候选统一 ≥阈值才进画像。
/// 纠正前缀挖掘器置信度 0.5+3×0.1=0.8——阈值提到 0.99 时被收口闸拦下
///（画像零条目）；缺省阈值 0.7 时照常入画像（对照臂证明拦截来自阈值闸
/// 而非挖掘器没触发；滚动窗语义 = 每次扫描 ≥最近 24h，两臂同批消息可见）。
#[test]
fn sweep_once_confidence_gate_filters_subthreshold_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    // 种对话：3 条纠正前缀用户消息（触发纠正挖掘器，置信度 0.8）。
    let ts = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
    let logs = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&logs).unwrap();
    let mut body = String::new();
    for content in ["不对，这里逻辑反了", "不要用递归，改成迭代", "别动那个文件"]
    {
        let row = serde_json::json!({"role": "user", "content": content, "timestamp": ts});
        body.push_str(&serde_json::to_string(&row).unwrap());
        body.push('\n');
    }
    std::fs::write(logs.join("sess-gate.jsonl"), body).unwrap();

    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        p,
        serde_json::to_string_pretty(&serde_json::json!({"enabled_at": enabled_at})).unwrap(),
    )
    .unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    // 高阈值臂：0.8 候选低于 0.99 → 收口闸拦下，画像零条目。
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true, "confidence_threshold": 0.99}}"#,
    )
    .unwrap();
    sweep_once(&ws.join("config.json"), ws, &handle, None).unwrap();
    assert!(
        store::load_profile(ws).habits.is_empty(),
        "低于阈值的挖掘候选被收口闸拦下"
    );

    // 对照臂：缺省阈值 0.7 → 0.8 候选照常入画像。
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    sweep_once(&ws.join("config.json"), ws, &handle, None).unwrap();
    let profile = store::load_profile(ws);
    assert_eq!(profile.habits.len(), 1, "缺省阈值下纠正习惯入画像");
    assert_eq!(profile.habits[0].category, "correction");
}

#[test]
fn collect_stats_counts_all_channels() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    // 空工作区 = 全零统计（各通道独立容错，缺文件不陪葬）。
    let empty = super::surface::collect_stats(ws);
    assert_eq!(empty["todos"]["snapshot_count"], 0);
    assert_eq!(empty["lessons"]["pending"], 0);
    assert_eq!(empty["signals_recent"]["approval_cards"], 0);

    // 种齐四通道：画像 2 条（1 自动 1 手动）/ 教训队列 1+1 / 信号 2 审批
    // +1 急停 / todo 快照 1 项完成。
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    let profile = serde_json::json!({
        "enabled_at": chrono::Utc::now().to_rfc3339(),
        "habits": [
            {"category": "workflow", "content": "先计划再动手", "confidence": 0.8,
             "hits": 2, "status": "active", "auto": true},
            {"category": "style", "content": "偏好简洁回复", "confidence": 0.9,
             "hits": 5, "status": "active", "auto": false},
        ],
    });
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, serde_json::to_string_pretty(&profile).unwrap()).unwrap();

    let ldir = ws.join("forge").join("userprofile");
    std::fs::create_dir_all(&ldir).unwrap();
    std::fs::write(
        ldir.join("outbound_lessons.jsonl"),
        concat!(
            r#"{"id":"1","ts_ms":1,"kind":"lesson","category":"workflow","content":"a","confidence":0.5,"status":"pending"}"#, "\n",
            r#"{"id":"2","ts_ms":2,"kind":"lesson","category":"style","content":"b","confidence":0.5,"status":"acked"}"#, "\n",
            r#"{"id":"3","ts_ms":3,"kind":"lesson","category":"style","content":"c","confidence":0.5,"status":"pending"}"#, "\n",
        ),
    )
    .unwrap();
    std::fs::write(
        ldir.join("signals.jsonl"),
        concat!(
            r#"{"ts_ms":1,"kind":"approval","operation":"file_write","approved":true,"duration_ms":100}"#, "\n",
            r#"{"ts_ms":2,"kind":"approval","operation":"exec","approved":false,"duration_ms":200}"#, "\n",
            r#"{"ts_ms":3,"kind":"estop","action":"trigger"}"#, "\n",
        ),
    )
    .unwrap();
    let sdir = ws.join("sessions");
    std::fs::create_dir_all(&sdir).unwrap();
    let todo_path = sdir.join("todo_stats_seed.json");
    std::fs::write(&todo_path, r#"[{"content":"写测试","status":"completed"}]"#).unwrap();
    // mtime 保新鲜：刚写的文件天然在 7 天窗内。

    let s = super::surface::collect_stats(ws);
    assert_eq!(s["todos"]["snapshot_count"], 1);
    assert_eq!(s["todos"]["item_count"], 1);
    assert_eq!(s["todos"]["completed_count"], 1);
    assert_eq!(s["lessons"]["pending"], 2);
    assert_eq!(s["lessons"]["acked"], 1);
    assert_eq!(s["habits_by_category"]["workflow"], 1);
    assert_eq!(s["habits_by_category"]["style"], 1);
    assert_eq!(s["habits_by_source"]["auto"], 1);
    assert_eq!(s["habits_by_source"]["manual"], 1);
    assert_eq!(s["signals_recent"]["approval_cards"], 2);
    assert_eq!(s["signals_recent"]["estop"], 1);

    // 清理 todo（防 tempdir 外泄无关，保持种子局部性即可）。
    let _ = std::fs::remove_file(todo_path);
}
