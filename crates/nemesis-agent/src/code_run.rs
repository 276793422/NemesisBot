//! T2（内部专项总计划 L1 codemode，2026-10-13）— `code_run` 脚本工具。
//!
//! 给模型一个 QuickJS（rquickjs）沙箱，在**单次工具调用**里跑一段 JS：
//! 脚本内通过 `tool(name, argsJson)` 调用任意已注册工具，编排读-改-写等
//! 多步组合——省掉每步一个 LLM 轮次的上下文开销（codemode 的经济学动机）。
//!
//! 安全面（不旁路是底线）：
//! - 脚本内 `tool()` 走 [`AgentLoop::handle_tool_call_at_depth`] **完整
//!   dispatch 瀑布**（estop → hidden → min-tier → Plan → 安全 8 层 → 审批
//!   → hooks）——与 WASM 插件 guest tool-invoke 同一桥形态（GatewayHostInvoker）；
//! - 脚本内不能调 `code_run` 自身（深度 1 闸，防递归线程放大）；
//! - 资源闸：interrupt handler 掐墙钟（`agents.codemode.timeout_secs`，
//!   JS 字节码检查点生效 + 每次工具调用的剩余预算透传）、
//!   `set_memory_limit` 限 JS 堆；脚本错误回传文本（不返 Err），模型可自纠；
//! - 编译闸 `codemode` cargo feature + 运行闸 `agents.codemode.enabled`
//!   （2026-10-25 默认开，Dashboard Tools 页可关；check_config_reload 热生效
//!   ——开关切换下一轮 LLM 前增删注册表成员资格，无需重启）+ tier 闸
//!   `min_tier() = Some("big")`（供给+分发双闸，2026-09-30 插件体系复查
//!   #2 同模型）。
//!
//! 执行模型：rquickjs 非 parallel 形态的 Runtime/Context 不是 Send，整个
//! JS 生命周期封闭在 `tokio::task::spawn_blocking` 线程内；宿主函数同步
//! 返回 → `Handle::block_on` 桥进异步 dispatch（阻塞线程不占 runtime
//! worker；要求 multi-thread runtime——current_thread 下 block_on 于
//! blocking 线程无法驱动定时器/IO，会挂死）。

// 注意：运行期类型导入全部住在 `mod imp` 内（codemode feature 关时该模块
// 整体编译期消失）——放顶层会在非 codemode feature 组合下全部变 unused 警告。

/// 工具名（模型可见）。
pub const CODE_RUN_TOOL_NAME: &str = "code_run";

/// 脚本返回值的输出上限（字节）。超限截断并附诚实注记——模型侧上下文
/// 是共享资源，脚本一把梭出几 MB 回复是成本事故。
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[cfg(feature = "codemode")]
pub use imp::CodeRunTool;

#[cfg(feature = "codemode")]
mod imp {
    use std::sync::Weak;
    use std::time::{Duration, Instant};

    use crate::context::RequestContext;
    use crate::r#loop::AgentLoop;
    use crate::r#loop::Tool;

    use super::*;

    /// console 聚合行数上限（A2 残余，2026-10-18）。超限丢弃后续行并附
    /// 诚实注记——脚本里 console.log 一百万行同样是被共享上下文的事故。
    pub(crate) const MAX_CONSOLE_LINES: usize = 200;

    /// console 行缓冲（脚本生命周期内同步收集；行数闸 + 截断标志）。
    struct ConsoleBuf {
        lines: Vec<String>,
        truncated: bool,
    }

    impl ConsoleBuf {
        fn new() -> std::rc::Rc<std::cell::RefCell<Self>> {
            std::rc::Rc::new(std::cell::RefCell::new(Self {
                lines: Vec::new(),
                truncated: false,
            }))
        }

        /// 单行入缓冲（超限丢弃 + 置截断标志，不炸脚本）。
        fn push(buf: &std::rc::Rc<std::cell::RefCell<Self>>, line: String) {
            let mut b = buf.borrow_mut();
            if b.lines.len() >= MAX_CONSOLE_LINES {
                b.truncated = true;
                return;
            }
            b.lines.push(line);
        }
    }

    /// console 多参数拼接：字符串直取；其余 JSON 序列化（undefined →
    /// 字面量；序列化失败 → 诚实占位，不炸脚本）。泛型 'js 统一 ctx 与
    /// value 生命周期（invariance 下拆不开）。
    fn join_console_values<'js>(
        ctx: &rquickjs::Ctx<'js>,
        args: rquickjs::function::Rest<rquickjs::Value<'js>>,
    ) -> String {
        args.iter()
            .map(|v| {
                if let Some(s) = v.as_string()
                    && let Ok(s) = s.to_string()
                {
                    return s;
                }
                if v.is_undefined() {
                    return "undefined".to_string();
                }
                match ctx.json_stringify(v.clone()) {
                    Ok(Some(s)) => s.to_string().unwrap_or_else(|_| "[unserializable]".into()),
                    _ => "[unserializable]".to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// 安装全局 `console`（log/info/warn/error → 行缓冲；warn/error 带级别
    /// 前缀）。可变参数走 `Rest<Value>`——固定 arity 的 Func 接不住
    /// `console.log("a", b, c)`。
    fn install_console(
        globals: &rquickjs::Object<'_>,
        buf: &std::rc::Rc<std::cell::RefCell<ConsoleBuf>>,
    ) -> Result<(), String> {
        let console = rquickjs::Object::new(globals.ctx().clone())
            .map_err(|e| format!("failed to create console object: {e}"))?;
        for (name, prefix) in [
            ("log", ""),
            ("info", ""),
            ("warn", "[warn] "),
            ("error", "[error] "),
        ] {
            let buf = buf.clone();
            console
                .set(
                    name,
                    // 参数类型不注解：Func::from 的目标 trait 签名把 ctx 与
                    // args 推断到同一 'js（手写 Ctx<'_> / Rest<Value<'_>>
                    // 会被当作两个不相关匿名生命周期而撞 invariance）。
                    rquickjs::function::Func::from(move |ctx, args| -> rquickjs::Result<()> {
                        ConsoleBuf::push(
                            &buf,
                            format!("{prefix}{}", join_console_values(&ctx, args)),
                        );
                        Ok(())
                    }),
                )
                .map_err(|e| format!("failed to install console.{name}: {e}"))?;
        }
        globals
            .set("console", console)
            .map_err(|e| format!("failed to install console: {e}"))?;
        Ok(())
    }

    /// `code_run` 工具：持 loop 弱引用（Arc 定型后注入，同 GatewayHostInvoker
    /// 的 Weak 模式——loop 重建不死锁旧引用）+ 装配期读定的资源限制。
    pub struct CodeRunTool {
        loop_ref: parking_lot::RwLock<Option<Weak<AgentLoop>>>,
        timeout: Duration,
        memory_limit_bytes: usize,
    }

    impl CodeRunTool {
        pub fn new(timeout_secs: u64, memory_limit_mb: u64) -> Self {
            Self {
                loop_ref: parking_lot::RwLock::new(None),
                // 0 = 不限时（诚实语义：配置者显式选了不限；interrupt handler
                // 仍会在 QuickJS 指令检查点轮询，只是永假）。
                timeout: if timeout_secs == 0 {
                    Duration::MAX
                } else {
                    Duration::from_secs(timeout_secs)
                },
                memory_limit_bytes: (memory_limit_mb.max(1) as usize) * 1024 * 1024,
            }
        }

        /// Arc<AgentLoop> 定型后注入（agent_factory 装配点调用；未注入时
        /// 脚本内 tool() 诚实报「loop 不可用」）。
        pub fn set_loop(&self, loop_weak: Weak<AgentLoop>) {
            *self.loop_ref.write() = Some(loop_weak);
        }
    }

    #[async_trait::async_trait]
    impl Tool for CodeRunTool {
        fn description(&self) -> String {
            format!(
                "Run a JavaScript (QuickJS) script in a sandbox and return its completion value. \
PREFERRED over repeated single tool calls whenever a task needs 3+ tool invocations, loops, or \
conditional branching (batch file edits, bulk renames, filtered searches, staged pipelines): one \
code_run call replaces many round trips, and intermediate results stay inside the script instead \
of flooding the conversation. \
Inside the script, call any registered tool with `tool(name, argsJson)` where argsJson is a \
JSON string (e.g. `tool(\"read_file\", JSON.stringify({{path: \"a.txt\"}}))`); it returns the \
tool result as a string and THROWS on rejection — wrap in try/catch when a failure is expected. \
All tool calls go through the full security pipeline (estop / approval / 8 layers), exactly like \
direct tool use. `console.log/info/warn/error` are captured and appended to the result \
(first {} lines). The script's final expression value (string recommended) is returned. \
Budget: wall-clock {}s (infinite loops are interrupted), JS heap {}MB, output capped at {}KB. \
Note: a tool() call that triggers an approval card consumes budget while waiting (the card can \
outlive the budget) — budget is aligned with the approval-card lifetime by default; the script \
cannot call code_run itself.",
                MAX_CONSOLE_LINES,
                self.timeout.as_secs(),
                self.memory_limit_bytes / (1024 * 1024),
                MAX_OUTPUT_BYTES / 1024
            )
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "JavaScript source to run. Global helper `tool(name, argsJson)` invokes registered tools (throws on rejection). End with an expression returning the final string result."
                    }
                },
                "required": ["script"]
            })
        }

        fn min_tier(&self) -> Option<&str> {
            // T2 spec：默认 big tier 供给（供给+分发双闸；mini/normal 看不到
            // 也调不动——绕白名单秩比较，见 build_tool_defs / dispatch 闸）。
            Some("big")
        }

        async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
            let v: serde_json::Value =
                serde_json::from_str(args).map_err(|_| "Invalid JSON arguments".to_string())?;
            let script = v["script"]
                .as_str()
                .ok_or("missing required argument: script")?
                .to_string();
            if script.trim().is_empty() {
                return Err("script is empty".to_string());
            }

            let loop_weak = self.loop_ref.read().clone();
            let timeout = self.timeout;
            let memory_limit = self.memory_limit_bytes;
            let handle = tokio::runtime::Handle::try_current()
                .map_err(|_| "code_run requires a tokio runtime context".to_string())?;

            // JS 全生命周期封闭在 blocking 线程（rquickjs 非 parallel 形态
            // 非 Send）。脚本错误回传文本（Ok，模型可自纠；turn_guard 不判
            // 为错误——错误预算不被脚本笔误烧掉）；只有装配级失败走 Err。
            let outcome = tokio::task::spawn_blocking(move || {
                run_script(&script, loop_weak, timeout, memory_limit, handle)
            })
            .await
            .map_err(|e| format!("code_run worker panicked: {e}"))?;
            match outcome {
                Ok(s) => Ok(s),
                Err(e) => Ok(format!("[code_run] 脚本错误: {e}")),
            }
        }
    }

    /// 单次脚本的完整生命周期（blocking 线程内）。
    fn run_script(
        script: &str,
        loop_weak: Option<Weak<AgentLoop>>,
        timeout: Duration,
        memory_limit: usize,
        handle: tokio::runtime::Handle,
    ) -> Result<String, String> {
        let rt =
            rquickjs::Runtime::new().map_err(|e| format!("QuickJS runtime init failed: {e}"))?;
        rt.set_memory_limit(memory_limit);
        rt.set_max_stack_size(1024 * 1024);
        let deadline = Instant::now() + timeout;
        rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));

        let ctx = rquickjs::Context::full(&rt)
            .map_err(|e| format!("QuickJS context init failed: {e}"))?;

        ctx.with(|ctx| {
            // 脚本内工具调用面：Ok(str) 返回字符串；Err(拒绝文案) 抛 JS
            // Error（脚本可 try/catch）。estop/审批/8 层拒绝文案原样透传
            // ——可观测、可断言（安全负例三件全靠它）。`Ctx` 首参由
            // rquickjs 自动注入（FromParam），闭包只捕获 'static 的
            // loop_weak/handle。**闭包必须返回 Result**：throw_message 只
            // 挂起异常（pending），裸 String 返回值会让 QuickJS 视作正常
            // 返回——catch 永不触发（首轮实测 6 败的根因）；Err(Error)
            // 返回值才把 pending 异常拉起来。
            let globals = ctx.globals();
            // console 聚合（A2 残余）：log/info/warn/error 进行缓冲，脚本
            // 结束后附进输出摘要（限行）——调试信息不必再为看中间值多烧
            // LLM 轮次。
            let console_buf = ConsoleBuf::new();
            install_console(&globals, &console_buf)?;
            globals
                .set(
                    "tool",
                    rquickjs::function::Func::from(
                        move |ctx: rquickjs::Ctx<'_>,
                              name: String,
                              args: String|
                              -> rquickjs::Result<String> {
                            match invoke_tool(loop_weak.clone(), &name, &args, deadline, &handle) {
                                Ok(s) => Ok(s),
                                Err(e) => {
                                    let msg = format!("tool('{name}') rejected: {e}");
                                    Err(rquickjs::Exception::throw_message(&ctx, &msg))
                                }
                            }
                        },
                    ),
                )
                .map_err(|e| format!("failed to install tool(): {e}"))?;

            // 完成值：字符串直取；其他类型 JSON 序列化；undefined 给诚实占位。
            let value: rquickjs::Value = ctx
                .eval(script.to_string())
                .map_err(|e| err_text(&ctx, e))?;
            let mut out = if value.is_undefined() {
                "(script completed with no return value)".to_string()
            } else if let Some(s) = value.as_string() {
                s.to_string().map_err(|e| err_text(&ctx, e))?
            } else {
                match ctx.json_stringify(value) {
                    Ok(Some(s)) => s.to_string().map_err(|e| err_text(&ctx, e))?,
                    Ok(None) => "(script returned null)".to_string(),
                    Err(e) => format!(
                        "(script returned non-serializable value: {})",
                        err_text(&ctx, e)
                    ),
                }
            };
            // console 聚合：有行才附（空 console 零字节——不污染无调试
            // 脚本的输出）；总闸 truncate_output 兜底字节。
            {
                let buf = console_buf.borrow();
                if !buf.lines.is_empty() {
                    out.push_str("\n\n--- console output ---\n");
                    for line in &buf.lines {
                        out.push_str(line);
                        out.push('\n');
                    }
                    if buf.truncated {
                        out.push_str(&format!(
                            "(+ more console lines truncated at {MAX_CONSOLE_LINES})\n"
                        ));
                    }
                }
            }
            Ok(truncate_output(out))
        })
    }

    /// 脚本内单次工具调用 → 完整 dispatch 瀑布。
    fn invoke_tool(
        loop_weak: Option<Weak<AgentLoop>>,
        name: &str,
        args_json: &str,
        deadline: Instant,
        handle: &tokio::runtime::Handle,
    ) -> Result<String, String> {
        // 深度 1 闸：code_run 不能在脚本内调自身（每次嵌套吃一个 blocking
        // 线程 + JS runtime；递归无界是资源事故）。
        if name == CODE_RUN_TOOL_NAME {
            return Err("code_run cannot invoke itself inside a script".to_string());
        }
        let Some(loop_arc) = loop_weak.and_then(|w| w.upgrade()) else {
            return Err("agent loop unavailable (shutting down?)".to_string());
        };
        let call = crate::types::ToolCallInfo {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            arguments: args_json.to_string(),
        };
        // user 字段标 code-run 供审计溯源（同 GatewayHostInvoker 的
        // wasm-plugin 形态）；session_key 空——宿主侧审计已带工具名。
        let ctx = RequestContext::new("code-run", "", "code_run-script", "");
        let fut = loop_arc.handle_tool_call_at_depth(&call, &ctx, 0);
        // interrupt handler 只在 JS 字节码检查点生效——脚本阻塞在宿主函数里
        // 时不轮询。因此单次 dispatch 吃**剩余预算**：到点 future 被丢弃
        // （dispatch 中断点语义同 estop 中途打断），脚本在下一个 JS 检查点
        // 收到异常。不限时（Duration::MAX）透传不包 timeout。
        match deadline.checked_duration_since(Instant::now()) {
            None => Err(format!(
                "tool('{name}') not started: script wall-clock budget already exhausted \
(note: approval waits consume budget; raise agents.codemode.timeout_secs if approvals \
routinely exhaust it)"
            )),
            Some(rem) if rem == Duration::MAX => finish_invoke(handle.block_on(fut)),
            Some(rem) => match handle.block_on(tokio::time::timeout(rem, fut)) {
                Ok(res) => finish_invoke(res),
                Err(_) => Err(format!(
                    "tool('{name}') exceeded the script wall-clock budget and was aborted \
(note: approval waits consume budget; raise agents.codemode.timeout_secs if approvals \
routinely exhaust it)"
                )),
            },
        }
    }

    /// dispatch 结果 → 脚本侧 Ok/Err。拒绝两种形态都按异常透传：
    /// - 标准错误三前缀（`Error:` / `Tool error:` / `Exit code:`，见
    ///   [`crate::turn_guard::tool_result_indicates_error`]）；
    /// - `⛔` 硬拒家族（ESTOP / SECURITY BLOCKED / HOOK BLOCKED / GUARDIAN /
    ///   RATE LIMIT——dispatch 以「成功结果 + 指令文案」形态回传硬拒，见
    ///   handle_tool_call_at_depth 各闸返回值；对脚本它们是异常：可
    ///   try/catch，也让拒绝在断言面可观测）。
    fn finish_invoke(result: String) -> Result<String, String> {
        if result.starts_with('⛔') || crate::turn_guard::tool_result_indicates_error(&result) {
            Err(result)
        } else {
            Ok(result)
        }
    }

    /// rquickjs 错误 → 文本；Exception 时 catch 出 pending 异常读 message
    /// （脚本 `throw "msg"` / 内置 Error 都落到这条路径——拒绝文案对模型
    /// 可读是自纠的前提）。
    fn err_text(ctx: &rquickjs::Ctx<'_>, e: rquickjs::Error) -> String {
        if matches!(e, rquickjs::Error::Exception) {
            let caught = ctx.catch();
            if let Some(exc) = caught.as_exception() {
                if let Some(msg) = exc.message() {
                    return format!("script threw: {msg}");
                }
                return "script threw (no message)".to_string();
            }
            if let Some(s) = caught.as_string()
                && let Ok(msg) = s.to_string()
            {
                return format!("script threw: {msg}");
            }
            return "script threw".to_string();
        }
        e.to_string()
    }

    fn truncate_output(out: String) -> String {
        if out.len() <= MAX_OUTPUT_BYTES {
            return out;
        }
        let mut cut = MAX_OUTPUT_BYTES;
        while cut > 0 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        format!(
            "{}\n\n[输出已截断：{} 字节 → 保留前 {cut} 字节（上限 {}KB）]",
            &out[..cut],
            out.len(),
            MAX_OUTPUT_BYTES / 1024
        )
    }
}

#[cfg(test)]
mod tests;
