//! 声明式 flow 引擎（域剥离批 C）——宿主侧通用剧本执行器。
//!
//! `[[flow.template]]` 段在这里变成可执行流水线：gateway 装配期 spawn 周期
//! tick，按 `interval_secs` 触发 `connector_fetch` 取数建实例，并推进所有
//! active 实例的状态机；面板 `flow://` 绑定经 [`FlowBridge`] 代答查询/确认/
//! 手动触发。框架零领域硬编码——**领域知识只在模板声明面**：外部身份映射
//! （origin_type/origin_id/origin_meta）、条目筛选（key/title/body/filter/
//! labels）、分页（page_key/limit_key/page_size/max_pages）、发布与对账
//! 协议（create_op/create_args/result_ref/reconcile_op/reconcile_args/
//! reconcile_match）、回写协议（on_transition op+args+parse-origin-id）全部
//! 由 toml 声明，宿主只提供通用原语与 token 渲染/求值机制。
//!
//! 步骤生命周期：同步步（board_land）发起即完成；观察步（agent_fixloop /
//! check_regress）发起转 Running，逐 tick 轮询 board 状态裁决；执行步
//! （publish / report）发起转 Running，poll 期执行（幂等面：publish 按
//! task_id 幂等含 unknown 对账 / report 重导出覆盖）。board 单是执行真相
//! 源，flow 状态只是编排投影。
//!
//! 关键不变量：
//! - **磁盘 = 真相**：实例状态落 `<workspace>/logs/flows/<template>/
//!   <instance>.json`，回写游标落 `<flows-root>/_transitions/<template>/
//!   <origin_id>.json`，无内存注册表（重启即恢复，进程内零双份）；
//! - **gate = manual**：实例停 `awaiting_gate`，面板确认（confirm）后续跑；
//! - estop 冻结 tick；写操作由 `actions` 互斥锁串行（fetch/advance/confirm/
//!   resume/writeback-poll），读代答无锁（盘读=一致快照）。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use nemesis_agent::estop::EstopState;
use nemesis_agent::r#loop::AgentLoop;
use nemesis_board::store::BoardStore;
use nemesis_plugins_wasm::manifest::{
    FlowStep, FlowTemplate, FlowTransition, PluginKind, PluginManifest,
};

// 派发面私有别名：cluster 编译形态挂 Cluster 引用做派发，非 cluster 形态
// 诚实不可派发（web 侧 dispatch_issue_for_flow 的非 cluster 臂恒 false，
// 同款纪律）。
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
/// `repo_sync`（jira-gitlab-flow 批）：确定性 clone-or-pull——
/// 项目目录（注册表 `directory`）与远端对齐，fixloop 之前执行。
const KIND_REPO_SYNC: &str = "repo_sync";

/// 建单 origin_type 兜底（模板未声明 origin 时的旧形态回落；
/// 外部身份真实值由模板声明，见 [`resolve_instance_origin`]）。
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
    /// 外部身份映射（C-1 域剥离）：模板 origin_type/origin_id 声明渲染，
    /// origin_meta 透传外部条目元数据（回写 compose 首选 token 表）。
    /// 空串 = 旧实例（回落 {flow, {template}/{ext_key}} 旧键）。
    pub origin_type: String,
    pub origin_id: String,
    pub origin_meta: BTreeMap<String, String>,
    /// 外部条目正文（body_field 提取；board_land 时 `[CHECK]` 行提取为 AC）。
    pub body: String,
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
            origin_type: String::new(),
            origin_id: String::new(),
            origin_meta: BTreeMap::new(),
            body: String::new(),
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

    // -- 回写状态（C-2 轮询游标，磁盘=真相） --------------------------------

    /// 回写游标路径：`<flows-root>/_transitions/<模板>/<origin_id 段安全化>.json`。
    /// `_transitions` 前缀下划线（模板 id 词表 `[a-z0-9-]` 恒不撞）；origin_id
    /// 过 sanitize_path_segment（含 `/`/`#` 的外部键文件名安全化）。
    fn transition_state_path(&self, template: &str, origin_id: &str) -> PathBuf {
        self.root.join("_transitions").join(template).join(format!(
            "{}.json",
            nemesis_utils::sanitize::sanitize_path_segment(origin_id)
        ))
    }

    /// 读回写游标（缺席 = None；损坏 = None 诚实重踩 adopt）。
    fn load_transition_state(&self, template: &str, origin_id: &str) -> Option<String> {
        let raw = std::fs::read_to_string(self.transition_state_path(template, origin_id)).ok()?;
        let v: Value = serde_json::from_str(&raw).ok()?;
        v.get("status").and_then(|s| s.as_str()).map(str::to_string)
    }

    /// 原子落回写游标（tmp + 换名，实例 save 同款 Windows 语义）。
    fn save_transition_state(
        &self,
        template: &str,
        origin_id: &str,
        status: &str,
    ) -> Result<(), String> {
        let path = self.transition_state_path(template, origin_id);
        std::fs::create_dir_all(path.parent().ok_or("回写状态路径无父目录")?)
            .map_err(|e| format!("回写状态目录: {e}"))?;
        let tmp = path.with_extension("json.tmp");
        let body = json!({ "status": status, "ts_ms": now_ms() }).to_string();
        std::fs::write(&tmp, body).map_err(|e| format!("write 回写状态 tmp: {e}"))?;
        let _ = std::fs::remove_file(&path);
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename 回写状态: {e}"))
    }
}

// ---------------------------------------------------------------------------
// board 绑定（自由函数面：吃 &BoardStore，脱离引擎可单测）
// ---------------------------------------------------------------------------

/// 旧实例回落键（origin 字段为空的 v0 实例升级路径）。
fn flow_origin_id(template: &str, ext_key: &str) -> String {
    format!("{template}/{ext_key}")
}

/// 实例的取数身份（origin_type, origin_id）：模板渲染值优先，空 = 旧实例
/// 回落 {flow, {template}/{ext_key}} 旧键（升级路径不炸在途流程）。
fn inst_origin(inst: &FlowInstance) -> (String, String) {
    if inst.origin_id.is_empty() {
        (
            ORIGIN_TYPE.to_string(),
            flow_origin_id(&inst.template, &inst.ext_key),
        )
    } else {
        (inst.origin_type.clone(), inst.origin_id.clone())
    }
}

/// 按取数 origin 找回 board 单（幂等锚：land/fixloop/regress/publish/
/// report 五步共用）。
fn find_board_issue(
    store: &BoardStore,
    inst: &FlowInstance,
) -> Result<nemesis_board::Issue, String> {
    let (origin_type, origin_id) = inst_origin(inst);
    let origin_hit = store
        .list_issues_by_origin(&origin_type, &origin_id, 1)
        .map_err(|e| format!("board origin 查询失败: {e}"))?
        .into_iter()
        .next();
    origin_hit.ok_or_else(|| format!("流程 {}/{} 未落 board 单", inst.template, inst.instance))
}

/// `board_land`：按 origin 幂等建 board 单（origin 整串精确去重 +
/// NewIssue 字面量形态；title 不带 origin 前缀——origin 已是独立数据槽）。
/// origin 来自模板声明渲染（C-1）；外部正文 `[CHECK]` 行提取为 AC（
/// 缺陷根修同源）；args.project 可选：按项目名归属项目（缺席 = 不归属）；
/// args.body_preamble 可选：流程契约前导（域模板声明，确定性注入 issue
/// description 头部——worker 读单即读纪律，不靠提示词软约束）。
fn start_board_land(
    store: &Arc<BoardStore>,
    inst: &mut FlowInstance,
    project: Option<&str>,
    preamble: Option<&str>,
) -> Result<Value, String> {
    let (origin_type, origin_id) = inst_origin(inst);
    let existing = store
        .list_issues_by_origin(&origin_type, &origin_id, 1)
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
    let mut description = String::new();
    if let Some(pre) = preamble.map(str::trim).filter(|s| !s.is_empty()) {
        description.push_str(pre);
        description.push_str("\n\n");
    }
    description.push_str(&format!(
        "来源: {origin_type} {origin_id}\n\n外部条目: {}",
        inst.ext_key
    ));
    if !inst.body.trim().is_empty() {
        description.push_str("\n\n");
        description.push_str(inst.body.trim());
    }
    let issue = store.create_issue(nemesis_board::models::NewIssue {
        title: inst.title.clone(),
        description,
        priority: nemesis_board::models::priority::MEDIUM,
        assignee: None,
        assignee_id: None,
        creator: nemesis_board::Actor::system("flow-engine"),
        parent_issue_id: None,
        project_id,
        due_date: None,
        acceptance_criteria: extract_check_criteria(&inst.body), // 外部 [CHECK] 锚点透传；缺席走 ac_gen
        origin: Some(nemesis_board::models::TaskOrigin {
            origin_type,
            origin_id: origin_id.clone(),
            origin_meta: inst.origin_meta.clone(),
        }),
        required_role: None,
        required_tags: vec!["外部".to_string()],
        required_profession: None,
        required_capabilities: Vec::new(),
        required_backend: None,
        quality: String::new(),
        unattended: None,
        milestone: false,
    })?;
    Ok(json!({ "board_issue": issue.number, "board_issue_id": issue.id, "landed": "created" }))
}

/// 执行面声明物化（词法层纯函数）：trim 后原样透传。`self`（大小写不
/// 敏感）= **单机本地执行面字面量**，消费端 dispatch_issue_for_flow 识别
/// 后走 dispatch_issue_local（一等公民，与集群是否装配无关）；其余值 =
/// 显式节点名/id 原样透传（不在注册表 = 消费端诚实停车）。trim 与消费端
/// dispatch_issue_for_flow 同口径。声明是一等选择：钉住即走，不在线诚实
/// 停车——无匹配器无兜底（无回退语义）。
#[cfg(feature = "cluster")]
fn materialize_exec_target(raw: &str) -> String {
    raw.trim().to_string()
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
// 域剥离批 C 原语：外部身份映射（C-1）/ 回写轮询（C-2）/ 取数韧性
// （salvage/[CHECK]/labels 过滤等通用原语唯一依据在此）
// ---------------------------------------------------------------------------

/// 回写轮询每模板单轮限额（origin_type 订阅面取数上限）。
const WRITEBACK_POLL_LIMIT: usize = 100;

/// 写回熔断阈值：同一回写目标连续失败达到该次数 = 停投。
/// 请求可能已到达对端而响应丢失（网络抖动/对端崩溃重启）——无限重投即
/// 重复副作用（demo 事故 156 条重复评论实证）；对齐 board 派发
/// `max_send_retries`（缺省 5）熔断先例。计数为引擎内存态（重启 = 预算
/// 刷新），游标仍在盘上不受影响；running（guest 异步处理中）不计失败。
const WRITEBACK_MAX_RETRIES: u32 = 5;

/// 写回熔断判定（纯函数）：连续失败计数是否已到停投线。
fn writeback_circuit_open(consecutive_failures: u32) -> bool {
    consecutive_failures >= WRITEBACK_MAX_RETRIES
}

/// 实例外部身份解析（C-1）：模板 `origin_type`/`origin_id` 声明经 token 表
/// 渲染。token 表 = args-only：`{{connector}}`（fetch 步 connector）+
/// `{{arg.<name>}}`（fetch 步 args_json 内字段）+ `{{key}}`（条目键）。
/// 声明内残留 `{{`（未知 token / 未闭合）= Err（上游 WARN + 跳过该条目，
/// 诚实不静默）。origin_type 缺省回落取数 connector 名。
fn resolve_instance_origin(
    template: &FlowTemplate,
    fetch_step: &FlowStep,
    ext_key: &str,
) -> Result<(String, String, BTreeMap<String, String>), String> {
    let connector = fetch_step
        .args
        .get("connector")
        .cloned()
        .unwrap_or_default();
    let args_json: Value = fetch_step
        .args
        .get("args_json")
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_else(|| json!({}));
    let mut table = BTreeMap::new();
    table.insert("connector".to_string(), connector.clone());
    table.insert("key".to_string(), ext_key.to_string());
    if let Some(obj) = args_json.as_object() {
        for (k, v) in obj {
            table.insert(
                format!("arg.{k}"),
                match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                },
            );
        }
    }
    let origin_type = if template.origin_type.trim().is_empty() {
        connector.clone()
    } else {
        render_writeback_tokens(&template.origin_type, &table)?
    };
    if template.origin_id.trim().is_empty() {
        return Err("模板缺 origin_id 声明（外部身份映射必声明——宿主不猜）".to_string());
    }
    let origin_id = render_writeback_tokens(&template.origin_id, &table)?;
    // origin_meta 值同 token 表渲染（透传外部元数据面）。
    let mut origin_meta = BTreeMap::new();
    for (k, v) in &template.origin_meta {
        origin_meta.insert(k.clone(), render_writeback_tokens(v, &table)?);
    }
    Ok((origin_type, origin_id, origin_meta))
}

/// 严格 token 渲染（回写/身份面专用）：每个 `{{key}}` 必须命中表，未知键 =
/// Err；未闭合 `{{` = Err。不复用 [`render_tokens`]（未知保底语义会静默
/// 留残——身份/回写串错了必须响，不能带着错的 origin_id 建单/回帖）。
/// 值内容不回扫——值里合法含 `{{` 不会误伤。
fn render_writeback_tokens(tpl: &str, table: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(tpl.len());
    let bytes = tpl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            match tpl[i + 2..].find("}}") {
                Some(end) => {
                    let key = tpl[i + 2..i + 2 + end].trim();
                    let v = table
                        .get(key)
                        .ok_or_else(|| format!("token 声明未知键: {key}"))?;
                    out.push_str(v);
                    i += 2 + end + 2;
                    continue;
                }
                None => return Err("模板含未闭合 {{（畸形声明）".to_string()),
            }
        }
        let ch_len = tpl[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&tpl[i..i + ch_len]);
        i += ch_len;
    }
    Ok(out)
}

/// 引号实参解析（parse-origin-id 语言）：`'X'` / `"X"`，返回（内容，尾部）。
fn parse_quoted_arg(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    let quote = s.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let rest = &s[1..];
    let end = rest.find(quote)?;
    Some((&rest[..end], &rest[end + 1..]))
}

/// parse-origin-id 表达式求值（C-2 回写 compose 的 origin_id 分解语言）：
/// 步骤按 `|` 串联，左到右作用：
/// - `before 'X'`：首个 X 前缀段（找不到 = Err）；
/// - `after 'X'`：首个 X 后缀段（找不到 = Err）；
/// - `segment 'S' N`：按 S 切分的第 N 段（0 起；越界 = Err）。
///
/// 结果为空 = Err。示例：`origin/acme/webapp#12` → owner:
/// `segment '/' 1`；repo: `before '#' | segment '/' 2`；number:
/// `after '#'`。
fn eval_split_expr(origin_id: &str, expr: &str) -> Result<String, String> {
    let mut cur = origin_id.to_string();
    for raw_step in expr.split('|') {
        let step = raw_step.trim();
        if step.is_empty() {
            return Err("parse-origin-id 空步骤".to_string());
        }
        let (name, rest) = step
            .split_once(char::is_whitespace)
            .ok_or_else(|| format!("parse-origin-id 步骤缺参数: {step}"))?;
        match name {
            "before" | "after" => {
                let (sep, tail) = parse_quoted_arg(rest)
                    .ok_or_else(|| format!("parse-origin-id {name} 缺引号分隔子串: {rest}"))?;
                let head = tail.trim();
                if !head.is_empty() {
                    return Err(format!(
                        "parse-origin-id {name} 步骤参数后有多余内容: {head}"
                    ));
                }
                let hit = cur
                    .find(sep)
                    .ok_or_else(|| format!("parse-origin-id 分隔子串未命中: {sep}"))?;
                cur = if name == "before" {
                    cur[..hit].to_string()
                } else {
                    cur[hit + sep.len()..].to_string()
                };
            }
            "segment" => {
                let (sep, tail) = parse_quoted_arg(rest)
                    .ok_or_else(|| format!("parse-origin-id segment 缺引号分隔子串: {rest}"))?;
                let idx_raw = tail.trim();
                let idx: usize = idx_raw
                    .parse()
                    .map_err(|_| format!("parse-origin-id segment 序号非法: {idx_raw}"))?;
                cur = cur
                    .split(sep)
                    .nth(idx)
                    .ok_or_else(|| {
                        format!("parse-origin-id segment 越界: {idx}（分隔子串 {sep}）")
                    })?
                    .to_string();
            }
            other => return Err(format!("parse-origin-id 未知步骤: {other}")),
        }
    }
    if cur.is_empty() {
        return Err("parse-origin-id 结果为空".to_string());
    }
    Ok(cur)
}

/// 回写表组装结果（C-2 compose 的可测缝隙）。
enum WritebackTableOutcome {
    /// 表就绪（transition.* 恒在 + 坐标键已挂），可进入 args 渲染。
    Ready(BTreeMap<String, String>),
    /// origin_meta 缺席且模板无 parse-origin-id——无坐标可回帖，永久跳过
    /// （调用方游标落盘）。
    PermanentSkip,
    /// origin_id 分解失败——游标保留，下 tick 重试。
    Retry(String),
}

/// 回写 token 表组装：`transition.*`（status/from/number/text）四键恒在；
/// 坐标键——`origin_meta.<key>`（坐标落库载体，优先）。v1 旧单 origin_meta
/// 缺席时 parse-origin-id 现场分解，结果双前缀镜像落表（`origin_id.*` 与
/// `origin_meta.*` 各一份）：模板 args 静态声明一套前缀必须两态通用，只落
/// `origin_id.*` 会让引用 `origin_meta.*` 的模板对 v1 旧单永久渲染失败
/// （cancel 回帖 Retry 循环实证）。
fn build_writeback_table(
    origin: &nemesis_board::TaskOrigin,
    hook: &FlowTransition,
    number: &str,
    status: &str,
    from: &str,
) -> WritebackTableOutcome {
    let mut table = BTreeMap::new();
    table.insert("transition.status".to_string(), status.to_string());
    table.insert("transition.from".to_string(), from.to_string());
    table.insert("transition.number".to_string(), number.to_string());
    table.insert(
        "transition.text".to_string(),
        render_writeback_text(number, from, status),
    );
    let have_meta = !origin.origin_meta.is_empty();
    for (k, v) in &origin.origin_meta {
        table.insert(format!("origin_meta.{k}"), v.clone());
    }
    if have_meta {
        return WritebackTableOutcome::Ready(table);
    }
    if hook.parse_origin_id.is_empty() {
        return WritebackTableOutcome::PermanentSkip;
    }
    for (key, expr) in &hook.parse_origin_id {
        match eval_split_expr(&origin.origin_id, expr) {
            Ok(v) => {
                table.insert(format!("origin_id.{key}"), v.clone());
                table.insert(format!("origin_meta.{key}"), v);
            }
            Err(e) => return WritebackTableOutcome::Retry(e),
        }
    }
    WritebackTableOutcome::Ready(table)
}

/// publish 步声明解析（C-3 的可测缝隙）：协议字段全在模板步 args——必填
/// 缺失 / create_args|reconcile_args 非法 JSON / 值非字符串 = 诚实报错
/// （域知识只在模板，宿主无内置发布协议）。`get_arg` 取步 args 字段；
/// `render` 做 parse-time token 渲染（create_args/reconcile_args 逐值）。
/// 可选字段缺省：result_url=None、reconcile_limit_key="limit"、
/// reconcile_page_key="page"、reconcile_limit=20、reconcile_max_pages=5
/// （声明 ≤0 视作缺省）。
fn parse_publish_step_protocol(
    get_arg: &dyn Fn(&str) -> Option<String>,
    render: &dyn Fn(&str) -> String,
) -> Result<crate::publish::PublishProtocol, String> {
    // create_args / reconcile_args：JSON 对象形态（键 → 字符串值），逐值
    // token 渲染；非对象/非字符串值 = 声明错误诚实报错。
    let parse_obj_args = |key: &str| -> Result<BTreeMap<String, String>, String> {
        let raw = get_arg(key).filter(|s| !s.trim().is_empty());
        let Some(raw) = raw else {
            return Err(format!("publish 步缺 {key} 声明（域知识只在模板）"));
        };
        let v: Value =
            serde_json::from_str(&raw).map_err(|e| format!("publish 步 {key} 非法 JSON: {e}"))?;
        let obj = v
            .as_object()
            .ok_or_else(|| format!("publish 步 {key} 必须是 JSON 对象"))?;
        let mut out = BTreeMap::new();
        for (k, val) in obj {
            let s = match val {
                Value::String(s) => s.clone(),
                other => {
                    return Err(format!(
                        "publish 步 {key}.{k} 值必须是字符串（token 模板形态），当前 {other}"
                    ));
                }
            };
            out.insert(k.clone(), render(&s));
        }
        Ok(out)
    };
    let req = |key: &str| -> Result<String, String> {
        let v = get_arg(key)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("publish 步缺 {key} 声明（域知识只在模板）"))?;
        // op 形态校验（a）：`plugin:` 引用形态必须过 parse_plugin_step_ref
        // （与 on-transition/report 同纪律——装载期红，不留运行期黑洞）；
        // 其余自由文本（connector 词表裁决在 connector 侧）。
        if v.starts_with("plugin:")
            && nemesis_plugins_wasm::manifest::parse_plugin_step_ref(&v).is_none()
        {
            return Err(format!(
                "publish 步 {key} '{v}' 是 plugin 引用形态但解析失败（需 plugin:<slug>:<op>，段 [a-z0-9_-]+）"
            ));
        }
        Ok(v)
    };
    Ok(crate::publish::PublishProtocol {
        create_op: req("create_op")?,
        create_args: parse_obj_args("create_args")?,
        result_ref: req("result_ref")?,
        result_url: get_arg("result_url")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        reconcile_op: req("reconcile_op")?,
        reconcile_args: parse_obj_args("reconcile_args")?,
        reconcile_match: req("reconcile_match")?,
        reconcile_limit_key: get_arg("reconcile_limit_key")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "limit".to_string()),
        reconcile_page_key: get_arg("reconcile_page_key")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "page".to_string()),
        reconcile_limit: get_arg("reconcile_limit")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(20),
        reconcile_max_pages: get_arg("reconcile_max_pages")
            .and_then(|s| s.trim().parse().ok())
            .filter(|v| *v > 0)
            .unwrap_or(5),
        // Gerrit 臂提交信息模板（可选；push_ref 以 refs/for/ 开头时消费）。
        // parse 期渲染条目 token（ext_key/title/board_issue/report_path），
        // 发布侧 rt 期再渲染 change_id/branch/task_id（两段渲染走 protocol
        // 字段而非 args 原样拷贝——args 拷贝只有 rt 单段）。
        commit_message: get_arg("commit_message")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| render(&s)),
    })
}

/// 回写三态文案（`transition.text` token 缺省值——模板 on_transition args
/// 未声明 text 时模板侧可省，通用兜底；具体回帖措辞由模板声明覆盖）。
fn render_writeback_text(issue_number: &str, from: &str, to: &str) -> String {
    match to {
        "in_review" => format!(
            "🔄 **已提交评审**：该 issue 已导入看板（{number}）并完成执行，进入验收环节（{from} → in_review）。验收结论将回帖本 issue。",
            number = issue_number,
            from = from
        ),
        "done" => format!(
            "✅ **已完成**：该 issue 已通过验收收口（board {number}，{from} → done）。交付详情见看板任务档案。",
            number = issue_number,
            from = from
        ),
        "cancelled" => format!(
            "⛔ **已取消**：该任务在看板侧被取消（board {number}，{from} → cancelled）。",
            number = issue_number,
            from = from
        ),
        _ => String::new(),
    }
}

/// 回写轮询三态判定（磁盘游标 diff）：游标缺席 = Adopt（静默收编——首轮
/// 不为存量单发历史通知）；游标 = 当前 = Noop；游标 ≠ 当前 = Compose。
#[derive(Debug, PartialEq, Eq)]
enum WritebackDecision {
    Adopt,
    Noop,
    Compose { from: String },
}

fn writeback_decision(stored: Option<&str>, current: &str) -> WritebackDecision {
    match stored {
        None => WritebackDecision::Adopt,
        Some(prev) if prev == current => WritebackDecision::Noop,
        Some(prev) => WritebackDecision::Compose {
            from: prev.to_string(),
        },
    }
}

/// 写回投递结果（调用方熔断簿记驱动）。
enum WritebackResult {
    /// 游标已推进（done/skipped/connector ok/永久跳过）——清除失败计数。
    Advanced,
    /// 游标保留，计入连续失败（下 tick 重投；达 `WRITEBACK_MAX_RETRIES`
    /// 熔断停投）。
    Retried,
    /// 游标保留，不计失败（guest 异步处理中 running——重投幂等由 guest
    /// 负责，超时语义在 guest 侧）。
    Pending,
}

/// 截断页打捞（通用原语）：JSON 数组被截断时按 `{`/`}` 配对状态机捞出
/// 其中仍完整的对象条目。
fn salvage_json_objects(text: &str) -> Vec<Value> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0
                    && let Ok(v) = serde_json::from_str::<Value>(text[start..=i].trim())
                {
                    out.push(v);
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// 页文本 →（条目，页是否完好）。数组/包裹形态解析成功 = 完好；失败时
/// 按对象状态机打捞（截断页可救出完整条目）。
fn parse_entries_resilient(text: &str) -> (Vec<Value>, bool) {
    if let Ok(entries) = parse_entries(text) {
        return (entries, true);
    }
    (salvage_json_objects(text), false)
}

/// 外部正文 `[CHECK]` 行提取为 AC（通用原语：容忍缩进与 `- ` 列表前缀；
/// 行内非行首出现不认；无命中 = None 走 ac_gen）。
fn extract_check_criteria(body: &str) -> Option<String> {
    let lines: Vec<String> = body
        .lines()
        .map(|l| l.trim().trim_start_matches("- ").trim().to_string())
        .filter(|l| l.starts_with("[CHECK]"))
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// fetch 条目 label 过滤（通用原语）：空 want = 全过；条目 labels[].name
/// 任一命中 = 过；无 labels 数组 = 不过（非空 want 时）。
fn labels_any_match(entry: &Value, want: &[&str]) -> bool {
    if want.is_empty() {
        return true;
    }
    entry
        .get("labels")
        .and_then(|v| v.as_array())
        .map(|labels| {
            labels.iter().any(|l| {
                l.get("name")
                    .and_then(|v| v.as_str())
                    .map(|n| want.contains(&n))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// fetch 条目过滤（声明式 `filter`）：单比较式，任一侧 `$.` 前缀 = JSON
/// path 标量化（缺席 = "null"），否则字面量。畸形式 = false（条目不入选；
/// 模板级 validate_template 已在扫描期拦畸形声明，此处是运行期兜底）。
fn eval_filter(entry: &Value, expr: &str) -> bool {
    let Some((left, op, right)) = split_compare_expr(expr) else {
        return false;
    };
    let side = |s: &str| -> String {
        if s.starts_with("$.") {
            json_path_first(entry, s)
                .map(|v| json_scalar(Some(v)))
                .unwrap_or_else(|| "null".to_string())
        } else {
            s.to_string()
        }
    };
    compare_str(op, &side(left), &side(right))
}

// ---------------------------------------------------------------------------
// token 替换（report 步 connector 参数模板）
// ---------------------------------------------------------------------------

/// `{{key}}` 替换：先实例账面（instance/ext_key/title/board_issue/
/// report_path），再 extra 表。未知 token 原样保留（诚实不猜）。
fn substitute_tokens(tpl: &str, inst: &FlowInstance, extra: &Value) -> String {
    let lookup = |key: &str| -> Option<String> {
        // 保留键先读产物账面（后步覆盖前步）；账面空回落 extra——report 步
        // 执行中产物尚未入账，extra 是唯一活源（e2e 旧实现账面空串
        // 遮蔽 extra，{{report_path}} 恒空）。两处皆无 = None（原样保留，
        // 模板残留可）。
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
            // 模板写错可）。
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
/// 反斜杠进序列化串即非法 JSON 转义（e2e 已知坑），树内替换后由
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
/// 键兜底（键本身含点且顶层恰有该字面量字段）。外部条目常见
/// `{key, fields:{summary}}` 形态——标题在嵌套层。
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

/// 条目字段取标量串：`$.` 前缀 = JSON path（取首个命中值标量化）；否则走
/// [`entry_string`] 点路径。键/标题/正文声明共用。
fn entry_scalar(entry: &Value, field: &str) -> Option<String> {
    if field.starts_with("$.") {
        json_path_first(entry, field).and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None, // 对象/数组不配做键/标题/正文（诚实不猜）
        })
    } else {
        entry_string(entry, field)
    }
}

// ---------------------------------------------------------------------------
// 共享原语（publish.rs 跨模块消费 pub(crate)：token 渲染 / 比较式拆解 /
// JSON path 子集 / 外链 scheme 闸）
// ---------------------------------------------------------------------------

/// `{{key}}` 渲染单点：lookup 决定每个键的值，未知 token 原样保留（诚实
/// 不猜）。[`substitute_tokens`] 的实例账面/步骤投影词表经 lookup 注入；
/// publish.rs 的运行期 token（{{commit}}/{{base_sha}}）也走这里。
pub(crate) fn render_tokens<F: Fn(&str) -> Option<String>>(tpl: &str, lookup: F) -> String {
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

/// 外链 scheme 闸（同源纪律）：只放行 http(s)。空串 = 无外链（放行）。
pub(crate) fn safe_external_url(raw: &str) -> Option<String> {
    // scheme 大小写不敏感（RFC 3986：HTTPS:// 与 https:// 同义）——只放行
    // http(s)，判前缀用小写比对、回传保原形（与旧外链闸语义对齐）。
    let t = raw.trim();
    let lower = t.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(t.to_string())
    } else {
        None
    }
}

/// 比较式拆解：最早出现位置命中，同位取最长 op（`>=` 不被 `>` 截断）；
/// 两侧 trim + 成对引号剥离；任一侧为空 = None（畸形，调用方诚实报错）。
pub(crate) fn split_compare_expr(expr: &str) -> Option<(&str, &str, &str)> {
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
        .filter_map(|op| expr.find(op).map(|pos| (pos, *op)))
        .min_by_key(|(pos, op)| (*pos, std::cmp::Reverse(op.len()))); // 同位时长 op（>=/>）先中
    let (pos, op) = hit?;
    let left = strip_quotes(&expr[..pos]);
    let right = strip_quotes(&expr[pos + op.len()..]);
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, op, right))
}

/// 比较求值：两侧均可解析为数字按数值比，否则按字符串比（`==`/`!=` 全等，
/// `>`/`<` 字典序）。
pub(crate) fn compare_str(op: &str, left: &str, right: &str) -> bool {
    let (ln, rn) = (left.parse::<f64>(), right.parse::<f64>());
    match (ln, rn) {
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
    }
}

/// JSON 标量化（过滤/对账比较的统一取值面）：字符串原样；数字/布尔
/// `to_string`；**对象→"object"、数组→"array"**（不是 "null"——PR 过滤
/// `$.pull_request == null` 必须能拒掉对象形态的在场值）；缺席/Null→"null"。
pub(crate) fn json_scalar(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Object(_)) => "object".to_string(),
        Some(Value::Array(_)) => "array".to_string(),
        Some(Value::Null) | None => "null".to_string(),
    }
}

/// JSON path 子集词法：`$` +（`.` 字段 | `[*]`）*。字段段读到下一个
/// `.`/`[` 为止；未知语法（`$[0]`、裸字段、`..` 空段）= None（诚实：调用方
/// 按空结果处理，不猜）。
pub(crate) fn json_path_segments(path: &str) -> Option<Vec<String>> {
    let mut rest = path.strip_prefix('$')?;
    let mut segs = Vec::new();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(['.', '[']).unwrap_or(r.len());
            let field = &r[..end];
            if field.is_empty() {
                return None; // `$.` 尾随点 / `..` 空段
            }
            segs.push(field.to_string());
            rest = &r[end..];
        } else if let Some(r) = rest.strip_prefix("[*]") {
            segs.push("*".to_string());
            rest = r;
        } else {
            return None;
        }
    }
    Some(segs)
}

/// JSON path 求值：字段段 = 对象字段取值（缺席/类型不符丢弃）；`[*]` =
/// 数组展开。返回按序命中值（wildcard 可多个）；路径畸形/中途落空 = 空。
pub(crate) fn json_path_eval<'a>(v: &'a Value, path: &str) -> Vec<&'a Value> {
    let Some(segs) = json_path_segments(path) else {
        return Vec::new();
    };
    let mut cur: Vec<&Value> = vec![v];
    for seg in segs {
        let mut next = Vec::new();
        for node in cur {
            match seg.as_str() {
                "*" => {
                    if let Some(arr) = node.as_array() {
                        next.extend(arr.iter());
                    }
                }
                field => {
                    if let Some(child) = node.get(field) {
                        next.push(child);
                    }
                }
            }
        }
        if next.is_empty() {
            return Vec::new();
        }
        cur = next;
    }
    cur
}

/// JSON path 首个命中值。
pub(crate) fn json_path_first<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    json_path_eval(v, path).into_iter().next()
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
    /// flow-step 插件注册表（guest step 批）：`plugin:<slug>:<op>`
    /// 步骤的执行通路。None = 子系统未装配（宿主壳缺席/总闸关），plugin 步骤
    /// 投递诚实失败走 on_fail。
    plugin_manager: Option<Arc<nemesis_plugins_wasm::PluginManager>>,
    #[allow(dead_code)] // 非 cluster 形态；cluster 形态派发臂消费
    cluster: ClusterOwned,
    flows: FlowStore,
    /// 写操作互斥（fetch/advance/confirm/resume 串行；读代答无锁）。
    actions: tokio::sync::Mutex<()>,
    /// 模板缓存（目录戳变化才重扫）。
    templates: tokio::sync::Mutex<TemplatesCache>,
    /// 每模板上次 fetch 时刻（Instant 单调钟，interval 闸）。
    last_fetch: tokio::sync::Mutex<HashMap<String, Instant>>,
    /// 回写连续失败计数（key = `<template>/<origin_id>`；内存态，重启 =
    /// 预算刷新）。达到 `WRITEBACK_MAX_RETRIES` 熔断停投，成功即清除。
    writeback_failures: std::sync::Mutex<HashMap<String, u32>>,
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
        plugin_manager: Option<Arc<nemesis_plugins_wasm::PluginManager>>,
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
            plugin_manager,
            cluster,
            flows,
            actions: tokio::sync::Mutex::new(()),
            templates: tokio::sync::Mutex::new(TemplatesCache {
                stamp: (0, 0),
                templates: Vec::new(),
            }),
            last_fetch: tokio::sync::Mutex::new(HashMap::new()),
            writeback_failures: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// gateway 装配期 spawn 周期 tick 循环（模板导入循环驱动器）。
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
        // 运行期总闸（BUG 修复）：子系统翻关 → tick 空转
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
        // 3) 回写轮询（C-2）：board 状态三态 diff → connector 回帖。每个
        //    模板独立 actions 锁（写操作串行纪律同 advance）。
        for def in &templates {
            let _guard = self.actions.lock().await;
            if let Err(e) = self.poll_writebacks(def).await {
                tracing::warn!(
                    "[FlowEngine] 回写轮询 {}/{} 异常: {e}",
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
    /// 副作用）、重启用不恢复（缺陷 A 根修）。
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
    /// 诚实报错（tick 记 warn；interval 闸天然限频）。分页声明（全部缺省
    /// 关）：`page_size`>0 启用，`limit_key`/`page_key` 指定分页参数名，
    /// `max_pages` 封顶；页长 < page_size = 尾页提前停。截断页（解析失败）
    /// 走对象状态机打捞 + warn。
    async fn fetch_template(&self, def: &FlowTemplateDef) -> Result<Value, String> {
        let steps = &def.template.steps;
        let first = steps
            .first()
            .ok_or_else(|| format!("模板 {} 无步骤", def.template.id))?;
        // 取数入口双形态（自持样板批）：首步合法 `plugin:<slug>:<op>`
        // 引用 = plugin fetch（guest 专管取数——自持插件自连外部系统）；
        // connector_fetch = connector 原语旧形态。plugin fetch 出参契约 =
        // `{"status":"done","entries":[...]}`，entries 即条目数组（原始远端
        // 形态——key_field/title_field/body_field/filter/labels/origin 物化
        // 语义与 connector_fetch 完全一致，走同一 materialize_instances）。
        if let Some((slug, op)) = nemesis_plugins_wasm::manifest::parse_plugin_step_ref(&first.kind)
        {
            return self.plugin_fetch_template(def, slug, op).await;
        }
        if first.kind != KIND_FETCH {
            return Err(format!(
                "模板 {} 首步必须是 {KIND_FETCH} 或 plugin:<slug>:<op> 引用（当前 {}）",
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
        let page_size = arg("page_size").trim().parse::<usize>().unwrap_or(0);
        let limit_key = {
            let k = arg("limit_key");
            if k.trim().is_empty() {
                "limit".to_string()
            } else {
                k
            }
        };
        let page_key = {
            let k = arg("page_key");
            if k.trim().is_empty() {
                "page".to_string()
            } else {
                k
            }
        };
        let max_pages = arg("max_pages")
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|v| *v > 0)
            .unwrap_or(20);
        let base_args = match first.args.get("args_json") {
            Some(raw) if !raw.trim().is_empty() => serde_json::from_str::<Value>(raw)
                .map_err(|e| format!("模板 {} args_json 非法: {e}", def.template.id))?,
            _ => json!({}),
        };
        if page_size == 0 {
            // 单页（旧形态）：整页解析失败 = Err（garbage 不打捞——完整响应
            // 解析失败是模板/端点契约问题，得响）；只救截断形态没意义因为单
            // 页不存在截断位置。
            let text = self.invoke_connector(&connector, &op, &base_args).await?;
            let (entries, parse_ok) = parse_entries_resilient(&text);
            if !parse_ok && entries.is_empty() {
                return Err(format!(
                    "模板 {} 取数响应既非 JSON 数组/包裹形态也无完整对象可打捞",
                    def.template.id
                ));
            }
            let created = materialize_instances(&self.flows, def, &entries)?;
            return Ok(json!({
                "template": def.template.id,
                "entries": entries.len(),
                "created": created,
            }));
        }
        // 分页循环：逐页取数解析合流；截断页打捞 + 停（页边界丢了续取无意义）。
        let mut entries: Vec<Value> = Vec::new();
        for page in 1..=max_pages {
            let mut page_args = base_args.clone();
            let obj = page_args.as_object_mut().ok_or_else(|| {
                format!(
                    "模板 {} args_json 必须是对象（分页合流需要）",
                    def.template.id
                )
            })?;
            obj.insert(limit_key.clone(), json!(page_size));
            obj.insert(page_key.clone(), json!(page));
            let text = self.invoke_connector(&connector, &op, &page_args).await?;
            let (mut page_entries, parse_ok) = parse_entries_resilient(&text);
            if !parse_ok {
                tracing::warn!(
                    "[FlowEngine] {}/{} 第 {page} 页响应解析失败，打捞 {} 条后停取",
                    def.slug,
                    def.template.id,
                    page_entries.len()
                );
                entries.append(&mut page_entries);
                break;
            }
            let got = page_entries.len();
            entries.append(&mut page_entries);
            if got < page_size {
                break; // 尾页
            }
        }
        let created = materialize_instances(&self.flows, def, &entries)?;
        Ok(json!({
            "template": def.template.id,
            "entries": entries.len(),
            "created": created,
        }))
    }

    /// plugin fetch（guest 专管取数， 自持样板批）：模板级
    /// run_flow_step 投递——无实例上下文，session_key 固定
    /// `flow:<template>/fetch`（审计归因键），op + args = 首步声明直投
    /// （装载期已渲染 {{config.*}}；分页声明不适用——分页是 guest args 面）。
    /// 四态
    /// 映射：done.entries → 既有 materialize_instances；skipped = 本轮无新
    /// 条目（空物化）；running = 诚实 Err（fetch 单 tick 同步语义——异步
    /// 取数应改为同步完成后返回）；failed/malformed/子系统缺席 = 诚实 Err
    ///（tick 记 warn，interval 闸天然限频）。
    async fn plugin_fetch_template(
        &self,
        def: &FlowTemplateDef,
        slug: &str,
        op: &str,
    ) -> Result<Value, String> {
        let manager = self.plugin_manager.as_ref().ok_or_else(|| {
            format!(
                "模板 {} plugin fetch 失败: flow-step 插件子系统未装配（宿主壳缺席/总闸关）",
                def.template.id
            )
        })?;
        let first = &def.template.steps[0];
        let args: serde_json::Map<String, Value> = first
            .args
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let input = json!({
            "op": op,
            "args": Value::Object(args),
            "session_key": format!("flow:{}/fetch", def.template.id),
        })
        .to_string();
        let entries = match manager.run_flow_step(slug, op, input).await {
            Err(e) => {
                return Err(format!(
                    "模板 {} plugin fetch 投递失败: {e}",
                    def.template.id
                ));
            }
            Ok(out_json) => match plugin_step_outcome(&out_json) {
                PluginOutcome::Done(payload) => plugin_fetch_entries(&payload, &def.template.id)?,
                PluginOutcome::Skipped(_) => Vec::new(),
                PluginOutcome::Running => {
                    return Err(format!(
                        "模板 {} plugin fetch guest 处理中（running）——fetch 是单 tick \
                         同步语义，取数 op 应同步完成后返回",
                        def.template.id
                    ));
                }
                PluginOutcome::Failed(err) | PluginOutcome::Malformed(err) => {
                    return Err(format!("模板 {} plugin fetch 失败: {err}", def.template.id));
                }
            },
        };
        let created = materialize_instances(&self.flows, def, &entries)?;
        Ok(json!({
            "template": def.template.id,
            "entries": entries.len(),
            "created": created,
        }))
    }

    /// connector 工具面调用（过完整 dispatch 瀑布 + 审计 + estop）。
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
        if crate::connector_tools::is_tool_error(&text) {
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
        // plugin 步骤（guest step， 批）：专管路径全权处置——
        // 同步 done 同 tick 完成（board_land 形态）、running 转 Running 逐
        // tick 轮询、failed/skipped 立即记账。estop 冻结由 tick loop 承担，
        // 这里天然继承。
        if nemesis_plugins_wasm::manifest::parse_plugin_step_ref(&cur.kind).is_some() {
            self.run_plugin_step(inst, cur).await;
            return;
        }
        enum Out {
            Done(Value),
            Start,
            Fail(String),
        }
        let outcome = match cur.kind.as_str() {
            KIND_LAND => {
                let outcome = start_board_land(
                    &self.store,
                    inst,
                    cur.args_get("project").as_deref(),
                    cur.args_get("body_preamble").as_deref(),
                );
                match outcome {
                    Ok(a) => Out::Done(a),
                    Err(e) => Out::Fail(e),
                }
            }
            // 同步步（land 同款同 tick Done/Fail；阻塞 git2 调用与 publish
            // 步的阻塞 push 同先例）。
            KIND_REPO_SYNC => match self.run_repo_sync(cur) {
                Ok(a) => Out::Done(a),
                Err(e) => Out::Fail(e),
            },
            KIND_FIXLOOP => {
                // 执行面 = 模板步显式声明（args.target：`self` / 节点名）；
                // 未声明 = None 落既有匹配器/兜底阶梯（向后兼容）。
                let declared = cur.args_get("target");
                let dispatch =
                    |issue: &nemesis_board::Issue| self.dispatch_issue(issue, declared.as_deref());
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
        // plugin 步骤（guest step）：与 start 同一专管路径——running 出态
        // 保持 Running，下轮 tick 重投 run-step（轮询 = 重跑，幂等由 guest
        // 负责）。
        if nemesis_plugins_wasm::manifest::parse_plugin_step_ref(&cur.kind).is_some() {
            self.run_plugin_step(inst, cur).await;
            return;
        }
        let result: Result<Value, String> = match cur.kind.as_str() {
            KIND_FIXLOOP => poll_outcome(poll_fixloop(&self.store, inst)),
            KIND_REGRESS => poll_outcome(regress_outcome(&self.store, inst)),
            KIND_PUBLISH => match self.publish_protocol(inst, cur) {
                Ok((args, protocol)) => {
                    crate::publish::publish_task_delivery(
                        &self.db,
                        &self.store,
                        &self.agent_loop,
                        &self.workspace,
                        &args,
                        Some(&protocol),
                        self.plugin_manager.as_ref(),
                    )
                    .await
                }
                Err(e) => Err(e),
            },
            KIND_REPORT => self.run_report(inst, cur).await,
            // 重启恢复路径：Running 的 land 重跑（origin 幂等）。
            KIND_LAND => start_board_land(
                &self.store,
                inst,
                cur.args_get("project").as_deref(),
                cur.args_get("body_preamble").as_deref(),
            ),
            // 重启恢复路径：Running 的 repo_sync 幂等重跑（sync 自身幂等
            // ——重跑时 UpToDate = 零动作）。
            KIND_REPO_SYNC => self.run_repo_sync(cur),
            other => Err(format!("未知 step kind: {other}")),
        };
        match result {
            Ok(a) => complete_step(&self.flows, inst, &a),
            Err(e) => fail_step(&self.flows, inst, &e),
        }
    }

    /// plugin 步骤执行（guest step 专管路径）：投递 run-step + 四态映射。
    ///
    /// start（Pending）与 poll（Running）共用同一方法——running 出态不做
    /// 任何记账（fail_step 哨兵同语义），下轮 tick 重投；done/skipped 同
    /// tick 推进；failed 走 on_fail 既有链（Retry 账本由 fail_step 记，
    /// 投递本身不推计数避免双计）。
    async fn run_plugin_step(&self, inst: &mut FlowInstance, cur: &StepRun) {
        let Some((slug, op)) = nemesis_plugins_wasm::manifest::parse_plugin_step_ref(&cur.kind)
        else {
            fail_step(
                &self.flows,
                inst,
                &format!("plugin 步骤 kind 解析失败: {}", cur.kind),
            );
            return;
        };
        let Some(manager) = self.plugin_manager.as_ref() else {
            fail_step(
                &self.flows,
                inst,
                &format!(
                    "plugin:{slug}:{op} 投递失败: flow-step 插件子系统未装配（宿主壳缺席/总闸关）"
                ),
            );
            return;
        };
        let session_key = format!("flow:{}/{}", inst.template, inst.instance);
        let input = plugin_step_input(inst, cur, op, &session_key);
        match manager.run_flow_step(slug, op, input).await {
            Err(e) => fail_step(
                &self.flows,
                inst,
                &format!("plugin:{slug}:{op} 投递失败: {e}"),
            ),
            Ok(out_json) => match plugin_step_outcome(&out_json) {
                PluginOutcome::Done(artifacts) => complete_step(&self.flows, inst, &artifacts),
                PluginOutcome::Running => {} // 保持 Running，下轮重投
                PluginOutcome::Skipped(artifacts) => {
                    skip_step_with_payload(&self.flows, inst, artifacts)
                }
                PluginOutcome::Failed(err) => fail_step(&self.flows, inst, &err),
                PluginOutcome::Malformed(err) => fail_step(&self.flows, inst, &err),
            },
        }
    }

    /// `repo_sync`（jira-gitlab-flow 批）：项目仓库确定性
    /// clone-or-pull。项目目录唯一依据 = 注册表 `directory`（与 land
    /// 的按名归属同一绑定；项目未注册/无目录 = 诚实 Err，不自动注册
    /// ——directory 绑定不可变语义不动）。凭据经 `cred_secret` vault
    /// 别名（publish 同源 `resolve_alias_at`；缺席 = 匿名）。
    fn run_repo_sync(&self, cur: &StepRun) -> Result<Value, String> {
        let project = cur
            .args_get("project")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "repo_sync 步缺 project（board 项目名必填）".to_string())?;
        let url = cur
            .args_get("url")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "repo_sync 步缺 url（远端 clone URL 必填）".to_string())?;
        let projects = self
            .store
            .list_projects()
            .map_err(|e| format!("项目清单读取失败: {e}"))?;
        let proj = projects
            .into_iter()
            .find(|p| p.name == project)
            .ok_or_else(|| {
                format!("项目不存在: {project}（repo_sync 不自动注册——先建项目再发车）")
            })?;
        let dir = proj
            .directory
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                format!("项目 {project} 无目录（directory 空）——repo_sync 无法定位仓库")
            })?;
        let dir = PathBuf::from(dir);
        // 凭据：cred_secret 别名 → vault（如 plugin/<slug>/pat）；别名缺席/
        // 未配 = 匿名（私有仓 clone/pull 诚实失败）。
        let cred = match cur
            .args_get("cred_secret")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            Some(alias) => {
                let vault_path = nemesis_path::resolve_vault_path_in_workspace(&self.workspace);
                match crate::vault_runtime::resolve_alias_at(&vault_path, &alias) {
                    Ok(pat) if !pat.trim().is_empty() => {
                        let user = cur
                            .args_get("cred_user")
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "oauth2".to_string());
                        Some((user, pat))
                    }
                    _ => None,
                }
            }
            None => None,
        };
        let branch = cur
            .args_get("branch")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // 代理：proxy 配置键（{{config.proxy}} 渲染产物；空/"{{" 残留 = 未配置
        // → None = 直连现行为；Some(url) → libgit2 ProxyOptions 显式走代理——
        // libgit2 不读环境变量，企业内网仓库必须显式声明）。
        let proxy = cur
            .args_get("proxy")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !s.contains("{{"));
        let (outcome, head) = nemesis_board::git_repo::sync_repo_remote(
            &url,
            &dir,
            branch.as_deref(),
            cred.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
            proxy.as_deref(),
        )?;
        Ok(json!({
            "sync_outcome": match outcome {
                nemesis_board::git_repo::SyncOutcome::Cloned => "cloned",
                nemesis_board::git_repo::SyncOutcome::Checkout => "checkout",
                nemesis_board::git_repo::SyncOutcome::UpToDate => "up_to_date",
                nemesis_board::git_repo::SyncOutcome::FastForward => "fast_forward",
                nemesis_board::git_repo::SyncOutcome::Merged => "merged",
            },
            "head": head,
            "directory": dir.display().to_string(),
            "branch": branch.unwrap_or_else(|| "(default)".to_string()),
        }))
    }

    /// publish 入参 + 声明化协议（C-3）：任务锚点（最新派发的 task_id）+
    /// 步骤声明的 connector/owner/repo；协议字段（create/reconcile op、
    /// 参数模板、匹配式）从步骤 args 解析，模板 token 先按取数上下文渲染
    /// （parse-time），publish 执行期再做第二遍运行时 token（commit 等）。
    /// 协议缺失 = 诚实报错（宿主无内置发布协议——域知识只在模板声明面）。
    fn publish_protocol(
        &self,
        inst: &FlowInstance,
        cur: &StepRun,
    ) -> Result<(Value, crate::publish::PublishProtocol), String> {
        let issue = find_board_issue(&self.store, inst)?;
        let dispatches = self
            .store
            .list_dispatches(issue.id)
            .map_err(|e| format!("派发记录读取失败: {e}"))?;
        let task_id = dispatches
            .last()
            .map(|d| d.task_id.clone())
            .ok_or_else(|| format!("{} 无派发记录，无可发布交付", issue.number))?;
        let mut args = json!({ "task_id": task_id, "branch": format!("nemesis/task-{task_id}") });
        // push_ref/cred_secret/cred_user/proxy：发布侧原始参数（raw 拷贝——
        // parse 期 {{config.*}} 已被 render_config_tokens 渲掉；残留 "{{" =
        // 未配置语义，发布侧诚实按缺席处理）。
        for key in [
            "connector",
            "owner",
            "repo",
            "push_ref",
            "cred_secret",
            "cred_user",
            "proxy",
        ] {
            if let Some(v) = cur.args_get(key) {
                args[key] = json!(v);
            }
        }
        // parse-time token 渲染上下文：task/branch/issue 侧事实 + 最新产物。
        let extra = json!({
            "task_id": task_id,
            "branch": format!("nemesis/task-{task_id}"),
            "title": issue.title,
            "instance": inst.instance,
            "ext_key": inst.ext_key,
            "board_issue": issue.number,
            "report_path": latest_artifact_str(inst, "report_path"),
        });
        let get_arg = |key: &str| cur.args_get(key);
        let render = |tpl: &str| substitute_tokens(tpl, inst, &extra);
        let protocol = parse_publish_step_protocol(&get_arg, &render)?;
        Ok((args, protocol))
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
        // 可选回帖：args.connector 声明才做。路由双形态（自持样板
        // 批）：`plugin:<slug>:<op>` 引用 = guest 直投（自持插件回帖自有系统
        // ——op 段即引用组成，无需另声明 op）；其余 = connector 原语 op
        //（op 必填，宿主无内置回帖协议——域知识只在模板声明面）。
        if let Some(connector) = cur.args_get("connector").filter(|s| !s.trim().is_empty()) {
            let trimmed = connector.trim();
            if trimmed.starts_with("plugin:") {
                self.report_plugin_comment(inst, cur, trimmed, &report_path)
                    .await?;
            } else {
                let op = cur
                    .args_get("op")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "report 步缺 op 声明（域知识只在模板）".to_string())?;
                let args_raw = cur
                    .args_get("args_json")
                    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                    .unwrap_or_else(|| json!({}));
                // 在 Value 树上逐 String 替换（e2e 已知坑：序列化串上
                // 替换会把 Windows 路径反斜杠变成非法 JSON 转义，落到
                // {"comment": path} 兜底丢 key/body）；替换结果即请求体。
                let args = substitute_tokens_value(
                    &args_raw,
                    inst,
                    &json!({ "report_path": report_path }),
                );
                if let Err(e) = self.invoke_connector(trimmed, &op, &args).await {
                    return Err(format!("报告已导出但回帖失败: {e}"));
                }
            }
        }
        Ok(json!({ "report_path": report_path, "issues": outcome.issue_numbers.len() }))
    }

    /// report 回帖 plugin 臂：connector 字段 `plugin:<slug>:<op>` 引用经
    /// flow-step 通道直投（入参投影 = compose_plugin_writeback 同款
    /// `{"op", "args", "session_key": "flow:<template>/<instance>"}`；
    /// args_json 已含 report_path token 渲染）。done/skipped = 回帖收口；
    /// running = 诚实
    /// Err（回帖未确认——fetch 同款单 tick 同步语义）；failed/malformed =
    /// Err（报错文案保留「报告已导出」事实——dossier 产物不回滚）。
    async fn report_plugin_comment(
        &self,
        inst: &FlowInstance,
        cur: &StepRun,
        plugin_ref: &str,
        report_path: &str,
    ) -> Result<(), String> {
        let Some((slug, op)) = nemesis_plugins_wasm::manifest::parse_plugin_step_ref(plugin_ref)
        else {
            return Err(format!("report 步 plugin 引用非法: {plugin_ref}"));
        };
        let Some(manager) = self.plugin_manager.as_ref() else {
            return Err(
                "report 回帖失败: flow-step 插件子系统未装配（宿主壳缺席/总闸关）".to_string(),
            );
        };
        let args_raw = cur
            .args_get("args_json")
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .unwrap_or_else(|| json!({}));
        let args = substitute_tokens_value(&args_raw, inst, &json!({ "report_path": report_path }));
        let input = json!({
            "op": op,
            "args": args,
            "session_key": format!("flow:{}/{}", inst.template, inst.instance),
        })
        .to_string();
        match manager.run_flow_step(slug, op, input).await {
            Err(e) => Err(format!("报告已导出但回帖失败: {e}")),
            Ok(out_json) => match plugin_step_outcome(&out_json) {
                PluginOutcome::Done(_) | PluginOutcome::Skipped(_) => Ok(()),
                PluginOutcome::Running => Err(
                    "报告已导出但回帖投递中（guest running）——回帖未确认，本步按失败走 \
                     on_fail；重投幂等由 guest 负责"
                        .to_string(),
                ),
                PluginOutcome::Failed(err) | PluginOutcome::Malformed(err) => {
                    Err(format!("报告已导出但回帖失败: {err}"))
                }
            },
        }
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
    /// 非 cluster 形态诚实不可派发）。`declared` = fixloop 步显式声明的
    /// 执行面（模板 args `target`）：`self` = 本节点 / 节点名 = 指定节点
    /// ——一等声明、钉住即走、不在线诚实停车，不走匹配器与兜底阶梯
    /// （无回退语义；缺省 None 才落既有匹配阶梯）。
    #[cfg(feature = "cluster")]
    fn dispatch_issue(&self, issue: &nemesis_board::Issue, declared: Option<&str>) -> bool {
        let explicit = declared.map(materialize_exec_target);
        let actor = nemesis_board::Actor::system("flow-engine");
        nemesis_web::handlers::board::dispatch_issue_for_flow(
            &self.store,
            self.cluster.as_ref(),
            issue,
            explicit.as_deref(),
            &actor,
        )
    }

    #[cfg(not(feature = "cluster"))]
    fn dispatch_issue(&self, _issue: &nemesis_board::Issue, _declared: Option<&str>) -> bool {
        false // 非 cluster 编译形态：诚实不可派发（web 侧同款纪律）
    }

    // -- 回写轮询（C-2） ----------------------------------------------------

    /// board 状态三态 diff → connector 回帖（模板 `on_transition` 声明驱动）。
    /// 游标落 `<flows-root>/_transitions/<template>/<sanitized>.json`：
    /// 游标缺席 = 静默收编（存盘不通知——首轮不为存量单发历史通知）；
    /// 游标 = 当前 = noop；游标 ≠ 当前 = 组包回帖。组包失败/回帖失败 = 游标
    /// 不动（下 tick 重试，transient；连续超限由调用方熔断停投）。
    async fn poll_writebacks(&self, def: &FlowTemplateDef) -> Result<(), String> {
        let hook = &def.template.on_transition;
        if hook.op.trim().is_empty() {
            return Ok(()); // 模板未声明回写 = 不订阅
        }
        let fetch = def
            .template
            .steps
            .first()
            .ok_or_else(|| format!("模板 {} 无步骤", def.template.id))?;
        let (origin_type, _, _) = resolve_instance_origin(&def.template, fetch, "")
            .map_err(|e| format!("模板 {} 回写 origin_type 解析失败: {e}", def.template.id))?;
        let issues = self
            .store
            .list_issues_by_origin_type(&origin_type, WRITEBACK_POLL_LIMIT)
            .map_err(|e| format!("origin 订阅读取失败: {e}"))?;
        for issue in issues {
            let Some(origin) = issue.origin.as_ref() else {
                continue;
            };
            let status = issue.status.as_str();
            if status != "in_review" && status != "done" && status != "cancelled" {
                continue; // 非回写三态
            }
            let origin_id = origin.origin_id.as_str();
            let stored = self
                .flows
                .load_transition_state(&def.template.id, origin_id);
            match writeback_decision(stored.as_deref(), status) {
                WritebackDecision::Noop => continue,
                WritebackDecision::Adopt => {
                    let _ = self
                        .flows
                        .save_transition_state(&def.template.id, origin_id, status);
                    tracing::debug!("[FlowEngine] 回写游标静默收编 {origin_id} → {status}");
                    continue;
                }
                WritebackDecision::Compose { from } => {
                    // 写回熔断：同一目标连续失败达上限 = 停投
                    // （重复副作用防线——对端可能已收下请求只是响应丢失，
                    // 无限重投即重复写）。恢复 = 修复对端后重启 gateway
                    // （重试预算随进程刷新）；成功投递即清除计数。
                    let key = format!("{}/{}", def.template.id, origin_id);
                    let at_cap = {
                        let map = self
                            .writeback_failures
                            .lock()
                            .expect("writeback_failures lock");
                        let n = map.get(&key).copied().unwrap_or(0);
                        if writeback_circuit_open(n) {
                            tracing::debug!(
                                "[FlowEngine] 回写熔断中 {key}（连续失败 {n} 次）——停投"
                            );
                            true
                        } else {
                            false
                        }
                    };
                    if at_cap {
                        continue;
                    }
                    match self
                        .compose_writeback(def, hook, origin, &issue.number, status, &from)
                        .await
                    {
                        WritebackResult::Advanced => {
                            self.writeback_failures
                                .lock()
                                .expect("writeback_failures lock")
                                .remove(&key);
                        }
                        WritebackResult::Retried => {
                            let mut map = self
                                .writeback_failures
                                .lock()
                                .expect("writeback_failures lock");
                            let n = map.entry(key).or_insert(0);
                            *n += 1;
                            if *n == WRITEBACK_MAX_RETRIES {
                                tracing::warn!(
                                    "[FlowEngine] 回写连续失败 {WRITEBACK_MAX_RETRIES} 次，熔断停投 {origin_id}（模板 {}）；游标保留，修复对端后重启 gateway 恢复",
                                    def.template.id
                                );
                            }
                        }
                        WritebackResult::Pending => {}
                    }
                }
            }
        }
        Ok(())
    }

    /// 单条回帖组包 + 发送。token 表 = `transition.*`（status/from/number/
    /// text）+ `origin_meta.<key>`（坐标落库载体，优先）或 `origin_id.<name>`
    /// （parse-origin-id 现场分解，v1 旧单兜底）。两者皆缺 = 诚实静默跳过
    /// （游标落盘=永久跳过）；分解/渲染/发送失败 = 游标保留（下 tick 重试，
    /// 计入连续失败）。返回值驱动调用方熔断簿记。
    async fn compose_writeback(
        &self,
        def: &FlowTemplateDef,
        hook: &FlowTransition,
        origin: &nemesis_board::TaskOrigin,
        number: &str,
        status: &str,
        from: &str,
    ) -> WritebackResult {
        let origin_id = origin.origin_id.as_str();
        let table = match build_writeback_table(origin, hook, number, status, from) {
            WritebackTableOutcome::Ready(table) => table,
            WritebackTableOutcome::PermanentSkip => {
                tracing::debug!(
                    "[FlowEngine] {origin_id} 无 origin_meta 也无 parse-origin-id 声明，回写永久跳过"
                );
                let _ = self
                    .flows
                    .save_transition_state(&def.template.id, origin_id, status);
                return WritebackResult::Advanced;
            }
            WritebackTableOutcome::Retry(e) => {
                tracing::warn!(
                    "[FlowEngine] 回写 origin_id 分解失败 {origin_id}: {e}；游标保留待重试"
                );
                return WritebackResult::Retried;
            }
        };
        // 模板 args 逐值严格渲染（未知 token = Err）。
        let mut args = serde_json::Map::new();
        for (k, tpl) in &hook.args {
            match render_writeback_tokens(tpl, &table) {
                Ok(v) => {
                    args.insert(k.clone(), json!(v));
                }
                Err(e) => {
                    tracing::warn!(
                        "[FlowEngine] 回写 args 渲染失败 {origin_id} ({k}): {e}；游标保留待重试"
                    );
                    return WritebackResult::Retried;
                }
            }
        }
        let args = Value::Object(args);
        // 回写路由（自持写通道批）：`plugin:<slug>:<op>` 引用 =
        // guest 步直投（自持插件回写自有系统——域知识全在插件侧）；其余 =
        // connector 原语 op（fetch 步声明，回写与取数同一外部系统）。
        if hook.op.trim().starts_with("plugin:") {
            return self
                .compose_plugin_writeback(def, hook, origin_id, status, args)
                .await;
        }
        // connector = fetch 步声明（回写与取数同源——同一外部系统）。
        let connector = def
            .template
            .steps
            .first()
            .and_then(|s| s.args.get("connector").cloned())
            .unwrap_or_default();
        if connector.trim().is_empty() {
            tracing::warn!("[FlowEngine] 回写步缺 connector 声明 {origin_id}；游标保留待重试");
            return WritebackResult::Retried;
        }
        match self
            .invoke_connector(connector.trim(), &hook.op, &args)
            .await
        {
            Ok(_) => {
                let _ = self
                    .flows
                    .save_transition_state(&def.template.id, origin_id, status);
                tracing::info!("[FlowEngine] 回写完成 {origin_id} → {status}");
                WritebackResult::Advanced
            }
            Err(e) => {
                tracing::warn!("[FlowEngine] 回写发送失败 {origin_id}: {e}；游标保留待重试");
                WritebackResult::Retried
            }
        }
    }

    /// plugin: 回写投递（自持写通道批）：`plugin:<slug>:<op>` 经
    /// flow-step 通道直投 guest；入参投影 = `{"op": <op>, "args": <渲染后
    /// args>, "session_key": "flow:<template>/<origin_id>"}`（审计归因键与
    /// 实例键同形态）。四态映射：done/skipped = 游标推进（回写即收口——回写无
    /// 步骤记账面，负载只进日志不落盘）；failed/running/malformed = 游标
    /// 保留下 tick 重试（running = guest 异步处理中，重投幂等由 guest
    /// 负责，不计连续失败）。子系统未装配/引用非法 = 诚实失败游标保留。
    async fn compose_plugin_writeback(
        &self,
        def: &FlowTemplateDef,
        hook: &FlowTransition,
        origin_id: &str,
        status: &str,
        args: Value,
    ) -> WritebackResult {
        let op = hook.op.trim();
        let Some((slug, op_name)) = nemesis_plugins_wasm::manifest::parse_plugin_step_ref(op)
        else {
            tracing::warn!(
                "[FlowEngine] 回写 plugin 引用非法 {op}（模板 {}）；游标保留待重试",
                def.template.id
            );
            return WritebackResult::Retried;
        };
        let Some(manager) = self.plugin_manager.as_ref() else {
            tracing::warn!(
                "[FlowEngine] 回写 {op} 投递失败: flow-step 插件子系统未装配（宿主壳缺席/总闸关）；游标保留待重试"
            );
            return WritebackResult::Retried;
        };
        let input = json!({
            "op": op_name,
            "args": args,
            "session_key": format!("flow:{}/{}", def.template.id, origin_id),
        })
        .to_string();
        match manager.run_flow_step(slug, op_name, input).await {
            Err(e) => {
                tracing::warn!("[FlowEngine] 回写 {op} 投递失败: {e}；游标保留待重试");
                WritebackResult::Retried
            }
            Ok(out_json) => match plugin_step_outcome(&out_json) {
                PluginOutcome::Done(_) => {
                    let _ = self
                        .flows
                        .save_transition_state(&def.template.id, origin_id, status);
                    tracing::info!(
                        "[FlowEngine] 回写完成（plugin:{slug}:{op_name}）{origin_id} → {status}"
                    );
                    WritebackResult::Advanced
                }
                PluginOutcome::Skipped(_) => {
                    // guest 自报 skipped = 本轮无需回写（幂等去重声明）——
                    // 游标同步推进，与 done 同语义（重投只会重复跳过）。
                    let _ = self
                        .flows
                        .save_transition_state(&def.template.id, origin_id, status);
                    tracing::info!(
                        "[FlowEngine] 回写跳过（guest 自报 skipped，plugin:{slug}:{op_name}）{origin_id} → {status}"
                    );
                    WritebackResult::Advanced
                }
                PluginOutcome::Running => {
                    tracing::debug!(
                        "[FlowEngine] 回写 {op} guest 处理中（running）{origin_id}；游标保留待重投"
                    );
                    WritebackResult::Pending
                }
                PluginOutcome::Failed(err) | PluginOutcome::Malformed(err) => {
                    tracing::warn!("[FlowEngine] 回写 {op} 失败: {err}；游标保留待重试");
                    WritebackResult::Retried
                }
            },
        }
    }

    // -- 代答面（FlowBridge 委托） ------------------------------------------

    /// `flow://<template>/<view>` 代答单点。
    pub async fn answer(&self, template: &str, view: &str, args: &Value) -> Result<Value, String> {
        // 运行期总闸（BUG 修复）：`ui.invoke` 的 flow:// 分支
        // 在 web 侧 require_installer 之前直入本桥——子系总闸/宿主壳开关
        // 翻关后必须在此拦截，否则面板页面在框架关闭后仍可代答。
        if !nemesis_config::plugin_wasm::runtime_enabled(&self.workspace) {
            return Err(
                "WASM 插件子系统已关闭（总闸或宿主壳开关关）——流程面板不可用，开关改回即时恢复"
                    .to_string(),
            );
        }
        // 单插件闸（缺陷 B 根修）：current_templates 按开关过滤
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

/// 条件步不成立（决策 B）：Skipped 留痕（条件式进 artifacts，面板可）
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

/// plugin 步骤 guest 自报 skipped：when 假语义同款（Skipped + 游标同 tick
/// 推进，不走 on_fail），artifacts 存 guest 自由负载（原样落盘）。
fn skip_step_with_payload(flows: &FlowStore, inst: &mut FlowInstance, payload: Value) {
    if let Some(step) = inst.steps.get_mut(inst.current_step) {
        step.status = StepStatus::Skipped;
        step.artifacts = payload;
        step.finished_at_ms = Some(now_ms());
    }
    inst.current_step += 1;
    inst.updated_at_ms = now_ms();
    let _ = flows.save(inst);
}

// ---------------------------------------------------------------------------
// plugin 步骤（guest step）：入参投影构造 + 出参四态解析（自由函数，单测钉死）
// ---------------------------------------------------------------------------

/// run-step 出参四态（status 词表 done/running/failed/skipped + 自由负载）。
/// `pub(crate)`：publish 步 plugin 路由臂复用同一解析单点（a）。
#[derive(Debug, PartialEq)]
pub(crate) enum PluginOutcome {
    /// 同步完成：负载 = artifacts 落盘。
    Done(Value),
    /// 保持 Running，下轮 tick 重投（轮询 = 重跑，幂等由 guest 负责）。
    Running,
    /// guest 自报跳过：Skipped 同款推进，负载 = artifacts。
    Skipped(Value),
    /// guest 自报失败：走 on_fail 既有链。
    Failed(String),
    /// 出参畸形/未知 status：按失败走 on_fail（诚实不静默）。
    Malformed(String),
}

/// run-step 出参解析：`{"status": <词表>,...自由负载}`。畸形（非 JSON /
/// 缺 status / 未知 status）= Malformed；failed 的错误文本取负载 `error`
/// 字段（缺省 = status 原文）。负载 = 去 status 键后的对象（done/skipped
/// 落 artifacts；failed 不落——错误文本进 step.error 走 ERR_SNIPPET 截断）。
pub(crate) fn plugin_step_outcome(out_json: &str) -> PluginOutcome {
    let Ok(mut v) = serde_json::from_str::<Value>(out_json.trim()) else {
        return PluginOutcome::Malformed(format!("run-step 出参非 JSON: {}", snippet_of(out_json)));
    };
    let Some(status) = v.get("status").and_then(Value::as_str).map(str::to_string) else {
        return PluginOutcome::Malformed("run-step 出参缺 status 字段".to_string());
    };
    let payload = match v.as_object_mut() {
        Some(o) => {
            o.remove("status");
            Value::Object(o.clone())
        }
        None => json!({}),
    };
    match status.as_str() {
        "done" => PluginOutcome::Done(payload),
        "running" => PluginOutcome::Running,
        "failed" => {
            let err = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("guest 自报 failed（无 error 明细）");
            PluginOutcome::Failed(err.to_string())
        }
        "skipped" => PluginOutcome::Skipped(payload),
        other => PluginOutcome::Malformed(format!("run-step 出参未知 status: {other}")),
    }
}

/// 错误文本摘要（ERR_SNIPPET 同长度，畸形出参回灌用）。
fn snippet_of(s: &str) -> String {
    s.trim().chars().take(ERR_SNIPPET).collect()
}

/// plugin fetch 出参 entries 提取（done 负载 → 条目数组）。契约 =
/// `{"entries": [...]}` 键必带且为数组——缺键/非数组 = Err（guest 契约坏了
/// 必须响，不能落「零条目」静默）。条目保持 guest 返回的原始远端形态，
/// 物化路径（key_field/title_field/body_field 提取）不在此处做。
fn plugin_fetch_entries(payload: &Value, template_id: &str) -> Result<Vec<Value>, String> {
    match payload.get("entries") {
        Some(Value::Array(arr)) => Ok(arr.clone()),
        other => Err(format!(
            "模板 {template_id} plugin fetch 出参缺 entries 数组（guest 契约 \
             {{\"status\":\"done\",\"entries\":[...]}}），实得: {}",
            other
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<缺 entries 键>".to_string())
        )),
    }
}

/// run-step 入参投影（宿主构造）：op + token 渲染后 args + 实例身份 +
/// steps 状态投影 + board_issue 锚点 + attempts 计数 + session_key（审计
/// 归因键）。token 渲染与六原语同源（substitute_tokens_value；未知 token
/// 原样保留可）。attempts = 本轮重试序号（宿主 fail_step 的 Retry 账本
/// 记失败，投递本身不推计数避免双计；轮询重跑幂等由 guest 负责，guest
/// 对账用 instance + 自身 data 目录状态）。不含图片字节与凭据。
fn plugin_step_input(inst: &FlowInstance, cur: &StepRun, op: &str, session_key: &str) -> String {
    let mut steps = serde_json::Map::new();
    for s in &inst.steps {
        steps.insert(
            s.id.clone(),
            json!({
                "status": s.status.as_str(),
                "error": s.error.clone().unwrap_or_default(),
                "attempts": s.attempts,
            }),
        );
    }
    // board_issue 锚点：board_land 产物账面（数字化；缺席 = null）。
    let board_issue = latest_artifact_str(inst, "board_issue")
        .parse::<i64>()
        .map(Value::from)
        .unwrap_or(Value::Null);
    let args_raw: Value = cur
        .args
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let args = substitute_tokens_value(&args_raw, inst, &json!({}));
    json!({
        "op": op,
        "args": args,
        "instance": {
            "template": inst.template,
            "instance": inst.instance,
            "ext_key": inst.ext_key,
            "title": inst.title,
            "origin_type": inst.origin_type,
            "origin_id": inst.origin_id,
        },
        "steps": Value::Object(steps),
        "board_issue": board_issue,
        "attempts": cur.attempts + 1,
        "session_key": session_key,
    })
    .to_string()
}

/// 条件求值（决策 B）：单比较 `左 <op> 右`。op 词表 `==`/`!=`/`>=`/`<=`/
/// `>`/`<`（两字符 op 先于单字符命中——按首次出现位置取最早，同位取长）。
/// 两侧 trim + 成对引号剥离；两侧均可解析为数字按数值比，否则按字符串比
/// （`==`/`!=` 全等，`>`/`<` 字典序）。畸形（无 op / 空侧）→ Err，调用方
/// 按失败走 on_fail。
fn eval_when(rendered: &str) -> Result<bool, String> {
    let (left, op, right) = split_compare_expr(rendered)
        .ok_or_else(|| "无非比较符（== != >= <= > <）或比较侧为空".to_string())?;
    Ok(compare_str(op, left, right))
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
        // 实例配置 token 表（参数化批）：schema 缺省值打底 +
        // 开关文件运行值覆盖。装载期渲染 `{{config.<key>}}`——开关文件在
        // 缓存戳里（template_stamp），config.set 写盘即重扫热生效。
        let cfg = config_table(&manifest, switches.get(&slug));
        let declared: std::collections::BTreeSet<String> =
            manifest.config_schema.keys().cloned().collect();
        for template in &manifest.flow.templates {
            if let Err(reason) = validate_template(template) {
                tracing::warn!(
                    "[FlowEngine] 模板 {}/{} 声明非法跳过: {reason}",
                    slug,
                    template.id
                );
                continue;
            }
            let template = match render_config_tokens(template, &cfg, &declared) {
                Ok(t) => t,
                Err(reason) => {
                    tracing::warn!(
                        "[FlowEngine] 模板 {}/{} config token 渲染失败跳过: {reason}",
                        slug,
                        template.id
                    );
                    continue;
                }
            };
            out.push(FlowTemplateDef {
                slug: slug.clone(),
                template,
            });
        }
    }
    out
}

/// 实例配置 token 表（`config.<key>` → 值）：manifest config-schema 缺省值
/// 打底 + 开关文件运行值覆盖（set 侧已按声明键白名单校验，此处宽松读取）。
fn config_table(
    manifest: &PluginManifest,
    sw: Option<&nemesis_config::plugin_wasm::WasmSwitch>,
) -> BTreeMap<String, String> {
    let mut out = manifest.config_schema_defaults();
    if let Some(sw) = sw {
        for (k, v) in &sw.config {
            out.insert(
                k.clone(),
                match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                },
            );
        }
    }
    out
}

/// 装载期 config token 宽和渲染：声明面里 `{{config.<key>}}` 换实例配置值
/// （缺省/运行）。**已声明但未配置**（schema 无缺省且开关文件无覆盖）= 空串
/// ——诚实语义「声明说键存在，未配置 = 空」：proxy / gerrit_base_url 这类
/// 可选键（按 code_host 二选一使用）靠它免于整模板被残留检查拒绝。**未声明**
/// 键保持字面量 = Err（诚实跳过模板，不把字面量垃圾送给 connector）。
/// 其余 token（`{{key}}`/`{{arg.*}}`/`{{origin_meta.*}}` 等条目级）原样保留
/// 由下游逐条目渲染。`step.when` 同相参与装载期 config 渲染（config 值对
/// 实例静态，双源步骤按 code_host 裁剪靠它）；`steps.<id>.<field>` 投影
/// 仍归运行期 [`substitute_tokens`]。
fn render_config_tokens(
    template: &FlowTemplate,
    cfg: &BTreeMap<String, String>,
    declared: &std::collections::BTreeSet<String>,
) -> Result<FlowTemplate, String> {
    let lookup = |key: &str| -> Option<String> {
        let name = key.strip_prefix("config.")?;
        match cfg.get(name) {
            Some(v) => Some(v.clone()),
            None if declared.contains(name) => Some(String::new()),
            None => None,
        }
    };
    let mut t = template.clone();
    for step in &mut t.steps {
        for v in step.args.values_mut() {
            *v = render_tokens(v, lookup);
        }
        if !step.when.is_empty() {
            step.when = render_tokens(&step.when, lookup);
        }
    }
    t.origin_type = render_tokens(&t.origin_type, lookup);
    t.origin_id = render_tokens(&t.origin_id, lookup);
    for v in t.origin_meta.values_mut() {
        *v = render_tokens(v, lookup);
    }
    for v in t.on_transition.args.values_mut() {
        *v = render_tokens(v, lookup);
    }
    // 未声明配置键残留检查（渲染后仍带 `{{config.` = 键没进表——声明坏了
    // 必须响，不能等 connector 拿到模板字面量才在远端炸出莫名 404）。
    let mut leftover: Vec<String> = Vec::new();
    leftover.extend(config_token_names(&t.origin_type));
    leftover.extend(config_token_names(&t.origin_id));
    for s in t.origin_meta.values() {
        leftover.extend(config_token_names(s));
    }
    for s in t.on_transition.args.values() {
        leftover.extend(config_token_names(s));
    }
    for s in &t.steps {
        leftover.extend(config_token_names(&s.when));
        for v in s.args.values() {
            leftover.extend(config_token_names(v));
        }
    }
    if !leftover.is_empty() {
        leftover.sort();
        leftover.dedup();
        return Err(format!(
            "声明含未声明的 {{config.*}} 键（config-schema 无此键——先补声明或改引用）：{}",
            leftover.join(", ")
        ));
    }
    Ok(t)
}

/// 提取字符串里所有未渲染的 `{{config.<键>}}` 键名（报错自带现场）。
fn config_token_names(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(pos) = rest.find("{{") {
        let after = &rest[pos + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                if let Some(key) = name.strip_prefix("config.") {
                    out.push(key.to_string());
                }
                rest = &after[end + 2..];
            }
            None => break,
        }
    }
    out
}

/// 模板结构校验：取数入口双形态（connector_fetch 唯一首步，或首步合法
/// plugin 引用 = plugin fetch）；step kind 值域。
fn validate_template(t: &FlowTemplate) -> Result<(), String> {
    if t.id.trim().is_empty() {
        return Err("模板 id 为空".to_string());
    }
    if t.steps.is_empty() {
        return Err("模板无步骤".to_string());
    }
    // fetch 专管放宽（自持样板批）：首步 = connector_fetch（旧
    // 形态）或合法 plugin:<slug>:<op> 引用（guest 专管取数——自持插件自连
    // 外部系统）。plugin fetch 模板禁止再声明 connector_fetch（取数入口
    // 唯一，不可混用）；plugin 引用在后续位置仍是普通步（下方循环既有
    // continue 分支不动）。
    let first_is_plugin_fetch =
        nemesis_plugins_wasm::manifest::parse_plugin_step_ref(&t.steps[0].kind).is_some();
    let fetch_count = t.steps.iter().filter(|s| s.kind == KIND_FETCH).count();
    if first_is_plugin_fetch {
        if fetch_count != 0 {
            return Err(
                "plugin fetch 模板禁止 connector_fetch 步（取数入口唯一——首步 \
                 plugin:<slug>:<op> 即 guest 专管取数，与 connector_fetch 不可混用）"
                    .to_string(),
            );
        }
        // plugin fetch 的身份 token 面 = {{config.*}}（装载期渲染）+
        // {{key}}（物化期注入）；{{connector}}/{{arg.*}} 是 connector_fetch
        // 专管 token——plugin fetch 下 {{connector}} 渲染为空串（身份静默
        // 错位）、{{arg.*}} 未知 token 逐条目 Err（静默零实例）。两类陷阱
        // 装载期 loud 拒，不给运行期留黑洞。
        for (field, val) in [
            ("origin_type", t.origin_type.as_str()),
            ("origin_id", t.origin_id.as_str()),
        ] {
            if val.contains("{{connector}}") || val.contains("{{arg.") {
                return Err(format!(
                    "plugin fetch 模板 {field} 含 connector_fetch 专管 token \
                    （{{{{connector}}}}/{{{{arg.*}}}}）；只可用 {{{{config.*}}}} 与 {{{{key}}}}"
                ));
            }
        }
        for (k, v) in &t.origin_meta {
            if v.contains("{{connector}}") || v.contains("{{arg.") {
                return Err(format!(
                    "plugin fetch 模板 origin_meta.{k} 含 connector_fetch 专管 token \
                    （{{{{connector}}}}/{{{{arg.*}}}}）；只可用 {{{{config.*}}}} 与 {{{{key}}}}"
                ));
            }
        }
    } else {
        if fetch_count != 1 {
            return Err(format!(
                "模板必须恰好一个 {KIND_FETCH} 步（当前 {fetch_count}）"
            ));
        }
        if t.steps[0].kind != KIND_FETCH {
            return Err(format!("模板首步必须是 {KIND_FETCH}"));
        }
    }
    for s in &t.steps {
        // plugin 步骤（guest step， 批）：`plugin:<slug>:<op>` 引用
        // 形态合法即过声明面校验（slug/op 词表与安装期对账在运行期路由闸——
        // 未装载/越权 op 诚实失败走 on_fail，不在模板校验期猜注册表）。
        if nemesis_plugins_wasm::manifest::parse_plugin_step_ref(&s.kind).is_some() {
            continue;
        }
        if !matches!(
            s.kind.as_str(),
            KIND_FETCH
                | KIND_LAND
                | KIND_FIXLOOP
                | KIND_REGRESS
                | KIND_PUBLISH
                | KIND_REPORT
                | KIND_REPO_SYNC
        ) {
            return Err(format!("未知 step kind: {}", s.kind));
        }
        if s.kind == KIND_PUBLISH {
            for req in [
                "create_op",
                "create_args",
                "result_ref",
                "reconcile_op",
                "reconcile_match",
            ] {
                if s.args.get(req).map(|v| v.trim().is_empty()).unwrap_or(true) {
                    return Err(format!("publish 步缺 {req} 声明（域知识只在模板声明面）"));
                }
            }
        }
        if s.kind == KIND_REPO_SYNC {
            for req in ["project", "url"] {
                if s.args.get(req).map(|v| v.trim().is_empty()).unwrap_or(true) {
                    return Err(format!(
                        "repo_sync 步缺 {req}（项目目录=注册表 directory、远端 clone URL 必填）"
                    ));
                }
            }
        }
        if s.kind == KIND_REPORT {
            let connector = s.args.get("connector").map(|v| v.trim()).unwrap_or("");
            if connector.starts_with("plugin:") {
                // plugin 回帖引用：形态校验（op 段即引用组成，无需另声明 op）。
                if nemesis_plugins_wasm::manifest::parse_plugin_step_ref(connector).is_none() {
                    return Err(format!(
                        "report 步 connector plugin 引用非法: {connector}（形态 \
                         plugin:<slug>:<op>，段 [a-z0-9_-]+）"
                    ));
                }
            } else if !connector.is_empty()
                && s.args
                    .get("op")
                    .map(|v| v.trim().is_empty())
                    .unwrap_or(true)
            {
                return Err("report 步带 connector 必须声明 op（宿主无内置回帖协议）".to_string());
            }
        }
    }
    // 外部身份绑定必声明（域剥离批 C-1）：导入去重/回写订阅全靠它——
    // 缺声明 = 宿主猜身份 = 禁止。
    if t.origin_id.trim().is_empty() {
        return Err("模板缺 origin_id 声明（外部身份映射必声明——宿主不猜）".to_string());
    }
    // on_transition 回写 op（自持写通道批）：connector 原语 op
    // 自由文本（远端裁决）；`plugin:` 引用形态必须过 parse_plugin_step_ref
    //（非法引用 = 声明坏了 loud 拒——装载期诚实跳过模板，不给运行期游标
    // 留永久重试黑洞）。
    if t.on_transition.op.trim().starts_with("plugin:")
        && nemesis_plugins_wasm::manifest::parse_plugin_step_ref(&t.on_transition.op).is_none()
    {
        return Err(format!(
            "on_transition op plugin 引用非法: {}（形态 plugin:<slug>:<op>，段 [a-z0-9_-]+）",
            t.on_transition.op
        ));
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
/// C-1 导入语义：key/title 支持 `$.` JSON path 取字段；`body_field` 落
/// inst.body（land 步 AC 提取 + 描述富化）；`filter` 单比较式过滤（畸形
/// = Err 响——模板契约问题在取数期就响；逐条 false = 不入选）；`labels`
/// 逗号分隔任一命中；外部身份按模板声明逐条目解析（解析失败 = warn +
/// 跳过该条目，诚实不静默）。
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
    let body_field = fetch_step
        .args
        .get("body_field")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let filter = fetch_step
        .args
        .get("filter")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(expr) = &filter {
        // 声明畸形在取数期响（不是逐条吞掉——模板契约问题一次说清）。
        if split_compare_expr(expr).is_none() {
            return Err(format!("模板 {} filter 声明畸形: {expr}", def.template.id));
        }
    }
    let labels_want: Vec<&str> = fetch_step
        .args
        .get("labels")
        .map(|s| {
            s.split(',')
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut created = 0usize;
    for entry in entries {
        let Some(key) = entry_scalar(entry, &key_field).filter(|k| !k.trim().is_empty()) else {
            continue; // 缺键/空键条目：诚实跳过（不炸整批）
        };
        if let Some(expr) = &filter
            && !eval_filter(entry, expr)
        {
            continue;
        }
        if !labels_any_match(entry, &labels_want) {
            continue;
        }
        let instance = nemesis_utils::sanitize::sanitize_path_segment(key.trim());
        if flows.exists(&def.template.id, &instance) {
            continue;
        }
        let title = entry_scalar(entry, &title_field).unwrap_or_else(|| key.clone());
        let body = body_field
            .as_deref()
            .and_then(|f| entry_scalar(entry, f))
            .unwrap_or_default();
        // 外部身份（C-1）：模板声明逐条目渲染；失败 = warn + 跳过该条目
        // （身份错了建单去重就废——响得诚实）。
        let (origin_type, origin_id, origin_meta) =
            match resolve_instance_origin(&def.template, fetch_step, key.trim()) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "[FlowEngine] {}/{} 条目 {} 身份解析失败跳过: {e}",
                        def.slug,
                        def.template.id,
                        key
                    );
                    continue;
                }
            };
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
            origin_type,
            origin_id,
            origin_meta,
            body,
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
