//! task-eval — 12 题固定任务集评测 runner（外循环 P4 防倒退门禁）。
//!
//! 驱动 `nemesisbot run`（K1 headless 编码 agent 入口，安全 8 层与 gateway
//! 同源）在隔离任务工作区里跑固定小任务，宿主侧检查器判定验收，产出
//! JSON + Markdown 报告；baseline 保存/对比语义与 agent-bench 同款
//! （pass_rate 回退或任务覆盖缩水 → exit 1，可挂门禁）。
//!
//! Usage:
//!   task-eval --home <eval-home>                        # 全 12 题各跑 1 次
//!   task-eval --home <eval-home> --tasks bugfix-sum-off-by-one
//!   task-eval --home <eval-home> --runs 2               # 每题独立跑 2 次（§9.1）
//!   task-eval --check-only                              # 只验检查器（不跑 agent）
//!   task-eval --save-baseline baseline.json             # 存基线
//!   task-eval --compare baseline.json                   # 回退即 exit 1
//!
//! 前置：`cargo build --release -p nemesisbot`（或 --nemesisbot 指定二进制），
//! 以及 `--home` 指向已配置模型的 NEMESISBOT_HOME（config.json model_list 带可用
//! 模型与 key）。真实模型跑一轮花真钱——评测按需手动触发，不进普通 CI。
//!
//! 诚实边界：kill 超时只杀 nemesisbot 直接子进程（agent exec 工具再 spawn
//! 的孙子进程不追杀——v1 边界）；基线必须来自真实模型运行（工具不预置
//! 基线文件，不伪造通过率）。

mod checker;
mod tasks;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tasks::{Category, TASKS, TaskDef};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

struct Config {
    mode_check_only: bool,
    task_filter: Option<Vec<String>>,
    runs: usize,
    home: Option<PathBuf>,
    nemesisbot: Option<PathBuf>,
    model: Option<String>,
    max_turns: u32,
    budget_secs: u64,
    python: Option<String>,
    save_baseline: Option<PathBuf>,
    compare_baseline: Option<PathBuf>,
    out_dir: PathBuf,
}

fn usage() -> &'static str {
    "\
task-eval — 12 题固定任务集评测 runner

用法:
  task-eval --home <eval-home> [选项]
  task-eval --check-only

选项:
  --home <DIR>          NEMESISBOT_HOME（含已配置模型的 config.json）；跑 agent 必填
  --nemesisbot <PATH>   被测二进制（缺省探 target/release 再 target/debug）
  --model <ALIAS>       传给 run --model（缺省用 home 配置的默认模型）
  --max-turns <N>       传给 run --max-turns（默认 40）
  --runs <N>            每题独立运行次数（默认 1；§9.1 试点口径为 2）
  --budget-secs <N>     单次运行墙钟预算秒（默认 2700 = 45 分钟，§9.3）
  --tasks <a,b,c>       只跑指定题（逗号分隔 id）
  --python <PATH>       exec 检查器解释器（缺省探 python 再 python3）
  --save-baseline <F>   存基线（题级 pass/fail + pass_rate）
  --compare <F>         与基线对比：pass_rate 回退或覆盖缩水 → exit 1
  --out-dir <DIR>       报告与运行现场根（默认 target/task-eval）
  --check-only          只物化 seed + 跑检查器（不启动 agent）——全部任务
                        必须判 FAIL（防误报自检），有任一 PASS 则 exit 1
"
}

impl Config {
    fn resolve() -> Result<Self> {
        let args: Vec<String> = std::env::args().collect();
        let mut cfg = Config {
            mode_check_only: false,
            task_filter: None,
            runs: 1,
            home: None,
            nemesisbot: None,
            model: None,
            max_turns: 40,
            budget_secs: 2700,
            python: None,
            save_baseline: None,
            compare_baseline: None,
            out_dir: PathBuf::from("target/task-eval"),
        };
        let mut i = 1;
        while i < args.len() {
            let next = |i: &mut usize, flag: &str| -> Result<String> {
                *i += 1;
                args.get(*i)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("{flag} 缺少参数"))
            };
            match args[i].as_str() {
                "--help" | "-h" => {
                    println!("{}", usage());
                    std::process::exit(0);
                }
                "--check-only" => cfg.mode_check_only = true,
                "--home" => cfg.home = Some(PathBuf::from(next(&mut i, "--home")?)),
                "--nemesisbot" => {
                    cfg.nemesisbot = Some(PathBuf::from(next(&mut i, "--nemesisbot")?))
                }
                "--model" => cfg.model = Some(next(&mut i, "--model")?),
                "--max-turns" => cfg.max_turns = next(&mut i, "--max-turns")?.parse()?,
                "--runs" => cfg.runs = next(&mut i, "--runs")?.parse()?,
                "--budget-secs" => cfg.budget_secs = next(&mut i, "--budget-secs")?.parse()?,
                "--tasks" => {
                    cfg.task_filter = Some(
                        next(&mut i, "--tasks")?
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect(),
                    )
                }
                "--python" => cfg.python = Some(next(&mut i, "--python")?),
                "--save-baseline" => {
                    cfg.save_baseline = Some(PathBuf::from(next(&mut i, "--save-baseline")?))
                }
                "--compare" => {
                    cfg.compare_baseline = Some(PathBuf::from(next(&mut i, "--compare")?))
                }
                "--out-dir" => cfg.out_dir = PathBuf::from(next(&mut i, "--out-dir")?),
                other => bail!("未知参数: {other}（--help 看用法）"),
            }
            i += 1;
        }
        if cfg.runs == 0 {
            bail!("--runs 必须 ≥ 1");
        }
        if !cfg.mode_check_only && cfg.home.is_none() {
            bail!(
                "跑 agent 需要 --home（NEMESISBOT_HOME，含已配置模型）；只验检查器用 --check-only"
            );
        }
        Ok(cfg)
    }
}

fn resolve_nemesisbot(cfg: &Config) -> Result<PathBuf> {
    if let Some(p) = &cfg.nemesisbot {
        if p.exists() {
            return Ok(p.clone());
        }
        bail!("--nemesisbot 指定的二进制不存在: {}", p.display());
    }
    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };
    for cand in [
        PathBuf::from(format!("target/release/nemesisbot{exe_suffix}")),
        PathBuf::from(format!("target/debug/nemesisbot{exe_suffix}")),
    ] {
        if cand.exists() {
            return Ok(cand);
        }
    }
    bail!(
        "未找到 nemesisbot 二进制——先 cargo build --release -p nemesisbot 或用 --nemesisbot 指定"
    );
}

// ---------------------------------------------------------------------------
// 运行记录
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct RunRecord {
    task_id: String,
    category: &'static str,
    run_index: usize,
    /// pass / fail / timeout / spawn_error / check_only_fail / check_only_pass
    status: String,
    agent_exit: Option<i32>,
    agent_final: Option<String>,
    steps: Vec<StepRecord>,
    duration_ms: u64,
    workspace: String,
}

#[derive(Debug, Serialize)]
struct StepRecord {
    ok: bool,
    detail: String,
    env_blocked: bool,
}

#[derive(Debug, Serialize)]
struct Report {
    started_at: String,
    finished_at: String,
    mode: String,
    nemesisbot: String,
    nemesisbot_mtime: Option<String>,
    model: String,
    runs_per_task: usize,
    results: Vec<RunRecord>,
    total_runs: usize,
    passed_runs: usize,
    pass_rate: f64,
    per_category: Vec<(&'static str, usize, usize)>,
}

impl Report {
    fn summarize(&mut self) {
        self.total_runs = self.results.len();
        self.passed_runs = self.results.iter().filter(|r| r.status == "pass").count();
        self.pass_rate = if self.total_runs == 0 {
            0.0
        } else {
            self.passed_runs as f64 / self.total_runs as f64
        };
        self.per_category = [Category::BugFix, Category::Docs, Category::Tests]
            .iter()
            .map(|c| {
                let rs: Vec<&RunRecord> = self
                    .results
                    .iter()
                    .filter(|r| r.category == c.slug())
                    .collect();
                let passed = rs.iter().filter(|r| r.status == "pass").count();
                (c.slug(), passed, rs.len())
            })
            .collect();
    }
}

// ---------------------------------------------------------------------------
// agent 驱动
// ---------------------------------------------------------------------------

/// 跑一次 agent（`nemesisbot run`，JSON 事件流到文件），返回 (exit, final)。
async fn run_agent(
    cfg: &Config,
    binary: &Path,
    statement: &str,
    ws: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<(Option<i32>, Option<String>)> {
    use tokio::io::AsyncReadExt;
    use tokio::process::Command;

    let mut cmd = Command::new(binary);
    cmd.arg("run")
        .arg("--workspace")
        .arg(ws)
        .arg("--format")
        .arg("json")
        .arg("--max-turns")
        .arg(cfg.max_turns.to_string())
        .arg(statement)
        .env(
            "NEMESISBOT_HOME",
            cfg.home.as_ref().expect("run 模式 home 必填"),
        );
    if let Some(model) = &cfg.model {
        cmd.arg("--model").arg(model);
    }
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("spawn nemesisbot run 失败")?;

    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let out_f = tokio::fs::File::create(stdout_path).await?;
    let err_f = tokio::fs::File::create(stderr_path).await?;
    let (mut out_w, mut err_w) = (out_f, err_f);
    let pump_out = async move {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf).await;
        let _ = tokio::io::AsyncWriteExt::write_all(&mut out_w, &buf).await;
        buf
    };
    let pump_err = async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        let _ = tokio::io::AsyncWriteExt::write_all(&mut err_w, &buf).await;
        buf
    };

    let budget = std::time::Duration::from_secs(cfg.budget_secs);
    let joined = tokio::time::timeout(budget, async {
        tokio::join!(pump_out, pump_err, child.wait())
    })
    .await;

    match joined {
        // 正常退出：取 stdout NDJSON 里最后一个 final/error 的文本。
        Ok((out_buf, _err_buf, status)) => {
            let final_text = extract_final_from_stdout(&String::from_utf8_lossy(&out_buf));
            Ok((status.ok().and_then(|s| s.code()), final_text))
        }
        Err(_elapsed) => {
            // 超时：kill 直系子进程（孙子进程不追杀——v1 诚实边界）；
            // 泵 future 已随超时丢弃，两份输出文件可能不完整。
            let _ = child.start_kill();
            let _ = child.wait().await;
            let _ = tokio::fs::write(
                stderr_path,
                "[task-eval] budget 超时，进程已终止；stdout/stderr 可能不完整\n".as_bytes(),
            )
            .await;
            Ok((None, None))
        }
    }
}

/// stdout NDJSON 里取最后一个 final/error 事件的文本（解析失败=None）。
fn extract_final_from_stdout(stdout: &str) -> Option<String> {
    let mut found = None;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("final") => {
                found = Some(format!(
                    "final: {}",
                    v.get("text").and_then(|t| t.as_str()).unwrap_or("")
                ))
            }
            Some("error") => {
                found = Some(format!(
                    "error: {}",
                    v.get("message").and_then(|t| t.as_str()).unwrap_or("")
                ))
            }
            _ => {}
        }
    }
    found
}

// ---------------------------------------------------------------------------
// 基线
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Baseline {
    saved_at: String,
    /// `<task_id>#<run_index>` → pass。
    tasks: std::collections::BTreeMap<String, bool>,
    pass_rate: f64,
}

fn baseline_from_report(report: &Report) -> Baseline {
    let mut tasks = std::collections::BTreeMap::new();
    for r in &report.results {
        tasks.insert(format!("{}#{}", r.task_id, r.run_index), r.status == "pass");
    }
    Baseline {
        saved_at: report.finished_at.clone(),
        tasks,
        pass_rate: report.pass_rate,
    }
}

/// 对比基线：只看 pass_rate 回退与任务覆盖缩水（延迟/耗时跨机噪声大，
/// 不设门——agent-bench 同款语义）。返回违规明细；空 = 绿。
fn compare_baseline(baseline: &Baseline, current: &Report) -> Vec<String> {
    let mut violations = Vec::new();
    for key in baseline.tasks.keys() {
        let id = key.rsplit_once('#').map(|x| x.0).unwrap_or(key);
        if !current.results.iter().any(|r| r.task_id == id) {
            violations.push(format!("覆盖缩水: 基线含 {key} 但本轮未跑 {id}"));
        }
    }
    if current.pass_rate < baseline.pass_rate {
        violations.push(format!(
            "pass_rate 回退: 本轮 {:.3} < 基线 {:.3}",
            current.pass_rate, baseline.pass_rate
        ));
    }
    violations
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

fn selected_tasks(cfg: &Config) -> Result<Vec<&'static TaskDef>> {
    let mut list: Vec<&'static TaskDef> = TASKS.iter().collect();
    if let Some(filter) = &cfg.task_filter {
        for id in filter {
            if !TASKS.iter().any(|t| t.id == *id) {
                bail!("--tasks 含未知题 id: {id}");
            }
        }
        list.retain(|t| filter.iter().any(|id| id == t.id));
    }
    Ok(list)
}

async fn run() -> Result<i32> {
    let cfg = Config::resolve()?;
    let problems = tasks::validate();
    if !problems.is_empty() {
        bail!("任务集完整性校验失败: {problems:?}");
    }
    let selected = selected_tasks(&cfg)?;
    let python = checker::resolve_python(cfg.python.as_deref());
    if python.is_none() {
        eprintln!("[task-eval] WARN: 未找到 python——exec 类检查器将按环境阻塞判败（§9.2 单列）");
    }

    std::fs::create_dir_all(&cfg.out_dir)?;
    let started_at = chrono::Local::now().to_rfc3339();
    let mut report = Report {
        started_at,
        finished_at: String::new(),
        mode: if cfg.mode_check_only {
            "check-only"
        } else {
            "run"
        }
        .to_string(),
        nemesisbot: String::new(),
        nemesisbot_mtime: None,
        model: cfg
            .model
            .clone()
            .unwrap_or_else(|| "<home 默认>".to_string()),
        runs_per_task: if cfg.mode_check_only { 0 } else { cfg.runs },
        results: Vec::new(),
        total_runs: 0,
        passed_runs: 0,
        pass_rate: 0.0,
        per_category: Vec::new(),
    };

    let binary = if cfg.mode_check_only {
        None
    } else {
        let b = resolve_nemesisbot(&cfg)?;
        report.nemesisbot = b.display().to_string();
        report.nemesisbot_mtime = std::fs::metadata(&b).ok().and_then(|m| {
            m.modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339())
        });
        Some(b)
    };

    let runs = if cfg.mode_check_only { 1 } else { cfg.runs };
    for task in &selected {
        for k in 0..runs {
            let run_dir = cfg
                .out_dir
                .join("runs")
                .join(chrono::Local::now().format("%Y%m%d_%H%M%S").to_string())
                .join(format!("{}_r{}", task.id, k));
            std::fs::create_dir_all(run_dir.join("workspace"))?;
            std::fs::create_dir_all(run_dir.join("scratch"))?;
            let ws = run_dir.join("workspace");
            checker::materialize_seed(&ws, task.seed)
                .with_context(|| format!("{}: seed 物化失败", task.id))?;

            let started = Instant::now();
            let (agent_exit, agent_final, spawn_error) = if let Some(binary) = &binary {
                match run_agent(
                    &cfg,
                    binary,
                    task.statement,
                    &ws,
                    &run_dir.join("agent_stdout.ndjson"),
                    &run_dir.join("agent_stderr.log"),
                )
                .await
                {
                    Ok((exit, final_text)) => (exit, final_text, None),
                    Err(e) => (None, None, Some(format!("{e:#}"))),
                }
            } else {
                (None, None, None)
            };
            let duration_ms = started.elapsed().as_millis() as u64;

            let steps = checker::eval_task(task, &ws, &run_dir.join("scratch"), python.as_deref());
            let steps_rec: Vec<StepRecord> = steps.into_iter().map(into_step_record).collect();
            // 状态裁决：超时/.spawn 失败优先判非 pass（§9.3 预算语义——超墙钟
            // 即败，哪怕文件碰巧齐了，进程是被杀的中途态不可信）。
            let status = if let Some(err) = &spawn_error {
                format!("spawn_error: {err}")
            } else if binary.is_some() && agent_exit.is_none() {
                "timeout".to_string()
            } else if steps_rec.iter().all(|s| s.ok) {
                "pass".to_string()
            } else {
                "fail".to_string()
            };
            if cfg.mode_check_only {
                // 防误报自检语义：seed 未动必须判 FAIL。
                let s = if steps_rec.iter().all(|s| s.ok) {
                    "check_only_pass"
                } else {
                    "check_only_fail"
                };
                report.results.push(RunRecord {
                    task_id: task.id.to_string(),
                    category: task.category.slug(),
                    run_index: k,
                    status: s.to_string(),
                    agent_exit: None,
                    agent_final: None,
                    steps: steps_rec,
                    duration_ms,
                    workspace: ws.display().to_string(),
                });
            } else {
                report.results.push(RunRecord {
                    task_id: task.id.to_string(),
                    category: task.category.slug(),
                    run_index: k,
                    status,
                    agent_exit,
                    agent_final,
                    steps: steps_rec,
                    duration_ms,
                    workspace: ws.display().to_string(),
                });
            }
        }
    }

    report.finished_at = chrono::Local::now().to_rfc3339();
    report.summarize();

    // 防误报自检出口。
    if cfg.mode_check_only {
        let false_positives: Vec<&str> = report
            .results
            .iter()
            .filter(|r| r.status == "check_only_pass")
            .map(|r| r.task_id.as_str())
            .collect();
        write_reports(&cfg, &report)?;
        if !false_positives.is_empty() {
            eprintln!(
                "[task-eval] 误报：以下任务的检查器在未动过的 seed 上 PASS（评估器泄漏或验收过松）: {false_positives:?}"
            );
            return Ok(1);
        }
        println!("[task-eval] 检查器自检通过：12 题在 seed 上全部正确判 FAIL（无误报）。");
        return Ok(0);
    }

    write_reports(&cfg, &report)?;

    // 基线出口。
    if let Some(path) = &cfg.save_baseline {
        let bl = baseline_from_report(&report);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&bl)?)?;
        println!("[task-eval] 基线已保存: {}", path.display());
    }
    if let Some(path) = &cfg.compare_baseline {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("读基线失败: {}", path.display()))?;
        let baseline: Baseline = serde_json::from_str(&text)?;
        let violations = compare_baseline(&baseline, &report);
        if !violations.is_empty() {
            eprintln!("[task-eval] 门禁红（相对 {}）:", path.display());
            for v in &violations {
                eprintln!("  - {v}");
            }
            return Ok(1);
        }
        println!(
            "[task-eval] 门禁绿：pass_rate {:.3} ≥ 基线 {:.3}，覆盖无缩水。",
            report.pass_rate, baseline.pass_rate
        );
    }

    print_summary(&report);
    Ok(0)
}

fn into_step_record(s: checker::StepOutcome) -> StepRecord {
    StepRecord {
        ok: s.ok,
        detail: s.detail,
        env_blocked: s.env_blocked,
    }
}

fn write_reports(cfg: &Config, report: &Report) -> Result<()> {
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let json_path = cfg.out_dir.join(format!("report_{ts}.json"));
    std::fs::write(&json_path, serde_json::to_string_pretty(report)?)?;
    std::fs::write(
        cfg.out_dir.join(format!("report_{ts}.md")),
        render_md(report),
    )?;
    println!("[task-eval] 报告: {}", json_path.display());
    Ok(())
}

fn render_md(report: &Report) -> String {
    let mut md = String::new();
    md.push_str(&format!(
        "# task-eval 报告（{}）\n\n- 模式: {}\n- 二进制: {}（mtime {:?}）\n- 模型: {}\n- 每题运行: {} 次\n- 总通过: {}/{}（pass_rate {:.3}）\n",
        report.finished_at,
        report.mode,
        report.nemesisbot,
        report.nemesisbot_mtime,
        report.model,
        report.runs_per_task,
        report.passed_runs,
        report.total_runs,
        report.pass_rate,
    ));
    md.push_str("\n## 分类别\n\n| 类别 | 通过 | 总数 |\n|---|---|---|\n");
    for (cat, passed, total) in &report.per_category {
        md.push_str(&format!("| {cat} | {passed} | {total} |\n"));
    }
    md.push_str("\n## 明细\n\n");
    for r in &report.results {
        md.push_str(&format!(
            "### {}#r{} — {}（{} ms）\n\n",
            r.task_id, r.run_index, r.status, r.duration_ms
        ));
        if let Some(exit) = r.agent_exit {
            md.push_str(&format!("- agent exit: {exit}\n"));
        }
        if let Some(final_text) = &r.agent_final {
            let head: String = final_text.chars().take(300).collect();
            md.push_str(&format!("- agent 结语文本: {head}\n"));
        }
        md.push_str("\n| 检查 | 结果 | 说明 |\n|---|---|---|\n");
        for s in &r.steps {
            md.push_str(&format!(
                "| | {} | {}{} |\n",
                if s.ok { "✅" } else { "❌" },
                if s.env_blocked {
                    "（环境阻塞）"
                } else {
                    ""
                },
                s.detail.replace('|', "\\|")
            ));
        }
        md.push('\n');
    }
    md
}

fn print_summary(report: &Report) {
    println!(
        "[task-eval] 完成：{}/{} 通过（pass_rate {:.3}）",
        report.passed_runs, report.total_runs, report.pass_rate
    );
    for (cat, passed, total) in &report.per_category {
        println!("  {cat}: {passed}/{total}");
    }
}

#[tokio::main]
async fn main() {
    let code = match run().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[task-eval] 错误: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests;
