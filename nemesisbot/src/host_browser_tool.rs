//! host_browser 工具（浏览器接管框架 v1，2026-11-15）——bot 在用户机器
//! 浏览器里看页面 + 操作页面的 agent 消费点（与 VM 档 `browser` 工具并存，
//! 命名空间无冲突）。
//!
//! 职责形态（计划 §5）：单动作十一种（navigate/click/type/key/scroll/
//! extract_text/screenshot/wait_for + DOM 只读三动作 inspect/snapshot/
//! get_dom，2026-11-18 计划）+ `run_script` 编排（逐步展开基础动作，不进
//! Driver trait）+ **会话级四动作**（spawn/tabs/close_tab/close_browser，
//! 2026-11-28 所有权批——本工具只做胶水转发，所有权策略全在 nemesis-browser
//! 的 manager/session 层单一真相）。screenshot 的 b64 结果落盘
//! `<workspace>/uploads/browser/`，只回路径不回字节（LLM 上下文塞不进
//! 图像字节——generate_image 同纪律）。
//!
//! 安全形态：
//! - **dispatch 主管线照常**：本工具在注册表里走完整 dispatch 瀑布
//!   （estop/hidden/plan/8 层/hooks/凭据注入）。per-op 风险经
//!   [`Tool::operation_override`] 投影（navigate=network_request、
//!   click/type/key/scroll=network_upload、读类（extract/screenshot/wait +
//!   DOM 只读三动作）=internal；run_script=
//!   process_exec + **强制人工审批**展示完整脚本——D12③）；
//! - **run_script 每步二次过闸**：编排步不经 dispatch（工具内自执行），
//!   每步构造 ToolInvocation 过 SecurityPlugin（override_op_type=步 slug）
//!   ——闸面看到的是别名原文，明文在闸后现解；
//! - **凭据守卫三条件**（type 动作文本槽为 vault 别名时）：③ 防伪造
//!   （重解别名与槽内明文比对）、① origin 域名绑定（凭据域名 ↔ 当前
//!   working tab）、② 密码框元素校验（input[type=password] 严格）。任一
//!   不符 = 拒绝且**不回显明文**。LLM 拿到使用权（何时何地用），拿不到
//!   读取权（说不出密码内容）；
//! - estop 激活 = 全部动作拒绝（工具内检查 + dispatch 检查双保险；
//!   run_script 每步边界再查——编排中途 estop 即停车）。
//!
//! 注册面：`agent_factory/shared.rs::register_tools_and_mcp`
//! （cfg(feature="browser")，manager 在场才注册——诚实缺省）。

use base64::Engine as _;
use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::{OperationOverride, Tool};
use nemesis_browser::creds;
use nemesis_browser::driver::{WebAction, action_op_slug};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use nemesis_agent::estop::EstopState;
use nemesis_agent::r#loop::credential_injection::{CREDENTIAL_BINDINGS_KEY, CredentialBindings};

/// 工具注册名。
pub const TOOL_NAME: &str = "host_browser";

/// run_script 步数上限（编排面诚实上限；超过拒收——防单调用长事务霸占
/// 会话 operating 位）。
pub const MAX_SCRIPT_STEPS: usize = 32;

/// 前置校验产物（F42，2026-11-16 八审）：畸形脚本零副作用失败——所有
/// 步骤先解析（缺 action / 嵌套 / 参数不合法都在校验期诚实拒绝），循环
/// 内不再有 `?` 裸逃逸点。此前畸形步在编排中途整调 Err，已执行步的回执
/// 全部丢弃，模型拿不到现场只能整脚本重发 = 前序副作用二次执行。
#[derive(Debug)]
struct ScriptStep {
    /// 动作名（回执面）。
    action_name: String,
    /// 已解析动作（act 面不再重解析）。
    action: WebAction,
    /// 原始步骤 JSON（安全闸面看别名原文）。
    raw: serde_json::Value,
}

/// 编排计划：步骤清单 + 失败策略（continue_on_error 从 args 一起收编）。
#[derive(Debug)]
struct ScriptPlan {
    steps: Vec<ScriptStep>,
    continue_on_error: bool,
}

/// 截图产物目录（workspace 相对；落盘后结果只回路径）。
const SCREENSHOT_DIR: &str = "uploads/browser";

/// 编排回执汇总（纯函数面，F48，2026-11-16 十一审）。estop 停车时补
/// `stopped_at`/`stop_reason="estop"`——其余三停车臂（security_denied/
/// credential_rejected/step_failed）都带 stop_reason，estop 臂此前漏标 =
/// 模型面契约不对称（模型只能靠 receipts 里 action:"estop" 猜停车原因）。
/// 正常完成路径两字段缺席（无停车语义）。
fn script_summary(
    total: usize,
    executed: usize,
    receipts: Vec<serde_json::Value>,
    estop_stopped_at: Option<usize>,
) -> serde_json::Value {
    let all_ok = receipts
        .iter()
        .all(|r| r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
    let mut v = serde_json::json!({
        "ok": all_ok,
        "total": total,
        "executed": executed,
        "receipts": receipts,
    });
    if let Some(i) = estop_stopped_at {
        v["stopped_at"] = serde_json::json!(i);
        v["stop_reason"] = serde_json::json!("estop");
    }
    v
}

/// DOM 只读三动作的编排回执摘要投影（2026-11-18 计划 §3.6）：全量结果
/// （snapshot 可达 2000 节点 / get_dom 可达 50K 字符）会把 run_script 回执
/// 撑爆——编排回执投影成小摘要；单动作路径不投影（全量结果是读面价值
/// 本身）。inspect 摘 tag/open（元素识别 + 展开态最常用字段）。
fn dom_action_summary(action: &WebAction, result: &serde_json::Value) -> Option<serde_json::Value> {
    match action {
        WebAction::Inspect { .. } => Some(serde_json::json!({
            "tag": result.get("tag"),
            "open": result.get("open"),
        })),
        WebAction::SnapshotDom { .. } => Some(serde_json::json!({
            "nodes_emitted": result.get("nodes_emitted"),
            "truncated": result.get("truncated"),
        })),
        WebAction::GetDom { .. } => Some(serde_json::json!({
            "chars": result.get("chars"),
            "truncated": result.get("truncated"),
        })),
        _ => None,
    }
}

/// 结果注记合并（纯函数面）：默认挑选落到别人实例时的诚实注记并进结果
/// JSON（对象形态注 `note` 键；非对象结果前缀文本行）。注记是提示面，
/// 永不改写结果本体。
fn attach_note(result: String, note: Option<String>) -> String {
    let Some(note) = note else {
        return result;
    };
    match serde_json::from_str::<serde_json::Value>(&result) {
        Ok(serde_json::Value::Object(mut m)) => {
            m.insert("note".to_string(), serde_json::json!(note));
            serde_json::Value::Object(m).to_string()
        }
        _ => format!("【{note}】\n{result}"),
    }
}

/// 截图序号（进程级单调；毫秒时间戳会撞名——run_script 连续两步
/// screenshot 可在同一毫秒内完成，后者覆盖前者——序号后缀保证唯一，
/// 2026-11-15 复审 F12）。
static SHOT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// host_browser 工具。
pub struct HostBrowserTool {
    manager: Arc<nemesis_browser::BrowserManager>,
    /// workspace 根（截图落盘 `<workspace>/uploads/browser/`）。
    workspace: PathBuf,
    estop: Arc<EstopState>,
    /// run_script 每步安全闸（security feature 关闭时 None——闸缺席诚实
    /// 放行，与全系统「security.enabled=false 同语义一致」；字段双形态
    /// 对齐 SharedResources.security_plugin，构造点零 cfg 分叉）。
    #[cfg(feature = "security")]
    security: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    security: Option<()>,
}

impl HostBrowserTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        manager: Arc<nemesis_browser::BrowserManager>,
        workspace: PathBuf,
        estop: Arc<EstopState>,
        #[cfg(feature = "security")] security: Option<
            Arc<nemesis_security::pipeline::SecurityPlugin>,
        >,
        #[cfg(not(feature = "security"))] security: Option<()>,
    ) -> Self {
        Self {
            manager,
            workspace,
            estop,
            #[cfg(feature = "security")]
            security,
            #[cfg(not(feature = "security"))]
            security,
        }
    }

    /// 会话选择：显式 session_id 优先；缺省 = 所有权偏好挑选（2026-11-28
    /// 会话级四动作批堵洞④：bot 面偏爱自己 spawn 的实例，落到别人实例时
    /// 带诚实注记——调用方应把注记并入结果投影）。人面（None）维持旧行为。
    async fn pick_session(
        &self,
        session_id: Option<&str>,
        caller: Option<&str>,
    ) -> Result<(Arc<nemesis_browser::BrowserSession>, Option<String>), String> {
        if let Some(id) = session_id.map(str::trim).filter(|s| !s.is_empty()) {
            let s = self.manager.session(id).ok_or_else(|| {
                format!("浏览器会话 {id} 不存在（browser.sessions 可查当前会话清单）")
            })?;
            return Ok((s, None));
        }
        self.manager.pick_default_session_for(caller).await
    }

    /// 凭据守卫三条件（type 动作文本槽为 vault 别名时；任一不符拒绝，
    /// 错误串不含明文）。
    ///
    /// * `alias_full`：完整别名值 `vault:browser/<域名>/<name>`；
    /// * `text`：注入层已改写的明文（守卫③比对基准）。
    async fn credential_guards(
        &self,
        session: &nemesis_browser::BrowserSession,
        alias_full: &str,
        selector: &str,
        text: &str,
    ) -> Result<(), String> {
        // 守卫③ 防伪造：重解别名与槽内明文比对一致才放行（无 sidecar 可
        // 伪造——别名→明文映射只在 vault 与本守卫里重算）。
        let resolved = nemesis_config::resolve_vault_reference(alias_full)
            .ok_or("凭据解析器未装配：无法验证凭据引用（守卫③）")?
            .map_err(|_| "凭据引用无效（守卫③）".to_string())?;
        if resolved != text {
            return Err(
                "凭据绑定验证失败：参数明文与凭据库不一致（守卫③）。请原样使用 vault 别名，不要手工替换"
                    .to_string(),
            );
        }
        // 守卫① origin 域名绑定：凭据绑定域名 ↔ 当前 working tab。
        let domain = creds::browser_alias_domain(alias_full)
            .ok_or("凭据别名不是 browser 命名空间（vault:browser/<域名>/<名>）")?;
        let origin = session
            .working_tab_origin()
            .await
            .map_err(|e| format!("读取页面 origin 失败（守卫①前置）: {e}"))?;
        let host = creds::origin_host(&origin);
        if !creds::origin_matches_host(&host, &domain) {
            return Err(format!(
                "凭据域名绑定不符（守卫①）：凭据绑定 {domain}，当前页面 {host}。请先导航到绑定域名页面再打字"
            ));
        }
        // 守卫② 密码框校验：目标必须是 input[type=password]（严格）。
        let is_pw = session
            .working_tab_is_password(selector)
            .await
            .map_err(|e| format!("读取目标元素失败（守卫②前置）: {e}"))?;
        if !is_pw {
            return Err(
                "凭据打字目标不是密码输入框（守卫②：只允许 input[type=password]）。普通文本请用字面量，不要走凭据别名"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// 截图落盘：b64 → `<workspace>/uploads/browser/shot_{millis}.png`，
    /// 结果元数据换路径（不回字节）。
    fn save_screenshot(
        &self,
        session_id: &str,
        result: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let b64 = result
            .get("png_base64")
            .and_then(|v| v.as_str())
            .ok_or("截图结果缺 png_base64 数据")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("截图 b64 解码失败: {e}"))?;
        if bytes.is_empty() {
            return Err("截图字节为空".to_string());
        }
        let dir = self.workspace.join(SCREENSHOT_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建截图目录失败: {e}"))?;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let seq = SHOT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = dir.join(format!("shot_{millis}_{seq}.png"));
        std::fs::write(&path, &bytes).map_err(|e| format!("写入截图文件失败: {e}"))?;
        let mut meta = serde_json::json!({
            "ok": true,
            "path": path.to_string_lossy(),
            "bytes": bytes.len(),
            "session_id": session_id,
        });
        // F32（2026-11-15 六审）：坐标系元数据透传——模型从截图量坐标时
        // 需要知道图与 Input 坐标同空间（css_px）；视口读取失败的降级形态
        // 是 device_px（坐标需按 dpr 换算或改用 selector）。
        for k in ["viewport", "dpr", "coordinate_space"] {
            if let Some(v) = result.get(k) {
                meta[k] = v.clone();
            }
        }
        Ok(meta)
    }

    /// 失败现场截图（run_script 步骤失败时尽力而为；截图自身失败不掩盖
    /// 主错误）。
    async fn failure_screenshot(
        &self,
        session: &nemesis_browser::BrowserSession,
        caller: Option<&str>,
    ) -> Option<String> {
        let shot = session.act(&WebAction::Screenshot, caller).await.ok()?;
        let meta = self.save_screenshot(session.id.as_str(), &shot).ok()?;
        meta.get("path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    /// 单动作执行（type 凭据守卫 + screenshot 落盘 + wait_for 诚实超时）。
    async fn run_single(
        &self,
        session: &Arc<nemesis_browser::BrowserSession>,
        action: WebAction,
        bindings: &CredentialBindings,
        caller: Option<&str>,
    ) -> Result<String, String> {
        // type 动作 + 凭据绑定 → 三守卫（别名取自 sidecar，明文已在槽内）。
        if let WebAction::Type { selector, text } = &action
            && let Some((_, alias)) = bindings.iter().find(|(f, _)| f == "text")
        {
            let alias_full = format!("{}{}", nemesis_config::VAULT_PREFIX, alias);
            self.credential_guards(session, &alias_full, selector, text)
                .await?;
        }

        let result = session.act(&action, caller).await?;

        match &action {
            WebAction::Screenshot => {
                let meta = self.save_screenshot(session.id.as_str(), &result)?;
                Ok(meta.to_string())
            }
            WebAction::WaitFor { selector, .. } => {
                if result.get("found").and_then(|f| f.as_bool()) == Some(true) {
                    Ok(result.to_string())
                } else {
                    Err(format!("等待超时：元素未出现: {selector}"))
                }
            }
            _ => Ok(result.to_string()),
        }
    }

    /// 会话解析 + F40 存活闸（2026-11-16 七审）。默认臂（pick_default_session）
    /// 已在 F28 做过存活扫描；这里补的是显式指定臂的对称缺口（模型传的是
    /// 工具上一轮回执里的 session_id，窗口手关后就死指针了——act 会撞传输
    /// 层错误无从 actionable）。开销一次 loopback GET（毫秒级）。
    async fn pick_alive_session(
        &self,
        session_id: Option<&str>,
        caller: Option<&str>,
    ) -> Result<(Arc<nemesis_browser::BrowserSession>, Option<String>), String> {
        let (session, note) = self.pick_session(session_id, caller).await?;
        if !session.is_alive().await {
            return Err(format!(
                "浏览器会话 {} 已退出（窗口被手关或崩溃）：可用 host_browser 的 spawn 动作重新打开，或请用户在 Dashboard 启动后再操作",
                session.id
            ));
        }
        Ok((session, note))
    }

    /// 脚本级前置校验（F42，2026-11-16 八审）：所有步骤先解析再执行任何
    /// 一步——缺 action / 嵌套 run_script / 参数不合法 / 空 steps / 步数
    /// 超限都在这里诚实拒绝（错误注明「脚本未执行任何步骤」，模型可直接
    /// 修正重发、无需核实页面状态）。校验产物 = 编排计划。
    fn validate_script_plan(args: &serde_json::Value) -> Result<ScriptPlan, String> {
        let steps = args
            .get("steps")
            .and_then(|v| v.as_array())
            .ok_or("run_script 需要 steps 数组")?;
        if steps.is_empty() {
            return Err("run_script 的 steps 为空".to_string());
        }
        if steps.len() > MAX_SCRIPT_STEPS {
            return Err(format!(
                "run_script 步数超限（{} > {MAX_SCRIPT_STEPS}）：请拆分为多次调用",
                steps.len()
            ));
        }
        let mut plan = Vec::with_capacity(steps.len());
        for (i, step_raw) in steps.iter().enumerate() {
            let action_name = step_raw
                .get("action")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("第 {i} 步缺 action 字段（脚本未执行任何步骤）"))?
                .to_string();
            if action_name == "run_script" {
                return Err(format!(
                    "第 {i} 步是 run_script：编排不允许嵌套（步骤只允许基础动作；脚本未执行任何步骤）"
                ));
            }
            // 会话级四动作不进编排（会话簿记不是页面动作；serde 也会拒，
            // 这里给可读文案）。
            if matches!(
                action_name.as_str(),
                "spawn" | "tabs" | "close_tab" | "close_browser"
            ) {
                return Err(format!(
                    "第 {i} 步是 {action_name}：会话级动作不进 run_script 编排，请作为独立工具调用（脚本未执行任何步骤）"
                ));
            }
            let action: WebAction = serde_json::from_value(step_raw.clone()).map_err(|e| {
                format!("第 {i} 步参数不合法（{action_name}）: {e}（脚本未执行任何步骤）")
            })?;
            plan.push(ScriptStep {
                action_name,
                action,
                raw: step_raw.clone(),
            });
        }
        let continue_on_error = args
            .get("continue_on_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Ok(ScriptPlan {
            steps: plan,
            continue_on_error,
        })
    }

    /// run_script 编排：逐步展开基础动作（含 DOM 只读三动作——serde 反序列化
    /// 天然支持，2026-11-18 计划 §3.6）。每步：estop 边界 → 安全闸
    /// （per-op slug）→ 凭据自解析（步骤 text 别名 → 明文 + 三守卫）→
    /// act → 回执。失败停（continue_on_error 缺省 false）+ 失败截图。
    /// 步骤已前置校验（F42）——本函数循环内无 `?` 裸逃逸点，一切失败
    /// 都回执化（已执行步的回执永不丢失）。
    async fn run_script(
        &self,
        session: &Arc<nemesis_browser::BrowserSession>,
        plan: ScriptPlan,
        caller: Option<&str>,
    ) -> Result<String, String> {
        let mut receipts: Vec<serde_json::Value> = Vec::with_capacity(plan.steps.len());
        // F17：executed 只数真执行步（estop 伪回执不进计数——estop 停车时
        // receipts.len() 会把伪回执也算成「已执行」）。
        let mut executed_steps: usize = 0;
        // F48（2026-11-16 十一审）：estop 停车位（汇总补 stopped_at/stop_reason，
        // 与其余三停车臂契约对称）。
        let mut estop_stopped_at: Option<usize> = None;
        for (i, step) in plan.steps.iter().enumerate() {
            // estop 步边界：编排中途急停即停车（已执行步收尾回执）。
            if self.estop.is_engaged() {
                receipts.push(serde_json::json!({
                    "index": i, "action": "estop", "ok": false,
                    "error": "急停激活：脚本中止（此步及后续未执行）",
                }));
                estop_stopped_at = Some(i);
                break;
            }
            executed_steps += 1;

            // 安全闸（per-op slug；闸面看别名原文——明文在闸后现解）。
            // security feature 关闭时整块消失（字段恒 None 无闸——与全系统
            // security 关形态同语义）。
            #[cfg(feature = "security")]
            if let Some(sec) = &self.security {
                let invocation = nemesis_security::ToolInvocation {
                    tool_name: TOOL_NAME.to_string(),
                    args: step.raw.clone(),
                    user: "agent".to_string(),
                    source: "host_browser_script".to_string(),
                    metadata: std::collections::HashMap::from([(
                        "override_op_type".to_string(),
                        action_op_slug(&step.action).to_string(),
                    )]),
                };
                let (allowed, deny) = sec.execute(&invocation);
                if !allowed {
                    let summary = deny
                        .map(|d| d.summary)
                        .unwrap_or_else(|| "安全策略拒绝该步骤".to_string());
                    receipts.push(serde_json::json!({
                        "index": i, "action": step.action_name, "ok": false,
                        "error": format!("安全闸拒绝: {summary}"),
                    }));
                    let shot = self.failure_screenshot(session, caller).await;
                    let stopped = serde_json::json!({
                        "ok": false,
                        "total": plan.steps.len(),
                        "executed": i,
                        "receipts": receipts,
                        "stopped_at": i,
                        "stop_reason": "security_denied",
                        "failure_screenshot": shot,
                    });
                    return Ok(stopped.to_string());
                }
            }

            // 步骤凭据自解析：type 步 text 槽 vault 别名 → 明文 + 三守卫
            //（嵌套 steps 不在 dispatch 声明槽射程，工具内现解现守）。
            // F42：解析/守卫失败不再 `?` 整调 Err（已执行步回执会丢）——
            // 按步回执停车（credential_rejected；不带失败截图——凭据拒绝
            // 与页面状态无关）。
            let mut wa_step = step.action.clone();
            let cred = async {
                if let WebAction::Type { selector, text } = &mut wa_step
                    && text.starts_with(nemesis_config::VAULT_PREFIX)
                {
                    let alias_full = text.clone();
                    let resolved = nemesis_config::resolve_vault_reference(&alias_full)
                        .ok_or("凭据解析器未装配：无法解析脚本步骤凭据（守卫③）")?
                        .map_err(|e| format!("脚本步骤凭据引用无效（守卫③）: {e}"))?;
                    self.credential_guards(session, &alias_full, selector, &resolved)
                        .await?;
                    *text = resolved;
                }
                Ok::<(), String>(())
            }
            .await;
            if let Err(e) = cred {
                receipts.push(serde_json::json!({
                    "index": i, "action": step.action_name, "ok": false,
                    "error": e,
                }));
                let stopped = serde_json::json!({
                    "ok": false,
                    "total": plan.steps.len(),
                    "executed": i,
                    "receipts": receipts,
                    "stopped_at": i,
                    "stop_reason": "credential_rejected",
                });
                return Ok(stopped.to_string());
            }

            let t0 = std::time::Instant::now();
            let outcome = session.act(&wa_step, caller).await;
            let duration_ms = t0.elapsed().as_millis() as u64;
            match outcome {
                Ok(mut result) => {
                    // wait_for 未命中 = 步失败（与单动作语义一致）；
                    // screenshot 落盘换路径（回执不携带字节）。
                    let step_err = match &wa_step {
                        WebAction::WaitFor { selector, .. } => {
                            if result.get("found").and_then(|f| f.as_bool()) == Some(true) {
                                None
                            } else {
                                Some(format!("等待超时：元素未出现: {selector}"))
                            }
                        }
                        WebAction::Screenshot => {
                            match self.save_screenshot(session.id.as_str(), &result) {
                                Ok(meta) => {
                                    result = meta;
                                    None
                                }
                                Err(e) => Some(e),
                            }
                        }
                        _ => None,
                    };
                    match step_err {
                        None => {
                            // DOM 只读三动作回执投影摘要（snapshot/get_dom 全量
                            // 会撑爆编排回执；单动作路径回全量不投影）。
                            let result_out =
                                dom_action_summary(&wa_step, &result).unwrap_or(result);
                            receipts.push(serde_json::json!({
                                "index": i, "action": step.action_name,
                                "ok": true, "duration_ms": duration_ms,
                                "result": result_out,
                            }));
                        }
                        Some(err) => {
                            receipts.push(serde_json::json!({
                                "index": i, "action": step.action_name,
                                "ok": false, "duration_ms": duration_ms,
                                "error": err,
                            }));
                            if !plan.continue_on_error {
                                let shot = self.failure_screenshot(session, caller).await;
                                let stopped = serde_json::json!({
                                    "ok": false,
                                    "total": plan.steps.len(),
                                    "executed": i + 1,
                                    "receipts": receipts,
                                    "stopped_at": i,
                                    "stop_reason": "step_failed",
                                    "failure_screenshot": shot,
                                });
                                return Ok(stopped.to_string());
                            }
                        }
                    }
                }
                Err(e) => {
                    receipts.push(serde_json::json!({
                        "index": i, "action": step.action_name,
                        "ok": false, "duration_ms": duration_ms,
                        "error": e,
                    }));
                    if !plan.continue_on_error {
                        let shot = self.failure_screenshot(session, caller).await;
                        let stopped = serde_json::json!({
                            "ok": false,
                            "total": plan.steps.len(),
                            "executed": i + 1,
                            "receipts": receipts,
                            "stopped_at": i,
                            "stop_reason": "step_failed",
                            "failure_screenshot": shot,
                        });
                        return Ok(stopped.to_string());
                    }
                }
            }
        }

        let executed = executed_steps;
        Ok(script_summary(plan.steps.len(), executed, receipts, estop_stopped_at).to_string())
    }
}

#[async_trait::async_trait]
impl Tool for HostBrowserTool {
    fn description(&self) -> String {
        "Control the user's local browser (real Edge/Chrome window, not headless). \
         Single actions: navigate / click / type / key / scroll / extract_text / \
         screenshot / wait_for; read-only DOM inspection: inspect (element state \
         and geometry), snapshot (bounded DOM tree outline), get_dom (sanitized \
         subtree HTML) — these three accept frame=<URL substring> to read inside \
         iframes; or run_script with a steps array to chain basic actions in \
         order (no nesting). Session lifecycle: spawn opens a browser bound to \
         YOUR session, tabs lists pages (with owners), close_tab closes a page \
         you opened, close_browser closes a session you spawned — the ownership \
         ledger refuses closing anything opened by the user or another session, \
         so clean up only what you opened. Screenshots are saved under the \
         workspace uploads/browser/ directory and returned as a path. For \
         password fields, pass vault:browser/<domain>/<name> as type text — the \
         credential is domain-bound and only accepted on input[type=password] \
         fields of the matching site."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate","click","type","key","scroll","extract_text",
                             "screenshot","wait_for","inspect","snapshot","get_dom",
                             "run_script","spawn","tabs","close_tab","close_browser"],
                    "description": "The browser action to perform. Session lifecycle (spawn/tabs/close_tab/close_browser) is ownership-tracked: only close sessions/tabs YOUR session opened — the ledger refuses closing ones opened by the user or other sessions"
                },
                "session_id": {
                    "type": "string",
                    "description": "Target browser session id (optional; defaults prefer a session your chat spawned, then human-started/unowned ones; as a last resort another chat's running instance is taken over and the result carries a note saying so)"
                },
                "url": {"type": "string", "description": "navigate: absolute URL to open"},
                "selector": {"type": "string", "description": "CSS selector (click/type/key/extract_text/wait_for/inspect/get_dom; for key it focuses the element first; snapshot: optional — defaults to body)"},
                "text": {"type": "string", "description": "type: text to input; use vault:browser/<domain>/<name> for passwords"},
                "frame": {"type": "string", "description": "inspect/snapshot/get_dom only: target iframe by URL substring (e.g. \"admin\"; pick a longer substring to disambiguate — first match wins). Omit or \"main\" for the top page. NOT finding it returns an honest error listing every frame URL tagged [cross-process] (OOPIF) or [same-process]. Read-only: click/type/key do NOT accept frame"},
                "key": {"type": "string", "description": "key: key name, e.g. enter/escape/tab/a/w/s/d or single characters; combine with phase press|down|up. Uppercase single letters (e.g. \"W\") are auto-shift-wrapped (trusted Shift down→key→up) and only work with phase=press. Combo form \"ctrl+shift+a\" (prefixes ctrl/alt/shift) sends modifier physical frames + base frame with modifier bits (page sees ctrlKey/shiftKey correctly); combos only work with phase=press. Modifier state does NOT carry across separate calls — to HOLD an uppercase key, send shift phase=down, then the lowercase letter (phase=down / hold_ms / phase=up), then shift phase=up. Trusted key events need a focused element — pass selector so the element is focused first, otherwise the keystroke is silently dropped (or click the element first)"},
                "phase": {"type": "string", "enum": ["press","down","up"], "description": "key phase (default press); for holding a key longer use press with hold_ms; uppercase letters only support press (auto Shift wrap)"},
                "hold_ms": {"type": "integer", "description": "key press: how long to hold the key down in milliseconds before release (default 60, max 3000) — game hold scenarios"},
                "x": {"type": "integer", "description": "click/scroll: viewport X coordinate in CSS pixels — screenshots are normalized to the same coordinate space (coordinate_space=css_px), so coordinates measured from a screenshot work as-is (click: ignored when selector is given — selector wins)"},
                "y": {"type": "integer", "description": "click/scroll: viewport Y coordinate in CSS pixels — same space as screenshots (click: ignored when selector is given — selector wins)"},
                "dx": {"type": "integer", "description": "scroll: horizontal pixels (positive = right; omit for vertical-only scroll)"},
                "dy": {"type": "integer", "description": "scroll: vertical pixels (positive = down)"},
                "timeout_ms": {"type": "integer", "description": "wait_for: poll timeout in milliseconds (default 10000, max 60000); navigate: load-wait wall clock (default 30000, max 120000)"},
                "max_depth": {"type": "integer", "description": "snapshot: max tree depth (default 12, clamped to [1,30])"},
                "max_nodes": {"type": "integer", "description": "snapshot: max nodes emitted (default 300, clamped to [1,2000]); truncated=true when a bound is hit"},
                "max_chars": {"type": "integer", "description": "get_dom: output character cap (default 12000, clamped to [200,50000]); html is head+tail with a truncation marker, chars = original size"},
                "steps": {
                    "type": "array",
                    "description": "run_script: array of basic action objects executed in order; fails fast unless continue_on_error",
                    "items": {"type": "object"},
                    "maxItems": 32
                },
                "continue_on_error": {"type": "boolean", "description": "run_script: keep executing after a failed step (default false)"},
                "target": {"type": "string", "description": "close_tab: case-insensitive substring matched against target_id/url/title; 0 hits returns the full tab list, >1 hits returns the hit list (never guesses); the last remaining tab is refused — use close_browser for the whole window"},
                "kind": {"type": "string", "description": "spawn: browser kind edge/chrome/brave/vivaldi/chromium (optional; default = configured preference)"},
                "executable": {"type": "string", "description": "spawn: absolute path to browser executable (optional; overrides kind)"}
            },
            "required": ["action"]
        })
    }

    /// per-op 风险投影（dispatch 8 层管线 per-op 分档通道）：
    /// **会话级四动作先于 WebAction 反序列化**（2026-11-28 所有权批 §2.4
    /// 陷阱：它们不是 WebAction 变体，落进下方 from_value 返 None = dispatch
    /// fail-closed CRITICAL）——spawn=network_request（拉起外部进程访问
    /// 网络）、tabs/close_tab/close_browser=internal（会话簿记面）。
    /// navigate=network_request / click·type·key·scroll=network_upload /
    /// extract·screenshot·wait + DOM 只读三动作=internal /
    /// run_script=process_exec + **强制人工审批**（卡片展示完整脚本，跳过
    /// 「总是允许」规则表）。
    ///
    /// BUG-20261128-047（BUG-20261125-050 同族横向补齐）：畸形面（args 非
    /// JSON / action 缺失 / action 非四动作且非 WebAction）此前返回 None →
    /// 静态表无 `host_browser` 条目 → 未知名 fail-closed CRITICAL 误档
    /// （每调用 WARN + guardian 误档烧 token）。本工具不进静态表兜底
    /// （`crates/nemesis-security/src/types.rs` 静态表位点在他线 WIP 工作
    /// 树上，本轮避碰）——兜底投影收在工具侧：畸形面恒投影 NetworkRequest
    /// （宁过勿漏，8 层管线全跑，execute 侧对畸形 args 诚实报错；语义与
    /// 静态表补键等价，mail 静态兜底同款「按最危险分支」）。他线提交
    /// types.rs 后可改为静态表条目 + 删除本兜底（二者行为一致，冗余无害）。
    fn operation_override(&self, args: &str) -> Option<OperationOverride> {
        try_operation_override(args).or_else(|| {
            Some(OperationOverride {
                op_type: "network_request".to_string(),
                force_approval: None,
                receipt_ctx: None,
            })
        })
    }

    /// type 动作文本槽承载 vault 别名（dispatch 注入层改写明文 + sidecar
    /// 绑定回传；守卫③在 execute 侧比对）。
    fn credential_arg_keys(&self) -> &[&str] {
        &["text"]
    }

    /// trusted 输入注入是高危面（坐标点击/键盘派发可驱动页面任意交互），
    /// 恒 big 档——双闸（供给秩比较 + dispatch 拒低档）按 trait 消费。
    /// F20（2026-11-15 三审）：交付文档明文承诺「host_browser 恒 big」
    /// 但 impl 漏写 override——闸在文档里存在、在代码里缺席。
    fn min_tier(&self) -> Option<&str> {
        Some("big")
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let mut parsed: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("args 非 JSON: {e}"))?;
        // sidecar 剥离：dispatch 注入层合入的绑定清单（field/alias 对）。
        // 未知该键的下游本会忽略；本工具消费后即摘除。
        let bindings: CredentialBindings = parsed
            .get(CREDENTIAL_BINDINGS_KEY)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let field = item.get("field")?.as_str()?.to_string();
                        let alias = item.get("alias")?.as_str()?.to_string();
                        Some((field, alias))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(obj) = parsed.as_object_mut() {
            obj.remove(CREDENTIAL_BINDINGS_KEY);
        }

        // estop 全局闸（工具内双保险；dispatch 层还有一道）。
        if self.estop.is_engaged() {
            return Err("⛔ 急停激活中：浏览器操作已冻结（释放急停后重试）".to_string());
        }

        // 所有权主体（2026-11-28 会话级四动作批）：根会话键——spawn 绑定/
        // act 归因/关关闸都用它。空键回落 session_key（RequestContext::root_key）。
        let root = context.root_key().to_string();

        let session_id = parsed
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let action_str = parsed
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or("missing 'action' argument")?
            .to_string();

        // ==== 会话级四动作（2026-11-28 所有权批，计划 §2）====
        // 早臂在 WebAction 反序列化**之前**——四动作不是 WebAction 变体，
        // 落进下方 from_value 会假「参数不合法」。
        match action_str.as_str() {
            // spawn：bot 面启动浏览器（kind/executable 可选覆盖），实例 +
            // 初始 tab 绑定到本会话根键；收尾由本会话 close_browser。
            "spawn" => {
                let kind = parsed.get("kind").and_then(|v| v.as_str());
                let executable = parsed.get("executable").and_then(|v| v.as_str());
                let session = self
                    .manager
                    .spawn(kind, executable, Some(root.as_str()))
                    .await?;
                let mut v = session.summary();
                v["owner"] = serde_json::json!(root);
                v["hint"] = serde_json::json!(
                    "实例与初始标签页已绑定到本会话；用完后由本会话 close_browser 收尾（所有权台账只允许关闭自己启动的实例）"
                );
                return Ok(v.to_string());
            }
            // tabs：页签清单（谁都能读；行带 owner 投影）。缺省挑选走
            // 所有权偏好（偏爱自己 spawn 的实例）。
            "tabs" => {
                let (session, note) = self
                    .pick_alive_session(session_id.as_deref(), Some(root.as_str()))
                    .await?;
                let tabs = session.tabs_snapshot().await?;
                let mut v = serde_json::json!({
                    "session_id": session.id,
                    "working_tab_id": session.working_tab_id(),
                    "tabs": tabs,
                });
                if let Some(n) = note {
                    v["note"] = serde_json::json!(n);
                }
                return Ok(v.to_string());
            }
            // close_tab：关单页——target 子串匹配 + 所有权闸（只关自己
            // 打开的；无主/他人 → 诚实拒绝）在 session 层裁决。
            "close_tab" => {
                let target = parsed
                    .get("target")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or(
                        "close_tab 缺 target：要关闭的标签页用 target 指定（target_id/url/title 的大小写不敏感子串；先用 tabs 动作查看清单）",
                    )?;
                let (session, note) = self
                    .pick_alive_session(session_id.as_deref(), Some(root.as_str()))
                    .await?;
                let result = session.close_tab(target, Some(root.as_str())).await?;
                return Ok(attach_note(result.to_string(), note));
            }
            // close_browser：关实例——双闸（实例启动者 + 实例内全部活 tab
            // 均归本会话）在 manager 层裁决；人开的实例 bot 恒拒（人面
            // Dashboard 关闭按钮兜底）。显式 session_id 优先；缺省 = 所有权
            // 偏好挑选。死会话允许关（清卡动作，与 Dashboard 语义一致）。
            "close_browser" => {
                let explicit = session_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty());
                let (id, note) = match explicit {
                    Some(id) => {
                        let s = self.manager.session(id).ok_or_else(|| {
                            format!("浏览器会话 {id} 不存在（browser.sessions 可查当前会话清单）")
                        })?;
                        (s.id.clone(), None)
                    }
                    None => {
                        let (s, n) = self
                            .manager
                            .pick_default_session_for(Some(root.as_str()))
                            .await?;
                        (s.id.clone(), n)
                    }
                };
                let result = self.manager.close_session(&id, Some(root.as_str())).await?;
                return Ok(attach_note(result.to_string(), note));
            }
            _ => {}
        }

        if action_str == "run_script" {
            // F42：前置校验先于会话解析——畸形脚本零副作用失败，也不需要
            // 活浏览器才能发现脚本写错了。
            let plan = Self::validate_script_plan(&parsed)?;
            let (session, note) = self
                .pick_alive_session(session_id.as_deref(), Some(root.as_str()))
                .await?;
            let result = self.run_script(&session, plan, Some(root.as_str())).await?;
            return Ok(attach_note(result, note));
        }

        // 单动作：同样先解析再解析会话（畸形参数不需要浏览器就能发现）。
        let action: WebAction = serde_json::from_value(parsed)
            .map_err(|e| format!("动作参数不合法（{action_str}）: {e}"))?;
        let (session, note) = self
            .pick_alive_session(session_id.as_deref(), Some(root.as_str()))
            .await?;
        let result = self
            .run_single(&session, action, &bindings, Some(root.as_str()))
            .await?;
        Ok(attach_note(result, note))
    }
}

/// 正常面投影（会话级四动作早臂 / run_script 强审批 / WebAction 逐动作
/// slug）；畸形面（args 非 JSON / action 缺失或非字符串 / 非 WebAction 的
/// 未知名）返回 None（由 trait `operation_override` 的兜底臂接住——
/// BUG-20261128-047）。主体原样自旧 operation_override，拆出只为让兜底
/// 收口在 trait 方法一处。
fn try_operation_override(args: &str) -> Option<OperationOverride> {
    let v: serde_json::Value = serde_json::from_str(args).ok()?;
    let action = v.get("action").and_then(|a| a.as_str())?;
    let session_level = match action {
        "spawn" => "network_request",
        "tabs" | "close_tab" | "close_browser" => "internal",
        _ => "",
    };
    if !session_level.is_empty() {
        return Some(OperationOverride {
            op_type: session_level.to_string(),
            force_approval: None,
            receipt_ctx: None,
        });
    }
    if action == "run_script" {
        let n = v
            .get("steps")
            .and_then(|s| s.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        return Some(OperationOverride {
            op_type: "process_exec".to_string(),
            force_approval: Some(format!("浏览器脚本执行（{n} 步，逐步过安全闸）：{args}")),
            receipt_ctx: None,
        });
    }
    let wa: WebAction = serde_json::from_value(v).ok()?;
    Some(OperationOverride {
        op_type: action_op_slug(&wa).to_string(),
        force_approval: None,
        receipt_ctx: None,
    })
}

#[cfg(test)]
mod tests;
