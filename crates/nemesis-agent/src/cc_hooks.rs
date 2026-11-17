//! hooks.json 方言层（—）。
//!
//! 把 `hooks.json` 方言格式桥到 // 的钩子体系上：解析方言配置 →
//! 按事件生成子进程脚本调用（stdin JSON / env / 退出码拦放行）→ 把退出码
//! 翻译回 [`crate::hooks`] 的决策类型。
//!
//! # 事件映射（goal 表）
//!
//! | 事件 | 我们的钩点 | 方言语义 |
//! |---|---|---|
//! | `SessionStart` | [`LifecycleHook::on_user_prompt`]（桥内部：该 session 首条 prompt 时先跑） | 观察型（exit 2 不拦——SessionStart 无阻断语义） |
//! | `UserPromptSubmit` | [`LifecycleHook::on_user_prompt`]（在消息进 history 之前） | exit 2 → 拦下 prompt（模型永远看不到）；exit 0 → 观察（stdout 只记日志，**不**注入上下文——诚实边界，见下） |
//! | `PreToolUse` | [`ToolHook::pre_tool_use`]（security 固定闸之后） | exit 2 / JSON `{"decision":"block"}` → Block（stderr 作 reason 回灌模型） |
//! | `PostToolUse` | [`ToolHook::post_tool_use`]（Forge 之前） | exit 2 → 把 stderr 以 `[hook]` 注记**追加**到结果（反馈给模型；不撤销已执行的操作） |
//! | `Stop` | [`LifecycleHook::on_turn_end`]（最终答案被接受后、Done 前） | exit 2 → Block stopping：stderr 作 feedback 注入为 user 消息、再答一轮（`MAX_TURN_END_CONTINUES` 封顶 fail-open）；`stop_hook_active` 标志随第二次起置 true |
//! | `SubagentStart` | [`CcHookBridge::dispatch_subagent_start`]（spawn 闭包内、`run_detached` 前——**仅本地 spawn 的子代理**；内部 LLM 委托如评审/冲突解决不发） | exit 2 / JSON block → **拒绝 spawn**（SpawnFn 返 Err，子代理不启动） |
//! | `SubagentStop` | [`CcHookBridge::dispatch_subagent_stop`]（spawn 闭包内、detached 轮次返回/后台任务完成处） | 观察型（任务已跑完，exit 2 无阻断语义——响亮 warn 记录） |
//! | `Notification` | [`CcHookBridge::dispatch_notification`]（审批/提问边界 fire-and-forget）+ [`ObservingQuestionAsker`]（loop 提问端装饰）+ `ObservingApprovalManager`（security 审批 manager 装饰） | **纯观察型**（exit 2 无任何阻断语义——通知已发出，outcome 由真实装配端决定）；覆盖面诚实边界见两包装器文档 |
//!
//! LLM 调用级无对应方言事件——hooks.json 没有 per-LLM-call hook，不造。
//!
//! # 脚本执行协议
//!
//! - **stdin**：单行紧凑 JSON。公共字段 `session_id`（=我们的 session_key）、
//!   `cwd`、`hook_event_name`、`transcript_path`（**空串**——我们无转写
//!   文件可指，诚实标注；真脚本极少依赖）。事件字段见 [`build_event_payload`]。
//! - **env**：`CLAUDE_PROJECT_DIR`=<workspace 根>（生态脚本常读它）。
//! - **cwd**：workspace 根。
//! - **退出码**：`0` = 放行（stdout 记日志；PreToolUse/Stop 还会尝试解析
//!   stdout JSON `{"decision":"block","reason":...}`）；`2` = 拦停（stderr
//!   作 reason/feedback）；其他/超时/启动失败 = 非阻断错误（warn 日志、放
//!   行——fail-open）。
//! - **超时**：每脚本 `timeout` 秒（默认 60）；到点 kill + 放行。
//!
//! # 工具名方言（真脚本能触发的前提）
//!
//! 方言脚本的 matcher 与 payload 用的是方言工具名（`Bash`/`Edit`/`Write`/
//! `Read`/`Grep`）。我们一侧：matcher 对 **方言别名或原始名** 任一命中即触发；
//! stdin 的 `tool_name` 优先发方言别名；`tool_input` 在原 args 之上补
//! `file_path`/`content`/`command` 等方言字段名别名（真 lint-on-edit 脚本
//! 读 `jq.tool_input.file_path`，没这层别名永远拿不到值）。映射表见
//! [`cc_tool_alias`]。
//!
//! # 诚实边界（没做的）
//!
//! - UserPromptSubmit exit-0 stdout 的 additionalContext 注入（方言语义会把它加
//!   进上下文；我们只记日志——避免在 history 之外再开一条注入通道）。
//! - stdout JSON 的 `{"continue": false, "stopReason"}` / `permissionDecision`
//!   / `suppressOutput` 等扩展字段：解析到但不消费。
//! - `transcript_path` 空串。
//! - matcher 无效正则时退化为字面量子串匹配（warn 提示）。
//!
//! # 装配
//!
//! 配置落位（2026-08-29 收编）：`<workspace>/config/hooks.json`（经
//! `nemesis_path::resolve_hooks_config_path_in_workspace`）。原先游离在
//! `<home>/config/hooks.json`——08-28 路径大迁移的漏网项；读取方启动时经
//! `migrate_legacy_home_hooks_config` 一次性 copy-once 迁移（legacy 保留
//! 作备份）。网关主 agent 工厂（`agent_factory.rs`）启动时若存在则加载并注册
//! （[`CcHookBridge::load_from_dir`]）；**集群 agent 不挂**（远端节点跑的是
//! 本地用户的任务，hook 拦截语义不该跨节点复制——挂账决策，报告注明）。
//! 解析失败 = warn + 跳过（fail-open，绝不拖死 gateway）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::hooks::{
    HookDecision, HookPrompt, HookToolCall, LifecycleHook, PostHookAction, PromptDecision,
    ToolHook, TurnEndDecision,
};

/// hooks.json 文件名（相对 config 目录）。
pub const HOOKS_FILE: &str = "hooks.json";

/// 会话级钩子存储文件名（HOOK 会话维度批次三）。agent `hooks` 工具写入的
/// 落位，与用户 `hooks.json` 分文件互不踩写（Dashboard 编辑 hooks.json、
/// agent 写本文件，无竞写窗口）。形态：
/// `{"hooks":{"<sid>":{"PreToolUse":[{"id":"h1","matcher":"","command":"...","timeout":60}]}}}`。
/// 条目二型：command 型（shell 执行体，任意事件）/ prompt 型（LLM 执行体，
/// `prompt` 字段非空且 command 空——**仅 Stop 事件合法**，turn 收尾后注入
/// 提示词开新轮，永续循环语义，见 `hooks.rs::TurnEndDecision::Inject`）。
pub const HOOKS_SESSION_FILE: &str = "hooks.session.json";

/// 一次性迁移：旧落位 `<home>/config/hooks.json` → 新落位
/// `<workspace>/config/hooks.json`。copy-once（新位已存在则不动，legacy
/// 保留作备份）——与 `migrate_legacy_vector_store` 同款先例。
pub fn migrate_legacy_home_hooks_config(home_config_dir: &Path, workspace_config_dir: &Path) {
    let legacy = home_config_dir.join(HOOKS_FILE);
    let target = workspace_config_dir.join(HOOKS_FILE);
    if !legacy.is_file() || target.exists() {
        return;
    }
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::copy(&legacy, &target) {
        Ok(_) => tracing::info!(
            "[cc-hooks] migrated legacy hooks config {} -> {}",
            legacy.display(),
            target.display()
        ),
        Err(e) => tracing::warn!(
            "[cc-hooks] failed to migrate legacy hooks config {}: {}",
            legacy.display(),
            e
        ),
    }
}

/// 方言默认脚本超时（秒）。
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Web 会话键前缀（`nemesis-web` handlers/chat.rs 同源字面量；本 crate 内
/// 唯一定义点）。会话圈定的匹配与 agent 侧存储都按剥前缀的裸 sid 落地；
/// 非 web 通道 session_key 形状不同——剥不动就整串参与精确比较。
pub const WEB_SESSION_KEY_PREFIX: &str = "agent:main:session:";

// ---------------------------------------------------------------------------
// 配置解析（方言格式）
// ---------------------------------------------------------------------------

/// 一条 hook 命令。方言 schema：`{"type":"command","command":"...","timeout":60}`。
#[derive(Debug, Clone, Deserialize)]
pub struct CcCommand {
    #[serde(default)]
    r#type: String,
    command: String,
    /// 秒。缺省 [`DEFAULT_TIMEOUT_SECS`]。
    #[serde(default)]
    timeout: Option<u64>,
}

/// 会话圈定（HOOK 会话维度，2026-11-16）。匹配语义 = **精确相等**（不搞
/// 正则/通配——会话 id 是稳定身份）。`sessions` 条目按剥
/// [`WEB_SESSION_KEY_PREFIX`] 的裸 sid 落盘；解析时归一去空白、空条目
/// loud 拒绝（见 [`parse_cc_hooks`]）。空列表 = 全部会话（缺省兼容）。
#[derive(Debug, Clone, Deserialize)]
pub struct CcScope {
    #[serde(default)]
    sessions: Vec<String>,
}

/// hook 超时解析：`None` / `Some(0)` → 缺省 [`DEFAULT_TIMEOUT_SECS`]，
/// 显式正值直通。
///
/// BUG-20261117-061：0 视作缺省（同 oauth.rs 契约「不允许 0=不限」）——
/// `Some(0)` 原样透传会在 [`run_hook_script`] 里被 `max(1)` 钳成 1s 必超时
/// （hook 静默全废），且超时报错文案用原值 0s 与实际不符。
fn resolve_hook_timeout(timeout: Option<u64>) -> u64 {
    timeout.filter(|t| *t > 0).unwrap_or(DEFAULT_TIMEOUT_SECS)
}

/// 一个事件下的 matcher 分组：`{"matcher":"Edit|Write","hooks":[...]}`。
/// 可选 `scope`（会话圈定）：`{"scope":{"sessions":["<sid>"]}}`。
#[derive(Debug, Clone, Deserialize)]
pub struct CcHookGroup {
    #[serde(default)]
    matcher: Option<String>,
    /// 会话圈定。`None` / 空 `sessions` = 全部会话（缺省兼容，老配置行为
    /// 零变化）；非空 = 只对列出的会话生效（[`Self::matches_session`]）。
    #[serde(default)]
    scope: Option<CcScope>,
    #[serde(default)]
    hooks: Vec<CcCommand>,
}

impl CcHookGroup {
    /// 会话圈定是否命中：未圈定（`scope: None`）/ 圈了但列表为空 = 全命中；
    /// 圈定 = 精确相等，候选双串（剥前缀 sid + 完整 session_key）任一即中。
    /// 空 `session_key` 输入（无会话身份的事件）只被未圈定的组命中——空串
    /// 不会是合法条目（解析已拒绝），永不误命中。
    fn matches_session(&self, session_key: &str) -> bool {
        let Some(scope) = &self.scope else {
            return true; // 未圈定 = 全部会话（缺省兼容，方言默认）
        };
        if scope.sessions.is_empty() {
            return true;
        }
        let stripped = session_key
            .strip_prefix(WEB_SESSION_KEY_PREFIX)
            .unwrap_or(session_key);
        scope
            .sessions
            .iter()
            .any(|s| s == stripped || s == session_key)
    }

    /// matcher 是否命中该工具（方言别名或原始名任一）。
    fn matches(&self, raw_tool: &str) -> bool {
        let Some(pattern) = &self.matcher else {
            return true; // 无 matcher = 全命中（方言默认）
        };
        let candidates: Vec<&str> = match cc_tool_alias(raw_tool) {
            Some(alias) => vec![alias, raw_tool],
            None => vec![raw_tool],
        };
        match regex::Regex::new(pattern) {
            Ok(re) => candidates.iter().any(|c| re.is_match(c)),
            Err(_) => {
                // 无效正则：加载期 parse_cc_hooks → warn_invalid_matchers 已
                // 逐条 warn（BUG-20261117-054 前该承诺从未落地）；此处按
                // 既定语义退化为字面量子串匹配，每次调用不再打日志。
                candidates.iter().any(|c| c.contains(pattern.as_str()))
            }
        }
    }
}

/// 五个方言事件各自的分组列表。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CcEvents {
    #[serde(default)]
    pre_tool_use: Vec<CcHookGroup>,
    #[serde(default)]
    post_tool_use: Vec<CcHookGroup>,
    /// 工具执行失败后（三段化扩展）。观察型：stderr 只记日志。
    #[serde(default)]
    post_tool_use_failure: Vec<CcHookGroup>,
    #[serde(default)]
    session_start: Vec<CcHookGroup>,
    #[serde(default)]
    user_prompt_submit: Vec<CcHookGroup>,
    #[serde(default)]
    stop: Vec<CcHookGroup>,
    /// 会话结束（TTL 过期清理 / 显式删除）。观察型（会话已结束，无阻断）。
    #[serde(default)]
    session_end: Vec<CcHookGroup>,
    /// 压缩前后（扩展）。观察型：exit 2 不阻止压缩（稳定性机制）。
    #[serde(default)]
    pre_compact: Vec<CcHookGroup>,
    #[serde(default)]
    post_compact: Vec<CcHookGroup>,
    /// 子代理开始。exit 2 / JSON block = 拒绝
    /// spawn。仅挂本地 spawn 闭包（内部 LLM 委托不发，见模块头表）。
    #[serde(default)]
    subagent_start: Vec<CcHookGroup>,
    /// 子代理结束。观察型：任务已跑完，exit 2
    /// 无阻断语义。
    #[serde(default)]
    subagent_stop: Vec<CcHookGroup>,
    /// 审批/提问观察通知。纯观察型：边界处
    /// fire-and-forget，exit 2 无任何语义。层：loop 提问端（问题流）/
    /// auditor 审批 manager（security 三入口 + 管道 RequireApproval）。
    #[serde(default)]
    notification: Vec<CcHookGroup>,
}

impl CcEvents {
    fn total_scripts(&self) -> usize {
        self.script_counts().iter().map(|(_, n)| n).sum()
    }

    fn is_empty(&self) -> bool {
        self.total_scripts() == 0
    }

    /// Per-event script counts（hooks.json 的 PascalCase 事件名）。诊断/用
    /// UI 用（Hooks Tab 的 summary）——字段私有，外部 crate 走这里。
    /// 顺序：PreToolUse, PostToolUse, PostToolUseFailure, SessionStart,
    /// UserPromptSubmit, Stop, SessionEnd, PreCompact, PostCompact,
    /// SubagentStart, SubagentStop, Notification。
    pub fn script_counts(&self) -> [(&'static str, usize); 12] {
        let count = |v: &Vec<CcHookGroup>| v.iter().map(|g| g.hooks.len()).sum::<usize>();
        [
            ("PreToolUse", count(&self.pre_tool_use)),
            ("PostToolUse", count(&self.post_tool_use)),
            ("PostToolUseFailure", count(&self.post_tool_use_failure)),
            ("SessionStart", count(&self.session_start)),
            ("UserPromptSubmit", count(&self.user_prompt_submit)),
            ("Stop", count(&self.stop)),
            ("SessionEnd", count(&self.session_end)),
            ("PreCompact", count(&self.pre_compact)),
            ("PostCompact", count(&self.post_compact)),
            ("SubagentStart", count(&self.subagent_start)),
            ("SubagentStop", count(&self.subagent_stop)),
            ("Notification", count(&self.notification)),
        ]
    }

    /// 会话命中摘要（2026-11-17 会话资源盘点，hooks_global 类目）：逐事件
    /// 统计「会话圈定命中本会话」的分组脚本数——圈定判定复用
    /// [`CcHookGroup::matches_session`] 既有规则（裸 sid + 完整 session_key
    /// 双串任一即中；未圈定/空列表 = 全命中，口径地雷 3：照抄勿自造）。
    /// 顺序与 [`Self::script_counts`] 一致；0 计数事件不在返回里（看板只
    /// 展示命中的）。
    pub fn session_match_counts(&self, session_key: &str) -> Vec<(&'static str, usize)> {
        let count_matched = |v: &Vec<CcHookGroup>| {
            v.iter()
                .filter(|g| g.matches_session(session_key))
                .map(|g| g.hooks.len())
                .sum::<usize>()
        };
        let all = [
            ("PreToolUse", count_matched(&self.pre_tool_use)),
            ("PostToolUse", count_matched(&self.post_tool_use)),
            (
                "PostToolUseFailure",
                count_matched(&self.post_tool_use_failure),
            ),
            ("SessionStart", count_matched(&self.session_start)),
            ("UserPromptSubmit", count_matched(&self.user_prompt_submit)),
            ("Stop", count_matched(&self.stop)),
            ("SessionEnd", count_matched(&self.session_end)),
            ("PreCompact", count_matched(&self.pre_compact)),
            ("PostCompact", count_matched(&self.post_compact)),
            ("SubagentStart", count_matched(&self.subagent_start)),
            ("SubagentStop", count_matched(&self.subagent_stop)),
            ("Notification", count_matched(&self.notification)),
        ];
        all.into_iter().filter(|(_, n)| *n > 0).collect()
    }
}

/// 解析 hooks.json 内容。两种形态都收：标准 `{"hooks":{...}}` 与裸顶层
/// `{"PreToolUse":[...]}`（手写文件常见省略外层）。
pub fn parse_cc_hooks(json: &str) -> Result<CcEvents, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let events_value = match root.get("hooks") {
        Some(inner) => inner.clone(),
        None => root,
    };
    let mut events: CcEvents = serde_json::from_value(events_value.clone())
        .map_err(|e| format!("unsupported hooks.json format: {e}"))?;
    // 只认 type=="command"（方言唯一类型）；未知的跳过并统计。
    // 必须覆盖全部 12 个事件组（漏组 = type 写错的条目
    // 在该组照跑 command，「只认 command」契约失效）。
    let mut skipped = 0usize;
    for groups in [
        &mut events.pre_tool_use,
        &mut events.post_tool_use,
        &mut events.post_tool_use_failure,
        &mut events.session_start,
        &mut events.session_end,
        &mut events.user_prompt_submit,
        &mut events.stop,
        &mut events.pre_compact,
        &mut events.post_compact,
        &mut events.subagent_start,
        &mut events.subagent_stop,
        &mut events.notification,
    ] {
        for g in groups.iter_mut() {
            // scope.sessions 归一 + 校验（会话圈定）：条目去空白；去空白后
            // 为空的条目 = 永不命中的静默配置错误，loud 拒绝（走现有
            // valid:false 通道：load_from_dir fail-open / hooks.set 拒写）。
            if let Some(scope) = g.scope.as_mut() {
                scope.sessions = scope
                    .sessions
                    .iter()
                    .map(|s| s.trim().to_string())
                    .collect();
                if scope.sessions.iter().any(String::is_empty) {
                    return Err("scope.sessions contains empty entry".to_string());
                }
            }
            let before = g.hooks.len();
            g.hooks
                .retain(|h| h.r#type.is_empty() || h.r#type == "command");
            skipped += before - g.hooks.len();
        }
    }
    if skipped > 0 {
        tracing::warn!("[cc-hooks] skipped {} non-command hook(s)", skipped);
    }
    // matcher 加载期编译验证（BUG-20261117-054）：无效正则运行期按既定
    // 语义退化为字面量子串匹配（`CcHookGroup::matches`，模块头「诚实边界」
    // 明示 warn 提示）——此前该 warn 承诺从未落地，打错字的 matcher 全链
    // 零诊断、hook 静默永不触发。加载只走一次，逐条带 pattern 原文 +
    // 编译错误，不吵。
    let invalid_matchers = warn_invalid_matchers(&events);
    if invalid_matchers > 0 {
        tracing::warn!(
            "[cc-hooks] {invalid_matchers} matcher(s) failed regex compilation — degraded to literal substring matching"
        );
    }
    Ok(events)
}

/// 逐事件组编译 matcher 正则，无效则逐条 warn（含 pattern 原文与编译错误），
/// 返回无效条数。12 个事件组必须全覆盖（与上方 type 过滤同清单纪律——
/// 漏组 = 该组的坏 matcher 零诊断）。
fn warn_invalid_matchers(events: &CcEvents) -> usize {
    let mut invalid = 0usize;
    for groups in [
        &events.pre_tool_use,
        &events.post_tool_use,
        &events.post_tool_use_failure,
        &events.session_start,
        &events.session_end,
        &events.user_prompt_submit,
        &events.stop,
        &events.pre_compact,
        &events.post_compact,
        &events.subagent_start,
        &events.subagent_stop,
        &events.notification,
    ] {
        for g in groups {
            if let Some(pattern) = &g.matcher
                && let Err(e) = regex::Regex::new(pattern)
            {
                invalid += 1;
                tracing::warn!(
                    "[cc-hooks] invalid matcher regex {pattern:?} ({e}) — will fall back to literal substring matching"
                );
            }
        }
    }
    invalid
}

// ---------------------------------------------------------------------------
// 会话级钩子存储（hooks.session.json，agent `hooks` 工具的落盘形态）
// ---------------------------------------------------------------------------

/// 会话钩子合法事件名（12 事件全量；单一真相源——`hooks` 工具 add 校验、
/// 测试断言同源引用，顺序同 [`CcEvents::script_counts`] 注释）。
pub const SESSION_HOOK_EVENTS: [&str; 12] = [
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "SessionEnd",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
    "Notification",
];

/// 会话级钩子条目（agent `hooks` 工具写入的存储形态，比方言 CcCommand 多
/// `id`——本会话内 `h1..hn` 递增分配，delete 按它摘除）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionHookEntry {
    pub id: String,
    /// 工具名 matcher（方言正则语义；空串 = 全部工具，序列化保留空串）。
    #[serde(default)]
    pub matcher: String,
    /// shell 型条目的命令。与 [`SessionHookEntry::prompt`] 恰好一项非空
    /// （Exactly one，parse 校验强制——双空无执行体、双非空混合条目语义未定
    /// 均 loud 拒绝；工具写入路径已保证，防御手编坏文件）。
    #[serde(default)]
    pub command: String,
    /// 提示词型条目（LLM 执行体）：turn 收尾后把这段提示词作为新的用户消息
    /// 注入会话开启全新一轮。仅 `Stop` 事件有意义——Stop 钩点每轮触发 →
    /// 每轮收尾后注入 → 自然循环（无封顶，永续是钩点正常执行的自然副作用）。
    #[serde(default)]
    pub prompt: String,
    /// 秒。`None` = 桥侧缺省 [`DEFAULT_TIMEOUT_SECS`]（仅 shell 型消费）。
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// hooks.session.json 存储形态：`sid → 事件名 → 条目`。BTreeMap 保证序列化
/// 键序确定（原子写产物字节稳定，diff/测试可断言）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SessionHookStore {
    #[serde(default)]
    pub hooks: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, Vec<SessionHookEntry>>,
    >,
}

impl SessionHookStore {
    /// 是否有实际内容（任一会话任一事件有条目）。空存储 ≠ 在场——
    /// load_from_dir 的建桥判定与冷启动提示都按它。
    pub fn is_empty(&self) -> bool {
        self.hooks.values().all(|m| m.values().all(Vec::is_empty))
    }

    /// 某会话（裸 sid）的全部条目数。
    pub fn session_len(&self, sid: &str) -> usize {
        self.hooks
            .get(sid)
            .map(|m| m.values().map(Vec::len).sum())
            .unwrap_or(0)
    }
}

/// 解析 hooks.session.json 文本。顶层 `{"hooks":{...}}`（与 hooks.json 同款
/// 外层包装，手写/排查时形态一致）。
pub fn parse_session_hook_store(text: &str) -> Result<SessionHookStore, String> {
    let store: SessionHookStore =
        serde_json::from_str(text).map_err(|e| format!("invalid hooks.session.json: {e}"))?;
    // 条目校验：command/prompt 至少一项非空、id 非空（工具写入路径已保证，
    // 防御手编坏文件）。
    for (sid, per_event) in &store.hooks {
        for (event, entries) in per_event {
            for e in entries {
                if e.id.trim().is_empty() {
                    return Err(format!("session {sid} event {event}: empty hook id"));
                }
                if e.command.trim().is_empty() && e.prompt.trim().is_empty() {
                    return Err(format!(
                        "session {sid} event {event}: hook needs non-empty command or prompt"
                    ));
                }
                // 恰好一项（与工具写入路径 Exactly one 契约同源）：双非空混合
                // 条目语义未定（shell 投影滤掉 prompt 条目 → command 被静默
                // 忽略）——loud 拒绝整文件，让手编者修好再落。
                if !e.command.trim().is_empty() && !e.prompt.trim().is_empty() {
                    return Err(format!(
                        "session {sid} event {event}: hook id {} has both command and prompt — \
                         exactly one must be non-empty",
                        e.id
                    ));
                }
                if !e.prompt.trim().is_empty() && event != "Stop" {
                    return Err(format!(
                        "session {sid} event {event}: prompt hooks are only valid on Stop"
                    ));
                }
            }
        }
    }
    Ok(store)
}

/// 会话级钩子存储的盘上路径（config 目录 + [`HOOKS_SESSION_FILE`]）。
pub fn session_hooks_path(config_dir: &Path) -> PathBuf {
    config_dir.join(HOOKS_SESSION_FILE)
}

/// 读 hooks.session.json。文件缺失/读失败 = 空存储（正常态，非错误）。
pub fn read_session_hook_store(config_dir: &Path) -> SessionHookStore {
    let path = session_hooks_path(config_dir);
    match nemesis_utils::fsutil::read_config_string(&path) {
        Ok(text) => match parse_session_hook_store(&text) {
            Ok(store) => store,
            Err(e) => {
                tracing::warn!(
                    "[cc-hooks] failed to parse {} — ignoring: {e}",
                    path.display()
                );
                SessionHookStore::default()
            }
        },
        Err(_) => SessionHookStore::default(),
    }
}

/// 原子写 hooks.session.json（BTreeMap 键序确定，产物字节稳定）。
pub fn write_session_hook_store(config_dir: &Path, store: &SessionHookStore) -> Result<(), String> {
    let path = session_hooks_path(config_dir);
    let text = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), text.as_bytes(), 0o600)
        .map_err(|e| e.to_string())
}

/// 孤儿对账（gateway 启动一次）：存储里 sid 不在 `live_sids` 的条目直删。
/// 返回摘除的 sid 数。幂等；写失败 warn 不 panic（下次启动重试）。
pub fn reconcile_session_orphans(config_dir: &Path, live_sids: &HashSet<String>) -> usize {
    let mut store = read_session_hook_store(config_dir);
    if store.is_empty() {
        return 0;
    }
    let before = store.hooks.len();
    store.hooks.retain(|sid, _| live_sids.contains(sid));
    let removed = before - store.hooks.len();
    if removed > 0 && write_session_hook_store(config_dir, &store).is_err() {
        tracing::warn!("[cc-hooks] reconcile_session_orphans: failed to write back");
    }
    removed
}

/// 会话级条目（某 sid）→ 方言 [`CcEvents`]：每条各自成组，组内
/// `scope.sessions` 钉死该 sid——**与用户 hooks.json 走同一
/// [`CcHookGroup::matches_session`] 过滤路径，零特殊分支**。
fn session_store_to_events(
    sid: &str,
    per_event: &std::collections::BTreeMap<String, Vec<SessionHookEntry>>,
) -> CcEvents {
    let group = |e: &SessionHookEntry| CcHookGroup {
        matcher: if e.matcher.trim().is_empty() {
            None
        } else {
            Some(e.matcher.trim().to_string())
        },
        scope: Some(CcScope {
            sessions: vec![sid.to_string()],
        }),
        hooks: vec![CcCommand {
            r#type: "command".to_string(),
            command: e.command.clone(),
            timeout: e.timeout,
        }],
    };
    let mut events = CcEvents::default();
    // shell 组只收 shell 型条目；prompt 型条目不进 CcEvents（shell 执行路径
    // 不认识它），由桥的会话 prompt 钩子表单独收集（load_from_dir /
    // refresh_session_events 与本函数同源消费同一 store）。
    let shell_only = |entries: &Vec<SessionHookEntry>| {
        entries
            .iter()
            .filter(|e| e.prompt.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>()
    };
    let push = |slot: &mut Vec<CcHookGroup>, entries: &Vec<SessionHookEntry>| {
        slot.extend(entries.iter().map(&group));
    };
    for (event, entries) in per_event {
        let entries = &shell_only(entries);
        match event.as_str() {
            "PreToolUse" => push(&mut events.pre_tool_use, entries),
            "PostToolUse" => push(&mut events.post_tool_use, entries),
            "PostToolUseFailure" => push(&mut events.post_tool_use_failure, entries),
            "SessionStart" => push(&mut events.session_start, entries),
            "UserPromptSubmit" => push(&mut events.user_prompt_submit, entries),
            "Stop" => push(&mut events.stop, entries),
            "SessionEnd" => push(&mut events.session_end, entries),
            "PreCompact" => push(&mut events.pre_compact, entries),
            "PostCompact" => push(&mut events.post_compact, entries),
            "SubagentStart" => push(&mut events.subagent_start, entries),
            "SubagentStop" => push(&mut events.subagent_stop, entries),
            "Notification" => push(&mut events.notification, entries),
            other => tracing::warn!("[cc-hooks] session store: unknown event {other:?} ignored"),
        }
    }
    events
}

/// 会话级条目（某 sid）的**提示词型**钩子提取：仅 `Stop` 事件下 prompt 非空
/// 条目，按存储序收集 prompt 文本。与 [`session_store_to_events`] 同源消费
/// 同一 store（load_from_dir / refresh_session_events 两处换装点成对调用）。
fn session_store_to_prompt_hooks(
    per_event: &std::collections::BTreeMap<String, Vec<SessionHookEntry>>,
) -> Vec<String> {
    per_event
        .get("Stop")
        .map(|entries| {
            entries
                .iter()
                .filter(|e| !e.prompt.trim().is_empty())
                .map(|e| e.prompt.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// 把会话级 [`CcEvents`] 并入全局快照（同名字段 Vec 追加；会话组自带
/// scope 钉死，run_group 过滤天然只对属主会话放行）。
fn merge_session_events(base: &mut CcEvents, extra: &CcEvents) {
    base.pre_tool_use.extend(extra.pre_tool_use.iter().cloned());
    base.post_tool_use
        .extend(extra.post_tool_use.iter().cloned());
    base.post_tool_use_failure
        .extend(extra.post_tool_use_failure.iter().cloned());
    base.session_start
        .extend(extra.session_start.iter().cloned());
    base.user_prompt_submit
        .extend(extra.user_prompt_submit.iter().cloned());
    base.stop.extend(extra.stop.iter().cloned());
    base.session_end.extend(extra.session_end.iter().cloned());
    base.pre_compact.extend(extra.pre_compact.iter().cloned());
    base.post_compact.extend(extra.post_compact.iter().cloned());
    base.subagent_start
        .extend(extra.subagent_start.iter().cloned());
    base.subagent_stop
        .extend(extra.subagent_stop.iter().cloned());
    base.notification.extend(extra.notification.iter().cloned());
}

// ---------------------------------------------------------------------------
// 工具名方言映射
// ---------------------------------------------------------------------------

/// 我们的工具名 → 方言工具名。方言脚本 matcher / payload 里用的是右边。
/// 覆盖方言生态高频工具（缺别名 = 方言 matcher 对该工具
/// 静默不命中，PreToolUse 审计面漏触发）。
pub fn cc_tool_alias(raw: &str) -> Option<&'static str> {
    Some(match raw {
        "exec" | "async_shell" => "Bash",
        "edit" | "edit_file" => "Edit",
        "multiedit" => "MultiEdit",
        "write_file" => "Write",
        "read_file" => "Read",
        "grep" => "Grep",
        "spawn" => "Task",
        "web_fetch" => "WebFetch",
        _ => return None,
    })
}

/// 在原始 args JSON 上补方言字段名别名（`file_path` 等），让
/// `jq.tool_input.file_path` 型真脚本直接可用。原字段全保留。
fn enrich_tool_input(arguments: &str) -> Value {
    let mut input: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    if !input.is_object() {
        input = serde_json::json!({});
    }
    let obj = input.as_object_mut().expect("just made it an object");
    if !obj.contains_key("file_path") {
        for k in ["file_path", "path", "file"] {
            if let Some(v) = obj.get(k) {
                obj.insert("file_path".to_string(), v.clone());
                break;
            }
        }
    }
    input
}

// ---------------------------------------------------------------------------
// payload 构建（纯函数，可单测）
// ---------------------------------------------------------------------------

/// 构建发往脚本 stdin 的事件 JSON（单行紧凑）。
/// `extra`：事件特有字段（tool_name/tool_input/tool_response/prompt/...）。
pub fn build_event_payload(event: &str, session_key: &str, cwd: &Path, extra: Value) -> String {
    let mut payload = serde_json::json!({
        // 方言公共字段。session_id 用我们的 session_key；transcript_path
        // 无可指（诚实空串，见模块文档）。
        "session_id": session_key,
        "transcript_path": "",
        "cwd": cwd.to_string_lossy(),
        "hook_event_name": event,
    });
    if let (Some(dst), Some(src)) = (payload.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    payload.to_string()
}

/// PreToolUse 的 stdin JSON（含方言别名 + tool_input 别名增补）。
pub fn pre_tool_use_payload(call: &HookToolCall, cwd: &Path) -> String {
    build_event_payload(
        "PreToolUse",
        &call.session_key,
        cwd,
        serde_json::json!({
            "tool_name": cc_tool_alias(&call.name).unwrap_or(call.name.as_str()),
            "tool_input": enrich_tool_input(&call.arguments),
        }),
    )
}

/// SubagentStart/Stop 事件的发起方信息。由
/// spawn 闭包（`agent_factory.rs`）填写——桥侧只透传进 payload，不解释。
#[derive(Debug, Clone)]
pub struct SubagentInfo {
    /// 发起 spawn 的 agent id（父 loop 的标识）。
    pub agent_id: String,
    /// 子代理任务描述。payload 里**截断**到 [`SUBAGENT_TASK_TRUNCATE`] 字符
    /// （char 边界安全）——任务文本可能整段 prompt，stdin 不该无限膨胀。
    pub task: String,
    /// 子代理嵌套深度（父深度 + 1）。
    pub depth: usize,
    /// 是否后台路径。
    pub background: bool,
    /// 工具档位（readonly/full/...）。
    pub tools_profile: String,
    /// 挂载的 loop 类别：`"main"` | `"project"`（集群不挂钩子，无第三态）。
    pub loop_kind: &'static str,
    /// 发起 spawn 的父会话身份（session_key 或裸 sid；空串 = 无会话身份，
    /// 只被未圈定的组命中）。HOOK 会话维度批次三接线——此前硬编码空串，
    /// agent 给自己设的 SubagentStart/Stop 圈定钩子将永不触发。spawn 闭包
    /// 从发起会话的上下文填充（agent_factory shared.rs/project.rs）。
    pub session_key: String,
}

/// payload 里 task 字段的截断上限（字符数）。Notification 的 question/reason
/// 摘录共用同一上限。
pub const SUBAGENT_TASK_TRUNCATE: usize = 200;

/// 按字符截断（不切 UTF-8 字节）。
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

impl SubagentInfo {
    fn truncated_task(&self) -> String {
        truncate_chars(&self.task, SUBAGENT_TASK_TRUNCATE)
    }

    /// Start 事件的 stdin JSON（公共字段之上）。session_id = 父会话身份
    /// （批次三：会话圈定过滤锚点；空串 = 无会话身份，只被未圈定组命中）。
    fn start_payload(&self, cwd: &Path) -> String {
        build_event_payload(
            "SubagentStart",
            &self.session_key,
            cwd,
            serde_json::json!({
                "agent_id": self.agent_id,
                "task": self.truncated_task(),
                "depth": self.depth,
                "background": self.background,
                "tools_profile": self.tools_profile,
                "loop": self.loop_kind,
            }),
        )
    }

    /// Stop 事件的 stdin JSON。`outcome`：completed | cancelled | failed；
    /// `error`：失败摘录（仅 failed 时 Some，截到 200 字符）。
    fn stop_payload(&self, cwd: &Path, outcome: &str, error: Option<&str>) -> String {
        build_event_payload(
            "SubagentStop",
            &self.session_key,
            cwd,
            serde_json::json!({
                "agent_id": self.agent_id,
                "task": self.truncated_task(),
                "depth": self.depth,
                "background": self.background,
                "tools_profile": self.tools_profile,
                "loop": self.loop_kind,
                "outcome": outcome,
                "error": error.map(|e| truncate_chars(e, SUBAGENT_TASK_TRUNCATE)),
            }),
        )
    }
}

// ---------------------------------------------------------------------------
// 脚本执行
// ---------------------------------------------------------------------------

/// 一次脚本执行的结果。
#[derive(Debug, Clone)]
pub struct ScriptOutcome {
    /// 进程退出码（被 kill / 启动失败时 None）。
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// 跑一条 hook 命令：shell 包裹（Windows `cmd /C`，其他 `sh -c`）、cwd 与
/// `CLAUDE_PROJECT_DIR` 指向 project_dir、stdin 喂 payload、超时 kill。
/// 失败（启动失败/超时）返回带标志的 outcome，由调用方按非阻断处理。
pub async fn run_hook_script(
    command: &str,
    timeout_secs: u64,
    payload: &str,
    project_dir: &Path,
) -> ScriptOutcome {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C");
        // raw_arg：命令串**原样**进命令行。默认的 argv 转义会把内嵌引号
        // 变成 `\"`（MSVCRT 规则），而 cmd 不认 `\"`——用户脚本里带引号的
        // 路径（`python "C:\my lint\check.py"`）会被打挂。raw_arg 让 cmd
        // 按自己的原生语法解析，与真 shell 收到的一致。（实测echo
        // JSON 的退出码测试揭出此 bug。）
        #[cfg(windows)]
        c.raw_arg(command);
        #[cfg(not(windows))]
        c.arg(command);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    cmd.current_dir(project_dir)
        .env("CLAUDE_PROJECT_DIR", project_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // env 窄版闸（BUG-20261117-048，家族闸纪律对齐 cli_delegation）：hook
    // 是用户配置面第三方脚本，不继承 bot 内部命名空间变量；窄剥保留
    // 第三方自身 auth env（git hooks 场景需要 PATH/HOME/登录态）。
    let _stripped = nemesis_utils::env_sanitize::sanitize_tokio_command_narrow(&mut cmd);
    // Windows 下子控制台进程不弹新窗口（CLAUDE.md 禁令：绝不创建可见窗口）。
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

    let spawn_res = cmd.spawn();
    let mut child = match spawn_res {
        Ok(c) => c,
        Err(e) => {
            return ScriptOutcome {
                code: None,
                stdout: String::new(),
                stderr: format!("failed to spawn hook: {e}"),
                timed_out: false,
            };
        }
    };
    // stdin 写入放独立任务：payload 大于 pipe buffer
    // （PostToolUse 的 tool_response 可达数 MB）且脚本不读 stdin 时，inline
    // write_all 会阻塞到脚本退出——timeout 只包下面的 wait，管不到这里，
    // 等于超时保护整段失效（agent loop 挂死面）。独立任务写：写完 drop 关
    // 写端（脚本读到 EOF）；超时 kill 后管道断裂，write 返回错误、任务自然
    // 结束，无泄漏。
    if let Some(stdin) = child.stdin.take() {
        let payload_bytes = payload.as_bytes().to_vec();
        tokio::spawn(async move {
            let mut stdin = stdin;
            let _ = stdin.write_all(&payload_bytes).await;
        });
    }

    let fut = child.wait_with_output();
    match tokio::time::timeout(Duration::from_secs(timeout_secs.max(1)), fut).await {
        Ok(Ok(out)) => ScriptOutcome {
            code: out.status.code(),
            // 解码走单一真相源（BUG-20261115-034）：hook 经 cmd /C 无控制台
            // 管道执行，cmd 内建/控制台工具输出按 OEM 码页产出。
            stdout: nemesis_utils::child_decode::decode_child_output(&out.stdout),
            stderr: nemesis_utils::child_decode::decode_child_output(&out.stderr),
            timed_out: false,
        },
        Ok(Err(e)) => ScriptOutcome {
            code: None,
            stdout: String::new(),
            stderr: format!("hook process failed: {e}"),
            timed_out: false,
        },
        Err(_) => {
            // 超时：wait_with_output future 被 drop，kill_on_drop(true) 随之
            // 杀掉子进程（future 拥有 child，drop 即 kill）。
            ScriptOutcome {
                code: None,
                stdout: String::new(),
                stderr: format!("hook timed out after {timeout_secs}s"),
                timed_out: true,
            }
        }
    }
}

impl ScriptOutcome {
    /// 方言退出码语义：2 = 阻断；0 = 放行；其他（含 None/超时）= 非阻断错误。
    fn is_blocking_exit(&self) -> bool {
        self.code == Some(2)
    }

    /// exit 0 时的 stdout JSON 决议：`{"decision":"block","reason":...}`。
    /// 其他形态（含解析失败）一律 None。
    fn json_block_reason(&self) -> Option<String> {
        if self.code != Some(0) {
            return None;
        }
        let v: Value = serde_json::from_str(self.stdout.trim()).ok()?;
        if v.get("decision").and_then(|d| d.as_str()) == Some("block") {
            Some(
                v.get("reason")
                    .and_then(|r| r.as_str())
                    .unwrap_or("blocked by hook (no reason given)")
                    .to_string(),
            )
        } else {
            None
        }
    }

    /// 阻断理由文本：stderr 优先（方言语义），空则 stdout，再空则占位。
    fn block_text(&self) -> String {
        let s = self.stderr.trim();
        if !s.is_empty() {
            return s.to_string();
        }
        let s = self.stdout.trim();
        if !s.is_empty() {
            return s.to_string();
        }
        "blocked by hook (no output)".to_string()
    }
}

// ---------------------------------------------------------------------------
// 桥本体
// ---------------------------------------------------------------------------

/// hooks.json 方言 → 钩子体系桥。实现 [`ToolHook`]（PreToolUse/PostToolUse）与
/// [`LifecycleHook`]（SessionStart/UserPromptSubmit/Stop）。
///
/// **热更（hooks）**：盘上加载的桥（经
/// [`Self::load_from_dir`]）在每次分发入口惰性检查 hooks.json 的内容，
/// 变化即重新解析并原子换装——保存即下条消息生效，无需重启 Agent。语义：
/// 解析失败保留旧配置（fail-open 保旧，坏文件不炸会话）；文件清空/消失 =
/// 换装为空 = 事实注销（与启动态「没有 hooks.json = 没配」同一语义）。纯
/// 内存桥（[`Self::from_json`]，测试/手工构造）无热更。
pub struct CcHookBridge {
    /// 当前生效配置（热更换装点）。分发入口取 clone 快照再跑脚本——锁不跨
    /// await（CcEvents 为 Vec 簇小结构，clone 便宜）。含会话级合并结果：
    /// 会话级组自带 scope 钉死（[`session_store_to_events`]），与全局组同
    /// 一过滤路径。
    events: RwLock<CcEvents>,
    /// hooks.json 盘上路径；`None` = 纯内存桥，热更关闭。
    hooks_path: Option<PathBuf>,
    /// 上次记账的盘上原始文本（read_config_string 同源剥 BOM；None = 缺失）。
    /// 与文件现状比较决定是否重载；**解析失败也记录**——同一版坏文件只 warn
    /// 一次，用户改好文件（内容再变）后自然重试。内容比对不赌 mtime
    /// （同族收编：Windows 毫秒级粒度下同刻改写漏检 =
    /// 换装静默滞留旧版）。
    seen_raw: Mutex<Option<String>>,
    /// hooks.session.json 盘上路径（agent `hooks` 工具存储；`None` = 纯内存
    /// 桥，会话级热更关闭）。
    session_path: Option<PathBuf>,
    /// 会话级文件上次记账的原始文本（与 [`Self::seen_raw`] 同语义，双文件
    /// 各自独立内容比对）。
    seen_session_raw: Mutex<Option<String>>,
    /// 会话级钩子内存态：`sid → CcEvents`（scope 已钉死 sid）。热更整表
    /// 换装；`on_session_end` 摘除单条。
    session_events: RwLock<HashMap<String, CcEvents>>,
    /// 会话级**提示词型**钩子表：`sid → [prompt]`（仅 Stop 事件、prompt 非空
    /// 条目）。与 [`Self::session_events`] 同闸同生命周期：同一内容比对触发
    /// 同一次换装（load_from_dir / refresh_session_events 三分支同步）、
    /// `on_session_end` purge 同步摘除。消费点在 [`Self::on_turn_end`] 尾部
    /// （shell 全放行后）——返回 [`TurnEndDecision::Inject`]，由 loop 在
    /// turn 收尾后把 prompt 作为新用户消息注入（永续循环语义，见 hooks.rs）。
    session_prompt_hooks: RwLock<HashMap<String, Vec<String>>>,
    project_dir: PathBuf,
    /// SessionStart 只跑一次的判重（session_key → 已）。
    seen_sessions: Mutex<HashSet<String>>,
    /// 每 session 的 Stop 连续阻断计数（`stop_hook_active` 来源；正常
    /// Stop 时清除）。真正的轮次封顶在 loop 侧（MAX_TURN_END_CONTINUES）。
    stop_blocks: Mutex<HashMap<String, u32>>,
}

impl CcHookBridge {
    /// 解析 + 绑定 workspace。不读盘（盘上加载见 [`Self::load_from_dir`]）。
    pub fn from_json(json: &str, project_dir: PathBuf) -> Result<Self, String> {
        let events = parse_cc_hooks(json)?;
        Ok(Self {
            events: RwLock::new(events),
            hooks_path: None,
            seen_raw: Mutex::new(None),
            session_path: None,
            seen_session_raw: Mutex::new(None),
            session_events: RwLock::new(HashMap::new()),
            session_prompt_hooks: RwLock::new(HashMap::new()),
            project_dir,
            seen_sessions: Mutex::new(HashSet::new()),
            stop_blocks: Mutex::new(HashMap::new()),
        })
    }

    /// 从 config 目录加载。**任一文件在场且有内容即建桥**（HOOK 会话维度
    /// 批次三冷启动扩展）：hooks.json 缺失但 hooks.session.json 有钩子 →
    /// 照常建桥（全局侧空 `CcEvents`），否则 agent 先装的会话级钩子永不
    /// 生效（写盘无载体）。两侧都空（双缺/零脚本/空存储）→ None（静默，
    /// 正常态，HookView 冷启动提示即此）；hooks.json 存在但解析失败且会话
    /// 侧也无内容 → warn + None（fail-open，不拖死 gateway）——会话侧有
    /// 内容时全局侧降级为空 + warn（桥照建，会话钩子不受全局坏文件拖累，
    /// 下次内容变化热更自然重试全局侧）。
    pub fn load_from_dir(config_dir: &Path, project_dir: PathBuf) -> Option<Arc<Self>> {
        let path = config_dir.join(HOOKS_FILE);
        let session_path = session_hooks_path(config_dir);
        let global_text = nemesis_utils::fsutil::read_config_string(&path).ok();
        let session_text = nemesis_utils::fsutil::read_config_string(&session_path).ok();
        // 会话侧解析（缺失/失败 = 空存储；失败 warn 一次）。
        let session_store = match &session_text {
            Some(t) => match parse_session_hook_store(t) {
                Ok(s) => Some(s),
                Err(e) => {
                    tracing::warn!(
                        "[cc-hooks] failed to parse {} — session hooks ignored: {e}",
                        session_path.display()
                    );
                    None
                }
            },
            None => None,
        };
        let has_session = session_store.as_ref().is_some_and(|s| !s.is_empty());
        // 全局侧文本在场但两边都无内容 → 与旧语义一致：不建桥。
        let global_empty_or_missing = global_text
            .as_deref()
            .is_none_or(|t| parse_cc_hooks(t).map(|e| e.is_empty()).unwrap_or(true));
        if !has_session && global_empty_or_missing {
            if let Some(t) = &global_text {
                // 在场但空/坏：区分日志（坏 = warn 保旧语义；空 = info）。
                if parse_cc_hooks(t).is_err() {
                    tracing::warn!(
                        "[cc-hooks] failed to parse {} — hooks DISABLED (fail-open): 不可解析",
                        path.display()
                    );
                } else {
                    tracing::info!(
                        "[cc-hooks] {} loaded but declares no scripts",
                        path.display()
                    );
                }
            }
            return None;
        }
        // 到这里至少一侧有内容：建桥。全局侧缺失/坏 = 空 CcEvents + warn。
        let global_events = match &global_text {
            Some(t) => match parse_cc_hooks(t) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        "[cc-hooks] failed to parse {} — global hooks DISABLED (fail-open), \
                         session hooks stay active: {e}",
                        path.display()
                    );
                    CcEvents::default()
                }
            },
            None => CcEvents::default(),
        };
        // 会话级内存态：sid → CcEvents（scope 钉死）+ sid → prompt 钩子表
        // （两表同源消费同一 store，成对构建）。
        let mut session_events = HashMap::new();
        let mut session_prompt_hooks = HashMap::new();
        if let Some(store) = &session_store {
            for (sid, per_event) in &store.hooks {
                if !per_event.values().all(Vec::is_empty) {
                    session_events.insert(sid.clone(), session_store_to_events(sid, per_event));
                }
                let prompts = session_store_to_prompt_hooks(per_event);
                if !prompts.is_empty() {
                    session_prompt_hooks.insert(sid.clone(), prompts);
                }
            }
        }
        let total = global_events.total_scripts()
            + session_events
                .values()
                .map(|e| e.total_scripts())
                .sum::<usize>();
        tracing::info!(
            "[cc-hooks] loaded {} script(s) from {} (+{} session-scoped) — {:?}",
            global_events.total_scripts(),
            path.display(),
            total - global_events.total_scripts(),
            global_events.script_counts()
        );
        let bridge = Self {
            events: RwLock::new(global_events),
            hooks_path: Some(path),
            seen_raw: Mutex::new(global_text),
            session_path: Some(session_path),
            seen_session_raw: Mutex::new(session_text),
            session_events: RwLock::new(session_events),
            session_prompt_hooks: RwLock::new(session_prompt_hooks),
            project_dir,
            seen_sessions: Mutex::new(HashSet::new()),
            stop_blocks: Mutex::new(HashMap::new()),
        };
        Some(Arc::new(bridge))
    }

    /// 挂到 AgentLoop（工具钩子 + 生命周期钩子一起）。
    pub fn register(self: Arc<Self>, agent_loop: &crate::r#loop::AgentLoop) {
        agent_loop.add_tool_hook(self.clone());
        agent_loop.add_lifecycle_hook(self);
    }

    /// **分发入口**：hooks.json + hooks.session.json 内容变化则热更换装，
    /// 返回当前生效配置（全局 + 该会话圈定钩子合并）的 clone 快照（锁不跨
    /// await）。纯内存桥直接返回快照。`session`：发起事件的会话身份——
    /// 会话级钩子按剥 [`WEB_SESSION_KEY_PREFIX`] 的 sid（或整键）取出并
    /// 并入快照（组自带 scope 钉死，[`CcHookGroup::matches_session`] 再滤
    /// 一道，纵深防御）。
    ///
    /// 换装语义（hooks.json 与 hooks.session.json 各自独立、同构）：
    /// - 内容变化 + 解析成功 → 原子换装（info 记脚本计数）。
    /// - 内容变化 + 解析失败 → warn + **保留旧配置**；文本已记账，同版
    ///   坏文件只 warn 一次。
    /// - 文件消失（NotFound）→ 视同用户清空：换装为空 = 事实注销，与
    ///   启动态「没有 hooks.json = 没配」同一语义。
    /// - 瞬时读失败（编辑器/杀软锁，非 NotFound）→ 保留现状不记账，下轮
    ///   重试（与「消失」分立——读不到 ≠ 不存在； 同款
    ///   教训。旧实现 metadata 一并 Err 混同两态，此处顺手分立）。
    fn current_events(&self, session: &str) -> CcEvents {
        let mut events = self.refresh_global_events();
        if !session.is_empty() {
            let per = self.refresh_session_events();
            if !per.is_empty() {
                let sid = session
                    .strip_prefix(WEB_SESSION_KEY_PREFIX)
                    .unwrap_or(session);
                if let Some(extra) = per.get(sid).or_else(|| per.get(session)) {
                    merge_session_events(&mut events, extra);
                }
            }
        }
        events
    }

    /// hooks.json 侧热更（原 current_events 主体，逻辑零变化）。
    fn refresh_global_events(&self) -> CcEvents {
        let Some(path) = self.hooks_path.clone() else {
            return self.events.read().unwrap().clone();
        };
        // 内容比对不赌钟（同族收编）：mtime 等值判定在
        // Windows 毫秒级粒度（346d3dc2 同 tick 同 nanos CI）下漏检
        // 同刻改写——hooks.json 换装静默滞留旧版。
        let text = match nemesis_utils::fsutil::read_config_string(&path) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            // 非 NotFound（瞬时锁等）：保留现状不记账，下轮重试。
            Err(_) => return self.events.read().unwrap().clone(),
        };
        if text == *self.seen_raw.lock().unwrap() {
            return self.events.read().unwrap().clone();
        }
        // 内容变了（或首检）。按文件现状三分支处理。
        *self.seen_raw.lock().unwrap() = text.clone();
        match text {
            None => {
                tracing::info!(
                    "[cc-hooks] {} gone — hooks deactivated (hot-reload)",
                    path.display()
                );
                *self.events.write().unwrap() = CcEvents::default();
            }
            Some(t) => match parse_cc_hooks(&t) {
                Ok(events) => {
                    tracing::info!(
                        "[cc-hooks] hot-reloaded {} ({} script(s))",
                        path.display(),
                        events.total_scripts()
                    );
                    *self.events.write().unwrap() = events;
                }
                Err(e) => {
                    tracing::warn!(
                        "[cc-hooks] hot-reload failed to parse {} — keeping previous config: {}",
                        path.display(),
                        e
                    );
                }
            },
        }
        self.events.read().unwrap().clone()
    }

    /// hooks.session.json 侧热更（与 hooks.json 同构三分支；整表换装）。
    /// 纯内存桥（session_path None）直接返回现表——工具写入的即时生效由
    /// 「桥在场 → 有路径 → 下次分发内容比对捡到」承载；桥不在场（双文件
    /// 双缺的冷启动）工具侧诚实提示「重启后生效」。
    fn refresh_session_events(&self) -> HashMap<String, CcEvents> {
        let Some(path) = self.session_path.clone() else {
            return self.session_events.read().unwrap().clone();
        };
        let text = match nemesis_utils::fsutil::read_config_string(&path) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return self.session_events.read().unwrap().clone(),
        };
        if text == *self.seen_session_raw.lock().unwrap() {
            return self.session_events.read().unwrap().clone();
        }
        *self.seen_session_raw.lock().unwrap() = text.clone();
        match text.as_deref() {
            None => {
                tracing::info!(
                    "[cc-hooks] {} gone — session hooks deactivated (hot-reload)",
                    path.display()
                );
                self.session_events.write().unwrap().clear();
                self.session_prompt_hooks.write().unwrap().clear();
            }
            Some(t) => match parse_session_hook_store(t) {
                Ok(store) => {
                    let mut map = HashMap::new();
                    let mut prompts = HashMap::new();
                    for (sid, per_event) in &store.hooks {
                        if !per_event.values().all(Vec::is_empty) {
                            map.insert(sid.clone(), session_store_to_events(sid, per_event));
                        }
                        let sid_prompts = session_store_to_prompt_hooks(per_event);
                        if !sid_prompts.is_empty() {
                            prompts.insert(sid.clone(), sid_prompts);
                        }
                    }
                    tracing::info!(
                        "[cc-hooks] hot-reloaded {} ({} session(s) with hooks)",
                        path.display(),
                        map.len()
                    );
                    *self.session_events.write().unwrap() = map;
                    *self.session_prompt_hooks.write().unwrap() = prompts;
                }
                Err(e) => {
                    tracing::warn!(
                        "[cc-hooks] hot-reload failed to parse {} — keeping previous config: {}",
                        path.display(),
                        e
                    );
                }
            },
        }
        self.session_events.read().unwrap().clone()
    }

    /// 依次跑一组里所有命中**会话圈定**与 matcher 的命令。`session`：发起
    /// 事件的会话身份（session_key 或裸 sid；空串 = 无会话身份的事件，如
    /// subagent 历史载荷——只被未圈定的组命中）。圈定过滤先于 matcher：
    /// 不命中直接跳过，**不 spawn 子进程**（零进程开销）。
    async fn run_group(
        &self,
        groups: &[CcHookGroup],
        tool: Option<&str>,
        payload: &str,
        session: &str,
    ) -> Vec<ScriptOutcome> {
        let mut out = Vec::new();
        for g in groups {
            if !g.matches_session(session) {
                continue;
            }
            if let Some(t) = tool
                && !g.matches(t)
            {
                continue;
            }
            for h in &g.hooks {
                let timeout = resolve_hook_timeout(h.timeout);
                let o = run_hook_script(&h.command, timeout, payload, &self.project_dir).await;
                // 非阻断失败（超时/非 0 非 2 退出码/启动失败）响亮 warn 后继续。
                if o.code.is_none() || (o.code != Some(0) && o.code != Some(2)) {
                    tracing::warn!(
                        "[cc-hooks] non-blocking failure: code={:?} timed_out={} stderr={}",
                        o.code,
                        o.timed_out,
                        o.stderr.trim()
                    );
                } else if !o.stdout.trim().is_empty() {
                    tracing::info!(
                        "[cc-hooks] hook stdout (event={}): {}",
                        payload_json_event(payload),
                        o.stdout.trim()
                    );
                }
                out.push(o);
            }
        }
        out
    }

    /// 当前 session 的 stop_hook_active（测试用 pub(crate) 可）。
    // 仅 cc_hooks/tests.rs 调用（断言二次 Stop 的 stop_hook_active 标志），
    // 非测试构建无调用方 → allow 消警（生产链路走 `end.stop_hook_active`
    // 直读结构体，见下方 dispatch 处，不经此访问器）。
    #[allow(dead_code)]
    pub(crate) fn stop_hook_active_for(&self, session_key: &str) -> bool {
        self.stop_blocks
            .lock()
            .unwrap()
            .get(session_key)
            .copied()
            .unwrap_or(0)
            > 0
    }

    /// 全局侧钩子的会话匹配计数（资源看板 hooks_global 类目，2026-11-17
    /// 会话资源盘点）。锁内取全局 `CcEvents` 快照后委托
    /// [`CcEvents::session_match_counts`]（双串匹配复用 matches_session；
    /// 0 计数事件已过滤）。会话级钩子不走这里——hooks_session 类目按
    /// 计划口径直读 `hooks.session.json`（裸 sid 键）。
    pub fn global_session_match_counts(&self, session_key: &str) -> Vec<(&'static str, usize)> {
        // 走 refresh（懒重载：盘上内容变化即热更）而非直读缓存——盘点面
        // 与 dispatch 面同一新鲜度口径。
        let events = self.refresh_global_events();
        events.session_match_counts(session_key)
    }

    /// 方言 `SubagentStart`：spawn 闭包内、
    /// `run_detached` 前调。exit 2 / JSON block → **Err（拒绝 spawn）**——
    /// 阻断文案即拒绝原因，由调用方作为 SpawnFn 的 Err 上抛。无子代理钩子
    /// 配置时零开销直通。
    pub async fn dispatch_subagent_start(&self, info: &SubagentInfo) -> Result<(), String> {
        let events = self.current_events(&info.session_key);
        if events.subagent_start.is_empty() {
            return Ok(());
        }
        let payload = info.start_payload(&self.project_dir);
        // 会话身份 = 父会话（批次三接线 SubagentInfo.session_key）——agent
        // 给自己设的圈定 SubagentStart 钩子只对发起会话命中。
        for o in self
            .run_group(&events.subagent_start, None, &payload, &info.session_key)
            .await
        {
            if o.is_blocking_exit() {
                return Err(o.block_text());
            }
            if let Some(reason) = o.json_block_reason() {
                return Err(reason);
            }
        }
        Ok(())
    }

    /// 方言 `SubagentStop`（观察型）：detached 轮次返回 / 后台任务完成处调。
    /// 任务已跑完——exit 2 / JSON block 都**无阻断语义**，响亮 warn 记录
    /// （脚本想说的话只能进日志），绝不影响调用方的结果回传。
    pub async fn dispatch_subagent_stop(
        &self,
        info: &SubagentInfo,
        outcome: &str,
        error: Option<&str>,
    ) {
        let events = self.current_events(&info.session_key);
        if events.subagent_stop.is_empty() {
            return;
        }
        let payload = info.stop_payload(&self.project_dir, outcome, error);
        for o in self
            .run_group(&events.subagent_stop, None, &payload, &info.session_key)
            .await
        {
            if o.is_blocking_exit() || o.json_block_reason().is_some() {
                tracing::warn!(
                    "[cc-hooks] SubagentStop hook tried to block — task already finished, \
                     observation only: {}",
                    o.block_text()
                );
            }
        }
    }

    /// 方言 `Notification`：审批/提问边界观察
    /// 事件。**纯观察型**——exit 2 / JSON block 一律只 debug 记录，无任何
    /// 阻断语义（通知发出时审批/提问还没开始，outcome 尚不存在）。空配置
    /// 零开销直通。同步入口见 [`spawn_notification`]。
    pub async fn dispatch_notification(
        &self,
        session_key: &str,
        kind: &str,
        layer: &str,
        extra: Value,
    ) {
        let events = self.current_events(session_key);
        if events.notification.is_empty() {
            return;
        }
        let payload = notification_payload(session_key, &self.project_dir, kind, layer, extra);
        for o in self
            .run_group(&events.notification, None, &payload, session_key)
            .await
        {
            if o.is_blocking_exit() || o.json_block_reason().is_some() {
                tracing::debug!(
                    "[cc-hooks] Notification hook exit 2 ignored (observe-only): {}",
                    o.block_text()
                );
            }
        }
    }
}

/// `Notification` 载荷合成（纯函数，start_payload/stop_payload 同款可单测）：
/// kind/layer 打头，extra 字段并入，再叠方言公共字段。
fn notification_payload(
    session_key: &str,
    project_dir: &Path,
    kind: &str,
    layer: &str,
    extra: Value,
) -> String {
    let mut combined = serde_json::Map::new();
    combined.insert("kind".to_string(), Value::String(kind.to_string()));
    combined.insert("layer".to_string(), Value::String(layer.to_string()));
    if let Some(obj) = extra.as_object() {
        for (k, v) in obj {
            combined.insert(k.clone(), v.clone());
        }
    }
    build_event_payload(
        "Notification",
        session_key,
        project_dir,
        Value::Object(combined),
    )
}

/// `Notification` 的同步 fire-and-forget 入口：有 tokio runtime 句柄就
/// spawn 异步分发（绝不阻塞审批/提问边界）；无句柄（纯同步上下文，如部分
/// 测试）诚实降级为 debug 日志跳过。
fn spawn_notification(
    bridge: &Arc<CcHookBridge>,
    session_key: String,
    kind: &str,
    layer: &str,
    extra: Value,
) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            let bridge = Arc::clone(bridge);
            let kind = kind.to_string();
            let layer = layer.to_string();
            handle.spawn(async move {
                bridge
                    .dispatch_notification(&session_key, &kind, &layer, extra)
                    .await;
            });
        }
        Err(_) => {
            tracing::debug!("[cc-hooks] notification ({kind}/{layer}) skipped: no tokio runtime");
        }
    }
}

/// 从 payload 字符串抠 hook_event_name（日志用；解析失败给空串）。
fn payload_json_event(payload: &str) -> String {
    serde_json::from_str::<Value>(payload)
        .ok()
        .and_then(|v| {
            v.get("hook_event_name")
                .and_then(|e| e.as_str())
                .map(String::from)
        })
        .unwrap_or_default()
}

#[async_trait]
impl ToolHook for CcHookBridge {
    fn name(&self) -> String {
        "cc-hooks".to_string()
    }

    async fn pre_tool_use(&self, call: &HookToolCall) -> HookDecision {
        let events = self.current_events(&call.session_key);
        let payload = pre_tool_use_payload(call, &self.project_dir);
        for o in self
            .run_group(
                &events.pre_tool_use,
                Some(&call.name),
                &payload,
                &call.session_key,
            )
            .await
        {
            if o.is_blocking_exit() {
                return HookDecision::Block {
                    reason: o.block_text(),
                };
            }
            if let Some(reason) = o.json_block_reason() {
                return HookDecision::Block { reason };
            }
        }
        HookDecision::Allow
    }

    async fn post_tool_use(&self, call: &HookToolCall, result: &str) -> PostHookAction {
        let events = self.current_events(&call.session_key);
        let payload = build_event_payload(
            "PostToolUse",
            &call.session_key,
            &self.project_dir,
            serde_json::json!({
                "tool_name": cc_tool_alias(&call.name).unwrap_or(call.name.as_str()),
                "tool_input": enrich_tool_input(&call.arguments),
                "tool_response": result,
            }),
        );
        let mut notes = String::new();
        for o in self
            .run_group(
                &events.post_tool_use,
                Some(&call.name),
                &payload,
                &call.session_key,
            )
            .await
        {
            if o.is_blocking_exit() {
                // 方言语义：PostToolUse 阻断不撤销操作，stderr 反馈给模型。
                notes.push_str(&format!("\n\n[hook] {}", o.block_text()));
            } else if let Some(reason) = o.json_block_reason() {
                notes.push_str(&format!("\n\n[hook] {reason}"));
            }
        }
        if notes.is_empty() {
            PostHookAction::Continue
        } else {
            PostHookAction::Replace(format!("{result}{notes}"))
        }
    }

    /// 方言 `PostToolUseFailure`（三段化扩展）：工具执行失败后触发。
    /// 观察型——stderr 只记日志（失败已发生，无撤销/改写语义）。
    async fn post_tool_use_failure(&self, call: &HookToolCall, err: &str) -> PostHookAction {
        let events = self.current_events(&call.session_key);
        let payload = build_event_payload(
            "PostToolUseFailure",
            &call.session_key,
            &self.project_dir,
            serde_json::json!({
                "tool_name": cc_tool_alias(&call.name).unwrap_or(call.name.as_str()),
                "tool_input": enrich_tool_input(&call.arguments),
                "tool_error": err,
            }),
        );
        for o in self
            .run_group(
                &events.post_tool_use_failure,
                Some(&call.name),
                &payload,
                &call.session_key,
            )
            .await
        {
            if !o.stdout.is_empty() {
                tracing::info!("[cc-hooks] PostToolUseFailure stdout: {}", o.stdout);
            }
        }
        PostHookAction::Continue
    }
}

#[async_trait]
impl LifecycleHook for CcHookBridge {
    fn name(&self) -> String {
        "cc-hooks".to_string()
    }

    async fn on_user_prompt(&self, prompt: &HookPrompt) -> PromptDecision {
        let events = self.current_events(&prompt.session_key);
        // SessionStart：该 session 第一条 prompt 时先跑（source=startup）。
        let first_prompt = !self
            .seen_sessions
            .lock()
            .unwrap()
            .contains(&prompt.session_key);
        if first_prompt {
            self.seen_sessions
                .lock()
                .unwrap()
                .insert(prompt.session_key.clone());
            let payload = build_event_payload(
                "SessionStart",
                &prompt.session_key,
                &self.project_dir,
                serde_json::json!({ "source": "startup" }),
            );
            self.run_group(&events.session_start, None, &payload, &prompt.session_key)
                .await;
        }
        // UserPromptSubmit：exit 2 = 拦下 prompt（模型看不到）。
        let payload = build_event_payload(
            "UserPromptSubmit",
            &prompt.session_key,
            &self.project_dir,
            serde_json::json!({ "prompt": prompt.prompt }),
        );
        for o in self
            .run_group(
                &events.user_prompt_submit,
                None,
                &payload,
                &prompt.session_key,
            )
            .await
        {
            if o.is_blocking_exit() {
                return PromptDecision::Block {
                    reason: o.block_text(),
                };
            }
            if let Some(reason) = o.json_block_reason() {
                return PromptDecision::Block { reason };
            }
        }
        PromptDecision::Allow
    }

    async fn on_turn_end(&self, end: &crate::hooks::HookTurnEnd) -> TurnEndDecision {
        let events = self.current_events(&end.session_key);
        let payload = build_event_payload(
            "Stop",
            &end.session_key,
            &self.project_dir,
            serde_json::json!({ "stop_hook_active": end.stop_hook_active }),
        );
        for o in self
            .run_group(&events.stop, None, &payload, &end.session_key)
            .await
        {
            if o.is_blocking_exit() || o.json_block_reason().is_some() {
                *self
                    .stop_blocks
                    .lock()
                    .unwrap()
                    .entry(end.session_key.clone())
                    .or_insert(0) += 1;
                return TurnEndDecision::Continue {
                    feedback: o.block_text(),
                };
            }
        }
        // 正常放行 → 清计数（下一个 turn 的 stop_hook_active 从 false 起）。
        self.stop_blocks.lock().unwrap().remove(&end.session_key);
        // 提示词型钩子（LLM 执行体）：shell 全放行后查会话 prompt 表。
        // 表的新鲜度由开头 current_events → refresh_session_events 保证
        // （同一内容比对闸已触发换装）。多条 prompt 按存储序拼接为一条注入
        // （一次注入一个消息；钩子内容全部到达）。空 = 无 prompt 钩子，照常
        // Stop。Inject 由 loop 在 turn 收尾后注入新用户消息开启下一轮——
        // 下一轮结束 Stop 钩点再次触发 → 自然循环（无封顶，永续是钩点正常
        // 执行的自然副作用；estop/取消打断 turn → 非 Accept 路径 → 本钩子
        // 不触发 → 循环自然停）。
        if !end.session_key.is_empty() {
            let sid = end
                .session_key
                .strip_prefix(WEB_SESSION_KEY_PREFIX)
                .unwrap_or(&end.session_key);
            let prompts = self.session_prompt_hooks.read().unwrap();
            if let Some(list) = prompts
                .get(sid)
                .or_else(|| prompts.get(&end.session_key))
                .filter(|list| !list.is_empty())
            {
                let prompt = list.join("\n\n");
                tracing::info!(
                    "[cc-hooks] prompt hook fired for session '{}' ({} hook(s), {} bytes) — \
                     loop will inject after turn finalization",
                    end.session_key,
                    list.len(),
                    prompt.len()
                );
                return TurnEndDecision::Inject { prompt };
            }
        }
        TurnEndDecision::Stop
    }
}

impl CcHookBridge {
    /// 方言 `SessionEnd`（观察型）：会话被清理/删除时触发。exit 2 无阻断语义。
    /// 固有方法而非 trait——唯一实现者是本桥，不建单实现 trait（YAGNI）。
    ///
    /// 分发之后顺手做**会话钩子孤儿对账**：该会话在 hooks.session.json 里的
    /// 专属钩子随会话一起摘除（先分发后 purge——SessionEnd 钩子自身若被
    /// 圈定到本会话，仍要在死前响最后一次）。
    pub async fn on_session_end(&self, session_key: &str, reason: &str) {
        let events = self.current_events(session_key);
        let payload = build_event_payload(
            "SessionEnd",
            session_key,
            &self.project_dir,
            serde_json::json!({ "reason": reason }),
        );
        self.run_group(&events.session_end, None, &payload, session_key)
            .await;
        self.purge_session_hooks(session_key);
    }

    /// 会话结束后的孤儿清理：从 hooks.session.json 摘除该会话的全部条目，
    /// 并同步内存缓存（session_events map + seen_session_raw）。磁盘无该
    /// 会话条目 = 零写入；session 槽缺席（None）= 跳过（存储面未接线）。
    fn purge_session_hooks(&self, session_key: &str) {
        let Some(path) = self.session_path.as_ref() else {
            return;
        };
        let sid = session_key
            .strip_prefix(WEB_SESSION_KEY_PREFIX)
            .unwrap_or(session_key);
        let changed = {
            let config_dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
            let mut store = read_session_hook_store(config_dir);
            let removed = store.hooks.remove(sid).is_some();
            if !removed {
                return;
            }
            if let Err(e) = write_session_hook_store(config_dir, &store) {
                tracing::warn!(
                    "[cc-hooks] session hook purge write failed (kept in memory clean): {e}"
                );
                return;
            }
            true
        };
        if changed {
            // 内存侧同步：摘 map 条目 + 让下次热更比对不因内容倒退误判。
            self.session_events.write().unwrap().remove(sid);
            self.session_prompt_hooks.write().unwrap().remove(sid);
            let mut seen = self.seen_session_raw.lock().unwrap();
            if let Some(raw) = seen.as_mut()
                && let Ok(mut v) = serde_json::from_str::<Value>(raw)
            {
                if let Some(map) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) {
                    map.remove(sid);
                }
                *raw = v.to_string();
            }
            tracing::info!("[cc-hooks] purged session-scoped hooks for ended session {sid}");
        }
    }
}

impl CcHookBridge {
    /// 方言 `PreCompact` / `PostCompact`（观察型）：压缩流水线前后触发。
    /// exit 2 不阻止压缩（稳定性机制，诚实边界）。`session_key`：被压缩的
    /// 会话身份——会话圈定过滤锚点 + payload 的 session_id 字段（旧实现
    /// 占位 "system"，会话维度上线后改传真值，诚实标注来源会话）。
    pub async fn run_compact_hooks(&self, trigger: &str, phase: &str, session_key: &str) {
        let events = self.current_events(session_key);
        let event = if phase == "pre" {
            "PreCompact"
        } else {
            "PostCompact"
        };
        let payload = build_event_payload(
            event,
            session_key,
            &self.project_dir,
            serde_json::json!({ "trigger": trigger }),
        );
        let groups = if phase == "pre" {
            &events.pre_compact
        } else {
            &events.post_compact
        };
        for o in self.run_group(groups, None, &payload, session_key).await {
            if !o.stdout.is_empty() {
                tracing::info!("[cc-hooks] {event} stdout: {}", o.stdout);
            }
        }
    }
}

#[cfg(test)]
mod tests;

// 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;

// 覆盖率补充批次：legacy 迁移 / 方言解析 / run_hook_script 失败面 /
// ScriptOutcome 方言语义 / load_from_dir fail-open。
#[cfg(test)]
mod cov_tests;

// ---------------------------------------------------------------------------
// 观察包装器：审批/提问边界 → Notification
// ---------------------------------------------------------------------------

/// loop 提问端观察装饰（件3 层1）： doom-loop 审批卡与 结构化提问都
/// 经 `AgentLoop::question_asker()` 槽（wiring.rs 的 setter 处包装）。ask
/// 发起前发 `Notification`（kind=question, layer=loop），然后**无条件原样
/// 委托**——装饰器改不了 outcome（红线靠类型不靠纪律）。
///
/// 覆盖面诚实边界：只观察 **ask 方向**；用户作答（responder 方向）是已通
/// 知过的提问的结局，不重复发事件。无桥（集群/裸构建）时 setter 原样直挂。
pub struct ObservingQuestionAsker {
    inner: Arc<dyn nemesis_types::agent::QuestionAsker>,
    bridge: Arc<CcHookBridge>,
}

impl ObservingQuestionAsker {
    pub fn new(
        inner: Arc<dyn nemesis_types::agent::QuestionAsker>,
        bridge: Arc<CcHookBridge>,
    ) -> Self {
        Self { inner, bridge }
    }
}

impl nemesis_types::agent::QuestionAsker for ObservingQuestionAsker {
    fn ask(
        &self,
        request: nemesis_types::agent::QuestionRequest,
    ) -> Result<nemesis_types::agent::QuestionOutcome, String> {
        spawn_notification(
            &self.bridge,
            request.session_key.clone(),
            "question",
            "loop",
            serde_json::json!({
                "question_id": request.question_id,
                "question": truncate_chars(&request.question, SUBAGENT_TASK_TRUNCATE),
                "options": request.options,
                "multi": request.multi,
                "chat_id": request.chat_id,
            }),
        );
        self.inner.ask(request)
    }
}

/// security 审批 manager 观察装饰（件3 层2）：包住网关装配的真实 manager，
/// 审批发起前发 `Notification`（kind=security_ask, layer=auditor），然后
/// **无条件原样委托**（审批裁决完全由真实 manager 决定）。同步方法里不能
/// 阻塞等异步分发——[`spawn_notification`] fire-and-forget，脚本慢一拍无碍
/// （通知先行于审批窗口，窗口本身就是秒级）。
///
/// 覆盖面诚实边界（实施时逐点核对 auditor.rs）：auditor 的 manager 槽有
/// **四个**调用点——guardian_failure 二审（381）/ rate-limit 升级审批
/// （512）/ guardian verdict 二审（630）/ 管道 RequireApproval 交互审批
/// （904）——包装槽即全覆盖。**不覆盖**：skill_manage 写审批与 memory 写
/// 门（gateway 直接持有裸 manager Arc，不经 auditor 槽）。不宣称全覆盖。
#[cfg(feature = "security")]
pub struct ObservingApprovalManager {
    inner: Arc<dyn nemesis_security::auditor::ApprovalManager>,
    bridge: Arc<CcHookBridge>,
}

#[cfg(feature = "security")]
impl ObservingApprovalManager {
    pub fn new(
        inner: Arc<dyn nemesis_security::auditor::ApprovalManager>,
        bridge: Arc<CcHookBridge>,
    ) -> Self {
        Self { inner, bridge }
    }
}

#[cfg(feature = "security")]
impl nemesis_security::auditor::ApprovalManager for ObservingApprovalManager {
    fn is_running(&self) -> bool {
        self.inner.is_running()
    }

    fn request_approval_sync(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        spawn_notification(
            &self.bridge,
            String::new(),
            "security_ask",
            "auditor",
            serde_json::json!({
                "request_id": request_id,
                "operation": operation,
                "target": truncate_chars(target, SUBAGENT_TASK_TRUNCATE),
                "risk_level": risk_level,
                "reason": truncate_chars(reason, SUBAGENT_TASK_TRUNCATE),
                "timeout_secs": timeout_secs,
            }),
        );
        self.inner.request_approval_sync(
            request_id,
            operation,
            target,
            risk_level,
            reason,
            timeout_secs,
        )
    }

    fn request_approval_sync_ctx(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
        ctx: &nemesis_security::auditor::ApprovalContext,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        spawn_notification(
            &self.bridge,
            String::new(),
            "security_ask",
            "auditor",
            serde_json::json!({
                "request_id": request_id,
                "operation": operation,
                "target": truncate_chars(target, SUBAGENT_TASK_TRUNCATE),
                "risk_level": risk_level,
                "reason": truncate_chars(reason, SUBAGENT_TASK_TRUNCATE),
                "timeout_secs": timeout_secs,
            }),
        );
        self.inner.request_approval_sync_ctx(
            request_id,
            operation,
            target,
            risk_level,
            reason,
            timeout_secs,
            ctx,
        )
    }
}

// Wave6B 覆盖率收尾批次：观察型事件 exit-2 日志臂（SubagentStop warn /
// Notification debug / compact stdout info）/ notification_payload extra
// 并入 / legacy 迁移 copy 失败 warn / ObservingApprovalManager is_running。
#[cfg(test)]
mod cov2_tests;
