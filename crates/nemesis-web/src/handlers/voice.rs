//! Voice handler — voice environment status, setup, config, TTS test, STT dictation.
//!
//! Commands: status, check, setup, stop_setup, install_runtime, install_model,
//!           config_get, config_set, voice_config_get, voice_config_set,
//!           tts, stt_start, stt_stop, speakers, devices,
//!           engine_start, engine_stop, pipeline_start, pipeline_stop

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

// Kokoro multi-lang v1.1 speaker list (Chinese speakers)
const KOKORO_SPEAKERS: &[(&str, &str, u32)] = &[
    ("zf_xiaobei", "女声", 45),
    ("zf_xiaoni", "女声", 41),
    ("zf_xiaoxiao", "女声", 47),
    ("zf_xiaoyi", "女声", 46),
    ("zm_yunyang", "男声", 43),
    ("zm_yunxi", "男声", 42),
    ("zm_yunhao", "男声", 44),
    ("zm_yunjian", "男声", 40),
];

const VOICE_CONFIG_FILENAME: &str = "config.voice.json";
const DEFAULT_VOICE_CONFIG: &str = include_str!("../../../../nemesisbot/config/config.voice.default.json");

// Global setup cancellation token
fn setup_cancel() -> &'static std::sync::Mutex<Option<CancellationToken>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<CancellationToken>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

// Per-model install lock — prevents concurrent downloads of the same model.
// Simple HashSet: insert on start, remove on finish. No nesting, no deadlock.
fn install_locks() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static INSTANCE: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn model_label(model: &str) -> &str {
    match model {
        "stt" => "STT",
        "vad" => "VAD",
        "tts" => "TTS",
        "punct" => "标点",
        _ => model,
    }
}

// Global STT session (only one active dictation at a time)
fn stt_state() -> &'static Arc<Mutex<Option<SttSession>>> {
    static INSTANCE: OnceLock<Arc<Mutex<Option<SttSession>>>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(Mutex::new(Option::None)))
}

struct SttSession {
    cancel: CancellationToken,
}

// ---------------------------------------------------------------------------
// Persistent engine state (Phase 1)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn stt_engine_state() -> &'static std::sync::Mutex<Option<nemesis_voice::SttEngine>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<nemesis_voice::SttEngine>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(target_os = "windows")]
fn tts_engine_state() -> &'static std::sync::Mutex<Option<nemesis_voice::TtsEngine>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<nemesis_voice::TtsEngine>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

// ---------------------------------------------------------------------------
// STT output interface (Phase 3)
// ---------------------------------------------------------------------------

/// Trait for routing STT recognition results.
#[cfg(target_os = "windows")]
trait SttOutput: Send {
    fn send_text(&self, text: &str);
}

/// Default implementation: push via WebSocket to the originating session.
#[cfg(target_os = "windows")]
struct WsSttOutput {
    session_id: String,
    session_mgr: Arc<crate::session::SessionManager>,
}

#[cfg(target_os = "windows")]
impl SttOutput for WsSttOutput {
    fn send_text(&self, text: &str) {
        push_stt_result(&self.session_id, self.session_mgr.clone(), text);
    }
}

// ---------------------------------------------------------------------------
// TTS input interface (Phase 3)
// ---------------------------------------------------------------------------

/// Trait for text input sources that trigger TTS synthesis.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
trait TtsInput: Send {
    /// Start listening. Calls `callback` for each text to synthesize.
    fn listen(&self, callback: Box<dyn Fn(&str) + Send + 'static>);
}

/// No-op TTS input: no source configured, never triggers.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
struct NoopTtsInput;

#[cfg(target_os = "windows")]
impl TtsInput for NoopTtsInput {
    fn listen(&self, _callback: Box<dyn Fn(&str) + Send + 'static>) {
        // No source configured — never triggers.
    }
}

pub struct VoiceHandler {
    _priv: (),
}

impl VoiceHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for VoiceHandler {
    fn module_name(&self) -> &str {
        "voice"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (cmd, data, ctx);
            return Err("Voice pipeline is only supported on Windows".to_string());
        }

        #[cfg(target_os = "windows")]
        {
            let workspace = require_workspace(ctx)?;
            let voice_dir = PathBuf::from(workspace).join("tools").join("voice");
            let config_dir = PathBuf::from(workspace).join("config");

            match cmd {
                "status" => self.cmd_status(&voice_dir),
                "check" => self.cmd_status(&voice_dir),
                "setup" => self.cmd_setup(&voice_dir, ctx).await,
                "stop_setup" => self.cmd_stop_setup(),
                "install_runtime" => self.cmd_install_runtime(&voice_dir, ctx).await,
                "install_model" => {
                    let d = data.ok_or("missing data")?;
                    let model = d.get("model").and_then(|v| v.as_str()).ok_or("missing field: model")?;
                    self.cmd_install_model(&voice_dir, model, ctx).await
                }
                "config_get" => self.cmd_config_get(&voice_dir),
                "config_set" => {
                    let d = data.ok_or("missing data")?;
                    let content = d.get("content").and_then(|v| v.as_str()).ok_or("missing field: content")?;
                    self.cmd_config_set(&voice_dir, content)
                }
                "voice_config_get" => self.cmd_voice_config_get(&config_dir),
                "voice_config_set" => {
                    let d = data.ok_or("missing data")?;
                    self.cmd_voice_config_set(&config_dir, &d)
                }
                "tts" => {
                    let d = data.ok_or("missing data")?;
                    self.cmd_tts(&voice_dir, &config_dir, &d).await
                }
                "stt_start" => self.cmd_stt_start(&voice_dir, &config_dir, ctx).await,
                "stt_stop" => self.cmd_stt_stop().await,
                "speakers" => self.cmd_speakers(),
                "devices" => self.cmd_devices(),
                "engine_start" => {
                    let d = data.ok_or("missing data")?;
                    let model = d.get("model").and_then(|v| v.as_str()).ok_or("missing field: model")?;
                    self.cmd_engine_start(&voice_dir, &config_dir, model).await
                }
                "engine_stop" => {
                    let d = data.ok_or("missing data")?;
                    let model = d.get("model").and_then(|v| v.as_str()).ok_or("missing field: model")?;
                    self.cmd_engine_stop(model)
                }
                "pipeline_start" => {
                    let d = data.ok_or("missing data")?;
                    let model = d.get("model").and_then(|v| v.as_str()).ok_or("missing field: model")?;
                    self.cmd_pipeline_start(&voice_dir, model, ctx).await
                }
                "pipeline_stop" => {
                    let d = data.ok_or("missing data")?;
                    let model = d.get("model").and_then(|v| v.as_str()).ok_or("missing field: model")?;
                    self.cmd_pipeline_stop(model).await
                }
                _ => Err(format!("unknown command: voice.{}", cmd)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Voice config helpers (config.voice.json)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn ensure_voice_config(config_dir: &std::path::Path) -> PathBuf {
    let path = config_dir.join(VOICE_CONFIG_FILENAME);
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, DEFAULT_VOICE_CONFIG);
    }
    path
}

#[cfg(target_os = "windows")]
fn read_voice_config(config_dir: &std::path::Path) -> serde_json::Value {
    let path = ensure_voice_config(config_dir);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::from_str(DEFAULT_VOICE_CONFIG).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Windows implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
impl VoiceHandler {
    fn cmd_status(&self, voice_dir: &std::path::Path) -> Result<Option<serde_json::Value>, String> {
        let libs = nemesis_voice::bootstrap::required_lib_names();
        let dlls: Vec<serde_json::Value> = libs
            .iter()
            .map(|dll| {
                let path = voice_dir.join(dll);
                let exists = path.exists();
                let size = if exists {
                    std::fs::metadata(&path).ok().map(|m| m.len())
                } else {
                    None
                };
                serde_json::json!({
                    "name": dll,
                    "exists": exists,
                    "size_bytes": size,
                })
            })
            .collect();

        let all_dlls_present = dlls.iter().all(|d| d["exists"].as_bool().unwrap_or(false));

        let config_path = voice_dir.join("config.toml");
        let config_exists = config_path.exists();

        let model_dir = if config_exists {
            let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);
            cfg.model_dir()
        } else {
            voice_dir.join("data")
        };

        let stt_model = model_dir.join("stt");
        let vad_model = model_dir.join("vad");
        let tts_model = model_dir.join("tts");
        let punct_model = model_dir.join("punct");

        let stt_ready = check_model_subdir_any(&stt_model);
        let vad_ready = check_model_subdir_any(&vad_model);
        let tts_ready = check_model_subdir_any(&tts_model);
        let punct_ready = check_model_subdir_any(&punct_model);

        Ok(Some(serde_json::json!({
            "ready": all_dlls_present && config_exists,
            "dlls": dlls,
            "all_dlls_present": all_dlls_present,
            "config_exists": config_exists,
            "voice_dir": voice_dir.to_string_lossy(),
            "model_dir": model_dir.to_string_lossy(),
            "models": {
                "stt": { "ready": stt_ready, "path": stt_model.to_string_lossy() },
                "vad": { "ready": vad_ready, "path": vad_model.to_string_lossy() },
                "tts": { "ready": tts_ready, "path": tts_model.to_string_lossy() },
                "punct": { "ready": punct_ready, "path": punct_model.to_string_lossy() },
            },
        })))
    }

    async fn cmd_setup(&self, voice_dir: &std::path::Path, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let cancel = CancellationToken::new();
        {
            let mut guard = setup_cancel().lock().unwrap();
            *guard = Some(cancel.clone());
        }

        let dir = voice_dir.to_path_buf();
        let hub = ctx.state.event_hub.clone();

        let result = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("failed to create voice dir: {}", e))?;

            let config_path = dir.join("config.toml");
            hub.publish("voice-setup", serde_json::json!({
                "phase": "setup", "status": "starting",
                "message": "开始安装语音环境..."
            }));

            if cancel.is_cancelled() {
                hub.publish("voice-setup", serde_json::json!({
                    "phase": "setup", "status": "cancelled",
                    "message": "安装已取消"
                }));
                return Err("setup cancelled".to_string());
            }

            match nemesis_voice::bootstrap_run_in_dir(&config_path, &dir) {
                Ok(_) => {
                    hub.publish("voice-setup", serde_json::json!({
                        "phase": "setup", "status": "complete",
                        "message": "语音环境安装完成"
                    }));
                    Ok(Some(serde_json::json!({ "success": true })))
                }
                Err(e) => {
                    hub.publish("voice-setup", serde_json::json!({
                        "phase": "setup", "status": "error",
                        "message": format!("安装失败: {}", e)
                    }));
                    Err(format!("setup failed: {}", e))
                }
            }
        }).await
            .map_err(|e| format!("setup task panicked: {}", e))?;

        // Clear cancel token
        {
            let mut guard = setup_cancel().lock().unwrap();
            *guard = None;
        }

        result
    }

    fn cmd_stop_setup(&self) -> Result<Option<serde_json::Value>, String> {
        let mut guard = setup_cancel().lock().unwrap();
        match guard.take() {
            Some(cancel) => {
                cancel.cancel();
                Ok(Some(serde_json::json!({ "stopped": true })))
            }
            None => Err("no setup in progress".to_string()),
        }
    }

    async fn cmd_install_runtime(&self, voice_dir: &std::path::Path, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let cancel = CancellationToken::new();
        {
            let mut guard = setup_cancel().lock().unwrap();
            *guard = Some(cancel.clone());
        }

        let dir = voice_dir.to_path_buf();
        let hub = ctx.state.event_hub.clone();

        let result = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("failed to create voice dir: {}", e))?;

            let config_path = dir.join("config.toml");
            if !config_path.exists() {
                let default_config = nemesis_voice::bootstrap::default_config_toml();
                std::fs::write(&config_path, default_config)
                    .map_err(|e| format!("failed to write config: {}", e))?;
            }

            hub.publish("voice-setup", serde_json::json!({
                "phase": "runtime", "status": "starting",
                "message": "正在安装运行库..."
            }));

            if cancel.is_cancelled() {
                hub.publish("voice-setup", serde_json::json!({
                    "phase": "runtime", "status": "cancelled",
                    "message": "安装已取消"
                }));
                return Err("cancelled".to_string());
            }

            match nemesis_voice::bootstrap_run_in_dir(&config_path, &dir) {
                Ok(_) => {
                    hub.publish("voice-setup", serde_json::json!({
                        "phase": "runtime", "status": "complete",
                        "message": "运行库安装完成"
                    }));
                    Ok(Some(serde_json::json!({ "success": true })))
                }
                Err(e) => {
                    hub.publish("voice-setup", serde_json::json!({
                        "phase": "runtime", "status": "error",
                        "message": format!("运行库安装失败: {}", e)
                    }));
                    Err(format!("runtime install failed: {}", e))
                }
            }
        }).await
            .map_err(|e| format!("install task panicked: {}", e))?;

        {
            let mut guard = setup_cancel().lock().unwrap();
            *guard = None;
        }

        result
    }

    async fn cmd_install_model(&self, voice_dir: &std::path::Path, model: &str, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        // Acquire per-model install lock
        {
            let mut locks = install_locks().lock().unwrap();
            if locks.contains(model) {
                return Err(format!("{}模型正在安装中，请稍候", model_label(model)));
            }
            locks.insert(model.to_string());
        }

        let cancel = CancellationToken::new();
        {
            let mut guard = setup_cancel().lock().unwrap();
            *guard = Some(cancel.clone());
        }

        let dir = voice_dir.to_path_buf();
        let model_type = model.to_string();
        let hub = ctx.state.event_hub.clone();

        let result = tokio::task::spawn_blocking(move || {
            let config_path = dir.join("config.toml");
            if !config_path.exists() {
                return Err("config.toml not found. Run setup first.".to_string());
            }

            let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);

            // Set up progress callback — pushes download progress to UI via SSE
            {
                let hub_progress = hub.clone();
                let mt = model_type.clone();
                nemesis_voice::set_progress(Some(Box::new(move |msg: &str| {
                    hub_progress.publish("voice-setup", serde_json::json!({
                        "phase": "model", "status": "progress",
                        "model": &mt,
                        "message": msg,
                    }));
                })));
            }

            hub.publish("voice-setup", serde_json::json!({
                "phase": "model", "status": "starting",
                "model": &model_type,
                "message": format!("正在安装{}模型...", model_type)
            }));

            if cancel.is_cancelled() {
                hub.publish("voice-setup", serde_json::json!({
                    "phase": "model", "status": "cancelled",
                    "message": "安装已取消"
                }));
                nemesis_voice::set_progress(None);
                return Err("cancelled".to_string());
            }

            let result = match model_type.as_str() {
                "stt" => nemesis_voice::model::ensure_stt_model(&cfg).map_err(|e| e.to_string()),
                "vad" => nemesis_voice::model::ensure_vad_model(&cfg).map_err(|e| e.to_string()),
                "tts" => nemesis_voice::model::ensure_tts_model(&cfg).map_err(|e| e.to_string()),
                "punct" => nemesis_voice::model::ensure_punct_model(&cfg).map_err(|e| e.to_string()),
                _ => Err(format!("unknown model type: {}", model_type)),
            };

            nemesis_voice::set_progress(None);

            let result: Result<(), String> = result.map(|_| ());

            match result {
                Ok(()) => {
                    hub.publish("voice-setup", serde_json::json!({
                        "phase": "model", "status": "complete",
                        "model": &model_type,
                        "message": format!("{}模型安装完成", model_type)
                    }));
                    Ok(Some(serde_json::json!({ "success": true, "model": model_type })))
                }
                Err(e) => {
                    hub.publish("voice-setup", serde_json::json!({
                        "phase": "model", "status": "error",
                        "model": &model_type,
                        "message": format!("{}模型安装失败: {}", model_type, e)
                    }));
                    Err(format!("model install failed: {}", e))
                }
            }
        }).await
            .map_err(|e| format!("install task panicked: {}", e))?;

        // Release install lock
        {
            let mut locks = install_locks().lock().unwrap();
            locks.remove(model);
        }

        // Clear cancel token
        {
            let mut guard = setup_cancel().lock().unwrap();
            *guard = None;
        }

        result
    }

    fn cmd_config_get(&self, voice_dir: &std::path::Path) -> Result<Option<serde_json::Value>, String> {
        let config_path = voice_dir.join("config.toml");
        if !config_path.exists() {
            return Ok(Some(serde_json::json!({
                "exists": false,
                "content": "",
            })));
        }
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("failed to read config: {}", e))?;
        Ok(Some(serde_json::json!({
            "exists": true,
            "content": content,
        })))
    }

    fn cmd_config_set(&self, voice_dir: &std::path::Path, content: &str) -> Result<Option<serde_json::Value>, String> {
        std::fs::create_dir_all(voice_dir)
            .map_err(|e| format!("failed to create voice dir: {}", e))?;
        let config_path = voice_dir.join("config.toml");
        std::fs::write(&config_path, content)
            .map_err(|e| format!("failed to write config: {}", e))?;
        Ok(Some(serde_json::json!({ "success": true })))
    }

    fn cmd_voice_config_get(&self, config_dir: &std::path::Path) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(read_voice_config(config_dir)))
    }

    fn cmd_voice_config_set(&self, config_dir: &std::path::Path, data: &serde_json::Value) -> Result<Option<serde_json::Value>, String> {
        let path = ensure_voice_config(config_dir);
        let mut current = read_voice_config(config_dir);

        if let Some(obj) = current.as_object_mut() {
            if let Some(v) = data.get("speaker_id").and_then(|v| v.as_u64()) {
                obj.insert("speaker_id".to_string(), serde_json::json!(v));
            }
            if let Some(v) = data.get("volume").and_then(|v| v.as_u64()) {
                obj.insert("volume".to_string(), serde_json::json!(v));
            }
            if let Some(v) = data.get("speed").and_then(|v| v.as_f64()) {
                obj.insert("speed".to_string(), serde_json::json!(v));
            }
            if let Some(v) = data.get("capture_device") {
                obj.insert("capture_device".to_string(), v.clone());
            }
            if let Some(v) = data.get("playback_device") {
                obj.insert("playback_device".to_string(), v.clone());
            }
            if let Some(v) = data.get("stt_enabled") {
                obj.insert("stt_enabled".to_string(), v.clone());
            }
            if let Some(v) = data.get("tts_enabled") {
                obj.insert("tts_enabled".to_string(), v.clone());
            }
            if let Some(v) = data.get("punct_enabled") {
                obj.insert("punct_enabled".to_string(), v.clone());
            }
        }

        let content = serde_json::to_string_pretty(&current)
            .map_err(|e| format!("failed to serialize voice config: {}", e))?;
        std::fs::write(&path, content)
            .map_err(|e| format!("failed to write voice config: {}", e))?;

        Ok(Some(serde_json::json!({ "success": true })))
    }

    async fn cmd_tts(&self, voice_dir: &std::path::Path, config_dir: &std::path::Path, data: &serde_json::Value) -> Result<Option<serde_json::Value>, String> {
        let text = data.get("text").and_then(|v| v.as_str()).ok_or("missing field: text")?.to_string();

        if text.trim().is_empty() {
            return Err("text cannot be empty".to_string());
        }
        if text.len() > 1000 {
            return Err("text too long (max 1000 characters)".to_string());
        }

        let voice_cfg = read_voice_config(config_dir);
        let default_speaker = voice_cfg.get("speaker_id").and_then(|v| v.as_u64()).unwrap_or(45) as u32;
        let default_speed = voice_cfg.get("speed").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
        let default_volume = voice_cfg.get("volume").and_then(|v| v.as_u64()).unwrap_or(50);

        let speaker_id = data.get("speaker").and_then(|v| v.as_u64()).unwrap_or(default_speaker as u64) as u32;
        let speed = data.get("speed").and_then(|v| v.as_f64()).unwrap_or(default_speed as f64) as f32;
        let volume = data.get("volume").and_then(|v| v.as_u64()).unwrap_or(default_volume);

        let dir = voice_dir.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let config_path = dir.join("config.toml");
            if !config_path.exists() {
                return Err("Voice not set up. Run setup first.".to_string());
            }

            let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);

            if !nemesis_voice::sherpa_is_initialized() {
                nemesis_voice::bootstrap::init_sherpa(&dir)
                    .map_err(|e| format!("sherpa init failed: {}", e))?;
            }

            // Use persistent engine if available, otherwise create temporary
            let (samples, sample_rate) = {
                let guard = tts_engine_state().lock().unwrap();
                if let Some(ref engine) = *guard {
                    engine.generate(&text, speaker_id, speed)
                        .map_err(|e| format!("TTS generation failed: {}", e))?
                } else {
                    drop(guard);
                    let tts_dir = nemesis_voice::model::ensure_tts_model(&cfg)
                        .map_err(|e| format!("TTS model not ready: {}", e))?;
                    let tts_engine = nemesis_voice::TtsEngine::new(&tts_dir, cfg.tts.num_threads)
                        .map_err(|e| format!("TTS engine init failed: {}", e))?;
                    tts_engine.generate(&text, speaker_id, speed)
                        .map_err(|e| format!("TTS generation failed: {}", e))?
                }
            };

            if samples.is_empty() {
                return Err("TTS generated empty audio".to_string());
            }

            let duration = samples.len() as f64 / sample_rate as f64;
            let gain = volume as f32 / 50.0 * cfg.audio.gain;

            let playback = nemesis_voice::AudioPlayback::new(&cfg.audio.playback_device, sample_rate, gain)
                .map_err(|e| format!("audio playback init failed: {}", e))?;

            playback.play_blocking(&samples, sample_rate)
                .map_err(|e| format!("audio playback failed: {}", e))?;

            Ok(Some(serde_json::json!({
                "duration": (duration * 10.0).round() / 10.0,
                "sample_rate": sample_rate,
            })))
        }).await
            .map_err(|e| format!("TTS task panicked: {}", e))?
    }

    async fn cmd_stt_start(&self, voice_dir: &std::path::Path, config_dir: &std::path::Path, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        {
            let state = stt_state().lock().await;
            if state.is_some() {
                return Err("STT dictation already running".to_string());
            }
        }

        // Ensure persistent STT engine is loaded
        let needs_load = stt_engine_state().lock().unwrap().is_none();
        if needs_load {
            self.cmd_stt_engine_start(voice_dir, config_dir).await?;
        }

        let config_path = voice_dir.join("config.toml");
        let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);
        let capture_device = cfg.audio.capture_device.clone();
        let target_sr = cfg.audio.target_sample_rate;
        let cfg_for_detector = cfg.clone();

        let cancel = CancellationToken::new();
        let session_id = ctx.session_id.clone();
        let session_mgr = ctx.state.session_manager.clone();

        {
            let mut state = stt_state().lock().await;
            *state = Some(SttSession { cancel: cancel.clone() });
        }

        let output: Box<dyn SttOutput> = Box::new(WsSttOutput {
            session_id: session_id.clone(),
            session_mgr: session_mgr.clone(),
        });

        tokio::task::spawn_blocking(move || {
            run_stt_pipeline(
                &capture_device,
                target_sr,
                &cfg_for_detector,
                &cancel,
                output.as_ref(),
            );
        });

        Ok(Some(serde_json::json!({ "started": true })))
    }

    async fn cmd_stt_stop(&self) -> Result<Option<serde_json::Value>, String> {
        let mut state = stt_state().lock().await;
        match state.take() {
            Some(session) => {
                session.cancel.cancel();
                Ok(Some(serde_json::json!({ "stopped": true })))
            }
            None => Err("STT dictation not running".to_string()),
        }
    }

    fn cmd_speakers(&self) -> Result<Option<serde_json::Value>, String> {
        let speakers: Vec<serde_json::Value> = KOKORO_SPEAKERS
            .iter()
            .map(|(id, gender, sid)| {
                serde_json::json!({
                    "id": id,
                    "gender": gender,
                    "speaker_id": sid,
                })
            })
            .collect();
        Ok(Some(serde_json::json!({ "speakers": speakers })))
    }

    fn cmd_devices(&self) -> Result<Option<serde_json::Value>, String> {
        let devices = nemesis_voice::audio::list_devices()
            .map_err(|e| format!("failed to list audio devices: {}", e))?;

        let input_devices: Vec<serde_json::Value> = devices
            .iter()
            .filter(|d| d.is_input)
            .map(|d| serde_json::json!({ "index": d.index, "name": d.name, "is_default": d.is_default }))
            .collect();

        let output_devices: Vec<serde_json::Value> = devices
            .iter()
            .filter(|d| !d.is_input)
            .map(|d| serde_json::json!({ "index": d.index, "name": d.name, "is_default": d.is_default }))
            .collect();

        Ok(Some(serde_json::json!({
            "input": input_devices,
            "output": output_devices,
            "total": devices.len(),
        })))
    }

    // -----------------------------------------------------------------------
    // Phase 1: Engine start / stop
    // -----------------------------------------------------------------------

    async fn cmd_engine_start(&self, voice_dir: &std::path::Path, config_dir: &std::path::Path, model: &str) -> Result<Option<serde_json::Value>, String> {
        match model {
            "stt" => self.cmd_stt_engine_start(voice_dir, config_dir).await,
            "tts" => self.cmd_tts_engine_start(voice_dir).await,
            _ => Err(format!("unknown model: {}", model)),
        }
    }

    async fn cmd_stt_engine_start(&self, voice_dir: &std::path::Path, config_dir: &std::path::Path) -> Result<Option<serde_json::Value>, String> {
        {
            let guard = stt_engine_state().lock().unwrap();
            if guard.is_some() {
                return Ok(Some(serde_json::json!({ "started": true, "model": "stt", "already_loaded": true })));
            }
        }

        let voice_cfg = read_voice_config(config_dir);
        let punct_enabled = voice_cfg.get("punct_enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let dir = voice_dir.to_path_buf();

        let result = tokio::task::spawn_blocking(move || {
            let config_path = dir.join("config.toml");
            if !config_path.exists() {
                return Err("config.toml not found. Run setup first.".to_string());
            }

            let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);

            if !nemesis_voice::sherpa_is_initialized() {
                nemesis_voice::bootstrap::init_sherpa(&dir)
                    .map_err(|e| format!("sherpa init failed: {}", e))?;
            }

            let stt_dir = nemesis_voice::model::ensure_stt_model(&cfg)
                .map_err(|e| format!("STT model not ready: {}", e))?;

            let use_itn = if punct_enabled {
                match nemesis_voice::model::ensure_punct_model(&cfg) {
                    Ok(_) => false,
                    Err(_) => true,
                }
            } else {
                true
            };

            let engine = nemesis_voice::SttEngine::new(&stt_dir, &cfg.stt.language, use_itn, cfg.stt.num_threads)
                .map_err(|e| format!("STT engine init failed: {}", e))?;

            Ok(engine)
        }).await
            .map_err(|e| format!("engine init panicked: {}", e))?;

        match result {
            Ok(engine) => {
                let mut guard = stt_engine_state().lock().unwrap();
                *guard = Some(engine);
                tracing::info!("[Voice] STT engine loaded and persistent");
                Ok(Some(serde_json::json!({ "started": true, "model": "stt" })))
            }
            Err(e) => Err(e),
        }
    }

    async fn cmd_tts_engine_start(&self, voice_dir: &std::path::Path) -> Result<Option<serde_json::Value>, String> {
        {
            let guard = tts_engine_state().lock().unwrap();
            if guard.is_some() {
                return Ok(Some(serde_json::json!({ "started": true, "model": "tts", "already_loaded": true })));
            }
        }

        let dir = voice_dir.to_path_buf();

        let result = tokio::task::spawn_blocking(move || {
            let config_path = dir.join("config.toml");
            if !config_path.exists() {
                return Err("config.toml not found. Run setup first.".to_string());
            }

            let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);

            if !nemesis_voice::sherpa_is_initialized() {
                nemesis_voice::bootstrap::init_sherpa(&dir)
                    .map_err(|e| format!("sherpa init failed: {}", e))?;
            }

            let tts_dir = nemesis_voice::model::ensure_tts_model(&cfg)
                .map_err(|e| format!("TTS model not ready: {}", e))?;

            let engine = nemesis_voice::TtsEngine::new(&tts_dir, cfg.tts.num_threads)
                .map_err(|e| format!("TTS engine init failed: {}", e))?;

            Ok(engine)
        }).await
            .map_err(|e| format!("engine init panicked: {}", e))?;

        match result {
            Ok(engine) => {
                let mut guard = tts_engine_state().lock().unwrap();
                *guard = Some(engine);
                tracing::info!("[Voice] TTS engine loaded and persistent");
                Ok(Some(serde_json::json!({ "started": true, "model": "tts" })))
            }
            Err(e) => Err(e),
        }
    }

    fn cmd_engine_stop(&self, model: &str) -> Result<Option<serde_json::Value>, String> {
        match model {
            "stt" => {
                let mut guard = stt_engine_state().lock().unwrap();
                if guard.take().is_some() {
                    tracing::info!("[Voice] STT engine released");
                    Ok(Some(serde_json::json!({ "stopped": true, "model": "stt" })))
                } else {
                    Ok(Some(serde_json::json!({ "stopped": true, "model": "stt", "was_loaded": false })))
                }
            }
            "tts" => {
                let mut guard = tts_engine_state().lock().unwrap();
                if guard.take().is_some() {
                    tracing::info!("[Voice] TTS engine released");
                    Ok(Some(serde_json::json!({ "stopped": true, "model": "tts" })))
                } else {
                    Ok(Some(serde_json::json!({ "stopped": true, "model": "tts", "was_loaded": false })))
                }
            }
            _ => Err(format!("unknown model: {}", model)),
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2: Pipeline start / stop (uses persistent engine)
    // -----------------------------------------------------------------------

    async fn cmd_pipeline_start(&self, voice_dir: &std::path::Path, model: &str, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        match model {
            "stt" => self.cmd_stt_pipeline_start(voice_dir, ctx).await,
            _ => Err(format!("pipeline not supported for model: {}", model)),
        }
    }

    async fn cmd_stt_pipeline_start(&self, voice_dir: &std::path::Path, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        // Check engine is loaded
        {
            let guard = stt_engine_state().lock().unwrap();
            if guard.is_none() {
                return Err("STT engine not loaded. Enable STT toggle first.".to_string());
            }
        }

        // Check no existing pipeline
        {
            let state = stt_state().lock().await;
            if state.is_some() {
                return Err("STT pipeline already running".to_string());
            }
        }

        let config_path = voice_dir.join("config.toml");
        let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);
        let capture_device = cfg.audio.capture_device.clone();
        let target_sr = cfg.audio.target_sample_rate;
        let cfg_for_detector = cfg.clone();

        let cancel = CancellationToken::new();
        let session_id = ctx.session_id.clone();
        let session_mgr = ctx.state.session_manager.clone();

        {
            let mut state = stt_state().lock().await;
            *state = Some(SttSession { cancel: cancel.clone() });
        }

        let output: Box<dyn SttOutput> = Box::new(WsSttOutput {
            session_id: session_id.clone(),
            session_mgr: session_mgr.clone(),
        });

        tokio::task::spawn_blocking(move || {
            run_stt_pipeline(
                &capture_device,
                target_sr,
                &cfg_for_detector,
                &cancel,
                output.as_ref(),
            );
        });

        Ok(Some(serde_json::json!({ "started": true })))
    }

    async fn cmd_pipeline_stop(&self, model: &str) -> Result<Option<serde_json::Value>, String> {
        match model {
            "stt" => {
                let mut state = stt_state().lock().await;
                match state.take() {
                    Some(session) => {
                        session.cancel.cancel();
                        Ok(Some(serde_json::json!({ "stopped": true })))
                    }
                    None => Err("STT pipeline not running".to_string()),
                }
            }
            _ => Err(format!("pipeline not supported for model: {}", model)),
        }
    }
}

// ---------------------------------------------------------------------------
// STT loop (blocking, creates its own engine — legacy fallback)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
#[allow(dead_code)]
fn run_stt_loop(
    stt_dir: &std::path::Path,
    language: &str,
    use_itn: bool,
    num_threads: u32,
    capture_device: &str,
    target_sample_rate: u32,
    cfg: &nemesis_voice::AppConfig,
    cancel: &CancellationToken,
    session_id: &str,
    session_mgr: Arc<crate::session::SessionManager>,
) {
    let stt_engine = match nemesis_voice::SttEngine::new(stt_dir, language, use_itn, num_threads) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[STT] Engine init failed: {}", e);
            push_stt_result(session_id, session_mgr.clone(), &format!("[错误] STT引擎初始化失败: {}", e));
            return;
        }
    };

    let mut detector = nemesis_voice::create_detector(cfg);

    let capture = match nemesis_voice::AudioCapture::new(capture_device) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[STT] Audio capture failed: {}", e);
            push_stt_result(session_id, session_mgr.clone(), &format!("[错误] 麦克风初始化失败: {}", e));
            return;
        }
    };

    let mut resampler = match nemesis_voice::Resampler::new(capture.sample_rate, target_sample_rate) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[STT] Resampler init failed: {}", e);
            push_stt_result(session_id, session_mgr.clone(), &format!("[错误] 重采样器初始化失败: {}", e));
            return;
        }
    };

    tracing::info!("[STT] Dictation started (session={}, detector={})", session_id, detector.name());
    push_stt_result(session_id, session_mgr.clone(), "[听写已开始，请说话...]");

    let mut chunk_count: u64 = 0;
    let mut speech_count: u64 = 0;

    while !cancel.is_cancelled() {
        match capture.try_receive() {
            Some(chunk) => {
                chunk_count += 1;
                let resampled = resampler.resample(&chunk);

                if let Some(speech) = detector.process(&resampled, target_sample_rate) {
                    speech_count += 1;
                    if !speech.is_empty() {
                        match stt_engine.recognize(&speech, target_sample_rate) {
                            Ok(text) => {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    push_stt_result(session_id, session_mgr.clone(), trimmed);
                                }
                            }
                            Err(e) => tracing::warn!("[STT] Recognition error: {}", e),
                        }
                    }
                }
            }
            None => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    // Flush remaining audio on exit
    if let Some(speech) = detector.flush() {
        if !speech.is_empty() {
            match stt_engine.recognize(&speech, target_sample_rate) {
                Ok(text) => {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        tracing::info!("[STT] Recognized (final flush): {}", trimmed);
                        push_stt_result(session_id, session_mgr.clone(), trimmed);
                    }
                }
                Err(e) => tracing::warn!("[STT] Final recognition error: {}", e),
            }
        }
    }

    tracing::info!("[STT] Dictation stopped (session={}, chunks={}, speech_segments={})", session_id, chunk_count, speech_count);
    push_stt_result(session_id, session_mgr.clone(), "[听写已停止]");
}

// ---------------------------------------------------------------------------
// STT pipeline (blocking, uses persistent engine)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn run_stt_pipeline(
    capture_device: &str,
    target_sr: u32,
    cfg: &nemesis_voice::AppConfig,
    cancel: &CancellationToken,
    output: &dyn SttOutput,
) {
    {
        let guard = stt_engine_state().lock().unwrap();
        if guard.is_none() {
            tracing::error!("[STT Pipeline] Engine not loaded");
            output.send_text("[错误] STT引擎未加载");
            return;
        }
    }

    let mut detector = nemesis_voice::create_detector(cfg);

    let capture = match nemesis_voice::AudioCapture::new(capture_device) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[STT Pipeline] Audio capture failed: {}", e);
            output.send_text(&format!("[错误] 麦克风初始化失败: {}", e));
            return;
        }
    };

    let mut resampler = match nemesis_voice::Resampler::new(capture.sample_rate, target_sr) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[STT Pipeline] Resampler init failed: {}", e);
            output.send_text(&format!("[错误] 重采样器初始化失败: {}", e));
            return;
        }
    };

    tracing::info!("[STT Pipeline] Started (detector={})", detector.name());
    output.send_text("[听写已开始，请说话...]");

    let mut chunk_count: u64 = 0;
    let mut speech_count: u64 = 0;

    while !cancel.is_cancelled() {
        match capture.try_receive() {
            Some(chunk) => {
                chunk_count += 1;
                let resampled = resampler.resample(&chunk);

                if let Some(speech) = detector.process(&resampled, target_sr) {
                    speech_count += 1;
                    if !speech.is_empty() {
                        let guard = stt_engine_state().lock().unwrap();
                        if let Some(ref engine) = *guard {
                            match engine.recognize(&speech, target_sr) {
                                Ok(text) => {
                                    let trimmed = text.trim();
                                    if !trimmed.is_empty() {
                                        output.send_text(trimmed);
                                    }
                                }
                                Err(e) => tracing::warn!("[STT Pipeline] Recognition error: {}", e),
                            }
                        } else {
                            tracing::error!("[STT Pipeline] Engine released during pipeline");
                            output.send_text("[错误] STT引擎已释放");
                            return;
                        }
                    }
                }
            }
            None => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    // Flush remaining audio
    if let Some(speech) = detector.flush() {
        if !speech.is_empty() {
            let guard = stt_engine_state().lock().unwrap();
            if let Some(ref engine) = *guard {
                match engine.recognize(&speech, target_sr) {
                    Ok(text) => {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            tracing::info!("[STT Pipeline] Recognized (final flush): {}", trimmed);
                            output.send_text(trimmed);
                        }
                    }
                    Err(e) => tracing::warn!("[STT Pipeline] Final recognition error: {}", e),
                }
            }
        }
    }

    tracing::info!("[STT Pipeline] Stopped (chunks={}, speech_segments={})", chunk_count, speech_count);
    output.send_text("[听写已停止]");
}

#[cfg(target_os = "windows")]
fn push_stt_result(session_id: &str, session_mgr: Arc<crate::session::SessionManager>, text: &str) {
    let msg = crate::protocol::ProtocolMessage::push(
        "voice",
        "stt_result",
        Some(serde_json::json!({ "text": text })),
    );
    if let Ok(bytes) = msg.to_json() {
        let rt = tokio::runtime::Handle::current();
        let sid = session_id.to_string();
        rt.spawn(async move {
            if let Err(e) = session_mgr.broadcast(&sid, &bytes).await {
                tracing::warn!("[STT] Failed to push result: {}", e);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn check_model_subdir_any(parent: &std::path::Path) -> bool {
    if !parent.exists() {
        return false;
    }
    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if has_onnx_file(&entry.path()) {
                    return true;
                }
            }
        }
    }
    false
}

fn has_onnx_file(dir: &std::path::Path) -> bool {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".onnx") {
                return true;
            }
        }
    }
    false
}
