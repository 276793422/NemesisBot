//! Draft store for AI-generated workflow definitions (对话生成 two-phase flow).
//!
//! The `workflow` agent tool (action=create) writes drafts; applying is a
//! separate explicit step (draft = zero side effects on registered
//! workflows). Drafts live in
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
    /// 意图」的提示，见 [`crate::lint`]。生成器据 `workflow` 工具
    /// action=create 响应的 hints 当轮自纠。
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
/// **单一真相源**（BUG-20261125-073）：engine.rs 的 persist/清理/读取/
/// execution 记录面同样消费本函数（`use crate::drafts::sanitize_workflow_filename`
///），旧的双副本逐字节镜像已删——漂移即丢保留名中和。
pub fn sanitize_workflow_filename(name: &str) -> String {
    if name.is_empty() {
        return "wf_unnamed".to_string();
    }
    // 白名单（ASCII 字母数字 + `_` + `-`）已把 `.`、空白、路径分隔符、
    // 控制字符全部映射为 `_`——`.`/`..` 穿越形态与隐藏文件首点在映射后
    // 结构性不可能出现（旧实现的 `starts_with('.')` 分支永假，已删）。
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
    // Windows 保留设备名中和（BUG-20261125-073）：白名单全字母数字的
    // `con`/`nul` 等此前逐字节直通——`<defs>/con.yaml` / `<drafts>/nul.yaml`
    // 在旧 Windows（10/Server）设备解析（创建诡误/写入设备/静默丢数据），
    // 与 nemesis-utils `sanitize_path_segment` 出口中和（BUG-20261111-083）
    // 同威胁面同形态——本消毒器是该家族漏网。前缀 `_` 断开设备名解析
    // （保留名 ≤4 字符，+1 无越界风险）。判定时机在白名单映射**之后**：
    // 输入里的 `.` 已映射为 `_`，故 `con.yaml` 输入产出 `con_yaml`
    //（基名 `con_yaml` 非 `con`，天然安全不命中）；命中的是白名单后
    // 恰为纯保留名的形态（输入仅由保留名自身字符组成，任意大小写）。
    if nemesis_utils::sanitize::is_windows_reserved_device_base(&sanitized) {
        return format!("_{sanitized}");
    }
    sanitized
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
        // 原子写（BUG-20261117-046）：草稿是覆盖写的用户资产——写中段
        // 进程死亡/磁盘满 = 半截 YAML 覆盖掉上一版（list 虽诚实报
        // valid=false，但草稿内容已丢）。委托统一 helper（唯一临时名 +
        // sync + rename + 失败清理），与 save_config / persist_workflow
        // 同纪律。
        nemesis_utils::file::write_file_atomic(
            &path.to_string_lossy(),
            workflow_to_yaml(wf)?.as_bytes(),
            0o600,
        )
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

        // Back up the current definition before overwriting (only when one
        // exists). 三变体全覆盖（BUG-20261119-188 同族）：既有定义可能落
        // .yml/.json（load_workflows_from_dir 接受三个扩展名）——只查 .yaml
        // 会漏备份旧 .yml，而旧 .yml 随后会被 persist_workflow 的同名单名
        // 变体清理删除，备份契约（"AI 改坏了我的正式工作流"一瞥回滚）失守。
        let mut replaced_existing = false;
        let mut backup_file = None;
        let hist_dir = self.defs_dir.join(".history");
        for ext in ["yaml", "yml", "json"] {
            let existing = self.defs_dir.join(format!("{}.{}", stem, ext));
            if !existing.exists() {
                continue;
            }
            std::fs::create_dir_all(&hist_dir)
                .map_err(|e| format!("create history dir {:?}: {}", hist_dir, e))?;
            // Millisecond precision: two applies inside the same second must
            // not silently overwrite each other's backup (fs::copy replaces
            // the destination) — an agent iterating create→apply without
            // runs can easily produce rapid consecutive applies.
            let ts = chrono::Local::now().format("%Y%m%d_%H%M%S%3f");
            let backup_name = format!("{}.{}.{}", stem, ts, ext);
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
        let _ = nemesis_utils::fsutil::remove_file_resilient(self.draft_path(&stem));

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
        // 深度闸（BUG-20261110-044 同族接线）：草稿可由模型生成写入
        //（workflow_chat 辅助编辑），serde_yaml 深嵌套栈溢出闸同 SKILL.md。
        nemesis_utils::yaml_depth::check_yaml_nesting_depth(&text)
            .map_err(|e| format!("parse draft {:?}: {}", stem, e))?;
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
