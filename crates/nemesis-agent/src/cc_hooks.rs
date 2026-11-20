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
//! 作备份）。网关主 agent 工厂（`agent_factory.rs`）启动即加载并注册
//! （[`CcHookBridge::load_from_dir`] **恒建桥**——缺失/零条目/坏文件也建
//! 桥待命，首个钩子落盘即被热更闸捡到，即刻生效无需重启）；**集群 agent
//! 不挂**（远端节点跑的是本地用户的任务，hook 拦截语义不该跨节点复制——
//! 挂账决策，报告注明）。解析失败 = warn + 空快照待命（fail-open，绝不
//! 拖死 gateway）。
//!
//! # 统一存储（2026-11-19 重构）
//!
//! **单存储双类型**：全部钩子（全局 + 会话圈定 + command/prompt 两型）住
//! 同一个 hooks.json，`type` 字段分派执行体，`scope.sessions` 圈定会话。
//! 旧 hooks.session.json 退役（[`retire_session_store`] 启动清理，零数据
//! 迁移）；agent `hooks` 工具经本文件 RMW API（[`add_session_hook`] 等）
//! 写同一文件，与 Dashboard hooks.set 共享 [`lock_hooks_file`] 互斥。
//! 会话钩子 = 工具专属组（scope == `[sid]`、id `h1..hn`），与用户手写组
//! 同表共存、同一条 matches_session 过滤路径。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hooks::{
    HookDecision, HookPrompt, HookToolCall, LifecycleHook, PostHookAction, PromptDecision,
    ToolHook, TurnEndDecision,
};

/// hooks.json 文件名（相对 config 目录）。
pub const HOOKS_FILE: &str = "hooks.json";

/// 旧会话级钩子存储文件名（**已退役**，2026-11-19 统一存储重构）：会话钩子
/// 现住 hooks.json 的 `scope` 圈定组（单存储双类型，不再分文件）。本常量仅
/// 供 [`retire_session_store`] 启动清理引用（在场即删 + warn，零数据迁移）。
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

/// 一条 hook 条目。方言 schema **二型**（`type` 分派，2026-11-19 统一存储）：
/// - command 型：`{"type":"command","command":"...","timeout":60}`（shell
///   执行体，全部 12 事件合法）
/// - prompt 型：`{"type":"prompt","prompt":"..."}`（LLM 执行体，**仅 Stop
///   事件**——turn 收尾后注入提示词开新轮，永续循环语义，见
///   `hooks.rs::TurnEndDecision::Inject`；合法事件单一真相源
///   [`PROMPT_HOOK_EVENTS`]）
///
/// `command`/`prompt` 按 type 恰好一项非空（parse 校验强制：双空无执行体、
/// 双非空混合条目语义未定，均 loud 拒绝）。`id`：agent `hooks` 工具分配的
/// 条目标识（`h1..hn`），用户手写条目缺省。序列化跳过空值——原子写产物
/// 字节确定，diff/测试可断言。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CcCommand {
    /// `command` / `prompt`；空 = 按内容回填（[`parse_cc_hooks`] 归一）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    r#type: String,
    /// shell 型条目的命令；prompt 型条目为空。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    command: String,
    /// prompt 型条目的提示词文本；command 型条目为空。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    prompt: String,
    /// 会话工具（`hooks` 工具）分配的条目 id；用户手写条目缺省。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    id: String,
    /// 秒。缺省 [`DEFAULT_TIMEOUT_SECS`]（仅 command 型消费）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
}

impl CcCommand {
    /// type 归一 + 二型校验（[`parse_cc_hooks`] 唯一调用方）。空 type 按内容
    /// 回填（手写省略 type 的兼容）；其余非法形态 loud Err：
    /// - 未知 type（只认 `command`/`prompt`）
    /// - 双空执行体（无 shell 也无提示词 = 无语义）
    /// - 双非空（混合条目语义未定——shell 路径不读 prompt、prompt 路径不读
    ///   command，写双值必有一侧被静默忽略）
    /// - prompt 出现在非 Stop 事件（prompt 执行体只挂 turn 收尾钩点）
    fn normalize_kind(&mut self, event: &str) -> Result<(), String> {
        if self.r#type.is_empty() {
            let has_command = !self.command.trim().is_empty();
            let has_prompt = !self.prompt.trim().is_empty();
            if has_command == has_prompt {
                // 双空无法回填 / 双非空无法推断——统一报形态错。
                return Err(format!(
                    "hook needs exactly one of command/prompt (event {event})"
                ));
            }
            self.r#type = if has_prompt { "prompt" } else { "command" }.to_string();
        }
        let command_empty = self.command.trim().is_empty();
        let prompt_empty = self.prompt.trim().is_empty();
        match self.r#type.as_str() {
            "command" => {
                if !prompt_empty {
                    return Err(format!(
                        "hook type=command must not carry prompt (event {event})"
                    ));
                }
                if command_empty {
                    return Err(format!(
                        "hook type=command needs non-empty command (event {event})"
                    ));
                }
            }
            "prompt" => {
                if !command_empty {
                    return Err(format!(
                        "hook type=prompt must not carry command (event {event})"
                    ));
                }
                if prompt_empty {
                    return Err(format!(
                        "hook type=prompt needs non-empty prompt (event {event})"
                    ));
                }
                if !PROMPT_HOOK_EVENTS.contains(&event) {
                    return Err(format!(
                        "prompt hooks are only valid on {} (event {event})",
                        PROMPT_HOOK_EVENTS.join("/")
                    ));
                }
            }
            other => {
                return Err(format!(
                    "unsupported hook type {other:?} (event {event}) — expected \"command\" or \"prompt\""
                ));
            }
        }
        Ok(())
    }
}

/// 会话圈定（HOOK 会话维度，2026-11-16）。匹配语义 = **精确相等**（不搞
/// 正则/通配——会话 id 是稳定身份）。`sessions` 条目按剥
/// [`WEB_SESSION_KEY_PREFIX`] 的裸 sid 落盘；解析时归一去空白、空条目
/// loud 拒绝（见 [`parse_cc_hooks`]）。空列表 = 全部会话（缺省兼容）。
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CcHookGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    matcher: Option<String>,
    /// 会话圈定。`None` / 空 `sessions` = 全部会话（缺省兼容，老配置行为
    /// 零变化）；非空 = 只对列出的会话生效（[`Self::matches_session`]）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<CcScope>,
    #[serde(default)]
    hooks: Vec<CcCommand>,
}

impl CcHookGroup {
    /// 是否为会话圈定组（scope 在场且 sessions 非空）。统一存储后的计数
    /// 分账闸：圈定组归 hooks_session 类目（[`count_session_hooks`]），未
    /// 圈定组归 hooks_global 类目（[`CcEvents::global_hook_counts`]）——
    /// 一张表里两族并存，不按此分账就会双计。
    fn is_session_scoped(&self) -> bool {
        self.scope.as_ref().is_some_and(|s| !s.sessions.is_empty())
    }

    /// 是否 agent `hooks` 工具专属组（scope.sessions == 恰好 `[sid]`）。
    /// 工具 add 恒建新组、delete/list 只认这种形态——用户手写组（多会话
    /// 圈定或带 matcher 语义）永不被工具改写（动它 = 用户配置被工具篡改）。
    fn tool_owned_by(&self, sid: &str) -> bool {
        self.scope
            .as_ref()
            .is_some_and(|s| s.sessions.len() == 1 && s.sessions[0] == sid)
    }

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

/// 十二个方言事件各自的分组列表。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CcEvents {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pre_tool_use: Vec<CcHookGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    post_tool_use: Vec<CcHookGroup>,
    /// 工具执行失败后（三段化扩展）。观察型：stderr 只记日志。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    post_tool_use_failure: Vec<CcHookGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    session_start: Vec<CcHookGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    user_prompt_submit: Vec<CcHookGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stop: Vec<CcHookGroup>,
    /// 会话结束（TTL 过期清理 / 显式删除）。观察型（会话已结束，无阻断）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    session_end: Vec<CcHookGroup>,
    /// 压缩前后（扩展）。观察型：exit 2 不阻止压缩（稳定性机制）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pre_compact: Vec<CcHookGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    post_compact: Vec<CcHookGroup>,
    /// 子代理开始。exit 2 / JSON block = 拒绝
    /// spawn。仅挂本地 spawn 闭包（内部 LLM 委托不发，见模块头表）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subagent_start: Vec<CcHookGroup>,
    /// 子代理结束。观察型：任务已跑完，exit 2
    /// 无阻断语义。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subagent_stop: Vec<CcHookGroup>,
    /// 审批/提问观察通知。纯观察型：边界处
    /// fire-and-forget，exit 2 无任何语义。层：loop 提问端（问题流）/
    /// auditor 审批 manager（security 三入口 + 管道 RequireApproval）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notification: Vec<CcHookGroup>,
}

impl CcEvents {
    /// 全部 12 个事件组的统一遍历（共享引用；`（事件名, 组列表）` 定序数组，
    /// 顺序 = [`SESSION_HOOK_EVENTS`]）。解析归一 / matcher 体检 / 计数全走
    /// 这里——**新增事件先加字段再在这里加一行**，遍历清单不可能再漏组
    /// （旧实现 parse 过滤与 matcher 体检各自手写 12 项清单，漏组即契约
    /// 失效，那是两组清单永不同步的结构性风险）。
    fn groups(&self) -> [(&'static str, &Vec<CcHookGroup>); 12] {
        [
            ("PreToolUse", &self.pre_tool_use),
            ("PostToolUse", &self.post_tool_use),
            ("PostToolUseFailure", &self.post_tool_use_failure),
            ("SessionStart", &self.session_start),
            ("UserPromptSubmit", &self.user_prompt_submit),
            ("Stop", &self.stop),
            ("SessionEnd", &self.session_end),
            ("PreCompact", &self.pre_compact),
            ("PostCompact", &self.post_compact),
            ("SubagentStart", &self.subagent_start),
            ("SubagentStop", &self.subagent_stop),
            ("Notification", &self.notification),
        ]
    }

    /// [`Self::groups`] 的可变版（RMW 写路径用；借用不相交，数组字面量合法）。
    fn groups_mut(&mut self) -> [(&'static str, &mut Vec<CcHookGroup>); 12] {
        [
            ("PreToolUse", &mut self.pre_tool_use),
            ("PostToolUse", &mut self.post_tool_use),
            ("PostToolUseFailure", &mut self.post_tool_use_failure),
            ("SessionStart", &mut self.session_start),
            ("UserPromptSubmit", &mut self.user_prompt_submit),
            ("Stop", &mut self.stop),
            ("SessionEnd", &mut self.session_end),
            ("PreCompact", &mut self.pre_compact),
            ("PostCompact", &mut self.post_compact),
            ("SubagentStart", &mut self.subagent_start),
            ("SubagentStop", &mut self.subagent_stop),
            ("Notification", &mut self.notification),
        ]
    }

    fn total_scripts(&self) -> usize {
        self.groups()
            .iter()
            .map(|(_, v)| v.iter().map(|g| g.hooks.len()).sum::<usize>())
            .sum()
    }

    /// 仅测试面使用（断言「空快照/待命桥」形态）——生产代码不读它（恒建桥
    /// 后生产空态走 events 内部惰性热更，无需查询）。cfg(test) 防 lib 视角
    /// dead_code 告警；crate 内 cc_hooks/tests.rs + cov_tests.rs 在用。
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.total_scripts() == 0
    }

    /// Per-event script counts（hooks.json 的 PascalCase 事件名）。诊断/用
    /// UI 用（Hooks Tab 的 summary）——字段私有，外部 crate 走这里。
    /// 顺序：PreToolUse, PostToolUse, PostToolUseFailure, SessionStart,
    /// UserPromptSubmit, Stop, SessionEnd, PreCompact, PostCompact,
    /// SubagentStart, SubagentStop, Notification。
    pub fn script_counts(&self) -> [(&'static str, usize); 12] {
        self.groups()
            .map(|(name, groups)| (name, groups.iter().map(|g| g.hooks.len()).sum::<usize>()))
    }

    /// 未圈定（全局生效）条目的逐事件计数（资源看板 hooks_global 类目）。
    /// **只数未圈定组**——会话圈定组归 hooks_session 类目（[`count_session_hooks`]），
    /// 这里再数就双计（统一存储里 pinned + global 并存一张表）。顺序同
    /// [`Self::script_counts`]；0 计数事件不在返回里（看板只展示命中的）。
    pub fn global_hook_counts(&self) -> Vec<(&'static str, usize)> {
        self.groups()
            .map(|(name, groups)| {
                (
                    name,
                    groups
                        .iter()
                        .filter(|g| !g.is_session_scoped())
                        .map(|g| g.hooks.len())
                        .sum::<usize>(),
                )
            })
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .collect()
    }
}

/// 解析 hooks.json 内容。两种形态都收：标准 `{"hooks":{...}}` 与裸顶层
/// `{"PreToolUse":[...]}`（手写文件常见省略外层）。
///
/// 条目二型（`type` 分派）：`command`（shell）/ `prompt`（LLM，仅 Stop）。
/// 归一：空 type 按内容回填（手写省略 type 的老配置零迁移）；其余非法形态
/// （未知 type / 双空执行体 / 双非空 / prompt 出现在非 Stop 事件 /
/// scope.sessions 空条目）一律 **Err**——手写坏配置 loud 拒绝（fail-closed），
/// 由调用方分诊（Dashboard hooks.set 拒写、桥侧 warn 保旧），绝不静默丢
/// 条目（旧实现非 command 条目 retain-丢弃 + skipped 计数，写错 type 的
/// 条目消失得无声无息）。
pub fn parse_cc_hooks(json: &str) -> Result<CcEvents, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let events_value = match root.get("hooks") {
        Some(inner) => inner.clone(),
        None => root,
    };
    // 形状闸（BUG-20261120-029）：CcEvents 全字段带 default，`[]` 走 serde
    // visit_seq 零元素臂 = 「全空 hooks」的合法值——整份 hooks 配置被静默
    // 清空还 Ok，违背本模块「手写坏配置 loud 拒绝（fail-closed）」纪律。
    // 两种合法形态（标准 `{"hooks":{...}}` 与裸顶层方言）的事件体都是
    // object，反序列化前 loud 拒绝非 object。语义单一真相源
    // = nemesis_utils::json_shape（BUG-20261120-030 家族收口）。
    if let Err(e) = nemesis_utils::json_shape::ensure_object(&events_value, "hooks") {
        return Err(format!("unsupported hooks.json format: {e}"));
    }
    let mut events: CcEvents = serde_json::from_value(events_value)
        .map_err(|e| format!("unsupported hooks.json format: {e}"))?;
    for (event, groups) in events.groups_mut() {
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
            for h in g.hooks.iter_mut() {
                h.normalize_kind(event)?;
            }
        }
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
/// 返回无效条数。清单走 [`CcEvents::groups`]（12 组统一遍历，不可能漏组）。
fn warn_invalid_matchers(events: &CcEvents) -> usize {
    let mut invalid = 0usize;
    for (_, groups) in events.groups() {
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
// 统一存储常量 + agent `hooks` 工具的 RMW API（hooks.json 单一真相源）
// ---------------------------------------------------------------------------

/// 会话工具合法事件名（12 事件全量；单一真相源——`hooks` 工具 add 校验、
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

/// 提示词型（prompt）钩子合法事件名单一真相源。今天只有 `Stop`——turn 收尾
/// 钩点每轮触发 → 注入提示词开新轮 → 自然循环。扩事件 = 在这里加名字，
/// parse 校验（[`CcCommand::normalize_kind`]）/ `hooks` 工具 add 校验 /
/// HookView 前端全部同源跟进。
pub const PROMPT_HOOK_EVENTS: [&str; 1] = ["Stop"];

/// hooks.json 读-改-写的进程内互斥（gateway 内 Dashboard hooks.set 与 agent
/// `hooks` 工具并发写同一文件；跨进程不设防——编辑器手编是用户自己的事，
/// 原子写兜底）。`std::sync::Mutex` **不可重入**：持锁期间禁止再调任何
/// 内部加锁的 API（`_locked` 后缀 = 调用方已持锁的约定）。
static HOOKS_FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 获取 hooks.json RMW 互斥（Dashboard hooks.set 写前调用；本文件 RMW API
/// 内部自持，外部勿重复加锁——不可重入）。毒锁恢复 = into_inner：互斥保护
/// 的是 RMW 完整性，单次 panic 不值得报废整个闸门。
pub fn lock_hooks_file() -> std::sync::MutexGuard<'static, ()> {
    HOOKS_FILE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// 读 hooks.json 为 [`CcEvents`]（RMW 内部件）。缺失 = 空表（正常态，可
/// 起步）；解析失败 = **Err**——RMW 写回前必须拒绝（绝不拿空表覆盖用户的
/// 坏文件：「读失败就写空」会把用户手编到一半的配置静默清空）；非缺失的
/// 读失败（瞬时锁等）同 Err。
fn read_hooks_events(config_dir: &Path) -> Result<CcEvents, String> {
    let path = config_dir.join(HOOKS_FILE);
    let text = match nemesis_utils::fsutil::read_config_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(CcEvents::default()),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    parse_cc_hooks(&text)
}

/// 原子写 hooks.json（`{"hooks":{...}}` 外层 + 事件键 PascalCase + 空组
/// 跳过，产物字节确定；0o600——hook 命令行/提示词含用户工作流细节）。
fn write_hooks_events(config_dir: &Path, events: &CcEvents) -> Result<(), String> {
    let path = config_dir.join(HOOKS_FILE);
    let text = serde_json::to_string_pretty(&serde_json::json!({ "hooks": events }))
        .map_err(|e| e.to_string())?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), text.as_bytes(), 0o600)
        .map_err(|e| e.to_string())
}

/// 会话工具侧的条目视图（list/delete 返回形态）。id/matcher/command/prompt/
/// timeout 全量拷贝——agent 读回展示用，不暴露存储结构。
#[derive(Debug, Clone)]
pub struct HookEntryView {
    pub event: &'static str,
    pub id: String,
    pub matcher: String,
    pub command: String,
    pub prompt: String,
    pub timeout: Option<u64>,
}

/// agent `hooks` 工具 add：给会话 `sid` 在 `event` 下追加一条钩子（type 按
/// command/prompt 恰好一项非空自动分派；matcher 空 = 全部工具）。条目落进
/// **新建工具专属组**（scope.sessions == `[sid]`；永不并进/改写用户手写组），
/// id 在本会话工具专属组内 `h{n}` 递增分配（扫描已有最大值 +1，跨事件连续）。
/// 返回分配的 id。调用方**不要**预先加锁（内部自持，不可重入）。
pub fn add_session_hook(
    config_dir: &Path,
    sid: &str,
    event: &str,
    matcher: &str,
    command: &str,
    prompt: &str,
    timeout: Option<u64>,
) -> Result<String, String> {
    let _guard = lock_hooks_file();
    if !SESSION_HOOK_EVENTS.contains(&event) {
        return Err(format!("unknown hook event {event}"));
    }
    let has_command = !command.trim().is_empty();
    let has_prompt = !prompt.trim().is_empty();
    if has_command == has_prompt {
        return Err("hook needs exactly one of command/prompt".to_string());
    }
    if has_prompt && !PROMPT_HOOK_EVENTS.contains(&event) {
        return Err(format!(
            "prompt hooks are only valid on {} (event {event})",
            PROMPT_HOOK_EVENTS.join("/")
        ));
    }
    let mut events = read_hooks_events(config_dir)?;
    // id 分配：扫描本会话工具专属组已有的 hN，取 max+1。
    let mut max = 0usize;
    for (_, groups) in events.groups_mut() {
        for g in groups.iter_mut().filter(|g| g.tool_owned_by(sid)) {
            for h in g.hooks.iter().filter(|h| !h.id.is_empty()) {
                if let Some(n) = h.id.strip_prefix('h').and_then(|n| n.parse::<usize>().ok()) {
                    max = max.max(n);
                }
            }
        }
    }
    let id = format!("h{}", max + 1);
    let matcher_trim = matcher.trim();
    let group = CcHookGroup {
        matcher: if matcher_trim.is_empty() {
            None
        } else {
            Some(matcher_trim.to_string())
        },
        // 工具专属组：scope 钉死单会话——与全局钩子走同一条
        // matches_session 过滤路径，零特殊分支。
        scope: Some(CcScope {
            sessions: vec![sid.to_string()],
        }),
        hooks: vec![CcCommand {
            r#type: if has_prompt { "prompt" } else { "command" }.to_string(),
            command: command.trim().to_string(),
            prompt: prompt.trim().to_string(),
            id: id.clone(),
            timeout: timeout.filter(|t| *t > 0),
        }],
    };
    let mut slots = events.groups_mut();
    let slot = slots
        .iter_mut()
        .find(|(name, _)| *name == event)
        .map(|(_, groups)| groups)
        .ok_or_else(|| format!("unknown hook event {event}"))?;
    slot.push(group);
    write_hooks_events(config_dir, &events)?;
    tracing::info!(
        "[cc-hooks] session hook added: sid={sid} event={event} id={id} type={}",
        if has_prompt { "prompt" } else { "command" }
    );
    Ok(id)
}

/// agent `hooks` 工具 delete：按 id 摘除该会话工具专属组里的条目。返回被
/// 摘条目视图（工具回显用）。id 不存在/不属本会话 = Err 列出现有 id 清单；
/// 条目删空后组整组摘除（不留 matcher/scope 空壳）。调用方**不要**预先
/// 加锁（内部自持，不可重入）。
pub fn delete_session_hook(
    config_dir: &Path,
    sid: &str,
    id: &str,
) -> Result<HookEntryView, String> {
    let _guard = lock_hooks_file();
    let mut events = read_hooks_events(config_dir)?;
    let mut found = None;
    for (event, groups) in events.groups_mut() {
        for g in groups.iter_mut() {
            if !g.tool_owned_by(sid) {
                continue;
            }
            if let Some(pos) = g.hooks.iter().position(|h| h.id == id) {
                let h = g.hooks.remove(pos);
                found = Some(HookEntryView {
                    event,
                    id: h.id,
                    matcher: g.matcher.clone().unwrap_or_default(),
                    command: h.command,
                    prompt: h.prompt,
                    timeout: h.timeout,
                });
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let Some(view) = found else {
        let ids: Vec<String> = events
            .groups()
            .iter()
            .flat_map(|(_, gs)| gs.iter())
            .filter(|g| g.tool_owned_by(sid))
            .flat_map(|g| g.hooks.iter().map(|h| h.id.clone()))
            .collect();
        return Err(if ids.is_empty() {
            format!("session {sid} has no tool-owned hooks")
        } else {
            format!(
                "hook id {id} not found; session {sid} has: {}",
                ids.join(", ")
            )
        });
    };
    // 摘掉删空的组（全表清 no-op 空壳——空 hooks 列表无语义，删了不伤人）。
    for (_, groups) in events.groups_mut() {
        groups.retain(|g| !g.hooks.is_empty());
    }
    write_hooks_events(config_dir, &events)?;
    tracing::info!(
        "[cc-hooks] session hook deleted: sid={sid} id={} event={}",
        view.id,
        view.event
    );
    Ok(view)
}

/// agent `hooks` 工具 list：该会话工具专属组的全部条目（数组序 =
/// [`SESSION_HOOK_EVENTS`] 定序 = 存储序，组内按存储序）。不含用户手写组
/// （用户全量视图在 Dashboard）。
pub fn list_session_hooks(config_dir: &Path, sid: &str) -> Result<Vec<HookEntryView>, String> {
    let _guard = lock_hooks_file();
    let events = read_hooks_events(config_dir)?;
    let mut out = Vec::new();
    for (event, groups) in events.groups() {
        if !SESSION_HOOK_EVENTS.contains(&event) {
            continue;
        }
        for g in groups.iter().filter(|g| g.tool_owned_by(sid)) {
            for h in &g.hooks {
                out.push(HookEntryView {
                    event,
                    id: h.id.clone(),
                    matcher: g.matcher.clone().unwrap_or_default(),
                    command: h.command.clone(),
                    prompt: h.prompt.clone(),
                    timeout: h.timeout,
                });
            }
        }
    }
    Ok(out)
}

/// 该会话在统一存储中被圈定的钩子条目数（资源看板 hooks_session 类目口径）：
/// **scope 圈定组**（[`CcHookGroup::is_session_scoped`]）里圈定命中本会话的
/// 全部条目（含用户手写圈定组；未圈定组不在此账——归 hooks_global 类目，
/// 两处分账不双计）。入参可传裸 sid 或完整 session_key（matches_session
/// 双串语义同源）。调用方**不要**预先加锁。
pub fn count_session_hooks(config_dir: &Path, session_key: &str) -> Result<usize, String> {
    let _guard = lock_hooks_file();
    let events = read_hooks_events(config_dir)?;
    Ok(events
        .groups()
        .iter()
        .flat_map(|(_, gs)| gs.iter())
        .filter(|g| g.is_session_scoped() && g.matches_session(session_key))
        .map(|g| g.hooks.len())
        .sum())
}

/// 该会话被圈定钩子的**明细视图**（资源看板 hooks_session 类目行数据，
/// 2026-11-19「只有个数不知道是啥」补齐）：与 [`count_session_hooks`]
/// 同一谓词（`is_session_scoped` && `matches_session`——count/list 永不
/// 分裂），逐条展开 [`HookEntryView`]（事件 · 命令/提示词原文 · matcher ·
/// timeout）。入参可传裸 sid 或完整 session_key（双串语义同源）。调用方
/// **不要**预先加锁。
pub fn list_session_hooks_scoped(
    config_dir: &Path,
    session_key: &str,
) -> Result<Vec<HookEntryView>, String> {
    let _guard = lock_hooks_file();
    let events = read_hooks_events(config_dir)?;
    let mut out = Vec::new();
    for (event, groups) in events.groups() {
        for g in groups
            .iter()
            .filter(|g| g.is_session_scoped() && g.matches_session(session_key))
        {
            for h in &g.hooks {
                out.push(HookEntryView {
                    event,
                    id: h.id.clone(),
                    matcher: g.matcher.clone().unwrap_or_default(),
                    command: h.command.clone(),
                    prompt: h.prompt.clone(),
                    timeout: h.timeout,
                });
            }
        }
    }
    Ok(out)
}

/// 从 hooks.json 摘除给定会话集合的**圈定**（`_locked` = 调用方已持
/// [`HOOKS_FILE_LOCK`]，内部不再加锁——std Mutex 不可重入，重复加锁 =
/// 死锁）。摘除语义：只从 scope.sessions 里删死会话（scope 剩别的会话的
/// 组保留）；**本 round 被摘空的组整组删除**——绝不能留空 scope 组（空 =
/// 全部会话命中，钉死的钩子会瞬间变成全局钩子，语义反转事故）；未触碰
/// 的组（含用户手写的空 sessions 全局组）原样保留。返回改写组数（无改动
/// = 0 = 零写入）。
fn remove_session_scopes_locked(
    config_dir: &Path,
    dead_sids: &HashSet<String>,
) -> Result<usize, String> {
    if dead_sids.is_empty() {
        return Ok(0);
    }
    let mut events = read_hooks_events(config_dir)?;
    let mut changed = 0usize;
    for (_, groups) in events.groups_mut() {
        let mut emptied = Vec::new();
        for (gi, g) in groups.iter_mut().enumerate() {
            let Some(scope) = g.scope.as_mut() else {
                continue;
            };
            let before = scope.sessions.len();
            scope.sessions.retain(|s| !dead_sids.contains(s));
            if scope.sessions.len() != before {
                changed += 1;
                if scope.sessions.is_empty() {
                    emptied.push(gi);
                }
            }
        }
        for gi in emptied.into_iter().rev() {
            groups.remove(gi);
        }
    }
    if changed == 0 {
        return Ok(0);
    }
    write_hooks_events(config_dir, &events)?;
    Ok(changed)
}

/// 孤儿对账（gateway 启动一次）：hooks.json 里 scope 圈定了不在 `live_sids`
/// 的会话 → 该会话从 scope 摘除（圈定组随会话消亡；scope 摘空 → 整组删，
/// 见 [`remove_session_scopes_locked`]）。未圈定组不动（全局配置不随会话
/// 生死）。返回改写组数。幂等；写失败 warn 不 panic（下次启动重试）。
pub fn reconcile_session_orphans(config_dir: &Path, live_sids: &HashSet<String>) -> usize {
    let _guard = lock_hooks_file();
    let events = match read_hooks_events(config_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("[cc-hooks] reconcile skipped: {e}");
            return 0;
        }
    };
    let scoped: HashSet<String> = events
        .groups()
        .iter()
        .flat_map(|(_, gs)| gs.iter())
        .filter_map(|g| g.scope.as_ref())
        .flat_map(|s| s.sessions.iter().cloned())
        .collect();
    let dead: HashSet<String> = scoped
        .into_iter()
        .filter(|sid| !live_sids.contains(sid))
        .collect();
    match remove_session_scopes_locked(config_dir, &dead) {
        Ok(n) => {
            if n > 0 {
                tracing::info!("[cc-hooks] reconciled {n} group(s) scoped to ended sessions");
            }
            n
        }
        Err(e) => {
            tracing::warn!("[cc-hooks] reconcile write failed (retry next start): {e}");
            0
        }
    }
}

/// hooks.session.json 退役清理（gateway 启动一次，先于对账）：该文件自统一
/// 存储重构（2026-11-19）后不再是真相源——会话钩子现住 hooks.json 的
/// scope 圈定组。在场即删 + warn；**零数据迁移**（不读内容重建 scope 组：
/// 语义不等价的静默转换比诚实提示更危险，存量使用者极少、手迁成本几行
/// JSON）。删除失败 warn 不阻塞启动。
pub fn retire_session_store(config_dir: &Path) {
    let path = config_dir.join(HOOKS_SESSION_FILE);
    match std::fs::remove_file(&path) {
        Ok(_) => tracing::warn!(
            "[cc-hooks] retired legacy {} — session hooks now live in {} (scope 圈定组); \
             本文件内容未自动迁移",
            path.display(),
            HOOKS_FILE
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!("[cc-hooks] failed to remove legacy {}: {e}", path.display())
        }
    }
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
/// [`Self::load_from_dir`]，**恒建桥**——启动时文件缺失/空/坏也建桥待命）
/// 在每次分发入口惰性检查 hooks.json 的内容，
/// 变化即重新解析并原子换装——保存即下条消息生效，无需重启 Agent。语义：
/// 解析失败保留旧配置（fail-open 保旧，坏文件不炸会话）；文件清空/消失 =
/// 换装为空 = 事实注销（与启动态「没有 hooks.json = 没配」同一语义）。纯
/// 内存桥（[`Self::from_json`]，测试/手工构造）无热更。
pub struct CcHookBridge {
    /// 当前生效配置（热更换装点）。分发入口取 clone 快照再跑脚本——锁不跨
    /// await（CcEvents 为 Vec 簇小结构，clone 便宜）。全局组与会话圈定组
    /// 同住一张表（统一存储），圈定过滤在 run_group 统一走
    /// [`CcHookGroup::matches_session`]。
    events: RwLock<CcEvents>,
    /// hooks.json 盘上路径；`None` = 纯内存桥，热更关闭。
    hooks_path: Option<PathBuf>,
    /// 上次记账的盘上原始文本（read_config_string 同源剥 BOM；None = 缺失）。
    /// 与文件现状比较决定是否重载；**解析失败也记录**——同一版坏文件只 warn
    /// 一次，用户改好文件（内容再变）后自然重试。内容比对不赌 mtime
    /// （同族收编：Windows 毫秒级粒度下同刻改写漏检 =
    /// 换装静默滞留旧版）。
    seen_raw: Mutex<Option<String>>,
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
            project_dir,
            seen_sessions: Mutex::new(HashSet::new()),
            stop_blocks: Mutex::new(HashMap::new()),
        })
    }

    /// 从 config 目录加载。**恒建桥**（2026-11-20 即刻生效语义；原「缺/空/坏
    /// → None = 无热更载体，重启建桥」语义退役——hook 点是真实钩点，设置
    /// 必须即刻生效或失效，不该要求重启）：hooks.json 缺失/零条目/解析失败
    /// 也建桥待命——空快照起步 + [`Self::hooks_path`] 记账，内容完全交由
    /// 分发入口 [`Self::current_events`] 惰性热更（每次分发读文件比对）。
    /// 此后任何通道的首个钩子（Dashboard `hooks.set` / agent `hooks` 工具 /
    /// 手工编辑）都在下次分发即刻生效，不存在「重启建桥」中间态；文件清空/
    /// 消失同样即刻失效（换装空快照）。
    ///
    /// 日志口径：缺失 = 静默（绝大多数部署的常态，不打扰）；可解析（含零
    /// 条目，info 文案带 `0 script(s)`）= info 一条；解析失败 = warn + 空快照
    /// 待命（fail-open 不拖死 gateway；坏文已记 [`Self::seen_raw`]，同版只
    /// warn 一次，用户改好后下次分发自动激活）。
    ///
    /// 代价注记：桥恒在 = 每次工具/生命周期分发多一次 hooks.json 小文件读
    /// （内容比对热更闸）；与「有钩子部署」同价，换即刻生效语义。
    pub fn load_from_dir(config_dir: &Path, project_dir: PathBuf) -> Arc<Self> {
        let path = config_dir.join(HOOKS_FILE);
        let Some(text) = nemesis_utils::fsutil::read_config_string(&path).ok() else {
            // 缺失：空快照待命（seen_raw=None；文件首次落盘即被内容比对捡到）。
            return Arc::new(Self::armed(project_dir, path, None));
        };
        let events = match parse_cc_hooks(&text) {
            Ok(events) => {
                tracing::info!(
                    "[cc-hooks] loaded {} script(s) from {} — {:?}",
                    events.total_scripts(),
                    path.display(),
                    events.script_counts()
                );
                events
            }
            Err(_) => {
                tracing::warn!(
                    "[cc-hooks] failed to parse {} — hooks DISABLED until the file parses (fail-open)",
                    path.display()
                );
                // 解析失败：空快照待命，seen_raw 记坏文（同版只 warn 一次；
                // 用户改好后内容再变，下次分发自动激活）。
                return Arc::new(Self::armed(project_dir, path, Some(text)));
            }
        };
        Arc::new(Self {
            events: RwLock::new(events),
            hooks_path: Some(path),
            seen_raw: Mutex::new(Some(text)),
            project_dir,
            seen_sessions: Mutex::new(HashSet::new()),
            stop_blocks: Mutex::new(HashMap::new()),
        })
    }

    /// 冷启动/坏文件待命桥：空快照 + hooks_path 记账——热更入口
    /// （[`Self::current_events`]）从这份记账出发捡到后续落盘的任何内容。
    /// `seen` 语义与启动时文件形态对齐：缺失 = `None`、坏文 = `Some(坏文)`
    /// （同版坏文件不再重复 warn）。
    fn armed(project_dir: PathBuf, path: PathBuf, seen: Option<String>) -> Self {
        Self {
            events: RwLock::new(CcEvents::default()),
            hooks_path: Some(path),
            seen_raw: Mutex::new(seen),
            project_dir,
            seen_sessions: Mutex::new(HashSet::new()),
            stop_blocks: Mutex::new(HashMap::new()),
        }
    }

    /// 挂到 AgentLoop（工具钩子 + 生命周期钩子一起）。
    pub fn register(self: Arc<Self>, agent_loop: &crate::r#loop::AgentLoop) {
        agent_loop.add_tool_hook(self.clone());
        agent_loop.add_lifecycle_hook(self);
    }

    /// **分发入口**：hooks.json 内容变化则热更换装，返回当前生效配置的
    /// clone 快照（锁不跨 await）。纯内存桥直接返回快照。会话圈定组与全局
    /// 组同住一张表（统一存储）——快照全量携带，[`CcHookGroup::matches_session`]
    /// 在 run_group 里过滤（零特殊分支）。
    ///
    /// 换装语义：
    /// - 内容变化 + 解析成功 → 原子换装（info 记脚本计数）。
    /// - 内容变化 + 解析失败 → warn + **保留旧配置**；文本已记账，同版
    ///   坏文件只 warn 一次。
    /// - 文件消失（NotFound）→ 视同用户清空：换装为空 = 事实注销，与
    ///   启动态「没有 hooks.json = 没配」同一语义。
    /// - 瞬时读失败（编辑器/杀软锁，非 NotFound）→ 保留现状不记账，下轮
    ///   重试（与「消失」分立——读不到 ≠ 不存在）。
    fn current_events(&self) -> CcEvents {
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
                // prompt 型条目不进 shell 执行路径（LLM 执行体；Stop 事件的
                // prompt 条目由 on_turn_end 从同一快照单独收集注入）。
                if !h.prompt.trim().is_empty() {
                    continue;
                }
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

    /// 全局（未圈定）钩子的逐事件计数（资源看板 hooks_global 类目）。锁内
    /// 取快照后委托 [`CcEvents::global_hook_counts`]（**只数未圈定组**——
    /// 会话圈定组归 hooks_session 类目，两处分账不双计）。走 current_events
    /// （懒重载：盘上内容变化即热更）而非直读缓存——盘点面与 dispatch 面同
    /// 一新鲜度口径。
    pub fn global_hook_counts(&self) -> Vec<(&'static str, usize)> {
        self.current_events().global_hook_counts()
    }

    /// 方言 `SubagentStart`：spawn 闭包内、
    /// `run_detached` 前调。exit 2 / JSON block → **Err（拒绝 spawn）**——
    /// 阻断文案即拒绝原因，由调用方作为 SpawnFn 的 Err 上抛。无子代理钩子
    /// 配置时零开销直通。
    pub async fn dispatch_subagent_start(&self, info: &SubagentInfo) -> Result<(), String> {
        let events = self.current_events();
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
        let events = self.current_events();
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
        let events = self.current_events();
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
        let events = self.current_events();
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
        let events = self.current_events();
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
        let events = self.current_events();
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
        let events = self.current_events();
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
        let events = self.current_events();
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
        // 提示词型钩子（LLM 执行体）：shell 全放行后从**同一快照**收集 Stop
        // 事件下会话圈定命中的 prompt 条目（开头 current_events 已热更换装，
        // 新鲜度同源；matches_session 复用统一过滤路径）。多条 prompt 按存储
        // 序拼接为一条注入（一次注入一个消息；钩子内容全部到达）。空 = 无
        // prompt 钩子，照常 Stop。Inject 由 loop 在 turn 收尾后注入新用户
        // 消息开启下一轮——下一轮结束 Stop 钩点再次触发 → 自然循环（无封顶，
        // 永续是钩点正常执行的自然副作用；estop/取消打断 turn → 非 Accept
        // 路径 → 本钩子不触发 → 循环自然停）。
        if !end.session_key.is_empty() {
            let prompts: Vec<&str> = events
                .stop
                .iter()
                .filter(|g| g.matches_session(&end.session_key))
                .flat_map(|g| g.hooks.iter())
                .filter(|h| !h.prompt.trim().is_empty())
                .map(|h| h.prompt.as_str())
                .collect();
            if !prompts.is_empty() {
                let prompt = prompts.join("\n\n");
                tracing::info!(
                    "[cc-hooks] prompt hook fired for session '{}' ({} hook(s), {} bytes) — \
                     loop will inject after turn finalization",
                    end.session_key,
                    prompts.len(),
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
    /// 分发之后顺手做**会话钩子孤儿对账**：hooks.json 里圈定到该会话的
    /// 钩子随会话一起摘除（先分发后 purge——SessionEnd 钩子自身若被
    /// 圈定到本会话，仍要在死前响最后一次）。
    pub async fn on_session_end(&self, session_key: &str, reason: &str) {
        let events = self.current_events();
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

    /// 会话结束后的孤儿清理：从 hooks.json 摘除该会话的圈定（scope 圈定组
    /// 随会话消亡；scope 摘空 → 整组删，见 [`remove_session_scopes_locked`]；
    /// 未圈定组不动）。磁盘无该会话圈定 = 零写入；纯内存桥（无盘上路径）=
    /// 跳过（存储面未接线）。内存侧无需手工同步——下次分发 current_events
    /// 内容比对自然捡到 purge 刚写的盘上真相。
    fn purge_session_hooks(&self, session_key: &str) {
        if self.hooks_path.is_none() {
            return;
        }
        let sid = session_key
            .strip_prefix(WEB_SESSION_KEY_PREFIX)
            .unwrap_or(session_key);
        let config_dir = self
            .hooks_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let mut dead = HashSet::new();
        dead.insert(sid.to_string());
        // 分发（SessionEnd 钩子自身若圈定到本会话已在上面响过最后一次）之后
        // 摘圈定；锁在 remove_session_scopes_locked 内部自持。
        match remove_session_scopes_locked(&config_dir, &dead) {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                "[cc-hooks] purged session-scoped hooks for ended session {sid} ({n} group(s))"
            ),
            Err(e) => tracing::warn!("[cc-hooks] session hook purge write failed: {e}"),
        }
    }
}

impl CcHookBridge {
    /// 方言 `PreCompact` / `PostCompact`（观察型）：压缩流水线前后触发。
    /// exit 2 不阻止压缩（稳定性机制，诚实边界）。`session_key`：被压缩的
    /// 会话身份——会话圈定过滤锚点 + payload 的 session_id 字段（旧实现
    /// 占位 "system"，会话维度上线后改传真值，诚实标注来源会话）。
    pub async fn run_compact_hooks(&self, trigger: &str, phase: &str, session_key: &str) {
        let events = self.current_events();
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
