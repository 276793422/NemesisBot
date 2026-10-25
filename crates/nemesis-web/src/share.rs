//! L4 会话分享 —— 只读分享 token 存储 + chat_log 白名单投影。
//!
//! 存储：`<workspace>/data/session_shares.json`（派生数据布局约定，与
//! models_catalog.json 同层）。token = uuid v4 simple（32 hex，122 bit 熵，
//! 不可枚举）。**token 即凭据**：`GET /api/share/{token}` 不走 dashboard
//! `verify_token`（分享链接要发给无凭据的接收方，这正是分享的存在意义）；
//! 未知 / 已撤销 / 会话已删 → 404。可随时 `sessions.share_revoke` 撤销。
//!
//! 白名单投影（安全命门）：只透出 role/content/timestamp/model，images 仅
//! 数量（绝不透出本地路径 / base64）；file_changes / checkpoint_turn /
//! cron_* 一律丢弃；非 user/assistant 行（tool/error/system）跳过。
//!
//! 诚实边界：live 视图非快照（每次 fetch 重读 jsonl，会话继续增长则分享页
//! 同步增长，页面标注「实时只读视图」）；无过期时间/密码（token 熵够 + 可
//! 撤销兜底）；无防爬限流（自托管边界）。

use crate::api_handlers::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// 单条分享记录。只存坐标（session_id），不存内容 —— 标题在 fetch/list 时
/// 实时解析（会话改名分享页跟随，与 live 语义一致）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShareEntry {
    pub token: String,
    /// 裸 sid（无 `agent_main_session_` 前缀），与 Dashboard sessions.list
    /// 回给客户端的 id 同形。
    pub session_id: String,
    pub created_at: String,
    #[serde(default)]
    pub revoked: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct ShareFile {
    #[serde(default)]
    shares: Vec<ShareEntry>,
}

/// `<workspace>/data/session_shares.json`。
fn shares_path(workspace: &str) -> PathBuf {
    std::path::Path::new(workspace)
        .join("data")
        .join("session_shares.json")
}

/// 分享注册表写串行锁——罩 load→改→save 全程（session_shares.json 是共享
/// RMW 文件，create∥create 并发各持旧快照会互丢记录；姊妹文件
/// session_bindings.json 的 BINDING_LOCK 同构先例，2026-10-13 补锁）。
/// 读路径不入锁（tmp+rename 原子性已够）。
static SHARE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn load_shares(workspace: &str) -> Vec<ShareEntry> {
    match std::fs::read_to_string(shares_path(workspace)) {
        Ok(s) => match serde_json::from_str::<ShareFile>(&s) {
            Ok(f) => f.shares,
            Err(e) => {
                // BUG-20261025-40：注册表解析失败不能静默返空——空表会被下次
                // 写整体覆盖（全部分享链接无声死亡，404 还谎称「已撤销」）。
                // 保留返空恢复语义（注册表可自然重建），但必须留痕。
                tracing::warn!(
                    error = %e,
                    "[share] session_shares.json 解析失败，按空注册表继续（下次保存会覆盖盘上残留）"
                );
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
}

/// 原子落盘（torn write 会砸掉全部分享记录）。BUG-20261025-40：换 REL-002
/// 唯一权威 helper——手搓 tmp+rename 无 sync_all，断电可落零长度目标，叠加
/// load 静默返空即整表清零链；token 即凭据 → 0o600。
fn save_shares(workspace: &str, shares: &[ShareEntry]) -> Result<(), String> {
    let path = shares_path(workspace);
    let body = serde_json::to_string_pretty(&ShareFile {
        shares: shares.to_vec(),
    })
    .map_err(|e| format!("序列化分享失败: {e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), body.as_bytes(), 0o600)
        .map_err(|e| format!("写分享失败: {e}"))
}

/// 创建（或复用）分享。同一会话已存在未撤销的分享时直接复用（幂等，
/// 避免 token 泛滥）；撤销过的不再复用 —— 撤销即作废，重新创建给新 token。
///
/// session_id 就地消毒（`sanitize_session_id`，与 sessions export/rewind
/// 臂同源）：sid 会拼进 `read_chat_log` 的文件路径（log_path 只替换 `:`），
/// 含 `/` `\` `..` 的裸 sid 可路径穿越读任意 jsonl —— 守卫放在 store API
/// 这一层（单一真相源），所有调用方自动覆盖。
pub fn create_share(workspace: &str, session_id: &str) -> Result<ShareEntry, String> {
    let session_id = nemesis_agent::session::SessionStore::sanitize_session_id(session_id);
    let _g = SHARE_LOCK
        .lock()
        .map_err(|_| "分享注册表锁毒化".to_string())?;
    let mut shares = load_shares(workspace);
    if let Some(existing) = shares
        .iter()
        .find(|s| !s.revoked && s.session_id == session_id)
    {
        return Ok(existing.clone());
    }
    let entry = ShareEntry {
        token: uuid::Uuid::new_v4().simple().to_string(),
        session_id: session_id.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        revoked: false,
    };
    shares.push(entry.clone());
    save_shares(workspace, &shares)?;
    Ok(entry)
}

/// 撤销分享。返回 false = token 不存在。
pub fn revoke_share(workspace: &str, token: &str) -> Result<bool, String> {
    let _g = SHARE_LOCK
        .lock()
        .map_err(|_| "分享注册表锁毒化".to_string())?;
    let mut shares = load_shares(workspace);
    let mut found = false;
    for s in shares.iter_mut() {
        if s.token == token {
            s.revoked = true;
            found = true;
        }
    }
    if found {
        save_shares(workspace, &shares)?;
    }
    Ok(found)
}

/// 按 token 解析有效分享（已撤销 → None）。
pub fn resolve_share(workspace: &str, token: &str) -> Option<ShareEntry> {
    load_shares(workspace)
        .into_iter()
        .find(|s| !s.revoked && s.token == token)
}

pub fn list_shares(workspace: &str) -> Vec<ShareEntry> {
    load_shares(workspace)
}

/// chat_log 行白名单投影。见模块头注释。
pub fn project_messages(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .filter(|r| matches!(r["role"].as_str(), Some("user") | Some("assistant")))
        .map(|r| {
            let mut m = serde_json::Map::new();
            m.insert("role".into(), r["role"].clone());
            m.insert("content".into(), r["content"].clone());
            m.insert("timestamp".into(), r["timestamp"].clone());
            if let Some(model) = r.get("model")
                && model.is_string()
                && !model.as_str().unwrap().is_empty()
            {
                m.insert("model".into(), model.clone());
            }
            // 图片只透数量 —— 本地路径/base64 绝不出域。
            if let Some(imgs) = r.get("images").and_then(|v| v.as_array())
                && !imgs.is_empty()
            {
                m.insert("image_count".into(), Value::from(imgs.len()));
            }
            Value::Object(m)
        })
        .collect()
}

/// 实时解析会话标题（用户改名/首条消息截断，来自 sessions.list 同源
/// scan_session_logs）。会话已删 → None。share_list / handle_api_share
/// 共用（live 语义：标题跟随当前状态）。
pub fn session_title(workspace: &str, session_id: &str) -> Option<String> {
    let key = format!("agent_main_session_{session_id}");
    crate::handlers::logs::scan_session_logs(workspace)
        .into_iter()
        .find(|s| s["id"].as_str() == Some(key.as_str()))
        .and_then(|s| s["title"].as_str().map(str::to_string))
}

fn not_found(msg: &str) -> (axum::http::StatusCode, axum::Json<Value>) {
    (
        axum::http::StatusCode::NOT_FOUND,
        axum::Json(json!({ "error": msg })),
    )
}

/// axum state → workspace 路径（workspace 优先，home 回退拼 /workspace）。
fn share_workspace(state: &AppState) -> Option<String> {
    state.workspace.clone().or_else(|| {
        state.home.as_ref().map(|h| {
            std::path::Path::new(h)
                .join("workspace")
                .to_string_lossy()
                .into_owned()
        })
    })
}

/// GET /api/share/{token} —— 公开只读端点，**故意不校验 verify_token**。
pub async fn handle_api_share(
    axum::extract::Path(token): axum::extract::Path<String>,
    axum::extract::State(state): axum::extract::State<std::sync::Arc<AppState>>,
) -> Result<axum::Json<Value>, (axum::http::StatusCode, axum::Json<Value>)> {
    let Some(workspace) = share_workspace(&state) else {
        return Err(not_found("服务未配置工作区"));
    };
    let Some(entry) = resolve_share(&workspace, &token) else {
        return Err(not_found("分享不存在或已撤销"));
    };
    let key = format!("agent:main:session:{}", entry.session_id);
    let (rows, _total, _more, _oldest) =
        nemesis_agent::chat_log::read_chat_log(&key, usize::MAX, None);
    if rows.is_empty() {
        return Err(not_found("会话不存在或历史为空"));
    }
    let title =
        session_title(&workspace, &entry.session_id).unwrap_or_else(|| "会话分享".to_string());
    Ok(axum::Json(json!({
        "title": title,
        "created_at": entry.created_at,
        "view": "live",
        "messages": project_messages(&rows),
    })))
}

#[cfg(test)]
mod tests;
