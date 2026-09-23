//! 消息组装：shell 注入族（K3）、expand_shell_injections、build_messages*、prefetch_memory_context、note_read_for_instructions、invalidate_context_digests、render_workflow_edit_section。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

// -------------------------------------------------------------------------
// K3（devtool-upgrade 阶段 4）：`` !`cmd` `` shell 注入
// -------------------------------------------------------------------------

/// K3：`` !`cmd` `` 注入的超时（秒）——模板注入是"顺手拿点上下文"，不是
/// 长任务入口；卡死命令 3s 收尸。
const SHELL_INJECTION_TIMEOUT_SECS: u64 = 3;
/// K3：注入 stdout 的字符上限（超出按字符边界截断——byte 切片会在多字节
/// 字符上 panic，见 str-slice 教训）。
pub(crate) const SHELL_INJECTION_OUTPUT_CAP: usize = 4096;

/// K3：扫描文本中所有 `` !`cmd` `` token，返回（token 全长 span, 命令原文
/// trim 后）。扫描按 byte 找 `!` + `` ` ``（都是单字节 ASCII，不会落在多字
/// 节序列内部，切片安全）；无闭合反引号的 `!` 按字面保留（不进列表）。
fn shell_injection_spans(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'!'
            && bytes[i + 1] == b'`'
            && let Some(j) = text[i + 2..].find('`')
        {
            let end = i + 2 + j + 1;
            out.push((i..end, text[i + 2..end - 1].trim()));
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

/// K3：提取去重后的注入命令清单（首次出现序）。
pub(crate) fn extract_shell_injections(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (_, cmd) in shell_injection_spans(text) {
        if !cmd.is_empty() && !out.iter().any(|c| c == cmd) {
            out.push(cmd.to_string());
        }
    }
    out
}

/// K3：把每个 `` !`cmd` `` token 替换为 `resolve(cmd)` 的产物（纯函数，
/// 测试对象）。空命令 token 与未闭合 token 保持字面不动（与
/// [`extract_shell_injections`] 的跳过语义对齐，两边永不分歧）。
pub(crate) fn substitute_shell_injections(text: &str, resolve: &dyn Fn(&str) -> String) -> String {
    let spans = shell_injection_spans(text);
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (span, cmd) in spans {
        out.push_str(&text[last..span.start]);
        if cmd.is_empty() {
            out.push_str(&text[span.start..span.end]);
        } else {
            out.push_str(&resolve(cmd));
        }
        last = span.end;
    }
    out.push_str(&text[last..]);
    out
}

/// K3：执行单个注入命令并产出替换文本。成功（exit 0）→ stdout 去尾随空白
/// 并截断（空输出给 `(no output)` 占位，让 LLM/模板有确定内容）；spawn 失败、
/// 超时或非零退出 → `[command failed: <cmd>: <原因>]` 注记（stderr 尾部
/// 随注记带上，≤200 字符，便于人诊断）。
async fn exec_shell_injection(cmd: &str, cwd: Option<&std::path::Path>) -> String {
    let (code, stdout, stderr, timed_out) =
        crate::loop_tools::run_one_stage(cmd, cwd, SHELL_INJECTION_TIMEOUT_SECS).await;
    if timed_out {
        return format!("[command failed: {cmd}: timed out after {SHELL_INJECTION_TIMEOUT_SECS}s]");
    }
    match code {
        Some(0) => {
            let trimmed = stdout.trim_end();
            if trimmed.is_empty() {
                "(no output)".to_string()
            } else {
                cap_injection_output(trimmed)
            }
        }
        Some(c) => {
            let reason = tail_line(&stderr, 200);
            if reason.is_empty() {
                format!("[command failed: {cmd}: exit {c}]")
            } else {
                format!("[command failed: {cmd}: exit {c}: {reason}]")
            }
        }
        None => {
            let reason = tail_line(&stderr, 200);
            format!("[command failed: {cmd}: {reason}]")
        }
    }
}

/// K3：注入输出按字符数截断（头 4096 字符 + 截断注记）。
pub(crate) fn cap_injection_output(s: &str) -> String {
    if s.chars().count() <= SHELL_INJECTION_OUTPUT_CAP {
        return s.to_string();
    }
    let mut out: String = s.chars().take(SHELL_INJECTION_OUTPUT_CAP).collect();
    out.push_str("…[truncated]");
    out
}

/// K3：取 stderr 的最后一个非空行并按字符数截断（注记保持单行可读）。
pub(crate) fn tail_line(s: &str, cap: usize) -> String {
    let line = s.lines().rev().map(str::trim).find(|l| !l.is_empty());
    let line = line.unwrap_or("");
    if line.chars().count() <= cap {
        line.to_string()
    } else {
        let mut out: String = line.chars().take(cap).collect();
        out.push('…');
        out
    }
}

impl AgentLoop {
    /// K3：对文本中的 `` !`cmd` `` 逐个执行并替换（相同命令只执行一次）。
    /// 无注入时零开销直返；cwd 取 workspace_root（未设则继承进程 cwd）。
    pub(crate) async fn expand_shell_injections(&self, text: String) -> String {
        let cmds = extract_shell_injections(&text);
        if cmds.is_empty() {
            return text;
        }
        let cwd = self.workspace_root.read().clone();
        let mut results: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for c in &cmds {
            let replaced = exec_shell_injection(c, cwd.as_deref()).await;
            results.insert(c.clone(), replaced);
        }
        substitute_shell_injections(&text, &|c| results.get(c).cloned().unwrap_or_default())
    }

    /// I3 (devtool-upgrade 阶段 3): lazy sub-directory instruction discovery.
    /// Called on a SUCCESSFUL read_file whose path resolves inside the
    /// workspace: if the file's directory (≠ workspace root — the root chain
    /// is always injected) carries AGENTS.md/CLAUDE.md and was never claimed
    /// this session, queue its contents for the next build's one-shot
    /// injection (same channel shape as the I1 external_changes section).
    ///
    /// Directories without instruction files stay UNCLAIMED so instructions
    /// added later (agent- or user-authored) are still discovered on a later
    /// read. Relative paths resolve against the workspace root (read_file
    /// accepts them); outside-root paths are silently ignored. Loop-level
    /// state is not needed — the instance owns claims+buffer (per-session
    /// dedup, drained at build).
    pub(crate) fn note_read_for_instructions(&self, instance: &AgentInstance, path: &str) {
        use nemesis_path::paths::canonicalize_for_compare;
        let root = match self.workspace_root.read().clone() {
            Some(r) => r,
            None => return,
        };
        let p = std::path::Path::new(path);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        };
        let Some(parent) = abs.parent() else {
            return;
        };
        let parent_canon = canonicalize_for_compare(parent);
        let root_canon = canonicalize_for_compare(&root);
        // Outside the workspace, or the root itself → not a lazy subdir.
        if !parent_canon.starts_with(&root_canon) || parent_canon == root_canon {
            return;
        }
        if instance.instruction_dir_claimed(&parent_canon) {
            return;
        }
        let files = crate::workspace_instructions::load_dir_instruction_files(parent);
        if files.is_empty() {
            return;
        }
        instance.claim_instruction_dir(parent_canon);
        instance.queue_pending_instructions(files);
        info!(
            "[AgentLoop] I3 lazy instructions: sub-directory {} queued for next build",
            parent.display()
        );
    }

    /// H5 (U18): touch-driven digest invalidation. Called by the dispatch
    /// path when a read_file/write_file/edit_file call touched a file that
    /// is on the session's instruction chain — the next build_messages
    /// re-reads the chain and re-injects. `session_key` is the stable key
    /// build_messages derived (first-user-content hash); the dispatch path
    /// does not know it, so this drops the WHOLE digest state entry list is
    /// not viable — instead we clear all session keys whose chain contains
    /// the touched path. Simplest correct form given the keying: clear ALL
    /// entries (re-inject once for every live session on the next build) —
    /// file touches on the chain are rare, so over-invalidation is cheap.
    pub fn invalidate_context_digests(&self) {
        self.skills_digest_state.clear_all();
    }

    /// P3.1 (sixth batch): retrieve relevant long-term memories for the
    /// LATEST user message (vector top-K, score-thresholded, deduped by
    /// pairwise cosine > 0.92 — goal §三 semantics). `None` whenever the
    /// feature is off (default), the manager/vector store is absent, or no
    /// hit clears the bar — `None` keeps build_messages byte-identical.
    #[cfg_attr(not(feature = "memory"), allow(unused_variables))]
    pub(crate) async fn prefetch_memory_context(
        &self,
        instance: &AgentInstance,
    ) -> Option<Vec<String>> {
        #[cfg(feature = "memory")]
        {
            let (auto, top_k) = *self.memory.memory_inject_cfg.read();
            if !auto {
                return None;
            }
            // Latest USER message is the retrieval signal.
            let history = instance.get_history();
            let query = history
                .iter()
                .rev()
                .find(|t| t.role == "user")
                .map(|t| t.content.clone())?;
            if query.trim().is_empty() {
                return None;
            }
            let mgr = self.memory.memory_inject_manager.read().clone()?;
            // AUTO_INJECT_MIN_SCORE (0.35) gates INSIDE search_auto_inject —
            // the plain search() runs the store's 0.7 bar (memory_search
            // tuning), which silently emptied loosely-related injection hits
            // (2026-08-29 根因).
            let result = mgr
                .search_auto_inject(&query, top_k.max(1) + 2)
                .await
                .ok()?;
            let mut scored: Vec<(f64, String)> = result
                .entries
                .into_iter()
                .map(|e| {
                    let content = e.entry.content;
                    let cut = content
                        .char_indices()
                        .nth(300)
                        .map(|(i, _)| i)
                        .unwrap_or(content.len());
                    (e.score, content[..cut].to_string())
                })
                .collect();
            // Sort best-first, cap at top_k.
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            // Pairwise dedup: drop a weaker hit whose cosine with a kept hit
            // exceeds 0.92. Cheap at top_k+2 candidates.
            let mut kept: Vec<(f64, String)> = Vec::new();
            'cand: for cand in scored {
                for k in &kept {
                    if textwise_similar(&cand.1, &k.1) > 0.92 {
                        continue 'cand;
                    }
                }
                kept.push(cand);
                if kept.len() >= top_k.max(1) {
                    break;
                }
            }
            if kept.is_empty() {
                return None;
            }
            Some(kept.into_iter().map(|(_, c)| c).collect())
        }
        #[cfg(not(feature = "memory"))]
        {
            None
        }
    }

    /// Build the LLM message list from the instance conversation history.
    ///
    /// Injects an ephemeral "# Current Time / # Environment" system message
    /// immediately before the latest user message. The historical prefix (system
    /// prompt + earlier turns) stays byte-identical across requests, preserving
    /// prompt-cache hits; only the trailing user message and the dynamic marker
    /// are billed at the cache-miss rate. The platform/shell hint steers the
    /// model away from interactive commands that hang the exec tool (e.g. bare
    /// Windows `date` vs `date /t`) — small-model-tool-robustness plan Phase 1.
    ///
    /// P3.1 (sixth batch): `memory_hits` (caller-side async prefetch — search
    /// is async, this fn is sync) renders a `# Memory Context` section into
    /// the merged snapshot. `None`/empty = no section, byte-identical to
    /// pre-P3.1 output.
    pub fn build_messages(&self, instance: &AgentInstance) -> Vec<LlmMessage> {
        self.build_messages_with_memory(instance, None)
    }

    /// P3.1 companion: same as [`build_messages`] plus an optional
    /// pre-fetched memory-hit section.
    pub fn build_messages_with_memory(
        &self,
        instance: &AgentInstance,
        memory_hits: Option<&[String]>,
    ) -> Vec<LlmMessage> {
        self.build_messages_with_memory_annotated(instance, memory_hits)
            .0
    }

    /// 对话生成（2026-09-22）：渲染 workflow_edit 会话的 merged-digest section。
    ///
    /// - `_new`（workflow_name=None）：能力表 + 两阶段引导（先查
    ///   workflow_capabilities，产出走 workflow_create 草稿，人工在 UI 应用）。
    /// - 命名会话：先给当前定义 YAML（引擎未装配或工作流不存在时诚实注明，
    ///   不编造），再给同款引导。
    ///
    /// 无时钟输入（定义来自引擎 DashMap 快照，能力表是编译期静态数据）→
    /// 同状态同字节，与 merged snapshot 的确定性约定一致。
    fn render_workflow_edit_section(
        &self,
        target: &nemesis_types::channel::WorkflowEditTarget,
    ) -> String {
        // 命名会话的「当前定义」块。
        #[cfg(feature = "workflow")]
        let current_definition = target.workflow_name.as_ref().map(|name| {
            let engine_guard = self.workflow_engine.read().clone();
            match engine_guard.as_ref().and_then(|e| e.get_workflow(name)) {
                Some(wf) => match serde_yaml::to_string(&wf) {
                    Ok(yaml) => format!("```yaml\n{yaml}```"),
                    Err(e) => format!("(当前定义序列化失败: {e})"),
                },
                None => format!(
                    "(工作流 {name:?} 当前不存在——可能是新名字，或已被删除。\
                     把这当作全新创建处理。)"
                ),
            }
        });
        #[cfg(not(feature = "workflow"))]
        let current_definition: Option<String> = None;

        let mut out = String::from("# Workflow Editor Session (对话生成)\n\n");
        out.push_str(
            "这是工作流编辑/生成会话。目标：帮助用户创建或修改一个 NemesisBot 工作流定义。\n\n",
        );
        match (&target.workflow_name, &current_definition) {
            (Some(name), Some(def)) => {
                out.push_str(&format!("## 当前工作流：{name}\n\n{def}\n\n"));
            }
            (Some(name), None) => {
                out.push_str(&format!(
                    "## 当前工作流：{name}\n\n(工作流引擎未装配，无法读取当前定义。)\n\n"
                ));
            }
            (None, _) => {
                out.push_str("## 目标：新建工作流\n\n先用 workflow_capabilities 工具查看可用的节点类型、config 字段与结构规则，再动手写定义。\n\n");
            }
        }
        out.push_str(
            "## 产出流程（两阶段，必须遵守）\n\n\
             1. 用 workflow_capabilities 查看节点/触发器能力表（每次会话至少一次）。\n\
             2. 把完整定义交给 workflow_create 工具——它只落**草稿**，不会影响任何已注册工作流。\n\
             3. 草稿保存后把结果告诉用户：用户需要在「工作流页 → 对话生成 → 草稿面板」点「应用」草稿才真正生效。\n\
             4. 响应里 validation_errors 非空时，修正定义并用同名重新保存。\n\n\
             (本 section 每轮由系统注入；修改工作流不在此处直接执行——执行仍走安全策略。)",
        );
        out
    }

    /// T8 (U9 ②) companion: [`build_messages_with_memory`] plus a
    /// [`crate::replay::BuildAnnotation`] recording everything a later
    /// byte-exact replay needs that is NOT derivable from the final session
    /// file — the digest injection's final-vec position, the folded history
    /// length, and the summary-cache state AS OF this build (the session
    /// file's final summary may have advanced later in the same turn).
    /// Byte-identical output to the unannotated build; the annotation rides
    /// alongside and never feeds the provider.
    pub fn build_messages_with_memory_annotated(
        &self,
        instance: &AgentInstance,
        memory_hits: Option<&[String]>,
    ) -> (Vec<LlmMessage>, crate::replay::BuildAnnotation) {
        let history = instance.get_history();

        // Inline-summary pipeline. When a summary cache is active, its `text`
        // folds the covered prefix `history[..covers_up_to]` into the leading
        // system message, and `history[covers_up_to..]` is sent verbatim. Every
        // message is either summarized (in `text`) or verbatim — no gap, no
        // overlap. With no active cache this degrades to sending the entire
        // history verbatim, byte-identical to pre-refactor behavior.
        let cache = instance.get_summary_cache();
        let active_cache = cache
            .as_ref()
            .filter(|c| !c.text.is_empty() && c.covers_up_to >= 1);

        let turns = project_history_for_request(
            &history,
            active_cache.map(|c| (c.text.as_str(), c.covers_up_to)),
        );

        // T10（多模态）：最新 user 注入点的判定提前——vision 投影与后面的
        // digest 注入共用同一个 index（纯 turns 派生，与注入内容无关）。
        let last_user_idx = turns
            .iter()
            .rposition(|t| t.role == "user")
            .filter(|&i| i > 0)
            .filter(|_| turns.first().is_some_and(|t| t.role == "system"));

        // T10（多模态 D4）：vision=no 时把 image_refs 投影为稳定占位文本
        // （当前轮拒绝注明 / 历史轮已省略）。supported（默认放行含）零改动
        // ——纯文本请求字节不受影响（字节兼容验收 #2）。投影发生在
        // turn → 请求消息转换**之前**，图片字节根本不进请求；占位写入的是
        // 投影视图，历史持久化不动（切回 vision 模型图片原样回来）。投影
        // 标志随投影台账落盘，replay 用同一共享纯函数重放。
        let mut turns = turns;
        let vision_projected = !self.current_vision().supported;
        if vision_projected {
            project_turns_for_no_vision(&mut turns, last_user_idx);
        }

        let mut annotation = crate::replay::BuildAnnotation {
            digest_index: None,
            history_len: turns.len(),
            summary_as_of: active_cache.map(|c| crate::replay::SummaryAsOf {
                covers_up_to: c.covers_up_to,
                text: c.text.clone(),
            }),
            vision_projected,
        };

        // I2 (U8): time/env becomes the FIRST section of the merged context
        // snapshot (was a standalone system-role dyn_msg). Minute granularity:
        // the timestamp truncates to the minute so a burst of calls within
        // the same minute does not churn the digest (runtime-context
        // snapshot discipline: identical content ⇒ no re-injection).
        let now = chrono::Local::now()
            .format("%Y-%m-%d %H:%M (%A)")
            .to_string();
        #[cfg(target_os = "windows")]
        let env_hint = "platform: windows\ndefault_shell: cmd\ntime_cmd: use `date /t` or `echo %date% %time%` or PowerShell `Get-Date`";
        #[cfg(not(target_os = "windows"))]
        let env_hint = "platform: unix\ndefault_shell: sh\ntime_cmd: use `date`";
        // FT（2026-09-17）：Environment 快照注入工作区绝对路径——治「模型
        // 不知道工作区在哪」的提示词半边（工具层相对路径锚定是另一半）。
        // 路径整个会话不变 → 字节稳定纪律成立（不触发重注入）。
        let mut snapshot_section = format!(
            "# Current Time / Environment snapshot\n{}\n# Environment\n{}",
            now, env_hint
        );
        if let Some(ref root) = *self.workspace_root.read()
            && !root.as_os_str().is_empty()
        {
            snapshot_section.push_str(&format!("\nworkspace: {}", root.display()));
        }
        snapshot_section.push_str("\n(本快照取代之前的时间/环境快照)");

        // T5/T6（多模态）：turn → 请求消息统一走 `turn_to_request_message`
        // （与 replay 重建共用；image_refs 每轮水合重读，失效 → 占位文本）。
        let turn_to_msg = |turn: &crate::types::ConversationTurn| turn_to_request_message(turn);

        // Inject dyn_msg just before the last user message, but only when there
        // is a system prompt at turns[0] to protect (otherwise there's no
        // cached prefix to preserve).
        //
        // H3 (P2.2) + H5 (U18) + I2 (U8): the skills-catalog digest, the
        // workspace instruction chain, AND the time/env snapshot ride ONE
        // injection point as a single MERGED message (sections inside one
        // <system-reminder> wrapper), with the same prefix-protection
        // condition. Re-emitted on EVERY build (not persisted in history) —
        // deterministic rendering keeps it byte-identical while nothing
        // changed, which is what preserves the provider prefix. Sections
        // re-read from disk each build, so file touches are picked up
        // naturally (H5's invalidate call is a structural no-op).
        let context_digest_msg: Option<LlmMessage> = {
            let loader = self.skills_loader.read().clone();
            let ws_root = self.workspace_root.read().clone();
            // Build the merged content: time/env snapshot (I2) + skills
            // section (if any) + workspace instructions section (if any).
            // The snapshot is ALWAYS present (time always renders), so the
            // merged message exists for every session.
            let mut sections: Vec<String> = vec![snapshot_section.clone()];
            if let Some(ref l) = loader {
                let infos = l.list_skills();
                if !infos.is_empty() {
                    let catalog = crate::skills_digest::catalog_from_skills_infos(&infos);
                    let rendered = crate::skills_digest::render_skills_digest(&catalog);
                    sections.push(crate::skills_digest::digest_message(&rendered));
                }
            }
            if let Some(ref root) = ws_root {
                let cwd = root.clone(); // workspace root ≈ conversation cwd
                let chain = crate::workspace_instructions::load_instruction_chain(root, &cwd);
                let rendered = crate::workspace_instructions::render_instructions_section(&chain);
                if !rendered.is_empty() {
                    sections.push(rendered);
                }
            }
            // I3 (devtool-upgrade 阶段 3): lazily discovered sub-directory
            // instructions — one-shot section drained from the instance
            // (same channel shape as the external_changes section below;
            // empty between discoveries keeps the merged message
            // byte-stable). Claims live on the instance, so dedup is
            // session-scoped.
            let pending_instructions = instance.drain_pending_instructions();
            if !pending_instructions.is_empty() {
                sections.push(
                    crate::workspace_instructions::render_new_instructions_section(
                        &pending_instructions,
                    ),
                );
            }
            // P3.1 (sixth batch): pre-fetched memory hits as a section. The
            // caller (run_llm_loop) did the async search against the CURRENT
            // user message; here we only render. Empty/None ⇒ no section ⇒
            // byte-identical output to auto_inject=false.
            if let Some(hits) = memory_hits
                && !hits.is_empty()
            {
                let body = hits
                    .iter()
                    .map(|h| format!("- {}", h))
                    .collect::<Vec<_>>()
                    .join("\n");
                sections.push(format!(
                        "# Memory Context\n{body}\n\n(以上是自动检索到的相关长期记忆，可能与当前对话有关，也可能无关——自行判断取舍。)"
                    ));
            }
            // I1 (devtool-upgrade 阶段 3): externally-changed workspace
            // files since the last build — one-shot section (drained here;
            // absent when nothing changed, keeping the message byte-stable
            // between turns). The watcher-side ignore table + self-write
            // window keep this quiet; ≤10 entries per flush, ≤32 buffered.
            let external_changes = self.drain_external_changes();
            if !external_changes.is_empty() {
                sections.push(crate::fs_watcher::render_external_changes_section(
                    &external_changes,
                ));
            }
            // I5（devtool-upgrade 阶段 7）：当前轮客户端上报的打开文件路径
            // —— per-turn ephemeral 状态（process_admitted set / 出轮 clear）。
            // 只渲染路径不读内容（annotation 语义；agent 真正读文件仍走
            // read_file 工具与安全 8 层，零新增信任面）。空 = 无 section
            // （字节稳定）；turn 内多迭代状态不变 → 每次构建字节一致；
            // 回放走 InjectionRecord 台账记录的 digest 内容（transient
            // 注入既有机制，零额外回放工作）。
            let open_files = self.pending_open_files.read().clone();
            if !open_files.is_empty() {
                let body = open_files
                    .iter()
                    .map(|p| format!("- {}", p))
                    .collect::<Vec<_>>()
                    .join("\n");
                sections.push(format!(
                    "# Open Files (client-reported)\n{body}\n\n(以上是客户端上报的当前打开文件路径，顺序=上报顺序，仅供参考——读取文件仍受安全策略约束。)"
                ));
            }
            // 对话生成（2026-09-22）：workflow_edit 会话上下文。命名会话渲染
            // 当前定义 YAML（引擎未装配/未找到时诚实注明）；_new 会话渲染
            // 能力表引导块。空目标 = 无 section（字节稳定）。内容随既有
            // InjectionRecord 台账落账 → 回放免费一致（同 I5 机制）。
            let wf_edit_target = self.pending_workflow_edit.read().clone();
            if let Some(target) = wf_edit_target {
                sections.push(self.render_workflow_edit_section(&target));
            }
            // X2 (U8 refinement): runtime policy facts as the LAST section.
            // All three inputs are plain state rendered without clocks —
            // deterministic (same state ⇒ identical bytes, so the merged
            // message stays byte-stable between turns and the historical
            // prefix before it is untouched either way):
            //   approval — live wiring flag (gateway sets after attaching
            //     the desktop popup adapter);
            //   guardian — live judge presence on the security plugin
            //     (feature-off builds render "off");
            //   model_tier — the live capability tier (auto re-resolves on
            //     config reload; the snapshot picks the new value up at the
            //     next build).
            #[cfg(feature = "security")]
            let guardian_on = self
                .security
                .security_plugin
                .as_ref()
                .is_some_and(|p| p.judge().is_some());
            #[cfg(not(feature = "security"))]
            let guardian_on = false;
            let approval_on = *self.interactive_approval.read();
            let tier_now = *self.tier.read();
            // F1（devtool-upgrade 阶段 4）：模式进快照——Plan 时模型当轮就
            // 能读到「不许改文件」，与供给/分发双闸互补（提醒是软约束）。
            let mode_now = self.mode();
            let mode_line = match mode_now {
                crate::types::AgentMode::Plan => "plan（PLAN mode: do not modify files. Present your plan as text; write_file under plans/ is the only write allowed. User can switch back with /build.）".to_string(),
                crate::types::AgentMode::Build => "build（正常全量工具）".to_string(),
            };
            sections.push(format!(
                "# Runtime Policy\napproval: {}\nguardian: {}\nmodel_tier: {}\nmode: {}\n(当前审批/守护/模型档位/工作模式运行时策略快照；策略变更后下一次构建生效。)",
                if approval_on {
                    "interactive（ask 规则触发弹窗审批）"
                } else {
                    "off（无交互审批，ask 规则按默认策略处理）"
                },
                if guardian_on {
                    "on（CRITICAL 操作语义二审）"
                } else {
                    "off"
                },
                tier_now,
                mode_line,
            ));
            if sections.is_empty() {
                None
            } else {
                let merged = format!(
                    "<system-reminder>\n{}\n</system-reminder>",
                    sections.join("\n\n")
                );
                self.skills_digest_state
                    .should_inject("", &merged) // stateless since round-5
                    .map(|m| LlmMessage {
                        // I2 (U8): user-role snapshot (was system) — the
                        // system prompt stays byte-frozen; dynamic facts
                        // arrive as conversation messages. M4: role is
                        // configurable for strict chat templates that
                        // reject adjacent user/user pairs.
                        role: self.snapshot_role.read().clone(),
                        content: m,
                        tool_calls: None,
                        tool_call_id: None,
                        reasoning_content: None,
                        images: Vec::new(),
                    })
            }
        };

        match last_user_idx {
            Some(idx) => {
                let mut messages: Vec<LlmMessage> =
                    Vec::with_capacity(turns.len() + 1 + context_digest_msg.is_some() as usize);
                messages.extend(turns[..idx].iter().map(turn_to_msg));
                if let Some(d) = context_digest_msg {
                    messages.push(d);
                    // T8 (U9 ②): final-vec position of the digest injection —
                    // recorded because it is rebuilt-on-every-request and never
                    // persisted, so replay must re-insert it at this position.
                    annotation.digest_index = Some(messages.len() - 1);
                }
                messages.extend(turns[idx..].iter().map(turn_to_msg));
                (messages, annotation)
            }
            None => (turns.iter().map(turn_to_msg).collect(), annotation),
        }
    }

    // -----------------------------------------------------------------------
    // Slash command handling
    // -----------------------------------------------------------------------
}
