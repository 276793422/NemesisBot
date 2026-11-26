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
use super::{SweepOpts, sweep_once, sweep_once_opts};
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
    assert_eq!(processed.total_user_messages(), 2, "窗内 2 条用户消息");

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
    assert_eq!(processed.total_user_messages(), 0);
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

    // 种齐四通道：画像 4 条（四态来源各 1：模板兜底/AI 总结/观察转正/
    // 外部回抛）/ 教训队列 1+1 / 信号 2 审批 + 1 急停 / todo 快照 1 项完成。
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
            {"category": "workflow", "content": "观察转正结论", "confidence": 0.8,
             "hits": 1, "status": "active", "auto": false, "source": "observation"},
            {"category": "style", "content": "外部回抛经验", "confidence": 0.7,
             "hits": 1, "status": "active", "auto": false, "source": "learn_dialogue"},
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
    assert_eq!(s["habits_by_category"]["workflow"], 2);
    assert_eq!(s["habits_by_category"]["style"], 2);
    // 四桶来源统计（批三 §4.5：source_kind 单一派生）。
    assert_eq!(s["habits_by_source"]["ai"], 1);
    assert_eq!(s["habits_by_source"]["template"], 1);
    assert_eq!(s["habits_by_source"]["observation"], 1);
    assert_eq!(s["habits_by_source"]["external"], 1);
    assert_eq!(s["signals_recent"]["approval_cards"], 2);
    assert_eq!(s["signals_recent"]["estop"], 1);

    // 清理 todo（防 tempdir 外泄无关，保持种子局部性即可）。
    let _ = std::fs::remove_file(todo_path);
}

// ---- 话术 AI 化批二：分流门 + AI 主通道（mock caller 编排级）----

/// 记录 prompt 的 mock caller（断言信号卡确实进了反思 prompt）。
struct CapturingCaller {
    prompts: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl LLMCaller for CapturingCaller {
    async fn chat(
        &self,
        _system_prompt: &str,
        user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        self.prompts.lock().unwrap().push(user_prompt.to_string());
        Ok("[]".to_string())
    }
}

/// 首调失败次调成功的 mock caller（UAT 场景三：中段断模型）。
struct FlakyCaller {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl LLMCaller for FlakyCaller {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            Err("模拟断模型".to_string())
        } else {
            Ok(r#"[{"kind": "habit", "category": "style",
                    "content": "用户爱用两三个字下指令", "confidence": 0.9,
                    "rule_ids": ["stat.brief"]}]"#
                .to_string())
        }
    }
}

/// 种 12 条超短用户消息（触发 stat.brief 信号卡——n≥10 闸 + 短消息占比）。
fn seed_short_messages(ws: &Path) {
    let ts = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
    let dir = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for _ in 0..12 {
        let row = serde_json::json!({"role": "user", "content": "继续", "timestamp": ts});
        body.push_str(&serde_json::to_string(&row).unwrap());
        body.push('\n');
    }
    std::fs::write(dir.join("sess-brief.jsonl"), body).unwrap();
}

/// 种反思可触发的画像锚点（enabled_at 1h 前 + 计数器 3——消息数主触发）。
fn seed_reflect_anchor(ws: &Path) {
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        p,
        serde_json::to_string_pretty(&serde_json::json!({
            "enabled_at": enabled_at,
            "sweeps_since_reflect": 3,
        }))
        .unwrap(),
    )
    .unwrap();
}

fn write_gate_config(ws: &Path) {
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true, "reflect_min_new_messages": 2}}"#,
    )
    .unwrap();
}

/// 门开（caller 在场 + 消息数到闸）：信号卡进反思 prompt（rule id 可见），
/// 且兜底模板**不再落库**——批一时代 12 条短消息会落 stat.brief 模板条目，
/// 现在画像零条目（mock 返回 [] = AI 未产结论，信号已消费不重落）。
#[test]
fn gate_open_routes_signals_into_reflection_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    seed_reflect_anchor(ws);
    seed_short_messages(ws);
    let prompts_log: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(CapturingCaller {
        prompts: prompts_log.clone(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();

    let prompts = prompts_log.lock().unwrap();
    assert_eq!(prompts.len(), 1, "反思恰好调用一次");
    assert!(
        prompts[0].contains("stat.brief"),
        "信号卡规则 id 必须进 prompt"
    );
    assert!(prompts[0].contains("本拍确定性信号"), "信号节标题在 prompt");
    // 门开 + AI 未产结论：兜底模板不落库（分流核心断言）。
    let profile = store::load_profile(ws);
    assert!(
        profile.habits.is_empty(),
        "门开时信号不落兜底条目，AI 未产结论 = 本拍零写入"
    );
}

/// 原地改写（§4.3 主路径）：AI 点名 stat.brief → 既有兜底条目被改写为
/// AI 措辞；hits 保留（不重置不 +1——改写不是信号命中）；auto true→false
/// （AI 总结桶）；rule_id 不变（后续同 id 信号仍归并到位）。
#[test]
fn gate_open_rewrite_preserves_hits_and_flips_to_ai_bucket() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        p,
        serde_json::to_string_pretty(&serde_json::json!({
            "enabled_at": enabled_at,
            "sweeps_since_reflect": 3,
            "habits": [
                {"category": "style", "content": "回复保持简短", "confidence": 0.8,
                 "hits": 5, "status": "active", "auto": true, "rule_id": "stat.brief",
                 "evidence": ["sess-orig"]}
            ],
        }))
        .unwrap(),
    )
    .unwrap();
    seed_short_messages(ws);
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FixedCaller {
        resp: r#"[{"kind": "habit", "category": "style",
                   "content": "用户爱用两三个字下指令", "confidence": 0.9,
                   "rule_ids": ["stat.brief"]}]"#
            .to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();

    let profile = store::load_profile(ws);
    assert_eq!(profile.habits.len(), 1, "原地改写不新增条目");
    let h = &profile.habits[0];
    assert_eq!(h.content, "用户爱用两三个字下指令", "措辞替换为 AI 版");
    assert!(!h.auto, "改写后落 AI 总结桶（auto=true 会挂模板徽标撒谎）");
    assert_eq!(h.hits, 5, "hits 保留：不重置也不因改写 +1");
    assert_eq!(h.rule_id, "stat.brief", "规则身份证不变");
    assert!(h.confidence >= 0.9, "置信度取高");
    assert!(
        h.evidence.contains(&"sess-orig".to_string()),
        "既有溯源保留"
    );
    // 门开：12 条短消息的信号已被 prompt 消费，不再对同 id 兜底归并
    // （否则 hits 会变 6）。
}

/// 多 id 合并覆盖（§4.3）：首个可改写位 = 目标，其余可改写位 = 被吞
/// （evidence 并入 + 删除）；vetoed / pinned 持有者整位跳过——否决条目
/// 不复活，用户钉住的措辞不被 AI 静默改写。
#[test]
fn gate_open_merge_swallow_spares_vetoed_and_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        p,
        serde_json::to_string_pretty(&serde_json::json!({
            "enabled_at": enabled_at,
            "sweeps_since_reflect": 3,
            "habits": [
                {"category": "style", "content": "回复保持简短", "confidence": 0.8,
                 "hits": 5, "status": "active", "auto": true, "rule_id": "stat.brief",
                 "evidence": ["sess-a"]},
                {"category": "style", "content": "长短混合穿插", "confidence": 0.8,
                 "hits": 2, "status": "active", "auto": true, "rule_id": "stat.mixed",
                 "evidence": ["sess-b"]},
                {"category": "style", "content": "已被否决的混合结论", "confidence": 0.8,
                 "hits": 1, "status": "vetoed", "auto": true, "rule_id": "stat.mixed"},
                {"category": "workflow", "content": "先计划再动手", "confidence": 0.9,
                 "hits": 3, "status": "active", "auto": false, "pinned": true,
                 "rule_id": "ritual.plan"}
            ],
        }))
        .unwrap(),
    )
    .unwrap();
    seed_short_messages(ws);
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FixedCaller {
        resp: r#"[{"kind": "habit", "category": "style",
                   "content": "用户习惯用超短指令推进对话", "confidence": 0.95,
                   "rule_ids": ["stat.brief", "stat.mixed", "ritual.plan"]}]"#
            .to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();

    let profile = store::load_profile(ws);
    assert_eq!(profile.habits.len(), 3, "目标改写 + 被吞删除 + 两豁免保留");
    // 注意：sweep 末尾 prune_and_cap 会按 pinned 优先 + manual/auto/vetoed
    // 分桶重排（store.rs 既有行为）——改写产物 auto=false 进 manual 桶，
    // pinned 条目排桶首；vetoed 垫底。按身份断言而非位置。
    let pinned = profile
        .habits
        .iter()
        .find(|h| h.pinned)
        .expect("pinned 条目保留");
    assert_eq!(
        pinned.content, "先计划再动手",
        "pinned 措辞不被 AI 静默改写"
    );
    assert_eq!(pinned.rule_id, "ritual.plan");
    let target = profile
        .habits
        .iter()
        .find(|h| h.rule_id == "stat.brief")
        .expect("改写目标保留");
    assert_eq!(target.content, "用户习惯用超短指令推进对话");
    assert!(!target.auto, "改写落 AI 总结桶");
    assert_eq!(target.hits, 5, "改写不是信号命中，hits 保留");
    assert!(
        target.evidence.contains(&"sess-b".to_string()),
        "被吞条目溯源并入目标"
    );
    assert!(
        target.evidence.contains(&"sess-a".to_string()),
        "目标既有溯源保留"
    );
    let vetoed = profile
        .habits
        .iter()
        .find(|h| h.status == store::STATUS_VETOED)
        .expect("vetoed 同 id 持有者不被删除");
    assert_eq!(vetoed.content, "已被否决的混合结论", "否决条目内容原样");
    assert_eq!(
        profile
            .habits
            .iter()
            .filter(|h| h.rule_id == "stat.mixed" && h.status != store::STATUS_VETOED)
            .count(),
        0,
        "被吞条目（同 id 可改写位）已删除"
    );
}

/// 纯 AI 结论（不带 rule_ids）回归不变：按 content upsert 插入，rule_id
/// 保持空；同时门开的兜底抑制仍生效（12 条短消息不落 stat.brief 模板）。
#[test]
fn gate_open_pure_ai_conclusion_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    seed_reflect_anchor(ws);
    seed_short_messages(ws);
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FixedCaller {
        resp: r#"[{"kind": "habit", "category": "style",
                   "content": "用户回复极简，常以两字指令推进", "confidence": 0.9}]"#
            .to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();

    let profile = store::load_profile(ws);
    assert_eq!(profile.habits.len(), 1, "只收 AI 结论，模板不落库");
    let h = &profile.habits[0];
    assert_eq!(h.content, "用户回复极简，常以两字指令推进");
    assert!(h.rule_id.is_empty(), "纯 AI 结论不带规则身份证");
    assert!(!h.auto);
}

/// 门关（caller=None）兜底归并的翻桶单向性（§4.4）：AI 条目（rule_id
/// 非空 + auto=false）被同 id 兜底信号命中 → 只刷统计（hits+1），措辞与
/// 桶位原样——不翻回模板桶挂「模板兜底」徽标撒谎。
#[test]
fn gate_closed_template_merge_never_flips_ai_entry_back() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        p,
        serde_json::to_string_pretty(&serde_json::json!({
            "enabled_at": enabled_at,
            "habits": [
                {"category": "style", "content": "用户爱用两三个字下指令",
                 "confidence": 0.9, "hits": 5, "status": "active", "auto": false,
                 "rule_id": "stat.brief"}
            ],
        }))
        .unwrap(),
    )
    .unwrap();
    seed_short_messages(ws);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    // caller=None：门关（无模型环境），兜底模板照旧落库。
    sweep_once(&ws.join("config.json"), ws, &handle, None).unwrap();

    let profile = store::load_profile(ws);
    // 12 条「继续」同时触发 stat.brief 与 stat.language 两族信号卡；
    // brief 与既有 AI 条目同 id 归并（不产生第二个同 id 条目），language
    // 照常落模板条目（门关行为对照）。
    let brief: Vec<_> = profile
        .habits
        .iter()
        .filter(|h| h.rule_id == crate::userprofile::mining::RULE_STAT_BRIEF)
        .collect();
    assert_eq!(brief.len(), 1, "同 id 归并，不产生重复条目");
    let h = brief[0];
    assert_eq!(
        h.content, "用户爱用两三个字下指令",
        "AI 措辞不被模板话术覆盖"
    );
    assert!(!h.auto, "auto 不翻回模板桶（翻桶单向性）");
    assert_eq!(h.hits, 6, "兜底信号命中正常刷计数");
    assert!(
        profile.habits.iter().any(|h| h.auto),
        "门关对照：language 兜底模板照常落库（auto=true）"
    );
}

/// source_kind 四态派生（批三 §4.5 单元面）：source 标记优先于 auto 位
/// （观察转正/外部回抛即便是 auto=true 仍按来源徽标——auto 位是固定产物
/// 标记，不是来源）；两者皆缺时 auto=true = 模板兜底、false = AI 总结。
#[test]
fn source_kind_four_way_derivation() {
    let mut h = store::HabitEntry::auto_entry("style", "x", 0.8, 1, "now");
    assert_eq!(
        super::surface::source_kind(&h),
        "template",
        "auto=true = 模板兜底"
    );
    h.auto = false;
    assert_eq!(
        super::surface::source_kind(&h),
        "ai",
        "auto=false + source 空 = AI 总结"
    );
    h.source = "observation".into();
    assert_eq!(
        super::surface::source_kind(&h),
        "observation",
        "source 标记优先于 auto 位"
    );
    h.auto = true;
    assert_eq!(super::surface::source_kind(&h), "observation");
    h.source = "learn_dialogue".into();
    assert_eq!(
        super::surface::source_kind(&h),
        "external",
        "未知标记归外部"
    );
}

/// UAT 场景三（方案 §七）：中段断模型——门开首拍反思失败 = 信号丢弃、
// 兜底不落库、反思计数器不清零（失败重置会让永不成功的反思通道永远
// 够不到 20-sweep 闸）；次拍（同批消息仍在滚动 24h 证据窗内）反思成功
// = AI 主通道恢复，点名 stat.brief 新插 AI 条目并认领归并位。
#[test]
fn gate_open_reflection_failure_drops_signals_then_retries_next_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    seed_reflect_anchor(ws);
    seed_short_messages(ws);
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FlakyCaller {
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    // 第一拍：反思 Err（断模型）。
    let first = sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    let profile = store::load_profile(ws);
    assert!(
        profile.habits.is_empty(),
        "门开 + 反思失败 = 信号已消费不落兜底条目"
    );
    assert_eq!(
        profile.sweeps_since_reflect, 4,
        "计数器不清零（快照 3 + 本拍 1）——失败不清计数，下拍仍可触发"
    );
    assert!(profile.last_reflect_ms.is_none(), "反思水线不推进");
    // 反思失败留痕（横扫 2026-11-26）：手动触发（reflect_now）据此诚实
    // 上抛，不伪装「无新内容」。对话 scope 命中断模型，项目 scope 无新
    // 消息不进反思 = None。
    assert_eq!(
        first.dialogue.reflect_error.as_deref(),
        Some("模拟断模型"),
        "对话反思失败必须留痕上抛"
    );
    assert!(
        first.project.reflect_error.is_none(),
        "项目 scope 无反思调用 = 无错误留痕"
    );

    // 第二拍：同批消息仍在滚动 24h 证据窗内被重扫，反思成功。
    sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    let profile = store::load_profile(ws);
    assert_eq!(profile.habits.len(), 1, "AI 主通道恢复，仍无模板落库");
    let h = &profile.habits[0];
    assert_eq!(h.content, "用户爱用两三个字下指令");
    assert_eq!(
        h.rule_id,
        crate::userprofile::mining::RULE_STAT_BRIEF,
        "点名 id 无既有条目 = NoHit 新插 AI 条目并认领归并位"
    );
    assert!(!h.auto);
    assert_eq!(profile.sweeps_since_reflect, 0, "成功反思归零");
    assert!(profile.last_reflect_ms.is_some(), "反思水线推进");
}

// ---------- 双 scope sweep（批二 D4/D5/D6，2026-11-25） ----------

use super::store::ProfileScope;

/// 种双 scope 画像锚点：两份画像文件各写 enabled_at（1h 前）+ 计数器 0。
fn seed_dual_anchors(ws: &Path) {
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    for scope in [ProfileScope::Dialogue, ProfileScope::Project] {
        let profile = serde_json::json!({
            "enabled_at": enabled_at,
            "sweeps_since_reflect": 0,
        });
        let p = store::profile_path_scope(ws, scope);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::to_string_pretty(&profile).unwrap()).unwrap();
    }
}

/// 种会话：`project` 为真时摆 sidecar meta（project_id="1"）。
fn seed_session(ws: &Path, name: &str, project: bool, messages: &[&str]) {
    let ts = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
    let dir = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for content in messages {
        let row = serde_json::json!({"role": "user", "content": content, "timestamp": ts});
        body.push_str(&serde_json::to_string(&row).unwrap());
        body.push('\n');
    }
    std::fs::write(dir.join(format!("{name}.jsonl")), body).unwrap();
    if project {
        std::fs::write(
            dir.join(format!("{name}.meta.json")),
            r#"{"project_id": "1"}"#,
        )
        .unwrap();
    }
}

/// sweep 分流（D5 编排级回归锁）：对话/项目会话行互不串流——纠正确认度
/// 挖掘器 hits 恰等于本 scope 行数（3），串流会让任一侧变 6；两份画像
/// 水线独立推进。
#[test]
fn sweep_once_scope_split_routes_rows_and_watermarks() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    seed_dual_anchors(ws);
    seed_session(
        ws,
        "sess-d",
        false,
        &["不对，按钮应是蓝色", "不对，间距太窄", "不要用红色主题"],
    );
    seed_session(
        ws,
        "sess-p",
        true,
        &["我意思是改成竖排", "不，先做后端", "别动前端文件"],
    );
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let processed = sweep_once(&ws.join("config.json"), ws, &handle, None).unwrap();
    assert_eq!(
        processed.total_user_messages(),
        6,
        "双 scope 各 3 条用户消息"
    );

    let dialogue = store::load_profile_scope(ws, ProfileScope::Dialogue);
    let project = store::load_profile_scope(ws, ProfileScope::Project);
    fn correction(p: &store::UserProfile) -> &store::HabitEntry {
        p.habits
            .iter()
            .find(|h| h.rule_id == crate::userprofile::mining::RULE_STAT_CORRECTION)
            .unwrap_or_else(|| panic!("纠正确认度条目缺席"))
    }
    let d = correction(&dialogue);
    let pj = correction(&project);
    assert_eq!(d.hits, 3, "对话画像只吃到对话行（串流会变 6）");
    assert_eq!(pj.hits, 3, "项目画像只吃到项目行（串流会变 6）");
    // 水线独立推进（各自文件各自字段）。
    assert!(dialogue.last_sweep_ms.is_some(), "对话水线推进");
    assert!(project.last_sweep_ms.is_some(), "项目水线推进");
    assert_eq!(dialogue.sweeps_since_reflect, 1);
    assert_eq!(project.sweeps_since_reflect, 1);
}

/// 批次〇（2026-11-26 存量迁移）：项目文件缺席 + 对话画像已锚 = 项目
/// sweep 首拍自动补锚（enabled_at 取对话锚点值，幂等仅向前），且**本拍
/// 直接继续采集**（不白等下一拍）——项目行当拍进项目画像。
#[test]
fn sweep_once_backfills_project_anchor_and_collects_same_tick() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    // 只种对话锚点（单 scope 时代部署形态：项目文件不存在）。
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let profile = serde_json::json!({ "enabled_at": enabled_at, "sweeps_since_reflect": 0 });
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, serde_json::to_string_pretty(&profile).unwrap()).unwrap();
    seed_session(ws, "sess-d", false, &["你好", "帮我看看日志", "谢谢"]);
    seed_session(ws, "sess-p", true, &["你好", "帮我改配置"]);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let processed = sweep_once(&ws.join("config.json"), ws, &handle, None).unwrap();
    assert_eq!(
        processed.total_user_messages(),
        5,
        "补锚后本拍项目行直接采集（对话 3 + 项目 2）"
    );
    // 项目文件被创建 + 锚点取对话锚点值 + 项目水线本拍推进。
    let project = store::load_profile_scope(ws, ProfileScope::Project);
    assert!(
        project.enabled_at.is_some(),
        "补锚写入 enabled_at（存量迁移）"
    );
    assert!(project.last_sweep_ms.is_some(), "项目水线本拍推进");
    assert_eq!(processed.project.user_messages, 2, "项目 scope 吃到项目行");
    assert_eq!(processed.dialogue.user_messages, 3);
}

/// 批次〇反向臂：对话画像也未锚（用户从未开启）= 不补锚——项目文件不
/// 凭空创建、行不采集（老语义保留：no-op 的判定锚在对话画像）。
#[test]
fn sweep_once_no_dialogue_anchor_keeps_project_noop() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    seed_session(ws, "sess-p", true, &["你好", "帮我改配置"]);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let processed = sweep_once(&ws.join("config.json"), ws, &handle, None).unwrap();
    assert_eq!(processed.total_user_messages(), 0, "对话未锚 = 全 no-op");
    assert!(
        !store::profile_path_scope(ws, ProfileScope::Project).exists(),
        "不凭空创建项目画像文件"
    );
    assert!(
        !store::profile_path(ws).exists(),
        "对话画像文件也不凭空创建"
    );
}

/// 批次〇损坏臂：项目画像文件损坏（非法 JSON）= 项目 scope sweep 诚实
/// Err（编排层 scope 隔离 warn），对话 scope 照常跑不陪葬；损坏文件不回写。
#[test]
fn sweep_once_corrupted_project_profile_is_scope_isolated_error() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        p,
        serde_json::to_string_pretty(&serde_json::json!({ "enabled_at": enabled_at })).unwrap(),
    )
    .unwrap();
    let ppath = store::profile_path_scope(ws, ProfileScope::Project);
    std::fs::write(&ppath, "{corrupt").unwrap();
    seed_session(ws, "sess-d", false, &["你好", "帮我看看日志", "谢谢"]);
    seed_session(ws, "sess-p", true, &["你好", "帮我改配置"]);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    let processed = sweep_once(&ws.join("config.json"), ws, &handle, None).unwrap();
    assert_eq!(
        processed.total_user_messages(),
        3,
        "项目 scope 失败被隔离（计 0），对话 scope 照常"
    );
    assert!(
        store::load_profile_scope(ws, ProfileScope::Dialogue)
            .last_sweep_ms
            .is_some()
    );
    // 损坏文件保持原样（Err 路径零回写）。
    assert_eq!(std::fs::read_to_string(&ppath).unwrap(), "{corrupt");
}

/// 全局信号源只进对话 scope（D6 回归锁）：todo 快照（全局数据，无会话
/// 归属）在对话画像落条目，项目画像零污染——项目会话行是中性消息、自身
/// 无任何可挖信号。
#[test]
fn sweep_once_global_signals_stay_in_dialogue_scope() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    seed_dual_anchors(ws);
    // 项目会话：单条中性消息（n<10 触发不了任何统计规则）。
    seed_session(ws, "sess-p", true, &["你好"]);
    // 两个 todo 快照（≥TODO_MIN_SESSIONS=2），各 3 步且最后两步是测试类
    // （「做好测试了么」仪式）。
    let todos = serde_json::json!([
        {"content": "实现功能", "status": "completed"},
        {"content": "跑测试", "status": "completed"},
        {"content": "验证结果", "status": "pending"}
    ]);
    let sessions_dir = nemesis_path::resolve_sessions_dir_in_workspace(ws);
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::write(
        sessions_dir.join("todo_a.json"),
        serde_json::to_string(&todos).unwrap(),
    )
    .unwrap();
    std::fs::write(
        sessions_dir.join("todo_b.json"),
        serde_json::to_string(&todos).unwrap(),
    )
    .unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    sweep_once(&ws.join("config.json"), ws, &handle, None).unwrap();

    let dialogue = store::load_profile_scope(ws, ProfileScope::Dialogue);
    assert!(
        dialogue.habits.iter().any(|h| h.category == "quality_bar"),
        "全局 todo 信号进对话画像"
    );
    let project = store::load_profile_scope(ws, ProfileScope::Project);
    assert!(project.habits.is_empty(), "全局信号源不得泄漏进项目画像");
    assert!(project.last_sweep_ms.is_some(), "项目 scope 照常推进水线");
}

/// BUG-20261126-131 回归锁：mining 日键必须走**本地**墙钟——三个跨天阈值
/// 闸（antecedent/route/retry 的 *_MIN_DAYS）旧实现从 epoch 直取 UTC 日期，
/// 非 UTC 用户被平移时区差（BUG-20261110-045 同族）：单个本地上午跨 UTC
/// 日界被拆成两个「天」（假触发跨天阈值），固定早间用户永远凑不满
/// MIN_DAYS（漏报）。摘掉 local_day_label 的 with_timezone(&Local) →
/// 非 UTC 机器上本测试红。边界构造对任意非零偏移成立：UTC 午夜 −1h 与
/// +30min 两时刻 UTC 日必不同、本地日必同。UTC 机器（offset==0）两形态
/// 输出相同——强断言无区分力，诚实早退（CI=UTC 只弱锁意图）。
#[test]
fn mining_day_label_uses_local_wall_clock() {
    use chrono::TimeZone as _;
    let now = chrono::Utc::now();
    let offset_secs = chrono::Local
        .offset_from_utc_datetime(&now.naive_utc())
        .local_minus_utc();
    // 契约直断言：标签 == 该 epoch 的本地日历日（同一 instant 派生，无
    // 午夜竞态；chrono Local 是「本地日历日」的权威定义）。
    assert_eq!(
        super::mining::local_day_label(now.timestamp_millis()),
        now.with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string()
    );
    if offset_secs == 0 {
        return; // UTC 机器：Local==Utc，边界构造无区分力（诚实退化）
    }
    let utc_midnight = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Utc)
        .unwrap();
    let ts1 = (utc_midnight - chrono::Duration::hours(1)).timestamp_millis();
    let ts2 = (utc_midnight + chrono::Duration::minutes(30)).timestamp_millis();
    assert_eq!(
        super::mining::local_day_label(ts1),
        super::mining::local_day_label(ts2),
        "同一本地日的两个时刻（UTC 日不同）必须产出同一日标签"
    );
}
// ---------- 批次一：force_reflect 手动触发（2026-11-26） ----------

/// 捕获 prompt + 固定响应（批次一/三测试用）。
struct CaptureAndRespond {
    prompts: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    resp: String,
}

#[async_trait::async_trait]
impl LLMCaller for CaptureAndRespond {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        self.prompts.lock().unwrap().push(_user_prompt.to_string());
        Ok(self.resp.clone())
    }
}

/// force 放宽触发闸（不放宽空窗闸）：闸关死（计数 4 < 20 / 消息 12 < 25）
/// 时普通拍零反思调用；force 拍门开，反思恰好一次 + 计数归零 + 水线推进。
#[test]
fn sweep_once_opts_force_reflect_bypasses_trigger_gate() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true, "reflect_min_new_messages": 25}}"#,
    )
    .unwrap();
    seed_reflect_anchor(ws); // 计数器 3
    seed_short_messages(ws); // 12 条 < 25
    let prompts: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(CaptureAndRespond {
        prompts: prompts.clone(),
        resp: "[]".to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    // 普通拍：闸关死 → 零反思调用。
    sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    assert!(prompts.lock().unwrap().is_empty(), "未到触发闸不反思");
    assert_eq!(store::load_profile(ws).sweeps_since_reflect, 4, "计数器 +1");

    // force 拍：同批消息仍在滚动证据窗内，门开（force || should_reflect）。
    let opts = SweepOpts {
        force_reflect: true,
    };
    let stats = sweep_once_opts(&ws.join("config.json"), ws, &handle, Some(&caller), opts).unwrap();
    assert_eq!(prompts.lock().unwrap().len(), 1, "force 绕过触发闸");
    let profile = store::load_profile(ws);
    assert_eq!(profile.sweeps_since_reflect, 0, "反思成功计数归零");
    assert!(profile.last_reflect_ms.is_some(), "反思水线推进");
    assert_eq!(stats.dialogue.user_messages, 12, "证据窗整窗采集");
}

/// force 不放宽空窗判定：反思水线刚推进（reflect_rows 空）→ force 也不
/// 调 LLM（诚实空跑，不造空反思）。
#[test]
fn sweep_once_opts_force_reflect_respects_empty_window() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(
        ws.join("config.json"),
        r#"{"user_profile": {"enabled": true}}"#,
    )
    .unwrap();
    // 锚点 + last_reflect_ms = now（水线刚推进，窗内消息全在反思线之前）。
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        p,
        serde_json::to_string_pretty(&serde_json::json!({
            "enabled_at": enabled_at,
            "sweeps_since_reflect": 3,
            "last_reflect_ms": chrono::Utc::now().timestamp_millis(),
        }))
        .unwrap(),
    )
    .unwrap();
    seed_session(ws, "sess-d", false, &["你好", "帮我看看日志"]);
    let prompts: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(CaptureAndRespond {
        prompts: prompts.clone(),
        resp: "[]".to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let opts = SweepOpts {
        force_reflect: true,
    };
    let stats = sweep_once_opts(&ws.join("config.json"), ws, &handle, Some(&caller), opts).unwrap();
    assert!(prompts.lock().unwrap().is_empty(), "空窗 force 也不反思");
    assert_eq!(stats.dialogue.user_messages, 2, "采集照常，反思不跑");
}

// ---------- 批次三：反思结论 scope 分型路由（2026-11-26） ----------

/// 纯 AI 结论（无点名）提案 scope=project → 对话反思里落**项目画像文件**
/// （含跨文件），ai_scope_routed 计 1；对话画像零条目。
#[test]
fn sweep_once_pure_ai_project_conclusion_crosses_scope() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    seed_reflect_anchor(ws);
    seed_session(ws, "sess-a", false, &["你好", "帮我看看日志"]);
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FixedCaller {
        resp: r#"[{"kind": "habit", "category": "workflow",
                   "content": "项目协作规范：先对齐再动手", "confidence": 0.9,
                   "scope": "project"}]"#
            .to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let stats = sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    let dialogue = store::load_profile_scope(ws, ProfileScope::Dialogue);
    assert!(dialogue.habits.is_empty(), "跨文件结论不落对话画像");
    let project = store::load_profile_scope(ws, ProfileScope::Project);
    assert_eq!(
        project.habits.len(),
        1,
        "纯 AI 新插按 target_scope 落项目画像"
    );
    assert_eq!(project.habits[0].content, "项目协作规范：先对齐再动手");
    assert!(!project.habits[0].auto, "反思产物 auto=false");
    assert_eq!(
        stats.dialogue.ai_scope_routed, 1,
        "跨文件计 ai_scope_routed"
    );
    assert_eq!(stats.dialogue.habits, 1, "落谁家都算本次反思产出");
}

/// 对侧画像读取失败 = 跨文件路由降级为本 scope-only（批次三③兜底语义，
/// 代码注释明文承诺「不炸本拍已拿到的 AI 结论」）：项目画像文件损坏 +
/// 对话反思产出 scope=project 纯 AI 结论 → 反思照常完成，结论降级落
/// 对话画像（位置兜底），ai_scope_routed 不计（没真跨成文件）；项目
/// scope 自身损坏 = Err 被 scope 隔离（stats.project 全零且无反思错误
/// ——项目 sweep 在快照读取就失败，没进到反思）。
#[test]
fn sweep_once_cross_scope_degrades_to_local_when_other_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    seed_reflect_anchor(ws);
    seed_session(ws, "sess-a", false, &["你好", "帮我看看日志"]);
    // 对侧（项目画像）损坏：读取失败 → other=None 全链降级。
    let ppath = store::profile_path_scope(ws, ProfileScope::Project);
    std::fs::create_dir_all(ppath.parent().unwrap()).unwrap();
    std::fs::write(&ppath, "{corrupt").unwrap();
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FixedCaller {
        resp: r#"[{"kind": "habit", "category": "workflow",
                   "content": "项目协作规范：先对齐再动手", "confidence": 0.9,
                   "scope": "project"}]"#
            .to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let stats = sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    // 对话反思照常产出（不炸），结论降级落对话画像。
    let dialogue = store::load_profile_scope(ws, ProfileScope::Dialogue);
    assert_eq!(
        dialogue.habits.len(),
        1,
        "对侧缺席 = 降级落本 scope（位置兜底），结论不丢"
    );
    assert_eq!(dialogue.habits[0].content, "项目协作规范：先对齐再动手");
    assert_eq!(
        stats.dialogue.ai_scope_routed, 0,
        "没真跨成文件，不计跨文件路由"
    );
    assert_eq!(stats.dialogue.habits, 1);
    assert!(
        stats.dialogue.reflect_error.is_none(),
        "反思本身成功，无错误留痕"
    );
    // 项目 scope 自身损坏 = Err 被隔离（全零 stats），损坏文件零回写。
    assert_eq!(stats.project.user_messages, 0);
    assert!(stats.project.reflect_error.is_none());
    assert_eq!(std::fs::read_to_string(&ppath).unwrap(), "{corrupt");
}

/// 点名认领插入跨文件（工作流主通道）：对话信号卡 id + AI 提案
/// scope=project → 对话画像无可改写目标（NoHit）→ 认领插入落项目画像，
/// rule_id 归并位认领在项目侧。
#[test]
fn sweep_once_claim_insert_routes_to_target_scope() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    seed_reflect_anchor(ws);
    seed_short_messages(ws); // stat.brief 信号卡
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FixedCaller {
        resp: r#"[{"kind": "habit", "category": "style",
                   "content": "用户爱用两三个字下指令", "confidence": 0.9,
                   "rule_ids": ["stat.brief"], "scope": "project"}]"#
            .to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let stats = sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    let dialogue = store::load_profile_scope(ws, ProfileScope::Dialogue);
    assert!(dialogue.habits.is_empty(), "认领插入不落对话画像");
    let project = store::load_profile_scope(ws, ProfileScope::Project);
    assert_eq!(project.habits.len(), 1);
    assert_eq!(
        project.habits[0].rule_id,
        crate::userprofile::mining::RULE_STAT_BRIEF,
        "rule_id 认领归并位随条目落项目侧"
    );
    assert_eq!(project.habits[0].content, "用户爱用两三个字下指令");
    assert_eq!(stats.dialogue.ai_scope_routed, 1);
}

/// 对侧已持有同 rule_id = 落对侧归并（不异地重建、不产生第二份）；
/// existing_rules 回灌含对侧条目措辞（AI 可见的合并上下文）。
#[test]
fn sweep_once_cross_scope_claim_merges_not_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    seed_reflect_anchor(ws);
    seed_short_messages(ws);
    // 项目画像已有 stat.brief 持有者（此前拍认领产物）。
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let ppath = store::profile_path_scope(ws, ProfileScope::Project);
    std::fs::create_dir_all(ppath.parent().unwrap()).unwrap();
    std::fs::write(
        &ppath,
        serde_json::to_string_pretty(&serde_json::json!({
            "enabled_at": enabled_at,
            "sweeps_since_reflect": 0,
            "habits": [
                {"category": "style", "content": "旧措辞：指令极短",
                 "confidence": 0.8, "hits": 5, "status": "active", "auto": false,
                 "rule_id": "stat.brief"}
            ],
        }))
        .unwrap(),
    )
    .unwrap();
    let prompts: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(CaptureAndRespond {
        prompts: prompts.clone(),
        resp: r#"[{"kind": "habit", "category": "style",
                   "content": "用户新措辞", "confidence": 0.9,
                   "rule_ids": ["stat.brief"], "scope": "project"}]"#
            .to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let stats = sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    let dialogue = store::load_profile_scope(ws, ProfileScope::Dialogue);
    assert!(dialogue.habits.is_empty(), "对话画像零条目");
    let project = store::load_profile_scope(ws, ProfileScope::Project);
    let brief: Vec<_> = project
        .habits
        .iter()
        .filter(|h| h.rule_id == crate::userprofile::mining::RULE_STAT_BRIEF)
        .collect();
    assert_eq!(brief.len(), 1, "落对侧归并，不异地重建第二份");
    assert_eq!(
        brief[0].content, "旧措辞：指令极短",
        "rule_id 持有者措辞保留"
    );
    assert_eq!(brief[0].hits, 6, "归并刷计数");
    assert_eq!(stats.dialogue.ai_scope_routed, 1);
    // existing_rules 回灌含对侧条目措辞（AI 可见合并上下文）。
    assert!(
        prompts.lock().unwrap()[0].contains("旧措辞：指令极短"),
        "对侧 rule_id 条目进反思 prompt"
    );
}

/// 批次三补口（复查 2026-11-26）：AI 点名**只存在于对侧**的 rule_id
/// （本拍信号卡不产、本 scope 也没有——仅 prompt 回灌可见）→ 必须通过
/// 有效 id 域走对侧归并（不异地重建第二份）。修复前该 id 被当编造过滤
/// 走纯 AI 插入 = 跨文件重复生成（防重复闭环漏半环）。
#[test]
fn sweep_once_other_scope_only_id_claim_merges_into_other() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    seed_reflect_anchor(ws);
    // 两条消息 n<10 → stat.brief 信号不触发（id 只存在于对侧画像）。
    seed_session(ws, "sess-a", false, &["你好", "帮我看看日志"]);
    // 项目画像已有 stat.brief 持有者（此前拍认领产物）。
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let ppath = store::profile_path_scope(ws, ProfileScope::Project);
    std::fs::create_dir_all(ppath.parent().unwrap()).unwrap();
    std::fs::write(
        &ppath,
        serde_json::to_string_pretty(&serde_json::json!({
            "enabled_at": enabled_at,
            "sweeps_since_reflect": 0,
            "habits": [
                {"category": "style", "content": "旧措辞：指令极短",
                 "confidence": 0.8, "hits": 5, "status": "active", "auto": false,
                 "rule_id": "stat.brief"}
            ],
        }))
        .unwrap(),
    )
    .unwrap();
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FixedCaller {
        resp: r#"[{"kind": "habit", "category": "style",
                   "content": "用户新措辞", "confidence": 0.9,
                   "rule_ids": ["stat.brief"], "scope": "project"}]"#
            .to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let stats = sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    let dialogue = store::load_profile_scope(ws, ProfileScope::Dialogue);
    assert!(dialogue.habits.is_empty(), "对侧归并不落本 scope");
    let project = store::load_profile_scope(ws, ProfileScope::Project);
    assert_eq!(
        project.habits.len(),
        1,
        "点名对侧 id 归并到持有者，不插第二份"
    );
    assert_eq!(
        project.habits[0].content, "旧措辞：指令极短",
        "rule_id 持有者措辞保留"
    );
    assert_eq!(project.habits[0].hits, 6, "归并刷计数");
    assert_eq!(stats.dialogue.ai_scope_routed, 1);
}

/// 改写恒留本 scope（路由规则①）：对话画像既有 stat.brief 持有者被点名
/// 且提案 scope=project → 原地改写留对话画像，条目不迁移。
#[test]
fn sweep_once_rewrite_never_migrates_across_scope() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_gate_config(ws);
    // 对话画像：锚点 + 计数器 3 + stat.brief 既有条目。
    let enabled_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let p = store::profile_path(ws);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        p,
        serde_json::to_string_pretty(&serde_json::json!({
            "enabled_at": enabled_at,
            "sweeps_since_reflect": 3,
            "habits": [
                {"category": "style", "content": "旧措辞：指令极短",
                 "confidence": 0.9, "hits": 5, "status": "active", "auto": false,
                 "rule_id": "stat.brief"}
            ],
        }))
        .unwrap(),
    )
    .unwrap();
    seed_short_messages(ws);
    let caller: std::sync::Arc<dyn LLMCaller> = std::sync::Arc::new(FixedCaller {
        resp: r#"[{"kind": "habit", "category": "style",
                   "content": "改写后的措辞", "confidence": 0.9,
                   "rule_ids": ["stat.brief"], "scope": "project"}]"#
            .to_string(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();

    let stats = sweep_once(&ws.join("config.json"), ws, &handle, Some(&caller)).unwrap();
    let dialogue = store::load_profile_scope(ws, ProfileScope::Dialogue);
    assert_eq!(dialogue.habits.len(), 1, "Rewrote 原地改写恒留本 scope");
    assert_eq!(dialogue.habits[0].content, "改写后的措辞");
    assert!(!dialogue.habits[0].auto);
    let project = store::load_profile_scope(ws, ProfileScope::Project);
    assert!(project.habits.is_empty(), "条目不迁移进项目画像");
    assert_eq!(stats.dialogue.ai_scope_routed, 0, "原地改写不算跨文件");
}
