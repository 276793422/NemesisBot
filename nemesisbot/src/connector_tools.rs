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
//! - 全 outcome 落统一回执（executed/error/budget_blocked/trust_blocked/
//!   disabled/unknown；denied_approval 由 dispatch 的 ABAC deny 分支代写——
//!   审批拒绝发生在 execute 之前，工具自身写不了，工具侧见 ⛔ 前缀即跳过
//!   不双写）；
//! - 执行前强制（R4，本地/远程单点覆盖）：args_schema 校验 + 资金参数形态
//!   （finance op 的 qty 必须在场且为有限正数）在出网前拒绝，不消耗额度；
//! - 预算护栏（限频 + 资金 fail-closed）在 execute 内、dispatch 审批之后
//!   ——先过人工卡再过限额，两道闸各自留痕；R3 起 reserve/commit/release
//!   三段原子占用（裁决即记账，收口按 outcome 决策表，RPC 未知结果保留
//!   账目），workspace 级共享单例（agent/loop 重建不清零）；
//! - 凭据注入：vault 别名 `connector/<id>/<slot>` 现开现解锁，真值只进
//!   HTTP 请求头/查询串，回执/日志只见别名与 digest。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::{AgentLoop, OperationOverride, Tool};
use nemesis_connectors::budget::{BudgetGuard, Limits, shared_budget_guard};
use nemesis_connectors::manifest::{OpClass, RiskLevel, ValidatedConnector, ValidatedOp};
use nemesis_connectors::receipt::{
    BudgetConsumed, Receipt, append_receipt, args_digest, new_receipt_id,
};
#[cfg(feature = "security")]
use nemesis_connectors::registry::search_connectors;
use nemesis_connectors::registry::{find_connector, find_op, validate_op_args};

/// 工具名（消费者 = agent_factory 的 security 闸注册块——同闸单一形态）。
/// 真相源在 nemesis-connectors（与 [`INVOKE_TOOL_NAME`] 同族 re-export）。
#[cfg(feature = "security")]
pub use nemesis_connectors::FIND_TOOL_NAME;
pub use nemesis_connectors::INVOKE_TOOL_NAME;

/// 远程执行通道（E1，收官批 2026-10-15）：与 ClusterWiring 的
/// `cluster_rpc_call_fn` 同形——`(peer, action, payload)` → 异步 JSON 响应。
/// 复用共享闭包（其 peer_chat 特殊逻辑按 action 过滤，`connector_invoke`
/// 走干净同步 RPC 路径）；集群未启用 = None = target_node 诚实报错。
pub type RemoteCallFn = Arc<
    dyn Fn(
            &str,
            &str,
            serde_json::Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
        > + Send
        + Sync,
>;

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

/// 资金类 op 的数量参数提取（R4 强制）：`finance_budget=true` 时 `qty`
/// **必须在场且为有限正数**——缺失/null/字符串/负数/0/非有限值一律拒绝。
/// 旧形态 `qty 缺失 → None → 跳过资金闸` 是免检旁路（声明了资金类的 op
/// 却能不带数量过闸），fail-closed 语义不容许。
fn finance_qty_of(op: &ValidatedOp, op_args: &serde_json::Value) -> Result<Option<f64>, String> {
    if !op.finance_budget {
        return Ok(None);
    }
    let Some(q) = op_args.get("qty") else {
        return Err("资金类 op 缺少必填数量参数 `qty`（number，>0）".to_string());
    };
    let Some(qty) = q.as_f64() else {
        return Err(format!(
            "资金类参数 `qty` 必须是 number，实际为 {}",
            json_type_name(q)
        ));
    };
    if !qty.is_finite() {
        return Err(format!("资金类参数 `qty` 非有限值（{qty}）"));
    }
    if qty <= 0.0 {
        return Err(format!("资金类参数 `qty` 必须为正数，实际为 {qty}"));
    }
    Ok(Some(qty))
}

/// 人读 JSON 类型名（错误文案用）。
fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
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
    undoable: bool,
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
        undoable,
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

/// 静态头 + 凭据头合并（A 段 manifest 增强）：同名裁决 = **vault 凭据胜**
/// ——静态头（`[connector.http.headers]`）是清单作者的缺省形态，凭据注入
/// 是运行时的显式意图；头名比较按 ASCII 小写折叠（HTTP 头名大小写不敏感）。
/// 输出按头名排序（确定性，测试可断言）。
fn merge_request_headers(
    static_headers: &std::collections::BTreeMap<String, String>,
    credential_headers: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut merged: std::collections::BTreeMap<String, (String, String)> =
        std::collections::BTreeMap::new();
    for (name, value) in static_headers {
        merged.insert(name.to_ascii_lowercase(), (name.clone(), value.clone()));
    }
    for (name, value) in credential_headers {
        merged.insert(name.to_ascii_lowercase(), (name, value));
    }
    merged.into_values().collect()
}

/// GET 剩余 args + 凭据 query 对合并（R6）：普通参数在前、**凭据 query 同名
/// 时凭据胜**（与 merge_request_headers 同一裁决——args 不得覆盖凭据）。
/// 顺序保持（普通参数按 args 出现序在前，凭据对在后），reqwest `.query()`
/// 按 Vec 序列化为重复 k=v 对并统一 percent-encoding。
fn merge_query_pairs(
    extra_query: Vec<(String, String)>,
    credential_query: &[(String, String)],
) -> Vec<(String, String)> {
    let mut merged: Vec<(String, String)> = extra_query
        .into_iter()
        .filter(|(k, _)| !credential_query.iter().any(|(ck, _)| ck == k))
        .collect();
    merged.extend(credential_query.iter().cloned());
    merged
}

// ---------------------------------------------------------------------------
// connector_find
// ---------------------------------------------------------------------------

/// 注册表发现工具：`{query?: string}`——空 query 列出全部可用 connector
/// （含 ops 全量元数据 + args_schema），非空按词元匹配。损坏 connector
/// 诚实进 `errors` 数组（不静默）。消费者 = agent_factory security 闸
/// 注册块——随 security feature 同闸（无 security 构建无工具供给面，
/// demo 也不用 find）。
#[cfg(feature = "security")]
pub struct ConnectorFindTool {
    workspace: PathBuf,
    /// B2：loop 快照句柄（镜像 op 枚举用）。缺省 None = 镜像面不出——
    /// 无 loop 的测试/独立装配形态下独占 connector 只列显式 ops（诚实，
    /// 不假列调不到的镜像）。
    loop_ref: parking_lot::RwLock<Option<Weak<AgentLoop>>>,
}

#[cfg(feature = "security")]
impl ConnectorFindTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            loop_ref: parking_lot::RwLock::new(None),
        }
    }

    /// Arc 定型后注入（agent_factory，与 ConnectorInvokeTool::set_loop
    /// 同点）。消费者 = security 闸注册块——同闸。
    #[cfg(feature = "security")]
    pub fn set_loop(&self, loop_weak: Weak<AgentLoop>) {
        *self.loop_ref.write() = Some(loop_weak);
    }

    /// B2+B3：单 connector 的镜像 op 枚举（独占声明为空 / 无 loop = 空表）。
    /// 基线按 server 自述 annotations hints 定档（B3 四档，registry::
    /// mirror_baseline）；description 随行（A4 检索词元 + 展示）。
    fn mirrors_of(&self, c: &ValidatedConnector) -> Vec<serde_json::Value> {
        if c.exclusive_mcp.is_empty() {
            return Vec::new();
        }
        let loop_arc = self.loop_ref.read().clone().and_then(|w| w.upgrade());
        let snapshot = loop_arc.map(|l| l.mcp_tool_snapshot());
        let guard = snapshot.as_ref().map(|s| s.read());
        let names: Vec<String> = guard
            .as_deref()
            .map(|g| g.iter().map(|e| e.name.clone()).collect())
            .unwrap_or_default();
        nemesis_connectors::registry::exclusive_mirror_ops(&self.workspace, c, &names)
            .into_iter()
            .map(|(tool, _)| {
                let entry = guard
                    .as_deref()
                    .and_then(|g| g.iter().find(|e| e.name == tool));
                let parameters = entry.map(|e| e.parameters.clone());
                let description = entry.map(|e| e.description.clone()).unwrap_or_default();
                let (risk, class, confirm) = nemesis_connectors::registry::mirror_baseline(
                    entry.and_then(|e| e.read_only_hint),
                    entry.and_then(|e| e.destructive_hint),
                );
                serde_json::json!({
                    "id": tool,
                    "intent": ["auto-mirror（独占接管基线）"],
                    "description": description,
                    "risk": risk.as_str(),
                    "class": class.as_str(),
                    "confirm_required": confirm,
                    "finance_budget": false,
                    "binding": format!("mirror:tool:{tool}"),
                    "args_schema": parameters,
                })
            })
            .collect()
    }

    /// B2+A4：镜像 (id, description) 清单（检索补漏用；独占声明为空 /
    /// 无 loop = 空表）。描述随行——检索词元可命中描述里的英文词根。
    fn mirror_pairs_of(&self, c: &ValidatedConnector) -> Vec<(String, String)> {
        if c.exclusive_mcp.is_empty() {
            return Vec::new();
        }
        let Some(loop_arc) = self.loop_ref.read().clone().and_then(|w| w.upgrade()) else {
            return Vec::new();
        };
        let snapshot = loop_arc.mcp_tool_snapshot();
        let guard = snapshot.read();
        nemesis_connectors::registry::exclusive_mirror_ops(
            &self.workspace,
            c,
            &guard.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
        )
        .into_iter()
        .map(|(t, _)| {
            let desc = guard
                .iter()
                .find(|e| e.name == t)
                .map(|e| e.description.clone())
                .unwrap_or_default();
            (t, desc)
        })
        .collect()
    }
}

#[cfg(feature = "security")]
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
        let tokens: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
        let mut hits = search_connectors(&self.workspace, &query);
        // B2+A4 检索补漏：search_connectors 只看显式 op / connector 级字段，
        // token 命中**镜像 id 或镜像描述词根**（英文词根可命中——原生工具
        // description 是独占面最富的检索素材）的独占 connector 会被它
        // 先行过滤掉——按注册表全量补漏，命中镜像 = 也算命中（独占接管面
        // 按原生根名词/描述可检索，与 execute 循环内的镜像并入注释同一意图）。
        if !tokens.is_empty() {
            let hit_ids: std::collections::HashSet<String> =
                hits.iter().map(|h| h.connector.id.clone()).collect();
            for loaded in nemesis_connectors::registry::scan_connectors(&self.workspace) {
                let Some(c) = loaded.connector else { continue };
                if !c.enabled || hit_ids.contains(&c.id) {
                    continue;
                }
                let matched: Vec<String> = self
                    .mirror_pairs_of(&c)
                    .into_iter()
                    .filter(|(id, desc)| {
                        let desc = desc.to_lowercase();
                        tokens.iter().any(|t| {
                            id.to_lowercase().contains(t.as_str()) || desc.contains(t.as_str())
                        })
                    })
                    .map(|(id, _)| id)
                    .collect();
                if !matched.is_empty() {
                    hits.push(nemesis_connectors::registry::ConnectorSearchHit {
                        matched_ops: matched,
                        trust: loaded.trust,
                        connector: c,
                    });
                }
            }
        }
        let errors: Vec<String> = nemesis_connectors::registry::scan_connectors(&self.workspace)
            .into_iter()
            .filter_map(|l| l.error.map(|e| format!("{}: {e}", l.dir.display())))
            .collect();
        let mut connectors: Vec<serde_json::Value> = Vec::with_capacity(hits.len());
        for h in hits.iter_mut() {
            // B2：镜像 op 并入展示与命中（token 命中镜像工具名**或描述词根**
            // = 该 connector 也算命中——独占接管面按原生根名词/描述可检索）。
            let mirrors = self.mirrors_of(&h.connector);
            let mirror_matched: Vec<String> = mirrors
                .iter()
                .filter(|m| {
                    tokens.is_empty()
                        || tokens.iter().any(|t| {
                            let id = m.get("id").and_then(|i| i.as_str()).unwrap_or("");
                            let desc = m
                                .get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_lowercase();
                            id.to_lowercase().contains(t.as_str()) || desc.contains(t.as_str())
                        })
                })
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                .map(|id| id.to_string())
                .collect();
            if h.matched_ops.is_empty() && !mirror_matched.is_empty() {
                h.matched_ops = mirror_matched.clone();
            } else {
                h.matched_ops.extend(mirror_matched.iter().cloned());
            }
            connectors.push(serde_json::json!({
                "id": h.connector.id,
                "name": h.connector.name,
                "version": h.connector.version,
                "description": h.connector.description,
                // M1：签名徽标随命中透出——模型可感知，自主优先高信任
                // 源（弱信号，不门控）。
                "trust": h.trust.as_str(),
                "matched_ops": h.matched_ops,
                "ops": h.connector.ops.iter().map(op_json).collect::<Vec<_>>(),
                // B2：镜像 op 追加在显式 ops 之后（独立数组——形态不同，
                // 基线档位见各字段；consume 方式与 ops 一致）。
                "mirrors": mirrors,
            }));
        }
        let total = connectors.len();
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "total": total,
            "connectors": connectors,
            "errors": errors,
        }))
        .map_err(honest_err)?)
    }
}

/// 单 op 元数据 → LLM 可见 JSON（含 args_schema——invoke 入参的权威契约）。
/// 唯一消费者 = ConnectorFindTool（同闸）。
#[cfg(feature = "security")]
fn op_json(o: &ValidatedOp) -> serde_json::Value {
    let binding = match &o.binding {
        nemesis_connectors::manifest::OpBinding::Http { method, path, .. } => {
            format!("{method:?} {path}")
        }
        nemesis_connectors::manifest::OpBinding::Tool { tool } => format!("tool:{tool}"),
        nemesis_connectors::manifest::OpBinding::Action { action } => format!("action:{action}"),
        nemesis_connectors::manifest::OpBinding::Script {
            command,
            timeout_secs,
            ..
        } => match timeout_secs {
            Some(t) => format!("script:{command} (timeout={t}s)"),
            None => format!("script:{command}"),
        },
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
    /// 集群 RPC 通道（agent_factory 注入，与 cluster_rpc 工具同源闭包；
    /// 缺省 None = target_node 诚实报错）。
    remote_call_fn: parking_lot::RwLock<Option<RemoteCallFn>>,
    /// R7：本次调用的父深度（`set_invocation_depth` 注入，0 = 顶层 agent）。
    /// tool 绑定回灌时透传给 `handle_tool_call_at_depth`——经 connector
    /// 绑定的 spawn 嵌套限制照常生效（SharedTool AtomicUsize 同模式；
    /// 共享实例 set-then-read 的并发边际误归属与 SpawnTool 同类已接受）。
    invocation_depth: std::sync::atomic::AtomicUsize,
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
        // R3：workspace 级共享预算单例——agent/loop 重建走同一 Arc，
        // 滑窗与日累计不清零（per-instance 计数器让重启即重置额度）。
        let budget = shared_budget_guard(&workspace);
        Self {
            workspace,
            config_path,
            #[cfg(feature = "security")]
            security,
            budget,
            loop_ref: parking_lot::RwLock::new(None),
            order_lock: parking_lot::Mutex::new(()),
            remote_call_fn: parking_lot::RwLock::new(None),
            invocation_depth: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Arc 定型后注入（agent_factory 调用；Weak 不延长 loop 生命周期）。
    /// 消费者 = security 闸注册块——同闸。
    #[cfg(feature = "security")]
    pub fn set_loop(&self, loop_weak: Weak<AgentLoop>) {
        *self.loop_ref.write() = Some(loop_weak);
    }

    /// 注入集群 RPC 通道（agent_factory；与 cluster_rpc 工具同源闭包）。
    /// 消费者 = security 闸注册块——同闸。
    #[cfg(feature = "security")]
    pub fn set_remote_call_fn(&self, f: RemoteCallFn) {
        *self.remote_call_fn.write() = Some(f);
    }

    /// 推送条件：写/exec/网络资金类执行面动作（读类静默只入账本——§3.5）。
    /// unknown（结果未知）是**最**需要推送的结局——恰恰因为它需要人核账
    /// （外部写可能已生效而账目被保留），漏推 = 用户无从发现需人工对账。
    fn should_push(class: OpClass, finance_budget: bool, outcome: &str) -> bool {
        (class != OpClass::Read || finance_budget)
            && matches!(
                outcome,
                "executed" | "error" | "budget_blocked" | "trust_blocked" | "unknown"
            )
    }

    /// B2（2026-10-19 独占接管）：镜像 op 解析——op_id 不在显式 `[[op]]`
    /// 清单时，若该 connector 独占的 MCP server 前缀命中此工具且 sidecar
    /// 策略为 default，伪造镜像 [`ValidatedOp`]（基线：risk=medium /
    /// class=read / 无 confirm / 无资金 / binding=Tool 直连原生工具名 /
    /// args_schema = 原生工具的注册参数 schema；基线档位按 server 自述
    /// annotations hints 四档定（B3，registry::mirror_baseline——readOnly
    /// 自述=low/read 免卡，未声明=medium/read 保守基线，自述非只读或破坏
    /// 性=high/write 强制确认卡）。真实危险度仍由 dispatch 瀑布按**原生
    /// 工具名**的注册声明裁决，镜像侧档位只决定「不比裸奔松」的接管基线。
    /// None = 非镜像（显式 op 查找失败照旧走 honest error 路径）。
    fn resolve_mirror_op(&self, c: &ValidatedConnector, op_id: &str) -> Option<ValidatedOp> {
        let loop_arc = self.loop_ref.read().clone().and_then(|w| w.upgrade())?;
        let snapshot = loop_arc.mcp_tool_snapshot();
        let guard = snapshot.read();
        let names: Vec<String> = guard.iter().map(|e| e.name.clone()).collect();
        let mirrors =
            nemesis_connectors::registry::exclusive_mirror_ops(&self.workspace, c, &names);
        if !mirrors.iter().any(|(t, _)| t == op_id) {
            return None;
        }
        let entry = guard.iter().find(|e| e.name == op_id)?;
        let (risk, class, confirm_required) = nemesis_connectors::registry::mirror_baseline(
            entry.read_only_hint,
            entry.destructive_hint,
        );
        Some(ValidatedOp {
            id: op_id.to_string(),
            intent: vec!["auto-mirror（独占接管基线，B2）".to_string()],
            risk,
            class,
            confirm_required,
            finance_budget: false,
            receipt_key: None,
            undo: None,
            binding: nemesis_connectors::manifest::OpBinding::Tool {
                tool: op_id.to_string(),
            },
            args_schema: Some(entry.parameters.clone()),
        })
    }
}

#[async_trait::async_trait]
impl Tool for ConnectorInvokeTool {
    // R7：dispatch 注入父深度（tool_dispatch.rs set_context 同点），tool
    // 绑定回灌时透传——经 connector 绑定的工具链 spawn 嵌套限制不重置。
    fn set_invocation_depth(&self, depth: usize) {
        self.invocation_depth
            .store(depth, std::sync::atomic::Ordering::Relaxed);
    }

    fn description(&self) -> String {
        "Execute a connector op declared in the local registry (connector.toml). \
         Requires connector id + op id + args object — discover them via \
         connector_find first. Every action gets a numbered receipt (see the \
         Action Ledger); write/finance ops require explicit approval and are \
         subject to budget limits (rate / finance caps, fail-closed). \
         Optionally set target_node to run the op on a cluster node instead — \
         credentials never cross the wire (the remote node unlocks its own \
         vault), and both nodes' approval/budget gates still apply."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "connector": {"type": "string", "description": "Connector id (from connector_find)"},
                "op": {"type": "string", "description": "Op id within the connector"},
                "args": {"type": "object", "description": "Op arguments (see the op's args_schema)"},
                "target_node": {"type": "string", "description": "Optional cluster node name/id — set to execute on that remote node (credentials stay local; the remote node uses its own vault). Omit for local execution."}
            },
            "required": ["connector", "op"]
        })
    }

    fn min_tier(&self) -> Option<&str> {
        Some("normal")
    }

    /// per-op 风险投影（dispatch 嗅探 → metadata 三键 → 管线裁决）。解析
    /// 不出（连接器/op 不存在或损坏）= None → 静态路径 fail-closed 兜底。
    /// M1 信任警示（2026-10-19）：HIGH/CRITICAL op 且 connector 未签名 →
    /// force_approval 追加警示行（审批卡多因展示；语义与 pre-exec 命中同
    /// 家族——非拦截，卡片必须到达人）。
    ///
    /// B2（2026-10-19）：显式 op 未命中时先试**镜像 op**解析（独占接管
    /// 基线：凭条照记，档位按 annotations hints 四档定，见
    /// resolve_mirror_op 注记）——否则镜像 id 落 None = 静态
    /// 未知名 fail-closed CRITICAL，每次镜像调用都逼出危急审批卡，接管
    /// 基线被自身管线击穿（§6.3③ 预判的自堵陷阱）。
    fn operation_override(&self, args: &str) -> Option<OperationOverride> {
        let v: serde_json::Value = serde_json::from_str(args).ok()?;
        let connector = v.get("connector")?.as_str()?.trim();
        let op_id = v.get("op")?.as_str()?.trim();
        if connector.is_empty() || op_id.is_empty() {
            return None;
        }
        let (c, trust) = find_connector(&self.workspace, connector).ok()?;
        let op_args = v.get("args").cloned().unwrap_or(serde_json::json!({}));
        let (op_type, force_approval, risk, class) = match find_op(&c, op_id) {
            Some(op) => {
                // M1：unsigned + 高危 op → 警示并入强审批文案（confirm=
                // required 的理由在前，警示在后；无 confirm 的 HIGH/CRITICAL
                // 警示单独成卡——未经人看一眼的未签名高危动作不值得自动
                // 放行）。
                let mut force = op.confirm_required.then(|| {
                    format!(
                        "connector `{}` op `{}` 声明强制人工确认（confirm=required）——\
                         执行前必须得到用户明确批准",
                        c.id, op.id
                    )
                });
                if matches!(op.risk, RiskLevel::High | RiskLevel::Critical)
                    && trust == nemesis_connectors::signing::ConnectorTrust::Unsigned
                {
                    let warn = "⚠ 该 connector 未经签名验证（无 connector.toml.sig 或签名者不在信任库）\
                         ——请自行确认来源可信";
                    force = Some(match force {
                        Some(f) => format!("{f}；{warn}"),
                        None => warn.to_string(),
                    });
                }
                (
                    op_slug_for(op.risk, op.class).to_string(),
                    force,
                    op.risk,
                    op.class,
                )
            }
            // B2 镜像基线（B3 升级：四档化）——基线由镜像 hints 推导：
            // 默认 medium+read → registry_read 档（管线按 slug 反查 =
            // MEDIUM；字面语义不承担实际类别，见 resolve_mirror_op 注记）；
            // 自述破坏性/非只读 → high+write+confirm（slug file_write），
            // confirm_required 带理由成强制审批卡。
            None => {
                let m = self.resolve_mirror_op(&c, op_id)?;
                let force = m.confirm_required.then(|| {
                    format!(
                        "connector `{}` 镜像 op `{}` 自述破坏性（MCP destructive/非只读）——\
                         执行前必须得到用户明确批准",
                        c.id, op_id
                    )
                });
                (
                    op_slug_for(m.risk, m.class).to_string(),
                    force,
                    m.risk,
                    m.class,
                )
            }
        };
        Some(OperationOverride {
            op_type,
            force_approval,
            receipt_ctx: serde_json::to_string(&ReceiptCtxJson {
                workspace: self.workspace.to_string_lossy().to_string(),
                connector: c.id.clone(),
                op: op_id.to_string(),
                risk: risk.as_str().to_string(),
                class: class.as_str().to_string(),
                args_digest: args_digest(&op_args),
            })
            .ok(),
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let (result, _receipt) = self.execute_with_receipt(args, context).await;
        result
    }
}

impl ConnectorInvokeTool {
    /// execute 的本体（E1，收官批 2026-10-15）：返回值带凭条本体——远程
    /// 分支（run_remote）与页内撤销（E3 connectors.undo）需要凭条 id /
    /// outcome / 主键做镜像与回读，Tool trait 形态不变（execute 委托）。
    ///
    /// 流程：args 解析（Err 无回执）→ 注册表三态（Disabled→disabled 回执；
    /// NotFound/Invalid→Err 无回执）→ find_op → **target_node 指定 = 远程
    /// 分支**（master 闸 + 对端执行 + 镜像凭条）→ 本地预算闸 → 按绑定
    /// 分派（⛔ 前缀不双写；错误形态文本走 error 回执）。
    pub async fn execute_with_receipt(
        &self,
        args: &str,
        context: &RequestContext,
    ) -> (Result<String, String>, Option<Receipt>) {
        let v: serde_json::Value = match serde_json::from_str(args) {
            Ok(v) => v,
            Err(e) => return (Err(honest_err(format!("args JSON 非法: {e}"))), None),
        };
        let Some(connector) = v
            .get("connector")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return (Err(honest_err("缺少 connector 字段")), None);
        };
        let Some(op_id) = v
            .get("op")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return (Err(honest_err("缺少 op 字段")), None);
        };
        let target_node = v
            .get("target_node")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let op_args = v.get("args").cloned().unwrap_or(serde_json::json!({}));
        if !op_args.is_object() {
            return (
                Err(honest_err("args 必须是对象（见该 op 的 args_schema）")),
                None,
            );
        }
        let digest = args_digest(&op_args);

        // 注册表三态：NotFound/Invalid = 无账可记（诚实报错回模型）；
        // Disabled = connector 在册但停用——记 disabled 回执（存在性是账）。
        let (c, trust) = match find_connector(&self.workspace, connector) {
            Ok((c, trust)) => (c, trust),
            Err(nemesis_connectors::registry::ConnectorLookupError::Disabled { id }) => {
                let r = write_receipt(
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
                    false,
                );
                return (
                    Err(honest_err(format!(
                        "connector `{id}` 已停用——如需启用改 connector.toml enabled = true"
                    ))),
                    Some(r),
                );
            }
            Err(e) => return (Err(honest_err(e)), None),
        };
        // B2：显式 op 未命中 → **镜像 op 兜底**（独占接管基线）。镜像 op
        // 是治理壳伪造的 ValidatedOp（binding=Tool 直连原生工具名，
        // args_schema = 原生注册参数），后续预检/预算/信任闸/回执/分派流程
        // 与显式 op 完全同路——接管不加旁路。
        let mirror_slot: ValidatedOp;
        let op: &ValidatedOp = match find_op(&c, op_id) {
            Some(op) => op,
            None => match self.resolve_mirror_op(&c, op_id) {
                Some(m) => {
                    mirror_slot = m;
                    &mirror_slot
                }
                None => {
                    let hint = if c.exclusive_mcp.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "；该 connector 独占接管 MCP server（{}）——镜像 op \
                             = 原生工具名（connector_find 可列，封禁者除外）",
                            c.exclusive_mcp.join(", ")
                        )
                    };
                    return (
                        Err(honest_err(format!(
                            "connector `{}` 无 op `{}`（可用: {}）{}",
                            c.id,
                            op_id,
                            c.ops
                                .iter()
                                .map(|o| o.id.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                            hint
                        ))),
                        None,
                    );
                }
            },
        };

        // R4 执行前强制（单点，本地/远程同覆盖）：args_schema 校验 + 资金
        // 参数形态强制——必须在出网（本地 dispatch / 远程 RPC）之前拒绝，
        // 且不消耗任何额度（reserve 之前）。
        if let Err(e) = validate_op_args(op.args_schema.as_ref(), &op_args) {
            return (
                Err(honest_err(format!(
                    "⛔ INVALID ARGS: {e}（未执行，未消耗额度）"
                ))),
                None,
            );
        }
        let finance_qty = match finance_qty_of(op, &op_args) {
            Ok(q) => q,
            Err(e) => {
                return (
                    Err(honest_err(format!(
                        "⛔ INVALID ARGS: {e}（未执行，未消耗额度）"
                    ))),
                    None,
                );
            }
        };

        // M2：script 绑定占位符实参在出网前校验——声明式命令模板不能半
        // 展开（白名单在解析期钉死占位符集合，这里钉实参在审批/预算之前
        // 就位；与 args_schema 校验同 family，不消耗额度）。
        if let nemesis_connectors::manifest::OpBinding::Script { placeholders, .. } = &op.binding {
            for ph in placeholders {
                if op_args.get(ph).map(|v| v.is_null()).unwrap_or(true) {
                    return (
                        Err(honest_err(format!(
                            "⛔ INVALID ARGS: script 命令占位符 `{ph}` 缺实参（未执行，未消耗额度）"
                        ))),
                        None,
                    );
                }
            }
        }

        // M1 信任总闸（connectors.require_signed=true）：unsigned 一律拒执行。
        // 单点落在全部形态校验之后、出网（本地 dispatch / 远程 RPC）之前——
        // 本地与远程同覆盖；凭条 outcome=trust_blocked（budget_blocked 家族
        // 语义：拒绝有账，额度未消耗）。require_signed 缺省 false = 只警示
        // （operation_override 的审批卡警示行），拦截是用户显式选择。
        let cfg = connectors_config(&self.config_path);
        if cfg.require_signed && trust == nemesis_connectors::signing::ConnectorTrust::Unsigned {
            let r = write_receipt(
                &self.workspace,
                &c.id,
                &op.id,
                op.risk.as_str(),
                op.class.as_str(),
                "trust_blocked",
                &digest,
                "connector 未经签名验证（connectors.require_signed=true 拒绝执行 unsigned 清单）",
                None,
                None,
                None,
                Some(&context.session_key),
                context.approval_ref.clone(),
                false,
            );
            self.maybe_push_card(context, &c, op, &r, &cfg).await;
            return (
                Err(honest_err(format!(
                    "⛔ TRUST: connector `{}` 未经签名验证（无 connector.toml.sig 或签名者不在信任库）\
                     ——require_signed=true 拒绝执行。本地自签开发请 `connector trust` 自己的公钥，\
                     或显式关闭该闸",
                    c.id
                ))),
                Some(r),
            );
        }

        // 远程分支（E1）：target_node 指定 → master 闸 + 集群派发 +
        // 镜像凭条（对端还有完整 dispatch 瀑布与本地预算，双闸是刻意的）。
        if let Some(target) = target_node {
            return self
                .run_remote(&c, op, &op_args, &digest, &target, context, finance_qty)
                .await;
        }

        // 预算闸（fail-closed；审批已在 dispatch 过完——先人工后限额）。
        // R3：reserve 原子占用（裁决即记账），收口按 outcome 决策表：
        // DispatchRejected/Failed/Err → release（确定性未出网/本地失败）；
        // Done → commit（既成事实）。
        let limits = budget_limits(&cfg);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let reservation = match self.budget.reserve(&limits, &c.id, now_ms, finance_qty) {
            Ok(r) => r,
            Err(verdict) => {
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
                    false,
                );
                self.maybe_push_card(context, &c, op, &r, &cfg).await;
                return (
                    Err(honest_err(format!(
                        "⛔ RATE LIMIT: {}",
                        verdict.denial_message()
                    ))),
                    Some(r),
                );
            }
        };

        // 按绑定分派。⛔ 前缀（dispatch 安全层拒绝族）= denied_approval
        // 回执已由 dispatch 的 ABAC deny 分支代写——工具侧跳过不双写，原文
        // 透传；错误形态文本（Error:/Tool error:/Exit code:/Plan mode:）
        // = dispatch 未代写凭条，走 error 回执（错误也是账）。
        let exec = self.dispatch_binding(&c, op, &op_args, context).await;
        match exec {
            // 安全闸拒绝 = 确定未出网 → 回吐额度（R3 决策表）
            Ok(InvocationOutcome::DispatchRejected(raw)) => {
                self.budget.release(&reservation);
                (Ok(raw), None)
            }
            Ok(InvocationOutcome::Done {
                summary,
                receipt_key,
                receipt_value,
                result_text,
            }) => {
                // 既成事实 → 预备转正式（幂等；记账在 reserve 已落）
                self.budget.commit(&reservation);
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
                    op.undo.is_some(),
                );
                self.maybe_push_card(context, &c, op, &r, &cfg).await;
                (Ok(result_text), Some(r))
            }
            Ok(InvocationOutcome::Failed {
                summary,
                result_text,
            }) => {
                // 本地绑定确定性失败（未产生外部效果）→ 回吐额度
                self.budget.release(&reservation);
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
                    false,
                );
                self.maybe_push_card(context, &c, op, &r, &cfg).await;
                (Err(honest_err(result_text)), Some(r))
            }
            Ok(InvocationOutcome::UnknownOutcome {
                summary,
                result_text,
            }) => {
                // R3 决策表第四臂（复查补口；F1 扩面：超时或传输中途断开）=
                // 结果未知——请求可能已送达并被对端处理（外部写可能已生效）。
                // 保留账目不回吐（release 会放行额度 = 绕过限额），与
                // run_remote 的 RPC Err 臂同语义。
                tracing::warn!(
                    connector = %c.id,
                    op = %op.id,
                    "[Connector] 本地 HTTP 执行结果未知（超时/传输中断），预算占用保留（不回吐）"
                );
                let r = write_receipt(
                    &self.workspace,
                    &c.id,
                    &op.id,
                    op.risk.as_str(),
                    op.class.as_str(),
                    // 词表与远程路径同源：unknown（结果未知）不是 error——
                    // 审计账本必须能区分「对端明确报错（未执行）」与「超时
                    // 结果未知（可能已执行）」；undo 闸对两者都拒撤，但文案
                    // 语义不同。
                    "unknown",
                    &digest,
                    &summary,
                    None,
                    None,
                    None,
                    Some(&context.session_key),
                    context.approval_ref.clone(),
                    false,
                );
                self.maybe_push_card(context, &c, op, &r, &cfg).await;
                (Err(honest_err(result_text)), Some(r))
            }
            Err(e) => {
                // 本地绑定异常（未产生外部效果）→ 回吐额度
                self.budget.release(&reservation);
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
                    false,
                );
                self.maybe_push_card(context, &c, op, &r, &cfg).await;
                (Err(honest_err(e)), Some(r))
            }
        }
    }

    /// 远程执行分支（E1，收官批 2026-10-15）：target_node 指定 → 经集群
    /// RPC 在对端节点执行（action=`connector_invoke`，对端 worker handler
    /// 走本地 `handle_tool_call` 完整 dispatch 瀑布）。
    ///
    /// 凭据铁律跨节点不变：payload 只含 `connector/op/args/session_key`，
    /// **凭据永不上 RPC 线**——对端用自己的 vault 解锁。master 侧闸照跑：
    /// operation_override 投影在 dispatch 已裁决（审批卡在 master 出），
    /// budget.reserve 在派发前 fail-closed（E2 聚合记账第二道闸；对端还有
    /// 自己的一套本地限额）。R3 收口决策表：对端 `executed` → commit；
    /// 其他 outcome（denied/error/budget_blocked/disabled，对端明确回执）
    /// → release；**RPC 本身 Err（超时/传输错误）→ 保留账目不回吐**——
    /// 结果未知（外部写可能已生效），放行额度 = 绕过限额。镜像凭条
    /// 新铸 receipt_id（非对端的）、`undoable=false`（跨节点撤销 v1 挂账
    /// ——LLM 可手动带 target_node 调 undo op）。
    #[allow(clippy::too_many_arguments)]
    async fn run_remote(
        &self,
        c: &ValidatedConnector,
        op: &ValidatedOp,
        op_args: &serde_json::Value,
        digest: &str,
        target: &str,
        context: &RequestContext,
        finance_qty: Option<f64>,
    ) -> (Result<String, String>, Option<Receipt>) {
        let f = self.remote_call_fn.read().clone();
        let Some(f) = f else {
            return (
                Err(honest_err(format!(
                    "target_node=`{target}` 指定远程执行，但本节点未启用集群（无 RPC 通道）\
                     ——去掉 target_node 本地执行，或先在集群页启用"
                ))),
                None,
            );
        };

        // master 侧预算闸（fail-closed；审批卡在 master dispatch 已出——
        // 这里是派发前的第二道闸，被拒 = 请求不出线）。R3：reserve 原子
        // 占用，收口见下方决策表。
        let cfg = connectors_config(&self.config_path);
        let limits = budget_limits(&cfg);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let reservation = match self.budget.reserve(&limits, &c.id, now_ms, finance_qty) {
            Ok(r) => r,
            Err(verdict) => {
                let r = write_receipt(
                    &self.workspace,
                    &c.id,
                    &op.id,
                    op.risk.as_str(),
                    op.class.as_str(),
                    "budget_blocked",
                    digest,
                    &verdict.denial_message(),
                    None,
                    None,
                    None,
                    Some(&context.session_key),
                    context.approval_ref.clone(),
                    false,
                );
                self.maybe_push_card(context, c, op, &r, &cfg).await;
                return (
                    Err(honest_err(format!(
                        "⛔ RATE LIMIT: {}",
                        verdict.denial_message()
                    ))),
                    Some(r),
                );
            }
        };

        // 派发载荷：无凭据（铁律），session_key 随行供对端会话归属。
        let payload = serde_json::json!({
            "connector": c.id,
            "op": op.id,
            "args": op_args,
            "session_key": context.session_key,
        });
        let resp = match f(target, INVOKE_TOOL_NAME, payload).await {
            Ok(resp) => resp,
            Err(e) => {
                // R3：RPC 失败（超时/传输错误）= 结果未知——外部写可能已
                // 生效，**保留账目不回吐**（release 会放行额度 = 绕过限额）。
                // 凭条词表与本地 UnknownOutcome 臂同源：落 "unknown" 不是
                // "error"——审计账本必须能区分「对端明确报错（未执行）」与
                // 「RPC 超时结果未知（可能已执行）」；undo 前置闸对两者的
                // 拒撤措辞也不同（未真实生效 vs 结果未知可能已生效）。
                tracing::warn!(
                    connector = %c.id,
                    op = %op.id,
                    target = %target,
                    error = %e,
                    "[Connector] 远程执行 RPC 失败，预算占用保留（结果未知，不回吐）"
                );
                let r = write_receipt(
                    &self.workspace,
                    &c.id,
                    &op.id,
                    op.risk.as_str(),
                    op.class.as_str(),
                    "unknown",
                    digest,
                    &format!("[远程@{target}] RPC 失败（结果未知）: {e}"),
                    None,
                    None,
                    None,
                    Some(&context.session_key),
                    context.approval_ref.clone(),
                    false,
                );
                self.maybe_push_card(context, c, op, &r, &cfg).await;
                return (
                    Err(honest_err(format!("远程执行失败（{target}）: {e}"))),
                    Some(r),
                );
            }
        };

        // 解析对端响应 `{receipt, result_text}`——字段缺失诚实兜底。
        let worker_receipt = resp
            .get("receipt")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let worker_text = resp
            .get("result_text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // 对端回执缺 outcome（或非字符串）分两臂：
        // ① result_text 带 ⛔ 前缀 = 对端 dispatch 层明确拒绝（estop/hidden/
        //    Plan/tier 闸——工具从未执行、凭条从未落账，worker 回执面诚实
        //    不造假 = null）。与本地 classify_tool_result 同一裁决标记：
        //    确定性「未执行」落 "error"（下方决策表 release 回吐预算，R3：
        //    对端确认未执行），不是 unknown。
        // ② 其余形态（执行后凭条读回失败等真未知）= 请求可能已送达并在
        //    B 端执行（外部写可能已生效），不能按 "error" 兜底（error 会
        //    走 release 回吐预算 = R3 语义旁路）。落 "unknown" 凭条，决策
        //    表保留账目（与 RPC Err 臂同语义）。
        let worker_outcome = match worker_receipt.get("outcome") {
            Some(serde_json::Value::String(s)) => s.clone(),
            _ if worker_text.starts_with('⛔') => {
                tracing::warn!(
                    connector = %c.id,
                    op = %op.id,
                    target = %target,
                    "[Connector] 对端 dispatch 层拒绝（无凭条回执），镜像 error + 预算回吐"
                );
                "error".to_string()
            }
            _ => "unknown".to_string(),
        };
        let worker_receipt_id = worker_receipt
            .get("receipt_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        let worker_summary = worker_receipt
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // E2 聚合记账 + R3 收口决策表：对端明确回执 = 确定性结论——
        // executed → commit（既成事实）；denied/error/budget_blocked/disabled
        // → release（对端确认未执行，回吐额度）；unknown（envelope 缺
        // outcome，结果未知）→ 保留账目不回吐（同 RPC Err 臂）。（RPC Err
        // 分支不走到这——上方已提前 return 且保留账目。）
        let executed = worker_outcome == "executed";
        if worker_outcome == "unknown" {
            tracing::warn!(
                connector = %c.id,
                op = %op.id,
                "[Connector] 远程回执缺 outcome 字段，预算占用保留（结果未知，不回吐）"
            );
        } else if executed {
            self.budget.commit(&reservation);
        } else {
            self.budget.release(&reservation);
        }
        let rate_used = self
            .budget
            .rate_used(&c.id, chrono::Utc::now().timestamp_millis());
        let finance_today = op.finance_budget.then(|| {
            self.budget
                .finance_today(chrono::Utc::now().timestamp_millis())
                .0
        });
        // 对端无凭条（worker_summary 空）时把对端文本拒因带进账本——审计
        // 凭条自解释，不逼人回查 gateway 日志（截 160 字符防爆账）。
        let mirror_summary = if worker_summary.is_empty() && !worker_text.is_empty() {
            let reason: String = worker_text.chars().take(160).collect();
            format!("[远程@{target} · {worker_receipt_id}] {reason}")
        } else {
            format!("[远程@{target} · {worker_receipt_id}] {worker_summary}")
        };
        let r = write_receipt(
            &self.workspace,
            &c.id,
            &op.id,
            op.risk.as_str(),
            op.class.as_str(),
            &worker_outcome,
            digest,
            &mirror_summary,
            worker_receipt.get("receipt_key").and_then(|v| v.as_str()),
            worker_receipt.get("receipt_value").and_then(|v| v.as_str()),
            executed.then_some(BudgetConsumed {
                ops_last_hour: rate_used,
                finance_orders_today: finance_today,
            }),
            Some(&context.session_key),
            context.approval_ref.clone(),
            false, // 跨节点撤销 v1 挂账——LLM 可手动带 target_node 调 undo op
        );
        self.maybe_push_card(context, c, op, &r, &cfg).await;
        let note = format!("\n（已在节点 {target} 远程执行，对端凭条 {worker_receipt_id}）");
        match worker_outcome.as_str() {
            // executed / denied_approval（⛔ 家族）= 本地同形态透传（dispatch
            // 对 Err 才加 Tool error: 前缀——安全裁决原文不动）。
            "executed" | "denied_approval" => (
                Ok(format!(
                    "{}{}",
                    worker_text
                        .strip_prefix("Tool error: ")
                        .unwrap_or(&worker_text),
                    note
                )),
                Some(r),
            ),
            // error/budget_blocked/disabled/未知 = 诚实失败回模型。对端文本
            // 可能已带 "Tool error: " 前缀——剥掉避免双前缀（dispatch 会
            // 重新包一层）。
            _ => (
                Err(format!(
                    "{}{note}",
                    worker_text
                        .strip_prefix("Tool error: ")
                        .unwrap_or(&worker_text)
                )),
                Some(r),
            ),
        }
    }

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
            nemesis_connectors::manifest::OpBinding::Script {
                command,
                placeholders,
                timeout_secs,
                workdir,
                capture,
            } => {
                self.run_script_binding(
                    c,
                    op,
                    command,
                    placeholders,
                    *timeout_secs,
                    workdir.as_deref(),
                    *capture,
                    op_args,
                    context,
                )
                .await
            }
            nemesis_connectors::manifest::OpBinding::Action { action } => {
                self.run_action(c, op, action, op_args, context).await
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
        // header 形态支持 prefix（认证头惯用形态：Gitea `token <PAT>`）。
        // vault 解析器随 security feature 门控（vault_runtime 模块同闸）——
        // 无 security 构建 = 必填槽位诚实报「解析器未安装」，可选槽位跳过
        // 注入（与缺凭据同路：无 key 走公开路径）。
        #[cfg_attr(not(feature = "security"), allow(unused_mut))]
        let mut cred_headers: Vec<(String, String)> = Vec::new();
        #[cfg_attr(not(feature = "security"), allow(unused_mut))]
        let mut query_pairs: Vec<(String, String)> = Vec::new();
        #[cfg(feature = "security")]
        let vault_path = nemesis_path::resolve_vault_path_in_workspace(&self.workspace);
        for (slot, binding) in &c.credentials {
            // 无 security 构建：vault 解析器缺席（vault_runtime 模块同闸）——
            // 必填槽位诚实报「解析器未安装」，可选槽位跳过注入（与缺凭据
            // 同路：无 key 走公开路径）。
            #[cfg(not(feature = "security"))]
            {
                if binding.required {
                    return Err(format!(
                        "凭据槽位 `{slot}` 缺失: vault 解析器未安装（security feature 未编译）"
                    ));
                }
                continue; // 可选槽位 = 跳过注入（无 key 走公开路径）
            }
            #[cfg(feature = "security")]
            {
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
                        let value = match &binding.prefix {
                            Some(p) => format!("{p}{secret}"),
                            None => secret,
                        };
                        cred_headers.push((h.clone(), value));
                    }
                    nemesis_connectors::manifest::CredentialInto::Query(q) => {
                        query_pairs.push((q.clone(), secret));
                    }
                }
            }
        }
        // 静态头 + 凭据头合并：同名时 vault 凭据胜（凭据是显式意图，
        // 静态头是缺省形态——见 merge_request_headers）。
        let headers = merge_request_headers(&c.static_headers, cred_headers);

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

        // R6：剩余 args（GET）与凭据 query 对合并发送——普通参数在前、
        // 凭据同名时凭据胜；全部经 reqwest `.query()` percent-encoding，
        // 已有 query 串的 URL 用 `&` 追加（reqwest 追加语义）。
        let merged_query = merge_query_pairs(extra_query, &query_pairs);

        // SSRF 闸：resolve+校验 → 钉死 IP client（防 rebinding TOCTOU）。
        let client = self.build_http_client(&url)?;

        let mut req = match method {
            nemesis_connectors::manifest::HttpMethod::Get => client.get(&url),
            nemesis_connectors::manifest::HttpMethod::Post => client.post(&url),
            nemesis_connectors::manifest::HttpMethod::Put => client.put(&url),
            nemesis_connectors::manifest::HttpMethod::Patch => client.patch(&url),
            nemesis_connectors::manifest::HttpMethod::Delete => client.delete(&url),
        };
        for (h, v) in &headers {
            req = req.header(h, v);
        }
        // Vec<(String, String)> 经 serde 序列化为重复 k=v 对（reqwest 惯例）。
        if !merged_query.is_empty() {
            req = req.query(&merged_query);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                // 客户端超时 = 结果未知（请求可能已送达并被对端处理，外部
                // 写可能已生效）→ UnknownOutcome 臂保留账目（R3 决策表第四
                // 臂，与 run_remote 的 RPC Err 臂同语义），不当普通失败串。
                return Ok(InvocationOutcome::UnknownOutcome {
                    summary: "HTTP 客户端超时（结果未知）".to_string(),
                    result_text: format!(
                        "HTTP 请求超时（30s）：结果未知——请求可能已送达并被对端\
                         处理，外部写可能已生效；预算占用保留。错误: {e}"
                    ),
                });
            }
            Err(e) if e.is_connect() || e.is_request() => {
                // 连接建立失败 / 请求构造错误（reqwest 语义：connect=TCP/
                // TLS 握手期失败，request=发送前构造失败）= 请求**未发出**，
                // 确定性未生效 → 普通失败串（上层 Failed 臂 release 回吐
                // 额度，R3 决策表 Failed 臂）。
                return Err(format!("HTTP 请求失败: {e}"));
            }
            Err(e) => {
                // 其余传输层失败（body 写入中途断开、连接 reset 等）= 请求
                // 可能已完整送达并被对端处理——结果未知，同超时语义保留账目
                // （release 会放行额度 = R3 超买旁路，2026-10-16 二轮复查
                // F1 补口）。
                return Ok(InvocationOutcome::UnknownOutcome {
                    summary: "HTTP 传输错误（结果未知）".to_string(),
                    result_text: format!(
                        "HTTP 传输层失败（非超时）：结果未知——请求可能已送达并\
                         被对端处理，外部写可能已生效；预算占用保留。错误: {e}"
                    ),
                });
            }
        };
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
    /// Plan/8 层/hooks 照跑——声明型 connector 不是旁路）。结果按
    /// [`classify_tool_result`] 三分类落账：⛔ 家族透传不双写（安全层已
    /// 裁决）；错误形态文本落 error 回执（dispatch 未代写的拒绝/失败也是
    /// 账）；成功按 manifest 声明的 receipt_key 抠主键（镜像 run_http）。
    async fn run_tool_binding(
        &self,
        _c: &ValidatedConnector,
        op: &ValidatedOp,
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
        // R7：clone 原 ctx 只改审计标签（channel→"connector-binding" 供
        // 溯源），tool_path_base / correlation_id / async_callback 原样
        // 保留——档案管线任务的工作副本路径基准、RPC 关联 ID、异步回调跨
        // 桥不丢（此前 new() 把四者全置 None）。approval_ref 例外：dispatch
        // 层刻意覆盖（tool_dispatch.rs「凭条不伪造背书」——只信本次调用
        // 真实发生的交互审批），透传上游引用反而是伪造背书。原 ctx 不被
        // 修改。chat/session/user 随行（bound tool 发消息/写会话落到发起
        // 对话）。
        let mut ctx = context.clone();
        ctx.channel = "connector-binding".to_string();
        let depth = self
            .invocation_depth
            .load(std::sync::atomic::Ordering::Relaxed);
        let result = loop_arc.handle_tool_call_at_depth(&call, &ctx, depth).await;
        let summary = result.lines().next().unwrap_or("").to_string();
        Ok(match classify_tool_result(&result) {
            ToolBindingClass::DispatchRejected => InvocationOutcome::DispatchRejected(result),
            ToolBindingClass::Failed => InvocationOutcome::Failed {
                summary,
                result_text: result,
            },
            ToolBindingClass::UnknownOutcome => InvocationOutcome::UnknownOutcome {
                summary,
                result_text: result,
            },
            ToolBindingClass::Done => InvocationOutcome::Done {
                summary,
                // C4：receipt 主键镜像 run_http——manifest 声明 receipt_key
                // 时对结果文本尽力 JSON 顶层字段解析（playwright-mcp 等
                // 返回 JSON 的 MCP 工具可抠主键；非形态匹配 = None 不强求）。
                receipt_key: op.receipt_key.clone(),
                receipt_value: text_receipt_value(&result, &op.receipt_key),
                result_text: result,
            },
        })
    }

    /// M2（2026-10-19）script 绑定：声明式命令模板 → exec 同源通路。
    ///
    /// 展开的命令整体作为 exec 的单参数（command 字段）——占位符白名单
    /// 在解析期钉死、实参在 R4 出网前已验；执行经 [`Self::run_tool_binding`]
    /// 回灌 `handle_tool_call_at_depth`，命令守卫/安全 8 层/审批/审计照跑
    /// ——script 绑定不是绕过命令守卫的后门。超时/目录/捕获是清单作者
    /// 钉死的声明式语义（不是模型现场决定）：timeout 缺省走 config
    /// `connectors.script.default_timeout_secs`（0 = 拒绝所有 script op），
    /// workdir 相对 workspace 根（解析期已拒绝对路径/`..`），capture 决定
    /// Done 凭条 summary 捕获 stdout 尾部字节数（缺省 400；Failed/Unknown
    /// 不截——错误文本是诊断主体）。
    #[allow(clippy::too_many_arguments)]
    async fn run_script_binding(
        &self,
        c: &ValidatedConnector,
        op: &ValidatedOp,
        command: &str,
        placeholders: &[String],
        timeout_secs: Option<u64>,
        workdir: Option<&str>,
        capture: Option<u32>,
        op_args: &serde_json::Value,
        context: &RequestContext,
    ) -> Result<InvocationOutcome, String> {
        // 全局闸（config connectors.script.default_timeout_secs = 0 = 拒绝）。
        let cfg = connectors_config(&self.config_path);
        let default_timeout = cfg.script_timeout_secs();
        if default_timeout == 0 {
            return Err(
                "script 执行被配置禁用（connectors.script.default_timeout_secs = 0）".to_string(),
            );
        }
        let expanded = expand_script_command(command, placeholders, op_args)?;
        let timeout = timeout_secs.unwrap_or(default_timeout);
        let mut args = serde_json::json!({ "command": expanded, "timeout": timeout });
        if let Some(wd) = workdir {
            args["cwd"] = serde_json::json!(wd);
        }
        let outcome = self.run_tool_binding(c, op, "exec", &args, context).await?;
        match outcome {
            InvocationOutcome::Done {
                result_text,
                receipt_key,
                receipt_value,
                ..
            } => {
                let n = usize::try_from(capture.unwrap_or(400)).unwrap_or(400);
                Ok(InvocationOutcome::Done {
                    summary: utf8_tail(&result_text, n),
                    receipt_key,
                    receipt_value,
                    result_text,
                })
            }
            other => Ok(other),
        }
    }

    /// action 绑定（内置动作）：纸面账本三动作 + monitor 语义检查（E4）。
    async fn run_action(
        &self,
        c: &ValidatedConnector,
        op: &ValidatedOp,
        action: &str,
        op_args: &serde_json::Value,
        context: &RequestContext,
    ) -> Result<InvocationOutcome, String> {
        match action {
            "builtin.noop" => self.action_noop(c, op),
            "builtin.paper_order" => self.action_paper_order(c, op, op_args),
            "builtin.paper_positions" => self.action_paper_positions(c, op),
            "builtin.paper_cancel" => self.action_paper_cancel(c, op, op_args),
            "builtin.monitor_check" => self.action_monitor_check(c, op, op_args, context).await,
            other => Err(format!(
                "未知内置动作 `{other}`（运行时支持: builtin.noop / builtin.paper_order / \
                 builtin.paper_positions / builtin.paper_cancel / builtin.monitor_check）"
            )),
        }
    }

    /// E4（收官批 2026-10-15）monitor 语义变化检测（域 A 收官）：fetch（
    /// web_fetch 原语经 loop 回灌，完整 dispatch 瀑布照跑）→ 快照 diff →
    /// 变化时 LLM 语义摘要。快照约定 `<workspace>/data/monitor_snapshots/
    /// <name>`（name 经 sanitize 防路径逃逸）；首跑建基线、全等无变化；
    /// `semantic=true`（缺省）摘要走 `agents.small_model` 通道（未配置 =
    /// 主模型 + WARN，见 [`AgentLoop::chore_completion`]），`semantic=false`
    /// 机械比对（P0 旧行为——agent 自行比对的剧本原语化）。
    ///
    /// 诚实边界：变化检测是启发式——比对在截断归一形态上进行（纯布局/
    /// 动态时间戳噪声可能误报）；摘要失败不吞变化事实（变化照常成立，
    /// 摘要位诚实注明失败原因）。快照轮转（新内容覆盖旧快照）在任何摘要
    /// 之前落盘——写失败 = 本次检查结果不可信，诚实报错不落账。
    async fn action_monitor_check(
        &self,
        c: &ValidatedConnector,
        op: &ValidatedOp,
        op_args: &serde_json::Value,
        context: &RequestContext,
    ) -> Result<InvocationOutcome, String> {
        let url = op_args
            .get("url")
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少 url（要监控的完整 URL）".to_string())?;
        let name = op_args
            .get("name")
            .and_then(|s| s.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少 name（监控项名称，用作快照标识）".to_string())?;
        let semantic = op_args
            .get("semantic")
            .and_then(|s| s.as_bool())
            .unwrap_or(true);

        // 1) fetch：复用 web_fetch 原语（SSRF 闸/8 层管线照跑，声明型
        // connector 不是旁路）。
        let fresh = match self
            .run_tool_binding(
                c,
                op,
                "web_fetch",
                &serde_json::json!({ "url": url }),
                context,
            )
            .await?
        {
            InvocationOutcome::Done { result_text, .. } => result_text,
            InvocationOutcome::DispatchRejected(text) => {
                return Ok(InvocationOutcome::DispatchRejected(text));
            }
            InvocationOutcome::Failed { result_text, .. } => {
                return Ok(InvocationOutcome::Failed {
                    summary: "抓取失败".to_string(),
                    result_text,
                });
            }
            // monitor 抓取无预算语义（不 reserve），超时按失败透传。
            InvocationOutcome::UnknownOutcome { result_text, .. } => {
                return Ok(InvocationOutcome::Failed {
                    summary: "抓取超时".to_string(),
                    result_text,
                });
            }
        };

        // 2) 快照 + 状态判定 + 轮转（fetch 之后的全部逻辑收在
        // [`monitor_finalize`]，纯 fs + 字符串——单测不经 loop 直接驱动）。
        let decision = monitor_finalize(&self.workspace, name, url, semantic, &fresh)?;

        // 3) 摘要（变化 + semantic 开关分流；baseline/unchanged 零 token）。
        let (state, summary, semantic_used) = match &decision {
            MonitorDecision::Baseline { summary } => ("baseline", summary.clone(), false),
            MonitorDecision::Unchanged { summary } => ("unchanged", summary.clone(), false),
            MonitorDecision::ChangedMechanical { summary } => ("changed", summary.clone(), false),
            MonitorDecision::ChangedNeedsSummary { prompt } => {
                match self.summary_llm("monitor-diff", prompt).await {
                    Ok(text) => {
                        let one_line: String = text
                            .lines()
                            .map(str::trim)
                            .filter(|l| !l.is_empty())
                            .collect::<Vec<_>>()
                            .join(" ");
                        ("changed", format!("`{name}` 变化摘要：{one_line}"), true)
                    }
                    Err(e) => (
                        "changed",
                        format!("`{name}` 内容有变化（语义摘要生成失败：{e}）"),
                        false,
                    ),
                }
            }
        };

        Ok(InvocationOutcome::Done {
            summary,
            receipt_key: Some("state".to_string()),
            receipt_value: Some(state.to_string()),
            result_text: serde_json::to_string_pretty(&serde_json::json!({
                "url": url,
                "name": name,
                "changed": state == "changed",
                "state": state,
                "semantic_used": semantic_used,
                "snapshot_chars": fresh.chars().count(),
            }))
            .map_err(|e| format!("结果序列化失败: {e}"))?,
        })
    }

    /// E4 语义摘要 LLM 调用：small_model 槽位优先 / 未配置回主模型 + WARN
    /// （都在 [`AgentLoop::chore_completion`] 内）；无 loop（停机窗口）诚实
    /// 失败，摘要位会注明，变化事实不受影响。
    async fn summary_llm(&self, label: &str, prompt: &str) -> Result<String, String> {
        let Some(loop_arc) = self.loop_ref.read().clone().and_then(|w| w.upgrade()) else {
            return Err("agent loop unavailable".to_string());
        };
        loop_arc
            .chore_completion(label, prompt.to_string(), MONITOR_SUMMARY_MAX_TOKENS)
            .await
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

    /// 内置空动作（前端「新建连接器」最小模板的运行期支撑）：不做任何事，
    /// 恒成功——新作者粘模板即得一个可真实 invoke 的连接器，走完整治理面
    /// （凭条/预算/审批链照跑）验证管线连通，再逐步换成自己的绑定。
    fn action_noop(
        &self,
        c: &ValidatedConnector,
        op: &ValidatedOp,
    ) -> Result<InvocationOutcome, String> {
        Ok(InvocationOutcome::Done {
            summary: format!("noop（内置空动作）: {}.{} 执行成功，零副作用", c.id, op.id),
            receipt_key: None,
            receipt_value: None,
            result_text: format!(
                "ok: connector `{}` op `{}` 是 builtin.noop 绑定（零副作用验证动作）。\
                 把 [op] 的 action 换成 http/tool/script 绑定即可接入真实能力。",
                c.id, op.id
            ),
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

/// E4：快照比对/喂模的单侧字符上限（诚实边界：变化检测是启发式——比对在
/// 截断形态上进行；同时防超大页面把 token/内存打爆）。
const MONITOR_SNAPSHOT_MAX_CHARS: usize = 20_000;

/// E4：语义摘要输出 token 预算（杂务短输出；护栏复用 aux 治理面）。
const MONITOR_SUMMARY_MAX_TOKENS: u32 = 1024;

/// E4：单侧内容截断（比对与喂模同口径——差异只落在截断点之后 = 视为无
/// 变化，这是「变化检测是启发式」边界的实现形态）。
fn monitor_clip(s: &str) -> String {
    let trimmed = s.trim();
    trimmed.chars().take(MONITOR_SNAPSHOT_MAX_CHARS).collect()
}

/// E4：monitor check 的快照/判定决策（fetch 之后、LLM 之前的全部分叉——
/// 纯 fs + 字符串，单测不经 loop 直接驱动）。`ChangedNeedsSummary` 只携带
/// 组装好的 prompt，LLM 调用留在 action（需要 loop_ref 的杂务通道）。
#[derive(Debug)]
enum MonitorDecision {
    /// 首跑：建了基线，无比对结论。
    Baseline { summary: String },
    /// 与上次快照全等。
    Unchanged { summary: String },
    /// 有变化但摘要被关掉（semantic=false，P0 机械比对旧行为）。
    ChangedMechanical { summary: String },
    /// 有变化且需要 LLM 语义摘要（semantic=true 缺省臂）。
    ChangedNeedsSummary { prompt: String },
}

/// E4：check 的快照机体——读旧快照 → 截断比对 → 轮转写新（写失败诚实报
/// 错，不落半可信账）→ 按状态/开关产出决策。快照约定
/// `<workspace>/data/monitor_snapshots/<name>`（name 经 sanitize 防路径
/// 逃逸）。
/// E4：快照文件 slug（sanitize 白名单单一真相源 + 非直通名摘要分离——
/// 非 ASCII 名经白名单摊平后互相碰撞（「首页」「仓库」同为 "__"），追加
/// 原名摘要 8 位一一对应；ASCII 直通名保持干净文件名）。
fn monitor_slug(name: &str) -> String {
    let slug = nemesis_utils::sanitize::sanitize_path_segment(name);
    if slug == name {
        slug
    } else {
        format!(
            "{slug}-{}",
            &nemesis_connectors::receipt::args_digest(&serde_json::json!(name))[..8]
        )
    }
}

fn monitor_finalize(
    ws: &Path,
    name: &str,
    url: &str,
    semantic: bool,
    fresh: &str,
) -> Result<MonitorDecision, String> {
    let slug = monitor_slug(name);
    let dir = ws.join("data").join("monitor_snapshots");
    std::fs::create_dir_all(&dir).map_err(|e| format!("快照目录创建失败: {e}"))?;
    let snap = dir.join(format!("{slug}.txt"));
    let previous = std::fs::read_to_string(&snap).ok();
    let changed = previous
        .as_deref()
        .map(|prev| monitor_clip(prev) != monitor_clip(fresh));
    std::fs::write(&snap, fresh.as_bytes()).map_err(|e| format!("快照写入失败: {e}"))?;

    Ok(match (changed, semantic) {
        (None, _) => MonitorDecision::Baseline {
            summary: format!("首次抓取 `{name}`，已建立基线快照（下次运行开始比对变化）"),
        },
        (Some(false), _) => MonitorDecision::Unchanged {
            summary: format!("`{name}` 无变化"),
        },
        (Some(true), false) => MonitorDecision::ChangedMechanical {
            summary: format!("`{name}` 内容有变化（机械比对；semantic=false 未生成语义摘要）"),
        },
        (Some(true), true) => {
            let prev_show = previous.as_deref().map(monitor_clip).unwrap_or_default();
            MonitorDecision::ChangedNeedsSummary {
                prompt: build_monitor_diff_prompt(name, url, &prev_show, &monitor_clip(fresh)),
            }
        }
    })
}

/// E4：语义摘要 prompt（变化检测的喂模构造，纯函数供测试）。
fn build_monitor_diff_prompt(name: &str, url: &str, prev: &str, fresh: &str) -> String {
    format!(
        "你在为一个网页监控任务生成变化摘要。监控项「{name}」（{url}）的内容发生了变化。\n\
         下面是同一页面的新旧两次抓取文本（各自截断，省略号处有更多未展示内容）。\n\
         请用 2-4 句中文说明：①什么内容变了（新增/删除/修改了什么）；\n\
         ②如果变化看起来只是时间戳/广告/推荐位等无意义噪声，直接说明是噪声，不必夸大。\n\
         只输出摘要正文，不要标题、不要 markdown 代码块。\n\n\
         === 旧内容 ===\n{prev}\n\n=== 新内容 ===\n{fresh}\n"
    )
}

/// tool 绑定结果的分类（C4 回执语义，2026-10-15）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolBindingClass {
    /// dispatch 安全层主动拦截（⛔ 家族：estop/limits/security/guardian/
    /// hook/script-scan）——其中 ABAC deny 分支已凭 receipt_ctx 代写
    /// denied_approval 凭条，其余分支属安全基础设施裁决。统一透传不双写。
    DispatchRejected,
    /// dispatch 早退拒绝（Unknown tool / hidden / min-tier / allowed-set /
    /// tools-disabled，`Error:` 前缀——dispatch 只在 ABAC deny 分支代写
    /// 凭条，这些路径无账）或绑定工具执行失败（`Tool error:` /
    /// `Exit code:` 前缀）或 Plan 闸拦截（`Plan mode:` 前缀，此前落 Done
    /// 分支产生假 executed 凭条）。工具侧落 error 回执——错误也是账。
    Failed,
    /// 绑定工具执行超时（R3 决策表第四臂，2026-10-16 二轮复查补口）：
    /// 错误形态文本含 "timed out" 且非 `Exit code:` 前缀——工具可能已在
    /// 目标上生效后才被掐断（外部写可能已发生）。**保留账目不回吐**，
    /// unknown 凭条（与 run_http / run_remote 超时臂同语义）。
    UnknownOutcome,
    /// 执行成功 → executed 回执。
    Done,
}

/// 判定刻度与 dispatch/turn WAL 同源：⛔ 前缀 = 安全层裁决（WAL 同款
/// ok 判定）；错误形态复用 `turn_guard::tool_result_indicates_error`
/// （Error:/Tool error:/Exit code: 三前缀，单一真相源）；`Plan mode:` 是
/// dispatch Plan 闸的拒绝文本（工具成功输出不以它开头）。
///
/// 超时特判（F2，二轮复查）：错误形态 + 含 "timed out" → unknown。只对
/// `Error:`/`Tool error:` 前缀生效——`Exit code:` 前缀是进程**跑完后**
/// 退出非零（确定性失败），即便输出恰好含 "timed out" 字样（如测试二进
/// 制的超时失败输出）也是确定性结果，不误判（executor/shell 的真实超时
/// 文本形态见 executor.rs:81/107、shell.rs:661，均为单行独立错误非进程
/// 输出混入）。
/// M2：script 命令模板单遍扫描展开——`{name}`（在白名单内的占位符）替换
/// 为 args 实参（字符串原样；其余 JSON 序列化形态），其余字符照抄。单遍
/// 而非逐名 replace 的原因：实参值不做二次展开（值里含 `{xxx}` 字样保持
/// 字面——逐名 replace 会把前一个实参值里的占位符字样再展开，注入面）。
/// 白名单外的 `{xxx}` 保持字面（解析期已拒，防御性兜底）。
fn expand_script_command(
    command: &str,
    placeholders: &[String],
    op_args: &serde_json::Value,
) -> Result<String, String> {
    let bytes = command.as_bytes();
    let mut out = String::with_capacity(command.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'}' {
                j += 1;
            }
            if j < bytes.len() {
                let name = &command[i + 1..j];
                if placeholders.iter().any(|p| p == name) {
                    let raw = op_args
                        .get(name)
                        .ok_or_else(|| format!("script 命令占位符 `{name}` 缺实参"))?;
                    match raw {
                        serde_json::Value::String(s) => out.push_str(s),
                        other => out.push_str(&other.to_string()),
                    }
                    i = j + 1;
                    continue;
                }
            }
        }
        let ch = command[i..].chars().next().expect("合法 UTF-8 切片");
        out.push(ch);
        i += ch.len_utf8();
    }
    Ok(out)
}

/// 取文本尾部至多 n 字节（UTF-8 字符边界对齐——切点向后找首个字符边界，
/// 不劈半个字符）。
fn utf8_tail(text: &str, n: usize) -> String {
    if text.len() <= n {
        return text.to_string();
    }
    let start = text.len() - n;
    let start = (start..=text.len())
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(text.len());
    text[start..].to_string()
}

fn classify_tool_result(result: &str) -> ToolBindingClass {
    if result.starts_with('⛔') {
        return ToolBindingClass::DispatchRejected;
    }
    // F2 超时特判扩面（M2 script 绑定实测发现）：exec 工具的超时收尸 =
    // `Ok("Command timed out …")`（loop_tools.rs `exec_output_passed` 单一
    // 真相源的失败形态之二），**无错误前缀**——走不到下面的错误头分支，
    // 会被误判 Done → 假 executed 凭条。含 "timed out" 即结果未知（命令
    // 可能已被处理了一部分）；`Exit code:` 前缀 = 进程跑完退出非零（确定
    // 性失败），输出含 "timed out" 字样也不误判（防线保留）。
    if !result.starts_with("Exit code:") && result.contains("timed out") {
        return ToolBindingClass::UnknownOutcome;
    }
    if nemesis_agent::turn_guard::tool_result_indicates_error(result)
        || result.starts_with("Plan mode:")
    {
        return ToolBindingClass::Failed;
    }
    ToolBindingClass::Done
}

/// 单次执行结局（execute 尾部统一落回执用）。
enum InvocationOutcome {
    /// dispatch 安全层拒绝（⛔ 家族）——ABAC deny 已由 dispatch 代写
    /// denied_approval，其余安全分支属基础设施裁决；原文透传不双写。
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
    /// 结果未知（R3 决策表第四臂，2026-10-16 复查补口）：客户端超时——
    /// 请求可能已送达并被对端处理（外部写可能已生效）。**保留账目不回吐**
    /// （与 run_remote 的 RPC Err 臂同语义），落 error 回执 + WARN。
    UnknownOutcome {
        summary: String,
        result_text: String,
    },
}
