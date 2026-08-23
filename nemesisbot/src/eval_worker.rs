//! eval-agent role worker: runs the agent loop INSIDE the NemesisEvalBox
//! sandbox under `nemesisbot eval`.
//!
//! Mirrors the executor role short-circuit pattern (`exec_worker.rs`): the
//! workspace comes from env `NEMESISBOT_EVAL_WORKSPACE`, path resolution is
//! never re-run, and the process exits when the evaluation is done.
//!
//! Responsibilities (plan Step 2 + Step 7):
//! 1. Load the sanitized minimal config from the temporary workspace.
//! 2. Assemble a SecurityPlugin with `enabled=false` (no interception — the
//!    evaluation observes the prompt/skill's natural behaviour) but with the
//!    detection engines constructed, so the tagging observer can call them.
//! 3. Build the agent loop via the standard factory (LLM goes through the
//!    local proxy; fake key; executor separation off — process-level sandbox).
//! 4. Register the EvalTaggingObserver (records every tool call with 8-layer
//!    findings — plan record points ①+②).
//! 5. Run the subject via process_direct, write the report, exit.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};

/// Entry point — called from main.rs before CLI parsing.
pub async fn run() -> Result<()> {
    // Every error path also lands in <workspace>/worker_error.txt (inside the
    // box mirror) so the headless sandbox run is diagnosable — stderr/stdout
    // are swallowed by Start.exe and never reach the command process.
    if let Err(e) = run_inner().await {
        if let Ok(ws) = std::env::var("NEMESISBOT_EVAL_WORKSPACE") {
            let _ = std::fs::write(
                std::path::Path::new(&ws).join("worker_error.txt"),
                format!("{e:#}"),
            );
        }
        return Err(e);
    }
    Ok(())
}

async fn run_inner() -> Result<()> {
    let workspace = PathBuf::from(
        std::env::var("NEMESISBOT_EVAL_WORKSPACE")
            .context("NEMESISBOT_EVAL_WORKSPACE not set — eval-agent must be spawned by `nemesisbot eval`")?,
    );

    // Entry marker (in-box mirror) — proves the worker main was reached and
    // when it died, even when everything else fails.
    let _ = std::fs::write(
        workspace.join("worker_alive.txt"),
        format!("entry pid={} role={:?}", std::process::id(), std::env::var("NEMESISBOT_ROLE")),
    );

    let prompt = std::env::var("NEMESISBOT_EVAL_PROMPT").unwrap_or_else(|_| {
        // Fallback: read the subject file written by the command process.
        let p = workspace.join("subject.txt");
        std::fs::read_to_string(&p).unwrap_or_default()
    });

    tracing::info!("[eval-agent] workspace={} prompt_len={}", workspace.display(), prompt.len());

    // 1. Load the sanitized config from the temporary home.
    let config_path = workspace.join("config.json");
    let cfg = nemesis_config::load_config(&config_path)
        .with_context(|| format!("load eval config {}", config_path.display()))?;
    // U15: if the sanitized config carries `yaml:<alias>` api_key references,
    // resolve them against the eval home's credentials.yaml (missing file
    // fails loud with the remedy — no silent empty key).
    nemesis_config::credentials::set_global_credentials_path(
        nemesis_config::credentials::credentials_path_for_home(&workspace),
    );
    let config_store = Arc::new(nemesis_config::ConfigStore::from_config(cfg, config_path));

    // 2. SecurityPlugin: enabled=false → the pipeline short-circuits to allow
    //    (no interception), but the layer engines are constructed normally so
    //    the tagging observer can call them through the accessors.
    let security_plugin = build_security_plugin(&workspace);

    // 3. Assemble SharedResources with the temporary home.
    let shared = Arc::new(crate::agent_factory::SharedResources {
        home: workspace.clone(),
        config_store,
        security_plugin: Some(Arc::new(security_plugin)),
        mcp_enabled: false,
        mcp_config_path: PathBuf::default(),
        ..Default::default()
    });

    let mut agent_loop = crate::agent_factory::build_agent_loop(&shared)
        .context("build agent loop for eval-agent")?;

    // 4. Register the tagging observer (record points ①+②).
    let observer_manager = Arc::new(nemesis_observer::Manager::new());
    let tagger = Arc::new(EvalTaggingObserver::new(&shared));
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            observer_manager
                .register(tagger.clone() as Arc<dyn nemesis_observer::Observer>)
                .await;
        })
    });
    // build_agent_loop returns an Arc; no clones exist yet, so get_mut is
    // guaranteed to succeed right after construction.
    if let Some(al) = Arc::get_mut(&mut agent_loop) {
        al.set_observer_manager(observer_manager);
    } else {
        anyhow::bail!("agent loop Arc already shared — cannot attach observer");
    }

    // 5. Run the subject.
    let session_key = format!(
        "eval:{}",
        chrono::Local::now().format("%Y%m%d%H%M%S")
    );
    tracing::info!("[eval-agent] running subject (session {})", session_key);
    let response = agent_loop
        .process_direct(&prompt, &session_key)
        .await
        .map_err(|e| anyhow::anyhow!("agent loop error: {e}"))?;

    // 6. Write the report into the workspace (lands in the box mirror).
    let report_dir = workspace.join("logs").join("eval");
    std::fs::create_dir_all(&report_dir)?;
    std::fs::write(report_dir.join("final_response.md"), &response)?;
    let tags = tagger.take_tags();
    std::fs::write(
        report_dir.join("tool_trace.json"),
        serde_json::to_string_pretty(&tags).unwrap_or_else(|_| "[]".to_string()),
    )?;
    let summary = summarize(&tags);
    std::fs::write(
        report_dir.join("security_findings.json"),
        serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".to_string()),
    )?;
    // Done marker is written by the COMMAND process (outside the box); the
    // monitor shell watches the box process list, not this file.
    tracing::info!(
        "[eval-agent] report written to {} ({} tool calls)",
        report_dir.display(),
        tags.len()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// SecurityPlugin construction (enabled=false, engines live)
// ---------------------------------------------------------------------------

fn build_security_plugin(home: &std::path::Path) -> nemesis_security::pipeline::SecurityPlugin {
    // Defaults construct every layer engine (injection detector, command
    // guard, credential scanner, DLP engine, SSRF guard). `enabled=false`
    // makes `execute()` a pass-through, but the accessors still hand out the
    // engines for the tagging observer.
    let mut config = nemesis_security::pipeline::SecurityPluginConfig::default();
    config.enabled = false;
    let _ = home; // rules file loading is optional; defaults are enough for v1
    nemesis_security::pipeline::SecurityPlugin::new(config)
}

// ---------------------------------------------------------------------------
// EvalTaggingObserver — record points ① + ② (plan Step 7)
// ---------------------------------------------------------------------------

use nemesis_observer::{ConversationEvent, EventData, EventType, Observer};
use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct ToolTag {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub result: Option<String>,
    pub success: bool,
    pub duration_ms: u64,
    pub llm_round: u32,
    pub timestamp: String,
    pub findings: LayerFindings,
}

#[derive(Serialize, Clone, Default)]
pub struct LayerFindings {
    pub injection: Option<InjectionFinding>,
    pub command_guard: Option<CommandFinding>,
    pub credentials_in: Option<Vec<String>>,
    pub credentials_out: Option<Vec<String>>,
    pub dlp_in: Option<Vec<String>>,
    pub dlp_out: Option<Vec<String>>,
    pub ssrf: Option<SsrfFinding>,
}

#[derive(Serialize, Clone)]
pub struct InjectionFinding {
    pub is_injection: bool,
    pub score: f64,
    pub level: String,
}

#[derive(Serialize, Clone)]
pub struct CommandFinding {
    pub blocked: bool,
    pub reason: String,
}

#[derive(Serialize, Clone)]
pub struct SsrfFinding {
    pub url: String,
    pub blocked: bool,
    pub reason: String,
}

#[derive(Serialize)]
struct FindingsSummary {
    total_tool_calls: usize,
    injection_hits: usize,
    blocked_commands: usize,
    credential_hits_in: usize,
    credential_hits_out: usize,
    dlp_hits_in: usize,
    dlp_hits_out: usize,
    ssrf_blocks: usize,
}

pub struct EvalTaggingObserver {
    /// Engines borrowed through the plugin accessors (plugin itself is
    /// disabled — analysis only, never interception).
    plugin: Arc<nemesis_security::pipeline::SecurityPlugin>,
    tags: std::sync::Mutex<Vec<ToolTag>>,
}

impl EvalTaggingObserver {
    fn new(shared: &Arc<crate::agent_factory::SharedResources>) -> Self {
        // Build a dedicated analysis-only plugin (not the gateway's).
        let _ = shared;
        Self {
            plugin: Arc::new(build_security_plugin(&shared.home)),
            tags: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn take_tags(&self) -> Vec<ToolTag> {
        std::mem::take(&mut self.tags.lock().unwrap())
    }

    #[allow(clippy::too_many_arguments)]
    fn push_tag(&self, tag: ToolTag) {
        self.tags.lock().unwrap().push(tag);
    }
}

#[async_trait::async_trait]
impl Observer for EvalTaggingObserver {
    fn name(&self) -> &str {
        "eval-tagging"
    }

    async fn on_event(&self, event: ConversationEvent) {
        if event.event_type != EventType::ToolCall {
            return;
        }
        let EventData::ToolCall(d) = event.data else {
            return;
        };
        let args_value = serde_json::to_value(&d.arguments).unwrap_or(serde_json::Value::Null);
        let args_str = serde_json::to_string(&d.arguments).unwrap_or_default();
        let result_str = d.result.clone().unwrap_or_default();

        let findings = self.run_layers(&d.tool_name, &args_value, &args_str, &result_str).await;

        self.push_tag(ToolTag {
            tool_name: d.tool_name,
            arguments: args_value,
            result: d.result,
            success: d.success,
            duration_ms: d.duration.as_millis() as u64,
            llm_round: d.llm_round,
            timestamp: event.timestamp.to_rfc3339(),
            findings,
        });
    }
}

impl EvalTaggingObserver {
    /// Layer-by-layer analysis via the plugin accessors — deliberately NOT
    /// `SecurityPlugin::execute` (that short-circuits on the first denial and
    /// would drop the remaining layers' findings).
    async fn run_layers(
        &self,
        tool: &str,
        args_value: &serde_json::Value,
        args_str: &str,
        result: &str,
    ) -> LayerFindings {
        let mut out = LayerFindings::default();

        // L1 injection
        if let Some(det) = self.plugin.injection_detector() {
            let r = det.analyze_tool_input(tool, args_value);
            out.injection = Some(InjectionFinding {
                is_injection: r.is_injection,
                score: r.score,
                // level is already a lowercase string ("low".."critical") — clone
                // it directly. (Was `format!("{:?}", r.level)` which serialized
                // as "\"low\"" — doubly-quoted, forcing rule authors into
                // four-escape regexes. Fixed per assessor plan C2.)
                level: r.level.clone(),
            });
        }

        // L2 command guard (exec-like tools with a command field)
        if let Some(guard) = self.plugin.command_guard() {
            if let Some(cmd) = args_value.get("command").and_then(|v| v.as_str()) {
                match guard.check(cmd) {
                    Err(e) => {
                        out.command_guard = Some(CommandFinding {
                            blocked: true,
                            reason: e.to_string(),
                        });
                    }
                    Ok(()) => {
                        out.command_guard = Some(CommandFinding {
                            blocked: false,
                            reason: String::new(),
                        });
                    }
                }
            }
        }

        // L3 credentials — args in, result out
        if let Some(scanner) = self.plugin.credential_scanner() {
            let r_in = scanner.scan_content(args_str);
            out.credentials_in = if r_in.has_matches { Some(vec![r_in.summary]) } else { None };
            if !result.is_empty() {
                let r_out = scanner.scan_tool_output(tool, result);
                out.credentials_out = if r_out.has_matches {
                    Some(vec![r_out.summary])
                } else {
                    None
                };
            }
        }

        // L4 DLP — args in, result out
        if let Some(dlp) = self.plugin.dlp_engine() {
            let r_in = dlp.scan_tool_input(tool, args_value);
            out.dlp_in = if r_in.has_matches { Some(vec![r_in.summary]) } else { None };
            if !result.is_empty() {
                let r_out = dlp.scan_tool_output(tool, result);
                out.dlp_out = if r_out.has_matches { Some(vec![r_out.summary]) } else { None };
            }
        }

        // L5 SSRF (url-ish fields)
        if let Some(ssrf) = self.plugin.ssrf_guard() {
            for key in ["url", "endpoint", "base_url"] {
                if let Some(url) = args_value.get(key).and_then(|v| v.as_str()) {
                    out.ssrf = Some(match ssrf.validate_url(url) {
                        Err(e) => SsrfFinding {
                            url: url.to_string(),
                            blocked: true,
                            reason: e.to_string(),
                        },
                        Ok(()) => SsrfFinding {
                            url: url.to_string(),
                            blocked: false,
                            reason: String::new(),
                        },
                    });
                    break;
                }
            }
        }

        // L6 audit chain: append-only log, skipped for tagging.
        // L7 scanner / L8 guardian: v2 (plan).

        out
    }
}

fn summarize(tags: &[ToolTag]) -> FindingsSummary {
    let mut s = FindingsSummary {
        total_tool_calls: tags.len(),
        injection_hits: 0,
        blocked_commands: 0,
        credential_hits_in: 0,
        credential_hits_out: 0,
        dlp_hits_in: 0,
        dlp_hits_out: 0,
        ssrf_blocks: 0,
    };
    for t in tags {
        let f = &t.findings;
        if let Some(i) = &f.injection {
            if i.is_injection {
                s.injection_hits += 1;
            }
        }
        if let Some(c) = &f.command_guard {
            if c.blocked {
                s.blocked_commands += 1;
            }
        }
        if f.credentials_in.as_ref().is_some_and(|v| !v.is_empty()) {
            s.credential_hits_in += 1;
        }
        if f.credentials_out.as_ref().is_some_and(|v| !v.is_empty()) {
            s.credential_hits_out += 1;
        }
        if f.dlp_in.as_ref().is_some_and(|v| !v.is_empty()) {
            s.dlp_hits_in += 1;
        }
        if f.dlp_out.as_ref().is_some_and(|v| !v.is_empty()) {
            s.dlp_hits_out += 1;
        }
        if let Some(x) = &f.ssrf {
            if x.blocked {
                s.ssrf_blocks += 1;
            }
        }
    }
    s
}
