//! profile.json schema v1 类型（总控 §6.2）。
//!
//! 纪律：
//! - **profile 是数据不是脚本**：`tools[].action` 只能引用 §5 原语与参数模板；
//!   任何未知字段 / 未知枚举值 serde 直接报错（loud 拒绝，与 args_validator
//!   「未知 loud 拒绝」同哲学）。
//! - 所有 struct `deny_unknown_fields`；所有字符串枚举无 `Other` 兜底——
//!   拼错字段名 / 拼错枚举值在加载期就炸，不静默吞。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 当前 schema 版本。加载时严格等于本值，否则 loud 拒绝（不做静默迁移）。
pub const SCHEMA_VERSION: u32 = 1;

/// 逐层 hook 收敛链长上限（决议 5 / §7.5：超限标注期诚实拒绝，转人工）。
pub const CHAIN_CAP: usize = 4;

/// profile 根对象。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema_version: u32,
    pub profile_id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_verified_at: DateTime<Utc>,
    /// active（可用）/ stale（目标已更新待找回）/ revoked（用户吊销）。
    pub status: ProfileStatus,
    pub target: TargetFingerprint,
    #[serde(default)]
    pub attach: AttachInfo,
    #[serde(default)]
    pub functions: Vec<FunctionDef>,
    #[serde(default)]
    pub data_structures: Vec<DataStructure>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    pub verification: Verification,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    Active,
    Stale,
    Revoked,
}

impl ProfileStatus {
    /// 状态机（T5.1）：合法转移 = Active→Stale / Stale→Active /
    /// {Active,Stale}→Revoked；同态 no-op；Revoked→* loud 拒绝（吊销不可
    /// 自动复活，只能重建/删除 profile）。
    pub fn can_transition_to(self, to: ProfileStatus) -> Result<(), String> {
        if self == to {
            return Ok(());
        }
        match (self, to) {
            (Self::Active, Self::Active) | (Self::Stale, Self::Stale) => Ok(()), // 上方同态早退的穷尽性补全
            (Self::Active, Self::Stale)
            | (Self::Stale, Self::Active)
            | (Self::Active, Self::Revoked)
            | (Self::Stale, Self::Revoked) => Ok(()),
            (Self::Revoked, _) => {
                Err("profile 已吊销（revoked 不可自动复活——重建或删除后重配）".into())
            }
        }
    }
}

/// 目标指纹：附加时逐项核对（§6.4 验证链第一环）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TargetFingerprint {
    pub exe_name: String,
    #[serde(default)]
    pub exe_path_hint: Option<String>,
    pub machine: Machine,
    #[serde(default)]
    pub file_sha256: Option<String>,
    /// VERSIONINFO 资源抽取（ProductName / FileVersion …）；hash 不符时的
    /// 降级比对来源。
    #[serde(default)]
    pub version_resource: std::collections::BTreeMap<String, String>,
    /// hash 不符再退 version 比对（v1 唯一策略，钉死枚举）。
    pub verify_policy: VerifyPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Machine {
    X64,
    X86,
}

impl Machine {
    /// payload DLL 文件名后缀（`ziwei_agent_x64.dll` / `ziwei_agent_x86.dll`）。
    pub fn payload_suffix(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::X86 => "x86",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerifyPolicy {
    HashOrVersion,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AttachInfo {
    /// attach（附加常驻）/ launch（拉起受控）。
    pub mode: String,
    #[serde(default)]
    pub note: Option<String>,
}

impl Default for AttachInfo {
    fn default() -> Self {
        Self {
            mode: "attach".to_string(),
            note: None,
        }
    }
}

/// 已标注函数（locate = 当初怎么找到它）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FunctionDef {
    /// 语义名（用户标注，如「数据推送（绘图前最后一次经过）」）；锚与
    /// call_fn 地址模板都按它引用。
    pub semantic: String,
    pub locate: LocateSpec,
    /// 调用约定。
    pub conv: Conv,
    #[serde(default)]
    pub args: Vec<ArgSpec>,
    #[serde(default)]
    pub capture: Option<CaptureSpec>,
}

/// 函数定位描述（v1：byte_pattern 是唯一已实现 kind；其余 kind 枚举在位，
/// M3/M5 找回流程消费——未知值在 serde 层即被拒绝）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocateSpec {
    pub kind: LocateKind,
    /// 所属模块名（通常主 exe）。
    #[serde(default)]
    pub module: Option<String>,
    /// byte_pattern：十六进制字节串，`??` 为通配（如 "48 89 5C 24 ?? 57"）。
    #[serde(default)]
    pub pattern: Option<String>,
    /// addr：绝对地址定位（RVA 形态 "0x…"；不稳定布局不推荐，找回流程不保）。
    #[serde(default)]
    pub rva: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocateKind {
    BytePattern,
    Rva,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Conv {
    /// x64 微软约定。
    MsX64,
    /// x86 stdcall（被调方清栈——Win32 API 形态）。
    Stdcall,
    /// x86 cdecl（调用方清栈）。
    Cdecl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArgSpec {
    /// 寄存器名（x64：rcx/rdx/r8/r9；x86：栈槽序号 "stack:0"）。
    pub reg: String,
    #[serde(default)]
    pub desc: Option<String>,
}

/// 捕获描述：从该函数截获什么、怎么解码、上行走哪条通道。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureSpec {
    /// 解码模板，如 "struct:DataRecord"（引用 data_structures[].name）。
    #[serde(default)]
    pub decode: Option<String>,
    /// 上行通道（v1 唯一值 hook_capture）。
    #[serde(default)]
    pub emit: Option<EmitSink>,
    /// 逐层 hook 收敛链（§7.5）：稳定锚点起逐层推进到数据产生点。
    /// 链长上限 [`CHAIN_CAP`]，`Profile::validate` 强制。
    #[serde(default)]
    pub chain: Vec<ChainStep>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmitSink {
    HookCapture,
}

/// 链上单步：在哪 hook + 本层观测怎么解码。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChainStep {
    pub locate: LocateSpec,
    #[serde(default)]
    pub decode: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// 数据结构布局（profile 的解码真相源）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DataStructure {
    pub name: String,
    /// 字节总大小（字段越界校验基准）。
    pub size: u32,
    #[serde(default)]
    pub fields: Vec<FieldDef>,
}

/// 字段类型：字符串形态（"u64" / "f64" / "char[32]" / "u32[16]"），解析后
/// 内部为基类型 + 可选数组长度。未知基类型 loud 拒绝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldType {
    pub base: FieldBase,
    pub array_len: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldBase {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Bool,
    /// 单字节字符（char[N] = 定长 ASCII 串）。
    Char,
    /// UTF-16 码元（wchar[N] = Windows 宽字符串）。
    WChar,
}

impl FieldType {
    /// 单元素字节数（数组整体大小 = size_of * array_len）。
    pub fn size_of(&self) -> u32 {
        let base = match self.base {
            FieldBase::U8 | FieldBase::I8 | FieldBase::Char | FieldBase::Bool => 1,
            FieldBase::U16 | FieldBase::I16 | FieldBase::WChar => 2,
            FieldBase::U32 | FieldBase::I32 | FieldBase::F32 => 4,
            FieldBase::U64 | FieldBase::I64 | FieldBase::F64 => 8,
        };
        base * self.array_len.unwrap_or(1)
    }
}

impl serde::Serialize for FieldType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let base = match self.base {
            FieldBase::U8 => "u8",
            FieldBase::U16 => "u16",
            FieldBase::U32 => "u32",
            FieldBase::U64 => "u64",
            FieldBase::I8 => "i8",
            FieldBase::I16 => "i16",
            FieldBase::I32 => "i32",
            FieldBase::I64 => "i64",
            FieldBase::F32 => "f32",
            FieldBase::F64 => "f64",
            FieldBase::Bool => "bool",
            FieldBase::Char => "char",
            FieldBase::WChar => "wchar",
        };
        match self.array_len {
            Some(n) => s.serialize_str(&format!("{base}[{n}]")),
            None => s.serialize_str(base),
        }
    }
}

impl<'de> Deserialize<'de> for FieldType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom(format!("未知字段类型: {raw:?}")))
    }
}

impl FieldType {
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        let (base_str, array_len) = if let Some(close) = raw.rfind(']') {
            if !raw.ends_with(']') {
                return None;
            }
            let open = raw[..close].rfind('[')?;
            let n: u32 = raw[open + 1..close].parse().ok()?;
            if n == 0 {
                return None;
            }
            (&raw[..open], Some(n))
        } else {
            (raw, None)
        };
        let base = match base_str {
            "u8" => FieldBase::U8,
            "u16" => FieldBase::U16,
            "u32" => FieldBase::U32,
            "u64" => FieldBase::U64,
            "i8" => FieldBase::I8,
            "i16" => FieldBase::I16,
            "i32" => FieldBase::I32,
            "i64" => FieldBase::I64,
            "f32" => FieldBase::F32,
            "f64" => FieldBase::F64,
            "bool" => FieldBase::Bool,
            "char" => FieldBase::Char,
            "wchar" => FieldBase::WChar,
            _ => return None,
        };
        Some(Self { base, array_len })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FieldDef {
    pub offset: u32,
    pub name: String,
    /// JSON 键保持 `"type"`（对齐总控 §6.2 schema 草案）；`type_` 只是
    /// Rust 关键字回避。
    #[serde(rename = "type")]
    pub type_: FieldType,
    /// 解码语义（utf8 / unix_ms / unix_sec / hex / utf16）。
    #[serde(default)]
    pub decode: Option<FieldDecode>,
}

/// 字段解码语义标注。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldDecode {
    Utf8,
    Utf16,
    UnixMs,
    UnixSec,
    Hex,
}

/// 固化出的子工具（"万物皆工具"的落地单元）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ToolDef {
    /// 动态注册进工具注册表的名字（`^[a-z][a-z0-9_]{2,63}$`，validate 强制）。
    pub tool_name: String,
    pub description: String,
    /// JSON Schema（对 agent 投影；结构自由，v1 不做 schema-of-schema 校验）。
    #[serde(default)]
    pub input_schema: serde_json::Value,
    pub action: ToolAction,
    #[serde(default)]
    pub output: Option<OutputSpec>,
    /// auto（自动放行）/ confirm（每次审批卡——call_fn 默认值，决议 1）。
    pub run_policy: RunPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunPolicy {
    Auto,
    Confirm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ToolAction {
    /// 只能是 §5 原语（未知原语 loud 拒绝——profile 是数据不是脚本）。
    pub primitive: Primitive,
    /// 地址模板："struct:Name"（结构驻留）/ "fn:语义名"（已标注函数）。
    pub addr: String,
    /// poll 模式（on_change / every）。
    #[serde(default)]
    pub mode: Option<PollMode>,
    /// poll 间隔秒（bot 侧调度；payload 无状态）。
    #[serde(default)]
    pub interval_secs: Option<u64>,
    #[serde(default)]
    pub max_records: Option<u32>,
    /// call_fn 超时毫秒（看门狗；缺省 5000）。
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// 报警条件（最小 DSL，M2 实现；这里只做非空 + 字符集校验）。
    #[serde(default)]
    pub notify: Option<NotifyRule>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Primitive {
    PollReport,
    CallFn,
    HookCapture,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PollMode {
    OnChange,
    Every,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NotifyRule {
    /// 条件表达式，如 "state==ALERT" / "value>90"。v1 DSL：字段名 + 比较符
    /// （==/!=/>/>=/</<=）+ 字面量（数或短标识符）。语法校验在这里，
    /// 求值在 PollScheduler（M2）。
    pub on: String,
    /// 出站通道（缺省 = 默认出站）。
    #[serde(default)]
    pub channel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OutputSpec {
    /// 相对路径（钉死 workspace 内，拒 `..`/绝对路径——validate 强制）。
    pub dest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    /// 附加时验证项，如 ["file_sha256", "anchors_vote>=2"]。
    pub on_attach: Vec<String>,
    /// 失效姿态（v1 唯一值 notify_user）。
    #[serde(default)]
    pub stale_action: Option<String>,
}

impl Verification {
    /// 从 on_attach 抽锚投票阈值（"anchors_vote>=N"；缺省/未配置 = 2）。
    pub fn anchors_vote_threshold(&self) -> u32 {
        for entry in &self.on_attach {
            if let Some(rest) = entry.strip_prefix("anchors_vote>=")
                && let Ok(n) = rest.trim().parse::<u32>()
                && n >= 1
            {
                return n;
            }
        }
        2
    }
}

impl Profile {
    /// 保存前结构校验。serde 管字段名/枚举值；这里管跨字段语义：
    /// - schema_version 必须 == [`SCHEMA_VERSION`]
    /// - tool_name 形态 / 唯一性
    /// - action.addr 引用的 struct:/fn: 目标存在
    /// - capture.decode 引用的结构存在；chain 长度 ≤ [`CHAIN_CAP`]
    /// - 字段偏移不越结构大小
    /// - output.dest 相对路径（拒 `..` / 盘符 / 绝对路径）
    /// - notify.on 条件 DSL 语法
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "schema_version={} 不受支持（本版本只认 {}）",
                self.schema_version, SCHEMA_VERSION
            ));
        }
        if !self.profile_id.starts_with("zw-pro-") {
            return Err(format!(
                "profile_id {:?} 形态非法（须 zw-pro- 前缀）",
                self.profile_id
            ));
        }
        if self.name.trim().is_empty() {
            return Err("profile name 不得为空".to_string());
        }

        let struct_names: std::collections::HashSet<&str> = self
            .data_structures
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        // 结构自身校验：字段越界 + 偏移重叠是数据错误（重叠在标注期人工裁量，
        // 这里只拦越界——union 语义 v1 不支持）。
        for ds in &self.data_structures {
            if ds.size == 0 {
                return Err(format!("结构 {} size=0 非法", ds.name));
            }
            for f in &ds.fields {
                let end = f
                    .offset
                    .checked_add(f.type_.size_of())
                    .ok_or_else(|| format!("结构 {} 字段 {} 偏移溢出", ds.name, f.name))?;
                if end > ds.size {
                    return Err(format!(
                        "结构 {} 字段 {} 越界（offset {} + size {} > struct size {}）",
                        ds.name,
                        f.name,
                        f.offset,
                        f.type_.size_of(),
                        ds.size
                    ));
                }
            }
        }

        let fn_names: std::collections::HashSet<&str> =
            self.functions.iter().map(|f| f.semantic.as_str()).collect();

        let mut seen_tools = std::collections::HashSet::new();
        for tool in &self.tools {
            if !is_valid_tool_name(&tool.tool_name) {
                return Err(format!(
                    "tool_name {:?} 非法（须 ^[a-z][a-z0-9_]{{2,63}}$）",
                    tool.tool_name
                ));
            }
            if !seen_tools.insert(tool.tool_name.as_str()) {
                return Err(format!("tool_name {:?} 重复", tool.tool_name));
            }
            validate_action(&tool.action, &struct_names, &fn_names)?;
            if let Some(out) = &tool.output {
                validate_rel_dest(&out.dest)?;
            }
            // 决议 1：call_fn 类子工具默认 confirm——profile 内写 auto 是用户
            // 固化时显式选择，尊重；这里不强制改写。
        }

        for f in &self.functions {
            if let Some(cap) = &f.capture {
                if let Some(decode) = &cap.decode {
                    validate_struct_ref(decode, &struct_names)?;
                }
                if cap.chain.len() > CHAIN_CAP {
                    return Err(format!(
                        "函数 {} capture.chain 长度 {} 超上限 {}（该目标不适合 v1，转人工评估）",
                        f.semantic,
                        cap.chain.len(),
                        CHAIN_CAP
                    ));
                }
            }
            validate_locate(&f.locate)?;
            for step in f
                .capture
                .as_ref()
                .map(|c| c.chain.iter())
                .unwrap_or_default()
            {
                validate_locate(&step.locate)?;
            }
        }

        if let Some(action) = self.verification.stale_action.as_deref()
            && action != "notify_user"
        {
            return Err(format!(
                "stale_action {action:?} 不受支持（只认 notify_user）"
            ));
        }
        Ok(())
    }
}

fn validate_action(
    action: &ToolAction,
    structs: &std::collections::HashSet<&str>,
    fns: &std::collections::HashSet<&str>,
) -> Result<(), String> {
    match action.primitive {
        Primitive::PollReport => {
            // addr 必须是 struct: 引用（poll 读的是结构驻留）。
            validate_struct_ref(&action.addr, structs)?;
        }
        Primitive::CallFn => {
            // addr 必须是 fn: 引用（调用已标注函数）。
            let target = action
                .addr
                .strip_prefix("fn:")
                .ok_or_else(|| format!("call_fn addr {:?} 须为 fn:语义名 形态", action.addr))?;
            if !fns.contains(target) {
                return Err(format!("call_fn 引用的函数 {target:?} 不在 functions 中"));
            }
        }
        Primitive::HookCapture => {
            // addr 须 fn:（hook 已标注函数）或 chain 引用。
            let target = action.addr.strip_prefix("fn:").ok_or_else(|| {
                format!("hook_capture addr {:?} 须为 fn:语义名 形态", action.addr)
            })?;
            if !fns.contains(target) {
                return Err(format!(
                    "hook_capture 引用的函数 {target:?} 不在 functions 中"
                ));
            }
        }
    }
    if let Some(notify) = &action.notify {
        validate_notify_expr(&notify.on)?;
    }
    Ok(())
}

fn validate_struct_ref(
    addr: &str,
    structs: &std::collections::HashSet<&str>,
) -> Result<(), String> {
    let name = addr
        .strip_prefix("struct:")
        .ok_or_else(|| format!("addr {addr:?} 须为 struct:结构名 形态"))?;
    if !structs.contains(name) {
        return Err(format!("引用的结构 {name:?} 不在 data_structures 中"));
    }
    Ok(())
}

fn validate_locate(loc: &LocateSpec) -> Result<(), String> {
    match loc.kind {
        LocateKind::BytePattern => {
            let pat = loc
                .pattern
                .as_deref()
                .ok_or_else(|| "byte_pattern 缺 pattern".to_string())?;
            parse_byte_pattern(pat).map_err(|e| format!("byte_pattern {pat:?} 非法: {e}"))?;
        }
        LocateKind::Rva => {
            if loc.rva.is_none() {
                return Err("rva 定位缺 rva 值".to_string());
            }
        }
    }
    Ok(())
}

/// 解析十六进制字节模式（"48 89 5C 24 ?? 57"；`??` 或 `?` 通配）。返回
/// `(字节, 掩码)` 对——1 = 须精确匹配，0 = 通配。
/// 连续形态（"48895C24"，偶数长纯 hex）同样接受——candidates 的 payload
/// 与 LLM 应答都是无空格形态，空格分词只是人读习惯。
pub fn parse_byte_pattern(pat: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut bytes = Vec::new();
    let mut mask = Vec::new();
    let trimmed = pat.trim();
    // 连续形态：非空、全 hex、偶数长（无空白/通配）→ 按字节对直切
    let compact = !trimmed.is_empty()
        && !trimmed.chars().any(char::is_whitespace)
        && trimmed.len().is_multiple_of(2)
        && trimmed.bytes().all(|b| b.is_ascii_hexdigit());
    if compact {
        for pair in trimmed.as_bytes().chunks_exact(2) {
            let s = std::str::from_utf8(pair).expect("hexdigit 切片");
            bytes.push(u8::from_str_radix(s, 16).map_err(|e| e.to_string())?);
            mask.push(1);
        }
        return Ok((bytes, mask));
    }
    for tok in pat.split_whitespace() {
        if tok == "??" || tok == "?" {
            bytes.push(0);
            mask.push(0);
        } else {
            if tok.len() != 2 || !tok.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("非法字节 token {tok:?}"));
            }
            bytes.push(u8::from_str_radix(tok, 16).map_err(|e| e.to_string())?);
            mask.push(1);
        }
    }
    if bytes.is_empty() {
        return Err("空 pattern".to_string());
    }
    Ok((bytes, mask))
}

/// 工具名形态（agent 工具表安全：小写 + 下划线，3–64 字符）。
pub fn is_valid_tool_name(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() >= 3
        && b.len() <= 64
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

fn validate_rel_dest(dest: &str) -> Result<(), String> {
    if dest.trim().is_empty() {
        return Err("output.dest 不得为空".to_string());
    }
    let p = std::path::Path::new(dest);
    if p.is_absolute() {
        return Err(format!("output.dest {dest:?} 须为相对路径"));
    }
    for comp in p.components() {
        match comp {
            std::path::Component::Normal(_) => {}
            _ => return Err(format!("output.dest {dest:?} 含非法路径组件（../盘符/根）")),
        }
    }
    // Windows 保留名 + 冒号（对齐 image_gen 路径纪律）。
    if dest.contains(':') {
        return Err(format!("output.dest {dest:?} 含冒号"));
    }
    Ok(())
}

/// notify.on 最小条件 DSL 语法校验：`字段 比较符 字面量`，字段名
/// `[A-Za-z_][A-Za-z0-9_]*`，比较符 ==/!=/>/>=/</<=，字面量为整数/小数/短标识符。
/// 求值在 M2 PollScheduler；这里只拦语法（loud）。
pub fn validate_notify_expr(expr: &str) -> Result<(), String> {
    let expr = expr.trim();
    let ops = ["==", "!=", ">=", "<=", ">", "<"];
    let mut found: Option<(usize, &str)> = None;
    for op in ops {
        if let Some(pos) = expr.find(op) {
            found = Some((pos, op));
            break;
        }
    }
    let (pos, op) = found.ok_or_else(|| format!("notify 条件 {expr:?} 缺比较符"))?;
    let field = expr[..pos].trim();
    let value = expr[pos + op.len()..].trim();
    let ident = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .next()
                .map(|c| c.is_ascii_alphabetic() || c == b'_')
                .unwrap_or(false)
            && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    };
    if !ident(field) {
        return Err(format!("notify 条件 {expr:?} 字段名 {field:?} 非法"));
    }
    if !ident(value) && value.parse::<f64>().is_err() {
        return Err(format!(
            "notify 条件 {expr:?} 比较值 {value:?} 非法（须数或标识符）"
        ));
    }
    Ok(())
}
