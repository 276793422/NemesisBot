//! B（2026-09-23 多会话并行清账）：会话绑定注册表 ——「目标语义键 → 会话」
//! 的服务端真相源。
//!
//! 背景：工作流「对话生成」等场景需要「目标（如工作流名）↔ 专用会话」的
//! 稳定绑定。此前绑定存浏览器 localStorage（`nemesisbot_wf_agentGen_sids`）：
//! 每浏览器各自为政 + 本地列表误判即重建（fetchList 在飞竞态 / 5s 缓存 /
//! 乐观行被整表替换冲掉）→ 实测 55 秒复制 5 个同目标空会话（BUG
//! `2026-09-23_workflow-agentgen-session-crosstalk` §三）。真相源上移服务端：
//!
//! - `sessions.create` 带可选 `binding_key` → **原子 get-or-create**：key
//!   已绑且会话存活 → 直接返回既有会话（零新建）；key 未绑或绑定陈旧 →
//!   新建会话并写入绑定。全程持进程锁，并发 create 同 key 只产一个会话。
//! - `sessions.set_binding` → 重绑（键 upsert，旧会话自动让出键）。
//! - `sessions.list` 回带 `bindings`（仅存活绑定）——前端删掉本地映射，
//!   零本地状态。
//! - `sessions.delete` 级联 `remove_session`（键摘除，不留悬空）。
//!
//! 存储：`<workspace>/data/session_bindings.json`（与 session_shares.json
//! 同层同模式：serde + tmp+rename 原子落盘）。存活判定**纯 workspace
//! 派生**（jsonl 或 meta 侧车任一在），不走 `default_path_manager()` 全局
//! 单例——测试重定向 home 到临时目录即可全链路隔离，不碰真实 home。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 单条绑定：key → 裸 sid（与 sessions.list 回给客户端的 id 同形）。
/// 只存坐标不存内容（标题实时解析，与 share 同语义）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionBinding {
    pub session_id: String,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Default)]
struct BindingFile {
    #[serde(default)]
    bindings: BTreeMap<String, SessionBinding>,
}

/// 进程内串行化 read-modify-write。命令处理全程无 await（同步文件写），
/// 短临界区持锁安全；毒化锁按「上一写入方半途崩溃」处理（诚实报错）。
static BINDING_LOCK: Mutex<()> = Mutex::new(());

fn bindings_path(workspace: &str) -> PathBuf {
    Path::new(workspace)
        .join("data")
        .join("session_bindings.json")
}

fn load_bindings(workspace: &str) -> BTreeMap<String, SessionBinding> {
    match std::fs::read_to_string(bindings_path(workspace)) {
        Ok(s) => {
            serde_json::from_str::<BindingFile>(&s)
                .unwrap_or_default()
                .bindings
        }
        Err(_) => BTreeMap::new(),
    }
}

/// tmp + rename 原子落盘（torn write 会砸掉全部绑定 → 空会话复制机器复发）。
fn save_bindings(
    workspace: &str,
    bindings: &BTreeMap<String, SessionBinding>,
) -> Result<(), String> {
    let path = bindings_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 data 目录失败: {e}"))?;
    }
    let body = serde_json::to_string_pretty(&BindingFile {
        bindings: bindings.clone(),
    })
    .map_err(|e| format!("序列化绑定失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("写绑定失败: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("替换绑定文件失败: {e}"))?;
    Ok(())
}

/// binding_key 规范化：trim + 非空 + 长度上限。key 只进 JSON map（不进
/// 文件路径），无消毒面；上限防垃圾键膨胀。
fn normalize_key(key: &str) -> Result<String, String> {
    let k = key.trim();
    if k.is_empty() {
        return Err("binding_key 为空".to_string());
    }
    if k.len() > 200 {
        return Err("binding_key 过长（>200 字符）".to_string());
    }
    Ok(k.to_string())
}

/// 会话存活判定：jsonl（言过的会话）或 meta 侧车（create 即写，言前）任一
/// 在即算存活。纯 workspace 派生路径（logs/session_logs 同一拼接点），与
/// `chat_log::log_path` / `meta_path` 同 sanitize 规则，但不触全局
/// path manager——生产两处同源（workspace 由 home 派生），测试可重定向。
fn session_artifact_exists(workspace: &str, sid: &str) -> bool {
    let session_key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(sid)
    );
    let safe = nemesis_utils::sanitize::sanitize_path_segment(&session_key);
    let dir = nemesis_path::resolve_session_logs_dir_in_workspace(Path::new(workspace));
    dir.join(format!("{safe}.jsonl")).exists() || dir.join(format!("{safe}.meta.json")).exists()
}

/// get-or-create 结果。`created=false` = 幂等命中（既有会话，零新建）。
pub struct GetOrCreateOutcome {
    pub session_id: String,
    pub title: String,
    pub created: bool,
}

/// 原子 get-or-create：binding_key 已绑且会话存活 → 返回既有 sid（标题
/// 实时解析，解析不到落回调用方传入的 title——新建场景 meta 即写，永远
/// 命中传入值）；未绑 / 绑定陈旧（会话已删、home 重建——localStorage 时代
/// 的绑定腐烂自此在服务端终结）→ 新建会话并覆盖绑定。uuid 生成 + meta
/// 写 + 绑定写入在同一临界区内，并发同 key 只产一个会话。
///
/// `manual_title` 语义与 sessions.create 同源：显式命名 = 用户意志
/// （`title_manual`，自动标题永不覆盖）。
pub fn get_or_create_session(
    workspace: &str,
    binding_key: &str,
    title: &str,
    manual_title: bool,
) -> Result<GetOrCreateOutcome, String> {
    let key = normalize_key(binding_key)?;
    let _g = BINDING_LOCK
        .lock()
        .map_err(|_| "绑定注册表锁毒化".to_string())?;
    let mut bindings = load_bindings(workspace);
    if let Some(b) = bindings.get(&key) {
        if session_artifact_exists(workspace, &b.session_id) {
            let sid = b.session_id.clone();
            let t =
                crate::share::session_title(workspace, &sid).unwrap_or_else(|| title.to_string());
            return Ok(GetOrCreateOutcome {
                session_id: sid,
                title: t,
                created: false,
            });
        }
        tracing::info!(
            "[session_bindings] stale binding replaced: key={}, dead_sid={}",
            key,
            b.session_id
        );
    }
    let session_id = uuid::Uuid::new_v4().to_string();
    let session_key = format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
    );
    if manual_title {
        nemesis_agent::chat_log::write_session_meta_manual(&session_key, title);
    } else {
        nemesis_agent::chat_log::write_session_meta(&session_key, title);
    }
    bindings.insert(
        key,
        SessionBinding {
            session_id: session_id.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
        },
    );
    save_bindings(workspace, &bindings)?;
    Ok(GetOrCreateOutcome {
        session_id,
        title: title.to_string(),
        created: true,
    })
}

/// 重绑：binding_key → session_id（upsert；旧持有者自动让出键）。目标
/// 会话必须存活——绑到不存在的会话 = 制造孤儿绑定，诚实拒绝。
pub fn set_binding(workspace: &str, binding_key: &str, session_id: &str) -> Result<(), String> {
    let key = normalize_key(binding_key)?;
    let sid = nemesis_agent::session::SessionStore::sanitize_session_id(session_id);
    if !session_artifact_exists(workspace, &sid) {
        return Err(format!("会话不存在或已删除，无法绑定: {sid}"));
    }
    let _g = BINDING_LOCK
        .lock()
        .map_err(|_| "绑定注册表锁毒化".to_string())?;
    let mut bindings = load_bindings(workspace);
    bindings.insert(
        key,
        SessionBinding {
            session_id: sid.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        },
    );
    save_bindings(workspace, &bindings)
}

/// 摘除单个键（draft_apply 后释放「__new__」引导键：其会话已重绑到正式
/// 工作流名，键留着会让下次「新建工作流」落进已被接管的会话）。返回是否
/// 摘除（0 = 键本就不存在，幂等）。
pub fn remove_binding(workspace: &str, binding_key: &str) -> Result<bool, String> {
    let key = normalize_key(binding_key)?;
    let _g = BINDING_LOCK
        .lock()
        .map_err(|_| "绑定注册表锁毒化".to_string())?;
    let mut bindings = load_bindings(workspace);
    if bindings.remove(&key).is_none() {
        return Ok(false);
    }
    save_bindings(workspace, &bindings)?;
    Ok(true)
}

/// 会话删除级联：摘掉指向该会话的全部键。返回摘除数（0 = 无绑定，幂等）。
pub fn remove_session(workspace: &str, session_id: &str) -> usize {
    let sid = nemesis_agent::session::SessionStore::sanitize_session_id(session_id);
    let _g = match BINDING_LOCK.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    let mut bindings = load_bindings(workspace);
    let before = bindings.len();
    bindings.retain(|_, b| b.session_id != sid);
    let removed = before - bindings.len();
    if removed > 0 {
        let _ = save_bindings(workspace, &bindings);
    }
    removed
}

/// sessions.list 投影：仅存活绑定（key → 裸 sid）。陈旧条目不透出、也不
/// 在读路径就地清除（写路径 get-or-create 自然覆盖）——list 保持纯读。
pub fn live_bindings(workspace: &str) -> BTreeMap<String, String> {
    load_bindings(workspace)
        .into_iter()
        .filter(|(_, b)| session_artifact_exists(workspace, &b.session_id))
        .map(|(k, b)| (k, b.session_id))
        .collect()
}

#[cfg(test)]
mod tests;

// AGT 覆盖率批次（2026-09-25）：get_or_create_session 的 manual_title 臂
// （title_manual 旗标落 sidecar）+ 存活工件种子后的幂等命中。豁免（收括号
// lcov 伪零 + 锁毒化臂）见 agt_tests 头注。
#[cfg(test)]
mod agt_tests;
