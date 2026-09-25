// Wave-5 round-2 coverage tests for eval.rs helpers that the main tests.rs
// does not reach: run_phases head (up to the monitor-DLL gate), box_api_base,
// wait_with_timeout (real process handles), sbieini_set/clean_box with fake
// external tools, assess_and_report happy paths, exit_if_risk_flagged(false).
//
// Safety notes:
// - `sbieini` / `start_exe` are FAKE .cmd batch files (exit /b 0|3) — nothing
//   touches a real Sandboxie.ini or service.
// - run_phases is called only in an environment where the monitor DLL is
//   absent (dev shape): it bails at `monitor_dll_path()?` BEFORE
//   `launch_and_inject_with_env`, so no DLL injection ever happens.
// - RISK_EXIT_FLAG is only ever read while false; storing true would
//   exit(2) the test process and is never done here.
use super::*;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Fake external tool: a .cmd batch that exits with the given code.
/// std::process::Command runs .cmd files via cmd.exe /c on Windows.
fn fake_tool(dir: &std::path::Path, name: &str, exit_code: u8) -> std::path::PathBuf {
    let p = dir.join(format!("{name}.cmd"));
    std::fs::write(&p, format!("@echo off\r\nexit /b {exit_code}\r\n")).unwrap();
    p
}

// ---------------------------------------------------------------------------
// assess_and_report — happy paths (rules load + real assess + risk exit
// signal). The corrupted/zero-enabled paths live in tests.rs.
// ---------------------------------------------------------------------------

/// Assemble a complete, integrity-healthy eval report (assessor Step 0 passes)
/// plus a rules file with a single enabled subject rule.
fn seed_report(out_dir: &std::path::Path, subject: &str, response: &str) {
    std::fs::create_dir_all(out_dir).unwrap();
    std::fs::write(
        out_dir.join("meta.json"),
        serde_json::json!({
            "kind": "prompt",
            "worker_error": false,
            "agent_exit": 0,
            "monitor_shell_exit": 0,
            "final_response_len": response.len(),
            "tool_call_count": 0,
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(out_dir.join("driver_events.jsonl"), "").unwrap();
    std::fs::write(out_dir.join("tool_trace.json"), "[]").unwrap();
    std::fs::write(out_dir.join("subject.txt"), subject).unwrap();
    std::fs::write(out_dir.join("final_response.md"), response).unwrap();
}

fn write_rules(home: &std::path::Path, rule_value: &str) {
    let rules = crate::eval_assessor::rules_file_path(home);
    std::fs::create_dir_all(rules.parent().unwrap()).unwrap();
    std::fs::write(
        &rules,
        format!(
            r#"{{"rules": [{{
                "id": "cov-subject-rule",
                "description": "cov rule",
                "level": "high",
                "enabled": true,
                "source": "subject",
                "conditions": [{{"field": "text", "op": "contains", "value": "{rule_value}"}}]
            }}]}}"#
        ),
    )
    .unwrap();
}

#[test]
fn assess_and_report_risk_with_fail_on_risk_returns_true() {
    let home = tmp();
    write_rules(home.path(), "RISKY_MARKER");

    let out = tmp();
    let out_dir = out.path().join("report");
    seed_report(&out_dir, "RISKY_MARKER body", "did things");

    let risk_exit = assess_and_report(&out_dir, home.path(), true);
    assert!(risk_exit, "risk + --fail-on-risk → 退码信号 true");

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("assessment.json")).unwrap())
            .unwrap();
    assert_eq!(saved["conclusion"], "risk");
    assert_eq!(saved["matched_rules"][0]["id"], "cov-subject-rule");
    // meta.json 的 assessment 段同步（write_assessment 合并）。
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["assessment"]["conclusion"], "risk");
}

#[test]
fn assess_and_report_safe_with_fail_on_risk_returns_false() {
    let home = tmp();
    write_rules(home.path(), "NEVER_MATCHES");

    let out = tmp();
    let out_dir = out.path().join("report");
    seed_report(&out_dir, "benign subject", "all good");

    let risk_exit = assess_and_report(&out_dir, home.path(), true);
    assert!(!risk_exit, "安全不退 2（即使 --fail-on-risk）");

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("assessment.json")).unwrap())
            .unwrap();
    assert_eq!(saved["conclusion"], "safe");
}

#[test]
fn assess_and_report_risk_without_fail_on_risk_returns_false() {
    let home = tmp();
    write_rules(home.path(), "RISKY_MARKER");

    let out = tmp();
    let out_dir = out.path().join("report");
    seed_report(&out_dir, "RISKY_MARKER body", "did things");

    let risk_exit = assess_and_report(&out_dir, home.path(), false);
    assert!(!risk_exit, "risk 但未开 --fail-on-risk → 信号 false");
}

// ---------------------------------------------------------------------------
// exit_if_risk_flagged — 只在 flag=false 时调用（true 会 exit(2) 杀死进程）。
// ---------------------------------------------------------------------------

#[test]
fn exit_if_risk_flagged_noop_when_flag_clear() {
    RISK_EXIT_FLAG.store(false, std::sync::atomic::Ordering::Release);
    exit_if_risk_flagged(); // 不得 exit
}

// ---------------------------------------------------------------------------
// box_api_base — lane 分派：HttpCompat 保留 /v1，Anthropic/Codex 剥 /v1，
// 解析失败兜底 HttpCompat 形态。
// ---------------------------------------------------------------------------

#[test]
fn box_api_base_http_compat_keeps_v1() {
    // 显式 openai 协议（API key 缺省但显式协议路径不看 key）。
    assert_eq!(
        box_api_base("http://127.0.0.1:9/v1", "any/model", "openai"),
        "http://127.0.0.1:9/v1"
    );
}

#[test]
fn box_api_base_anthropic_and_codex_trim_v1() {
    // 前缀 anthropic（protocol 空）→ Anthropic lane → 剥 /v1。
    assert_eq!(
        box_api_base("http://127.0.0.1:9/v1", "anthropic/claude-3", ""),
        "http://127.0.0.1:9"
    );
    // 显式 protocol 覆盖 provider 前缀：responses → Codex → 剥 /v1。
    assert_eq!(
        box_api_base("http://127.0.0.1:9/v1", "zhipu/glm", "responses"),
        "http://127.0.0.1:9"
    );
    // 无 /v1 后缀时剥除是 no-op。
    assert_eq!(
        box_api_base("http://127.0.0.1:9", "anthropic/claude-3", ""),
        "http://127.0.0.1:9"
    );
}

#[test]
fn box_api_base_resolve_failure_falls_back_to_httpcompat_shape() {
    // 裸名 + 默认协议 + 空 key → resolve 报 no-API-key 错 → 兜底 HttpCompat
    // 形态（base 原样返回，不剥 /v1）。
    assert_eq!(
        box_api_base("http://127.0.0.1:9/v1", "glm-5.3-flash", ""),
        "http://127.0.0.1:9/v1"
    );
}

// ---------------------------------------------------------------------------
// wait_with_timeout / wait_timeout_ms — 真进程句柄。
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod handle_wait {
    use super::*;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, TerminateProcess,
    };

    fn open(pid: u32) -> windows_sys::Win32::Foundation::HANDLE {
        unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        }
    }

    #[tokio::test]
    async fn wait_with_timeout_returns_exit_code_of_exited_child() {
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "exit 7"])
            .spawn()
            .expect("spawn cmd");
        let h = open(child.id());
        assert!(!h.is_null(), "OpenProcess on live child");
        let code = wait_with_timeout(h, Duration::from_secs(15)).await;
        assert_eq!(code, Some(7), "子进程 exit 7 → GetExitCodeProcess 读到 7");
        let _ = child.wait();
        unsafe { CloseHandle(h) };
    }

    #[tokio::test]
    async fn wait_with_timeout_times_out_returns_none() {
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 5 127.0.0.1 >nul"]) // ~4s 存活
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleeper");
        let h = open(child.id());
        assert!(!h.is_null());
        let code = wait_with_timeout(h, Duration::from_millis(150)).await;
        assert_eq!(code, None, "未退出 + 超时 → None");
        unsafe { TerminateProcess(h, 1) };
        let _ = child.wait();
        unsafe { CloseHandle(h) };
    }
}

// ---------------------------------------------------------------------------
// sbieini_set / sbieini_append / clean_box — 假外部工具（不碰真 Sandboxie）。
// ---------------------------------------------------------------------------

#[test]
fn sbieini_set_success_with_fake_tool() {
    let root = tmp();
    let tool = fake_tool(root.path(), "SbieIni", 0);
    sbieini_set(&tool, "NemesisEvalBox_cov", "FileTrace", "*").expect("exit 0 → Ok");
}

#[test]
fn sbieini_set_nonzero_exit_bails() {
    let root = tmp();
    let tool = fake_tool(root.path(), "SbieIni", 3);
    let err =
        sbieini_set(&tool, "NemesisEvalBox_cov", "FileTrace", "*").expect_err("exit 3 → bail");
    assert!(err.to_string().contains("failed"), "err: {err:#}");
}

#[test]
fn sbieini_set_missing_tool_bails_with_context() {
    // 注意：缺失的 .cmd 在 Windows 上经 cmd.exe /c 代跑会以 exit code 1 落到
    // 非零退出分支；要覆盖 with_context 的 spawn 失败臂必须用缺失的 .exe。
    let ghost = std::env::temp_dir().join(format!("no_such_sbieini_{}.exe", std::process::id()));
    let err = sbieini_set(&ghost, "S", "K", "V").expect_err("缺工具 → Err");
    assert!(
        err.to_string().contains("run SbieIni set"),
        "spawn 失败必须带上下文: {err:#}"
    );
}

#[test]
fn sbieini_append_success_and_failure() {
    let root = tmp();
    let ok = fake_tool(root.path(), "SbieIniOk", 0);
    sbieini_append(&ok, "Box", "ClosedFilePath", "x").expect("exit 0 → Ok");
    let bad = fake_tool(root.path(), "SbieIniBad", 2);
    let err = sbieini_append(&bad, "Box", "ClosedFilePath", "x").expect_err("exit 2 → bail");
    assert!(err.to_string().contains("SbieIni append"), "err: {err:#}");
}

#[test]
fn clean_box_skips_when_box_root_missing() {
    let root = tmp();
    let tool = fake_tool(root.path(), "Start", 0);
    let ghost_root = root.path().join("no_such_box_root");
    // 不得 panic，也不得运行任何命令（box root 不存在直接返回）。
    clean_box(&tool, "NemesisEvalBox_cov", &ghost_root);
}

#[test]
fn clean_box_runs_delete_sandbox_when_root_exists() {
    let root = tmp();
    let tool = fake_tool(root.path(), "Start", 0);
    let box_root = root.path().join("box_root");
    std::fs::create_dir_all(&box_root).unwrap();
    clean_box(&tool, "NemesisEvalBox_cov", &box_root); // status 被忽略，只求不 panic
}

// ---------------------------------------------------------------------------
// run_phases — 头段（config 写入 / trace 开关 / clean_box / agent 伪 spawn），
// 在 monitor DLL 缺失的开发形态下于 monitor_dll_path()? 处 bail。
// launch_and_inject_with_env 永不触达。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_phases_writes_config_and_bails_at_missing_monitor_dll() {
    let real_home = tmp();
    let home = tmp();
    let workspace = tmp();
    let tools = tmp();
    let out = tmp();
    let out_dir = out.path().join("report");

    let sbieini = fake_tool(tools.path(), "SbieIni", 0);
    let start_exe = fake_tool(tools.path(), "Start", 0);
    let sbiectrl = tools.path().join("SbieCtrl.exe");
    let sbiedll = tools.path().join("SbieDLL.dll");
    let agent_exe = tools.path().join("agent.exe");
    // box root 不存在 → clean_box 走早退分支（不弹真对话框）。
    let eval_box_root = tools.path().join("box_root_missing");

    let common = EvalCommon {
        output: Some(out_dir),
        allow_network: false,
        observe_secs: 1,
        local: false,
        fail_on_risk: false,
    };

    let result = run_phases(
        "http://127.0.0.1:18080/v1",
        real_home.path(),
        "prompt",
        home.path(),
        workspace.path(),
        "testai-1.1",
        "test/testai-1.1",
        "http://127.0.0.1:19001/v1",
        "",
        &sbieini,
        "NemesisEvalBox_cov",
        &eval_box_root,
        &start_exe,
        &sbiectrl,
        &sbiedll,
        &common,
        "cov subject",
        "cov prompt",
        &agent_exe,
    )
    .await;

    let err = result.expect_err("monitor DLL 缺失 → run_phases 必须 bail");
    assert!(
        err.to_string().contains("monitor DLL"),
        "bail 原因应指向 monitor DLL 缺失: {err:#}"
    );

    // 头段副作用断言：最小 config.json 已写入盒内 home。
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(cfg["model_list"][0]["model_name"], "testai-1.1");
    assert_eq!(
        cfg["model_list"][0]["api_base"],
        "http://127.0.0.1:19001/v1"
    );
    assert_eq!(cfg["agents"]["defaults"]["llm"], "testai-1.1");
    assert_eq!(cfg["executor"]["enabled"], false);
}

// ---------------------------------------------------------------------------
// monitor_dll_path — 开发形态（exe 旁无 plugins\ 且仓库路径缺 DLL）→ bail。
// ---------------------------------------------------------------------------

#[test]
fn monitor_dll_path_dev_shape_missing_dll_bails() {
    // 测试 exe 在 target/<...>/deps/ 下，旁无 plugins\；仓库
    // plugins/plugin-eval-monitor/target/release/eval_monitor_dll.dll 未构建
    // （CI 外的开发机前提）。两者皆缺 → 明确报错而非静默回落。
    let err = monitor_dll_path().expect_err("DLL 缺失必须报错");
    assert!(
        err.to_string().contains("monitor DLL not found"),
        "err: {err:#}"
    );
}
