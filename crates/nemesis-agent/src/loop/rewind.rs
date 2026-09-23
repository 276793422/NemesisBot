//! 消息级回退/重做：REWIND_UNDO_STACK_CAP/RewindUndoEntry、rewind/rewind_to_message/redo_rewind、checkpoint 挂载/列表、session_file_diff。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
use super::prelude::*;
use super::*;

/// E3（devtool-upgrade 阶段 5）：每会话 undo 栈深度上限。超限丢最旧——
/// 内存态栈只服务本进程内的连续 undo/redo，不设界会话泄漏无界内存。
const REWIND_UNDO_STACK_CAP: usize = 8;

/// E3：一条消息级回退的 redo 依据（`rewind_to_message` 压栈，`redo_rewind`
/// 弹栈反向）。被截断的 jsonl 行 VERBATIM 随行保存（原时间戳/标记不动），
/// 影子 tree 支撑文件态的前向恢复与陈旧性守卫。
#[derive(Debug, Clone)]
pub(crate) struct RewindUndoEntry {
    /// 被 `rewind_to_message` 截掉的行（原样，`rows[cut..]`）。
    removed_rows: Vec<serde_json::Value>,
    /// 回退后剩余行数（= cut）。redo 守卫：期间该会话无任何新行。
    kept_count: usize,
    /// 截断点之后第一个 checkpoint turn（恢复/截断索引用的那个）。
    restore_turn: Option<usize>,
    /// undo 时刻的最新影子 tree——redo 的文件前向恢复目标。
    forward_tree: Option<String>,
    /// undo 完成后（`truncate_from` 之后的索引里）剩的最新 tree——redo 的
    /// 陈旧性守卫基线（期间任何新 checkpoint turn 都会改变它）。
    post_tree: Option<String>,
}

impl AgentLoop {
    /// Attach a checkpoint store for the edit safety net. When set, every writer
    /// tool call (write_file/edit_file/append_file/delete_file) snapshots the
    /// file's pre-edit content before execution, so a rewind can restore it.
    pub fn set_checkpoint_store(&self, store: Arc<crate::checkpoint::CheckpointStore>) {
        *self.security.checkpoint_store.write() = Some(store);
    }

    /// L6++（2026-09-08）：checkpoint store 只读口——项目 loop 工厂测试用
    /// 它断言影子库落在主 workspace（`logs/project_checkpoints/{pid}`）而非
    /// 用户项目目录。
    pub fn checkpoint_store(&self) -> Option<Arc<crate::checkpoint::CheckpointStore>> {
        self.security.checkpoint_store.read().clone()
    }

    /// Rewind the workspace to the start of turn `from_turn`: restores every file
    /// changed at or after that turn to its pre-edit content (the edit safety
    /// net). Returns `(written, deleted)` paths. Errors if no checkpoint store is
    /// attached. Conversation rewinding (truncating session history) is handled
    /// by the caller — this only restores code.
    pub async fn rewind(&self, from_turn: usize) -> Result<(Vec<String>, Vec<String>), String> {
        let cp = self
            .security
            .checkpoint_store
            .read()
            .as_ref()
            .cloned()
            .ok_or("checkpoint store not attached")?;
        Ok(cp.restore_code(from_turn).await)
    }

    /// List checkpoint turns (for a rewind picker UI). Empty if no store attached.
    pub fn checkpoint_list(&self) -> Vec<crate::checkpoint::CheckpointMeta> {
        match self.security.checkpoint_store.read().as_ref() {
            Some(cp) => cp.list_meta(),
            None => Vec::new(),
        }
    }

    /// E3 消息级回退：会话截断到 `message_index` 所在 turn 结束（该消息
    /// 及其 turn 的回复**保留**，其后所有行截掉），文件恢复到其后第一个
    /// checkpoint turn 开始时的状态，并压入 undo 栈供 [`Self::redo_rewind`]
    /// 反向。
    ///
    /// 定位契约：`message_index` 是 [`crate::chat_log::read_chat_log`]（=
    /// `sessions.export` / `logs.session_detail` 的行序，同一 jsonl）里该
    /// 会话消息数组的下标。行→turn 精确定位靠 user 行的 `checkpoint_turn`
    /// 标记（本 turn begin 序号随 admission 穿针写入）；无标记的旧行退化为
    /// 「只截断对话不回滚文件」。
    ///
    /// 返回回执 JSON（kept/removed 计数 + 恢复文件清单 + redoable）。
    pub async fn rewind_to_message(
        &self,
        session_key: &str,
        message_index: usize,
    ) -> Result<serde_json::Value, String> {
        if self.is_session_busy(session_key) {
            return Err("会话正在处理消息，请等当前回合完成后再回退".to_string());
        }
        // 全量读（回退是一次性管理操作，非热路径——fork 同款）。
        let (rows, _total, _, _) = crate::chat_log::read_chat_log(session_key, usize::MAX, None);
        if rows.is_empty() {
            return Err("会话没有可回退的消息".to_string());
        }
        if message_index >= rows.len() {
            return Err(format!(
                "message_index {} 超出范围（会话共 {} 条消息）",
                message_index,
                rows.len()
            ));
        }

        // 对话截断点：index 之后第一个 user 行（turn 边界——assistant 行
        // 属于所在 turn，必须随 turn 一起保留/截掉）。
        let cut = rows[message_index + 1..]
            .iter()
            .position(|r| r.get("role").and_then(|v| v.as_str()) == Some("user"))
            .map(|p| p + message_index + 1)
            .unwrap_or(rows.len());
        // 文件恢复锚：cut 起第一个带 checkpoint_turn 标记的行（第一个仍在
        // 的 checkpoint turn）——恢复到它 begin 之前 = index 所在 turn 完成
        // 时的文件态。None = 之后没有 checkpoint turn（或全是无标记旧行）。
        let restore_turn = rows[cut.min(rows.len())..].iter().find_map(|r| {
            r.get("checkpoint_turn")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
        });
        let removed: Vec<serde_json::Value> = rows[cut..].to_vec();

        // undo 依据在任何突变前采集（truncate_from 会清掉 turn ≥ restore 的
        // 索引，tree 值要趁索引还在时读）。
        let (forward_tree, post_tree) = match self.attached_checkpoint() {
            Some(cp) => {
                // redo 前向恢复目标 = rewind 时刻的实时 tree（begin 树只到
                // 各 turn 开始态，最后一个 turn 的变更只在实况里）。仅文件
                // 恢复路径活跃（有 restore_turn 锚）时采集实时树；无锚时保
                // 持 begin 树语义（可能 None），redo 只回填行、文件步诚实跳
                // 过，陈旧性守卫基线不变。
                let forward = if restore_turn.is_some() {
                    cp.current_tree_hex().or_else(|| cp.latest_tree_hex())
                } else {
                    cp.latest_tree_hex()
                };
                // post 基线 = truncate 之后索引里剩的最新 tree（restore 无
                // 锚 = 索引不动，基线就是现状）。
                let post = match restore_turn {
                    Some(t) => cp.tree_hex_before(t),
                    None => forward.clone(),
                };
                (forward, post)
            }
            None => (None, None),
        };

        // 1) 文件恢复（先于 jsonl 截断——中途崩溃时对话完好，可重试）。
        let mut file_restore = "skipped";
        let mut file_note: Option<String> = None;
        let mut written: Vec<String> = Vec::new();
        let mut deleted: Vec<String> = Vec::new();
        if let (Some(t), Some(cp)) = (restore_turn, self.attached_checkpoint()) {
            let (w, d) = cp.restore_code(t).await;
            written = w;
            deleted = d;
            cp.truncate_from(t);
            file_restore = "applied";
        } else if restore_turn.is_some() {
            file_note = Some("checkpoint store 未挂载，文件未回滚".to_string());
        } else if cut < rows.len() {
            file_note = Some(
                "之后没有 checkpoint 标记（旧行或无 store），只截断对话不回滚文件".to_string(),
            );
        }

        // 2) jsonl 截断（tmp+rename 原子重写；verbatim 保留保留侧行）。
        let n = crate::chat_log::truncate_chat_log_rows(session_key, &rows[..cut]);
        if n != cut {
            warn!(
                "[AgentLoop] rewind 截断写回不完整（期望 {cut} 行写 {n} 行）——jsonl 保留原样，回退未生效"
            );
            return Err("会话日志写回失败，回退未生效（原会话完好）".to_string());
        }

        // 3) SessionStore 丢缓存（jsonl 是单一真相源，下次 get_or_create
        // 从截断后的 jsonl 自愈重建——sessions.delete/clear 同款纪律）。
        if let Some(store) = self.session_store() {
            store.clear_session(session_key);
        }

        // 4) 压 undo 栈（截断成功后才压——失败路径不留脏条目）。
        let redoable = !removed.is_empty();
        if redoable {
            let mut stacks = self.rewind_undo_stacks.lock();
            let stack = stacks.entry(session_key.to_string()).or_default();
            stack.push_back(RewindUndoEntry {
                removed_rows: removed,
                kept_count: cut,
                restore_turn,
                forward_tree,
                post_tree,
            });
            while stack.len() > REWIND_UNDO_STACK_CAP {
                stack.pop_front();
            }
        }

        Ok(serde_json::json!({
            "session_key": session_key,
            "message_index": message_index,
            "kept_count": cut,
            "removed_count": rows.len() - cut,
            "restore_turn": restore_turn,
            "file_restore": file_restore,
            "file_restore_note": file_note,
            "restored_files": { "written": written, "deleted": deleted },
            "redoable": redoable,
        }))
    }

    /// E3 redo：弹本会话 undo 栈顶，反向恢复——被截断的行 VERBATIM 回填
    /// jsonl，文件恢复到 undo 时刻的影子 tree。行数/tree 基线双重陈旧性
    /// 守卫：undo 之后该会话有任何新消息、或任何地方有新 checkpoint turn
    /// （工作区变过），诚实拒绝（弹出的条目作废）。
    ///
    /// 诚实边界：redo 不重建 checkpoint 索引（undo 时 turn-*.json 已清），
    /// redo 过的区间无法再次 rewind；文件只恢复工作区内（tree 覆盖范围）。
    pub async fn redo_rewind(&self, session_key: &str) -> Result<serde_json::Value, String> {
        if self.is_session_busy(session_key) {
            return Err("会话正在处理消息，请等当前回合完成后再重做".to_string());
        }
        let entry = {
            let mut stacks = self.rewind_undo_stacks.lock();
            stacks.get_mut(session_key).and_then(|s| s.pop_back())
        }
        .ok_or_else(|| "没有可重做的回退（先执行一次消息级回退）".to_string())?;

        // 守卫 1：会话行数未变（undo 后没发过新消息）。
        let (rows, _total, _, _) = crate::chat_log::read_chat_log(session_key, usize::MAX, None);
        if rows.len() != entry.kept_count {
            return Err(format!(
                "回退后产生了新消息，redo 已失效（期望 {} 行，实际 {} 行）",
                entry.kept_count,
                rows.len()
            ));
        }
        // 守卫 2：工作区 tree 基线未变（undo 后没有新 checkpoint turn）。
        let cur_tree = self
            .attached_checkpoint()
            .and_then(|cp| cp.latest_tree_hex());
        if cur_tree != entry.post_tree {
            return Err("回退后工作区发生了新的变化，redo 已失效".to_string());
        }

        // 1) 文件前向恢复（best-effort——失败只注记，行回填照常进行）。
        let mut file_restore = "skipped";
        let mut file_note: Option<String> = None;
        let mut written: Vec<String> = Vec::new();
        let mut deleted: Vec<String> = Vec::new();
        match (entry.forward_tree.as_deref(), self.attached_checkpoint()) {
            (Some(hex), Some(cp)) => match cp.restore_to_tree(hex) {
                Ok((w, d)) => {
                    written = w;
                    deleted = d;
                    file_restore = "applied";
                }
                Err(e) => file_note = Some(e),
            },
            (Some(_), None) => file_note = Some("checkpoint store 未挂载，文件未恢复".to_string()),
            (None, _) => {
                file_note = Some("无影子 tree（JSON 形态或该区间无变更），文件未恢复".to_string())
            }
        }

        // 2) 行回填（verbatim——原时间戳/标记原样回来）。
        let mut all = rows;
        all.extend(entry.removed_rows.iter().cloned());
        let expected = all.len();
        let n = crate::chat_log::truncate_chat_log_rows(session_key, &all);
        if n != expected {
            warn!("[AgentLoop] redo 回填写回不完整（期望 {expected} 行写 {n} 行）——jsonl 保留原样");
            return Err("会话日志写回失败，重做未生效（会话保持回退态）".to_string());
        }

        // 3) SessionStore 丢缓存（同 rewind——从回填后的 jsonl 重建）。
        if let Some(store) = self.session_store() {
            store.clear_session(session_key);
        }

        Ok(serde_json::json!({
            "session_key": session_key,
            "restored_count": entry.removed_rows.len(),
            "kept_count": entry.kept_count,
            "restore_turn": entry.restore_turn,
            "file_restore": file_restore,
            "file_restore_note": file_note,
            "restored_files": { "written": written, "deleted": deleted },
        }))
    }

    /// E3：checkpoint store 已挂载时克隆出 Arc（`rewind_to_message` /
    /// `redo_rewind` 的共享读法）。
    pub(crate) fn attached_checkpoint(&self) -> Option<Arc<crate::checkpoint::CheckpointStore>> {
        self.security.checkpoint_store.read().as_ref().cloned()
    }

    /// M3（devtool-upgrade 阶段 5）：会话级文件 diff——某文件「最早
    /// checkpoint 基线（pre-edit 态）vs 现盘」的 unified diff。
    ///
    /// 基线 = checkpoint 索引里首个声明过该文件的条目（git 形态读影子
    /// tree 的 blob；JSON 形态用快照 content；索引 store 全局——多会话
    /// 共改同一文件时基线取更早 turn，语义是「最早已知 pre-edit 态」）。
    /// 文件已不在磁盘 → head 视为空串（diff 呈全删）+ `head_on_disk:
    /// false` 诚实标注；diff 为空 → note 说明（可能已被回退/手动恢复）。
    /// `session_key` v1 只用于将来按会话收窄基线（现留参保调用形状稳定）。
    pub async fn session_file_diff(
        &self,
        _session_key: &str,
        path: &str,
    ) -> Result<serde_json::Value, String> {
        let cp = self
            .attached_checkpoint()
            .ok_or_else(|| "checkpoint 未挂载，无法对比会话文件变更".to_string())?;
        let base = cp.base_for_path(path).ok_or_else(|| {
            "该文件无 checkpoint 基线（未在本会话声明变更，或基线已被回退清除）".to_string()
        })?;

        // 基线内容：git 形态从影子 tree 读 blob；JSON 形态用快照 content。
        let base_content = match &base.tree {
            Some(tree) => cp.read_file_from_tree(tree, path)?.unwrap_or_default(),
            None => base.content.clone().unwrap_or_default(),
        };

        // 现盘内容：缺文件 = 空 head（diff 呈全删）+ head_on_disk=false。
        let abs = cp.root().join(path);
        let (head_content, head_on_disk) = match tokio::fs::read(&abs).await {
            Ok(bytes) => (String::from_utf8_lossy(&bytes).into_owned(), true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
            Err(e) => return Err(format!("读取现盘文件失败: {e}")),
        };

        let diff = crate::loop_tools::edit_hint::unified_diff(path, &base_content, &head_content);
        let note = if !head_on_disk {
            "文件已不在磁盘（对照基线呈全删除）".to_string()
        } else if diff.is_empty() {
            "当前内容与基线无差异（可能已被回退或手动恢复）".to_string()
        } else {
            String::new()
        };
        Ok(serde_json::json!({
            "path": path,
            "backend": if base.tree.is_some() { "git" } else { "json" },
            "base_turn": base.turn,
            "diff": diff,
            "head_on_disk": head_on_disk,
            "note": note,
        }))
    }
}
