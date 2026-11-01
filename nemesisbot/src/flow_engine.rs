//! P3 flow 引擎（2026-10-31 插件 UI 面板与流程扩展）——宿主侧通用剧本执行器。
//!
//! 声明式 wasm 面板插件的 `[[flow.template]]` 段在这里变成可执行流水线：
//! gateway 装配期 spawn 周期 tick（external_source::spawn 同款模式），按
//! `interval_secs` 触发 `connector_fetch` 取数建实例，并推进所有 active
//! 实例的状态机；面板 `flow://` 绑定经 [`FlowBridge`] 代答查询/确认/手动
//! 触发。框架零领域硬编码——剧本由插件 toml 声明，原语绑定既有件
//! （connector 工具面 / external_source 建单链 / board 派发评审 /
//! publish / dossier）。
//!
//! 步骤生命周期：同步步（board_land）发起即完成；观察步（agent_fixloop /
//! check_regress）发起转 Running，逐 tick 轮询 board 状态裁决；执行步
//! （publish / report）发起转 Running，poll 期执行（幂等面：publish 按
//! task_id 幂等含 unknown 对账 / report 重导出覆盖）。board 单是执行真相
//! 源，flow 状态只是编排投影。
//!
//! 关键不变量：
//! - **磁盘 = 真相**：实例状态落 `<workspace>/logs/flows/<template>/
//!   <instance>.json`，无内存注册表（重启即恢复，进程内零双份）；
//! - **gate = manual**：实例停 `awaiting_gate`，面板确认（confirm）后续跑；
//! - estop 冻结 tick（external_source 同款 `is_engaged()`）；写操作由
//!   `actions` 互斥锁串行（fetch/advance/confirm/resume），读代答无锁
//!   （盘读=一致快照）。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use nemesis_agent::estop::EstopState;
use nemesis_agent::r#loop::AgentLoop;
use nemesis_board::store::BoardStore;
use nemesis_plugins_wasm::manifest::{FlowStep, FlowTemplate, PluginKind, PluginManifest};

// 与 external_source 同款私有别名：cluster 编译形态挂 Cluster 引用做派发，
// 非 cluster 形态诚实不可派发（web 侧 dispatch_issue_for_external_source
// 的非 cluster 臂恒 false，同款纪律）。
#[cfg(feature = "cluster")]
type ClusterOwned = Option<Arc<nemesis_cluster::cluster::Cluster>>;
#[cfg(not(feature = "cluster"))]
type ClusterOwned = ();

const KIND_FETCH: &str = "connector_fetch";
const KIND_LAND: &str = "board_land";
const KIND_FIXLOOP: &str = "agent_fixloop";
const KIND_REGRESS: &str = "check_regress";
const KIND_PUBLISH: &str = "publish";
const KIND_REPORT: &str = "report";

/// 建单 origin_type（external_source 的 TaskOrigin 去重面同源消费）。
const ORIGIN_TYPE: &str = "flow";

/// tick 启动延迟（等 gateway 其余面就绪）。
const STARTUP_DELAY_SECS: u64 = 20;
/// tick 周期。
const LOOP_TICK_SECS: u64 = 5;
/// estop 冻结期的退避（避免空转烧 CPU）。
const ESTOP_BACKOFF_SECS: u64 = 30;
/// fetch 失败后的最小重试间隔（interval 闸天然下限——on_fail=retry 的
/// 真实节奏）。
const MIN_FETCH_INTERVAL_DOC: &str = "interval_secs 即 fetch 重试下限";
/// 错误摘要截断长度。
const ERR_SNIPPET: usize = 300;

/// poll 仍运行中的哨兵错误——fail_step 识别后静默返回（不记账不推进）。
const FLOW_STILL_RUNNING: &str = "\u{0}__flow_still_running__";

// ---------------------------------------------------------------------------
// 模型
// ---------------------------------------------------------------------------

/// 步骤执行状态（实例内逐步骤记账）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// 未开始（gate 未确认时也停在这里）。
    Pending,
    /// 进行中（已发起，等后续 tick 观察/执行）。
    Running,
    /// 完成。
    Ok,
    /// 失败（on_fail 分支裁决后可能重试/停车/带隙续跑）。
    Failed,
    /// 条件步不成立被跳过（决策 B：when=false 同 tick 推进，留痕不执行）。
    Skipped,
}

impl StepStatus {
    /// serde rename_all = "snake_case" 的文本形态（条件步 steps.<id>.status
    /// 投影与 timeline state 词汇共用，与磁盘序列化字节一致）。
    fn as_str(&self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Running => "running",
            StepStatus::Ok => "ok",
            StepStatus::Failed => "failed",
            StepStatus::Skipped => "skipped",
        }
    }
}

/// 步骤运行时状态（模板声明 + 执行投影）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StepRun {
    pub id: String,
    pub kind: String,
    pub status: StepStatus,
    /// 失败重试计数（on_fail=retry:N 的 N 是上限）。
    pub attempts: u32,
    /// `manual` = 面板卡点；空 = 无卡点。
    pub gate: String,
    /// 条件声明（决策 B）：空 = 无条件恒执行；非空 = 单比较式
    /// `{{token}} <op> <值>`，host 侧求值（见 eval_when）——false 跳过本步，
    /// 畸形按失败走 on_fail。
    pub when: String,
    /// `park_human` | `retry:N` | `continue`；空 = park_human（防御默认）。
    pub on_fail: String,
    /// gate 已确认（manual 卡点的放行凭据）。
    pub gate_confirmed: bool,
    /// 步骤声明参数（FlowStep.args 原样投影——publish/report/project 等
    /// 参数面从这里读）。
    pub args: BTreeMap<String, String>,
    /// 步骤产物（board 单号/PR 号/报告路径等，JSON 对象）。
    pub artifacts: Value,
    pub error: Option<String>,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
}

impl Default for StepRun {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: String::new(),
            status: StepStatus::Pending,
            attempts: 0,
            gate: String::new(),
            when: String::new(),
            on_fail: String::new(),
            gate_confirmed: false,
            args: BTreeMap::new(),
            artifacts: json!({}),
            error: None,
            started_at_ms: None,
            finished_at_ms: None,
        }
    }
}

impl StepRun {
    fn from_decl(step: &FlowStep) -> Self {
        StepRun {
            id: step.id.clone(),
            kind: step.kind.clone(),
            gate: step.gate.clone(),
            when: step.when.clone(),
            on_fail: step.on_fail.clone(),
            args: step.args.clone(),
            ..Default::default()
        }
    }

    fn args_get(&self, key: &str) -> Option<String> {
        self.args.get(key).cloned()
    }
}

/// 流水线实例（一个外部条目一条）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FlowInstance {
    pub template: String,
    /// 实例键（sanitize_path_segment 后的文件名安全形态）。
    pub instance: String,
    /// 外部原始键（回写/展示用）。
    pub ext_key: String,
    pub title: String,
    /// 游标（steps 下标）。
    pub current_step: usize,
    pub steps: Vec<StepRun>,
    /// 取数原始条目（后续步骤的参数源）。
    pub entry: Value,
    /// on_fail=continue 产生的缺口账（不停车，诚实记账）。
    pub gaps: Vec<Value>,
    /// 停车原因（park_human / 重试超限）；非空 = 实例停车待人。
    pub parked: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Default for FlowInstance {
    fn default() -> Self {
        let now = now_ms();
        Self {
            template: String::new(),
            instance: String::new(),
            ext_key: String::new(),
            title: String::new(),
            current_step: 0,
            steps: Vec::new(),
            entry: json!({}),
            gaps: Vec::new(),
            parked: None,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
        .unwrap_or_default()
}

/// on_fail 声明解析：`continue` / `retry:N`（1..=10）/ 其他 = park_human
/// （manifest 校验已拦非法值；此处防御性默认）。
fn parse_on_fail(raw: &str) -> OnFail {
    match raw.trim() {
        "continue" => OnFail::Continue,
        other => {
            if let Some(rest) = other.strip_prefix("retry:")
                && let Ok(n) = rest.trim().parse::<u32>()
                && (1..=10).contains(&n)
            {
                return OnFail::Retry(n);
            }
            OnFail::ParkHuman
        }
    }
}

enum OnFail {
    ParkHuman,
    Retry(u32),
    Continue,
}

// ---------------------------------------------------------------------------
// 状态投影（纯函数，单测钉死）
// ---------------------------------------------------------------------------

fn activity_name(kind: &str) -> &'static str {
    match kind {
        KIND_FETCH => "fetching",
        KIND_LAND => "landing",
        KIND_FIXLOOP => "fixing",
        KIND_REGRESS => "regressing",
        KIND_PUBLISH => "delivering",
        KIND_REPORT => "reporting",
        _ => "working",
    }
}

/// 实例展示态（面板 state 徽标词汇）。优先级：parked > done > gate 等待 >
/// 活动态。
fn state_of(inst: &FlowInstance) -> String {
    if inst.parked.is_some() {
        return "parked".to_string();
    }
    if inst.current_step >= inst.steps.len() {
        return "done".to_string();
    }
    let cur = &inst.steps[inst.current_step];
    match cur.status {
        StepStatus::Pending if !cur.gate.is_empty() && !cur.gate_confirmed => {
            "awaiting_gate".to_string()
        }
        _ => activity_name(&cur.kind).to_string(),
    }
}

/// 取实例账面上最近一次出现的产物键（rev find：后步覆盖前步）。
fn latest_artifact_str<'a>(inst: &'a FlowInstance, key: &str) -> &'a str {
    inst.steps
        .iter()
        .rev()
        .find_map(|s| s.artifacts.get(key).and_then(|v| v.as_str()))
        .unwrap_or("")
}

// ---------------------------------------------------------------------------
// FlowStore：磁盘真相 + 代答投影
// ---------------------------------------------------------------------------

/// 实例存储（`<workspace>/logs/flows/<template>/<instance>.json`）。磁盘 =
/// 真相，无内存注册表；所有代答投影（list/detail/timeline/confirm/resume）
/// 都住这里，脱离引擎可单测。
pub struct FlowStore {
    root: PathBuf,
}

impl FlowStore {
    pub fn new(workspace: &Path) -> Self {
        FlowStore {
            root: workspace.join("logs").join("flows"),
        }
    }

    fn template_dir(&self, template: &str) -> PathBuf {
        self.root.join(template)
    }

    fn instance_path(&self, template: &str, instance: &str) -> PathBuf {
        self.template_dir(template).join(format!("{instance}.json"))
    }

    pub fn exists(&self, template: &str, instance: &str) -> bool {
        self.instance_path(template, instance).is_file()
    }

    /// 原子落盘（tmp + 换名）；删旧残留再 rename（Windows rename 语义）。
    pub fn save(&self, inst: &FlowInstance) -> Result<(), String> {
        let dir = self.template_dir(&inst.template);
        std::fs::create_dir_all(&dir).map_err(|e| format!("flow dir: {e}"))?;
        let path = self.instance_path(&inst.template, &inst.instance);
        let tmp = path.with_extension("json.tmp");
        let body =
            serde_json::to_string_pretty(inst).map_err(|e| format!("serialize flow: {e}"))?;
        std::fs::write(&tmp, body).map_err(|e| format!("write flow tmp: {e}"))?;
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename flow: {e}"))
    }

    pub fn load(&self, template: &str, instance: &str) -> Result<FlowInstance, String> {
        let path = self.instance_path(template, instance);
        let raw = std::fs::read_to_string(&path)
            .map_err(|_| format!("流程实例不存在: {template}/{instance}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("流程实例损坏: {template}/{instance}: {e}"))
    }

    /// 列出模板下全部实例（文件名排序——read_dir 顺序是平台实现细节）。
    fn list_all(&self, template: &str) -> Vec<FlowInstance> {
        let dir = self.template_dir(template);
        let mut names: Vec<String> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                .filter(|n| n.ends_with(".json"))
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
            .into_iter()
            .filter_map(|n| {
                let instance = n.trim_end_matches(".json").to_string();
                match self.load(template, &instance) {
                    Ok(inst) => Some(inst),
                    Err(_) => {
                        tracing::warn!("[FlowEngine] 实例文件损坏跳过: {template}/{instance}");
                        None
                    }
                }
            })
            .collect()
    }

    /// 指定实例（args.instance 显式点名）；缺省 = 最近更新的一条。
    pub fn pick(&self, template: &str, instance: Option<&str>) -> Result<FlowInstance, String> {
        match instance {
            Some(name) if !name.trim().is_empty() => self.load(template, name.trim()),
            _ => self
                .list_all(template)
                .into_iter()
                .max_by_key(|i| i.updated_at_ms)
                .ok_or_else(|| format!("流程 {template} 尚无实例")),
        }
    }

    /// `list` 投影（PanelTable 行）。
    pub fn answer_list(&self, template: &str) -> Result<Value, String> {
        let mut rows: Vec<FlowInstance> = self.list_all(template);
        rows.sort_by_key(|r| std::cmp::Reverse(r.updated_at_ms));
        let rows: Vec<Value> = rows
            .iter()
            .map(|inst| {
                json!({
                    "instance": inst.instance,
                    "ext_key": inst.ext_key,
                    "title": inst.title,
                    "state": state_of(inst),
                    "step": inst.steps.get(inst.current_step).map(|s| s.id.clone()).unwrap_or_default(),
                    "board_issue": latest_artifact_str(inst, "board_issue"),
                    "updated_at": rfc3339(inst.updated_at_ms),
                    "gaps": inst.gaps.len(),
                    "parked": inst.parked.clone().unwrap_or_default(),
                })
            })
            .collect();
        Ok(json!({ "rows": rows }))
    }

    /// `detail` 投影（PanelDetail：全部步骤 + 产物 + 缺口 + 停车原因）。
    pub fn answer_detail(&self, template: &str, instance: Option<&str>) -> Result<Value, String> {
        let inst = self.pick(template, instance)?;
        let steps: Vec<Value> = inst
            .steps
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "kind": s.kind,
                    "status": s.status,
                    "attempts": s.attempts,
                    "gate": s.gate,
                    "error": s.error,
                    "started_at": s.started_at_ms.map(rfc3339).unwrap_or_default(),
                    "finished_at": s.finished_at_ms.map(rfc3339).unwrap_or_default(),
                    "artifacts": s.artifacts,
                })
            })
            .collect();
        Ok(json!({
            "instance": inst.instance,
            "ext_key": inst.ext_key,
            "title": inst.title,
            "state": state_of(&inst),
            "current_step": inst.steps.get(inst.current_step).map(|s| s.id.clone()).unwrap_or_default(),
            "board_issue": latest_artifact_str(&inst, "board_issue"),
            "steps": steps,
            "gaps": inst.gaps,
            "parked": inst.parked,
        }))
    }

    /// `timeline` 投影（PanelTimeline 词汇：events[] {title,detail,ts_ms,state}）。
    pub fn answer_timeline(&self, template: &str, instance: Option<&str>) -> Result<Value, String> {
        let inst = self.pick(template, instance)?;
        let mut events: Vec<Value> = Vec::new();
        for s in &inst.steps {
            let title = format!("{}（{}）", s.id, s.kind);
            if let Some(ts) = s.started_at_ms {
                events.push(
                    json!({ "title": title, "detail": "开始执行", "ts_ms": ts, "state": "running" }),
                );
            }
            if let Some(ts) = s.finished_at_ms {
                match s.status {
                    StepStatus::Ok => events.push(
                        json!({ "title": title, "detail": "完成", "ts_ms": ts, "state": "ok" }),
                    ),
                    StepStatus::Failed => events.push(json!({
                        "title": title,
                        "detail": s.error.clone().unwrap_or_else(|| "失败".to_string()),
                        "ts_ms": ts,
                        "state": "failed",
                    })),
                    StepStatus::Skipped => {
                        let why = s
                            .artifacts
                            .get("skipped_when")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        events.push(json!({
                            "title": title,
                            "detail": format!("条件不成立已跳过（{why}）"),
                            "ts_ms": ts,
                            "state": "skipped",
                        }));
                    }
                    _ => {}
                }
            }
        }
        for g in &inst.gaps {
            let ts = g.get("ts_ms").and_then(|v| v.as_i64()).unwrap_or(0);
            let step = g.get("step").and_then(|v| v.as_str()).unwrap_or("");
            let err = g.get("error").and_then(|v| v.as_str()).unwrap_or("");
            events.push(json!({
                "title": format!("{step} 缺口"),
                "detail": err,
                "ts_ms": ts,
                "state": "error",
            }));
        }
        if let Some(reason) = &inst.parked {
            events.push(json!({
                "title": "停车待处理",
                "detail": reason,
                "ts_ms": inst.updated_at_ms,
                "state": "pending",
            }));
        }
        events.sort_by_key(|e| e.get("ts_ms").and_then(|v| v.as_i64()).unwrap_or(0));
        Ok(json!({ "events": events }))
    }

    /// `confirm`：放行当前 manual gate（面板确认按钮）。
    pub fn answer_confirm(&self, template: &str, instance: Option<&str>) -> Result<Value, String> {
        let mut inst = self.pick(template, instance)?;
        if inst.parked.is_some() {
            return Err(format!(
                "流程 {template}/{} 已停车（{}）——先在面板处理停车原因再确认",
                inst.instance,
                inst.parked.clone().unwrap_or_default()
            ));
        }
        let step = inst
            .steps
            .get_mut(inst.current_step)
            .ok_or_else(|| format!("流程 {template}/{} 已完成", inst.instance))?;
        if step.status != StepStatus::Pending {
            return Err(format!(
                "流程 {template}/{} 当前步骤 {} 非 Pending，无可确认 gate",
                inst.instance, step.id
            ));
        }
        if step.gate.is_empty() {
            return Err(format!(
                "流程 {template}/{} 当前步骤 {} 无 gate",
                inst.instance, step.id
            ));
        }
        step.gate_confirmed = true;
        inst.updated_at_ms = now_ms();
        self.save(&inst)?;
        Ok(json!({ "ok": true, "message": format!("已确认，{} 将在下一轮推进", inst.instance) }))
    }

    /// `resume`：停车实例复活——清停车标记、当前失败步骤回 Pending。
    pub fn answer_resume(&self, template: &str, instance: Option<&str>) -> Result<Value, String> {
        let mut inst = self.pick(template, instance)?;
        if inst.parked.is_none() {
            return Err(format!(
                "流程 {template}/{} 未停车，无需恢复",
                inst.instance
            ));
        }
        inst.parked = None;
        if let Some(step) = inst.steps.get_mut(inst.current_step)
            && step.status == StepStatus::Failed
        {
            step.status = StepStatus::Pending;
            step.error = None;
        }
        inst.updated_at_ms = now_ms();
        self.save(&inst)?;
        Ok(json!({ "ok": true, "message": format!("{} 已恢复推进", inst.instance) }))
    }
}

// ---------------------------------------------------------------------------
// board 绑定（自由函数面：吃 &BoardStore，脱离引擎可单测）
// ---------------------------------------------------------------------------

fn flow_origin_id(template: &str, ext_key: &str) -> String {
    format!("{template}/{ext_key}")
}

/// 按取数 origin 找回 board 单（幂等锚：land/fixloop/regress/publish/
/// report 五步共用）。
fn find_board_issue(
    store: &BoardStore,
    inst: &FlowInstance,
) -> Result<nemesis_board::Issue, String> {
    let origin_id = flow_origin_id(&inst.template, &inst.ext_key);
    let origin_hit = store
        .list_issues_by_origin(ORIGIN_TYPE, &origin_id, 1)
        .map_err(|e| format!("board origin 查询失败: {e}"))?
        .into_iter()
        .next();
    origin_hit.ok_or_else(|| format!("流程 {}/{} 未落 board 单", inst.template, inst.instance))
}

/// `board_land`：按 origin 幂等建 board 单（external_source::import_entry
/// 同款去重面 + NewIssue 字面量形态；title 不带 origin 前缀——origin 已是
/// 独立数据槽）。args.project 可选：按项目名归属项目（缺席 = 不归属）。
fn start_board_land(
    store: &Arc<BoardStore>,
    inst: &mut FlowInstance,
    project: Option<&str>,
) -> Result<Value, String> {
    let origin_id = flow_origin_id(&inst.template, &inst.ext_key);
    let existing = store
        .list_issues_by_origin(ORIGIN_TYPE, &origin_id, 1)
        .map_err(|e| format!("board origin 查询失败: {e}"))?
        .into_iter()
        .next();
    if let Some(existing) = existing {
        return Ok(json!({
            "board_issue": existing.number,
            "board_issue_id": existing.id,
            "landed": "existing",
        }));
    }
    let project_id = match project.map(str::trim).filter(|s| !s.is_empty()) {
        Some(pname) => {
            let projects = store
                .list_projects()
                .map_err(|e| format!("项目清单读取失败: {e}"))?;
            let proj = projects
                .into_iter()
                .find(|p| p.name == pname)
                .ok_or_else(|| format!("项目不存在: {pname}"))?;
            Some(proj.id)
        }
        None => None,
    };
    let issue = store.create_issue(nemesis_board::models::NewIssue {
        title: inst.title.clone(),
        description: format!(
            "来源: {ORIGIN_TYPE} {origin_id}\n\n外部条目: {}",
            inst.ext_key
        ),
        priority: nemesis_board::models::priority::MEDIUM,
        assignee: None,
        assignee_id: None,
        creator: nemesis_board::Actor::system("flow-engine"),
        parent_issue_id: None,
        project_id,
        due_date: None,
        acceptance_criteria: None, // ac_gen 自动生成（NB-125 单一真相点）
        origin: Some(nemesis_board::models::TaskOrigin {
            origin_type: ORIGIN_TYPE.to_string(),
            origin_id: origin_id.clone(),
        }),
        required_role: None,
        required_tags: vec!["外部".to_string()],
        required_profession: None,
        required_capabilities: Vec::new(),
        required_backend: None,
    })?;
    Ok(json!({ "board_issue": issue.number, "board_issue_id": issue.id, "landed": "created" }))
}

/// `agent_fixloop` 启动：Backlog/Todo → 派发（injector 决定成败）；进行中
/// 状态 → 直接观察态。两段式分界：InReview|Done 即算工作交付（fixloop
/// 完成），验收结论归 `check_regress`。
fn start_fixloop(
    store: &Arc<BoardStore>,
    inst: &mut FlowInstance,
    dispatch: &dyn Fn(&nemesis_board::Issue) -> bool,
) -> Result<Value, String> {
    let issue = find_board_issue(store, inst)?;
    match issue.status {
        nemesis_board::models::IssueStatus::Backlog | nemesis_board::models::IssueStatus::Todo => {
            if dispatch(&issue) {
                Ok(json!({ "dispatched": true, "board_issue": issue.number }))
            } else {
                Err(format!(
                    "{} 派发未成行（无可用 worker/集群未启用）",
                    issue.number
                ))
            }
        }
        nemesis_board::models::IssueStatus::InProgress
        | nemesis_board::models::IssueStatus::InReview
        | nemesis_board::models::IssueStatus::Done => {
            Ok(json!({ "dispatched": false, "board_issue": issue.number, "observing": true }))
        }
        other => Err(format!(
            "{} 状态 {} 不可派发（停车/取消单不自动续跑）",
            issue.number,
            other.as_str()
        )),
    }
}

#[derive(Debug)]
enum PollOutcome {
    Completed(Value),
    Failed(String),
    StillRunning,
}

/// `agent_fixloop` 观察：InReview|Done = 工作交付完成；Cancelled/Blocked =
/// 失败；其余（Backlog 派发在途/InProgress 执行中）继续等。
fn poll_fixloop(store: &Arc<BoardStore>, inst: &FlowInstance) -> PollOutcome {
    let issue = match find_board_issue(store, inst) {
        Ok(i) => i,
        Err(e) => return PollOutcome::Failed(e),
    };
    match issue.status {
        nemesis_board::models::IssueStatus::InReview | nemesis_board::models::IssueStatus::Done => {
            PollOutcome::Completed(json!({ "board_issue": issue.number, "delivered": true }))
        }
        nemesis_board::models::IssueStatus::Cancelled
        | nemesis_board::models::IssueStatus::Blocked => PollOutcome::Failed(format!(
            "{} 状态 {}——修复循环终止",
            issue.number,
            issue.status.as_str()
        )),
        _ => PollOutcome::StillRunning,
    }
}

/// `check_regress` 观察：只有 Done（评审通过）才算验收成立——结论
/// `accepted`；InReview 继续等评审；Cancelled/Blocked = 失败。
fn regress_outcome(store: &Arc<BoardStore>, inst: &FlowInstance) -> PollOutcome {
    let issue = match find_board_issue(store, inst) {
        Ok(i) => i,
        Err(e) => return PollOutcome::Failed(e),
    };
    match issue.status {
        nemesis_board::models::IssueStatus::Done => PollOutcome::Completed(json!({
            "verdict": "accepted",
            "board_issue": issue.number,
        })),
        nemesis_board::models::IssueStatus::Cancelled
        | nemesis_board::models::IssueStatus::Blocked => PollOutcome::Failed(format!(
            "{} 状态 {}——验收终止",
            issue.number,
            issue.status.as_str()
        )),
        _ => PollOutcome::StillRunning,
    }
}

fn poll_outcome(outcome: PollOutcome) -> Result<Value, String> {
    match outcome {
        PollOutcome::Completed(v) => Ok(v),
        PollOutcome::Failed(e) => Err(e),
        PollOutcome::StillRunning => Err(FLOW_STILL_RUNNING.to_string()),
    }
}

// ---------------------------------------------------------------------------
// token 替换（report 步 connector 参数模板）
// ---------------------------------------------------------------------------

/// `{{key}}` 替换：先实例账面（instance/ext_key/title/board_issue/
/// report_path），再 extra 表。未知 token 原样保留（诚实不猜）。
fn substitute_tokens(tpl: &str, inst: &FlowInstance, extra: &Value) -> String {
    let lookup = |key: &str| -> Option<String> {
        // 保留键先读产物账面（后步覆盖前步）；账面空回落 extra——report 步
        // 执行中产物尚未入账，extra 是唯一活源（P4 e2e 实录：旧实现账面空串
        // 遮蔽 extra，{{report_path}} 恒空）。两处皆无 = None（原样保留，
        // 模板残留可见）。
        let ledger_then_extra = |k: &str| {
            let v = latest_artifact_str(inst, k);
            (!v.is_empty()).then(|| v.to_string()).or_else(|| {
                extra.get(k).map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
            })
        };
        match key {
            "instance" => Some(inst.instance.clone()),
            "ext_key" => Some(inst.ext_key.clone()),
            "title" => Some(inst.title.clone()),
            "board_issue" | "report_path" => ledger_then_extra(key),
            // 步骤投影（决策 B 条件步数据源）：steps.<id>.<field>，field 词表
            // status/error/attempts。未知 id/field = None（token 原样保留，
            // 模板写错可见）。
            other if other.starts_with("steps.") => {
                let (_, rest) = other.split_once('.').expect("prefix checked");
                let (id, field) = rest.split_once('.')?;
                let step = inst.steps.iter().find(|s| s.id == id)?;
                match field {
                    "status" => Some(step.status.as_str().to_string()),
                    "error" => Some(step.error.clone().unwrap_or_default()),
                    "attempts" => Some(step.attempts.to_string()),
                    _ => None,
                }
            }
            other => extra.get(other).map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            }),
        }
    };
    let mut out = String::with_capacity(tpl.len());
    let bytes = tpl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'{'
            && let Some(end) = tpl[i + 2..].find("}}")
        {
            let key = tpl[i + 2..i + 2 + end].trim();
            match lookup(key) {
                Some(v) => out.push_str(&v),
                None => out.push_str(&tpl[i..i + 2 + end + 2]),
            }
            i += 2 + end + 2;
            continue;
        }
        // 按 char 边界推进（非 ASCII 安全）。
        let ch_len = tpl[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&tpl[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// Value 树逐 String 替换：在结构上替换而非序列化串上替换——Windows 路径
/// 反斜杠进序列化串即非法 JSON 转义（P4 e2e 实录第二坑），树内替换后由
/// serde 统一转义。非字符串标量原样克隆。
fn substitute_tokens_value(v: &Value, inst: &FlowInstance, extra: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(substitute_tokens(s, inst, extra)),
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|x| substitute_tokens_value(x, inst, extra))
                .collect(),
        ),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), substitute_tokens_value(x, inst, extra)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// 条目字段取串：支持点路径（`fields.summary`，逐段下钻对象）+ 完整字面量
/// 键兜底（键本身含点且顶层恰有该字面量字段）。真实 Jira 条目形如
/// `{key, fields:{summary}}`——标题在嵌套层。
fn entry_string(entry: &Value, key: &str) -> Option<String> {
    let str_of = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    if key.contains('.') {
        let mut cur = entry;
        let mut walked = true;
        for seg in key.split('.') {
            match cur.get(seg) {
                Some(v) => cur = v,
                None => {
                    walked = false;
                    break;
                }
            }
        }
        if walked && let Some(s) = str_of(cur) {
            return Some(s);
        }
    }
    entry.get(key).and_then(str_of)
}

// ---------------------------------------------------------------------------
// 引擎
// ---------------------------------------------------------------------------

pub struct FlowEngine {
    agent_loop: Arc<AgentLoop>,
    store: Arc<BoardStore>,
    /// publish 复用 publish_task_delivery 的入参（任务 db 独立连接，多连接
    /// WAL 既有纪律）。
    db: Arc<nemesis_taskstore::TaskDb>,
    workspace: PathBuf,
    /// 主 home（usage 账本 workspace_data_dir 的根）。
    home: PathBuf,
    plugins_dir: PathBuf,
    config_dir: PathBuf,
    estop: Arc<EstopState>,
    #[allow(dead_code)] // 非 cluster 形态 ()；cluster 形态派发臂消费
    cluster: ClusterOwned,
    flows: FlowStore,
    /// 写操作互斥（fetch/advance/confirm/resume 串行；读代答无锁）。
    actions: tokio::sync::Mutex<()>,
    /// 模板缓存（目录戳变化才重扫）。
    templates: tokio::sync::Mutex<TemplatesCache>,
    /// 每模板上次 fetch 时刻（Instant 单调钟，interval 闸）。
    last_fetch: tokio::sync::Mutex<HashMap<String, Instant>>,
}

struct TemplatesCache {
    stamp: (u64, usize),
    templates: Vec<FlowTemplateDef>,
}

/// 引擎消费的模板视图（所属插件 slug + 模板声明）。
#[derive(Debug, Clone)]
struct FlowTemplateDef {
    slug: String,
    template: FlowTemplate,
}

impl FlowEngine {
    /// 装配（gateway post_agent 期）。任何输入都不 panic——装配失败走
    /// 返回值，由调用方 warn 降级。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent_loop: Arc<AgentLoop>,
        store: Arc<BoardStore>,
        db: Arc<nemesis_taskstore::TaskDb>,
        workspace: PathBuf,
        home: PathBuf,
        estop: Arc<EstopState>,
        cluster: ClusterOwned,
    ) -> Self {
        let plugins_dir = nemesis_plugins_wasm::registry::resolve_plugins_wasm_dir();
        let flows = FlowStore::new(&workspace);
        FlowEngine {
            agent_loop,
            store,
            db,
            config_dir: workspace.join("config"),
            workspace,
            home,
            plugins_dir,
            estop,
            cluster,
            flows,
            actions: tokio::sync::Mutex::new(()),
            templates: tokio::sync::Mutex::new(TemplatesCache {
                stamp: (0, 0),
                templates: Vec::new(),
            }),
            last_fetch: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// gateway 装配期 spawn 周期循环（external_source::spawn 同款）。
    pub fn spawn_tick_loop(self: &Arc<Self>) {
        let engine = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(STARTUP_DELAY_SECS)).await;
            tracing::info!(
                "[FlowEngine] tick loop started（interval={LOOP_TICK_SECS}s；{MIN_FETCH_INTERVAL_DOC}）"
            );
            loop {
                if engine.estop.is_engaged() {
                    tokio::time::sleep(Duration::from_secs(ESTOP_BACKOFF_SECS)).await;
                    continue;
                }
                if let Err(e) = engine.tick().await {
                    tracing::warn!("[FlowEngine] tick 异常（下轮继续）: {e}");
                }
                tokio::time::sleep(Duration::from_secs(LOOP_TICK_SECS)).await;
            }
        });
    }

    async fn tick(&self) -> Result<(), String> {
        // 运行期总闸（2026-11-01 BUG 修复）：子系统翻关 → tick 空转
        // （fetch/advance 全停，active 实例冻存盘上；开关改回即时恢复）。
        if !nemesis_config::plugin_wasm::runtime_enabled(&self.workspace) {
            return Ok(());
        }
        let templates = self.current_templates().await;
        let now = Instant::now();
        // 1) 到期模板 fetch 建实例。
        for def in &templates {
            if def.template.interval_secs == 0 {
                continue; // 只手动
            }
            let due = {
                let map = self.last_fetch.lock().await;
                map.get(&def.template.id)
                    .map(|t| now.duration_since(*t).as_secs() >= def.template.interval_secs)
                    .unwrap_or(true)
            };
            if !due {
                continue;
            }
            {
                let mut map = self.last_fetch.lock().await;
                map.insert(def.template.id.clone(), now);
            }
            let _guard = self.actions.lock().await;
            if let Err(e) = self.fetch_template(def).await {
                tracing::warn!(
                    "[FlowEngine] fetch {}/{} 失败: {e}",
                    def.slug,
                    def.template.id
                );
            }
        }
        // 2) 推进全部模板的 active 实例。
        for def in &templates {
            let _guard = self.actions.lock().await;
            if let Err(e) = self.advance_template(&def.template.id).await {
                tracing::warn!(
                    "[FlowEngine] advance {}/{} 异常: {e}",
                    def.slug,
                    def.template.id
                );
            }
        }
        Ok(())
    }

    /// 模板缓存：戳（载荷目录 + 开关文件，见 template_stamp）变化才重扫。
    /// 开关文件必须入戳——enable/disable 只写 config.plugin.wasm.json 不碰
    /// 载荷目录，戳不覆盖则禁用后 tick 照跑（真实 connector/board/publish
    /// 副作用）、重启用不恢复（2026-11-01 缺陷 A 根修）。
    async fn current_templates(&self) -> Vec<FlowTemplateDef> {
        let stamp = template_stamp(&self.plugins_dir, &self.config_dir);
        {
            let cache = self.templates.lock().await;
            if cache.stamp == stamp && !cache.templates.is_empty() {
                return cache.templates.clone();
            }
        }
        let templates = scan_templates(&self.plugins_dir, &self.config_dir);
        let mut cache = self.templates.lock().await;
        cache.stamp = stamp;
        cache.templates = templates.clone();
        templates
    }

    /// `fetch` 代答：手动触发一次取数（绕 interval 闸）。
    async fn answer_fetch(&self, template: &str) -> Result<Value, String> {
        let templates = self.current_templates().await;
        let def = templates
            .iter()
            .find(|d| d.template.id == template)
            .ok_or_else(|| format!("流程模板不存在: {template}"))?;
        let _guard = self.actions.lock().await;
        self.fetch_template(def).await
    }

    // -- fetch 建实例 -------------------------------------------------------

    /// connector_fetch：取数 → 逐条目物化实例（幂等：已存在跳过）。取数失败
    /// 诚实报错（tick 记 warn；interval 闸天然限频）。
    async fn fetch_template(&self, def: &FlowTemplateDef) -> Result<Value, String> {
        let steps = &def.template.steps;
        let first = steps
            .first()
            .ok_or_else(|| format!("模板 {} 无步骤", def.template.id))?;
        if first.kind != KIND_FETCH {
            return Err(format!(
                "模板 {} 首步必须是 {KIND_FETCH}（当前 {}）",
                def.template.id, first.kind
            ));
        }
        let arg = |key: &str| first.args.get(key).cloned().unwrap_or_default();
        let (connector, op) = (arg("connector"), arg("op"));
        if connector.is_empty() || op.is_empty() {
            return Err(format!(
                "模板 {} fetch 步缺 connector/op 声明",
                def.template.id
            ));
        }
        let args_json = match first.args.get("args_json") {
            Some(raw) if !raw.trim().is_empty() => serde_json::from_str::<Value>(raw)
                .map_err(|e| format!("模板 {} args_json 非法: {e}", def.template.id))?,
            _ => json!({}),
        };
        let text = self.invoke_connector(&connector, &op, &args_json).await?;
        let entries = parse_entries(&text)?;
        let created = materialize_instances(&self.flows, def, &entries)?;
        Ok(json!({
            "template": def.template.id,
            "entries": entries.len(),
            "created": created,
        }))
    }

    /// connector 工具面调用（external_source 同源路径：过完整 dispatch
    /// 瀑布 + 审计 + estop）。
    async fn invoke_connector(
        &self,
        connector: &str,
        op: &str,
        args: &Value,
    ) -> Result<String, String> {
        let call = nemesis_agent::types::ToolCallInfo {
            id: format!("flow-{}", uuid::Uuid::new_v4()),
            name: crate::connector_tools::INVOKE_TOOL_NAME.to_string(),
            arguments: json!({
                "connector": connector,
                "op": op,
                "args": args,
            })
            .to_string(),
        };
        let ctx = nemesis_agent::context::RequestContext::new(
            "flow-engine",
            "flow-engine",
            "flow-engine",
            "flow-engine",
        );
        let text = self.agent_loop.handle_tool_call(&call, &ctx).await;
        if crate::external_source::is_tool_error(&text) {
            return Err(text);
        }
        Ok(text)
    }

    // -- 状态机推进 ---------------------------------------------------------

    /// 推进一个模板下全部实例（跳过 parked/done；gate 未确认跳过；
    /// Pending→start、Running→poll）。
    async fn advance_template(&self, template: &str) -> Result<(), String> {
        for mut inst in self.flows.list_all(template) {
            if inst.parked.is_some() || inst.current_step >= inst.steps.len() {
                continue;
            }
            let cur = inst.steps[inst.current_step].clone();
            match cur.status {
                StepStatus::Ok | StepStatus::Failed | StepStatus::Skipped => continue,
                StepStatus::Pending if !cur.when.is_empty() => {
                    // 决策 B 条件步：先 token 内插（含 steps.<id>.<field> 投影）
                    // 再求值。true = gate/start 语义照旧；false = Skipped 同
                    // tick 推进；畸形 = 按失败走 on_fail（诚实不静默）。
                    let rendered = substitute_tokens(&cur.when, &inst, &json!({}));
                    match eval_when(&rendered) {
                        Ok(true) => {
                            if !cur.gate.is_empty() && !cur.gate_confirmed {
                                continue; // 条件成立但 manual gate 未放行，照旧等
                            }
                            self.start_step(&mut inst, &cur).await;
                        }
                        Ok(false) => skip_step(&self.flows, &mut inst, &rendered),
                        Err(e) => fail_step(&self.flows, &mut inst, &format!("条件式畸形: {e}")),
                    }
                }
                StepStatus::Pending if !cur.gate.is_empty() && !cur.gate_confirmed => continue,
                StepStatus::Pending => self.start_step(&mut inst, &cur).await,
                StepStatus::Running => self.poll_step(&mut inst, &cur).await,
            }
        }
        Ok(())
    }

    /// 发起当前步骤（Pending → Running，或同步完成/失败）。
    async fn start_step(&self, inst: &mut FlowInstance, cur: &StepRun) {
        inst.updated_at_ms = now_ms();
        enum Out {
            Done(Value),
            Start,
            Fail(String),
        }
        let outcome = match cur.kind.as_str() {
            KIND_LAND => {
                match start_board_land(&self.store, inst, cur.args_get("project").as_deref()) {
                    Ok(a) => Out::Done(a),
                    Err(e) => Out::Fail(e),
                }
            }
            KIND_FIXLOOP => {
                let dispatch = |issue: &nemesis_board::Issue| self.dispatch_issue(issue);
                match start_fixloop(&self.store, inst, &dispatch) {
                    Ok(_) => Out::Start,
                    Err(e) => Out::Fail(e),
                }
            }
            // 观察步/执行步：先转 Running，poll 期观察或执行。
            KIND_REGRESS | KIND_PUBLISH | KIND_REPORT => Out::Start,
            other => Out::Fail(format!(
                "step kind {other} 不可由推进器启动（fetch 由取数专管）"
            )),
        };
        match outcome {
            Out::Done(a) => complete_step(&self.flows, inst, &a),
            Out::Start => mark_running(&self.flows, inst),
            Out::Fail(e) => fail_step(&self.flows, inst, &e),
        }
    }

    /// 观察当前步骤（Running → 完成/失败/继续）。
    async fn poll_step(&self, inst: &mut FlowInstance, cur: &StepRun) {
        inst.updated_at_ms = now_ms();
        let result: Result<Value, String> = match cur.kind.as_str() {
            KIND_FIXLOOP => poll_outcome(poll_fixloop(&self.store, inst)),
            KIND_REGRESS => poll_outcome(regress_outcome(&self.store, inst)),
            KIND_PUBLISH => match self.publish_args(inst, cur) {
                Ok(args) => {
                    crate::publish::publish_task_delivery(
                        &self.db,
                        &self.store,
                        &self.agent_loop,
                        &self.workspace,
                        &args,
                    )
                    .await
                }
                Err(e) => Err(e),
            },
            KIND_REPORT => self.run_report(inst, cur).await,
            // 重启恢复路径：Running 的 land 重跑（origin 幂等）。
            KIND_LAND => start_board_land(&self.store, inst, cur.args_get("project").as_deref()),
            other => Err(format!("未知 step kind: {other}")),
        };
        match result {
            Ok(a) => complete_step(&self.flows, inst, &a),
            Err(e) => fail_step(&self.flows, inst, &e),
        }
    }

    /// publish 入参：最新派发的 task_id + 步骤声明的 connector/owner/repo/
    /// base（publish_task_delivery 直接入参形态）。
    fn publish_args(&self, inst: &FlowInstance, cur: &StepRun) -> Result<Value, String> {
        let issue = find_board_issue(&self.store, inst)?;
        let dispatches = self
            .store
            .list_dispatches(issue.id)
            .map_err(|e| format!("派发记录读取失败: {e}"))?;
        let task_id = dispatches
            .last()
            .map(|d| d.task_id.clone())
            .ok_or_else(|| format!("{} 无派发记录，无可发布交付", issue.number))?;
        let mut args = json!({ "task_id": task_id });
        for key in ["connector", "owner", "repo", "base"] {
            if let Some(v) = cur.args_get(key) {
                args[key] = json!(v);
            }
        }
        Ok(args)
    }

    /// report：dossier 导出 + 可选 connector 回帖（token 替换）。
    async fn run_report(&self, inst: &FlowInstance, cur: &StepRun) -> Result<Value, String> {
        let issue = find_board_issue(&self.store, inst)?;
        let usage_of = self.usage_closure();
        let out_root = self.workspace.join("logs").join("dossiers");
        let outcome = nemesis_board::dossier::export_issue_tree(
            &self.store,
            &issue.number,
            &self.workspace,
            &out_root,
            &usage_of,
        )?;
        let report_path = outcome.root.display().to_string();
        // 可选回帖：args.connector 声明才做；参数模板 token 替换。
        if let Some(connector) = cur.args_get("connector").filter(|s| !s.trim().is_empty()) {
            let op = cur
                .args_get("op")
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "issue_comment".to_string());
            let args_raw = cur
                .args_get("args_json")
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .unwrap_or_else(|| json!({}));
            // 在 Value 树上逐 String 替换（P4 e2e 实录第二坑：序列化串上
            // 替换会把 Windows 路径反斜杠变成非法 JSON 转义，落到
            // {"comment": path} 兜底丢 key/body）；替换结果即请求体。
            let args =
                substitute_tokens_value(&args_raw, inst, &json!({ "report_path": report_path }));
            if let Err(e) = self.invoke_connector(connector.trim(), &op, &args).await {
                return Err(format!("报告已导出但回帖失败: {e}"));
            }
        }
        Ok(json!({ "report_path": report_path, "issues": outcome.issue_numbers.len() }))
    }

    /// dossier usage 闭包（commands/dossier.rs usage_closer 同款：DataStore
    /// 打开一次，失败 = usage 缺行不阻塞）。
    fn usage_closure(&self) -> impl Fn(&str) -> Option<nemesis_board::dossier::DossierUsage> {
        let data_store = nemesis_data::DataStore::open(
            &nemesis_path::workspace_data_dir(&self.home).join("nemesisbot_data.db"),
        )
        .ok();
        move |task_id: &str| {
            data_store
                .as_ref()
                .and_then(|ds| ds.aggregate_session_usage_by_task(task_id).ok())
                .map(|agg| nemesis_board::dossier::DossierUsage {
                    input_tokens: agg.input_tokens,
                    output_tokens: agg.output_tokens,
                })
        }
    }

    /// 派发当前步骤对应的 board 单（cluster 编译形态走 web 同款双臂；
    /// 非 cluster 形态诚实不可派发）。
    #[cfg(feature = "cluster")]
    fn dispatch_issue(&self, issue: &nemesis_board::Issue) -> bool {
        let actor = nemesis_board::Actor::system("flow-engine");
        nemesis_web::handlers::board::dispatch_issue_for_external_source(
            &self.store,
            self.cluster.as_ref(),
            issue,
            None,
            &actor,
        )
    }

    #[cfg(not(feature = "cluster"))]
    fn dispatch_issue(&self, _issue: &nemesis_board::Issue) -> bool {
        false // 非 cluster 编译形态：诚实不可派发（web 侧同款纪律）
    }

    // -- 代答面（FlowBridge 委托） ------------------------------------------

    /// `flow://<template>/<view>` 代答单点。
    pub async fn answer(&self, template: &str, view: &str, args: &Value) -> Result<Value, String> {
        // 运行期总闸（2026-11-01 BUG 修复）：`ui.invoke` 的 flow:// 分支
        // 在 web 侧 require_installer 之前直入本桥——子系总闸/宿主壳开关
        // 翻关后必须在此拦截，否则面板页面在框架关闭后仍可代答。
        if !nemesis_config::plugin_wasm::runtime_enabled(&self.workspace) {
            return Err(
                "WASM 插件子系统已关闭（总闸或宿主壳开关关）——流程面板不可用，开关改回即时恢复"
                    .to_string(),
            );
        }
        // 单插件闸（2026-11-01 缺陷 B 根修）：current_templates 按开关过滤
        // （scan_templates 纪律同 ui_registry_impl）——模板解析不到 = 所属
        // 插件禁用或载荷缺席，全部视图统一诚实拒绝。此前只有总闸：单插件
        // disable 后 list/detail/timeline 照答数据、confirm/resume/fetch
        // 照执行动作（registry 页面已消失而数据/动作面仍活——正是漏修面）。
        let templates = self.current_templates().await;
        if !templates.iter().any(|d| d.template.id == template) {
            return Err(format!("流程模板不存在（或所属插件已禁用）: {template}"));
        }
        let instance = args
            .get("instance")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        match view {
            "list" => self.flows.answer_list(template),
            "detail" => self.flows.answer_detail(template, instance.as_deref()),
            "timeline" => self.flows.answer_timeline(template, instance.as_deref()),
            "confirm" => {
                let _guard = self.actions.lock().await;
                self.flows.answer_confirm(template, instance.as_deref())
            }
            "resume" => {
                let _guard = self.actions.lock().await;
                self.flows.answer_resume(template, instance.as_deref())
            }
            "fetch" => self.answer_fetch(template).await,
            other => Err(format!(
                "未知 flow view: {other}（词表 list/detail/timeline/confirm/resume/fetch）"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// 步骤记账（自由函数，单测钉死）
// ---------------------------------------------------------------------------

/// 转运行态：status=Running + started_at 记账落盘。
fn mark_running(flows: &FlowStore, inst: &mut FlowInstance) {
    if let Some(step) = inst.steps.get_mut(inst.current_step) {
        step.status = StepStatus::Running;
        step.started_at_ms = Some(now_ms());
    }
    inst.updated_at_ms = now_ms();
    let _ = flows.save(inst);
}

/// 完成：记账产物 + 游标推进。
fn complete_step(flows: &FlowStore, inst: &mut FlowInstance, artifacts: &Value) {
    if let Some(step) = inst.steps.get_mut(inst.current_step) {
        step.status = StepStatus::Ok;
        step.artifacts = artifacts.clone();
        step.error = None;
        step.finished_at_ms = Some(now_ms());
        if step.started_at_ms.is_none() {
            step.started_at_ms = step.finished_at_ms;
        }
    }
    inst.current_step += 1;
    inst.updated_at_ms = now_ms();
    let _ = flows.save(inst);
}

/// 失败：按 on_fail 分支（park_human / retry:N / continue）。
fn fail_step(flows: &FlowStore, inst: &mut FlowInstance, error: &str) {
    if error == FLOW_STILL_RUNNING {
        return; // 观察态：不记账不推进
    }
    let snippet: String = error.chars().take(ERR_SNIPPET).collect();
    let Some(step) = inst.steps.get_mut(inst.current_step) else {
        return;
    };
    step.status = StepStatus::Failed;
    step.error = Some(snippet.clone());
    step.finished_at_ms = Some(now_ms());
    if step.started_at_ms.is_none() {
        step.started_at_ms = step.finished_at_ms;
    }
    inst.updated_at_ms = now_ms();
    match parse_on_fail(&step.on_fail) {
        OnFail::ParkHuman => {
            inst.parked = Some(format!("步骤 {} 失败: {snippet}", step.id));
        }
        OnFail::Retry(max) => {
            step.attempts += 1;
            if step.attempts >= max {
                inst.parked = Some(format!("步骤 {} 重试 {max} 次仍失败: {snippet}", step.id));
            } else {
                step.status = StepStatus::Pending; // 下轮重试（保留 attempts）
            }
        }
        OnFail::Continue => {
            inst.gaps.push(json!({
                "step": step.id,
                "error": snippet,
                "ts_ms": inst.updated_at_ms,
            }));
            inst.current_step += 1; // 带隙续跑（步骤保持 Failed 态留痕）
        }
    }
    let _ = flows.save(inst);
}

/// 条件步不成立（决策 B）：Skipped 留痕（条件式进 artifacts，面板可见）
/// + 游标同 tick 推进——不执行步骤本体，不走 on_fail（跳过不是失败）。
fn skip_step(flows: &FlowStore, inst: &mut FlowInstance, rendered_when: &str) {
    if let Some(step) = inst.steps.get_mut(inst.current_step) {
        step.status = StepStatus::Skipped;
        step.artifacts = json!({ "skipped_when": rendered_when });
        step.finished_at_ms = Some(now_ms());
    }
    inst.current_step += 1;
    inst.updated_at_ms = now_ms();
    let _ = flows.save(inst);
}

/// 条件求值（决策 B）：单比较 `左 <op> 右`。op 词表 `==`/`!=`/`>=`/`<=`/
/// `>`/`<`（两字符 op 先于单字符命中——按首次出现位置取最早，同位取长）。
/// 两侧 trim + 成对引号剥离；两侧均可解析为数字按数值比，否则按字符串比
/// （`==`/`!=` 全等，`>`/`<` 字典序）。畸形（无 op / 空侧）→ Err，调用方
/// 按失败走 on_fail。
fn eval_when(rendered: &str) -> Result<bool, String> {
    // 闭包的显式 `-> &str` 返回注解会引入独立生命周期（不吃实参省略规则），
    // 嵌套 fn 走函数省略：唯一入参生命周期约束全部输出。
    fn strip_quotes(s: &str) -> &str {
        let t = s.trim();
        if t.len() >= 2
            && ((t.starts_with('"') && t.ends_with('"'))
                || (t.starts_with('\'') && t.ends_with('\'')))
        {
            &t[1..t.len() - 1]
        } else {
            t
        }
    }
    const OPS: [&str; 6] = ["==", "!=", ">=", "<=", ">", "<"];
    let hit = OPS
        .iter()
        .filter_map(|op| rendered.find(op).map(|pos| (pos, *op)))
        .min_by_key(|(pos, op)| (*pos, std::cmp::Reverse(op.len()))); // 同位时长 op（>=/>）先中
    let (pos, op) = hit.ok_or_else(|| "无非比较符（== != >= <= > <）".to_string())?;
    let op_len = op.len();
    let left = strip_quotes(&rendered[..pos]);
    let right = strip_quotes(&rendered[pos + op_len..]);
    if left.is_empty() {
        return Err("比较符左侧为空".to_string());
    }
    if right.is_empty() {
        return Err("比较符右侧为空".to_string());
    }
    let (ln, rn) = (left.parse::<f64>(), right.parse::<f64>());
    let outcome = match (ln, rn) {
        (Ok(a), Ok(b)) => match op {
            "==" => a == b,
            "!=" => a != b,
            ">=" => a >= b,
            "<=" => a <= b,
            ">" => a > b,
            _ => a < b,
        },
        _ => match op {
            "==" => left == right,
            "!=" => left != right,
            ">=" => left >= right,
            "<=" => left <= right,
            ">" => left > right,
            _ => left < right,
        },
    };
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// 模板扫描 + fetch 解析（自由函数，单测钉死）
// ---------------------------------------------------------------------------

/// 目录戳：条目数 + 最大 mtime 秒。
fn dir_stamp(dir: &Path) -> (u64, usize) {
    let mut max_mtime: u64 = 0;
    let mut count: usize = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(|e| e.ok()) {
            count += 1;
            if let Ok(meta) = e.metadata()
                && let Ok(mtime) = meta.modified()
                && let Ok(secs) = mtime.duration_since(std::time::UNIX_EPOCH)
            {
                max_mtime = max_mtime.max(secs.as_secs());
            }
        }
    }
    (max_mtime, count)
}

/// 模板缓存戳：载荷目录戳 ∪ 开关文件 mtime。开关文件用毫秒精度并入——
/// enable/disable 是毫秒级连续操作，秒精度下同秒两次翻转戳不变（漏检）。
/// 毫秒值与 dir_stamp 的秒值同向可比（epoch 毫秒恒大于 epoch 秒），max
/// 语义不受混合精度影响。开关文件缺席 = 0，删除也触发重扫。
fn template_stamp(plugins_dir: &Path, config_dir: &Path) -> (u64, usize) {
    let (mut mtime, count) = dir_stamp(plugins_dir);
    if let Ok(meta) = std::fs::metadata(nemesis_config::plugin_wasm::plugin_wasm_config_path(
        config_dir,
    )) && let Ok(modified) = meta.modified()
        && let Ok(ms) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        mtime = mtime.max(ms.as_millis() as u64);
    }
    (mtime, count)
}

/// 扫描全部 enabled panel 插件的 flow 模板（ui_registry_impl 同款纪律：
/// 开关过滤 + kind 过滤 + 排序 + 声明面扫描；坏 manifest WARN 跳过）。
fn scan_templates(plugins_dir: &Path, config_dir: &Path) -> Vec<FlowTemplateDef> {
    let switches = nemesis_config::plugin_wasm::load_switches(config_dir);
    let mut slugs: Vec<String> = switches
        .iter()
        .filter(|(_, sw)| sw.enabled)
        .map(|(slug, _)| slug.clone())
        .collect();
    slugs.sort();
    let mut out = Vec::new();
    for slug in slugs {
        let dir = plugins_dir.join(&slug);
        // load_from_dir 返回 (manifest, 原始文档)；此处只用声明面。
        let (manifest, _raw) = match PluginManifest::load_from_dir(&dir) {
            Ok(m) => m,
            Err(_) => continue, // 载荷缺席/坏 manifest：诚实跳过（switch 残留）
        };
        if *manifest.kind != PluginKind::Panel {
            continue;
        }
        for template in &manifest.flow.templates {
            if let Err(reason) = validate_template(template) {
                tracing::warn!(
                    "[FlowEngine] 模板 {}/{} 声明非法跳过: {reason}",
                    slug,
                    template.id
                );
                continue;
            }
            out.push(FlowTemplateDef {
                slug: slug.clone(),
                template: template.clone(),
            });
        }
    }
    out
}

/// 模板结构校验：恰好一个 connector_fetch 且必须是首步；step kind 值域。
fn validate_template(t: &FlowTemplate) -> Result<(), String> {
    if t.id.trim().is_empty() {
        return Err("模板 id 为空".to_string());
    }
    if t.steps.is_empty() {
        return Err("模板无步骤".to_string());
    }
    let fetch_count = t.steps.iter().filter(|s| s.kind == KIND_FETCH).count();
    if fetch_count != 1 {
        return Err(format!(
            "模板必须恰好一个 {KIND_FETCH} 步（当前 {fetch_count}）"
        ));
    }
    if t.steps[0].kind != KIND_FETCH {
        return Err(format!("模板首步必须是 {KIND_FETCH}"));
    }
    for s in &t.steps {
        if !matches!(
            s.kind.as_str(),
            KIND_FETCH | KIND_LAND | KIND_FIXLOOP | KIND_REGRESS | KIND_PUBLISH | KIND_REPORT
        ) {
            return Err(format!("未知 step kind: {}", s.kind));
        }
    }
    Ok(())
}

/// 工具面文本 → 条目数组：数组直收；`{"issues"|"items"|"entries"|"rows": [...]}`
/// 包裹形态解包；其余诚实报错。
fn parse_entries(text: &str) -> Result<Vec<Value>, String> {
    let v: Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("connector 返回非 JSON: {e}"))?;
    match v {
        Value::Array(items) => Ok(items),
        Value::Object(map) => {
            for key in ["issues", "items", "entries", "rows"] {
                if let Some(Value::Array(arr)) = map.get(key) {
                    return Ok(arr.clone());
                }
            }
            Err("connector 返回对象无 issues/items/entries/rows 数组字段，无法取数".to_string())
        }
        other => Err(format!("connector 返回形态不支持取数: {other}")),
    }
}

/// 条目 → 实例物化（幂等：已存在跳过）。fetch 步记账 Ok、游标落 1。
fn materialize_instances(
    flows: &FlowStore,
    def: &FlowTemplateDef,
    entries: &[Value],
) -> Result<usize, String> {
    let fetch_step = def
        .template
        .steps
        .first()
        .ok_or_else(|| "模板无 fetch 步".to_string())?;
    let key_field = fetch_step
        .args
        .get("key_field")
        .cloned()
        .unwrap_or_else(|| "key".to_string());
    let title_field = fetch_step
        .args
        .get("title_field")
        .cloned()
        .unwrap_or_else(|| "title".to_string());
    let mut created = 0usize;
    for entry in entries {
        let Some(key) = entry_string(entry, &key_field) else {
            continue; // 缺键条目：诚实跳过（不炸整批）
        };
        if key.trim().is_empty() {
            continue;
        }
        let instance = nemesis_utils::sanitize::sanitize_path_segment(key.trim());
        if flows.exists(&def.template.id, &instance) {
            continue;
        }
        let title = entry_string(entry, &title_field).unwrap_or_else(|| key.clone());
        let now = now_ms();
        let steps: Vec<StepRun> = def.template.steps.iter().map(StepRun::from_decl).collect();
        let mut inst = FlowInstance {
            template: def.template.id.clone(),
            instance,
            ext_key: key.clone(),
            title,
            current_step: 1, // fetch 已完成
            steps,
            entry: entry.clone(),
            gaps: Vec::new(),
            parked: None,
            created_at_ms: now,
            updated_at_ms: now,
        };
        if let Some(fetch) = inst.steps.first_mut() {
            fetch.status = StepStatus::Ok;
            fetch.started_at_ms = Some(now);
            fetch.finished_at_ms = Some(now);
            fetch.artifacts = json!({ "count": entries.len() });
        }
        flows.save(&inst)?;
        created += 1;
    }
    Ok(created)
}

// ---------------------------------------------------------------------------
// WS 桥（nemesis-web FlowEngineBridge 实现）
// ---------------------------------------------------------------------------

/// `flow://` 代答桥：thin 委托 engine.answer。
pub struct FlowBridge {
    engine: Arc<FlowEngine>,
}

impl FlowBridge {
    pub fn new(engine: Arc<FlowEngine>) -> Self {
        FlowBridge { engine }
    }
}

#[async_trait::async_trait]
impl nemesis_web::handlers::plugins_wasm::FlowEngineBridge for FlowBridge {
    async fn invoke(
        &self,
        template: &str,
        view: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.engine.answer(template, view, args).await
    }
}

#[cfg(test)]
mod tests;
