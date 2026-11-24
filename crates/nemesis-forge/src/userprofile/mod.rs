//! 用户习惯学习子系统（Forge 化重构 2026-11-24；与 Forge 自学习组成学习
//! 系统族——Forge 学 LLM 错误做法，userprofile 学用户习惯）。
//!
//! 模块分工（扁平文件形态，比 PLAN 目录树少一层——实施时按规模裁定）：
//! - [`store`]：画像存储 + 条目 schema（steps/scope/evidence/auto）+ 合并
//!   /衰减/分桶截断/渲染/否决语义；
//! - [`collection`]：session_logs 结构化行（含批 C 富投影：文件种类/
//!   中间轮叙述/通道/模型名）+ todo 快照 + 负反馈事件 IO + 交互强信号
//!   IO（signals.jsonl）；
//! - [`mining`]：确定性挖掘（统计规则/仪式序列/todo 收尾/遥测前因），
//!   零 LLM 成本；
//! - [`reflection`]：LLM 反思（序列感知 digest + ledger 知识复利，G1/G2/G3）；
//! - [`surface`]：veto/forget/pin/enable/list API（web/CLI 共源）；
//! - [`telemetry`]：WSAPI 调用流落盘（UI 行为学习数据源，G6/例 2）。
//!
//! 编排 = [`sweep_once`]（自 nemesisbot/profile_service.rs 迁入，bot 侧
//! 只留 `ensure_sweep_spawned` 薄壳 + LLM caller 解析——forge 无
//! nemesis-data 依赖，caller 由 shim 经 `resolve_judge_caller` 传入）。

pub mod bridge;
pub mod collection;
pub mod mining;
pub mod reflection;
pub mod store;
pub mod surface;
pub mod telemetry;

use std::path::Path;

use collection::DialogueRow;

/// todo 快照新鲜度（7 天窗：陈年死会话的 todo 不参与习惯挖掘，足够覆盖
/// 周末间隔）。
pub(crate) const TODO_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// ledger 回灌条数（反思 prompt 的「此前已归纳」上下文）。
const LEDGER_CONTEXT_LIMIT: usize = 10;
/// 负反馈事件回灌条数。
const EVENT_CONTEXT_LIMIT: usize = 10;

/// 单次 sweep：开关关 = no-op。返回本 tick 处理的用户消息数（诊断用）。
///
/// 两段式（四路评审 B- 根修，随迁保留）：候选收集（确定性挖掘 + LLM
/// 反思，含 30s+ 墙钟的 LLM 调用）全程**锁外**——`config_write_lock` 是
/// 进程级互斥锁，持锁跨 LLM 会把 dashboard config 保存整段卡死；收集到的
/// 候选在**短临界区**（加锁、fresh load、upsert、衰减、分桶截断、水线
/// 写回、落盘）内落到锁内新读的画像上——锁外修改的本地副本不回写，锁内
/// fresh load 消灭 RMW 读改写窗口。
pub fn sweep_once(
    cfg_path: &Path,
    workspace: &Path,
    rt: &tokio::runtime::Handle,
    caller: Option<&std::sync::Arc<dyn crate::reflector_llm::LLMCaller>>,
) -> Result<usize, String> {
    let cfg = nemesis_config::load_config(cfg_path).map_err(|e| format!("load config: {e}"))?;
    let Some(up_cfg) = cfg.user_profile.as_ref() else {
        return Ok(0);
    };
    if !up_cfg.enabled {
        return Ok(0);
    }
    // 锁外只读基准（仅用于证据窗/触发判定；写路径见下方短临界区）。
    let snapshot = store::try_load_profile(workspace)?;
    // 增量采集：证据窗 = 滚动 24h（`sweep_window_start_ms`：锚点夹紧 +
    // 水线停机下探）。统计规则的 n≥10 闸需要足够大的样本窗，小时级增量
    // 批对稀疏用户永远凑不齐。
    let collect_started_ms = chrono::Utc::now().timestamp_millis();
    let Some(window_start) = store::sweep_window_start_ms(&snapshot, collect_started_ms) else {
        return Ok(0); // 从未开启或锚点损坏 = 不采集
    };
    let rows = collection::collect_dialogue_after(workspace, window_start)?;
    let user_messages: Vec<String> = rows
        .iter()
        .filter(|r| r.role == "user")
        .map(|r| r.content.clone())
        .collect();

    // ---- 候选收集（锁外）----
    let now = chrono::Local::now().to_rfc3339();
    let mut candidates = Vec::new();
    // 确定性挖掘（零 LLM）：统计规则（行为不变）+ 仪式序列（例 1）+
    // todo 收尾（例 1 第 3 步）+ 批 B 对话结构挖掘（开场白/催促/分工/
    // 返工节奏——全是对话行纯函数）。
    candidates.extend(mining::stat_candidates(&user_messages, &now));
    candidates.extend(mining::ritual_candidates(&rows, &now));
    candidates.extend(mining::opener_candidates(&rows, &now));
    candidates.extend(mining::urge_candidates(&rows, &now));
    candidates.extend(mining::delegation_candidates(&rows, &now));
    // 返工节奏（批 B ⑨，rewind 用户读法）：rewind 计数 = events.jsonl 尾部
    // rewind 行；会话数 = 证据窗内出现过的会话数。
    let rewind_count = collection::read_recent_events(workspace, 50)
        .iter()
        .filter(|ev| ev.get("kind").and_then(|k| k.as_str()) == Some("rewind"))
        .count();
    let session_count = rows
        .iter()
        .map(|r| r.session.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    candidates.extend(mining::rework_candidates(rewind_count, session_count, &now));
    // 批 C 挖掘面：工作域（文件接触种类）+ 模型切换行为——对话行纯函数；
    // 审批强信号读 signals.jsonl（与 events.jsonl 同族旁路面，不设遥测闸）。
    candidates.extend(mining::work_domain_candidates(&rows, &now));
    candidates.extend(mining::model_switch_candidates(&rows, &now));
    candidates.extend(mining::approval_strong_candidates(
        &collection::read_recent_signals(workspace, 200),
        &now,
    ));
    match collection::collect_todos(workspace, collect_started_ms, TODO_MAX_AGE_MS) {
        Ok(todos) => candidates.extend(mining::todo_candidates(&todos, &now)),
        Err(e) => tracing::warn!("[UserProfile] todo collect failed: {e}"),
    }
    // 遥测挖掘（例 2 + 批 B：页面足迹链/障碍处置顺序/审批弱信号）：仅遥测
    // 开着才有新鲜数据可挖。
    if up_cfg.ui_telemetry_enabled_or(true) {
        let usage = mining::read_usage_rows(workspace);
        candidates.extend(mining::antecedent_candidates(&usage, &now));
        candidates.extend(mining::route_chain_candidates(&usage, &now));
        candidates.extend(mining::retry_candidates(&usage, &now));
        candidates.extend(mining::approval_signal_candidates(&usage, &now));
    }

    // LLM 反思通道（G1 重构）：持久化计数器到顶 **或** 窗内新增用户消息
    // ≥ min_new 触发；反思输入 = last_reflect_ms 之后的行（fallback 全窗）。
    // 失败/跳过**不清计数器**（计数器只表达「距上次成功反思的 sweep 数」
    // ——失败重置会让永不成功的反思通道永远够不到 20-sweep 闸）。
    let reflect_window_start = snapshot.last_reflect_ms.unwrap_or(i64::MIN);
    let reflect_rows: Vec<DialogueRow> = rows
        .iter()
        .filter(|r| r.ts_ms >= reflect_window_start)
        .cloned()
        .collect();
    let new_user_msgs = reflect_rows.iter().filter(|r| r.role == "user").count();
    let mut reflect_done = false;
    if !reflect_rows.is_empty()
        && reflection::should_reflect(
            snapshot.sweeps_since_reflect,
            up_cfg.reflect_every_sweeps_or(reflection::REFLECT_EVERY_SWEEPS_DEFAULT),
            new_user_msgs,
            up_cfg.reflect_min_new_messages_or(reflection::REFLECT_MIN_NEW_MESSAGES_DEFAULT),
        )
        && let Some(caller) = caller
    {
        let priors = reflection::read_recent_conclusions(workspace, LEDGER_CONTEXT_LIMIT);
        let events = collection::read_recent_events(workspace, EVENT_CONTEXT_LIMIT);
        match reflection::reflect_semantic_habits(
            caller,
            &reflect_rows,
            &priors,
            &events,
            rt,
            up_cfg.confidence_threshold_or(store::CONFIDENCE_THRESHOLD),
        ) {
            // 批 A 主语分型路由：habit → 画像 upsert；lesson（助手/LLM
            // 教训）→ bridge 队列落盘（Forge 未来消费，**不丢弃**也不入
            // 画像——upsert 双保险兜底）。两类结论都进 ledger（知识复利
            // 回灌，教训不每轮重提）。
            Ok((habits, lessons)) => {
                let mut conclusions: Vec<String> =
                    habits.iter().map(|e| e.content.clone()).collect();
                conclusions.extend(lessons.iter().map(|l| l.content.clone()));
                reflection::append_ledger(workspace, &conclusions);
                candidates.extend(habits);
                if !lessons.is_empty() {
                    bridge::append_lessons(workspace, &lessons);
                }
                reflect_done = true;
            }
            Err(e) => tracing::warn!("[UserProfile] reflect failed: {e}"),
        }
    }

    // ---- 短临界区（锁内）：fresh load 消灭读改写窗口；同一次原子写落盘，
    //      中途失败水线不推进，下轮重扫本轮消息——at-least-once 语义。----
    let _serial = nemesis_config::config_write_lock();
    let mut profile = store::try_load_profile(workspace)?;
    for c in candidates {
        store::upsert_habit(&mut profile, c);
    }
    store::decay_sweep_with_days(
        &mut profile,
        chrono::Local::now(),
        up_cfg.decay_days_or(store::DECAY_DAYS),
    );
    store::prune_and_cap_with_max(&mut profile, up_cfg.max_entries_or(store::MAX_ENTRIES));
    profile.last_sweep_ms = Some(collect_started_ms);
    profile.sweeps_since_reflect = if reflect_done {
        0
    } else {
        snapshot.sweeps_since_reflect.saturating_add(1)
    };
    if reflect_done {
        profile.last_reflect_ms = Some(collect_started_ms);
    }
    store::save_profile_locked(workspace, &profile)?;

    Ok(user_messages.len())
}

#[cfg(test)]
mod tests;
