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
//! 照常 at-most-once；但账本自此置**不可信**（`durable=false`，sticky 不复活）：
//! [`Journal::record_intent`] 返回 false，`serve_line` 据此拒绝有副作用的
//! 非只读工具（F2，二轮复评）——持久保护缺席时危险工具不得继续（否则执行
//! 体重启后 intent 随内存蒸发，「结果未知」保护失效）。只读白名单照常放行
//! （可用性取舍）。读失败且文件在场 / 中断恢复失败 = loud error + 同样置
//! 不可信，**绝不当空账**（与首次启动无账区分——replay 面已受损必须说话）。
//!
//! 压缩原子化（F1，二轮复评）：写 `.tmp` → 平台替换——Unix `fs::rename`
//! 自身原子覆盖已存在目标（POSIX），无「先删后改名」窗口；Windows 走
//! rename 舞步（current → `.bak`、tmp → current、删 `.bak`）。重写中断的
//! 磁盘状态由 open 时 [`recover_interrupted_rewrite`] 收编（恢复失败 = 置
//! 不可信）。**承诺面 = 进程崩溃/被杀 + 断电**（rename 语义内原子 + 意图/
//! 完成行与压缩 tmp 落盘即 fsync，①断电级收口 2026-10-19：丢意图 = 重启
//! 重发无账可查 → 副作用重放，故 append 路径不豁免）。诚实注记：Windows
//! `FlushFileBuffers` 语义弱于 POSIX fsync（目录项/元数据持久化时序平台
//! 差），注记不建分支。
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
    /// 持久可信旗标（F2）：true = 账本读/写均正常；append 失败、加载读
    /// 错误、中断恢复失败任一发生 → false（**sticky，本进程生命周期不复
    /// 信**）。false 时 `record_intent` 恒返 false，serve_line 据此拒绝
    /// 有副作用的非只读工具。禁用态恒 false（无持久面；`record_intent`
    /// 对禁用直接放行，不查此旗标）。
    durable: bool,
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
            durable: true,
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
            durable: false,
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
    /// 「内存保留集」（原子替换，见 `atomic_rewrite`）——中断状态已先经
    /// `recover_interrupted_rewrite` 收编，此后重写窗口内崩溃的最坏代价 =
    /// 中断态留给下次 open 恢复（不再有丢账形态）。坏行（崩溃截断的半行，
    /// 两种 kind 皆可能）跳过不入内存。
    fn load_and_compact(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        // F1（二轮复评）：先收编上次重写的中断状态（恢复/清残留），再读——
        // 顺序不可换，否则中断态会被 NotFound 误当空账。恢复失败 = 账本面
        // 受损，置不可信（F2 闸拒绝有副作用工具），绝不静默当空账。
        if !recover_interrupted_rewrite(&path) {
            self.durable = false;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            // 首次启动无账 = 空账（静默）。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                // 文件在场但读不了：replay 面已受损，必须说话（不当空账），
                // 且置不可信——持久保护缺席，有副作用工具不得继续（F2）。
                tracing::error!(
                    "[executor] journal load failed (file present; ledger marked not-durable, side-effectful tools will be refused): {e}"
                );
                self.durable = false;
                return;
            }
        };
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
                // 重写失败分两态：current 仍在场（Unix rename 失败 / Windows
                // 舞步前三步失败 / 回滚成功）= 旧账完好原地，append-only 继续
                // 成立（warn 即可）；current 缺席（Windows 舞步 rename 失败
                // 且回滚 rename 也失败）= 旧账滞留 .bak，后续 append 落进无
                // 历史的新文件——崩溃窗口内保留集丢持久面（含未决意图 →
                // 「结果未知」保护失效）。缺席即置不可信（与恢复失败同类：
                // 存储故障不静默），监督器重启经 recover 收编 .bak 自愈。
                if path.exists() {
                    tracing::warn!(
                        "[executor] journal compaction failed (continuing append-only): {e}"
                    );
                } else {
                    tracing::error!(
                        "[executor] journal compaction left no current file (old account stranded in .bak) — ledger marked not-durable, side-effectful tools will be refused: {e}"
                    );
                    self.durable = false;
                }
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

    /// 记意图行（dispatch 前调用）。返回意图是否**可靠落盘**：
    /// - 未启用 / 空 call_id → `true`（无持久面也无闸，per-call 形态零变化）；
    /// - 已有完成账 / 幂等重记 → 返回当前 `durable` 态（no-op）；
    /// - 账本已不可信（sticky）→ `false`（内存意图面照常登记）；
    /// - append 失败 → 置不可信 + `false`（F2：不再静默降级，调用方须拒绝
    ///   有副作用工具）；
    /// - 成功 → `true`。
    pub fn record_intent(&mut self, call_id: &str, tool: &str) -> bool {
        if !self.enabled || call_id.is_empty() {
            return true;
        }
        if self.map.contains_key(call_id) || self.intents.contains_key(call_id) {
            return self.durable;
        }
        if !self.durable {
            // sticky：账本已不可信（历史加载失败/中断恢复失败/此前写失败），
            // 不因后续写成功复信——本进程生命周期内有副作用工具保持拒绝。
            self.intents.insert(call_id.to_string(), tool.to_string());
            return false;
        }
        let line = serde_json::to_string(&IntentLine {
            kind: KIND_INTENT,
            call_id,
            tool,
        })
        .unwrap_or_default();
        if line.is_empty() {
            // 序列化失败（本结构理论不可达）：同按不可信处理，不放行。
            self.durable = false;
            self.intents.insert(call_id.to_string(), tool.to_string());
            return false;
        }
        let Some(path) = self.path.clone() else {
            return false;
        };
        if let Err(e) = append_line(&path, &line) {
            self.durable = false;
            tracing::warn!(
                "[executor] journal intent append failed: ledger marked not-durable, side-effectful tools will be refused: {e}"
            );
        }
        self.intents.insert(call_id.to_string(), tool.to_string());
        self.durable
    }

    /// 记账一行完整响应 JSON（无换行）。未启用 / call_id 为空 / 重复 /
    /// 解析失败 = no-op。append 失败仍 admit 进内存环（进程内 replay 照常
    /// at-most-once），但账本置不可信（F2：有副作用工具自此被闸，不再
    /// 「跨重启退化 at-least-once」的静默降级）。
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
            self.durable = false;
            tracing::warn!(
                "[executor] journal append failed: ledger marked not-durable, side-effectful tools will be refused (in-memory admitted): {e}"
            );
        }
        self.admit(response_line);
    }
}

/// 追加一行到账本（建目录 + append + flush + fsync）。record/record_intent
/// 共用。fsync（①断电级收口，2026-10-19 三步计划）：意图/完成行落盘即持久
/// ——断电窗口不再丢最近几行账（丢意图 = 监督器重启重发无账可查 → 副作用
/// 重放，F2 要防的退化正好从这回来）。sync 失败走既有 append 失败路径
/// （durable=false + F2 闸），无新分支。
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
    file.flush()?;
    file.sync_all()
}

/// 压缩临时文件路径（同目录 `.tmp` 后缀——rename 同卷原子）。
fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

/// Windows rename 舞步的旧账暂存路径（同目录 `.bak` 后缀）。
fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".bak");
    PathBuf::from(s)
}

/// F1（二轮复评）：重写中断状态收编（open 时、任何读/清理之前）。返回
/// 账本是否可信可读（false = 恢复尝试失败，调用方置 durable=false）：
/// - current 在场：替换已完成，`.tmp`/`.bak` = 残留垃圾，韧性清理（清理
///   失败不阻断加载）；
/// - current 缺席：优先 `.bak`（Windows 舞步「current→bak」后中断——旧账
///   完整且是 tmp 保留集的信息超集）；其次 `.tmp`（Unix 原子重写 rename
///   未完成 / 旧实现删-改间隙的遗留形态——tmp 即重写保留集）。rename
///   回位，另一残留顺手清；
/// - 全缺 = 首次启动，空账合法。
fn recover_interrupted_rewrite(path: &Path) -> bool {
    let tmp = tmp_path(path);
    let bak = backup_path(path);
    if path.exists() {
        for stale in [&tmp, &bak] {
            if stale.exists()
                && let Err(e) = nemesis_utils::fsutil::remove_file_retry(stale)
            {
                tracing::warn!("[executor] journal stale rewrite leftover cleanup failed: {e}");
            }
        }
        return true;
    }
    let (src, other) = if bak.exists() {
        (Some(&bak), Some(&tmp))
    } else if tmp.exists() {
        (Some(&tmp), Some(&bak))
    } else {
        (None, None)
    };
    let Some(src) = src else {
        return true;
    };
    match std::fs::rename(src, path) {
        Ok(()) => {
            if let Some(other) = other {
                let _ = nemesis_utils::fsutil::remove_file_retry(other);
            }
            tracing::warn!(
                "[executor] journal recovered interrupted rewrite (adopted {})",
                src.display()
            );
            true
        }
        Err(e) => {
            tracing::error!(
                "[executor] journal rewrite recovery failed ({} -> {}): {e} — ledger marked not-durable, side-effectful tools will be refused",
                src.display(),
                path.display()
            );
            false
        }
    }
}

/// 原子重写账本（F1，二轮复评）：写 `.tmp`（fsync 落盘后才 rename——
/// ①断电级收口：rename 后断电的最坏形态是新旧皆失，tmp 内容必须先到盘）
/// → 平台替换语义。
/// - Unix：`fs::rename` 自身原子覆盖已存在目标（POSIX）——一步到位，
///   旧实现「先删后改名」的丢账窗口整体消除；rename 后父目录 sync 尽力
///   而为（目录项持久化，失败不影响结果正确性；`write_file_atomic` 同款
///   先例）。
/// - Windows：rename 不覆盖已存在目标 → 舞步：`current → .bak` →
///   `.tmp → current` → 删 `.bak`；第 2 步失败把 `.bak` 回位后错误上抛。
///   舞步中断的磁盘状态由 [`recover_interrupted_rewrite`] 在下次 open
///   收编（current 缺 + bak 在 = 恢复旧账）。
///
/// 承诺面 = 进程崩溃/被杀 + 断电（rename 原子 + 落盘即 fsync；Windows
/// `FlushFileBuffers` 语义弱于 POSIX fsync，平台差注记不建分支）。
/// 单写者契约不变：服务循环未启动才可能走到这里。
fn atomic_rewrite(path: &Path, body: &str) -> std::io::Result<()> {
    let tmp = tmp_path(path);
    write_synced(&tmp, body)?;
    #[cfg(unix)]
    {
        std::fs::rename(&tmp, path)?;
        if let Some(dir) = path.parent()
            && let Ok(d) = std::fs::File::open(dir)
        {
            let _ = d.sync_all();
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let bak = backup_path(path);
        if path.exists() {
            // 上轮残留 `.bak` 先清（Windows rename 目标不可已存在）。
            if let Err(e) = nemesis_utils::fsutil::remove_file_retry(&bak) {
                tracing::warn!("[executor] journal stale .bak cleanup failed (continuing): {e}");
            }
            std::fs::rename(path, &bak)?;
        }
        match std::fs::rename(&tmp, path) {
            Ok(()) => {
                let _ = nemesis_utils::fsutil::remove_file_retry(&bak);
                Ok(())
            }
            Err(e) => {
                // tmp → current 失败：旧账回位（尽力而为），错误照常上抛
                // （重写未成 = 保留集还在 tmp/旧账在 bak，下轮重试）。
                if std::fs::rename(&bak, path).is_err() {
                    tracing::error!("[executor] journal rewrite failed and rollback failed: {e}");
                }
                Err(e)
            }
        }
    }
}

/// 全量写入并 fsync（压缩 tmp 落盘专用）。
fn write_synced(path: &Path, body: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::File::create(path)?;
    f.write_all(body.as_bytes())?;
    f.flush()?;
    f.sync_all()
}

#[cfg(test)]
mod tests;
