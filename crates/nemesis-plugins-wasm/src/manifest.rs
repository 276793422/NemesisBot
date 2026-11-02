//! 插件 manifest（`plugin.toml`）解析与校验。
//!
//! 形态（TOML；根级 `signature` 表由验签器只剥参与规范字节计算）：
//!
//! ```toml
//! api-version = 1
//! slug = "translate"
//! kind = "tool"              # tool | observer | policy（一 manifest 一 kind 不混装）
//! name = "Translate"
//! version = "0.1.0"
//! description = "..."
//! wasm = "plugin.wasm"       # 缺省
//! wasm-sha256 = "hex64"      # 必填
//! min-tier = "big"           # mini|normal|big，缺省 big（ Invocation 期闸）
//!
//! [permissions]
//! egress = []                # 精确域名 allowlist；空 = 无出站
//! allow-private = false
//! x-secret = ["api_key"]     # 凭据名声明（vault:plugin/<slug>/<name>）
//! x-llm = true               # 面板帧 llm-complete 声明（仅 kind=panel 消费；
//!                            # 还须宿主总闸 panel_llm.enabled 打开才生效）
//!
//! [limits]                   # 可选；只允许在宿主全局上限内收紧
//! fuel = 100_000_000
//!
//! [config-schema]            # 可选；实例配置声明（键 → 人读描述）
//! target_lang = "翻译目标语言"
//!
//! # —— 以下仅 kind = "panel"（2026-10-31 面板扩展批）——
//! # 纯声明式面板：可省略 wasm / wasm-sha256（无载荷）
//! [[ui.page]]
//! route = "bugfix"
//! title = "BUG 修复流水线"
//! [[ui.page.views]]
//! kind = "table"                      # form|table|timeline|detail|actions|chart
//! bind = "flow://bugfix-pipeline/list"  # flow:// 宿主代答 | cmd://<cmd> guest
//! [[flow.template]]
//! id = "bugfix-pipeline"
//! [[flow.template.steps]]
//! id = "fetch"
//! kind = "connector_fetch"
//! when = "{{steps.fetch.count}} > 0"  # 可选条件（宿主求值；假=跳过该步）
//! gate = "manual"                     # manual = 面板人工卡点
//! on_fail = "retry:3"                 # park_human(缺省) | retry:<1-10> | continue
//! ```

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::PluginError;

/// manifest 允许的最大字节数（防畸形大文件拖垮解析）。
pub const MANIFEST_MAX_BYTES: usize = 256 * 1024;

/// 插件 kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginKind {
    /// 工具插件（world `plugin-tool`）。
    Tool,
    /// 观察者插件（world `plugin-observer`）。
    Observer,
    /// 循环策略插件（world `plugin-loop-policy`；2026-10-22 样板工程批 4）。
    Policy,
    /// 面板插件（world `plugin-panel`；2026-10-31 面板扩展批）——Dashboard
    /// 「定制」分组页面：骨架由 `[[ui]]` 段声明、宿主预置组件渲染，命令
    /// 通道 `handle-ui-command`；流程剧本由 `[[flow]]` 段声明、宿主 flow
    /// 引擎执行。可带 guest 载荷（动作翻译），也可纯声明式（无载荷）。
    Panel,
}

impl PluginKind {
    /// manifest 字符串形态。
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Observer => "observer",
            Self::Policy => "policy",
            Self::Panel => "panel",
        }
    }

    /// 该 kind 是否必须有 guest 载荷（wasm-sha256 必填）。
    /// panel 是唯一允许纯声明式（无载荷）的 kind。
    #[must_use]
    pub fn requires_payload(&self) -> bool {
        !matches!(self, Self::Panel)
    }
}

/// manifest `permissions` 段。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ManifestPermissions {
    /// 出站域名 allowlist（精确匹配；空 = 无出站能力）。
    #[serde(default)]
    pub egress: Vec<String>,
    /// 允许私网/回环目标（默认 false；测试场景显式开）。
    #[serde(default)]
    pub allow_private: bool,
    /// 凭据名声明（x-secret；进入 vault 别名约定）。
    #[serde(default, rename = "x-secret")]
    pub x_secret: Vec<String>,
    /// 面板帧 llm-complete 声明（2026-11-01 决策 C）。仅 `kind=panel`
    /// 消费（build_frame 授予面）；还须宿主总闸
    /// `panel_llm_enabled(config_dir)` 打开才生效——双闸任一缺 = 帧内
    /// 恒 unavailable。policy 帧不受此键影响（其 llm 授予走 kind 硬规则）。
    #[serde(default, rename = "x-llm")]
    pub x_llm: bool,
}

/// 面板视图词汇表（v1 六种；2026-10-31 面板扩展批，chart 加入 =
/// 2026-11-01 决策 A）。
///
/// 骨架由宿主预置组件渲染，插件永不自带 HTML/JS（三方任意前端代码 =
/// XSS/凭据窃取面）。超出词汇表的呈现诉求等 v2 iframe 沙盒通道；
/// tree/自由布局明确不做（声明式自由布局 = 重新发明前端模板引擎）。
pub const UI_VIEW_KINDS: &[&str] = &["form", "table", "timeline", "detail", "actions", "chart"];

/// chart 视图支持的图表类型（echarts 预置渲染器词表）。
pub const CHART_TYPES: &[&str] = &["line", "bar", "pie"];

/// flow step 原语词表（v1 六种；宿主 flow 引擎的绑定面）。
pub const FLOW_STEP_KINDS: &[&str] = &[
    "connector_fetch", // connector op 拉外部条目 → 建实例
    "board_land",      // board 建单 + 任务副本（origin 三件套声明）
    "agent_fixloop",   // 派发执行 + 交付捕获（测试⇄分析⇄改码循环）
    "check_regress",   // 锚点双检 + 必检对账 + 评审
    "publish",         // publish 协议执行（模板声明：推送 + 建外部引用 + unknown 对账）
    "report",          // dossier 导出 + connector 回帖
];

/// 表格/详情列定义。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct UiColumn {
    /// 数据键（行 JSON 的字段名）。
    pub key: String,
    /// 人读表头。
    pub label: String,
}

/// 表单字段定义。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct UiField {
    /// 配置键（写入实例配置；secret 型进 vault 别名）。
    pub key: String,
    /// 人读标签。
    pub label: String,
    /// 控件形态：string | text | bool | int | secret（缺省 string；
    /// secret = 值走 vault，表单只显占位不回显原文）。
    #[serde(default = "default_field_type")]
    pub field_type: String,
    /// 占位提示。
    #[serde(default)]
    pub placeholder: String,
    /// 说明文字。
    #[serde(default)]
    pub hint: String,
}

fn default_field_type() -> String {
    "string".to_string()
}

/// 动作按钮定义。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct UiButton {
    /// 按钮文案。
    pub label: String,
    /// 触发的 guest 命令名（经 `plugins.ui.invoke` 转发 `handle-ui-command`）。
    pub command: String,
    /// 点击后需二次确认（默认 false）。
    #[serde(default)]
    pub confirm: bool,
    /// 危险样式（默认 false）。
    #[serde(default)]
    pub danger: bool,
}

/// 面板视图声明（`[[ui.page.views]]`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct UiView {
    /// 组件词汇表（[`UI_VIEW_KINDS`]）。
    pub kind: String,
    /// 数据绑定：`flow://<template-id>/<view>`（宿主 flow 引擎代答）或
    /// `cmd://<command>`（guest `handle-ui-command`）。
    pub bind: String,
    /// table/detail 列定义。
    #[serde(default)]
    pub columns: Vec<UiColumn>,
    /// form 字段定义。
    #[serde(default)]
    pub fields: Vec<UiField>,
    /// actions 按钮定义。
    #[serde(default)]
    pub buttons: Vec<UiButton>,
    /// chart 图表类型（[`CHART_TYPES`]；仅 kind=chart 消费，其余 kind
    /// 声明 = loud 拒——manifest 写错信号）。
    #[serde(default)]
    pub chart: String,
    /// chart X 轴/类目字段（行 JSON 的字段名；仅 kind=chart 消费）。
    #[serde(default, rename = "x-key")]
    pub x_key: String,
    /// chart 系列定义（每项一条线/一柱组/一饼扇；仅 kind=chart 消费）。
    #[serde(default)]
    pub series: Vec<UiColumn>,
}

/// 面板页面声明（`[[ui.page]]`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct UiPage {
    /// 路由段（`/panel/<route>`；`[a-z0-9-]{1,64}` 形态同 slug）。
    pub route: String,
    /// 页面标题。
    pub title: String,
    /// 图标名（词汇表内；未知名宿主诚实回退默认图标）。
    #[serde(default)]
    pub icon: String,
    /// 视图清单（自上而下渲染）。
    #[serde(default)]
    pub views: Vec<UiView>,
}

/// flow 步骤声明（`[[flow.template.steps]]`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FlowStep {
    /// 步骤 id（`[a-z0-9-]{1,64}`；模板内唯一）。
    pub id: String,
    /// 原语种类（[`FLOW_STEP_KINDS`]；宿主 flow 引擎绑定面）。
    pub kind: String,
    /// 卡点：缺省 auto；`manual` = 面板人工闸（实例停 awaiting-gate 态，
    /// actions 视图渲染确认按钮）。
    #[serde(default)]
    pub gate: String,
    /// 失败分支：缺省 park-human；`retry:<N>`（1..=10，超限转 parked）；
    /// `continue`（记缺口继续）。
    #[serde(default)]
    pub on_fail: String,
    /// 步骤参数（如 connector id、op id、模板变量；语义由 step kind 定）。
    #[serde(default)]
    pub args: BTreeMap<String, String>,
    /// 运行条件（2026-11-01 决策 B）：单比较表达式 `{{token}} <op> <值>`，
    /// op ∈ == != > >= < <=；左侧先过 token 插值（含 `steps.<id>.<field>`）
    /// 再求值；两侧都能转数字按数值比，否则按字符串比。求值假 = 步骤记
    /// **skipped** 并同 tick 顺延到下一步；表达式畸形（算不出布尔）= 步骤
    /// 诚实失败（走 on_fail）。宿主求值，guest 无参与。
    #[serde(default)]
    pub when: String,
}

/// flow 模板状态回写声明（`[flow.template.on-transition]` 表；2026-11-02
/// 外循环域剥离批 C-2）。board 单状态转移 → 按声明拼 connector op——op 名
/// 与 args 形状（`issue_comment` + `{owner,repo,index,body}` 之类）全部是
/// 域知识，只在模板里；宿主只提供 token 插值与 origin 坐标解析原语。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FlowTransition {
    /// connector op 名（空 = 本模板不订阅回写）。
    #[serde(default)]
    pub op: String,
    /// op args（值过 token 插值：`{{transition.*}}` / `{{origin_meta.<key>}}` /
    /// `{{origin_id.<name>}}`）。
    #[serde(default)]
    pub args: BTreeMap<String, String>,
    /// v1 旧单 origin_meta 缺席时的 origin_id 切分表达式表
    /// （`name = "segment '/' 1 | after '#'"` 形态；宿主通用字符串切分
    /// 原语，分隔符/段序=域知识在模板）。
    #[serde(default, rename = "parse-origin-id")]
    pub parse_origin_id: BTreeMap<String, String>,
}

/// flow 剧本模板声明（`[[flow.template]]`）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FlowTemplate {
    /// 模板 id（`[a-z0-9-]{1,64}`；flow:// 绑定与实例落账的命名空间）。
    pub id: String,
    /// fetch 轮询周期秒（0 = 只手动触发；缺省 0）。
    #[serde(default)]
    pub interval_secs: u64,
    /// 步骤序列（≥1；顺序执行。serde default 让零步骤模板走到 validate
    /// 报「needs at least one step」人读错误，而非 TOML 层 missing field）。
    #[serde(default)]
    pub steps: Vec<FlowStep>,
    /// 本模板绑定的来源类型（TaskOrigin.origin_type；回写订阅匹配键 + 导入
    /// 建单的 origin_type 透传。空 = 不参与回写订阅。2026-11-02 外循环域
    /// 剥离批 C-1/C-2）。
    #[serde(default)]
    pub origin_type: String,
    /// 建单 origin_id 模板（token 插值：`{{connector}}` / `{{arg.<name>}}` /
    /// `{{key}}`；board 去重键 = 整串精确匹配）。
    #[serde(default)]
    pub origin_id: String,
    /// 建单 origin_meta 映射（键 → `$.<path>` 取条目字段，否则当 token
    /// 字面插值；回写坐标落库载体）。
    #[serde(default)]
    pub origin_meta: BTreeMap<String, String>,
    /// 状态回写声明（op 空 = 不订阅）。
    #[serde(default)]
    pub on_transition: FlowTransition,
}

/// manifest 根结构（`signature` 表不在此——由验签器从原始文档剥离）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PluginManifest {
    /// 合同代际（必须等于宿主 [`crate::CONTRACT_API_VERSION`]）。
    pub api_version: u32,
    /// 插件 slug（目录名；`[a-z0-9-]{1,64}`）。
    pub slug: String,
    /// kind。
    pub kind: PluginKindSerde,
    /// 人读名称。
    pub name: String,
    /// 语义版本。
    pub version: String,
    /// 描述。
    #[serde(default)]
    pub description: String,
    /// wasm 文件名（相对插件目录；缺省 `plugin.wasm`）。
    #[serde(default = "default_wasm_file")]
    pub wasm: String,
    /// wasm sha256（hex64；安装期重算比对）。`None` = 纯声明式插件
    /// （无载荷，仅 `kind=panel` 合法——ui/flow 段全由宿主引擎兑现）。
    #[serde(default)]
    pub wasm_sha256: Option<String>,
    /// 最低调用档（mini/normal/big；缺省 big）。
    #[serde(default = "default_min_tier")]
    pub min_tier: String,
    /// 权限段。
    #[serde(default)]
    pub permissions: ManifestPermissions,
    /// 资源收紧覆盖（只允许收紧）。
    #[serde(default)]
    pub limits: BTreeMap<String, u64>,
    /// 实例配置声明（键 → 人读描述；x-secret 键不在此——凭据面分离）。
    #[serde(default)]
    pub config_schema: BTreeMap<String, String>,
    /// 面板页面声明（仅 `kind=panel` 可声明；2026-10-31 面板扩展批）。
    #[serde(default, rename = "ui")]
    pub ui: UiSection,
    /// flow 剧本模板声明（仅 `kind=panel` 可声明；2026-10-31 面板扩展批）。
    #[serde(default)]
    pub flow: FlowSection,
    /// 接管声明（内置工具名清单；2026-10-23 工具接管层）。仅 `kind=tool`
    /// 可声明；deny-list 命中 parse 拒绝。声明 = 安装审批卡 + wasm.list
    /// 徽标双留痕，激活还要过「目标在 loop 注册表在场」闸（agent_factory 8d）。
    #[serde(default)]
    pub takeover: Vec<String>,
}

/// 面板页面声明段（`ui` 表）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct UiSection {
    /// 页面清单（可多页；`[[ui.page]]`）。
    #[serde(default, rename = "page")]
    pub pages: Vec<UiPage>,
}

/// flow 剧本声明段（`flow` 表）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FlowSection {
    /// 剧本模板清单（`[[flow.template]]`）。
    #[serde(default, rename = "template")]
    pub templates: Vec<FlowTemplate>,
}

/// 接管 deny-list（宿主特权语义工具——沙盒内 guest 无法诚实兑现，接管
/// 一律拒绝）。与 `nemesis_agent::MOVE_TOOLS`（remote_executor_tool.rs）+
/// 进程/注册表/系统面同源；两清单相等性由 nemesisbot plugin_bridge tests
/// 的 drift 断言钉住（本 crate 依赖方向无 nemesis-agent，只能字面复制）。
pub const TAKEOVER_DENIED_TOOLS: &[&str] = &[
    // nemesis-agent::MOVE_TOOLS（执行体分离高危族——exec/脚本/文件写面/搜索）
    "exec",
    "run_script",
    "run_checks",
    "read_file",
    "write_file",
    "list_dir",
    "edit_file",
    "multiedit",
    "append_file",
    "delete_file",
    "move_file",
    "copy_file",
    "create_dir",
    "delete_dir",
    "grep",
    "git",
    // 进程/注册表/系统面（CRITICAL/宿主特权）
    "process_spawn",
    "process_kill",
    "process_exec",
    "process_list",
    "registry_read",
    "registry_write",
    "system_shutdown",
    "system_info",
    "hardware_i2c",
];

/// `builtin` 是否在接管 deny-list（宿主特权语义，沙盒内不可诚实兑现）。
#[must_use]
pub fn takeover_denied(builtin: &str) -> bool {
    TAKEOVER_DENIED_TOOLS.contains(&builtin)
}

/// kind 的 serde 中介（字符串 ↔ 枚举；错误信息可读）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginKindSerde(pub PluginKind);

impl std::ops::Deref for PluginKindSerde {
    type Target = PluginKind;
    fn deref(&self) -> &PluginKind {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PluginKindSerde {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "tool" => Ok(Self(PluginKind::Tool)),
            "observer" => Ok(Self(PluginKind::Observer)),
            "policy" => Ok(Self(PluginKind::Policy)),
            "panel" => Ok(Self(PluginKind::Panel)),
            other => Err(serde::de::Error::custom(format!(
                "invalid kind '{other}' (expected tool | observer | policy | panel)"
            ))),
        }
    }
}

fn default_wasm_file() -> String {
    "plugin.wasm".to_string()
}

fn default_min_tier() -> String {
    "big".to_string()
}

/// slug 合法性（`[a-z0-9-]{1,64}`，不以 - 开头结尾）。
#[must_use]
pub fn valid_slug(slug: &str) -> bool {
    (1..=64).contains(&slug.len())
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && slug != "all"
}

/// sha256 hex64 形态校验。
#[must_use]
pub fn valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

impl PluginManifest {
    /// 从目录里的 `plugin.toml` 解析 + 结构校验（不含签名/wasm 校验）。
    pub fn load_from_dir(dir: &std::path::Path) -> Result<(Self, String), PluginError> {
        let path = dir.join("plugin.toml");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| PluginError::Manifest(format!("read {}: {e}", path.display())))?;
        if raw.len() > MANIFEST_MAX_BYTES {
            return Err(PluginError::Manifest(format!(
                "manifest exceeds {MANIFEST_MAX_BYTES} bytes"
            )));
        }
        Self::parse(&raw)
    }

    /// 解析 + 结构校验（返回 manifest 与原始文本；签名校验见 `trust`）。
    pub fn parse(raw: &str) -> Result<(Self, String), PluginError> {
        let m: PluginManifest = toml_edit::de::from_str(raw)
            .map_err(|e| PluginError::Manifest(format!("parse: {e}")))?;
        m.validate()?;
        Ok((m, raw.to_string()))
    }

    /// 结构校验（与签名无关的字段级约束）。
    pub fn validate(&self) -> Result<(), PluginError> {
        if self.api_version != crate::CONTRACT_API_VERSION {
            return Err(PluginError::ContractVersion {
                manifest: self.api_version,
                supported: crate::CONTRACT_API_VERSION,
            });
        }
        if !valid_slug(&self.slug) {
            return Err(PluginError::Manifest(format!(
                "invalid slug: {}",
                self.slug
            )));
        }
        match &self.wasm_sha256 {
            Some(h) if valid_sha256_hex(h) => {}
            Some(_) => {
                return Err(PluginError::Manifest(
                    "wasm-sha256 must be 64 hex chars".into(),
                ));
            }
            // 无载荷 = 纯声明式，仅 panel 合法（tool/observer/policy 必须
            // 带 guest 组件——其扩展点本体就是 guest 出口）。
            None if self.kind.requires_payload() => {
                return Err(PluginError::Manifest(format!(
                    "kind={} requires wasm-sha256 (guest payload mandatory; only kind=panel may omit it)",
                    self.kind.as_str()
                )));
            }
            None => {}
        }
        if !matches!(self.min_tier.as_str(), "mini" | "normal" | "big") {
            return Err(PluginError::Manifest(format!(
                "invalid min-tier: {} (mini|normal|big)",
                self.min_tier
            )));
        }
        if self.name.len() > 128 {
            return Err(PluginError::Manifest("name exceeds 128 chars".into()));
        }
        if self.version.len() > 64 {
            return Err(PluginError::Manifest("version exceeds 64 chars".into()));
        }
        // egress allowlist 形态：host 或 host:port。
        for entry in &self.permissions.egress {
            let e = entry.to_lowercase();
            if e.is_empty()
                || e.starts_with('.')
                || e.contains('/')
                || e.contains('*')
                || e.split(':').count() > 2
            {
                return Err(PluginError::Manifest(format!(
                    "invalid egress entry (exact host or host:port only): {entry}"
                )));
            }
        }
        // x-secret 名形态：标识符样。
        for s in &self.permissions.x_secret {
            if s.is_empty()
                || s.len() > 64
                || !s
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(PluginError::Manifest(format!("invalid x-secret name: {s}")));
            }
        }
        // config-schema 键不与 x-secret 冲突（同名键语义分裂）。
        for key in self.config_schema.keys() {
            if self.permissions.x_secret.iter().any(|s| s == key) {
                return Err(PluginError::Manifest(format!(
                    "config-schema key '{key}' collides with x-secret declaration"
                )));
            }
            if key.is_empty() || key.len() > 64 {
                return Err(PluginError::Manifest(format!(
                    "invalid config-schema key (1-64 chars): {key}"
                )));
            }
        }
        // policy kind 权限收紧（2026-10-22 批 4）：循环策略组件决策纯计算面
        // ——无凭据（x-secret）、无直网（egress/allow-private）。数据来源只
        // 有宿主投影入参 + config-get + workspace-read + llm-complete +
        // tool-invoke（借道原生工具过安全 8 层全闸）。
        if *self.kind == PluginKind::Policy {
            if !self.permissions.egress.is_empty() {
                return Err(PluginError::Manifest(
                    "kind=policy forbids permissions.egress (policy components have no direct network access)".into(),
                ));
            }
            if self.permissions.allow_private {
                return Err(PluginError::Manifest(
                    "kind=policy forbids permissions.allow-private".into(),
                ));
            }
            if !self.permissions.x_secret.is_empty() {
                return Err(PluginError::Manifest(
                    "kind=policy forbids permissions.x-secret (policy components hold no credentials)".into(),
                ));
            }
        }
        // x-llm 声明位校验（2026-11-01 决策 C）：仅 panel 可声明——tool/
        // observer 帧无 llm 消费面（callbacks 闸按 kind 拒），policy 帧
        // llm 走 kind 硬规则授予（声明了也是死键 = manifest 写错信号）。
        if self.permissions.x_llm && *self.kind != PluginKind::Panel {
            return Err(PluginError::Manifest(format!(
                "permissions.x-llm requires kind=panel (kind={} frames have no llm surface)",
                self.kind.as_str()
            )));
        }
        // 接管声明校验（2026-10-23 工具接管层）：仅 tool kind；目标名形态 +
        // deny-list（宿主特权语义——沙盒内 guest 无法诚实兑现，接管一律拒）。
        if !self.takeover.is_empty() {
            if *self.kind != PluginKind::Tool {
                return Err(PluginError::Manifest(
                    "takeover requires kind=tool (only tool components can take over built-in interfaces)".into(),
                ));
            }
            for t in &self.takeover {
                if t.is_empty() || t.len() > 64 {
                    return Err(PluginError::Manifest(format!(
                        "invalid takeover target (1-64 chars): {t:?}"
                    )));
                }
                if t.starts_with(crate::PLUGIN_TOOL_PREFIX) {
                    return Err(PluginError::Manifest(format!(
                        "invalid takeover target '{t}': plugin namespace is isolated by design (builtin names only)"
                    )));
                }
                if takeover_denied(t) {
                    return Err(PluginError::Manifest(format!(
                        "takeover of '{t}' is denied: host-privileged semantics cannot be honestly honored in-sandbox"
                    )));
                }
            }
        }
        // ui/flow 段位校验（2026-10-31 面板扩展批）：仅 kind=panel 可声明
        // （其他 kind 带段 = manifest 写错的信号，loud 拒）；纯声明式（无
        // 载荷）必须至少声明一个页面——零声明零载荷的包装无意义。
        let is_panel = *self.kind == PluginKind::Panel;
        if !is_panel {
            if !self.ui.pages.is_empty() {
                return Err(PluginError::Manifest(
                    "ui section requires kind=panel".into(),
                ));
            }
            if !self.flow.templates.is_empty() {
                return Err(PluginError::Manifest(
                    "flow section requires kind=panel".into(),
                ));
            }
        }
        if self.wasm_sha256.is_none() && self.ui.pages.is_empty() {
            return Err(PluginError::Manifest(
                "declarative plugin (no wasm payload) must declare at least one [[ui.page]]".into(),
            ));
        }
        if is_panel {
            // 路由跨页面唯一（前端 /panel/<route> 动态挂载键）。
            let mut routes = std::collections::BTreeSet::new();
            for page in &self.ui.pages {
                if !valid_slug(&page.route) {
                    return Err(PluginError::Manifest(format!(
                        "invalid ui.page.route: {} (form same as slug)",
                        page.route
                    )));
                }
                if page.title.trim().is_empty() || page.title.chars().count() > 128 {
                    return Err(PluginError::Manifest(format!(
                        "ui.page '{}' title must be 1-128 chars",
                        page.route
                    )));
                }
                if !routes.insert(page.route.as_str()) {
                    return Err(PluginError::Manifest(format!(
                        "duplicate ui.page.route: {}",
                        page.route
                    )));
                }
                for view in &page.views {
                    validate_view(view)?;
                }
            }
            // 模板 id 唯一（flow:// 绑定与实例落账的命名空间）。
            let mut tids = std::collections::BTreeSet::new();
            for tpl in &self.flow.templates {
                if !valid_slug(&tpl.id) {
                    return Err(PluginError::Manifest(format!(
                        "invalid flow.template.id: {}",
                        tpl.id
                    )));
                }
                if !tids.insert(tpl.id.as_str()) {
                    return Err(PluginError::Manifest(format!(
                        "duplicate flow.template.id: {}",
                        tpl.id
                    )));
                }
                if tpl.steps.is_empty() {
                    return Err(PluginError::Manifest(format!(
                        "flow.template '{}' needs at least one step",
                        tpl.id
                    )));
                }
                if !tpl.on_transition.op.is_empty() && tpl.origin_type.is_empty() {
                    return Err(PluginError::Manifest(format!(
                        "flow.template '{}': on-transition.op requires origin-type",
                        tpl.id
                    )));
                }
                let mut sids = std::collections::BTreeSet::new();
                for step in &tpl.steps {
                    if !valid_slug(&step.id) {
                        return Err(PluginError::Manifest(format!(
                            "flow.template '{}' step id invalid: {}",
                            tpl.id, step.id
                        )));
                    }
                    if !sids.insert(step.id.as_str()) {
                        return Err(PluginError::Manifest(format!(
                            "flow.template '{}' duplicate step id: {}",
                            tpl.id, step.id
                        )));
                    }
                    if !FLOW_STEP_KINDS.contains(&step.kind.as_str()) {
                        return Err(PluginError::Manifest(format!(
                            "flow.template '{}' step '{}': invalid kind '{}' (allowed: {})",
                            tpl.id,
                            step.id,
                            step.kind,
                            FLOW_STEP_KINDS.join("|")
                        )));
                    }
                    if !step.gate.is_empty() && step.gate != "manual" {
                        return Err(PluginError::Manifest(format!(
                            "flow.template '{}' step '{}': invalid gate '{}' (empty|manual)",
                            tpl.id, step.id, step.gate
                        )));
                    }
                    validate_on_fail(tpl.id.as_str(), step)?;
                    validate_when(tpl.id.as_str(), step)?;
                }
            }
        }
        Ok(())
    }

    /// config-schema 声明键清单（实例配置读取的白名单）。
    #[must_use]
    pub fn config_schema_keys(&self) -> Vec<String> {
        self.config_schema.keys().cloned().collect()
    }
}

/// 视图声明校验：词表成员 + bind 形态 + 词汇专用段最低配置。
fn validate_view(view: &UiView) -> Result<(), PluginError> {
    if !UI_VIEW_KINDS.contains(&view.kind.as_str()) {
        return Err(PluginError::Manifest(format!(
            "invalid ui view kind '{}' (allowed: {})",
            view.kind,
            UI_VIEW_KINDS.join("|")
        )));
    }
    // bind 形态：flow://<template>/<view>（宿主代答）或 cmd://<command>（guest）。
    // actions 视图允许空 bind（语义在 per-button command，视图级 bind 无
    // 消费者——强填只会造出无人读取的假锚点）。
    let actions_empty_bind = view.kind == "actions" && view.bind.is_empty();
    if !actions_empty_bind {
        if let Some(rest) = view.bind.strip_prefix("flow://") {
            let parts: Vec<&str> = rest.split('/').collect();
            if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
                return Err(PluginError::Manifest(format!(
                    "invalid flow bind '{}' (expected flow://<template>/<view>)",
                    view.bind
                )));
            }
        } else if let Some(rest) = view.bind.strip_prefix("cmd://") {
            if rest.is_empty()
                || rest.len() > 64
                || rest.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                return Err(PluginError::Manifest(format!(
                    "invalid cmd bind '{}' (expected cmd://<command>, 1-64 chars, no whitespace)",
                    view.bind
                )));
            }
        } else {
            return Err(PluginError::Manifest(format!(
                "invalid ui view bind '{}' (must start with flow:// or cmd://)",
                view.bind
            )));
        }
    }
    // chart 视图（2026-11-01 决策 A）：图表类型 + X 轴字段 + 系列定义，
    // 数据仍走 bind 投影行（与 table 同源）；专用段与其他词汇互斥。
    if view.kind == "chart" {
        if !CHART_TYPES.contains(&view.chart.as_str()) {
            return Err(PluginError::Manifest(format!(
                "ui view bind '{}' kind=chart: invalid chart type '{}' (allowed: {})",
                view.bind,
                view.chart,
                CHART_TYPES.join("|")
            )));
        }
        if view.x_key.is_empty() {
            return Err(PluginError::Manifest(format!(
                "ui view bind '{}' kind=chart needs x-key (category axis field)",
                view.bind
            )));
        }
        if view.series.is_empty() {
            return Err(PluginError::Manifest(format!(
                "ui view bind '{}' kind=chart needs at least one series",
                view.bind
            )));
        }
        for s in &view.series {
            if s.key.is_empty() || s.label.is_empty() {
                return Err(PluginError::Manifest(format!(
                    "ui view bind '{}': series key/label must be non-empty",
                    view.bind
                )));
            }
        }
        if !view.columns.is_empty() || !view.fields.is_empty() || !view.buttons.is_empty() {
            return Err(PluginError::Manifest(format!(
                "ui view bind '{}' kind=chart forbids columns/fields/buttons sections",
                view.bind
            )));
        }
        return Ok(());
    }
    // 非 chart 视图声明 chart 专用段 = manifest 写错信号（跨词汇段互斥）。
    if !view.chart.is_empty() || !view.x_key.is_empty() || !view.series.is_empty() {
        return Err(PluginError::Manifest(format!(
            "ui view bind '{}': chart/x-key/series only apply to kind=chart",
            view.bind
        )));
    }
    match view.kind.as_str() {
        // table/detail 都靠列定义渲染行（detail = 键值逐行）；零列无法呈现。
        "table" | "detail" => {
            if view.columns.is_empty() {
                return Err(PluginError::Manifest(format!(
                    "ui view bind '{}' kind={} needs at least one column",
                    view.bind, view.kind
                )));
            }
            for col in &view.columns {
                if col.key.is_empty() || col.label.is_empty() {
                    return Err(PluginError::Manifest(format!(
                        "ui view bind '{}': column key/label must be non-empty",
                        view.bind
                    )));
                }
            }
        }
        "form" => {
            if view.fields.is_empty() {
                return Err(PluginError::Manifest(format!(
                    "ui view bind '{}' kind=form needs at least one field",
                    view.bind
                )));
            }
            for f in &view.fields {
                if f.key.is_empty() || f.label.is_empty() {
                    return Err(PluginError::Manifest(format!(
                        "ui view bind '{}': field key/label must be non-empty",
                        view.bind
                    )));
                }
                if !matches!(
                    f.field_type.as_str(),
                    "string" | "text" | "bool" | "int" | "secret"
                ) {
                    return Err(PluginError::Manifest(format!(
                        "ui view bind '{}': invalid field type '{}' (string|text|bool|int|secret)",
                        view.bind, f.field_type
                    )));
                }
            }
        }
        "actions" => {
            if view.buttons.is_empty() {
                return Err(PluginError::Manifest(format!(
                    "ui view bind '{}' kind=actions needs at least one button",
                    view.bind
                )));
            }
            for b in &view.buttons {
                if b.label.trim().is_empty() || b.command.is_empty() {
                    return Err(PluginError::Manifest(format!(
                        "ui view bind '{}': button label/command must be non-empty",
                        view.bind
                    )));
                }
            }
        }
        // timeline 无专用段（条目结构由渲染层定）；kind 已验词表，直通。
        _ => {}
    }
    Ok(())
}

/// on_fail 分支形态校验：空（park_human 缺省）| retry:<1-10> | continue。
fn validate_on_fail(template_id: &str, step: &FlowStep) -> Result<(), PluginError> {
    if step.on_fail.is_empty() || step.on_fail == "park_human" || step.on_fail == "continue" {
        return Ok(());
    }
    if let Some(n) = step.on_fail.strip_prefix("retry:") {
        match n.parse::<u32>() {
            Ok(k) if (1..=10).contains(&k) => return Ok(()),
            _ => {}
        }
    }
    Err(PluginError::Manifest(format!(
        "flow.template '{template_id}' step '{}': invalid on_fail '{}' (park_human|retry:<1-10>|continue)",
        step.id, step.on_fail
    )))
}

/// when 条件形态静态校验（2026-11-01 决策 B）：非空时必须含比较算子
/// 且两侧非空。算子匹配按二字优先（`>=` 不被 `>` 吞掉）。token 插值与
/// 求值在运行期（token 值装配期不可知），这里只抓拼写级畸形。
fn validate_when(template_id: &str, step: &FlowStep) -> Result<(), PluginError> {
    if step.when.is_empty() {
        return Ok(());
    }
    const OPS: &[&str] = &["==", "!=", ">=", "<=", ">", "<"];
    for op in OPS {
        if let Some(idx) = step.when.find(op) {
            let left = step.when[..idx].trim();
            let right = step.when[idx + op.len()..].trim();
            if left.is_empty() || right.is_empty() {
                return Err(PluginError::Manifest(format!(
                    "flow.template '{template_id}' step '{}': invalid when '{}' (both sides of '{op}' must be non-empty)",
                    step.id, step.when
                )));
            }
            return Ok(());
        }
    }
    Err(PluginError::Manifest(format!(
        "flow.template '{template_id}' step '{}': invalid when '{}' (expected <left> <op> <right>, op: == != >= <= > <)",
        step.id, step.when
    )))
}
