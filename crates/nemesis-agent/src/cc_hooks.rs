//! CC hooks.json 方言层（K2 — U14 第七批）。
//!
//! 把 Claude Code 格式的 `hooks.json` 桥到 K1a/K1b/K2 的钩子体系上：解析
//! CC 格式配置 → 按事件生成子进程脚本调用（协议对齐 CC：stdin JSON / env
//! / 退出码拦放行）→ 把退出码翻译回 [`crate::hooks`] 的决策类型。
//!
//! # 事件映射（goal 第七批 K2 表）
//!
//! | CC 事件 | 我们的钩点 | 方言语义 |
//! |---|---|---|
//! | `SessionStart` | [`LifecycleHook::on_user_prompt`]（桥内部：该 session 首条 prompt 时先跑） | 观察型（exit 2 不拦——CC 里 SessionStart 无阻断语义） |
//! | `UserPromptSubmit` | [`LifecycleHook::on_user_prompt`]（在消息进 history 之前） | exit 2 → 拦下 prompt（模型永远看不到）；exit 0 → 观察（stdout 只记日志，**不**注入上下文——诚实边界，见下） |
//! | `PreToolUse` | [`ToolHook::pre_tool_use`]（K1a，security 固定闸之后） | exit 2 / JSON `{"decision":"block"}` → Block（stderr 作 reason 回灌模型） |
//! | `PostToolUse` | [`ToolHook::post_tool_use`]（K1a，Forge 之前） | exit 2 → 把 stderr 以 `[hook]` 注记**追加**到结果（对齐 CC「反馈给 Claude」；不撤销已执行的操作） |
//! | `Stop` | [`LifecycleHook::on_turn_end`]（最终答案被接受后、Done 前） | exit 2 → Block stopping：stderr 作 feedback 注入为 user 消息、再答一轮（`MAX_TURN_END_CONTINUES` 封顶 fail-open）；`stop_hook_active` 标志随第二次起置 true |
//!
//! LLM 调用级（K1b）无对应 CC 事件——CC 没有 per-LLM-call hook，不造。
//!
//! # 脚本执行协议（对齐 CC）
//!
//! - **stdin**：单行紧凑 JSON。公共字段 `session_id`（=我们的 session_key）、
//!   `cwd`、`hook_event_name`、`transcript_path`（**空串**——我们无 CC 转写
//!   文件可指，诚实标注；真脚本极少依赖）。事件字段见 [`build_event_payload`]。
//! - **env**：`CLAUDE_PROJECT_DIR`=<workspace 根>（真 CC 脚本常读它）。
//! - **cwd**：workspace 根。
//! - **退出码**：`0` = 放行（stdout 记日志；PreToolUse/Stop 还会尝试解析
//!   stdout JSON `{"decision":"block","reason":...}`）；`2` = 拦停（stderr
//!   作 reason/feedback）；其他/超时/启动失败 = 非阻断错误（warn 日志、放
//!   行——CC 同款 fail-open）。
//! - **超时**：每脚本 `timeout` 秒（CC 默认 60）；到点 kill + 放行。
//!
//! # 工具名方言（真脚本能触发的前提）
//!
//! CC 脚本的 matcher 与 payload 用的是 CC 工具名（`Bash`/`Edit`/`Write`/
//! `Read`/`Grep`）。我们一侧：matcher 对 **CC 别名或原始名** 任一命中即触发；
//! stdin 的 `tool_name` 优先发 CC 别名；`tool_input` 在原 args 之上补
//! `file_path`/`content`/`command` 等 CC 字段名别名（真 lint-on-edit 脚本
//! 读 `jq .tool_input.file_path`，没这层别名永远拿不到值）。映射表见
//! [`cc_tool_alias`]。
//!
//! # 诚实边界（没做的）
//!
//! - UserPromptSubmit exit-0 stdout 的 additionalContext 注入（CC 会把它加
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
use std::sync::{Arc, Mutex};
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

/// CC 默认脚本超时（秒）。
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// 配置解析（CC 格式）
// ---------------------------------------------------------------------------

/// 一条 hook 命令。CC schema：`{"type":"command","command":"...","timeout":60}`。
#[derive(Debug, Clone, Deserialize)]
pub struct CcCommand {
    #[serde(default)]
    r#type: String,
    command: String,
    /// 秒。缺省 [`DEFAULT_TIMEOUT_SECS`]。
    #[serde(default)]
    timeout: Option<u64>,
}

/// 一个事件下的 matcher 分组：`{"matcher":"Edit|Write","hooks":[...]}`。
#[derive(Debug, Clone, Deserialize)]
pub struct CcHookGroup {
    #[serde(default)]
    matcher: Option<String>,
    #[serde(default)]
    hooks: Vec<CcCommand>,
}

impl CcHookGroup {
    /// matcher 是否命中该工具（CC 别名或原始名任一）。
    fn matches(&self, raw_tool: &str) -> bool {
        let Some(pattern) = &self.matcher else {
            return true; // 无 matcher = 全命中（CC 同款）
        };
        let candidates: Vec<&str> = match cc_tool_alias(raw_tool) {
            Some(alias) => vec![alias, raw_tool],
            None => vec![raw_tool],
        };
        match regex::Regex::new(pattern) {
            Ok(re) => candidates.iter().any(|c| re.is_match(c)),
            Err(_) => {
                // 无效正则（CC 里会让整个 hook 报错；我们退化成子串匹配，
                // warn 一次都不做——每次调用打日志太吵，加载时已统计）。
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
    /// 工具执行失败后（2026-08-29 三段化扩展）。观察型：stderr 只记日志。
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
    /// 压缩前后（2026-08-29 扩展）。观察型：exit 2 不阻止压缩（稳定性机制）。
    #[serde(default)]
    pre_compact: Vec<CcHookGroup>,
    #[serde(default)]
    post_compact: Vec<CcHookGroup>,
}

impl CcEvents {
    fn total_scripts(&self) -> usize {
        self.script_counts().iter().map(|(_, n)| n).sum()
    }

    fn is_empty(&self) -> bool {
        self.total_scripts() == 0
    }

    /// Per-event script counts（hooks.json 的 PascalCase 事件名）。诊断/用
    /// UI 用（P4 Hooks Tab 的 summary）——字段私有，外部 crate 走这里。
    /// 顺序：PreToolUse, PostToolUse, PostToolUseFailure, SessionStart,
    /// UserPromptSubmit, Stop, SessionEnd, PreCompact, PostCompact。
    pub fn script_counts(&self) -> [(&'static str, usize); 9] {
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
        ]
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
        .map_err(|e| format!("not CC hooks format: {e}"))?;
    // 只认 type=="command"（当前 CC 唯一类型）；未知的跳过并统计。
    let mut skipped = 0usize;
    for groups in [
        &mut events.pre_tool_use,
        &mut events.post_tool_use,
        &mut events.session_start,
        &mut events.user_prompt_submit,
        &mut events.stop,
    ] {
        for g in groups.iter_mut() {
            let before = g.hooks.len();
            g.hooks
                .retain(|h| h.r#type.is_empty() || h.r#type == "command");
            skipped += before - g.hooks.len();
        }
    }
    if skipped > 0 {
        tracing::warn!("[cc-hooks] skipped {} non-command hook(s)", skipped);
    }
    Ok(events)
}

// ---------------------------------------------------------------------------
// 工具名方言映射
// ---------------------------------------------------------------------------

/// 我们的工具名 → CC 工具名。真 CC 脚本 matcher / payload 里用的是右边。
pub fn cc_tool_alias(raw: &str) -> Option<&'static str> {
    Some(match raw {
        "exec" | "async_shell" => "Bash",
        "edit" | "edit_file" => "Edit",
        "write_file" => "Write",
        "read_file" => "Read",
        "grep" => "Grep",
        _ => return None,
    })
}

/// 在原始 args JSON 上补 CC 字段名别名（`file_path` 等），让
/// `jq .tool_input.file_path` 型真脚本直接可用。原字段全保留。
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
        // CC 公共字段。session_id 用我们的 session_key；transcript_path
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

/// PreToolUse 的 stdin JSON（含 CC 别名 + tool_input 别名增补）。
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
        // 按自己的原生语法解析，与真 shell 收到的一致。（实测教训：echo
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
    if let Some(mut stdin) = child.stdin.take() {
        // hook payload 都是小 JSON（KB 级），inline 写完即关——脚本不读
        // stdin 也不会挂住写端。
        let _ = stdin.write_all(payload.as_bytes()).await;
        drop(stdin);
    }

    let fut = child.wait_with_output();
    match tokio::time::timeout(Duration::from_secs(timeout_secs.max(1)), fut).await {
        Ok(Ok(out)) => ScriptOutcome {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
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
    /// CC 退出码语义：2 = 阻断；0 = 放行；其他（含 None/超时）= 非阻断错误。
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

    /// 阻断理由文本：stderr 优先（CC 语义），空则 stdout，再空则占位。
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

/// CC hooks.json → 钩子体系桥。实现 [`ToolHook`]（PreToolUse/PostToolUse）与
/// [`LifecycleHook`]（SessionStart/UserPromptSubmit/Stop）。
pub struct CcHookBridge {
    events: CcEvents,
    project_dir: PathBuf,
    /// SessionStart 只跑一次的判重（session_key → 已见）。
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
            events,
            project_dir,
            seen_sessions: Mutex::new(HashSet::new()),
            stop_blocks: Mutex::new(HashMap::new()),
        })
    }

    /// 从 config 目录加载 `<dir>/hooks.json`。文件不存在 → None（静默，
    /// 正常态）；存在但解析失败 → warn + None（fail-open，不拖死 gateway）。
    pub fn load_from_dir(config_dir: &Path, project_dir: PathBuf) -> Option<Arc<Self>> {
        let path = config_dir.join(HOOKS_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return None, // 没有 hooks.json = 没配，正常
        };
        match Self::from_json(&text, project_dir) {
            Ok(bridge) => {
                if bridge.events.is_empty() {
                    tracing::info!(
                        "[cc-hooks] {} loaded but declares no scripts",
                        path.display()
                    );
                    return None;
                }
                tracing::info!(
                    "[cc-hooks] loaded {} script(s) from {} (PreToolUse={}, PostToolUse={}, \
                     SessionStart={}, UserPromptSubmit={}, Stop={})",
                    bridge.events.total_scripts(),
                    path.display(),
                    bridge
                        .events
                        .pre_tool_use
                        .iter()
                        .map(|g| g.hooks.len())
                        .sum::<usize>(),
                    bridge
                        .events
                        .post_tool_use
                        .iter()
                        .map(|g| g.hooks.len())
                        .sum::<usize>(),
                    bridge
                        .events
                        .session_start
                        .iter()
                        .map(|g| g.hooks.len())
                        .sum::<usize>(),
                    bridge
                        .events
                        .user_prompt_submit
                        .iter()
                        .map(|g| g.hooks.len())
                        .sum::<usize>(),
                    bridge
                        .events
                        .stop
                        .iter()
                        .map(|g| g.hooks.len())
                        .sum::<usize>(),
                );
                Some(Arc::new(bridge))
            }
            Err(e) => {
                tracing::warn!(
                    "[cc-hooks] failed to parse {} — hooks DISABLED (fail-open): {}",
                    path.display(),
                    e
                );
                None
            }
        }
    }

    /// 挂到 AgentLoop（工具钩子 + 生命周期钩子一起）。
    pub fn register(self: Arc<Self>, agent_loop: &crate::r#loop::AgentLoop) {
        agent_loop.add_tool_hook(self.clone());
        agent_loop.add_lifecycle_hook(self);
    }

    /// 依次跑一组里所有命中 matcher 的命令。
    async fn run_group(
        &self,
        groups: &[CcHookGroup],
        tool: Option<&str>,
        payload: &str,
    ) -> Vec<ScriptOutcome> {
        let mut out = Vec::new();
        for g in groups {
            if let Some(t) = tool
                && !g.matches(t)
            {
                continue;
            }
            for h in &g.hooks {
                let timeout = h.timeout.unwrap_or(DEFAULT_TIMEOUT_SECS);
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

    /// 当前 session 的 stop_hook_active（测试用 pub(crate) 可见）。
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
        let payload = pre_tool_use_payload(call, &self.project_dir);
        for o in self
            .run_group(&self.events.pre_tool_use, Some(&call.name), &payload)
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
            .run_group(&self.events.post_tool_use, Some(&call.name), &payload)
            .await
        {
            if o.is_blocking_exit() {
                // CC 语义：PostToolUse 阻断不撤销操作，stderr 反馈给模型。
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

    /// CC `PostToolUseFailure`（2026-08-29 三段化扩展）：工具执行失败后触发。
    /// 观察型——stderr 只记日志（失败已发生，无撤销/改写语义）。
    async fn post_tool_use_failure(&self, call: &HookToolCall, err: &str) -> PostHookAction {
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
                &self.events.post_tool_use_failure,
                Some(&call.name),
                &payload,
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
            self.run_group(&self.events.session_start, None, &payload)
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
            .run_group(&self.events.user_prompt_submit, None, &payload)
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
        let payload = build_event_payload(
            "Stop",
            &end.session_key,
            &self.project_dir,
            serde_json::json!({ "stop_hook_active": end.stop_hook_active }),
        );
        for o in self.run_group(&self.events.stop, None, &payload).await {
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
        TurnEndDecision::Stop
    }
}

impl CcHookBridge {
    /// CC `SessionEnd`（观察型）：会话被清理/删除时触发。exit 2 无阻断语义。
    /// 固有方法而非 trait——唯一实现者是本桥，不建单实现 trait（YAGNI）。
    pub async fn on_session_end(&self, session_key: &str, reason: &str) {
        let payload = build_event_payload(
            "SessionEnd",
            session_key,
            &self.project_dir,
            serde_json::json!({ "reason": reason }),
        );
        self.run_group(&self.events.session_end, None, &payload)
            .await;
    }
}

impl CcHookBridge {
    /// CC `PreCompact` / `PostCompact`（观察型）：压缩流水线前后触发。
    /// exit 2 不阻止压缩（稳定性机制，诚实边界）。
    pub async fn run_compact_hooks(&self, trigger: &str, phase: &str) {
        let event = if phase == "pre" {
            "PreCompact"
        } else {
            "PostCompact"
        };
        let payload = build_event_payload(
            event,
            "system",
            &self.project_dir,
            serde_json::json!({ "trigger": trigger }),
        );
        let groups = if phase == "pre" {
            &self.events.pre_compact
        } else {
            &self.events.post_compact
        };
        for o in self.run_group(groups, None, &payload).await {
            if !o.stdout.is_empty() {
                tracing::info!("[cc-hooks] {event} stdout: {}", o.stdout);
            }
        }
    }
}

#[cfg(test)]
mod tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
