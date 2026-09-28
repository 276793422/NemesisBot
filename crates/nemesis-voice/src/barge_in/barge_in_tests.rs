//! barge_in 纯逻辑测试（onset 沿 / 最短持续 / 播放窗口）。

use super::*;
use std::thread::sleep;

#[test]
fn onset_fires_only_on_rising_edge() {
    let mut w = BargeInWatch::new(Duration::from_millis(0));
    assert!(w.onset(true), "silent→speaking 是 onset");
    assert!(!w.onset(true), "持续说话不是新 onset");
    assert!(!w.onset(false));
    assert!(w.onset(true), "再次开口是 onset");
}

#[test]
fn sustained_requires_min_duration() {
    let mut w = BargeInWatch::new(Duration::from_millis(250));
    assert!(w.onset(true));
    assert!(!w.sustained(), "刚 onset 未到最短持续");
    sleep(Duration::from_millis(280));
    assert!(w.sustained(), "超过 250ms 后 armed");
}

#[test]
fn no_onset_never_sustained() {
    let mut w = BargeInWatch::new(Duration::from_millis(0));
    assert!(!w.sustained(), "从未 onset 不会 armed");
    let _ = w.onset(false);
    assert!(!w.sustained());
}

#[test]
fn reset_clears_armed_state() {
    let mut w = BargeInWatch::new(Duration::from_millis(0));
    let _ = w.onset(true);
    assert!(w.sustained());
    w.reset();
    assert!(!w.sustained(), "reset 后不再 armed");
}

#[test]
fn playback_window_logic() {
    // 从未播放 → 关
    assert!(!BargeInWatch::playback_window_open(
        false,
        None,
        Duration::from_millis(300)
    ));
    // 正在播放 → 开
    assert!(BargeInWatch::playback_window_open(
        true,
        None,
        Duration::from_millis(300)
    ));
    // 100ms 前停 → 开（尾窗）
    let recent = Instant::now() - Duration::from_millis(100);
    assert!(BargeInWatch::playback_window_open(
        false,
        Some(recent),
        Duration::from_millis(300)
    ));
    // 500ms 前停 → 关
    let old = Instant::now() - Duration::from_millis(500);
    assert!(!BargeInWatch::playback_window_open(
        false,
        Some(old),
        Duration::from_millis(300)
    ));
}

#[test]
fn full_trigger_flow() {
    // 模拟真实时序：播放中 onset → 持续到 250ms → 窗口开 → 触发 → reset
    let mut w = BargeInWatch::new(Duration::from_millis(50));
    assert!(w.onset(true));
    sleep(Duration::from_millis(60));
    assert!(w.sustained());
    assert!(BargeInWatch::playback_window_open(
        true,
        None,
        Duration::from_millis(300)
    ));
    w.reset();
    assert!(!w.sustained(), "触发后 reset 防重复");
}
