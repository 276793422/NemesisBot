//! BUG-20261115-011 回归锁：cluster_task_detail 多轮执行必须取最新轮。
//!
//! 同 task_id 多轮执行 = `cluster_logs/{device}/{ts}_{task_id}` 多个目录
//! （重派/重试场景，outbox `find_task_dir` 注释「多轮执行取字典序最大 =
//! 最新」为同场景既有纪律）。旧实现以 first-match 消费**无序** read_dir：
//! Windows（NTFS 字典序）恒取最旧轮、ext4 任意轮，与 cluster_task_list 的
//! ts 倒序逐轮展示相矛盾——列表里点最新轮，详情页显示最旧轮。修复 = 候选
//! 按目录名降序（ts 前缀定宽，字典序 = 时间序）后再 perspective 过滤与
//! first() 兜底。本文件钉三臂：同设备多轮取最新 / local perspective 取
//! 本地最新轮（peer 有更新轮不干扰）/ peer perspective 在 peer 候选内取
//! 最新。

use super::*;

fn detail_round_dir(ws: &str, device: &str, dir_name: &str) {
    let dir = cluster_log_dir(ws).join(device).join(dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("00.request.md"), "## Message\nround\n").unwrap();
}

/// 同设备多轮：必须返回最新轮（旧实现 NTFS 字典序 first-match 恒取最旧）。
#[test]
fn detail_picks_newest_round_same_device() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    detail_round_dir(ws, "devA", "2026-08-28_07-00-00_t1");
    detail_round_dir(ws, "devA", "2026-08-28_08-30-00_t1");
    detail_round_dir(ws, "devA", "2026-08-28_07-45-00_t1");

    let out = LogsHandler
        .cluster_task_detail(None, ws, "t1", "")
        .unwrap()
        .unwrap();
    assert_eq!(
        out["timestamp"], "2026-08-28_08-30-00",
        "多轮必须取最新轮（旧实现 NTFS 恒取最旧轮）: {out}"
    );
}

/// local perspective：peer 设备有更新轮也不干扰——取本地候选中的最新轮。
#[test]
fn detail_local_perspective_prefers_local_newest_round() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    detail_round_dir(ws, "peerB", "2026-08-28_09-00-00_t1");
    detail_round_dir(ws, "devLocal", "2026-08-28_07-00-00_t1");
    detail_round_dir(ws, "devLocal", "2026-08-28_07-30-00_t1");

    let out = LogsHandler
        .cluster_task_detail(Some("devLocal".into()), ws, "t1", "local")
        .unwrap()
        .unwrap();
    assert_eq!(
        out["timestamp"], "2026-08-28_07-30-00",
        "local perspective 取本地候选中的最新轮: {out}"
    );
    assert_eq!(
        out["direction"], "outbound",
        "local == device → outbound（语义回归面）"
    );
}

/// peer perspective：peer 候选内取最新轮；本地候选更新也不可入选。
#[test]
fn detail_peer_perspective_picks_newest_peer_round() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    detail_round_dir(ws, "peerA", "2026-08-28_07-00-00_t1");
    detail_round_dir(ws, "peerB", "2026-08-28_08-00-00_t1");
    detail_round_dir(ws, "peerB", "2026-08-28_07-30-00_t1");
    detail_round_dir(ws, "devLocal", "2026-08-28_09-30-00_t1");

    let out = LogsHandler
        .cluster_task_detail(Some("devLocal".into()), ws, "t1", "peer")
        .unwrap()
        .unwrap();
    assert_eq!(
        out["timestamp"], "2026-08-28_08-00-00",
        "peer perspective 在 peer 候选里取最新轮: {out}"
    );
    assert_eq!(
        out["peerNode"], "peerB",
        "inbound 方向 peerNode = 来源设备（语义回归面）"
    );
}
