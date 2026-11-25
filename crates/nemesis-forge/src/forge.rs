//! Forge - main orchestrator for the self-learning framework.
//!
//! Coordinates the Collector, Reflector, Registry, Syncer, and
//! LearningEngine subsystems. This is the top-level entry point.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::task::JoinHandle;

use crate::bridge::ClusterForgeBridge;
use crate::collector::Collector;
use crate::config::ForgeConfig;
use crate::cycle_store::CycleStore;
use crate::exporter::Exporter;
use crate::learning_engine::LearningEngine;
use crate::mcp_installer::MCPInstaller;
use crate::monitor::DeploymentMonitor;
use crate::pipeline::Pipeline;
use crate::reflector::Reflector;
use crate::registry::Registry;
use crate::sanitizer::Sanitizer;
use crate::syncer::Syncer;
use crate::trace::TraceEvent;
use crate::trace_store::TraceStore;
use crate::types::{CollectorConfig, RegistryConfig};

/// The main Forge struct that owns and coordinates all subsystems.
pub struct Forge {
    config: ForgeConfig,
    workspace: PathBuf,
    forge_dir: PathBuf,
    collector: Collector,
    registry: Registry,
    sanitizer: Sanitizer,
    exporter: Exporter,
    reflector: Option<Reflector>,
    pipeline: Option<Arc<Pipeline>>,
    mcp_installer: Option<MCPInstaller>,
    syncer: Option<Syncer>,
    trace_store: Option<TraceStore>,
    learning_engine: Option<LearningEngine>,
    deployment_monitor: Option<Arc<DeploymentMonitor>>,
    cycle_store: Option<CycleStore>,
    /// 学习对话 LLM caller 槽（自学习铺路批次三）：`set_provider` 级联 +
    /// `set_learn_caller` 独立注入双入口。None = 学习对话诚实跳过
    /// （warn）。学习对话不走 learning_engine——其 provider/pipeline
    /// 注入面已随线 A 退役。
    learn_caller: Mutex<Option<std::sync::Arc<dyn crate::reflector_llm::LLMCaller>>>,
    /// 学习对话 estop 检查闭包槽：gateway 装配注入（SharedEstopState
    /// 现读）。None = 不设闸（引擎侧仍按急停契约在入口与每阶段前检查）。
    learn_estop: Mutex<Option<std::sync::Arc<dyn Fn() -> bool + Send + Sync>>>,
    running: Mutex<bool>,
    bridge: Mutex<Option<Arc<dyn ClusterForgeBridge>>>,
    /// Background task handles (reflector_loop, cleanup_loop).
    bg_tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Shared running flag that background loops observe.
    bg_running: Arc<Mutex<bool>>,
    /// Timestamp when Forge was last started (RFC3339).
    started_at: Mutex<Option<String>>,
    /// Runtime toggle for learning (mirrors config.learning.enabled, mutable at runtime).
    learning_enabled: std::sync::atomic::AtomicBool,
    /// 总闸热读源（2026-11-20 C3 热生效语义）：Some(config.json) =
    /// `is_enabled()` mtime 检测现读 `forge.enabled`（改键即刻生效——
    /// recording 闸下次调用、后台循环下个 cycle 即新值，无需重启）；
    /// None = 构造期值（`config.enabled`，CLI/agent 最小装配旧行为）。
    master_switch_path: Option<PathBuf>,
    /// 总闸 mirror 缓存：上次热读 (mtime, enabled)。None = 尚未热读过
    /// （首次 `is_enabled()` 即触发现读——构造期注入与磁盘现值的窗口期
    /// 差异一并覆盖）。std Mutex：热读是同步 stat + 小 JSON 解析。
    master_switch_cache: std::sync::Mutex<Option<(std::time::SystemTime, bool)>>,
    /// 热读失败 warn 一次性闸（同 exec 工具热读模式）：读失败每次调用
    /// 都会走到——首次 warn 留痕，后续防刷屏；读成功复位重上膛。
    master_switch_warned: std::sync::atomic::AtomicBool,
}

impl Forge {
    /// Create a new Forge instance with the given configuration and workspace.
    pub fn new(config: ForgeConfig, workspace: PathBuf) -> Self {
        let forge_dir = workspace.join("forge");

        tracing::info!(
            workspace = %workspace.display(),
            forge_dir = %forge_dir.display(),
            "[Forge] Instance created"
        );

        let collector = Collector::new(CollectorConfig {
            persistence_path: forge_dir
                .join("experiences")
                .join("experiences.jsonl")
                .to_string_lossy()
                .to_string(),
            ..Default::default()
        });

        let registry = Registry::new(RegistryConfig {
            index_path: forge_dir
                .join(crate::MANUAL_REGISTRY_FILE)
                .to_string_lossy()
                .to_string(),
        });

        let sanitizer = Sanitizer::new();
        let exporter = Exporter::new(crate::exporter::ExportConfig::new(&workspace));

        let learning_enabled = config.learning.enabled;

        Self {
            config,
            workspace,
            forge_dir: forge_dir.clone(),
            collector,
            registry,
            sanitizer,
            exporter,
            reflector: None,
            pipeline: None,
            mcp_installer: None,
            syncer: None,
            trace_store: None,
            learning_engine: None,
            deployment_monitor: None,
            cycle_store: None,
            learn_caller: Mutex::new(None),
            learn_estop: Mutex::new(None),
            running: Mutex::new(false),
            bridge: Mutex::new(None),
            bg_tasks: Mutex::new(Vec::new()),
            bg_running: Arc::new(Mutex::new(false)),
            started_at: Mutex::new(None),
            learning_enabled: std::sync::atomic::AtomicBool::new(learning_enabled),
            master_switch_path: None,
            master_switch_cache: std::sync::Mutex::new(None),
            master_switch_warned: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// 总闸热读源注入（2026-11-20 C3）：gateway 装配传入 config.json
    /// 路径。Some = `is_enabled()` 现读语义（见字段注）。
    pub fn with_master_switch_path(mut self, path: PathBuf) -> Self {
        self.master_switch_path = Some(path);
        self
    }

    /// Whether the forge system is enabled (the master switch from config).
    /// This is the truth source for "should forge collect experiences / run
    /// learning" — the per-tool-call recording path gates on it so that
    /// `forge.enabled = false` actually stops collection (not just the
    /// background loops).
    ///
    /// 2026-11-20 C3 热生效语义：`master_switch_path` 在场 → mtime 检测
    /// 现读 config.json 的 `forge.enabled`（首读也触发；mtime 变化才重读
    /// 文件）。翻转时 info 留痕；读失败诚实降级保持上次值 + 首次 warn。
    /// path 缺席 = 构造期值（旧行为字节不变）。
    pub fn is_enabled(&self) -> bool {
        let Some(path) = &self.master_switch_path else {
            return self.config.enabled;
        };
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let mut cache = self
            .master_switch_cache
            .lock()
            .expect("forge master switch cache mutex poisoned");
        // 缓存命中（mtime 一致）→ 免读。
        if let (Some(m), Some((last, v))) = (mtime, cache.as_ref())
            && m == *last
        {
            return *v;
        }
        // 首读 / mtime 变化 / mtime 丢失 → 现读 `forge.enabled`。
        match mtime.zip(
            nemesis_config::load_config(path)
                .ok()
                .map(|c| c.forge.as_ref().map(|f| f.enabled).unwrap_or(false)),
        ) {
            Some((m, fresh)) => {
                // 翻转留痕（含首次热读即与构造期值不同的形态）。
                let prev = cache
                    .as_ref()
                    .map(|(_, v)| *v)
                    .unwrap_or(self.config.enabled);
                if prev != fresh {
                    tracing::info!(
                        "[Forge] master switch flipped: {} -> {}（config.json forge.enabled，即刻生效）",
                        prev,
                        fresh
                    );
                }
                *cache = Some((m, fresh));
                self.master_switch_warned
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                fresh
            }
            None => {
                // 读失败：诚实降级——保持上次生效值（缓存空 = 构造期值）。
                if !self
                    .master_switch_warned
                    .swap(true, std::sync::atomic::Ordering::Relaxed)
                {
                    tracing::warn!(
                        "[Forge] master switch hot-read: config 读失败（{}）——保持上次生效值",
                        path.display()
                    );
                }
                cache
                    .as_ref()
                    .map(|(_, v)| *v)
                    .unwrap_or(self.config.enabled)
            }
        }
    }

    /// Start the forge subsystems.
    ///
    /// Spawns two background tokio tasks:
    /// - **reflector_loop**: periodically runs reflection + learning.
    /// - **cleanup_loop**: periodically removes old data.
    ///
    /// The former collector_loop (periodic buffer flush) was removed together
    /// with the aggregate line: `record()` writes through to disk
    /// directly, so there is no buffered channel left to drain.
    pub async fn start(self: Arc<Self>) {
        {
            let mut running = self.running.lock();
            if *running {
                tracing::warn!("[Forge] Already running");
                return;
            }
            *running = true;
        }

        *self.bg_running.lock() = true;
        *self.started_at.lock() = Some(chrono::Local::now().to_rfc3339());

        let reflect_interval = self.config.reflection.interval_secs;
        let cleanup_interval = self.config.storage.cleanup_interval_secs;

        // 0=关 惯例（缺陷 22）：reflector/cleanup 循环对
        // interval=0 不 spawn——旧实现 `elapsed >= 0` 恒真，反思（use_llm
        // =true 时含 LLM 调用）与清理每秒各跑一轮，成本与磁盘 churn 失控；
        // 本仓 0=关 惯例（board.budget/sweep 等同款）下用户设 0 的意图是
        // 关闭循环，旧行为恰好相反。
        if reflect_interval == 0 {
            tracing::warn!("[Forge] reflection.interval_secs = 0，反思循环已关闭（0=关）");
        }
        if cleanup_interval == 0 {
            tracing::warn!("[Forge] storage.cleanup_interval_secs = 0，清理循环已关闭（0=关）");
        }

        // reflector_loop（interval=0 = 关闭，不 spawn）
        if reflect_interval > 0 {
            let forge = Arc::clone(&self);
            let flag = self.bg_running.clone();

            let handle = tokio::spawn(async move {
                let check_interval = std::time::Duration::from_secs(1);
                let mut elapsed = 0u64;
                loop {
                    tokio::time::sleep(check_interval).await;
                    if !*flag.lock() {
                        break;
                    }
                    elapsed += 1;
                    if elapsed >= reflect_interval {
                        elapsed = 0;
                        // 总闸看闸（2026-11-20 C3 热生效语义）：关 = 跳过
                        // 本 cycle（含 LLM 调用的反思成本归零）；重开后到
                        // 点即恢复。is_enabled() 本身热读 config 总闸。
                        if !forge.is_enabled() {
                            continue;
                        }
                        forge.run_reflection_cycle().await;
                    }
                }
            });
            self.bg_tasks.lock().push(handle);
        }

        // cleanup_loop（interval=0 = 关闭，不 spawn）
        if cleanup_interval > 0 {
            let forge = Arc::clone(&self);
            let flag = self.bg_running.clone();

            let handle = tokio::spawn(async move {
                let check_interval = std::time::Duration::from_secs(1);
                let mut elapsed = 0u64;
                loop {
                    tokio::time::sleep(check_interval).await;
                    if !*flag.lock() {
                        break;
                    }
                    elapsed += 1;
                    if elapsed >= cleanup_interval {
                        elapsed = 0;
                        // 总闸看闸（2026-11-20 C3）：同 reflector_loop。
                        if !forge.is_enabled() {
                            continue;
                        }
                        forge.run_cleanup_cycle().await;
                    }
                }
            });
            self.bg_tasks.lock().push(handle);
        }

        tracing::info!("[Forge] Started with background tasks (master switch hot-read per cycle)");
    }

    /// Run a single reflection cycle: read experiences → reflect → learn → write report → share.
    async fn run_reflection_cycle(&self) {
        let store = crate::experience_store::ExperienceStore::from_forge_dir(&self.forge_dir);
        // F-: parse only the most-recent experiences (bounded by
        // collection.max_experiences) instead of the whole file each cycle.
        // (File size itself is age-bounded by cleanup — F-.)
        let limit = self.config.collection.max_experiences.max(1);
        let experiences = match store.read_recent(limit).await {
            Ok(exps) if !exps.is_empty() => exps,
            _ => return,
        };

        tracing::info!(
            count = experiences.len(),
            "[Forge] Running reflection cycle"
        );

        let Some(ref reflector) = self.reflector else {
            return;
        };

        // Step 1: Learning cycle first（语义反思接入 时序修正）：
        // 先跑学习闭环（模式检出 + 技能生成），本轮 LearningCycle 产出后
        // 再做语义反思——报告首次携带**本轮**闭环状态（旧序传 None，语义
        // 分析看到的永远是上一轮的闭环，时序错位）。
        let mut learning_cycle = None;
        if let Some(ref learning_engine) = self.learning_engine
            && self.is_learning_enabled()
        {
            let cycle = learning_engine.run_cycle(&experiences).await;
            tracing::info!(cycle_id = %cycle.id, patterns = cycle.patterns_found, actions = cycle.actions_taken, "[Forge] learning cycle completed");
            learning_cycle = Some(cycle);
        }

        // Step 1.5: 加载 TraceStore 真轨迹（缺陷整改批次 A1）——反思
        // 报告的 Trace Analysis 段数据源从 experiences 冒充改为真轨迹。
        // 读取失败/无 store → 空 Vec（报告诚实无 Trace Analysis 段，
        // 不炸反思循环）。
        let traces: Vec<crate::monitor::ConversationTrace> = match self.trace_store() {
            Some(store) => match store.read_all().await {
                Ok(events) => {
                    let t = crate::monitor::build_conversation_traces(&events);
                    tracing::info!(
                        traces = t.len(),
                        "[Forge] trace store loaded for reflection"
                    );
                    t
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[Forge] trace store read failed, report will omit trace analysis");
                    Vec::new()
                }
            },
            None => Vec::new(),
        };

        // Step 1.6（批 D，用户行为学习扩展；2026-11-25 双缝互通修订 ack
        // 责任）：用户会话教训 + veto 负样本消费——userprofile 反思分流
        // （批 A 接口一）落 outbound_lessons pending 队列的助手教训，
        // Forge 侧消费形态 = **反思输入 + 报告留痕**：lesson 内容进语义
        // 分析提示词（洞察吸收）、进报告「用户会话教训」节（人读留痕）。
        // **ack 责任随 learn.enabled 分流**（单一消费方纪律）：深学对话
        // 开启（learn.enabled 且 use_llm——use_llm=false 时对话被关）→
        // 对话侧是唯一 ack 责任人，本处只读展示不 ack；否则保持批 D 原
        // 行为（报告落盘成功后 ack——报告消化即消费，防 pending 永积）。
        // veto 回传（批 A 挂账项）= events.jsonl veto/forget 行作语义
        // 分析负样本注记（「勿重提同类」）。**lessons 永不进用户画像注入
        // 面**（主语 = 助手，userprofile upsert 硬拒双保险）。队列缺失/
        // 损坏 = 空 Vec 诚实降级，不炸反思循环。Dashboard 手动反思 /
        // CLI reflect 不走本循环，天然只读。
        let dialogue_owns_lesson_ack = self.config.learn.enabled && self.config.reflection.use_llm;
        let (user_lessons, lesson_ids, veto_notes) = match self.forge_dir.parent() {
            Some(ws) => {
                let lessons = crate::userprofile::bridge::read_pending_lessons(ws, 20);
                let notes: Vec<String> = crate::userprofile::collection::read_recent_events(ws, 50)
                    .into_iter()
                    .filter(|ev| {
                        matches!(
                            ev.get("kind").and_then(|k| k.as_str()),
                            Some("veto") | Some("forget")
                        )
                    })
                    .filter_map(|ev| {
                        let content = ev
                            .get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let kind = ev.get("kind").and_then(|k| k.as_str()).unwrap_or("?");
                        (!content.is_empty()).then(|| format!("[{kind}] {content}"))
                    })
                    .take(10)
                    .collect();
                let ids: Vec<String> = lessons.iter().map(|l| l.id.clone()).collect();
                let contents: Vec<String> = lessons.iter().map(|l| l.content.clone()).collect();
                (contents, ids, notes)
            }
            None => (Vec::new(), Vec::new(), Vec::new()),
        };
        if !user_lessons.is_empty() {
            tracing::info!(
                count = user_lessons.len(),
                "[Forge] consuming pending user-session lessons"
            );
        }

        // Step 2: Reflect（统计 + use_llm 时 LLM 语义分析）。语义反思带：全量 artifacts
        // （旧调用面只传统计不传产物——LLM 看不到已部署技能会重复提名）、
        // 本轮 learning_cycle、真轨迹、统一杂务预算（thinking 系防烧穿）。
        // 桥接护栏空输出 → Err → warn+skip，报告无 insights 节，诚实降级
        // 不炸循环。use_llm=false = 纯统计反思（修复：该键文档契约
        // 「是否用 LLM 做语义分析」，此前 gateway 无条件走 LLM、键只在 CLI
        // 被消费——用户设 false 省成本的意图被静默无视）。
        // A4：period 从硬编码 "today" 换成实际数据窗口（最近 N 条 +
        // 首末时间戳），反思报告的 Period 行第一次反映真实覆盖面。
        // 标签计算单一真相源 = compute_reflection_period（三入口同源：
        // run_reflection_cycle / Dashboard 手动反思 / CLI reflect）。
        let period = Reflector::compute_reflection_period(&experiences);
        let report = if self.config.reflection.use_llm {
            reflector
                .reflect_with_llm(
                    &experiences,
                    &traces,
                    &self.merged_artifacts_for_reflection(),
                    learning_cycle.as_ref(),
                    &period,
                    "all",
                    Some(crate::reflector_llm::AUX_FORGE_MAX_TOKENS),
                    &user_lessons,
                    &veto_notes,
                )
                .await
        } else {
            reflector.reflect(
                &experiences,
                &traces,
                learning_cycle.as_ref(),
                &period,
                "all",
                &user_lessons,
                &veto_notes,
            )
        };

        // A5：学习闭环开关状态注入报告头部（反思报告必须能让用户一眼
        // 看到闭环是关的——「51 天零工件」审计的教训：关着的学习开关
        // 在报告里不可见，读者会误以为循环在跑）。
        let mut report = report;
        report.learning_enabled = Some(self.is_learning_enabled());

        // Step 3: Write report
        if let Ok(path) = reflector.write_report(&report) {
            tracing::info!(path = %path.display(), "[Forge] reflection report written");

            // Step 3.5（批 D；2026-11-25 双缝互通修订）：教训消费确认——
            // 责任随 learn.enabled 分流：深学对话开启时对话侧是唯一 ack
            // 责任人（本处只读展示，防双消费方竞写）；关闭时报告落盘成
            // 功后 ack 兜底（报告消化即消费，失败留 pending 下轮重新消
            // 费，at-least-once）。ack 按精确 id 确认（bridge 纪律），失
            // 败 loud warn 不炸循环。
            if !lesson_ids.is_empty()
                && !dialogue_owns_lesson_ack
                && let Some(ws) = self.forge_dir.parent()
            {
                match crate::userprofile::bridge::ack_lessons(ws, &lesson_ids) {
                    Ok(n) => {
                        tracing::info!(acked = n, "[Forge] user-session lessons acked");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "[Forge] lesson ack failed (stay pending)")
                    }
                }
            }

            // Step 4: Cluster share
            if let Some(ref syncer) = self.syncer
                && let Ok(content) = std::fs::read_to_string(&path)
            {
                let json = serde_json::json!({
                    "content": content,
                    "filename": path.file_name().map(|n| n.to_string_lossy().to_string()),
                });
                if let Err(e) = syncer.share_reflection(json).await {
                    tracing::warn!(error = %e, "[Forge] cluster share failed");
                }
            }
        } else {
            // 诚实化：报告落盘失败此前静默——反思明明跑完，
            // 盘上却无新报告，用户只看到「latest report 停在旧日期」。loud warn。
            tracing::warn!("[Forge] failed to write reflection report (see prior errors)");
        }

        // Step 5: 学习对话（批次三生成面，替代已退役的 auto_skill 蒸馏：
        // LLM 当脑子多轮对话决定学什么，Rust 只铺路——证据扫描/组包/预算/
        // 闸门全是确定性形状工作）。`learn.enabled` 门控自动对话（手动入口
        // WSAPI learn.run / CLI forge learn run 是显式意志，不受此门控）；
        // use_llm=false 一并关：对话是 LLM 工作（同旧蒸馏语义——语义分析
        // 关而对话开会自相矛盾）。失败只 warn 不炸反思循环（对话是本轮
        // 增益，非关键路径）；「没有值得学的」是引擎一等答案（empty_reason
        // 收场），info 留痕即可。
        if self.config.learn.enabled {
            if !self.config.reflection.use_llm {
                tracing::warn!(
                    "[Forge] learn.enabled but reflection.use_llm=false — learn dialogue requires LLM, skipping"
                );
            } else {
                match self.run_learn_dialogue("cycle").await {
                    Ok(outcome) => {
                        if let Some(reason) = &outcome.empty_reason {
                            tracing::info!(
                                reason = %reason,
                                "[Forge] learn dialogue: nothing worth learning this cycle"
                            );
                        } else {
                            tracing::info!(
                                insights = outcome.insights.len(),
                                skills = outcome.skills.len(),
                                user_findings = outcome.user_findings.len(),
                                dropped = outcome.dropped.len(),
                                "[Forge] learn dialogue completed"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "[Forge] learn dialogue failed, skipping");
                    }
                }
            }
        }
    }

    /// 学习对话共享入口（自学习铺路批次三）：证据扫描（确定性形状匹配）
    /// → 组包（候选/洞察台账/模式高亮/失败经验/报告摘录/技能库摘要）→
    /// 四阶段对话（引擎侧 DIALOGUE_BUSY 互斥 + estop 闸 + 留痕）→ 成功
    /// 后推进候选消费游标。cycle 路径（`learn.enabled` 门控）与手动入口
    /// （WSAPI `learn.run` / CLI `forge learn run`，不受门控——显式用户
    /// 动作即用户意志）共用此路径；触发来源进留痕目录名便于回查。
    pub async fn run_learn_dialogue(
        &self,
        trigger: &str,
    ) -> Result<crate::learn_dialogue::LearnDialogueOutcome, String> {
        let caller = self.learn_caller.lock().clone();
        let Some(caller) = caller else {
            return Err("学习对话未配置 LLM caller（set_learn_caller / forge provider）".into());
        };
        let estop_slot = self.learn_estop.lock().clone();
        let is_stopped = move || estop_slot.as_ref().map(|f| f()).unwrap_or(false);
        if is_stopped() {
            return Err("急停中：学习对话拒绝启动".into());
        }

        // ① 证据扫描：session_logs 确定性形状匹配 → 候选台账 ingest
        // （失败只 warn——包继续用既有候选，扫描是增量不是关键路径）。
        // 路径单一真相源 nemesis-path（2026-11-24 修复：此前指 logs/ 顶层
        // ——会话日志在 logs/session_logs/ 子目录，生产扫描恒空）。
        let store = crate::session_evidence::CandidateStore::from_forge_dir(&self.forge_dir);
        let drafts = crate::session_evidence::scan_workspace_logs(
            &nemesis_path::resolve_session_logs_dir_in_workspace(&self.workspace),
        );
        match store.ingest(drafts) {
            Ok(n) if n > 0 => {
                tracing::info!(
                    new = n,
                    "[Forge] learn: candidate scan ingested new candidates"
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "[Forge] learn: candidate ingest failed, pack continues with existing candidates");
            }
        }
        let candidates = store.unconsumed(crate::session_evidence::CONSUMER_DIALOGUE);

        // ①.5 缝一（2026-11-25 双缝互通）：pending 用户会话教训并入证据
        // 包（独立节深学）。读上限 20 与反思循环对齐；lessons 队列缺失/
        // 损坏 = 空 Vec 诚实降级（bridge 宽容读）。教训队列尾部取数（最
        // 新 N 条）是 bridge 既有语义。
        let lessons = crate::userprofile::bridge::read_pending_lessons(&self.workspace, 20);
        if !lessons.is_empty() {
            tracing::info!(
                pending = lessons.len(),
                "[Forge] learn: packing pending user-session lessons into evidence pack"
            );
        }

        // ② 组包原料（全部失败诚实降级为空节，不阻塞对话）。
        let insights_store = crate::insights::InsightStore::with_recurrence(
            &self.workspace,
            self.config.learn.activation_recurrence,
        );
        let insights = insights_store.list(None).unwrap_or_default();
        let exp_store = crate::experience_store::ExperienceStore::from_forge_dir(&self.forge_dir);
        let limit = self.config.collection.max_experiences.max(1);
        let collected = exp_store.read_recent(limit).await.unwrap_or_default();
        let experiences: Vec<crate::types::Experience> =
            collected.iter().map(|c| c.experience.clone()).collect();
        let patterns_digest = self.patterns_digest(&collected);
        let latest_report = self
            .reflector
            .as_ref()
            .and_then(|r| r.get_latest_report_content().ok());
        let skills_digest = self.skills_digest();

        let pack = crate::session_evidence::build_evidence_pack(
            None,
            &candidates,
            &lessons,
            Some(&insights),
            patterns_digest.as_deref(),
            &experiences,
            latest_report.as_deref(),
            skills_digest.as_deref(),
        );

        // 场景归属（洞察页与场景拆分 2026-11-24）：批内候选全为同一非 None
        // 项目戳 → 本轮洞察继承该项目；混合/全对话 = 对话（None）。
        let dialogue_scope = unanimous_scope(&candidates);

        // ③ 四阶段对话（留痕目录带时间戳 + 触发来源；锁文件 = learn/
        // 平级 learn.lock——跨进程互斥面，CLI 与 gateway 双进程共守）。
        let trace_dir = self.forge_dir.join("learn").join(format!(
            "{}_{}",
            chrono::Local::now().format("%Y%m%d_%H%M%S%3f"),
            trigger
        ));
        let lock_path = self.forge_dir.join("learn.lock");
        let mut outcome = crate::learn_dialogue::run(
            caller.as_ref(),
            &pack,
            &insights_store,
            skills_digest.as_deref().unwrap_or(""),
            &is_stopped,
            &trace_dir,
            &lock_path,
            self.config.learn.budget_tokens,
            dialogue_scope.as_deref(),
            // 缝二：画像工作区根——subject=user 结论回抛目标（接口二）。
            Some(self.workspace.as_path()),
        )
        .await?;

        // ⑤ 装前 tests 闸（自学习铺路批次四）：带 tests 草稿 → A 线实跑
        // → refine ≤N → 过闸落 workspace/skills/{slug}-forge/。执行器
        // 缺席 = fail-closed 全拒（草稿只在 outcome.gate 留痕，不落盘）。
        if !outcome.skills.is_empty() {
            let runner = self.pipeline.as_ref().and_then(|p| p.skill_test_runner());
            outcome.gate = crate::skill_gate::gate_skill_drafts(
                runner.as_ref(),
                Some(&caller),
                &self.workspace,
                &outcome.skills,
                &is_stopped,
                self.config.learn.budget_tokens,
                self.config.test.judge_threshold,
            )
            .await;
            let installed = outcome
                .gate
                .iter()
                .filter(|r| matches!(r.verdict, crate::skill_gate::GateVerdict::Installed))
                .count();
            tracing::info!(
                drafts = outcome.skills.len(),
                installed,
                "[Forge] learn: skill draft gate completed"
            );
        }

        // ④ 成功才推进消费游标（失败重试时同批候选重进包——诚实重放）。
        // 推进数 = pack.candidate_count（≡实际进包条数），**不是**
        // candidates.len()——候选节封顶后没进包的候选不被标记消费
        // （凡标记消费的候选必须真实进过包），留待下一轮（BUG-1）。
        if pack.candidate_count > 0
            && let Err(e) = store.mark_consumed(
                crate::session_evidence::CONSUMER_DIALOGUE,
                pack.candidate_count,
            )
        {
            tracing::warn!(error = %e, "[Forge] learn: mark_consumed failed (same batch may replay)");
        }

        // ④.5 缝一（2026-11-25 双缝互通）：教训消费确认——与游标推进同
        // 位（对话成功才 ack；estop/传输失败已在上方 `?` 收场，自然不
        // ack）。按 pack.lesson_ids 精确确认（≡实际进包 ids——凡确认的
        // 教训必须真实进过包；超容量/超预算未进包的留 pending 下轮重
        // 放）。此时本函数是唯一 ack 责任人（learn.enabled 且 use_llm
        // 时反思循环 Step 3.5 不 ack；learn.enabled=false 的手动对话也
        // ack——对话真实消化了内容，ack 幂等无竞害）。
        if !pack.lesson_ids.is_empty()
            && let Err(e) =
                crate::userprofile::bridge::ack_lessons(&self.workspace, &pack.lesson_ids)
        {
            tracing::warn!(error = %e, "[Forge] learn: lesson ack failed (stay pending)");
        }

        // ⑥ 实效统计刷新（自学习铺路批次四）：纠正率 delta 前后窗口对照
        // 落 insights.json `stats` 字段。失败只 warn——统计是只读推导
        // 非关键路径，不阻塞对话结果返回。
        if let Err(e) = insights_store.refresh_stats() {
            tracing::warn!(error = %e, "[Forge] learn: insight stats refresh failed");
        }
        Ok(outcome)
    }

    /// 监控闭环模式高亮 digest（证据包 ④ 节原料）：learning engine 的
    /// 结构化模式检出（证据块已脱敏），封顶 6 条防单节吃穿证据包预算；
    /// 无引擎 / 无模式 = None（节省略）。
    fn patterns_digest(&self, collected: &[crate::types::CollectedExperience]) -> Option<String> {
        let engine = self.learning_engine.as_ref()?;
        let patterns = engine.extract_patterns(collected);
        if patterns.is_empty() {
            return None;
        }
        let mut s = String::new();
        for p in patterns.iter().take(6) {
            s.push_str(&format!(
                "- [{}] {}（频次 {}，置信 {:.2}）\n",
                p.pattern_type, p.description, p.frequency, p.confidence
            ));
            if let Some(ev) = &p.evidence
                && !ev.trim().is_empty()
            {
                let cut: String = ev.trim().chars().take(300).collect();
                s.push_str(&format!("  证据：{cut}\n"));
            }
        }
        Some(s)
    }

    /// 技能库摘要（证据包 ⑦ 节原料 + 对话阶段 3 对账清单）：合并注册表
    /// （手动 ∪ 闭环）的 kind/name/version/status 行，封顶 40 行。
    fn skills_digest(&self) -> Option<String> {
        let arts = self.merged_artifacts_for_reflection();
        if arts.is_empty() {
            return None;
        }
        let mut s = String::new();
        for a in arts.iter().take(40) {
            s.push_str(&format!(
                "- [{:?}] {} v{}（{:?}）\n",
                a.kind, a.name, a.version, a.status
            ));
        }
        Some(s)
    }

    /// Run a single cleanup cycle for old data.
    async fn run_cleanup_cycle(&self) {
        // u64→i64 回绕防线（R735 数字恐慌面）：>i64::MAX 的配置值 `as i64`
        // 回绕成负数 → cutoff 落到未来 → 经验/反思/循环三 store 全量删除；
        // 巨值（≥1.07e14）直接 Duration::days panic。钳到 100 年上界（单一
        // 真相源 time_bounds::MAX_AGE_DAYS）；store 入口另有兜底钳。0 保留
        // 现语义（cutoff=今天，只留当日）。
        let max_age = self
            .config
            .storage
            .max_experience_age_days
            .min(nemesis_utils::time_bounds::MAX_AGE_DAYS as u64) as i64;

        let store = crate::experience_store::ExperienceStore::from_forge_dir(&self.forge_dir);
        if let Ok(removed) = store.cleanup(max_age).await
            && removed > 0
        {
            tracing::info!(removed, "[Forge] cleaned up old experiences");
        }

        if let Some(ref reflector) = self.reflector {
            let removed = reflector.cleanup_reports(max_age as u64);
            if removed > 0 {
                tracing::info!(removed, "[Forge] cleaned up old reports");
            }
        }

        if let Some(ref cycle_store) = self.cycle_store
            && let Ok(removed) = cycle_store.cleanup(max_age).await
            && removed > 0
        {
            tracing::info!(removed, "[Forge] cleaned up old learning cycles");
        }

        // Trace events (- 接线的盘上产物)：按 trace.max_trace_age_days
        // 滚动清理 JSONL（读-滤-重写）。验证轮二接线——此前该配置键零消费者，
        // traces 目录只增不删。清理 Forge 实际持有的那个 store（未 init 时
        // 无写入，无需清理）。
        if let Some(store) = &self.trace_store
            && let Ok(removed) = store
                .cleanup(
                    // u64→i64 回绕防线（R735）：同 max_experience_age_days，
                    // 钳到 100 年上界后再进 store。
                    self.config
                        .trace
                        .max_trace_age_days
                        .min(nemesis_utils::time_bounds::MAX_AGE_DAYS as u64),
                )
                .await
            && removed > 0
        {
            tracing::info!(removed, "[Forge] cleaned up old trace events");
        }
    }

    /// Stop the forge subsystems.
    ///
    /// Sets the running flag to false (which causes background loops to exit
    /// within 1 second). No final flush is needed: `record()` writes through
    /// to disk directly (the buffered collector was removed).
    pub async fn stop(&self) {
        *self.running.lock() = false;
        *self.bg_running.lock() = false; // Signal background loops to exit
        *self.started_at.lock() = None;

        // Wait for background tasks to finish (they check the flag every 1s).
        let tasks: Vec<_> = {
            let mut guard = self.bg_tasks.lock();
            std::mem::take(&mut *guard)
        };
        for handle in tasks {
            let _ = handle.await;
        }

        tracing::info!("[Forge] Stopped");
    }

    /// Check if forge is running.
    pub fn is_running(&self) -> bool {
        *self.running.lock()
    }

    /// Set the cluster bridge for cross-node communication.
    pub fn set_bridge(&self, bridge: Arc<dyn ClusterForgeBridge>) {
        tracing::info!("[Forge] Cluster bridge configured");
        *self.bridge.lock() = Some(bridge);
    }

    /// Cascade-set the LLM provider to all subsystems that need it:
    /// reflector, pipeline, and the learn-dialogue slot (all via LLMCaller).
    pub fn set_provider(&self, provider: Arc<dyn crate::reflector_llm::LLMCaller>) {
        tracing::info!("[Forge] Setting LLM provider on subsystems");
        // Reflector gets the LLM caller for semantic analysis (Stage 2 LLM).
        if let Some(ref reflector) = self.reflector {
            reflector.set_provider(provider.clone());
        }
        // Pipeline gets the LLM caller for quality evaluation (Stage 3).
        if let Some(ref pipeline) = self.pipeline {
            pipeline.set_provider(provider.clone());
        }
        // 学习对话槽（批次三：旧 learning engine 注入臂已随线 A 退役）。
        *self.learn_caller.lock() = Some(provider);
    }

    // ----- Accessor methods (mirrors Go Forge.GetXxx) -----

    /// Get a reference to the configuration.
    pub fn config(&self) -> &ForgeConfig {
        &self.config
    }

    /// Get the timestamp when Forge was last started (RFC3339).
    pub fn started_at(&self) -> Option<String> {
        self.started_at.lock().clone()
    }

    /// Set the learning enabled flag at runtime.
    /// Takes effect on the next reflection cycle — no restart needed.
    pub fn set_learning_enabled(&self, enabled: bool) {
        self.learning_enabled
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
        tracing::info!(enabled, "[Forge] Learning flag updated at runtime");
    }

    /// Check whether learning is currently enabled.
    pub fn is_learning_enabled(&self) -> bool {
        self.learning_enabled
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Get a reference to the collector.
    pub fn collector(&self) -> &Collector {
        &self.collector
    }

    /// Get a reference to the registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Read-only peek over the closed-loop learning registry
    /// (`learning_registry.json`, BUG 批次 2026-11-14 跨库互查面）。
    ///
    /// 双注册表架构（手动/蒸馏 → registry.json；闭环引擎 →
    /// learning_registry.json）此前互盲：蒸馏去重、反思输入都看不见闭环
    /// 侧产物。peek 缺失=空、损坏=WARN 留痕不阻塞（Registry::peek_sync）。
    /// 对称面 = [`LearningEngine` 对 registry.json 的窥视]。
    pub(crate) fn peek_learning_registry(&self) -> Registry {
        Registry::peek_sync(&self.forge_dir.join(crate::LEARNING_REGISTRY_FILE))
    }

    /// Reflection input universe: manual registry ∪ learning
    /// registry (dedup by id).
    ///
    /// 「全量 artifacts」注入点（reflect_with_llm 调度循环 / CLI forge
    /// reflect / web 手动反思 handler）此前只传 registry.json——闭环侧已
    /// 部署技能对反思 LLM 不可见 → 重复提名（A4 防线在跨库面失效）。按 id
    /// 去重：历史撞车形态（两库各自持有 skill-{slug}）防御。合并语义单一
    /// 真相源 = [`merge_artifacts_by_id`]（pub 自由函数：web 手动反思
    /// handler 无 Forge 实例，经 peek_sync + 本函数复用同一合并规则）。
    /// 旧蒸馏消费点（run_reflection_cycle Step 5 / CLI distill_and_store_draft）
    /// 已随线 B 蒸馏退役（2026-11-24 批次三），生成面改道学习对话。
    pub fn merged_artifacts_for_reflection(&self) -> Vec<nemesis_types::forge::Artifact> {
        let learning = self.peek_learning_registry().list(None, None);
        merge_artifacts_by_id(self.registry.list(None, None), learning)
    }

    /// Get a reference to the sanitizer.
    pub fn sanitizer(&self) -> &Sanitizer {
        &self.sanitizer
    }

    /// Get a reference to the exporter.
    pub fn exporter(&self) -> &Exporter {
        &self.exporter
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Get the forge directory path.
    pub fn forge_dir(&self) -> &PathBuf {
        &self.forge_dir
    }

    /// Get the cluster bridge (if configured).
    pub fn bridge(&self) -> Option<Arc<dyn ClusterForgeBridge>> {
        let guard = self.bridge.lock();
        guard.clone()
    }

    /// Get the reflector (if initialized).
    pub fn reflector(&self) -> Option<&Reflector> {
        self.reflector.as_ref()
    }

    /// Set the skill behavior-test runner (A 线回归闸执行器，)。
    /// Pipeline 在 Skill artifact 校验时调用；未注入 = 带 tests 段的
    /// artifact 诚实失败（fail-closed，不允许无执行器的静默跳过）。
    pub fn set_skill_test_runner(
        &self,
        runner: std::sync::Arc<dyn crate::skill_tests::SkillTestRunner>,
    ) {
        if let Some(ref pipeline) = self.pipeline {
            pipeline.set_skill_test_runner(runner);
        }
    }

    /// Set the Stage 2 judge-layer caller（杂务模型回落链注入点，
    /// 三轮复查对齐 CLI/WSAPI/部署闸三入口口径）。级联到 pipeline 的
    /// judge 专用槽；Stage 3 质量评估维持主模型语义不受影响。
    pub fn set_judge_caller(&self, caller: std::sync::Arc<dyn crate::reflector_llm::LLMCaller>) {
        if let Some(ref pipeline) = self.pipeline {
            pipeline.set_judge_caller(caller);
        }
    }

    /// 学习对话 LLM caller 注入（自学习铺路批次三）。`set_provider` 级联
    /// 之外独立注入面：允许学习对话用专门模型（`learn.model` 回落链在
    /// bot 侧装配后经此槽传入，不影响主模型/杂务链）。
    pub fn set_learn_caller(&self, caller: std::sync::Arc<dyn crate::reflector_llm::LLMCaller>) {
        *self.learn_caller.lock() = Some(caller);
    }

    /// 学习对话 estop 检查闭包注入（gateway 装配：SharedEstopState 现读；
    /// 引擎侧入口 + 每阶段前双检查的信号源）。
    pub fn set_learn_estop_check(&self, check: std::sync::Arc<dyn Fn() -> bool + Send + Sync>) {
        *self.learn_estop.lock() = Some(check);
    }

    /// Get the pipeline (if initialized).
    pub fn pipeline(&self) -> Option<&Pipeline> {
        self.pipeline.as_deref()
    }

    /// Get the MCP installer (if initialized).
    pub fn mcp_installer(&self) -> Option<&MCPInstaller> {
        self.mcp_installer.as_ref()
    }

    /// Get the syncer (if initialized).
    pub fn syncer(&self) -> Option<&Syncer> {
        self.syncer.as_ref()
    }

    /// Get the trace store (if initialized).
    pub fn trace_store(&self) -> Option<&TraceStore> {
        self.trace_store.as_ref()
    }

    /// Get the learning engine (if initialized).
    pub fn learning_engine(&self) -> Option<&LearningEngine> {
        self.learning_engine.as_ref()
    }

    /// Get the deployment monitor (if initialized).
    pub fn deployment_monitor(&self) -> Option<&DeploymentMonitor> {
        self.deployment_monitor.as_deref()
    }

    /// Get the cycle store (if initialized).
    pub fn cycle_store(&self) -> Option<&CycleStore> {
        self.cycle_store.as_ref()
    }

    // ----- Trace recording -----

    /// Record a trace event: direct JSONL append to the TraceStore.
    ///
    /// Tool-dispatch 侧的低频喂入口（审计 - 修复，
    /// TraceStore 原先零生产调用）。trace 是观测数据——写失败只 warn，
    /// 绝不反过来影响工具执行链。
    ///
    /// 验证轮二：不再同步喂 in-memory `TraceCollector`——该
    /// collector 全仓零读者（getter 无生产调用方），喂它等于制造无上界
    /// write-only 内存增长（每笔工具调用永久驻留 Vec）；盘上 TraceStore
    /// 是学习引擎的唯一输入真相源（learning_engine 轨迹臂读盘）。
    pub async fn record_trace_event(&self, event: TraceEvent) {
        if let Some(store) = &self.trace_store
            && let Err(e) = store.append(&event).await
        {
            tracing::warn!(
                error = %e,
                event_type = %event.event_type,
                "[Forge] Failed to append trace event"
            );
        }
    }

    // ----- Initialization methods -----

    /// Initialize the reflector subsystem.
    pub fn init_reflector(&mut self, reflector: Reflector) {
        tracing::info!("[Forge] Reflector subsystem initialized");
        self.reflector = Some(reflector);
    }

    /// Initialize the pipeline subsystem.
    pub fn init_pipeline(&mut self, pipeline: Arc<Pipeline>) {
        tracing::info!("[Forge] Pipeline subsystem initialized");
        self.pipeline = Some(pipeline);
    }

    /// Initialize the MCP installer.
    pub fn init_mcp_installer(&mut self, installer: MCPInstaller) {
        tracing::info!("[Forge] MCP installer initialized");
        self.mcp_installer = Some(installer);
    }

    /// Initialize the syncer with the current bridge.
    pub fn init_syncer(&mut self) {
        let bridge_opt = self.bridge.lock().clone();
        if let Some(bridge) = bridge_opt {
            tracing::info!("[Forge] Syncer initialized with cluster bridge");
            self.syncer = Some(Syncer::with_forge_dir(bridge, self.forge_dir.clone()));
        } else {
            tracing::debug!("[Forge] Syncer not initialized: no bridge configured");
        }
    }

    /// Initialize the trace subsystem.
    ///
    /// 验证轮二：只收 TraceStore。原 collector 参数随 write-only
    /// 内存面移除（见 `record_trace_event` 注记）。
    pub fn init_trace(&mut self, store: TraceStore) {
        tracing::info!("[Forge] Trace subsystem initialized");
        self.trace_store = Some(store);
    }

    /// Initialize the learning subsystem.
    pub fn init_learning(
        &mut self,
        engine: LearningEngine,
        monitor: Arc<DeploymentMonitor>,
        store: CycleStore,
    ) {
        // Monitor injection only（自学习铺路批次三收窄）：pipeline 注入
        // 臂已随线 A 退役（engine 的 refine 循环删除，批次四对话产
        // tests 闭环在 bot 侧装配面重新消费 Pipeline）；学习动作改由
        // `learn_dialogue` 多轮对话引擎承担（Forge::run_learn_dialogue）。
        engine.set_monitor(monitor.clone());
        tracing::info!("[Forge] Learning subsystem initialized (engine + monitor + cycle store)");
        self.learning_engine = Some(engine);
        self.deployment_monitor = Some(monitor);
        self.cycle_store = Some(store);
    }

    // ----- High-level methods -----

    /// Trigger a manual reflection cycle.
    pub fn reflect_now(
        &self,
        experiences: &[crate::types::CollectedExperience],
    ) -> Result<nemesis_types::forge::Reflection, String> {
        tracing::info!(
            experience_count = experiences.len(),
            "[Forge] Manual reflection triggered"
        );
        if let Some(ref reflector) = self.reflector {
            let result = reflector.generate_reflection(experiences);
            tracing::info!(
                insight_count = result.insights.len(),
                recommendation_count = result.recommendations.len(),
                "[Forge] Reflection completed"
            );
            Ok(result)
        } else {
            tracing::error!("[Forge] Reflection failed: reflector not initialized");
            Err("Reflector not initialized".to_string())
        }
    }

    /// Receive a remote reflection report and store it.
    pub fn receive_reflection(&self, payload: &serde_json::Value) -> Result<(), String> {
        tracing::info!("[Forge] Receiving remote reflection report");
        if let Some(ref syncer) = self.syncer {
            let result = syncer.receive_reflection(payload);
            if result.is_ok() {
                tracing::info!("[Forge] Remote reflection report stored successfully");
            } else if let Err(e) = &result {
                tracing::error!(error = %e, "[Forge] Failed to store remote reflection");
            }
            result
        } else {
            tracing::error!("[Forge] Receive reflection failed: syncer not initialized");
            Err("Syncer not initialized".to_string())
        }
    }

    /// Create and register a Skill artifact.
    ///
    /// Mirrors Go's `Forge.CreateSkill()`. This is a shared method used by
    /// both the `forge_create` tool and the `LearningEngine`.
    ///
    /// Steps:
    /// 1. Auto-generate frontmatter if missing.
    /// 2. Write skill content to `{forge_dir}/skills/{name}/SKILL.md`.
    /// 3. Register the artifact in the registry.
    /// 4. Copy to `{workspace}/skills/{name}-forge/SKILL.md`.
    /// 5. Run validation pipeline if auto_validate is enabled.
    pub fn create_skill(
        &self,
        name: &str,
        content: &str,
        description: &str,
        tool_signature: Vec<String>,
    ) -> Result<nemesis_types::forge::Artifact, String> {
        use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};

        // 防御纵深（路径穿越修复）：name 是路径段
        // （`skills/{name}` 与下方 `skills/{name}-forge` 两处 join），在
        // 本体入口归一，覆盖所有调用方（工具执行器已在入口 slugify，这里
        // 兜底 learning_engine 等其余 caller 与历史回归）。空名诚实报错。
        let name = slugify_skill_name(name);
        if name.is_empty() {
            return Err("skill name contains no usable characters (need [a-z0-9_-])".into());
        }
        // Windows 保留设备名闸（BUG-20261123-035）：slugify 对 "nul"/"con"
        // 等恒等放行——保留名目录创建在 Windows 响亮失败（cryptic OS error
        // 而非明确拒绝），Linux 上能落盘但跨节点同步到 Windows 侧即成
        // 不可读死档。围栏统一拒绝（与 web/CLI test/optimize 同一判定），
        // 覆盖工具执行器之外的全部 caller（learning_engine 蒸馏等）。
        valid_skill_dir_name(&name)?;

        // F-: frontmatter must be at the very start — `contains("---")`
        // treated any Markdown horizontal rule as existing frontmatter and
        // skipped adding the wrapper.
        let skill_content = if content.starts_with("---") {
            content.to_string()
        } else {
            format!(
                "---\nname: {}\ndescription: {}\n---\n\n{}",
                name, description, content
            )
        };

        // Security gate (F-): block dangerous commands (rm -rf /, curl|bash)
        // and hardcoded secrets BEFORE writing the file — previously the file
        // was written first with only a weak post-write check, so such content
        // became an active agent instruction. Length/frontmatter are quality,
        // not security, so they don't block here.
        let sec_errors = crate::validator::StaticValidator::new().security_errors(&skill_content);
        if !sec_errors.is_empty() {
            return Err(format!(
                "Skill content failed security validation: {}",
                sec_errors.join("; ")
            ));
        }

        // 1. Write content to forge skills directory
        let artifact_dir = self.forge_dir.join("skills").join(&name);
        let artifact_path = artifact_dir.join("SKILL.md");
        std::fs::create_dir_all(&artifact_dir).map_err(|e| format!("create dir failed: {}", e))?;
        std::fs::write(&artifact_path, &skill_content)
            .map_err(|e| format!("write file failed: {}", e))?;

        // 2. Register in registry
        let artifact_id = format!("skill-{}", name);
        let artifact = Artifact {
            id: artifact_id.clone(),
            name: name.to_string(),
            kind: ArtifactKind::Skill,
            version: "1.0".to_string(),
            status: match self.config.artifacts.default_status.as_str() {
                "active" => ArtifactStatus::Active,
                "observing" => ArtifactStatus::Observing,
                _ => ArtifactStatus::Draft,
            },
            content: skill_content.clone(),
            tool_signature,
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
            usage_count: 0,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
            deployed_at: None,
            last_efficacy: None,
        };

        self.registry.add(artifact.clone());

        // 3. Copy to workspace/skills/ with -forge suffix
        // （副本失败 WARN 留痕不回滚：forge 主副本已落盘已注册（上方 ? 诚实
        // 透传），缺副本=skills 列表看不见；整单报错=假失败，横扫族 C 四轮，
        // 2026-11-06。）
        let workspace_skill_dir = self
            .workspace
            .join("skills")
            .join(format!("{}-forge", name));
        // 原子落盘（BUG-20261123-007，家族唯一真相源；失败 WARN 留痕不回滚
        // 语义不变）。
        let ws_copy_write = std::fs::create_dir_all(&workspace_skill_dir)
            .map_err(|e| e.to_string())
            .and_then(|_| {
                nemesis_utils::file::write_file_atomic(
                    &workspace_skill_dir.join("SKILL.md").to_string_lossy(),
                    skill_content.as_bytes(),
                    0o644,
                )
            });
        if let Err(e) = ws_copy_write {
            tracing::warn!(
                path = %workspace_skill_dir.display(),
                error = %e,
                "[forge] workspace skill copy failed (forge primary copy intact)"
            );
        }

        // 4. Auto-validate if configured
        let mut final_artifact = artifact;
        if self.config.validation.auto_validate
            && let Some(ref pipeline) = self.pipeline
        {
            let validation = pipeline.validate(ArtifactKind::Skill, &name, &skill_content);
            let new_status = pipeline.determine_status(&validation);
            self.registry.update(&artifact_id, |a| {
                a.status = new_status;
            });
            if let Some(updated) = self.registry.get(&artifact_id) {
                final_artifact = updated;
            }
        }

        tracing::info!(
            artifact_id = %final_artifact.id,
            name = %name,
            "[Forge] Created and registered skill artifact"
        );

        Ok(final_artifact)
    }

    // ----- Cleanup methods -----

    /// Remove prompt suggestion files older than `max_age_days`.
    ///
    /// Matches Go's `cleanupPromptSuggestions`. Scans the `workspace/prompts/`
    /// directory for files matching `*_suggestion.md` and removes any whose
    /// modification time is older than the cutoff.
    pub fn cleanup_prompt_suggestions(&self, max_age_days: i64) {
        let prompts_dir = self.workspace.join("prompts");
        if !prompts_dir.exists() {
            return;
        }

        let cutoff = chrono::Local::now() - chrono::Duration::days(max_age_days);

        let entries = match std::fs::read_dir(&prompts_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.ends_with("_suggestion.md") {
                continue;
            }
            if let Ok(metadata) = entry.metadata()
                && let Ok(modified) = metadata.modified()
            {
                let modified_time: chrono::DateTime<chrono::Local> = modified.into();
                if modified_time < cutoff {
                    // 清理面语义（横扫族 C 三轮，2026-11-06）：过期建议删除
                    // 失败 WARN 留痕（旧 `let _` 静默——滞留无任何线索），
                    // 下轮周期自然重试。
                    nemesis_utils::fsutil::remove_file_resilient(entry.path());
                }
            }
        }
    }
}

/// 批内候选场景一致性判定（洞察页与场景拆分 2026-11-24）：**全部**候选
/// 都带同一非 None 项目戳 → Some(该项目)；任一 None（对话会话混入）/
/// 混合项目 / 全对话 / 空批 = None（对话）。纯函数，单测在
/// [`crate::forge::tests`]。
fn unanimous_scope(candidates: &[crate::session_evidence::CorrectionCandidate]) -> Option<String> {
    let mut scopes = candidates.iter().map(|c| c.draft.scope.clone());
    let Some(Some(first)) = scopes.next() else {
        return None;
    };
    if scopes.all(|s| s.as_deref() == Some(first.as_str())) {
        Some(first)
    } else {
        None
    }
}

/// 按 artifact id 去重合并两份产物列表（反思输入并集单一真相源）。
///
/// primary 优先：id 冲突时保留 primary 侧条目（手动库是人工在环真相源）。
/// pub 自由函数：[`Forge::merged_artifacts_for_reflection`]（Forge 实例
/// 路径）与 web 手动反思 handler（无 Forge 实例，`Registry::peek_sync`
/// 读 learning_registry.json 后调用本函数）共用同一合并规则，防再次
/// 分叉（BUG 批次 2026-11-15：web/CLI 两个注入点漏并集的根因就是
/// 合并逻辑长在 Forge 方法里，无实例的入口接不上）。
pub fn merge_artifacts_by_id(
    primary: Vec<nemesis_types::forge::Artifact>,
    secondary: Vec<nemesis_types::forge::Artifact>,
) -> Vec<nemesis_types::forge::Artifact> {
    let mut merged = primary;
    let seen: std::collections::HashSet<String> = merged.iter().map(|a| a.id.clone()).collect();
    merged.extend(secondary.into_iter().filter(|a| !seen.contains(&a.id)));
    merged
}

/// Normalize an LLM-produced skill name to a filesystem-safe kebab-case slug.
///
/// 蒸馏草稿名是 LLM 自由文本，落盘前归一：小写、非 `[a-z0-9_-]` 折叠为
/// `-`、压缩连续 `-`、去首尾 `-`、截断 48 字符。空名/全非法字符返回空串
/// （调用方据以跳过，不猜不救）。pub(crate)：learning_engine 的
/// execute_create_skill 落盘点共用同一规则（防御纵深）。
pub(crate) fn slugify_skill_name(name: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            let c = c.to_ascii_lowercase();
            let is_dash = c == '-' || c == '_';
            if is_dash && (slug.is_empty() || last_dash) {
                continue;
            }
            slug.push(if is_dash { '-' } else { c });
            last_dash = is_dash;
        } else {
            if !slug.is_empty() && !last_dash {
                slug.push('-');
                last_dash = true;
            }
        }
        if slug.len() >= 48 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// slug 产物 + 空 slug 占位回落（`_unnamed_`）的统一规则——exporter 写面、
/// forge_tools resolve_artifact_path、learning_engine disable 面共用的单一
/// 真相源（此前三处各自内联，learning_engine disable 面漏配 = N 轮复查修）。
///
/// 注册表是磁盘态数据，历史遗留的中毒名（旧 create 弱消毒时期写入的
/// `../x` 等穿越序列、`..`/`../..` 等纯符号名）流进下游 join 仍可越出目标
/// 目录。合法名（slug 产物）经 slugify 后逐字节不变，零行为差异；空 slug
/// = 全非法输入，回落受控占位段把路径关在目标子目录内（占位段永不命中
/// 正常产物：两条 create 生产路径都在空 slug 处诚实拒绝，从不落库）。
/// 注意回落值不是 slugify 不动点（首 `_` 会被吞）——调用方不要把本函数
/// 产物再喂回 slugify（exporter export() 的 M 教训）。
pub(crate) fn slug_or_placeholder(name: &str) -> String {
    let slug = slugify_skill_name(name);
    if slug.is_empty() {
        "_unnamed_".to_string()
    } else {
        slug
    }
}

/// 技能目录名合法性围栏（跨入口统一定义）：必须是单段 kebab 目录
/// 名。`../x` 直拼 join 有穿越面；`C:evil` 盘符相对形态 `is_absolute()==false`
/// 且无分隔符，但 join 会整段换基座（Windows）；`.`/`..`/空串同理拒。
/// Windows 保留设备名族（BUG-20261123-035）：裸名与带扩展名形态（base
/// 判定）一并拒绝——目录创建响亮失败尚算诚实，但裸名 FILE 写入（script
/// 型工件 `forge/scripts/{name}`）报成功而数据永不落盘（Win11 实测，见
/// nemesis-utils sanitize 同族注释）= 静默丢数据；判定复用 nemesis-utils
/// 22 名单单一真相源（skills installer / image_gen 同源）。合法域 =
/// `slugify_skill_name` 产物域去除保留设备名。web `forge.test` 与 CLI
/// `forge test` / `forge optimize` 三入口共用同一判定（版本段消费方
/// exporter/forge_tools 基底为数字恒不命中，零行为漂移）。
pub fn valid_skill_dir_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name == "."
        || name == ".."
        // Win32 归一化族：路径组件的尾随点/空格在 Win32 路径解析中被
        // 剥除——`.. `、`.. .` 等形态与 `..` 等价（父目录穿越面）；
        // 首尾空白 trim 后可能落回 `.`/`..`。一律拒绝（消费面虽经
        // Rust std 字面传 NT 层当前不可穿越，围栏按契约自洽封死，
        // 不依赖文件系统行为兜底）。
        || name != name.trim()
        || name.trim_end_matches(['.', ' ']) != name.trim()
        || nemesis_utils::sanitize::is_windows_reserved_device_base(name)
    {
        return Err(format!("invalid skill name: {name}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
