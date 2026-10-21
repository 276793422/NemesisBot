//! 记分卡：场景结果聚合 → JSON + Markdown 渲染 → baseline 保存/对比。
//!
//! 防倒退语义（A3 修订，2026-10-21 前代实现思路引入）：
//! - pass_rate 回归任何机器都判倒退（功能正确性无噪声）；
//! - 延迟只在 **同机** 判门（相对偏差，默认 +30%）——跨机器延迟噪声大，
//!   基线换了机器就不比（老基线无 host 字段同样跳过，只提示不炸）。
//!   借鉴前代实现的同环境相对基线口径：不比绝对值，比「同机相对退化」。

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
    /// 每轮相对基准开始的墙钟偏移（毫秒）——chrome trace 的时间轴
    /// （A3；与 `latencies_ms` 一一对应）。
    pub offsets_ms: Vec<u64>,
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
            offsets_ms: Vec::new(),
            passed: 0,
            failed: 0,
            failure_notes: Vec::new(),
        }
    }

    /// 记一轮通过 + 墙钟偏移（A3 trace 时间轴；唯一记账入口）。
    pub fn record_pass_at(&mut self, latency_ms: u64, offset_ms: u64) {
        self.passed += 1;
        self.latencies_ms.push(latency_ms);
        self.offsets_ms.push(offset_ms);
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
    /// A3：宿主机指纹（延迟门的同机判定依据；老基线无此字段时延迟门
    /// 自动跳过）。
    pub host: String,
}

impl Scorecard {
    pub fn new(gateway_version: String) -> Self {
        Self {
            scenarios: Vec::new(),
            started_at: chrono::Local::now().to_rfc3339(),
            gateway_version,
            bench_wall_secs: 0,
            host: host_fingerprint(),
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
            "host": self.host,
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
        "identifier_survival" => "testai-sum-1.0",
        "tool_parallel_batch" => "testai-2.1",
        "security_boundary_block" => "testai-5.0",
        "vision_roundtrip" => "testai-vision-1.0",
        "codemode_orchestration" => "testai-codemode-1.0",
        "docs_lift" => "testai-docslift-1.0",
        "disconnect_resume" => "testai-2.1",
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

/// 宿主机指纹（零依赖）：Windows COMPUTERNAME → Unix HOSTNAME env →
/// `/proc/sys/kernel/hostname`。第三级是给 Linux 的：bash 的 HOSTNAME 是
/// shell 变量、CI 常不导出进 env，只靠 env 会让指纹恒 "unknown"，延迟门
/// 在 Linux 上被静默废掉（六轮回查 2026-10-21）。全链取不到 = `"unknown"`
/// （诚实跳过延迟门，不猜）。macOS 无 /proc 兜底，回落 unknown——
/// agent-bench 主场是 Windows/Linux，诚实缺省优于引子进程。
pub fn host_fingerprint() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(read_proc_hostname)
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(target_os = "linux")]
fn read_proc_hostname() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(not(target_os = "linux"))]
fn read_proc_hostname() -> Option<String> {
    None
}

/// 对比 baseline（A3 修订）：
/// - **pass_rate**：任何场景下降（>1e-9）= 倒退（任何机器都判）；
/// - **覆盖**：baseline 有而本轮没有的场景 = 覆盖缩水，倒退；
/// - **延迟**：同机才判——p50 相对偏差超过 `latency_tolerance`（0.30 =
///   +30%）= 延迟倒退。基线无 `host` 字段（老格式）或 host 不同 →
///   延迟门整体跳过并在返回列表里给一条说明（不判倒退）。
///
/// 返回字符串列表：倒退项 + 提示项（提示以 `[note]` 前缀区分，不计
/// 失败）；调用方按是否有倒退决定 exit code。
pub fn compare_baseline(
    card: &Scorecard,
    baseline_path: &Path,
    latency_tolerance: f64,
) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(baseline_path)?;
    let base: Value = serde_json::from_str(&raw)?;
    if base.get("schema").and_then(|v| v.as_str()) != Some("agent-bench/v1") {
        bail!(
            "baseline schema 不是 agent-bench/v1: {}",
            baseline_path.display()
        );
    }

    let mut findings = Vec::new();
    let mut regressions = 0usize;
    let base_scenarios = base
        .get("scenarios")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 延迟门资格：基线带 host 且与本机一致。
    let base_host = base.get("host").and_then(|v| v.as_str());
    let latency_gate = match base_host {
        None => {
            findings.push(
                "[note] 老基线无 host 字段，跳过延迟对比（重新 --save-baseline 后生效）"
                    .to_string(),
            );
            false
        }
        // 两侧都是 "unknown"（env 取不到）≠ 同机——未知身份不猜，跳过门。
        Some(h) if h == "unknown" || card.host == "unknown" => {
            findings.push(
                "[note] 本机或基线的 host 指纹缺失（unknown），无法确认同机，跳过延迟对比"
                    .to_string(),
            );
            false
        }
        Some(h) if h != card.host => {
            findings.push(format!(
                "[note] 基线来自其他机器（{h} vs 本机 {}），跳过延迟对比（跨机噪声不判门）",
                card.host
            ));
            false
        }
        Some(_) => true,
    };

    for bs in &base_scenarios {
        let name = bs.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let base_rate = bs.get("pass_rate").and_then(|v| v.as_f64()).unwrap_or(0.0);
        match card.scenarios.iter().find(|s| s.name == name) {
            Some(cur) => {
                if cur.pass_rate() < base_rate - 1e-9 {
                    regressions += 1;
                    findings.push(format!(
                        "场景 `{name}` 通过率回退：{:.1}% → {:.1}%",
                        base_rate * 100.0,
                        cur.pass_rate() * 100.0
                    ));
                } else if latency_gate {
                    let base_p50 = bs
                        .get("latency_ms")
                        .and_then(|l| l.get("p50"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let cur_p50 = cur.percentile(0.5);
                    // 同机相对门：基线 p50 有效（>0）才比；偏差超容忍 = 回退。
                    if base_p50 > 0 && cur_p50 as f64 > base_p50 as f64 * (1.0 + latency_tolerance)
                    {
                        regressions += 1;
                        findings.push(format!(
                            "场景 `{name}` p50 延迟回退：{base_p50}ms → {cur_p50}ms（+{:.0}%，容忍 +{:.0}%，同机相对门）",
                            (cur_p50 as f64 / base_p50 as f64 - 1.0) * 100.0,
                            latency_tolerance * 100.0
                        ));
                    }
                }
            }
            None => {
                regressions += 1;
                findings.push(format!("场景 `{name}` 本轮未执行（覆盖缩水）"));
            }
        }
    }
    if regressions == 0 {
        findings.insert(0, "[ok] 无回退".to_string());
    }
    Ok(findings)
}

// ---------------------------------------------------------------------------
// chrome trace 导出（A3，2026-10-21 前代实现思路引入）
// ---------------------------------------------------------------------------

/// 场景轮次 → Chrome Trace Event Format（`trace viewer` 可直接打开）。
/// 每个通过轮 = 一条 complete 事件（ph=X）；pid 恒 1（网关进程），tid 按
/// 场景序（并发场景内部并行在一条轨道上折叠，够看时序）。失败轮无时长
/// 不入轨（诊断在记分卡 failure_notes）。
pub fn export_chrome_trace(card: &Scorecard, path: &Path) -> Result<()> {
    let mut events = Vec::new();
    // tid 从 1 起（0 留给主轨惯例）；enumerate + offset 避免显式计数器。
    for (tid, s) in (1usize..).zip(card.scenarios.iter()) {
        for (i, (&lat, &off)) in s.latencies_ms.iter().zip(s.offsets_ms.iter()).enumerate() {
            events.push(json!({
                "name": format!("{} #{}", s.name, i + 1),
                "cat": "agent-bench",
                "ph": "X",
                "ts": off * 1000, // ms → µs（trace 时间轴单位）
                "dur": lat * 1000,
                "pid": 1,
                "tid": tid,
                "args": { "model": s.model, "pass": true },
            }));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(&json!({
            "traceEvents": events,
            "displayTimeUnit": "ms",
            "metadata": {
                "host": card.host,
                "gateway": card.gateway_version,
                "started_at": card.started_at,
            },
        }))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
