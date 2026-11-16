use super::*;

const H: u64 = 3_600_000;
/// 时间轴基点（取大值避免与真实 epoch 相差过远，语义一致）。
const B: u64 = 1_000_000_000;

#[test]
fn cooldown_zero_is_open_gate() {
    let mut g = CooldownGate::new();
    g.record(1000);
    assert!(g.allow(1001, 0)); // 0 = 关闭（无视 last）
    let fresh = CooldownGate::new();
    assert!(fresh.allow(1001, 10)); // 首次无冷却
    let mut g2 = CooldownGate::new();
    g2.record(1000);
    assert!(!g2.allow(1001, 10)); // 10s 冷却未到 → 拒
}

#[test]
fn cooldown_blocks_within_window_and_opens_after() {
    let mut g = CooldownGate::new();
    g.record(1000);
    assert!(!g.allow(1000 + 60_000 - 1, 60)); // 差 1ms 满 60s
    assert!(g.allow(1000 + 60_000, 60)); // 恰满 = 开
}

#[test]
fn hour_window_respects_cap_and_slides() {
    let mut w = HourWindow::new();
    assert!(w.allow(B, 2));
    w.record(B);
    assert!(w.allow(B + 1, 2));
    w.record(B + 1);
    assert!(!w.allow(B + 2, 2)); // 满员
    assert!(!w.allow(B + H - 1, 2)); // 两条都还在窗内（<1h）
    // 窗口滑动：恰满 1h 后两条出窗（B+1 差值恰 = H），名额腾空
    assert!(w.allow(B + H + 1, 2));
    w.record(B + H + 1);
    assert_eq!(w.len(), 1);
    w.record(B + H + 2);
    assert_eq!(w.len(), 2);
    assert!(!w.allow(B + H + 3, 2)); // 又满员（窗内两条 diff=2/1ms）
}

#[test]
fn hour_window_boundary_exactly_one_hour_old_expires() {
    let mut w = HourWindow::new();
    w.record(B);
    assert!(!w.allow(B + H - 1, 1)); // 差 1ms 满 1h → 仍在窗
    assert!(w.allow(B + H, 1)); // 恰满 1h = 出窗
}

#[test]
fn hour_window_zero_is_unlimited() {
    let mut w = HourWindow::new();
    for i in 0..100u64 {
        assert!(w.allow(B + i, 0));
        w.record(B + i);
    }
    assert_eq!(w.len(), 100);
    assert!(!w.is_empty());
}

#[test]
fn merge_buffer_flushes_after_quiet_window_and_zero_is_immediate() {
    let mut b = MergeBuffer::new(1000);
    b.push("a.txt", 1100);
    assert!(!b.should_flush(1100 + 5_000 - 1, 5));
    assert!(b.should_flush(1100 + 5_000, 5));
    assert!(b.should_flush(9999, 0)); // 0 = 立即投
}

#[test]
fn merge_buffer_counts_all_but_lists_capped() {
    let mut b = MergeBuffer::new(0);
    for i in 0..(MAX_PATHS_LISTED as u32 + 5) {
        b.push(&format!("f{i}.txt"), i as u64);
    }
    assert_eq!(b.count(), MAX_PATHS_LISTED as u32 + 5);
    assert_eq!(b.paths().len(), MAX_PATHS_LISTED);
}

#[test]
fn merge_buffer_duplicate_folds_same_path_within_debounce() {
    let mut b = MergeBuffer::new(1000);
    assert!(!b.duplicate("a.txt", 1000, 500)); // 首见不折叠
    b.push("a.txt", 1000);
    // duplicate 命中即滑动刷新 last_seen（持续写盘一直折叠）
    assert!(b.duplicate("a.txt", 1200, 500)); // 200ms 内同路径 → 折叠（刷至 1200）
    b.push("a.txt", 1400);
    assert!(b.duplicate("a.txt", 1500, 500)); // 100ms → 折叠（刷至 1500）
    assert!(!b.duplicate("a.txt", 2001, 500)); // 501ms → 不折叠
    assert!(!b.duplicate("b.txt", 2001, 500)); // 不同路径不折叠
    assert_eq!(b.count(), 2); // 只有两次 push 计数（duplicate true 的调用方跳过 push）
}

#[test]
fn merge_buffer_debounce_zero_disables_folding() {
    let mut b = MergeBuffer::new(0);
    assert!(!b.duplicate("a.txt", 0, 0));
    b.push("a.txt", 0);
    assert!(!b.duplicate("a.txt", 1, 0)); // 0 = 不折叠
    b.push("a.txt", 1);
    assert_eq!(b.count(), 2);
}
