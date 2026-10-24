//! 紫微动态工具桥（T2.2）：profile.tools[] → AgentLoop 工具注册表。
//!
//! 依赖方向（决议 8①）：nemesis-ziwei 不依赖 nemesis-agent——桥在本
//! crate，会话管理器经 `nemesis_ziwei::session::global()` 取（未装配 =
//! 诚实报错）。注册时机：gateway 装配 / agent 重建（factory 单点，每次
//! 重建幂等重跑）；新增 profile 后经 WSAPI `ziwei.refresh` 重注册。
//!
//! 安全投影（总控 §8）：子工具 `min_tier = Some("normal")`（供给+分发
//! 双闸，mini 档不可见）；`operation_override` 投 `ziwei` 操作类型
//! （OperationType::Ziwei = CRITICAL 档，8 层管线照跑）；`run_policy
//! = confirm` 投强审批卡（跳过「总是允许」规则表）。
//!
//! M2 范围：`poll_report` 工具实装（一次性快照读）；M4 起 `call_fn` /
//! `hook_capture` 实装——call_fn 整数参校验（T4.1）+ 看门狗超时；hook_capture
//! 全锚幂等订阅（T4.3/T4.6）+ 拉取解码回流 + output.dest 报表渲染（T4.5）。

use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::{AgentLoop, OperationOverride, Tool};
use nemesis_ziwei::profile::{Primitive, ProfileStore, RunPolicy, ToolDef};
use nemesis_ziwei::session::SessionManager;
use nemesis_ziwei::setup::{DISASM_MAX_INSNS, LIST_MAX_ROWS};

/// 动态工具名前缀（LLM 可见名 = `zw_{profile_id}_{tool_name}`，sanitize 后）。
pub const TOOL_NAME_PREFIX: &str = "zw_";

/// 当前主 loop 槽（agent 每次重建时更新；`ziwei.refresh` 钩子读它）。
static CURRENT_LOOP: std::sync::RwLock<Option<Arc<AgentLoop>>> = std::sync::RwLock::new(None);

/// gateway / 工厂每次建 loop 后登记（后到覆盖——重建后 refresh 落新 loop）。
pub fn set_current_loop(loop_arc: Arc<AgentLoop>) {
    *CURRENT_LOOP.write().unwrap() = Some(loop_arc);
}

/// refresh 钩子实现（nemesis-web 侧 `ziwei.refresh` 经 set_refresh_hook 调）。
pub fn refresh_current(workspace: &std::path::Path) -> usize {
    let Some(loop_arc) = CURRENT_LOOP.read().unwrap().clone() else {
        tracing::warn!("[ziwei] refresh：当前 loop 槽为空（gateway 未就绪）");
        return 0;
    };
    register_profile_tools(&loop_arc, workspace)
}

/// profile_id/tool_name → 合法工具名段（`[a-z0-9_]`，其余压 `_`）。
fn sanitize_segment(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if !out.starts_with(|c: char| c.is_ascii_lowercase()) {
        out.insert(0, 'z');
    }
    out.truncate(28); // `zw_`(3) + 段(28) + `_`(1) + 段(28) = 60，压在 64 内
    out
}

/// 动态工具 LLM 可见名。
pub fn dynamic_tool_name(profile_id: &str, tool_name: &str) -> String {
    format!(
        "{TOOL_NAME_PREFIX}{}_{}",
        sanitize_segment(profile_id),
        sanitize_segment(tool_name)
    )
}

/// 一个 profile 子工具的 agent 侧动态工具实例。
pub struct ZiweiDynamicTool {
    profile_id: String,
    profile_name: String,
    tool: ToolDef,
}

impl ZiweiDynamicTool {
    fn manager() -> Result<Arc<SessionManager>, String> {
        nemesis_ziwei::session::global()
            .ok_or_else(|| "紫微会话管理器未装配（gateway 未启用或非 Windows 平台）".to_string())
    }

    /// call_fn 执行体（T4.1）：统一实现下沉 runtime（与 WSAPI invoke 同源），
    /// 这里只做 addr 模板解析 + 入参 JSON + 工具身份装饰。
    #[cfg(windows)]
    async fn exec_call_fn(
        &self,
        manager: &Arc<SessionManager>,
        args: &str,
    ) -> Result<String, String> {
        let semantic = self.tool.action.addr.strip_prefix("fn:").ok_or_else(|| {
            format!(
                "工具 {} 地址形态非法（{}）：call_fn 须 fn:语义名",
                self.tool.tool_name, self.tool.action.addr
            )
        })?;
        let fn_args = nemesis_ziwei::runtime::parse_callfn_args(args)?;
        let timeout_ms = self.tool.action.timeout_ms.unwrap_or(5_000).min(60_000) as u32;
        let mut obj = nemesis_ziwei::runtime::invoke_call_fn(
            manager,
            manager.workspace(),
            &self.profile_id,
            semantic,
            fn_args,
            timeout_ms,
        )
        .await?;
        if let Some(o) = obj.as_object_mut() {
            o.insert("tool".into(), serde_json::json!(self.tool.tool_name));
            o.insert("profile".into(), serde_json::json!(self.profile_id));
        }
        Ok(serde_json::to_string(&obj).unwrap_or_default())
    }

    /// hook_capture 执行体（T4.3/T4.5/T4.6）：统一实现下沉 runtime（与
    /// WSAPI invoke 同源），这里只做 addr 模板解析 + 工具身份装饰。
    #[cfg(windows)]
    async fn exec_hook_capture(&self, manager: &Arc<SessionManager>) -> Result<String, String> {
        let semantic = self.tool.action.addr.strip_prefix("fn:").ok_or_else(|| {
            format!(
                "工具 {} 地址形态非法（{}）：hook_capture 须 fn:语义名",
                self.tool.tool_name, self.tool.action.addr
            )
        })?;
        let mut obj = nemesis_ziwei::runtime::invoke_hook_capture(
            manager,
            manager.workspace(),
            &self.profile_id,
            semantic,
            self.tool.output.as_ref().map(|o| o.dest.as_str()),
            self.tool.action.max_records.unwrap_or(200),
        )
        .await?;
        if let Some(o) = obj.as_object_mut() {
            o.insert("tool".into(), serde_json::json!(self.tool.tool_name));
            o.insert("profile".into(), serde_json::json!(self.profile_id));
        }
        Ok(serde_json::to_string(&obj).unwrap_or_default())
    }
}

#[async_trait::async_trait]
impl Tool for ZiweiDynamicTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let manager = Self::manager()?;
        match self.tool.action.primitive {
            Primitive::PollReport => {
                // 结构地址：会话注入映射（attach 时给；未解析 = 诚实报错）
                let session = manager.get(&self.profile_id).ok_or_else(|| {
                    format!(
                        "会话未附加（profile {}）：先经 ziwei.attach 附加目标进程",
                        self.profile_id
                    )
                })?;
                let ds_name = self
                    .tool
                    .action
                    .addr
                    .strip_prefix("struct:")
                    .ok_or_else(|| {
                        format!(
                            "工具 {} 地址形态非法（{}）",
                            self.tool.tool_name, self.tool.action.addr
                        )
                    })?;
                let addr = session.struct_addr(ds_name).unwrap_or(0);
                if addr == 0 {
                    return Err(format!(
                        "结构 {ds_name} 地址未解析（attach 未提供 struct_addrs；M3 起由锚自动解析）"
                    ));
                }
                // 结构定义从 profile 取（execute 时现读——profile 可能已被编辑）
                let store = ProfileStore::new(manager.workspace());
                let profile = store.load(&self.profile_id).map_err(|e| e.to_string())?;
                let ds = profile
                    .data_structures
                    .iter()
                    .find(|s| s.name == ds_name)
                    .ok_or_else(|| format!("结构 {ds_name} 定义不存在"))?;
                let snapshot = manager.read_struct(&self.profile_id, addr, ds).await?;
                Ok(serde_json::to_string(&serde_json::json!({
                    "tool": self.tool.tool_name,
                    "profile": self.profile_id,
                    "struct": ds_name,
                    "data": snapshot,
                }))
                .unwrap_or_default())
            }
            Primitive::CallFn => {
                #[cfg(windows)]
                {
                    self.exec_call_fn(&manager, args).await
                }
                #[cfg(not(windows))]
                {
                    let _ = (manager, args);
                    Err("紫微系统运行时原语仅支持 Windows 平台".to_string())
                }
            }
            Primitive::HookCapture => {
                #[cfg(windows)]
                {
                    self.exec_hook_capture(&manager).await
                }
                #[cfg(not(windows))]
                {
                    Err("紫微系统运行时原语仅支持 Windows 平台".to_string())
                }
            }
        }
    }

    fn description(&self) -> String {
        let kind = match self.tool.action.primitive {
            Primitive::PollReport => "读取",
            Primitive::CallFn => "调用",
            Primitive::HookCapture => "挂钩捕获",
        };
        format!(
            "【紫微系统】{}目标进程（{}）的结构化数据：{}",
            kind, self.profile_name, self.tool.description
        )
    }

    fn parameters(&self) -> serde_json::Value {
        if self.tool.action.primitive == Primitive::CallFn {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "array",
                        "items": {"type": "integer"},
                        "maxItems": 4,
                        "description": "整数位置参数（≤4；x64 寄存器序 rcx/rdx/r8/r9，x86 栈序）。浮点/字符串参数 v1 不支持",
                    },
                },
                "additionalProperties": false,
            })
        } else {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            })
        }
    }

    /// 子工具最低调用档：normal（总控 §8——ziwei 工具不给 mini 档）。
    fn min_tier(&self) -> Option<&str> {
        Some("normal")
    }

    fn is_read_only(&self) -> bool {
        // poll_report 一次性快照读 = 纯读（可进并行池）；call_fn/hook_capture
        // 是写面（M4 实装时仍 false）。
        self.tool.action.primitive == Primitive::PollReport
    }

    /// 安全投影（W5 模式）：ziwei 操作类型 + confirm 强审批。
    fn operation_override(&self, _args: &str) -> Option<OperationOverride> {
        let force_approval = match self.tool.run_policy {
            RunPolicy::Confirm => Some(format!(
                "紫微系统操作需人工确认（profile={} tool={}）",
                self.profile_id, self.tool.tool_name
            )),
            RunPolicy::Auto => None,
        };
        Some(OperationOverride {
            op_type: "ziwei".to_string(),
            force_approval,
            receipt_ctx: None,
        })
    }
}

/// 把 workspace 全部 profile 的工具注册到 loop（gateway 装配 / agent 重建
/// / `ziwei.refresh` 同源入口）。返回注册数。幂等：先清本桥旧注册再装
/// （防 refresh 重名 _2 污染）。
pub fn register_profile_tools(agent_loop: &Arc<AgentLoop>, workspace: &std::path::Path) -> usize {
    let _ = remove_profile_tools(agent_loop);
    let store = ProfileStore::new(workspace);
    let Ok(profiles) = store.list() else {
        return 0;
    };
    let mut count = 0usize;
    for entry in &profiles {
        // T2.2 注销语义：只有 active 入库注册——stale/revoked 在 refresh 的
        // remove-first 前置下自然注销（不重注册）。
        if entry.status != "active" {
            continue;
        }
        let profile = match store.load(&entry.profile_id) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    "[ziwei] profile {} 加载失败，跳过工具注册：{e}",
                    entry.profile_id
                );
                continue;
            }
        };
        for tool in &profile.tools {
            let name = dynamic_tool_name(&profile.profile_id, &tool.tool_name);
            let dyn_tool = Arc::new(ZiweiDynamicTool {
                profile_id: profile.profile_id.clone(),
                profile_name: profile.name.clone(),
                tool: tool.clone(),
            });
            let final_name = agent_loop.register_plugin_tool(name.clone(), dyn_tool);
            // 安全管线 declare（W5 配套）：ziwei 操作类型（CRITICAL 档）。
            #[cfg(feature = "security")]
            nemesis_security::types::declare_tool_operation(
                &final_name,
                nemesis_security::types::OperationType::Ziwei,
            );
            count += 1;
        }
    }
    if count > 0 {
        tracing::info!("[ziwei] 动态工具注册完成：{count} 个（workspace profiles）");
    }
    count
}

/// 移除本桥注册的全部 zw_* 工具（refresh 幂等前置 + 兜底清理）。
/// 返回移除数。只动 `zw_` 前缀——不碰其他工具。
pub fn remove_profile_tools(agent_loop: &Arc<AgentLoop>) -> usize {
    let names: Vec<String> = agent_loop
        .tool_names()
        .into_iter()
        .filter(|n| n.starts_with(TOOL_NAME_PREFIX))
        .collect();
    let mut removed = 0;
    for n in names {
        if agent_loop.remove_plugin_tool(&n) {
            #[cfg(feature = "security")]
            nemesis_security::types::undeclare_tool_operation(&n);
            removed += 1;
        }
    }
    removed
}

// ── ziwei_bridge 配置期向导元工具（T3.8）────────────────────────────────

/// 配置期向导元工具名（唯一、常驻注册）。
pub const BRIDGE_TOOL_NAME: &str = "ziwei_bridge";

/// LLM 经 `{action, params}` 驱动 SetupManager 的单一入口——与 WSAPI
/// `setup.*` / CLI `ziwei setup <step>` 三方同源（同一 global() 实例、
/// 同一校验面）。人生节点（select_target/commit/abort）强制审批卡。
pub struct ZiweiBridgeTool {
    workspace: std::path::PathBuf,
}

/// 查询入参取字符串字段（非字符串 = None；容错 LLM 数字形态的 value/query）。
fn param_str<'a>(params: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

/// 查询入参取 rva 字段原始值（缺席/类型怪 = 带字段名的诚实报错）。
fn rva_arg(params: &serde_json::Value) -> Result<&serde_json::Value, String> {
    params
        .get("rva")
        .ok_or_else(|| "缺 rva（十六进制或数值）".to_string())
}

/// 查询面 RVA 入参归一：Number 直取（≤u32 校验）；String 走 parse_hex_u32
/// 容错（`0x` 前缀可选）——与 manager 层同一真相源。
fn parse_rva_param(v: &serde_json::Value) -> Result<u32, String> {
    match v {
        serde_json::Value::Number(n) => n
            .as_u64()
            .filter(|x| *x <= u32::MAX as u64)
            .map(|x| x as u32)
            .ok_or_else(|| format!("rva {v} 超 u32 范围")),
        serde_json::Value::String(s) => nemesis_ziwei::setup::parse_hex_u32(s)
            .ok_or_else(|| format!("rva {s:?} 非法（十六进制，0x 前缀可选）")),
        other => Err(format!("rva 形态 {other:?} 非法（数值或十六进制字符串）")),
    }
}

impl ZiweiBridgeTool {
    /// 需要 force_approval 的写人生动作（approve 一次只放行本次调用）。
    fn approval_gate(action: &str) -> Option<String> {
        match action {
            "select_target" | "commit" | "abort" => Some(format!(
                "紫微系统配置向导「{action}」需人工确认（目标选取/配置固化/作废）"
            )),
            _ => None,
        }
    }
}

#[async_trait::async_trait]
impl Tool for ZiweiBridgeTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("参数非合法 JSON：{e}"))?;
        let action = v
            .get("action")
            .and_then(|a| a.as_str())
            .ok_or("缺 action（字符串）")?;
        let params = v.get("params").cloned().unwrap_or(serde_json::json!({}));
        if !params.is_object() {
            return Err("params 必须是对象".to_string());
        }
        let mgr = nemesis_ziwei::setup::global();
        let ws = self.workspace.as_path();
        let out = match action {
            "status" => mgr.status(ws).ok_or_else(|| {
                "无活跃向导（status 只报活跃 draft；磁盘残留见 select_target 报错）".to_string()
            })?,
            "select_target" => {
                mgr.select_target(
                    ws,
                    params.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32),
                    params.get("exe").and_then(|e| e.as_str()),
                )
                .await?
            }
            "snapshot" => mgr.snapshot(ws).await?,
            "analyze" => mgr.analyze(ws)?,
            "diff_window" => {
                mgr.diff_window(
                    ws,
                    params.get("wait_secs").and_then(|w| w.as_u64()),
                    params.get("hook").and_then(|h| h.as_bool()),
                )
                .await?
            }
            "candidates" => mgr.candidates(ws)?,
            "annotate" => mgr.annotate(ws, &params)?,
            "define_struct" => mgr.define_struct(ws, &params)?,
            "define_tool" => mgr.define_tool(ws, &params)?,
            "commit" => {
                let out = mgr.commit(ws)?;
                // 新 profile 动态工具即时上线（同 ziwei.refresh 钩子）
                let n = refresh_current(ws);
                let mut obj = out;
                if let Some(o) = obj.as_object_mut() {
                    o.insert("tools_registered".into(), serde_json::json!(n));
                }
                obj
            }
            "abort" => mgr.abort(ws).await?,
            // ── 查询面（B1）：只读、无审批、有界返回；地址十六进制 0x 可选 ──
            "disasm" => {
                let rva = parse_rva_param(rva_arg(&params)?)?;
                let max_insns = params
                    .get("max_insns")
                    .and_then(|m| m.as_u64())
                    .unwrap_or(DISASM_MAX_INSNS as u64) as usize;
                mgr.query_disasm(ws, rva, max_insns)?
            }
            "xref_to" => {
                let kind = param_str(&params, "kind").ok_or("缺 kind（str/imm/data）")?;
                let value = param_str(&params, "value").ok_or("缺 value")?;
                mgr.query_xref_to(ws, kind, value)?
            }
            "xref_from" => {
                let rva = parse_rva_param(rva_arg(&params)?)?;
                mgr.query_xref_from(ws, rva)?
            }
            "callers" => {
                let rva = parse_rva_param(rva_arg(&params)?)?;
                mgr.query_callers(ws, rva)?
            }
            "search" => {
                let kind =
                    param_str(&params, "kind").ok_or("缺 kind（bytes/string/imm/name/insn）")?;
                let query = param_str(&params, "query").ok_or("缺 query")?;
                mgr.query_search(ws, kind, query)?
            }
            "read_data" => {
                let rva = parse_rva_param(rva_arg(&params)?)?;
                let size = params.get("size").and_then(|s| s.as_u64()).unwrap_or(256) as usize;
                let as_utf8 = param_str(&params, "as") == Some("utf8");
                mgr.query_read_data(ws, rva, size, as_utf8)?
            }
            "pe" => {
                let what = param_str(&params, "what")
                    .ok_or("缺 what（sections/exports/imports/pdata/tls/debug/rich）")?;
                mgr.query_pe(ws, what)?
            }
            "locate" => {
                let rva = parse_rva_param(rva_arg(&params)?)?;
                mgr.query_locate(ws, rva)?
            }
            "functions" => {
                let sort = param_str(&params, "sort").unwrap_or("size");
                let top = params
                    .get("top")
                    .and_then(|t| t.as_u64())
                    .unwrap_or(LIST_MAX_ROWS as u64) as usize;
                let name = param_str(&params, "name");
                mgr.query_functions(ws, sort, top, name)?
            }
            "gaps" => mgr.query_gaps(ws)?,
            other => {
                return Err(format!(
                    "未知 action「{other}」；合法（向导 11）：status/select_target/snapshot/analyze/\
                     diff_window/candidates/annotate/define_struct/define_tool/commit/abort；\
                     （查询 10）：disasm/xref_to/xref_from/callers/search/read_data/pe/locate/functions/gaps"
                ));
            }
        };
        Ok(serde_json::to_string(&out).unwrap_or_default())
    }

    fn description(&self) -> String {
        "【紫微系统】配置期向导：对目标进程做 快照→分析→候选→标注→定义结构/工具→\
         固化配置 的完整流程（一次调用执行一步，按 action 分发）。"
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["status", "select_target", "snapshot", "analyze", "diff_window",
                             "candidates", "annotate", "define_struct", "define_tool",
                             "commit", "abort",
                             "disasm", "xref_to", "xref_from", "callers", "search",
                             "read_data", "pe", "locate", "functions", "gaps"],
                    "description": "向导步骤（前 11 个）：select_target → snapshot → analyze \
                                    → candidates → annotate*(≥1锚/函数) → define_struct? \
                                    → define_tool+ → commit（人生节点 select_target/commit/abort \
                                    会弹审批卡）。查询步骤（后 10 个，只读无审批）：静态分析投影，\
                                    供标注前定位/取证",
                },
                "params": {
                    "type": "object",
                    "description": "向导入参：select_target: {pid}|{exe}; diff_window: {wait_secs?, hook?（观测窗并入临时 hook 命中计数）}; \
                                    annotate: {rva, semantic, anchors:[{kind,pattern|imm|...}], conv?, note?, params?:[{reg,ty?,desc?}]（参数签名，ty 为 FieldType 字面量如 u64/char[32]）}; \
                                    define_struct: {name, size, fields:[{offset,name,type}]}; \
                                    define_tool: {tool_name, description, action:{primitive,addr,mode?,...}}; \
                                    其余向导步骤无参数。\
                                    查询入参（地址一律十六进制，0x 前缀可选）：\
                                    disasm: {rva, max_insns?≤512}; xref_to: {kind:str|imm|data, value}; \
                                    xref_from/callers/locate: {rva}; search: {kind:bytes|string|imm|name|insn, query}; \
                                    read_data: {rva, size?≤4096, as?:hex|utf8}; pe: {what:sections|exports|imports|pdata|tls|debug|rich}; \
                                    functions: {sort?:size|refs|imports, top?≤64, name?}; gaps: {}",
                },
            },
            "additionalProperties": false,
        })
    }

    fn min_tier(&self) -> Option<&str> {
        Some("normal")
    }

    fn is_read_only(&self) -> bool {
        false // 桥含写人生动作（select_target/commit/abort）
    }

    fn operation_override(&self, args: &str) -> Option<OperationOverride> {
        let action = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|v| v.get("action").and_then(|a| a.as_str()).map(str::to_owned))
            .unwrap_or_default();
        Some(OperationOverride {
            op_type: "ziwei".to_string(),
            force_approval: Self::approval_gate(&action),
            receipt_ctx: None,
        })
    }
}

/// 把桥元工具注册进 loop（agent_factory ziwei 启用块单点调用）。
/// 幂等：register_plugin_tool 同名覆盖。返回是否新注册。
pub fn register_bridge_tool(agent_loop: &Arc<AgentLoop>, workspace: &std::path::Path) -> bool {
    let name = agent_loop.register_plugin_tool(
        BRIDGE_TOOL_NAME.to_string(),
        Arc::new(ZiweiBridgeTool {
            workspace: workspace.to_path_buf(),
        }),
    );
    #[cfg(feature = "security")]
    nemesis_security::types::declare_tool_operation(
        &name,
        nemesis_security::types::OperationType::Ziwei,
    );
    true
}

#[cfg(all(test, feature = "ziwei"))]
mod tests;
