//! spawn park 注册表（会话流过程语言整改 2026-11-19）。
//!
//! 后台子代理 spawn 后主 turn 不再伪造 Done 收口，而是 park 在工具
//! 调用上等待真实完成（`park_bg_spawn`）。本模块是被 park 等待的任务
//! 的投递信箱：
//!
//! - spawn 闭包在 `tokio::spawn` **之前** [`ParkRegistry::register`]
//!   （先于任何可能的完成，杜绝「完成回灌先到、park 还没挂上」竞态
//!   ——该竞态若存在会双丢：快照未存 + slot 未挂，完成帧无处落地）；
//! - turn 侧 organ 8b [`ParkRegistry::wait`] 取 oneshot rx，
//!   select 完成 / estop / cancel 三臂；
//! - 完成回灌（`run_continuation_turn` 顶部 park 分支，先于 load/busy
//!   判定——parked turn 持 session busy guard，走到底下 try_acquire
//!   必死锁：投递入队→回合不结束→队列永不 drain）[`ParkRegistry::deliver`]
//!   投递 parcel 给等待者；等待者已死（estop 打断）或本无等待（集群
//!   peer_chat 等非 park 任务）→ 落回 legacy 快照续行语义。
//!
//! 全局单例；条目数 = 在飞后台 spawn 数（个位数量级），`Mutex<HashMap>`
//! 足够（临界区内零 await）。条目生命周期：deliver / mark_cancelled
//! 时摘除；进程重启自然清零——park 是进程内概念，跨重启由 legacy
//! 快照恢复路径接管（adapters 启动期诚实丢失注入 + estop 释放恢复）。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use tokio::sync::oneshot;

/// 完成投递物（`run_continuation_turn` park 分支构造；三值与完成帧
/// metadata/content 同源）。
pub struct ParkParcel {
    pub content: String,
    pub failed: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParkState {
    /// 有 turn 在等（或即将等——tx 由 `wait` 挂上）。
    Waiting,
    /// 等待 turn 已被用户取消——完成回灌不得复活该任务。
    Cancelled,
}

struct ParkSlot {
    state: ParkState,
    tx: Option<oneshot::Sender<ParkParcel>>,
    session_key: String,
    chat_id: String,
}

/// deliver 的结论（`run_continuation_turn` park 分支据此分臂）。
#[derive(Debug)]
pub enum DeliverOutcome {
    /// parcel 已交等待者（其原地覆写 marker 结果回灌 LLM）。
    Delivered {
        session_key: String,
        chat_id: String,
    },
    /// 等待 turn 已被用户取消——结果丢弃、绝不续行（快照由调用方回收）。
    CancelledNoResume,
    /// 等待者已死（estop 打断 park / 等待 future 被放弃）——落回
    /// legacy 快照续行（estop 释放恢复 = 旧世界语义）。
    WaiterGone,
    /// 本任务从未 park（集群 peer_chat / 重启注入等）——legacy 原样。
    NotParked,
}

/// 全局 park 注册表（keyed by bg task_id）。
#[derive(Default)]
pub struct ParkRegistry(Mutex<HashMap<String, ParkSlot>>);

static PARK: OnceLock<ParkRegistry> = OnceLock::new();

pub fn global() -> &'static ParkRegistry {
    PARK.get_or_init(Default::default)
}

impl ParkRegistry {
    /// spawn 闭包注册（**必须**在 `tokio::spawn` 之前，见模块头竞态
    /// 论证）。同 id 重复注册覆盖（理论不可达：task_id 含毫秒+计数器）。
    pub fn register(&self, task_id: &str, session_key: &str, chat_id: &str) {
        let mut reg = self.0.lock().unwrap();
        reg.insert(
            task_id.to_string(),
            ParkSlot {
                state: ParkState::Waiting,
                tx: None,
                session_key: session_key.to_string(),
                chat_id: chat_id.to_string(),
            },
        );
    }

    /// turn 侧取等待 rx。返回 `None` = slot 缺席 / 已被消费 / 已取消
    /// （异常装配）——调用方落回 marker 即返的旧世界形态，完成走
    /// legacy 快照续行。
    pub fn wait(&self, task_id: &str) -> Option<oneshot::Receiver<ParkParcel>> {
        let mut reg = self.0.lock().unwrap();
        let slot = reg.get_mut(task_id)?;
        if slot.state != ParkState::Waiting {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        slot.tx = Some(tx);
        Some(rx)
    }

    /// 用户取消等待 turn：置 Cancelled + 弃 tx（若有）。此后 deliver
    /// 恒 [`DeliverOutcome::CancelledNoResume`]——完成回灌不复活已
    /// 取消的任务。
    pub fn mark_cancelled(&self, task_id: &str) {
        let mut reg = self.0.lock().unwrap();
        if let Some(slot) = reg.get_mut(task_id) {
            slot.state = ParkState::Cancelled;
            slot.tx = None;
        }
    }

    /// 完成回灌投递（`run_continuation_turn` park 分支）。结论见
    /// [`DeliverOutcome`]；Delivered / CancelledNoResume 条目即时摘除；
    /// WaiterGone 同样摘除（不再有人会等；任务结果改走 legacy 快照
    /// 续行）。
    pub fn deliver(&self, task_id: &str, parcel: ParkParcel) -> DeliverOutcome {
        let mut reg = self.0.lock().unwrap();
        let Some(slot) = reg.get_mut(task_id) else {
            return DeliverOutcome::NotParked;
        };
        match slot.state {
            ParkState::Cancelled => {
                reg.remove(task_id);
                DeliverOutcome::CancelledNoResume
            }
            ParkState::Waiting => {
                let session_key = slot.session_key.clone();
                let chat_id = slot.chat_id.clone();
                match slot.tx.take() {
                    Some(tx) => {
                        if tx.send(parcel).is_ok() {
                            reg.remove(task_id);
                            DeliverOutcome::Delivered {
                                session_key,
                                chat_id,
                            }
                        } else {
                            // rx 已弃（estop 打断的 park / 等待 future 被
                            // 放弃）→ 等待者不在。
                            reg.remove(task_id);
                            DeliverOutcome::WaiterGone
                        }
                    }
                    // 无 tx（等待者从未挂上）→ 等待者不在。
                    None => {
                        reg.remove(task_id);
                        DeliverOutcome::WaiterGone
                    }
                }
            }
        }
    }

    /// 测试/诊断面：条目是否在场。
    pub fn contains(&self, task_id: &str) -> bool {
        self.0.lock().unwrap().contains_key(task_id)
    }
}

/// A2 到达行（过程语言：✅/❌ + 单行摘录；经 `emit_round_output`
/// 随 work block 落盘 + RoundText 事件双路）。失败优先 error 原文，
/// 成功摘首个非空行。
pub fn arrival_line(failed: bool, content: &str, error: Option<&str>) -> String {
    let excerpt = |s: &str| -> String {
        let first = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let mut t: String = first.chars().take(160).collect();
        if first.chars().count() > 160 {
            t.push('…');
        }
        t
    };
    if failed {
        format!(
            "❌ 后台子代理失败：{}",
            excerpt(error.filter(|e| !e.trim().is_empty()).unwrap_or(content))
        )
    } else {
        format!("✅ 后台子代理返回：{}", excerpt(content))
    }
}

#[cfg(test)]
mod tests;
