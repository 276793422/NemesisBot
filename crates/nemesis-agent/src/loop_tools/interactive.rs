// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// 结构化提问工具。两形态：
/// - **选项形态**（legacy，`options`）：单选 radio / `multi=true` 多选
///   checkbox；单选 >6 项前端自动收纳为下拉 select（上限 12）。
/// - **表单形态**（`fields`）：多个 text 输入框 + select 下拉字段组成的
///   表单卡；回答以 object（键=字段 key）回灌。`fields` 非空时生效，
///   `options`/`multi` 被忽略。
/// - **凭据形态**（B4，2026-11-08 批次④）：`type: "secret"` 字段——密码
///   输入，值**不回灌上下文**，直写 vault 会话作用域别名
///   `agent/<会话>/<key>`，回灌掩码占位 + `{{vault:<别名>}}` 使用指引；
///   后续任意工具参数以占位符引用（展开/闸面见
///   `loop::session_secrets`）。
///
/// 回答以选中项原文 / 表单作答回灌，模型据此继续；超时回灌 "user did not
/// answer, proceed with your best judgment"（不是错误——轮次照常推进）。
/// broker 经槽注入（gateway 装配 `WebQuestionBroker`）；进程内 id 自增
/// 避免并发提问撞号。
pub struct QuestionTool {
    broker: QuestionBrokerSlot,
}

impl QuestionTool {
    pub fn new(broker: QuestionBrokerSlot) -> Self {
        Self { broker }
    }
}

///  共用：提问卡 ID 发号器（question 工具与 doom-loop 审批卡同源，
/// 保证 Dashboard 上 ID 全局唯一）。
pub(crate) fn next_question_id() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!("q-{}", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
}

/// 解析并校验表单字段定义（工具入参 `fields` 数组 → `QuestionField`）。
/// 校验：数量 ≤[`QUESTION_MAX_FIELDS`]、元素全为 object、key/label 非空且
/// key 一次提问内唯一、type ∈ {text, select, secret}、select 的 options
/// 2-6 项非空、required 缺省 false。
fn parse_question_fields(
    v: &serde_json::Value,
) -> Result<Vec<nemesis_types::agent::QuestionField>, String> {
    let arr = v["fields"]
        .as_array()
        .ok_or_else(|| "'fields' must be an array of field objects".to_string())?;
    if arr.len() > QUESTION_MAX_FIELDS {
        return Err(format!(
            "'fields' must have at most {} entries, got {}",
            QUESTION_MAX_FIELDS,
            arr.len()
        ));
    }
    let mut out = Vec::with_capacity(arr.len());
    let mut seen = std::collections::HashSet::new();
    for (i, f) in arr.iter().enumerate() {
        let obj = f
            .as_object()
            .ok_or_else(|| format!("fields[{}] must be an object", i))?;
        let key = obj
            .get("key")
            .and_then(|k| k.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("fields[{}].key is required (non-empty string)", i))?;
        if !seen.insert(key.to_string()) {
            return Err(format!(
                "fields[{}].key '{}' duplicates an earlier key",
                i, key
            ));
        }
        let label = obj
            .get("label")
            .and_then(|l| l.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("fields[{}] ({}) label is required", i, key))?;
        let field_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("text");
        let options: Vec<String> = match obj.get("options") {
            Some(o) => o
                .as_array()
                .ok_or_else(|| {
                    format!(
                        "fields[{}] ({}) options must be an array of strings",
                        i, key
                    )
                })?
                .iter()
                .map(|o| {
                    o.as_str()
                        .map(|s| s.trim().to_string())
                        .ok_or_else(|| format!("fields[{}] ({}) options must be strings", i, key))
                })
                .collect::<Result<Vec<String>, String>>()?,
            None => Vec::new(),
        };
        match field_type {
            "text" => {}
            "secret" => {
                // B4 凭据形态：无选项；required 语义与 text 相同。
                if !options.is_empty() {
                    return Err(format!(
                        "fields[{}] ({}) is secret: 'options' is not applicable",
                        i, key
                    ));
                }
            }
            "select" => {
                if options.len() < 2 || options.len() > 6 {
                    return Err(format!(
                        "fields[{}] ({}) is select: options must have 2-6 entries, got {}",
                        i,
                        key,
                        options.len()
                    ));
                }
                if options.iter().any(String::is_empty) {
                    return Err(format!(
                        "fields[{}] ({}) select options must be non-empty",
                        i, key
                    ));
                }
            }
            other => {
                return Err(format!(
                    "fields[{}] ({}) has unknown type '{}' (text|select|secret)",
                    i, key, other
                ));
            }
        }
        out.push(nemesis_types::agent::QuestionField {
            key: key.to_string(),
            label: label.to_string(),
            field_type: field_type.to_string(),
            required: obj
                .get("required")
                .and_then(|r| r.as_bool())
                .unwrap_or(false),
            options,
        });
    }
    Ok(out)
}

#[async_trait]
impl Tool for QuestionTool {
    fn description(&self) -> String {
        "Ask the user a structured question and block until they answer (or it times out). \
         Two forms: (a) options — the user picks from fixed candidates (single-choice by \
         default; multi=true for several; >6 options render as a dropdown); (b) fields — \
         a small form of text inputs and select dropdowns when you need several pieces \
         of structured input at once. Returns the selected option text(s) or the form \
         answers as a JSON object; on timeout returns a note to proceed with your best \
         judgment. secret-type fields collect credentials: the value is stored into the \
         session-scoped credential vault and NEVER echoed into the conversation — later \
         reference it in any tool argument with {{vault:agent/<session>/<key>}}. Use when \
         the answer materially changes what you do next. Do NOT use for trivial choices \
         or free-form conversation."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask (clear and self-contained)"
                },
                "options": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Options form: 2-12 candidate answers (single-choice; \
                                    >6 renders as a dropdown). Multi-choice caps at 6. \
                                    Ignored when fields is given."
                },
                "multi": {
                    "type": "boolean",
                    "default": false,
                    "description": "Options form: true = user may select multiple options. \
                                    Ignored when fields is given."
                },
                "fields": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string", "description": "Answer key (unique, non-empty)"},
                            "label": {"type": "string", "description": "Display label"},
                            "type": {"type": "string", "enum": ["text", "select", "secret"],
                                     "description": "text = single-line input; select = pick one of options; \
                                     secret = masked credential input, stored to the session credential \
                                     vault (never echoed) — reference later via {{vault:<alias>}}"},
                            "required": {"type": "boolean", "default": false,
                                         "description": "true = user must fill this field"},
                            "options": {"type": "array", "items": {"type": "string"},
                                        "description": "select fields only: 2-6 candidates"}
                        },
                        "required": ["key", "label"]
                    },
                    "description": "Form form: 1-8 fields (text inputs, selects, secret \
                                    credential inputs). Takes precedence over options/multi."
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let question = v["question"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing required 'question' field".to_string())?;

        // 两形态分流：fields 非空数组 = 表单形态（options/multi 忽略）；
        // 否则 = 选项形态（options 必填）。
        let fields = match v.get("fields") {
            Some(f) if !f.is_null() => parse_question_fields(&v)?,
            _ => Vec::new(),
        };
        let (options, multi) = if fields.is_empty() {
            let options: Vec<String> = v["options"]
                .as_array()
                .ok_or_else(|| {
                    "missing required options: give 'options' (2-12 entries) or 'fields' (form)"
                        .to_string()
                })?
                .iter()
                .map(|o| o.as_str().map(|s| s.trim().to_string()))
                .collect::<Option<Vec<String>>>()
                .ok_or_else(|| "'options' must be an array of strings".to_string())?;
            let multi = v["multi"].as_bool().unwrap_or(false);
            let max = if multi {
                6
            } else {
                QUESTION_MAX_SINGLE_OPTIONS
            };
            if options.len() < 2 || options.len() > max {
                return Err(format!(
                    "'options' must have 2-{} entries (multi-choice caps at 6), got {}",
                    max,
                    options.len()
                ));
            }
            if options.iter().any(String::is_empty) {
                return Err("'options' entries must be non-empty".to_string());
            }
            (options, multi)
        } else {
            (Vec::new(), false)
        };
        // B4 凭据形态：先记下哪些字段是 secret（fields 随 req move 走）。
        let secret_keys: Vec<String> = fields
            .iter()
            .filter(|f| f.field_type == "secret")
            .map(|f| f.key.clone())
            .collect();

        let broker = {
            let guard = self.broker.read();
            match guard.as_ref() {
                Some(b) => b.clone(),
                None => {
                    return Err(
                        "question tool unavailable: no interactive session wired".to_string()
                    );
                }
            }
        };

        let req = nemesis_types::agent::QuestionRequest {
            question_id: next_question_id(),
            question: question.to_string(),
            options,
            multi,
            fields,
            chat_id: context.chat_id.clone(),
            session_key: context.session_key.clone(),
            timeout_secs: QUESTION_DEFAULT_TIMEOUT_SECS,
            bypass_auto_approve: false,
        };

        // ask 是同步阻塞（最长 120s）——spawn_blocking 出 worker 线程，
        // broker 内部再按上下文决定 block_in_place/直等（同审批先例）。
        let outcome = tokio::task::spawn_blocking(move || broker.ask(req))
            .await
            .map_err(|e| format!("question task failed: {}", e))??;

        match outcome {
            nemesis_types::agent::QuestionOutcome::Answered(selected) => {
                if selected.len() == 1 {
                    Ok(format!("User selected: {}", selected[0]))
                } else {
                    Ok(format!(
                        "User selected {} option(s): {}",
                        selected.len(),
                        selected.join("; ")
                    ))
                }
            }
            nemesis_types::agent::QuestionOutcome::AnsweredForm(values) => {
                // 紧凑 JSON object 回灌（键=字段 key）；空 map 理论不可达
                // （至少一个必填或可选字段作答），兜底诚实注记。
                if values.is_empty() {
                    return Ok("User submitted the form with all fields empty.".to_string());
                }
                // B4（2026-11-08 批次④）凭据形态：secret 字段值不回灌
                // 上下文——直写 vault 会话作用域别名（agent/<会话>/<key>），
                // 回灌掩码占位 + `{{vault:<别名>}}` 使用指引。写失败 fail
                // loud（工具 Err），凭据永不落历史/session_log。
                let scope =
                    crate::r#loop::session_secrets::session_scope_prefix(&context.session_key);
                let mut masked = serde_json::Map::new();
                let mut stored: Vec<(String, String)> = Vec::new(); // (key, bare alias)
                for (k, val) in values.iter() {
                    if secret_keys.iter().any(|sk| sk == k) {
                        if val.is_empty() {
                            masked.insert(
                                k.clone(),
                                serde_json::Value::String(format!("[凭据未填写: {k}]")),
                            );
                        } else {
                            let alias = format!("{scope}{k}");
                            nemesis_config::write_vault_secret(&alias, val).map_err(|e| {
                                format!("凭据入库失败（{k}）：{e}——凭据未落库，请重新索要")
                            })?;
                            masked.insert(
                                k.clone(),
                                serde_json::Value::String(
                                    crate::r#loop::session_secrets::received_text(k),
                                ),
                            );
                            stored.push((k.clone(), alias));
                        }
                    } else {
                        masked.insert(
                            k.clone(),
                            serde_json::Value::String(val.clone()),
                        );
                    }
                }
                let obj: serde_json::Value = masked.into();
                let mut reply = format!(
                    "User answered form: {}",
                    serde_json::to_string(&obj).unwrap_or_else(|_| "{}".to_string())
                );
                if !stored.is_empty() {
                    reply.push_str("\n\n以上凭据已存入凭据库（会话作用域），不会再次展示；\
                        后续任意工具参数中以 {{vault:<别名>}} 占位符引用，值不经对话上下文：");
                    for (k, alias) in &stored {
                        reply.push_str(&format!("\n- {{{{vault:{alias}}}}}（{k}）"));
                    }
                }
                Ok(reply)
            }
            nemesis_types::agent::QuestionOutcome::Timeout => {
                Ok("No answer: user did not answer, proceed with your best judgment.".to_string())
            }
            // 循环干预阶梯：急停触发，broker 把 pending 提问
            // 强制了结——按「停止」语义回灌，不鼓励模型继续。
            nemesis_types::agent::QuestionOutcome::Cancelled => {
                Ok("No answer: emergency stop was triggered while waiting; do not retry this question, stop and report.".to_string())
            }
        }
    }
}

// ===========================================================================
// present_files — turn 交付物登记
// ===========================================================================

/// 交付物流水桶类型别名（AgentLoop 与 PresentFilesTool 共享同一 Arc；
/// drain 语义与 `turn_file_changes` 同构——assistant 回复落盘时取走即清）。
pub type TurnDeliverablesBucket =
    Arc<parking_lot::Mutex<HashMap<String, Vec<crate::chat_log::PresentedFile>>>>;

/// 交付物登记工具。agent 认为本 turn 的用户**应该
/// 去看**的产物文件（报告/图/导出物）在此声明——与 write_file「改了什么」
/// （file_changes）分工：这是「该看什么」的结果面指引。登记项随
/// assistant 最终回复落盘为 chat_log 的 `deliverables` 字段，Dashboard 在
/// 消息气泡下渲染交付物卡片。纯登记：不复制/不改文件，只做存在性验真
/// （全部存在才登记——all-or-nothing，诚实报缺失清单）。
pub struct PresentFilesTool {
    workspace: String,
    bucket: TurnDeliverablesBucket,
}

impl PresentFilesTool {
    pub fn new(workspace: String, bucket: TurnDeliverablesBucket) -> Self {
        Self { workspace, bucket }
    }
}

#[async_trait]
impl Tool for PresentFilesTool {
    fn description(&self) -> String {
        "Register deliverables — files the user should look at as the outcome of this turn \
         (report, chart, exported artifact). Purely declarative: the files must already \
         exist; nothing is copied or modified. Registered files are shown as deliverable \
         cards under your final reply. Call once at the end of a turn with the complete \
         list. Paths are workspace-relative (or absolute). Use ONLY for results worth the \
         user's attention — not for every intermediate file."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "File path (workspace-relative or absolute); must exist"
                            },
                            "title": {
                                "type": "string",
                                "description": "Display title (default: file name)"
                            },
                            "note": {
                                "type": "string",
                                "description": "One-line description (optional)"
                            }
                        },
                        "required": ["path"]
                    },
                    "description": "Deliverable files of this turn (1+ entries)"
                }
            },
            "required": ["files"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;
        let files_v = v
            .get("files")
            .and_then(|f| f.as_array())
            .ok_or("missing required 'files' field (array of {path,title,note})")?;
        if files_v.is_empty() {
            return Err("'files' must contain at least one entry".to_string());
        }

        // 逐条解析（path 必填非空；title 缺省取文件名；note 可缺省）。
        let mut parsed: Vec<crate::chat_log::PresentedFile> = Vec::with_capacity(files_v.len());
        for (i, f) in files_v.iter().enumerate() {
            let path = f
                .get("path")
                .and_then(|p| p.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("files[{}]: missing required 'path' field", i))?;
            let title = f
                .get("title")
                .and_then(|t| t.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string())
                });
            let note = f
                .get("note")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            parsed.push(crate::chat_log::PresentedFile {
                path: path.to_string(),
                title,
                note,
            });
        }

        // 存在性验真：all-or-nothing（任一缺失 = 整批拒绝，缺失清单诚实回灌
        // ——半登记会让交付物卡出现幽灵条目）。相对路径按 workspace 解析，
        // 绝对路径放行（与 media 点名路径同语义）。
        let ws = std::path::Path::new(&self.workspace);
        let mut missing: Vec<String> = Vec::new();
        for f in &parsed {
            let p = std::path::Path::new(&f.path);
            let resolved = if p.is_absolute() {
                p.to_path_buf()
            } else {
                ws.join(p)
            };
            if !resolved.is_file() {
                missing.push(f.path.clone());
            }
        }
        if !missing.is_empty() {
            return Err(format!(
                "file(s) do not exist (nothing registered): {}",
                missing.join(", ")
            ));
        }

        // 会话内去重后入桶（drain 侧还有一道全局 dedup；这里先收口让结果
        // 文本与桶内实态一致）。
        let deduped = crate::chat_log::dedup_presented_files(parsed);
        let n = deduped.len();
        self.bucket
            .lock()
            .entry(context.session_key.clone())
            .or_default()
            .extend(deduped);

        Ok(format!("Registered {} deliverable(s) for this turn.", n))
    }
}

// ===========================================================================
// present_plan — plan 模式审批语义（内部专项）
// ===========================================================================

/// present_plan 审批等待窗口（秒）。计划需要阅读时间，比 question 的 120s
/// 长；超时按驳回处理（结束本轮，仍处 plan 模式）——无人值守时绝不放行。
pub const PLAN_PRESENT_TIMEOUT_SECS: u64 = 600;

/// present_plan 审批选项原文（与 Dashboard 卡片渲染、测试断言共用）。
pub const PLAN_PRESENT_OPTION_APPROVE: &str = "批准：切换到 build 模式并按计划执行";
pub const PLAN_PRESENT_OPTION_REJECT: &str = "驳回：结束本轮（先修改计划）";

/// present_plan 的审批结论（工具写入共享 verdict 槽，run_loop 批次尾消费）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanPresentVerdict {
    /// 用户批准 → 工具已把 mode 翻到 Build，本轮照常继续执行计划。
    Approved,
    /// 用户驳回 → run_loop 消费后结束本轮（不空转，仍处 plan 模式）。
    Rejected,
    /// 审批超时 → 与驳回同语义（结束本轮，不放行）。
    Timeout,
}

/// present_plan 工具与 run_loop 之间的共享接线。agent_factory 构造，
/// 同一 Arc 双侧注入：工具侧（SharedToolConfig.plan_present）做 mode 翻转 +
/// verdict 写入；loop 侧（`set_plan_present_handle`）在批次尾按 session_key
/// 取走本会话的 verdict 决定是否终止本轮。`verdicts` 按 session_key 分键、
/// `remove` 取走即清——loop 内跨会话 turn 并发（admission「gate 内联 +
/// turn spawn」），单槽会让 A 会话的裁决被 B 会话覆盖/领走；同会话 turn
/// 由 busy 闸串行，会话键就是正确的隔离粒度。
pub struct PlanPresentHandle {
    /// 提问 broker（与 question 工具同源槽——present_plan 的审批卡就是
    /// 一张单选 question 卡，复用同一 WebQuestionBroker 装配）。
    pub broker: QuestionBrokerSlot,
    /// loop 的 mode 槽（AgentLoop.mode 同一 Arc——批准侧直接翻转）。
    pub mode: Arc<parking_lot::RwLock<crate::types::AgentMode>>,
    /// ModeChanged 广播（徽标刷新；与 set_mode_with_event 同一通道形态）。
    pub agent_event_tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,
    /// 审批结论槽（按 session_key 分键；工具写 / run_loop 批次尾 remove）。
    pub verdicts: Arc<parking_lot::Mutex<std::collections::HashMap<String, PlanPresentVerdict>>>,
}

impl PlanPresentHandle {
    /// 批准臂：mode 翻 Build + ModeChanged 广播（幂等，事件照发——与
    /// `AgentLoop::set_mode_with_event` 同语义；loop 侧方法拿不到本句柄，
    /// 翻转逻辑在此内联，`*mode.write()` 单点写入无竞态面）。
    fn flip_to_build(&self, session_key: &str, chat_id: &str) {
        *self.mode.write() = crate::types::AgentMode::Build;
        if let Some(tx) = self.agent_event_tx.as_ref() {
            let _ = tx.send(nemesis_types::agent::AgentEvent::ModeChanged {
                session_key: session_key.to_string(),
                chat_id: chat_id.to_string(),
                mode: crate::types::AgentMode::Build.as_str().to_string(),
            });
        }
    }
}

/// 计划提交审批工具。plan 模式下把写好的计划交给用户裁决：
/// 批准 → mode 翻到 Build，工具结果告知模型按计划执行（本轮照常继续）；
/// 驳回/超时 → verdict 槽落 Rejected/Timeout，run_loop 批次尾消费后
/// **结构化终止本轮**（模型拿不到下一轮——不空转不靠提示词自觉）。
/// 仅 plan 模式可调用（build 模式诚实报错；供给闸同时隐藏 build 侧定义，
/// 双闸同 hidden_tools 形态）。broker 未接线（headless 变体误注册）=
/// 诚实报错不猜默认放行。
pub struct PlanPresentTool {
    handle: Arc<PlanPresentHandle>,
}

impl PlanPresentTool {
    pub fn new(handle: Arc<PlanPresentHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl Tool for PlanPresentTool {
    fn description(&self) -> String {
        "Submit your plan for user approval while in plan mode. Call this AFTER you have \
         finished researching and drafted the plan (optionally saved to plans/ via write_file). \
         The user either approves (mode switches to build; execute the plan next) or rejects \
         (this turn ends immediately — revise the plan and present again when asked). Do NOT \
         call with a half-formed plan; do NOT continue calling other tools after a rejection."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "plan": {
                    "type": "string",
                    "description": "The full plan text (markdown) for the user to review"
                },
                "title": {
                    "type": "string",
                    "description": "Short plan title (optional, shown as the card heading)"
                }
            },
            "required": ["plan"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        // build 模式诚实拒绝（供给闸已隐藏定义，这里是 dispatch 直呼的兜底）。
        if *self.handle.mode.read() == crate::types::AgentMode::Build {
            return Err("already in build mode: no plan approval needed, just execute".to_string());
        }
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;
        let plan = v["plan"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing required 'plan' field".to_string())?;
        let title = v["title"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("执行计划审批");

        let broker = {
            let guard = self.handle.broker.read();
            match guard.as_ref() {
                Some(b) => b.clone(),
                None => {
                    return Err(
                        "present_plan unavailable: no interactive session wired".to_string()
                    );
                }
            }
        };

        let req = nemesis_types::agent::QuestionRequest {
            question_id: next_question_id(),
            question: format!("{title}\n\n{plan}"),
            options: vec![
                PLAN_PRESENT_OPTION_APPROVE.to_string(),
                PLAN_PRESENT_OPTION_REJECT.to_string(),
            ],
            multi: false,
            fields: Vec::new(),
            chat_id: context.chat_id.clone(),
            session_key: context.session_key.clone(),
            timeout_secs: PLAN_PRESENT_TIMEOUT_SECS,
            bypass_auto_approve: false,
        };

        // ask 是同步阻塞（最长 600s）——spawn_blocking 出 worker 线程
        // （question 工具同款形态）。
        let outcome = tokio::task::spawn_blocking(move || broker.ask(req))
            .await
            .map_err(|e| format!("present_plan task failed: {}", e))??;

        match outcome {
            nemesis_types::agent::QuestionOutcome::Answered(selected)
                if selected.first().map(String::as_str) == Some(PLAN_PRESENT_OPTION_APPROVE) =>
            {
                self.handle
                    .flip_to_build(&context.session_key, &context.chat_id);
                self.handle
                    .verdicts
                    .lock()
                    .insert(context.session_key.clone(), PlanPresentVerdict::Approved);
                Ok("✅ 用户已批准计划：已切换到 build 模式。现在按计划开始执行。".to_string())
            }
            nemesis_types::agent::QuestionOutcome::Answered(_) => {
                self.handle
                    .verdicts
                    .lock()
                    .insert(context.session_key.clone(), PlanPresentVerdict::Rejected);
                // 本文本模型本轮看不到（run_loop 消费 verdict 后直接终局），
                // 落在这里是给 history/审计留一致记录。
                Ok("❌ 用户驳回了计划：本轮到此结束。".to_string())
            }
            // present_plan 只发选项卡（无 fields）——表单作答形态不可达，
            // 防御臂按拒绝收口（语义最接近：非「批准」即不进 build）。
            nemesis_types::agent::QuestionOutcome::AnsweredForm(_) => {
                self.handle
                    .verdicts
                    .lock()
                    .insert(context.session_key.clone(), PlanPresentVerdict::Rejected);
                Ok("❌ 用户驳回了计划：本轮到此结束。".to_string())
            }
            nemesis_types::agent::QuestionOutcome::Timeout => {
                self.handle
                    .verdicts
                    .lock()
                    .insert(context.session_key.clone(), PlanPresentVerdict::Timeout);
                Ok("⏱ 计划审批超时未获响应：本轮到此结束（仍处 plan 模式）。".to_string())
            }
            // 循环干预阶梯：急停强制了结——与超时同收口
            // （留在 plan 模式、本轮终局），不进 build 不继续执行。
            nemesis_types::agent::QuestionOutcome::Cancelled => {
                self.handle
                    .verdicts
                    .lock()
                    .insert(context.session_key.clone(), PlanPresentVerdict::Timeout);
                Ok("⏹ 急停触发，计划审批中断：本轮到此结束（仍处 plan 模式）。".to_string())
            }
        }
    }
}

// ===========================================================================
// Coding tools (grep / git)
// ===========================================================================
