//! 宿主侧检查器（P4 12 题评测集）：seed 物化 + [`crate::tasks::CheckStep`]
//! 评估。检查器全部在宿主侧执行——评估器脚本不落任务工作区（§9.1：agent
//! 不得读取预期补丁或修改评估器）。

use anyhow::{Context, Result};
use std::path::Path;

/// seed 物化：把任务 seed 逐文件写到 `ws`（父目录必须已存在）。
pub fn materialize_seed(ws: &Path, seed: &[(&'static str, &'static str)]) -> Result<()> {
    for (rel, content) in seed {
        let path = ws.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建 seed 目录失败: {}", parent.display()))?;
        }
        std::fs::write(&path, content)
            .with_context(|| format!("写出 seed 失败: {}", path.display()))?;
    }
    Ok(())
}

/// 单步检查结果（`env_note` 在场 = 环境阻塞导致未真正执行，判定保守取败
/// 但单独标注——§9.2 环境阻塞单列）。
#[derive(Debug)]
pub struct StepOutcome {
    pub ok: bool,
    pub detail: String,
    pub env_blocked: bool,
}

impl StepOutcome {
    fn pass(detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            detail: detail.into(),
            env_blocked: false,
        }
    }
    fn fail(detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: detail.into(),
            env_blocked: false,
        }
    }
}

/// 解析 python 解释器（`--python` 优先；缺省探 `python` 再探 `python3`）。
/// None = 本机无 python（exec 类检查器将诚实判 env 阻塞）。
pub fn resolve_python(explicit: Option<&str>) -> Option<String> {
    if let Some(p) = explicit {
        return Some(p.to_string());
    }
    for cand in ["python", "python3"] {
        if std::process::Command::new(cand)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some(cand.to_string());
        }
    }
    None
}

/// 评估单步（`ws` = 任务工作区；`scratch` = 检查器临时目录，exec 脚本落
/// 这里不落工作区；`python` = 已解析解释器）。
pub fn eval_step(
    step: &crate::tasks::CheckStep,
    ws: &Path,
    scratch: &Path,
    python: Option<&str>,
) -> StepOutcome {
    match step {
        crate::tasks::CheckStep::Contains { path, any_of } => {
            let full = ws.join(path);
            match std::fs::read_to_string(&full) {
                Ok(text) => {
                    if any_of.iter().any(|needle| text.contains(needle)) {
                        StepOutcome::pass(format!("{path} 命中验收子串"))
                    } else {
                        StepOutcome::fail(format!(
                            "{path} 未命中任一验收子串（{} 个候选）",
                            any_of.len()
                        ))
                    }
                }
                Err(e) => StepOutcome::fail(format!("读取 {path} 失败: {e}")),
            }
        }
        crate::tasks::CheckStep::NotContains { path, any_of } => {
            let full = ws.join(path);
            match std::fs::read_to_string(&full) {
                Ok(text) => {
                    let hit: Vec<&str> = any_of
                        .iter()
                        .copied()
                        .filter(|n| text.contains(n))
                        .collect();
                    if hit.is_empty() {
                        StepOutcome::pass(format!("{path} 无禁用形态"))
                    } else {
                        StepOutcome::fail(format!("{path} 仍含禁用形态: {hit:?}"))
                    }
                }
                Err(e) => StepOutcome::fail(format!("读取 {path} 失败: {e}")),
            }
        }
        crate::tasks::CheckStep::ExecPython { code } => {
            let Some(py) = python else {
                return StepOutcome {
                    ok: false,
                    detail: "环境阻塞: 本机未找到 python 解释器（exec 类检查器无法执行）"
                        .to_string(),
                    env_blocked: true,
                };
            };
            run_python_checker(py, code, ws, scratch)
        }
    }
}

/// exec 检查器：脚本落 `scratch`（preamble 注入工作区根到 sys.path），
/// `python -I <script>`，cwd = 工作区，exit 0 = 过。
fn run_python_checker(python: &str, code: &str, ws: &Path, scratch: &Path) -> StepOutcome {
    let ws_fwd = ws.to_string_lossy().replace('\\', "/");
    let script = format!("import sys\nsys.path.insert(0, r'{ws_fwd}')\n{code}");
    let script_path = scratch.join(format!(
        "checker_{}.py",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    if let Err(e) = std::fs::write(&script_path, script) {
        return StepOutcome::fail(format!("写出检查器脚本失败: {e}"));
    }
    let output = std::process::Command::new(python)
        .arg("-I")
        .arg(&script_path)
        .current_dir(ws)
        .output();
    let _ = std::fs::remove_file(&script_path);
    match output {
        Ok(out) if out.status.success() => StepOutcome::pass("行为断言通过".to_string()),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let total = stderr.chars().count();
            let tail: String = stderr.chars().skip(total.saturating_sub(400)).collect();
            StepOutcome::fail(format!(
                "行为断言失败（exit {:?}）: {}",
                out.status.code(),
                tail.trim()
            ))
        }
        Err(e) => StepOutcome::fail(format!("启动 python 失败: {e}")),
    }
}

/// 评估整个任务（AND 语义：全步通过 = 过）。
pub fn eval_task(
    task: &crate::tasks::TaskDef,
    ws: &Path,
    scratch: &Path,
    python: Option<&str>,
) -> Vec<StepOutcome> {
    task.checkers
        .iter()
        .map(|s| eval_step(s, ws, scratch, python))
        .collect()
}
