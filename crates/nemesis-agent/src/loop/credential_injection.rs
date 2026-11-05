//! 凭据别名最后一刻注入（vault 计划 /，2026-09-22 计划）。
//!
//! 通用机制原则：本模块不认识任何业务名词（不知道"webhook"、"api_key"是
//! 什么）——工具通过 [`crate:loop:Tool:credential_arg_keys`] **声明**自己
//! 参数里承载凭据别名的槽位；机制只在 dispatch 瀑布最内层、`execute` 调用
//! 前的一瞬，把槽位里的 `vault:<alias>` 改写成真值（copy 语义）。
//!
//! 为什么是"最后一刻 + copy"：`HookToolCall.arguments` 这一个字符串派生出
//! 全部四个泄漏表面——pre-hooks、ToolEventHook 的 args_preview、会话历史
//! StoredToolCall、observer/request_logger。在最内层闭包里改写**副本**，
//! 上游全部表面天然只见别名；改写后的真值只活在 `execute` 的入参里。
//!
//! 槽位语义：值以 `vault:` 开头 → 经全局解析器现查（与 provider api_key
//! 同链路，见 nemesis_config:vault_ref）；非 vault 值（模型传的字面量）
//! 原样通过（向后兼容，工具自行决定语义）。解析失败 → 结构化错误
//! [`CredentialInjectionError`]（Display = 人类可读错误串，fail loud），
//! 调用方可选接凭据发卡恢复 [`try_card_recovery`]（2026-11-02）：
//! broker 在场时发凭据卡 → Stored 后原地重注入续行；缺席/被拒/超时走
//! 原错误串零回归。
//!
//! （2026-11-02）扩展：①错误串 oracle 收敛——「别名不存在」根因的
//! message 通用化「凭据引用无效」，不给探测方存在性裁决（其余环境级
//! 根因原文透传）；②[`find_declared_vault_references`] 供 dispatch 凭据
//! 闸（tool_dispatch，注入前）做闸面探测——真值出域前的人工同意面。
//! 槽位遍历经 [`walk_declared_slots`] 单点收拢，注入与检测零漂移。
//!
//! 扩展：声明槽位为**对象**时递归改写其字符串叶子（string→string
//! 结构，如 web_fetch 的 `headers`）——顶层键是业务分组、凭据活在叶值里。
//! 深度恒 1 层（声明键 → 对象 → 字符串叶），不做任意深度递归：声明的
//! 是「这个槽承载凭据」，更深的嵌套声明不出来，宁缺毋滥。

use crate::r#loop::Tool;
use nemesis_types::agent::{CredentialBroker, CredentialOutcome, CredentialRequest};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 发卡等待超时（秒）：与 auth 恢复卡 `_CREDENTIAL_CARD_TIMEOUT_SECS` 对齐。
const CARD_TIMEOUT_SECS: u64 = 300;
/// 发卡防滥用占位 TTL：同 `(session_key, alias)` 在窗内至多一张卡。
const CARD_GUARD_TTL: Duration = Duration::from_secs(600);
/// 占位表到顶先清过期条目的阈值。
const CARD_GUARD_SWEEP_AT: usize = 1024;
/// 清扫后仍满的硬顶（拒绝发卡 + debug 留痕，诚实降级；≈400KB 内存上界）。
const CARD_GUARD_HARD_CAP: usize = 4096;
/// 发卡 request_id 序号（进程单调，唯一性不依赖时钟）。
static CARD_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 凭据注入失败的结构化错误。`Display` 产出与 legacy 错误串**逐字
/// 节一致**的文案（既有模型可读契约零回归）；结构化字段供发卡流程取
/// 别名/槽位。`alias` 为空 = 空 `vault:` 引用形态（发不出卡）。
#[derive(Debug, Clone)]
pub struct CredentialInjectionError {
    /// 声明槽位（顶层键或 `键.叶键` 形态）。
    pub field: String,
    /// vault 别名（不含 `vault:` 前缀；空别名形态存在）。
    pub alias: String,
    /// 根因串（`resolve_vault_reference` 的 Err 文本 / 序列化失败）。
    pub message: String,
}

impl std::fmt::Display for CredentialInjectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "⛔ CREDENTIAL REFERENCE FAILED [layer:credential|field:{field}] {message}。\
             工具未执行。不要重试同一别名；若别名正确，请让用户运行 \
             `nemesisbot vault set <alias>` 写入后再试。",
            field = self.field,
            message = self.message,
        )
    }
}

/// 把 `args`（工具参数 JSON）中声明槽位里的 `vault:` 引用改写为真值。
///
/// legacy 签名（错误 = Display 串）：既有调用方（plugin_bridge、测试）
/// 零改动；发卡恢复面请用 [`inject_credential_aliases_structured`]。
pub fn inject_credential_aliases(tool: &dyn Tool, args: &str) -> Result<String, String> {
    inject_credential_aliases_structured(tool, args).map_err(|e| e.to_string())
}

/// [`inject_credential_aliases`] 的结构化变体：`Err` 带 field/alias/message，
/// 供发卡恢复流程（[`try_card_recovery`]）取发卡元数据。
pub fn inject_credential_aliases_structured(
    tool: &dyn Tool,
    args: &str,
) -> Result<String, CredentialInjectionError> {
    let keys = tool.credential_arg_keys();
    if keys.is_empty() {
        return Ok(args.to_string());
    }
    let mut val: serde_json::Value = match serde_json::from_str(args) {
        Ok(v) => v,
        // 非 JSON 参数：交给工具自身/上游校验去响亮失败，注入层不伪装。
        Err(_) => return Ok(args.to_string()),
    };
    walk_declared_slots(keys, &mut val, resolve_slot_value)?;
    serde_json::to_string(&val).map_err(|e| CredentialInjectionError {
        field: String::new(),
        alias: String::new(),
        message: format!("凭据注入序列化失败: {e}"),
    })
}

/// 声明槽位的通用遍历（抽取：槽位形态契约单点收拢于此）。对每个
/// 声明键下的字符串槽值调用 `visit(字段路径, 槽值)`——字段路径为顶层键
/// 或 `键.叶键` 形态。形态 = 顶层 string 槽 | 对象叶深度恒 1（声明键 →
/// 对象 → 字符串叶）；非字符串/对象槽位与缺席键跳过（工具的 schema 校验
/// 域，注入/检测层都不代管）。`visit` 返回 Err 即短路（注入 = 首个解析
/// 失败即停；检测 = 全量收集恒 Ok）。
fn walk_declared_slots<E>(
    keys: &[&str],
    val: &mut serde_json::Value,
    mut visit: impl FnMut(&str, &mut String) -> Result<(), E>,
) -> Result<(), E> {
    let serde_json::Value::Object(map) = val else {
        return Ok(());
    };
    for key in keys {
        let Some(slot) = map.get_mut(*key) else {
            // 槽位缺席：工具的 schema 校验域，注入层不代管。
            continue;
        };
        match slot {
            serde_json::Value::String(s) => visit(key, s)?,
            serde_json::Value::Object(inner) => {
                for (leaf_key, leaf) in inner.iter_mut() {
                    if let serde_json::Value::String(s) = leaf {
                        visit(&format!("{key}.{leaf_key}"), s)?;
                    }
                }
            }
            // 非字符串/对象槽位：工具的 schema 校验域，注入层不代管。
            _ => continue,
        }
    }
    Ok(())
}

/// 检测声明槽位值中的 `vault:` 引用（dispatch 凭据闸的闸面探测）。
/// **只查声明槽**——未声明槽位里的 `vault:` 子串不进闸面（闸位精确性；
/// 字面量明文凭据是凭据扫描层 Layer 4 的管辖面，闸不扩大）。返回
/// `(字段路径, 别名)` 列表，别名 = 剥 `vault:` 前缀（空别名形态保留：
/// 空引用也是引用）。非 JSON args → 空（与注入透传语义同构）。
pub fn find_declared_vault_references(tool: &dyn Tool, args: &str) -> Vec<(String, String)> {
    let keys = tool.credential_arg_keys();
    if keys.is_empty() {
        return Vec::new();
    }
    let Ok(mut val) = serde_json::from_str::<serde_json::Value>(args) else {
        return Vec::new();
    };
    let mut refs = Vec::new();
    match walk_declared_slots(
        keys,
        &mut val,
        |field, slot| -> Result<(), std::convert::Infallible> {
            if let Some(alias) = slot.strip_prefix(nemesis_config::VAULT_PREFIX) {
                refs.push((field.to_string(), alias.to_string()));
            }
            Ok(())
        },
    ) {
        Ok(()) => {}
    }
    refs
}

/// oracle 收敛：resolver Err 含 marker [`ALIAS_NOT_FOUND_MARKER`]
/// （跨 crate 事实契约，源 = nemesis-security vault.rs `AliasNotFound`
/// Display；pin 测试钉住）→ 通用化「凭据引用无效」，不给探测方存在性
/// 裁决、不回显别名。其余根因（vault 文件缺失 / 打开失败 / 已锁定 /
/// 未装解析器 / 空别名）原文透传——环境级常量差分对任意别名同形，无
/// 逐别名信号。
const ALIAS_NOT_FOUND_MARKER: &str = "别名不存在";
const CREDENTIAL_REF_INVALID_MSG: &str =
    "凭据引用无效：凭据库无法提供该引用的值（引用存在性不作判定）";

/// 单个字符串槽位的 vault 引用改写（`vault:` 前缀判定 + 现查 + 原位覆写）。
/// `field` 只用于错误文案（顶层键或 `键.叶键` 形态）。
fn resolve_slot_value(field: &str, slot: &mut String) -> Result<(), CredentialInjectionError> {
    if let Some(resolved) = nemesis_config::resolve_vault_reference(slot) {
        match resolved {
            Ok(secret) => *slot = secret,
            Err(e) => {
                let alias = slot
                    .strip_prefix(nemesis_config::VAULT_PREFIX)
                    .unwrap_or_default()
                    .to_string();
                let message = if e.contains(ALIAS_NOT_FOUND_MARKER) {
                    CREDENTIAL_REF_INVALID_MSG.to_string()
                } else {
                    e
                };
                return Err(CredentialInjectionError {
                    field: field.to_string(),
                    alias,
                    message,
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ：发卡恢复 + 防滥用占位
// ---------------------------------------------------------------------------

/// 发卡防滥用占位表：`(session_key, alias)` → 占位时刻。同一会话对同一
/// 别名在 TTL 窗内至多发一张卡（模型刷不存在别名不会轰炸用户卡）；
/// Stored 成功后清除占位（立即恢复可卡），Denied/超时保留（冷却窗口）。
/// 纯内存态，随 loop 重建清零。
pub struct CredentialCardGuard {
    entries: Mutex<HashMap<(String, String), Instant>>,
}

impl CredentialCardGuard {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// 是否允许发卡（允许即占位）。空别名恒 false（发不出卡）。
    pub fn try_acquire(&self, session_key: &str, alias: &str) -> bool {
        self.try_acquire_at(session_key, alias, Instant::now())
    }

    /// [`try_acquire`] 的时钟注入形态（TTL 过期路径可测）。
    pub(crate) fn try_acquire_at(&self, session_key: &str, alias: &str, now: Instant) -> bool {
        if alias.is_empty() {
            return false;
        }
        let mut map = self.entries.lock();
        if map.len() >= CARD_GUARD_SWEEP_AT {
            map.retain(|_, t| now.saturating_duration_since(*t) < CARD_GUARD_TTL);
        }
        if map.len() >= CARD_GUARD_HARD_CAP {
            tracing::debug!(
                "[CredentialInjection] card guard at hard cap ({CARD_GUARD_HARD_CAP}), refusing new cards"
            );
            return false;
        }
        let key = (session_key.to_string(), alias.to_string());
        if let Some(t) = map.get(&key)
            && now.saturating_duration_since(*t) < CARD_GUARD_TTL
        {
            return false;
        }
        map.insert(key, now);
        true
    }

    /// 清除占位（Stored 成功后调用：立即恢复可卡状态）。
    pub fn release(&self, session_key: &str, alias: &str) {
        self.entries
            .lock()
            .remove(&(session_key.to_string(), alias.to_string()));
    }
}

impl Default for CredentialCardGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// 注入失败的发卡恢复：broker 在场 → 发凭据卡（service=工具名、
/// key_name=别名）→ Stored 后**重跑整个 args 注入**（多槽一致性）返回
/// 续行参数；`None` = 走原错误路径（broker 缺席 / 空别名 / guard 冷却 /
/// Denied / 超时 / broker Err / 重注入再失败——每次 attempt 至多一张卡）。
///
/// 卡等待阻塞本工具调用至多 [`CARD_TIMEOUT_SECS`] 秒（auth 恢复卡同级）；
/// estop 在途收口链（broker `close_all` → Denied）自然落 `None` 臂。
/// 日志纪律：只记 tool/field/alias/session/request_id，值不落日志。
pub async fn try_card_recovery(
    broker: Option<Arc<dyn CredentialBroker>>,
    guard: &CredentialCardGuard,
    tool: &dyn Tool,
    args: &str,
    session_key: &str,
    tool_name: &str,
    err: &CredentialInjectionError,
) -> Option<String> {
    let Some(broker) = broker else {
        return None; // headless / 裸构建：原错误路径零回归
    };
    if err.alias.is_empty() {
        return None;
    }
    if !guard.try_acquire(session_key, &err.alias) {
        tracing::debug!(
            "[CredentialInjection] card suppressed by guard (TTL window) tool={tool_name} alias={}（值不落日志）",
            err.alias
        );
        return None;
    }
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let request_id = format!(
        "credinj-{millis}-{}",
        CARD_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let request = CredentialRequest {
        request_id: request_id.clone(),
        service: tool_name.to_string(),
        key_name: err.alias.clone(),
        reason: format!(
            "工具 {tool_name} 的参数 {} 引用 vault:{}，但凭据库暂无该别名。提交后将自动写入并重试本工具调用。",
            err.field, err.alias
        ),
        timeout_secs: CARD_TIMEOUT_SECS,
    };
    tracing::info!(
        "[CredentialInjection] raising credential card tool={tool_name} field={} alias={} session={session_key} request_id={request_id}（值不落日志）",
        err.field,
        err.alias
    );
    let b = Arc::clone(&broker);
    // spawn_blocking：broker 是同步阻塞语义（mpsc recv_timeout），且
    // current-thread runtime 下 block_in_place 会 panic（recovery.rs 同款）。
    let outcome = tokio::task::spawn_blocking(move || b.request_credential(&request))
        .await
        .ok()
        .and_then(|r| r.ok());
    match outcome {
        Some(CredentialOutcome::Stored) => {
            guard.release(session_key, &err.alias);
            match inject_credential_aliases_structured(tool, args) {
                Ok(a) => {
                    tracing::info!(
                        "[CredentialInjection] credential stored, re-injection succeeded tool={tool_name} alias={}（值不落日志）",
                        err.alias
                    );
                    Some(a)
                }
                Err(e2) => {
                    tracing::warn!(
                        "[CredentialInjection] credential stored but re-injection failed again tool={tool_name} field={}（值不落日志）",
                        e2.field
                    );
                    None
                }
            }
        }
        _ => {
            // Denied / broker Err / join 失败：占位保留（TTL 冷却）。
            None
        }
    }
}
