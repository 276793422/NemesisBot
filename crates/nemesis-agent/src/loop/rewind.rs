//! 消息级回退/重做：REWIND_UNDO_STACK_CAP/RewindUndoEntry、rewind/rewind_to_message/redo_rewind、checkpoint 挂载/列表、session_file_diff。
//!
//! P1 自 `loop.rs` 物理搬迁（docs/PLAN/2026-09-23_agentloop-god-object-decomposition.md §3.2）；语义零变化。
#[cfg(not(target_os = "linux"))]
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
    /// BUG-20261025-54：保留行集的内容身份指纹（sha256）。行数守卫只查
    /// 数量——clear 后重填到与 kept_count 相同的行数会绕过它，把旧行
    /// verbatim 回填进无关对话；指纹维度补上「行数未变但内容已变」的
    /// 陈旧性判定。
    kept_rows_hash: String,
    /// M4（2026-10-27 整改）：undo 完成时刻的**实时盘面**影子 tree（文件
    /// 恢复已生效后的工作区快照）。redo 冲突预检以它为基线 diff 当前盘面
    /// ——基线必须是「回退刚结束时的盘」而不是 forward_tree：回退本身就
    /// 会把文件搬离前向目标（恢复到 begin 态），拿 forward_tree 当基线会
    /// 把合法回退误报成漂移。JSON 形态 / store 未挂载 / tree 写失败 = None
    /// （扫描诚实跳过）。
    undo_disk_tree: Option<String>,
}

/// undo/redo 两侧共用的保留行集指纹：逐行 `serde_json::to_string` 后
/// `'\n'` 连接进 sha256（与 `truncate_chat_log_rows` 落盘侧同序列化形态，
/// 同 key 排序/转义，逐字节可比）。空行集 = 空 blob 的 sha256（恒定值，
/// kept_count=0 的 redo 语义不变）。
fn kept_rows_fingerprint(rows: &[serde_json::Value]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for r in rows {
        if let Ok(line) = serde_json::to_string(r) {
            h.update(line.as_bytes());
            h.update(b"\n");
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
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
    /// net). Returns a structured [`RestoreReport`]（R3：失败项分明，不再只回
    /// 两 vec 吞掉写失败）。Errors if no checkpoint store is attached.
    /// Conversation rewinding (truncating session history) is handled
    /// by the caller — this only restores code.
    pub async fn rewind(
        &self,
        from_turn: usize,
    ) -> Result<crate::checkpoint::RestoreReport, String> {
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
    /// P20（2026-09-25 能力扩展）冲突预检：文件恢复锚存在时，先对各 turn
    /// 封印的「变更后指纹」与现盘内容比对——有冲突且 `force == false` 时
    /// **零副作用拒绝**（返回 `blocked: true` + 结构化冲突清单 path/期望
    /// 指纹/当前指纹，前端可展示）；`force == true` 强过，回执与日志留痕
    /// （`forced: true`）。
    ///
    /// R31（2026-10-23 会话回退波及面收窄）：回退是会话级 API，但 turn 序
    /// 号与 checkpoint 索引是 loop 全局的——scoped 预检把回退区间内**其他
    /// 会话**声明过变更的路径单列进 `other_session_paths`（同为默认拒绝，
    /// `force` 显式强过）；文件恢复与截断也只作用于本会话的 checkpoint 条
    /// 目，并发会话的回退历史与文件在 force 强过后依然完整（其文件若在恢
    /// 复树内仍会被碾——这正是 other_session_paths 要拦的事，强过即用户
    /// 意志）。
    ///
    /// 会话闸：经 [`Self::under_history_gate`] 真互斥（BUG-HUNT R6——旧
    /// peek 存在「检查后、截断前」窗口，在飞回合的行会被陈旧快照截丢）。
    ///
    /// 返回回执 JSON（kept/removed 计数 + 恢复文件清单 + redoable；被拒时
    /// 为 blocked 结构）。
    pub async fn rewind_to_message(
        &self,
        session_key: &str,
        message_index: usize,
        force: bool,
    ) -> Result<serde_json::Value, String> {
        self.under_history_gate(session_key, "回退", || {
            self.rewind_to_message_inner(session_key, message_index, force)
        })
        .await
    }

    async fn rewind_to_message_inner(
        &self,
        session_key: &str,
        message_index: usize,
        force: bool,
    ) -> Result<serde_json::Value, String> {
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
        // 属于所在 turn，必须随 turn 一起保留/截掉）。轮边界判定走
        // [`crate::chat_log::is_user_turn_boundary`] 单一真相源（steer 注入
        // 行是轮中间的 user 形态行，不算轮首——BUG-20261025-19）。
        let cut = rows[message_index + 1..]
            .iter()
            .position(crate::chat_log::is_user_turn_boundary)
            .map(|p| p + message_index + 1)
            .unwrap_or(rows.len());
        self.rewind_from_cut(session_key, &rows, cut, message_index, force)
            .await
    }

    /// 行动代理 P0（2026-10-14）撤销一等动词后端：回退**最后一个用户轮**。
    /// 语义 = 把 jsonl 截到最后一个 user 行之前（该 user 行及其回复一并移
    /// 除）+ 文件还原到该轮开始前的 checkpoint 态 + 压 undo 栈（redo 可反
    /// 向）。与 [`Self::rewind_to_message`] 共享 [`Self::rewind_from_cut`]
    /// 机体；差异只在截断点计算——本入口允许 cut=0（首轮即可撤销，单轮会
    /// 话「下单→撤销」剧本的主路径，`rewind_to_message` 的「保留 index 所
    /// 在轮」契约表达不了这个形态）。
    ///
    /// busy 检查省略：唯一调用方是会话维护尾巴（gate 已获取会话 = 串行保
    /// 证，会话正忙根本进不到这里）；WSAPI 面的同语义入口走
    /// [`Self::undo_last_turn`]（会话闸 + 委托本方法）。
    pub(crate) async fn rewind_last_turn(
        &self,
        session_key: &str,
    ) -> Result<serde_json::Value, String> {
        let (rows, _total, _, _) = crate::chat_log::read_chat_log(session_key, usize::MAX, None);
        // 截断点 = 最后一个 user 行（turn 边界，user 行即轮首；steer 注入行
        // 不是轮首，轮边界判定走单一真相源跳过——BUG-20261025-19，否则
        // 「撤销最后一轮」切在 steer 行上只删尾巴、轮残留在会话里）。没有
        // user 行 = 没有可撤销的轮次（诚实拒绝）。
        let cut = rows
            .iter()
            .rposition(crate::chat_log::is_user_turn_boundary)
            .ok_or_else(|| "会话没有可撤销的轮次".to_string())?;
        // 回执/谱系引用的下标 = cut 本身（被撤销轮的 user 行原下标）。
        self.rewind_from_cut(session_key, &rows, cut, cut, false)
            .await
    }

    /// 行动代理 E3（收官批 2026-10-15）WSAPI 撤销入口：会话闸
    /// （[`Self::under_history_gate`] 真互斥）+ 委托
    /// [`Self::rewind_last_turn`]。会话正在处理消息时零副作用诚实拒绝
    /// （对话与文件都不动）——`connectors.undo` 的域 B（write 无 undo 声
    /// 明）走此入口回退最后一轮。维护尾巴调用方不经此入口（gate 已持会
    /// 话），直接走内部方法。
    pub async fn undo_last_turn(&self, session_key: &str) -> Result<serde_json::Value, String> {
        self.under_history_gate(session_key, "撤销", || self.rewind_last_turn(session_key))
            .await
    }

    /// 重新生成支撑（2026-10-20，docs/BUG/2026-10-20_chat-regenerate-rewind-buttons-mismatch.md）：
    /// 回退到 `message_index` 行**之前**（`cut = message_index`——保留
    /// `rows[..cut]`，该行及其后所有行移除）。与 [`Self::rewind_to_message`]
    /// （turn 对齐：index 所在整轮保留）的差异只在截断点计算；本入口能表
    /// 达「保留提问、撤掉回复」（前端重新生成）与 cut=0（首轮重新生成，
    /// 现有 turn 对齐入口表达不了的形态，[`Self::rewind_last_turn`] 注释
    /// 为证）。
    ///
    /// 文件恢复锚零新增：cut 落在提问行上时，提问行自带本轮
    /// `checkpoint_turn` begin 标记，共享机体现有 `rows[cut..]` 首标记
    /// find_map 天然命中本轮——文件回滚到提问前，正是重新生成要的语义。
    ///
    /// 契约：`message_index` 必须指向 user 行——assistant 行落点会产生
    /// 「删提问留回复」的破碎轮，loud 拒绝。会话闸与边界检查同
    /// [`Self::rewind_to_message`]；共享 [`Self::rewind_from_cut`] 机体
    /// （P20 冲突预检 / undo 栈 / F8 审计全量复用）。
    pub async fn rewind_before_message(
        &self,
        session_key: &str,
        message_index: usize,
        force: bool,
    ) -> Result<serde_json::Value, String> {
        self.under_history_gate(session_key, "回退", || {
            self.rewind_before_message_inner(session_key, message_index, force)
        })
        .await
    }

    async fn rewind_before_message_inner(
        &self,
        session_key: &str,
        message_index: usize,
        force: bool,
    ) -> Result<serde_json::Value, String> {
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
        if rows[message_index].get("role").and_then(|v| v.as_str()) != Some("user") {
            return Err(format!(
                "message_index {} 不是 user 行（本入口按轮首回退，请指向提问行）",
                message_index
            ));
        }
        if rows[message_index].get("steer").and_then(|v| v.as_bool()) == Some(true) {
            return Err(format!(
                "message_index {} 是 steer 注入行（轮中间标记，不是轮首）——请指向提问行",
                message_index
            ));
        }
        self.rewind_from_cut(session_key, &rows, message_index, message_index, force)
            .await
    }

    /// rewind 共享机体（`rewind_to_message` / `rewind_last_turn` 两入口的
    /// 单一真相源）：`rows` 是全量行快照、`cut` 是保留/截断分界（
    /// `rows[..cut]` 保留、`rows[cut..]` 移除）、`report_index` 是回执与
    /// 回退谱系引用的消息下标。调用方负责 busy 检查与边界检查。
    async fn rewind_from_cut(
        &self,
        session_key: &str,
        rows: &[serde_json::Value],
        cut: usize,
        report_index: usize,
        force: bool,
    ) -> Result<serde_json::Value, String> {
        // 文件恢复锚：cut 起第一个带 checkpoint_turn 标记的行（第一个仍在
        // 的 checkpoint turn）——恢复到它 begin 之前 = index 所在 turn 完成
        // 时的文件态。None = 之后没有 checkpoint turn（或全是无标记旧行）。
        let restore_turn = rows[cut.min(rows.len())..].iter().find_map(|r| {
            r.get("checkpoint_turn")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
        });
        let removed: Vec<serde_json::Value> = rows[cut..].to_vec();

        // P20（2026-09-25 能力扩展）：冲突预检——restore 锚之后各 turn 封印
        // 的「变更后指纹」vs 现盘内容。只读不突变（零副作用），冲突且未
        // force = 拒绝并列结构化清单；force = 强过（审计留痕）。
        // R31（2026-10-23 会话回退波及面收窄）：scoped 预检——回退区间内
        // 其他会话声明过变更的路径进 `other_session_paths` 同样默认拒绝
        // （restore 的全树恢复会碾掉它们，但它们不属于本会话的回退意图）。
        // 其他会话的 checkpoint 条目不会被 scoped 截断摧毁，其 rewind/redo
        // 能力在 force 强过后依然完整。
        let mut report = crate::checkpoint::CheckpointConflictReport::default();
        if let Some(t) = restore_turn
            && let Some(cp) = self.attached_checkpoint()
        {
            report = cp.conflict_scan_scoped(t, Some(session_key));
            if (!report.conflicts.is_empty() || !report.other_session_paths.is_empty()) && !force {
                warn!(
                    "[AgentLoop] rewind 冲突预检拒绝：session={session_key} turn>={t} 检出 {} 个外部修改冲突 + {} 个波及其他会话的路径（force 可强过）",
                    report.conflicts.len(),
                    report.other_session_paths.len()
                );
                return Ok(serde_json::json!({
                    "blocked": true,
                    "reason": "conflict",
                    "session_key": session_key,
                    "message_index": report_index,
                    "restore_turn": t,
                    "conflicts": report.conflicts,
                    "unchecked_paths": report.unchecked_paths,
                    "other_session_paths": report.other_session_paths,
                }));
            }
        }
        let forced =
            force && (!report.conflicts.is_empty() || !report.other_session_paths.is_empty());
        if forced {
            warn!(
                "[AgentLoop] rewind 冲突被 force 强过（审计留痕）：session={session_key} restore_turn={restore_turn:?} 覆盖 {} 个外部修改冲突 + {} 个其他会话波及路径",
                report.conflicts.len(),
                report.other_session_paths.len()
            );
        }

        // undo 依据采集：forward（redo 前向恢复目标）必须在任何突变前读
        // （truncate_from 会清掉 turn ≥ restore 的索引，兜底 latest 要趁
        // 索引还在时读）；post 基线相反——必须在 scoped 截断**之后**读
        // （见步骤 1，R71 口径修复）。
        let forward_tree = match self.attached_checkpoint() {
            // redo 前向恢复目标 = rewind 时刻的实时 tree（begin 树只到
            // 各 turn 开始态，最后一个 turn 的变更只在实况里）。仅文件
            // 恢复路径活跃（有 restore_turn 锚）时采集实时树；无锚时保
            // 持 begin 树语义（可能 None）。
            Some(cp) => {
                if restore_turn.is_some() {
                    cp.current_tree_hex().or_else(|| cp.latest_tree_hex())
                } else {
                    cp.latest_tree_hex()
                }
            }
            None => None,
        };
        // post 基线初值：restore 无锚 = 索引不动，基线就是现状；有锚在
        // scoped 截断后于步骤 1 读取。
        let mut post_tree = if restore_turn.is_none() {
            forward_tree.clone()
        } else {
            None
        };

        // 1) 文件恢复（先于 jsonl 截断——中途崩溃时对话完好，可重试）。
        // R3（2026-10-24 外评）：恢复**不完整时零推进**——jsonl 不截断、
        // checkpoint 索引不清（可重试），返回 blocked 结构化失败清单。旧形
        // 态吞掉恢复失败后照常盖「applied」戳 + 无条件 truncate_from_scoped
        // 摧毁 checkpoint：文件没恢复、对话被截、重试不可能，三输。
        let mut file_restore = "skipped";
        let mut file_note: Option<String> = None;
        // 成功路径恒空（失败走上方 blocked 提前返回）——字段保留为响应形状
        // 稳定性（前端/工具可统一读取）。
        let restore_failures: Vec<(String, String)> = Vec::new();
        let mut written: Vec<String> = Vec::new();
        let mut deleted: Vec<String> = Vec::new();
        if let (Some(t), Some(cp)) = (restore_turn, self.attached_checkpoint()) {
            let report = cp.restore_code(t).await;
            if report.ok() {
                written = report.written;
                deleted = report.deleted;
                // R31：scoped 截断——其他会话的 checkpoint 条目（含落盘
                // JSON）原样保留，其回退历史不被本会话的回退摧毁。
                cp.truncate_from_scoped(t, Some(session_key));
                // R71（BUG-20261024-02）：post 基线 = scoped 截断后的真实索引
                // 态。R31 改 scoped 截断时这里没跟着改口径——此前在截断前按
                // tree_hex_before(t) 采集（unscoped 时代与截断后 latest_tree_hex
                // 等值），其他会话 turn ≥ t 的条目截断后存活且带树，redo 守卫
                // 2 拿 latest_tree_hex() 对照会把它们误判成「undo 后工作区变
                // 了」：零变化也恒拒，且弹出条目作废、redo 永久死亡。两侧同
                // 读截断后索引即同口径（真阳性不变：截断后任何新 turn 仍会
                // 改变 latest → 拒绝）。
                post_tree = cp.latest_tree_hex();
                file_restore = "applied";
            } else {
                let partial = !report.written.is_empty() || !report.deleted.is_empty();
                warn!(
                    "[AgentLoop] rewind 文件恢复不完整（{} 成功 / {} 失败）——会话与 checkpoint 均未改动，可处理后重试：session={session_key} restore_turn={t} 失败项={:?}",
                    report.written.len() + report.deleted.len(),
                    report.failures.len(),
                    report.failures,
                );
                return Ok(serde_json::json!({
                    "blocked": true,
                    "reason": "restore_failed",
                    "session_key": session_key,
                    "message_index": report_index,
                    "restore_turn": t,
                    "file_restore": if partial { "partial" } else { "failed" },
                    "restore_failures": report.failures.iter().map(|(p, e)| {
                        serde_json::json!({ "path": p, "error": e })
                    }).collect::<Vec<_>>(),
                    "note": "文件恢复不完整，会话对话与 checkpoint 均未改动——解决失败原因后可重试本操作",
                }));
            }
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

        // F8（2026-09-27）：force 强过的强恢复动作落审计链台账——跳过冲突
        // 预检覆盖外部修改是高影响操作，warn! 日志轮转即失，Merkle
        // append-only 审计链才是「留痕」的诚实形态。截断成功后记账（文件
        // 恢复 + 截断都已生效，不为未发生的恢复记账）；审计链未启用（默认
        // 关）= None，保留上方 warn 兜底可见性。
        if forced {
            #[cfg(feature = "security")]
            if let Some(plugin) = self.security.security_plugin.as_ref() {
                let recorded = plugin.append_direct_audit_event(
                    "session.rewind",
                    "rewind_to_message",
                    "user",
                    "wsapi",
                    &format!("{session_key}#idx={report_index}"),
                    "forced",
                    &format!(
                        "rewind force 覆盖 {} 个外部修改冲突 + {} 个其他会话波及路径（restore_turn={restore_turn:?}）",
                        report.conflicts.len(),
                        report.other_session_paths.len()
                    ),
                );
                if recorded.is_none() {
                    warn!("[AgentLoop] rewind force 审计链未启用，仅日志留痕");
                }
            }
        }

        // 3) SessionStore 丢缓存（jsonl 是单一真相源，下次 get_or_create
        // 从截断后的 jsonl 自愈重建——sessions.delete/clear 同款纪律）。
        if let Some(store) = self.session_store() {
            store.clear_session(session_key);
        }

        // WS9/P17：回退谱系落盘（last_rewind）——截断成功后记录本会话丢
        // 掉了哪些行/轮（meta sidecar upsert；no-op 回退无谱系事实，不写）。
        // WS9/P18：遗弃后缀 ≥3 轮且 small_model 已配置 → 后台生成分支
        // 摘要写回**本会话** sidecar meta（回退后继续用的就是本会话；未
        // 达阈值/small_model 缺席 = 诚实跳过，不阻塞回退本体）。两步都在
        // 压 undo 栈前做——`removed` 下方要被 move 进栈条目。
        if !removed.is_empty() {
            let dropped_turns = crate::session_fork::row_user_turn_count(&removed);
            crate::chat_log::write_session_rewind(
                session_key,
                report_index,
                rows.len() - cut,
                dropped_turns,
            );
            if let Some(prepared) =
                super::prepare_branch_summary(self, session_key, &removed, dropped_turns)
            {
                prepared.spawn_write();
            }
        }

        // 4) 压 undo 栈（截断成功后才压——失败路径不留脏条目）。
        let redoable = !removed.is_empty();
        if redoable {
            let kept_rows_hash = kept_rows_fingerprint(&rows[..cut]);
            // M4：undo 完成时刻的实时盘面快照（文件恢复已生效）——redo 冲
            // 突预检的漂移基线。取不到（JSON 形态/tree 写失败）= None，扫
            // 描诚实跳过。
            let undo_disk_tree = self
                .attached_checkpoint()
                .and_then(|cp| cp.current_tree_hex());
            let mut stacks = self.rewind_undo_stacks.lock();
            let stack = stacks.entry(session_key.to_string()).or_default();
            stack.push_back(RewindUndoEntry {
                removed_rows: removed,
                kept_count: cut,
                restore_turn,
                forward_tree,
                post_tree,
                kept_rows_hash,
                undo_disk_tree,
            });
            while stack.len() > REWIND_UNDO_STACK_CAP {
                stack.pop_front();
            }
        }

        Ok(serde_json::json!({
            "session_key": session_key,
            "message_index": report_index,
            "kept_count": cut,
            "removed_count": rows.len() - cut,
            "restore_turn": restore_turn,
            "file_restore": file_restore,
            "file_restore_note": file_note,
            "restored_files": { "written": written, "deleted": deleted },
            "restore_failures": restore_failures.iter().map(|(p, e)| {
                serde_json::json!({ "path": p, "error": e })
            }).collect::<Vec<_>>(),
            "redoable": redoable,
            "forced": forced,
            "unchecked_paths": report.unchecked_paths,
        }))
    }

    /// E3 redo：弹本会话 undo 栈顶，反向恢复——被截断的行 VERBATIM 回填
    /// jsonl，文件恢复到 undo 时刻的影子 tree。行数/行身份/tree 基线三重
    /// 陈旧性守卫：undo 之后该会话有任何新消息（含 clear 后同数重填——
    /// 行数守卫的盲区，BUG-20261025-54）、或任何地方有新 checkpoint turn
    /// （工作区变过），诚实拒绝（弹出的条目作废）。
    ///
    /// 诚实边界：redo 不重建 checkpoint 索引（undo 时 turn-*.json 已清），
    /// redo 过的区间无法再次 rewind；文件只恢复工作区内（tree 覆盖范围）。
    /// 会话闸：经 [`Self::under_history_gate`] 真互斥（同 rewind 家族）。
    pub async fn redo_rewind(&self, session_key: &str) -> Result<serde_json::Value, String> {
        self.redo_rewind_with_force(session_key, false).await
    }

    /// M4（2026-10-27 整改）：带 force 的 redo——冲突预检（见
    /// `redo_rewind_inner`）检出外部漂移且未 force 时拒绝（结构化 blocked
    /// 回执，对齐 rewind 冲突块形态）；force=true 强过（审计留痕，回执带
    /// `forced: true`）。
    pub async fn redo_rewind_with_force(
        &self,
        session_key: &str,
        force: bool,
    ) -> Result<serde_json::Value, String> {
        self.under_history_gate(session_key, "重做", || {
            self.redo_rewind_inner(session_key, force)
        })
        .await
    }

    async fn redo_rewind_inner(
        &self,
        session_key: &str,
        force: bool,
    ) -> Result<serde_json::Value, String> {
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
        // 守卫 1b（BUG-20261025-54）：行身份指纹——行数相同但内容已变
        // （clear 后重填到 kept_count 行等）也判陈旧。旧守卫只查行数，
        // 同数重填会放行并把 removed_rows verbatim 回填进无关对话。
        if kept_rows_fingerprint(&rows) != entry.kept_rows_hash {
            return Err("回退后会话内容已变化，redo 已失效（行数未变但内容不同）".to_string());
        }
        // 守卫 2：工作区 tree 基线未变（undo 后没有新 checkpoint turn）。
        // 基线两侧同口径：post 在 undo 侧 scoped 截断后采集（R71），其他
        // 会话的存量条目两侧同现，不构成「变化」。
        let cur_tree = self
            .attached_checkpoint()
            .and_then(|cp| cp.latest_tree_hex());
        if cur_tree != entry.post_tree {
            return Err("回退后工作区发生了新的变化，redo 已失效".to_string());
        }

        // M4（2026-10-27 整改）：redo 冲突预检——undo→redo 窗口内的外部
        // 改动（IDE 手改等）不产生新 checkpoint turn，上面三守卫全盲
        // （守卫 2 的基线在 undo 时已被截断索引钉住，索引外漂移不可见）。
        // 以 **undo 完成时刻的实时盘面**（undo_disk_tree）为基线 diff 当前
        // 盘面——基线不能用 forward_tree：回退本身就会把文件搬离前向目标
        // （恢复到 begin 态），拿它当基线会把合法回退误报成漂移。检出路径
        // = redo restore 将碾掉且非回退本身造成的面（含外部新建文件——
        // restore 对 Added 路径做删除）。冲突且未 force = 拒绝（结构化
        // blocked 回执，对齐 rewind 冲突块形态）；force 强过（审计留痕）。
        // 基线不可用（JSON 形态 / tree 写失败）= None，诚实跳过（前向
        // 恢复本就带不完整注记，best-effort 语义不变）。
        let drifted = match (entry.undo_disk_tree.as_deref(), self.attached_checkpoint()) {
            (Some(hex), Some(cp)) => cp.conflict_scan_since_tree(hex),
            _ => None,
        }
        .unwrap_or_default();
        if !drifted.is_empty() && !force {
            // 条目还栈：冲突拒绝是「等用户确认」的可重试阻塞点，不是终态
            // ——上面弹出的条目必须 push_back（pop_back/push_back 同端，
            // 栈序不变），否则前端「确认覆盖 → force 重试」会撞「没有可
            // 重做的回退」。
            self.rewind_undo_stacks
                .lock()
                .entry(session_key.to_string())
                .or_default()
                .push_back(entry);
            warn!(
                "[AgentLoop] redo 冲突预检拒绝：session={session_key} 检出 {} 个回退窗口内外部修改的路径（force 可强过）: {:?}",
                drifted.len(),
                drifted
            );
            return Ok(serde_json::json!({
                "blocked": true,
                "reason": "conflict",
                "session_key": session_key,
                "drifted_paths": drifted,
                "note": "回退窗口内外部修改了以上文件，redo 的文件恢复将覆盖这些改动；确认覆盖请带 force 重试",
            }));
        }
        let forced = force && !drifted.is_empty();
        if forced {
            warn!(
                "[AgentLoop] redo 冲突被 force 强过（审计留痕）：session={session_key} 覆盖 {} 个外部漂移路径: {:?}",
                drifted.len(),
                drifted
            );
        }

        // 1) 文件前向恢复（best-effort——失败只注记，行回填照常进行；
        // R3：失败项现在带原因进 file_note/restore_failures，不再静默空转）。
        let mut file_restore = "skipped";
        let mut file_note: Option<String> = None;
        let mut restore_failures: Vec<(String, String)> = Vec::new();
        let mut written: Vec<String> = Vec::new();
        let mut deleted: Vec<String> = Vec::new();
        match (entry.forward_tree.as_deref(), self.attached_checkpoint()) {
            (Some(hex), Some(cp)) => match cp.restore_to_tree(hex) {
                Ok(report) => {
                    if report.ok() {
                        written = report.written;
                        deleted = report.deleted;
                        file_restore = "applied";
                    } else {
                        file_restore = if report.written.is_empty() && report.deleted.is_empty() {
                            "failed"
                        } else {
                            "partial"
                        };
                        restore_failures = report.failures.clone();
                        file_note = Some(format!(
                            "前向恢复不完整（{} 失败）：{}",
                            report.failures.len(),
                            report
                                .failures
                                .iter()
                                .map(|(p, e)| format!("{p}: {e}"))
                                .collect::<Vec<_>>()
                                .join("; ")
                        ));
                    }
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
            "restore_failures": restore_failures.iter().map(|(p, e)| {
                serde_json::json!({ "path": p, "error": e })
            }).collect::<Vec<_>>(),
            "forced": forced,
            "drifted_paths": drifted,
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
