//! 会话错误记录框架（v1 地基；docs/REPORT/2026-11-25_会话错误记录框架
//! 交付报告）。
//!
//! **纯追加旁路**：现有恢复流程（`call_llm_with_recovery` 的分类/分派/
//! 三恢复环/hook 终裁）无条件最优先、零改动；本模块只在其后记录
//! episode（发生行 pending + outcome 行两行制，trace_id + round 关联）。
//! 四元组 = 事件（分类+子形态+原文）/ sid / 首呼完整请求快照（图片引用
//! 降维 A 方案）/ 应答。estop/取消路径不经过任何记录点（estop 全局最高
//! 优先豁免）；记录器任何故障 fail-open（warn 放行，绝不影响 turn），与
//! capture_sink 同纪律。
//!
//! v1 只做记录：规则引擎（重发/调整重发/通知等动作）与【会话事件】页面
//! 后续另行计划；本模块只提供 JSONL 记录面（`<workspace>/logs/session_
//! errors/YYYY-MM-DD.jsonl` 按日追加）。

use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::llm_types::{LlmMessage, LlmResponse};

/// 全局单例（先到先得；gateway/run/acp 三入口 init 同一 workspace 语义）。
static GLOBAL: OnceLock<SessionErrorRecorder> = OnceLock::new();

/// 装配点入口（gateway ctx / run / acp 三处同源调用）。
/// `workspace` = 工作区根（记录落 `<workspace>/logs/session_errors/`）。
pub fn init(workspace: PathBuf, enabled: bool) {
    let dir = workspace.join("logs").join("session_errors");
    let _ = GLOBAL.set(SessionErrorRecorder::new(dir, enabled));
}

fn global() -> Option<&'static SessionErrorRecorder> {
    GLOBAL.get()
}

/// 快照门：recording 关 = None 零成本旁路（连请求 clone 都不做）。
/// 开 = 首呼完整请求快照（图片引用降维，正文文本字节完整）。
pub fn snapshot_if_enabled(
    model: &str,
    messages: &[LlmMessage],
    tool_defs: &[crate::types::ToolDefinition],
    opts: &crate::types::ChatOptions,
) -> Option<SessionErrorRequestSnapshot> {
    let recorder = global()?;
    if !recorder.enabled {
        return None;
    }
    Some(snapshot_request(model, messages, tool_defs, opts))
}

/// episode 发生行（分类分派点记录；outcome:pending）。
pub fn record_occurrence(
    sid: &str,
    trace_id: &str,
    round: u32,
    model: &str,
    snapshot: Option<&SessionErrorRequestSnapshot>,
    raw_error: &str,
) {
    if let Some(recorder) = global() {
        recorder.record_occurrence(sid, trace_id, round, model, snapshot, raw_error);
    }
}

/// episode outcome 行（恢复环成功 recovered_by / 终局出口 terminal +
/// outbound）。
pub fn record_outcome(trace_id: &str, round: u32, outcome: &SessionErrorOutcome) {
    if let Some(recorder) = global() {
        recorder.record_outcome(trace_id, round, outcome);
    }
}

/// hook Block 合并行（发生+终局同刻成立，单行含完整四元组 + outcome）。
pub fn record_hook_block(
    sid: &str,
    trace_id: &str,
    round: u32,
    model: &str,
    snapshot: Option<&SessionErrorRequestSnapshot>,
    reason: &str,
    response: &LlmResponse,
) {
    if let Some(recorder) = global() {
        recorder.record_hook_block(sid, trace_id, round, model, snapshot, reason, response);
    }
}

/// 事件分类单一推导点：class 锚 = `nemesis_utils::llm_error_class` 分类器
/// （8 值小写 snake 映射，不新造分类器）；sub = 恢复环同源的三个子形态锚
/// （会话级封禁 / connect error 传输 / 流中断前缀）。分类器 None 时
/// stream/connect 子形态按 timeout 诚实归类（与恢复环 transient 词表同
/// 义），其余 = unknown——原文全文在 event.raw，不丢信息。
pub fn classify_session_error(err: &str) -> (&'static str, Option<&'static str>) {
    let lower = err.to_lowercase();
    let sub = if nemesis_utils::llm_error_class::is_session_scoped_block(err) {
        Some("session_blocked")
    } else if lower.contains("connect error") {
        Some("connect_error")
    } else if lower.starts_with("stream interrupted from provider") {
        Some("stream_interrupted")
    } else {
        None
    };
    // 会话级封禁先于分类器：Forbidden 判定在 class_by_code（裸码路径），
    // 纯短语形态（is_session_scoped_block contains 锚）不走那条路——记录
    // 分类与恢复处置对齐（恢复侧同词表 → no-retry Forbidden 终局）。
    let class = if sub == Some("session_blocked") {
        "forbidden"
    } else {
        match nemesis_utils::llm_error_class::classify_llm_error(err) {
            Some(c) => class_name(c),
            None if sub == Some("stream_interrupted") || sub == Some("connect_error") => "timeout",
            None => "unknown",
        }
    };
    (class, sub)
}

fn class_name(c: nemesis_utils::llm_error_class::LlmErrorClass) -> &'static str {
    use nemesis_utils::llm_error_class::LlmErrorClass as C;
    match c {
        C::Auth => "auth",
        C::Forbidden => "forbidden",
        C::Billing => "billing",
        C::RateLimit => "rate_limit",
        C::Timeout => "timeout",
        C::Format => "format",
        C::Overloaded => "overloaded",
        C::ContextOverflow => "context_overflow",
    }
}

/// 首呼完整请求快照（messages + tool_defs + opts + model；「处理时必须
/// 给出完整的请求内容，一点都不能少」——只做图片引用降维，文本不裁剪）。
#[derive(Debug, Clone, Serialize)]
pub struct SessionErrorRequestSnapshot {
    pub model: String,
    pub messages: Vec<serde_json::Value>,
    pub tool_defs: Vec<crate::types::ToolDefinition>,
    pub opts: serde_json::Value,
}

pub fn snapshot_request(
    model: &str,
    messages: &[LlmMessage],
    tool_defs: &[crate::types::ToolDefinition],
    opts: &crate::types::ChatOptions,
) -> SessionErrorRequestSnapshot {
    SessionErrorRequestSnapshot {
        model: model.to_string(),
        messages: messages.iter().map(snapshot_message).collect(),
        tool_defs: tool_defs.to_vec(),
        opts: serde_json::to_value(opts).unwrap_or(serde_json::Value::Null),
    }
}

/// 单消息快照：serde 全形 + 图片引用降维（用户拍板 A，2026-11-25）。
/// base64 字节（单图 ≤25MB）→ sha256 引用 + 长度；path/media_type 保留，
/// 正文文本字节完整。与 `observer_msg_values` 同纪律（观测面不落像素字
/// 节），多出的 hash 供事后对账（uploads 7 天 TTL 内重放水合同一文件
/// hash 一致即可证字节一致；被清扫 = 重放时同样走「图片已失效」占位，
/// 与原始调用行为对齐）。
fn snapshot_message(m: &LlmMessage) -> serde_json::Value {
    let mut v = match serde_json::to_value(m) {
        Ok(v) => v,
        Err(_) => return serde_json::Value::Null,
    };
    if let Some(images) = v.get_mut("images").and_then(|x| x.as_array_mut()) {
        for img in images.iter_mut() {
            let Some(obj) = img.as_object_mut() else {
                continue;
            };
            let data = obj
                .get("data")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            if data.is_empty() {
                continue;
            }
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            hasher.update(data.as_bytes());
            let hash = format!("{:x}", hasher.finalize());
            obj.insert("data_sha256".to_string(), serde_json::Value::String(hash));
            obj.insert(
                "data_base64_len".to_string(),
                serde_json::Value::from(data.len()),
            );
            obj.insert(
                "data".to_string(),
                serde_json::Value::String(
                    "[sha256 引用降维：base64 字节不落记录，重放按 path 重新水合]".to_string(),
                ),
            );
        }
    }
    v
}

/// episode outcome（两行制的第二行；skip_serializing_if 保证每形态只有
/// 自有键——recovered 形态无 terminal/outbound 键，反之亦然）。
#[derive(Debug, Clone, Serialize)]
pub struct SessionErrorOutcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovered_by: Option<&'static str>,
    /// 实际重试次数（仅 transient 环带回；4b/effort 次数不外露，诚实省略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<&'static str>,
    /// 出站终文案（仅 terminal 形态——终局出口才存在；recovered 收口的
    /// 回复已在 chat log，不重复记录）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outbound: Option<String>,
}

impl SessionErrorOutcome {
    pub fn recovered(recovered_by: &'static str, retries: Option<u32>) -> Self {
        Self {
            recovered_by: Some(recovered_by),
            retries,
            terminal: None,
            outbound: None,
        }
    }

    /// `retries` = 该 episode 实际发起的重试次数（transient 环耗尽终局
    /// 带回；no_retry / 4a 压缩环耗尽等无环终局为 None，键诚实省略）。
    pub fn terminal(exit: &'static str, retries: Option<u32>, outbound: Option<String>) -> Self {
        Self {
            recovered_by: None,
            retries,
            terminal: Some(exit),
            outbound,
        }
    }
}

/// JSONL 按日追加写状态（Mutex 单写者；day 变化重开文件）。
struct WriterState {
    day: String,
    file: Option<File>,
}

/// 记录器实例。生产路径走 `GLOBAL` 单例（`init`）；测试直接构造实例绕开
/// OnceLock 毒化（与 session/tests.rs 的 CaptureSink 纪律同款）。
pub struct SessionErrorRecorder {
    dir: PathBuf,
    enabled: bool,
    state: Mutex<WriterState>,
}

impl SessionErrorRecorder {
    pub fn new(dir: PathBuf, enabled: bool) -> Self {
        Self {
            dir,
            enabled,
            state: Mutex::new(WriterState {
                day: String::new(),
                file: None,
            }),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn record_occurrence(
        &self,
        sid: &str,
        trace_id: &str,
        round: u32,
        model: &str,
        snapshot: Option<&SessionErrorRequestSnapshot>,
        raw_error: &str,
    ) {
        if !self.enabled {
            return;
        }
        let (class, sub) = classify_session_error(raw_error);
        let line = serde_json::json!({
            "kind": "occurrence",
            "ts": now_ms(),
            "trace_id": trace_id,
            "round": round,
            "model": model,
            "sid": sid,
            "event": { "class": class, "sub": sub, "raw": raw_error },
            "request": snapshot
                .and_then(|s| serde_json::to_value(s).ok())
                .unwrap_or(serde_json::Value::Null),
            "response": { "raw": raw_error },
            "outcome": "pending",
        });
        self.write_line(&line);
    }

    pub fn record_outcome(&self, trace_id: &str, round: u32, outcome: &SessionErrorOutcome) {
        if !self.enabled {
            return;
        }
        let line = serde_json::json!({
            "kind": "outcome",
            "ts": now_ms(),
            "trace_id": trace_id,
            "round": round,
            "outcome": outcome,
        });
        self.write_line(&line);
    }

    pub fn record_hook_block(
        &self,
        sid: &str,
        trace_id: &str,
        round: u32,
        model: &str,
        snapshot: Option<&SessionErrorRequestSnapshot>,
        reason: &str,
        response: &LlmResponse,
    ) {
        if !self.enabled {
            return;
        }
        let line = serde_json::json!({
            "kind": "hook_block",
            "ts": now_ms(),
            "trace_id": trace_id,
            "round": round,
            "model": model,
            "sid": sid,
            "event": { "class": "hook_block", "sub": null, "raw": reason },
            "request": snapshot
                .and_then(|s| serde_json::to_value(s).ok())
                .unwrap_or(serde_json::Value::Null),
            "response": { "raw": response.content },
            "outcome": { "terminal": "hook_block" },
        });
        self.write_line(&line);
    }

    /// 单行追加（fail-open 铁律：任何故障 warn 后丢弃当前行，绝不影响
    /// turn；文件句柄按日滚动复用，写失败置 None 下次重开）。
    fn write_line(&self, value: &serde_json::Value) {
        if !self.enabled {
            return;
        }
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        let Ok(line) = serde_json::to_string(value) else {
            return;
        };
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        if st.file.is_none() || st.day != day {
            if let Err(e) = std::fs::create_dir_all(&self.dir) {
                tracing::warn!(
                    "[SessionErrorRecorder] create_dir_all {} failed, line dropped (fail-open): {}",
                    self.dir.display(),
                    e
                );
                return;
            }
            match OpenOptions::new()
                .create(true)
                // read 与 append 并申请：append 句柄在 Windows 上无读权限，
                // 下面的撕裂尾修复末字节探测会静默失败。
                .read(true)
                .append(true)
                .open(self.dir.join(format!("{}.jsonl", day)))
            {
                Ok(mut f) => {
                    // 撕裂尾修复（BUG-20261126-130）：上一会话写失败/崩溃
                    // 留下的无换行尾巴不粘连本条——本文件是程序化消费的
                    // 失败取证账，两条记录拼成一行 = 解析全丢。
                    let _ = nemesis_utils::file::repair_torn_tail(&mut f);
                    st.file = Some(f);
                    st.day = day;
                }
                Err(e) => {
                    tracing::warn!(
                        "[SessionErrorRecorder] open jsonl failed, line dropped (fail-open): {}",
                        e
                    );
                    return;
                }
            }
        }
        let Some(f) = st.file.as_mut() else {
            return;
        };
        if let Err(e) = writeln!(f, "{}", line) {
            tracing::warn!(
                "[SessionErrorRecorder] write failed, line dropped (fail-open): {}",
                e
            );
            st.file = None;
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
