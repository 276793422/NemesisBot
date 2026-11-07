// 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

// 跨子文件引用（机械拆分）：serde default helper 定义在 agents.rs
use super::agents::default_worker_max_inflight;

/// 停车场兜底派发档位（`board.dispatch_fallback`；用户裁决：
/// 角色缺失是集群常态，默认不停车但仍保角色纪律）。三档语义：
/// - `Off` = 无人匹配即诚实停车等节点（旧 false）。
/// - `Role`（默认）= 松弛阶梯走到「丢职能」为止（②保职能丢标签 →
///   ③丢职能保角色标签）：职能与随行的 tier 门槛放开，但 worker/coordinator
///   **角色纪律保留**（架构单可降级给开发，不可降级给 coordinator）。
/// - `Full` = 阶梯全开 ②③④，角色也放开（旧 true）。
///
/// serde 兼容：旧配置布尔 false→`Off`、true→`Full`；字符串 `off`/`role`/
/// `full` 大小写不敏感；序列化恒为小写字符串。缺省键 = `Role`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DispatchFallbackMode {
    Off,
    #[default]
    Role,
    Full,
}

impl Serialize for DispatchFallbackMode {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let s = match self {
            Self::Off => "off",
            Self::Role => "role",
            Self::Full => "full",
        };
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for DispatchFallbackMode {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = DispatchFallbackMode;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("\"off\"|\"role\"|\"full\" 或旧布尔 false/true")
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                v: bool,
            ) -> std::result::Result<Self::Value, E> {
                // 旧布尔兼容：false=停车、true=全松弛（旧行为字节保真）。
                Ok(if v {
                    DispatchFallbackMode::Full
                } else {
                    DispatchFallbackMode::Off
                })
            }
            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                match v.trim().to_ascii_lowercase().as_str() {
                    "off" | "false" => Ok(DispatchFallbackMode::Off),
                    "role" => Ok(DispatchFallbackMode::Role),
                    "full" | "true" => Ok(DispatchFallbackMode::Full),
                    other => Err(E::invalid_value(
                        serde::de::Unexpected::Str(other),
                        &"off | role | full",
                    )),
                }
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// 自治模式总开关（默认 `Assisted` = 现状零回归）。
/// 与 `unlimited_mode` 正交并存：unlimited 管**评审处置环**自动化
/// （FAIL 预算 ∞ 与 UNSURE 继续重派），autonomy 管**监察环**自动化
/// （超时/离线自动重派与触发式重估）。
///
/// 矛盾态（autonomous 且 unlimited=false）在两处拦截：config 加载期
/// （`post_process_for_compatibility`）诚实降级为 assisted 并 WARN
/// （安全方向回退，不炸网关——沙盒未就绪回退 Layer 1 同款先例）；
/// 面板保存（`board.config.set`）硬拒。estop 与外部熔断权不受此字段
/// 影响（不变量）；unlimited 的预算 ∞ 语义**不联动**到 sweep 监察环
/// （监察环预算是独立硬防线）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoardAutonomy {
    /// 辅助模式（默认）：一切建议转人工（现状行为，零回归）。
    #[default]
    Assisted,
    /// 自治模式：监察环策略自动裁决 + 事后审计评论留痕。
    Autonomous,
}

impl Serialize for BoardAutonomy {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let s = match self {
            Self::Assisted => "assisted",
            Self::Autonomous => "autonomous",
        };
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for BoardAutonomy {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = BoardAutonomy;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("\"assisted\" | \"autonomous\"")
            }
            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                match v.trim().to_ascii_lowercase().as_str() {
                    "assisted" => Ok(BoardAutonomy::Assisted),
                    "autonomous" => Ok(BoardAutonomy::Autonomous),
                    other => Err(E::invalid_value(
                        serde::de::Unexpected::Str(other),
                        &"assisted | autonomous",
                    )),
                }
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// 看板系统旗标（`config.json` 的 `board` 段；`#[serde(default)]` 每字段全可省）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardFlagConfig {
    /// 派发超时兜底秒数：dispatched 超过该时长未回报 → sweep 标 failed +
    /// 评论 + dispatch_failed 通知（0 = 关闭 sweep）。
    pub dispatch_timeout_secs: u64,
    /// sweep 扫描间隔秒数（默认 20s；不暴露到配置模板，供测试调快）。
    pub dispatch_sweep_interval_secs: u64,
    /// 指派后自动派发（.5 接口预留；默认 false = 不自动派发）。
    /// 现阶段不做自动派发，但留出接口——置 true 后，
    /// `issue.assign` 指派给 worker 成功即自动走派发单一入口
    /// （dispatch_issue_core；失败不回滚指派，⛔ 系统评论留痕）。
    pub auto_dispatch: bool,
    /// 频道讨论区配置（`board.discussion`）。
    pub discussion: BoardDiscussionConfig,
    /// 每日备份配置（`board.backup`）。
    pub backup: BoardBackupConfig,
    /// 任务拆解配置（`board.plan`；Swarm）。
    pub plan: BoardPlanConfig,
    /// 任务档案导出配置（`board.dossier`）。
    pub dossier: BoardDossierConfig,
    /// in_review 自动触发验收 agent（Swarm +；默认 true）。删除本键即回
    /// 「纯人工验收」。
    pub auto_review: bool,
    /// 验收 PASS 是否自动 done（Swarm +；默认 false = 人工签字放行）。
    pub auto_accept: bool,
    /// FAIL 重派上限，超次转 UNSURE 等人工（Swarm）。
    pub max_redispatch: u32,
    /// 子单全部落定后父单自动收口（全自动流转；默认 false = 停 in_review
    /// 等人工）。true 时父单进 in_review 即触发一次汇总验收（输入=全部子单
    /// 结论 + 各子单最新交付汇报），PASS 自动 done；FAIL/UNSURE 转人工。
    /// 存在 cancelled 子单的父单不适用（范围缺口必须人工裁决）。
    pub auto_close_parent: bool,
    /// 无限模式（全自动流转；用户裁定；默认 false = 护栏生效）。
    /// true：max_redispatch 视为 ∞（FAIL 无限重派）、UNSURE 带「无法定案」
    /// 意见继续重派——但 B 语义变化（批准）：UNSURE 连续达到
    /// `board.review.max_unsure_streak`（默认 2，0=不限恢复完全旧行为）转
    /// 人工——unlimited 放大的是重派次数不设限，不是无收敛重派授权。
    /// estop 急停不受影响（保险丝非护栏）。停滞仍可观测：连续重派且差距无
    /// 变化时 WARN 告警（只告警不停）。
    pub unlimited_mode: bool,
    /// 自治模式总开关（默认 `Assisted` = 现状零回归）。见
    /// [`BoardAutonomy`] 类型文档：与 unlimited_mode 正交并存，矛盾态在
    /// 加载期降级 / 面板保存硬拒两处拦截。
    pub autonomy: BoardAutonomy,
    /// 停车场兜底派发档位（集群完备性加固 引入； 三档化，
    /// 默认 `Role` = 角色缺失是常态：职能/tier 门槛随职能放开，但角色纪律
    /// 保留，不停车）。`Off` = 无人匹配即停车等节点；`Full` = 全松弛（角色
    /// 也放开）。所有档位：派发前落 ⚠ 系统评论留痕（松弛级说明）；无任何
    /// 在线节点时仍诚实停车（兜底造不出客户端）；estop 急停冻结兜底派发；
    /// 钉住目标（`dispatch_fallback_target`）不在线 = 停车不悄悄换人。
    pub dispatch_fallback: DispatchFallbackMode,
    /// 兜底客户端钉住（`dispatch_fallback` 开时生效；按节点 name 或 id 匹配，
    /// 大小写不敏感）。Some 且该节点不在线 = 诚实停车**不悄悄换人**（钉住
    /// 即点名）；None = 在线节点里松弛排序自动选。
    pub dispatch_fallback_target: Option<String>,
    /// 派发准入控制（goal /；默认 1）：自动派发链里，目标 worker 的
    /// 在途派发数达到该上限 → 本单不派、留 backlog——新节点上线后由 sweep
    /// 自然承接（防「同波多就绪单全挤单 worker、后上线的设备接不到活」）。
    /// 0 = 不限（旧行为）。人工 `issue.dispatch` 显式派发不受此闸（人类明确
    /// 意图，后果自担）。
    #[serde(default = "default_worker_max_inflight")]
    pub worker_max_inflight: i64,
    /// 验收 agent 行为配置（全自动流转；`board.review` 段）。
    pub review: BoardReviewConfig,
    /// 自动流转预算保险丝（全自动流转；`board.budget` 段；0 = 该项关闭）。
    pub budget: BoardBudgetConfig,
    /// 项目档案传输护栏（看板项目档案 goal /；`board.archive` 段）。
    pub archive: BoardArchiveConfig,
    /// 合并冲突自动处置（看板项目档案 goal /；默认 false = human 档：
    /// 项目域冻结等人工解，`project.resume` 续行）。true = auto 档：AI 硬解
    /// →机械失败重派原 worker→离线三轮接触→换人——项目不停摆为第一原则，
    /// 重派循环套既有预算保险丝打满转人工。开关只控制冲突处置策略，不改
    /// 变影响域（冻结始终限本项目，不株连其他项目）。
    pub conflict_auto_resolve: bool,
    /// 重派决策量化（默认 false = 换节点重派维持匹配器
    /// 决策表现状行为字节等价）。true：换节点候选在匹配器排序之上叠加
    /// worker × 任务类型历史成功率指纹三档权重（prefer 提前 / avoid 靠后，
    /// 非绝对排除；样本 <3 一律 neutral），重派换节点评论带档位留痕。
    /// 记账与开关解耦（评审定案照常入账，灰度期攒数据，开闸即有历史）。
    pub fingerprint_weighting: bool,
    /// LLM 端点不可用期封存（U-B 断网封存；`board.unavailable` 段）。
    pub unavailable: BoardUnavailableConfig,
    /// 重规划器配置（`board.replan` 段）。
    pub replan: BoardReplanConfig,
    /// worker 执行面配置（`board.worker` 段；B36 工具供给裁剪开关）。
    pub worker: BoardWorkerConfig,
}

impl Default for BoardFlagConfig {
    fn default() -> Self {
        Self {
            dispatch_timeout_secs: 3600,
            dispatch_sweep_interval_secs: 20,
            auto_dispatch: false,
            discussion: BoardDiscussionConfig::default(),
            backup: BoardBackupConfig::default(),
            plan: BoardPlanConfig::default(),
            dossier: BoardDossierConfig::default(),
            auto_review: true,
            auto_accept: false,
            max_redispatch: 2,
            auto_close_parent: false,
            unlimited_mode: false,
            autonomy: BoardAutonomy::Assisted,
            dispatch_fallback: DispatchFallbackMode::default(),
            dispatch_fallback_target: None,
            worker_max_inflight: 1,
            review: BoardReviewConfig::default(),
            budget: BoardBudgetConfig::default(),
            archive: BoardArchiveConfig::default(),
            conflict_auto_resolve: false,
            fingerprint_weighting: false,
            unavailable: BoardUnavailableConfig::default(),
            replan: BoardReplanConfig::default(),
            worker: BoardWorkerConfig::default(),
        }
    }
}

/// worker 执行面配置（`board.worker` 段；B36）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardWorkerConfig {
    /// 职能工具供给裁剪开关（D5 拍板：做成开关，默认 false = 零行为变化）。
    /// true 时 B 端按职能 CATALOG 声明的 `tool_trim` 清单从执行面**补集**
    /// 白名单里裁掉对应工具（声明常在、开闸才生效——关闸时 CATALOG 声明
    /// 不产生任何行为）。经 config mtime 热感知（check_config_reload 同链），
    /// 改动无需重启。
    pub tool_trim: BoardToolTrimConfig,
}

/// 职能工具供给裁剪（`board.worker.tool_trim` 段）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardToolTrimConfig {
    /// 开关本体（默认 false = 零行为变化，D5）。
    pub enabled: bool,
}

/// 重规划器配置（`config.json` 的 `board.replan` 段）。
/// 重规划 = 事件驱动的**建议**产出（proposal ≠ authorization）：四个触发
/// 点（重派无意义失败停车 / 父评审 FAIL / 冻结类停车 / FINISH 谓词被
/// unknown 阻塞超时）命中时收集证据 → LLM 起草结构化建议 → 落看板系统
/// 评论。assisted 模式建议只落评论 @创建者审批；autonomous 模式过策略闸
/// （下方硬约束）后才自动执行，任一闸不通过 = 冻结 + 审计评论 + 站内
/// 通知——宁可停不可闯。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardReplanConfig {
    /// 重规划建议总开关：false = 触发点命中时静默（不起草不评论）。
    pub enabled: bool,
    /// 自治执行策略闸：单次重规划新增子单数上限（超出 = 拒绝执行，
    /// 冻结转人工）。
    pub max_new_subissues: u32,
    /// 自治执行策略闸：是否允许取消「已 done 之外」的子单（默认 false =
    /// 任何针对非终态子单的 cancel 建议都不自动执行，冻结转人工）。
    pub allow_cancel_active: bool,
    /// 触发点④ 超时秒数：子树全终态但 FINISH 谓词被未裁决 unknown 阻塞
    /// 超过该时长 → 起草重规划建议。0 = 关（默认关）。纯事件驱动在全员
    /// 终态后无事件可挂（终态单不再产生 settle 事件），靠派发 sweep
    /// ticker 周期体检兜底。
    pub unknown_block_timeout_secs: u64,
}

impl Default for BoardReplanConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_new_subissues: 6,
            allow_cancel_active: false,
            unknown_block_timeout_secs: 0,
        }
    }
}

/// 验收 agent 行为配置（`board.review` 段；全自动流转）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardReviewConfig {
    /// 执行型验收轮数上限（默认 1 = 只跑一轮纯文本验收，与历史行为字节
    /// 等价）。>1 且最近一次派发的 worker 是本机节点时，验收 agent 以只读
    /// 工具白名单最多执行 N 轮取证后再下结论；远端 worker 一律纯文本
    /// （安全闸）。
    pub max_turns: u32,
    /// 验收需要更多证据时允许向执行 worker 发起一轮自检（默认 false）。
    /// worker 在其安全 8 层内按证据请求取证，回报后触发二段验收。
    pub selfcheck: bool,
    /// 项目验收：项目下全部顶层父单 done 后自动汇总验收，PASS 自动
    /// completed（默认 false = completed 仅人工/归档脚本设置）。FAIL/UNSURE
    /// 只写缺口评论 + 回滚 completed→in_progress，不自动重开父单。
    pub auto_close_project: bool,
    /// 多检查员面板（全自动流转）：每次验收并行发出的评审路数
    /// （默认 1 = 单检查员，与历史行为字节等价）。>1 时 N 路独立评审多数
    /// 投票聚合（平票/无多数 → Unsure 转人工，fail-safe 不偏向自动收货）；
    /// 消费侧收敛上限 5，并发上限 4。
    pub checkers: u32,
    /// 评审档位（gap ②：评审分层；"thorough" | "fast"）。thorough（默认）
    /// = 全量纪律 + 取证 + 经验蒸馏（渲染字节与历史一致）；fast = 精简档
    /// （保留注入防线与三态 JSON 契约，关闭取证/经验）。无法识别的值安全
    /// 回落 thorough（宁可全量也不放水）。
    pub tier: String,
    /// exec: 锚点执行开关（+ 激活；代码默认 false = 安全缺省不变，
    /// 三机环境配置开启）。开启后 `[CHECK] exec:<命令>` 在锚点根目录以子进程
    /// 执行，退出码 0 = 过。planner 对可运行时验证的子单应产出此形态（
    /// 改动二），危险命令形态解析期黑名单拒绝；环境依赖缺失诚实跳过不进
    /// 失败集（探针闸）。
    pub exec_anchors_enabled: bool,
    /// exec: 锚点单条执行超时秒数（默认 120；超时 = 不过 + kill）。
    pub exec_anchor_timeout_secs: u64,
    /// 评审自身失败自动重评次数（默认 1，**0 = 关**回旧行为——评审
    /// LLM 调用失败恒转人工）。评审 LLM 失败（非 verdict）时延迟自动重评，
    /// 全部重评仍失败才转人工。约束：estop / 单据状态已变则不重试（尊重
    /// 迟到评审守卫）；每次重评审计评论留痕。与 fail_class 重派方向相反
    ///重评不重派，评审侧故障不放大成 worker 重写循环。
    pub failure_retries: u32,
    /// UNSURE 连续转人工阈值（B；默认 **2**，**0 = 不限**=旧行为）。
    /// 同签名 UNSURE 连续达到阈值 → 复用 EscalateHuman 路径转人工（自带
    /// 清账 + 审计 + reasons 评论）。语义：给一次 worker 自愈机会，超出
    /// 即认定「只有人能解」（第 4 轮实证）。unlimited_mode 下 UNSURE
    /// 处置从「无限重派」限缩为「同签名连续 2 轮转人工」（
    /// 用户批准的语义变化）。
    pub max_unsure_streak: u32,
    /// 职能→评审预设查表（职能能力演进；`board.review.professions`
    /// 嵌套 map，键=职能 slug，大小写不敏感）。查表三层覆盖：职能预设 >
    /// 本结构全局字段 > 内置默认；slug 先精确匹配再裸 family 回落
    /// （`dev:cpp` 无精确键时落 `dev` 键——自定义 spec 项继承 family 预设）。
    /// 代码内置预设：`product`（max_turns=0 + checkers=2 + selfcheck=true，
    /// 需求源头单全维加强）、`test-blackbox`（checkers=2，人工级防线双
    /// 检查员防单评委漏判）——config 同名键**逐维度覆盖**内置预设。
    /// 缺键零迁移（`#[serde(default)]`，空 map = 全部走全局/内置）。
    #[serde(default)]
    pub professions: std::collections::HashMap<String, BoardProfessionReviewPreset>,
    /// 父单收口 FAIL 自动返工承接轮数上限（默认 **2**，**0 = 关**
    /// = 旧行为恒转人工）。FAIL（差距具体）时自动创建「返工：」子单承接差距
    /// 并派发，返工单落定后 sync 链自动复评父单；UNSURE（证据不足）不在此列
    /// 恒转人工（返工修不了「无法判定」）。轮数按「返工：」前缀子单累计数计
    /// （含 cancelled——按尝试计不按存活计），超限保持转人工。
    pub parent_remediation_max: u32,
    /// 交付证据契约总闸（默认 false = 语义不变）。
    /// 开启后**所有**单子验收 PASS 前置校验最新派发任务的交付记录在场
    /// （delivered/confirmed），缺失 = 降级 FAIL「缺交付证据」（不凭自然
    /// 语言进 done）。issue 级 `requires_evidence`（AC 含 file: 锚点自动
    /// 置位）不受本闸控制、恒生效——本闸是全局加码，不是全局豁免。
    pub require_delivery_evidence: bool,
    /// 交付证据 typed 块强制闸（②，；默认 **false**
    /// 烧入期）。true 后交付证据闸要求 `$.delivery.files` typed 块**在场**
    /// （git tree diff 确定性派生，任何 git 交付都应可派生——缺席 = 证据面
    /// 残缺，FAIL「交付缺类型化证据块」）。`checks` 块**恒可选**（没跑检查
    /// = 诚实的事实陈述，不是证据缺失；强制会诱发造值）。typed 块**在场
    /// 时**无论本闸开关都验形态（path 形态闸 + sha256 hex 形态 + cmd 非空），
    /// 验不过 = FAIL——加法演进，旧 payload 缺席块行为不变。
    pub evidence_typed_required: bool,
    /// 必需检查契约（项 B「必需的必须跑」，；键 = 匹配键，
    /// 值 = 必需锚点 id 列表）。匹配键与 issue `required_tags` 做**大小写
    /// 不敏感交集**（与派发 matcher 标签硬条件同规则同词表）；命中任一
    /// 标签键 → 该键的检查列表生效（多键命中取并集）；无标签命中 → 落
    /// **空串键**的全局兜底规则（无空串键 = 无要求，行为不变）。锚点 id
    /// 匹配 `[CHECK]` 锚点的 target（file 路径 / exec 命令）或 pattern
    ///（re:/contains: 文本）。命中单子验收时，任一必需锚点「AC 未声明 /
    /// 被跳过（exec 关或环境缺）/ 未产出核验结果」→ 强制停车转人工：
    /// 不进语义评审、不允许 auto_accept，系统评论列缺口明细（哪条规则/
    /// 哪个锚点/为何未满足）。已运行（过或不过）= 覆盖满足，裁决归锚点
    /// 双检既有短路。空 map = 契约关闭（默认，零回归）。
    #[serde(default)]
    pub required_checks: std::collections::HashMap<String, Vec<String>>,
    /// 必检计划闸（外循环，；默认 **false** 烧入期）。开启后
    /// 验收 PASS 前对账必检计划：单据正文反引号包裹的多词命令（如
    /// `` `cargo test` ``）= 本单必检项，worker 交付证据 `checks[]` 里没有
    /// 对应执行记录 = 交付不完整（PASS 降级 FAIL「计划内检查未跑」），
    /// 对账表同时注入评审 prompt。`[CHECK]` 锚点行不重复计（归锚点双检
    /// 与 `required_checks` 契约管辖）。`[serde(default)]` 容器缺键兼容，
    /// 旧 config 零迁移。
    #[serde(default)]
    pub require_check_plan: bool,
}

impl BoardReviewConfig {
    /// 职能评审参数解析（单一入口）：全局字段打底 → 内置默认预设 →
    /// config 职能预设逐维度覆盖。`profession` 空/None = 无职能，返回全局值。
    pub fn resolve_for_profession(&self, profession: Option<&str>) -> BoardReviewParams {
        let mut out = BoardReviewParams {
            max_turns: self.max_turns,
            checkers: self.checkers,
            selfcheck: self.selfcheck,
        };
        let Some(slug) = profession.map(str::trim).filter(|s| !s.is_empty()) else {
            return out;
        };
        let lower = slug.to_ascii_lowercase();
        let preset = profession_preset_lookup(&self.professions, &lower).or_else(|| {
            // 裸 family 回落：`dev:cpp` 无精确键 → `dev` 键（无冒号时精确
            // 查找已覆盖，不重复取）。
            let family = lower.split(':').next().unwrap_or(&lower);
            (family != lower)
                .then_some(family)
                .and_then(|f| profession_preset_lookup(&self.professions, f))
        });
        let builtin = builtin_profession_presets()
            .into_iter()
            .find(|(k, _)| *k == lower)
            .or_else(|| {
                let family = lower.split(':').next().unwrap_or(&lower);
                builtin_profession_presets()
                    .into_iter()
                    .find(|(k, _)| *k == family)
            })
            .map(|(_, p)| p);
        // config 预设逐维度覆盖内置预设（None 维度沿用内置）。
        let merged = match (preset.cloned(), builtin) {
            (Some(cfg_p), Some(b)) => BoardProfessionReviewPreset {
                max_turns: cfg_p.max_turns.or(b.max_turns),
                checkers: cfg_p.checkers.or(b.checkers),
                selfcheck: cfg_p.selfcheck.or(b.selfcheck),
            },
            (Some(p), None) | (None, Some(p)) => p,
            (None, None) => return out,
        };
        if let Some(v) = merged.max_turns {
            out.max_turns = v;
        }
        if let Some(v) = merged.checkers {
            out.checkers = v;
        }
        if let Some(v) = merged.selfcheck {
            out.selfcheck = v;
        }
        out
    }
}

/// config 职能预设键查找（大小写不敏感——文档口径「键=职能 slug，大小写
/// 不敏感」，与派发 matcher / `required_checks` 标签交集 /
/// `dispatch_fallback_target` 钉住匹配同款纪律）。键是用户手写 JSON 键，
/// 不在写入点归一（board.config.set 白名单不覆盖本嵌套 map），匹配侧统一
/// 宽和：trim + ASCII 大小写不敏感。线性扫描（职能目录 ≤17 项，开销可忽略）。
fn profession_preset_lookup<'a>(
    map: &'a std::collections::HashMap<String, BoardProfessionReviewPreset>,
    key: &str,
) -> Option<&'a BoardProfessionReviewPreset> {
    map.iter()
        .find(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
}

/// 代码内置职能预设（config `board.review.professions` 同名键逐维度
/// 覆盖）。`dev:*` 无内置预设（实现单评审与通用评审无差异诉求，维持全局
/// 默认）。
fn builtin_profession_presets() -> [(&'static str, BoardProfessionReviewPreset); 2] {
    [
        (
            "product",
            BoardProfessionReviewPreset {
                max_turns: Some(0),
                checkers: Some(2),
                selfcheck: Some(true),
            },
        ),
        (
            "test-blackbox",
            BoardProfessionReviewPreset {
                max_turns: None,
                checkers: Some(2),
                selfcheck: None,
            },
        ),
    ]
}

/// 职能评审预设（`board.review.professions.<slug>`）。三个键全 Option：
/// 只覆盖显式设置的维度，其余沿用内置预设/全局 `board.review.*`——局部
/// 预设不必照抄全局值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BoardProfessionReviewPreset {
    /// 覆盖 `board.review.max_turns`（0 = 不限取证轮数）。
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// 覆盖 `board.review.checkers`（多检查员路数）。
    #[serde(default)]
    pub checkers: Option<u32>,
    /// 覆盖 `board.review.selfcheck`（评审取证挂起开关）。
    #[serde(default)]
    pub selfcheck: Option<bool>,
}

/// 解析后的评审参数三元组（[`BoardReviewConfig::resolve_for_profession`]
/// 返回值；消费点直接替换对 `board.review.*` 的直读）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoardReviewParams {
    pub max_turns: u32,
    pub checkers: u32,
    pub selfcheck: bool,
}

impl Default for BoardReviewConfig {
    fn default() -> Self {
        Self {
            // 默认 1 → 0（0 = 不限，取证到能下结论为
            // 止）。max_turns=1 时评审 agent 单轮取证经常不够下结论 → UNSURE
            // 转人工或假 FAIL；doom-loop / estop 守卫仍在，0 语义安全（
            // 已对齐 UI min=0）。
            max_turns: 0,
            selfcheck: false,
            auto_close_project: false,
            checkers: 1,
            tier: "thorough".to_string(),
            exec_anchors_enabled: false,
            exec_anchor_timeout_secs: 120,
            failure_retries: 1,
            max_unsure_streak: 2,
            professions: std::collections::HashMap::new(),
            parent_remediation_max: 2,
            require_delivery_evidence: false,
            evidence_typed_required: false,
            required_checks: std::collections::HashMap::new(),
            require_check_plan: false,
        }
    }
}

/// 自动流转预算保险丝（`board.budget` 段；全自动流转 /；0 = 关闭）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardBudgetConfig {
    /// 单个父单最大子单数（planner 拆解超出即告警；硬性派发侧预算）。
    pub max_subissues_per_parent: u32,
    /// 单个父单全链累计重派次数上限（含全部子单的 dispatch 记录；0 = 关闭）。
    pub max_total_redispatch: u32,
    /// 单个父单从创建起的墙钟预算秒数（超时不再自动重派转人工；0 = 关闭）。
    pub wall_clock_budget_secs: u64,
    /// 单个父单全链 token 预算（二期：worker 经回调回传的用量累计，
    /// input+output 口径；0 = 关闭。依赖集群回传，单机路径无 token 记账）。
    pub max_tokens_per_parent: u64,
    /// 单个子单送达失败自动重试次数上限（重试池；**按本单 ⛔ 标记评论
    /// 计数**，与 max_total_redispatch 的父单全链派发行口径互不掺和）。
    /// 超限 → 🛑 系统评论转人工、出池。默认 5（退避阶梯 60+120+240+480+960
    /// = 1860s ≈ 31 分钟内重试 5 次，此后第 6 次失败熔断）；0 = 关闭保险丝
    ///（无限重试）。`unlimited_mode` = true 时视为 0（操作者显式放弃护栏）。
    pub max_send_retries: u32,
}

impl Default for BoardBudgetConfig {
    fn default() -> Self {
        Self {
            max_subissues_per_parent: 20,
            max_total_redispatch: 0,
            wall_clock_budget_secs: 0,
            max_tokens_per_parent: 0,
            max_send_retries: 5,
        }
    }
}

/// 项目档案传输护栏（`board.archive` 段；看板项目档案 goal）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardArchiveConfig {
    /// 单次档案传输最大字节数（worker 与 master 双侧同源护栏）。超限诚实
    /// 失败：worker 侧标记 over_limit 停推 + 通知 master 出决策流/收件箱卡，
    /// 评审转人工；**绝不截断、绝不只回索引**。0 = 不限（不建议）。
    /// 默认 2 GiB。
    pub max_transfer_bytes: u64,
}

impl Default for BoardArchiveConfig {
    fn default() -> Self {
        Self {
            max_transfer_bytes: 2 * 1024 * 1024 * 1024, // 2 GiB
        }
    }
}

/// 任务档案导出配置（`board.dossier` 段）。导出本体见
/// `nemesis_board::dossier`（单任务树/整项目两入口；WSAPI `dossier.export`
/// 与 CLI `nemesisbot dossier export`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardDossierConfig {
    /// 项目收口（completed，自动 PASS / 人工 project.update 两条路径）
    /// 后自动生成项目任务档案到 `<workspace>/logs/dossiers/`（默认 true；
    /// 失败只 WARN + 父单评论留痕，不阻塞收口）。
    pub auto_on_complete: bool,
}

impl Default for BoardDossierConfig {
    fn default() -> Self {
        Self {
            auto_on_complete: true,
        }
    }
}

/// 任务拆解配置（`board.plan` 段；Swarm；超时重拆四键 无人值守
/// 硬化 #1——凡可致流程停滞处必有开关）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardPlanConfig {
    /// planner 拆解用模型别名（None = 主模型）。
    pub model: Option<String>,
    /// 拆解后跳过人工确认闸直接建单派发（默认 false = 拆完等人确认）。
    /// 消费点：`issue_plan` 一段异步拆解产出 plan_ready 时现读本旗标，
    /// true 即自动 confirm 发车（全自动流转 接线；planner 失败仍转
    /// 人工通知，不自动重试）。
    pub auto_confirm: bool,
    /// 单次拆解链超时阈值秒数（默认 1200 = 20 分钟，用户裁定；0 = 不限）。
    /// 超时处置链见 `timeout_reattempt`。计时起点 = 单次链的 started_at
    ///（重拆后重置）——这是「单次拆解」阈值，不是累计。
    pub timeout_secs: u64,
    /// 拆解重试上限（默认 3；0 = 不限）。超时重拆达到上限后停车：父单评论
    ///「拆解连续超时」+ `board.plan_failed` 事件 + RUNNING_PLANS 摘除，
    /// 等人工重拆——停车而非无限烧 LLM。
    pub max_attempts: u32,
    /// 拆解超时是否自动重拆（默认 true）。false = 超时不重拆，直接转人工
    /// 评论（现行为补强前的语义，保留开关）。
    pub timeout_reattempt: bool,
    /// gateway 重启后自动恢复中断拆解（默认 true）。重启杀拆解任务且
    /// RUNNING_PLANS 是内存态——旧行为 = 父单停留「已开始拆解」评论态等
    /// 手动重拆（无人值守语义下 = 流程中断）。true 时启动扫描满足
    /// 「状态为 backlog/todo 且带拆解中评论形态且不在 RUNNING_PLANS」的
    /// 单子，自动重新发起拆解（走同一超时重拆链，attempt 重计）。
    /// false = 关回旧行为。手动 cancel 的单子状态非 backlog/todo 天然排除；
    /// 恢复拆解同受 `max_attempts` 闸。
    pub restart_recover: bool,
}

impl Default for BoardPlanConfig {
    fn default() -> Self {
        Self {
            model: None,
            auto_confirm: false,
            timeout_secs: 1200,
            max_attempts: 3,
            timeout_reattempt: true,
            restart_recover: true,
        }
    }
}

/// LLM 端点不可用期封存配置（`board.unavailable` 段；U-B 断网封存）。
/// 语义：master 侧 circuit breaker 只认**连接建立类**错误（connect refused /
/// DNS 失败；reqwest `is_connect`/`is_request`）——超时不计入（慢模型/长
/// 思考跑满 provider 600s 是常态，计入会误封存）。连续阈值次连接类失败 →
/// 端点标记 unavailable → 封存：只短路「下一步需要 LLM 的自动动作」（拆解
/// 发起 / 评审触发 / 自动重派），不需 LLM 的动作照常；手动操作放行 + WARN。
/// 恢复探针发最小真实 completion（TCP 通 ≠ LLM 活）。estop 优先级更高；
/// 封存状态独立于 estop（release 不解封，探针才解封）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardUnavailableConfig {
    /// 封存开关（默认 **true**——封存是无人值守语义的默认正确行为）。
    /// false = 关回各环节各自失败现状（连接失败各自 fail_class 处理）。
    pub freeze: bool,
    /// 连续连接类失败判定阈值（默认 3）。
    pub connect_fail_threshold: u32,
    /// 恢复探针间隔秒数（默认 300）。**0 = 只封存不自动解封**（人工恢复：
    /// 手动触发探针或重启）。
    pub probe_interval_secs: u64,
}

impl Default for BoardUnavailableConfig {
    fn default() -> Self {
        Self {
            freeze: true,
            connect_fail_threshold: 3,
            probe_interval_secs: 300,
        }
    }
}

/// 频道讨论区配置（`board.discussion` 段；Swarm）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardDiscussionConfig {
    /// 频道消息保留天数（默认 90；0 = 永久保留）。
    pub retention_days: i64,
    /// 每线程 agent 发言额度（master 记账随 wake.post 下发；默认 8，0=不限）。
    pub max_agent_turns_per_thread: u32,
    /// 每节点每小时发言上限（默认 20，0=不限）。
    pub hourly_budget_per_node: u32,
    /// 每节点每分钟 board.comment.post 上行限速（默认 12，0=不限）。
    pub rate_limit_per_min: u32,
    /// 定稿评审有界等待（秒；默认 1800，0=不等直接裁决）。定稿单
    /// worker 据此 sleep + 轮询讨论线程（双阈值的时间维；条数维在提示词
    /// 契约里按职能自选）。消费面 = 定稿单任务提示词渲染（master config
    /// 经 planner/派发描述传给 worker，worker 本地 config 不作数）。
    pub review_wait_secs: u32,
}

impl Default for BoardDiscussionConfig {
    fn default() -> Self {
        Self {
            retention_days: 90,
            max_agent_turns_per_thread: 8,
            hourly_budget_per_node: 20,
            rate_limit_per_min: 12,
            review_wait_secs: 1800,
        }
    }
}

/// 看板数据库每日备份配置（`board.backup` 段；Swarm）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardBackupConfig {
    /// 备份保留份数（默认 14 = 两周；0 = 关闭每日备份）。
    pub keep: u32,
}

impl Default for BoardBackupConfig {
    fn default() -> Self {
        Self { keep: 14 }
    }
}

// ============================================================================
// Executor Separation Config (执行体剥离 + 沙盒开关)
// ============================================================================
