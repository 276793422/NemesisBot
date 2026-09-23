//! Draft store for AI-generated workflow definitions (对话生成 two-phase flow).
//!
//! `workflow_create` writes drafts; applying is a separate explicit step
//! (draft = zero side effects on registered workflows). Drafts live in
//! `<defs_dir>/../drafts/` as `<sanitized-name>.yaml` — same serializer as
//! real definitions so a draft's bytes are exactly what `persist_workflow`
//! would write.
//!
//! The apply path is the only way an AI-generated definition reaches the
//! registry: re-validate → back up any existing definition to
//! `<defs_dir>/.history/<name>.<ts>.yaml` → `persist_workflow` → delete the
//! draft. The backup is what makes "AI 改坏了我的正式工作流" a one-glance
//! rollback instead of a reconstruction job.

use std::path::{Path, PathBuf};

use crate::engine::WorkflowEngine;
use crate::types::Workflow;

/// A pending draft, as returned by list/get.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DraftSummary {
    /// Workflow name (unsanitized — as it will register).
    pub name: String,
    /// File stem on disk (sanitized form).
    pub file_stem: String,
    /// Last modified, unix millis.
    pub mtime_ms: u64,
    /// True when the draft passes `parser::validate` right now.
    pub valid: bool,
    /// Validation errors (empty when `valid`).
    pub validation_errors: Vec<String>,
    /// 语义 lint 结果（warning 通道，不阻断 apply）——「能跑但大概率不合
    /// 意图」的提示，见 [`crate::lint`]。生成器据 workflow_create 响应的
    /// hints 当轮自纠。
    #[serde(default)]
    pub warnings: Vec<String>,
    pub node_count: usize,
    pub trigger_types: Vec<String>,
}

/// Full draft detail: summary + parsed definition + raw YAML text.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DraftDetail {
    #[serde(flatten)]
    pub summary: DraftSummary,
    pub workflow: Workflow,
    pub yaml: String,
}

/// Result of applying a draft.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppliedDraft {
    /// Workflow name as registered.
    pub name: String,
    /// True when a previous definition was backed up to `.history/`.
    pub replaced_existing: bool,
    /// Backup file name (inside `<defs_dir>/.history/`), when replaced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_file: Option<String>,
}

/// Compute the drafts directory from the definitions directory:
/// `<defs_dir>/../drafts/` (sibling of `definitions/`).
pub fn drafts_dir_from_defs(defs_dir: &Path) -> PathBuf {
    defs_dir
        .parent()
        .map(|p| p.join("drafts"))
        .unwrap_or_else(|| defs_dir.join("drafts"))
}

/// Sanitize a workflow name for use as a draft/definition filename.
/// Mirrors `engine.rs::sanitize_workflow_filename` (kept in sync by a test
/// in engine.rs's test module exercising both via persist + draft roundtrip).
pub fn sanitize_workflow_filename(name: &str) -> String {
    if name.is_empty() {
        return "wf_unnamed".to_string();
    }
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.starts_with('.') {
        format!("wf_{}", sanitized)
    } else {
        sanitized
    }
}

/// Serialize a workflow to the canonical on-disk YAML form.
fn workflow_to_yaml(wf: &Workflow) -> Result<String, String> {
    serde_yaml::to_string(wf).map_err(|e| format!("serialize workflow {:?}: {}", wf.name, e))
}

/// Run structural validation, collecting all errors (empty = valid).
fn validation_errors(wf: &Workflow) -> Vec<String> {
    match crate::parser::validate(wf) {
        Ok(()) => Vec::new(),
        Err(e) => vec![e],
    }
}

// ---------------------------------------------------------------------------
// DraftStore
// ---------------------------------------------------------------------------

/// Filesystem-backed draft store. All operations are synchronous and cheap
/// (small YAML files, called from async contexts via `spawn_blocking`-free
/// direct calls — file sizes are ~KB and callers already tolerate the
/// blocking time in the WSAPI handler path, same as `persist_workflow`).
pub struct DraftStore {
    dir: PathBuf,
    defs_dir: PathBuf,
}

impl DraftStore {
    /// Build a store rooted next to the engine's definitions directory.
    /// Returns `None` when the engine has no `workflow_defs_dir` configured
    /// (drafts without a persistence target would be unappliable — we refuse
    /// early so the tool can tell the LLM why).
    pub fn from_engine(engine: &WorkflowEngine) -> Option<Self> {
        let defs_dir = engine.workflow_defs_dir()?;
        Some(Self {
            dir: drafts_dir_from_defs(&defs_dir),
            defs_dir,
        })
    }

    /// Direct constructor (tests / non-standard roots).
    pub fn new(dir: PathBuf, defs_dir: PathBuf) -> Self {
        Self { dir, defs_dir }
    }

    /// Persist (or overwrite) a draft. Returns the summary including current
    /// validation state — the writer decides whether errors block (apply
    /// does; the draft write itself never does).
    pub fn save(&self, wf: &Workflow) -> Result<DraftSummary, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("create drafts dir {:?}: {}", self.dir, e))?;
        let stem = sanitize_workflow_filename(&wf.name);
        let path = self.dir.join(format!("{}.yaml", stem));
        std::fs::write(&path, workflow_to_yaml(wf)?)
            .map_err(|e| format!("write draft {:?}: {}", path, e))?;
        Ok(self.summarize(&stem, wf))
    }

    /// List all pending drafts, newest first. Unparsable drafts still appear
    /// (valid=false + error) so nothing silently vanishes.
    pub fn list(&self) -> Vec<DraftSummary> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(_) => return out, // no drafts dir yet = empty list
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let stem = stem.to_string();
            match self.load_workflow(&stem) {
                Ok(wf) => out.push(self.summarize(&stem, &wf)),
                Err(e) => out.push(DraftSummary {
                    name: stem.clone(),
                    file_stem: stem,
                    mtime_ms: mtime_ms(&path).unwrap_or(0),
                    valid: false,
                    validation_errors: vec![e],
                    warnings: Vec::new(),
                    node_count: 0,
                    trigger_types: Vec::new(),
                }),
            }
        }
        out.sort_by_key(|d| std::cmp::Reverse(d.mtime_ms));
        out
    }

    /// Load one draft in full detail.
    pub fn get(&self, name: &str) -> Result<DraftDetail, String> {
        let stem = sanitize_workflow_filename(name);
        let yaml = std::fs::read_to_string(self.draft_path(&stem))
            .map_err(|e| format!("draft {:?} not readable: {}", name, e))?;
        let wf = self.load_workflow(&stem)?;
        let mut summary = self.summarize(&stem, &wf);
        summary.name = wf.name.clone(); // summarize seeds from file stem; report the real name
        Ok(DraftDetail {
            summary,
            workflow: wf,
            yaml,
        })
    }

    /// Discard a draft. Idempotent: discarding a nonexistent draft is Ok.
    pub fn discard(&self, name: &str) -> Result<(), String> {
        let stem = sanitize_workflow_filename(name);
        let path = self.draft_path(&stem);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("remove draft {:?}: {}", path, e)),
        }
    }

    /// Apply a draft: re-validate → back up any existing definition →
    /// `persist_workflow` (validates again + writes + registers) → delete
    /// the draft. Only this function moves a definition from draft state to
    /// registry state.
    pub fn apply(&self, engine: &WorkflowEngine, name: &str) -> Result<AppliedDraft, String> {
        let stem = sanitize_workflow_filename(name);
        let wf = self.load_workflow(&stem)?;

        let errors = validation_errors(&wf);
        if !errors.is_empty() {
            return Err(format!(
                "draft {:?} failed validation: {}",
                name,
                errors.join("; ")
            ));
        }

        // Back up the current definition before overwriting (only when one exists).
        let existing = self.defs_dir.join(format!("{}.yaml", stem));
        let mut replaced_existing = false;
        let mut backup_file = None;
        if existing.exists() {
            let hist_dir = self.defs_dir.join(".history");
            std::fs::create_dir_all(&hist_dir)
                .map_err(|e| format!("create history dir {:?}: {}", hist_dir, e))?;
            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let backup_name = format!("{}.{}.yaml", stem, ts);
            std::fs::copy(&existing, hist_dir.join(&backup_name))
                .map_err(|e| format!("backup {:?}: {}", existing, e))?;
            replaced_existing = true;
            backup_file = Some(backup_name);
        }

        engine
            .persist_workflow(wf)
            .map_err(|e| format!("persist workflow {:?}: {}", name, e))?;

        // Registered successfully — remove the draft. A leftover on failure
        // here is harmless (next apply discards it after persisting).
        let _ = std::fs::remove_file(self.draft_path(&stem));

        Ok(AppliedDraft {
            name: name.to_string(),
            replaced_existing,
            backup_file,
        })
    }

    // -- internals ---------------------------------------------------------

    fn draft_path(&self, stem: &str) -> PathBuf {
        self.dir.join(format!("{}.yaml", stem))
    }

    fn load_workflow(&self, stem: &str) -> Result<Workflow, String> {
        let text = std::fs::read_to_string(self.draft_path(stem))
            .map_err(|e| format!("read draft {:?}: {}", stem, e))?;
        serde_yaml::from_str(&text).map_err(|e| format!("parse draft {:?}: {}", stem, e))
    }

    fn summarize(&self, stem: &str, wf: &Workflow) -> DraftSummary {
        let errors = validation_errors(wf);
        DraftSummary {
            name: wf.name.clone(),
            file_stem: stem.to_string(),
            mtime_ms: mtime_ms(&self.draft_path(stem)).unwrap_or(0),
            valid: errors.is_empty(),
            validation_errors: errors,
            warnings: crate::lint::lint(wf),
            node_count: wf.nodes.len(),
            trigger_types: wf.triggers.iter().map(|t| t.trigger_type.clone()).collect(),
        }
    }
}

fn mtime_ms(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests;
