//! B4（2026-10-18 凭据 vault §7）：凭据索要卡 broker——**值不 transit 通道**。
//!
//! 触发点（模型 API key 401/403、connector 必需凭据槽位缺失）发卡阻塞等
//! 裁决；Dashboard 凭据卡经 WSAPI `credential.respond` 提交值，broker
//! **先落 vault 再送裁决**——mpsc 通道载荷只有 `CredentialOutcome`（无值）。
//! 值的完整路径：用户 → WSAPI credential.respond → broker 落 vault →
//! 调用点原地重解析。broker 内存零驻留凭据值（随裁决即弃）。
//!
//! 协议纪律（同 [`crate::web_approval::WebApprovalManager`]）：
//! - **先入册后广播**——反序时消费方秒点 respond 查表落空（ET Linux 假红
//!   实录的同族时序缝）。
//! - **单点广播**：`CredentialResolved` 只由等待侧（request_credential）
//!   广播——它知道每个出口的最终结局（stored/denied/timeout）；respond 侧
//!   只送通道裁决，不广播（无双发）。
//! - **超时竞速宽限**：超时先出册记账，出册返回 None = respond 抢先 →
//!   2s 宽限收在途裁决（不广播假 timeout）；返回 Some = 超时成立。
//! - **阻塞包装**：`block_in_place` 仅当 tokio 上下文（多线程 runtime 形态
//!   不可假设，同 WebApprovalManager）；非 tokio（纯 sync 测试/线程）直接
//!   `recv_timeout`。调用方（async 侧）须用 `spawn_blocking` 包住
//!   `request_credential`（`LateInstallApprover` 同款）。
//! - respond 落库失败 → 请求放回 pending（等待者还在等，用户可修正重试，
//!   剩余窗口由等待侧的 recv_timeout 决定）。
//!
//! vault 落盘面镜像 `nemesis-web::handlers::vault`（open/unlock/create 三
//! 胶水同语义）：现开现解锁（无长驻句柄）；argon2id 模式口令取
//! `NEMESISBOT_VAULT_PASSPHRASE`；覆盖写即轮换（卡片提交 = 显式轮换意图，
//! 不再要 force 二次确认——卡面本身就是确认）。**凭据值不落日志/错误/审计**
//! （vault 纪律，只记 alias + service）。
//!
//! 装配：gateway 侧 `install()` 进静态槽（connector 等工具读取侧零接线
//! 改动，automations TRIGGER_CACHE / plugins_wasm set_installer 先例）；
//! respond 侧经 `nemesis_web::handlers::credential::set_broker` 暴露给
//! WSAPI。headless run/acp 不装配（槽空 = 触发点走原错误路径零回归）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nemesis_security::vault::{VaultMode, VaultStore};
use nemesis_types::agent::{
    AgentEvent, CredentialBroker, CredentialOutcome, CredentialRequest, CredentialResponder,
};

/// 等待中的一张凭据卡。
struct PendingEntry {
    service: String,
    key_name: String,
    created_at: Instant,
    tx: std::sync::mpsc::Sender<CredentialOutcome>,
}

/// Web 凭据索要卡 broker（gateway 形态常驻；headless 不装配）。
pub struct WebCredentialBroker {
    workspace: PathBuf,
    event_tx: Option<tokio::sync::broadcast::Sender<AgentEvent>>,
    pending: parking_lot::Mutex<HashMap<String, PendingEntry>>,
}

impl WebCredentialBroker {
    /// `workspace` 决定 vault 文件位置（`<workspace>/config/vault.enc`，
    /// nemesis-path 单一真相源）；`event_tx` 为 None 时卡片事件只进 pending
    /// 表（无 SSE 广播——测试/无 web 形态）。
    pub fn new(
        workspace: PathBuf,
        event_tx: Option<tokio::sync::broadcast::Sender<AgentEvent>>,
    ) -> Self {
        Self {
            workspace,
            event_tx,
            pending: parking_lot::Mutex::new(HashMap::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// 发起端（CredentialBroker）：发卡 + 阻塞等裁决
// ---------------------------------------------------------------------------

impl CredentialBroker for WebCredentialBroker {
    fn request_credential(&self, request: &CredentialRequest) -> Result<CredentialOutcome, String> {
        // std mpsc（非 tokio）：全同步语义，recv_timeout 是本协议的等待原语
        // （超时出册记账 + 竞速宽限都建立在它上面）。
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut map = self.pending.lock();
            // 先入册（后广播）——见模块头时序纪律。
            map.insert(
                request.request_id.clone(),
                PendingEntry {
                    service: request.service.clone(),
                    key_name: request.key_name.clone(),
                    created_at: Instant::now(),
                    tx,
                },
            );
        }
        if let Some(ref ev) = self.event_tx {
            let _ = ev.send(AgentEvent::CredentialRequested {
                request_id: request.request_id.clone(),
                service: request.service.clone(),
                key_name: request.key_name.clone(),
                reason: request.reason.clone(),
                timeout_secs: request.timeout_secs,
            });
        }

        let wait = || rx.recv_timeout(Duration::from_secs(request.timeout_secs.max(1)));
        let result = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(wait)
        } else {
            wait()
        };

        let outcome = match result {
            Ok(outcome) => outcome,
            Err(_) => {
                // 超时：先出册记账；remove 返回 None = respond 抢先 → 宽限
                // 收在途裁决（不广播假 timeout）。
                let raced = { self.pending.lock().remove(&request.request_id).is_none() };
                if raced && let Ok(outcome) = rx.recv_timeout(Duration::from_secs(2)) {
                    self.broadcast_resolved(&request.request_id, &outcome);
                    return Ok(outcome);
                }
                CredentialOutcome::Denied
            }
        };
        self.broadcast_resolved(&request.request_id, &outcome);
        Ok(outcome)
    }
}

impl WebCredentialBroker {
    /// 了结广播（单点：只由等待侧调用）。Timeout 出口也广播——前端据此摘卡。
    fn broadcast_resolved(&self, request_id: &str, outcome: &CredentialOutcome) {
        let decision = match outcome {
            CredentialOutcome::Stored => "stored",
            CredentialOutcome::Denied => "denied",
        };
        if let Some(ref ev) = self.event_tx {
            let _ = ev.send(AgentEvent::CredentialResolved {
                request_id: request_id.to_string(),
                decision: decision.to_string(),
            });
        }
    }

    /// 当前等待中的凭据卡元数据（前端 pending 面；**无值字段**）。
    pub fn pending_cards(&self) -> Vec<serde_json::Value> {
        self.pending
            .lock()
            .iter()
            .map(|(id, e)| {
                serde_json::json!({
                    "request_id": id,
                    "service": e.service,
                    "key_name": e.key_name,
                    "age_secs": e.created_at.elapsed().as_secs(),
                })
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 响应端（CredentialResponder）：落 vault + 送裁决
// ---------------------------------------------------------------------------

impl CredentialResponder for WebCredentialBroker {
    fn respond(&self, request_id: &str, value: &str) -> Result<serde_json::Value, String> {
        if value.trim().is_empty() {
            return Err("凭据值为空，已取消".to_string());
        }
        let entry = {
            let mut map = self.pending.lock();
            match map.remove(request_id) {
                Some(e) => e,
                None => return Err(format!("凭据请求不存在（已超时或已裁决）: {request_id}")),
            }
        };
        // 先落 vault 再送裁决；落库失败 → 请求放回 pending（等待者还在等，
        // 用户可修正重试）。错误文案只含 vault 侧诊断，不含值。
        match persist_credential(&self.workspace, &entry.key_name, value) {
            Ok(rotated) => {
                let _ = entry.tx.send(CredentialOutcome::Stored);
                tracing::info!(
                    "[CredentialBroker] credential stored alias={} service={} rotated={}（值不落日志）",
                    entry.key_name,
                    entry.service,
                    rotated
                );
                Ok(serde_json::json!({
                    "stored": true,
                    "key_name": entry.key_name,
                    "rotated": rotated,
                }))
            }
            Err(e) => {
                self.pending.lock().insert(request_id.to_string(), entry);
                Err(format!("凭据落库失败（请求保留，可修正后重试）: {e}"))
            }
        }
    }

    fn pending(&self) -> Vec<serde_json::Value> {
        self.pending_cards()
    }
}

// ---------------------------------------------------------------------------
// vault 落盘胶水（镜像 nemesis-web::handlers::vault 的 open/unlock/create）
// ---------------------------------------------------------------------------

/// 写一条凭据进 vault。返回是否为覆盖（轮换）。卡片提交 = 显式轮换意图，
/// 不做 force 二次确认（模块头注）。
fn persist_credential(workspace: &Path, alias: &str, value: &str) -> Result<bool, String> {
    let path = nemesis_path::resolve_vault_path_in_workspace(workspace);
    let passphrase = std::env::var("NEMESISBOT_VAULT_PASSPHRASE")
        .ok()
        .filter(|p| !p.is_empty());
    let mut store =
        if path.exists() {
            // 写路径必须真解锁——set 需要 DEK；锁死 = 诚实报错带指引。
            let mut s = VaultStore::open(&path)
                .map_err(|e| format!("vault 打开失败: {e}（文件: {}）", path.display()))?;
            if !s.is_unlocked() {
                match passphrase.as_deref() {
                    Some(pw) => s.unlock(pw).map_err(|e| {
                        format!("vault 解锁失败: {e}（检查 NEMESISBOT_VAULT_PASSPHRASE）")
                    })?,
                    None => return Err(
                        "vault 已锁定（argon2 模式）——设置 NEMESISBOT_VAULT_PASSPHRASE 后重启进程"
                            .to_string(),
                    ),
                }
            }
            s
        } else {
            create_fresh(&path, passphrase.as_deref())?
        };
    let existed = store.aliases().iter().any(|a| a == alias);
    store
        .set(alias, value, "credential-card", "凭据索要卡写入")
        .map_err(|e| format!("vault 写入失败: {e}"))?;
    store
        .save()
        .map_err(|e| format!("vault 落盘失败: {e}（文件: {}）", path.display()))?;
    Ok(existed)
}

/// 新建 vault（平台默认模式；argon2id 需要口令在场，缺失 = 诚实报错不造
/// 一个永远解不开的空文件）。
fn create_fresh(path: &Path, passphrase: Option<&str>) -> Result<VaultStore, String> {
    let mode = VaultStore::default_mode();
    if mode == VaultMode::Argon2id && passphrase.is_none() {
        return Err(format!(
            "vault 文件不存在（{}）且平台默认模式为 argon2id：创建需要 \
             NEMESISBOT_VAULT_PASSPHRASE 在场",
            path.display()
        ));
    }
    VaultStore::create(path, mode, passphrase)
        .map_err(|e| format!("创建 vault 失败: {e}（文件: {}）", path.display()))
}

// ---------------------------------------------------------------------------
// 静态槽（工具读取侧零接线；automations TRIGGER_CACHE 先例）
// ---------------------------------------------------------------------------

static BROKER: std::sync::LazyLock<parking_lot::RwLock<Option<Arc<WebCredentialBroker>>>> =
    std::sync::LazyLock::new(|| parking_lot::RwLock::new(None));

/// gateway 装配点安装（init_agent 一次性；headless 不装 = 槽空）。
pub fn install(broker: Arc<WebCredentialBroker>) {
    *BROKER.write() = Some(broker);
}

/// 测试卸载（槽空 = 触发点走原错误路径）。
#[cfg(test)]
pub fn uninstall() {
    *BROKER.write() = None;
}

/// 当前 broker（None = 未装配）。
pub fn current() -> Option<Arc<WebCredentialBroker>> {
    BROKER.read().clone()
}

/// 工具槽位便捷入口：发卡阻塞等落库。`true` = 用户已提交且已落 vault
/// （调用方原地重解析即续行）；`false` = broker 缺席/拒绝/超时/内部故障
/// （调用方走原错误路径）。同步阻塞——async 侧须 `spawn_blocking` 包裹。
pub fn request_credential_card(
    service: &str,
    key_name: &str,
    reason: &str,
    timeout_secs: u64,
) -> bool {
    let Some(broker) = current() else {
        return false;
    };
    let request = CredentialRequest {
        request_id: format!("cred-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
        service: service.to_string(),
        key_name: key_name.to_string(),
        reason: reason.to_string(),
        timeout_secs,
    };
    matches!(
        broker.request_credential(&request),
        Ok(CredentialOutcome::Stored)
    )
}

#[cfg(test)]
mod tests;
