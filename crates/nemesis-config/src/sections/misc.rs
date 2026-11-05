// 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

/// checkpoint gc 配置（`config.json` 的 `"checkpoint"` 段；
///
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CheckpointConfig {
    /// gc 参数（见 [`CheckpointGcConfig`]）。
    pub gc: CheckpointGcConfig,
}

/// checkpoint gc 参数。**默认关**：gc 删错 = 回退网失效，宁默认关、
/// 开关在用户手里（长任务开发建议显式开启）。git 影子形态走**重建式**
/// gc（新建临时对象集 → 完整性自检 → 原子换名 `objects/` 子树，不碰主仓
/// `.git/objects`、不区分 loose/pack）；JSON 形态窗口外 turn-N.json 直删。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CheckpointGcConfig {
    /// 总闸（false 默认：gc 全路径零触碰）。
    pub enabled: bool,
    /// 按 turn 序号保留最近 N 个（0 = 关闭该维）。默认 200。
    pub keep_turns: usize,
    /// 按时间保留最近 N 天（0 = 关闭该维）。默认 7。
    pub keep_days: u64,
}

impl Default for CheckpointGcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            keep_turns: 200,
            keep_days: 7,
        }
    }
}

/// 紫微系统配置（`config.json` 的 `"ziwei"` 段；总控）。
///
/// `enabled` 默认 false：总闸关 = WSAPI 全拒 + CLI 子命令诚实报未启用 +
/// payload 嵌入字节不落盘（feature 关则三 crate 连同 payload 整体不编译）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ZiweiConfig {
    /// 总闸（false 默认：全部命令诚实拒绝）。
    pub enabled: bool,
    /// exe 路径/sha256 白名单；空 = 仅配置期可附加（使用期附加全弹卡）。
    #[serde(default)]
    pub allowed_targets: Vec<String>,
    /// payload DLL 落位目录（None = `<workspace>/tools/ziwei/`，bot 内嵌
    /// 落盘——决议 9）。
    #[serde(default)]
    pub payload_dir: Option<String>,
    /// 同时附加的会话上限。
    pub max_sessions: u32,
    /// 配置期快照保留天数（决议 3）。
    pub snapshots_ttl_days: u32,
    /// 使用期数据回流保留天数（对齐 uploads 惯例）。
    pub runs_ttl_days: u32,
}

impl Default for ZiweiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_targets: Vec::new(),
            payload_dir: None,
            max_sessions: 4,
            snapshots_ttl_days: 30,
            runs_ttl_days: 7,
        }
    }
}

/// connector 注册表配置（`config.json` 的 `connectors` 段；`#[serde(default)]`
/// 每字段全可省——老 config.json 零迁移）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectorsConfig {
    /// 总闸（false = connector_find/connector_invoke 两工具不上供给面）。
    pub enabled: bool,
    /// per-connector 每小时执行上限（0=限频关闭）。
    pub max_ops_per_hour: u32,
    /// 资金类限额（None = 全 0 = fail-closed 拒绝所有资金 op）。
    #[serde(default)]
    pub finance: Option<ConnectorsFinanceConfig>,
    /// 远程执行面总闸（集群：worker 节点是否接受 master 经 RPC 派发的
    /// `connector_invoke`；false = worker 侧 handler 诚实拒绝。缺省 true）。
    /// 凭据铁律不变：派发载荷只含 op+args，凭据永不上 RPC 线——worker 用
    /// 自己的 vault 解锁。
    #[serde(default = "default_remote_invoke_enabled")]
    pub remote_invoke_enabled: bool,
    /// 信任总闸：true = unsigned connector 一律拒执行（trust_blocked
    /// 诚实凭条）；false（缺省）= 只在审批卡加警示行，不拦截——本地自签
    /// 开发场景合法，拦截是用户显式选择。
    #[serde(default)]
    pub require_signed: bool,
    /// script 绑定全局配置（None = 全缺省：default_timeout_secs 60）。
    #[serde(default)]
    pub script: Option<ConnectorsScriptConfig>,
}

fn default_remote_invoke_enabled() -> bool {
    true
}

impl ConnectorsConfig {
    /// script 绑定全局缺省超时秒数（op 级 timeout_secs 可覆盖；**0 = 拒绝
    /// 所有 script op**——防误配无限挂；上限 600 与 exec 工具闸对齐）。
    pub fn script_timeout_secs(&self) -> u64 {
        self.script
            .as_ref()
            .map(|s| s.default_timeout_secs)
            .unwrap_or(60)
    }
}

impl Default for ConnectorsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_ops_per_hour: 0,
            finance: None,
            remote_invoke_enabled: true,
            require_signed: false,
            script: None,
        }
    }
}

/// script 绑定全局配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectorsScriptConfig {
    /// 全局缺省超时秒数（0 = 拒绝所有 script op）。
    pub default_timeout_secs: u64,
}

impl Default for ConnectorsScriptConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: 60,
        }
    }
}

/// 资金类限额（fail-closed：0 = 拒绝所有——不设限 = 不成交）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectorsFinanceConfig {
    /// 单笔数量上限（0 = 拒绝所有资金 op）。
    pub max_order_qty: f64,
    /// 日累计笔数上限（0 = 拒绝所有资金 op；UTC 自然日滚转）。
    pub max_orders_per_day: u32,
}

impl Default for ConnectorsFinanceConfig {
    fn default() -> Self {
        Self {
            max_order_qty: 0.0,
            max_orders_per_day: 0,
        }
    }
}

/// 皮肤子系统策略（`config.json` 的 `ui.skins` 段；`#[serde(default)]`
/// 每字段全可省——老 config.json 零迁移）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SkinsPolicy {
    /// true = 「设为默认观感」（WSAPI `skins.set_active`）拒绝非 verified
    /// 皮肤包。闸在**信任决策点**——数据面（active.css / app 分发）不拦，
    /// 签名仍是徽标而非加载闸（定案），这里只约束「把哪个包设为默认」
    /// 这一个动作。默认 false（未签名皮肤照常可用）。
    pub require_signed: bool,
    /// true = 皮肤脚本载荷（manifest `script`）可经 `/skins/active/script`
    /// 分发。**脚本能力总闸**（默认 false）：关 = 端点 403，脚本不进前端。
    /// 开 ≠ 免许可——每个带脚本包首次激活仍需用户逐包同意（consent 卡，
    /// `skins.script_consent` 记账）。签名状态照旧只是徽标（同意卡展示
    /// ⚠/🚫 但不拦截）。
    pub allow_scripts: bool,
}

/// Dashboard 前端表现配置（`config.json` 的 `ui` 段）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiConfig {
    /// 激活皮肤 id。`"default"`（默认）/ 空 = 不启用任何 `.nbskin`（内置
    /// static 皮肤原样）；其余值 = 服务端从 exe 同级 `skins/<id>.nbskin`
    /// 加载并在 `/skins/active.css` 提供 CSS（前端注入后打
    /// `<html data-skin="<id>">`）。Dashboard 设置页「皮肤」tab 写回此处。
    #[serde(default)]
    pub skin: String,
    /// 皮肤子系统策略段（require_signed 后手开关等）。
    #[serde(default)]
    pub skins: SkinsPolicy,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            skin: "default".to_string(),
            skins: SkinsPolicy::default(),
        }
    }
}

/// 反向桥配置（`config.json` 的 `bridge` 段；`#[serde(default)]` 每字段全可省）。
/// 分服务端/客户端两侧：服务端 = 中继接入门（`--relay` 与正常启动共用）；
/// 客户端 = 本机作为桥设备连远端中继服务端（goal：节点显示名 + 反向桥）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BridgeConfig {
    /// 服务端角色配置（`/bridge` 接入门）。
    pub server: BridgeServerConfig,
    /// 客户端角色配置（桥出站连接）。
    pub client: BridgeClientConfig,
}

/// 服务端接入门配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BridgeServerConfig {
    /// 接入门 ws token（预共享钥匙的服务端半把；与桥客户端 `client.token`
    /// 同值——同一把钥匙的两端各自配置、永不交换、hello 帧校验、不走 URL
    /// query）。**空 = 接入门不开放**（fail-closed：不挂 /bridge 等桥路由）；
    /// `--relay` 启动时此处为空 → 拒绝启动并提示。
    pub token: String,
}

/// 客户端（桥设备）配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BridgeClientConfig {
    /// 客户端总开关（false = 零行为变化，不连中继）。
    pub enabled: bool,
    /// 中继服务端地址（如 `ws://vps.example.com:60600`；客户端自动拼
    /// `/bridge` 路径）。
    pub relay_url: String,
    /// 接入门 token（与远端服务端 `server.token` 同值）。
    pub token: String,
    /// 面板访问密码（**只存本机**，服务端零存储——VPS 被拿下也翻不出任何
    /// 设备密码；远程面板访问经 access_check 帧由本机比对哈希）。空 =
    /// 拒绝所有远程面板访问（fail-closed）。本地直连面板不受影响（两回事）。
    pub access_token: String,
}

/// PTY 内嵌终端配置（`config.json` 的 `terminal` 段；。交互 shell 无法
/// 逐命令审批——默认关闭，启用即接受「dashboard 登录用户=shell 信任级」
/// +全 I/O 审计 `<workspace>/logs/terminal/`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalConfig {
    /// 终端总开关（默认 false；启用还需 `terminal` cargo feature 在场）。
    pub enabled: bool,
    /// 并发 PTY 会话上限（满员拒绝新连接；默认 4）。
    pub max_sessions: u32,
    /// shell 覆盖（None=Windows powershell / Unix $SHELL >/bin/sh）。
    pub shell: Option<String>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_sessions: 4,
            shell: None,
        }
    }
}

/// 日志五族 TTL 清扫（`config.json` 的 `logs` 段；/ 统一清扫器，
/// ）。每族 = 保留天数，`0 = 该族关闭清扫`；缺键 =
/// 全默认。清扫器（nemesisbot logs_sweeper）启动扫一次 + 每 6h 一拍，
/// 每族每拍限量 200 件防 IO 尖峰。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LogsTtlConfig {
    /// 对话历史 JSONL（`logs/session_logs/`，含.meta.json sidecar 配对
    /// 同删）。默认 30。
    pub session_logs: u64,
    /// 轮次边界事件 sidecar（`logs/boundary/`）。默认 30。
    pub boundary: u64,
    /// 主 Agent LLM 请求日志会话目录（`logs/request_logs/{ts}_{rand}/`）。
    /// 默认 14。
    pub request_logs: u64,
    /// PTY 终端全 I/O 审计（`logs/terminal/`，**含回显密码**，敏感面最短）。
    /// 默认 7。
    pub terminal: u64,
    /// 对话账本（`logs/event_ledger/`；**删除条件 = 超龄 且 末行
    /// op==append**——truncate/fork 行携带 chat_log 的唯一旁证正文，
    /// 末行非 append 保留）。默认 30。
    pub event_ledger: u64,
    /// 会话回收站（`logs/trash/sessions/`，整目录清扫，目录 mtime ≈ 删除
    /// 时刻）。默认 **90**（30 天太短）。
    pub trash: u64,
}

impl Default for LogsTtlConfig {
    fn default() -> Self {
        Self {
            session_logs: 30,
            boundary: 30,
            request_logs: 14,
            terminal: 7,
            event_ledger: 30,
            trash: 90,
        }
    }
}

/// 项目注册表参数（`config.json` 的 `projects` 段； 。
/// `#[serde(default)]` 每字段全可省）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectsConfig {
    /// 项目数上限（每项目 = 一个常驻 AgentLoop，上限即可预期的资源面；
    /// 默认 4。`config set_field projects.max` 经 typed round-trip 自动可用）。
    pub max: usize,
    /// **未受信**项目（注册表条目 `instructions_trusted:
    /// false`，`projects.trust` 翻转）的工作区指令处理策略——`""`/`annotate`
    /// = 注入但前置不可信来源横幅（缺省）；`isolated` = 整段跳过；未知值
    /// lenient 按 annotate 处理 + warn。受信条目恒 verbatim 注入（逐项目
    /// 显式信任是用户最具体的意图，全局策略不降级它）。空串不序列化
    /// （`skip_serializing_if`）——缺省态 typed 保存字节零变化（role 先例）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instruction_trust: String,
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self {
            max: 4,
            instruction_trust: String::new(),
        }
    }
}

/// 使用统计保留策略（`config.json` 的 `usage` 段；`#[serde(default)]`
/// 每字段全可省）。gateway 周期任务（启动 + 每 6h）对 request_logs 执行
/// retention sweep：到期行 rollup 进 daily_rollups 后删除；条数超限按
/// 最旧优先直接裁。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UsageConfig {
    /// 明细行保留天数（0 = 不按天清理，只 rollup？——0 视为「永不按天
    /// 清理」；默认 30）。rollup 聚合不受此值影响：到期行先聚合再删。
    pub retention_days: i64,
    /// 明细行条数上限（0 = 无上限；默认 500_000）。超出按 created_at
    /// 最旧优先直接删除。
    pub max_rows: i64,
}

impl Default for UsageConfig {
    fn default() -> Self {
        Self {
            retention_days: 30,
            max_rows: 500_000,
        }
    }
}
/// Debug/diagnostic switches. Currently only `capture` — the failure-triggered
/// diagnostic capture sink for the intermittent "context error → session
/// corruption → LLM no-response" bug. Capture writes only on failure signals,
/// so the happy path has zero disk overhead; it defaults to enabled.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebugConfig {
    #[serde(default)]
    pub capture: CaptureConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// Enable failure-triggered diagnostic capture (writes to
    /// `logs/capture/{session_key}/{ts}_{signal}/` only when a failure signal
    /// fires — LLM retry exhausted / context overflow / session overwrite /
    /// agent error funnel).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

// ============================================================================
// Agents Config
// ============================================================================
/// 健康监测旗标（`config.json` 的 `health` 段）。
/// None = 全默认——磁盘水位监测开（warn 5GB / critical 1GB / 60s）。
/// critical 臂只升级告警面（SSE `system-alert` + `/ready` 降级），**不做
/// 自动 estop**（防误伤）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HealthFlagConfig {
    /// 磁盘水位监测（workspace 所在卷剩余空间）。
    pub disk_watermark: DiskWatermarkConfig,
}

/// `health.disk_watermark`：workspace 所在卷剩余空间监测参数。字段全可省。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DiskWatermarkConfig {
    /// 总开关（默认开——「快满了」可见是 的最小代价最大收益项）。
    pub enabled: bool,
    /// warn 阈值（GB；低于此值升 warn 告警）。
    pub warn_gb: u64,
    /// critical 阈值（GB；低于此值升 critical——回退网/历史落盘即将失效）。
    pub critical_gb: u64,
    /// 监测周期秒数（默认 60；0 视作缺省防呆，与 format_on_save 同口径）。
    pub interval_secs: u64,
}

/// 磁盘水位监测周期缺省秒数（serde 缺省 / Default / 0 防呆共用）。
pub fn default_disk_watermark_interval_secs() -> u64 {
    60
}

impl Default for DiskWatermarkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            warn_gb: 5,
            critical_gb: 1,
            interval_secs: default_disk_watermark_interval_secs(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_heartbeat_interval_minutes")]
    pub interval: i64,
}

impl Default for HeartbeatConfig {
    /// 手写而非 derive（serde 双路径纪律，WebChannelConfig 同款——
    /// ）：`heartbeat` 顶层键整段缺席时 serde 走结构体
    /// `Default::default`，字段级缺省函数不经过。derive 的
    /// {enabled:false, interval:0} 与字段级缺省/出厂模板/default_config
    /// 的 {true, 30} 相反——migrate 骨架（openclaw_config.rs 无 heartbeat
    /// 段）与手精简 config 的心跳服务被静默关闭（bot_service.rs 装配闸
    /// 直接读 enabled）。两条构造路径必须收敛同值。
    fn default() -> Self {
        Self {
            enabled: true,
            interval: default_heartbeat_interval_minutes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicesConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub monitor_usb: bool,
}

impl Default for DevicesConfig {
    /// 手写收敛（同批）：`monitor_usb` 字段级缺省 true，
    /// derive 给 false——devices 服务缺省关使分歧暂不可达，但用户经
    /// Dashboard 开启 enabled 保存的是内存结构体（缺席键场景=derive 值），
    /// monitor_usb:false 会落盘，与模板 monitor_usb:true 相反。
    fn default() -> Self {
        Self {
            enabled: false,
            monitor_usb: true,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityFlagConfig {
    #[serde(default)]
    pub enabled: bool,
}
/// 用户习惯画像配置（B 线）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserProfileConfig {
    /// 默认关：开启后画像自动学习并注入（仅向前，不回溯）。
    #[serde(default)]
    pub enabled: bool,
    /// 置信度阈值（缺省 0.7）。
    #[serde(default)]
    pub confidence_threshold: Option<f64>,
    /// 注入条目上限（缺省 10）。
    #[serde(default)]
    pub max_entries: Option<usize>,
    /// 衰减天数（缺省 30：active→decaying；再 30 天→expired，两段同值）。
    #[serde(default)]
    pub decay_days: Option<i64>,
    /// 统计扫描间隔秒数（缺省 3600 = 每小时）。
    #[serde(default)]
    pub stats_scan_interval_secs: Option<u64>,
    /// 反思通道触发间隔（每 N 次扫描一次，缺省 20）。
    #[serde(default)]
    pub reflect_every_sweeps: Option<u32>,
}

impl UserProfileConfig {
    /// 置信度阈值（缺省回落 user_profile 模块常量）。
    pub fn confidence_threshold_or(&self, default: f64) -> f64 {
        self.confidence_threshold.unwrap_or(default)
    }
    /// 注入条目上限（缺省 10；0 = 回落缺省，上限不允许关闭）。
    pub fn max_entries_or(&self, default: usize) -> usize {
        self.max_entries.filter(|m| *m > 0).unwrap_or(default)
    }
    /// 衰减天数（缺省 30）。
    pub fn decay_days_or(&self, default: i64) -> i64 {
        self.decay_days.filter(|d| *d > 0).unwrap_or(default)
    }
    /// 扫描间隔秒数（缺省 3600；非法值 0/负 = 回落缺省）。
    pub fn stats_scan_interval_secs_or(&self, default: u64) -> u64 {
        self.stats_scan_interval_secs
            .filter(|s| *s > 0)
            .unwrap_or(default)
    }
    /// 反思间隔 sweep 数（缺省 20；0 = 回落缺省，不允许关掉反思闸）。
    pub fn reflect_every_sweeps_or(&self, default: u32) -> u32 {
        self.reflect_every_sweeps
            .filter(|n| *n > 0)
            .unwrap_or(default)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForgeFlagConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClusterFlagConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryFlagConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 记忆 dreaming（三段式记忆巩固；None = 全默认——enabled=false 关）。
    #[serde(default)]
    pub dreaming: Option<DreamingConfig>,
}

/// `memory.dreaming` 配置节（记忆巩固）。
///
/// sweep 挂载于 gateway 启动同步（job 名 `memory-dreaming:sweep`）；LLM 决策
/// 走 `agents.small_model` 通道（未配则回落主模型）。默认全关——dreaming 是
/// 自主改写记忆的行为，opt-in。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamingConfig {
    /// 总开关，默认 false（opt-in）。
    #[serde(default)]
    pub enabled: bool,
    /// sweep cron 表达式（5/6 字段，本地时间），默认每日 03:30。
    #[serde(default = "default_dreaming_cron")]
    pub cron: String,
    /// 每次 sweep 送 LLM 决策的候选条目上限（按 6 信号加权分取 topK）。
    #[serde(default = "default_dreaming_top_k")]
    pub top_k: usize,
    /// 6 信号权重逐项覆盖（None = 内置默认；未给的项用默认值）。
    #[serde(default)]
    pub weights: Option<DreamingWeightsConfig>,
}

impl Default for DreamingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cron: default_dreaming_cron(),
            top_k: default_dreaming_top_k(),
            weights: None,
        }
    }
}

/// 6 信号权重覆盖节。全部 Option——只覆盖用户显式给出的项。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DreamingWeightsConfig {
    /// 召回次数权重。
    #[serde(default)]
    pub recall: Option<f64>,
    /// 时间衰减（久未召回）权重。
    #[serde(default)]
    pub decay: Option<f64>,
    /// 条目冲突权重。
    #[serde(default)]
    pub conflict: Option<f64>,
    /// 冗余度权重。
    #[serde(default)]
    pub redundancy: Option<f64>,
    /// 年龄权重。
    #[serde(default)]
    pub age: Option<f64>,
    /// 来源可靠性（不可靠→更需审查）权重。
    #[serde(default)]
    pub source: Option<f64>,
}

fn default_dreaming_cron() -> String {
    "30 3 * * *".to_string()
}

fn default_dreaming_top_k() -> usize {
    8
}
