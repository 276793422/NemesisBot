//! 子进程侧幂等账（P3 T2：在途工具调用 kill -9 语义；R2 两相账：2026-10-18
//! 三方复评）。
//!
//! persistent 执行体（`NEMESISBOT_EXECUTOR_AGENT_ID` 在场且非空）对工具调用
//! 记两相账：`<workspace>/.runtime/journal.jsonl` 每行一条 JSON。
//!
//! - **意图行** `{"kind":"intent","call_id":..,"tool":..}`：dispatch **前**
//!   写（`serve_line` 侧排序）。意图在账而完成不在 = 上次尝试结果未知。
//! - **完成行**：完整 v2 响应 JSON，执行完成后写（**先 record 后回线**）。
//!   完成准入即了结同 call_id 的意图。
//!
//! 进程崩溃后 supervisor 重建会话并按原 call_id 重发请求时：
//! 完成账命中 → 直接回缓存结果（at-most-once）；未决意图命中 → 按工具分类
//! （只读白名单安全重执行；其余诚实报「结果未知」并**记为 done**——后续重发
//! 回缓存错误，模型以新 call_id 显式重试 = 副作用只发生一次）。诚实边界：
//! 外部副作用的物理 exactly-once 不存在（需业务幂等键，协议级改造），账本
//! 交付的是 outcome-unknown 诚实化 + 分类重发，不是虚假的恰好一次。
//!
//! 容量：完成账内存环形 [`JOURNAL_RING_CAP`] 条，超出逐出最旧——账本只服务
//! 崩溃重建窗口（最近的调用）；open 时把盘上超限旧账裁剪回容量。裁剪是卫生
//! 不是正确性：失败 warn 后照常 append。单写者契约：一个 agent home 同时只有
//! 一个执行体进程在跑，裁剪发生在子进程进入服务循环之前。
//!
//! 盘写失败（append/建目录）不再放弃记账：仍 admit 进内存环——进程内 replay
//! 照常 at-most-once，跨重启退化 at-least-once（warn 诚实注明）。读失败且
//! 文件在场 = loud error（与首次启动无账区分——replay 面已受损必须说话）。
//!
//! 压缩原子化：写 `.tmp` + rename（目标在场先经 [`nemesis_utils::fsutil`]
//! 韧性删除——Windows rename 不覆盖已存在目标；rename 前崩溃残留的 `.tmp`
//! 在下次 open 时清掉）。
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

/// 意图行 kind 标记（完成行无 kind 字段，解析互斥）。
const KIND_INTENT: &str = "intent";

#[derive(serde::Serialize)]
struct IntentLine<'a> {
    kind: &'a str,
    call_id: &'a str,
    tool: &'a str,
}

/// 解析一行意图 JSON；非意图行（含完成行/坏行）→ None。
fn parse_intent_line(line: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("kind")?.as_str()? != KIND_INTENT {
        return None;
    }
    let call_id = v.get("call_id")?.as_str()?.to_string();
    let tool = v.get("tool")?.as_str()?.to_string();
    Some((call_id, tool))
}

pub struct Journal {
    enabled: bool,
    path: Option<PathBuf>,
    /// call_id → 完整响应行（无换行）。
    map: HashMap<String, String>,
    /// 插入序（逐出最旧 + 盘上裁剪重写序）。
    order: VecDeque<String>,
    /// 未决意图：call_id → 工具名（intent 在账而完成不在 = 上次尝试结果
    /// 未知）。完成准入即移除。
    intents: HashMap<String, String>,
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
            intents: HashMap::new(),
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
            intents: HashMap::new(),
        }
    }

    /// 测试观测面（非测试构建无消费点，门掉防 dead_code 告警）。
    #[cfg(test)]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 载入盘上账本（两相：意图行 + 完成行）；超限/含坏行时把完成账重写回
    /// 「内存保留集」（原子替换，见 `atomic_rewrite`）——窗口内崩溃的最坏
    /// 代价 = 完成账部分丢失 → 该次 replay 退化 at-least-once。坏行（崩溃
    /// 截断的半行，两种 kind 皆可能）跳过不入内存。
    fn load_and_compact(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            // 首次启动无账 = 空账（静默）。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                // 文件在场但读不了：replay 面已受损，必须说话（不当空账）。
                tracing::error!(
                    "[executor] journal load failed (file present; crash replay degraded this boot): {e}"
                );
                return;
            }
        };
        // rename 前崩溃残留的压缩临时文件 = 垃圾，清掉（尽力而为）。
        let tmp = tmp_path(&path);
        if tmp.exists()
            && let Err(e) = nemesis_utils::fsutil::remove_file_retry(&tmp)
        {
            tracing::warn!("[executor] journal stale tmp cleanup failed: {e}");
        }
        let mut done_total = 0usize;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some((call_id, tool)) = parse_intent_line(line) {
                // 完成账已在的意图 = 迟到的重复意图行，不准入。
                if !self.map.contains_key(&call_id) {
                    self.intents.insert(call_id, tool);
                }
                continue;
            }
            if self.admit(line) {
                done_total += 1;
            }
        }
        if self.order.len() < done_total {
            // 完成账发生逐出/坏行剔除 → 重写保留集（未决意图行保活在前——
            // 它们不占环形容量、不由裁剪管理，丢掉 = 未决面误报「无账」）。
            let mut body = String::new();
            for (call_id, tool) in &self.intents {
                let line = serde_json::to_string(&IntentLine {
                    kind: KIND_INTENT,
                    call_id,
                    tool,
                })
                .unwrap_or_default();
                if !line.is_empty() {
                    body.push_str(&line);
                    body.push('\n');
                }
            }
            for id in &self.order {
                if let Some(l) = self.map.get(id) {
                    body.push_str(l);
                    body.push('\n');
                }
            }
            if let Err(e) = atomic_rewrite(&path, &body) {
                tracing::warn!(
                    "[executor] journal compaction failed (continuing append-only): {e}"
                );
            }
        }
    }

    /// 准入一行完整响应 JSON（无换行）：解析取 call_id → 入 map/order；
    /// 坏行 / 空 call_id / 重复 call_id 不准入（返回 false）；超限逐出最旧。
    /// 返回 true = 本次新准入。完成准入 = 同 call_id 意图了结。
    fn admit(&mut self, line: &str) -> bool {
        let Ok(resp) = serde_json::from_str::<ExecutorResponseWire>(line) else {
            return false;
        };
        if resp.call_id.is_empty() || self.map.contains_key(&resp.call_id) {
            return false;
        }
        let call_id = resp.call_id.clone();
        self.intents.remove(&call_id);
        self.map.insert(call_id.clone(), line.to_string());
        self.order.push_back(call_id);
        while self.order.len() > JOURNAL_RING_CAP {
            if let Some(evicted) = self.order.pop_front() {
                self.map.remove(&evicted);
            }
        }
        true
    }

    /// replay 查询（完成账）。空 call_id（v1 请求）恒 miss（admit 从不收录
    /// 空 id）。
    pub fn lookup(&self, call_id: &str) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        self.map.get(call_id).map(String::as_str)
    }

    /// 未决意图查询：Some(tool) = 上次尝试结果未知（intent 在账而完成不在）。
    pub fn pending_intent(&self, call_id: &str) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        self.intents.get(call_id).map(String::as_str)
    }

    /// 记意图行（dispatch 前调用）。未启用 / 空 call_id / 已有完成账 /
    /// 幂等重记 = no-op。落盘失败仍入内存（意图面进程内成立，跨重启丢失
    /// = 退回无账语义 at-least-once，warn 注明）。
    pub fn record_intent(&mut self, call_id: &str, tool: &str) {
        if !self.enabled || call_id.is_empty() {
            return;
        }
        if self.map.contains_key(call_id) || self.intents.contains_key(call_id) {
            return;
        }
        let line = serde_json::to_string(&IntentLine {
            kind: KIND_INTENT,
            call_id,
            tool,
        })
        .unwrap_or_default();
        if line.is_empty() {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        if let Err(e) = append_line(&path, &line) {
            tracing::warn!(
                "[executor] journal intent append failed (in-memory only; cross-restart replay degrades to at-least-once): {e}"
            );
        }
        self.intents.insert(call_id.to_string(), tool.to_string());
    }

    /// 记账一行完整响应 JSON（无换行）。未启用 / call_id 为空 / 重复 /
    /// 解析失败 = no-op。append 失败仍 admit 进内存环——进程内 replay 照常
    /// at-most-once，跨重启退化 at-least-once（不做假成功，warn 注明）。
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
        let Some(path) = self.path.clone() else {
            return;
        };
        if let Err(e) = append_line(&path, response_line) {
            tracing::warn!(
                "[executor] journal append failed (in-memory only; cross-restart replay degrades to at-least-once): {e}"
            );
        }
        self.admit(response_line);
    }
}

/// 追加一行到账本（建目录 + append + flush）。record/record_intent 共用。
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()
}

/// 压缩临时文件路径（同目录 `.tmp` 后缀——rename 同卷原子）。
fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

/// 原子重写账本：写 `.tmp` → 目标在场先韧性删除（Windows rename 不覆盖
/// 已存在目标）→ rename。单写者（服务循环未启动），窗口内崩溃最坏代价 =
/// 目标保持旧内容或 `.tmp` 残留（下次 open 清理）。
fn atomic_rewrite(path: &Path, body: &str) -> std::io::Result<()> {
    let tmp = tmp_path(path);
    std::fs::write(&tmp, body)?;
    if path.exists() {
        nemesis_utils::fsutil::remove_file_retry(path)?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests;
