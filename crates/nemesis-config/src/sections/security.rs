// 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_security_action")]
    pub default_action: String,
    /// CFG-05（死键接线）：审计 JSONL 是否记录放行事件
    /// （false = 只记拦截/告警）。runtime 侧 security_setup.rs 构造期读取。
    #[serde(default = "default_true")]
    pub log_all_operations: bool,
    // ── 死键恢复区（用户指令：代码不得随便删，先恢复保留待
    // 讨论）────────────────────────────────────────────────────
    // 以下 5 键曾在 CFG-05 被裁「确认无用直接删」，现按用户指令
    // 恢复原字段原属性。**未接线**（零 runtime 消费方，runtime 走裸 JSON
    // 或 NemesisPath 硬拼）：log_denials_only（与 log_all_operations=false
    // 同义）、max_pending_requests（无满额语义实现）、audit_log_retention_days
    // （无保留清扫器，盲清扫会误伤审计链文件）、audit_log_path（gateway 经
    // nemesis-path 硬拼安全日志目录）、synchronous_mode（无语义）。
    // 保留原因：字段必须与出厂模板键共存（typed round-trip 不丢键），
    // 删除与否待用户后续裁决。
    #[serde(default)]
    pub log_denials_only: bool,
    /// CFG-05：审批卡超时秒数。runtime 侧 security_setup.rs 构造期读取，
    /// 接入 AuditorConfig.approval_timeout_secs。
    #[serde(default = "default_approval_timeout")]
    pub approval_timeout_seconds: i64,
    #[serde(default = "default_max_pending")]
    pub max_pending_requests: i64,
    #[serde(default = "default_audit_retention")]
    pub audit_log_retention_days: i64,
    #[serde(default)]
    pub audit_log_path: String,
    #[serde(default = "default_true")]
    pub audit_log_file_enabled: bool,
    #[serde(default)]
    pub synchronous_mode: bool,
    // ── 死键恢复区结束 ────────────────────────────────────────────
    /// 审计链（Merkle append-only，第 8 层）开关。runtime 侧
    /// security_setup.rs 按裸 JSON 顶层键读取；此前 typed 无此字段，
    /// Dashboard 安全设置页保存一次即把用户手加的该键整行抹掉
    /// （与 CFG-01 directory_rules 同族 round-trip 删键 bug）。
    #[serde(default)]
    pub audit_chain_enabled: bool,
    // （扩展计划）：子进程环境凭据清洗开关（默认开）。
    // runtime 侧 security_setup.rs 构造期同步到
    // nemesis_utils::env_sanitize 进程级静态；三处 spawn 点
    // （agent loop exec/run_checks 闸 + nemesis-tools shell/async_shell）
    // 统一查询。
    #[serde(default = "default_true")]
    pub sanitize_child_env: bool,
    // pre-exec 脚本内容扫描开关（默认开）。runtime 侧
    // security_setup.rs 构造期按裸 JSON 键读取注入 SecurityPluginConfig；
    // typed 字段的存在只为 Dashboard typed round-trip 不丢键（CFG-01 同族
    // 删键 bug 防线）。
    #[serde(default = "default_true")]
    pub preexec_scan_enabled: bool,
    #[serde(default)]
    pub file_rules: Option<FileSecurityRules>,
    // CFG-01（横扫存量加固）：磁盘/出厂模板真相源键名是
    // `dir_rules`（runtime 侧 security_setup.rs 按裸 JSON 键 `dir_rules`
    // 读取）。此前 typed 字段无 rename → 反序列化拿不到、保存时以
    // `directory_rules` 键（或 None）整文件覆盖，磁盘 `dir_rules` 被整段
    // 抹掉（Dashboard 安全设置页保存一次即触发）。rename 对齐后 round-trip
    // 字节稳定；历史键名 `directory_rules` 的别名读取仍在 runtime 侧保留
    // （security_setup.rs，WARN 提示改名）。
    // alias（复核）：存量部署文件里写的是历史键名
    // `directory_rules`——仅 rename 时该文件经 typed 读-改-写会把规则段
    // 变成 `"dir_rules": null`（内容丢失），且 runtime 读 `dir_rules` 得
    // Some(Null) 连别名分支也不再触发。alias 让 legacy 键内容被读进
    // typed、保存统一迁到 `dir_rules`，规则内容零丢失。
    #[serde(default, rename = "dir_rules", alias = "directory_rules")]
    pub directory_rules: Option<DirectorySecurityRules>,
    #[serde(default)]
    pub process_rules: Option<ProcessSecurityRules>,
    #[serde(default)]
    pub network_rules: Option<NetworkSecurityRules>,
    #[serde(default)]
    pub hardware_rules: Option<HardwareSecurityRules>,
    #[serde(default)]
    pub registry_rules: Option<RegistrySecurityRules>,
    #[serde(default)]
    pub layers: Option<SecurityLayersConfig>,
    // （用户裁决）：exec/spawn 未知命令（无规则命中）姿态。
    // allow（默认=旧行为）/ ask（送审批卡）/ deny（硬拦）。runtime 侧
    // security_setup.rs 按裸 JSON 键读取注入 auditor。
    // A-（复核）：typed 缺省必须是**空串**（= runtime 缺键
    // 语义，落 auditor 旧行为），不得物化裁决值——Dashboard 保存走 typed
    // round-trip，若 serde 缺省给 "allow"/"ask"，用户从未显式配置的部署
    // 保存一次即被静默写入（缺键语义： 落 default_action、 旧
    // fail-open——被替换成裁决值 = 语义漂移）。前端已按「缺键显示裁决
    // 默认、保存不写键」实现（SecurityView POLICY_KEYS + spec 钉死），
    // typed 侧对齐；`skip_serializing_if` 让未配置（空串）不落盘。用户
    // 显式选择后才物化。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub exec_unknown_policy: String,
    // （用户裁决）：Guardian LLM judge 失败（异常/不可用）
    // 时的姿态。ask（默认，送审批卡）/ allow（按策略放行+WARN）/ deny。
    // 缺省语义同上（A）：空串 = runtime 缺键 = 旧行为，不物化。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub guardian_failure_policy: String,
    // 无上下文 LLM 命令审计（默认 off）：guardian 覆盖
    // 面开关。off（默认）= 不装配 judge（零 LLM 成本）；critical = 旧
    // CRITICAL 全审；high = HIGH+CRITICAL 破坏形态预筛 + LLM 审。A-
    // 同款：typed 缺省空串（= runtime 缺键 = off），`skip_serializing_if`
    // 未配置不落盘，用户显式选择后才物化。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub guardian_mode: String,
    // ── vault（计划）：声明式量化风险限制 ──────
    // 类别 → 规则。**缺省空表 = 全关**（A- 同款缺省语义：空表不落盘，
    // 未配置的部署保存一次字节不变）。类别键由工具经
    // `Tool::limit_categories` 声明（机制不认识业务名词，工具 declares，
    // 配置 binds）。窗口 = 滑动窗口（进程内存计数，重启清零——诚实边界，
    // 见计划）。
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub limits: std::collections::BTreeMap<String, RateLimitRule>,
    // ── 签名验证三态开关（接入计划）──────────────────
    // ""（缺省）/ "off" / "warn" / "enforce"；未知值启动时 loud 拒绝。
    // **只读于启动，改动必须重启**（与 tier 等热载键刻意不同——避免
    // 「运行中验证策略被改」这个额外可篡改面）。A- 同款缺省语义：
    // 空串 = 用户从未配置 = 落默认 warn，`skip_serializing_if` 不落盘，
    // Dashboard 保存一次字节不变；用户显式选择后才物化。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature_verify: String,
    // ── 免打扰（Part 2）──────────────────
    // 审批自动批准段。缺省关（弹卡照旧）；`auto_approve=true` 时
    // RequireApproval 一律自动放行（审计标 auto_by_config，安全 8 层其余
    // 各层照常全跑）。整段缺省不落盘（skip_serializing_if）：未配置的
    // 部署 typed round-trip 字节不变（A- 缺省语义同款）。
    #[serde(default, skip_serializing_if = "approvals_is_default")]
    pub approvals: ApprovalsConfig,
}

/// 免打扰：审批自动批准配置段（Part 2）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ApprovalsConfig {
    /// 无人值守免打扰：RequireApproval 审批卡一律自动批准（pattern 记忆
    /// 优先；guardian 语义二审不是弹窗不纳入；技能安装闸按发起流程分叉
    /// 见计划）。默认关 = 现行为（弹窗）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_approve: bool,
}

/// serde skip_serializing_if：整段处于缺省（关）时不落盘。
fn approvals_is_default(v: &ApprovalsConfig) -> bool {
    !v.auto_approve
}

/// 单类别的滑动窗口限制规则。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RateLimitRule {
    /// 窗口内允许的最大次数。
    pub max: u32,
    /// 窗口长度（秒）。
    pub window_secs: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            default_action: "deny".to_string(),
            log_all_operations: true,
            // 死键恢复区（用户指令恢复保留，未接线，见字段区注释）
            log_denials_only: false,
            approval_timeout_seconds: 300,
            max_pending_requests: 100,
            audit_log_retention_days: 90,
            audit_log_path: String::new(),
            audit_log_file_enabled: true,
            synchronous_mode: false,
            audit_chain_enabled: false,
            sanitize_child_env: true,
            preexec_scan_enabled: true,
            file_rules: None,
            directory_rules: None,
            process_rules: None,
            network_rules: None,
            hardware_rules: None,
            registry_rules: None,
            layers: None,
            exec_unknown_policy: String::new(),
            guardian_failure_policy: String::new(),
            guardian_mode: String::new(),
            // 缺省空表 = 限制全关。
            limits: std::collections::BTreeMap::new(),
            // 签名验证三态开关：空串 = 未配置 = 落默认 warn。
            signature_verify: String::new(),
            // 免打扰：缺省关。
            approvals: ApprovalsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityLayersConfig {
    #[serde(default)]
    pub injection: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub command_guard: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub dlp: Option<DLPLayerConfig>,
    #[serde(default)]
    pub ssrf: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub credential: Option<SecurityLayerConfig>,
    // ── 死键恢复区（用户指令恢复保留，未接线）──────
    // `signature`：签名验证层尚未接入管线（v4 verify 是独立子系统）；
    // `audit_chain`：runtime 读取的是顶层 `audit_chain_enabled` 而非本键，
    // 曾被定性误导性假开关。恢复保留待用户裁决。
    #[serde(default)]
    pub signature: Option<SignatureLayerConfig>,
    #[serde(default)]
    pub audit_chain: Option<SecurityLayerConfig>,
    // ── 死键恢复区结束 ────────────────────────────────────────
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignatureLayerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityLayerConfig {
    #[serde(default)]
    pub enabled: bool,
    /// BTreeMap 键序确定（同 `ModelConfig.extra`，security 段字节稳定）。
    #[serde(default)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
    /// SSRF 白名单域名（runtime `apply_security_layer_switches` 读
    /// `layers.ssrf.allowed_hosts` **层对象顶层**）。此前 typed 无此字段——
    /// security.config.get 反序列化丢弃、save 整档回存把它从磁盘抹掉
    /// （CFG-01 同族 round-trip 删键 bug，2026-11-10 十层页面化时修复）。
    /// 仅 SSRF 层消费；其他层带此键的部署同样 round-trip 保留（语义无害）。
    /// skip_serializing_if = 空 Vec 不落盘，未配置的层保存一次字节不变。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DLPLayerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub action: String,
    /// Action for low-confidence matches. Empty → default "log" (detect, don't block).
    #[serde(default)]
    pub low_confidence_action: String,
    /// Action for DLP hits on inbound/local-storage ops. Empty → default "log".
    #[serde(default)]
    pub inbound_action: String,
}

// CFG-05：`SignatureLayerConfig` 已随 `SecurityLayersConfig.signature` 死键
// 一并删除（签名验证层尚未接入安全管线；v4 verify 是独立子系统，未来接入
// 时再补 schema）。

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityRule {
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub action: String,
    /// 规则注释（复核）：CLI `security rules add` 会写入
    /// `comment` 字段、runtime `parse_rules` 读取展示；typed 缺此字段时
    /// Dashboard 保存一次即把手写 comment 抹掉（CFG-01 同族）。
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileSecurityRules {
    #[serde(default)]
    pub read: Vec<SecurityRule>,
    #[serde(default)]
    pub write: Vec<SecurityRule>,
    #[serde(default)]
    pub delete: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectorySecurityRules {
    #[serde(default)]
    pub read: Vec<SecurityRule>,
    #[serde(default)]
    pub create: Vec<SecurityRule>,
    #[serde(default)]
    pub delete: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessSecurityRules {
    #[serde(default)]
    pub exec: Vec<SecurityRule>,
    #[serde(default)]
    pub spawn: Vec<SecurityRule>,
    #[serde(default)]
    pub kill: Vec<SecurityRule>,
    #[serde(default)]
    pub suspend: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkSecurityRules {
    #[serde(default)]
    pub request: Vec<SecurityRule>,
    #[serde(default)]
    pub download: Vec<SecurityRule>,
    #[serde(default)]
    pub upload: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HardwareSecurityRules {
    #[serde(default)]
    pub i2c: Vec<SecurityRule>,
    #[serde(default)]
    pub spi: Vec<SecurityRule>,
    #[serde(default)]
    pub gpio: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistrySecurityRules {
    #[serde(default)]
    pub read: Vec<SecurityRule>,
    #[serde(default)]
    pub write: Vec<SecurityRule>,
    #[serde(default)]
    pub delete: Vec<SecurityRule>,
}
