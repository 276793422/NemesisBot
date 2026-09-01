//! `crate::exec_world` 测试（U10 统一执行世界）。
//!
//! 覆盖：path_within_roots 语义（component 前缀/`..` 逃逸/不存在路径）、
//! check_writable 默认实现、env 清洗、DirectWorld 两车道（Spawn 受守卫
//! 直跑 + 工具车道诚实 Err）、超时非崩溃。

use std::collections::HashMap;
use std::path::PathBuf;

use super::{
    DirectWorld, ExecOp, ExecOutcome, ExecutionWorld, SpawnOp, ToolOp, guarded_direct_spawn,
    path_within_roots, sanitize_env,
};

fn tmp() -> PathBuf {
    let d = tempfile::tempdir().expect("tempdir");
    // keep()：守卫测试需要目录活到断言后（tempdir Drop 会删）。
    d.keep()
}

// ---------------------------------------------------------------------------
// path_within_roots
// ---------------------------------------------------------------------------

#[test]
fn within_roots_component_prefix_semantics() {
    let root = tmp();
    assert!(path_within_roots(&root.join("a").join("b.txt"), std::slice::from_ref(&root)));
    // 字符串前缀但不 component 前缀：C:\ws 不覆盖 C:\ws2\...
    let sibling = root.join("ws2");
    assert!(!path_within_roots(&sibling, &[root.join("ws")]));
}

#[test]
fn within_roots_dotdot_escape_rejected() {
    let root = tmp();
    let escape = root.join("sub").join("..").join("..").join("outside.txt");
    // root 的父目录在 root 之外 → 拒
    assert!(!path_within_roots(&escape, std::slice::from_ref(&root)));
    // 合法留在根内的 ..
    let inside = root.join("sub").join("..").join("inside.txt");
    assert!(path_within_roots(&inside, &[root]));
}

#[test]
fn within_roots_nonexistent_path_lexical_fallback() {
    let root = tmp();
    let not_yet = root.join("defs").join("new_workflow.yaml");
    assert!(!not_yet.exists());
    assert!(path_within_roots(&not_yet, std::slice::from_ref(&root)));
    assert!(!path_within_roots(
        &root.join("..").join("elsewhere.yaml"),
        &[root]
    ));
}

#[test]
fn within_roots_exact_root_itself() {
    let root = tmp();
    assert!(path_within_roots(&root, std::slice::from_ref(&root)));
}

#[cfg(target_os = "windows")]
#[test]
fn within_roots_representation_mismatch_short_name_ancestor() {
    // 回归（2026-09-01 CI 首次暴露）：path 不存在时旧实现纯词法回退，
    // 保留输入的原始表示；存在的 root 被 canonicalize 成卷上真实表示
    // （8.3 短名 → 长名、大小写归一）。CI runner 的 TEMP 在
    // `C:\Users\RUNNER~1\...`（用户名 runneradmin 短名化），本地短用户名
    // 无短名化 → 本地绿 CI 红。这里用大小写变体在同一机制上确定性复现：
    // NTFS 大小写不敏感 → 变体路径存在但表示不同，旧实现 component 前缀
    // 比较恒 false（根内写入被误拒）。
    let root = tmp();
    let name = root.file_name().unwrap().to_string_lossy().to_string();
    let flipped: String = name
        .chars()
        .map(|c| if c.is_ascii_alphabetic() && !c.is_ascii_uppercase() {
            c.to_ascii_uppercase()
        } else if c.is_ascii_uppercase() {
            c.to_ascii_lowercase()
        } else {
            c
        })
        .collect();
    assert_ne!(flipped, name, "tempdir name should contain a letter");
    let variant = root.parent().unwrap().join(flipped);
    assert!(path_within_roots(
        &variant.join("a").join("new.txt"),
        std::slice::from_ref(&root)
    ));
    // 表示差异不得影响逃逸判定。
    assert!(!path_within_roots(
        &variant.join("..").join("outside.txt"),
        std::slice::from_ref(&root)
    ));
}

// ---------------------------------------------------------------------------
// check_writable（trait 默认实现）
// ---------------------------------------------------------------------------

#[test]
fn check_writable_default_impl() {
    let root = tmp();
    let world = DirectWorld::new("test", vec![root.clone()], vec![root.clone()], super::SpawnSemantics::InProcess);
    assert!(world.check_writable(&root.join("x.jsonl")).is_ok());
    let denied = world.check_writable(&root.join("..").join("evil.jsonl"));
    assert!(denied.is_err(), "out-of-root write must be denied");
    assert!(
        denied.unwrap_err().contains("writable roots"),
        "denial should explain the guard"
    );
}

// ---------------------------------------------------------------------------
// env 清洗
// ---------------------------------------------------------------------------

#[test]
fn sanitize_env_strips_executor_internals_and_applies_overrides() {
    let mut base = HashMap::new();
    base.insert("NEMESISBOT_ROLE".to_string(), "executor".to_string());
    base.insert("NEMESISBOT_EXECUTOR_PIPE".to_string(), "\\\\.\\pipe\\x".to_string());
    base.insert("PATH".to_string(), "/bin".to_string());
    let mut overrides = HashMap::new();
    overrides.insert("MY_FLAG".to_string(), "1".to_string());

    let out = sanitize_env(&base, &overrides);
    assert!(!out.contains_key("NEMESISBOT_ROLE"));
    assert!(!out.contains_key("NEMESISBOT_EXECUTOR_PIPE"));
    assert_eq!(out.get("PATH").map(String::as_str), Some("/bin"));
    assert_eq!(out.get("MY_FLAG").map(String::as_str), Some("1"));
}

// ---------------------------------------------------------------------------
// guarded_direct_spawn（真子进程；平台无关的 echo/exit）
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn echo_ok() -> SpawnOp {
    SpawnOp { program: "sh".into(), args: vec!["-c".into(), "echo hi".into()], cwd: None, stdin: None, timeout_secs: Some(10) }
}
#[cfg(windows)]
fn echo_ok() -> SpawnOp {
    SpawnOp { program: "cmd".into(), args: vec!["/C".into(), "echo hi".into()], cwd: None, stdin: None, timeout_secs: Some(10) }
}

#[tokio::test]
async fn spawn_runs_and_captures_output() {
    let root = tmp();
    let out = guarded_direct_spawn(&echo_ok(), &[root]).await.expect("spawn ok");
    assert!(!out.failed());
    assert!(out.stdout.trim().contains("hi"), "stdout={}", out.stdout);
}

#[cfg(unix)]
fn slow_cmd() -> SpawnOp {
    SpawnOp { program: "sh".into(), args: vec!["-c".into(), "sleep 5".into()], cwd: None, stdin: None, timeout_secs: Some(1) }
}
#[cfg(windows)]
fn slow_cmd() -> SpawnOp {
    SpawnOp { program: "cmd".into(), args: vec!["/C".into(), "ping -n 5 127.0.0.1 >nul".into()], cwd: None, stdin: None, timeout_secs: Some(1) }
}

#[tokio::test]
async fn spawn_timeout_reports_timed_out_not_err() {
    let root = tmp();
    let out = guarded_direct_spawn(&slow_cmd(), &[root]).await.expect("timeout is an outcome, not Err");
    assert!(out.timed_out);
    assert!(out.failed());
}

#[tokio::test]
async fn spawn_cwd_outside_roots_denied_before_spawn() {
    let root = tmp();
    let other = tmp();
    let job = SpawnOp {
        program: "definitely-not-a-real-program".into(),
        args: vec![],
        cwd: Some(other),
        stdin: None,
        timeout_secs: Some(5),
    };
    // 守卫先于 spawn：不会到 spawn 错误。
    let err = guarded_direct_spawn(&job, &[root]).await.expect_err("must deny");
    assert!(err.contains("spawn roots"), "err={err}");
}

#[tokio::test]
async fn direct_world_tool_lane_is_honest_error() {
    let root = tmp();
    let world = DirectWorld::new("dw", vec![root.clone()], vec![root], super::SpawnSemantics::InProcess);
    assert!(!world.supports_tool_calls());
    let err = world
        .run(ExecOp::Tool(ToolOp { tool: "run_script".into(), args: "{}".into() }))
        .await
        .expect_err("no tool lane");
    assert!(err.contains("no executor tool lane"), "err={err}");
}

#[tokio::test]
async fn direct_world_spawn_lane_routes_to_guarded_spawn() {
    let root = tmp();
    let world = DirectWorld::new("dw", vec![root.clone()], vec![root], super::SpawnSemantics::InProcess);
    let out: ExecOutcome = world.run(ExecOp::Spawn(echo_ok())).await.expect("spawn");
    assert!(!out.failed());
}

#[test]
fn spawn_semantics_display() {
    assert_eq!(super::SpawnSemantics::SandboxBoxed.to_string(), "sandbox-boxed");
    assert_eq!(super::SpawnSemantics::ExecutorChild.to_string(), "executor-child");
    assert_eq!(super::SpawnSemantics::InProcess.to_string(), "in-process");
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：trait 默认
// supports_tool_calls、DirectWorld::name/spawn_semantics、cwd 合法 spawn、
// stdin 注写。（strip_verbatim / normalize_lexical 的直接覆盖随
// canonicalize_for_compare 收敛迁到 nemesis-path/src/paths/tests.rs，
// 2026-09-01 8.3 短名统一修复。）
// ---------------------------------------------------------------------------

#[test]
fn default_supports_tool_calls_is_false_for_minimal_world() {
    struct BareWorld;
    #[async_trait::async_trait]
    impl ExecutionWorld for BareWorld {
        fn name(&self) -> &str {
            "bare"
        }
        fn writable_roots(&self) -> Vec<PathBuf> {
            vec![]
        }
        fn spawn_semantics(&self) -> super::SpawnSemantics {
            super::SpawnSemantics::InProcess
        }
        async fn run(&self, _op: ExecOp) -> Result<ExecOutcome, String> {
            Err("bare world has no lanes".into())
        }
    }
    let w = BareWorld;
    assert!(!w.supports_tool_calls(), "trait 默认实现必须 false");
    assert_eq!(w.name(), "bare");
}

#[tokio::test]
async fn direct_world_name_and_spawn_semantics_accessors() {
    let root = tmp();
    let world = DirectWorld::new("dw-s6", vec![root.clone()], vec![root], super::SpawnSemantics::SandboxBoxed);
    assert_eq!(world.name(), "dw-s6");
    assert_eq!(world.spawn_semantics(), super::SpawnSemantics::SandboxBoxed);
    assert_eq!(world.writable_roots().len(), 1);
}

#[tokio::test]
async fn spawn_with_valid_cwd_and_stdin_roundtrip() {
    let root = tmp();
    // cwd 落在 spawn 根内 → current_dir 生效；stdin 注写后子进程回显
    #[cfg(windows)]
    let job = SpawnOp {
        program: "cmd".into(),
        args: vec!["/C".into(), "more".into()],
        cwd: Some(root.clone()),
        stdin: Some("s6-stdin-line".into()),
        timeout_secs: Some(20),
    };
    #[cfg(unix)]
    let job = SpawnOp {
        program: "sh".into(),
        args: vec!["-c".into(), "cat".into()],
        cwd: Some(root.clone()),
        stdin: Some("s6-stdin-line".into()),
        timeout_secs: Some(20),
    };
    let out = guarded_direct_spawn(&job, &[root]).await.expect("spawn ok");
    assert!(!out.failed(), "exit={:?} stderr={}", out.exit_code, out.stderr);
    assert!(out.stdout.contains("s6-stdin-line"), "stdin 必须写进子进程: {}", out.stdout);
}

// ---------------------------------------------------------------------------
// R5 覆盖率批次（2026-08-27）：SpawnOp.stdin 管道臂——子进程真实读入
// stdin（findstr/grep 过滤），钉住 write_all + shutdown 路径。
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn stdin_filter_cmd() -> SpawnOp {
    SpawnOp {
        program: "sh".into(),
        args: vec!["-c".into(), "grep needle".into()],
        cwd: None,
        stdin: Some("noise\nneedle-line\n".into()),
        timeout_secs: Some(10),
    }
}
#[cfg(windows)]
fn stdin_filter_cmd() -> SpawnOp {
    SpawnOp {
        program: "cmd".into(),
        args: vec!["/C".into(), "findstr needle".into()],
        cwd: None,
        stdin: Some("noise\r\nneedle-line\r\n".into()),
        timeout_secs: Some(10),
    }
}

#[tokio::test]
async fn spawn_pipes_stdin_to_child() {
    let root = tmp();
    let out = guarded_direct_spawn(&stdin_filter_cmd(), &[root])
        .await
        .expect("spawn ok");
    assert!(!out.failed(), "exit={:?} stderr={}", out.exit_code, out.stderr);
    assert!(
        out.stdout.trim().contains("needle-line"),
        "子进程必须真读到 stdin: stdout={}",
        out.stdout
    );
}
