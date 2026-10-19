//! connector.toml 清单 schema 与校验（v1）。
//!
//! 结构形态（单文件）：
//!
//! ```toml
//! [connector]
//! id = "paper-broker"
//! name = "纸面券商"
//! version = "0.1.0"
//! kind = "rest"                       # rest | local | script
//! enabled = true
//! description = "..."
//! [connector.http]                    # rest 型必填
//! base = "https://example.com"
//! [connector.http.headers]            # 可选：静态自定义头（非敏感用途；
//! User-Agent = "my-agent/1.0"         #  头名冲突时 vault 凭据胜）
//! [connector.credentials]             # 凭据槽位 → vault 别名 connector/<id>/<key>
//! api_key = { required = false, into = "header:X-API-Key" }
//! pat = { required = true, into = "header:Authorization", prefix = "token " }
//!
//! [[op]]
//! id = "get_quote"
//! intent = ["查询行情"]
//! risk = "low"
//! class = "read"
//! http = { method = "GET", path = "/quote?symbol={symbol}" }
//!
//! [[op]]
//! id = "place_order"
//! risk = "critical"
//! class = "write"
//! confirm = "required"                # 强授权卡
//! budget = "finance"                  # 资金预算维
//! receipt = "order_id"                # 回执主键字段
//! undo = { op = "cancel_order", arg = "order_id" }  # E3 撤销声明
//! tool = "exec"                       # local/script 型用 tool / action 绑定
//!
//! [[op]]                              # M2（2026-10-19）script 声明式绑定：
//! id = "rotate_logs"                  #  命令模板 + 钉死的超时/目录/捕获
//! risk = "high"                       #  语义——不是模型现场决定；执行走
//! class = "exec"                      #  exec 同源通路（命令守卫+8 层照跑）
//! [op.script]
//! command = "powershell -File scripts/rotate.ps1 -Dir {dir}"
//! timeout_secs = 120                  # 缺省走 config connectors.script
//! workdir = "scripts"                 # 相对 workspace 根，缺省 workspace 根
//! capture = 400                       # 凭条 summary 捕获 stdout 尾部字节
//! ```
//!
//! 全结构 `deny_unknown_fields`（拼错字段 loud 报错，不静默丢）。原始反序
//! 列化形态经 [`parse_connector_manifest`] 校验为 [`ValidatedConnector`]：
//! id/op 字符集、op 唯一、恰一绑定（kind 仅描述性，混合绑定合法）、
//! confirm/budget 值域、http path / script command 占位符语法 + 白名单
//! （占位符必须被 args_schema.properties 声明——未声明的 `{xxx}` 解析期
//! 报错，不静默透传给 shell）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 原始反序列化形态
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorManifestRaw {
    #[serde(rename = "connector")]
    pub connector: ConnectorMetaRaw,
    #[serde(rename = "op", default)]
    pub ops: Vec<OpDefRaw>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorMetaRaw {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: ConnectorKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub http: Option<HttpBaseRaw>,
    #[serde(default)]
    pub credentials: Option<BTreeMap<String, CredentialSlotRaw>>,
    /// B2（2026-10-19 独占接管）：独占声明的 MCP server 名（单串或数组）。
    /// 非空 + 启用 = 该 server 的全部导出工具从模型工具面隐藏（`mcp_` 前缀
    /// 整组），只能经本 connector 的镜像 op / 显式 [[op]] 间接调用——治理闸
    /// 唯一入口；停用/删除/总开关关 = 原生直通恢复。缺省/空 = 不独占
    /// （默认态「装了即生效」，拦截只经显式声明出现）。
    #[serde(default)]
    pub exclusive_mcp: Option<ExclusiveMcpSpec>,
}

/// B2：`exclusive_mcp` 两形态（`exclusive_mcp = "playwright-mcp"` 或
/// `exclusive_mcp = ["a", "b"]`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExclusiveMcpSpec {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectorKind {
    /// HTTP API 声明型（op 用 http 绑定）。
    Rest,
    /// 本地能力映射（op 用 tool/action 绑定）。
    Local,
    /// 脚本型（op 映射 exec 类工具）。
    Script,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpBaseRaw {
    pub base: String,
    /// 静态自定义请求头（非敏感用途：User-Agent/Accept 等声明形态）。
    /// 与 credentials 注入共存；头名冲突时 vault 凭据胜（执行面
    /// `merge_request_headers` 裁决——凭据是显式意图，静态头是缺省形态）。
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialSlotRaw {
    #[serde(default)]
    pub required: bool,
    /// 注入形态：`"header:<头名>"` 或 `"query:<参数名>"`；缺省 `header:<槽位名>`。
    #[serde(default)]
    pub into: Option<String>,
    /// 注入值前缀（认证头惯用形态：Gitea `Authorization: token <PAT>`、
    /// GitHub `Authorization: Bearer <PAT>`）。仅 header 形态消费；最终值
    /// = `prefix + secret`。禁止 CRLF（头注入防线，validate 兜底）。
    #[serde(default)]
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpDefRaw {
    pub id: String,
    /// LLM 层意图匹配素材（中文短语列表）。
    #[serde(default)]
    pub intent: Vec<String>,
    pub risk: RiskLevel,
    pub class: OpClass,
    /// `"required"` = 强授权卡。
    #[serde(default)]
    pub confirm: Option<String>,
    /// `"finance"` = 资金预算维。
    #[serde(default)]
    pub budget: Option<String>,
    /// 回执主键字段名（如 `order_id`）。
    #[serde(default)]
    pub receipt: Option<String>,
    /// E3（收官批）撤销声明：`{ op = "cancel_order", arg = "order_id" }` —
    /// 本 op 可用 `op`（同 connector 内的另一 op）以凭条主键值填 `arg` 参
    /// 反向。校验三规则见 [`validate`]。
    #[serde(default)]
    pub undo: Option<OpUndoRaw>,
    /// rest 型绑定：`{ method, path }`。
    #[serde(default)]
    pub http: Option<HttpOpRaw>,
    /// local/script 型绑定：映射到现有工具名。
    #[serde(default)]
    pub tool: Option<String>,
    /// 内置动作绑定（demo 域专用，如 `builtin.demo`）。
    #[serde(default)]
    pub action: Option<String>,
    /// M2（2026-10-19）script 声明式绑定：`[op.script]` 表。
    #[serde(default)]
    pub script: Option<ScriptOpRaw>,
    #[serde(default)]
    pub args_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpOpRaw {
    pub method: HttpMethod,
    pub path: String,
}

/// M2（2026-10-19）script 绑定原始形态：`[op.script]`。
///
/// 声明式语义（与 `tool = "exec"` 的差别）：超时/目录/输出截断是**作者
/// 钉死的**，不是模型现场决定；命令模板 `{arg}` 占位从 args_schema 展开
/// 为单参数整体替换。执行映射到 exec 同源通路（完整 dispatch 瀑布 + 命令
/// 守卫 + 安全 8 层照跑）——script 绑定不是绕过命令守卫的后门。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptOpRaw {
    /// 命令模板（`{arg}` 占位符从 args_schema.properties 展开；未声明的
    /// 占位符解析期报错）。
    pub command: String,
    /// 超时秒数；缺省走 config `connectors.script.default_timeout_secs`
    /// （全局缺省 60）。上限 600 与 exec 工具闸对齐（声明超出 = 假承诺，
    /// 解析期 loud 拒）。
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// 工作目录（相对 workspace 根；缺省 workspace 根。绝对路径/含 `..`
    /// 解析期拒——声明式 workdir 不开越界通道；运行期 exec 工具的
    /// restrict 边界照跑）。
    #[serde(default)]
    pub workdir: Option<String>,
    /// 凭条 summary 捕获 stdout 尾部字节数（缺省 400；上限 65536——凭条
    /// 是人看/卡面的，不是日志转储）。
    #[serde(default)]
    pub capture: Option<u32>,
}

/// E3（收官批）撤销声明原始形态：`undo = { op = "cancel_order", arg = "order_id" }`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpUndoRaw {
    /// 反向 op id（同 connector 内已声明的另一个 op）。
    pub op: String,
    /// 反向 op 的参数名——执行撤销时以源 op 凭条主键值（receipt_value）填入。
    pub arg: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    /// 排序权重（Low=0 … Critical=3）。
    pub fn rank(self) -> u8 {
        match self {
            RiskLevel::Low => 0,
            RiskLevel::Medium => 1,
            RiskLevel::High => 2,
            RiskLevel::Critical => 3,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpClass {
    Read,
    Write,
    Exec,
    Network,
}

impl OpClass {
    pub fn as_str(self) -> &'static str {
        match self {
            OpClass::Read => "read",
            OpClass::Write => "write",
            OpClass::Exec => "exec",
            OpClass::Network => "network",
        }
    }
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// 校验后形态
// ---------------------------------------------------------------------------

/// 校验通过的 connector（解析产物，消费方只接触这个形态）。
#[derive(Debug, Clone)]
pub struct ValidatedConnector {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: ConnectorKind,
    pub enabled: bool,
    pub description: Option<String>,
    /// `[connector.http].base`（rest 型必有）。
    pub base_url: Option<String>,
    /// `[connector.http].headers` 静态自定义头（非敏感缺省形态；凭据头
    /// 冲突时被 vault 凭据覆盖——执行面 merge 裁决）。
    pub static_headers: BTreeMap<String, String>,
    pub credentials: BTreeMap<String, CredentialBinding>,
    /// B2：独占的 MCP server 名清单（归一后形态：去空白、去空、去重、保序）。
    /// 空 = 不独占。
    pub exclusive_mcp: Vec<String>,
    /// ops（保清单书写序）。
    pub ops: Vec<ValidatedOp>,
}

#[derive(Debug, Clone)]
pub struct CredentialBinding {
    pub slot: String,
    pub required: bool,
    pub into: CredentialInto,
    /// header 形态的值前缀（如 `"token "` / `"Bearer "`）。
    pub prefix: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CredentialInto {
    Header(String),
    Query(String),
}

#[derive(Debug, Clone)]
pub struct ValidatedOp {
    pub id: String,
    pub intent: Vec<String>,
    pub risk: RiskLevel,
    pub class: OpClass,
    pub confirm_required: bool,
    pub finance_budget: bool,
    pub receipt_key: Option<String>,
    /// E3（收官批）撤销声明（校验通过后形态）。
    pub undo: Option<OpUndo>,
    pub binding: OpBinding,
    pub args_schema: Option<serde_json::Value>,
}

/// E3（收官批）撤销声明（校验后形态）。
#[derive(Debug, Clone)]
pub struct OpUndo {
    /// 反向 op id。
    pub op: String,
    /// 反向 op 的参数名（填源 op 凭条主键值）。
    pub arg: String,
}

#[derive(Debug, Clone)]
pub enum OpBinding {
    Http {
        method: HttpMethod,
        path: String,
        /// path 中的占位符名（按出现序，`{symbol}` → `symbol`）。
        placeholders: Vec<String>,
    },
    Tool {
        tool: String,
    },
    Action {
        action: String,
    },
    /// M2（2026-10-19）script 声明式绑定。
    Script {
        /// 命令模板（`{arg}` 占位待展开）。
        command: String,
        /// 模板占位符名（全部必须在 args_schema.properties 声明——解析期
        /// 已验，运行期只按此展开）。
        placeholders: Vec<String>,
        /// 清单级超时覆盖（None = config 全局缺省 60）。
        timeout_secs: Option<u64>,
        /// 工作目录（相对 workspace 根；None = workspace 根）。
        workdir: Option<String>,
        /// 凭条 summary 捕获 stdout 尾部字节数（None = 400）。
        capture: Option<u32>,
    },
}

// ---------------------------------------------------------------------------
// 解析与校验
// ---------------------------------------------------------------------------

/// 解析 + 校验 connector.toml 文本（单一入口）。
pub fn parse_connector_manifest(text: &str) -> Result<ValidatedConnector, String> {
    let raw: ConnectorManifestRaw =
        toml::from_str(text).map_err(|e| format!("TOML 解析失败: {e}"))?;
    validate(raw)
}

fn validate(raw: ConnectorManifestRaw) -> Result<ValidatedConnector, String> {
    let meta = raw.connector;

    validate_id(&meta.id, "connector id")?;

    // http base + 静态头
    let base_url = meta.http.as_ref().map(|h| h.base.trim().to_string());
    if base_url.as_deref().is_some_and(str::is_empty) {
        return Err(format!(
            "connector `{}`: [connector.http].base 不能为空",
            meta.id
        ));
    }
    let mut static_headers = BTreeMap::new();
    if let Some(h) = &meta.http {
        for (name, value) in &h.headers {
            validate_header_name(name)
                .map_err(|e| format!("connector `{}`: 静态头 `{name}` {e}", meta.id))?;
            validate_header_value(value)
                .map_err(|e| format!("connector `{}`: 静态头 `{name}` 的值 {e}", meta.id))?;
            static_headers.insert(name.clone(), value.clone());
        }
    }

    // credentials `into` 解析
    let mut credentials = BTreeMap::new();
    if let Some(slots) = meta.credentials {
        for (slot, raw_slot) in slots {
            if slot.is_empty() {
                return Err(format!("connector `{}`: 凭据槽位名不能为空", meta.id));
            }
            let into = match raw_slot.into.as_deref() {
                None => CredentialInto::Header(slot.clone()),
                Some(s) => parse_credential_into(s)
                    .ok_or_else(|| {
                        format!(
                            "connector `{}`: 凭据槽位 `{slot}` 的 into `{s}` 非法（应为 \"header:<名>\" 或 \"query:<名>\"）",
                            meta.id
                        )
                    })?,
            };
            // prefix 仅 header 形态消费；query 串带前缀几乎必是笔误，loud 拒。
            if raw_slot.prefix.is_some() && matches!(into, CredentialInto::Query(_)) {
                return Err(format!(
                    "connector `{meta}`: 凭据槽位 `{slot}` 的 prefix 只支持 header 注入形态",
                    meta = meta.id
                ));
            }
            if let Some(p) = &raw_slot.prefix {
                validate_header_value(p).map_err(|e| {
                    format!("connector `{}`: 凭据槽位 `{slot}` 的 prefix {e}", meta.id)
                })?;
            }
            credentials.insert(
                slot.clone(),
                CredentialBinding {
                    slot,
                    required: raw_slot.required,
                    into,
                    prefix: raw_slot.prefix,
                },
            );
        }
    }

    // 治理补全 P1（2026-10-19）：纯独占接管形态合法化——ops 为空但声明了
    // exclusive_mcp 时不再要求 [[op]]（镜像 op 就是它的操作面，B2 语义下
    // 「整体接管一个 server、一个 op 都不写」是主场景；此前被迫写假 op）。
    // 既无 op 又无独占 = 连接器没有任何可调用面，仍拒绝。
    if raw.ops.is_empty() && meta.exclusive_mcp.is_none() {
        return Err(format!(
            "connector `{}`: 至少要声明一个 [[op]]（或 exclusive_mcp 独占接管）",
            meta.id
        ));
    }

    // B2：独占声明归一（去空白、拒空项、去重保序）。server 名是否真实
    // 在册**不在此处校验**——独占声明可以走在装 MCP 之前（治理壳先行的
    // 合法形态），前缀匹配在运行期按当下注册真相计算，未注册 = 无工具
    // 可独占（无操作），不报错。
    let mut exclusive_mcp: Vec<String> = Vec::new();
    if let Some(spec) = meta.exclusive_mcp {
        let entries = match spec {
            ExclusiveMcpSpec::One(s) => vec![s],
            ExclusiveMcpSpec::Many(v) => v,
        };
        for e in entries {
            let e = e.trim().to_string();
            if e.is_empty() {
                return Err(format!(
                    "connector `{}`: exclusive_mcp 不能含空项（server 名不能为空白）",
                    meta.id
                ));
            }
            if !exclusive_mcp.contains(&e) {
                exclusive_mcp.push(e);
            }
        }
    }

    let mut ops = Vec::with_capacity(raw.ops.len());
    let mut seen = std::collections::BTreeSet::new();
    for op in raw.ops {
        validate_id(&op.id, "op id")?;
        if !seen.insert(op.id.clone()) {
            return Err(format!("connector `{}`: op id `{}` 重复", meta.id, op.id));
        }

        // kind↔绑定一致性 + 恰一绑定（M2 起 script 四选一）
        let http = op.http.is_some();
        let tool = op.tool.is_some();
        let action = op.action.is_some();
        let script = op.script.is_some();
        let binding_count =
            usize::from(http) + usize::from(tool) + usize::from(action) + usize::from(script);
        if binding_count == 0 {
            return Err(format!(
                "connector `{}` op `{}`: 缺执行绑定（http / tool / action / script 四选一）",
                meta.id, op.id
            ));
        }
        if binding_count > 1 {
            return Err(format!(
                "connector `{}` op `{}`: 执行绑定只能声明一个（http / tool / action / script）",
                meta.id, op.id
            ));
        }
        let binding = if let Some(h) = op.http {
            // kind 只是描述性分组（list 视图用）；执行形态按 op 绑定逐个决定
            // ——混合型 connector（如 paper-broker：行情走 http、下单走本地
            // 账本）是合法形态。唯一硬约束：http 绑定必须有 base。
            if base_url.is_none() {
                return Err(format!(
                    "connector `{}` op `{}`: rest 型缺 [connector.http].base",
                    meta.id, op.id
                ));
            }
            let placeholders = parse_path_placeholders(&h.path).map_err(|e| {
                format!(
                    "connector `{}` op `{}`: http path 非法: {e}",
                    meta.id, op.id
                )
            })?;
            OpBinding::Http {
                method: h.method,
                path: h.path,
                placeholders,
            }
        } else if let Some(t) = op.tool {
            if t.trim().is_empty() {
                return Err(format!(
                    "connector `{}` op `{}`: tool 绑定不能为空",
                    meta.id, op.id
                ));
            }
            OpBinding::Tool {
                tool: t.trim().to_string(),
            }
        } else if let Some(s) = op.script {
            validate_script_binding(&meta.id, &op.id, s, op.args_schema.as_ref())?
        } else {
            let a = op.action.expect("恰一绑定已保证");
            if a.trim().is_empty() {
                return Err(format!(
                    "connector `{}` op `{}`: action 绑定不能为空",
                    meta.id, op.id
                ));
            }
            OpBinding::Action {
                action: a.trim().to_string(),
            }
        };

        // confirm / budget 值域
        let confirm_required = match op.confirm.as_deref() {
            None => false,
            Some("required") => true,
            Some(other) => {
                return Err(format!(
                    "connector `{}` op `{}`: confirm 值 `{other}` 非法（只支持 \"required\"）",
                    meta.id, op.id
                ));
            }
        };
        let finance_budget = match op.budget.as_deref() {
            None => false,
            Some("finance") => true,
            Some(other) => {
                return Err(format!(
                    "connector `{}` op `{}`: budget 值 `{other}` 非法（只支持 \"finance\"）",
                    meta.id, op.id
                ));
            }
        };

        // R4：args_schema 形态扫描（加载期）——只认本方校验器支持的关键词，
        // 超出白名单 loud 拒收（诚实拒绝假装支持：装得进去 ≠ 校验得住）。
        validate_args_schema_shape(&meta.id, &op.id, op.args_schema.as_ref())?;

        ops.push(ValidatedOp {
            id: op.id,
            intent: op.intent,
            risk: op.risk,
            class: op.class,
            confirm_required,
            finance_budget,
            receipt_key: op.receipt,
            undo: op.undo.map(|u| OpUndo {
                op: u.op.trim().to_string(),
                arg: u.arg.trim().to_string(),
            }),
            binding,
            args_schema: op.args_schema,
        });
    }

    // E3（收官批）：undo 声明二次校验——引用完整性跨 op（前向引用合法：
    // cancel_order 可声明在 place_order 之前），需全部 op 收集后统一检。
    // 规则：① undo.op 存在于本 connector ② undo.op ≠ 自身 ③ 源 op 已声明
    // receipt（撤销靠凭条主键回填，无主键的 op 谈不上撤销）+ 附带守卫：
    // 目标 op 不得再声明 undo（撤销链禁止嵌套——「撤销撤销」无语义）。
    for v in &ops {
        let Some(u) = &v.undo else { continue };
        if u.op.is_empty() || u.arg.is_empty() {
            return Err(format!(
                "connector `{}` op `{}`: undo 的 op/arg 不能为空",
                meta.id, v.id
            ));
        }
        if u.op == v.id {
            return Err(format!(
                "connector `{}` op `{}`: undo.op 不能指向自身",
                meta.id, v.id
            ));
        }
        let target = ops.iter().find(|o| o.id == u.op).ok_or_else(|| {
            format!(
                "connector `{}` op `{}`: undo.op `{}` 未在本 connector 声明",
                meta.id, v.id, u.op
            )
        })?;
        if target.undo.is_some() {
            return Err(format!(
                "connector `{}` op `{}`: undo.op `{}` 自身也声明了 undo（撤销链禁止嵌套）",
                meta.id, v.id, u.op
            ));
        }
        if v.receipt_key.is_none() {
            return Err(format!(
                "connector `{}` op `{}`: 声明 undo 前必须声明 receipt（撤销凭条主键）",
                meta.id, v.id
            ));
        }
    }

    Ok(ValidatedConnector {
        id: meta.id,
        name: meta.name,
        version: meta.version,
        kind: meta.kind,
        enabled: meta.enabled,
        description: meta.description,
        base_url,
        static_headers,
        credentials,
        exclusive_mcp,
        ops,
    })
}

/// id / op id / 目录名字符集：`[a-z0-9_-]`，1..=64。附加 Windows 保留设备名
/// 拒绝（`con`/`nul`/`com1` 等——id 同时派生注册目录与 sidecar 文件名，保留
/// 名在 Windows 上连目录创建/文件写都会被 OS 拒且报错难懂；清单层诚实拒绝
/// 带原因。边界补强 2026-10-20，与 image_gen 路径保留名校验同族）。
pub(crate) fn validate_id(id: &str, what: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err(format!("{what} `{id}` 长度非法（1..=64）"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(format!(
            "{what} `{id}` 含非法字符（只允许小写字母/数字/-/_）"
        ));
    }
    const WINDOWS_RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    // 字符集无点 → 整段即基名（Windows 保留名按「首个点之前」判定；
    // `con-x`/`con_x` 不在保留集，精确比对不误伤）。
    if WINDOWS_RESERVED.contains(&id) {
        return Err(format!(
            "{what} `{id}` 使用了 Windows 保留设备名（目录/文件名在 Windows 上无法创建）"
        ));
    }
    Ok(())
}

/// args_schema 顶层白名单（R4 加载期 shape 扫描）：registry::validate_op_args
/// 消费的关键词全集。清单作者用了白名单外的 JSON Schema 关键词（pattern /
/// items / oneOf…）时**加载即拒**——运行期校验器不认它们，装作支持 =
/// 非法参数漏进出网。
const SCHEMA_OBJECT_KEYWORDS: &[&str] = &["type", "required", "properties"];
/// property 级白名单（description 是描述性文本，registry 校验器不消费但
/// 全仓清单大量使用，必须放行）。
const SCHEMA_PROPERTY_KEYWORDS: &[&str] = &["type", "enum", "description", "minimum", "maximum"];

/// 加载期 args_schema 形态扫描（R4）：顶层必须是 `type: "object"`；property
/// type 限 string|number|integer|boolean（array/object 本方校验器不递归，不
/// 放行）；required 必须是全字符串数组（registry 消费侧对形态坏**静默跳过**
/// ——`as_array()` None / `as_str()` None 双重兜空，required 闸整体失效）；
/// properties 缺席合法、**在场但非 object loud 拒**（同因：字段级校验整体
/// 失效，不能与「缺席 = 只有 required 闸」混为一谈）；enum 必须是字符串数
/// 组；minimum/maximum 必须是数字且**只允许声明在 number/integer 型
/// property 上**（registry 校验器对其他类型静默跳过边界，装作支持 = 非法
/// 参数漏进出网）；白名单外关键词 loud 报错。
fn validate_args_schema_shape(
    connector_id: &str,
    op_id: &str,
    schema: Option<&serde_json::Value>,
) -> Result<(), String> {
    let Some(schema) = schema else {
        return Ok(());
    };
    let err = |msg: String| -> String {
        format!("connector `{connector_id}` op `{op_id}`: args_schema 非法: {msg}")
    };
    let Some(obj) = schema.as_object() else {
        return Err(err("顶层必须是 object（JSON Schema 形态）".to_string()));
    };
    for key in obj.keys() {
        if !SCHEMA_OBJECT_KEYWORDS.contains(&key.as_str()) {
            return Err(err(format!(
                "不支持的关键词 `{key}`（本方校验器只认 {SCHEMA_OBJECT_KEYWORDS:?}；不支持的关键词装作支持会让非法参数漏过执行前校验）"
            )));
        }
    }
    match obj.get("type").and_then(|v| v.as_str()) {
        Some("object") => {}
        other => {
            return Err(err(format!(
                "顶层 type 必须是 \"object\"，实际为 {:?}",
                other.unwrap_or("(缺失)")
            )));
        }
    }
    // required 形态闸：必须是全字符串数组。registry 消费侧 `as_array()` /
    // `as_str()` 都是静默跳过——形态坏 = 必填闸无声失效，加载期 loud 拒。
    if let Some(required) = obj.get("required") {
        let Some(arr) = required.as_array() else {
            return Err(err(
                "required 必须是数组（非数组会让必填闸整体失效）".to_string()
            ));
        };
        if let Some(idx) = arr.iter().position(|v| !v.is_string()) {
            return Err(err(format!(
                "required 成员必须全为字符串，第 {idx} 项非字符串（非字符串成员会被运行期校验静默跳过）"
            )));
        }
    }
    // properties 缺席 = 只有 required 闸，形态合法；在场但非 object = 字段
    // 级校验（type/enum/min-max）整体静默失效，loud 拒——不与缺席混同。
    let properties = match obj.get("properties") {
        None => return Ok(()),
        Some(v) => v.as_object().ok_or_else(|| {
            err("properties 必须是 object（非 object 会让全部字段级校验静默失效）".to_string())
        })?,
    };
    for (name, spec) in properties {
        let Some(spec_obj) = spec.as_object() else {
            return Err(err(format!("property `{name}` 必须是 object")));
        };
        for key in spec_obj.keys() {
            if !SCHEMA_PROPERTY_KEYWORDS.contains(&key.as_str()) {
                return Err(err(format!(
                    "property `{name}` 不支持的关键词 `{key}`（只认 {SCHEMA_PROPERTY_KEYWORDS:?}）"
                )));
            }
        }
        match spec_obj.get("type").and_then(|v| v.as_str()) {
            Some("string" | "number" | "integer" | "boolean") => {}
            other => {
                return Err(err(format!(
                    "property `{name}` type 必须是 string|number|integer|boolean，实际为 {:?}（array/object 本方校验器不递归校验，不冒充支持）",
                    other.unwrap_or("(缺失)")
                )));
            }
        }
        if let Some(allowed) = spec_obj.get("enum") {
            if !allowed.is_array() {
                return Err(err(format!("property `{name}` enum 必须是数组")));
            }
            if spec_obj.get("type").and_then(|v| v.as_str()) == Some("string")
                && allowed.as_array().unwrap().iter().any(|v| !v.is_string())
            {
                return Err(err(format!(
                    "property `{name}` enum（string 型）成员必须全为字符串"
                )));
            }
        }
        for bound in ["minimum", "maximum"] {
            if let Some(v) = spec_obj.get(bound) {
                if !v.is_number() {
                    return Err(err(format!("property `{name}` {bound} 必须是数字")));
                }
                // 边界只对数值型有意义：registry 校验器只对 number/integer
                // 消费 min/max，string/boolean 上声明会被运行期静默跳过——
                // 同「装作支持」防线，加载期 loud 拒（2026-10-16 四轮复查）。
                if !matches!(
                    spec_obj.get("type").and_then(|v| v.as_str()),
                    Some("number" | "integer")
                ) {
                    return Err(err(format!(
                        "property `{name}` {bound} 只对 number/integer 型有意义（其他类型上会被运行期校验静默跳过）"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// 解析 `header:<名>` / `query:<名>`。
fn parse_credential_into(s: &str) -> Option<CredentialInto> {
    let (scheme, name) = s.split_once(':')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    match scheme {
        "header" => Some(CredentialInto::Header(name.to_string())),
        "query" => Some(CredentialInto::Query(name.to_string())),
        _ => None,
    }
}

/// 头名校验：非空、无空白/冒号/控制字符（HTTP 头名 token 字符集的务实
/// 子集——拼错的头名 loud 拒，不静默发坏请求）。
fn validate_header_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("名字不能为空".into());
    }
    if name
        .chars()
        .any(|c| c.is_whitespace() || c == ':' || c.is_control() || (c as u32) > 0x7e)
    {
        return Err("名字含非法字符（空白/冒号/控制字符/非 ASCII）".into());
    }
    Ok(())
}

/// 头值校验：禁止 CR/LF（头注入防线——静态头与凭据 prefix 都走这里，
/// 带换行的值可拆行注入任意头）。
fn validate_header_value(value: &str) -> Result<(), String> {
    if value.contains('\r') || value.contains('\n') {
        return Err("含换行（头注入防线：值必须是单行）".into());
    }
    Ok(())
}

/// 校验 http path 占位符语法，返回占位符名列表。
/// 合法形态：`{name}`（name 为 `[a-z0-9_]`，非空）；孤立 `}` 非法；孤立 `{`
/// 或空名/非法字符 loud 报错。
/// M2（2026-10-19）script 绑定校验：命令模板占位符语法 + 白名单 +
/// 声明式字段的边界（超时上限对齐 exec 工具闸；workdir 相对且禁越界；
/// capture 有界）。全部解析期 loud 拒——装得进去 ≠ 执行得住，假声明
/// （超 600 的 timeout、越界 workdir）不该活到运行期才炸。
fn validate_script_binding(
    connector_id: &str,
    op_id: &str,
    s: ScriptOpRaw,
    schema: Option<&serde_json::Value>,
) -> Result<OpBinding, String> {
    let err = |msg: String| -> String {
        format!("connector `{connector_id}` op `{op_id}`: [op.script] {msg}")
    };
    let command = s.command;
    if command.trim().is_empty() {
        return Err(err("command 不能为空".to_string()));
    }
    // 占位符语法（与 http path 同款解析器）+ 白名单：每个占位符必须被
    // args_schema.properties 声明——未声明的 `{xxx}` 解析期报错，不静默
    // 透传给 shell（提示词约定形态下模型自由发挥面大的根因就是它）。
    let placeholders =
        parse_path_placeholders(&command).map_err(|e| err(format!("command 占位符非法: {e}")))?;
    let properties = schema
        .and_then(|v| v.get("properties"))
        .and_then(|v| v.as_object());
    for ph in &placeholders {
        let declared = properties.is_some_and(|p| p.contains_key(ph.as_str()));
        if !declared {
            return Err(err(format!(
                "command 占位符 `{{{ph}}}` 未在 args_schema.properties 声明（占位符必须白名单化，未声明的占位符不透传给 shell）"
            )));
        }
    }
    // 超时：0 无意义（立即超时）；>600 是假承诺（exec 工具闸 cap 600）。
    if s.timeout_secs == Some(0) {
        return Err(err("timeout_secs 不能为 0（要禁 script op 用 config connectors.script.default_timeout_secs = 0）".to_string()));
    }
    if s.timeout_secs.is_some_and(|t| t > 600) {
        return Err(err(
            "timeout_secs 上限 600（exec 工具闸 600s，声明超出 = 假承诺）".to_string(),
        ));
    }
    // workdir：相对 workspace 根；绝对路径 / `..` 越界解析期拒。运行期
    // exec 工具的 restrict 边界照跑（纵深防御第二道）。
    if let Some(wd) = &s.workdir {
        let wd_trim = wd.trim();
        if wd_trim.is_empty() {
            return Err(err(
                "workdir 不能为空（要 workspace 根就删掉这个键）".to_string()
            ));
        }
        let p = std::path::Path::new(wd_trim);
        if p.is_absolute() || wd_trim.starts_with('~') {
            return Err(err(format!(
                "workdir `{wd_trim}` 必须是相对 workspace 根的路径（绝对路径/~ 开头都拒）"
            )));
        }
        if p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(err(format!(
                "workdir `{wd_trim}` 含 `..`（越界通道不开——声明式 workdir 不是沙箱旁路）"
            )));
        }
    }
    const MAX_CAPTURE: u32 = 65536;
    if s.capture.is_some_and(|c| c as u64 > MAX_CAPTURE as u64) {
        return Err(err(format!(
            "capture 上限 {MAX_CAPTURE} 字节（凭条是人看/卡面的，不是日志转储）"
        )));
    }
    Ok(OpBinding::Script {
        command,
        placeholders,
        timeout_secs: s.timeout_secs,
        workdir: s.workdir,
        capture: s.capture,
    })
}

fn parse_path_placeholders(path: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'}' => return Err("孤立的 `}`".into()),
            b'{' => {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j] != b'}' {
                    j += 1;
                }
                if j >= bytes.len() {
                    return Err("占位符缺少闭合 `}`".into());
                }
                let name = &path[start..j];
                if name.is_empty()
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                {
                    return Err(format!("占位符名 `{name}` 非法（只允许小写字母/数字/_）"));
                }
                out.push(name.to_string());
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
