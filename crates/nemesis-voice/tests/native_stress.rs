// Windows-only：SttEngine / PunctEngine / TtsEngine 三个引擎模块均
// `#[cfg(target_os = "windows")]`——Linux 上整个测试目标编译期消失
// （Windows 形态测试约定），否则 E0432 unresolved imports。
#![cfg(windows)]

//! native 并发压测（§四.2，2026-10-16）——保留共享能力的三个引擎包装
//! （SttEngine / PunctEngine / TtsEngine，`docs/INFO/2026-10-15_voice-ffi-safety-audit.md`
//! C-1/C-3/C-8）在真实 native 模型下的并发 `&self` 调用压力。
//!
//! 常规套件不跑（`#[ignore]`）：需要真实模型（数百 MB 下载 + 每次冷启动
//! 全量分析），且模型推理秒级耗时，混进 `cargo test` 会拖垮日常迭代。
//! 由 `scripts/voice-native-stress.sh` 在有模型环境显式拉起：
//!
//! ```bash
//! NEMESIS_VOICE_STRESS_MODELS=/path/to/models \
//!   cargo test -p nemesis-voice --release --test native_stress -- --ignored --nocapture
//! ```
//!
//! 模型缺席 = 诚实 SKIP（eprintln + return，不假红也不假绿）。
//!
//! 诚实边界：
//! - TtsEngine 的销毁/重建（recreate_engine）是 private 自愈路径，只在
//!   kokoro C++ 异常时触发——外部无法确定性驱动，本压测覆盖的是「并发
//!   generate 下锁段无死锁、无段错误、无静默损坏」；异常注入场景由
//!   真实运行期兜底设计承担（tts.rs 自愈设计注释）。
//! - Rust 侧 `-Zsanitizer=address` 不覆盖预编译 sherpa-onnx native 堆；
//!   真正的 native ASan 需以 ASan 重编 sherpa-onnx 本体（专用环境，
//!   见 scripts/voice-native-stress.sh 头注与 docs/INFO 文档）。

use std::sync::Arc;
use std::time::Instant;

use nemesis_voice::{PunctEngine, SttEngine, TtsEngine};

/// 模型根目录（env 显式传入；目录布局同 workspace/config/data：
/// `stt/sensevoice-small/`、`punct/ct-transformer-zh-en/`、`tts/<name>/`）。
fn models_root() -> Option<std::path::PathBuf> {
    let root = std::env::var_os("NEMESIS_VOICE_STRESS_MODELS")?;
    let p = std::path::PathBuf::from(root);
    if p.is_dir() { Some(p) } else { None }
}

/// 伪语音样本：440Hz 正弦 + 渐弱包络（真实引擎对纯静音/纯噪声都能解码，
/// 正弦形态更接近有声输入，避免触发引擎的静音短路路径使压测失真）。
fn synth_samples(len_ms: u32, sample_rate: u32) -> Vec<f32> {
    let n = (sample_rate / 1000 * len_ms) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            0.6 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
        })
        .collect()
}

const THREADS: usize = 8;
const ITERS: usize = 8;

#[ignore = "native 压测：需真实模型 + 显式 NEMESIS_VOICE_STRESS_MODELS，见文件头"]
#[tokio::test(flavor = "multi_thread")]
async fn stress_stt_concurrent_shared_arc() {
    let Some(root) = models_root() else {
        eprintln!(
            "SKIP: NEMESIS_VOICE_STRESS_MODELS not set or not a dir — download voice models first"
        );
        return;
    };
    let stt_dir = root.join("stt").join("sensevoice-small");
    if !stt_dir.exists() {
        eprintln!("SKIP: stt model dir missing: {}", stt_dir.display());
        return;
    }
    let engine = Arc::new(
        SttEngine::new(&stt_dir, "sensevoice-small", "auto", false, true, 2)
            .expect("STT engine build"),
    );
    let samples = synth_samples(800, 16_000);
    let started = Instant::now();
    let mut joins = Vec::new();
    for t in 0..THREADS {
        let engine = engine.clone();
        let samples = samples.clone();
        joins.push(std::thread::spawn(move || {
            for i in 0..ITERS {
                // 噪声/正弦输入的识别文本不预设内容——压测断言的是
                // 「并发 &self 调用全程 Ok、不段错误、不静默空转」。
                let out = engine
                    .recognize(&samples, 16_000)
                    .unwrap_or_else(|e| panic!("thread {t} iter {i}: recognize failed: {e}"));
                assert!(!out.contains('\0'), "thread {t}: NUL in output");
            }
        }));
    }
    for j in joins {
        j.join().expect("worker thread panicked");
    }
    eprintln!(
        "stt stress ok: {THREADS} threads x {ITERS} iters in {:.1}s",
        started.elapsed().as_secs_f32()
    );
}

#[ignore = "native 压测：需真实模型 + 显式 NEMESIS_VOICE_STRESS_MODELS，见文件头"]
#[tokio::test(flavor = "multi_thread")]
async fn stress_punct_concurrent_shared_arc() {
    let Some(root) = models_root() else {
        eprintln!(
            "SKIP: NEMESIS_VOICE_STRESS_MODELS not set or not a dir — download voice models first"
        );
        return;
    };
    let model = root
        .join("punct")
        .join("ct-transformer-zh-en")
        .join("model.onnx");
    if !model.exists() {
        eprintln!("SKIP: punct model missing: {}", model.display());
        return;
    }
    let engine = Arc::new(PunctEngine::new(&model, 2).expect("punct engine build"));
    let started = Instant::now();
    let mut joins = Vec::new();
    for t in 0..THREADS {
        let engine = engine.clone();
        joins.push(std::thread::spawn(move || {
            for i in 0..ITERS * 2 {
                let raw = format!("今天天气很好我们去了公园然后吃了午饭第{i}次");
                let out = engine
                    .add_punctuation(&raw)
                    .unwrap_or_else(|e| panic!("thread {t} iter {i}: add_punctuation failed: {e}"));
                assert!(!out.is_empty(), "thread {t} iter {i}: empty output");
            }
        }));
    }
    for j in joins {
        j.join().expect("worker thread panicked");
    }
    eprintln!(
        "punct stress ok: {THREADS} threads x {} iters in {:.1}s",
        ITERS * 2,
        started.elapsed().as_secs_f32()
    );
}

#[ignore = "native 压测：需真实模型 + 显式 NEMESIS_VOICE_STRESS_MODELS，见文件头"]
#[tokio::test(flavor = "multi_thread")]
async fn stress_tts_concurrent_generate() {
    let Some(root) = models_root() else {
        eprintln!(
            "SKIP: NEMESIS_VOICE_STRESS_MODELS not set or not a dir — download voice models first"
        );
        return;
    };
    // 默认 kokoro（config.toml [tts] model_name）；vits-melo 同样可压（换名字即可）。
    let tts_dir = root.join("tts").join("kokoro-multi-lang-v1_1");
    if !tts_dir.exists() {
        eprintln!("SKIP: tts model dir missing: {}", tts_dir.display());
        return;
    }
    let engine = Arc::new(TtsEngine::new(&tts_dir, 2).expect("tts engine build"));
    let started = Instant::now();
    let mut joins = Vec::new();
    for t in 0..6 {
        let engine = engine.clone();
        joins.push(std::thread::spawn(move || {
            for i in 0..4 {
                let (samples, rate) = engine
                    .generate(&format!("并发压测第{t}轮第{i}次。"), 0, 1.0)
                    .unwrap_or_else(|e| panic!("thread {t} iter {i}: generate failed: {e}"));
                assert!(!samples.is_empty(), "thread {t} iter {i}: empty audio");
                assert!(rate > 0, "thread {t} iter {i}: bad sample rate {rate}");
            }
        }));
    }
    for j in joins {
        j.join().expect("worker thread panicked");
    }
    eprintln!(
        "tts stress ok: 6 threads x 4 iters in {:.1}s",
        started.elapsed().as_secs_f32()
    );
}
