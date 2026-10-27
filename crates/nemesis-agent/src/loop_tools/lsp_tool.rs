//! L1 (U19): the agent-facing `lsp` tool — read-only semantic code queries
//! (definition / references / implementation / hover) plus C7's write-capable
//! `rename` (server-computed WorkspaceEdit applied through the security
//! pipeline) and `code_action` quickfix listing. Driven through real language
//! servers by `nemesis-lsp`.
//!
//! Registration follows the delegation-tool PATH-probe pattern (loop_tools.rs): the
//! tool exists for the model only when (a) config opted in via
//! `agents.lsp_tool.enabled` AND (b) at least one language server was
//! found on PATH at registration. `registration_plan` encodes that
//! decision as a pure function so acceptance ② (missing server ⇒ not
//! registered) is unit-testable without touching PATH.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nemesis_lsp::{AppliedFileEdit, LspManager, LspOp};

use crate::context::RequestContext;
use crate::loop_tools::{Tool, WorkspaceBoundary};

pub struct LspTool {
    manager: Arc<LspManager>,
    /// M3（2026-10-27 整改）：rename 落盘的工作区边界（与 write_file 等文件
    /// 工具同源的第二道闸）。`gate_and_write` 对 WorkspaceEdit 逐文件过
    /// `validate_workspace_path`，越界文件进拒绝清单不落盘；相对路径解析
    /// 基点也随边界根（无边界维持 cwd 兼容——测试 / 独立 runner 形态）。
    boundary: Option<Arc<WorkspaceBoundary>>,
    /// C7：rename 落盘闸宿主（SecurityPlugin）。rename 的每文件新全文以
    /// 合成 `write_file` 调用走完整 8 层管线（注入/凭据/DLP/scanner 见真
    /// 内容），auditor 审批在 `execute()` 内同步生效；guardian 语义二审
    /// 经 `guardian_gate` 与 dispatch 同源（BUG-20261025-37：此前文档
    /// 声称继承但实现缺失，high 模式下同内容直写会被送审、rename 直落）。
    /// None（feature 裁剪 /
    /// 插件未注入）时 rename 直接落盘——与 executor 分离前普通写工具的
    /// 无闸形态一致，语义诚实（gate 缺席≠gate 通过）。
    #[cfg(feature = "security")]
    pub(crate) security: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    pub(crate) security: Option<()>,
}

impl LspTool {
    /// `timeout_secs`: per-request LSP budget (None = manager's 120s
    /// default). `idle_secs`: idle session reap threshold (None = 600s).
    ///
    /// Self-built-manager fallback path (tests, standalone agent runners).
    /// Production goes through [`LspTool::with_manager`] so the gateway owns
    /// the singleton and can `shutdown_all()` on graceful exit (C5).
    pub fn new(timeout_secs: Option<u64>, idle_secs: Option<u64>) -> Self {
        Self {
            manager: Arc::new(LspManager::new(
                timeout_secs.map(Duration::from_secs),
                idle_secs.map(Duration::from_secs),
            )),
            boundary: None,
            security: None,
        }
    }

    /// C5（2026-09-04 devtool-upgrade）: external-manager construction —
    /// agent_factory creates ONE `Arc<LspManager>` in `SharedResources`,
    /// hands it here, to the web server (`set_lsp_manager`), and calls
    /// `shutdown_all()` on gateway teardown. Without this, the tool built
    /// its own manager that no shutdown path could reach (orphaned language
    /// servers relied on kill_on_drop).
    pub fn with_manager(manager: Arc<LspManager>) -> Self {
        Self {
            manager,
            boundary: None,
            security: None,
        }
    }

    /// M3（2026-10-27 整改）：挂工作区边界（生产装配形态）。rename 落盘
    /// 逐文件过 `validate_workspace_path`，相对路径解析基点 = 边界根。
    /// `new` / `with_manager` 签名不动（25+ 处测试调用点零波及）。
    pub fn with_boundary(mut self, boundary: Arc<WorkspaceBoundary>) -> Self {
        self.boundary = Some(boundary);
        self
    }

    /// The backing manager (C5 test/observability access).
    pub fn manager(&self) -> &Arc<LspManager> {
        &self.manager
    }

    /// Pure registration decision (acceptance ②): enabled in config AND
    /// at least one language server available. The caller probes PATH and
    /// passes the count; this fn owns only the policy.
    pub fn registration_plan(enabled: bool, available_langs: usize) -> bool {
        enabled && available_langs > 0
    }
}

#[async_trait]
impl Tool for LspTool {
    fn description(&self) -> String {
        "对代码做语义操作：查找定义/引用/实现、查看悬停信息、重命名符号（跨文件，rename 结果经安全审批后落盘）、列出当前位置的快速修复（code_action）。由真实语言服务器（rust-analyzer/gopls 等）驱动，比文本 grep 更精确（跨文件、按符号）。".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["definition", "references", "implementation", "hover", "rename", "code_action"],
                    "description": "Operation: definition (where is this symbol defined), references (all usages), implementation (impls of a trait/type), hover (type/doc at this position), rename (rename the symbol across files — requires new_name), code_action (list quickfixes at this position)."
                },
                "path": {
                    "type": "string",
                    "description": "Absolute file path (relative resolved against the working directory)."
                },
                "line": {
                    "type": "integer",
                    "description": "0-based line number."
                },
                "character": {
                    "type": "integer",
                    "description": "0-based column in UTF-16 code units (LSP convention)."
                },
                "new_name": {
                    "type": "string",
                    "description": "New symbol name (rename op only)."
                }
            },
            "required": ["op", "path", "line", "character"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;

        let op_str = v
            .get("op")
            .and_then(|o| o.as_str())
            .ok_or("Missing 'op' argument")?;
        let op = LspOp::parse(op_str).ok_or_else(|| {
            format!(
                "Invalid 'op' {:?} — valid values: definition | references | implementation | hover | rename | code_action",
                op_str
            )
        })?;

        let path_str = v
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or("Missing 'path' argument")?;
        let path = PathBuf::from(path_str);
        // M3：相对路径解析基点与文件工具同源——有边界按边界根（工作区），
        // 无边界回落进程 cwd（测试 / 独立 runner 形态）。旧注释声称「与
        // exec/file 工具同一 cwd 约定」失实：文件工具在生产形态锚定的是
        // 工作区根，不是 cwd。
        let path = if path.is_relative() {
            match self.boundary.as_deref() {
                Some(b) => b.root.join(path),
                None => std::env::current_dir()
                    .map_err(|e| format!("cannot resolve relative path {path_str:?}: {e}"))?
                    .join(path),
            }
        } else {
            path
        };

        let line = v
            .get("line")
            .and_then(|l| l.as_u64())
            .ok_or("Missing or non-integer 'line' argument")?;
        let character = v
            .get("character")
            .and_then(|c| c.as_u64())
            .ok_or("Missing or non-integer 'character' argument")?;
        // LSP positions are u32; reject absurd values early rather than
        // truncating a u64 silently.
        let line = u32::try_from(line).map_err(|_| "'line' out of range")?;
        let character = u32::try_from(character).map_err(|_| "'character' out of range")?;

        match op {
            LspOp::Rename => {
                let new_name = v
                    .get("new_name")
                    .and_then(|n| n.as_str())
                    .ok_or("Missing 'new_name' argument for rename op")?;
                self.execute_rename(&path, line, character, new_name, context)
                    .await
            }
            LspOp::CodeAction => {
                let actions = self.manager.code_actions(&path, line, character).await?;
                if actions.is_empty() {
                    return Ok("(no quickfix code actions at this position)".to_string());
                }
                let mut out = format!("{} code action(s) available:\n", actions.len());
                for (i, a) in actions.iter().enumerate() {
                    let kind = a.kind.as_deref().unwrap_or("unclassified");
                    let flags = [
                        a.is_preferred.then_some("preferred"),
                        a.has_edit.then_some("with edit"),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", ");
                    out.push_str(&format!(
                        "{}. [{kind}] {}{}\n",
                        i + 1,
                        a.title,
                        if flags.is_empty() {
                            String::new()
                        } else {
                            format!(" ({flags})")
                        }
                    ));
                }
                // 列表只展示不执行（apply 后置）——明确告知模型如何落地。
                out.push_str(
                    "(listing only — to apply a fix, use edit_file with the suggested change)",
                );
                Ok(out)
            }
            _ => self.manager.query(op, &path, line, character).await,
        }
    }
}

impl LspTool {
    /// C7：rename 全链路。
    ///
    /// 1. 服务器算 WorkspaceEdit，逐文件读盘应用成新全文（manager::rename，
    ///    不写盘）；
    /// 2. **两阶段安全闸**：全部文件先各自过 8 层管线 + guardian 语义二审
    ///    （合成 `write_file` 调用，内容可见；guardian 走 guardian_gate 与
    ///    dispatch 同源），全部放行才进入写阶段——半改名状态是破碎代码，
    ///    宁可全有全无；
    /// 3. 逐文件写盘 → `notify_change` 回同步服务器文档 → 主文件等诊断
    ///    （编辑后闭环，复用 C2 的推送缓存：同一会话的 drain 顺带把其余
    ///    被改文件的推送也收进缓存）。
    async fn execute_rename(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        new_name: &str,
        context: &RequestContext,
    ) -> Result<String, String> {
        let outcome = self.manager.rename(path, line, character, new_name).await?;
        if outcome.files.is_empty() {
            let suffix = if outcome.errors.is_empty() {
                String::new()
            } else {
                format!("; errors: {}", outcome.errors.join("; "))
            };
            return Err(format!("rename produced no applicable file edits{suffix}"));
        }

        // Phase 1 + 2: gate ALL files, then write ALL files (atomic enough:
        // a mid-write failure is reported honestly with which files landed).
        // cfg 分支取引用（`()` 占位不 impl Deref，as_deref 在 no-security
        // 编译下不存在——单 crate 无 feature 构建曾在此炸 E0599，25 号
        // 雷区家族：workspace 全量 feature 统一会掩盖这个分支）。
        #[cfg(feature = "security")]
        let security = self.security.as_deref();
        #[cfg(not(feature = "security"))]
        let security = self.security.as_ref();
        let (written, rejected) =
            gate_and_write(&outcome.files, security, self.boundary.as_deref(), context).await?;

        // Phase 3: resync server docs and harvest diagnostics. notify_change
        // is best-effort (diagnostics closure is an optimization, not a
        // dependency — same policy as C3's edit path). 只同步实际落盘的文件
        // （被边界拒绝的文件内容未变，回灌服务器反而制造幻影文档状态）。
        let written_set: std::collections::HashSet<&str> =
            written.iter().map(|s| s.as_str()).collect();
        for f in outcome
            .files
            .iter()
            .filter(|f| written_set.contains(f.path.as_str()))
        {
            let _ = self
                .manager
                .notify_change(Path::new(&f.path), &f.new_text)
                .await;
        }
        let diags = self.manager.wait_for_diagnostics(path, 150, 2000).await;
        let errs = diags.iter().filter(|d| d.severity == 1).count();
        let warns = diags.iter().filter(|d| d.severity == 2).count();

        if written.is_empty() {
            // 全部文件被边界拒绝 = rename 一个字节都没落——诚实失败并把
            // 清单回给模型（与「rename produced no applicable file edits」
            // 同语义级别）。
            let detail = rejected
                .iter()
                .map(|(p, r)| format!("- {p}: {r}"))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(format!(
                "rename rejected by workspace boundary, no files were modified:\n{detail}"
            ));
        }

        let mut out = format!(
            "Renamed to {new_name:?}: {} file(s) written:\n",
            written.len()
        );
        for (i, f) in outcome
            .files
            .iter()
            .filter(|f| written_set.contains(f.path.as_str()))
            .enumerate()
        {
            out.push_str(&format!("{}. {} ({} edits)\n", i + 1, f.path, f.edit_count));
        }
        if !rejected.is_empty() {
            out.push_str("Not written (outside workspace boundary):\n");
            for (p, r) in &rejected {
                out.push_str(&format!("- {p}: {r}\n"));
            }
        }
        if !outcome.errors.is_empty() {
            out.push_str("Not applied (errors):\n");
            for e in &outcome.errors {
                out.push_str(&format!("- {e}\n"));
            }
        }
        if diags.is_empty() {
            out.push_str(&format!(
                "Diagnostics after rename ({}): none reported\n",
                path.display()
            ));
        } else {
            out.push_str(&format!(
                "Diagnostics after rename ({}): {errs} error(s), {warns} warning(s)\n",
                path.display()
            ));
            for d in diags.iter().take(20) {
                let sev = match d.severity {
                    1 => "error",
                    2 => "warning",
                    3 => "info",
                    _ => "hint",
                };
                out.push_str(&format!(
                    "- [{sev}] {}:{}:{}: {}\n",
                    d.source.as_deref().unwrap_or("lsp"),
                    d.range_start.0,
                    d.range_start.1,
                    d.message
                ));
            }
        }
        Ok(out)
    }
}

/// C7 两阶段落盘：先全部过安全闸（合成 `write_file` ToolInvocation 走完整
/// 8 层管线——注入/凭据/DLP/scanner 看到的是真实新全文；guardian 语义二审
/// 经 `guardian_gate` 与 dispatch 同源，BUG-20261025-37），全部放行才逐文
/// 件写盘。任一文件被拦（规则层或 guardian）→ 一个字节都不落盘（半改名=
/// 破碎代码）。无闸形态（feature 裁剪/插件未注入）直接写——gate 缺席≠gate
/// 通过，语义与无闸的普通写工具一致。
///
/// M3（2026-10-27 整改）：安全闸之前先做**逐文件工作区边界预分**——
/// `validate_workspace_path` 通过的文件才进闸+写阶段，越界文件进拒绝清单
/// （部分成功语义，与 rename 跨文件自然对齐：拒一处不连坐其余文件）。
/// 全部被拒时 written 为空，由调用方定失败语义。
///
/// 返回（实际落盘路径, 拒绝清单[(路径, 原因)]）；写盘中途失败时已落盘者
/// 如实上报（调用方把失败传给模型，由它决定补救——闸已全过，失败只可能
/// 是 IO 层）。
#[cfg(feature = "security")]
async fn gate_and_write(
    files: &[AppliedFileEdit],
    security: Option<&nemesis_security::pipeline::SecurityPlugin>,
    boundary: Option<&WorkspaceBoundary>,
    context: &RequestContext,
) -> Result<(Vec<String>, Vec<(String, String)>), String> {
    // 边界预分：越界文件不进闸（省审批面）也不落盘。界内文件带**解析后
    // 的绝对路径**进闸+落盘（相对路径锚定边界根）——预分只校验不替换落点
    // 的话，相对路径会写进进程 cwd（污染源树/丢文件，M3 实测抓到的缺陷）。
    // None 边界 = resolve 透传（行为同前）。
    let mut in_scope: Vec<(&AppliedFileEdit, std::path::PathBuf)> = Vec::with_capacity(files.len());
    let mut rejected = Vec::new();
    for f in files {
        match crate::loop_tools::resolve_tool_path(&f.path, boundary) {
            Ok(resolved) => in_scope.push((f, resolved)),
            Err(reason) => rejected.push((f.path.clone(), reason)),
        }
    }

    if let Some(security) = security {
        let approval_ctx = nemesis_security::auditor::ApprovalContext {
            channel: context.channel.clone(),
            chat_id: context.chat_id.clone(),
            session_key: context.session_key.clone(),
            ..Default::default()
        };
        for (f, resolved) in &in_scope {
            let args = serde_json::json!({"path": resolved, "content": f.new_text});
            let invocation = nemesis_security::types::ToolInvocation {
                tool_name: "write_file".to_string(),
                args: args.clone(),
                user: String::new(),
                source: context.channel.clone(),
                metadata: std::collections::HashMap::new(),
            };
            let (allowed, deny) = security.execute(&invocation);
            if !allowed {
                let info = deny.unwrap_or_else(|| nemesis_security::types::DenyInfo {
                    layer: "unknown",
                    policy: "security_pipeline".to_string(),
                    summary: "operation denied by security policy".to_string(),
                    suggestion: None,
                });
                return Err(format!(
                    "⛔ SECURITY BLOCKED rename write to {}: [{}:{}] {} — no files were modified. Inform the user that the operation was rejected.",
                    resolved.display(),
                    info.layer,
                    info.policy,
                    info.summary
                ));
            }
            // BUG-20261025-37：guardian 与 dispatch 同源（单一真相源
            // guardian_gate）。合成 write_file 无 op 投影，危级走静态表——
            // 与 dispatch 无 override 路径完全同口径。
            let danger = security.tool_danger_level("write_file");
            if let crate::guardian_gate::GuardianGate::Blocked(text) =
                crate::guardian_gate::guardian_gate(
                    security,
                    "write_file",
                    &args.to_string(),
                    &danger,
                    &approval_ctx,
                )
                .await
            {
                return Err(format!("{text} — no files were modified."));
            }
        }
    }
    let mut written = Vec::with_capacity(in_scope.len());
    for (f, resolved) in &in_scope {
        std::fs::write(resolved, &f.new_text)
            .map_err(|e| format!("write {} failed: {e}", resolved.display()))?;
        written.push(resolved.to_string_lossy().into_owned());
    }
    Ok((written, rejected))
}

/// 非 security 形态：无闸直写（签名对齐让调用方 cfg-free）。边界预分同上
/// （界内文件以解析后的绝对路径落盘——相对路径锚定边界根，不碰进程 cwd）。
#[cfg(not(feature = "security"))]
async fn gate_and_write(
    files: &[AppliedFileEdit],
    _security: Option<&()>,
    boundary: Option<&WorkspaceBoundary>,
    _context: &RequestContext,
) -> Result<(Vec<String>, Vec<(String, String)>), String> {
    let mut in_scope: Vec<(&AppliedFileEdit, std::path::PathBuf)> = Vec::with_capacity(files.len());
    let mut rejected = Vec::new();
    for f in files {
        match crate::loop_tools::resolve_tool_path(&f.path, boundary) {
            Ok(resolved) => in_scope.push((f, resolved)),
            Err(reason) => rejected.push((f.path.clone(), reason)),
        }
    }
    let mut written = Vec::with_capacity(in_scope.len());
    for (f, resolved) in &in_scope {
        std::fs::write(resolved, &f.new_text)
            .map_err(|e| format!("write {} failed: {e}", resolved.display()))?;
        written.push(resolved.to_string_lossy().into_owned());
    }
    Ok((written, rejected))
}

#[cfg(test)]
mod tests;

// 覆盖率补充批次：execute 参数校验决策表 / registration_plan /
// gate_and_write 无闸直写。
#[cfg(test)]
mod cov_tests;
