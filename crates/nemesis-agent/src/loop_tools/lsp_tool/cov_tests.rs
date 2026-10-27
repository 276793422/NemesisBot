// loop_tools/lsp_tool.rs 覆盖率补充测试（execute 参数校验决策表 /
// registration_plan / gate_and_write 无闸直写路径）。
//
// 需要真实语言服务器的链路（manager.rename 的 WorkspaceEdit 计算、
// 诊断回读格式化）由 nemesis-lsp crate 与真机场景覆盖；本文件只打
// 无服务器依赖的决策面。

use super::*;
use crate::context::RequestContext;

fn cov_ctx() -> RequestContext {
    RequestContext::new("web", "chat-1", "covuser", "agent:main:session:covlsp")
}

async fn exec_err(tool: &LspTool, args: serde_json::Value) -> String {
    crate::loop_tools::Tool::execute(tool, &args.to_string(), &cov_ctx())
        .await
        .expect_err("validation failure must be an Err")
}

// ---------------------------------------------------------------------------
// execute 参数校验决策表（全部在触达语言服务器之前返回）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_rejects_invalid_op() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "teleport", "path": "/x.rs", "line": 0, "character": 0}),
    )
    .await;
    assert!(err.contains("Invalid 'op'"), "got: {err}");
    assert!(
        err.contains("definition | references"),
        "valid values listed: {err}"
    );
}

#[tokio::test]
async fn execute_rejects_missing_path() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "hover", "line": 0, "character": 0}),
    )
    .await;
    assert!(err.contains("Missing 'path'"), "got: {err}");
}

/// 相对路径按进程 cwd 解析（join 分支），随后在缺 line 处如实报错。
#[tokio::test]
async fn execute_resolves_relative_path_against_cwd() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "hover", "path": "rel/file.rs", "character": 0}),
    )
    .await;
    assert!(err.contains("Missing or non-integer 'line'"), "got: {err}");
}

#[tokio::test]
async fn execute_rejects_missing_character() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "hover", "path": "/x.rs", "line": 1}),
    )
    .await;
    assert!(
        err.contains("Missing or non-integer 'character'"),
        "got: {err}"
    );
}

/// LSP 位置是 u32——超界 u64 早拒（不做静默截断）。
#[tokio::test]
async fn execute_rejects_out_of_range_positions() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "hover", "path": "/x.rs", "line": u64::from(u32::MAX) + 1, "character": 0}),
    )
    .await;
    assert!(err.contains("'line' out of range"), "got: {err}");

    let err = exec_err(
        &tool,
        serde_json::json!({"op": "hover", "path": "/x.rs", "line": 0, "character": u64::from(u32::MAX) + 1}),
    )
    .await;
    assert!(err.contains("'character' out of range"), "got: {err}");
}

#[tokio::test]
async fn execute_rename_requires_new_name() {
    let tool = LspTool::new(None, None);
    let err = exec_err(
        &tool,
        serde_json::json!({"op": "rename", "path": "/x.rs", "line": 0, "character": 0}),
    )
    .await;
    assert!(err.contains("Missing 'new_name'"), "got: {err}");
}

// ---------------------------------------------------------------------------
// registration_plan（纯函数）
// ---------------------------------------------------------------------------

#[test]
fn registration_plan_truth_table() {
    assert!(LspTool::registration_plan(true, 3));
    assert!(
        !LspTool::registration_plan(true, 0),
        "no languages → no registration"
    );
    assert!(
        !LspTool::registration_plan(false, 3),
        "disabled → no registration"
    );
    assert!(!LspTool::registration_plan(false, 0));
}

// ---------------------------------------------------------------------------
// gate_and_write：无闸形态直接落盘（gate 缺席 ≠ gate 通过）
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
#[tokio::test]
async fn gate_and_write_without_security_writes_all_files() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.rs");
    let b = dir.path().join("b.rs");
    let files = vec![
        AppliedFileEdit {
            path: a.to_string_lossy().into_owned(),
            new_text: "fn a() {}".to_string(),
            edit_count: 2,
        },
        AppliedFileEdit {
            path: b.to_string_lossy().into_owned(),
            new_text: "fn b() {}".to_string(),
            edit_count: 1,
        },
    ];

    let (written, rejected) = gate_and_write(&files, None, None, &cov_ctx())
        .await
        .expect("ungated write succeeds");
    assert!(
        rejected.is_empty(),
        "无边界时拒绝清单必须为空: {rejected:?}"
    );
    assert_eq!(
        written,
        vec![
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned()
        ]
    );
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "fn a() {}");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "fn b() {}");

    // 写盘中途失败（第二个文件路径非法）→ Err，第一个已如实上报过。
    let bad = vec![AppliedFileEdit {
        path: dir
            .path()
            .join("no-such-dir")
            .join("x.rs")
            .to_string_lossy()
            .into_owned(),
        new_text: "x".to_string(),
        edit_count: 1,
    }];
    let err = gate_and_write(&bad, None, None, &cov_ctx())
        .await
        .expect_err("unwritable target must fail");
    assert!(
        err.contains("write") && err.contains("failed"),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// M3（2026-10-27 整改）：gate_and_write 工作区边界预分——越界文件进拒绝
// 清单不落盘，界内文件照写（部分成功语义）；相对路径以边界根为基点。
// ---------------------------------------------------------------------------

/// 无闸形态下的边界预分：界外绝对路径被拒（不落盘），界内文件照写。
#[test]
fn gate_and_write_boundary_rejects_out_of_workspace_files() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let inside = ws.path().join("in.rs");
    let out_of_ws = outside.path().join("out.rs");
    let files = vec![
        AppliedFileEdit {
            path: inside.to_string_lossy().into_owned(),
            new_text: "fn inside() {}".to_string(),
            edit_count: 1,
        },
        AppliedFileEdit {
            path: out_of_ws.to_string_lossy().into_owned(),
            new_text: "fn outside() {}".to_string(),
            edit_count: 1,
        },
    ];
    let boundary = crate::loop_tools::WorkspaceBoundary {
        root: ws.path().to_path_buf(),
        restrict: true,
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (written, rejected) = rt
        .block_on(gate_and_write(&files, None, Some(&boundary), &cov_ctx()))
        .expect("partial success must be Ok");
    assert_eq!(written, vec![inside.to_string_lossy().into_owned()]);
    assert_eq!(
        std::fs::read_to_string(&inside).unwrap(),
        "fn inside() {}",
        "界内文件必须落盘"
    );
    assert_eq!(rejected.len(), 1, "越界文件进拒绝清单: {rejected:?}");
    assert_eq!(rejected[0].0, out_of_ws.to_string_lossy().into_owned());
    assert!(
        rejected[0].1.contains("outside the workspace"),
        "拒绝原因须注明越界: {}",
        rejected[0].1
    );
    assert!(
        !out_of_ws.exists(),
        "越界文件绝不能落盘（快照语义：目标不存在）"
    );
}

/// 相对路径以边界根为解析基点（与文件工具同源），不是进程 cwd。
#[test]
fn gate_and_write_boundary_resolves_relative_paths_against_root() {
    let ws = tempfile::tempdir().unwrap();
    // rename 写回既有文件的父目录本来就存在（模型语义）；这里显式摆出
    // src/ 夹具——gate_and_write 不代建父目录（那是 write_file 的语义）。
    std::fs::create_dir_all(ws.path().join("src")).unwrap();
    let files = vec![AppliedFileEdit {
        path: "src/rel.rs".to_string(),
        new_text: "fn rel() {}".to_string(),
        edit_count: 1,
    }];
    let boundary = crate::loop_tools::WorkspaceBoundary {
        root: ws.path().to_path_buf(),
        restrict: true,
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (written, rejected) = rt
        .block_on(gate_and_write(&files, None, Some(&boundary), &cov_ctx()))
        .expect("relative in-workspace write succeeds");
    assert!(rejected.is_empty());
    assert_eq!(written.len(), 1);
    let landed = ws.path().join("src").join("rel.rs");
    assert_eq!(
        std::fs::read_to_string(&landed).unwrap(),
        "fn rel() {}",
        "相对路径必须落在边界根下（落点={written:?}）"
    );
}

// ---------------------------------------------------------------------------
// BUG-20261025-37 回归锁：guardian 语义二审接入合成写闸（与 dispatch
// 同源 guardian_gate）。此前 guardian 只活在 dispatch 内联块，rename
// 合成闸只跑 8 层规则管线——guardian_mode=high 下同内容直写会被送审、
// rename 直落，文档声称「guardian 按既有语义继承」不实。
// ---------------------------------------------------------------------------

/// 恒 deny 的 mock judge（recommendation="deny"）。
struct DenyJudge;

#[async_trait::async_trait]
impl nemesis_security::guardian::LlmJudge for DenyJudge {
    async fn judge(
        &self,
        _req: &nemesis_security::guardian::JudgeRequest,
    ) -> Result<nemesis_security::guardian::JudgeVerdict, String> {
        Ok(nemesis_security::guardian::JudgeVerdict {
            intent: "writes destructive content".to_string(),
            matches_rules: true,
            risk_level: "high".to_string(),
            recommendation: "deny".to_string(),
            rationale: "content matches destructive patterns".to_string(),
        })
    }
}

/// 恒 allow 的 mock judge。
struct AllowJudge;

#[async_trait::async_trait]
impl nemesis_security::guardian::LlmJudge for AllowJudge {
    async fn judge(
        &self,
        _req: &nemesis_security::guardian::JudgeRequest,
    ) -> Result<nemesis_security::guardian::JudgeVerdict, String> {
        Ok(nemesis_security::guardian::JudgeVerdict {
            intent: "benign rename write".to_string(),
            matches_rules: false,
            risk_level: "low".to_string(),
            recommendation: "allow".to_string(),
            rationale: "no destructive pattern".to_string(),
        })
    }
}

/// 计数 judge：guardian 关闭时绝不应被调用（调用即测试失败）。
struct NeverJudge(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl nemesis_security::guardian::LlmJudge for NeverJudge {
    async fn judge(
        &self,
        _req: &nemesis_security::guardian::JudgeRequest,
    ) -> Result<nemesis_security::guardian::JudgeVerdict, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(nemesis_security::guardian::JudgeVerdict {
            intent: String::new(),
            matches_rules: false,
            risk_level: "low".to_string(),
            recommendation: "allow".to_string(),
            rationale: String::new(),
        })
    }
}

/// guardian_mode=high + judge 判 deny + 无审批通道 → fail-closed 拦停，
/// **一个字节都不落盘**（两阶段：guardian 属 gate 阶段，先于全部写盘）。
/// 内容带破坏词（`rm`）确保 destructive_shaped 预筛命中、真正进 LLM。
///
/// `flavor = "multi_thread"`：gate_and_write 带 plugin 时走 security.execute()
/// 八层管线，Layer 7 scanner 用 `block_in_place`（多线程 runtime 专属，current-thread
/// 上 panic）——与 nemesis-security 自家 pipeline 测试的运行时形态同款。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gate_and_write_guardian_flag_no_channel_blocks_all_files() {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    let plugin = SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        command_guard_enabled: false,
        injection_enabled: false,
        credential_enabled: false,
        dlp_enabled: false,
        ssrf_enabled: false,
        default_action: "allow".to_string(),
        ..Default::default()
    });
    plugin.set_guardian_mode("high");
    plugin.set_judge(std::sync::Arc::new(DenyJudge));

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("a.rs");
    std::fs::write(&target, "old\n").unwrap();
    let files = vec![AppliedFileEdit {
        path: target.to_string_lossy().into_owned(),
        // args JSON 全文会被词表扫（无 command 字段）——" rm " 命中词边界。
        new_text: "let x = command(\"rm -rf /\");\n".to_string(),
        edit_count: 1,
    }];

    let err = gate_and_write(&files, Some(&plugin), None, &cov_ctx())
        .await
        .expect_err("guardian flag + no approval channel must fail closed");
    assert!(
        err.contains("GUARDIAN FLAGGED") && err.contains("no files were modified"),
        "got: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "old\n",
        "blocked rename write must leave the file untouched"
    );
}

/// guardian_mode=high + judge 判 allow → 正常落盘（审查通过路径不误伤）。
/// multi_thread flavor 理由同上（Layer 7 block_in_place）。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gate_and_write_guardian_high_allow_writes_files() {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    let plugin = SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        command_guard_enabled: false,
        injection_enabled: false,
        credential_enabled: false,
        dlp_enabled: false,
        ssrf_enabled: false,
        default_action: "allow".to_string(),
        ..Default::default()
    });
    plugin.set_guardian_mode("high");
    plugin.set_judge(std::sync::Arc::new(AllowJudge));

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("a.rs");
    let files = vec![AppliedFileEdit {
        path: target.to_string_lossy().into_owned(),
        new_text: "fn renamed() {}\n".to_string(),
        edit_count: 1,
    }];

    let (written, rejected) = gate_and_write(&files, Some(&plugin), None, &cov_ctx())
        .await
        .expect("guardian allow must proceed to disk");
    assert!(rejected.is_empty());
    assert_eq!(written.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "fn renamed() {}\n"
    );
}

/// guardian_mode 未配置（默认关）→ judge 一次都不被调用、直接落盘
/// （钉死默认行为不变：guardian 接入不给默认配置添 LLM 成本/误拦）。
/// multi_thread flavor 理由同上（Layer 7 block_in_place）。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gate_and_write_guardian_default_off_never_calls_judge() {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    let plugin = SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        command_guard_enabled: false,
        injection_enabled: false,
        credential_enabled: false,
        dlp_enabled: false,
        ssrf_enabled: false,
        default_action: "allow".to_string(),
        ..Default::default()
    });
    let judge = std::sync::Arc::new(NeverJudge(std::sync::atomic::AtomicUsize::new(0)));
    plugin.set_judge(judge.clone());

    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("a.rs");
    let files = vec![AppliedFileEdit {
        path: target.to_string_lossy().into_owned(),
        new_text: "fn a() {}\n".to_string(),
        edit_count: 1,
    }];

    let (written, rejected) = gate_and_write(&files, Some(&plugin), None, &cov_ctx())
        .await
        .expect("guardian off must write without review");
    assert!(rejected.is_empty());
    assert_eq!(written.len(), 1);
    assert_eq!(
        judge.0.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "guardian default-off must never consult the judge"
    );
}

// ===========================================================================
// Wave4 覆盖批次：真实 rust-analyzer 端到端（execute_rename 全链路 /
// code_action 渲染 / hover 格式化）。SKIP 仅限「预检确认 rust-analyzer
// 不在」（--version 实跑探针，二进制在场 ≠ 可用）；预检通过后的就绪
// 失败、总预算耗尽、真实操作错误一律 fail（§四.1：真实错误不能全当
// SKIP）。CI（nextest）下三用例由 .config/nextest.toml 的 ra-serialize
// 组跨进程串行（CI 未预配 RA → 走合法 SKIP 臂）；本文件内 Semaphore
// 只对本地 cargo test 生效。
// ===========================================================================

/// 一次性 cargo 脚手架：零依赖，rust-analyzer 冷启动无需网络。
const RA_LIB_SRC: &str = "pub fn cov_target_fn() -> i32 {\n    42\n}\n\npub fn cov_caller() -> i32 {\n    cov_target_fn()\n}\n";

/// 预检：rust-analyzer 用自身实跑 trivial 探针（`--version`），不是纯
/// PATH 字符串匹配——二进制在场 ≠ 可用。缺席 = 唯一合法 SKIP 条件。
fn rust_analyzer_available() -> bool {
    std::process::Command::new("rust-analyzer")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 三个真实 rust-analyzer 用例共用的 SKIP 前置（唯一合法 SKIP）。
fn skip_unless_ra_available() -> bool {
    if rust_analyzer_available() {
        return true;
    }
    eprintln!("SKIP: rust-analyzer not available (--version probe failed)");
    false
}

fn scratch_ra_project() -> Option<(tempfile::TempDir, std::path::PathBuf)> {
    let dir = tempfile::tempdir().ok()?;
    std::fs::create_dir_all(dir.path().join("src")).ok()?;
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"cov_scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .ok()?;
    let lib = dir.path().join("src").join("lib.rs");
    std::fs::write(&lib, RA_LIB_SRC).ok()?;
    Some((dir, lib))
}

/// 三个真实 rust-analyzer 用例共用一把静态闸（P2-2）：并发跑会同时拉起
/// 多个 rust-analyzer（每个冷启动数百 MB 内存 + 全量分析 CPU），慢机上
/// 互相抢资源全部超时 SKIP。串行 = 后续用例吃到前一用例焐热的页缓存，
/// 预算更稳、结果更可复现。
static RA_SERIAL: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// 重试总预算（重试循环的墙钟总闸，P2-2）：旧实现 120 次循环名义「60s 预算」，
/// 实际单次 execute 自身可拖 60s（LspTool 内层超时），最坏总墙钟
/// 120×60s+60s——RA 卡死时套件挂半小时级。总闸一次包全程：每次尝试
/// （含 didOpen/touch_file）的等待钳制为剩余预算，耗尽即 Err（调用方
/// fail-closed，不再 SKIP）。§四.1：首次装配与重试的 touch_file 同样
/// 纳入本闸——初始化/didOpen/重试/清理全部受同一 deadline 约束。
const RA_TOTAL_BUDGET: std::time::Duration = std::time::Duration::from_secs(90);
const RA_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// didOpen 注册（touch_file）受总闸约束的包裹：等待钳制为剩余预算。
async fn touch_within_budget(
    tool: &LspTool,
    lib: &std::path::Path,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return Err(format!(
            "rust-analyzer never became ready within {}s total budget",
            RA_TOTAL_BUDGET.as_secs()
        ));
    }
    match tokio::time::timeout(remaining, tool.manager().touch_file(lib)).await {
        Ok(_) => Ok(()),
        Err(_) => Err(format!(
            "rust-analyzer never became ready within {}s total budget (didOpen hit the wall)",
            RA_TOTAL_BUDGET.as_secs()
        )),
    }
}

/// RA 就绪重试：先 didOpen 把文档注册进 vfs（touch_file，同受总闸），
/// 再执行 op；「file not found」/「No references found」/ null WorkspaceEdit /
/// null hover 都是工作区尚未分析完成的瞬态 → 500ms 后重试，总预算
/// （[`RA_TOTAL_BUDGET`]，全流程墙钟总闸）耗尽 → Err（调用方 fail-closed）。
/// 真实错误立即透传。
async fn ra_execute_with_retry(
    tool: &LspTool,
    lib: &std::path::Path,
    args: &serde_json::Value,
) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + RA_TOTAL_BUDGET;
    let args_str = args.to_string();
    touch_within_budget(tool, lib, deadline).await?;
    loop {
        // 每次尝试的可用时间 = 剩余总预算（内层 LspTool 60s 只是单请求
        // 护栏；真正的上限由这里钳制——任何一次尝试都不许越过总闸）。
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "rust-analyzer never became ready within {}s total budget",
                RA_TOTAL_BUDGET.as_secs()
            ));
        }
        let outcome = match tokio::time::timeout(
            remaining,
            LspTool::execute(tool, &args_str, &cov_ctx()),
        )
        .await
        {
            Ok(o) => o,
            Err(_) => {
                return Err(format!(
                    "rust-analyzer never became ready within {}s total budget (final attempt hit the wall)",
                    RA_TOTAL_BUDGET.as_secs()
                ));
            }
        };
        // hover 的空结果走 Ok 臂（format_result 对 null 悬停渲染成诚实文案）
        // ——同属「分析未就绪」的瞬态，一并重试；Ok 其余内容与真实错误
        // 立即返回。
        let retry = match &outcome {
            Ok(o) => o.contains("no hover information at this position"),
            Err(e) => {
                e.contains("file not found")
                    || e.contains("closed the stream")
                    || e.contains("timed out")
                    || e.contains("No references found")
                    || e.contains("server returned no edits")
                    || e.contains("no hover information at this position")
            }
        };
        if !retry {
            return outcome;
        }
        touch_within_budget(tool, lib, deadline).await?;
        // 退避间隔钳制在剩余预算内（剩余不足时立即回到总闸判定收口）。
        let pause =
            RA_RETRY_INTERVAL.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        if !pause.is_zero() {
            tokio::time::sleep(pause).await;
        }
    }
}

/// rename 全链路（lsp_tool.rs 218-300）：服务器算 WorkspaceEdit → 读盘应用
/// → 无闸直写 → notify_change → 诊断回读 → 格式化输出。落盘内容断言改名
/// 同时命中定义与调用两点。
#[tokio::test]
async fn execute_rename_end_to_end_with_rust_analyzer() {
    let _ra_permit = RA_SERIAL
        .acquire()
        .await
        .expect("RA_SERIAL semaphore closed");
    if !skip_unless_ra_available() {
        return;
    }
    let Some((_dir, lib)) = scratch_ra_project() else {
        eprintln!("SKIP: tempdir unavailable");
        return;
    };
    let tool = LspTool::new(Some(60), Some(60));
    let args = serde_json::json!({
        "op": "rename",
        "path": lib.to_string_lossy(),
        "line": 0,
        "character": 10,
        "new_name": "cov_renamed_fn",
    });
    // 预检已确认 RA 在场：就绪失败/预算耗尽/操作错误 = 测试失败（fail-closed）。
    let out = ra_execute_with_retry(&tool, &lib, &args)
        .await
        .unwrap_or_else(|e| panic!("rust-analyzer rename failed after precheck: {e}"));
    assert!(out.contains("Renamed to"), "{out}");
    assert!(out.contains("1 file(s) written"), "{out}");
    let after = std::fs::read_to_string(&lib).unwrap();
    assert_eq!(
        after.matches("cov_renamed_fn").count(),
        2,
        "定义与调用两点都要改名：{after}"
    );
    assert!(!after.contains("cov_target_fn"), "旧名必须清零：{after}");
    // 显式收尾：优雅关停 rust-analyzer（成功路径的确定性子进程清理；
    // SKIP 分支靠 LspTool drop 的 kill_on_drop 兜底——manager 测试已
    // 钉死超时/死亡两条关闭路径均无子进程残留）。
    let _ = tool.manager().shutdown_all().await;
}

/// code_action 渲染臂（169-201）：干净代码上 rust-analyzer 通常无 quickfix
/// → 诚实空文案；若有则必须带「listing only」尾注。两种形态都算过渲染路径。
#[tokio::test]
async fn execute_code_action_reports_honestly_with_rust_analyzer() {
    let _ra_permit = RA_SERIAL
        .acquire()
        .await
        .expect("RA_SERIAL semaphore closed");
    if !skip_unless_ra_available() {
        return;
    }
    let Some((_dir, lib)) = scratch_ra_project() else {
        eprintln!("SKIP: tempdir unavailable");
        return;
    };
    let tool = LspTool::new(Some(60), Some(60));
    let args = serde_json::json!({
        "op": "code_action",
        "path": lib.to_string_lossy(),
        "line": 0,
        "character": 10,
    });
    let out = ra_execute_with_retry(&tool, &lib, &args)
        .await
        .unwrap_or_else(|e| panic!("rust-analyzer code_action failed after precheck: {e}"));
    assert!(
        out.contains("no quickfix code actions") || out.contains("code action(s) available"),
        "{out}"
    );
    if out.contains("code action(s) available") {
        assert!(out.contains("listing only"), "{out}");
    }
    let _ = tool.manager().shutdown_all().await;
}

/// hover 只读链路（query → format_result）：真实服务器返回签名悬停。
/// 名字带 `with_rust_analyzer` 尾段——nextest ra-serialize 组按该子串
/// 捕获，真实 RA 用例必须沿用此命名约定才会在 CI 下串行。
#[tokio::test]
async fn execute_hover_formats_real_server_response_with_rust_analyzer() {
    let _ra_permit = RA_SERIAL
        .acquire()
        .await
        .expect("RA_SERIAL semaphore closed");
    if !skip_unless_ra_available() {
        return;
    }
    let Some((_dir, lib)) = scratch_ra_project() else {
        eprintln!("SKIP: tempdir unavailable");
        return;
    };
    let tool = LspTool::new(Some(60), Some(60));
    let args = serde_json::json!({
        "op": "hover",
        "path": lib.to_string_lossy(),
        "line": 0,
        "character": 10,
    });
    let out = ra_execute_with_retry(&tool, &lib, &args)
        .await
        .unwrap_or_else(|e| panic!("rust-analyzer hover failed after precheck: {e}"));
    assert!(!out.trim().is_empty(), "hover 不得为空：{out}");
    assert!(
        out.contains("cov_target_fn") || out.contains("i32") || out.contains("Hover"),
        "悬停应包含符号信息：{out}"
    );
    let _ = tool.manager().shutdown_all().await;
}
