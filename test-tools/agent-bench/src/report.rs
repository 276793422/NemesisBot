//! 记分卡：场景结果聚合 → JSON + Markdown 渲染 → baseline 保存/对比。
//!
//! 防倒退语义：对比只看 **pass_rate 回归**（场景通过率下降 = 倒退，exit 1）；
//! 延迟只记录不设门（跨机器噪声大，作参考指标）。

use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::path::Path;

/// 单场景聚合结果。
#[derive(Debug, Clone)]
pub struct ScenarioScore {
    pub name: String,
    pub model: String,
    /// 场景内每轮耗时（毫秒）；concurrent_sessions 记整批墙钟。
    pub latencies_ms: Vec<u64>,
    pub passed: u32,
    pub failed: u32,
    /// 失败轮的诊断摘录（响应片段 / 错误文本）。
    pub failure_notes: Vec<String>,
}

impl ScenarioScore {
    pub fn new(name: &str, model: &str) -> Self {
        Self {
            name: name.to_string(),
            model: model.to_string(),
            latencies_ms: Vec::new(),
            passed: 0,
            failed: 0,
            failure_notes: Vec::new(),
        }
    }

    pub fn record_pass(&mut self, latency_ms: u64) {
        self.passed += 1;
        self.latencies_ms.push(latency_ms);
    }

    pub fn record_fail(&mut self, note: String) {
        self.failed += 1;
        self.failure_notes.push(note);
    }

    pub fn total(&self) -> u32 {
        self.passed + self.failed
    }

    pub fn pass_rate(&self) -> f64 {
        let t = self.total();
        if t == 0 {
            0.0
        } else {
            self.passed as f64 / t as f64
        }
    }

    fn percentile(&self, p: f64) -> u64 {
        let mut v = self.latencies_ms.clone();
        if v.is_empty() {
            return 0;
        }
        v.sort_unstable();
        let idx = (((v.len() as f64 - 1.0) * p).round() as usize).min(v.len() - 1);
        v[idx]
    }
}

/// 完整记分卡。
pub struct Scorecard {
    pub scenarios: Vec<ScenarioScore>,
    pub started_at: String,
    pub gateway_version: String,
    /// 全套件墙钟（秒，含起停不计——只计场景执行段）。
    pub bench_wall_secs: u64,
}

impl Scorecard {
    pub fn new(gateway_version: String) -> Self {
        Self {
            scenarios: Vec::new(),
            started_at: chrono::Local::now().to_rfc3339(),
            gateway_version,
            bench_wall_secs: 0,
        }
    }

    pub fn score(&mut self, name: &str) -> &mut ScenarioScore {
        let model = scenario_model(name);
        let entry = ScenarioScore::new(name, model);
        self.scenarios.push(entry);
        self.scenarios.last_mut().expect("just pushed")
    }

    // -- JSON -------------------------------------------------------------

    pub fn to_json(&self) -> Value {
        let scenarios: Vec<Value> = self
            .scenarios
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "model": s.model,
                    "runs": s.total(),
                    "passed": s.passed,
                    "failed": s.failed,
                    "pass_rate": (s.pass_rate() * 1000.0).round() / 1000.0,
                    "latency_ms": {
                        "p50": s.percentile(0.5),
                        "p95": s.percentile(0.95),
                        "max": s.latencies_ms.iter().max().copied().unwrap_or(0),
                    },
                    "failure_notes": s.failure_notes,
                })
            })
            .collect();
        let total_passed: u32 = self.scenarios.iter().map(|s| s.passed).sum();
        let total_runs: u32 = self.scenarios.iter().map(|s| s.total()).sum();
        json!({
            "schema": "agent-bench/v1",
            "started_at": self.started_at,
            "gateway_version": self.gateway_version,
            "bench_wall_secs": self.bench_wall_secs,
            "totals": {
                "runs": total_runs,
                "passed": total_passed,
                "pass_rate": if total_runs == 0 { 0.0 } else {
                    (total_passed as f64 / total_runs as f64 * 1000.0).round() / 1000.0
                },
            },
            "scenarios": scenarios,
        })
    }

    // -- Markdown ---------------------------------------------------------

    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# NemesisBot 自建基准记分卡（agent-bench v1）\n\n");
        out.push_str(&format!("- 时间：{}\n", self.started_at));
        out.push_str(&format!("- 被测网关：{}\n", self.gateway_version));
        out.push_str(&format!("- 场景执行墙钟：{}s\n\n", self.bench_wall_secs));
        out.push_str("| 场景 | 模型 | 轮数 | 通过 | 通过率 | p50(ms) | p95(ms) |\n");
        out.push_str("|---|---|---|---|---|---|---|\n");
        for s in &self.scenarios {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {:.1}% | {} | {} |\n",
                s.name,
                s.model,
                s.total(),
                s.passed,
                s.pass_rate() * 100.0,
                s.percentile(0.5),
                s.percentile(0.95),
            ));
        }
        let failures: Vec<String> = self
            .scenarios
            .iter()
            .flat_map(|s| {
                s.failure_notes
                    .iter()
                    .map(|n| format!("- `{}`: {n}", s.name))
                    .collect::<Vec<_>>()
            })
            .collect();
        if !failures.is_empty() {
            out.push_str("\n## 失败明细\n\n");
            for f in failures {
                out.push_str(&f);
                out.push('\n');
            }
        }
        out
    }
}

/// 场景 → 默认模型（Scorecard::score 需要在 record 前知道模型名）。
pub(crate) fn scenario_model(name: &str) -> &'static str {
    match name {
        "context_integrity" => "testai-9.3",
        "tool_parallel_batch" => "testai-2.1",
        "security_boundary_block" => "testai-5.0",
        "vision_roundtrip" => "testai-vision-1.0",
        _ => "testai-1.1",
    }
}

// ---------------------------------------------------------------------------
// baseline 保存 / 对比
// ---------------------------------------------------------------------------

/// 保存 baseline（JSON）。目录不存在则创建。
pub fn save_baseline(card: &Scorecard, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&card.to_json())?)?;
    Ok(())
}

/// 对比 baseline：任何场景 pass_rate 下降（>1e-9）= 倒退。
/// baseline 里没有的新场景不判倒退（首轮只记录）；baseline 有而本轮没有的
/// 场景 = 覆盖缩水，同样判倒退（防止悄悄删场景过关）。
pub fn compare_baseline(card: &Scorecard, baseline_path: &Path) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(baseline_path)?;
    let base: Value = serde_json::from_str(&raw)?;
    if base.get("schema").and_then(|v| v.as_str()) != Some("agent-bench/v1") {
        bail!(
            "baseline schema 不是 agent-bench/v1: {}",
            baseline_path.display()
        );
    }

    let mut regressions = Vec::new();
    let base_scenarios = base
        .get("scenarios")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for bs in &base_scenarios {
        let name = bs.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let base_rate = bs.get("pass_rate").and_then(|v| v.as_f64()).unwrap_or(0.0);
        match card.scenarios.iter().find(|s| s.name == name) {
            Some(cur) => {
                if cur.pass_rate() < base_rate - 1e-9 {
                    regressions.push(format!(
                        "场景 `{name}` 通过率回退：{:.1}% → {:.1}%",
                        base_rate * 100.0,
                        cur.pass_rate() * 100.0
                    ));
                }
            }
            None => {
                regressions.push(format!("场景 `{name}` 本轮未执行（覆盖缩水）"));
            }
        }
    }
    Ok(regressions)
}
