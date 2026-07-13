//! Throwaway STT benchmark — measures single-utterance decode latency and the
//! detected language, and exercises the lang_remedy (补救) dual-engine path.
//!
//! Usage:
//!   cargo run --release --example stt_bench -p nemesis-voice -- <voice_dir> [wav]

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::time::Instant;

/// Read a PCM mono 16-bit WAV into f32 samples in [-1, 1]. Returns (samples, sample_rate).
fn read_wav_pcm_mono(path: &PathBuf) -> Result<(Vec<f32>, u32)> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a RIFF/WAVE file: {}", path.display());
    }
    let mut sr: u32 = 0;
    let mut bits: u16 = 0;
    let mut data: &[u8] = &[];
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let sz = u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        let body_end = (i + 8 + sz).min(bytes.len());
        let body = &bytes[i + 8..body_end];
        if id == b"fmt " && body.len() >= 16 {
            let af = u16::from_le_bytes([body[0], body[1]]);
            if af != 1 {
                bail!("non-PCM format {} in {}", af, path.display());
            }
            sr = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            data = body;
        }
        i += 8 + sz + (sz & 1); // chunks are word-aligned (pad to even)
    }
    if sr == 0 || bits == 0 {
        bail!("missing fmt/data chunk in {}", path.display());
    }
    if bits != 16 {
        bail!("expected 16-bit PCM, got {} bits in {}", bits, path.display());
    }
    let samples: Vec<f32> = data
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();
    Ok((samples, sr))
}

/// 用指定 language 跑 N 次解码（lang_remedy=false，测原始每语言延迟），打印耗时/语言/文本。
fn bench_lang(
    stt_dir: &std::path::Path,
    model_name: &str,
    lang_arg: &str,
    use_itn: bool,
    threads: u32,
    samples: &[f32],
    sr: u32,
) -> Result<()> {
    let engine = nemesis_voice::SttEngine::new(stt_dir, model_name, lang_arg, false, use_itn, threads)?;

    // warmup (first decode includes ORT session warm-up / graph allocation)
    let _ = engine.recognize_detail(samples, sr)?;

    const N: usize = 10;
    let mut times: Vec<f64> = Vec::with_capacity(N);
    let mut last_text = String::new();
    let mut last_lang: Option<String> = None;
    for _ in 0..N {
        let t = Instant::now();
        let (text, lang) = engine.recognize_detail(samples, sr)?;
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        last_text = text;
        last_lang = lang;
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = times.iter().sum::<f64>() / N as f64;
    let dur_s = samples.len() as f64 / sr as f64;
    let rt = if mean > 0.0 { dur_s / (mean / 1000.0) } else { 0.0 };
    println!(
        "language={:>5} | detected={:?} | mean={:6.1}ms min={:6.1}ms max={:6.1}ms ({:5.1}x RT) | text={:?}",
        lang_arg, last_lang, mean, times[0], times[N - 1], rt, last_text
    );
    Ok(())
}

fn main() -> Result<()> {
    let voice_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .deref_or_exit("usage: stt_bench <voice_dir> [wav]"),
    );
    let wav = PathBuf::from(
        std::env::args()
            .nth(2)
            .unwrap_or_else(|| voice_dir.join("tts_test.wav").to_string_lossy().into_owned()),
    );

    let config_path = voice_dir.join("config.toml");
    let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);

    println!("loading sherpa-onnx runtime from {} ...", voice_dir.display());
    nemesis_voice::bootstrap::init_sherpa(&voice_dir)?;

    let stt_dir = nemesis_voice::model::ensure_stt_model(&cfg)?;
    println!("stt model dir: {}", stt_dir.display());

    let (samples, sr) = read_wav_pcm_mono(&wav)?;
    let dur = samples.len() as f64 / sr as f64;
    println!(
        "wav: {} | {:.2}s, {} samples @ {}Hz, model={} num_threads={} (use_itn={})",
        wav.display(),
        dur,
        samples.len(),
        sr,
        cfg.stt.model_name,
        cfg.stt.num_threads,
        cfg.stt.use_itn
    );
    println!("{}", "-".repeat(96));

    for lang_arg in ["auto", "zh", "en"] {
        bench_lang(
            &stt_dir,
            &cfg.stt.model_name,
            lang_arg,
            cfg.stt.use_itn,
            cfg.stt.num_threads,
            &samples,
            sr,
        )?;
    }

    println!("{}", "-".repeat(96));
    println!("scaling (auto, repeat audio to simulate longer utterances):");
    for mul in [1usize, 2, 4] {
        let long: Vec<f32> = samples.repeat(mul);
        bench_lang(
            &stt_dir,
            &cfg.stt.model_name,
            "auto",
            cfg.stt.use_itn,
            cfg.stt.num_threads,
            &long,
            sr,
        )?;
    }

    println!("{}", "-".repeat(96));
    println!(
        "lang_remedy path (language=auto + lang_remedy=true → 模型声明补救则建第二引擎):"
    );
    // 用真实 model_name（sensevoice）+ lang_remedy=true，走生产路径：
    // 模型声明了补救 → 建英文 fallback 引擎。中文样本检测为 zh 不触发重解，但验证了双引擎构造。
    let fb_engine = nemesis_voice::SttEngine::new(
        &stt_dir,
        &cfg.stt.model_name,
        "auto",
        true,
        cfg.stt.use_itn,
        cfg.stt.num_threads,
    )?;
    let t = Instant::now();
    let (text, lang) = fb_engine.recognize_detail(&samples, sr)?;
    let elapsed = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "text={:?} lang={:?} decode={:.1}ms (补救仅在 auto 检测到 ja/ko/yue 时触发重解)",
        text, lang, elapsed
    );

    println!("{}", "-".repeat(96));
    println!(
        "lang_remedy=false (强制关闭补救，回退纯 auto — 模拟换不误判模型):"
    );
    let nofb = nemesis_voice::SttEngine::new(
        &stt_dir,
        &cfg.stt.model_name,
        "auto",
        false,
        cfg.stt.use_itn,
        cfg.stt.num_threads,
    )?;
    let (text, lang) = nofb.recognize_detail(&samples, sr)?;
    println!("text={:?} lang={:?} (无补救，单引擎 auto)", text, lang);

    Ok(())
}

// Small helper so we don't pull in a crate just for argv parsing.
trait DerefOrExit {
    fn deref_or_exit(self, msg: &str) -> String;
}
impl DerefOrExit for Option<String> {
    fn deref_or_exit(self, msg: &str) -> String {
        match self {
            Some(s) => s,
            None => {
                eprintln!("{msg}");
                std::process::exit(2);
            }
        }
    }
}
