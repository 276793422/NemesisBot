//! Chat log module — append-only JSONL log for user-facing chat history.
//!
//! Session files (`sessions/`) serve LLM context recovery (summarization,
//! truncation). This module provides a separate, append-only log that never
//! gets truncated, ensuring the user-facing chat history is always complete.

use chrono::Local;
use nemesis_path::default_path_manager;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::r#loop::FileChange;
use crate::work_timeline::WorkTimeline;

/// N5（2026-10-13 机会项）：交付物条目单一真相源在 `nemesis_types::agent`
/// （`AgentEvent::DeliverablesRegistered` 载荷与本字段共用同一 serde 形态，
/// 不维护两份镜像 struct）；本模块 re-export 保持 `crate::chat_log::
/// PresentedFile` 路径稳定。与 D3 `file_changes` 的分工：file_changes 是
/// 「改了什么」（消息↔文件变更映射，服务 rewind/diff），deliverables 是
/// 「该看什么」（结果面向用户的产物指引，服务前端交付物卡片）。路径引用
/// 不落字节（与 images 同语义）。
pub use nemesis_types::agent::PresentedFile;

/// D3（devtool-upgrade 阶段 5）：chat_log 条目在 `role`/`content` 之外的
/// 可选元数据。jsonl 行 shape 的单一真相源仍是 [`write_chat_entry`]；本
/// struct 是**写入 API** 的单一入口——扩字段只动这里，不再加位置参数
/// （`append_chat_log_full*` 旧签名保留为薄包装，存量调用点不动）。
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatLogMeta<'a> {
    /// `"model"` 字段（assistant 行的「供应商·模型名」徽标）。`None` 不写。
    pub model: Option<&'a str>,
    /// `"cron_job_id"` 字段（定时任务来源标记）。`None` 不写。
    pub cron_job_id: Option<&'a str>,
    /// `"cron_job_name"` 字段。`None` 不写。
    pub cron_job_name: Option<&'a str>,
    /// `"images"` 字段（图片**路径引用**，不落字节）。空切片不写。
    pub images: &'a [String],
    /// D3：`"file_changes"` 字段——本 turn 声明式文件工具的变更清单
    /// （`[{path, kind}]`；AgentLoop 在 dispatch 瀑布经 `preview_all`
    /// 收集，assistant 最终回复落盘时去重随行）。消息↔文件变更映射；
    /// M3 会话级 diff 查看器的数据源。空切片不写——旧条目/无变更解析
    /// 不受影响。
    pub file_changes: &'a [FileChange],
    /// N5（2026-10-13 机会项）：`"deliverables"` 字段——本 turn 的交付物
    /// 清单（`[{path, title, note}]`；`present_files` 工具登记，assistant
    /// 最终回复落盘时去重随行）。与 `file_changes` 并列不重叠：前者服务
    /// 变更映射（rewind/diff），后者服务交付展示。空切片不写。
    pub deliverables: &'a [PresentedFile],
    /// E3（devtool-upgrade 阶段 5）：`"checkpoint_turn"` 字段——本行所属
    /// turn 的 checkpoint 序号（`turn_preamble` begin 的值随 admission 穿针
    /// 到行落盘点）。E3 消息级回退用它把 jsonl 行精确定位回 checkpoint
    /// turn（不靠内容/时间戳猜测）。`None` 不写——无 checkpoint store 的
    /// 行回退时只截断对话不回滚文件。
    pub checkpoint_turn: Option<usize>,
    /// BUG-20261025-19：`"steer"` 字段——本行是轮中间的 steer 注入行
    /// （run_loop 注入路径写入，`role` 仍是 `"user"` 但**不是轮首**）。
    /// rewind/fork 的轮边界判定（[`is_user_turn_boundary`]）据此跳过本行
    /// ——否则「撤销最后一轮」的截断点会切在 steer 行上，轮的提问与首轮
    /// 回复残留、只删掉 steer 尾巴。`false` 不写（缺字段 = 旧行/普通行，
    /// 一律按轮边界处理，与旧行为逐字节一致）。
    pub steer: bool,
    /// Work 时间线（2026-10-30 ChatPanel 长任务时间线改造）：本 turn 的
    /// 「中间轮叙述+工具调用」交错时间线（`work_timeline::WorkTimeline`），
    /// assistant 最终回复行随行落盘（`"work"` 键）。`None` 不写——直答
    /// （无中间轮）与旧条目读侧不受影响（与 file_changes 同一宽容读契约）。
    pub work: Option<&'a WorkTimeline>,
}

/// D3：全字段追加入口（`ChatLogMeta` 携带全部可选元数据）。
pub fn append_chat_log_meta(session_key: &str, role: &str, content: &str, meta: &ChatLogMeta<'_>) {
    write_chat_entry(
        session_key,
        role,
        content,
        meta.model,
        meta.cron_job_id,
        meta.cron_job_name,
        meta.images,
        meta.file_changes,
        meta.deliverables,
        meta.checkpoint_turn,
        None,
        meta.steer,
        meta.work,
    );
}

/// D3：把本 turn 的 FileChange 流水按 `path` 去重——保留首次出现顺序，
/// `kind` 取**最后一次**声明（同文件先 Create 后 Modify，对消息级展示的
/// 语义就是「该文件被动过」；Create/Delete 的恢复语义由 checkpoint 保留，
/// 不受此投影影响）。AgentLoop drain 时调用（单一投影点，M3 依赖此形状）。
pub fn dedup_file_changes(changes: Vec<FileChange>) -> Vec<FileChange> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out: Vec<FileChange> = Vec::with_capacity(changes.len());
    for c in changes {
        match seen.get(&c.path) {
            Some(&i) => out[i].kind = c.kind,
            None => {
                seen.insert(c.path.clone(), out.len());
                out.push(c);
            }
        }
    }
    out
}

/// N5：把本 turn 的交付物按 `path` 去重——与 [`dedup_file_changes`] 同规则：
/// 保留首次出现顺序（展示顺序稳定），`title`/`note` 取**最后一次**登记
/// （agent 对同一产物的最新描述胜出）。
pub fn dedup_presented_files(files: Vec<PresentedFile>) -> Vec<PresentedFile> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out: Vec<PresentedFile> = Vec::with_capacity(files.len());
    for f in files {
        match seen.get(&f.path) {
            Some(&i) => {
                out[i].title = f.title;
                out[i].note = f.note;
            }
            None => {
                seen.insert(f.path.clone(), out.len());
                out.push(f);
            }
        }
    }
    out
}

/// Append a chat message to the JSONL log file.
pub fn append_chat_log(session_key: &str, role: &str, content: &str) {
    append_chat_log_full(session_key, role, content, None, None, None);
}

/// Append a chat message with an optional model badge (`provider/name`).
///
/// When `model` is `Some`, an extra `"model"` field is written so the
/// Dashboard can render a "供应商·模型名" badge on the assistant message after
/// a history reload. `None` (user rows, legacy callers) omits the field — old
/// jsonl entries without it parse fine (read side treats missing = no badge).
pub fn append_chat_log_with_model(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
) {
    append_chat_log_full(session_key, role, content, model, None, None);
}

/// 集群续行归属（2026-09-23）：模型徽章 + worker 节点归属的追加变体。
///
/// `source_node` = 实际执行任务的集群节点名（`cluster_rpc` 派发的续行回复
/// 由主 LLM 转述，干活的是远端节点）。`Some` 时写入 `"source_node"` 字段，
/// Dashboard 渲染「节点 X」徽章（与模型徽章并列）；`None`（普通回复）不写
/// 该键——旧 jsonl 条目照常解析，读侧缺字段 = 无徽章。
pub fn append_chat_log_with_model_and_node(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
    source_node: Option<&str>,
) {
    write_chat_entry(
        session_key,
        role,
        content,
        model,
        None,
        None,
        &[],
        &[],
        &[],
        None,
        source_node,
        false,
        None,
    );
}

/// Full append: optional model badge AND optional cron origin marker.
///
/// `cron_job_id` / `cron_job_name`: when `Some`, marks this entry as
/// originating from a scheduled (cron) task, so the Dashboard can label it
/// (🕒) and filter "只看定时任务" in the session browser. `None` (the common
/// case) omits the fields — old jsonl entries without them parse fine.
pub fn append_chat_log_full(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
    cron_job_id: Option<&str>,
    cron_job_name: Option<&str>,
) {
    write_chat_entry(
        session_key,
        role,
        content,
        model,
        cron_job_id,
        cron_job_name,
        &[],
        &[],
        &[],
        None,
        None,
        false,
        None,
    );
}

/// T6（多模态）：带图片路径引用的追加变体（`append_chat_log_full` 的签名
/// 不动，媒体信息走此函数）。`images` 非空时写入 `"images": [路径...]`
/// 字段——只存**路径引用**，不落 base64 字节（与 SessionStore 的
/// `StoredMessage.image_refs` 同源同语义）；读侧缺字段 = 无图，旧条目解析
/// 不受影响。
#[allow(clippy::too_many_arguments)]
pub fn append_chat_log_full_with_images(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
    cron_job_id: Option<&str>,
    cron_job_name: Option<&str>,
    images: &[String],
) {
    write_chat_entry(
        session_key,
        role,
        content,
        model,
        cron_job_id,
        cron_job_name,
        images,
        &[],
        &[],
        None,
        None,
        false,
        None,
    );
}

/// Write core shared by all append variants (single source of truth for the
/// jsonl entry shape). `images` non-empty → extra `"images"` array field.
/// `file_changes` non-empty → extra `"file_changes"` array field (D3).
/// `deliverables` non-empty → extra `"deliverables"` array field (N5).
/// `checkpoint_turn` Some → extra `"checkpoint_turn"` numeric field (E3).
/// `steer` true → extra `"steer": true` field (BUG-20261025-19 轮边界标记).
/// `work` Some → extra `"work"` object field（2026-10-30 Work 时间线，turn
/// 中间过程交错随行；直答/旧条目缺字段读侧宽容）.
#[allow(clippy::too_many_arguments)]
fn write_chat_entry(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
    cron_job_id: Option<&str>,
    cron_job_name: Option<&str>,
    images: &[String],
    file_changes: &[FileChange],
    deliverables: &[PresentedFile],
    checkpoint_turn: Option<usize>,
    source_node: Option<&str>,
    steer: bool,
    work: Option<&WorkTimeline>,
) {
    let path = log_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("[chat_log] Failed to open {}: {}", path.display(), e);
            // M2：写入失败面可观测——进降级登记表，gateway 泵到 SSE/健康面。
            nemesis_utils::degraded::report(
                "chat_log",
                format!("open failed: {} ({e})", path.display()),
            );
            return;
        }
    };
    let mut entry = serde_json::json!({
        "role": role,
        "content": content,
        "timestamp": Local::now().to_rfc3339(),
    });
    if let Some(m) = model {
        entry["model"] = serde_json::Value::String(m.to_string());
    }
    if let Some(id) = cron_job_id {
        entry["cron_job_id"] = serde_json::Value::String(id.to_string());
    }
    if let Some(name) = cron_job_name {
        entry["cron_job_name"] = serde_json::Value::String(name.to_string());
    }
    if let Some(node) = source_node {
        entry["source_node"] = serde_json::Value::String(node.to_string());
    }
    if !images.is_empty() {
        entry["images"] = serde_json::Value::Array(
            images
                .iter()
                .map(|p| serde_json::Value::String(p.clone()))
                .collect(),
        );
    }
    // D3：本 turn 声明式文件工具变更（`[{path, kind}]`）。缺字段 = 旧条目
    // /无变更，读侧解析不受影响（与 model/cron/images 同一宽容读法）。
    if !file_changes.is_empty() {
        entry["file_changes"] = serde_json::to_value(file_changes).unwrap_or(Value::Null);
    }
    // N5：本 turn 交付物（`[{path, title, note}]`）。缺字段 = 旧条目/无交付
    // 物，读侧解析不受影响（同一宽容读法）。
    if !deliverables.is_empty() {
        entry["deliverables"] = serde_json::to_value(deliverables).unwrap_or(Value::Null);
    }
    // E3：本行所属 turn 的 checkpoint 序号（消息级回退的行→turn 定位锚）。
    // 缺字段 = 无 store 时代的行，回退只截断对话不回滚文件。
    if let Some(t) = checkpoint_turn {
        entry["checkpoint_turn"] = serde_json::Value::from(t);
    }
    // BUG-20261025-19：steer 注入标记（轮边界判定据此跳过本行；缺字段 =
    // 旧行/普通行，一律按轮边界处理）。
    if steer {
        entry["steer"] = serde_json::Value::Bool(true);
    }
    // Work 时间线（2026-10-30 ChatPanel 长任务时间线改造）：本 turn 中间
    // 过程交错时间线（work_timeline::WorkTimeline serde 形态）。缺字段 =
    // 直答/旧条目，读侧宽容（与 file_changes 同纪律）。
    if let Some(w) = work {
        entry["work"] = serde_json::to_value(w).unwrap_or(Value::Null);
    }
    // R100（BUG-20261025-25）：整行（含换行）先序列化成 String 再**单次**
    // write_all。此前 `writeln!(file, "{}", entry)` 对 Value 走 Display 的
    // 逐片段写（append 模式每片段独立定位 EOF），同会话并发写（spawned
    // 续行 final vs 主循环下一条 user 行）可把别人的行插进本行 JSON 片段
    // 中间 → 行撕裂。单次 write 的追加定位是原子的（POSIX O_APPEND /
    // Windows FILE_APPEND_DATA），行间只可能整体排序、不可能半行插入。
    let mut line = entry.to_string();
    line.push('\n');
    if let Err(e) = file.write_all(line.as_bytes()) {
        tracing::warn!("[chat_log] Failed to write to {}: {}", path.display(), e);
        // M2：写入失败面可观测（磁盘满/句柄失效等）。
        nemesis_utils::degraded::report(
            "chat_log",
            format!("write failed: {} ({e})", path.display()),
        );
        return;
    }
    // T6（扩展计划 D5）：append 记账——旁路事件账本，sha-only（chat_log
    // 本体是正文真相源，账本职责是完整性证据）。best-effort，失败不阻断。
    crate::event_ledger::ledger_record(
        session_key,
        crate::event_ledger::LedgerOp::Append,
        "chat_log",
        vec![crate::event_ledger::LedgerRow::from_chat_row(&entry, false)],
    );
    // U20 (sixth batch): lazy FTS index hook — best-effort, failures inside
    // are swallowed (the next full reindex repairs). Timestamp mirrors the
    // entry written above. history-search 关：整段不编译（FTS 索引随
    // rusqlite 进出，chat_log 本体照常写）。
    #[cfg(feature = "history-search")]
    {
        let ts = entry
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("");
        crate::history_search::index_append(session_key, role, content, ts);
    }
}

/// Read chat log with pagination.
///
/// Returns `(page, total_count, has_more, oldest_index)`. `before_index` is the
/// exclusive upper bound — "give me items before this index". `None` means the
/// newest batch. Messages are returned in chronological order (oldest first).
///
/// 单遍实现（BUG 2026-09-22 历史加载秒卡）：一次文件读同时完成「数行
/// （total）」与「取窗口」——滑窗只保留 end 之前的最近 `limit` 行，不再
/// 全文件扫两遍（几十 MB 日志在慢盘 + Defender 实时扫描下旧实现秒级）。
/// `total`/`oldest_index` 保持 raw 行号坐标（分页契约：E3 rewind 的
/// message_index 锚 + 前端行编号链的基准），与旧两遍实现逐字段一致。
/// 全量读者形态（fork/rewind/export/share/CLI 传 `usize::MAX`）解析行直接
/// 滑入窗口（`Option` 占位保持 raw 行号对齐——不可解析行也占一位），无
/// 驱逐、内存与旧实现持平；小窗口形态存 raw 行、EOF 后只解析窗口。
pub fn read_chat_log(
    session_key: &str,
    limit: usize,
    before_index: Option<usize>,
) -> (Vec<Value>, usize, bool, usize) {
    let path = log_path(session_key);
    // is_file 而非 exists：Linux 上 File::open 对目录成功（O_RDONLY 合法），
    // 而下面 BufReader::lines() 的 count/filter_map 在 read Err（目录 fd 每次
    // read 都 EISDIR，Lines 迭代器不熔断）上会无限自旋。目录占位按缺失处理
    // （Windows 上 open 目录本就失败走同臂）。（2026-09-01 Linux 首跑暴露）
    if !path.is_file() {
        return (Vec::new(), 0, false, 0);
    }

    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return (Vec::new(), 0, false, 0),
    };

    let whole_log = limit >= 100_000;
    let mut total = 0usize;
    let mut raw_window: std::collections::VecDeque<String> = Default::default();
    let mut parsed_window: std::collections::VecDeque<Option<Value>> = Default::default();
    for item in std::io::BufReader::new(file).lines() {
        // 计数含 Err 项（旧 count() 语义逐字节保持）；Err 后中断（旧
        // count 照数、pass2 读不到——窗口端等价于在此截断）。
        total += 1;
        let Ok(line) = item else {
            break;
        };
        let i = total - 1;
        if let Some(bi) = before_index
            && i >= bi
        {
            continue; // bi 之后的行只计数不进窗（bi = 排他上界）
        }
        if whole_log {
            let v = serde_json::from_str::<Value>(&line)
                .ok()
                .filter(|v| v.get("role").and_then(Value::as_str) != Some("boundary"));
            parsed_window.push_back(v);
            if parsed_window.len() > limit {
                parsed_window.pop_front();
            }
        } else {
            raw_window.push_back(line);
            if raw_window.len() > limit {
                raw_window.pop_front();
            }
        }
    }

    let end = before_index.map_or(total, |bi| bi.min(total));
    let start = end.saturating_sub(limit);
    let page: Vec<Value> = if whole_log {
        parsed_window.into_iter().flatten().collect()
    } else {
        raw_window
            .iter()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            // ROUND-5 FIX: boundary events moved to a sidecar file (see
            // `boundary_path`), so the message jsonl can no longer contain them.
            // The filter stays as a one-line guard for dev machines that ran a
            // batch-3 build with interleaved boundaries — cheap and harmless.
            .filter(|v| v.get("role").and_then(|r| r.as_str()) != Some("boundary"))
            .collect()
    };

    (page, total, start > 0, start)
}

/// Async wrapper of [`read_chat_log`]（BUG 2026-09-22 历史加载秒卡的
/// worker 占用半边）：读日志是纯同步阻塞 IO，wire 历史路径（WSAPI
/// `chat.history`）跑在 tokio worker 上——大日志的秒级读会占住 worker、
/// 拖慢同进程全部异步任务。`spawn_blocking` 移出（crate 内既有模式，
/// 见 loop_tools.rs / loop.rs）。
pub async fn read_chat_log_async(
    session_key: &str,
    limit: usize,
    before_index: Option<usize>,
) -> (Vec<Value>, usize, bool, usize) {
    let key = session_key.to_string();
    tokio::task::spawn_blocking(move || read_chat_log(&key, limit, before_index))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("[chat_log] read_chat_log blocking task join failed: {e}");
            (Vec::new(), 0, false, 0)
        })
}

/// Read ONLY the boundary (audit) events of a session — replay/audit tooling
/// reads through here. Round-5 fix: these live in the sidecar file
/// (`logs/boundary/<safe_key>.jsonl`), not the message jsonl.
pub fn read_boundary_events(session_key: &str) -> Vec<Value> {
    let path = boundary_path(session_key);
    if !path.exists() {
        return Vec::new();
    }
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .collect()
}

/// Resolve the JSONL file path for a session key.
/// `pub(crate)`：[`crate::chat_trash`] 的回收站 move/restore 按同键寻址
/// 同一三件套（路径真相源仍是本模块）。
pub(crate) fn log_path(session_key: &str) -> PathBuf {
    // SAN-01/SAN-05：白名单消毒（单一真相源
    // `nemesis_utils::sanitize::sanitize_path_segment`）。旧 `replace(':')`
    // 对 B 端复合键 `{node}/{chat}` 会拆出嵌套目录（F-U4-4 同根因）；对
    // 无 `/` 的键新旧映射一致，存量平面文件零迁移（嵌套目录由
    // [`migrate_nested_session_logs`] 启动平化）。
    let safe_key = nemesis_utils::sanitize::sanitize_path_segment(session_key);
    default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", safe_key))
}

/// Does this session have a chat_log jsonl on disk? (2026-08-25 fork 第三轮)
/// `session_fork::unique_key` consults this so a fork never APPENDS onto a
/// previous fork's surviving jsonl after its store json aged out of the
/// 7-day TTL (append would duplicate the whole prefix).
pub fn chat_log_exists(session_key: &str) -> bool {
    log_path(session_key).exists()
}

/// Write pre-read chat_log rows under `new_key`, VERBATIM (2026-08-25 fork
/// 第三轮). Each row is a complete jsonl `Value` — original timestamps,
/// model badges, cron markers, everything preserved byte-faithfully. This is
/// the fork's jsonl side: the fork dialog counts turns ON these rows, so the
/// copy must be exactly the rows the user picked, not a re-derived
/// projection. Returns the number of lines written.
/// 测试钩子登记表（BUG-20261030-205 回归锁）：登记的 key 下次进入
/// [`write_chat_log_rows`] 时模拟 open 失败（消费一次即摘除）。仅测试
/// 编译；key 定向而非全局开关——fork 测试并行跑，全局开关会串扰。
#[cfg(test)]
static TEST_FAIL_OPEN_KEYS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// 武装一次 fork 复制 open 失败注入（见 [`TEST_FAIL_OPEN_KEYS`]）。
#[cfg(test)]
pub(crate) fn arm_test_fail_open(key: &str) {
    TEST_FAIL_OPEN_KEYS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(key.to_string());
}

pub fn write_chat_log_rows(new_key: &str, rows: &[Value]) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let target = log_path(new_key);
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // 测试钩子（BUG-20261030-205 回归锁）：定向模拟 open 失败（消费一次
    // 即摘除）。key 定向而非全局开关——fork 测试并行跑，全局开关会串扰
    // 其他用例的正常写入。
    #[cfg(test)]
    {
        let mut armed = TEST_FAIL_OPEN_KEYS
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(pos) = armed.iter().position(|k| k == new_key) {
            armed.swap_remove(pos);
            tracing::warn!(
                "[chat_log] (test hook) simulated fork copy open failure: {}",
                target.display()
            );
            return 0;
        }
    }
    let mut written = 0usize;
    let mut written_rows: Vec<&Value> = Vec::new();
    // BUG-20261030-205：fork 复制的落盘失败不再无痕——open/write 失败与
    // write_chat_entry 同款 warn + M2 降级登记（健康面可见）；fork 侧据
    // 返回值诚实 Err 并原子回退（session_fork::fork_session_opts）。
    let mut f = match OpenOptions::new().create(true).append(true).open(&target) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                "[chat_log] fork copy open failed {}: {}",
                target.display(),
                e
            );
            nemesis_utils::degraded::report(
                "chat_log",
                format!("fork copy open failed: {} ({e})", target.display()),
            );
            return 0;
        }
    };
    // R100（BUG-20261025-25）：整批先拼成一个缓冲再**单次** write_all
    // （同 write_chat_entry 的行原子性理由；批写还把 fork 复制从每行
    // 两次系统调用收敛成一次）。序列化失败的行跳过；write_all 失败 =
    // 整批未落盘（记账按 0 处理，不许多记）。
    let mut buf = String::new();
    for v in rows {
        let Ok(line) = serde_json::to_string(v) else {
            continue;
        };
        buf.push_str(&line);
        buf.push('\n');
        written += 1;
        written_rows.push(v);
    }
    if buf.is_empty() {
        // 全部行序列化失败——rows 来自 read_chat_log 的合法 JSON，实际
        // 不可达；防御臂保持与写失败同等的可观测性（BUG-20261030-205）。
        written = 0;
        written_rows.clear();
        tracing::warn!(
            "[chat_log] fork copy {}: 全部行序列化失败",
            target.display()
        );
        nemesis_utils::degraded::report(
            "chat_log",
            format!("fork copy rows serialize failed: {}", target.display()),
        );
    } else if let Err(e) = f.write_all(buf.as_bytes()) {
        written = 0;
        written_rows.clear();
        tracing::warn!(
            "[chat_log] fork copy write failed {}: {}",
            target.display(),
            e
        );
        nemesis_utils::degraded::report(
            "chat_log",
            format!("fork copy write failed: {} ({e})", target.display()),
        );
    }
    // T6（扩展计划 D5）：fork 记账（落新会话账本，sha-only——复制的行在
    // 源会话仍完整存在，正文不入账）。只记真正写出的行（序列化 + writeln
    // 双成功——账本是完整性证据，不许多记）。
    if written > 0 {
        crate::event_ledger::ledger_record(
            new_key,
            crate::event_ledger::LedgerOp::Fork,
            "chat_log",
            written_rows
                .iter()
                .map(|v| crate::event_ledger::LedgerRow::from_chat_row(v, false))
                .collect(),
        );
    }
    written
}

/// E3（devtool-upgrade 阶段 5）：把会话 jsonl **原位截断**为 `kept` 里
/// 的行（VERBATIM 逐字节保留）。消息级回退/重做（`AgentLoop::rewind_to_message`
/// / `redo_rewind`）的落盘原语。
///
/// 崩溃安全：先写 `.rewinding` 临时文件再 rename 覆盖原文件——中途崩溃
/// 要么原文件完好、要么截断后完好，不会出现半截文件。与 fork 的
/// [`write_chat_log_rows`] 同一宽容边界：不走 `history_search::index_append`
/// （原样重写不加新词），FTS 懒索引下次全量重建时自愈。
///
/// 返回写入行数。`kept` 为空 = 清空文件（保留文件本身，会话仍可用）。
pub fn truncate_chat_log_rows(session_key: &str, kept: &[Value]) -> usize {
    let path = log_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // T6（扩展计划 D5）：改写前先取原文件原始行——rename 成功后按
    // 「原行 vs kept 前缀」差集把**被删行全文**入账（永不丢已提交数据）。
    let raw_lines: Vec<String> = fs::File::open(&path)
        .map(|f| {
            std::io::BufReader::new(f)
                .lines()
                .map_while(Result::ok)
                .collect()
        })
        .unwrap_or_default();
    let kept_lines: Vec<String> = kept
        .iter()
        .filter_map(|v| serde_json::to_string(v).ok())
        .collect();
    let tmp = path.with_extension("jsonl.rewinding");
    let mut written = 0usize;
    {
        let mut f = match OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
        {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(
                    "[chat_log] Failed to open tmp {} for truncate: {}",
                    tmp.display(),
                    e
                );
                // BUG-20261030-205：手术面写失败进降级登记表（健康面可见），
                // 不再只有日志行。rewind 侧已按返回值诚实 commit_failed。
                nemesis_utils::degraded::report(
                    "chat_log",
                    format!("truncate tmp open failed: {} ({e})", tmp.display()),
                );
                return 0;
            }
        };
        for v in kept {
            let Ok(line) = serde_json::to_string(v) else {
                continue;
            };
            if writeln!(f, "{}", line).is_ok() {
                written += 1;
            }
        }
    }
    // Windows 上 std::fs::rename 走 MOVEFILE_REPLACE_EXISTING，可覆盖已存在
    // 目标；失败时保留原文件（会话不损）并清掉 tmp。
    if let Err(e) = fs::rename(&tmp, &path) {
        tracing::warn!(
            "[chat_log] truncate rename failed ({}): {} — 原文件保留",
            path.display(),
            e
        );
        nemesis_utils::degraded::report(
            "chat_log",
            format!("truncate rename failed: {} ({e})", path.display()),
        );
        let _ = nemesis_utils::fsutil::remove_file_retry(&tmp);
        return 0;
    }
    // T6：rename 生效后才记账（截断真正发生）。原行按序与 kept 序列化串
    // 前缀匹配；未匹配上的 = 被删行，全文（含解析不出 JSON 的裸行）入账。
    // 序列化两侧同走 serde_json::to_string(Value)（同 key 排序/转义），字节
    // 可比。差集记账是保守方向：匹配错位只会多记「被删」，不会漏记。
    ledger_truncate_diff(session_key, &raw_lines, &kept_lines);
    written
}

/// T6（扩展计划 D5）：把 raw_lines 相对 kept_lines 的差集（=被删行）全文
/// 入账（truncate op）。truncate_chat_log_rows 与 clear_chat_log 共用。
fn ledger_truncate_diff(session_key: &str, raw_lines: &[String], kept_lines: &[String]) {
    let mut deleted_rows = Vec::new();
    let mut ptr = 0usize;
    for line in raw_lines {
        if ptr < kept_lines.len() && line == &kept_lines[ptr] {
            ptr += 1;
            continue;
        }
        let role = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|v| v.get("role").and_then(|r| r.as_str()).map(String::from))
            .unwrap_or_default();
        deleted_rows.push(crate::event_ledger::LedgerRow {
            role,
            content_sha256: crate::event_ledger::sha256_hex(line.as_bytes()),
            content: Some(line.clone()),
        });
    }
    if !deleted_rows.is_empty() {
        crate::event_ledger::ledger_record(
            session_key,
            crate::event_ledger::LedgerOp::Truncate,
            "chat_log",
            deleted_rows,
        );
    }
}

/// Z1 (Phase4-d): copy the first `at_turn` COMPLETE user turns of
/// `source_key`'s chat log to `new_key`, lines VERBATIM (original
/// timestamps and extra fields preserved — a fork must not re-stamp
/// history). Uses the same user-turn counting as the SessionStore-side
/// fork cut, so the Dashboard log and the model-context store stay aligned.
/// Returns the number of lines copied. The lazy FTS full-index picks the
/// new file up on first search; no per-line index_append needed here.
///
/// ⚠ SUPERSEDED TWICE — do not re-enable:
/// - Round 2 (2026-08-25 上午, fork 内容错位第一修): disabled in favor of
///   `write_chat_log_from_store` on the then-belief that SessionStore was
///   the single source of truth for turn semantics (see that note below).
/// - Round 3 (2026-08-25 深夜): the self-heal fix made jsonl the single
///   source of truth (store = rebuildable cache that compaction folds and
///   TTL deletes); the round-2 assumption was backwards, and forking off
///   the store produced garbage on real production sessions (store held a
///   truncated, tool-intermediate-polluted history while the user picked a
///   turn by the clean jsonl the UI renders). `fork_session` now reads the
///   rows itself and writes them via `write_chat_log_rows`.
///   Kept (not deleted) per the code-change discipline.
#[allow(dead_code)]
pub fn copy_chat_log_prefix(source_key: &str, new_key: &str, at_turn: usize) -> usize {
    // Whole-log read: fork is a one-shot admin op, not a hot path.
    let (all, _total, _more, _oldest) = read_chat_log(source_key, usize::MAX, None);
    let mut turns = 0usize;
    let mut lines: Vec<String> = Vec::new();
    for v in all {
        if v.get("role").and_then(|r| r.as_str()) == Some("user") {
            turns += 1;
            if turns > at_turn {
                break;
            }
        }
        if let Ok(line) = serde_json::to_string(&v) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return 0;
    }
    let target = log_path(new_key);
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&target) {
        for l in &lines {
            let _ = writeln!(f, "{}", l);
        }
    }
    lines.len()
}

/// FIX (2026-08-25): single source of truth for "which rows become visible
/// chat rows" — used by the self-heal rebuild / fork store mapping
/// (`session::projected_messages_from_rows`) AND the turns endpoint's
/// `end_preview` computation (api_handlers). The call sites must never
/// drift apart, or the fork dialog's "分叉末条" preview would disagree with
/// what the fork actually ends on — the exact class of bug this round
/// fixed. Tool/system rows never project (chat_log is the UI bubble
/// source); an `assistant` row with empty/whitespace content is a pure
/// tool_calls intermediate, not a displayable reply.
pub fn is_projected_chat_row(role: &str, content: &str) -> bool {
    match role {
        "user" => true,
        "assistant" => !content.trim().is_empty(),
        _ => false,
    }
}

/// 轮边界判定单一真相源（BUG-20261025-19）：jsonl 行是否开启一个完整
/// user 轮。`role == "user"` 且非 steer 注入行——steer 是轮中间注入的
/// user 形态行（`"steer": true` 标记，run_loop 注入路径写入），不是轮首；
/// 把它当轮边界会把 rewind/fork 的截断点切进轮中间（「撤销最后一轮」只
/// 删掉 steer 尾巴、轮的提问与首轮回复残留；fork「分叉到第 N 轮」切出
/// 半轮）。消费方：rewind 三入口的截断点计算 + `session_fork` 的轮计数
/// 与切点。缺 `steer` 字段的行 = 旧形态（真正的轮首与 system/maintenance
/// 等无标记行）——一律按轮边界处理，与旧行为逐字节一致（修复前写入的
/// steer 行无标记，按旧行为退化，与 checkpoint_turn 的旧行纪律同款）。
pub fn is_user_turn_boundary(row: &Value) -> bool {
    row.get("role").and_then(|v| v.as_str()) == Some("user")
        && row.get("steer").and_then(|v| v.as_bool()) != Some(true)
}

/// FIX (2026-08-25 分叉内容错位, round 2): generate `new_key`'s chat_log by
/// PROJECTING a SessionStore history prefix — the replacement for
/// `copy_chat_log_prefix` (see its ⚠ SUPERSEDED note for the divergence
/// bug this fixes).
///
/// ⚠ SUPERSEDED (2026-08-25 round 3, fork 第三轮): this function's premise —
/// "SessionStore is the single source of truth for turn semantics" — was
/// inverted by the self-heal fix later the same day: jsonl is the truth,
/// the store is a lossy, compaction-folded, TTL-deleted cache. Projecting
/// the fork's chat_log FROM the store made the fork inherit every store
/// defect (truncated history, tool-intermediate pollution, folded turns),
/// producing garbage forks on real production sessions. `fork_session` now
/// copies the jsonl rows verbatim (`write_chat_log_rows`) and derives the
/// store FROM those rows (`session::projected_messages_from_rows`) — the
/// store→jsonl direction is dead. Kept (not deleted) per the code-change
/// discipline.
///
/// Projection semantics (historical, for when this was live):
/// - only `user` / `assistant` rows are written (chat_log is the UI bubble
///   source; tool/system rows were never logged there);
/// - `assistant` rows with empty/whitespace content are skipped — those are
///   pure tool_calls intermediate messages; the final per-turn reply has
///   content. (A non-empty intermediate reply is real model output and is
///   kept — honest content beats byte-parity with a source log that may
///   itself be stale.)
/// - timestamps come from the stored messages (a fork must not re-stamp
///   history);
/// - model badge / cron markers are NOT carried over (the store does not
///   record them; a missing badge degrades to "no badge" on the read side,
///   which parses fine — an acceptable display-only cost for guaranteed
///   store↔log alignment).
///
/// By construction the new session's two stores agree: the Dashboard's
/// last displayed message == the last user/assistant message of
/// `messages`. The lazy FTS full-index picks the new file up on first
/// search; no per-line index_append needed here (same as the old copy).
/// Returns the number of lines written.
#[allow(dead_code)]
pub fn write_chat_log_from_store(
    new_key: &str,
    messages: &[crate::session::StoredMessage],
) -> usize {
    let mut lines: Vec<String> = Vec::new();
    for m in messages {
        if !is_projected_chat_row(&m.role, &m.content) {
            continue;
        }
        let entry = serde_json::json!({
            "role": m.role,
            "content": m.content,
            "timestamp": m.timestamp,
        });
        if let Ok(line) = serde_json::to_string(&entry) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return 0;
    }
    let target = log_path(new_key);
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut written = 0usize;
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&target) {
        for l in &lines {
            if writeln!(f, "{}", l).is_ok() {
                written += 1;
            }
        }
    }
    written
}

/// Delete a session's chat log file (JSONL). Used by session management
/// (delete conversation) to clear the user-facing history. Also deletes the
/// boundary-events sidecar (so a re-created session doesn't inherit stale
/// audit rows) and the title meta sidecar (2026-08-25: it used to survive as
/// an orphan — invisible to `sessions.list` which scans jsonl only, but dead
/// bytes on disk; `clear` deliberately KEEPS the meta because the
/// conversation stays alive with its title). No-op if absent.
pub fn delete_chat_log(session_key: &str) {
    // BUG-20261025-35 同族：会话删除的落盘面必须真删——裸单发 remove 被
    // Windows 瞬时句柄（Defender/索引器）打败时 jsonl 残留，`sessions.list`
    // 扫 jsonl 会把「已删除」的会话复活。走 fsutil 韧性重试（NotFound 在
    // 重试原语内部已归一为成功），耗尽才 warn 留痕。
    let path = log_path(session_key);
    if let Err(e) = nemesis_utils::fsutil::remove_file_retry(&path) {
        tracing::warn!("[chat_log] Failed to delete {}: {}", path.display(), e);
    }
    let bpath = boundary_path(session_key);
    if let Err(e) = nemesis_utils::fsutil::remove_file_retry(&bpath) {
        tracing::warn!("[chat_log] Failed to delete {}: {}", bpath.display(), e);
    }
    let mpath = meta_path(session_key);
    if let Err(e) = nemesis_utils::fsutil::remove_file_retry(&mpath) {
        tracing::warn!("[chat_log] Failed to delete {}: {}", mpath.display(), e);
    }
}

/// Clear (truncate) a session's chat log, keeping the file. Used by session
/// management "clear" — empties history but the session id stays usable.
/// Also truncates the boundary-events sidecar (same lifecycle).
///
/// BUG-20261030-205：返回 `Result`——主日志写失败不再静默成功。旧形态
/// warn-only，两个消费方（WSAPI `sessions.clear` / `/clear` slash）都无
/// 条件报「已清空」，而 jsonl 残留 + store 先清 = 下次 self-heal 把「已清」
/// 内容复活（正是 jsonl-first 纪律注释里描述要防的形态）。boundary 侧车
/// 写失败仍是 warn 降级（主日志成功 = 操作成立，账本差集已入账）。
pub fn clear_chat_log(session_key: &str) -> Result<(), String> {
    let path = log_path(session_key);
    // T6（扩展计划 D5）：清空即全删——清空前取原始行，成功后全额按
    // truncate 语义入账（被清内容全文可从账本找回，与 rewind 同一保障）。
    let raw_lines: Vec<String> = fs::File::open(&path)
        .map(|f| {
            std::io::BufReader::new(f)
                .lines()
                .map_while(Result::ok)
                .collect()
        })
        .unwrap_or_default();
    if let Err(e) = fs::write(&path, "") {
        tracing::warn!("[chat_log] Failed to clear {}: {}", path.display(), e);
        nemesis_utils::degraded::report(
            "chat_log",
            format!("clear write failed: {} ({e})", path.display()),
        );
        return Err(format!("清空会话日志失败 ({}): {e}", path.display()));
    }
    ledger_truncate_diff(session_key, &raw_lines, &[]);
    let bpath = boundary_path(session_key);
    if bpath.exists()
        && let Err(e) = fs::write(&bpath, "")
    {
        tracing::warn!("[chat_log] Failed to clear {}: {}", bpath.display(), e);
        nemesis_utils::degraded::report(
            "chat_log",
            format!("clear boundary write failed: {} ({e})", bpath.display()),
        );
    }
    Ok(())
}

/// Path for the sidecar title meta file (`{safe_key}.meta.json`, next to the
/// `.jsonl`). Stores a user-editable conversation title for multi-session
/// management without touching the lazy-created SessionStore.
pub(crate) fn meta_path(session_key: &str) -> PathBuf {
    let safe_key = nemesis_utils::sanitize::sanitize_path_segment(session_key);
    default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.meta.json", safe_key))
}

/// Write the conversation title to the sidecar meta file.
///
/// E4 (2026-09-05): read-modify-write UPSERT — preserves `parent` /
/// `forked_at_turn` already recorded by `write_session_parent` (fork writes
/// lineage first, then the fork endpoint writes a title; a blind overwrite
/// would erase the lineage).
pub fn write_session_meta(session_key: &str, title: &str) {
    upsert_meta(session_key, |m| m.title = Some(title.to_string()));
}

/// E7: 手动标题写入（rename / 建会话时用户显式命名）——置 `title_manual`
/// 标记，自动标题（`write_session_meta_auto_title`）永不覆盖。
pub fn write_session_meta_manual(session_key: &str, title: &str) {
    upsert_meta(session_key, |m| {
        m.title = Some(title.to_string());
        m.title_manual = true;
    });
}

/// P6-R4（D4 预留）：会话 → principal 首绑（`process_admitted` 会话首建
/// 时一次写入；只写不读，空串 = 无主体不落盘）。RMW 保留既有 lineage/
/// title 等字段（upsert_meta 同源语义）。
pub fn write_session_meta_principal(session_key: &str, principal: &str) {
    if principal.trim().is_empty() {
        return;
    }
    upsert_meta(session_key, |m| m.principal = Some(principal.to_string()));
}

/// E7: 会话侧栏的默认占位标题（sessions create 未显式命名时写入）。
/// 自动标题允许覆盖它（占位符不是用户意志）。
pub const DEFAULT_SESSION_TITLE: &str = "新对话";

/// E7: 自动标题写入。只填充「无标题 / 仅默认占位符」的会话；手动改名或
/// 建会话时显式命名的（`title_manual=true`）与已有真实标题的一律不动。
/// 返回是否实际写入。
pub fn write_session_meta_auto_title(session_key: &str, title: &str) -> bool {
    if !auto_title_eligible(session_key) {
        return false;
    }
    upsert_meta(session_key, |m| m.title = Some(title.to_string()));
    true
}

/// E7: 自动标题资格判定——无 meta / 无标题 / 仅默认占位符，且未被手动
/// 改名。生成端（loop 的标题任务）与写入端共用，双端一致。
pub fn auto_title_eligible(session_key: &str) -> bool {
    match read_meta_full(session_key) {
        None => true,
        Some(m) => {
            !m.title_manual
                && m.title
                    .as_deref()
                    .is_none_or(|t| t == DEFAULT_SESSION_TITLE)
        }
    }
}

/// E7: 本会话 chat log 中首条非空 user 消息（截到 `max_chars` 字符，
/// char 边界安全；自动标题的输入）。
pub fn first_user_message(session_key: &str, max_chars: usize) -> Option<String> {
    let data = fs::read_to_string(log_path(session_key)).ok()?;
    for line in data.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
        let trimmed = content.trim();
        if trimmed.is_empty() {
            continue;
        }
        return Some(trimmed.chars().take(max_chars).collect());
    }
    None
}

/// E4: record fork lineage (`parent` = source session key, `forked_at_turn`
/// = the kept-turn count) in the sidecar meta. Upsert — preserves `title`.
/// 重新上血缘同时解除 BUG-20261030-197 的 detach 封印位（「clear 是摘除
/// 不是封禁」的镜像面：re-fork 后新摘要可照常落盘）。
pub fn write_session_parent(session_key: &str, parent: &str, forked_at_turn: usize) {
    upsert_meta(session_key, |m| {
        m.parent = Some(parent.to_string());
        m.forked_at_turn = Some(forked_at_turn);
        m.lineage_detached = false;
    });
}

/// 分支摘要落盘的硬上限（chars，非 bytes；`write_session_branch_summary`
/// 截断用）。WS9/P18 生成侧同用此常量，双保险单一真相源。
pub const BRANCH_SUMMARY_MAX_CHARS: usize = 4000;

/// WS9/P17：记录分叉缘由（fork_session 侧「为什么分出去」的一段话）。
/// Upsert——已有 title/血缘/项目归属等字段全部保留；空串不写（无意义的
/// 缘由不落盘，保持 meta 精简）。
pub fn write_session_fork_reason(session_key: &str, reason: &str) {
    let reason = reason.trim();
    if reason.is_empty() {
        return;
    }
    upsert_meta(session_key, |m| {
        m.fork_reason = Some(reason.to_string());
    });
}

/// WS9/P17：记录一次消息级回退（rewind 也是谱系事件——本会话在此丢掉了
/// 哪些行/轮）。Upsert；只保留**最近一次**（`last_rewind` 语义，完整历史
/// 在 undo 栈/审计里，不重复记账）。
pub fn write_session_rewind(
    session_key: &str,
    at_index: usize,
    dropped_rows: usize,
    dropped_turns: usize,
) {
    upsert_meta(session_key, |m| {
        m.last_rewind = Some(RewindLineage {
            at_index,
            dropped_rows,
            dropped_turns,
            ts: Local::now().to_rfc3339(),
        });
    });
}

/// WS9/P18：把分支摘要落盘（fork/rewind 丢弃后缀 ≥3 turn 时由小模型通道
/// 生成；这里 trim + 空跳过 + 硬上限截断兜底——生成侧已按预算裁剪，此处
/// 防御性二次钳制，两边共用 [`BRANCH_SUMMARY_MAX_CHARS`]）。
/// BUG-20261030-197：detach 封印位在册时拒绝落盘——摘要 spawn 于
/// detach 之前、LLM 往返数秒后才到这里，无条件 upsert 会把
/// `clear_session_lineage` 刚摘除的 branch_summary 复活（Branch Context
/// 恢复注入，detach 契约违约）。检查在 upsert 闭包内（META_LOCK 下
/// check-then-write 原子）。
pub fn write_session_branch_summary(session_key: &str, summary: &str) {
    let summary = summary.trim();
    if summary.is_empty() {
        return;
    }
    let clipped: String = summary.chars().take(BRANCH_SUMMARY_MAX_CHARS).collect();
    upsert_meta(session_key, |m| {
        if m.lineage_detached {
            return;
        }
        m.branch_summary = Some(clipped);
    });
}

/// meta 文件原子落盘单一出口（2026-10-24 R1）：序列化 → 同目录 temp 唯一
/// 后缀 → rename 替换（Windows `MoveFileEx(REPLACE_EXISTING)` 语义）。此前
/// 六处 meta 写全部直写 `fs::write`：进程中途死 = 半截 JSON（读取侧按
/// corrupt 起空 meta 静默吃掉），写失败 = 吞错。调用方决定 warn 还是上浮。
fn write_meta_file_atomic(path: &std::path::Path, meta: &SessionMeta) -> Result<(), String> {
    let data = serde_json::to_string(meta).map_err(|e| format!("serialize: {e}"))?;
    let tmp = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default()
    ));
    fs::write(&tmp, data).map_err(|e| format!("tmp write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("replace {}: {e}", path.display())
    })?;
    Ok(())
}

/// Shared read-modify-write for the sidecar meta (single fs read + write).
///
/// 进程级互斥（2026-09-26 复查 F3）：read-modify-write 期间持锁——并发
/// upsert（如 outbound 路径 `mark_undelivered_reply` × 标题写入）各自读到
/// 同一基线会互相覆盖丢更新。临界区 = 单个小文件读写，阻塞锁可接受；
/// 跨进程一致性不在此担保（sidecar 是单 gateway 进程私有）。
static META_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Result 变体（R1）：持久化失败上浮给关心持久语义的调用方
/// （`write_session_allow_all`——撤销放行写失败必须让 handler 诚实响应）。
fn upsert_meta_result(session_key: &str, f: impl FnOnce(&mut SessionMeta)) -> Result<(), String> {
    let _guard = META_LOCK.lock();
    let path = meta_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Corrupt / unreadable existing file → start from an empty meta (same
    // warn-and-continue posture as before; nothing readable is lost).
    let mut meta = read_meta_full(session_key).unwrap_or_default();
    f(&mut meta);
    write_meta_file_atomic(&path, &meta)
}

fn upsert_meta(session_key: &str, f: impl FnOnce(&mut SessionMeta)) {
    if let Err(e) = upsert_meta_result(session_key, f) {
        tracing::warn!("[chat_log] failed to write meta: {e}");
    }
}

/// L6++（2026-09-08）：项目归属烧入（创建项目会话时一次写入；归属不可变
/// ——无改移 API，改归属 = 新会话，行业共识）。Upsert 语义同
/// write_session_parent（已写过的 title 等字段保留）。
pub fn write_session_project(session_key: &str, project_id: &str, project_path: &str) {
    upsert_meta(session_key, |m| {
        m.project_id = Some(project_id.to_string());
        m.project_path = Some(project_path.to_string());
    });
}

/// L6++（2026-09-08，M4 sessions.delete/clear 的 forget 联动）：摘除项目
/// 归属两字段（title/血缘/manual 标记全部保留）。语义：
/// - 文件缺失 = no-op（delete 路径 meta 已随 delete_chat_log 一并删除）；
/// - 本就无归属 = no-op（不空写文件）；
/// - 有归属 = 读改写摘除（防 owner_of 的 sidecar 兜底把绑定「复活」）。
///
/// 返回是否实际摘除了归属。
pub fn clear_session_project(session_key: &str) -> bool {
    // 与 upsert_meta 同锁（check-then-write 原子性：并发写入不得插在
    // 判定与落盘之间）。
    let _guard = META_LOCK.lock();
    let Some(mut meta) = read_meta_full(session_key) else {
        return false;
    };
    if meta.project_id.is_none() && meta.project_path.is_none() {
        return false;
    }
    meta.project_id = None;
    meta.project_path = None;
    let path = meta_path(session_key);
    if let Err(e) = write_meta_file_atomic(&path, &meta) {
        tracing::warn!(
            "[chat_log] failed to clear session project {}: {}",
            path.display(),
            e
        );
        return false;
    }
    true
}

/// 会话「脱离父分支」（2026-10-20）：摘除 fork 血缘四字段（parent /
/// forked_at_turn / fork_reason / branch_summary），其余 meta（title /
/// project 归属 / manual 标记 / last_rewind / snooze / undelivered）全部
/// 保留，并置 `lineage_detached` 封印位（BUG-20261030-197：挡住 spawn 于
/// detach 之前的 in-flight 分支摘要迟到复活；write_session_parent 解除）。
/// 语义：
/// - meta 缺失 / 本就无血缘 = no-op 返 false（调用方诚实报「没有父分支」）；
/// - 有血缘 = 读改写摘除（META_LOCK 下 check-then-write 原子，同
///   clear_session_project）；
/// - 消息内容零改动——fork 时新会话 jsonl 本就是整段 VERBATIM 拷贝，脱离
///   只断 meta 派生视图（谱系树 / 祖先链 / Branch Context 注入 gate 只看
///   branch_summary，清空后下一轮自然停止）。
///
/// 返回是否实际摘除了血缘。
pub fn clear_session_lineage(session_key: &str) -> bool {
    let _guard = META_LOCK.lock();
    let Some(mut meta) = read_meta_full(session_key) else {
        return false;
    };
    if meta.parent.is_none()
        && meta.forked_at_turn.is_none()
        && meta.fork_reason.is_none()
        && meta.branch_summary.is_none()
    {
        return false;
    }
    meta.parent = None;
    meta.forked_at_turn = None;
    meta.fork_reason = None;
    meta.branch_summary = None;
    // BUG-20261030-197：封印位——spawn 于 detach 之前的分支摘要还在 LLM
    // 途中，落盘时若不挡，branch_summary 会被复活、Branch Context 恢复
    // 注入（detach 契约违约）。re-fork（write_session_parent）解除。
    meta.lineage_detached = true;
    let path = meta_path(session_key);
    if let Err(e) = write_meta_file_atomic(&path, &meta) {
        tracing::warn!(
            "[chat_log] failed to clear session lineage {}: {}",
            path.display(),
            e
        );
        return false;
    }
    true
}

/// Read the conversation title from the sidecar meta file, if present.
pub fn read_session_meta(session_key: &str) -> Option<String> {
    read_meta_full(session_key).and_then(|m| m.title)
}

/// P2（2026-09-11 真机日志）：assistant 回复实时推送失败时 +1——会话没有
/// 活动 WS 连接（如恢复回复回到已断开的 ws-send 驱动端）时内容仍照常落盘
/// （chat_log 是真相源），本标记只承载「有未读」这一通知事实。
pub fn mark_undelivered_reply(session_key: &str) {
    upsert_meta(session_key, |m| {
        m.undelivered = m.undelivered.saturating_add(1);
    });
}

/// P2：前端拉取会话历史后清零未读标记。返回是否确实清掉了非零计数。
pub fn clear_undelivered_replies(session_key: &str) -> bool {
    // 与 upsert_meta 同锁（同 clear_session_project 的原子性理由）。
    let _guard = META_LOCK.lock();
    let Some(mut meta) = read_meta_full(session_key) else {
        return false;
    };
    if meta.undelivered == 0 {
        return false;
    }
    meta.undelivered = 0;
    let path = meta_path(session_key);
    if let Err(e) = write_meta_file_atomic(&path, &meta) {
        tracing::warn!(
            "[chat_log] failed to clear undelivered marker {}: {}",
            path.display(),
            e
        );
        return false;
    }
    true
}

/// B11（2026-10-18 snooze）：把会话暂停到 `until_rfc3339`（RFC3339，含时区）。
/// 幂等 upsert（重复 snooze = 改期，旧 note 一并覆盖）。
pub fn write_session_snooze(session_key: &str, until_rfc3339: &str, note: Option<&str>) {
    upsert_meta(session_key, |m| {
        m.snoozed_until = Some(until_rfc3339.to_string());
        m.snooze_note = note.map(|n| n.to_string());
    });
}

/// B11：摘除暂停标记（与 upsert_meta 同锁——check-then-write 原子性）。
/// 返回被摘除的 `(until, note)`；无标记/无 meta = `None`（不空写文件）；
/// 落盘失败也返回 `None`（BUG-20261028-122：写失败 = 标记仍在盘上，谎报
/// 「已摘除」会让 sweeper append 恢复行后下一轮 sweep 再来一遍——磁盘故障
/// 持续期间每 10s 刷一条重复恢复行。返 None 让 sweeper 跳过下轮重试、
/// unsnooze 臂回 unsnoozed:false 诚实）。
pub fn clear_session_snooze(session_key: &str) -> Option<(String, Option<String>)> {
    let _guard = META_LOCK.lock();
    let mut meta = read_meta_full(session_key)?;
    let until = meta.snoozed_until.take()?;
    let note = meta.snooze_note.take();
    let path = meta_path(session_key);
    if let Err(e) = write_meta_file_atomic(&path, &meta) {
        tracing::warn!(
            "[chat_log] failed to clear snooze {}: {}",
            path.display(),
            e
        );
        return None;
    }
    Some((until, note))
}

/// B11：扫描全部暂停中的会话，返回 `(safe_stem, until, note)`。`safe_stem`
/// 是 sanitize 后的 meta 文件名去 `.meta.json` 后缀（调用方按
/// `agent_main_session_{sid}` 前缀还原裸 sid，logs.rs 同款规则）。输出按
/// stem 排序——消费与条目顺序无关（每条独立处理），排序只为测试确定性。
pub fn snoozed_sessions() -> Vec<(String, String, Option<String>)> {
    let dir = default_path_manager().sessions_log_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".meta.json") else {
            continue;
        };
        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
        // BUG-20261025-44：损坏 meta 留痕（静默跳过 = 暂停中的会话永不唤醒
        // 且无迹可查）；跳过语义不变。同族口径见 read_meta_full。
        let meta = match serde_json::from_str::<SessionMeta>(&data) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[chat_log] Corrupt meta file {}: {}", path.display(), e);
                continue;
            }
        };
        if let Some(until) = meta.snoozed_until {
            out.push((stem.to_string(), until, meta.snooze_note));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------------------------------------------------------------------------
// 会话级全部放行（2026-10-24 长任务无人值守计划）：sidecar 持久位
// ---------------------------------------------------------------------------

/// 写会话级全部放行的 sidecar 持久位（WSAPI set 的持久半边；运行态内存半边
/// 在 `nemesis_security::SessionAllowAllState`，由 handler 双写）。
///
/// - enabled=true：记 `allow_all=true` + `allow_all_since=since` +
///   `session_key`（真实键随行烧入——hydrate 靠它，文件名 stem 是 sanitize
///   有损形态）。`since` 由调用方传入（内存态 `set()` 的返回值——单一真相，
///   两半边不各打各的时间戳）；空串回落现在（防御性，正常调用不触发）。
/// - enabled=false：摘除 `allow_all`/`allow_all_since`（关闭即回默认，不留
///   陈旧真值；`session_key` 字段保留无害）。无 meta / 本就没开 = no-op
///   返 `Ok`（同 clear_session_snooze 姿态）。
///
/// 返回 `Err` = 持久化失败（2026-10-24 R1）：磁盘半边与本意不一致——调用方
/// （WSAPI handler）必须诚实响应（`persisted:false` + warning），不得静默
/// 当成功。写路径走 [`write_meta_file_atomic`] 原子替换，不再直写。
pub fn write_session_allow_all(
    session_key: &str,
    enabled: bool,
    since: &str,
) -> Result<(), String> {
    if enabled {
        let since = if since.is_empty() {
            Local::now().to_rfc3339()
        } else {
            since.to_string()
        };
        return upsert_meta_result(session_key, |m| {
            m.allow_all = true;
            m.allow_all_since = Some(since);
            m.session_key = Some(session_key.to_string());
        });
    }
    // 关 = 摘除（与 upsert_meta 同锁——check-then-write 原子性）。
    let _guard = META_LOCK.lock();
    let Some(mut meta) = read_meta_full(session_key) else {
        return Ok(());
    };
    if !meta.allow_all {
        return Ok(());
    }
    meta.allow_all = false;
    meta.allow_all_since = None;
    write_meta_file_atomic(&meta_path(session_key), &meta)
}

/// 读会话级全部放行持久位 `(enabled, since)`；无 meta / 未记 = `None`。
pub fn read_session_allow_all(session_key: &str) -> Option<(bool, String)> {
    let meta = read_meta_full(session_key)?;
    Some((meta.allow_all, meta.allow_all_since.unwrap_or_default()))
}

/// hydrate 扫描：全部 meta 中 `allow_all=true` 的 `(session_key, since)`
/// 列表（真实会话键来自 meta 内烧入的 `session_key` 字段；缺字段/空键的
/// 条目防御性跳过——旧形态不可能有 allow_all，此臂只兜手工改盘）。输出按
/// 会话键排序（read_dir 顺序是平台实现细节，消费与顺序无关，排序只为
/// 测试确定性）。
pub fn allow_all_sessions() -> Vec<(String, String)> {
    let dir = default_path_manager().sessions_log_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .ends_with(".meta.json")
        {
            continue;
        }
        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };
        // BUG-20261025-44：损坏 meta 留痕（静默跳过 = 放行状态 hydrate 静默
        // 失效且无迹可查——fail-safe 但不可观测）；跳过语义不变。同族口径
        // 见 read_meta_full。
        let meta = match serde_json::from_str::<SessionMeta>(&data) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[chat_log] Corrupt meta file {}: {}", path.display(), e);
                continue;
            }
        };
        if meta.allow_all
            && let Some(key) = meta.session_key
            && !key.is_empty()
        {
            out.push((key, meta.allow_all_since.unwrap_or_default()));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Read the full sidecar meta by session key. `None` when absent/unparsable.
fn read_meta_full(session_key: &str) -> Option<SessionMeta> {
    let path = meta_path(session_key);
    let data = fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<SessionMeta>(&data) {
        Ok(m) => Some(m),
        Err(e) => {
            // BUG-20261025-44：损坏 meta 不再静默当 None——upsert 路径会以
            // 空基线重写、静默抹掉 title/血缘/项目归属/allow_all 全部字段，
            // 损坏必须留痕（与 auth.json / 分享注册表 / checkpoint 镜像同族
            // 口径；上方 upsert_meta_result 注释声称的 "warn-and-continue
            // posture" 由此才真正成立）。跳过语义不变。
            tracing::warn!("[chat_log] Corrupt meta file {}: {}", path.display(), e);
            None
        }
    }
}

/// E4: read the full sidecar meta (title + fork lineage). `None` when the
/// file is absent or unparsable. Legacy files (`{"title": ...}` only)
/// deserialize with lineage fields `None` (compatible read).
pub fn read_session_meta_full(session_key: &str) -> Option<SessionMeta> {
    read_meta_full(session_key)
}

/// E4: full sidecar meta (`{safe_key}.meta.json`). All fields optional —
/// pre-E4 files carry only `title`; serde defaults keep them compatible.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SessionMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Source session key this session was forked from (fork_session writes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Turn boundary the fork was taken at (user-turn count kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_at_turn: Option<usize>,
    /// E7: 用户手动命名过（rename / 建会话显式标题）——自动标题永不覆盖。
    #[serde(default, skip_serializing_if = "is_false")]
    pub title_manual: bool,
    /// L6++：项目归属（创建项目会话时烧入，不可变；None = 对话组）。
    /// 真相源 = 本 sidecar；`StoredSession` 的同名字段只是缓存镜像。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// L6++：canonical 项目目录绝对路径（与 project_id 同批写入）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    /// P2（2026-09-11 真机日志）：实时推送失败（会话无活动连接，如崩溃恢复
    /// 回复回到已断开的驱动端）的 assistant 消息条数。前端拉取历史后经
    /// `sessions.mark_delivered` 清零。0 不落盘，兼容旧文件形态。
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub undelivered: u32,
    /// WS9/P17：分叉缘由（fork_session 时的一段说明文字；None = 旧分叉无
    /// 记录，谱系视图回退显示「fork」）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_reason: Option<String>,
    /// WS9/P17：最近一次消息级回退的谱系记录（回退也是谱系事件；只记最近
    /// 一次）。None = 本会话从未回退过。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rewind: Option<RewindLineage>,
    /// WS9/P18：分支摘要（fork/rewind 丢弃后缀 ≥3 turn 时由小模型生成的
    /// 六节结构化前情提要；新会话首轮 build_messages 注入）。None = 无摘要
    /// （未达阈值 / 小模型未配置——诚实跳过）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_summary: Option<String>,
    /// BUG-20261030-197：本会话已「脱离父分支」（sessions.detach →
    /// clear_session_lineage）。true = 迟到的分支摘要后台写不再落盘——
    /// detach 契约「Branch Context 停止注入」不被 spawn 于 detach 之前的
    /// in-flight 摘要（LLM 往返数秒后落盘）复活击穿；re-fork
    /// （write_session_parent 重新上血缘）解除封印。
    #[serde(default, skip_serializing_if = "is_false")]
    pub lineage_detached: bool,
    /// B11（2026-10-18 snooze）：会话暂停到点（RFC3339，含时区）。None =
    /// 未暂停。到期由 gateway 的 snooze sweeper 摘除标记并注入恢复 system
    /// 行；期间消息照常处理（snooze 只是 UI 标记 + 到期唤醒语义，不拦截
    /// 入站消息）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<String>,
    /// B11：暂停备注（用户可选说明，唤醒注入行尾追加；None = 无）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze_note: Option<String>,
    /// P6-R4（D4 预留）：会话首绑 principal（`{channel}/{sender_id}`，映射
    /// 单一真相源 `nemesis_types::channel::principal_for_channel`）。只写
    /// 不读——多租户闸不做，消费方出现前任何代码不得据它分叉行为。
    /// None = 未绑定（cron/heartbeat/system 等无主体来源，或旧文件）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    /// 会话级全部放行（2026-10-24 长任务无人值守计划）：true = 本会话的
    /// 安全审批卡全自动放行（含 CRITICAL 与 ziwei force 卡；硬 deny / estop
    /// 不受影响）。持久真相源 = 本字段；gateway 的 SessionAllowAllState 是
    /// 运行期镜像（启动 hydrate 回载）。无 TTL 无自动关闭（用户裁决）。
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_all: bool,
    /// 放行开启时刻（RFC3339 本地时区；前端「自 HH:MM 放行中」+ hydrate 种
    /// SessionAllowAllState 的 since）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_all_since: Option<String>,
    /// 真实会话键（写 allow_all 时随行烧入——文件名 stem 是 sanitize 后的
    /// 有损形态还原不了，hydrate 直接读本字段）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
}

/// WS9/P17：一次消息级回退的谱系记录（`last_rewind` 的载荷）。
/// `dropped_rows == 0`（末尾 no-op 回退）不落盘——回退未发生时没有谱系
/// 事实可记（rewind 侧 no-op 路径本就不调用写入）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct RewindLineage {
    /// 回退锚定的行 index（`rewind_to_message` 的入参）。
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub at_index: usize,
    /// 被截掉的 jsonl 行数。
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub dropped_rows: usize,
    /// 被截掉的完整 user 轮数（rows → turn 换算后）。
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub dropped_turns: usize,
    /// 回退发生时间（RFC3339 本地时区，与 chat_log 行时间戳同形）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ts: String,
}

/// WS9/P17：`skip_serializing_if` 助手（0 不落盘，兼容旧文件形态）。
fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

/// P2: `skip_serializing_if` 助手（0 不落盘）。
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// E7: `skip_serializing_if` 助手（false 不落盘，兼容旧文件形态）。
fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// I3 (U9): boundary events (round-5 fix: sidecar file)
// ---------------------------------------------------------------------------

/// Resolve the boundary-events SIDECAR path (`logs/boundary/<safe_key>.jsonl`).
///
/// Round-5 review fix: boundary events used to be interleaved into the
/// message jsonl — which skewed `read_chat_log` pagination counts (total
/// counted boundary lines, pages underfilled or came back empty with
/// has_more=true), made every NEW session's first line a `turn_start` row
/// (blank title/preview in the Dashboard session list, which reads
/// `lines[0]["content"]`), and rendered empty bubbles in raw readers
/// (`logs.rs` session_detail / scan_session_logs bypass read_chat_log).
/// A separate file fixes all of them at the storage layer. Deliberately NOT
/// inside `session_logs/`: that dir is scanned for `*.jsonl` as sessions —
/// a sidecar there would appear as a phantom session.
pub(crate) fn boundary_path(session_key: &str) -> PathBuf {
    let safe_key = nemesis_utils::sanitize::sanitize_path_segment(session_key);
    default_path_manager()
        .boundary_events_dir()
        .join(format!("{}.jsonl", safe_key))
}

/// Append a turn/step boundary event to the session's boundary sidecar.
/// Lightweight durable markers for replay/audit: `turn_start` / `turn_end`
/// (with a reason) / `llm_request` (model + token estimate) /
/// `steer_injected`. Never contains message bodies.
pub fn append_boundary_event(session_key: &str, kind: &str, detail: &str) {
    let path = boundary_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                "[chat_log] boundary event open failed {}: {}",
                path.display(),
                e
            );
            return;
        }
    };
    let entry = serde_json::json!({
        "role": "boundary",
        "event": kind,
        "detail": detail,
        "timestamp": Local::now().to_rfc3339(),
    });
    // R100（BUG-20261025-25）：与 write_chat_entry 同一行原子性理由——
    // 整行含换行单次 write_all，boundary 侧车并发写不撕行。
    if let Ok(line) = serde_json::to_string(&entry) {
        let mut buf = line;
        buf.push('\n');
        // 写失败与 open 失败同款可见（BUG 2026-10-29 横扫角度 A）：侧车是
        // 重放/审计的轻量证据面，丢行不允许无痕。
        if let Err(e) = file.write_all(buf.as_bytes()) {
            tracing::warn!(
                "[chat_log] boundary event write failed {}: {}",
                path.display(),
                e
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SAN-01/D4 启动迁移：旧「只替换 `:`」映射产生的嵌套目录平化
// ---------------------------------------------------------------------------

/// 把 `sessions_log_dir` / `boundary_events_dir` 下旧映射写出的嵌套子目录
/// 平化到顶层。B 端复合键 `{node}/{chat}` 在旧 `replace(':', "_")` 映射下
/// 会落成 `<root>/{node}/{chat}.jsonl`（目录逃逸，SAN-01；2026-09-15 真机
/// F-U4-4 同根因在 todo 路径已修、本族漏网）；白名单消毒
/// （[`nemesis_utils::sanitize::sanitize_path_segment`]）写平面
/// `<root>/{node}_{chat}.jsonl`。本函数按「目录名_文件名」拼接平移，与
/// 新白名单对原始键的映射一致。
///
/// 语义：只平移文件；目标已存在则跳过并 WARN（不覆盖）；平移后目录为空
/// 则顺手删除；幂等（平化后无子目录，二次运行零操作）；全程 best-effort
/// （失败 WARN 不阻断启动——旧文件留在原地，读不到但不损坏）。
///
/// D-2（复核 2026-09-16）：本迁移在部分入口早于 tracing logger 装配执行
/// （gateway Step 3-2），纯 tracing 事件会静默丢失——凡有实际动作（移动/
/// 跳过/失败）的点同时 eprintln 到 stderr 兜底；无迁移时零输出（幂等），
/// 不产生启动噪音。
pub fn migrate_nested_session_logs() {
    let pm = default_path_manager();
    for root in [pm.sessions_log_dir(), pm.boundary_events_dir()] {
        flatten_legacy_nested_dirs(&root);
    }
}

/// 迁移可见性兜底（D-2）：tracing（常规日志面）+ stderr（logger 未装配时
/// 的可见面）双写。只用于本迁移的实际动作点。
fn migration_log(msg: &str) {
    tracing::warn!("[chat_log] {msg}");
    eprintln!("[chat_log] {msg}");
}

fn flatten_legacy_nested_dirs(root: &std::path::Path) {
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return, // 目录不存在 = 无历史，零操作
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if !child.is_dir() {
            continue;
        }
        let Ok(dir_name) = entry.file_name().into_string() else {
            migration_log(&format!("平化迁移跳过非 UTF-8 目录: {}", child.display()));
            continue;
        };
        let Ok(files) = fs::read_dir(&child) else {
            continue;
        };
        let mut moved = 0usize;
        for f in files.flatten() {
            let fp = f.path();
            if !fp.is_file() {
                continue; // 更深的嵌套不追（现实键只一层 `/`）
            }
            let Some(name) = fp.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let target = root.join(format!("{dir_name}_{name}"));
            if target.exists() {
                migration_log(&format!(
                    "平化迁移跳过（目标已存在，不覆盖）: {}",
                    target.display()
                ));
                continue;
            }
            match fs::rename(&fp, &target) {
                Ok(()) => moved += 1,
                Err(e) => migration_log(&format!(
                    "平化迁移失败 {} -> {}: {}",
                    fp.display(),
                    target.display(),
                    e
                )),
            }
        }
        if moved > 0 {
            migration_log(&format!(
                "平化迁移 {}: 移动 {} 个文件（目录名={dir_name:?}）",
                root.display(),
                moved
            ));
        }
        let _ = fs::remove_dir(&child); // 空了就删；非空（跳过/子目录）留着
    }
}

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;

// D3 (devtool-upgrade 阶段 5)：ChatLogMeta 全字段入口 + file_changes 字段
// 写入 + dedup_file_changes 投影测试。
#[cfg(test)]
mod d3_tests;

// E3 (devtool-upgrade 阶段 5)：checkpoint_turn 行标记 + truncate_chat_log_rows
// 原位截断（消息级回退的落盘原语）测试。
#[cfg(test)]
mod e3_tests;

// T6（扩展计划 D5，2026-09-24）：append/truncate/fork 三写路径的事件账本
// 记账集成测试。
#[cfg(test)]
mod t6_tests;

// 覆盖率补充测试（全字段 meta / 行写截断原语 / sidecar meta 分支）。
#[cfg(test)]
mod cov_tests;

// Wave6B 覆盖率收尾批次（读/截断/复制 IO 错误分支 + whole-log 滑窗 +
// sidecar 写失败 + 平化迁移各分支）。
#[cfg(test)]
mod cov2_tests;

// N5（2026-10-13 机会项）：deliverables 字段写入 + dedup_presented_files
// 投影 + PresentedFile serde 缺省契约测试。
#[cfg(test)]
mod n5_tests;

// B11（2026-10-18 snooze）：SessionMeta 暂停标记三函数（write 幂等 upsert /
// clear 摘除返回原值 / snoozed_sessions 扫描过滤）测试。
#[cfg(test)]
mod b11_tests;

// detach（2026-10-20「脱离父分支」）：clear_session_lineage 血缘四字段摘除
// + 其余 meta 保留 + 无血缘诚实返 false 测试。
#[cfg(test)]
mod detach_tests;

// 会话级全部放行（2026-10-24）：write/read 持久位 round-trip + 旧 meta 兼容
// + allow_all_sessions 扫描过滤测试。
#[cfg(test)]
mod allow_all_tests;

// cov 系文件型测试的共享串行锁（2026-09-28 复查新增）：目录型夹具（rewind
// tmp 目录 / rename 目标目录 / legacy 嵌套目录 / boundary sidecar 目录）与

// R100（2026-10-25 会话持久化面审计）：并发追加的行原子性——多线程同会话
// 高频写入断言行数精确 + 逐行完整可解析（jsonl 撕行回归锁）。
#[cfg(test)]
mod r100_concurrent_append_tests;

// BUG-20261025-44（BUG-HUNT R121，会话持久化面读侧损坏静默吞 → loud 留痕）：
// read_meta_full / snoozed_sessions / allow_all_sessions 三读取点跳过语义
// 回归锁（warn 不可进程内断言，BUG-43 同族诚实边界）。
#[cfg(test)]
mod r44_tests;

// Work 时间线（2026-10-30 ChatPanel 长任务时间线改造）：assistant 行
// `"work"` 键写入形状（有中间轮才带 / 直答不带 / serde 形态与前端镜像）。
#[cfg(test)]
mod work_tests;
