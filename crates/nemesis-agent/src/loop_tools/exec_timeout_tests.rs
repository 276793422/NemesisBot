//! （2026-09-04 devtool-upgrade）：exec 超时收尸保留残余输出 +
//! 默认/上限 cap 的单元与集成测试。

use crate::context::RequestContext;
use crate::r#loop::Tool;
use crate::loop_tools::ExecTool;

fn tool() -> ExecTool {
    ExecTool::new(".", false)
}

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

// ---------------------------------------------------------------------------
// exec_timeout_secs 纯函数层（cap 语义的编译器锚点）
// ---------------------------------------------------------------------------

#[test]
fn timeout_defaults_to_30() {
    assert_eq!(super::exec_timeout_secs(None), 30);
}

#[test]
fn timeout_passthrough_under_cap() {
    assert_eq!(super::exec_timeout_secs(Some(5)), 5);
    assert_eq!(super::exec_timeout_secs(Some(600)), 600);
}

#[test]
fn timeout_capped_at_600() {
    // 模型传 999999 不再被照单全收。
    assert_eq!(super::exec_timeout_secs(Some(999_999)), 600);
    assert_eq!(super::exec_timeout_secs(Some(u64::MAX)), 600);
}

// ---------------------------------------------------------------------------
// exec_timeout_secs_capped 纯函数层（2026-11-08 config 化：
// `tools.exec.max_timeout_secs` 部署主权上限）
// ---------------------------------------------------------------------------

#[test]
fn timeout_capped_custom_cap_clamps() {
    // cap=2：模型请求 100 也被 clamp 到 2（部署上限优先于模型请求）。
    assert_eq!(super::exec_timeout_secs_capped(Some(100), 2), 2);
    assert_eq!(super::exec_timeout_secs_capped(Some(5), 2), 2);
    // cap=1800（部署放宽）：600 旧写死上限不再是天花板。
    assert_eq!(super::exec_timeout_secs_capped(Some(1000), 1800), 1000);
    assert_eq!(super::exec_timeout_secs_capped(Some(999_999), 1800), 1800);
    // 缺省 30 也受 cap 钳制（cap 是绝对天花板——否则 cap<30 的部署可被
    // 缺省值绕过；config 侧 effective_max_timeout_secs 的 30 下限保护
    // 保证经 config 路径 cap 恒 ≥30，此处 unit 钉纯函数语义）。
    assert_eq!(super::exec_timeout_secs_capped(None, 2), 2);
    assert_eq!(super::exec_timeout_secs_capped(None, 1800), 30);
}

// ---------------------------------------------------------------------------
// schema description 动态上限（部署侧可见性）
// ---------------------------------------------------------------------------

#[test]
fn schema_description_reflects_cap() {
    // 默认构造：600 旧行为文案。
    assert!(
        tool().parameters()["properties"]["timeout"]["description"]
            .as_str()
            .unwrap()
            .contains("max 600"),
        "default cap text must stay 600"
    );
    // 链式覆写后 description 同步（模型能看到当前部署的真实上限）。
    let capped = tool().with_max_timeout_secs(1800);
    assert!(
        capped.parameters()["properties"]["timeout"]["description"]
            .as_str()
            .unwrap()
            .contains("max 1800"),
        "custom cap must surface in schema description"
    );
}

// ---------------------------------------------------------------------------
// execute 集成层：部署上限压制模型请求（config 化端到端）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deploy_cap_overrides_model_request() {
    // cap=2 + 模型请求 timeout=100：实际在 2s 收尸（部署上限优先）。
    // SPIN_CMD 纯 shell 内建死循环无孙进程（见 PARTIAL_CMD 注记）。
    let out = tool()
        .with_max_timeout_secs(2)
        .execute(
            &serde_json::json!({"command": SPIN_CMD, "timeout": 100}).to_string(),
            &ctx(),
        )
        .await
        .expect("timeout must be Ok");
    assert!(
        out.contains("timed out after 2 seconds"),
        "deploy cap must clamp model's 100s request to 2s: {out}"
    );
}

#[tokio::test]
async fn default_cap_unchanged_byte_identical_behavior() {
    // 默认构造（无 config 注入）：模型请求 999999 仍被 clamp 到 600 语义
    // ——这里用 2s 请求验证正常透传（不真等 600s），上限 clamp 行为由
    // 纯函数层测试覆盖；本测试钉「默认构造路径零行为漂移」。
    let out = tool()
        .execute(
            &serde_json::json!({"command": SPIN_CMD, "timeout": 1}).to_string(),
            &ctx(),
        )
        .await
        .expect("timeout must be Ok");
    assert!(
        out.contains("timed out after 1 seconds"),
        "default-construction path must behave exactly as before: {out}"
    );
}

// ---------------------------------------------------------------------------
// tail_chars 纯函数层（多字节安全尾部截取）
// ---------------------------------------------------------------------------

#[test]
fn tail_chars_short_input_passthrough() {
    assert_eq!(super::tail_chars("hello", 10), "hello");
}

#[test]
fn tail_chars_keeps_last_n_chars() {
    assert_eq!(super::tail_chars("abcdefgh", 3), "fgh");
}

#[test]
fn tail_chars_multibyte_safe() {
    // 中文字符按 char 切，不按字节切（str 字节切片会 panic）。
    let s = "一二三四五";
    assert_eq!(super::tail_chars(s, 2), "四五");
}

// ---------------------------------------------------------------------------
// execute 集成层：超时路径
// ---------------------------------------------------------------------------

/// 「先输出一行再死循环」命令——验证超时后残余 stdout 可读回。
///
/// 【为什么不能用 ping/sleep 等外部命令】exec 在 Windows 经 `cmd /C` 包裹、
/// Linux 经 `sh -c` 包裹，外部命令是 shell 的**孙进程**且以 bInheritHandles
/// 复制了包括测试 harness stdout 捕获管道在内的全部可继承句柄（`>nul` 重定向
/// 只改 std 句柄不影响句柄表继承）。kill 只杀 shell 不杀孙进程——孙进程活
/// 多久，harness 就等多久（本机 127.0.0.1 ICMP 限流时 `ping -n 30` 实测
/// ~148s，测试看似挂死）。纯 shell 内建死循环无孙进程：kill shell 即全部
/// 句柄关闭，无孤儿。
///
/// 【窗口为什么是 5 不是 1】BUG-20261110-050：断言的本质是「超时杀进程后
/// 残余 stdout 能读回」这个机制，与窗口长短无关——窗口只是给 `echo` 一段
/// 确定性完成的预算。取 1s 时在重载机上与 cmd shell 启动赛跑（全量 150s+
/// 慢轮实录：echo 未赶上窗口 → 残余为空 → 断言假红），按「不按通常够大给
/// 值」纪律给超限窗口。
#[cfg(windows)]
const PARTIAL_CMD: &str = "echo before-sleep & for /l %i in (0,0,1) do @rem";
#[cfg(not(windows))]
const PARTIAL_CMD: &str = "echo before-sleep; while :; do :; done";

/// 纯内建死循环、无任何输出——验证超时零残余的诚实文案。
#[cfg(windows)]
const SPIN_CMD: &str = "for /l %i in (0,0,1) do @rem";
#[cfg(not(windows))]
const SPIN_CMD: &str = "while :; do :; done";

#[tokio::test]
async fn timeout_returns_partial_output_as_ok() {
    let out = tool()
        .execute(
            &serde_json::json!({"command": PARTIAL_CMD, "timeout": 5}).to_string(),
            &ctx(),
        )
        .await
        .expect("timeout must be Ok (model-repairable), not Err");
    assert!(
        out.contains("timed out after 5"),
        "should report timeout: {out}"
    );
    // 非空洞断言：该序列只可能来自 stdout 残余段（「Command was:」回显在
    // 消息末尾，构不成这个前缀序列）。
    assert!(
        out.contains("Partial output:\nstdout (tail):\nbefore-sleep"),
        "residual stdout before the hang must survive: {out}"
    );
    assert!(out.contains("Tip:"), "should carry repair tip: {out}");
    assert!(out.contains("Command was:"), "should echo command: {out}");
}

#[tokio::test]
async fn timeout_with_no_output_reports_none() {
    let out = tool()
        .execute(
            &serde_json::json!({"command": SPIN_CMD, "timeout": 1}).to_string(),
            &ctx(),
        )
        .await
        .expect("timeout must be Ok");
    assert!(
        out.contains("(no output before timeout)"),
        "empty residual must say so: {out}"
    );
}

// ---------------------------------------------------------------------------
// execute 集成层：正常路径回归（消息字节不变）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn normal_success_path_unchanged() {
    #[cfg(windows)]
    let cmd = "echo hello-b2";
    #[cfg(not(windows))]
    let cmd = "echo hello-b2";
    let out = tool()
        .execute(&serde_json::json!({"command": cmd}).to_string(), &ctx())
        .await
        .expect("must succeed");
    assert_eq!(out.trim(), "hello-b2", "success message unchanged: {out}");
}

#[tokio::test]
async fn normal_failure_path_unchanged() {
    #[cfg(windows)]
    let cmd = "cmd /c exit 7";
    #[cfg(not(windows))]
    let cmd = "exit 7";
    let out = tool()
        .execute(&serde_json::json!({"command": cmd}).to_string(), &ctx())
        .await
        .expect("failure is Ok-shaped (exit code report)");
    assert!(
        out.starts_with("Exit code: 7"),
        "failure format unchanged: {out}"
    );
}

#[tokio::test]
async fn large_output_pipe_not_deadlocked() {
    // 设计点回归：并发排干管道——输出超 ~64KB 管道缓冲时子进程不能被
    // 墙钟顶破成假超时（只 wait 不读的老毛病）。120s：本测试区分的是
    // 「死锁（永不完成→超时）」与「完成（tail 到达）」，墙钟只兜死锁；
    // PowerShell 启动 + 2 万行在满载机（多套件并发/AV 扫描）可破 60s，
    // 按 CLAUDE.md 环境敏感纪律给超限值，不压负载 flake。
    #[cfg(windows)]
    let cmd = "powershell -NoProfile -Command \"1..20000 | ForEach-Object { \\\"line-{0:00000}\\\" -f $_ }\"";
    #[cfg(not(windows))]
    let cmd = "seq 1 20000 | sed 's/^/line-/'";
    let out = tool()
        .execute(
            &serde_json::json!({"command": cmd, "timeout": 120}).to_string(),
            &ctx(),
        )
        .await
        .expect("large output must complete, not deadlock into timeout");
    assert!(
        !out.contains("timed out"),
        "must NOT degrade into a timeout: {}..",
        &out.chars().take(200).collect::<String>()
    );
    assert!(out.contains("line-20000"), "tail of large output present");
}
