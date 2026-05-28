//! Voice pipeline management commands.

use anyhow::Result;
use clap::Subcommand;
#[cfg(target_os = "windows")]
use std::sync::Arc;

#[cfg(target_os = "windows")]
use crate::common;

#[derive(Subcommand)]
pub enum VoiceAction {
    /// Check voice environment status
    Status,
    /// Setup voice environment (download libraries + models)
    Setup,
    /// Download all voice models
    Download,
    /// Text-to-speech test
    Tts {
        text: String,
        #[arg(short = 'S', long)]
        speaker: Option<u32>,
        #[arg(short, long, default_value = "1.0")]
        speed: f32,
    },
    /// Speech-to-text dictation test (microphone → VAD → STT → text output)
    Stt,
    /// Full voice chat loop (microphone → VAD → STT → TTS → playback)
    Chat,
    /// List audio devices
    Devices,
}

pub fn run(action: VoiceAction, _local: bool) -> Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = action;
        anyhow::bail!("Voice pipeline is only supported on Windows");
    }

    #[cfg(target_os = "windows")]
    {
        let home = common::resolve_home(_local);
        let workspace = common::workspace_path(&home);
        let voice_dir = workspace.join("tools").join("voice");

        match action {
            VoiceAction::Status => cmd_status(&voice_dir),
            VoiceAction::Setup => cmd_setup(&voice_dir),
            VoiceAction::Download => cmd_download(&voice_dir),
            VoiceAction::Tts { text, speaker, speed } => cmd_tts(&voice_dir, &text, speaker, speed),
            VoiceAction::Stt => cmd_stt(&voice_dir),
            VoiceAction::Chat => cmd_chat(&voice_dir),
            VoiceAction::Devices => cmd_devices(),
        }
    }
}

#[cfg(target_os = "windows")]
fn require_config(voice_dir: &std::path::Path) -> Result<nemesis_voice::AppConfig> {
    let config_path = voice_dir.join("config.toml");
    if !config_path.exists() {
        anyhow::bail!("Voice not set up. Run: nemesisbot voice setup");
    }
    Ok(nemesis_voice::AppConfig::load_or_default(&config_path))
}

#[cfg(target_os = "windows")]
fn cmd_status(voice_dir: &std::path::Path) -> Result<()> {
    println!("=== Voice Environment Status ===\n");

    // Check shared libraries
    let libs = nemesis_voice::bootstrap::required_lib_names();
    let mut libs_ok = true;
    for lib in libs {
        let path = voice_dir.join(lib);
        if path.exists() {
            let size = std::fs::metadata(&path)?.len();
            println!("  [OK] {} ({:.1} MB)", lib, size as f64 / (1024.0 * 1024.0));
        } else {
            println!("  [--] {} (missing)", lib);
            libs_ok = false;
        }
    }

    // Check config
    let config_path = voice_dir.join("config.toml");
    if config_path.exists() {
        println!("  [OK] config.toml");
    } else {
        println!("  [--] config.toml (missing)");
    }

    // Load config and check models
    if config_path.exists() {
        let cfg = nemesis_voice::AppConfig::load(&config_path)
            .unwrap_or_else(|_| nemesis_voice::AppConfig::default());

        let model_dir = cfg.model_dir();
        let model_name = &cfg.tts.model_name;
        let tts_dir = model_dir.join("tts").join(model_name);
        if tts_dir.exists() {
            println!("  [OK] TTS model: {}", model_name);
        } else {
            println!("  [--] TTS model: {} (not downloaded)", model_name);
        }

        let stt_name = &cfg.stt.model_name;
        let stt_dir = model_dir.join("stt").join(stt_name);
        if stt_dir.exists() {
            println!("  [OK] STT model: {}", stt_name);
        } else {
            println!("  [--] STT model: {} (not downloaded)", stt_name);
        }

        let vad_name = &cfg.vad.model_name;
        let vad_dir = model_dir.join("vad").join(vad_name);
        if vad_dir.exists() {
            println!("  [OK] VAD model: {}", vad_name);
        } else {
            println!("  [--] VAD model: {} (not downloaded)", vad_name);
        }

        let punct_name = &cfg.punct.model_name;
        let punct_dir = model_dir.join("punct").join(punct_name);
        if punct_dir.exists() {
            println!("  [OK] Punct model: {}", stt_name);
        } else {
            println!("  [--] Punct model: {} (not downloaded)", punct_name);
        }
    }

    println!();
    if libs_ok {
        println!("  Voice environment is ready.");
    } else {
        println!("  Voice environment incomplete. Run: nemesisbot voice setup");
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn cmd_setup(voice_dir: &std::path::Path) -> Result<()> {
    println!("=== Voice Setup ===\n");

    std::fs::create_dir_all(voice_dir)?;

    let config_path = voice_dir.join("config.toml");

    // Bootstrap: download libraries + create config
    nemesis_voice::bootstrap::run_in_dir(&config_path, voice_dir)?;

    println!("\nVoice setup complete.");
    println!("  Directory: {}", voice_dir.display());
    Ok(())
}

#[cfg(target_os = "windows")]
fn cmd_download(voice_dir: &std::path::Path) -> Result<()> {
    println!("=== Download Voice Models ===\n");

    let cfg = require_config(voice_dir)?;

    println!("Mirror: {}", cfg.models.mirror.base);
    println!("Target: {}\n", cfg.model_dir().display());

    std::fs::create_dir_all(cfg.model_dir())?;

    println!("[1/5] STT ({})", cfg.stt.model_name);
    match nemesis_voice::model::ensure_stt_model(&cfg) {
        Ok(path) => println!("  Ready: {}\n", path.display()),
        Err(e) => eprintln!("  Failed: {}\n", e),
    }

    println!("[2/5] VAD ({})", cfg.vad.model_name);
    match nemesis_voice::model::ensure_vad_model(&cfg) {
        Ok(path) => println!("  Ready: {}\n", path.display()),
        Err(e) => eprintln!("  Failed: {}\n", e),
    }

    println!("[3/5] TTS ({})", cfg.tts.model_name);
    match nemesis_voice::model::ensure_tts_model(&cfg) {
        Ok(path) => println!("  Ready: {}\n", path.display()),
        Err(e) => eprintln!("  Failed: {}\n", e),
    }

    println!("[4/5] Punct ({})", cfg.punct.model_name);
    match nemesis_voice::model::ensure_punct_model(&cfg) {
        Ok(path) => println!("  Ready: {}\n", path.display()),
        Err(e) => eprintln!("  Failed: {}\n", e),
    }

    println!("[5/5] Speaker ({})", cfg.speaker.model_name);
    match nemesis_voice::model::ensure_speaker_model(&cfg) {
        Ok(path) => println!("  Ready: {}\n", path.display()),
        Err(e) => eprintln!("  Failed: {}\n", e),
    }

    println!("Done.");
    Ok(())
}

#[cfg(target_os = "windows")]
fn cmd_tts(voice_dir: &std::path::Path, text: &str, speaker: Option<u32>, speed: f32) -> Result<()> {
    println!("=== TTS Test ===\n");

    let cfg = require_config(voice_dir)?;
    nemesis_voice::bootstrap::init_sherpa(voice_dir)?;

    let tts_dir = nemesis_voice::model::ensure_tts_model(&cfg)?;
    let tts_engine = nemesis_voice::TtsEngine::new(&tts_dir, cfg.tts.num_threads)?;
    println!("  [TTS] Engine loaded.\n");

    let sid = speaker.unwrap_or(cfg.tts.speaker_id);
    println!("Generating: {}", text);

    let (samples, sample_rate) = tts_engine.generate(text, sid, speed)?;
    if samples.is_empty() {
        anyhow::bail!("TTS generated empty audio");
    }

    let audio_duration = samples.len() as f64 / sample_rate as f64;
    println!(
        "Generated {} samples at {} Hz ({:.1}s audio)",
        samples.len(), sample_rate, audio_duration
    );

    // Save to WAV
    let output_path = voice_dir.join("tts_test.wav");
    let path_c = nemesis_voice::sherpa::to_cstr(output_path.to_str().unwrap_or(""));
    let ret = unsafe {
        nemesis_voice::sherpa::SherpaOnnxWriteWave(
            samples.as_ptr(),
            samples.len() as i32,
            sample_rate as i32,
            path_c.as_ptr(),
        )
    };
    if ret == 0 {
        anyhow::bail!("Failed to write WAV file");
    }
    println!("Saved to: {}", output_path.display());

    // Try playback
    let playback = nemesis_voice::AudioPlayback::new("", sample_rate, cfg.audio.gain)?;
    playback.play_blocking(&samples, sample_rate)?;
    println!("Playback complete.");

    Ok(())
}

#[cfg(target_os = "windows")]
fn cmd_stt(voice_dir: &std::path::Path) -> Result<()> {
    println!("=== STT Dictation Test ===\n");
    println!("Speak into your microphone. Press Ctrl+C to stop.\n");

    let cfg = require_config(voice_dir)?;
    nemesis_voice::bootstrap::init_sherpa(voice_dir)?;

    // Load engines
    let stt_dir = nemesis_voice::model::ensure_stt_model(&cfg)?;
    let stt_engine = Arc::new(nemesis_voice::SttEngine::new(
        &stt_dir, &cfg.stt.language, cfg.stt.use_itn, cfg.stt.num_threads,
    )?);
    println!("  [STT] Engine loaded.");

    let punct_engine = match nemesis_voice::model::ensure_punct_model(&cfg) {
        Ok(dir) => {
            let model_path = dir.join("model.onnx");
            match nemesis_voice::PunctEngine::new(&model_path, cfg.punct.num_threads) {
                Ok(engine) => Some(Arc::new(engine)),
                Err(_) => None,
            }
        }
        Err(_) => None,
    };

    let mut detector = nemesis_voice::create_detector(&cfg);
    println!("  [VAD] Using {}.\n", detector.name());

    // Start audio capture
    let capture = nemesis_voice::AudioCapture::new(&cfg.audio.capture_device)?;
    let capture_sr = capture.sample_rate;
    println!("  [Audio] Capturing at {} Hz\n", capture_sr);

    let mut resampler = nemesis_voice::Resampler::new(capture_sr, cfg.audio.target_sample_rate)?;
    let target_sr = cfg.audio.target_sample_rate;

    // Main loop
    loop {
        while let Some(chunk) = capture.try_receive() {
            let resampled = resampler.resample(&chunk);
            if let Some(speech) = detector.process(&resampled, target_sr) {
                let text = stt_engine.recognize(&speech, target_sr)?;
                if !text.is_empty() {
                    let result = match &punct_engine {
                        Some(p) => p.add_punctuation(&text).unwrap_or(text),
                        None => text,
                    };
                    println!("{}", result);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(target_os = "windows")]
fn cmd_chat(voice_dir: &std::path::Path) -> Result<()> {
    println!("=== Voice Chat Loop ===\n");
    println!("Speak into your microphone. Your speech will be recognized and played back.\n");
    println!("Press Ctrl+C to stop.\n");

    let cfg = require_config(voice_dir)?;
    nemesis_voice::bootstrap::init_sherpa(voice_dir)?;

    // Load STT
    let stt_dir = nemesis_voice::model::ensure_stt_model(&cfg)?;
    let stt_engine = Arc::new(nemesis_voice::SttEngine::new(
        &stt_dir, &cfg.stt.language, cfg.stt.use_itn, cfg.stt.num_threads,
    )?);
    println!("  [STT] Engine loaded.");

    let punct_engine = match nemesis_voice::model::ensure_punct_model(&cfg) {
        Ok(dir) => {
            let model_path = dir.join("model.onnx");
            match nemesis_voice::PunctEngine::new(&model_path, cfg.punct.num_threads) {
                Ok(engine) => Some(Arc::new(engine)),
                Err(_) => None,
            }
        }
        Err(_) => None,
    };

    // Load TTS
    let tts_dir = nemesis_voice::model::ensure_tts_model(&cfg)?;
    let tts_engine = Arc::new(nemesis_voice::TtsEngine::new(&tts_dir, cfg.tts.num_threads)?);
    println!("  [TTS] Engine loaded.");

    let mut detector = nemesis_voice::create_detector(&cfg);
    println!("  [VAD] Using {}.", detector.name());

    // Start audio capture + playback
    let capture = nemesis_voice::AudioCapture::new(&cfg.audio.capture_device)?;
    let capture_sr = capture.sample_rate;
    let playback = nemesis_voice::AudioPlayback::new(
        &cfg.audio.playback_device, cfg.audio.target_sample_rate, cfg.audio.gain,
    )?;
    println!("  [Audio] Capture {} Hz → Playback {} Hz\n", capture_sr, playback.sample_rate);

    let mut resampler = nemesis_voice::Resampler::new(capture_sr, cfg.audio.target_sample_rate)?;
    let target_sr = cfg.audio.target_sample_rate;
    let speaker_id = cfg.tts.speaker_id;
    let speed = cfg.tts.speed;

    // Main loop
    loop {
        while let Some(chunk) = capture.try_receive() {
            let resampled = resampler.resample(&chunk);
            if let Some(speech) = detector.process(&resampled, target_sr) {
                // STT
                let text = stt_engine.recognize(&speech, target_sr)?;
                if text.is_empty() {
                    continue;
                }
                let result = match &punct_engine {
                    Some(p) => p.add_punctuation(&text).unwrap_or(text),
                    None => text,
                };
                println!("[You] {}", result);

                // TTS
                match tts_engine.generate(&result, speaker_id, speed) {
                    Ok((samples, tts_sr)) if !samples.is_empty() => {
                        if let Err(e) = playback.play_blocking(&samples, tts_sr) {
                            eprintln!("[Playback] Error: {}", e);
                        }
                    }
                    Ok(_) => eprintln!("[TTS] Generated empty audio"),
                    Err(e) => eprintln!("[TTS] Error: {}", e),
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(target_os = "windows")]
fn cmd_devices() -> Result<()> {
    println!("=== Audio Devices ===\n");
    let devices = nemesis_voice::audio::list_devices()?;

    if devices.is_empty() {
        println!("No audio devices found.");
        return Ok(());
    }

    for dev in &devices {
        let kind = if dev.is_input { "INPUT " } else { "OUTPUT" };
        let default = if dev.is_default { " (default)" } else { "" };
        println!("  [{}] {} {}{}", dev.index, kind, dev.name, default);
    }

    Ok(())
}
