//! （2026-11-02）：chat.send 写入侧幂等键（去重索引）。
//!
//! 同一消息因网络重试 / 多端并发重复提交：`(session_key, client_msg_id)`
//! 命中去重索引 → **不重复入环、不二次触发 agent**（server.rs 入站咽喉点
//! 短路 bus 转发），回声帧带**原 seq + `deduped:true`**（游标推进语义与
//! 首投一致；前端尾行同文守卫自然不重复渲染）。`client_msg_id` 不带或空
//! = 旧行为零变化。
//!
//! 结构：内存 LRU（per-session cap [`DEDUP_CAP`]，超限最旧逐出——幂等窗口
//! 有限是可接受语义，防的是重试风暴而非永久唯一性约束，文档口径）+
//! sidecar 落盘 `logs/session_logs/<safe_key>.dedup.json`（首触碰惰性加载，
//! 重启不失）。未安装 workspace（单测 / 装配前）= 纯内存模式，诚实降级。
//!
//! 会话表沿用 chat_event_log 同款 FIFO 逐出纪律（[`MAX_SESSIONS`] 上限，
//! victim = 最老插入的非本会话条目；被逐会话只是失去幂等窗口）。
//!
//! 纯图消息（content 空带 media， 语义不入环不回声）：幂等键照常登记，
//! seq 记 0 标记「无环行」——重复提交仍被去重短路，回声跳过。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

/// per-session 幂等窗口容量（client_msg_id → seq 条目数，超限最旧逐出）。
pub const DEDUP_CAP: usize = 128;
/// 会话表 FIFO 上限（同 chat_event_log MAX_SESSIONS 纪律）。
const MAX_SESSIONS: usize = 256;

/// sidecar 落盘根（`<workspace>/logs/session_logs/`）。RwLock 而非 OnceLock：
/// 进程级槽 + 测试需要换 workspace（OnceLock 设一次即死全局）。
static WORKSPACE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// gateway 装配期安装 sidecar 根（web_init，install_event_hub 同层装配点）。
/// 未安装 = 纯内存模式（重启后窗口清零，诚实降级不报错）。
pub fn install_workspace(workspace_dir: &Path) {
    *WORKSPACE
        .write()
        .expect("chat_dedup workspace lock poisoned") = Some(workspace_dir.to_path_buf());
}

/// `(client_msg_id, seq)` 有序对：back = 最新。LRU 语义 = lookup 命中移
/// back、insert 移 back、超 cap 逐 front。
struct DedupEntry {
    items: VecDeque<(String, u64)>,
    /// 惰性加载旗：首触碰（lookup/insert）读 sidecar 一次；加载失败不重试
    /// （损坏窗口 = 空窗口 + warn，诚实降级）。
    loaded: bool,
}

/// 会话表 + 插入序（FIFO 逐出用，同 chat_event_log 纪律）。
#[derive(Default)]
struct DedupTable {
    map: HashMap<String, DedupEntry>,
    order: VecDeque<String>,
}

static TABLE: OnceLock<Mutex<DedupTable>> = OnceLock::new();

fn table() -> &'static Mutex<DedupTable> {
    TABLE.get_or_init(|| {
        Mutex::new(DedupTable {
            map: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

fn workspace_dir() -> Option<PathBuf> {
    WORKSPACE
        .read()
        .expect("chat_dedup workspace lock poisoned")
        .clone()
}

fn sidecar_path(workspace: &Path, session_key: &str) -> PathBuf {
    // SAN-05 同款：白名单消毒单一真相源（todo 文件同构路径族）。
    let safe = nemesis_utils::sanitize::sanitize_path_segment(session_key);
    nemesis_path::resolve_sessions_dir_in_workspace(workspace).join(format!("{safe}.dedup.json"))
}

/// sidecar 惰性加载（首触碰一次）。文件缺失 = 空窗口（新会话常态）；
/// 损坏 = 空窗口 + warn（保留内存态继续服务，不炸入站咽喉点）。
fn load_sidecar(workspace: &Path, session_key: &str, entry: &mut DedupEntry) {
    entry.loaded = true;
    let path = sidecar_path(workspace, session_key);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return; // 缺失 = 新会话，静默
    };
    #[derive(serde::Deserialize)]
    struct Sidecar {
        #[serde(default)]
        items: Vec<(String, u64)>,
    }
    match serde_json::from_str::<Sidecar>(&body) {
        Ok(sc) => {
            entry.items = sc.items.into_iter().collect();
            // 落盘超 cap（老版本更大 cap / 手工编辑）：加载即收敛。
            while entry.items.len() > DEDUP_CAP {
                entry.items.pop_front();
            }
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "[chat_dedup] corrupt sidecar; starting with empty idempotency window"
            );
            entry.items.clear();
        }
    }
}

/// sidecar 整体重写（insert 后调用；条目 ≤128，小 JSON）。失败 = warn 保留
/// 内存态（重启后窗口缺这几条 = 幂等窗口短暂收窄，可接受）。
fn persist(workspace: &Path, session_key: &str, items: &VecDeque<(String, u64)>) {
    let path = sidecar_path(workspace, session_key);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(path = %path.display(), error = %e, "[chat_dedup] sidecar dir create failed");
        return;
    }
    let body = serde_json::json!({ "v": 1, "items": items });
    if let Err(e) = std::fs::write(&path, body.to_string()) {
        tracing::warn!(path = %path.display(), error = %e, "[chat_dedup] sidecar write failed");
    }
}

/// 建条目（若缺）+ 新会话登记插入序 + FIFO 逐出（victim = 最老插入的非本
/// 会话条目）。lookup 与 insert 共用——BUG-20261106-021 之前只有 insert
/// 登记 order/逐出，而生产路径恒为 lookup→insert（server.rs 入站咽喉点），
/// insert 的 `created` 判定恒 false：`MAX_SESSIONS` 上限对全部生产路径
/// 失效，map 与 sidecar 文件随会话累积无界增长。收口后两入口同纪律，
/// map 与 order 恢复「始终同键集」不变量。
fn ensure_entry(t: &mut DedupTable, session_key: &str) {
    if t.map.contains_key(session_key) {
        return;
    }
    t.map.insert(
        session_key.to_string(),
        DedupEntry {
            items: VecDeque::new(),
            loaded: false,
        },
    );
    t.order.push_back(session_key.to_string());
    if t.map.len() > MAX_SESSIONS {
        let victim_idx = t
            .order
            .iter()
            .position(|k| k.as_str() != session_key && t.map.contains_key(k.as_str()));
        if let Some(idx) = victim_idx {
            let key = t.order.remove(idx).expect("idx from position() is valid");
            t.map.remove(&key);
        }
    }
}

/// 查重：命中返回**原 seq**（0 = 纯图消息无环行）并做 LRU touch；未命中
/// 返回 None。首触碰惰性加载 sidecar（重启不失的读路径）。
pub fn lookup(session_key: &str, client_msg_id: &str) -> Option<u64> {
    let mut t = table().lock().expect("chat_dedup table lock poisoned");
    ensure_entry(&mut t, session_key);
    let seq = {
        let entry = t
            .map
            .get_mut(session_key)
            .expect("ensure_entry guarantees presence");
        if !entry.loaded {
            if let Some(ws) = workspace_dir() {
                load_sidecar(&ws, session_key, entry);
            } else {
                entry.loaded = true;
            }
        }
        entry
            .items
            .iter()
            .position(|(id, _)| id == client_msg_id)
            .map(|pos| {
                entry
                    .items
                    .remove(pos)
                    .expect("pos from position() is valid")
                    .1
            })
    };
    if let Some(seq) = seq {
        // LRU touch：命中移 back（entry 借用已随内层块结束释放）。
        let entry = t.map.get_mut(session_key).expect("just ensured");
        entry.items.push_back((client_msg_id.to_string(), seq));
    }
    seq
}

/// 登记：user 行入环后调用（seq = chat_event_log 分配的原 seq；纯图消息
/// 无环行记 0）。cap 超限逐最旧；落盘尽力而为。
pub fn insert(session_key: &str, client_msg_id: &str, seq: u64) {
    let snapshot: Vec<(String, u64)> = {
        let mut t = table().lock().expect("chat_dedup table lock poisoned");
        ensure_entry(&mut t, session_key);
        let snapshot;
        {
            let entry = t
                .map
                .get_mut(session_key)
                .expect("ensure_entry guarantees presence");
            if !entry.loaded {
                if let Some(ws) = workspace_dir() {
                    load_sidecar(&ws, session_key, entry);
                } else {
                    entry.loaded = true;
                }
            }
            // 同 id 二连插（不该发生——调用方先 lookup 后 insert；防御性：
            // 旧值顶替移 back，不产生重复条目）。
            if let Some(pos) = entry.items.iter().position(|(id, _)| id == client_msg_id) {
                entry.items.remove(pos);
            }
            entry.items.push_back((client_msg_id.to_string(), seq));
            while entry.items.len() > DEDUP_CAP {
                entry.items.pop_front();
            }
            snapshot = entry.items.iter().cloned().collect();
        }
        snapshot
    };
    // 落盘前放锁（写盘是慢操作，不持全局锁）。
    if let Some(ws) = workspace_dir() {
        persist(&ws, session_key, &VecDeque::from(snapshot));
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// 测试专用串行闸：进程级全局表 + workspace 槽，并行测试共用
    /// （chat_event_log GLOBAL_TABLE_LOCK 同款纪律）。
    pub static GLOBAL_TABLE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// 测试专用 workspace 槽改写（None = 纯内存模式）。
    pub fn set_workspace_for_tests(dir: Option<PathBuf>) {
        *WORKSPACE.write().unwrap() = dir;
    }

    /// 测试专用清表 + 清 workspace 槽（配合 set_workspace_for_tests(None)）。
    pub fn reset_for_tests() {
        let mut t = table().lock().unwrap();
        t.map.clear();
        t.order.clear();
        drop(t);
        set_workspace_for_tests(None);
    }

    /// 测试专用：只清表内容（workspace 槽保留）——持久化测试清内存表模拟
    /// 重启。字段私有，清理由本模块内聚完成。
    pub fn clear_table_only() {
        let mut t = table().lock().unwrap();
        t.map.clear();
        t.order.clear();
    }

    /// 测试探针：map 大小（MAX_SESSIONS 逐出纪律回归锁用）。
    pub fn table_size_for_tests() -> usize {
        table().lock().unwrap().map.len()
    }

    /// 测试探针：order 是否含指定键（map/order 同键集不变量回归锁用）。
    pub fn order_contains_for_tests(session_key: &str) -> bool {
        table()
            .lock()
            .unwrap()
            .order
            .iter()
            .any(|k| k.as_str() == session_key)
    }
}

#[cfg(test)]
mod tests;
