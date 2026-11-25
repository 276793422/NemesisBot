//! 用户习惯学习子系统（Forge 化重构 2026-11-24；与 Forge 自学习组成学习
//! 系统族——Forge 学 LLM 错误做法，userprofile 学用户习惯）。
//!
//! 模块分工（扁平文件形态，比 PLAN 目录树少一层——实施时按规模裁定）：
//! - [`store`]：画像存储 + 条目 schema（steps/scope/evidence/auto）+ 合并
//!   /衰减/分桶截断/渲染/否决语义；
//! - [`collection`]：session_logs 结构化行（含批 C 富投影：文件种类/
//!   中间轮叙述/通道/模型名）+ todo 快照 + 负反馈事件 IO + 交互强信号
//!   IO（signals.jsonl）；
//! - [`mining`]：确定性信号挖掘（统计规则/仪式序列/todo 收尾/遥测前因），
//!   零 LLM 成本——挖掘器是信号生产者（SignalCard），条目化经兜底桥
//!   [`mining::signal_to_entry`]（话术 AI 化批一，2026-11-25）；
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

/// 单次 sweep：开关关 = no-op。返回本 tick 处理的用户消息数（诊断用，
/// 双 scope 之和）。
///
/// 双 scope 编排（2026-11-25 批二 D4）：对话与项目画像**各自独立**走一遍
/// 完整 sweep——独立证据窗（各自画像文件的 enabled_at / last_sweep_ms 水
/// 线）、独立行分流（collection D5 sidecar 判定）、独立反思门（各自
/// sweeps_since_reflect 计数）、独立临界区落盘。scope 隔离：单 scope 失败
/// warn 跳过，另一 scope 照常跑（学习侧旁路面，两个画像互不拖死）。
///
/// 全局信号源（D6）：todos / 终端命令 / 遥测 / signals.jsonl / events.jsonl
/// rewind 计数不随会话分流（无会话归属，无法判 scope），只进对话 scope
/// 信号流；项目 scope 只吃对话行纯函数挖掘器。教训/观察/ledger 三队列
/// （D7）保持全局单文件，两个 scope 的反思共用同一知识底座。
///
/// 两段式（四路评审 B- 根修，随迁保留）：信号收集（确定性信号挖掘 + LLM
/// 反思，含 30s+ 墙钟的 LLM 调用）全程**锁外**——`config_write_lock` 是
/// 进程级互斥锁，持锁跨 LLM 会把 dashboard config 保存整段卡死；收集到的
/// 信号在**短临界区**（加锁、fresh load、兜底桥 + upsert、衰减、分桶截
/// 断、水线写回、落盘）内落到锁内新读的画像上——锁外修改的本地副本不回
/// 写，锁内 fresh load 消灭 RMW 读改写窗口。
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
    // 顺序恒对话先、项目后（对话是既有主路径，行为不变；项目画像未开锚
    // 时在 sweep_window_start_ms 处自然 no-op）。
    let mut total = 0usize;
    for scope in [store::ProfileScope::Dialogue, store::ProfileScope::Project] {
        match sweep_scope_once(up_cfg, workspace, rt, caller, scope) {
            Ok(n) => total += n,
            Err(e) => tracing::warn!("[UserProfile] {scope:?} sweep failed (scope isolated): {e}"),
        }
    }
    Ok(total)
}

/// 单 scope 的完整 sweep（D4）：独立快照/证据窗/行流/反思门/临界区/水线。
fn sweep_scope_once(
    up_cfg: &nemesis_config::UserProfileConfig,
    workspace: &Path,
    rt: &tokio::runtime::Handle,
    caller: Option<&std::sync::Arc<dyn crate::reflector_llm::LLMCaller>>,
    scope: store::ProfileScope,
) -> Result<usize, String> {
    // 锁外只读基准（仅用于证据窗/触发判定；写路径见下方短临界区）。
    let snapshot = store::try_load_profile_scope(workspace, scope)?;
    // 增量采集：证据窗 = 滚动 24h（`sweep_window_start_ms`：锚点夹紧 +
    // 水线停机下探）。统计规则的 n≥10 闸需要足够大的样本窗，小时级增量
    // 批对稀疏用户永远凑不齐。从未开启或锚点损坏 = 不采集（项目画像未
    // 开锚时在此自然 no-op）。
    let collect_started_ms = chrono::Utc::now().timestamp_millis();
    let Some(window_start) = store::sweep_window_start_ms(&snapshot, collect_started_ms) else {
        return Ok(0); // 从未开启或锚点损坏 = 不采集
    };
    let rows = collection::collect_dialogue_after_scope(workspace, window_start, scope)?;
    let user_messages: Vec<String> = rows
        .iter()
        .filter(|r| r.role == "user")
        .map(|r| r.content.clone())
        .collect();

    // ---- 信号收集（锁外；挖掘器是信号生产者——门关时经兜底桥落库，门
    //      开时进反思 prompt 由 AI 自主措辞）----
    let now = chrono::Local::now().to_rfc3339();
    let mut signals = Vec::new();
    // 对话行纯函数挖掘器（双 scope 同款，行流已按 D5 分流）：统计规则
    // （行为不变）+ 仪式序列（例 1）+ 批 B 对话结构挖掘（开场白/催促/
    // 分工——全是对话行纯函数）+ 批 C 工作域/模型切换。
    signals.extend(mining::stat_candidates(&user_messages));
    signals.extend(mining::ritual_candidates(&rows));
    signals.extend(mining::opener_candidates(&rows));
    signals.extend(mining::urge_candidates(&rows));
    signals.extend(mining::delegation_candidates(&rows));
    signals.extend(mining::work_domain_candidates(&rows));
    signals.extend(mining::model_switch_candidates(&rows));
    // 全局信号源（D6，仅对话 scope）：todos / 终端 / 遥测 / signals.jsonl /
    // events rewind 无会话归属，不随会话分流。
    if scope == store::ProfileScope::Dialogue {
        // 返工节奏（批 B ⑨，rewind 用户读法）：rewind 计数 = events.jsonl
        // 尾部 rewind 行（全局文件）；会话数 = 对话行流内出现过的会话数。
        let rewind_count = collection::read_recent_events(workspace, 50)
            .iter()
            .filter(|ev| ev.get("kind").and_then(|k| k.as_str()) == Some("rewind"))
            .count();
        let session_count = rows
            .iter()
            .map(|r| r.session.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        signals.extend(mining::rework_candidates(rewind_count, session_count));
        // 审批强信号读 signals.jsonl（与 events.jsonl 同族旁路面，不设遥
        // 测闸）。
        signals.extend(mining::approval_strong_candidates(
            &collection::read_recent_signals(workspace, 200),
        ));
        // 终端会话命令挖掘（2026-11-24 T 批）：PTY 审计（logs/terminal）旁路
        // 读取——读取失败 warn 不炸 sweep。凭据防线在采集层（密码提示邻接行
        // 整行丢弃 + 秘密形态行整行丢弃 + ANSI 行跳过），挖掘器只见清洗后的
        // 命令行（shell 偏好 + 常用命令族，参数不采集）。
        match collection::collect_terminal_sessions(workspace, window_start) {
            Ok(terms) if !terms.is_empty() => {
                signals.extend(mining::shell_preference_candidates(&terms));
                signals.extend(mining::terminal_command_candidates(&terms));
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("[UserProfile] terminal collect failed: {e}"),
        }
        match collection::collect_todos(workspace, collect_started_ms, TODO_MAX_AGE_MS) {
            Ok(todos) => {
                signals.extend(mining::todo_candidates(&todos));
                signals.extend(mining::todo_sequence_candidates(&todos));
            }
            Err(e) => tracing::warn!("[UserProfile] todo collect failed: {e}"),
        }
        // 遥测挖掘（例 2 + 批 B：页面足迹链/障碍处置顺序/审批弱信号）：仅
        // 遥测开着才有新鲜数据可挖。
        if up_cfg.ui_telemetry_enabled_or(true) {
            let usage = mining::read_usage_rows(workspace);
            signals.extend(mining::antecedent_candidates(&usage));
            signals.extend(mining::route_chain_candidates(&usage));
            signals.extend(mining::retry_candidates(&usage));
            signals.extend(mining::approval_signal_candidates(&usage));
        }
    }

    // LLM 反思通道（G1 重构）：持久化计数器到顶 **或** 窗内新增用户消息
    // ≥ min_new 触发；反思输入 = last_reflect_ms 之后的行（fallback 全窗）。
    // 失败/跳过**不清计数器**（计数器只表达「距上次成功反思的 sweep 数」
    // ——失败重置会让永不成功的反思通道永远够不到 20-sweep 闸）。
    // 置信度阈值统一取口（复查 2026-11-24）：反思通道内部分档与下方收口
    // 闸共用同一值——用户配置覆盖两处同源生效。
    let confidence_threshold = up_cfg.confidence_threshold_or(store::CONFIDENCE_THRESHOLD);
    let reflect_window_start = snapshot.last_reflect_ms.unwrap_or(i64::MIN);
    let reflect_rows: Vec<DialogueRow> = rows
        .iter()
        .filter(|r| r.ts_ms >= reflect_window_start)
        .cloned()
        .collect();
    let new_user_msgs = reflect_rows.iter().filter(|r| r.role == "user").count();
    // 分流门（话术 AI 化批二 §4.2）：门开 = 信号卡只进反思 prompt（AI 自
    // 主措辞），兜底模板**本拍不落库**；门关（无 caller / 未到反思触发）
    // = 兜底模板照旧落库。门在挖掘产物消费前定性——杜绝「信号既不落库也
    // 没进 prompt」的中间态。
    let reflect_gate_open = !reflect_rows.is_empty()
        && caller.is_some()
        && reflection::should_reflect(
            snapshot.sweeps_since_reflect,
            up_cfg.reflect_every_sweeps_or(reflection::REFLECT_EVERY_SWEEPS_DEFAULT),
            new_user_msgs,
            up_cfg.reflect_min_new_messages_or(reflection::REFLECT_MIN_NEW_MESSAGES_DEFAULT),
        );
    let mut reflect_done = false;
    let mut reflection_habits = Vec::new();
    if reflect_gate_open {
        let caller = caller.expect("gate open 已含 caller 在场判定");
        // 既有规则化条目清单（prompt 回灌）：id + 现措辞；vetoed 不回灌
        // （否决条目不该被 AI 找回来重写）。
        let existing_rules: Vec<(String, String)> = snapshot
            .habits
            .iter()
            .filter(|h| !h.rule_id.is_empty() && h.status != store::STATUS_VETOED)
            .map(|h| (h.rule_id.clone(), h.content.clone()))
            .collect();
        let priors = reflection::read_recent_conclusions(workspace, LEDGER_CONTEXT_LIMIT);
        let events = collection::read_recent_events(workspace, EVENT_CONTEXT_LIMIT);
        match reflection::reflect_semantic_habits(
            caller,
            &reflect_rows,
            &priors,
            &events,
            rt,
            confidence_threshold,
            &signals,
            &existing_rules,
        ) {
            // 批 A 主语分型路由：habit → 画像 upsert；lesson（助手/LLM
            // 教训）→ bridge 队列落盘（Forge 未来消费，**不丢弃**也不入
            // 画像——upsert 双保险兜底）；观察提名（接口二·低于置信度闸
            // 的边缘带 habit 结论）→ bridge 观察队列，人工裁决。
            Ok((habits, lessons, observations)) => {
                // 观察提名先入队（有 cap——只把实际入队的写 ledger）。
                let mut admitted_observations: Vec<String> = Vec::new();
                if !observations.is_empty() {
                    let admitted = bridge::append_observations(workspace, &observations);
                    admitted_observations = observations
                        .iter()
                        .take(admitted)
                        .map(|o| o.content.clone())
                        .collect();
                    if admitted < observations.len() {
                        tracing::warn!(
                            "[UserProfile] observation queue full: {admitted}/{} admitted",
                            observations.len()
                        );
                    }
                }
                // 两类结论 + 已入队提名都进 ledger（知识复利回灌——教训/
                // 提名不每轮重提；被 cap 丢弃的提名不锁死，用户清完积压
                // 后同结论证据支持时可再提名）。
                let mut conclusions: Vec<String> =
                    habits.iter().map(|rh| rh.entry.content.clone()).collect();
                conclusions.extend(lessons.iter().map(|l| l.content.clone()));
                conclusions.extend(admitted_observations);
                reflection::append_ledger(workspace, &conclusions);
                reflection_habits.extend(habits);
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
    // 分流落地（话术 AI 化批二 §4.2/§4.3）：
    // - 门关：兜底桥照旧（信号卡 → 条目，auto=true + rule_id；steps 清洗
    //   与 evidence 截断在桥上做——行为与批一逐位一致）。
    // - 门开：信号卡已进反思 prompt（被消费），本拍**不落兜底条目**；AI
    //   结论按 rule_ids 三分（原地改写 / 新插 AI 条目 / 纯 AI）。反思失
    //   败 = 本拍信号丢弃——反思触发计数器不清零，下拍重试（at-least-once
    //   无损；信号证据窗滚动 24h，下拍重扫天然兜住）。
    // 置信度收口闸（复查 2026-11-24）：门关路径的桥接条目统一 ≥阈值才进
    // 画像（结构性防线：挖掘器公式调整低于阈值不会静默入画像）；AI 结论
    // 在解析层已按阈值分档（低于闸落观察队列，不重复收口）。
    let _serial = nemesis_config::config_write_lock();
    let mut profile = store::try_load_profile_scope(workspace, scope)?;
    if reflect_gate_open {
        // 有效 id 域：本拍信号卡 ∪ 画像既有 rule_id（锁外快照 = prompt
        // 构建时的域，口径一致）。LLM 点名域外 id = 编造，按纯 AI 结论
        // 处理；同拍已消费的 id 不二次消费（首条生效余者按纯 AI）。
        let valid_ids: std::collections::HashSet<&str> = signals
            .iter()
            .map(|c| c.rule_id.as_str())
            .chain(
                snapshot
                    .habits
                    .iter()
                    .filter(|h| !h.rule_id.is_empty())
                    .map(|h| h.rule_id.as_str()),
            )
            .collect();
        let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
        for rh in reflection_habits {
            let ids: Vec<String> = rh
                .rule_ids
                .iter()
                .filter(|id| valid_ids.contains(id.as_str()) && !claimed.contains(id.as_str()))
                .cloned()
                .collect();
            if ids.is_empty() {
                // 纯 AI 结论（无点名 / 点名全部无效或同拍已消费）。
                store::upsert_habit(&mut profile, rh.entry);
                continue;
            }
            match store::apply_reflected_rewrite(&mut profile, rh.entry.clone(), &ids, &now) {
                store::RewriteOutcome::Rewrote(done) => claimed.extend(done),
                store::RewriteOutcome::NoHit => {
                    // 点名 id 无可改写目标 → 新插 AI 条目（rule_id = 首个
                    // 点名 id 认领归并位，后续同 id 信号并进来）；全部
                    // 点名 id 认领。
                    let mut e = rh.entry;
                    e.rule_id = ids[0].clone();
                    store::upsert_habit(&mut profile, e);
                    claimed.extend(ids);
                }
                store::RewriteOutcome::Rejected => {
                    // 安全硬边界：不删不改不插（被吞条目原样保留）。
                    tracing::warn!("[UserProfile] 反思改写被安全边界拒绝，条目未落地");
                }
            }
        }
    } else {
        let mut entries: Vec<store::HabitEntry> = signals
            .iter()
            .map(|c| mining::signal_to_entry(c, &now))
            .collect();
        entries.retain(|c| c.confidence >= confidence_threshold);
        for c in entries {
            store::upsert_habit(&mut profile, c);
        }
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
    store::save_profile_locked_scope(workspace, scope, &profile)?;

    Ok(user_messages.len())
}

#[cfg(test)]
mod tests;
