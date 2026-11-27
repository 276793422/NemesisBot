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

/// sweep 选项（批次一，2026-11-26）。
#[derive(Debug, Clone, Default)]
pub struct SweepOpts {
    /// 手动触发（`profile.reflect_now`）：反思门放宽为
    /// `should_reflect(...) || force`。**空窗判定不放宽**——上次总结以来
    /// 零新对话时门照关（空跑 LLM 无意义，诚实空结果由调用方注记）。
    pub force_reflect: bool,
}

/// 单 scope 一拍的处理计数（批次一：sweep 返回值从 usize 升级，供
/// `profile.reflect_now` 响应展示）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SweepScopeStats {
    /// 本拍落库的习惯结论数（AI 结论含改写 + 兜底模板条目）。
    pub habits: usize,
    /// 本拍入队教训数（bridge lessons 队列）。
    pub lessons: usize,
    /// 本拍入队观察提名数（cap 后实收）。
    pub observations: usize,
    /// 本拍处理的新用户消息数（窗内 user 行）。
    pub user_messages: usize,
    /// 批次三：被路由到**对侧**画像文件的 AI 结论数（跨文件 upsert）。
    pub ai_scope_routed: usize,
    /// 本拍反思 LLM 调用失败的错误文本（None = 反思未失败或门未开）。
    /// 定时 sweep 只 warn（scope 隔离，下拍重试）；**手动触发**
    /// （`profile.reflect_now`）据此诚实上抛——用户点按钮要的是反思，
    /// 模型失败不能伪装成「无新内容，本次总结为空」（横扫 2026-11-26）。
    pub reflect_error: Option<String>,
}

/// 双 scope 一拍的总计数。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SweepStats {
    pub dialogue: SweepScopeStats,
    pub project: SweepScopeStats,
}

impl SweepStats {
    pub fn total_habits(&self) -> usize {
        self.dialogue.habits + self.project.habits
    }

    pub fn total_lessons(&self) -> usize {
        self.dialogue.lessons + self.project.lessons
    }

    pub fn total_observations(&self) -> usize {
        self.dialogue.observations + self.project.observations
    }

    pub fn total_user_messages(&self) -> usize {
        self.dialogue.user_messages + self.project.user_messages
    }
}

/// 单次 sweep：开关关 = no-op。返回本拍双 scope 处理计数（批次一：
/// usize → [`SweepStats`]，供 `profile.reflect_now` 响应展示；既有调用
/// 点零行为变化——对话先项目后、scope 隔离失败语义原样）。
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
) -> Result<SweepStats, String> {
    sweep_once_opts(cfg_path, workspace, rt, caller, SweepOpts::default())
}

/// [`sweep_once`] 的 opts 形态（批次一：手动触发经 `force_reflect` 绕过
/// 反思触发闸；编排层其余语义不变）。
pub fn sweep_once_opts(
    cfg_path: &Path,
    workspace: &Path,
    rt: &tokio::runtime::Handle,
    caller: Option<&std::sync::Arc<dyn crate::reflector_llm::LLMCaller>>,
    opts: SweepOpts,
) -> Result<SweepStats, String> {
    let cfg = nemesis_config::load_config(cfg_path).map_err(|e| format!("load config: {e}"))?;
    let Some(up_cfg) = cfg.user_profile.as_ref() else {
        return Ok(SweepStats::default());
    };
    if !up_cfg.enabled {
        return Ok(SweepStats::default());
    }
    // 顺序恒对话先、项目后（对话是既有主路径，行为不变；项目画像未开锚
    // 时经批次〇锚点自动回填链路——对话也未锚才保持 no-op）。
    let mut stats = SweepStats::default();
    for scope in [store::ProfileScope::Dialogue, store::ProfileScope::Project] {
        match sweep_scope_once(up_cfg, workspace, rt, caller, scope, &opts) {
            Ok(s) => {
                if scope == store::ProfileScope::Dialogue {
                    stats.dialogue = s;
                } else {
                    stats.project = s;
                }
            }
            Err(e) => tracing::warn!("[UserProfile] {scope:?} sweep failed (scope isolated): {e}"),
        }
    }
    Ok(stats)
}

/// 单 scope 的完整 sweep（D4）：独立快照/证据窗/行流/反思门/临界区/水线。
fn sweep_scope_once(
    up_cfg: &nemesis_config::UserProfileConfig,
    workspace: &Path,
    rt: &tokio::runtime::Handle,
    caller: Option<&std::sync::Arc<dyn crate::reflector_llm::LLMCaller>>,
    scope: store::ProfileScope,
    opts: &SweepOpts,
) -> Result<SweepScopeStats, String> {
    // 锁外只读基准（仅用于证据窗/触发判定；写路径见下方短临界区）。
    let snapshot = store::try_load_profile_scope(workspace, scope)?;
    // 增量采集：证据窗 = 滚动 24h（`sweep_window_start_ms`：锚点夹紧 +
    // 水线停机下探）。统计规则的 n≥10 闸需要足够大的样本窗，小时级增量
    // 批对稀疏用户永远凑不齐。从未开启或锚点损坏 = 不采集；项目 scope
    // 例外走批次〇锚点自动回填（对话已锚 = 补锚后本拍直接继续采集——
    // 存量迁移：单 scope 时代 enable 只锚对话文件，项目文件缺席曾被判
    // 永久 no-op）。
    let collect_started_ms = chrono::Utc::now().timestamp_millis();
    let (snapshot, window_start) = match store::sweep_window_start_ms(&snapshot, collect_started_ms)
    {
        Some(w) => (snapshot, w),
        None if scope == store::ProfileScope::Project => {
            let backfilled = {
                let _serial = nemesis_config::config_write_lock();
                store::backfill_project_anchor_locked(workspace)?
            };
            if !backfilled {
                // 对话也未锚 = 用户从未开启画像，不补（尊重）。
                return Ok(SweepScopeStats::default());
            }
            tracing::info!(
                "[UserProfile] 项目画像锚点自动回填（存量迁移：对话锚点 → 项目文件），本拍继续采集"
            );
            let refilled = store::try_load_profile_scope(workspace, scope)?;
            match store::sweep_window_start_ms(&refilled, collect_started_ms) {
                Some(w) => (refilled, w),
                None => return Ok(SweepScopeStats::default()),
            }
        }
        None => return Ok(SweepScopeStats::default()), // 从未开启或锚点损坏 = 不采集
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
    // 没进 prompt」的中间态。批次一：`opts.force_reflect`（手动触发）放宽
    // 触发闸——但**空窗判定不放宽**（`!reflect_rows.is_empty()` 仍在 force
    // 之外：上次总结以来零新对话时门照关，空跑 LLM 无意义）。
    let reflect_gate_open = !reflect_rows.is_empty()
        && caller.is_some()
        && (opts.force_reflect
            || reflection::should_reflect(
                snapshot.sweeps_since_reflect,
                up_cfg.reflect_every_sweeps_or(reflection::REFLECT_EVERY_SWEEPS_DEFAULT),
                new_user_msgs,
                up_cfg.reflect_min_new_messages_or(reflection::REFLECT_MIN_NEW_MESSAGES_DEFAULT),
            ));
    let mut reflect_done = false;
    let mut reflection_habits = Vec::new();
    let mut lessons_count = 0usize;
    let mut observations_count = 0usize;
    // 反思 LLM 失败留痕（横扫 2026-11-26）：定时 sweep 只 warn（scope 隔离、
    // 下拍重试）；手动触发（reflect_now）据此诚实上抛，不伪装「无新内容」。
    let mut reflect_error: Option<String> = None;
    // 批次三补口（复查 2026-11-26）：对侧回灌条目的 rule_id 同步进有效
    // id 域（prompt 侧半环的消费侧闭环）——prompt 明示「rule_ids 只能取
    // 自上文信号与既有条目给出的 id」，而既有条目自批次三起含对侧条目；
    // 域过滤若不认对侧 id，AI 点名即被当编造走纯 AI 插入，对侧归并分支
    // 对这类 id 永不可达 = 跨文件重复生成（防重复闭环漏半环）。域取
    // prompt 构建时快照（口径一致原则同本 scope 快照）；vetoed 不同步
    // （回灌不含 vetoed，AI 不该点被否决的 id）。对侧读取失败 = 空表
    // （回灌与域同步降级，只影响本拍跨文件归并，不炸反思）。
    let mut other_rule_ids: Vec<String> = Vec::new();
    if reflect_gate_open {
        let caller = caller.expect("gate open 已含 caller 在场判定");
        // 既有规则化条目清单（prompt 回灌）：id + 现措辞；vetoed 不回灌
        // （否决条目不该被 AI 找回来重写）。批次三：回灌扩展 = 本 scope ∪
        // 对侧画像 rule_id 条目——AI 见对侧已有同规则措辞 → 不再重 derive
        // （防重复闭环的 prompt 侧半环）。对侧文件读取失败 = 降级只用本
        // scope（跨文件上下文是增强不是硬依赖，不炸本拍反思）。
        let other_snapshot = match store::try_load_profile_scope(workspace, scope.other()) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    "[UserProfile] 对侧画像读取失败，existing_rules 回灌只用本 scope: {e}"
                );
                Default::default()
            }
        };
        other_rule_ids = other_snapshot
            .habits
            .iter()
            .filter(|h| !h.rule_id.is_empty() && h.status != store::STATUS_VETOED)
            .map(|h| h.rule_id.clone())
            .collect();
        let existing_rules: Vec<(String, String)> = snapshot
            .habits
            .iter()
            .chain(other_snapshot.habits.iter())
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
            scope,
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
                    observations_count = admitted;
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
                lessons_count = lessons.len();
                reflect_done = true;
            }
            Err(e) => {
                tracing::warn!("[UserProfile] reflect failed: {e}");
                reflect_error = Some(e);
            }
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
    // 批次三（语义分型路由，2026-11-26）：门开分支加载**对侧画像**
    // fresh 副本——AI 结论按 `target_scope` 跨文件落位：
    //   ① 原地改写（Rewrote）恒留本 scope（既有条目已在文件里，AI 重写
    //     措辞不改归属——静默迁移是风险面，存量条目归属修正走
    //     `profile.move_scope` 手动通道）；
    //   ② NoHit 认领插入与纯 AI 新插按 `target_scope` 落（含跨文件——
    //     workflow 类信号认领插入是项目习惯的主通道）；对侧已持有同
    //     rule_id → 落对侧归并（不异地重建——封住「跨文件认领/搬走后
    //     下拍重复生成」），本 scope 已持有（vetoed/pinned）→ 留本 scope
    //     归并（既有 NoHit 语义不变）；
    //   ③ 对侧画像读取失败 = 跨文件路由降级为本 scope-only（位置分型
    //     兜底语义），不炸本拍已拿到的 AI 结论。
    // 置信度收口闸（复查 2026-11-24）：门关路径的桥接条目统一 ≥阈值才进
    // 画像（结构性防线：挖掘器公式调整低于阈值不会静默入画像）；AI 结论
    // 在解析层已按阈值分档（低于闸落观察队列，不重复收口）。
    let _serial = nemesis_config::config_write_lock();
    let mut profile = store::try_load_profile_scope(workspace, scope)?;
    let mut other = if reflect_gate_open {
        match store::try_load_profile_scope(workspace, scope.other()) {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("[UserProfile] 对侧画像读取失败，跨 scope 路由降级为本 scope: {e}");
                None
            }
        }
    } else {
        None
    };
    let mut other_dirty = false;
    let mut ai_scope_routed = 0usize;
    let mut habits_landed = 0usize;
    if reflect_gate_open {
        // 有效 id 域：本拍信号卡 ∪ 画像既有 rule_id ∪ 对侧回灌 rule_id
        // （批次三补口，2026-11-26——锁外快照 = prompt 构建时的域，口径
        // 一致）。LLM 点名域外 id = 编造，按纯 AI 结论处理；同拍已消费
        // 的 id 不二次消费（首条生效余者按纯 AI）。
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
            .chain(other_rule_ids.iter().map(|s| s.as_str()))
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
                // 纯 AI 结论（无点名 / 点名全部无效或同拍已消费）：按
                // target_scope 落（批次三语义分型，含跨文件）；对侧画像
                // 缺席（读取失败）= 降级落本 scope（位置兜底）。
                if rh.target_scope != scope {
                    match other.as_mut() {
                        Some(o) => {
                            store::upsert_habit(o, rh.entry);
                            other_dirty = true;
                            ai_scope_routed += 1;
                        }
                        None => store::upsert_habit(&mut profile, rh.entry),
                    }
                } else {
                    store::upsert_habit(&mut profile, rh.entry);
                }
                habits_landed += 1;
                continue;
            }
            match store::apply_reflected_rewrite(&mut profile, rh.entry.clone(), &ids, &now) {
                store::RewriteOutcome::Rewrote(done) => {
                    // 原地改写恒留本 scope（批次三路由规则①）。
                    claimed.extend(done);
                    habits_landed += 1;
                }
                store::RewriteOutcome::NoHit => {
                    // 点名 id 无本 scope 可改写目标 → 认领插入（全新条
                    // 目）。落位裁决（批次三路由规则② + 防重复闭环）：
                    // 本 scope 已持有该 id（vetoed/pinned 持有者不算
                    // NoHit 例外）→ 留本 scope 归并（既有语义不变）；
                    // 对侧已持有 → 落对侧归并（不异地重建同 id 条目）；
                    // 两者皆无 → 按 target_scope 落（含跨文件）。
                    let held_here = profile.habits.iter().any(|h| h.rule_id == ids[0]);
                    let held_other = other
                        .as_ref()
                        .is_some_and(|o| o.habits.iter().any(|h| h.rule_id == ids[0]));
                    let land_other = if held_here {
                        false
                    } else {
                        held_other || rh.target_scope != scope
                    };
                    let mut e = rh.entry;
                    e.rule_id = ids[0].clone();
                    if land_other {
                        match other.as_mut() {
                            Some(o) => {
                                store::upsert_habit(o, e);
                                other_dirty = true;
                                ai_scope_routed += 1;
                            }
                            None => store::upsert_habit(&mut profile, e),
                        }
                    } else {
                        store::upsert_habit(&mut profile, e);
                    }
                    claimed.extend(ids);
                    habits_landed += 1;
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
        habits_landed = entries.len();
        for c in entries {
            store::upsert_habit(&mut profile, c);
        }
    }
    if other_dirty {
        // 先对侧后本侧（落盘顺序）：中途失败最坏 = 同条目两边重复
        // （下拍重扫兜住），绝不丢结论。
        if let Some(o) = other.as_ref() {
            store::save_profile_locked_scope(workspace, scope.other(), o)?;
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

    Ok(SweepScopeStats {
        habits: habits_landed,
        lessons: lessons_count,
        observations: observations_count,
        user_messages: user_messages.len(),
        ai_scope_routed,
        reflect_error,
    })
}

#[cfg(test)]
mod tests;
