//! `nemesisbot eval` — prompt / skill security behaviour assessment.
//!
//! Plan: docs/PLAN/2026-08-14_eval-prompt-skill-security-assessment.md
//! Four-layer architecture (command process / LLM proxy / sandboxed agent /
//! monitor shell), all coordinated here (plan Step 6):
//!
//! 6a parse input + concurrent-eval lock
//! 6b read the real config → real key/base/model (memory only)
//! 6c temp dir (home=tmp, workspace=tmp/workspace)
//! 6d sanitized minimal config + unique-named agent exe + skill copy
//! 6e sandbox readiness check
//! 6f create the NemesisEvalBox section (restored on exit)
//! 6g start the LLM proxy (fake key inside, real key swapped here)
//! 6h enable trace for the eval box
//! 6i clean the eval box
//! 6j launch the monitor shell (signed host + injected monitor DLL)
//! 6k spawn the sandboxed agent
//! 6l wait for the monitor shell (it exits when the box empties)
//! 6m read monitor events (env-error marker → abort with reason)
//! 6n pending-file enumeration + agent report from the box mirror
//! 6o assemble the final report
//! 6p cleanup — real environment untouched (ini section restored)


// Platform isolation: everything below the CLI types is `#[cfg(windows)]`
// (see each item); on other targets only `run()` remains, which bails with a
// clear runtime error. Imports used solely by windows-gated code are gated
// too, so non-Windows builds stay warning-free.
use std::path::PathBuf;

#[cfg(target_os = "windows")]
use std::path::Path;
#[cfg(target_os = "windows")]
use std::process::Command;

use anyhow::{bail, Result};
use clap::{Args, Subcommand};

#[cfg(target_os = "windows")]
use anyhow::Context;

#[cfg(target_os = "windows")]
use crate::common;

#[derive(Subcommand)]
pub enum EvalAction {
    /// Evaluate a prompt by running the agent loop against its text.
    Prompt {
        /// The prompt text (or use --file).
        text: Option<String>,
        /// Read the prompt from a file instead.
        #[arg(long)]
        file: Option<PathBuf>,
        #[command(flatten)]
        common: EvalCommon,
    },
    /// Evaluate an installed skill by running it through the agent loop.
    Skill {
        /// Skill name (workspace → global → builtin resolution).
        name: String,
        #[command(flatten)]
        common: EvalCommon,
    },
}

#[derive(Args)]
pub struct EvalCommon {
    /// Output directory for the report. Default: <home>/workspace/logs/eval/<ts>_<slug>/
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Allow network access inside the eval box.
    #[arg(long)]
    pub allow_network: bool,
    /// Hard total-timeout fuse in seconds (the monitor shell normally ends
    /// when the box empties; this guards against resident background exes).
    #[arg(long, default_value_t = 1800)]
    pub observe_secs: u64,
    /// Use ./.nemesisbot as the real home (same as the global --local).
    #[arg(long)]
    pub local: bool,
}

pub async fn run(action: EvalAction, cli_local: bool) -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (action, cli_local); // silence unused params on non-Windows
        bail!("`nemesisbot eval` is Windows-only (requires Sandboxie).");
    }
    #[cfg(target_os = "windows")]
    {
        let (kind, subject, prompt_text, common) = match action {
            EvalAction::Prompt { text, file, common } => {
                let subject = match (&text, &file) {
                    (Some(t), _) => t.clone(),
                    (None, Some(f)) => std::fs::read_to_string(f)
                        .with_context(|| format!("read prompt file {}", f.display()))?,
                    (None, None) => bail!("provide the prompt text or --file <path>"),
                };
                let prompt_text = subject.clone();
                ("prompt".to_string(), subject, prompt_text, common)
            }
            EvalAction::Skill { name, common } => {
                let loader = skills_loader_for_current_home(cli_local || common.local)?;
                let body = loader
                    .load_skill(&name)
                    .with_context(|| format!("skill '{name}' not found (workspace/global/builtin)"))?;
                let subject = format!("# Skill: {name}\n\n{body}");
                let prompt_text = format!(
                    "Execute the skill '{name}' now. Use skills_info to read its full \
                     definition if needed, then follow it step by step."
                );
                ("skill".to_string(), subject, prompt_text, common)
            }
        };
        run_eval(&kind, subject, prompt_text, common, cli_local).await
    }
}

/// Build a SkillsLoader against the REAL home (skill resolution happens
/// before the temporary environment exists).
#[cfg(target_os = "windows")]
fn skills_loader_for_current_home(
    local: bool,
) -> Result<std::sync::Arc<nemesis_skills::loader::SkillsLoader>> {
    let home = common::resolve_home(local);
    let workspace = home.join("workspace");
    let ws = workspace.to_string_lossy().to_string();
    let global = workspace.join("skills").to_string_lossy().to_string();
    Ok(std::sync::Arc::new(nemesis_skills::loader::SkillsLoader::new(
        &ws, &global, "",
    )))
}

#[cfg(target_os = "windows")]
async fn run_eval(
    kind: &str,
    subject: String,
    prompt_text: String,
    common_args: EvalCommon,
    cli_local: bool,
) -> Result<()> {
    let local = cli_local || common_args.local;
    // Canonicalize: NEMESISBOT_HOME may use forward slashes, which breaks the
    // engine_owned substring match against the registered service binary
    // path. Strip the \\?\ verbatim prefix canonicalize adds (sc reports plain
    // paths; a verbatim runtime dir would never substring-match).
    let real_home = {
        let h = common::resolve_home(local);
        let c = h.canonicalize().unwrap_or(h);
        let s = c.to_string_lossy().to_string();
        let stripped = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
        PathBuf::from(stripped)
    };

    // 6b — real config → key/base/model (memory only, never written to disk).
    let real_cfg_path = real_home.join("config.json");
    let real_cfg = nemesis_config::load_config(&real_cfg_path)
        .with_context(|| format!("load real config {}", real_cfg_path.display()))?;
    let llm_ref = nemesis_config::get_effective_llm(Some(&real_cfg));
    let resolution = nemesis_config::resolve_model_config(&real_cfg, &llm_ref)
        .with_context(|| format!("resolve model '{llm_ref}'"))?;
    let real_base = resolution
        .api_base
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_string();
    let model_name = resolution.model_name.clone();
    // Full ref with provider prefix — what model_list[].model expects
    // (real config stores e.g. "deepseek/deepseek-v4-flash" there while
    // agents.defaults.llm holds the bare name).
    let model_ref = if resolution.provider_name.is_empty() {
        model_name.clone()
    } else {
        format!("{}/{}", resolution.provider_name, resolution.model_name)
    };
    println!("[eval] model={model_name} via proxy (real key stays in memory)");

    // 6c — temp home. TempDir dropped at the end deletes the box root too
    // (FileRootPath lives inside it).
    let tmp = tempfile::tempdir().context("create temp dir")?;
    let home = tmp.path().to_path_buf();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace)?;

    // 6d — sanitized minimal config + agent exe + subject file.
    let agent_exe = copy_agent_exe(&home)?;
    std::fs::write(workspace.join("subject.txt"), &subject)?;
    if kind == "skill" {
        copy_skill_dir(&real_home, &workspace, &subject)?;
    }

    // Sandbox paths (engine is system-level; runtime lives in the real home).
    let sandbox_root = real_home.join("workspace").join("tools").join("sandboxie");
    let runtime = sandbox_root.join("runtime");
    let start_exe = runtime.join("Start.exe");
    let sbiectrl = runtime.join("SbieCtrl.exe");
    let sbiedll = runtime.join("SbieDll.dll");
    let sbieini = runtime.join("SbieIni.exe");

    // 6e — sandbox readiness (three conditions, mirrors agent_factory).
    let paths = nemesis_sandbox::SandboxPaths::new(&real_home);
    let c1 = start_exe.exists();
    let c2 = matches!(
        nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE),
        nemesis_sandbox::status::ServiceState::Running
    );
    let c3 = nemesis_sandbox::status::engine_owned(&paths);
    if !(c1 && c2 && c3) {
        // Diagnostic detail so the user knows exactly what is wrong.
        bail!(
            "Sandbox engine not ready (start_exe={} svc_running={} engine_owned={}; \
             home={}). Run:\n  nemesisbot sandbox install\n  nemesisbot sandbox start\n\
             then retry. (eval never elevates)",
            c1,
            c2,
            c3,
            real_home.display()
        );
    }

    // 6f — create the eval box section (restored in the guard at the end).
    // The ConfigLevel / Template=SkipHook / AutoRecover / BlockNetworkFiles
    // keys are REQUIRED for Start.exe to launch anything in the box — without
    // them every spawn fails with ERROR_MOD_NOT_FOUND(126) + an error dialog
    // (verified by the NemesisBox-vs-eval-box key diff, 2026-08-15).
    // A UNIQUE section name per run: SbieSvc caches per-section state and
    // repeated create/delete of the SAME section left it in a stale
    // "refuses to start processes" state (the `eval skill` exit-1 bug,
    // 2026-08-16). A fresh name sidesteps the cache entirely. Still ≤33
    // chars (SbieApi_QueryProcess box[34] limit).
    let eval_box = format!("NemesisEvalBox_{}", std::process::id());
    let eval_box_root = home.join("box_root");
    // Do NOT pre-create the box root: Sandboxie creates it on first launch,
    // and a pre-created root makes delete_sandbox's internal rmdir fail with
    // Code 3 + a dialog (dialog-capture verified, 2026-08-16).
    let ini_backup = std::fs::read_to_string(sandbox_root.join("Sandboxie.ini"))?;

    // Guard: EVERYTHING that can fail after the ini section starts being
    // written goes inside this block. A mid-loop SbieIni failure must not
    // skip the ini restore below (section keys would stay in the REAL ini —
    // worse than a leaked temp dir). Cleanup after the block runs on success
    // and error paths alike.
    let result = async {
        for (k, v) in [
            ("Enabled", "y"),
            ("FileRootPath", &format!(r"\??\{}", eval_box_root.display())),
            ("DropAdminRights", "y"),
            ("AllowNetworkAccess", if common_args.allow_network { "y" } else { "n" }),
            ("ConfigLevel", "9"),
            ("Template", "SkipHook"),
            ("AutoRecover", "y"),
            ("BlockNetworkFiles", "y"),
        ] {
            sbieini_set(&sbieini, &eval_box, k, v)?;
        }

        // 6g — LLM proxy. The fake-key config is only complete once the
        // proxy port is known.
        let proxy = nemesis_eval_proxy::start(real_base, resolution.api_key.clone())
            .await
            .context("start eval LLM proxy")?;

        let eval_result = run_phases(
            &proxy,
            &real_home,
            &kind,
            &home,
            &workspace,
            &model_name,
            &model_ref,
            &sbieini,
            &eval_box,
            &eval_box_root,
            &start_exe,
            &sbiectrl,
            &sbiedll,
            &common_args,
            &subject,
            &prompt_text,
            &agent_exe,
        )
        .await;

        proxy.shutdown().await;
        eval_result
    }
    .await;

    // 6p — cleanup: box content, ini restore, temp dir removal — ALL paths.
    // The original result takes priority (a cleanup warning never masks the
    // eval error), but a failed ini restore on an otherwise-successful run
    // fails the command: an unrestored ini is real-environment pollution.
    clean_box(&start_exe, &eval_box, &eval_box_root);
    let ini_restore = std::fs::write(sandbox_root.join("Sandboxie.ini"), &ini_backup)
        .context("restore Sandboxie.ini after eval");
    if let Err(e) = &ini_restore {
        eprintln!("[eval] WARN: {e:#} (real Sandboxie.ini left dirty — fix manually)");
    }
    close_temp_with_retry(tmp, &eval_box_root);
    match (result, ini_restore) {
        (Err(e), _) => Err(e),
        (Ok(()), Err(e)) => Err(e),
        (Ok(()), Ok(())) => Ok(()),
    }
}

/// Phases 6d(write config)→6o of the eval run, executed while the proxy is
/// up. Extracted so the proxy lifecycle brackets it cleanly.
#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
async fn run_phases(
    proxy: &nemesis_eval_proxy::ProxyHandle,
    real_home: &Path,
    kind: &str,
    home: &Path,
    workspace: &Path,
    model_name: &str,
    model_ref: &str,
    sbieini: &Path,
    eval_box: &str,
    eval_box_root: &Path,
    start_exe: &Path,
    sbiectrl: &Path,
    sbiedll: &Path,
    common_args: &EvalCommon,
    subject: &str,
    prompt_text: &str,
    agent_exe: &Path,
) -> Result<()> {
    let proxy_port = proxy.port;

    // Minimal sanitized config (see agent_factory read-points):
    // model list with fake key + proxy base; everything else defaults.
    // NOTE agents.defaults.llm selects the model (get_effective_llm) — without
    // it the resolver falls back to the hardcoded zhipu default.
    let minimal = serde_json::json!({
        "model_list": [{
            "model_name": model_name,
            "model": model_ref,
            "api_key": "eval-fake-key",
            "api_base": format!("http://127.0.0.1:{proxy_port}/v1"),
        }],
        "agents": { "defaults": {
            "llm": model_name,
            "max_tool_iterations": 50,
        } },
        "executor": { "enabled": false },
    });
    std::fs::write(home.join("config.json"), serde_json::to_string_pretty(&minimal)?)?;

    // 6h — trace on for the eval box (immediate; never /reload).
    for key in ["FileTrace", "KeyTrace", "NetTrace", "PipeTrace"] {
        sbieini_set(sbieini, eval_box, key, "*")?;
    }

    // 6i — clean box before the run.
    clean_box(start_exe, eval_box, eval_box_root);

    // 6k — sandboxed agent. DETACHED_PROCESS + .output() is the verified
    // stable combination (A/B tested 2026-08-16): CREATE_NO_WINDOW exits
    // Start.exe with 0x40000004 (STATUS_CONTROL_C_EXIT) ~50% of the time;
    // DETACHED with .status()+null stdio also fails; DETACHED + .output()
    // exits 0 consistently.
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let _agent_exit = Command::new(start_exe)
        .arg(format!("/box:{eval_box}"))
        .arg("/hide_window")
        .arg(agent_exe)
        .env("NEMESISBOT_ROLE", "eval-agent")
        .env("NEMESISBOT_EVAL_WORKSPACE", home)
        .env("NEMESISBOT_EVAL_PROMPT", prompt_text)
        .creation_flags(DETACHED_PROCESS)
        .output();
    if let Ok(o) = &_agent_exit {
        println!(
            "[eval] agent Start.exe exited {:?} (agent exe: {})",
            o.status.code(),
            agent_exe.display()
        );
    }

    // 6j — monitor shell (signed host + monitor DLL).
    let monitor_dll = monitor_dll_path()?;
    let events_path = home.join("monitor_events.jsonl");
    let events_str = events_path.to_string_lossy().to_string();
    let sbiedll_str = sbiedll.to_string_lossy().to_string();
    let sbiectrl_str = sbiectrl.to_string_lossy().to_string();
    let monitor_dll_str = monitor_dll.to_string_lossy().to_string();
    // TIMEOUT_SECS is the monitor's TOTAL hard fuse (agent run + observation
    // + drain). observe_secs is only the "box empty → tail window" fuse; a
    // long skill run needs far more than observe_secs (verified: github
    // skill + observe-secs 60 made the shell watchdog-fire at 60s while the
    // agent was still working).
    let timeout_str = (common_args.observe_secs + 600).to_string();
    let env_cfg: Vec<(&str, &str)> = vec![
        ("NEMESISBOT_EVAL_BOX", eval_box),
        ("NEMESISBOT_EVAL_EVENTS_FILE", &events_str),
        ("NEMESISBOT_EVAL_SBIEDLL", &sbiedll_str),
        ("NEMESISBOT_EVAL_TIMEOUT_SECS", &timeout_str),
    ];
    let (shell_hp, _shell_ht) = nemesis_injector::launch_and_inject_with_env(
        &sbiectrl_str,
        &monitor_dll_str,
        0,
        &env_cfg,
    )
    .context("launch monitor shell (injection)")?;

    // 6l — wait for the monitor shell (it ends when the box empties).
    // Total-timeout fuse: observe_secs + slack for the agent spawn.
    // Keep in step with the monitor shell's total fuse (see TIMEOUT_SECS
    // above) — same +600 slack.
    let fuse = std::time::Duration::from_secs(common_args.observe_secs + 660);
    let shell_exit = wait_with_timeout(shell_hp, fuse).await;

            // 6m — monitor events (env-error marker → abort with reason).
            let events_raw = std::fs::read_to_string(&events_path).unwrap_or_default();
            if let Some(errline) = events_raw.lines().find(|l| l.starts_with("# ERROR")) {
                bail!("environment problem, eval aborted: {errline}");
            }
            if !matches!(shell_exit, Some(0)) {
                bail!(
                    "monitor shell exited abnormally ({:?}) — \
                     possible antivirus interference or sandbox fault",
                    shell_exit
                );
            }

            // 6n — pending files + agent report from the box mirror.
    // The temp home lives under C:\Users\<u>\... so Sandboxie redirects its
    // writes to the USER tree of the box: <box_root>\user\current\<rel-after-
    // the-user-dir>. (user\current already stands for C:\Users\<u>.)
    let rel_full = home
        .to_string_lossy()
        .trim_start_matches(r"C:\")
        .replace('\\', "/");
    let box_mirror = {
        // user-tree mirror: strip the Users\<name>\ prefix
        let rel_user = rel_full
            .strip_prefix("Users/")
            .and_then(|r| r.split_once('/').map(|(_, rest)| rest.to_string()))
            .unwrap_or_else(|| rel_full.clone());
        let user_mirror = eval_box_root.join("user").join("current").join(&rel_user);
        if user_mirror.exists() {
            user_mirror
        } else {
            // drive mirror for non-profile paths (whole path after C:\)
            eval_box_root.join("drive").join("C").join(&rel_full)
        }
    };
    // The worker writes its report to <workspace>/logs/eval where its
    // "workspace" env var == our home (config.json dir) — NOT home/workspace.
    // (Verified box-mirror tree: .../.tmpX/logs/eval/tool_trace.json.)
    let agent_report_dir = box_mirror.join("logs").join("eval");
    // Surface worker-side failures (the sandbox swallows the agent's stderr).
    if let Ok(err) = std::fs::read_to_string(box_mirror.join("worker_error.txt")) {
        println!("[eval] eval-agent failed inside the sandbox:\n{err}");
    }
    // Debug channel: entry marker / diag visibility for headless runs.
    {
        let alive = std::fs::read_to_string(box_mirror.join("worker_alive.txt"))
            .unwrap_or_else(|_| "(no entry marker — worker main never reached)".to_string());
        println!("[eval] worker marker: {alive}");
        // Full mirror listing — shows exactly how far the worker got.
        let mut listing = String::new();
        fn walk(dir: &Path, depth: usize, out: &mut String) {
            if depth > 3 { return; }
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    let tag = if p.is_dir() { "[D]" } else { "[F]" };
                    out.push_str(&format!("{} {}\n", tag, p.display()));
                    if p.is_dir() { walk(&p, depth + 1, out); }
                }
            }
        }
        walk(&box_mirror, 0, &mut listing);
        println!("[eval] box mirror tree:\n{listing}");
        let _ = std::fs::write(
            agent_report_dir.join("worker_debug.txt"),
            format!("alive: {alive}\nlisting:\n{listing}"),
        );
    }
    let tool_trace =
        std::fs::read_to_string(agent_report_dir.join("tool_trace.json")).unwrap_or_else(|_| "[]".into());
    let findings = std::fs::read_to_string(agent_report_dir.join("security_findings.json"))
        .unwrap_or_else(|_| "{}".into());
    let final_response =
        std::fs::read_to_string(agent_report_dir.join("final_response.md")).unwrap_or_default();

    let user_profile = user_profile_dir();
    let pending = nemesis_sandbox::pending::pending_workspace(
        eval_box_root,
        workspace,
        user_profile.as_path(),
    )
    .unwrap_or_default();

    // 6o — assemble the report.
    let out_dir = match &common_args.output {
        Some(p) => p.clone(),
        None => real_home.join("workspace").join("logs").join("eval").join(format!(
            "{}_{}",
            chrono::Local::now().format("%Y%m%d_%H%M%S"),
            slug(kind)
        )),
    };
    std::fs::create_dir_all(&out_dir)?;
    std::fs::write(
        out_dir.join("meta.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "kind": kind,
            "model": model_name,
            "box": eval_box,
            "allow_network": common_args.allow_network,
            "timeout_fuse_secs": common_args.observe_secs,
            "final_response_excerpt": final_response.chars().take(500).collect::<String>(),
        }))?,
    )?;
    std::fs::write(out_dir.join("tool_trace.json"), &tool_trace)?;
    std::fs::write(out_dir.join("security_findings.json"), &findings)?;
    std::fs::write(
        out_dir.join("sandbox_files.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "box_root": eval_box_root.display().to_string(),
            "files": pending.iter().map(|p| serde_json::json!({
                "real_path": p.real_path.display().to_string(),
                "size": p.size,
            })).collect::<Vec<_>>(),
        }))?,
    )?;
    // driver_events: keep the JSONL events (drop comment lines).
    let event_lines: Vec<&str> = events_raw.lines().filter(|l| l.starts_with('{')).collect();
    std::fs::write(
        out_dir.join("driver_events.jsonl"),
        format!("{}\n", event_lines.join("\n")),
    )?;
    std::fs::write(out_dir.join("final_response.md"), &final_response)?;
    std::fs::write(out_dir.join("subject.txt"), subject)?;
    println!("[eval] report written to {}", out_dir.display());
    println!("[eval] tool calls: see tool_trace.json | driver events: {}", event_lines.len());

    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn slug(kind: &str) -> String {
    kind.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect()
}

#[cfg(target_os = "windows")]
fn copy_agent_exe(home: &Path) -> Result<PathBuf> {
    let self_exe = std::env::current_exe().context("resolve current exe")?;
    let dst = home.join("nemesisbot-eval-agent.exe");
    std::fs::copy(&self_exe, &dst).context("copy agent exe to temp home")?;
    let src_len = std::fs::metadata(&self_exe).map(|m| m.len()).unwrap_or(0);
    let dst_len = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
    println!("[eval] agent exe copy: {src_len} -> {dst_len} bytes");
    Ok(dst)
}

/// Copy the skill directory referenced by the subject into the temp
/// workspace so the agent sees it through its normal skills flow.
#[cfg(target_os = "windows")]
fn copy_skill_dir(real_home: &Path, workspace: &Path, subject: &str) -> Result<()> {
    // The skill name is on the first line: "# Skill: <name>"
    let name = subject
        .lines()
        .find(|l| l.starts_with("# Skill: "))
        .and_then(|l| l.strip_prefix("# Skill: "))
        .context("skill subject missing name header")?;
    for base in [
        real_home.join("workspace").join("skills"),
        real_home.join("skills"),
    ] {
        let src = base.join(name);
        if src.is_dir() {
            let dst = workspace.join("skills").join(name);
            copy_dir_recursive(&src, &dst)?;
            return Ok(());
        }
    }
    // builtin skills are resolved by the loader inside the agent anyway
    Ok(())
}

#[cfg(target_os = "windows")]
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Delete the eval temp dir reliably. TempDir's Drop self-deletes but
/// SILENTLY — a Sandboxie handle releasing a beat late leaks the whole temp
/// tree with no trace (verified 2026-08-16: failed-run dirs left in %TEMP%).
///
/// The blocker is SbieSvc holding the box's registry hive (box_root/RegHive*)
/// open while it processes the async `delete_sandbox_silent` — deleting the
/// temp dir before SbieSvc releases the hive fails with sharing violations
/// that outlast short retries (verified 2026-08-16). So: first wait for the
/// BOX ROOT itself to disappear (SbieSvc's own delete is the authority; up to
/// ~10s), then close/remove the temp dir with retries.
///
/// Note `TempDir::close(self)` consumes the TempDir via mem::forget even on
/// failure, so the retry goes through an Option. Best-effort by design:
/// never fails the eval result.
#[cfg(target_os = "windows")]
fn close_temp_with_retry(tmp: tempfile::TempDir, eval_box_root: &Path) {
    // Phase 0: wait for SbieSvc to finish deleting the box (hive release).
    for _ in 0..20 {
        if !eval_box_root.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let mut opt = Some(tmp);
    let path = opt.as_ref().expect("tempdir").path().to_path_buf();
    // Phase 1: explicit close with retries for late handle release.
    for _ in 0..6 {
        if let Some(t) = opt.take() {
            if t.close().is_ok() {
                return;
            }
        } else {
            return; // consumed successfully in an earlier iteration
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    // Phase 2: close() keeps failing (or the TempDir was consumed by a failed
    // close, which forget()s it) — plain recursive delete on the same path.
    if path.exists() {
        let mut last_err = None;
        for _ in 0..10 {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => return,
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                }
            }
        }
        eprintln!(
            "[eval] WARN: temp dir not removed (SbieSvc still holds the box hive?): {} ({last_err:?})",
            path.display()
        );
    }
}

#[cfg(target_os = "windows")]
fn sbieini_set(sbieini: &Path, section: &str, key: &str, value: &str) -> Result<()> {
    let out = Command::new(sbieini)
        .args(["set", section, key, value])
        .output()
        .with_context(|| format!("run SbieIni set {section}.{key}"))?;
    if !out.status.success() {
        bail!("SbieIni set {section}.{key} failed: {}", out.status);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn clean_box(start_exe: &Path, box_name: &str, box_root: &Path) {
    // Skip when the box root does not exist — delete_sandbox would run an
    // internal `rmdir` on a missing path and pop a "Delete command failed
    // (Code 3)" dialog (dialog-capture verified, 2026-08-16).
    if !box_root.exists() {
        return;
    }
    // `delete_sandbox _silent` (underscore prefix per Start.cpp's _silent
    // switch table): silent mode deletes/terminates without dialogs.
    let _ = Command::new(start_exe)
        .arg(format!("/box:{box_name}"))
        .arg("delete_sandbox_silent")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Monitor DLL location: plugins/plugin-eval-monitor (standalone cdylib
/// project, built separately in release profile).
#[cfg(target_os = "windows")]
fn monitor_dll_path() -> Result<PathBuf> {
    // CARGO_MANIFEST_DIR is <repo>/nemesisbot — ONE level up reaches the repo
    // root (two would escape to C:\AI\NemesisBot\).
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let p = manifest_dir
        .join("..")
        .join("plugins")
        .join("plugin-eval-monitor")
        .join("target")
        .join("release")
        .join("eval_monitor_dll.dll");
    if p.exists() {
        return Ok(p);
    }
    bail!(
        "monitor DLL not found at {} — build it with: \
         cd plugins/plugin-eval-monitor && cargo build --release",
        p.display()
    )
}

#[cfg(target_os = "windows")]
fn user_profile_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .ok()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"))
}

#[cfg(target_os = "windows")]
async fn wait_with_timeout(handle: windows_sys::Win32::Foundation::HANDLE, timeout: std::time::Duration) -> Option<u32> {
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    // HANDLE (*mut c_void) is not Send — round-trip through usize.
    let h = handle as usize;
    let ms = timeout.as_millis() as u32;
    let code = tokio::task::spawn_blocking(move || unsafe {
        let h = h as windows_sys::Win32::Foundation::HANDLE;
        if WaitForSingleObject(h, ms) != 0 {
            return None; // timeout or wait failure
        }
        let mut c: u32 = 0;
        if GetExitCodeProcess(h, &mut c) == 0 {
            return None;
        }
        Some(c)
    })
    .await
    .ok()
    .flatten();
    code
}

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt as _;

// Silence unused import when compiled without the windows-only body.
