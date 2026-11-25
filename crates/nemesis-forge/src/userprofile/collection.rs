//! 用户习惯学习采集层（Forge 化重构批一，G6/G10/G11；批 C 采集增强
//! 2026-11-24）。
//!
//! 采集通路：
//! - [`collect_dialogue_after`]：session_logs **结构化读**——user/assistant
//!   全行 + assistant 行 `work` 时间线的工具名投影 + 批 C 四投影（文件种类
//!   `file_changes`/`deliverables` 路径按扩展名分类、中间轮叙述 `work`
//!   rounds text、来源通道 `channel` 键、模型名 `model` 键——缺字段宽容，
//!   旧条目照常解析）；
//! - [`collect_todos`]：`<ws>/sessions/todo_*.json` 读（todo 是用户「流程化
//!   执行仪式」的机器可读痕迹，数据已在盘上零成本可采）；
//! - [`append_event`]：负反馈事件落盘（veto/forget/rewind）——负样本回流
//!   的原始素材（G7 证据面）；
//! - [`append_signal`]：交互强信号落盘（批 C ④⑤——审批卡决策耗时/类别、
//!   estop 触发/释放）。**独立 signals.jsonl**，与 events.jsonl 分文件：
//!   events 是反思负样本回灌面（「不要再提同类结论」），signals 是行为
//!   观测面（挖掘器读），语义不同不混流。

use std::path::Path;

/// 会话日志行投影（时间升序批次的最小单元）。
#[derive(Debug, Clone, PartialEq)]
pub struct DialogueRow {
    /// epoch 毫秒。
    pub ts_ms: i64,
    /// "user" | "assistant"（boundary/system 通知行不进管线）。
    pub role: String,
    /// 消息正文（原文全量——截断由消费方决定）。
    pub content: String,
    /// 会话标识（session_logs 文件名去扩展名）。
    pub session: String,
    /// assistant 行 `work.rounds[].tools[].tool` 名投影（去重保序、截 8）。
    pub work_tools: Vec<String>,
    /// 批 C ①：assistant 行文件接触种类投影（`file_changes`+`deliverables`
    /// 路径按扩展名分类：code/doc/image/config/other，去重保序、截 8）。
    /// user 行恒空。
    pub file_kinds: Vec<String>,
    /// 批 C ②：assistant 行 `work.rounds[].text` 非空轮叙述投影
    /// （trim、截 80 字符、带该轮工具名截 3；总条数 ≤3——反思摘要的
    /// 「中间过程」读法素材，预算可控）。user 行/直答恒空。
    pub work_notes: Vec<String>,
    /// 批 C ⑥：user 行来源通道（chat_log `channel` 键原样；缺字段 =
    /// 未知通道 `""`——旧条目/未标记行宽容读）。
    pub channel: String,
    /// 批 C ③（替代形态）：assistant 行模型名（chat_log `model` 键原样；
    /// 缺字段 = `""`）。模型切换行为挖掘的数据源——替代原计划的
    /// request_logs 接入（chat_log 每行自带 model 徽标，同一信号零新
    /// 采集面）。
    pub model: String,
}

impl DialogueRow {
    /// 基础五字段构造器（批 C 四投影缺省空/未知——单测与旧消费方的
    /// 便利入口；全字段构造走结构体字面量）。
    pub fn new(
        ts_ms: i64,
        role: &str,
        content: &str,
        session: &str,
        work_tools: Vec<String>,
    ) -> Self {
        Self {
            ts_ms,
            role: role.to_string(),
            content: content.to_string(),
            session: session.to_string(),
            work_tools,
            file_kinds: Vec::new(),
            work_notes: Vec::new(),
            channel: String::new(),
            model: String::new(),
        }
    }
}

/// 读 session_logs 全部 `*.jsonl`，收集 `role in {user, assistant}` 且时间戳
/// 不早于 `window_start_ms`（证据窗起点，epoch 毫秒）的行，**按时间升序**
/// 返回（此前按文件序返回——反思通道「近期 60 条」采样取的是列表尾部；
/// 文件序由 session key 字典序决定，与新旧无关）。
///
/// 过滤纪律：
/// - `role=="boundary"`（轮边界哨兵行）与带 `system_notice` 的行（cron tick/
///   审批回灌/后台唤醒等系统注入）**不进管线**——它们不是真人消息，混入会
///   污染统计规则（平均长度/纠正前缀）与反思采样（旧采集器只筛 role==user
///   时把系统注入行当成了用户消息）；
/// - 行时间戳解析失败视为早于水线跳过（仅向前语义：无时间锚的数据不参与
///   画像）；
/// - **文件 mtime 预过滤**（复查 2026-11-25 批 F2）：行时间戳 ≤ 文件 mtime
///   （行先写、mtime 后盖），mtime < window_start ⇒ 整文件不可能有窗内行
///   ——stat 跳过，免大文件全量 `read_to_string`（`collect_terminal_sessions`
///   同款先例）。stat 失败同跳（无新鲜度证明不读，与「无时间锚不参与」同
///   纪律）；跨机时钟偏斜大于窗口间隙时可能误跳，与终端采集器同接受面；
/// - 缺目录 = 还没有任何会话（onboard 后未对话的常态），不是错误——按
///   Ok(空) 处理，不留每 tick warn 噪声（四路评审 B-Minor1 根修）；
/// - `read_dir` 结果显式排序（NTFS/ext4 目录序是平台实现细节，结果顺序
///   必须与时间排序解耦确定）。
pub fn collect_dialogue_after(
    workspace: &Path,
    window_start_ms: i64,
) -> Result<Vec<DialogueRow>, String> {
    let dir = workspace.join("logs").join("session_logs");
    let mut out = Vec::new();
    // 缺目录 = 常态空，不报错。
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(format!("read session_logs: {e}")),
    };
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.extension().map(|e| e != "jsonl").unwrap_or(true) {
            continue;
        }
        // mtime 预过滤（复查 2026-11-25 批 F2）：mtime < 窗起点 ⇒ 窗内行
        // 不可能在文件里（行先写、mtime 后盖），整文件免读。collect_terminal_
        // sessions 同款；stat 失败 = 无新鲜度证明，保守跳过。
        let mtime_fresh = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .map(|mtime| mtime >= window_start_ms);
        if mtime_fresh != Some(true) {
            continue;
        }
        let session = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        for line in content.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role != "user" && role != "assistant" {
                continue;
            }
            // 系统注入行（role=user 但带 system_notice 标记）不是真人消息。
            if v.get("system_notice").is_some() {
                continue;
            }
            // 行时间戳 = `timestamp`（RFC3339 字符串）。
            let ts = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            let Some(ts) = ts else { continue };
            let ts_ms = ts.timestamp_millis();
            if ts_ms < window_start_ms {
                continue;
            }
            let Some(c) = v.get("content").and_then(|c| c.as_str()) else {
                continue;
            };
            // assistant 行 work 时间线工具名投影（user 行恒空）。
            let work_tools = if role == "assistant" {
                extract_work_tools(&v)
            } else {
                Vec::new()
            };
            // 批 C 投影（assistant 行专属；user 行恒空缺省值）。
            let (file_kinds, work_notes) = if role == "assistant" {
                (extract_file_kinds(&v), extract_work_notes(&v))
            } else {
                (Vec::new(), Vec::new())
            };
            out.push(DialogueRow {
                ts_ms,
                role: role.to_string(),
                content: c.to_string(),
                session: session.clone(),
                work_tools,
                file_kinds,
                work_notes,
                channel: v
                    .get("channel")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string(),
                model: v
                    .get("model")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }
    // 时间升序（同毫秒保持文件/行序——sort_by_key 稳定）：反思采样与
    // 统计通道的输入从此与文件名字典序解耦。
    out.sort_by_key(|r| r.ts_ms);
    Ok(out)
}

/// assistant 行 `work.rounds[].tools[].tool` 名投影（去重保序、截 8）。
fn extract_work_tools(row: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(rounds) = row
        .get("work")
        .and_then(|w| w.get("rounds"))
        .and_then(|r| r.as_array())
    else {
        return out;
    };
    for round in rounds {
        let Some(tools) = round.get("tools").and_then(|t| t.as_array()) else {
            continue;
        };
        for t in tools {
            if let Some(name) = t.get("tool").and_then(|n| n.as_str())
                && !name.is_empty()
                && !out.iter().any(|e| e == name)
            {
                out.push(name.to_string());
                if out.len() >= 8 {
                    return out;
                }
            }
        }
    }
    out
}

/// 文件路径 → 种类（扩展名白名单分类；批 C ① 工作域挖掘的数据源）。
/// 未知扩展名 = "other"（消费方对 other 不出条目——宁缺勿滥）。
fn classify_file_kind(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "c" | "h" | "cpp" | "hpp"
        | "cs" | "kt" | "swift" | "vue" | "sh" | "ps1" | "rb" | "php" | "lua" | "sql" => "code",
        "md" | "txt" | "rst" | "doc" | "docx" | "pdf" => "doc",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp" => "image",
        "json" | "yaml" | "yml" | "toml" | "ini" | "conf" => "config",
        _ => "other",
    }
}

/// 批 C ①：assistant 行文件接触种类投影——`file_changes` + `deliverables`
/// 路径按扩展名分类（去重保序、截 8；`other` 也保留——种类占比的分子
/// 需要「接触过文件」这一事实，占比语义由消费方裁决）。
fn extract_file_kinds(row: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for key in ["file_changes", "deliverables"] {
        let Some(arr) = row.get(key).and_then(|a| a.as_array()) else {
            continue;
        };
        for item in arr {
            let Some(p) = item.get("path").and_then(|p| p.as_str()) else {
                continue;
            };
            if p.is_empty() {
                continue;
            }
            let kind = classify_file_kind(p);
            if !out.iter().any(|e| e == kind) {
                out.push(kind.to_string());
            }
            if out.len() >= 8 {
                return out;
            }
        }
    }
    out
}

/// 批 C ②：assistant 行中间轮叙述投影——`work.rounds[].text` 非空轮
/// （trim、截 [`WORK_NOTE_CHARS`] 字符；带该轮工具名截 3 条），总条数
/// ≤ [`WORK_NOTE_MAX`]。空叙述轮（纯工具轮/注记轮）跳过。
fn extract_work_notes(row: &serde_json::Value) -> Vec<String> {
    const WORK_NOTE_CHARS: usize = 80;
    const WORK_NOTE_MAX: usize = 3;
    const WORK_NOTE_TOOLS: usize = 3;
    let mut out: Vec<String> = Vec::new();
    let Some(rounds) = row
        .get("work")
        .and_then(|w| w.get("rounds"))
        .and_then(|r| r.as_array())
    else {
        return out;
    };
    for round in rounds {
        let text = round
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let excerpt: String = text.chars().take(WORK_NOTE_CHARS).collect();
        let tools: Vec<String> = round
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.get("tool").and_then(|n| n.as_str()))
                    .filter(|n| !n.is_empty())
                    .take(WORK_NOTE_TOOLS)
                    .map(|n| n.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let note = if tools.is_empty() {
            excerpt
        } else {
            format!("{excerpt}｜工具: {}", tools.join(", "))
        };
        out.push(note);
        if out.len() >= WORK_NOTE_MAX {
            return out;
        }
    }
    out
}

/// todo 快照（单文件）。
#[derive(Debug, Clone)]
pub struct TodoSnapshot {
    /// todo 文件名（如 `todo_web_xxx.json`，溯源用）。
    pub file: String,
    /// 条目（content + status）。
    pub items: Vec<(String, String)>,
}

/// 读 `<ws>/sessions/todo_*.json`（`nemesis-path::session_todo_file_in_workspace`
/// 写路径的盘上产物），解析 `Vec<TodoItem>`。
///
/// `now_ms`/`max_age_ms`：文件 mtime 距今超过 `max_age_ms` 的跳过（陈年
/// 死会话的 todo 不参与习惯挖掘——7 天窗足够覆盖周末间隔）。解析失败的
/// 文件跳过（损坏 todo 不炸采集）。
pub fn collect_todos(
    workspace: &Path,
    now_ms: i64,
    max_age_ms: i64,
) -> Result<Vec<TodoSnapshot>, String> {
    let dir = nemesis_path::resolve_sessions_dir_in_workspace(workspace);
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(format!("read sessions dir: {e}")),
    };
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.starts_with("todo_") || !name.ends_with(".json") {
            continue;
        }
        // 陈旧 todo 跳过（mtime 缺失 = 当陈旧处理，保守）。
        let mtime_ms = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        let Some(mt) = mtime_ms else { continue };
        if now_ms.saturating_sub(mt) > max_age_ms {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(items) = serde_json::from_str::<Vec<nemesis_types::agent::TodoItem>>(&content)
        else {
            continue;
        };
        out.push(TodoSnapshot {
            file: name.to_string(),
            items: items
                .into_iter()
                .map(|t| {
                    // TodoStatus 无 Display：serde snake_case 串化（写侧同形态）。
                    let status = match t.status {
                        nemesis_types::agent::TodoStatus::Pending => "pending",
                        nemesis_types::agent::TodoStatus::InProgress => "in_progress",
                        nemesis_types::agent::TodoStatus::Completed => "completed",
                    };
                    (t.content, status.to_string())
                })
                .collect(),
        });
    }
    Ok(out)
}

/// 负反馈事件落盘（`<ws>/forge/userprofile/events.jsonl` append-only）。
/// 失败静默（学习侧旁路数据，不炸主流程）。`event` 需自带 ts/kind 字段。
/// 持 WRITE_LOCK（复查 2026-11-25：prune 引入后的 RMW 竞窗防线）。
///
/// **总闸**（复查 2026-11-25 批 F3）：与 [`append_signal`] 同款——config
/// `user_profile.enabled` 关 = 事件也不落（关闭状态下事件消费者——反思
/// 负样本注记 / rework rewind 计数——全在 enabled 闸后，落盘只积无消费
/// 数据）；`home` 缺失 = 闸面不可得，fail-closed 不落。
pub fn append_event(workspace: &Path, home: Option<&str>, event: &serde_json::Value) {
    let Some(home) = home else {
        return;
    };
    if !super::telemetry::profile_enabled(home) {
        return;
    }
    let dir = workspace.join("forge").join("userprofile");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let line = match serde_json::to_string(event) {
        Ok(l) => l,
        Err(_) => return,
    };
    let path = dir.join("events.jsonl");
    let _w = WRITE_LOCK.lock();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

/// 读最近 `limit` 条负反馈事件（时间升序尾部取回，反思上下文用）。
/// 缺失/损坏 = 空_vec（旁路数据，诚实降级）。
///
/// 尾部优先解析（复查 2026-11-25）：从文件尾倒序攒够 `limit` 条有效行即
/// 停——语义与旧「全量 parse 后 tail-N」一致（坏行跳过、升序返回），
/// parse 成本从 O(全量) 降到 O(尾部窗口)；文件无清理期长期积累的旧行
/// 不再逐轮重付。
pub fn read_recent_events(workspace: &Path, limit: usize) -> Vec<serde_json::Value> {
    if limit == 0 {
        return Vec::new();
    }
    let path = workspace
        .join("forge")
        .join("userprofile")
        .join("events.jsonl");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines().rev() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            out.push(v);
            if out.len() >= limit {
                break;
            }
        }
    }
    out.reverse();
    out
}

/// 交互强信号落盘（批 C ④⑤：`<ws>/forge/userprofile/signals.jsonl`
/// append-only）。失败静默（学习侧旁路数据，不炸主流程——审批/estop
/// 主语义零依赖此文件）。`signal` 需自带 `ts_ms`/`kind` 字段。
///
/// **总闸**（复查 2026-11-25）：config `user_profile.enabled` 关 = 信号
/// 也不落（`telemetry::profile_enabled`，mtime 缓存）——关闭状态下挖掘
/// 器不跑，信号零消费者，落盘只积无消费数据（存储即隐私闸统一，与
/// wsapi_usage 面 `/ route` 闸同语义）。`home` 缺失 = 闸面不可得，
/// fail-closed 不落（隐私方向安全侧）。
///
/// 行形态（挖掘器 [`super::mining`] 消费）：
/// - 审批卡：`{"ts_ms", "kind":"approval", "operation", "risk_level",
///   "approved", "duration_ms", "session_key"}`（duration_ms = 卡片创建到
///   裁决的墙钟——用户决策耗时的直接观测）；
/// - 急停：`{"ts_ms", "kind":"estop", "action":"engage"|"release"}`。
pub fn append_signal(workspace: &Path, home: Option<&str>, signal: &serde_json::Value) {
    let Some(home) = home else {
        return;
    };
    if !super::telemetry::profile_enabled(home) {
        return;
    }
    let dir = workspace.join("forge").join("userprofile");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let line = match serde_json::to_string(signal) {
        Ok(l) => l,
        Err(_) => return,
    };
    let path = dir.join("signals.jsonl");
    let _w = WRITE_LOCK.lock();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

/// 读最近 `limit` 条交互强信号（时间升序尾部取回，挖掘器上下文用）。
/// 缺失/损坏 = 空 vec（旁路数据，诚实降级）。尾部优先解析（
/// read_recent_events 同款，复查 2026-11-25）。
pub fn read_recent_signals(workspace: &Path, limit: usize) -> Vec<serde_json::Value> {
    if limit == 0 {
        return Vec::new();
    }
    let path = workspace
        .join("forge")
        .join("userprofile")
        .join("signals.jsonl");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in content.lines().rev() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            out.push(v);
            if out.len() >= limit {
                break;
            }
        }
    }
    out.reverse();
    out
}

// ---------- 信号/事件文件清理（复查 2026-11-25，prune_queue 消费） ----------

/// append 写串行（`signals.jsonl` / `events.jsonl` 共用：append 与 prune
/// 的 RMW 竞窗防线——bridge.rs / telemetry.rs 同族模式。复查 2026-11-25
/// 补锁：append 原本无锁 fire-and-forget，prune 引入后写回会吞并发
/// append 行，统一持锁纪律）。纯读面（read_recent_*）锁外快照无害。
static WRITE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// 通用行清理：删除 `ts_ms` 早于 `cutoff_ms` 的行，其余原样保留（损坏行
/// 不过滤——不吃数据）。非零删除才原子写回（省一次原子写）。返回删除
/// 条数。缺文件 = Ok(0)。**全程持 WRITE_LOCK**（RMW 纪律，与 append 同锁）。
fn prune_ts_lines_older_than(path: &Path, cutoff_ms: i64) -> Result<usize, String> {
    let _w = WRITE_LOCK.lock();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let mut pruned = 0usize;
    let mut out = String::new();
    for line in content.lines() {
        let ts = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("ts_ms").and_then(|t| t.as_i64()));
        match ts {
            Some(ts) if ts < cutoff_ms => {
                pruned += 1; // 整行删除
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if pruned == 0 {
        return Ok(0);
    }
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), out.as_bytes(), 0o600)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(pruned)
}

/// 清理信号文件：`ts_ms` 早于 `cutoff_ms` 的行整行删除（signals.jsonl 是
/// 纯流式观测数据——无 pending 概念，全部行都是清理对象；挖掘器只读
/// 最近滚动窗，清理不伤挖掘）。返回删除条数。缺文件 = Ok(0)。
pub fn prune_signals_older_than(workspace: &Path, cutoff_ms: i64) -> Result<usize, String> {
    prune_ts_lines_older_than(
        &workspace
            .join("forge")
            .join("userprofile")
            .join("signals.jsonl"),
        cutoff_ms,
    )
}

/// 清理负反馈事件文件（events.jsonl，语义同 [`prune_signals_older_than`]）。
pub fn prune_events_older_than(workspace: &Path, cutoff_ms: i64) -> Result<usize, String> {
    prune_ts_lines_older_than(
        &workspace
            .join("forge")
            .join("userprofile")
            .join("events.jsonl"),
        cutoff_ms,
    )
}

// ---------- 终端会话采集（2026-11-24 T 批：PTY 审计 → 命令行投影） ----------

/// 单文件终端命令行数上限（防超长 TUI 会话淹没挖掘器；截断 = 诚实降级）。
pub const TERMINAL_MAX_LINES_PER_SESSION: usize = 200;
/// 单条命令行字符上限（参数超长的命令截断——挖掘器只消费首 token，
/// 截断只为防内存异常行）。
pub const TERMINAL_LINE_CHARS: usize = 120;

/// 输出流凭据提示标记（大小写不敏感子串）：命中 = 下一条完整输入行是
/// 密码类输入（password/passphrase 提示后紧跟的键入），整行丢弃。
/// 过度丢弃（提示后第一条是正常命令时误丢）= 保守方向，可接受。
const PASSWORD_PROMPT_MARKERS: &[&str] = &[
    "password:",
    "密码：",
    "密码:",
    "passphrase",
    "口令：",
    "口令:",
    "[sudo]",
    "pin:",
    "pin：",
];

/// 输入行秘密形态标记（大小写不敏感子串）：命中 = 该行携带凭据字面量
/// （token=xxx / api_key=yyy / Bearer zzz / sk-…），整行丢弃——这些行
/// 永远不进挖掘面。
const INPUT_SECRET_PATTERNS: &[&str] = &[
    "sk-",
    "ghp_",
    "gho_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "akia",
    "bearer ",
    "token=",
    "password=",
    "passwd=",
    "pwd=",
    "secret=",
    "api_key=",
    "apikey=",
    "apisecret",
    "access_key=",
    "private_key=",
];

/// 终端会话命令投影（单 PTY 审计文件清洗后的命令行集）。
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalSession {
    /// 审计文件名（溯源）。
    pub file: String,
    /// `# shell:` 头声明（缺头 = 未知空串——shell 偏好挖掘器跳过）。
    pub shell: String,
    /// 文件 mtime（epoch 毫秒；审计行无时间戳，文件级时间是诚实下限）。
    pub ts_ms: i64,
    /// 清洗后的输入命令行（凭据防线已过；保序；截 [`TERMINAL_MAX_LINES_PER_SESSION`]）。
    pub commands: Vec<String>,
}

/// 读 `<ws>/logs/terminal/pty_*.log` 审计，投影清洗后的命令行集。
///
/// 审计形态（nemesis-web pty.rs AuditWriter）：`#` 头行（含
/// `# shell: {shell}`）+ 帧行 `[I] <data>` / `[O] <data>`（原始终端字节
/// 可含换行 → 续行不属于任何帧头）。纪律：
/// - `read_dir` 结果显式排序（目录序是平台实现细节）；
/// - 文件 mtime 缺失或早于 `window_start_ms` → 整文件跳过（仅向前）；
/// - 输出帧（含续行）含凭据提示标记 → **下一条完整输入行丢弃**
///   （密码提示后紧跟的键入；两道防线之一）；
/// - 输入行含秘密形态标记 → 整行丢弃（二）；含 ANSI ESC（\x1b）→ 跳过
///   （TUI 序列行不是人敲的命令）；空行跳过；
/// - 缺目录 = 从未用内嵌终端（常态），Ok(空) 不报错；
/// - 解析永远成功（审计是行文本，坏行只是不产出命令）。
pub fn collect_terminal_sessions(
    workspace: &Path,
    window_start_ms: i64,
) -> Result<Vec<TerminalSession>, String> {
    let dir = workspace.join("logs").join("terminal");
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(format!("read terminal dir: {e}")),
    };
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.starts_with("pty_") || !name.ends_with(".log") {
            continue;
        }
        let mtime_ms = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        let Some(ts_ms) = mtime_ms else { continue };
        if ts_ms < window_start_ms {
            continue;
        }
        // 非法 UTF-8 容错（复查 2026-11-25）：写侧（nemesis-web pty.rs）落的是
        // 原始终端字节——ConPTY 读块边界可劈开多字节字符（中文输出高发），
        // 中文 Windows cmd.exe 默认 chcp 936 输出直接是 GBK；read_to_string
        // 会整文件 Err → 整会话静默丢失。改 lossy 解码：坏字节变替换符，
        // ASCII 命令首 token 不受损，挖掘照常；仅真出现替换时 warn 一次。
        let Ok(bytes) = std::fs::read(&p) else {
            continue;
        };
        let content = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "终端审计文件含非 UTF-8 字节（ConPTY 块边界劈开多字节字符或非 UTF-8 编码输出），lossy 解码继续: {}",
                    p.display()
                );
                String::from_utf8_lossy(e.into_bytes().as_slice()).into_owned()
            }
        };
        let (shell, commands) = parse_terminal_audit(&content);
        out.push(TerminalSession {
            file: name.to_string(),
            shell,
            ts_ms,
            commands,
        });
    }
    Ok(out)
}

/// 单文件审计解析（纯函数）：返回 (shell 声明, 清洗后命令行)。
///
/// 单遍扫描（清理与时序一致——密码提示标记只压制**其后**出现的输入行，
/// 两遍法会把提示误套到更早的命令上）：`[I] `/`[O] ` 开行 = 新帧，其余
/// 非头非空行 = 上一帧的续行（原始终端数据换行）。每个输入帧/续行行即
/// 一条终端输入行，就地清洗（空行 / ANSI 行 / 秘密形态行 / 密码提示邻接
/// 行过滤 + 长度与条数上限）；输出帧只用于凭据提示探测。`# shell:` 头
/// 提取 shell 声明。
fn parse_terminal_audit(content: &str) -> (String, Vec<String>) {
    let mut shell = String::new();
    let mut commands: Vec<String> = Vec::new();
    let mut suppress_next_input = false;
    let mut in_input_frame = false;
    for line in content.lines() {
        let (input_line, output_line) = if let Some(data) = line.strip_prefix("[I] ") {
            in_input_frame = true;
            (Some(data), false)
        } else if line.starts_with("[O] ") {
            in_input_frame = false;
            (None, true)
        } else if line.starts_with('#') {
            // 头部行（writer 每帧后补空行，头只出现在文件头部；续行以
            // '#' 开头的误判代价 = 该行不进命令流，保守方向可接受）。
            in_input_frame = false;
            if let Some(rest) = line.strip_prefix("# shell:") {
                shell = rest.trim().to_string();
            }
            (None, false)
        } else if line.is_empty() {
            continue; // 帧间分隔空行
        } else if in_input_frame {
            // 输入帧续行（终端原始数据换行）= 同样是一条终端输入行。
            (Some(line), false)
        } else {
            // 输出帧续行：凭据提示也可能跨行，同帧头同判。
            (None, true)
        };
        if output_line {
            let lower = line.to_lowercase();
            if PASSWORD_PROMPT_MARKERS.iter().any(|m| lower.contains(m)) {
                suppress_next_input = true;
            }
            continue;
        }
        let Some(raw) = input_line else { continue };
        let line = raw.trim();
        if line.is_empty() || line.contains('\u{1b}') {
            continue; // 空行 / ANSI TUI 序列行（不是人敲的命令）
        }
        let lower = line.to_lowercase();
        if INPUT_SECRET_PATTERNS.iter().any(|m| lower.contains(m)) {
            continue; // 秘密形态行整行丢弃（两道防线之二）
        }
        if suppress_next_input {
            suppress_next_input = false;
            continue; // 密码提示邻接输入行整行丢弃（两道防线之一）
        }
        let clipped: String = line.chars().take(TERMINAL_LINE_CHARS).collect();
        commands.push(clipped);
        if commands.len() >= TERMINAL_MAX_LINES_PER_SESSION {
            break;
        }
    }
    (shell, commands)
}

#[cfg(test)]
mod tests;
