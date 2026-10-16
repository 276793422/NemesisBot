//! 子进程侧幂等账（P3 T2：在途工具调用 kill -9 语义）。
//!
//! persistent 执行体（`NEMESISBOT_EXECUTOR_AGENT_ID` 在场且非空）对**已
//! 完成**的工具调用记账：`<workspace>/.runtime/journal.jsonl` 每行一条完整
//! v2 响应 JSON。进程崩溃后 supervisor 重建会话并按原 call_id 重发请求时，
//! replay 命中直接回缓存结果——已完成的调用不重复执行（at-most-once）。
//!
//! 记账纪律（`exec_worker::serve_line` 侧排序）：dispatch 完成 → **先
//! record 后回线**。crash 窗口落在「已记账未回线」时，重建会话 replay 能
//! 拿到缓存结果；反向排序会「回线成功但账没记」→ replay 重执行 → 双执行。
//!
//! 容量：内存环形 [`JOURNAL_RING_CAP`] 条，超出逐出最旧——账本只服务崩溃
//! 重建窗口（最近的调用）；open 时把盘上超限旧账裁剪回容量。裁剪是卫生不
//! 是正确性：失败 warn 后照常 append。单写者契约：一个 agent home 同时只有
//! 一个执行体进程在跑，裁剪发生在子进程进入服务循环之前。
//!
//! per-call 子进程（无 agent id）journal 恒禁用：每次一调用即退，账本无
//! 意义，v1 线形态零变化。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use nemesis_agent::executor_transport::ExecutorResponseWire;

/// journal 文件在 workspace 下的固定位置。
pub const JOURNAL_REL: &str = ".runtime/journal.jsonl";

/// 内存环形容量（条）。
pub const JOURNAL_RING_CAP: usize = 512;

pub struct Journal {
    enabled: bool,
    path: Option<PathBuf>,
    /// call_id → 完整响应行（无换行）。
    map: HashMap<String, String>,
    /// 插入序（逐出最旧 + 盘上裁剪重写序）。
    order: VecDeque<String>,
}

impl Journal {
    /// 生产构造：persistent 模式（env agent id 在场且非空）才启用。
    pub fn open(workspace: &Path) -> Self {
        Self::with_agent_id(
            workspace,
            &std::env::var("NEMESISBOT_EXECUTOR_AGENT_ID").unwrap_or_default(),
        )
    }

    /// 显式 agent id 构造（测试用，避免进程级 env 竞态）。
    pub fn with_agent_id(workspace: &Path, agent_id: &str) -> Self {
        if agent_id.is_empty() {
            return Self::disabled();
        }
        let mut this = Self {
            enabled: true,
            path: Some(workspace.join(JOURNAL_REL)),
            map: HashMap::new(),
            order: VecDeque::new(),
        };
        this.load_and_compact();
        this
    }

    /// 恒禁用形态（per-call 子进程 / 测试壳）。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            path: None,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// 测试观测面（非测试构建无消费点，门掉防 dead_code 告警）。
    #[cfg(test)]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 载入盘上账本；确实超限/含坏行时把文件重写回「内存保留集」（截断写，
    /// 服务循环未启动、单写者，窗口内崩溃的最坏代价 = 账本部分丢失 → 该次
    /// replay 退化 at-least-once）。坏行（崩溃截断的半行）跳过不入内存。
    fn load_and_compact(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            return; // 首次启动无账 = 空账
        };
        let total = content.lines().filter(|l| !l.trim().is_empty()).count();
        for line in content.lines() {
            let line = line.trim();
            if !line.is_empty() {
                self.admit(line);
            }
        }
        if self.order.len() < total {
            let body = self
                .order
                .iter()
                .filter_map(|id| self.map.get(id))
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            let body = if body.is_empty() {
                String::new()
            } else {
                format!("{body}\n")
            };
            if let Err(e) = std::fs::write(&path, body) {
                tracing::warn!(
                    "[executor] journal compaction failed (continuing append-only): {e}"
                );
            }
        }
    }

    /// 准入一行完整响应 JSON（无换行）：解析取 call_id → 入 map/order；
    /// 坏行 / 空 call_id / 重复 call_id 不准入；超限逐出最旧。
    fn admit(&mut self, line: &str) {
        let Ok(resp) = serde_json::from_str::<ExecutorResponseWire>(line) else {
            return;
        };
        if resp.call_id.is_empty() || self.map.contains_key(&resp.call_id) {
            return;
        }
        let call_id = resp.call_id.clone();
        self.map.insert(call_id.clone(), line.to_string());
        self.order.push_back(call_id);
        while self.order.len() > JOURNAL_RING_CAP {
            if let Some(evicted) = self.order.pop_front() {
                self.map.remove(&evicted);
            }
        }
    }

    /// replay 查询。空 call_id（v1 请求）恒 miss（admit 从不收录空 id）。
    pub fn lookup(&self, call_id: &str) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        self.map.get(call_id).map(String::as_str)
    }

    /// 记账一行完整响应 JSON（无换行）。未启用 / call_id 为空 / 重复 /
    /// 解析失败 = no-op。append 失败 = 放弃记账并 warn——同 call_id 重发会
    /// 重执行（语义退化为 at-least-once），不做假成功。
    pub fn record(&mut self, response_line: &str) {
        if !self.enabled {
            return;
        }
        // 先验形态（提取 call_id 判空判重），再落盘，成功后才入内存。
        let Ok(resp) = serde_json::from_str::<ExecutorResponseWire>(response_line) else {
            return;
        };
        if resp.call_id.is_empty() || self.map.contains_key(&resp.call_id) {
            return;
        }
        let Some(path) = &self.path else {
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::warn!("[executor] journal dir create failed (call not journalled): {e}");
            return;
        }
        use std::io::Write as _;
        let opened = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path);
        let mut file = match opened {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("[executor] journal open failed (call not journalled): {e}");
                return;
            }
        };
        if let Err(e) = file
            .write_all(response_line.as_bytes())
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.flush())
        {
            tracing::warn!("[executor] journal append failed (call not journalled): {e}");
            return;
        }
        self.admit(response_line);
    }
}

#[cfg(test)]
mod tests;
