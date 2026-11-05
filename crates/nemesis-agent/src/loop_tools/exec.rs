// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// A tool that executes shell commands.
///
/// Mirrors Go's `ExecTool` which is registered as "exec" in the agent.
///
/// 输出留存契约：本工具**不做工具内截断**——全量输出返回给
/// agent loop，由 loop 层两档闸统一治理（`spill.rs` ≥65536 字符全量落盘
/// `<workspace>/logs/spill/` + locator 回灌；`prune.rs` 8193..65535 head+tail
/// 内联收窄）。工具层再截一刀会让全量字节在生产路径上不可恢复，且与
/// loop 闸双重截断；如需调整阈值改 spill/prune 常量，勿在本工具加截断。
pub struct ExecTool {
    workspace: String,
    restrict: bool,
}

impl ExecTool {
    /// Create a new exec tool.
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: workspace.to_string(),
            restrict,
        }
    }
}

#[async_trait]
impl Tool for ExecTool {
    fn description(&self) -> String {
        "Execute a shell command and wait for completion".to_string()
    }

    // vault（计划）：exec 类工具参与 `security.limits`
    // 的 exec 类别限额（滑动窗口 + 超限审批升级，见 loop/limits）。
    fn limit_categories(&self) -> &[&str] {
        &["exec"]
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"command":{"type":"string","description":"Command to execute"},"timeout":{"type":"integer","description":"Timeout in seconds (default 30, max 600)"},"cwd":{"type":"string","description":"Working directory"}},"required":["command"]})
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {}", e))?;

        let command = val
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'command' argument")?;

        let timeout_secs = exec_timeout_secs(val.get("timeout").and_then(|v| v.as_u64()));

        let cwd = val
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.workspace);

        // Workspace restriction check
        if self.restrict {
            // 与 ExecTool（shell.rs）对齐：相对 cwd join workspace；双侧统一
            // 归一化（nemesis-path 单一真相源）——组件级比较防前缀误判，
            // workspace 侧归一化防 8.3 短名/大小写失配。
            let target = if Path::new(cwd).is_absolute() {
                PathBuf::from(cwd)
            } else {
                Path::new(&self.workspace).join(cwd)
            };
            let resolved = canonicalize_for_compare(&target);
            let ws = canonicalize_for_compare(Path::new(&self.workspace));
            if !resolved.starts_with(&ws) {
                return Err(format!(
                    "Access denied: path '{}' is outside workspace",
                    cwd
                ));
            }
        }

        // ：stdout/stderr 改为显式 piped——超时收尸后要从管道
        // 缓冲区读回进程被杀前已产生的残余输出（旧 `cmd.output()` 的管道随
        // future 取消一起丢弃，超时即全丢）。stdin 保持 null（见下）。
        // 平台 shell 形态抽到 `make_piped_shell_command`（与 run_checks
        // 共用单一真相源）。
        let mut cmd = make_piped_shell_command(command);
        // ：workspace-dacl 受限模式下 piped→文件重定向
        // （std 匿名管道创建在受限令牌下必死，见 ShellOutputCapture 注释）。
        let mut capture = ShellOutputCapture::current()
            .map_err(|e| format!("Failed to prepare executor output capture: {e}"))?;
        if let Some(c) = capture.as_mut() {
            c.apply_to(&mut cmd);
        }

        // spawn 即挂子进程树守卫（Windows Job Object / Unix
        // 进程组）+ 会话注册——estop sweep 与守卫 Drop 都能收整棵树；正常
        // 完成路径 mark_complete 后 Drop 无操作（零行为变化）。
        let (guard, mut child) = crate::exec_tree::ExecTreeGuard::attach_and_register(
            cmd.current_dir(cwd),
            Some(context.session_key.as_str()),
        )
        .map_err(|e| format!("Failed to execute command: {}", e))?;

        // 管道读取必须**独立任务并发**进行——
        // ① 不能只 `child.wait()`：子进程写满 ~64KB 管道缓冲会阻塞在 write
        //    上永不退出，正常长输出路径会整体退化成超时；
        // ② 不能把管道移进被 timeout 包住的 future：超时取消时管道随 future
        //    一起 drop，残余输出照样丢。任务持管道所有权，与 wait 竞争无关；
        //    进程死后管道 EOF，任务自然收尾。（受限文件形态下 take 出 None，
        //    任务立即空返回；输出统一走 read_shell_output 出口。）
        let out_task = tokio::spawn(drain_pipe(child.stdout.take()));
        let err_task = tokio::spawn(drain_pipe(child.stderr.take()));

        let output =
            tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child.wait()).await;

        match output {
            Ok(Ok(status)) => {
                let (stdout, stderr) = read_shell_output(capture, out_task, err_task).await;
                if status.success() {
                    guard.mark_complete();
                    Ok(if stdout.is_empty() {
                        "(no output)".to_string()
                    } else {
                        stdout
                    })
                } else {
                    guard.mark_complete();
                    let mut text = format!(
                        "Exit code: {}\nstdout: {}\nstderr: {}",
                        status.code().unwrap_or(-1),
                        stdout,
                        stderr
                    );
                    // estop sweep 中途收树：诚实标注（追加不换前缀——
                    // `exec_output_passed` 失败判定依赖 "Exit code:" 开头）。
                    if guard.reason() == Some(crate::exec_tree::TerminationReason::Estop) {
                        text.push_str("\n（进程树已被 estop 终止）");
                    }
                    Ok(text)
                }
            }
            Ok(Err(e)) => {
                guard.mark_complete();
                Err(format!("Failed to execute command: {}", e))
            }
            Err(_) => {
                // 超时改 Ok 回灌（模型可自纠），先收尸再读残余输出。
                // 管道任务持有所有权不受本次取消影响；kill + reap 后管道
                // EOF，任务收尾。任务 await 兜 5s 上限：若命令 fork 了继承
                // 管道的后台孙进程，EOF 要等孙进程死——此时如实返回已读不到
                // 的空残余（旧实现同场景连收尸都没有，此处严格更优）。
                // 收尸升级为树杀（背景见 `tree_kill` 注记）——
                // start_kill 只杀 cmd.exe/sh 本体，cargo/rustc 等真实工作
                // 进程整棵存活，超时后继续烧 CPU/占 target 锁。树杀后仍
                // start_kill + wait 兜底直杀 shell 本体并收尸。：守卫先
                // 记超时原因（estop sweep 后到来的超时不得降级其标注）。
                guard.terminate(crate::exec_tree::TerminationReason::Timeout);
                if let Some(pid) = child.id() {
                    crate::background_registry::tree_kill(pid).await;
                }
                let _ = child.start_kill();
                let _ = child.wait().await;
                let (stdout, stderr) = read_shell_output(capture, out_task, err_task).await;

                let mut partial = String::new();
                if !stdout.trim().is_empty() {
                    partial.push_str("stdout (tail):\n");
                    partial.push_str(&tail_chars(&stdout, 2048));
                }
                if !stderr.trim().is_empty() {
                    if !partial.is_empty() {
                        partial.push('\n');
                    }
                    partial.push_str("stderr (tail):\n");
                    partial.push_str(&tail_chars(&stderr, 2048));
                }
                if partial.is_empty() {
                    partial.push_str("(no output before timeout)");
                }
                let mut text = format!(
                    "Command timed out after {} seconds. Partial output:\n{}\nTip: the \
                     command may be waiting for interactive input — on Windows \
                     `date`/`time` are interactive (use `date /t`/`time /t` or \
                     PowerShell Get-Date). Raise the timeout for genuinely long \
                     commands or narrow the command. Command was: {}",
                    timeout_secs, partial, command
                );
                // estop sweep 收树后的窄竞态窗口（wait 恰在超时后返回）：
                // 诚实标注，同样追加不换前缀。
                if guard.reason() == Some(crate::exec_tree::TerminationReason::Estop) {
                    text.push_str("\n（进程树已被 estop 终止）");
                }
                guard.mark_complete();
                Ok(text)
            }
        }
    }
}

/// ：exec 超时解析——默认 30s、上限 600s（对齐
/// nemesis-tools shell.rs 的 cap）。模型传 999999 之类不再被照单全收。
pub(super) fn exec_timeout_secs(v: Option<u64>) -> u64 {
    v.unwrap_or(30).min(600)
}

/// ExecTool 输出成败判定的**单一真相源**（协议消费者共用）：三种失败
/// 形态——非零退出 = `Ok("Exit code: N\n…")`；超时收尸 =
/// `Ok("Command timed out …")`；进程无法启动 = `Err`。成功路径 = 裸
/// stdout 或 `(no output)`。纪律证伪钩子（discipline）按此判定 pass/fail
/// ——超时此前被消费方误判为通过（收口当日复核修复），收此函数防漂移。
pub(crate) fn exec_output_passed(output: &str) -> bool {
    !output.starts_with("Exit code:") && !output.starts_with("Command timed out")
}

/// ：workspace-dacl 受限令牌下的 shell 输出捕获形态。
///
/// **为什么存在**：std 匿名管道在 Windows 上是命名管道实现（
/// NtCreateNamedPipeFile）——受限令牌下创建 = 对 `\Device\NamedPipe` 目录
/// 的写类访问，workspace SID 在该目录无 GRANT ACE → restricting 侧拒绝
/// （os error 5）。`nemesis-sandbox` token_tests 的 execsim 八臂矩阵实证：
/// piped 全死 / inherit 与**文件重定向**两形态活。executor 协议自身的三通
/// 管道由 gateway（普通令牌）预建、句柄继承不重评，不受影响——死的是
/// executor 子进程内部再 spawn 时 std 现建管道的四个捕获点（exec /
/// run_checks / run_script / async_shell）。
///
/// 受限模式下输出落工作区 `.exec_out/`（GRANT ACE 放行创建与写，句柄经
/// bInheritHandles 给孙进程不重评），wait 后读回。普通模式 None = 维持
/// piped 形态零变化。残留边界（证伪后收窄）：基线 mask 保含
/// DELETE，受限侧读回后自删通常成功；属主异常等失败场景留 gateway 侧清扫
/// （DaclSpawnFn spawn 前清超龄文件）兜底。
pub(super) struct ShellOutputCapture {
    out_file: Option<std::fs::File>,
    err_file: Option<std::fs::File>,
    out_path: PathBuf,
    err_path: PathBuf,
}

impl ShellOutputCapture {
    /// workspace-dacl 受限模式判定（executor 子进程内 gateway 注入的
    /// 台账标签 env；普通进程恒 false → 全部走 piped 现状）。
    fn is_restricted() -> bool {
        std::env::var("NEMESISBOT_SANDBOX_BACKEND").as_deref() == Ok("workspace-dacl")
    }

    /// 受限模式：在工作区 `.exec_out/` 建本调用的 stdout/stderr 捕获文件。
    /// fail-closed：文件建不了 = 受限树写不了（ACE 树没铺好/损坏），Err
    /// 诚实传播而非降级 piped（piped 在受限下必然 spawn 死，降级是假善意）。
    pub(super) fn current() -> Result<Option<Self>, String> {
        if !Self::is_restricted() {
            return Ok(None);
        }
        let ws = std::env::var("NEMESISBOT_EXECUTOR_WORKSPACE")
            .map_err(|_| "workspace-dacl 模式缺 NEMESISBOT_EXECUTOR_WORKSPACE".to_string())?;
        let dir = Path::new(&ws).join(".exec_out");
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("受限输出目录 {} 创建失败: {e}", dir.display()))?;
        let name = format!(
            "{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let out_path = dir.join(format!("{name}.out"));
        let err_path = dir.join(format!("{name}.err"));
        let out_file = std::fs::File::create(&out_path)
            .map_err(|e| format!("受限输出文件 {} 创建失败: {e}", out_path.display()))?;
        let err_file = std::fs::File::create(&err_path)
            .map_err(|e| format!("受限输出文件 {} 创建失败: {e}", err_path.display()))?;
        Ok(Some(Self {
            out_file: Some(out_file),
            err_file: Some(err_file),
            out_path,
            err_path,
        }))
    }

    /// 把捕获文件接上命令的 stdout/stderr（覆盖 make_piped_shell_command
    /// 设置的 piped——setter 链最后一次设置生效）。
    fn apply_to(&mut self, cmd: &mut tokio::process::Command) {
        if let Some(f) = self.out_file.take() {
            cmd.stdout(std::process::Stdio::from(f));
        }
        if let Some(f) = self.err_file.take() {
            cmd.stderr(std::process::Stdio::from(f));
        }
    }

    /// [`apply_to`] 的 `std:process:Command` 形态（GitTool 等同步 spawn
    /// 点用）——同一捕获文件、同一读回路径，形态差异只在 Command 类型。
    pub(super) fn apply_to_std(&mut self, cmd: &mut std::process::Command) {
        if let Some(f) = self.out_file.take() {
            cmd.stdout(std::process::Stdio::from(f));
        }
        if let Some(f) = self.err_file.take() {
            cmd.stderr(std::process::Stdio::from(f));
        }
    }

    /// wait 后读回 (stdout, stderr)；受限侧删除尽力而为（失败=残留，见结构
    /// 注释——DELETE 不在 write-restricted 写类评估集合，基线 mask 保含
    /// DELETE 自删通常成功，属主异常等场景失败由 gateway 超龄清扫兜底）。
    /// stdout 读失败按空串 + stderr 注记（读不回
    /// 不该让整次已成功执行的命令变 Err）。
    pub(super) fn read_back(self) -> (String, String) {
        let out = std::fs::read(&self.out_path);
        let err = std::fs::read(&self.err_path);
        let mut stdout = match &out {
            Ok(b) => String::from_utf8_lossy(b).to_string(),
            Err(_) => String::new(),
        };
        let stderr = match &err {
            Ok(b) => String::from_utf8_lossy(b).to_string(),
            Err(_) => String::new(),
        };
        if out.is_err() {
            stdout.push_str("\n[受限输出文件读回失败（stdout 已丢弃）]");
        }
        for p in [&self.out_path, &self.err_path] {
            if let Err(e) = std::fs::remove_file(p) {
                debug!(
                    "[Tools] 受限输出文件 {} 清理失败（残留由 gateway 超龄清扫兜底）: {e}",
                    p.display()
                );
            }
        }
        (stdout, stderr)
    }
}

/// exec / exec_async / run_checks / run_script / git 五处共用的 console
/// creation flags 决策**单一真相源**（建立； 黑窗
/// 修复收编默认臂）。
///
/// **默认臂 = `Some(CREATE_NO_WINDOW)`（黑窗根修）**：release 版 gateway 是
/// windows 子系统进程（main.rs `windows_subsystem = "windows"`，自身无
/// console），console 子进程（cmd.exe/git.exe/CLI 工具）以默认 flags 从无
/// console 父进程 spawn 时，系统为其分配**新的可见 console** → 黑窗一闪/
/// 长命令躺桌面（stdout/stderr 已被管道收走，窗内无内容，纯视觉污染）。
/// CREATE_NO_WINDOW 只压 console（conhost）分配，GUI 程序窗口是程序自建
/// CreateWindow、与此类 flags 完全无关——projects.open_dir 用它 spawn
/// explorer.exe 照常弹窗（仓内实证，MCP/LSP/后台进程等 ~15 处同款先例）；
/// 且优于 DETACHED_PROCESS（console 仍存在，控制台 API 可用，管道不受影
/// 响）。唯一边界：console TUI 界面不可见（exec 非交互 + 管道收输出，零
/// 实际损失）。
///
/// **workspace-dacl 受限臂保持 语义**：受限令牌树下**新** console 分配
/// 必死 0xC0000142（nemesis-sandbox token.rs 实证；CREATE_NO_WINDOW 的隐
/// 藏 console 同样算新分配），故：
/// - exec_worker 附着成功（`NEMESISBOT_CONSOLE=1`）→ `None`：不加 flags，
///   子进程继承附着 console（无新分配），第三方链式工具链一路继承可用；
/// - 未附着/未设 → `Some(DETACHED_PROCESS)`：无 console 无分配，命令本体
///   可跑（深层第三方默认 flags spawn 是诚实边界）。
#[cfg(target_os = "windows")]
pub(super) fn console_creation_flags() -> Option<u32> {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let restricted = std::env::var("NEMESISBOT_SANDBOX_BACKEND").as_deref() == Ok("workspace-dacl");
    if restricted && std::env::var("NEMESISBOT_CONSOLE").as_deref() != Ok("1") {
        // 受限令牌 + 未附着：新 console 分配（含隐藏）必死 → 完全无 console
        Some(DETACHED_PROCESS)
    } else if restricted {
        // 受限令牌 + 附着成功：继承附着 console（无新分配）
        None
    } else {
        // 生产默认：压黑窗（只管 console 分配，GUI 程序窗口不受影响）
        Some(CREATE_NO_WINDOW)
    }
}

/// 输出读取的统一出口：piped 形态走并发排水任务，受限文件形态走读回。
async fn read_shell_output(
    capture: Option<ShellOutputCapture>,
    out_task: tokio::task::JoinHandle<Vec<u8>>,
    err_task: tokio::task::JoinHandle<Vec<u8>>,
) -> (String, String) {
    match capture {
        Some(c) => c.read_back(),
        None => {
            let stdout = String::from_utf8_lossy(&join_pipe_task(out_task).await).to_string();
            let stderr = String::from_utf8_lossy(&join_pipe_task(err_task).await).to_string();
            (stdout, stderr)
        }
    }
}

/// ：平台 shell 形态的单一真相源——ExecTool 与 RunChecksTool
/// 共用。逐字保留 语义：Windows `cmd /C` + raw_arg（.arg() 的自动加引号
/// 会搅乱 cmd.exe 自身的引号处理）；stdin null（交互式命令立即 EOF 不挂满
/// 超时）；stdout/stderr piped；kill_on_drop（挂死命令不留孤儿）。
/// （扩展计划）：spawn 前剥继承环境中的凭据类变量（*KEY*/TOKEN/
/// SECRET/PASSWORD/CREDENTIAL/NEMESISBOT_*）——`exec` 里 echo 环境变量
/// 即可读出 agent 进程凭据的缺口在此封堵；`security.sanitize_child_env`
/// 可关（默认开）。本函数是 exec/run_checks 的接闸点； 是**家族闸**
/// 不是单一闸——所有 LLM 驱动 exec 类 spawn 点（run_script/exec_async/
/// git/background_start 等）各自接同一闸（新 spawn 点
/// 必须显式接闸，覆盖率靠家族清点不靠单一入口）。
fn make_piped_shell_command(command: &str) -> tokio::process::Command {
    #[cfg(target_os = "windows")]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        let mut c = tokio::process::Command::new("cmd");
        c.raw_arg(format!("/C {}", command));
        c.stdin(std::process::Stdio::null());
        c.stdout(std::process::Stdio::piped());
        c.stderr(std::process::Stdio::piped());
        c.kill_on_drop(true);
        // 黑窗根修：release gateway 无 console（windows 子系统），
        // cmd.exe 默认 flags 会分配新可见 console → 黑窗。flags 决策单一真
        // 相源在 `console_creation_flags`（exec/exec_async/run_checks/
        // run_script/git 五处共用；普通模式=CREATE_NO_WINDOW 压黑窗，GUI 程
        // 序窗口不受影响；workspace-dacl=附着继承/DETACHED）。
        if let Some(f) = console_creation_flags() {
            c.creation_flags(f);
        }
        let stripped = nemesis_utils::env_sanitize::sanitize_tokio_command(&mut c);
        if stripped > 0 {
            debug!("[Tools] env sanitized: {stripped} vars stripped");
        }
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
        c.stdin(std::process::Stdio::null());
        c.stdout(std::process::Stdio::piped());
        c.stderr(std::process::Stdio::piped());
        c.kill_on_drop(true);
        // 树杀前提（与 background_registry:build_platform_shell
        // 同款）：子进程自成一进程组（pgid == pid），超时收尸的
        // `tree_kill` 才能用 `kill -TERM -- -PGID` 一锅端 shell 的孙进程
        // （cargo/rustc 等），否则进程组号不存在、树杀静默落空。
        c.process_group(0);
        let stripped = nemesis_utils::env_sanitize::sanitize_tokio_command(&mut c);
        if stripped > 0 {
            debug!("[Tools] env sanitized: {stripped} vars stripped");
        }
        c
    }
}

/// 排干管道到字节缓冲（pipe 缺失时返空）。有界排干（BUG
/// 横扫角度 B）：1 MiB 丢头保尾（`join_pipe_task` 的 5s 上限只兜孙进程
/// 占管道 EOF 不达，不兜体量）。
pub(super) async fn drain_pipe(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> Vec<u8> {
    match pipe {
        Some(p) => nemesis_utils::pipe_cap::drain_capped_async(p).await,
        None => Vec::new(),
    }
}

/// 收管道读取任务的结果；兜 5s 上限（孙进程继承管道 EOF 不达时不再挂）。
async fn join_pipe_task(handle: tokio::task::JoinHandle<Vec<u8>>) -> Vec<u8> {
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default()
}

/// 超时残余输出的字符安全尾部截取（多字节安全，不按字节切）。
pub(super) fn tail_chars(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        s.to_string()
    } else {
        s.chars().skip(total - max_chars).collect()
    }
}

// ---------------------------------------------------------------------------
// （devtool-upgrade）：run_checks 构建/测试运行器
// ---------------------------------------------------------------------------

/// 项目类型探测结果（探测顺序即数组顺序，首个命中者胜）。
const CHECK_ECOSYSTEMS: &[(&str, &str)] = &[
    ("rust", "Cargo.toml"),
    ("node", "package.json"),
    ("go", "go.mod"),
    ("python", "pyproject.toml"),
    ("maven", "pom.xml"),
];

/// 各生态 × scope 的命令映射（`filter` 只对 test 有意义的生态追加）。
/// 返回 (阶段标签, 命令) 有序对——`all` 按 build→test→lint 顺序排列。
/// 故意**不做**映射的格子返回 None——回灌诚实报「未映射」，不硬猜命令。
pub(super) fn checks_commands(
    eco: &str,
    scope: &str,
    filter: Option<&str>,
) -> Option<Vec<(&'static str, String)>> {
    let filter = filter.map(str::trim).filter(|s| !s.is_empty());
    match (eco, scope) {
        ("rust", "build") => Some(vec![("build", "cargo build --message-format short".into())]),
        ("rust", "test") => Some(vec![(
            "test",
            match filter {
                Some(f) => format!("cargo test {f}"),
                None => "cargo test".into(),
            },
        )]),
        ("rust", "lint") => Some(vec![("lint", "cargo clippy --message-format short".into())]),
        ("rust", "all") => Some(vec![
            ("build", "cargo build --message-format short".into()),
            ("test", "cargo test".into()),
            ("lint", "cargo clippy --message-format short".into()),
        ]),
        ("node", "build") => Some(vec![("build", "npm run build".into())]),
        ("node", "test") => Some(vec![("test", "npm test".into())]),
        ("node", "lint") => Some(vec![("lint", "npm run lint".into())]),
        ("node", "all") => Some(vec![
            ("build", "npm run build".into()),
            ("test", "npm test".into()),
            ("lint", "npm run lint".into()),
        ]),
        ("go", "build") => Some(vec![("build", "go build ./...".into())]),
        ("go", "test") => Some(vec![(
            "test",
            match filter {
                Some(f) => format!("go test -run {f} ./..."),
                None => "go test ./...".into(),
            },
        )]),
        ("go", "lint") => Some(vec![("lint", "go vet ./...".into())]),
        ("go", "all") => Some(vec![
            ("build", "go build ./...".into()),
            ("test", "go test ./...".into()),
            ("lint", "go vet ./...".into()),
        ]),
        ("python", "test") => Some(vec![(
            "test",
            match filter {
                Some(f) => format!("pytest -q -k {f}"),
                None => "pytest -q".into(),
            },
        )]),
        ("python", "lint") => Some(vec![("lint", "ruff check .".into())]),
        // python 无标准 build 命令——不猜。
        ("maven", "build") => Some(vec![("build", "mvn -q compile".into())]),
        ("maven", "test") => Some(vec![(
            "test",
            match filter {
                Some(f) => format!("mvn -q test -Dtest={f}"),
                None => "mvn -q test".into(),
            },
        )]),
        // maven 无独立 lint——checkstyle 插件不假设。
        _ => None,
    }
}

/// 在目录下探测项目类型（首个命中的标记文件决定生态）。
pub(super) fn detect_project_eco(root: &Path) -> Option<&'static str> {
    for (eco, marker) in CHECK_ECOSYSTEMS {
        if root.join(marker).is_file() {
            return Some(eco);
        }
    }
    None
}

/// 相邻 token 对（数字 + 计数词）扫描：返回 (passed, failed, ignored, any)。
/// 容忍段内装饰 token（cargo 的 `ok.` 前缀、pytest 的 `====` 围栏）——数字
/// 对出现在哪一对都收。pytest 的 `N error` 计入 failed。
pub(super) fn count_labeled_numbers(toks: &[&str]) -> (u64, u64, u64, bool) {
    let mut p = 0u64;
    let mut f = 0u64;
    let mut i = 0u64;
    let mut any = false;
    for w in toks.windows(2) {
        let Ok(num) = w[0].parse::<u64>() else {
            continue;
        };
        match w[1] {
            l if l.starts_with("passed") => {
                p += num;
                any = true;
            }
            l if l.starts_with("failed") || l.starts_with("error") => {
                f += num;
                any = true;
            }
            l if l.starts_with("ignored") || l.starts_with("skipped") => {
                i += num;
                any = true;
            }
            _ => {}
        }
    }
    (p, f, i, any)
}

/// 从输出里聚合结构化测试统计（passed, failed, ignored）。只解析**有结构**
/// 的汇总行，解析不出来返 None（不编造数字）：
/// - cargo：多段 `test result: ok. 3 passed; 0 failed;...` 全部累加（lib +
///   多 bin 各一段）。逐段容错——`ok.` 前缀等非数字 token 跳过，不毒化整
///   个函数（曾有 `?` 连坐 bug：一段垃圾 token 让全部统计变 None）。
/// - pytest：`===== N passed, M failed, 1 skipped in 0.5s =====` 汇总行（取
///   最后一行；`====` 装饰 token 不在数字位则天然跳过）。
pub(super) fn parse_test_stats(output: &str) -> Option<(u64, u64, u64)> {
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut ignored = 0u64;
    let mut saw_cargo = false;
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("test result:") {
            saw_cargo = true;
            for part in rest.split(';') {
                let toks: Vec<&str> = part.split_whitespace().collect();
                let (p, f, i, _) = count_labeled_numbers(&toks);
                passed += p;
                failed += f;
                ignored += i;
            }
        }
    }
    if saw_cargo {
        return Some((passed, failed, ignored));
    }
    // pytest 汇总：取匹配的最后一行。
    let mut pytest: Option<(u64, u64, u64)> = None;
    for line in output.lines() {
        if !(line.contains("passed") || line.contains("failed")) {
            continue;
        }
        let toks: Vec<&str> = line.split_whitespace().collect();
        let (p, f, i, any) = count_labeled_numbers(&toks);
        if any {
            pytest = Some((p, f, i));
        }
    }
    pytest
}

/// 失败签名行提取（去重 + 计数，保序，cap 条）。聚焦回灌的核心：模型先看
/// 到错误行，全量在 spill 文件里。签名清单保守——只收确定是失败信号的行。
pub(super) fn extract_failure_lines(output: &str, cap: usize) -> Vec<String> {
    const SIGNATURES: &[&str] = &[
        "error[",              // rustc E---- 结构化错误
        "error:",              // rustc / clippy / go / npm 通用
        "test result: FAILED", // cargo test 失败汇总行
        "---- ",               // cargo test 失败用例头（`---- name stdout ----`）
        "FAILED",              // pytest FAILURES / npm
        "AssertionError",
        "SyntaxError",
        "ELIFECYCLE",
        "panicked",
        "✗",
    ];
    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        let t = line.trim_end();
        if t.is_empty() {
            continue;
        }
        if SIGNATURES.iter().any(|s| t.contains(s)) {
            // 同一行重复（如多 target 重编）→ 附加计数提示（best-effort，
            // 只做降噪不追求精确次数）。
            if let Some(pos) = out.iter().position(|e| e == t) {
                out[pos] = format!("{t}  (×2)");
                continue;
            }
            if out.len() >= cap {
                out.push(format!(
                    "…（更多失败行省略，共 {} 行签名，全文见下方存档）",
                    cap
                ));
                break;
            }
            out.push(t.to_string());
        }
    }
    out
}

/// 构建产物/测试运行的输出聚焦回灌工具。
///
/// 与 `exec` 的差异：exec 是自由命令，输出全量回灌（≥64KB 才 spill）；
/// run_checks 是**固定命令表**（按项目类型 × scope 映射），回灌只有统计 +
/// 失败签名行摘录，全量输出无条件存档到 spill（`save_tool_output`），模型
/// 要细节时再 read_file/grep 检索——编译/测试输出动辄几千行，全量回灌烧
/// context 且噪声淹没真正的 error 行。
///
/// 注册在 `register_shared_tools`（有 workspace 时）；进 MOVE_TOOLS（构建
/// 会写 target/，executor 分离/沙盒应罩住它；plan 模式下供给与 dispatch
/// 双闸都拦）；tier Normal/Big（Mini 不给——exec 已够）。
pub struct RunChecksTool {
    workspace: String,
}

impl RunChecksTool {
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: workspace.to_string(),
        }
    }
}

#[async_trait]
impl Tool for RunChecksTool {
    fn description(&self) -> String {
        "Run project build/test/lint checks with output focusing: replies contain pass/fail \
         stats plus deduplicated error-signature lines only; the full output is archived to \
         a file (path included in the reply) for read_file/grep retrieval. Use this instead \
         of `exec` for builds and test suites."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type":"object",
            "properties":{
                "scope":{"type":"string","enum":["build","test","lint","all"],"description":"Which checks to run (default \"all\": build → test → lint, stops at first failing stage)"},
                "filter":{"type":"string","description":"Optional single-test filter passed to the test command (e.g. cargo test <name>, pytest -k <name>)"},
                "timeout":{"type":"integer","description":"Timeout in seconds, applied PER STAGE independently (default 600, max 600). With scope=all (build → test → lint run sequentially), total wall clock can reach ~3x this value; a timed-out stage is skipped and the next stage still runs."}
            }
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid JSON arguments: {e}"))?;
        let scope = val
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("all")
            .to_lowercase();
        let filter = val
            .get("filter")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let timeout_secs = val
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(600)
            .min(600);

        // 项目类型探测：无标记 → 诚实报错（列出查过的标记文件）。
        let root = Path::new(&self.workspace);
        let Some(eco) = detect_project_eco(root) else {
            let markers: Vec<&str> = CHECK_ECOSYSTEMS.iter().map(|(_, m)| *m).collect();
            return Err(format!(
                "No recognizable project at workspace root (checked: {}). run_checks only \
                 maps commands for detected project types — use `exec` for anything else.",
                markers.join(", ")
            ));
        };
        let Some(stages) = checks_commands(eco, &scope, filter.as_deref()) else {
            return Err(format!(
                "Scope \"{scope}\" is not mapped for {eco} projects (mapped scopes differ \
                 per ecosystem). Use `exec` with the project's own command instead."
            ));
        };

        // 逐阶段执行；失败早停（build 失败后跑 test 只会复读同一批错误）。
        let mut full = String::new();
        let mut summary = Vec::new();
        let mut failed_stage = false;
        // estop sweep 收杀的阶段——不计失败/超时，后续阶段
        // 不再运行（sweep 是时点快照，break 后剩余阶段本可在 estop 下存活）。
        let mut estop_killed_stage = false;
        for (label, command) in stages {
            let started = std::time::Instant::now();
            let (exit_code, stdout, stderr, timed_out, estop_killed) =
                run_one_stage_in_session(&command, Some(root), timeout_secs, &context.session_key)
                    .await;
            let elapsed = started.elapsed().as_secs_f32();
            let combined =
                format!("$ {command}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}\n");
            full.push_str(&combined);
            let stats = parse_test_stats(&combined);
            if estop_killed {
                summary.push(format!(
                    "⛔ {label}: 被 estop 终止（{elapsed:.1}s elapsed，本阶段不计失败/超时）"
                ));
                estop_killed_stage = true;
                break;
            }
            if timed_out {
                summary.push(format!(
                    "⏱ {label}: TIMED OUT after {timeout_secs}s ({elapsed:.1}s elapsed)"
                ));
                failed_stage = true;
                break;
            }
            let stats_note = match stats {
                Some((p, f, i)) => format!(" — passed {p} / failed {f} / skipped {i}"),
                None => String::new(),
            };
            match exit_code {
                Some(0) => summary.push(format!("▶ {label}: exit 0 ({elapsed:.1}s){stats_note}")),
                Some(code) => {
                    summary.push(format!(
                        "✗ {label}: exit {code} ({elapsed:.1}s){stats_note}"
                    ));
                    failed_stage = true;
                }
                None => {
                    summary.push(format!("✗ {label}: spawn failed ({elapsed:.1}s)"));
                    failed_stage = true;
                }
            }
            if failed_stage {
                break;
            }
        }

        // 全量存档（无条件；失败诚实注明，聚焦部分照常回灌）。
        let session_key = &context.session_key;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .unwrap_or_default();
        let spill_root = nemesis_path::resolve_spill_dir_in_workspace(root);
        let archive = crate::spill::save_tool_output(
            &full,
            &spill_root,
            session_key,
            &stamp,
            &format!("run_checks_{scope}"),
        );
        let archive_note = match archive {
            Ok(path) => format!(
                "\n[完整输出（{} 字符）已保存：{} —— read_file/grep 可检索]",
                full.chars().count(),
                path.display()
            ),
            Err(e) => format!("\n[完整输出存档失败：{e}——上方摘录即全部可查内容]"),
        };

        // 聚焦回灌：统计行 + 失败签名摘录（去重计数）。
        let focus = extract_failure_lines(&full, 40);
        let mut reply = format!(
            "[run_checks] scope={scope} · {eco}\n{}\n",
            summary.join("\n")
        );
        if focus.is_empty() {
            if estop_killed_stage {
                reply.push_str("存在被 estop 终止的阶段，后续阶段未运行——结果不代表检查通过。\n");
            } else if failed_stage {
                reply.push_str("（exit 非零但未匹配到失败签名行——细节见完整输出）\n");
            } else {
                reply.push_str("全部通过，无失败签名行。\n");
            }
        } else {
            reply.push_str(&format!(
                "失败签名行（去重后 {} 行）：\n  {}\n",
                focus.len(),
                focus.join("\n  ")
            ));
        }
        reply.push_str(&archive_note);
        // 失败提示：单测重跑（计划验收要求的指引）。
        if failed_stage {
            reply.push_str(
                "\n提示：重跑单个检查用 run_checks（scope=test + filter），或用 exec 跑带 \
                 参数的完整命令。",
            );
        }
        Ok(reply)
    }
}

/// 跑单个检查阶段，返回 (exit_code, stdout, stderr, timed_out)。
/// exit_code None = 进程没能启动（程序不存在等）。spawn 形态与超时收尸
/// 语义复用 exec 内核（`make_piped_shell_command` + 并发管道排水 +
/// kill 后读残余——此处超时走独立返回值，不混进输出文本）。
/// 改为 `pub(crate)` + `cwd: Option`——`` !`cmd` `` 注入（loop.rs
/// rewrite_custom_command）共用同一执行内核；cwd None = 不设（继承进程）。
/// spawn 挂 `ExecTreeGuard`（超时/estop sweep/drop 均可收
/// 树）；本签名保持无会话注册（`` !`cmd` `` 注入与 pre-pause 探针无会话
/// 语义），run_checks 走 `run_one_stage_in_session`。
pub(crate) async fn run_one_stage(
    command: &str,
    cwd: Option<&Path>,
    timeout_secs: u64,
) -> (Option<i32>, String, String, bool) {
    let (code, stdout, stderr, timed_out, _estop_killed) =
        run_one_stage_guarded(command, cwd, timeout_secs, None).await;
    (code, stdout, stderr, timed_out)
}

/// 带会话注册的形态（run_checks 消费）——末位 bool = 本阶段
/// 是否被 estop sweep 收杀（「被 estop 终止」诚实标注的判定源，不记失败/
/// 超时）。会话键为空串时不注册（守卫的超时/drop 兜底照常生效）。
pub(crate) async fn run_one_stage_in_session(
    command: &str,
    cwd: Option<&Path>,
    timeout_secs: u64,
    session_key: &str,
) -> (Option<i32>, String, String, bool, bool) {
    run_one_stage_guarded(command, cwd, timeout_secs, Some(session_key)).await
}

/// 共享执行内核——spawn 挂 `ExecTreeGuard`（超时臂显式 terminate +
/// drop RAII 兜底 + estop sweep 可达）；其余形态与原 run_one_stage 逐字节
/// 等价（make_piped_shell_command + dacl capture + 并发排水 + 树杀收尸）。
async fn run_one_stage_guarded(
    command: &str,
    cwd: Option<&Path>,
    timeout_secs: u64,
    session_key: Option<&str>,
) -> (Option<i32>, String, String, bool, bool) {
    let mut cmd = make_piped_shell_command(command);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    // ：workspace-dacl 受限模式下 piped→文件重定向（同 exec）。
    let mut capture = match ShellOutputCapture::current() {
        Ok(c) => c,
        Err(e) => {
            return (
                None,
                String::new(),
                format!("output capture setup failed: {e}"),
                false,
                false,
            );
        }
    };
    if let Some(c) = capture.as_mut() {
        c.apply_to(&mut cmd);
    }
    let (guard, mut child) =
        match crate::exec_tree::ExecTreeGuard::attach_and_register(&mut cmd, session_key) {
            Ok(pair) => pair,
            Err(e) => {
                return (
                    None,
                    String::new(),
                    format!("failed to spawn: {e}"),
                    false,
                    false,
                );
            }
        };
    let out_task = tokio::spawn(drain_pipe(child.stdout.take()));
    let err_task = tokio::spawn(drain_pipe(child.stderr.take()));
    let waited =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child.wait()).await;
    match waited {
        Ok(Ok(status)) => {
            // estop sweep 中途收树也走本臂（wait 正常返回非成功状态）——
            // 先读原因再 mark_complete。
            let estop_killed = guard.reason() == Some(crate::exec_tree::TerminationReason::Estop);
            guard.mark_complete();
            let (stdout, stderr) = read_shell_output(capture, out_task, err_task).await;
            (status.code(), stdout, stderr, false, estop_killed)
        }
        Ok(Err(e)) => {
            guard.mark_complete();
            (
                None,
                String::new(),
                format!("wait failed: {e}"),
                false,
                false,
            )
        }
        Err(_) => {
            // 超时：先收尸再读残余输出（同款——管道任务持所有权不受
            // 取消影响，kill 后 EOF 收尾；文件形态读回被杀前已写内容）。
            // 收尸升级为树杀（背景见 `tree_kill` 注记）——
            // 本工具声明 timeout 参数，cargo 树不真死 = 参数没落地（产品
            // 验证实验 #1：声明 600s，超时后 cargo 又跑了 25 分钟）。树杀
            // 后仍 start_kill + wait 兜底直杀 shell 本体并收尸。：守卫
            // 先记超时原因（estop sweep 后到来的超时不得降级其标注）。
            guard.terminate(crate::exec_tree::TerminationReason::Timeout);
            if let Some(pid) = child.id() {
                crate::background_registry::tree_kill(pid).await;
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
            let estop_killed = guard.reason() == Some(crate::exec_tree::TerminationReason::Estop);
            guard.mark_complete();
            let (stdout, stderr) = read_shell_output(capture, out_task, err_task).await;
            (None, stdout, stderr, true, estop_killed)
        }
    }
}

/// Execute a script with a caller-chosen interpreter (`interpreter [flag] script`).
///
/// Registered as `"run_script"` and listed in [`MOVE_TOOLS`], so when executor
/// separation is enabled it runs in the executor subprocess (Layer 1) or the
/// Sandboxie box (Layer 2) — identical containment to `exec`. The workflow
/// `script` node delegates to this tool to gain sandbox awareness: the spawn is
/// byte-identical to `ScriptNodeExecutor`'s direct spawn (`Command:new(interp).
/// arg(flag).arg(script)`), so enabling the sandbox changes only *where* the
/// script runs, not *how*.
///
/// Unlike `exec` (which flattens output to a string for the LLM), this returns
/// STRUCTURED `{stdout, stderr, exit_code}` as a JSON string, so the workflow
/// `script` node preserves its `{stdout,stderr,exit_code,language}` output
/// contract. The interpreter + flag are passed in by the caller (the workflow
/// node resolves `language → (interpreter, flag)` via its own table), keeping
/// this tool generic and free of workflow-specific mapping.
pub struct RunScriptTool {
    workspace: String,
    restrict: bool,
}

impl RunScriptTool {
    /// Create a new run_script tool. `workspace` is the default cwd; `restrict`
    /// confines cwd to the workspace (mirrors [`ExecTool:new`]).
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: workspace.to_string(),
            restrict,
        }
    }
}

#[async_trait]
impl Tool for RunScriptTool {
    fn description(&self) -> String {
        "Run a script with a given interpreter (e.g. `bash -c <script>`, \
         `python3 -c <script>`, `cmd /C <script>`). Returns structured \
         {stdout, stderr, exit_code}."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "interpreter": {
                    "type": "string",
                    "description": "Interpreter executable (bash, sh, python3, node, cmd, powershell, ...)"
                },
                "flag": {
                    "type": "string",
                    "description": "Interpreter flag passing the script (e.g. -c, /C, -Command). Empty string for none."
                },
                "script": {
                    "type": "string",
                    "description": "The script source to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 60)"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory (default: workspace)"
                }
            },
            "required": ["interpreter", "script"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;

        let interpreter = val
            .get("interpreter")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'interpreter' argument")?;
        let flag = val.get("flag").and_then(|v| v.as_str()).unwrap_or("");
        let script = val
            .get("script")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'script' argument")?;
        let timeout_secs = val.get("timeout").and_then(|v| v.as_u64()).unwrap_or(60);
        let cwd = val
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.workspace);

        // Workspace restriction (mirrors ExecTool).
        if self.restrict {
            // 与 ExecTool（shell.rs）对齐：相对 cwd join workspace；双侧统一
            // 归一化（nemesis-path 单一真相源）——组件级比较防前缀误判，
            // workspace 侧归一化防 8.3 短名/大小写失配。
            let target = if Path::new(cwd).is_absolute() {
                PathBuf::from(cwd)
            } else {
                Path::new(&self.workspace).join(cwd)
            };
            let resolved = canonicalize_for_compare(&target);
            let ws = canonicalize_for_compare(Path::new(&self.workspace));
            if !resolved.starts_with(&ws) {
                return Err(format!(
                    "Access denied: path '{}' is outside workspace",
                    cwd
                ));
            }
        }

        // Spawn `interpreter [flag] script` — identical to ScriptNodeExecutor's
        // direct spawn. stdin=null + kill_on_drop so interactive prompts / hung
        // children don't survive the timeout.
        // Windows：裸 "bash"/"sh" 经 PATH 恒命中 System32 的 WSL launcher
        // （feature 启用但无发行版 = exit 1 空壳， CI 实证）→
        // 解析到 Git/MSYS2 的真 bash。单一真相源 nemesis-tools:shell。
        let interpreter = nemesis_tools::shell::resolve_posix_shell_path(interpreter);
        let mut cmd = tokio::process::Command::new(&interpreter);
        if !flag.is_empty() {
            cmd.arg(flag);
        }
        cmd.arg(script)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        // 环境凭据剥离：run_script 与 exec 同为 LLM
        // 驱动的 exec 类 spawn——脚本里 printenv 即可读出 agent 进程继承的
        // 凭据，同闸必须同源（`security.sanitize_child_env` 总开关）。
        let stripped = nemesis_utils::env_sanitize::sanitize_tokio_command(&mut cmd);
        if stripped > 0 {
            debug!("[Tools] env sanitized: {stripped} vars stripped");
        }
        // 树杀前提（Unix；同 make_piped_shell_command）：子进程
        // 自成一进程组（pgid == pid），超时收尸的 tree_kill 才能一锅端
        // 脚本派生的工作进程。
        #[cfg(not(target_os = "windows"))]
        {
            cmd.process_group(0);
        }
        // console flags 单一真相源（黑窗根修后默认臂即
        // CREATE_NO_WINDOW，此前的普通模式硬编码分叉收回——语义不变）。
        // workspace-dacl 受限模式下 CREATE_NO_WINDOW 的隐藏 console 也是新
        // 分配（\Device\ConDrv 写类访问）必死 0xC0000142（token_tests
        // grandchild 臂 b 实证）→ 附着成功=继承，未附着=DETACHED。
        #[cfg(target_os = "windows")]
        {
            if let Some(f) = console_creation_flags() {
                cmd.creation_flags(f);
            }
        }

        // ：workspace-dacl 受限模式下 output() 的隐式 piped
        // 必死（std 匿名管道创建受限令牌下被拒）→ spawn+wait+文件读回形态，
        // 结果契约（{stdout, stderr, exit_code} JSON）不变。
        let mut capture = ShellOutputCapture::current()
            .map_err(|e| format!("Failed to prepare executor output capture: {e}"))?;
        if let Some(c) = capture.as_mut() {
            c.apply_to(&mut cmd);
            // spawn 前置到 timeout 之外——超时收尸要有 pid 才能
            // 树杀（spawn 藏在 timed future 里 = 取消时句柄一并丢弃，只剩
            // kill_on_drop 直杀解释器本体，脚本派生的工作进程全部存活）。
            // spawn 即挂守卫 + 会话注册（estop sweep/drop 可
            // 收树）；结构化 JSON 契约不变。
            let (guard, mut child) = crate::exec_tree::ExecTreeGuard::attach_and_register(
                cmd.current_dir(cwd),
                Some(context.session_key.as_str()),
            )
            .map_err(|e| format!("Failed to execute script: {}", e))?;
            let waited =
                tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child.wait())
                    .await;
            return match waited {
                Ok(Ok(status)) => {
                    guard.mark_complete();
                    let (stdout, stderr) = capture.take().expect("capture present").read_back();
                    Ok(serde_json::json!({
                        "stdout": stdout,
                        "stderr": stderr,
                        "exit_code": status.code().unwrap_or(-1),
                    })
                    .to_string())
                }
                Ok(Err(e)) => {
                    guard.mark_complete();
                    Err(format!("Failed to execute script: {}", e))
                }
                Err(_) => {
                    // 守卫先记超时原因（estop sweep 后到来的超时不得
                    // 降级其标注）。
                    guard.terminate(crate::exec_tree::TerminationReason::Timeout);
                    if let Some(pid) = child.id() {
                        crate::background_registry::tree_kill(pid).await;
                    }
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    let _ = capture.take().expect("capture present").read_back();
                    guard.mark_complete();
                    Err(format!(
                        "Script timed out after {} seconds (interpreter: {}). It may be \
                     waiting for input (e.g. an interactive prompt).",
                        timeout_secs, interpreter
                    ))
                }
            };
        }

        // output() → spawn + 并发排水 + wait（exec/run_checks 同
        // 款 形态）。output() 的隐式 future 取消时只剩 kill_on_drop 直杀
        // 解释器本体；结构化出 pid 后超时走树杀。命令形态字节不变
        // （interpreter [flag] script 直 spawn 契约保持），只换捕获管道。
        // spawn 即挂守卫 + 会话注册（estop sweep/drop 可收树）。
        let (guard, mut child) = crate::exec_tree::ExecTreeGuard::attach_and_register(
            cmd.current_dir(cwd)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped()),
            Some(context.session_key.as_str()),
        )
        .map_err(|e| format!("Failed to execute script: {}", e))?;
        let out_task = tokio::spawn(drain_pipe(child.stdout.take()));
        let err_task = tokio::spawn(drain_pipe(child.stderr.take()));
        let waited =
            tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child.wait()).await;

        match waited {
            Ok(Ok(status)) => {
                guard.mark_complete();
                let stdout = String::from_utf8_lossy(&join_pipe_task(out_task).await).to_string();
                let stderr = String::from_utf8_lossy(&join_pipe_task(err_task).await).to_string();
                let exit_code = status.code().unwrap_or(-1);
                // Always Ok: encode success/failure in exit_code so the caller
                // (workflow script node) keeps its structured contract. Only
                // spawn/timeout failures are Err.
                Ok(serde_json::json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": exit_code,
                })
                .to_string())
            }
            Ok(Err(e)) => {
                guard.mark_complete();
                Err(format!("Failed to execute script: {}", e))
            }
            Err(_) => {
                // 守卫先记超时原因（estop sweep 后到来的超时不得降级
                // 其标注）。
                guard.terminate(crate::exec_tree::TerminationReason::Timeout);
                if let Some(pid) = child.id() {
                    crate::background_registry::tree_kill(pid).await;
                }
                let _ = child.start_kill();
                let _ = child.wait().await;
                let _ = join_pipe_task(out_task).await;
                let _ = join_pipe_task(err_task).await;
                guard.mark_complete();
                Err(format!(
                    "Script timed out after {} seconds (interpreter: {}). It may be \
                     waiting for input (e.g. an interactive prompt).",
                    timeout_secs, interpreter
                ))
            }
        }
    }
}

/// A tool that executes shell commands asynchronously (starts and returns quickly).
///
/// Mirrors Go's `AsyncExecTool` which is registered as "exec_async" in the agent.
pub struct AsyncExecTool {
    workspace: String,
    restrict: bool,
}

impl AsyncExecTool {
    pub fn new(workspace: &str, restrict: bool) -> Self {
        Self {
            workspace: workspace.to_string(),
            restrict,
        }
    }
}

#[async_trait]
impl Tool for AsyncExecTool {
    fn description(&self) -> String {
        "Start applications asynchronously and return quickly. Use this for GUI apps (notepad, calc, etc.) or any program where you don't need to wait for exit.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"command":{"type":"string","description":"Command to start"},"working_dir":{"type":"string","description":"Working directory"},"wait_seconds":{"type":"integer","description":"Seconds to wait for startup confirmation (default 3, range 1-10)"}},"required":["command"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;

        let command = val
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or("Missing 'command' argument")?;

        let cwd = val
            .get("working_dir")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.workspace);

        let wait_secs = val
            .get("wait_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .clamp(1, 10);

        // Workspace restriction check
        if self.restrict {
            // 与 ExecTool（shell.rs）对齐：相对 cwd join workspace；双侧统一
            // 归一化（nemesis-path 单一真相源）——组件级比较防前缀误判，
            // workspace 侧归一化防 8.3 短名/大小写失配。
            let target = if Path::new(cwd).is_absolute() {
                PathBuf::from(cwd)
            } else {
                Path::new(&self.workspace).join(cwd)
            };
            let resolved = canonicalize_for_compare(&target);
            let ws = canonicalize_for_compare(Path::new(&self.workspace));
            if !resolved.starts_with(&ws) {
                return Err(format!(
                    "Access denied: path '{}' is outside workspace",
                    cwd
                ));
            }
        }

        let mut child = {
            #[cfg(target_os = "windows")]
            let mut c = {
                #[allow(unused_imports)]
                use std::os::windows::process::CommandExt;
                let mut c = tokio::process::Command::new("cmd");
                c.raw_arg(format!("/C {}", command));
                c
            };
            #[cfg(not(target_os = "windows"))]
            let mut c = {
                let mut c = tokio::process::Command::new("sh");
                c.arg("-c").arg(command);
                c
            };
            // ：workspace-dacl 受限模式下 std piped 必死
            //（ShellOutputCapture 注释）；本工具本就不读输出 → 直接 null。
            c.current_dir(cwd);
            // console 治理与五工具同源（黑窗根修：普通模式默认臂
            // 即 CREATE_NO_WINDOW；本工具不在 MOVE_TOOLS、gateway 从不把
            // exec_async 路由进受限 executor——受限臂纯防御）。
            #[cfg(target_os = "windows")]
            {
                if let Some(f) = console_creation_flags() {
                    c.creation_flags(f);
                }
            }
            // 环境凭据剥离：exec_async 不读输出 ≠
            // 不漏——子进程命令可自行把环境落盘/外传，同闸必须同源。
            let stripped = nemesis_utils::env_sanitize::sanitize_tokio_command(&mut c);
            if stripped > 0 {
                debug!("[Tools] env sanitized: {stripped} vars stripped");
            }
            if ShellOutputCapture::is_restricted() {
                c.stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
            } else {
                c.stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
            }
            .spawn()
            .map_err(|e| format!("Failed to start command: {}", e))?
        };

        // Wait briefly to confirm startup
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(wait_secs), child.wait()).await;

        match result {
            Ok(Ok(status)) => {
                if status.success() || status.code().is_none() {
                    // Still running (no exit code) or exited cleanly
                    Ok(format!(
                        "Command '{}' started and confirmed running",
                        command
                    ))
                } else {
                    Err(format!(
                        "Command '{}' exited prematurely with status: {}",
                        command, status
                    ))
                }
            }
            Ok(Err(e)) => Err(format!("Failed to wait for command: {}", e)),
            Err(_) => {
                // Timeout — process still running, which is the expected async case
                Ok(format!(
                    "Command '{}' started successfully (still running)",
                    command
                ))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CronTool - Job scheduling (mirrors Go's CronTool)
// ---------------------------------------------------------------------------
