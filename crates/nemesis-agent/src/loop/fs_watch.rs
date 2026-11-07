//! 文件系统监视与注入：start_fs_watcher、push/drain_external_changes、note_self_write、workspace/spill 根、set_snapshot_role/interactive_approval/skills_loader/commands_path/cc_hooks_bridge、run_session_end_hooks。
//!
//! 自 `loop.rs` 物理搬迁；语义零变化。
#[cfg(not(target_os = "linux"))]
use super::prelude::*;
use super::*;

impl AgentLoop {
    /// 自定义 slash 命令表路径（主 agent 专用；集群 agent 不接——命令不该
    /// 跨节点复制，同 hooks 挂账决策）。设置时立即加载一次；mtime 变化在
    /// `rewrite_custom_command` 的 check() 里自动重载（HotReloader 统一收编，
    /// dashboard/CLI 改命令表后无需重启）。
    pub fn set_commands_path(&self, path: std::path::PathBuf) {
        *self.commands_hot.write() = Some(nemesis_config::HotReloader::new(
            path,
            nemesis_config::load_commands_config,
        ));
    }

    /// hooks 方言桥注入（PreCompact/PostCompact 触发用；工具/生命周期钩子走
    /// 各自注册表，与此并存）。
    pub fn set_cc_hooks_bridge(&self, bridge: std::sync::Arc<crate::cc_hooks::CcHookBridge>) {
        *self.cc_bridge.write() = Some(bridge);
    }

    /// 桥的只读访问（Dashboard 删除会话时触发方言 SessionEnd 用）。
    pub fn cc_hooks_bridge(&self) -> Option<std::sync::Arc<crate::cc_hooks::CcHookBridge>> {
        self.cc_bridge.read().clone()
    }

    /// 触发 SessionEnd 钩子（会话 TTL 过期清理/显式删除时由装配点调用）。
    /// 直接走方言桥（唯一实现者；无桥 = no-op）。
    pub async fn run_session_end_hooks(&self, session_key: &str, reason: &str) {
        // 复用 cc_hooks_bridge()（锁内 clone Arc 出来），读 guard 不得跨 await——
        // 否则 on_session_end 整个回调期间 cc_bridge 写锁全部阻塞。
        if let Some(bridge) = self.cc_hooks_bridge() {
            bridge.on_session_end(session_key, reason).await;
        }
    }

    /// enable tool-result spill with the given root directory
    /// (expected `<home>/logs/spill`). Results above the spill threshold are
    /// written whole under this root and the conversation keeps a bounded
    /// preview + locator. Without a call, spilling stays disabled and results
    /// use only the prune tier.
    pub fn set_spill_root(&self, root: std::path::PathBuf) {
        *self.spill_root.write() = Some(root);
    }

    /// (arch review): read back the configured spill root, if
    /// any. Diagnostics/test seam for verifying factory wiring — both the
    /// main and cluster factories point at `<home>/logs/spill`.
    pub fn spill_root_path(&self) -> Option<std::path::PathBuf> {
        self.spill_root.read().clone()
    }

    /// (.2): enable skills-catalog digest injection by providing the
    /// loader. The digest is injected (same point as the time/env hint) only
    /// when the catalog changed since the last injection for this session.
    pub fn set_skills_loader(&self, loader: Arc<nemesis_skills::loader::SkillsLoader>) {
        *self.skills_loader.write() = Some(loader);
    }

    /// enable the workspace instruction-chain section by giving
    /// the workspace root (chain root = this dir; the chain itself is read
    /// per-injection).
    pub fn set_workspace_root(&self, root: std::path::PathBuf) {
        *self.workspace_root.write() = Some(root);
    }

    /// VM 档执行环境注记 resolver（审计收口）。宿主侧在执行体桥挂载
    /// 成功时安装（见 nemesisbot agent_factory `register_tools_and_mcp`）：
    /// 每轮 build_messages 调用，非空返回值追加进 # Environment 快照；
    /// 未安装（headless/测试）= 零行为差异。
    pub fn set_exec_env_resolver(
        &self,
        resolver: std::sync::Arc<dyn Fn() -> String + Send + Sync>,
    ) {
        *self.exec_env_resolver.write() = Some(resolver);
    }

    /// 节点工作区根（装配时 [`Self::set_workspace_root`] 注入；未设 = None）。
    /// F-：cluster agent 以此定位档案管线任务的工作副本目录。
    pub fn workspace_root(&self) -> Option<std::path::PathBuf> {
        self.workspace_root.read().clone()
    }

    /// 注入工作区指令信任档（项目 loop 装配时解析注入；
    /// `projects.trust` WSAPI 对运行中 loop 热更新——下一轮 build_messages
    /// 即生效，区块逐 build 重渲染无残留）。
    pub fn set_instruction_trust(&self, trust: crate::workspace_instructions::InstructionTrust) {
        *self.instruction_trust.write() = trust;
    }

    /// 当前信任档（诊断/测试缝）。
    pub fn instruction_trust(&self) -> crate::workspace_instructions::InstructionTrust {
        *self.instruction_trust.read()
    }

    /// (devtool-upgrade): start the workspace fs watcher. Call
    /// AFTER the `Arc<AgentLoop>` is finalized — the watcher handle is
    /// stored INSIDE the loop, so its callbacks hold a `Weak<AgentLoop>`
    /// (an Arc would cycle and leak). Requires `set_workspace_root` first;
    /// startup failure warns once and leaves the watcher disabled (never
    /// retries, never blocks the loop).
    pub fn start_fs_watcher(
        loop_arc: &std::sync::Arc<Self>,
        cfg: &nemesis_config::FsWatcherConfig,
    ) -> Result<(), String> {
        let root = match loop_arc.workspace_root.read().clone() {
            Some(r) => r,
            None => {
                tracing::info!("[AgentLoop] fs watcher skipped: no workspace root set");
                return Ok(());
            }
        };
        let weak = std::sync::Arc::downgrade(loop_arc);
        let on_instruction_change: crate::fs_watcher::Callback = {
            let weak = weak.clone();
            std::sync::Arc::new(move || {
                if let Some(l) = weak.upgrade() {
                    // Round-5 note: digest state is stateless (sections
                    // re-read from disk every build) so external instruction
                    // edits already surface next build — this call is the
                    // kept anchor (same shape as the dispatch path's
                    // touch-driven call) in case change-gating returns.
                    l.invalidate_context_digests();
                }
            })
        };
        let on_external_change: crate::fs_watcher::PathCallback = {
            let weak = weak.clone();
            std::sync::Arc::new(move |p| {
                if let Some(l) = weak.upgrade() {
                    l.push_external_change(p);
                }
            })
        };
        match crate::fs_watcher::start(&root, cfg, on_instruction_change, on_external_change) {
            Ok(Some(handle)) => {
                tracing::info!(
                    root = %root.display(),
                    "[AgentLoop] fs watcher started (external changes surface next turn)"
                );
                *loop_arc.fs_watcher.write() = Some(handle);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(e) => {
                tracing::warn!(
                    "[AgentLoop] fs watcher failed to start (disabled, will not retry): {}",
                    e
                );
                Err(e)
            }
        }
    }

    /// buffer an externally-changed workspace file (workspace-relative).
    /// Drops self-inflicted writes (the agent's own write_file/edit_file
    /// within [`crate::fs_watcher:SELF_WRITE_WINDOW`]) and caps the buffer.
    pub fn push_external_change(&self, rel_path: &str) {
        let norm = crate::fs_watcher::normalize_workspace_rel(rel_path);
        {
            let mut writes = self.recent_self_writes.lock();
            writes.retain(|_, t| t.elapsed() < crate::fs_watcher::SELF_WRITE_WINDOW);
            if writes.contains_key(&norm) {
                tracing::debug!(
                    "[AgentLoop] fs watcher event for self-written file dropped: {}",
                    rel_path
                );
                return;
            }
        }
        let mut buf = self.external_changes.lock();
        if buf.len() < crate::fs_watcher::MAX_BUFFERED_CHANGES {
            buf.push(rel_path.to_string());
        }
    }

    /// one-shot drain for the build_messages `<external_changes>` section.
    pub fn drain_external_changes(&self) -> Vec<String> {
        std::mem::take(&mut *self.external_changes.lock())
    }

    /// record an agent-authored write (dispatch path, successful
    /// write_file/edit_file) so the watcher's own event for the same path is
    /// dropped inside the self-write window.
    pub fn note_self_write(&self, path: &str) {
        let key = self.self_write_key(path);
        let mut writes = self.recent_self_writes.lock();
        // Prune expired entries opportunistically (map stays tiny).
        writes.retain(|_, t| t.elapsed() < crate::fs_watcher::SELF_WRITE_WINDOW);
        writes.insert(key, std::time::Instant::now());
    }

    /// raw 工具路径 → 与 fs_watcher 事件同形的 workspace-relative 归一形态
    /// （`\`→`/` + lowercase + 绝对形态按组件剥 workspace 根前缀 + 清洗 `.`
    /// 组件）。BUG-20261106-032：此前只做分隔符/大小写归一——工具层
    /// resolve_workspace_path 原样接受绝对路径与 `./` 前缀相对形态，agent
    /// 用这些形态写文件时登记的 key 与 watcher strip 后的 rel 事件失配，
    /// 自写窗口过滤失效，agent 刚写的文件被谎报为「外部修改（非本会话
    /// 操作）」污染下一轮上下文。剥根用 `Path::strip_prefix`（组件级）而非
    /// 字符级——`<root>2/…` 与 `<root>` 是不同目录，字符级会误剥；剥不出
    /// （workspace 外路径 / 无根设置）回落全路径归一形态，与旧行为一致。
    fn self_write_key(&self, path: &str) -> String {
        let norm = crate::fs_watcher::normalize_workspace_rel(path);
        let rel = match self.workspace_root.read().clone() {
            Some(root) => {
                let rn = crate::fs_watcher::normalize_workspace_rel(&root.to_string_lossy());
                match std::path::Path::new(&norm)
                    .strip_prefix(std::path::Path::new(rn.trim_end_matches('/')))
                {
                    Ok(r) if !r.as_os_str().is_empty() => r.to_path_buf(),
                    _ => std::path::PathBuf::from(norm),
                }
            }
            None => std::path::PathBuf::from(norm),
        };
        rel.components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Full-review: set the context-snapshot message role ("user" |
    /// "system"). Anything unrecognized stays/becomes "user" (default).
    pub fn set_snapshot_role(&self, role: &str) {
        let r = if role.eq_ignore_ascii_case("system") {
            "system"
        } else {
            "user"
        };
        *self.snapshot_role.write() = r.to_string();
    }

    /// (refinement): tell the loop whether interactive approval
    /// (desktop popup) is wired. Called by the gateway after it attaches
    /// the approval adapter; renders into the `# Runtime Policy` snapshot
    /// section. Default `false` (standalone / non-desktop builds).
    pub fn set_interactive_approval(&self, enabled: bool) {
        *self.interactive_approval.write() = enabled;
    }
}
