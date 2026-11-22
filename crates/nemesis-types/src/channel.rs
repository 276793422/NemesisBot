//! Channel message types.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Inbound message from a channel to the agent engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboundMessage {
    pub channel: String,
    pub sender_id: String,
    pub chat_id: String,
    pub content: String,
    pub media: Vec<MediaAttachment>,
    pub session_key: String,
    pub correlation_id: String,
    /// Optional metadata for routing (peer_kind, peer_id, account_id, guild_id, team_id, etc.)
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    /// Whether to inject voice playback prompt in AgentLoop.
    #[serde(default)]
    pub voice_playback: Option<bool>,
    /// 消息语义类（2026-11-21 msg_class 架构）：闸门按 class 查政策表，
    /// 形状推断退役。User 是唯一 serde default——缺字段（旧 wire / 旧
    /// sidecar 条目）= User；持有区（pending/队列/stash）反序列化点用
    /// [`InboundMessage::legacy_class_coerce`] 按章/标记一次性改判。
    /// 内部生产者**禁止**手写字面量造 User 语义的内部消息——必须走
    /// 构造器家族（`notice`/`instruction`/`continuation`，class 烤死）；
    /// 通道 ingress 走 [`InboundMessage::user_ingress`]。
    #[serde(default)]
    pub msg_class: MsgClass,
}

// ---------------------------------------------------------------------------
// 消息语义类（msg_class，2026-11-21 架构计划；真相源 docs/REPORT 归档后
// 以交付报告为准）
// ---------------------------------------------------------------------------

/// 入站消息语义类。
///
/// 纪律：[`MsgClass::User`] 是唯一默认（serde default）——只有通道 ingress
/// 白名单区（[`InboundMessage::user_ingress`] 及其调用点）允许产生 class
/// 缺省的构造；内部生产者必须经构造器家族（`notice`/`instruction`/
/// `continuation`），class 在构造器内烤死，生产者碰不到字段。
///
/// 扩展纪律：新变体 = 一枚举臂 + 一政策行，闸门政策表 match 不许 `=> {}`
/// 通配吞新变体（编译器强制补臂）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MsgClass {
    /// 真实用户消息（唯一默认；完整用户语义：回执/排队/弹回留痕/物化）。
    #[default]
    User,
    /// 信息通告：唤醒语义（A1 拍板维持空闲开轮=唤醒机制本体；三路唤醒各有
    /// 拍板记录：事件唤醒 events / 循环推进 loop / 审批续行
    /// approval_continuation；原 background 路已随 2026-11-22 完成唤醒
    /// 摘除批退场）。
    /// busy→stash 轮边界合并；snooze→静默 pending；空闲→通知条真轮。
    /// 零回执零弹回。落盘两态：空闲准入=带章 user 行；busy 合并注入=system 注记行。
    Notice(NoticeKind),
    /// 指令注入：开真轮。busy→静默排队（零回执，Reject 也不弹回——
    /// Reject 是用户对自己并发消息的态度，不作用内部指令）；
    /// snooze→静默 pending。落盘=user 行带 kind 章。
    Instruction(InstructionKind),
    /// 子代理回灌 / 集群续行：闸门直通消费臂（原 channel=="system" +
    /// 内容前缀推断的 class 化）；下游路由继续用内容前缀（不改）。
    Continuation,
}

/// 通告子类（[`MsgClass::Notice`] 携带）。取值域与旧 `system_notice`
/// 章值域（除已摘除的 background）一一对应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    /// 事件唤醒（events injector）。
    Events,
    /// durable loop 循环推进。
    Loop,
    /// 审批续行回灌（批准/拒绝都回灌，2026-10-22 拍板）。
    ApprovalContinuation,
    /// turn 收口合成通告轮（孤儿 kick 等合并注入）。
    Merged,
}

impl NoticeKind {
    /// 章/slug 形态（旧 `system_notice` 章值域；显示面投影与检测网共用）。
    pub fn slug(&self) -> &'static str {
        match self {
            NoticeKind::Events => "events",
            NoticeKind::Loop => "loop",
            NoticeKind::ApprovalContinuation => "approval_continuation",
            NoticeKind::Merged => "merged",
        }
    }

    /// 从旧章值反解（`legacy_class_coerce` 与显示面兜底读共用）。
    /// 已摘除的 background 生产者不在值域（返回 None，持有区条目按
    /// 退役生产者诚实丢弃）。`durable_loop` 是 loop 生产者的旧章值
    /// （构造器 slug 统一为 `loop` 后保留为反解别名）。
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "events" => Some(NoticeKind::Events),
            "loop" | "durable_loop" => Some(NoticeKind::Loop),
            "approval_continuation" => Some(NoticeKind::ApprovalContinuation),
            "merged" => Some(NoticeKind::Merged),
            _ => None,
        }
    }
}

/// 指令子类（[`MsgClass::Instruction`] 携带）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionKind {
    /// goals 评估闸 CONTINUE 缺口注入（下一轮开场指令）。
    GoalRound,
    /// cron job tick 调度轮。
    CronTick,
    /// turn WAL 崩溃续轮。
    WalResume,
    /// 看板本地派发任务轮。
    BoardDispatch,
}

impl InstructionKind {
    /// `instruction_kind` metadata 章 slug（构造器写入 + 显示面/检测网共用）。
    pub fn slug(&self) -> &'static str {
        match self {
            InstructionKind::GoalRound => "goal_round",
            InstructionKind::CronTick => "cron_tick",
            InstructionKind::WalResume => "wal_resume",
            InstructionKind::BoardDispatch => "board_dispatch",
        }
    }

    /// 从章 slug 反解（`legacy_class_coerce` 与显示面兜底读共用）。
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "goal_round" => Some(InstructionKind::GoalRound),
            "cron_tick" => Some(InstructionKind::CronTick),
            "wal_resume" => Some(InstructionKind::WalResume),
            "board_dispatch" => Some(InstructionKind::BoardDispatch),
            _ => None,
        }
    }
}

/// 旧格式持有区条目的改判结论（[`InboundMessage::legacy_class_coerce`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyVerdict {
    /// 已带 class（或确无内部标记）——原样保留。
    Keep,
    /// 按章/标记改判了 class（Remapped 后全系统单轨）。
    Remapped,
    /// 退役生产者的残留条目（如已摘除的 background 通告）——诚实丢弃。
    Drop,
}

/// `instruction_kind` metadata 键（构造器写入；显示/检测网/改判共用）。
pub const INSTRUCTION_KIND_STAMP_KEY: &str = "instruction_kind";

impl InboundMessage {
    /// 信息通告（Notice class 烤死，sender="system"）。不再写
    /// `system_notice` 章——class 是唯一语义真相源；显示面对旧 jsonl 行
    /// 读章兜底（永久兼容读）。
    pub fn notice(
        kind: NoticeKind,
        channel: impl Into<String>,
        chat_id: impl Into<String>,
        session_key: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            channel: channel.into(),
            sender_id: "system".to_string(),
            chat_id: chat_id.into(),
            content: content.into(),
            media: Vec::new(),
            session_key: session_key.into(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
            voice_playback: None,
            msg_class: MsgClass::Notice(kind),
        }
    }

    /// 指令注入（Instruction class 烤死，sender="system"，metadata 带
    /// `instruction_kind` 章——持久化行与旧章消费方读的投影面）。
    pub fn instruction(
        kind: InstructionKind,
        channel: impl Into<String>,
        chat_id: impl Into<String>,
        session_key: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            INSTRUCTION_KIND_STAMP_KEY.to_string(),
            kind.slug().to_string(),
        );
        Self {
            channel: channel.into(),
            sender_id: "system".to_string(),
            chat_id: chat_id.into(),
            content: content.into(),
            media: Vec::new(),
            session_key: session_key.into(),
            correlation_id: String::new(),
            metadata,
            voice_playback: None,
            msg_class: MsgClass::Instruction(kind),
        }
    }

    /// 续行回灌（Continuation class + channel="system" 烤死）。
    ///
    /// `sender_id` 由调用方提供且**必须保持既有前缀形态**（下游
    /// admission 续行臂从 sender 前缀/后缀提取 task_id 路由——
    /// `subagent_continuation:{task_id}` / `cluster_continuation:{task_id}`，
    /// 前缀常量在 [`crate::constants`]）。
    pub fn continuation(
        sender_id: impl Into<String>,
        session_key: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            channel: "system".to_string(),
            sender_id: sender_id.into(),
            chat_id: String::new(),
            content: content.into(),
            media: Vec::new(),
            session_key: session_key.into(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
            voice_playback: None,
            msg_class: MsgClass::Continuation,
        }
    }

    /// 通道 ingress 的 User 消息构造（**唯一合法的 User 字面量入口**）。
    ///
    /// 通道回调与 web server ingress 用此 helper 起底（media/metadata/
    /// correlation_id/session_key 缺省空），再按需以 `with_*` builder 补
    /// 字段——grep 门禁据此把 User 字面量白名单收敛到本文件。
    pub fn user_ingress(
        channel: impl Into<String>,
        sender_id: impl Into<String>,
        chat_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            channel: channel.into(),
            sender_id: sender_id.into(),
            chat_id: chat_id.into(),
            content: content.into(),
            media: Vec::new(),
            session_key: String::new(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
            voice_playback: None,
            msg_class: MsgClass::User,
        }
    }

    /// Builder：附加媒体列表。
    pub fn with_media(mut self, media: Vec<MediaAttachment>) -> Self {
        self.media = media;
        self
    }

    /// Builder：设置会话键。
    pub fn with_session_key(mut self, session_key: impl Into<String>) -> Self {
        self.session_key = session_key.into();
        self
    }

    /// Builder：设置关联 ID（RPC 请求-响应匹配）。
    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = correlation_id.into();
        self
    }

    /// Builder：合并 metadata 键值。
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Builder：可选 metadata 章（None = 不写键）——条件章的生产者形态
    /// （如 cron tick 的 per-fire `cron_max_rounds` 预算章）。
    pub fn with_metadata_opt(
        mut self,
        key: impl Into<String>,
        value: Option<impl Into<String>>,
    ) -> Self {
        if let Some(v) = value {
            self.metadata.insert(key.into(), v.into());
        }
        self
    }

    /// Builder：整体替换 metadata 表（通道 ingress 迁移用——平台回调
    /// 常持有整个 map 表达式）。
    pub fn with_metadata_map(
        mut self,
        metadata: std::collections::HashMap<String, String>,
    ) -> Self {
        self.metadata = metadata;
        self
    }

    /// Builder：设置语音回放旗标。
    pub fn with_voice_playback(mut self, voice_playback: Option<bool>) -> Self {
        self.voice_playback = voice_playback;
        self
    }

    /// 旧格式持有区条目的一次性改判（pending sidecar / busy 指令队列 /
    /// 通告 stash 的**反序列化点**调用）：已带 class 原样保留；无 class
    /// 的旧条目按章/标记改判（`system_notice`→Notice /
    /// `cron_job_id`→Instruction(CronTick) / `turn_wal_resume`→WalResume /
    /// `instruction_kind`→Instruction）；退役生产者残留（background 章）
    /// 判 Drop 由调用方丢弃。改判后进闸即单轨，不做迁移脚本。
    pub fn legacy_class_coerce(&mut self) -> LegacyVerdict {
        if !matches!(self.msg_class, MsgClass::User) {
            return LegacyVerdict::Keep;
        }
        // instruction_kind 章（新构造器写的投影面，旧 wire 无 class 但可能有章）。
        if let Some(kind) = self
            .metadata
            .get(INSTRUCTION_KIND_STAMP_KEY)
            .and_then(|slug| InstructionKind::from_slug(slug))
        {
            self.msg_class = MsgClass::Instruction(kind);
            return LegacyVerdict::Remapped;
        }
        if self.metadata.contains_key("cron_job_id") {
            self.msg_class = MsgClass::Instruction(InstructionKind::CronTick);
            return LegacyVerdict::Remapped;
        }
        if self.metadata.contains_key("turn_wal_resume") {
            self.msg_class = MsgClass::Instruction(InstructionKind::WalResume);
            return LegacyVerdict::Remapped;
        }
        if let Some(slug) = self.metadata.get("system_notice") {
            match NoticeKind::from_slug(slug) {
                Some(kind) => {
                    self.msg_class = MsgClass::Notice(kind);
                    return LegacyVerdict::Remapped;
                }
                // 已摘除生产者（background 等）的残留条目——诚实丢弃。
                None => return LegacyVerdict::Drop,
            }
        }
        LegacyVerdict::Keep
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// （msg_class 单轨 2026-11-21 §10-6）：web ingress 语义伪造键剥离
// ---------------------------------------------------------------------------

/// 语义伪造键清单（web ingress 剥离面，单一真相源）：这些键是**服务端
/// 内部语义章**——闸门归类（system_notice/cron_job_id/turn_wal_resume）、
/// 直塞门识别（hook_prompt）、构造器投影（instruction_kind）。客户端
/// 传入即伪造闸门/直塞语义（如伪造 system_notice 让用户消息在 busy 时
/// 被 stash 进通告面 = 幻影通告路径复活）。ingress 剥离点统一调
/// [`strip_forge_keys`]。
///
/// 三分类纪律（计划 §10-6）：显示类键（session_id/open_files/priority/
/// client_msg_id/workflow_edit…）放行；本清单键服务端剥离；服务端控制
/// 面键（request_type=history 等 websocket_handler 自写键）不在剥离
/// 逻辑射程——它们不经本函数的威胁模型（客户端无入口），若误入清单
/// 反而破坏服务端自写链路。
pub const INGRESS_FORGE_KEYS: [&str; 5] = [
    "system_notice",
    "cron_job_id",
    "turn_wal_resume",
    "hook_prompt",
    INSTRUCTION_KIND_STAMP_KEY,
];

/// 剥离语义伪造键（保留其余键原样返回）。web ingress 装配点
/// （nemesis-web server.rs 入站咽喉）统一调用——当前 chat.send 的
/// metadata 是构造式白名单（客户端无透传字段），剥离层是纵深防御：
/// 未来有人加透传字段时本闸兜底。
pub fn strip_forge_keys(
    metadata: std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, String> {
    if INGRESS_FORGE_KEYS.iter().any(|k| metadata.contains_key(*k)) {
        let mut m = metadata;
        for k in INGRESS_FORGE_KEYS {
            if m.remove(k).is_some() {
                tracing::warn!(
                    key = k,
                    "[ingress] 语义伪造键已剥离（客户端不得携带服务端内部章）"
                );
            }
        }
        return m;
    }
    metadata
}

// （devtool-upgrade）：客户端上报的「当前打开文件」
// ---------------------------------------------------------------------------

/// 打开文件列表条数上限。超出保留前 N 条（协议约定顺序 = 上报顺序，
/// 首条最近活跃），其余诚实丢弃——上报是尽力而为的上下文信号，不是请求
/// 参数，超限不报错。
pub const MAX_OPEN_FILES: usize = 20;

/// 单条路径长度上限（UTF-8 字符数）。超长条目整条丢弃而非截断——截断后的
/// 路径指向不存在的文件，对模型是误导。
pub const MAX_OPEN_FILE_PATH_LEN: usize = 1024;

/// 清洗客户端上报的打开文件路径列表：trim / 去空 / 去重保序 /
/// 条数与单条长度封顶。web 通道写入端与 AgentLoop 渲染端共用此单点，
/// 两端看到的列表永远一致。
pub fn sanitize_open_files(paths: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut cleaned = Vec::new();
    for p in paths {
        let p = p.trim();
        if p.is_empty() || p.chars().count() > MAX_OPEN_FILE_PATH_LEN {
            continue;
        }
        if seen.insert(p.to_string()) {
            cleaned.push(p.to_string());
        }
        if cleaned.len() >= MAX_OPEN_FILES {
            break;
        }
    }
    cleaned
}

/// 从 [`InboundMessage::metadata`] 解析 `open_files` 键（web 通道写入的
/// JSON 字符串数组）。缺失 / 空 / 非法 JSON / 元素非字符串 → 空表（诚实
/// 丢弃，不炸消息）；清洗走 [`sanitize_open_files`] 单点。
pub fn open_files_from_metadata(
    metadata: &std::collections::HashMap<String, String>,
) -> Vec<String> {
    metadata
        .get("open_files")
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .map(sanitize_open_files)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 对话生成：workflow_edit 注入目标
// ---------------------------------------------------------------------------

/// `chat.send` 附带的 workflow_edit 注入目标（照抄 open_files/ 的
/// metadata 通道模式：前端序列化为 JSON 字符串塞 metadata，AgentLoop 读取）。
///
/// `workflow_name = None` 表示 `_new` 共享引导会话（所有「新建工作流」对话
/// 共用一条会话；引导块由 loop.rs 渲染，内容含能力表）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowEditTarget {
    /// `Some(name)` = 针对已注册工作流的编辑会话；`None` = 新建会话。
    #[serde(default)]
    pub workflow_name: Option<String>,
}

/// `workflow_edit` metadata 键写入侧唯一合法封装（web 通道构造用）。
pub fn workflow_edit_to_metadata(target: &WorkflowEditTarget) -> String {
    serde_json::to_string(target).unwrap_or_default()
}

/// 从 [`InboundMessage::metadata`] 解析 `workflow_edit` 键。缺失 / 空 /
/// 非法 JSON → `None`（诚实丢弃，消息照常按普通对话处理）。
pub fn workflow_edit_from_metadata(
    metadata: &std::collections::HashMap<String, String>,
) -> Option<WorkflowEditTarget> {
    metadata
        .get("workflow_edit")
        .and_then(|raw| serde_json::from_str::<WorkflowEditTarget>(raw).ok())
}

// ---------------------------------------------------------------------------
// principal 映射（预留）：通道会话 → 主体标识
// ---------------------------------------------------------------------------

/// 通道会话 → principal 映射（**单一真相源**；形态 `{channel}/{sender_id}`，
/// 与集群跨节点复合键同形）。 预留：只立映射与绑定，租户闸不做——
/// 消费方（未来多租户授权）出现前任何代码不得读取本值做行为分叉。
///
/// `sender_id` 为空（cron/heartbeat/system 等无主体来源）→ 空串 = 不绑定。
pub fn principal_for_channel(channel: &str, sender_id: &str) -> String {
    if sender_id.trim().is_empty() {
        return String::new();
    }
    format!("{channel}/{sender_id}")
}

/// Extensible per-delivery metadata attached to an `OutboundMessage`.
///
/// Holds attributes that are optional/channel-specific (not every channel
/// cares about every field). Adding a future field here does **not** require
/// touching the ~161 `OutboundMessage` construction sites — they write
/// `meta: Default::default()` and absorb new fields automatically. Channels
/// read what they need and ignore the rest (`#[serde(default)]`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutboundMeta {
    /// Model that produced this assistant response, in `provider/name` form
    /// (e.g. `deepseek/deepseek-v4-flash`). The web channel surfaces it to the
    /// Dashboard as a per-message "供应商·模型名" badge so users can see which
    /// model generated each reply. Other channels ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// （devtool-upgrade）：产生本回复的 agent 会话键
    /// （`agent:main:session:{sid}`）。web 通道用它做 chat_event_log 断线
    /// 补拉的环形缓冲键——chat_id 在 web 通道是**连接级** id（重连即变），
    /// 补拉必须按**会话**跨连接寻址。其他通道忽略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    /// 集群续行归属：实际执行任务的 worker 节点名。集群
    /// `cluster_rpc` 派发的续行回复由主 LLM 转述，但**干活的是远端节点**
    /// ——web 通道据此渲染「节点 X」徽章，与模型徽章并列（各说各的真话）。
    /// 缺席 = 非集群回复 / 旧快照，出站帧不带该键，与历史行为逐字节一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_node: Option<String>,
    /// 通知送达关联（项 C）：本出站消息的投递结果要记账到
    /// 哪笔交付（deliveries.idempotency_key = 任务 id）。只有集群续行的
    /// **最终回复**携带（中途工具回执不挂——通知义务针对任务结果本身）。
    /// None = 无关联，出站帧不带该键，与历史行为逐字节一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// 通道 sync 镜像回环标记：本消息由某通道的
    /// `sync_to` 镜像产出，值 = 镜像发起通道名。接收通道的 `send()` 见到
    /// 该标记不再二次 sync 扩散——互配 `sync_to` 的两通道（send 均镜像）
    /// 会无限乒乓，每轮都是一次真实外部投递的消息风暴。None = 非镜像
    /// 消息，出站帧不带该键，与历史行为逐字节一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_via: Option<String>,
    /// 非回合产物标记：本出站是系统回执（busy 排队/
    /// 拒绝/drain/estop 冻结/slash 静态确认/压缩与限流进度/续行中途回执/
    /// connector 凭条卡），不是 agent 对任务文本的回合回复。按
    /// `meta.session_key` 匹配出站等终局回复的消费方（看板本地派发回复
    /// 捕获）必须跳过 receipt=true——queue-ack 与终局回复在内容上都是
    /// 文本，生产者打标是唯一依据（不做文本匹配）。false = 正常回合
    /// 回复（含错误终局），缺省 false，出站帧不带该键，与历史行为逐字节
    /// 一致。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub receipt: bool,
    /// 压缩过程徽标（2026-11-09 压缩提示徽标化）：本出站是压缩过程的
    /// 系统注记帧（开始/完成/暂停/预警），web 通道据此以 role="system" +
    /// `note_kind` 帧键下发，前端渲染成与工具调用同族的折叠小徽标（实时
    /// 与历史重载同形态）。None = 非注记帧，出站帧不带该键，role 维持
    /// "assistant"（receipt 旧行为逐字节不变）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_kind: Option<String>,
    /// 帧占行真相（2026-11-11 行号锚漂移根修）：本出站的正文是否对应
    /// chat_log jsonl 落盘行。`Some(true)` = 落盘（前端行号锚 +1——含
    /// role="system" 的压缩完成注记、💤 暂停排队回执这类落盘 system 行）；
    /// `Some(false)` = 瞬态回执（⏳ busy 排队 / ⚡ 插话确认 / slash 静态
    /// 确认 / 维护 ⏳✓ / ⏳ 直派收据——不落盘，前端**不占行号锚**）；
    /// `None` = 不带帧键（旧行为：前端按 user/assistant 角色占行）。
    /// 此前瞬态回执以 role="assistant" 下发被误计入锚、落盘 system 注记
    /// 以 role="system" 下发被漏计——两个方向的漂移都会让 rewind/regenerate
    /// 的 message_index 错行（前缀截断删错行）。生产者打标是唯一依据
    /// （不做内容前缀匹配——note_kind 同款纪律）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupies: Option<bool>,
}

/// Outbound message from the agent engine to a channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub channel: String,
    pub chat_id: String,
    pub content: String,
    /// Message type: "" = normal, "history" = history response.
    /// Mirrors Go's OutboundMessage.Type field.
    #[serde(default, rename = "type")]
    pub message_type: String,
    /// Extensible delivery metadata (model, future fields). Default = empty.
    #[serde(default)]
    pub meta: OutboundMeta,
}

impl OutboundMessage {
    /// Create a new outbound message with default type.
    pub fn new(channel: &str, chat_id: &str, content: &str) -> Self {
        Self {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            message_type: String::new(),
            meta: OutboundMeta::default(),
        }
    }

    /// Create with a specific message type.
    pub fn with_type(channel: &str, chat_id: &str, content: &str, message_type: &str) -> Self {
        Self {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            content: content.to_string(),
            message_type: message_type.to_string(),
            meta: OutboundMeta::default(),
        }
    }
}

/// Media attachment in a message.
///
/// Supports dual serialization for compatibility with Go's `[]string` format:
/// - If only `url` is set (media_type empty, data None), serializes as a plain string.
/// - Otherwise serializes as a full object.
///
/// Deserialization accepts both a plain string (treated as URL) and a full object.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaAttachment {
    pub media_type: String,
    pub url: String,
    pub data: Option<String>,
}

impl Serialize for MediaAttachment {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // If media_type is empty and data is None, serialize as a plain URL string.
        if self.media_type.is_empty() && self.data.is_none() && !self.url.is_empty() {
            serializer.serialize_str(&self.url)
        } else {
            // Full object serialization.
            #[derive(Serialize)]
            struct MediaObj<'a> {
                #[serde(rename = "type", skip_serializing_if = "str::is_empty")]
                media_type: &'a str,
                url: &'a str,
                #[serde(skip_serializing_if = "Option::is_none")]
                data: &'a Option<String>,
            }
            MediaObj {
                media_type: &self.media_type,
                url: &self.url,
                data: &self.data,
            }
            .serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for MediaAttachment {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de;

        // Helper struct for object-form deserialization.
        #[derive(Deserialize)]
        struct MediaObj {
            #[serde(rename = "type", default)]
            media_type: String,
            url: String,
            #[serde(default)]
            data: Option<String>,
        }

        // Use a visitor that handles both string and object.
        struct MediaVisitor;

        impl<'de> de::Visitor<'de> for MediaVisitor {
            type Value = MediaAttachment;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "a string or a media object")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<MediaAttachment, E> {
                Ok(MediaAttachment {
                    media_type: String::new(),
                    url: v.to_string(),
                    data: None,
                })
            }

            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<MediaAttachment, A::Error> {
                let obj = MediaObj::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(MediaAttachment {
                    media_type: obj.media_type,
                    url: obj.url,
                    data: obj.data,
                })
            }
        }

        deserializer.deserialize_any(MediaVisitor)
    }
}

/// Channel content types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChannelContent {
    Text(String),
    Markdown(String),
    Html(String),
}

/// Channel user information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelUser {
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub is_admin: bool,
}

#[cfg(test)]
mod tests;
