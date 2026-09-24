//! 纪律闭环（2026-09-24 HOOK 三合一收口件4 —— 钩子系统第一个**内部策略
//! 消费者**）。Rust 原生模块，不经 hooks.json 方言（声明闸/证伪预算是
//! 内建策略，不是用户可编排的脚本）。
//!
//! # 语义（计划 §6.1）
//!
//! **参与开关**（D3）：任务描述含 `[discipline:bugfix]` marker（拆解 prompt
//! 对 bug 类子单追加/用户手加）或聊天命令 `/discipline on|off`（交互会话 =
//! 会话键态）。总开关 `agents.discipline.enabled` 默认 **false**（D5 灰度
//! ——关 = 整个子系统不注册不生效）。
//!
//! **声明闸**（内部 ToolHook，首位注册）：参与中 + 工具 ∈
//! {write_file, edit_file, append_file, delete_file} 且路径不在
//! `.discipline/**` 且无有效声明 → `HookDecision::Block`（文案即纪律提示：
//! 六字段 schema）。`.discipline/**` 豁免——否则鸡生蛋（首次写声明本身
//! 被闸）。
//!
//! **证伪执行**（内部 LifecycleHook）：参与中 + 有声明 + 证伪未跑或上次
//! 未过 → on_turn_end 先跑 `falsification_cmd`（走既有 ExecTool 管线：
//! 工作区边界/超时/管道收尸全适用）→ 结果落 `.discipline/falsification-{n}.json`
//! （run/命令/成败/输出节选）并作为 feedback 注入（`TurnEndDecision::Continue`）。
//! 通过后后续轮次不再续（条件「未跑或上次未过」转假）。
//!
//! **预算双帽（诚实注记）**：自有 [`MAX_FALSIFICATION_RUNS`]（D4：耗尽 =
//! 停车升级不静默——warn + falsification-{n}.json 留盘供看板评审注记；
//! 交互会话的失败反馈早已逐轮可见 = 交还用户）+ 循环侧
//! [`crate::hooks::MAX_TURN_END_CONTINUES`]（本钩子**首位注册**，纪律续轮
//! 计入共享帽——全局轮数有界，代价是与 Stop 方言续轮互相挤占）。
//!
//! **逃生门（全路径留痕）**：`/discipline off [理由]`（带理由时追加
//! `.discipline/waive-audit.jsonl`）；estop 触发 → fail-open 整体停用；
//! 证伪命令执行失败 → 失败事实回灌模型（升级而非绕行——预算照扣）。
//!
//! # 覆盖面诚实边界（计划 §6.2 原文）
//!
//! - v1 闸面 = 四文件变更工具；**exec 不闸**（读写分类不可靠，诚实不硬拦
//!   ——模型可用 exec 绕过闸写文件，本闸防的是「顺手改」不是对抗）。
//! - 闸门保证「声明在不在、证伪跑没跑、过没过」，保证不了「实验设计好
//!   不好」——抗糊弄二道闸是看板评审（组件 5，board_review 注入，件5）。
//! - 声明质量不判；实验意义性交评审。
//! - 集群 worker 免费获得（同 loop 代码，marker 在任务描述即可）。
//! - spawn 任务 fresh exec 天然防陈旧声明（每任务新 loop 新状态；交互
//!   会话 = 会话键态，随会话存续）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;

use crate::context::RequestContext;
use crate::hooks::{
    HookDecision, HookPrompt, HookToolCall, LifecycleHook, PromptDecision, ToolHook,
    TurnEndDecision,
};
use crate::r#loop::Tool;

/// 任务描述 marker（D3）：出现即该会话进入纪律参与态。
pub const DISCIPLINE_MARKER: &str = "[discipline:bugfix]";

/// 声明目录（workspace 相对）。
pub const DISCIPLINE_DIR: &str = ".discipline";

/// 声明规范文件（`.discipline/` 下）。
pub const DECLARATION_FILE: &str = "declaration.json";

/// 自有证伪预算（D4）：每会话最多跑 2 次证伪；耗尽 = 停车升级不静默。
pub const MAX_FALSIFICATION_RUNS: u32 = 2;

/// 单次证伪超时（秒）——走 ExecTool 的 timeout 参数（上限 600，取 120：
/// 证伪是快速回归不是全量构建）。
const FALSIFICATION_TIMEOUT_SECS: u64 = 120;

/// falsification-{n}.json 里的输出节选上限（字符）。
const FALSIFICATION_OUTPUT_EXCERPT: usize = 2000;

/// 闸面工具集（计划 §6.2 v1）：四文件变更工具。名字与
/// `config_watch.rs::PLAN_MODE_WRITE_TOOLS` 的文件子集一致（注册名真相源
/// 在 registry 构建）。
const GATED_TOOLS: &[&str] = &["write_file", "edit_file", "append_file", "delete_file"];

// ---------------------------------------------------------------------------
// 声明规范文件
// ---------------------------------------------------------------------------

/// `.discipline/declaration.json` 六字段声明（v2 弃新工具改规范文件）。
/// 全部非空字符串即有效；**声明质量不判**（交评审，组件 5）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    /// 根因（file:line 级定位）。
    pub root_cause: String,
    /// 真相源（哪个事实/文档/代码说这是根因）。
    pub truth_source: String,
    /// 修复不得破坏的不变量。
    pub invariant: String,
    /// 影响面（改这里会动到什么）。
    pub impact: String,
    /// 单变量承诺（本次只改这一个变量）。
    pub single_variable: String,
    /// 证伪命令（可执行验证，exit 0 = 假设未被证伪）。
    pub falsification_cmd: String,
}

impl Declaration {
    /// 从 workspace 读声明。缺失/坏 JSON/字段缺失/字段空 → Err（理由进
    /// 闸拦截文案）。
    pub fn from_workspace(workspace: &Path) -> Result<Self, String> {
        let path = workspace.join(DISCIPLINE_DIR).join(DECLARATION_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|_| format!("声明缺失：{DISCIPLINE_DIR}/{DECLARATION_FILE} 不存在"))?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("声明不是合法 JSON：{e}"))?;
        let field = |name: &str| -> Result<String, String> {
            let s = v
                .get(name)
                .and_then(|x| x.as_str())
                .map(str::trim)
                .unwrap_or("");
            if s.is_empty() {
                return Err(format!("声明字段 `{name}` 缺失或为空"));
            }
            Ok(s.to_string())
        };
        Ok(Self {
            root_cause: field("root_cause")?,
            truth_source: field("truth_source")?,
            invariant: field("invariant")?,
            impact: field("impact")?,
            single_variable: field("single_variable")?,
            falsification_cmd: field("falsification_cmd")?,
        })
    }

    /// 六字段 schema 文案（闸拦截文案 + 文档共用）。
    pub fn schema_text() -> String {
        "```json\n\
         {\"root_cause\": \"根因（file:line）\", \"truth_source\": \"真相源\", \
         \"invariant\": \"不得破坏的不变量\", \"impact\": \"影响面\", \
         \"single_variable\": \"本次唯一变量\", \"falsification_cmd\": \"证伪命令\"}\n\
         ```"
        .to_string()
    }
}

// ---------------------------------------------------------------------------
// 共享态
// ---------------------------------------------------------------------------

/// 单会话证伪进度。
#[derive(Debug, Clone, Copy, Default)]
struct FalsificationProgress {
    runs: u32,
    last_passed: bool,
}

/// 纪律闭环共享态（每 loop 一份，工厂构造注入）。锁全部短临界区（无
/// await 持锁），钩子/gate 各自从 loop 槽借 Arc。
pub struct DisciplineState {
    /// `agents.discipline.enabled`（D5 总开关）。false = 全链路惰性。
    enabled: bool,
    /// workspace 根（声明/证伪产物/审计落盘基准）。
    workspace: PathBuf,
    /// 文件工具工作区边界开关（跟 `agents.defaults.restrict_to_workspace`，
    /// 证伪 exec 同款语义）。
    restrict_exec: bool,
    /// 参与会话集（marker 或 `/discipline on` 进入）。
    participating: Mutex<HashSet<String>>,
    /// 每会话证伪进度（次数 + 上次是否通过）。
    progress: Mutex<HashMap<String, FalsificationProgress>>,
    /// estop 句柄（set_estop 装配；触发 = fail-open 整体停用）。
    estop: RwLock<Option<Arc<crate::estop::EstopState>>>,
}

impl DisciplineState {
    pub fn new(enabled: bool, workspace: PathBuf, restrict_exec: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled,
            workspace,
            restrict_exec,
            participating: Mutex::new(HashSet::new()),
            progress: Mutex::new(HashMap::new()),
            estop: RwLock::new(None),
        })
    }

    /// estop 句柄注入（工厂在 set_estop 同点装配）。
    pub fn set_estop(&self, estop: Arc<crate::estop::EstopState>) {
        *self.estop.write().expect("discipline estop lock") = Some(estop);
    }

    /// estop 触发？（未装配 = 不停用——裸构建测试路径 fail-open 基线。）
    fn estop_engaged(&self) -> bool {
        self.estop
            .read()
            .expect("discipline estop lock")
            .as_ref()
            .map(|e| e.is_engaged())
            .unwrap_or(false)
    }

    /// 该会话是否纪律参与中（总开关 + 参与 + 非 estop 三条件）。
    pub fn active(&self, session_key: &str) -> bool {
        self.enabled
            && !self.estop_engaged()
            && self
                .participating
                .lock()
                .expect("discipline participating lock")
                .contains(session_key)
    }

    /// workspace 根（钩子构造证伪 exec 用）。
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// 证伪 exec 的工作区边界开关。
    pub fn restrict_exec(&self) -> bool {
        self.restrict_exec
    }

    /// marker 参与检测：prompt 含 marker 即加入（on_user_prompt 收口——
    /// 交互/spawn/detached 全部 prompt 都过 lifecycle on_user_prompt，
    /// spawn 侧零改动）。
    pub fn note_participation_from_prompt(&self, session_key: &str, prompt: &str) {
        if self.enabled && prompt.contains(DISCIPLINE_MARKER) {
            self.participating
                .lock()
                .expect("discipline participating lock")
                .insert(session_key.to_string());
        }
    }

    /// `/discipline on`。返回进入前是否**已**在参与态（幂等；HashSet::insert
    /// 的 bool 是「新插入」，这里取反成「已存在」语义）。
    pub fn set_interactive(&self, session_key: &str) -> bool {
        !self
            .participating
            .lock()
            .expect("discipline participating lock")
            .insert(session_key.to_string())
    }

    /// `/discipline off [理由]`。带理由 = waive 入审计（escape hatch 留痕）。
    pub fn clear_interactive(&self, session_key: &str, reason: Option<&str>) {
        self.participating
            .lock()
            .expect("discipline participating lock")
            .remove(session_key);
        if let Some(why) = reason {
            if why.trim().is_empty() {
                return;
            }
            self.append_waive_audit(session_key, why.trim());
        }
    }

    /// 参与会话数（测试检视）。
    pub fn participating_len(&self) -> usize {
        self.participating
            .lock()
            .expect("discipline participating lock")
            .len()
    }

    /// waive 审计（best-effort：审计盘写失败不影响停用本身，warn 留痕）。
    fn append_waive_audit(&self, session_key: &str, reason: &str) {
        let dir = self.workspace.join(DISCIPLINE_DIR);
        let entry = serde_json::json!({
            "ts": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            "session": session_key,
            "reason": reason,
        });
        let line = format!("{entry}\n");
        let file = dir.join("waive-audit.jsonl");
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file)?;
            f.write_all(line.as_bytes())
        };
        if let Err(e) = write() {
            tracing::warn!("[discipline] waive audit append failed: {e}");
        }
    }

    /// 证伪进度快照。
    fn progress_of(&self, session_key: &str) -> FalsificationProgress {
        *self
            .progress
            .lock()
            .expect("discipline progress lock")
            .get(session_key)
            .unwrap_or(&FalsificationProgress::default())
    }

    /// 记一次证伪结果。
    fn record_progress(&self, session_key: &str, passed: bool) {
        let mut map = self.progress.lock().expect("discipline progress lock");
        let p = map.entry(session_key.to_string()).or_default();
        p.runs += 1;
        p.last_passed = passed;
    }

    /// 证伪结果落盘 `.discipline/falsification-{n}.json`（best-effort：
    /// 盘写失败只 warn——结果已进 feedback，留盘是给评审的注记不是唯一载体）。
    fn write_falsification_record(
        &self,
        session_key: &str,
        run: u32,
        command: &str,
        passed: bool,
        output: &str,
    ) {
        let dir = self.workspace.join(DISCIPLINE_DIR);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("[discipline] create {DISCIPLINE_DIR} failed: {e}");
            return;
        }
        let excerpt: String = output.chars().take(FALSIFICATION_OUTPUT_EXCERPT).collect();
        let record = serde_json::json!({
            "run": run,
            "session": session_key,
            "command": command,
            "passed": passed,
            "output_excerpt": excerpt,
        });
        let pretty = serde_json::to_string_pretty(&record).unwrap_or_else(|_| "{}".to_string());
        let path = dir.join(format!("falsification-{run}.json"));
        if let Err(e) = std::fs::write(&path, pretty) {
            tracing::warn!("[discipline] falsification record write failed: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// 钩子本体
// ---------------------------------------------------------------------------

/// 声明闸（内部 ToolHook）：参与中 + 四文件工具 + 非 `.discipline/**` +
/// 无有效声明 → Block。**首位注册**（先于方言桥；metrics/事件观察者本就
/// 不拦，实际策略钩子中恒第一）。
pub struct DisciplineGateHook {
    state: Arc<DisciplineState>,
}

impl DisciplineGateHook {
    pub fn new(state: Arc<DisciplineState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl ToolHook for DisciplineGateHook {
    fn name(&self) -> String {
        "discipline-gate".to_string()
    }

    async fn pre_tool_use(&self, call: &HookToolCall) -> HookDecision {
        // 非闸面工具直通（首判零开销——非参与会话的大部分分发在此返回）。
        if !GATED_TOOLS.contains(&call.name.as_str()) {
            return HookDecision::Allow;
        }
        if !self.state.active(&call.session_key) {
            return HookDecision::Allow;
        }
        // `.discipline/**` 豁免（写声明/证伪产物本身不被闸——鸡生蛋）。
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&call.arguments)
            && let Some(p) = v.get("path").and_then(|x| x.as_str())
            && is_discipline_path(p)
        {
            return HookDecision::Allow;
        }
        match Declaration::from_workspace(self.state.workspace()) {
            Ok(_) => HookDecision::Allow,
            Err(why) => HookDecision::Block {
                reason: gate_block_reason(&why),
            },
        }
    }
}

/// 证伪钩子（内部 LifecycleHook）：on_user_prompt 收 marker 参与；on_turn_end
/// 跑证伪（未跑或上次未过 → Continue 注入结果；通过/预算耗尽 → Stop）。
pub struct DisciplineFalsificationHook {
    state: Arc<DisciplineState>,
}

impl DisciplineFalsificationHook {
    pub fn new(state: Arc<DisciplineState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl LifecycleHook for DisciplineFalsificationHook {
    fn name(&self) -> String {
        "discipline-falsification".to_string()
    }

    async fn on_user_prompt(&self, prompt: &HookPrompt) -> PromptDecision {
        self.state
            .note_participation_from_prompt(&prompt.session_key, &prompt.prompt);
        PromptDecision::Allow
    }

    async fn on_turn_end(&self, end: &crate::hooks::HookTurnEnd) -> TurnEndDecision {
        if !self.state.active(&end.session_key) {
            return TurnEndDecision::Stop;
        }
        // 无有效声明 = 闸之下没有文件变更可证伪（写声明目录豁免，声明本身
        // 不需要证伪）→ 放行收尾。
        let decl = match Declaration::from_workspace(self.state.workspace()) {
            Ok(d) => d,
            Err(_) => return TurnEndDecision::Stop,
        };
        let prog = self.state.progress_of(&end.session_key);
        // 上次已通过 → 条件「未跑或上次未过」转假 → **安静放行**（先于
        // 预算检查：run=2 才通过的会话此后每次终答不再误报预算耗尽——
        // 收口当日复核修复，warn 只留给真正耗尽未过的形态）。
        if prog.last_passed {
            return TurnEndDecision::Stop;
        }
        // 预算耗尽（D4 停车升级不静默）：warn + 产物留盘（看板评审注记/
        // 交互反馈逐轮可见），放行收尾——绝不无限续轮。
        if prog.runs >= MAX_FALSIFICATION_RUNS {
            tracing::warn!(
                "[discipline] falsification budget exhausted ({} runs, last_passed={}) — escalating: session {}",
                prog.runs,
                prog.last_passed,
                end.session_key
            );
            return TurnEndDecision::Stop;
        }

        let run = prog.runs + 1;
        let passed = self
            .run_falsification(&end.session_key, run, &decl.falsification_cmd)
            .await;
        self.state.record_progress(&end.session_key, passed);
        let remaining = MAX_FALSIFICATION_RUNS - run;
        let feedback = if passed {
            format!(
                "✅ [纪律] 证伪通过（第 {run}/{MAX_FALSIFICATION_RUNS} 次）：`{}` exit 0——假设未被证伪。\
                 请核对 `.discipline/falsification-{run}.json` 并收尾：下一条消息将正常结束，不再续轮。",
                decl.falsification_cmd
            )
        } else {
            format!(
                "❌ [纪律] 证伪失败（第 {run}/{MAX_FALSIFICATION_RUNS} 次）：`{}` 非零退出——假设被证伪。\
                 请依据输出修正修复或声明（输出节选已落 `.discipline/falsification-{run}.json`）。\
                 剩余证伪预算 {remaining} 次；耗尽即停车升级（看板注记/交还用户）。",
                decl.falsification_cmd
            )
        };
        TurnEndDecision::Continue { feedback }
    }
}

impl DisciplineFalsificationHook {
    /// 跑一次证伪命令（走既有 ExecTool 管线：工作区边界/超时/管道收尸全
    /// 适用；**不经 loop 分发**——hook 无 registry 句柄，security 限额类
    /// 分发闸不适用，诚实注记）。返回是否通过（exit 0）。
    async fn run_falsification(&self, session_key: &str, run: u32, command: &str) -> bool {
        let exec = crate::loop_tools::ExecTool::new(
            &self.state.workspace().to_string_lossy(),
            self.state.restrict_exec(),
        );
        let args = serde_json::json!({
            "command": command,
            "timeout": FALSIFICATION_TIMEOUT_SECS,
        })
        .to_string();
        let ctx = RequestContext {
            channel: "discipline".to_string(),
            chat_id: session_key.to_string(),
            user: "discipline-hook".to_string(),
            session_key: session_key.to_string(),
            correlation_id: None,
            async_callback: None,
            tool_path_base: None,
        };
        let outcome = exec.execute(&args, &ctx).await;
        // 成败判定走 loop_tools 单一真相源 [`crate::loop_tools::exec_output_passed`]
        // （B2 协议：非零退出 `Ok("Exit code: N…")` 与超时收尸
        // `Ok("Command timed out …")` 皆失败形态；Err 仅进程无法启动——
        // 同败。超时误判通过的缺陷收口当日复核修复）。
        let passed = match &outcome {
            Ok(output) => crate::loop_tools::exec_output_passed(output),
            Err(_) => false,
        };
        let text = match outcome {
            Ok(output) => output,
            Err(err) => err,
        };
        self.state
            .write_falsification_record(session_key, run, command, passed, &text);
        passed
    }
}

// ---------------------------------------------------------------------------
// 路径豁免 + 闸文案
// ---------------------------------------------------------------------------

/// 路径是否落在 `.discipline/**`（任一路径组件命中即豁免——保守方向：
/// 宁多豁免不误闸声明写入；绝对/相对/正反斜杠统一走组件比较）。
pub fn is_discipline_path(path: &str) -> bool {
    Path::new(path)
        .components()
        .any(|c| c.as_os_str() == DISCIPLINE_DIR)
}

/// 闸拦截文案（⛔ 与 security/HOOK 同风格；带六字段 schema 修复指引）。
fn gate_block_reason(why: &str) -> String {
    format!(
        "⛔ 纪律闸：本会话处于 bug-fix 纪律模式（任务含 {DISCIPLINE_MARKER} 或 /discipline on），\
         修改文件前必须先写声明。\n{why}\n\n\
         请用 write_file 写入 `{DISCIPLINE_DIR}/{DECLARATION_FILE}`（六字段全非空）：\n{}\n\n\
         （写声明与证伪产物不受本闸限制；证伪由收尾时自动执行。）",
        Declaration::schema_text()
    )
}

// ---------------------------------------------------------------------------
// AgentLoop 装配（槽 + 访问器；field/init 在 loop.rs 根，仅三行）
// ---------------------------------------------------------------------------

impl crate::r#loop::AgentLoop {
    /// 注入纪律共享态（工厂在 enabled=true 时调用；None = 子系统整体惰性）。
    pub fn set_discipline(&self, state: Arc<DisciplineState>) {
        *self.discipline.write() = Some(state);
    }

    /// 借出纪律态（gate 臂用；None = 未启用）。
    pub(crate) fn discipline_state(&self) -> Option<Arc<DisciplineState>> {
        self.discipline.read().clone()
    }
}

#[cfg(test)]
mod tests;
