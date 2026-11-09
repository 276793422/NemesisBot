//! （重评估，2026-10-13）：工具调用三态 WAL（Write-Ahead
//! Log）。每个工具调用在 dispatch 瀑布里落三行 JSONL——admitted（过了全部
//! 闸门）/ started（真实执行前）/ finished（含 ok）——写到
//! `<workspace>/logs/turn_wal/wal.jsonl`。进程被强杀（`taskkill /F`、崩溃）
//! 时，执行中的调用会留下「started 无 finished」的断头记录；下次启动
//! [`TurnWal::recover_and_finalize`] 扫描补写 interrupted 收尾行并返回续轮
//! 候选（pump 据此合成续轮消息，走正常 gate/admission 流程回到原会话）。
//!
//! 崩溃恢复分流（2026-11-07 runtime 可信化待办 1）：started 后失联的调用
//! 若命中 [`is_irreversible_call`] 不可逆清单（副作用出本机 / 重放=双执行），
//! 补写 unknown 收尾行且**不产续轮候选**——续轮消息会指示模型「必要时重新
//! 执行该工具」，对不可逆工具等于盲目重放；这类调用转人工确认。幂等工具
//! 行为不变（interrupted + 续轮）。
//!
//! 诚实边界：纯 `write` + `flush`（无 fsync）——数据落 OS 页缓存，进程级
//! 崩溃（taskkill /F、panic abort）可靠；**断电/内核崩溃不保**。要顶断电
//! 得每行 fsync（毫秒级 × 每调用三行），对热路径不可接受，故不做。
//!
//! 生命周期：单文件 append + 单代轮转（触 `max_bytes` 时 `wal.jsonl` →
//! `wal.jsonl.old` 覆盖，最新一代优先恢复）；恢复期做 TTL 清扫（超龄的
//! interrupted 候选诚实放弃——会话上下文早已不可信，续轮只会制造混乱）
//! 并 drain 重写。所有 IO 失败一律 warn 降级，绝不 panic——WAL 是观测与
//! 恢复设施，不能反过来拖垮 agent loop。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 单条记录参数预览截断上限（字符）。
pub const ARGS_PREVIEW_MAX_CHARS: usize = 200;
/// 默认轮转阈值（字节）：8 MiB。
pub const DEFAULT_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// 默认恢复 TTL（秒）：7 天——跨过它的工作区中断不再自动续轮。
pub const DEFAULT_TTL_SECS: u64 = 7 * 24 * 3600;

/// WAL 记录状态：运行态三态 + 恢复期补写的两种收尾态。
pub const STATE_ADMITTED: &str = "admitted";
pub const STATE_STARTED: &str = "started";
pub const STATE_FINISHED: &str = "finished";
pub const STATE_INTERRUPTED: &str = "interrupted";
/// 恢复期收尾（不可逆工具专用）：started 后失联且命中不可逆清单——执行
/// 状态不可知，重放=双执行，补 unknown 留痕且不产续轮候选（转人工）。
pub const STATE_UNKNOWN: &str = "unknown";

/// 不可逆工具判定（单一真相源，恢复期消费）。清单**从窄不从宽**：只列
/// 「副作用出本机 / 崩溃瞬间执行状态不可知 / 重放造成双执行」的工具。
/// 本地文件/进程类操作（`MOVE_TOOLS` 家族）有 checkpoint 兜底、重放无害或
/// 可观察回滚，**不进清单**——误伤会把可自动续轮的会话推进人工队列。
///
/// 五态映射（PLAN 待办 1）：admitted=Prepared（未执行，可安全续轮）、
/// started=Executing（崩溃失联→本判定分unknown/幂等续轮两臂）、
/// finished{ok}=Succeeded|Failed。
pub fn is_irreversible_call(tool: &str, args_preview: &str) -> bool {
    match tool {
        // cluster_rpc：派发远端任务（B 端可能已开始 LLM 处理，重复派发 =
        // 远端双执行）；message：出站消息可能已投递，重发 = 用户收两条；
        // connector_invoke：外部系统 op（Gitea 回帖等）可能已生效；
        // mcp_call：第三方 MCP 工具，副作用面不可知（含外部写）。
        "cluster_rpc" | "message" | "connector_invoke" | "mcp_call" => true,
        // git 只有 push 出网（远端状态变更）；本地 commit/分支操作有
        // checkpoint 兜底可安全重放。args_preview 是序列化参数，subcommand
        // 位于前部（200 字符截断内）。
        "git" => args_preview.contains("push"),
        _ => false,
    }
}

/// 单个工具调用的一次状态迁移记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalRecord {
    pub ts_ms: u64,
    pub session_key: String,
    pub channel: String,
    pub chat_id: String,
    pub user: String,
    pub call_id: String,
    pub tool: String,
    pub args_preview: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
}

/// dispatch 侧一次工具调用的身份上下文（三态共享同一份）。
#[derive(Debug, Clone)]
pub struct WalCall {
    pub session_key: String,
    pub channel: String,
    pub chat_id: String,
    pub user: String,
    pub call_id: String,
    pub tool: String,
}

/// 恢复期产出：一个执行到一半被打断的工具调用（已补 interrupted 行）。
/// pump 据此合成续轮 InboundMessage（session_key 原样回填——web 通道的
/// `agent:` 前缀键会被 route_message 原样采纳）。
#[derive(Debug, Clone)]
pub struct ResumeCandidate {
    pub record: WalRecord,
}

/// 工具调用三态 WAL。见模块头。
pub struct TurnWal {
    path: PathBuf,
    old_path: PathBuf,
    file: Mutex<Option<std::fs::File>>,
    max_bytes: u64,
    ttl_secs: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 按**字符**截断（不是字节——中文预览不被腰斩成半字符）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// 工具调用 id 兜底：模型漏给 id（或空串）时用 时间戳+工具名 合成，
/// 保证 WAL 分组键永不空。
pub fn call_id_for(raw_id: &str, tool: &str) -> String {
    if raw_id.trim().is_empty() {
        format!("{}-{}", now_ms(), tool)
    } else {
        raw_id.to_string()
    }
}

impl TurnWal {
    /// 打开（或创建）WAL。`dir` 通常为 `<workspace>/logs/turn_wal`。
    /// IO 失败返回 Err——调用方（agent_factory）据此诚实降级为「无 WAL」。
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("wal.jsonl");
        let old_path = dir.join("wal.jsonl.old");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            path,
            old_path,
            file: Mutex::new(Some(file)),
            max_bytes: DEFAULT_MAX_BYTES,
            ttl_secs: DEFAULT_TTL_SECS,
        })
    }

    /// 测试钩子：自定义轮转阈值与 TTL。
    #[cfg(test)]
    pub(crate) fn with_limits(mut self, max_bytes: u64, ttl_secs: u64) -> Self {
        self.max_bytes = max_bytes;
        self.ttl_secs = ttl_secs;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 记录一次状态迁移。args_preview 按 200 字符截断；IO 失败 warn 降级。
    pub fn log(&self, call: &WalCall, state: &str, args_preview: &str, ok: Option<bool>) {
        let rec = WalRecord {
            ts_ms: now_ms(),
            session_key: call.session_key.clone(),
            channel: call.channel.clone(),
            chat_id: call.chat_id.clone(),
            user: call.user.clone(),
            call_id: call.call_id.clone(),
            tool: call.tool.clone(),
            args_preview: truncate_chars(args_preview, ARGS_PREVIEW_MAX_CHARS),
            state: state.to_string(),
            ok,
        };
        self.append_record(rec);
    }

    /// 追加一行（内部：锁 → 轮转检查 → 写 → flush）。
    ///
    /// 轮转与写共用同一把文件锁：此前轮转判定+rename
    /// 在锁外做，与并发写者（同一 loop 的并发 turn 任务，D-4 信号量放行）
    /// 竞态——①guard=None 窗口进来的写者被 `if let Some` 静默跳过（admitted
    /// 行无 warn 消失 = 恢复链盲区）；②rename 被瞬时句柄挡住（Windows
    /// AV/索引器是常态）时 guard 永久停在 None，此后**所有**行静默丢弃且
    /// degraded 报告永不触发。锁内轮转 + 失败重开兜底双修。
    fn append_record(&self, rec: WalRecord) {
        let line = match serde_json::to_string(&rec) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("[TurnWal] 记录序列化失败（丢弃该行）: {e}");
                return;
            }
        };
        let mut guard = match self.file.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Err(e) = self.rotate_locked(&mut guard, line.len() + 1) {
            tracing::warn!("[TurnWal] 轮转失败（本行仍尝试写入）: {e}");
        }
        if let Some(f) = guard.as_mut() {
            if let Err(e) = f
                .write_all(line.as_bytes())
                .and_then(|_| f.write_all(b"\n"))
            {
                tracing::warn!("[TurnWal] WAL 写入失败（降级为无 WAL）: {e}");
                // 写入失败面可观测（崩溃恢复窗口随之失效）。
                nemesis_utils::degraded::report(
                    "turn_wal",
                    format!(
                        "WAL 写入失败（降级为无 WAL）: {} ({e})",
                        self.path.display()
                    ),
                );
            } else if let Err(e) = f.flush() {
                tracing::warn!("[TurnWal] WAL flush 失败: {e}");
                nemesis_utils::degraded::report(
                    "turn_wal",
                    format!("WAL flush 失败: {} ({e})", self.path.display()),
                );
            }
        }
    }

    /// 触顶轮转（锁内版，`guard` 必须是已持有的文件锁）：`wal.jsonl` →
    /// `wal.jsonl.old`（覆盖上一代），重开句柄。单代保留——最旧一代丢弃
    /// （诚实取舍：恢复窗口只保最新两代文件）。
    ///
    /// 轮转失败不杀 WAL：重开原文件尽力续写——老代码
    /// 失败路径 guard 留 None，此后所有行被静默丢弃（写路径根本不走到
    /// degraded 报告的 write 失败分支，永久死亡不可观测）。
    fn rotate_locked(
        &self,
        guard: &mut Option<std::fs::File>,
        incoming: usize,
    ) -> std::io::Result<()> {
        let cur = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if cur + incoming as u64 <= self.max_bytes {
            return Ok(());
        }
        // 关句柄 → 换名 → 重开；任一步失败都尽力重开原文件续写。
        *guard = None;
        let reopened = (|| {
            if self.old_path.exists() {
                let _ = std::fs::remove_file(&self.old_path);
            }
            std::fs::rename(&self.path, &self.old_path)?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
        })();
        match reopened {
            Ok(f) => {
                *guard = Some(f);
                tracing::info!(
                    "[TurnWal] WAL 触顶轮转（{} → {}）",
                    self.path.display(),
                    self.old_path.display()
                );
                Ok(())
            }
            Err(e) => {
                if let Ok(f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                {
                    *guard = Some(f);
                }
                Err(e)
            }
        }
    }

    /// 读取两代文件（.old 在前），按 call_id 分组归并，返回全部记录
    /// （保持文件顺序）。坏行跳过并计数。
    fn read_all(&self) -> (Vec<WalRecord>, usize) {
        let mut records = Vec::new();
        let mut bad = 0usize;
        for p in [&self.old_path, &self.path] {
            let Ok(f) = std::fs::File::open(p) else {
                continue;
            };
            for line in BufReader::new(f).lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<WalRecord>(trimmed) {
                    Ok(r) => records.push(r),
                    Err(_) => bad += 1,
                }
            }
        }
        (records, bad)
    }

    /// 崩溃恢复（gateway 启动早期调用一次）：
    /// 1. 扫描两代文件，按 call_id 找「无 finished 收尾」的断头调用；
    /// 2. TTL 内的 → 补写 interrupted 行 + 返回续轮候选；
    ///    TTL 外的 → 仅 WARN（诚实放弃，不续轮）；
    /// 3. drain 重写：wal.jsonl 只保留本次补写的 interrupted 行（供再次
    ///    崩溃时留痕），删除.old，保证恢复幂等（二次调用零候选）。
    pub fn recover_and_finalize(&self) -> Vec<ResumeCandidate> {
        let (records, bad) = self.read_all();
        if bad > 0 {
            tracing::warn!("[TurnWal] 恢复扫描跳过 {bad} 条坏行");
        }
        // BUG-20261107-017：受检换算（同族 BUG-20261024-11 cron --every 先例、
        // spill.rs retention saturating_mul 先例）——u64 乘法溢出 debug panic /
        // release 回绕成微小 cutoff，recover_and_finalize 会把近期断头调用全判
        // 超龄误丢。saturating 后语义 = 「TTL 荒谬大 → 全部在窗内」。
        let ttl_cutoff = now_ms().saturating_sub(self.ttl_secs.saturating_mul(1000));

        // (session_key, call_id) → 最新一条非收尾记录。BUG-20261106-004：
        // 分组键必须是二元组——call_id 是模型回传的 id，跨会话可碰撞
        // （小模型自数计数器 per-conversation 重置；空 id 兜底是
        // 时间戳+工具名，同毫秒同工具跨会话同形），单键分组会把 A 会话的
        // finished 记到 B 会话头上，B 的断头调用被误吞、恢复候选静默丢失。
        // session_key 写入侧恒非空（WalCall 带全上下文），二元组天然安全。
        let mut open_calls: std::collections::HashMap<(String, String), WalRecord> =
            std::collections::HashMap::new();
        let mut finished: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for rec in &records {
            let key = (rec.session_key.clone(), rec.call_id.clone());
            match rec.state.as_str() {
                STATE_FINISHED | STATE_INTERRUPTED | STATE_UNKNOWN => {
                    finished.insert(key);
                }
                STATE_ADMITTED | STATE_STARTED => {
                    open_calls.insert(key, rec.clone());
                }
                _ => {}
            }
        }
        // 执行已开始过的调用集合（unrcoverable 与否的判定面）：同组内最新
        // 记录可能是 admitted，但只要出现过 started 就说明执行真实开始过
        // （状态只前进不回退）。
        let started_keys: std::collections::HashSet<(String, String)> = records
            .iter()
            .filter(|r| r.state == STATE_STARTED)
            .map(|r| (r.session_key.clone(), r.call_id.clone()))
            .collect();

        let mut candidates = Vec::new();
        let mut appended_terminal: Vec<WalRecord> = Vec::new();
        let mut unknown_count = 0usize;
        let mut stale = 0usize;
        // HashMap 迭代序随机——按发生时间排序后消费（多断头调用时续轮候选
        // 顺序跨平台/跨运行确定；同会话串行下多候选必跨会话，排序保日志
        // 与续轮消息的可读可重复）。
        let mut open_list: Vec<((String, String), WalRecord)> = open_calls.into_iter().collect();
        open_list.sort_by(|a, b| a.1.ts_ms.cmp(&b.1.ts_ms).then_with(|| a.0.cmp(&b.0)));
        for (key, rec) in open_list {
            if finished.contains(&key) {
                continue;
            }
            if rec.ts_ms < ttl_cutoff {
                stale += 1;
                continue;
            }
            let mut done = rec.clone();
            done.ts_ms = now_ms();
            if started_keys.contains(&key) && is_irreversible_call(&rec.tool, &rec.args_preview) {
                // 不可逆工具执行已开始、结果不可知：重放=双执行，补 unknown
                // 收尾且**不产续轮候选**（转人工确认副作用）。loud WARN——
                // 静默吞掉会让副作用悬案无人知晓。
                done.state = STATE_UNKNOWN.to_string();
                done.ok = None;
                self.append_record(done.clone());
                appended_terminal.push(done);
                unknown_count += 1;
                tracing::warn!(
                    "[TurnWal] 不可逆工具 `{}` 崩溃时执行状态不可知（session={}，args={}）——补 unknown 收尾、不自动续轮，请人工确认副作用是否已生效",
                    rec.tool,
                    rec.session_key,
                    rec.args_preview
                );
                continue;
            }
            done.state = STATE_INTERRUPTED.to_string();
            done.ok = Some(false);
            self.append_record(done.clone());
            appended_terminal.push(done);
            candidates.push(ResumeCandidate { record: rec });
        }
        if stale > 0 {
            tracing::warn!(
                "[TurnWal] {stale} 个断头调用超过恢复 TTL（{}s），诚实放弃续轮",
                self.ttl_secs
            );
        }
        if unknown_count > 0 {
            tracing::warn!(
                "[TurnWal] {unknown_count} 个不可逆工具断头调用标记 unknown（无续轮候选）"
            );
        }

        // drain：当前文件重写为仅含收尾留痕行（interrupted + unknown；旧
        // 扫描读到的 + 本次新补的——append 发生在快照读取之后，二者并集才
        // 完整）；.old 删除。
        let mut all_terminal: Vec<&WalRecord> = records
            .iter()
            .filter(|r| r.state == STATE_INTERRUPTED || r.state == STATE_UNKNOWN)
            .collect();
        all_terminal.extend(appended_terminal.iter());
        let terminal_lines: Vec<String> = all_terminal
            .iter()
            .map(|r| serde_json::to_string(r).unwrap_or_default())
            .collect();
        {
            let mut guard = match self.file.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = None;
            let body = if terminal_lines.is_empty() {
                String::new()
            } else {
                let mut s = terminal_lines.join("\n");
                s.push('\n');
                s
            };
            // 原子重写（write_file_atomic：sync 先于 rename，崩溃窗口内
            // WAL 要么完整新内容要么旧文件，绝不半截——WAL 是 forge C 线
            // 失败重放的数据源，半截 = 终态记录静默丢失）。
            if let Err(e) = nemesis_utils::file::write_file_atomic(
                &self.path.to_string_lossy(),
                body.as_bytes(),
                0o644,
            ) {
                tracing::warn!("[TurnWal] 恢复期 drain 重写失败: {e}");
            }
            // 兜底重开（同族）：写败/重开败任一发生都不许
            // guard 停在 None——否则此后所有 append 被 `if let Some` 静默
            // 跳过，WAL 无 warn 永久死亡。
            if guard.is_none()
                && let Ok(f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
            {
                *guard = Some(f);
            }
        }
        if self.old_path.exists() {
            let _ = std::fs::remove_file(&self.old_path);
        }

        candidates
    }
}

#[cfg(test)]
mod tests;
