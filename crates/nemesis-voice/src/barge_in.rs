//! Barge-in decision state machine — realtime P1（G4）
//!
//! 用户在 TTS 播放中开口 → 打断播放 + 取消 agent 当前轮。触发判定拆成
//! 纯逻辑（本模块，可单测）与执行（handlers/voice.rs 管线，碰全局态）：
//!
//! - **onset 沿**：VAD `is_speaking` false→true 才开始武装（持续在说话不算新 onset）
//! - **最短持续**：onset 起持续 ≥ `min_sustain` 才 armed（咳嗽/桌响防误触）
//! - **播放窗口**：TTS 正在播放，或刚停 ≤ `tail`（尾窗内用户抢话同样打断）
//!
//! AEC 装载检查与声纹校验在执行层（要碰全局引擎状态），不在本机。

use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct BargeInWatch {
    was_speaking: bool,
    onset_at: Option<Instant>,
    min_sustain: Duration,
}

impl BargeInWatch {
    pub fn new(min_sustain: Duration) -> Self {
        Self {
            was_speaking: false,
            onset_at: None,
            min_sustain,
        }
    }

    /// Feed the current VAD state. Returns `true` exactly on the rising edge
    /// (previous sample silent, now speaking) — the caller starts audio
    /// accumulation for speaker verification at that point.
    pub fn onset(&mut self, speaking: bool) -> bool {
        let rising = speaking && !self.was_speaking;
        if rising {
            self.onset_at = Some(Instant::now());
        }
        self.was_speaking = speaking;
        rising
    }

    /// Onset recorded and sustained for at least `min_sustain`.
    pub fn sustained(&self) -> bool {
        self.onset_at
            .is_some_and(|t| t.elapsed() >= self.min_sustain)
    }

    /// Clear armed state after a fired trigger (or manual cancel).
    pub fn reset(&mut self) {
        self.onset_at = None;
        self.was_speaking = false;
    }

    /// Whether a barge-in is currently admissible by the playback window:
    /// TTS actively playing, or stopped within `tail` (users cut in on the
    /// last syllable too). Never played = closed.
    pub fn playback_window_open(
        playing: bool,
        last_play_end: Option<Instant>,
        tail: Duration,
    ) -> bool {
        playing || last_play_end.is_some_and(|t| t.elapsed() <= tail)
    }
}

#[cfg(test)]
mod barge_in_tests;
