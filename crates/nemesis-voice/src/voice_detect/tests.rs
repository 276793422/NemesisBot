use super::*;
use std::time::Duration;

#[test]
fn rms_new_initializes_not_speaking() {
    let d = RmsVoiceDetector::new(0.01, 300, 15_000);
    assert!(!d.is_speaking());
    assert_eq!(d.name(), "RMS");
}

#[test]
fn rms_process_empty_chunk_returns_none() {
    let mut d = RmsVoiceDetector::new(0.01, 300, 15_000);
    let out = d.process(&[], 16000);
    assert!(out.is_none());
    assert!(!d.is_speaking());
}

#[test]
fn rms_process_silent_chunk_does_not_start_speaking() {
    let mut d = RmsVoiceDetector::new(0.5, 300, 15_000);
    // Pure silence — RMS = 0, well below threshold 0.5
    let chunk = vec![0.0_f32; 512];
    let out = d.process(&chunk, 16000);
    assert!(out.is_none());
    assert!(!d.is_speaking());
}

#[test]
fn rms_process_loud_chunk_starts_speaking() {
    let mut d = RmsVoiceDetector::new(0.01, 300_000, 15_000_000);
    // High-amplitude signal — RMS >> 0.01 threshold
    let chunk: Vec<f32> = (0..512).map(|_| 1.0).collect();
    let _ = d.process(&chunk, 16000);
    assert!(d.is_speaking(), "after loud chunk, should be speaking");
}

#[test]
fn rms_process_returns_audio_when_silence_elapsed_exceeds_threshold() {
    // Use threshold = 0.5 so 1.0 amplitude is "speaking"
    // silence_ms very low (1ms) so silence trips after a real sleep
    let mut d = RmsVoiceDetector::new(0.5, 1, 60_000);

    // Feed loud audio — starts speaking, builds buffer ≥ min_samples
    let loud: Vec<f32> = (0..16000).map(|_| 1.0).collect(); // 1 second @ 16kHz
    let _ = d.process(&loud, 16000);
    assert!(d.is_speaking());

    // First silent chunk: triggers silence_start, no emit yet (elapsed=0)
    let silent: Vec<f32> = vec![0.0; 512];
    let _ = d.process(&silent, 16000);

    // Wait long enough that silence_start.elapsed() > silence_ms (1ms)
    std::thread::sleep(Duration::from_millis(5));

    // Second silent chunk: silence_elapsed > 1ms now, buffer ≥ 300ms → emit
    let out = d.process(&silent, 16000);
    assert!(
        out.is_some(),
        "should return completed utterance after silence elapsed"
    );
    let audio = out.unwrap();
    assert!(
        audio.len() >= (16000.0 * 0.3) as usize,
        "returned audio should meet min_samples"
    );
    assert!(!d.is_speaking(), "after emission, is_speaking should reset");
}

#[test]
fn rms_flush_returns_buffered_audio_when_nonempty() {
    let mut d = RmsVoiceDetector::new(0.01, 1_000_000, 60_000_000);
    let chunk: Vec<f32> = (0..1024).map(|_| 0.5).collect();
    let _ = d.process(&chunk, 16000);
    let out = d.flush();
    assert!(out.is_some());
    assert_eq!(out.unwrap().len(), 1024);
}

#[test]
fn rms_flush_returns_none_when_buffer_empty() {
    let mut d = RmsVoiceDetector::new(0.01, 300, 15_000);
    let out = d.flush();
    assert!(out.is_none());
}

#[test]
fn rms_flush_after_emit_returns_none() {
    let mut d = RmsVoiceDetector::new(0.5, 1, 60_000);
    let loud: Vec<f32> = (0..16000).map(|_| 1.0).collect();
    let _ = d.process(&loud, 16000);
    let silent: Vec<f32> = vec![0.0; 512];
    let _ = d.process(&silent, 16000);
    std::thread::sleep(Duration::from_millis(5));
    let _ = d.process(&silent, 16000);
    // Buffer should now be drained by the emit
    let out = d.flush();
    assert!(out.is_none(), "flush after emit should return None");
}

#[test]
fn rms_name_returns_constant() {
    let d = RmsVoiceDetector::new(0.5, 300, 15_000);
    assert_eq!(d.name(), "RMS");
}

#[test]
fn rms_silence_chunk_in_speaking_state_extends_buffer() {
    let mut d = RmsVoiceDetector::new(0.5, 1_000_000, 60_000_000);
    // Start speaking
    let loud: Vec<f32> = (0..1024).map(|_| 1.0).collect();
    let _ = d.process(&loud, 16000);
    assert!(d.is_speaking());

    // Feed silence — buffer should still grow (extends by chunk length)
    let silent: Vec<f32> = vec![0.0; 256];
    let _ = d.process(&silent, 16000);

    // Flush to verify buffer length
    let audio = d.flush().unwrap();
    assert_eq!(audio.len(), 1024 + 256);
}

#[test]
fn rms_repeated_loud_chunks_grow_buffer_until_emit() {
    let mut d = RmsVoiceDetector::new(0.5, 1_000_000, 60_000_000);
    for _ in 0..5 {
        let loud: Vec<f32> = vec![1.0; 1024];
        let out = d.process(&loud, 16000);
        assert!(out.is_none(), "buffer still below min_samples; no emit");
    }
    // After 5 chunks of 1024 = 5120 samples (< 4800 min at 16kHz), still no emit
    // Flush should give us all 5120
    let audio = d.flush().unwrap();
    assert_eq!(audio.len(), 5120);
}

#[test]
fn rms_threshold_zero_marks_speaking_on_empty_chunk_due_to_geq() {
    // Quirk: when threshold=0 and chunk is empty, rms=0.0, and `0.0 >= 0.0` is true,
    // so is_speaking flips true even though no audio was added to the buffer.
    // Pinning this behavior to prevent silent regression if the comparison changes.
    let mut d = RmsVoiceDetector::new(0.0, 1_000_000, 60_000_000);
    let _ = d.process(&[], 16000);
    assert!(
        d.is_speaking(),
        "threshold=0 + empty chunk flips is_speaking (quirk)"
    );
    // Buffer is still empty though
    assert!(d.flush().is_none(), "buffer should still be empty");
}
