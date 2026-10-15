//! 行动代理 P0（2026-10-14）：connector 动态工具对——`connector_find`（注册
//! 表发现）+ `connector_invoke`（按 connector.toml 声明执行）。
//!
//! 设计要点（PLAN §3.3「不开任何旁路」）：
//! - `connector_find` 静态声明 Internal（LOW），纯读；
//! - `connector_invoke` **不静态声明**操作类型——per-op 风险经
//!   [`Tool::operation_override`] 投影给 dispatch（metadata 三键），管线层
//!   读键裁决；连接器/op 未知名（解析不出投影）回落静态路径 = 未知名
//!   fail-closed CRITICAL 兜底；
//! - 执行三绑定：`http`（SSRF 钉死 fetch）/ `tool`（经 Weak<AgentLoop>
//!   回灌 [`AgentLoop::handle_tool_call_at_depth`] **完整 dispatch 瀑布**，
//!   同 code_run 桥形态）/ `action`（内置 demo 动作，纸面账本）；
//! - 全 outcome 落统一回执（executed/error/budget_blocked/disabled；
//!   denied_approval 由 dispatch 的 ABAC deny 分支代写——审批拒绝发生在
//!   execute 之前，工具自身写不了，工具侧见 ⛔ 前缀即跳过不双写）；
//! - 预算护栏（限频 + 资金 fail-closed）在 execute 内、dispatch 审批之后
//!   ——先过人工卡再过限额，两道闸各自留痕；
//! - 凭据注入：vault 别名 `connector/<id>/<slot>` 现开现解锁，真值只进
//!   HTTP 请求头/查询串，回执/日志只见别名与 digest。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::{AgentLoop, OperationOverride, Tool};
use nemesis_connectors::budget::{BudgetGuard, Limits};
use nemesis_connectors::manifest::{OpClass, RiskLevel, ValidatedConnector, ValidatedOp};
use nemesis_connectors::receipt::{
    BudgetConsumed, Receipt, append_receipt, args_digest, new_receipt_id,
};
use nemesis_connectors::registry::{find_connector, find_op, search_connectors};

pub const FIND_TOOL_NAME: &str = "connector_find";
pub const INVOKE_TOOL_NAME: &str = "connector_invoke";

#[cfg(test)]
mod tests;

/// 单次 HTTP 响应回灌模型上下文的字节上限。
const HTTP_BODY_CAP: usize = 8 * 1024;
/// 回执 summary 截断长度（人话摘要，存层不截——工具侧截）。
const SUMMARY_CAP: usize = 160;

// ---------------------------------------------------------------------------
// op_type slug 投影（工具侧单一真相源）
// ---------------------------------------------------------------------------

/// op 声明（risk × class）→ 管线 op_type slug。
///
/// 取 `danger(by_class)` 与 `danger(declared risk)` 的**较高者**再映射：
/// class 隐含档 read=LOW / network=MEDIUM / write=HIGH / exec=CRITICAL；
/// 声明档 low=0 / medium=1 / high=2 / critical=3。slug 语义只承担「危险
/// 度」（管线按 slug 反查档位裁决），不承担字面语义——high+read 投影到
/// `file_write` 是机制正确（都 = HIGH 档）。network 在低档位优先投影
/// `network_request`（SSRF/网络中间件按 URL 工作的工具名面惯例）。
pub fn op_slug_for(risk: RiskLevel, class: OpClass) -> &'static str {
    let by_class = match class {
        OpClass::Read => 0u8,
        OpClass::Network => 1,
        OpClass::Write => 2,
        OpClass::Exec => 3,
    };
    match (class, by_class.max(risk.rank())) {
        (OpClass::Network, 0 | 1) => "network_request",
        (_, 0) => "file_read",
        (_, 1) => "registry_read",
        (_, 2) => "file_write",
        (_, _) => "process_exec",
    }
}

/// denied_approval 凭条上下文 JSON 的序列化形态（dispatch 侧
/// `ConnectorReceiptCtx` 反序列化的镜像——两处字段名必须一致）。
#[derive(Debug, serde::Serialize)]
struct ReceiptCtxJson {
    workspace: String,
    connector: String,
    op: String,
    risk: String,
    class: String,
    args_digest: String,
}

// ---------------------------------------------------------------------------
// 共用小件
// ---------------------------------------------------------------------------

/// 读 config 的 connectors 段（每次 execute 现读——config.json 是唯一真相
/// 源；读失败/缺段 = 全默认：enabled=true、限频关、资金限额 0=fail-closed）。
fn connectors_config(config_path: &Path) -> nemesis_config::ConnectorsConfig {
    nemesis_config::load_config(config_path)
        .ok()
        .and_then(|c| c.connectors)
        .unwrap_or_default()
}

fn budget_limits(cfg: &nemesis_config::ConnectorsConfig) -> Limits {
    Limits {
        max_ops_per_hour: cfg.max_ops_per_hour,
        finance_max_order_qty: cfg.finance.as_ref().map(|f| f.max_order_qty).unwrap_or(0.0),
        finance_max_orders_per_day: cfg
            .finance
            .as_ref()
            .map(|f| f.max_orders_per_day)
            .unwrap_or(0),
    }
}

/// 统一回执落盘 + summary 截断（写失败只 warn——回执是增值账目，不反压
/// 工具结果；与 dispatch 代写臂同纪律）。`approval_ref` 来自工具 ctx
/// （dispatch 经 execute_tracked 注入的交互审批引用；直通/规则表 = None
/// ——凭条不伪造背书）。
#[allow(clippy::too_many_arguments)]
fn write_receipt(
    workspace: &Path,
    connector: &str,
    op: &str,
    risk: &str,
    class: &str,
    outcome: &str,
    digest: &str,
    summary: &str,
    receipt_key: Option<&str>,
    receipt_value: Option<&str>,
    budget_consumed: Option<BudgetConsumed>,
    session_key: Option<&str>,
    approval_ref: Option<String>,
) -> Receipt {
    let now = chrono::Utc::now();
    let r = Receipt {
        receipt_id: new_receipt_id(now.timestamp_millis()),
        ts: now.to_rfc3339(),
        connector: connector.to_string(),
        op: op.to_string(),
        risk: risk.to_string(),
        class: class.to_string(),
        outcome: outcome.to_string(),
        args_digest: digest.to_string(),
        summary: summary.chars().take(SUMMARY_CAP).collect(),
        receipt_key: receipt_key.map(|s| s.to_string()),
        receipt_value: receipt_value.map(|s| s.to_string()),
        budget_consumed,
        approval_ref,
        session_key: session_key.map(|s| s.to_string()),
        undoable: false,
    };
    if let Err(e) = append_receipt(workspace, &r) {
        tracing::warn!("[Connector] 回执写入失败: {e}");
    }
    r
}

/// 凭条 IM 文本卡（固定格式，写类/资金类必推——感知层 §3.5）。
fn receipt_card(
    r: &Receipt,
    rate_used: u32,
    rate_limit: u32,
    finance: Option<(u32, u32)>,
) -> String {
    let mut s = format!(
        "🧾 凭条 {}\n动作：{}/{}（{}·{}）\n结果：{}",
        r.receipt_id, r.connector, r.op, r.risk, r.class, r.summary
    );
    if let (Some(k), Some(v)) = (&r.receipt_key, &r.receipt_value) {
        s.push_str(&format!("\n{k}: {v}"));
    }
    s.push_str(&format!("\n额度：本小时 {rate_used}/{rate_limit}"));
    if let Some((orders, day_limit)) = finance {
        s.push_str(&format!(" · 今日资金单 {orders}/{day_limit}"));
    }
    s.push_str("\n凭据可查：行动账本（Dashboard）或 connector receipts");
    s
}

fn honest_err(msg: impl std::fmt::Display) -> String {
    format!("Tool error: {msg}")
}

// ---------------------------------------------------------------------------
// connector_find
// ---------------------------------------------------------------------------

/// 注册表发现工具：`{query?: string}`——空 query 列出全部可用 connector
/// （含 ops 全量元数据 + args_schema），非空按词元匹配。损坏 connector
/// 诚实进 `errors` 数组（不静默）。
pub struct ConnectorFindTool {
    workspace: PathBuf,
}

impl ConnectorFindTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

#[async_trait::async_trait]
impl Tool for ConnectorFindTool {
    fn description(&self) -> String {
        "Search the local connector registry (workspace/connectors/*/connector.toml). \
         Empty query lists ALL available connectors with their ops (risk level, args \
         schema); a query filters by word match. ALWAYS call this first to discover \
         connector ids / op ids / argument schemas before connector_invoke."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Word-match filter (empty = list all connectors)"
                }
            },
            "required": []
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn min_tier(&self) -> Option<&str> {
        Some("normal")
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let query = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|v| {
                v.get("query")
                    .and_then(|q| q.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        let hits = search_connectors(&self.workspace, &query);
        let errors: Vec<String> = nemesis_connectors::registry::scan_connectors(&self.workspace)
            .into_iter()
            .filter_map(|l| l.error.map(|e| format!("{}: {e}", l.dir.display())))
            .collect();
        let connectors: Vec<serde_json::Value> = hits
            .iter()
            .map(|h| {
                serde_json::json!({
                    "id": h.connector.id,
                    "name": h.connector.name,
                    "version": h.connector.version,
                    "description": h.connector.description,
                    "matched_ops": h.matched_ops,
                    "ops": h.connector.ops.iter().map(op_json).collect::<Vec<_>>(),
                })
            })
            .collect();
        let total = connectors.len();
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "total": total,
            "connectors": connectors,
            "errors": errors,
        }))
        .map_err(|e| honest_err(e))?)
    }
}

/// 单 op 元数据 → LLM 可见 JSON（含 args_schema——invoke 入参的权威契约）。
fn op_json(o: &ValidatedOp) -> serde_json::Value {
    let binding = match &o.binding {
        nemesis_connectors::manifest::OpBinding::Http { method, path, .. } => {
            format!("{method:?} {path}")
        }
        nemesis_connectors::manifest::OpBinding::Tool { tool } => format!("tool:{tool}"),
        nemesis_connectors::manifest::OpBinding::Action { action } => format!("action:{action}"),
    };
    serde_json::json!({
        "id": o.id,
        "intent": o.intent,
        "risk": o.risk.as_str(),
        "class": o.class.as_str(),
        "confirm_required": o.confirm_required,
        "finance_budget": o.finance_budget,
        "binding": binding,
        "args_schema": o.args_schema,
    })
}

// ---------------------------------------------------------------------------
// connector_invoke
// ---------------------------------------------------------------------------

/// 执行工具：`{connector, op, args?}`。流程：注册表三态查找 → 预算闸 →
/// 按绑定分派（http/tool/action）→ 全 outcome 落回执 → 写类/资金类推 IM
/// 凭条卡。审批拒绝（⛔ 前缀）不双写——dispatch 的 ABAC deny 分支已凭
/// receipt_ctx 代写 denied_approval。
pub struct ConnectorInvokeTool {
    workspace: PathBuf,
    config_path: PathBuf,
    /// SSRF 闸来源（security 关闭 = None = 无闸直连，与全系统
    /// security.enabled=false 语义一致）。
    #[cfg(feature = "security")]
    security: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    budget: Arc<BudgetGuard>,
    /// tool 绑定回灌句柄（Arc 定型后注入，同 code_run）。
    loop_ref: parking_lot::RwLock<Option<Weak<AgentLoop>>>,
    /// action 账本互斥（load→mutate→save 串行化；磁盘是真相源，无内存缓存）。
    order_lock: parking_lot::Mutex<()>,
}

impl ConnectorInvokeTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace: PathBuf,
        config_path: PathBuf,
        #[cfg(feature = "security")] security: Option<
            Arc<nemesis_security::pipeline::SecurityPlugin>,
        >,
    ) -> Self {
        Self {
            workspace,
            config_path,
            #[cfg(feature = "security")]
            security,
            budget: Arc::new(BudgetGuard::default()),
            loop_ref: parking_lot::RwLock::new(None),
            order_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Arc 定型后注入（agent_factory 调用；Weak 不延长 loop 生命周期）。
    pub fn set_loop(&self, loop_weak: Weak<AgentLoop>) {
        *self.loop_ref.write() = Some(loop_weak);
    }

    /// 推送条件：写/exec/网络资金类执行面动作（读类静默只入账本——§3.5）。
    fn should_push(class: OpClass, finance_budget: bool, outcome: &str) -> bool {
        (class != OpClass::Read || finance_budget)
            && matches!(outcome, "executed" | "error" | "budget_blocked")
    }
}

#[async_trait::async_trait]
impl Tool for ConnectorInvokeTool {
    fn description(&self) -> String {
        "Execute a connector op declared in the local registry (connector.toml). \
         Requires connector id + op id + args object — discover them via \
         connector_find first. Every action gets a numbered receipt (see the \
         Action Ledger); write/finance ops require explicit approval and are \
         subject to budget limits (rate / finance caps, fail-closed)."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connector": {"type": "string", "description": "Connector id (from connector_find)"},
                "op": {"type": "string", "description": "Op id within the connector"},
                "args": {"type": "object", "description": "Op arguments (see the op's args_schema)"}
            },
            "required": ["connector", "op"]
        })
    }

    fn min_tier(&self) -> Option<&str> {
        Some("normal")
    }

    /// per-op 风险投影（dispatch 嗅探 → metadata 三键 → 管线裁决）。解析
    /// 不出（连接器/op 不存在或损坏）= None → 静态路径 fail-closed 兜底。
    fn operation_override(&self, args: &str) -> Option<OperationOverride> {
        let v: serde_json::Value = serde_json::from_str(args).ok()?;
        let connector = v.get("connector")?.as_str()?.trim();
        let op_id = v.get("op")?.as_str()?.trim();
        if connector.is_empty() || op_id.is_empty() {
            return None;
        }
        let c = find_connector(&self.workspace, connector).ok()?;
        let op = find_op(&c, op_id)?;
        let op_args = v.get("args").cloned().unwrap_or(serde_json::json!({}));
        Some(OperationOverride {
            op_type: op_slug_for(op.risk, op.class).to_string(),
            force_approval: op.confirm_required.then(|| {
                format!(
                    "connector `{}` op `{}` 声明强制人工确认（confirm=required）——\
                     执行前必须得到用户明确批准",
                    c.id, op.id
                )
            }),
            receipt_ctx: serde_json::to_string(&ReceiptCtxJson {
                workspace: self.workspace.to_string_lossy().to_string(),
                connector: c.id.clone(),
                op: op.id.clone(),
                risk: op.risk.as_str().to_string(),
                class: op.class.as_str().to_string(),
                args_digest: args_digest(&op_args),
            })
            .ok(),
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| honest_err(format!("args JSON 非法: {e}")))?;
        let connector = v
            .get("connector")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| honest_err("缺少 connector 字段"))?;
        let op_id = v
            .get("op")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| honest_err("缺少 op 字段"))?;
        let op_args = v.get("args").cloned().unwrap_or(serde_json::json!({}));
        if !op_args.is_object() {
            return Err(honest_err("args 必须是对象（见该 op 的 args_schema）"));
        }
        let digest = args_digest(&op_args);

        // 注册表三态：NotFound/Invalid = 无账可记（诚实报错回模型）；
        // Disabled = connector 在册但停用——记 disabled 回执（存在性是账）。
        let c = match find_connector(&self.workspace, connector) {
            Ok(c) => c,
            Err(nemesis_connectors::registry::ConnectorLookupError::Disabled { id }) => {
                write_receipt(
                    &self.workspace,
                    &id,
                    op_id,
                    "low",
                    "read",
                    "disabled",
                    &digest,
                    "connector 已停用（enabled=false），未执行",
                    None,
                    None,
                    None,
                    Some(&context.session_key),
                    context.approval_ref.clone(),
                );
                return Err(honest_err(format!(
                    "connector `{id}` 已停用——如需启用改 connector.toml enabled = true"
                )));
            }
            Err(e) => return Err(honest_err(e)),
        };
        let op = find_op(&c, op_id).ok_or_else(|| {
            honest_err(format!(
                "connector `{}` 无 op `{}`（可用: {}）",
                c.id,
                op_id,
                c.ops
                    .iter()
                    .map(|o| o.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;

        // 预算闸（fail-closed；审批已在 dispatch 过完——先人工后限额）。
        let cfg = connectors_config(&self.config_path);
        let limits = budget_limits(&cfg);
        let finance_qty = op
            .finance_budget
            .then(|| op_args.get("qty").and_then(|q| q.as_f64()))
            .flatten();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let verdict = self.budget.check(&limits, &c.id, now_ms, finance_qty);
        if !verdict.is_allow() {
            let r = write_receipt(
                &self.workspace,
                &c.id,
                &op.id,
                op.risk.as_str(),
                op.class.as_str(),
                "budget_blocked",
                &digest,
                &verdict.denial_message(),
                None,
                None,
                None,
                Some(&context.session_key),
                context.approval_ref.clone(),
            );
            self.maybe_push_card(context, &c, op, &r, &cfg).await;
            return Err(honest_err(format!(
                "⛔ RATE LIMIT: {}",
                verdict.denial_message()
            )));
        }

        // 按绑定分派。⛔ 前缀（dispatch 层拒绝族）= denied_approval 回执
        // 已由 dispatch 代写——工具侧跳过不双写，原文透传。
        let exec = self.dispatch_binding(&c, op, &op_args, context).await;
        match exec {
            Ok(InvocationOutcome::DispatchRejected(raw)) => Ok(raw),
            Ok(InvocationOutcome::Done {
                summary,
                receipt_key,
                receipt_value,
                result_text,
            }) => {
                self.budget.record(&limits, &c.id, now_ms, finance_qty);
                let rate_used = self
                    .budget
                    .rate_used(&c.id, chrono::Utc::now().timestamp_millis());
                let finance_today = op.finance_budget.then(|| {
                    self.budget
                        .finance_today(chrono::Utc::now().timestamp_millis())
                        .0
                });
                let r = write_receipt(
                    &self.workspace,
                    &c.id,
                    &op.id,
                    op.risk.as_str(),
                    op.class.as_str(),
                    "executed",
                    &digest,
                    &summary,
                    receipt_key.as_deref(),
                    receipt_value.as_deref(),
                    Some(BudgetConsumed {
                        ops_last_hour: rate_used,
                        finance_orders_today: finance_today,
                    }),
                    Some(&context.session_key),
                    context.approval_ref.clone(),
                );
                self.maybe_push_card(context, &c, op, &r, &cfg).await;
                Ok(result_text)
            }
            Ok(InvocationOutcome::Failed {
                summary,
                result_text,
            }) => {
                let r = write_receipt(
                    &self.workspace,
                    &c.id,
                    &op.id,
                    op.risk.as_str(),
                    op.class.as_str(),
                    "error",
                    &digest,
                    &summary,
                    None,
                    None,
                    None,
                    Some(&context.session_key),
                    context.approval_ref.clone(),
                );
                self.maybe_push_card(context, &c, op, &r, &cfg).await;
                Err(honest_err(result_text))
            }
            Err(e) => {
                let r = write_receipt(
                    &self.workspace,
                    &c.id,
                    &op.id,
                    op.risk.as_str(),
                    op.class.as_str(),
                    "error",
                    &digest,
                    &e,
                    None,
                    None,
                    None,
                    Some(&context.session_key),
                    context.approval_ref.clone(),
                );
                self.maybe_push_card(context, &c, op, &r, &cfg).await;
                Err(honest_err(e))
            }
        }
    }
}

impl ConnectorInvokeTool {
    /// 写类/资金类动作的 IM 凭条卡推送（bus 模式才有效；standalone 静默）。
    async fn maybe_push_card(
        &self,
        context: &RequestContext,
        c: &ValidatedConnector,
        op: &ValidatedOp,
        r: &Receipt,
        cfg: &nemesis_config::ConnectorsConfig,
    ) {
        if !Self::should_push(op.class, op.finance_budget, &r.outcome) {
            return;
        }
        let Some(loop_arc) = self.loop_ref.read().clone().and_then(|w| w.upgrade()) else {
            return;
        };
        let limits = budget_limits(cfg);
        let rate_used = self
            .budget
            .rate_used(&c.id, chrono::Utc::now().timestamp_millis());
        let finance = op.finance_budget.then(|| {
            let t = self
                .budget
                .finance_today(chrono::Utc::now().timestamp_millis());
            (t.0, limits.finance_max_orders_per_day)
        });
        let card = receipt_card(r, rate_used, limits.max_ops_per_hour, finance);
        loop_arc
            .push_connector_receipt_card(
                &context.channel,
                &context.chat_id,
                &context.session_key,
                card,
            )
            .await;
    }

    async fn dispatch_binding(
        &self,
        c: &ValidatedConnector,
        op: &ValidatedOp,
        op_args: &serde_json::Value,
        context: &RequestContext,
    ) -> Result<InvocationOutcome, String> {
        match &op.binding {
            nemesis_connectors::manifest::OpBinding::Http {
                method,
                path,
                placeholders,
            } => {
                self.run_http(c, op, *method, path, placeholders, op_args, context)
                    .await
            }
            nemesis_connectors::manifest::OpBinding::Tool { tool } => {
                self.run_tool_binding(c, op, tool, op_args, context).await
            }
            nemesis_connectors::manifest::OpBinding::Action { action } => {
                self.run_action(c, op, action, op_args)
            }
        }
    }

    /// http 绑定：SSRF 闸（有 security 时 resolve+钉死 IP+禁重定向）→
    /// 凭据注入（vault 别名 `connector/<id>/<slot>`）→ 占位符展开 → 发请求。
    /// 非 2xx = Failed（error 回执）。
    #[allow(clippy::too_many_arguments)]
    async fn run_http(
        &self,
        c: &ValidatedConnector,
        op: &ValidatedOp,
        method: nemesis_connectors::manifest::HttpMethod,
        path: &str,
        placeholders: &[String],
        op_args: &serde_json::Value,
        _context: &RequestContext,
    ) -> Result<InvocationOutcome, String> {
        let base = c
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("connector `{}` 缺 [connector.http].base", c.id))?;

        // 占位符展开：{name} ← args[name]；其余参数留给 query/body。
        let mut url = format!("{}{}", base.trim_end_matches('/'), path);
        let mut consumed = std::collections::BTreeSet::new();
        for ph in placeholders {
            let raw = op_args
                .get(ph)
                .ok_or_else(|| format!("缺少路径占位符参数 `{ph}`（args: {}）", op_args))?;
            let val = match raw {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            url = url.replace(&format!("{{{ph}}}"), &val);
            consumed.insert(ph.clone());
        }

        // 凭据注入：别名 connector/<id>/<slot>，真值只进请求，不进回执/日志。
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut query_pairs: Vec<(String, String)> = Vec::new();
        let vault_path = nemesis_path::resolve_vault_path_in_workspace(&self.workspace);
        for (slot, binding) in &c.credentials {
            let alias = format!("connector/{}/{}", c.id, slot);
            let secret = match crate::vault_runtime::resolve_alias_at(&vault_path, &alias) {
                Ok(s) => s,
                Err(e) => {
                    if binding.required {
                        return Err(format!("凭据槽位 `{slot}` 缺失: {e}"));
                    }
                    continue; // 可选槽位缺 = 跳过注入（无 key 走公开路径）
                }
            };
            match &binding.into {
                nemesis_connectors::manifest::CredentialInto::Header(h) => {
                    headers.push((h.clone(), secret));
                }
                nemesis_connectors::manifest::CredentialInto::Query(q) => {
                    query_pairs.push((q.clone(), secret));
                }
            }
        }

        // 剩余 args → GET 查询串 / POST 族 JSON body。
        let mut extra_query: Vec<(String, String)> = Vec::new();
        let mut body: Option<serde_json::Value> = None;
        if let Some(map) = op_args.as_object() {
            for (k, v) in map {
                if consumed.contains(k) {
                    continue;
                }
                if matches!(method, nemesis_connectors::manifest::HttpMethod::Get) {
                    let val = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    extra_query.push((k.clone(), val));
                } else {
                    body.get_or_insert_with(|| serde_json::Map::new().into());
                    if let Some(b) = body.as_mut().and_then(|b| b.as_object_mut()) {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }

        // SSRF 闸：resolve+校验 → 钉死 IP client（防 rebinding TOCTOU）。
        let client = self.build_http_client(&url)?;

        let mut req = match method {
            nemesis_connectors::manifest::HttpMethod::Get => client.get(&url),
            nemesis_connectors::manifest::HttpMethod::Post => client.post(&url),
            nemesis_connectors::manifest::HttpMethod::Put => client.put(&url),
            nemesis_connectors::manifest::HttpMethod::Delete => client.delete(&url),
        };
        for (h, v) in &headers {
            req = req.header(h, v);
        }
        // Vec<(String, String)> 经 serde 序列化为重复 k=v 对（reqwest 惯例）。
        if !query_pairs.is_empty() {
            req = req.query(&query_pairs);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("HTTP 请求失败: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let capped: String = text.chars().take(HTTP_BODY_CAP).collect();
        if !status.is_success() {
            return Ok(InvocationOutcome::Failed {
                summary: format!("HTTP {status}"),
                result_text: format!("HTTP {status}: {capped}"),
            });
        }
        Ok(InvocationOutcome::Done {
            summary: format!("HTTP {status}，返回 {} 字节", text.len()),
            receipt_key: op.receipt_key.clone(),
            receipt_value: text_receipt_value(&text, &op.receipt_key),
            result_text: capped,
        })
    }

    /// 无 redirect 的 HTTP client；security 在场时先过 SSRF resolve+校验并
    /// 钉死 IP（防 DNS rebinding TOCTOU）。
    fn build_http_client(&self, url: &str) -> Result<reqwest::Client, String> {
        #[cfg(feature = "security")]
        if let Some(plugin) = &self.security {
            let Some(guard) = plugin.ssrf_guard() else {
                return Err("SSRF 防护层未启用，拒绝 connector 网络出站（fail-closed）".to_string());
            };
            let ips = guard
                .resolve_and_validate_collect(url)
                .map_err(|e| format!("SSRF 校验拒绝: {e}"))?;
            return nemesis_agent::image_attach::build_pinned_no_redirect_client(
                url,
                &ips,
                "nemesisbot-connector",
                std::time::Duration::from_secs(10),
                std::time::Duration::from_secs(30),
            )
            .ok_or_else(|| "HTTP client 构造失败（URL 非法？）".to_string());
        }
        #[cfg(not(feature = "security"))]
        let _ = url;
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("nemesisbot-connector")
            .build()
            .map_err(|e| format!("HTTP client 构造失败: {e}"))
    }

    /// tool 绑定：经 Weak<AgentLoop> 回灌完整 dispatch 瀑布（estop/hidden/
    /// Plan/8 层/hooks 照跑——声明型 connector 不是旁路）。⛔ 前缀 =
    /// dispatch 层拒绝（denied_approval 已代写）→ DispatchRejected 透传。
    async fn run_tool_binding(
        &self,
        _c: &ValidatedConnector,
        _op: &ValidatedOp,
        tool: &str,
        op_args: &serde_json::Value,
        context: &RequestContext,
    ) -> Result<InvocationOutcome, String> {
        let Some(loop_arc) = self.loop_ref.read().clone().and_then(|w| w.upgrade()) else {
            return Err("agent loop unavailable (shutting down?)".to_string());
        };
        let call = nemesis_agent::ToolCallInfo {
            id: uuid::Uuid::new_v4().to_string(),
            name: tool.to_string(),
            arguments: serde_json::to_string(op_args)
                .map_err(|e| format!("args 序列化失败: {e}"))?,
        };
        // user 字段标 connector 供审计溯源；channel/chat/session 全随行
        // （bound tool 若发消息/写会话，落到发起对话）。
        let ctx = RequestContext::new(
            "connector-invoke",
            &context.chat_id,
            &context.user,
            &context.session_key,
        );
        let result = loop_arc.handle_tool_call_at_depth(&call, &ctx, 0).await;
        if result.starts_with('⛔') || result.starts_with("Error:") {
            return Ok(InvocationOutcome::DispatchRejected(result));
        }
        let summary = result.lines().next().unwrap_or("").to_string();
        Ok(InvocationOutcome::Done {
            summary,
            // receipt 主键在 fix_tool_receipt 里按 op 声明补抠（结果文本
            // 尽力 JSON 解析），这里统一 None。
            receipt_key: None,
            receipt_value: None,
            result_text: result,
        })
    }

    /// action 绑定（内置 demo 动作）：纸面账本三动作。
    fn run_action(
        &self,
        c: &ValidatedConnector,
        op: &ValidatedOp,
        action: &str,
        op_args: &serde_json::Value,
    ) -> Result<InvocationOutcome, String> {
        match action {
            "builtin.paper_order" => self.action_paper_order(c, op, op_args),
            "builtin.paper_positions" => self.action_paper_positions(c, op),
            "builtin.paper_cancel" => self.action_paper_cancel(c, op, op_args),
            other => Err(format!(
                "未知内置动作 `{other}`（运行时支持: builtin.paper_order / \
                 builtin.paper_positions / builtin.paper_cancel）"
            )),
        }
    }

    fn action_paper_order(
        &self,
        _c: &ValidatedConnector,
        _op: &ValidatedOp,
        op_args: &serde_json::Value,
    ) -> Result<InvocationOutcome, String> {
        let side = op_args
            .get("side")
            .and_then(|s| s.as_str())
            .map(str::trim)
            .ok_or_else(|| "缺少 side（buy|sell）".to_string())?;
        let symbol = op_args
            .get("symbol")
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少 symbol".to_string())?
            .to_string();
        let qty = op_args
            .get("qty")
            .and_then(|q| q.as_f64())
            .ok_or_else(|| "缺少 qty（数量）".to_string())?;
        let price = op_args
            .get("price")
            .and_then(|p| p.as_f64())
            .ok_or_else(|| {
                "缺少 price——纸面下单按显式价格成交，请先调该 connector 的行情 op 取价".to_string()
            })?;
        let ts = chrono::Utc::now().to_rfc3339();
        // 账本互斥：load→mutate→save 串行（磁盘真相源，无内存缓存）。
        let _guard = self.order_lock.lock();
        let mut ledger = nemesis_connectors::ledger::load_ledger(&self.workspace)?;
        let order = match side {
            "buy" => ledger.buy(&symbol, qty, price, &ts),
            "sell" => ledger.sell(&symbol, qty, price, &ts),
            other => return Err(format!("side 非法（{other}），应为 buy|sell")),
        }?;
        nemesis_connectors::ledger::save_ledger(&self.workspace, &ledger)?;
        let side_word = if order.side == nemesis_connectors::ledger::OrderSide::Buy {
            "买入"
        } else {
            "卖出"
        };
        let summary = format!(
            "{side_word} {} 股 {} @ {:.2}，订单 {}，余额 {:.2}（纸面模拟，零真实资金）",
            order.qty, order.symbol, order.price, order.order_id, ledger.cash
        );
        Ok(InvocationOutcome::Done {
            summary,
            receipt_key: Some("order_id".to_string()),
            receipt_value: Some(order.order_id.clone()),
            result_text: serde_json::to_string_pretty(&serde_json::json!({
                "executed": true,
                "order": order,
                "cash": ledger.cash,
            }))
            .map_err(|e| format!("结果序列化失败: {e}"))?,
        })
    }

    fn action_paper_positions(
        &self,
        _c: &ValidatedConnector,
        _op: &ValidatedOp,
    ) -> Result<InvocationOutcome, String> {
        let ledger = nemesis_connectors::ledger::load_ledger(&self.workspace)?;
        let summary = format!(
            "余额 {:.2}，持仓 {} 只，累计订单 {} 笔",
            ledger.cash,
            ledger.positions.len(),
            ledger.orders.len()
        );
        Ok(InvocationOutcome::Done {
            summary,
            receipt_key: None,
            receipt_value: None,
            result_text: serde_json::to_string_pretty(&ledger)
                .map_err(|e| format!("账本序列化失败: {e}"))?,
        })
    }

    fn action_paper_cancel(
        &self,
        _c: &ValidatedConnector,
        _op: &ValidatedOp,
        op_args: &serde_json::Value,
    ) -> Result<InvocationOutcome, String> {
        let order_id = op_args
            .get("order_id")
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少 order_id".to_string())?;
        let ledger = nemesis_connectors::ledger::load_ledger(&self.workspace)?;
        // cancel 恒 Err（v1 即时成交无挂单）——诚实边界即结果。
        let err = ledger.cancel(order_id).unwrap_err();
        Err(err)
    }
}

/// http 结果里抠回执主键值（receipt_key 声明时尽力解析 JSON 顶层字段；
/// 非形态匹配 = None，不强求）。
fn text_receipt_value(text: &str, key: &Option<String>) -> Option<String> {
    let key = key.as_deref()?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get(key).map(|x| match x {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

/// 单次执行结局（execute 尾部统一落回执用）。
enum InvocationOutcome {
    /// dispatch 层拒绝（⛔ 家族 / Error: 前缀）——denied_approval 已由
    /// dispatch 代写，原文透传不双写。
    DispatchRejected(String),
    /// 执行成功（executed 回执）。
    Done {
        summary: String,
        receipt_key: Option<String>,
        receipt_value: Option<String>,
        result_text: String,
    },
    /// 执行面失败（error 回执 + 诚实错误回模型）。
    Failed {
        summary: String,
        result_text: String,
    },
}
