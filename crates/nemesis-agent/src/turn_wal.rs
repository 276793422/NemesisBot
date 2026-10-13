//! L8（内部专项总计划重评估，2026-10-13）：工具调用三态 WAL（Write-Ahead
//! Log）。每个工具调用在 dispatch 瀑布里落三行 JSONL——admitted（过了全部
//! 闸门）/ started（真实执行前）/ finished（含 ok）——写到
//! `<workspace>/logs/turn_wal/wal.jsonl`。进程被强杀（`taskkill /F`、崩溃）
//! 时，执行中的调用会留下「started 无 finished」的断头记录；下次启动
//! [`TurnWal::recover_and_finalize`] 扫描补写 interrupted 收尾行并返回续轮
//! 候选（pump 据此合成续轮消息，走正常 gate/admission 流程回到原会话）。
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

/// WAL 记录状态三态 + 恢复期补写的收尾态。
pub const STATE_ADMITTED: &str = "admitted";
pub const STATE_STARTED: &str = "started";
pub const STATE_FINISHED: &str = "finished";
pub const STATE_INTERRUPTED: &str = "interrupted";

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

    /// 追加一行（内部：轮转检查 → 序列化 → 写 → flush）。
    fn append_record(&self, rec: WalRecord) {
        let line = match serde_json::to_string(&rec) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("[TurnWal] 记录序列化失败（丢弃该行）: {e}");
                return;
            }
        };
        if let Err(e) = self.maybe_rotate(line.len() + 1) {
            tracing::warn!("[TurnWal] 轮转失败（本行仍尝试写入）: {e}");
        }
        let mut guard = match self.file.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(f) = guard.as_mut() {
            if let Err(e) = f
                .write_all(line.as_bytes())
                .and_then(|_| f.write_all(b"\n"))
            {
                tracing::warn!("[TurnWal] WAL 写入失败（降级为无 WAL）: {e}");
            } else if let Err(e) = f.flush() {
                tracing::warn!("[TurnWal] WAL flush 失败: {e}");
            }
        }
    }

    /// 触顶轮转：`wal.jsonl` → `wal.jsonl.old`（覆盖上一代），重开句柄。
    /// 单代保留——最旧一代丢弃（诚实取舍：恢复窗口只保最新两代文件）。
    fn maybe_rotate(&self, incoming: usize) -> std::io::Result<()> {
        let cur = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if cur + incoming as u64 <= self.max_bytes {
            return Ok(());
        }
        // 关句柄 → 换名 → 重开。
        {
            let mut guard = match self.file.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = None;
        }
        if self.old_path.exists() {
            let _ = std::fs::remove_file(&self.old_path);
        }
        std::fs::rename(&self.path, &self.old_path)?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        *self.file.lock().unwrap_or_else(|p| p.into_inner()) = Some(f);
        tracing::info!(
            "[TurnWal] WAL 触顶轮转（{} → {}）",
            self.path.display(),
            self.old_path.display()
        );
        Ok(())
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
    ///    崩溃时留痕），删除 .old，保证恢复幂等（二次调用零候选）。
    pub fn recover_and_finalize(&self) -> Vec<ResumeCandidate> {
        let (records, bad) = self.read_all();
        if bad > 0 {
            tracing::warn!("[TurnWal] 恢复扫描跳过 {bad} 条坏行");
        }
        let ttl_cutoff = now_ms().saturating_sub(self.ttl_secs * 1000);

        // call_id → (最新一条非收尾记录, 是否 finished)。
        let mut open_calls: std::collections::HashMap<String, WalRecord> =
            std::collections::HashMap::new();
        let mut finished: std::collections::HashSet<String> = std::collections::HashSet::new();
        for rec in &records {
            match rec.state.as_str() {
                STATE_FINISHED | STATE_INTERRUPTED => {
                    finished.insert(rec.call_id.clone());
                }
                STATE_ADMITTED | STATE_STARTED => {
                    open_calls.insert(rec.call_id.clone(), rec.clone());
                }
                _ => {}
            }
        }

        let mut candidates = Vec::new();
        let mut appended_interrupted: Vec<WalRecord> = Vec::new();
        let mut stale = 0usize;
        // HashMap 迭代序随机——按发生时间排序后消费（多断头调用时续轮候选
        // 顺序跨平台/跨运行确定；同会话串行下多候选必跨会话，排序保日志
        // 与续轮消息的可读可重复）。
        let mut open_list: Vec<(String, WalRecord)> = open_calls.into_iter().collect();
        open_list.sort_by(|a, b| a.1.ts_ms.cmp(&b.1.ts_ms).then_with(|| a.0.cmp(&b.0)));
        for (call_id, rec) in open_list {
            if finished.contains(&call_id) {
                continue;
            }
            if rec.ts_ms < ttl_cutoff {
                stale += 1;
                continue;
            }
            let mut done = rec.clone();
            done.ts_ms = now_ms();
            done.state = STATE_INTERRUPTED.to_string();
            done.ok = Some(false);
            self.append_record(done.clone());
            appended_interrupted.push(done);
            candidates.push(ResumeCandidate { record: rec });
        }
        if stale > 0 {
            tracing::warn!(
                "[TurnWal] {stale} 个断头调用超过恢复 TTL（{}s），诚实放弃续轮",
                self.ttl_secs
            );
        }

        // drain：当前文件重写为仅含 interrupted 留痕行（旧扫描读到的 +
        // 本次新补的——append 发生在快照读取之后，二者并集才完整）；
        // .old 删除。
        let mut all_interrupted: Vec<&WalRecord> = records
            .iter()
            .filter(|r| r.state == STATE_INTERRUPTED)
            .collect();
        all_interrupted.extend(appended_interrupted.iter());
        let interrupted_lines: Vec<String> = all_interrupted
            .iter()
            .map(|r| serde_json::to_string(r).unwrap_or_default())
            .collect();
        {
            let mut guard = match self.file.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = None;
            let body = if interrupted_lines.is_empty() {
                String::new()
            } else {
                let mut s = interrupted_lines.join("\n");
                s.push('\n');
                s
            };
            if let Err(e) = std::fs::write(&self.path, body) {
                tracing::warn!("[TurnWal] 恢复期 drain 重写失败: {e}");
            } else if let Ok(f) = std::fs::OpenOptions::new().append(true).open(&self.path) {
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
