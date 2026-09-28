//! Voice handler — voice environment status, setup, config, TTS test, STT dictation.
//!
//! Commands: status, check, setup, stop_setup, install_runtime, install_model,
//!           config_get, config_set, voice_config_get, voice_config_set,
//!           tts, stt_start, stt_stop, speakers, devices,
//!           engine_start, engine_stop, pipeline_start, pipeline_stop

#[cfg(target_os = "windows")]
use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
#[cfg(target_os = "windows")]
use nemesis_voice::EchoCanceller; // trait needed to call SpeexAec::process
#[cfg(target_os = "windows")]
use std::path::PathBuf;
#[cfg(target_os = "windows")]
use std::sync::Arc;
#[cfg(target_os = "windows")]
use std::sync::OnceLock;
#[cfg(target_os = "windows")]
use tokio::sync::Mutex;
#[cfg(target_os = "windows")]
use tokio_util::sync::CancellationToken;

// Kokoro multi-lang v1.1 speaker list (Chinese speakers)
#[cfg(target_os = "windows")]
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

#[cfg(target_os = "windows")]
const VOICE_CONFIG_FILENAME: &str = "config.voice.json";
#[cfg(target_os = "windows")]
const DEFAULT_VOICE_CONFIG: &str =
    include_str!("../../../../nemesisbot/config/config.voice.default.json");
// chat 配置文件名 + 默认模板由 voice_relay 模块提供（gateway 启动装配与
// handler 共用单一来源，避免双 include 漂移）。
#[cfg(target_os = "windows")]
use crate::voice_relay::{CHAT_CONFIG_FILENAME, DEFAULT_CHAT_CONFIG};

// Global setup cancellation token
#[cfg(target_os = "windows")]
fn setup_cancel() -> &'static std::sync::Mutex<Option<CancellationToken>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<CancellationToken>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

// Per-model install lock — prevents concurrent downloads of the same model.
// Simple HashSet: insert on start, remove on finish. No nesting, no deadlock.
#[cfg(target_os = "windows")]
fn install_locks() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static INSTANCE: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

#[cfg(target_os = "windows")]
fn model_label(model: &str) -> &str {
    match model {
        "stt" => "STT",
        "stt_stream" => "流式STT",
        "vad" => "VAD",
        "tts" => "TTS",
        "punct" => "标点",
        "speaker" => "声纹",
        _ => model,
    }
}

// Global STT session (only one active dictation at a time)
#[cfg(target_os = "windows")]
fn stt_state() -> &'static Arc<Mutex<Option<SttSession>>> {
    static INSTANCE: OnceLock<Arc<Mutex<Option<SttSession>>>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(Mutex::new(Option::None)))
}

#[cfg(target_os = "windows")]
#[cfg(target_os = "windows")]
struct SttSession {
    cancel: CancellationToken,
    dialogue_output: Option<Arc<DialogueSttOutput>>,
}

// Global dialogue state for reset command
#[cfg(target_os = "windows")]
fn dialogue_state() -> &'static Arc<Mutex<Option<Arc<DialogueSttOutput>>>> {
    static INSTANCE: OnceLock<Arc<Mutex<Option<Arc<DialogueSttOutput>>>>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(Mutex::new(None)))
}

// Global TTS playback manager
#[cfg(target_os = "windows")]
fn tts_playback_state() -> &'static Arc<Mutex<Option<TtsPlaybackManager>>> {
    static INSTANCE: OnceLock<Arc<Mutex<Option<TtsPlaybackManager>>>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(Mutex::new(None)))
}

#[cfg(target_os = "windows")]
struct TtsPlaybackManager {
    tx: std::sync::mpsc::Sender<TtsPlaybackItem>,
    cancel: CancellationToken,
}

#[cfg(target_os = "windows")]
struct TtsPlaybackItem {
    text: String,
    speaker_id: u32,
    speed: f32,
    volume: u64,
}

// ---------------------------------------------------------------------------
// Persistent engine state (Phase 1)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn stt_engine_state() -> &'static std::sync::Mutex<Option<nemesis_voice::SttEngine>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<nemesis_voice::SttEngine>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

/// AEC 实例（可选）。启用且库已下载时，由 cmd_stt_engine_start 装载；STT pipeline
/// 读取它做回声消除。SpeexAec 内部带攒帧缓冲，状态跨迭代保留。
#[cfg(target_os = "windows")]
fn aec_state() -> &'static std::sync::Mutex<Option<nemesis_voice::SpeexAec>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<nemesis_voice::SpeexAec>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

/// 标点引擎（可选，ct-transformer）。启用时由 cmd_stt_engine_start 装载；
/// run_stt_pipeline 在 recognize 后调 add_punctuation 给文本加标点。
#[cfg(target_os = "windows")]
fn punct_engine_state() -> &'static std::sync::Mutex<Option<nemesis_voice::PunctEngine>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<nemesis_voice::PunctEngine>>> =
        OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(target_os = "windows")]
fn tts_engine_state() -> &'static std::sync::Mutex<Option<nemesis_voice::TtsEngine>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<nemesis_voice::TtsEngine>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(target_os = "windows")]
fn speaker_engine_state() -> &'static std::sync::Mutex<Option<nemesis_voice::SpeakerEngine>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<nemesis_voice::SpeakerEngine>>> =
        OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(target_os = "windows")]
fn speaker_manager_state() -> &'static std::sync::Mutex<Option<nemesis_voice::SpeakerManager>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<nemesis_voice::SpeakerManager>>> =
        OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(target_os = "windows")]
fn speaker_enabled_state() -> &'static std::sync::Mutex<bool> {
    static INSTANCE: OnceLock<std::sync::Mutex<bool>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(false))
}

#[cfg(target_os = "windows")]
fn speaker_threshold_state() -> &'static std::sync::Mutex<f32> {
    static INSTANCE: OnceLock<std::sync::Mutex<f32>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(0.65))
}

#[cfg(target_os = "windows")]
fn speaker_register_state() -> &'static std::sync::Mutex<Option<SpeakerRegistration>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<SpeakerRegistration>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(target_os = "windows")]
struct SpeakerRegistration {
    name: String,
    samples: std::sync::Mutex<Vec<f32>>,
    sample_rate: u32,
    start_time: std::time::Instant,
    cancel: CancellationToken,
}

#[cfg(target_os = "windows")]
struct SpeakerTestSession {
    cancel: CancellationToken,
    auto_loaded_stt: bool,
}

#[cfg(target_os = "windows")]
fn speaker_test_state() -> &'static tokio::sync::Mutex<Option<SpeakerTestSession>> {
    static INSTANCE: OnceLock<tokio::sync::Mutex<Option<SpeakerTestSession>>> = OnceLock::new();
    INSTANCE.get_or_init(|| tokio::sync::Mutex::new(None))
}

#[cfg(target_os = "windows")]
const VOICEPRINT_CONFIG_FILENAME: &str = "config.voice.print.json";
#[cfg(target_os = "windows")]
const DEFAULT_SPEAKER_THRESHOLD: f32 = 0.65;

// ---------------------------------------------------------------------------
// Realtime 共享态（barge-in 需要 tts_playback_loop 与 STT 管线跨任务共享
// 播放进度；tts_relay 入队需要可复用的播放通道）
// ---------------------------------------------------------------------------

/// 当前播放的停止句柄（barge-in 跨线程切播放用；播放间隙为 None）。
/// AudioPlayback 本体因 cpal Stream 是 !Send——播放循环线程持本体，
/// STT 管线线程经 StopHandle（Send+Sync，只触队列）打断。
#[cfg(target_os = "windows")]
fn active_playback() -> &'static std::sync::Mutex<Option<nemesis_voice::StopHandle>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<nemesis_voice::StopHandle>>> =
        OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

/// 播放中旗（tts_playback_loop 置位/清位；barge-in 播放窗口判定读）。
#[cfg(target_os = "windows")]
fn tts_playing() -> &'static std::sync::atomic::AtomicBool {
    static INSTANCE: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::atomic::AtomicBool::new(false))
}

/// 最近一次播放结束时刻（barge-in 尾窗 300ms 允许抢最后一句话）。
#[cfg(target_os = "windows")]
fn tts_last_play_end() -> &'static std::sync::Mutex<Option<std::time::Instant>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<std::time::Instant>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

/// barge-in 判定常量（G4）：最短持续防咳嗽/桌响误触发；尾窗允许抢最后一句话。
#[cfg(target_os = "windows")]
const BARGE_MIN_SUSTAIN: std::time::Duration = std::time::Duration::from_millis(250);
#[cfg(target_os = "windows")]
const BARGE_TAIL: std::time::Duration = std::time::Duration::from_millis(300);

/// realtime 管线参数（对话模式由 cmd_stt_dialogue_start 装配；听写/输入盒
/// 路径传 None = 逐字节旧行为）。
#[cfg(target_os = "windows")]
struct RealtimePipelineParams {
    stream_stt: bool,
    /// true = partial 实时 + VAD 段尾离线精识（默认）；false = final 直接用流式结果。
    two_pass: bool,
    barge_in: bool,
    /// 接力注册快照（barge-in 取消当前轮用；tts_relay 的入队在出站路径，
    /// 不经管线）。
    registration: Option<Arc<crate::voice_relay::RelayRegistration>>,
}

// ---------------------------------------------------------------------------
// STT output interface (Phase 3)
// ---------------------------------------------------------------------------

/// Trait for routing STT recognition results.
#[cfg(target_os = "windows")]
trait SttOutput: Send {
    fn send_text(&self, text: &str);

    /// 流式 partial 草稿（realtime stream_stt；默认实现 = no-op，听写/
    /// 输入盒路径不受影响）。
    fn send_partial(&self, _text: &str) {}

    /// barge-in 已触发通知（前端给对应气泡打「已打断」badge）。
    fn notify_barge_in(&self) {}
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

/// STT output for dictation mode: pushes recognized text to the input box via WebSocket.
#[cfg(target_os = "windows")]
struct InputBoxSttOutput {
    push_fn: Box<dyn Fn(&str) + Send + Sync>,
}

#[cfg(target_os = "windows")]
impl SttOutput for InputBoxSttOutput {
    fn send_text(&self, text: &str) {
        (self.push_fn)(text);
    }
}

/// STT output for dialogue mode: accumulates text, auto-sends after silence timeout.
#[cfg(target_os = "windows")]
struct DialogueSttOutput {
    push_fn: Box<dyn Fn(&str) + Send + Sync>,
    state: Arc<std::sync::Mutex<DialogueState>>,
}

#[cfg(target_os = "windows")]
#[allow(dead_code)]
struct DialogueState {
    buffer: String,
    silence_timeout_secs: f64,
    reset_flag: bool,
}

#[cfg(target_os = "windows")]
impl SttOutput for DialogueSttOutput {
    fn send_text(&self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with('[') {
            return; // Skip status messages like "[听写已开始]"
        }

        let mut state = self.state.lock().unwrap();
        if state.reset_flag {
            state.buffer.clear();
            state.reset_flag = false;
        }
        if !state.buffer.is_empty() {
            state.buffer.push(' ');
        }
        state.buffer.push_str(trimmed);

        let accumulated = state.buffer.clone();
        drop(state);

        // Push accumulated text to frontend (replaces input box content)
        (self.push_fn)(&format!("accumulate:{}", accumulated));
    }

    fn send_partial(&self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        // partial 是草稿：只上屏（前端灰字预览），不进累积缓冲——
        // 缓冲仍只收 VAD 段尾 final。
        (self.push_fn)(&format!("partial:{}", text));
    }

    fn notify_barge_in(&self) {
        (self.push_fn)("barge_in:1");
    }
}

#[cfg(target_os = "windows")]
impl DialogueSttOutput {
    /// Get current accumulated text and clear buffer. Called on silence timeout or manual stop.
    fn flush(&self) -> Option<String> {
        let mut state = self.state.lock().unwrap();
        if state.buffer.is_empty() {
            return None;
        }
        let text = state.buffer.clone();
        state.buffer.clear();
        Some(text)
    }

    /// Reset accumulation buffer and timer (called when user manually sends).
    fn reset(&self) {
        let mut state = self.state.lock().unwrap();
        state.buffer.clear();
        state.reset_flag = true;
    }
}

/// Wrapper to use DialogueSttOutput as SttOutput trait object.
#[cfg(target_os = "windows")]
struct DialogueSttOutputWrapper {
    inner: Arc<DialogueSttOutput>,
}

#[cfg(target_os = "windows")]
impl SttOutput for DialogueSttOutputWrapper {
    fn send_text(&self, text: &str) {
        self.inner.send_text(text);
    }

    fn send_partial(&self, text: &str) {
        self.inner.send_partial(text);
    }

    fn notify_barge_in(&self) {
        self.inner.notify_barge_in();
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

impl Default for VoiceHandler {
    fn default() -> Self {
        Self::new()
    }
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

    fn commands(&self) -> &'static [&'static str] {
        &[
            "status",
            "check",
            "setup",
            "stop_setup",
            "install_runtime",
            "install_aec",
            "install_model",
            "config_get",
            "config_set",
            "voice_config_get",
            "voice_config_set",
            "tts",
            "stt_start",
            "stt_stop",
            "speakers",
            "devices",
            "engine_status",
            "chat_config_get",
            "chat_config_set",
            "engine_start",
            "engine_stop",
            "pipeline_start",
            "pipeline_stop",
            "stt_to_input_start",
            "stt_to_input_stop",
            "stt_dialogue_start",
            "stt_dialogue_stop",
            "stt_dialogue_reset",
            "tts_playback",
            "tts_playback_stop",
            "speaker_status",
            "speaker_register_start",
            "speaker_register_stop",
            "speaker_register_cancel",
            "speaker_remove",
            "speaker_list",
            "speaker_test_start",
            "speaker_test_stop",
            "speaker_set_threshold",
        ]
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
                "install_aec" => self.cmd_install_aec(&voice_dir, ctx).await,
                "install_model" => {
                    let d = data.ok_or("missing data")?;
                    let model = d
                        .get("model")
                        .and_then(|v| v.as_str())
                        .ok_or("missing field: model")?;
                    self.cmd_install_model(&voice_dir, model, ctx).await
                }
                "config_get" => self.cmd_config_get(&voice_dir),
                "config_set" => {
                    let d = data.ok_or("missing data")?;
                    let content = d
                        .get("content")
                        .and_then(|v| v.as_str())
                        .ok_or("missing field: content")?;
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
                "engine_status" => self.cmd_engine_status(&voice_dir).await,
                "chat_config_get" => self.cmd_chat_config_get(&config_dir),
                "chat_config_set" => {
                    let d = data.ok_or("missing data")?;
                    self.cmd_chat_config_set(&config_dir, &d)
                }
                "engine_start" => {
                    let d = data.ok_or("missing data")?;
                    let model = d
                        .get("model")
                        .and_then(|v| v.as_str())
                        .ok_or("missing field: model")?;
                    self.cmd_engine_start(&voice_dir, &config_dir, model).await
                }
                "engine_stop" => {
                    let d = data.ok_or("missing data")?;
                    let model = d
                        .get("model")
                        .and_then(|v| v.as_str())
                        .ok_or("missing field: model")?;
                    self.cmd_engine_stop(model)
                }
                "pipeline_start" => {
                    let d = data.ok_or("missing data")?;
                    let model = d
                        .get("model")
                        .and_then(|v| v.as_str())
                        .ok_or("missing field: model")?;
                    self.cmd_pipeline_start(&voice_dir, model, ctx).await
                }
                "pipeline_stop" => {
                    let d = data.ok_or("missing data")?;
                    let model = d
                        .get("model")
                        .and_then(|v| v.as_str())
                        .ok_or("missing field: model")?;
                    self.cmd_pipeline_stop(model).await
                }
                "stt_to_input_start" => self.cmd_stt_to_input_start(&voice_dir, ctx).await,
                "stt_to_input_stop" => self.cmd_stt_to_input_stop().await,
                "stt_dialogue_start" => {
                    let timeout = data
                        .as_ref()
                        .and_then(|d| d.get("silence_timeout"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(3.0);
                    // realtime 接力寻址（G6）：会话 id 由前端随启动请求带上
                    let dialogue_session = data
                        .as_ref()
                        .and_then(|d| d.get("session_id"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    self.cmd_stt_dialogue_start(
                        &voice_dir,
                        &config_dir,
                        ctx,
                        timeout,
                        dialogue_session,
                    )
                    .await
                }
                "stt_dialogue_stop" => self.cmd_stt_dialogue_stop().await,
                "stt_dialogue_reset" => self.cmd_stt_dialogue_reset().await,
                "tts_playback" => {
                    let d = data.ok_or("missing data")?;
                    self.cmd_tts_playback(&voice_dir, &config_dir, &d).await
                }
                "tts_playback_stop" => self.cmd_tts_playback_stop().await,
                // Speaker verification commands
                "speaker_status" => self.cmd_speaker_status(&config_dir).await,
                "speaker_register_start" => {
                    let d = data.ok_or("missing data")?;
                    let name = d.get("name").and_then(|v| v.as_str()).unwrap_or("owner");
                    self.ensure_speaker_engine(&voice_dir, &config_dir).await?;
                    self.cmd_speaker_register_start(&config_dir, name)
                }
                "speaker_register_stop" => self.cmd_speaker_register_stop(&config_dir),
                "speaker_register_cancel" => self.cmd_speaker_register_cancel(),
                "speaker_remove" => {
                    let d = data.ok_or("missing data")?;
                    let name = d
                        .get("name")
                        .and_then(|v| v.as_str())
                        .ok_or("missing field: name")?;
                    self.cmd_speaker_remove(&config_dir, name)
                }
                "speaker_list" => self.cmd_speaker_list(&config_dir),
                "speaker_test_start" => {
                    self.ensure_speaker_engine(&voice_dir, &config_dir).await?;
                    self.cmd_speaker_test_start(&voice_dir, &config_dir, ctx)
                        .await
                }
                "speaker_test_stop" => self.cmd_speaker_test_stop().await,
                "speaker_set_threshold" => {
                    let d = data.ok_or("missing data")?;
                    let threshold = d
                        .get("threshold")
                        .and_then(|v| v.as_f64())
                        .ok_or("missing field: threshold")?;
                    self.cmd_speaker_set_threshold(&config_dir, threshold as f32)
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

#[cfg(target_os = "windows")]
fn ensure_chat_config(config_dir: &std::path::Path) -> PathBuf {
    let path = config_dir.join(CHAT_CONFIG_FILENAME);
    if !path.exists() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, DEFAULT_CHAT_CONFIG);
    }
    path
}

#[cfg(target_os = "windows")]
fn read_chat_config(config_dir: &std::path::Path) -> serde_json::Value {
    let path = ensure_chat_config(config_dir);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::from_str(DEFAULT_CHAT_CONFIG).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Voiceprint config helpers (config.voice.print.json)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn voiceprint_path(config_dir: &std::path::Path) -> PathBuf {
    config_dir.join(VOICEPRINT_CONFIG_FILENAME)
}

#[cfg(target_os = "windows")]
fn read_voiceprint_config(config_dir: &std::path::Path) -> serde_json::Value {
    let path = voiceprint_path(config_dir);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| {
            serde_json::json!({
                "threshold": DEFAULT_SPEAKER_THRESHOLD,
                "speakers": {}
            })
        })
}

#[cfg(target_os = "windows")]
fn write_voiceprint_config(
    config_dir: &std::path::Path,
    data: &serde_json::Value,
) -> Result<(), String> {
    let path = voiceprint_path(config_dir);
    // 首写前建父目录：读路径（read_voiceprint_config）容忍文件/目录缺失回退默认，
    // 写路径此前直接 fs::write，config/ 目录不存在时首写会以裸 OS error 3 失败
    // （BUG #18，与同文件 ensure_voice_config 的 create_dir_all 行为对齐）。
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = serde_json::to_string_pretty(data)
        .map_err(|e| format!("failed to serialize voiceprint config: {}", e))?;
    // REL-002：统一原子写入（声纹配置）。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
        .map_err(|e| format!("failed to write voiceprint config: {}", e))
}

/// Push speaker verification rejected notification to frontend.
/// Used by ChatPanel to display "⚠ 声纹验证未通过" warning.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
fn push_speaker_rejected(session_id: &str, session_mgr: Arc<crate::session::SessionManager>) {
    let msg = crate::protocol::ProtocolMessage::push(
        "voice",
        "speaker_rejected",
        Some(serde_json::json!({ "message": "声纹验证未通过，语音输入已忽略" })),
    );
    if let Ok(bytes) = msg.to_json() {
        let rt = tokio::runtime::Handle::current();
        let sid = session_id.to_string();
        rt.spawn(async move {
            let _ = session_mgr.broadcast(&sid, &bytes).await;
        });
    }
}

// ---------------------------------------------------------------------------
// Graceful shutdown — cancel all active voice sessions and release engines
// ---------------------------------------------------------------------------

/// Cancel all active voice sessions and release all engines.
/// Called during gateway shutdown to ensure spawn_blocking tasks exit promptly.
pub async fn voice_shutdown() {
    #[cfg(target_os = "windows")]
    {
        // 1. Cancel active STT session (dictation / dialogue)
        {
            let mut state = stt_state().lock().await;
            if let Some(session) = state.take() {
                tracing::info!("[Voice] Cancelling active STT session");
                session.cancel.cancel();
            }
        }

        // 2. Cancel active speaker registration
        {
            let mut reg = speaker_register_state().lock().unwrap();
            if let Some(r) = reg.take() {
                tracing::info!("[Voice] Cancelling speaker registration");
                r.cancel.cancel();
            }
        }

        // 2b. Cancel active speaker test
        {
            let mut test = speaker_test_state().lock().await;
            if let Some(t) = test.take() {
                tracing::info!("[Voice] Cancelling speaker test");
                t.cancel.cancel();
            }
        }

        // 3. Cancel active TTS playback
        {
            let mut pb = tts_playback_state().lock().await;
            if let Some(manager) = pb.take() {
                tracing::info!("[Voice] Cancelling TTS playback");
                manager.cancel.cancel();
            }
        }

        // 4. Release engines (drops ONNX sessions, stops ONNX thread pool)
        {
            let mut eng = stt_engine_state().lock().unwrap();
            if eng.is_some() {
                tracing::info!("[Voice] Releasing STT engine");
                *eng = None;
            }
        }
        {
            let mut eng = tts_engine_state().lock().unwrap();
            if eng.is_some() {
                tracing::info!("[Voice] Releasing TTS engine");
                *eng = None;
            }
        }
        {
            let mut eng = speaker_engine_state().lock().unwrap();
            if eng.is_some() {
                tracing::info!("[Voice] Releasing speaker engine");
                *eng = None;
            }
        }
        {
            let mut mgr = speaker_manager_state().lock().unwrap();
            if mgr.is_some() {
                tracing::info!("[Voice] Releasing speaker manager");
                *mgr = None;
            }
        }
        {
            let mut eng = punct_engine_state().lock().unwrap();
            if eng.is_some() {
                tracing::info!("[Voice] Releasing punctuation engine");
                *eng = None;
            }
        }

        tracing::info!("[Voice] Shutdown complete");
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Nothing to clean up on non-Windows platforms
    }
}

/// 启动时按 `config.voice.json` 自动初始化已启用的语音引擎（STT / TTS / speaker）。
///
/// 在 gateway 接受 web 请求**之前**同步调用——这样 dashboard 任何 `engine_status`
/// 查询拿到的都是真值，前端 chat 页一次询问就能正确反映按钮可用状态，无需轮询。
/// 引擎加载失败只 warn、不阻断启动（engine_state 留空，status 如实返回 not-ready，
/// 对应按钮保持灰——这是真实的"引擎没起来"，不是假状态）。
pub async fn init_engines_from_config(workspace: &std::path::Path) {
    #[cfg(target_os = "windows")]
    {
        let voice_dir = workspace.join("tools").join("voice");
        let config_dir = workspace.join("config");
        let voice_cfg = read_voice_config(&config_dir);
        let stt_enabled = voice_cfg
            .get("stt_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let tts_enabled = voice_cfg
            .get("tts_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let speaker_enabled = voice_cfg
            .get("speaker_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let handler = VoiceHandler::new();
        if stt_enabled {
            match handler.cmd_stt_engine_start(&voice_dir, &config_dir).await {
                Ok(_) => tracing::info!("[Voice] auto-init: STT engine ready"),
                Err(e) => tracing::warn!(
                    "[Voice] auto-init: STT engine failed (chat STT buttons stay disabled): {}",
                    e
                ),
            }
        }
        if tts_enabled {
            match handler.cmd_tts_engine_start(&voice_dir).await {
                Ok(_) => tracing::info!("[Voice] auto-init: TTS engine ready"),
                Err(e) => tracing::warn!("[Voice] auto-init: TTS engine failed: {}", e),
            }
        }
        if speaker_enabled {
            match handler
                .cmd_speaker_engine_start(&voice_dir, &config_dir)
                .await
            {
                Ok(_) => tracing::info!("[Voice] auto-init: speaker engine ready"),
                Err(e) => tracing::warn!("[Voice] auto-init: speaker engine failed: {}", e),
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = workspace;
    }
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
        let speaker_model = model_dir.join("speaker");

        let stt_ready = check_model_subdir_any(&stt_model);
        let vad_ready = check_model_subdir_any(&vad_model);
        let tts_ready = check_model_subdir_any(&tts_model);
        let punct_ready = check_model_subdir_any(&punct_model);
        let speaker_ready = check_model_subdir_any(&speaker_model);

        // AEC 库（可选，单独下载到 tools/voice/aec/）
        let aec_lib = voice_dir.join("aec").join("aec.dll");
        let aec_ready = aec_lib.exists();

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
                "speaker": { "ready": speaker_ready, "path": speaker_model.to_string_lossy() },
            },
            "aec": { "ready": aec_ready, "path": aec_lib.to_string_lossy() },
        })))
    }

    async fn cmd_setup(
        &self,
        voice_dir: &std::path::Path,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
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
            hub.publish(
                "voice-setup",
                serde_json::json!({
                    "phase": "setup", "status": "starting",
                    "message": "开始安装语音环境..."
                }),
            );

            if cancel.is_cancelled() {
                hub.publish(
                    "voice-setup",
                    serde_json::json!({
                        "phase": "setup", "status": "cancelled",
                        "message": "安装已取消"
                    }),
                );
                return Err("setup cancelled".to_string());
            }

            match nemesis_voice::bootstrap_run_in_dir(&config_path, &dir) {
                Ok(_) => {
                    hub.publish(
                        "voice-setup",
                        serde_json::json!({
                            "phase": "setup", "status": "complete",
                            "message": "语音环境安装完成"
                        }),
                    );
                    Ok(Some(serde_json::json!({ "success": true })))
                }
                Err(e) => {
                    hub.publish(
                        "voice-setup",
                        serde_json::json!({
                            "phase": "setup", "status": "error",
                            "message": format!("安装失败: {}", e)
                        }),
                    );
                    Err(format!("setup failed: {}", e))
                }
            }
        })
        .await
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

    async fn cmd_install_runtime(
        &self,
        voice_dir: &std::path::Path,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
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

            hub.publish(
                "voice-setup",
                serde_json::json!({
                    "phase": "runtime", "status": "starting",
                    "message": "正在安装运行库..."
                }),
            );

            if cancel.is_cancelled() {
                hub.publish(
                    "voice-setup",
                    serde_json::json!({
                        "phase": "runtime", "status": "cancelled",
                        "message": "安装已取消"
                    }),
                );
                return Err("cancelled".to_string());
            }

            match nemesis_voice::bootstrap_run_in_dir(&config_path, &dir) {
                Ok(_) => {
                    hub.publish(
                        "voice-setup",
                        serde_json::json!({
                            "phase": "runtime", "status": "complete",
                            "message": "运行库安装完成"
                        }),
                    );
                    Ok(Some(serde_json::json!({ "success": true })))
                }
                Err(e) => {
                    hub.publish(
                        "voice-setup",
                        serde_json::json!({
                            "phase": "runtime", "status": "error",
                            "message": format!("运行库安装失败: {}", e)
                        }),
                    );
                    Err(format!("runtime install failed: {}", e))
                }
            }
        })
        .await
        .map_err(|e| format!("install task panicked: {}", e))?;

        {
            let mut guard = setup_cancel().lock().unwrap();
            *guard = None;
        }

        result
    }

    async fn cmd_install_aec(
        &self,
        voice_dir: &std::path::Path,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let aec_dir = voice_dir.join("aec");
        let config_path = voice_dir.join("config.toml");
        let proxy_url = nemesis_voice::AppConfig::load_or_default(&config_path)
            .models
            .proxy
            .url
            .clone();
        let dir = aec_dir.clone();
        let hub = ctx.state.event_hub.clone();

        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("failed to create aec dir: {}", e))?;
            hub.publish(
                "voice-setup",
                serde_json::json!({
                    "phase": "aec", "status": "starting", "message": "正在安装回声消除库..."
                }),
            );
            match nemesis_voice::download_aec_lib(&dir, &proxy_url) {
                Ok(p) => {
                    hub.publish(
                        "voice-setup",
                        serde_json::json!({
                            "phase": "aec", "status": "complete", "message": "回声消除库安装完成"
                        }),
                    );
                    Ok(Some(
                        serde_json::json!({ "success": true, "path": p.to_string_lossy() }),
                    ))
                }
                Err(e) => {
                    hub.publish(
                        "voice-setup",
                        serde_json::json!({
                            "phase": "aec", "status": "error",
                            "message": format!("回声消除库安装失败: {}", e)
                        }),
                    );
                    Err(format!("aec install failed: {}", e))
                }
            }
        })
        .await
        .map_err(|e| format!("install task panicked: {}", e))?
    }

    async fn cmd_install_model(
        &self,
        voice_dir: &std::path::Path,
        model: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
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
                    hub_progress.publish(
                        "voice-setup",
                        serde_json::json!({
                            "phase": "model", "status": "progress",
                            "model": &mt,
                            "message": msg,
                        }),
                    );
                })));
            }

            hub.publish(
                "voice-setup",
                serde_json::json!({
                    "phase": "model", "status": "starting",
                    "model": &model_type,
                    "message": format!("正在安装{}模型...", model_type)
                }),
            );

            if cancel.is_cancelled() {
                hub.publish(
                    "voice-setup",
                    serde_json::json!({
                        "phase": "model", "status": "cancelled",
                        "message": "安装已取消"
                    }),
                );
                nemesis_voice::set_progress(None);
                return Err("cancelled".to_string());
            }

            let result = match model_type.as_str() {
                "stt" => nemesis_voice::model::ensure_stt_model(&cfg).map_err(|e| e.to_string()),
                // 流式 STT（realtime stream_stt）：zipformer 在线转写四件套
                "stt_stream" => {
                    nemesis_voice::model::ensure_stt_stream_model(&cfg).map_err(|e| e.to_string())
                }
                "vad" => nemesis_voice::model::ensure_vad_model(&cfg).map_err(|e| e.to_string()),
                "tts" => nemesis_voice::model::ensure_tts_model(&cfg).map_err(|e| e.to_string()),
                "punct" => {
                    nemesis_voice::model::ensure_punct_model(&cfg).map_err(|e| e.to_string())
                }
                "speaker" => {
                    nemesis_voice::model::ensure_speaker_model(&cfg).map_err(|e| e.to_string())
                }
                _ => Err(format!("unknown model type: {}", model_type)),
            };

            nemesis_voice::set_progress(None);

            let result: Result<(), String> = result.map(|_| ());

            match result {
                Ok(()) => {
                    hub.publish(
                        "voice-setup",
                        serde_json::json!({
                            "phase": "model", "status": "complete",
                            "model": &model_type,
                            "message": format!("{}模型安装完成", model_type)
                        }),
                    );
                    Ok(Some(
                        serde_json::json!({ "success": true, "model": model_type }),
                    ))
                }
                Err(e) => {
                    hub.publish(
                        "voice-setup",
                        serde_json::json!({
                            "phase": "model", "status": "error",
                            "model": &model_type,
                            "message": format!("{}模型安装失败: {}", model_type, e)
                        }),
                    );
                    Err(format!("model install failed: {}", e))
                }
            }
        })
        .await
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

    fn cmd_config_get(
        &self,
        voice_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>, String> {
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

    fn cmd_config_set(
        &self,
        voice_dir: &std::path::Path,
        content: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        std::fs::create_dir_all(voice_dir)
            .map_err(|e| format!("failed to create voice dir: {}", e))?;
        let config_path = voice_dir.join("config.toml");
        // REL-002：统一原子写入（voice config.toml）。
        nemesis_utils::write_file_atomic(&config_path.to_string_lossy(), content.as_bytes(), 0o600)
            .map_err(|e| format!("failed to write config: {}", e))?;
        Ok(Some(serde_json::json!({ "success": true })))
    }

    fn cmd_voice_config_get(
        &self,
        config_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(read_voice_config(config_dir)))
    }

    fn cmd_voice_config_set(
        &self,
        config_dir: &std::path::Path,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
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
            if let Some(v) = data.get("speaker_enabled") {
                obj.insert("speaker_enabled".to_string(), v.clone());
            }
            if let Some(v) = data.get("silence_timeout") {
                obj.insert("silence_timeout".to_string(), v.clone());
            }
            if let Some(v) = data.get("aec_enabled") {
                obj.insert("aec_enabled".to_string(), v.clone());
            }
        }

        let content = serde_json::to_string_pretty(&current)
            .map_err(|e| format!("failed to serialize voice config: {}", e))?;
        // REL-002：统一原子写入（config.voice.json）。
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
            .map_err(|e| format!("failed to write voice config: {}", e))?;

        Ok(Some(serde_json::json!({ "success": true })))
    }

    async fn cmd_tts(
        &self,
        voice_dir: &std::path::Path,
        config_dir: &std::path::Path,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let text = data
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("missing field: text")?
            .to_string();

        if text.trim().is_empty() {
            return Err("text cannot be empty".to_string());
        }
        if text.len() > 1000 {
            return Err("text too long (max 1000 characters)".to_string());
        }

        let voice_cfg = read_voice_config(config_dir);
        let default_speaker = voice_cfg
            .get("speaker_id")
            .and_then(|v| v.as_u64())
            .unwrap_or(45) as u32;
        let default_speed = voice_cfg
            .get("speed")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;
        let default_volume = voice_cfg
            .get("volume")
            .and_then(|v| v.as_u64())
            .unwrap_or(50);

        let speaker_id = data
            .get("speaker")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_speaker as u64) as u32;
        let speed = data
            .get("speed")
            .and_then(|v| v.as_f64())
            .unwrap_or(default_speed as f64) as f32;
        let volume = data
            .get("volume")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_volume);

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
                    engine
                        .generate(&text, speaker_id, speed)
                        .map_err(|e| format!("TTS generation failed: {}", e))?
                } else {
                    drop(guard);
                    let tts_dir = nemesis_voice::model::ensure_tts_model(&cfg)
                        .map_err(|e| format!("TTS model not ready: {}", e))?;
                    let tts_engine = nemesis_voice::TtsEngine::new(&tts_dir, cfg.tts.num_threads)
                        .map_err(|e| format!("TTS engine init failed: {}", e))?;
                    tts_engine
                        .generate(&text, speaker_id, speed)
                        .map_err(|e| format!("TTS generation failed: {}", e))?
                }
            };

            if samples.is_empty() {
                return Err("TTS generated empty audio".to_string());
            }

            let duration = samples.len() as f64 / sample_rate as f64;
            let gain = volume as f32 / 50.0 * cfg.audio.gain;

            let playback =
                nemesis_voice::AudioPlayback::new(&cfg.audio.playback_device, sample_rate, gain)
                    .map_err(|e| format!("audio playback init failed: {}", e))?;

            playback
                .play_blocking(&samples, sample_rate)
                .map_err(|e| format!("audio playback failed: {}", e))?;

            Ok(Some(serde_json::json!({
                "duration": (duration * 10.0).round() / 10.0,
                "sample_rate": sample_rate,
            })))
        })
        .await
        .map_err(|e| format!("TTS task panicked: {}", e))?
    }

    async fn cmd_stt_start(
        &self,
        voice_dir: &std::path::Path,
        config_dir: &std::path::Path,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
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
            *state = Some(SttSession {
                cancel: cancel.clone(),
                dialogue_output: None,
            });
        }

        let output: Box<dyn SttOutput> = Box::new(WsSttOutput {
            session_id: session_id.clone(),
            session_mgr: session_mgr.clone(),
        });

        tokio::task::spawn_blocking(move || {
            let mut source = match nemesis_voice::MicSource::new(&capture_device, target_sr) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("[STT Pipeline] Audio capture failed: {}", e);
                    output.send_text(&format!("[错误] 麦克风初始化失败: {}", e));
                    return;
                }
            };
            run_stt_pipeline(
                &mut source,
                target_sr,
                &cfg_for_detector,
                &cancel,
                output.as_ref(),
                None,
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

    async fn cmd_engine_status(
        &self,
        voice_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>, String> {
        #[cfg(target_os = "windows")]
        {
            let stt_ready = stt_engine_state().lock().unwrap().is_some();
            let tts_ready = tts_engine_state().lock().unwrap().is_some();
            let speaker_ready = speaker_engine_state().lock().unwrap().is_some();
            // 流式 STT 模型在位探测（realtime stream_stt 开关可用性；只探测不下载）
            let config_path = voice_dir.join("config.toml");
            let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);
            let stt_stream_model = nemesis_voice::model::probe_stt_stream_model(&cfg).is_some();
            let stt_dialogue_active = {
                let state = stt_state().lock().await;
                state.as_ref().is_some_and(|s| s.dialogue_output.is_some())
            };
            Ok(Some(serde_json::json!({
                "stt_ready": stt_ready,
                "tts_ready": tts_ready,
                "speaker_ready": speaker_ready,
                "stt_stream_model": stt_stream_model,
                "stt_dialogue_active": stt_dialogue_active,
            })))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = voice_dir;
            Ok(Some(serde_json::json!({
                "stt_ready": false,
                "tts_ready": false,
                "speaker_ready": false,
                "stt_stream_model": false,
            })))
        }
    }

    fn cmd_chat_config_get(
        &self,
        config_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(read_chat_config(config_dir)))
    }

    fn cmd_chat_config_set(
        &self,
        config_dir: &std::path::Path,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = ensure_chat_config(config_dir);
        let content = serde_json::to_string_pretty(data)
            .map_err(|e| format!("failed to serialize chat config: {}", e))?;
        // REL-002：统一原子写入（chat config）。
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
            .map_err(|e| format!("failed to write chat config: {}", e))?;
        // realtime 热态同步（G6）：写盘成功即 apply（缺 realtime 键 = no-op，
        // 不动现有热态）。管线循环/出站广播读 AtomicBool，下一拍生效。
        if let Some(rt) = data.get("realtime") {
            crate::voice_relay::apply_realtime_config(rt);
        }
        Ok(Some(serde_json::json!({ "success": true })))
    }

    // -----------------------------------------------------------------------
    // Phase 1: Engine start / stop
    // -----------------------------------------------------------------------

    async fn cmd_engine_start(
        &self,
        voice_dir: &std::path::Path,
        config_dir: &std::path::Path,
        model: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        match model {
            "stt" => self.cmd_stt_engine_start(voice_dir, config_dir).await,
            "tts" => self.cmd_tts_engine_start(voice_dir).await,
            "speaker" => self.cmd_speaker_engine_start(voice_dir, config_dir).await,
            _ => Err(format!("unknown model: {}", model)),
        }
    }

    async fn cmd_stt_engine_start(
        &self,
        voice_dir: &std::path::Path,
        config_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>, String> {
        {
            let guard = stt_engine_state().lock().unwrap();
            if guard.is_some() {
                return Ok(Some(
                    serde_json::json!({ "started": true, "model": "stt", "already_loaded": true }),
                ));
            }
        }

        let voice_cfg = read_voice_config(config_dir);
        let punct_enabled = voice_cfg
            .get("punct_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let aec_enabled = voice_cfg
            .get("aec_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // AEC 进阶参数：房间预设映射到 filter_length（16kHz 下采样数）。
        // 越界/缺省回退默认值；非法值会让 AecNew 返回 null，由下方 match 降级为关闭。
        let aec_filter_length = voice_cfg
            .get("aec_filter_length")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32)
            .filter(|&v| (160..=65536).contains(&v))
            .unwrap_or(nemesis_voice::DEFAULT_FILTER_LENGTH);
        let aec_preprocess = voice_cfg
            .get("aec_preprocess")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
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
                    Ok(punct_dir) => {
                        // 加载 ct-transformer 标点引擎，STT recognize 后给文本加标点
                        let model_file = punct_dir.join("model.onnx");
                        match nemesis_voice::PunctEngine::new(&model_file, cfg.stt.num_threads) {
                            Ok(pe) => {
                                *punct_engine_state().lock().unwrap() = Some(pe);
                                tracing::info!("[Voice] punctuation engine loaded (ct-transformer)");
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "[Voice] punct engine init failed, STT output won't be punctuated: {}",
                                    e
                                );
                                *punct_engine_state().lock().unwrap() = None;
                            }
                        }
                        false // 用 ct-transformer，关掉 SenseVoice 自带 ITN 避免重复标点
                    }
                    Err(_) => true, // 标点模型没下，回退 SenseVoice 自带 ITN
                }
            } else {
                *punct_engine_state().lock().unwrap() = None;
                true
            };

            let engine = nemesis_voice::SttEngine::new(&stt_dir, &cfg.stt.model_name, &cfg.stt.language, cfg.stt.lang_remedy, use_itn, cfg.stt.num_threads)
                .map_err(|e| format!("STT engine init failed: {}", e))?;

            // AEC：启用且库已下载时，加载 dll 并创建 SpeexAec 实例（采样率跟随 target_sr）。
            // 失败只警告、不阻断 STT（回声消除降级为关闭）。
            if aec_enabled {
                let aec_dll = dir.join("aec").join("aec.dll");
                if aec_dll.exists() {
                    match nemesis_voice::aec::init(&aec_dll).and_then(|_| {
                        nemesis_voice::SpeexAec::new(
                            nemesis_voice::DEFAULT_FRAME_SIZE,
                            aec_filter_length,
                            cfg.audio.target_sample_rate,
                            aec_preprocess,
                        )
                    }) {
                        Ok(aec) => {
                            *aec_state().lock().unwrap() = Some(aec);
                            // 起系统混音 loopback 采集，给 AEC 喂 far-end 参考（含 TTS+RustDesk+所有播放声）
                            nemesis_voice::start_loopback();
                            tracing::info!(
                                "[Voice] AEC enabled (SpeexDSP, sr={}, filter_length={}, preprocess={})",
                                cfg.audio.target_sample_rate, aec_filter_length, aec_preprocess
                            );
                        }
                        Err(e) => {
                            tracing::warn!("[Voice] AEC init failed, echo cancellation disabled: {}", e);
                            *aec_state().lock().unwrap() = None;
                            nemesis_voice::stop_loopback();
                        }
                    }
                } else {
                    tracing::info!(
                        "[Voice] AEC enabled but lib not downloaded (tools/voice/aec/aec.dll); echo cancellation off"
                    );
                    *aec_state().lock().unwrap() = None;
                    nemesis_voice::stop_loopback();
                }
            } else {
                *aec_state().lock().unwrap() = None;
                nemesis_voice::stop_loopback();
            }

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

    async fn cmd_tts_engine_start(
        &self,
        voice_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>, String> {
        {
            let guard = tts_engine_state().lock().unwrap();
            if guard.is_some() {
                return Ok(Some(
                    serde_json::json!({ "started": true, "model": "tts", "already_loaded": true }),
                ));
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
        })
        .await
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

    /// Auto-load speaker engine if not already loaded. Called before register/test operations.
    async fn ensure_speaker_engine(
        &self,
        voice_dir: &std::path::Path,
        config_dir: &std::path::Path,
    ) -> Result<(), String> {
        {
            let guard = speaker_engine_state().lock().unwrap();
            if guard.is_some() {
                return Ok(());
            }
        }
        self.cmd_speaker_engine_start(voice_dir, config_dir).await?;
        Ok(())
    }

    async fn cmd_speaker_engine_start(
        &self,
        voice_dir: &std::path::Path,
        config_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>, String> {
        {
            let guard = speaker_engine_state().lock().unwrap();
            if guard.is_some() {
                return Ok(Some(
                    serde_json::json!({ "started": true, "model": "speaker", "already_loaded": true }),
                ));
            }
        }

        let dir = voice_dir.to_path_buf();
        let cfg_dir = config_dir.to_path_buf();

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

            let speaker_dir = nemesis_voice::model::ensure_speaker_model(&cfg)
                .map_err(|e| format!("Speaker model not ready: {}", e))?;

            let engine = nemesis_voice::SpeakerEngine::new(&speaker_dir, cfg.speaker.num_threads)
                .map_err(|e| format!("Speaker engine init failed: {}", e))?;

            Ok((engine, cfg_dir))
        })
        .await
        .map_err(|e| format!("engine init panicked: {}", e))?;

        match result {
            Ok((engine, cfg_dir)) => {
                let dim = engine.embedding_dim();
                let manager = nemesis_voice::SpeakerManager::new(dim);

                // Load threshold from voiceprint config
                let vp_cfg = read_voiceprint_config(&cfg_dir);
                if let Some(t) = vp_cfg.get("threshold").and_then(|v| v.as_f64()) {
                    *speaker_threshold_state().lock().unwrap() = t as f32;
                }

                // Load registered speakers
                let mut mgr = manager;
                if let Some(speakers) = vp_cfg.get("speakers").and_then(|v| v.as_object()) {
                    for (name, data) in speakers {
                        if let Some(arr) = data.get("embedding").and_then(|v| v.as_array()) {
                            let embedding: Vec<f32> = arr
                                .iter()
                                .filter_map(|v| v.as_f64().map(|f| f as f32))
                                .collect();
                            if embedding.len() == dim as usize {
                                mgr.register(name, &embedding);
                            }
                        }
                    }
                }

                {
                    let mut engine_guard = speaker_engine_state().lock().unwrap();
                    *engine_guard = Some(engine);
                }
                {
                    let mut manager_guard = speaker_manager_state().lock().unwrap();
                    *manager_guard = Some(mgr);
                }
                *speaker_enabled_state().lock().unwrap() = true;
                tracing::info!("[Voice] Speaker engine loaded (dim={})", dim);
                Ok(Some(
                    serde_json::json!({ "started": true, "model": "speaker" }),
                ))
            }
            Err(e) => Err(e),
        }
    }

    // -----------------------------------------------------------------------
    // Speaker verification commands
    // -----------------------------------------------------------------------

    async fn cmd_speaker_status(
        &self,
        config_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>, String> {
        let enabled = *speaker_enabled_state().lock().unwrap();
        #[cfg(target_os = "windows")]
        let ready = speaker_engine_state().lock().unwrap().is_some();
        #[cfg(not(target_os = "windows"))]
        let ready = false;
        // Read threshold from file for accurate persisted value
        let threshold = {
            let vp_cfg = read_voiceprint_config(config_dir);
            vp_cfg
                .get("threshold")
                .and_then(|v| v.as_f64())
                .unwrap_or(DEFAULT_SPEAKER_THRESHOLD as f64) as f32
        };
        #[cfg(target_os = "windows")]
        let stt_dialogue_active = {
            let state = stt_state().lock().await;
            state.as_ref().is_some_and(|s| s.dialogue_output.is_some())
        };
        #[cfg(not(target_os = "windows"))]
        let stt_dialogue_active = false;
        #[cfg(target_os = "windows")]
        let speakers: Vec<String> = {
            let vp_cfg = read_voiceprint_config(config_dir);
            vp_cfg
                .get("speakers")
                .and_then(|v| v.as_object())
                .map(|obj| obj.keys().cloned().collect())
                .unwrap_or_default()
        };
        #[cfg(not(target_os = "windows"))]
        let speakers: Vec<String> = vec![];
        Ok(Some(serde_json::json!({
            "enabled": enabled,
            "ready": ready,
            "threshold": threshold,
            "speakers": speakers,
            "stt_dialogue_active": stt_dialogue_active,
        })))
    }

    fn cmd_speaker_register_start(
        &self,
        config_dir: &std::path::Path,
        name: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        #[cfg(target_os = "windows")]
        {
            {
                let mut reg = speaker_register_state().lock().unwrap();
                if reg.is_some() {
                    return Err("Speaker registration already in progress".to_string());
                }

                let cancel = CancellationToken::new();
                let cancel_clone = cancel.clone();

                // Read audio config for sample rate and device
                let voice_dir = config_dir
                    .parent()
                    .map(|p| p.join("tools").join("voice"))
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let config_path = voice_dir.join("config.toml");
                let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);
                let target_sr = cfg.audio.target_sample_rate;
                let device = cfg.audio.capture_device.clone();

                *reg = Some(SpeakerRegistration {
                    name: name.to_string(),
                    samples: std::sync::Mutex::new(Vec::new()),
                    sample_rate: target_sr,
                    start_time: std::time::Instant::now(),
                    cancel,
                });

                // Spawn background capture
                tokio::task::spawn_blocking(move || {
                    let capture = match nemesis_voice::AudioCapture::new(if device.is_empty() {
                        ""
                    } else {
                        &device
                    }) {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::error!("[Speaker] Capture init failed: {}", e);
                            // Clear registration state on failure
                            let mut reg = speaker_register_state().lock().unwrap();
                            *reg = None;
                            return;
                        }
                    };

                    let mut resampler =
                        match nemesis_voice::Resampler::new(capture.sample_rate, target_sr) {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::error!("[Speaker] Resampler init failed: {}", e);
                                let mut reg = speaker_register_state().lock().unwrap();
                                *reg = None;
                                return;
                            }
                        };

                    while !cancel_clone.is_cancelled() {
                        if let Some(samples) = capture.try_receive() {
                            let resampled = resampler.resample(&samples);
                            let reg = speaker_register_state().lock().unwrap();
                            if let Some(r) = reg.as_ref() {
                                if let Ok(mut s) = r.samples.lock() {
                                    s.extend_from_slice(&resampled);
                                }
                            } else {
                                break;
                            }
                        } else {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                });

                // Start a timeout: auto-stop at 20 seconds
                let _cfg_dir = config_dir.to_path_buf();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    // Check if still recording
                    let should_stop = {
                        let reg = speaker_register_state().lock().unwrap();
                        reg.is_some()
                    };
                    if should_stop {
                        // Send auto-stop notification via push
                        tracing::info!("[Speaker] Registration auto-stopped at 20s");
                    }
                });
            }
            Ok(Some(serde_json::json!({ "recording": true, "name": name })))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (config_dir, name);
            Err("Not supported on this platform".to_string())
        }
    }

    fn cmd_speaker_register_stop(
        &self,
        config_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>, String> {
        #[cfg(target_os = "windows")]
        {
            let reg = speaker_register_state()
                .lock()
                .unwrap()
                .take()
                .ok_or("No registration in progress")?;

            let elapsed = reg.start_time.elapsed().as_secs_f32();
            let samples = reg.samples.lock().unwrap().clone();
            reg.cancel.cancel();
            if elapsed < 5.0 {
                return Err(format!("录音时间过短（{:.1}秒），至少需要5秒", elapsed));
            }
            if samples.is_empty() {
                return Err("未录到音频数据".to_string());
            }

            let name = reg.name.clone();
            let sample_rate = reg.sample_rate;

            // Extract embedding
            let engine_guard = speaker_engine_state().lock().unwrap();
            let engine = engine_guard.as_ref().ok_or("Speaker engine not loaded")?;
            let embedding = engine
                .embed(&samples, sample_rate)
                .map_err(|e| format!("Embedding extraction failed: {}", e))?;

            // Register in manager
            let mut manager_guard = speaker_manager_state().lock().unwrap();
            let manager = manager_guard.as_mut().ok_or("Speaker manager not loaded")?;
            manager.remove(&name); // Remove existing if re-registering
            manager.register(&name, &embedding);

            // Persist to config.voice.print.json
            let mut vp_cfg = read_voiceprint_config(config_dir);
            if vp_cfg.get("speakers").is_none() {
                vp_cfg["speakers"] = serde_json::json!({});
            }
            if let Some(obj) = vp_cfg.get_mut("speakers").and_then(|v| v.as_object_mut()) {
                obj.insert(
                    name.clone(),
                    serde_json::json!({
                        "embedding": embedding,
                        "created_at": chrono::Local::now().to_rfc3339(),
                    }),
                );
            }
            write_voiceprint_config(config_dir, &vp_cfg)?;

            tracing::info!(
                "[Speaker] Registered '{}' ({} samples, {:.1}s)",
                name,
                samples.len(),
                elapsed
            );
            Ok(Some(serde_json::json!({
                "registered": true,
                "name": name,
                "duration": elapsed,
                "sample_count": samples.len(),
            })))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = config_dir;
            Err("Not supported on this platform".to_string())
        }
    }

    fn cmd_speaker_register_cancel(&self) -> Result<Option<serde_json::Value>, String> {
        let mut reg = speaker_register_state().lock().unwrap();
        if let Some(r) = reg.take() {
            r.cancel.cancel();
        }
        Ok(Some(serde_json::json!({ "cancelled": true })))
    }

    fn cmd_speaker_remove(
        &self,
        config_dir: &std::path::Path,
        name: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        #[cfg(target_os = "windows")]
        {
            let mut manager_guard = speaker_manager_state().lock().unwrap();
            if let Some(manager) = manager_guard.as_mut() {
                manager.remove(name);
            }
        }
        let mut vp_cfg = read_voiceprint_config(config_dir);
        if let Some(speakers) = vp_cfg.get_mut("speakers").and_then(|v| v.as_object_mut()) {
            speakers.remove(name);
        }
        write_voiceprint_config(config_dir, &vp_cfg)?;
        tracing::info!("[Speaker] Removed '{}'", name);
        Ok(Some(serde_json::json!({ "removed": true, "name": name })))
    }

    fn cmd_speaker_list(
        &self,
        config_dir: &std::path::Path,
    ) -> Result<Option<serde_json::Value>, String> {
        #[cfg(target_os = "windows")]
        {
            let vp_cfg = read_voiceprint_config(config_dir);
            let speakers: Vec<String> = vp_cfg
                .get("speakers")
                .and_then(|v| v.as_object())
                .map(|obj| obj.keys().cloned().collect())
                .unwrap_or_default();
            Ok(Some(serde_json::json!({ "speakers": speakers })))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = config_dir;
            Ok(Some(serde_json::json!({ "speakers": [] as Vec<String> })))
        }
    }

    fn cmd_speaker_set_threshold(
        &self,
        config_dir: &std::path::Path,
        threshold: f32,
    ) -> Result<Option<serde_json::Value>, String> {
        if !(0.0..=1.0).contains(&threshold) {
            return Err("Threshold must be between 0.0 and 1.0".to_string());
        }
        *speaker_threshold_state().lock().unwrap() = threshold;
        let mut vp_cfg = read_voiceprint_config(config_dir);
        vp_cfg["threshold"] = serde_json::json!(threshold);
        write_voiceprint_config(config_dir, &vp_cfg)?;
        Ok(Some(serde_json::json!({ "threshold": threshold })))
    }

    async fn cmd_speaker_test_start(
        &self,
        voice_dir: &std::path::Path,
        config_dir: &std::path::Path,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        #[cfg(target_os = "windows")]
        {
            // Auto-load STT engine if not already loaded
            let auto_loaded_stt = {
                let guard = stt_engine_state().lock().unwrap();
                guard.is_none()
            };
            if auto_loaded_stt {
                self.cmd_stt_engine_start(voice_dir, config_dir)
                    .await
                    .map_err(|e| format!("STT engine auto-load failed: {}", e))?;
            }

            // Load registered owner embedding
            let vp_cfg = read_voiceprint_config(config_dir);
            let owner_embedding: Vec<f32> = vp_cfg
                .get("speakers")
                .and_then(|s| s.get("owner"))
                .and_then(|o| o.get("embedding"))
                .and_then(|e| serde_json::from_value(e.clone()).ok())
                .unwrap_or_default();
            if owner_embedding.is_empty() {
                return Err("未注册声纹，请先录制声纹".to_string());
            }

            // Check not already testing
            {
                let state = speaker_test_state().lock().await;
                if state.is_some() {
                    return Err("测试已在进行中".to_string());
                }
            }

            let cancel = CancellationToken::new();
            {
                let mut state = speaker_test_state().lock().await;
                *state = Some(SpeakerTestSession {
                    cancel: cancel.clone(),
                    auto_loaded_stt,
                });
            }

            let session_id = ctx.session_id.to_string();
            let session_mgr = ctx.state.session_manager.clone();
            let dir = voice_dir.to_path_buf();
            let threshold = *speaker_threshold_state().lock().unwrap();
            let owner_emb = owner_embedding;

            tokio::task::spawn_blocking(move || {
                let config_path = dir.join("config.toml");
                let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);
                let target_sr = cfg.audio.target_sample_rate;
                let capture_device = if cfg.audio.capture_device.is_empty() {
                    ""
                } else {
                    &cfg.audio.capture_device
                };

                let mut detector = nemesis_voice::create_detector(&cfg);

                let capture = match nemesis_voice::AudioCapture::new(capture_device) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("[Speaker Test] Audio capture failed: {}", e);
                        return;
                    }
                };

                let mut resampler =
                    match nemesis_voice::Resampler::new(capture.sample_rate, target_sr) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!("[Speaker Test] Resampler failed: {}", e);
                            return;
                        }
                    };

                tracing::info!("[Speaker Test] Started");

                while !cancel.is_cancelled() {
                    match capture.try_receive() {
                        Some(chunk) => {
                            let resampled = resampler.resample(&chunk);

                            if let Some(speech) = detector.process(&resampled, target_sr)
                                && !speech.is_empty()
                            {
                                // Extract embedding and compute similarity
                                let similarity = {
                                    let engine_guard = speaker_engine_state().lock().unwrap();
                                    if let Some(ref engine) = *engine_guard {
                                        match engine.embed(&speech, target_sr) {
                                            Ok(emb) => {
                                                nemesis_voice::cosine_similarity(&emb, &owner_emb)
                                            }
                                            Err(_) => -1.0,
                                        }
                                    } else {
                                        -1.0
                                    }
                                };

                                // STT recognition
                                let text = {
                                    let stt_guard = stt_engine_state().lock().unwrap();
                                    if let Some(ref engine) = *stt_guard {
                                        engine.recognize(&speech, target_sr).unwrap_or_default()
                                    } else {
                                        String::new()
                                    }
                                };

                                let trimmed = text.trim();
                                if similarity >= 0.0 {
                                    let display_text = if trimmed.is_empty() {
                                        "(无文字识别)"
                                    } else {
                                        trimmed
                                    };
                                    let matched = similarity >= threshold;
                                    let msg = crate::protocol::ProtocolMessage::push(
                                        "voice",
                                        "speaker_test_result",
                                        Some(serde_json::json!({
                                            "text": display_text,
                                            "similarity": similarity,
                                            "matched": matched,
                                        })),
                                    );
                                    if let Ok(bytes) = msg.to_json() {
                                        let sid = session_id.clone();
                                        let smgr = session_mgr.clone();
                                        tokio::runtime::Handle::current().spawn(async move {
                                            let _ = smgr.broadcast(&sid, &bytes).await;
                                        });
                                    }
                                }
                            }
                        }
                        None => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                }

                // Flush remaining
                if let Some(speech) = detector.flush()
                    && !speech.is_empty()
                {
                    let similarity = {
                        let engine_guard = speaker_engine_state().lock().unwrap();
                        if let Some(ref engine) = *engine_guard {
                            match engine.embed(&speech, target_sr) {
                                Ok(emb) => nemesis_voice::cosine_similarity(&emb, &owner_emb),
                                Err(_) => -1.0,
                            }
                        } else {
                            -1.0
                        }
                    };
                    let text = {
                        let stt_guard = stt_engine_state().lock().unwrap();
                        if let Some(ref engine) = *stt_guard {
                            engine.recognize(&speech, target_sr).unwrap_or_default()
                        } else {
                            String::new()
                        }
                    };
                    let trimmed = text.trim();
                    if similarity >= 0.0 {
                        let display_text = if trimmed.is_empty() {
                            "(无文字识别)"
                        } else {
                            trimmed
                        };
                        let matched = similarity >= threshold;
                        let msg = crate::protocol::ProtocolMessage::push(
                            "voice",
                            "speaker_test_result",
                            Some(
                                serde_json::json!({ "text": display_text, "similarity": similarity, "matched": matched }),
                            ),
                        );
                        if let Ok(bytes) = msg.to_json() {
                            let sid = session_id.clone();
                            let smgr = session_mgr.clone();
                            tokio::runtime::Handle::current().spawn(async move {
                                let _ = smgr.broadcast(&sid, &bytes).await;
                            });
                        }
                    }
                }

                tracing::info!("[Speaker Test] Stopped");
            });

            Ok(Some(serde_json::json!({ "started": true })))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (voice_dir, config_dir, ctx);
            Err("Not supported on this platform".to_string())
        }
    }

    async fn cmd_speaker_test_stop(&self) -> Result<Option<serde_json::Value>, String> {
        let mut state = speaker_test_state().lock().await;
        match state.take() {
            Some(session) => {
                session.cancel.cancel();
                // Release STT engine if auto-loaded for this test
                if session.auto_loaded_stt {
                    let mut guard = stt_engine_state().lock().unwrap();
                    guard.take();
                    tracing::info!("[Speaker Test] Released auto-loaded STT engine");
                }
                tracing::info!("[Speaker Test] Cancelled");
                Ok(Some(serde_json::json!({ "stopped": true })))
            }
            None => Err("No speaker test running".to_string()),
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
                    Ok(Some(
                        serde_json::json!({ "stopped": true, "model": "stt", "was_loaded": false }),
                    ))
                }
            }
            "tts" => {
                let mut guard = tts_engine_state().lock().unwrap();
                if guard.take().is_some() {
                    tracing::info!("[Voice] TTS engine released");
                    Ok(Some(serde_json::json!({ "stopped": true, "model": "tts" })))
                } else {
                    Ok(Some(
                        serde_json::json!({ "stopped": true, "model": "tts", "was_loaded": false }),
                    ))
                }
            }
            "speaker" => {
                {
                    let mut engine_guard = speaker_engine_state().lock().unwrap();
                    engine_guard.take();
                }
                {
                    let mut manager_guard = speaker_manager_state().lock().unwrap();
                    manager_guard.take();
                }
                *speaker_enabled_state().lock().unwrap() = false;
                tracing::info!("[Voice] Speaker engine and manager released");
                Ok(Some(
                    serde_json::json!({ "stopped": true, "model": "speaker" }),
                ))
            }
            _ => Err(format!("unknown model: {}", model)),
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2: Pipeline start / stop (uses persistent engine)
    // -----------------------------------------------------------------------

    async fn cmd_pipeline_start(
        &self,
        voice_dir: &std::path::Path,
        model: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match model {
            "stt" => self.cmd_stt_pipeline_start(voice_dir, ctx).await,
            _ => Err(format!("pipeline not supported for model: {}", model)),
        }
    }

    async fn cmd_stt_pipeline_start(
        &self,
        voice_dir: &std::path::Path,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
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
            *state = Some(SttSession {
                cancel: cancel.clone(),
                dialogue_output: None,
            });
        }

        let output: Box<dyn SttOutput> = Box::new(WsSttOutput {
            session_id: session_id.clone(),
            session_mgr: session_mgr.clone(),
        });

        tokio::task::spawn_blocking(move || {
            let mut source = match nemesis_voice::MicSource::new(&capture_device, target_sr) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("[STT Pipeline] Audio capture failed: {}", e);
                    output.send_text(&format!("[错误] 麦克风初始化失败: {}", e));
                    return;
                }
            };
            run_stt_pipeline(
                &mut source,
                target_sr,
                &cfg_for_detector,
                &cancel,
                output.as_ref(),
                None,
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

    // -------------------------------------------------------------------
    // Dictation mode: STT → input box
    // -------------------------------------------------------------------

    async fn cmd_stt_to_input_start(
        &self,
        voice_dir: &std::path::Path,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // Ensure no existing pipeline
        {
            let state = stt_state().lock().await;
            if state.is_some() {
                return Err("STT already running. Stop current session first.".to_string());
            }
        }

        // Ensure persistent STT engine is loaded
        {
            let guard = stt_engine_state().lock().unwrap();
            if guard.is_none() {
                return Err(
                    "STT engine not loaded. Enable STT in voice settings first.".to_string()
                );
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
            *state = Some(SttSession {
                cancel: cancel.clone(),
                dialogue_output: None,
            });
        }

        let sid_clone = session_id.clone();
        let smgr_clone = session_mgr.clone();
        let push_fn: Box<dyn Fn(&str) + Send + Sync> = Box::new(move |text: &str| {
            push_stt_to_input(&sid_clone, smgr_clone.clone(), text);
        });

        let output: Box<dyn SttOutput> = Box::new(InputBoxSttOutput { push_fn });

        tokio::task::spawn_blocking(move || {
            let mut source = match nemesis_voice::MicSource::new(&capture_device, target_sr) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("[STT Pipeline] Audio capture failed: {}", e);
                    output.send_text(&format!("[错误] 麦克风初始化失败: {}", e));
                    return;
                }
            };
            run_stt_pipeline(
                &mut source,
                target_sr,
                &cfg_for_detector,
                &cancel,
                output.as_ref(),
                None,
            );
        });

        Ok(Some(serde_json::json!({ "started": true })))
    }

    async fn cmd_stt_to_input_stop(&self) -> Result<Option<serde_json::Value>, String> {
        let mut state = stt_state().lock().await;
        match state.take() {
            Some(session) => {
                session.cancel.cancel();
                // 防御性清 relay 注册（听写与对话共用 stt_state 槽，API 直调
                // stt_to_input 时避免遗留过期注册）。
                crate::voice_relay::set_relay_registration(None);
                Ok(Some(serde_json::json!({ "stopped": true })))
            }
            None => Err("STT dictation not running".to_string()),
        }
    }

    // -------------------------------------------------------------------
    // Dialogue mode: STT → accumulate → auto-send on silence
    // -------------------------------------------------------------------

    async fn cmd_stt_dialogue_start(
        &self,
        voice_dir: &std::path::Path,
        config_dir: &std::path::Path,
        ctx: &RequestContext,
        silence_timeout: f64,
        dialogue_session: Option<String>,
    ) -> Result<Option<serde_json::Value>, String> {
        // Ensure no existing pipeline
        {
            let state = stt_state().lock().await;
            if state.is_some() {
                return Err("STT already running. Stop current session first.".to_string());
            }
        }

        // Ensure persistent STT engine is loaded
        {
            let guard = stt_engine_state().lock().unwrap();
            if guard.is_none() {
                return Err(
                    "STT engine not loaded. Enable STT in voice settings first.".to_string()
                );
            }
        }

        let config_path = voice_dir.join("config.toml");
        let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);
        let capture_device = cfg.audio.capture_device.clone();
        let target_sr = cfg.audio.target_sample_rate;
        let cfg_for_detector = cfg.clone();

        // realtime 装配（G6）：热态开 + 前端给了会话 id → 注册接力 + 管线
        // 参数。resolve 失败（项目会话目录消失等）= 降级为 agent_loop:None
        // （barge-in 只停播不取消轮，管线照常跑）。
        let hot = crate::voice_relay::realtime_hot_state();
        let mut registration: Option<Arc<crate::voice_relay::RelayRegistration>> = None;
        let mut rt_params: Option<RealtimePipelineParams> = None;
        if hot.enabled.load(std::sync::atomic::Ordering::Relaxed) {
            match dialogue_session.as_deref().map(|sid| {
                // 与入站 chokepoint（server.rs send_to_session）同构：sid 过
                // sanitize 再拼 session_key——relay 靠精确匹配，两处构造必须
                // 共享同一变换（UUID 下 sanitize 恒等，但形态一变两边就漂移，
                // 症状是接力静默不接管）。
                format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(sid)
                )
            }) {
                Some(session_key) => {
                    let agent_loop =
                        match crate::handlers::projects::resolve_session_loop(ctx, &session_key) {
                            Ok(al) => Some(al),
                            Err(e) => {
                                tracing::warn!(
                                    "[Voice] realtime 接力注册无 loop（打断只停播不取消轮）: {}",
                                    e
                                );
                                None
                            }
                        };
                    registration = Some(Arc::new(crate::voice_relay::RelayRegistration {
                        session_key,
                        voice_dir: voice_dir.to_path_buf(),
                        config_dir: config_dir.to_path_buf(),
                        agent_loop,
                    }));
                    rt_params = Some(RealtimePipelineParams {
                        stream_stt: hot.stream_stt.load(std::sync::atomic::Ordering::Relaxed),
                        two_pass: hot.two_pass.load(std::sync::atomic::Ordering::Relaxed),
                        barge_in: hot.barge_in.load(std::sync::atomic::Ordering::Relaxed),
                        registration: registration.clone(),
                    });
                }
                None => {
                    tracing::warn!(
                        "[Voice] realtime 开但请求未带 session_id，本轮无接力/打断（旧前端？）"
                    );
                }
            }
        }
        crate::voice_relay::set_relay_registration(registration);

        let cancel = CancellationToken::new();
        let session_id = ctx.session_id.clone();
        let session_mgr = ctx.state.session_manager.clone();

        let sid_clone = session_id.clone();
        let smgr_clone = session_mgr.clone();
        let push_fn: Box<dyn Fn(&str) + Send + Sync> = Box::new(move |text: &str| {
            if let Some(rest) = text.strip_prefix("accumulate:") {
                push_stt_dialogue(&sid_clone, smgr_clone.clone(), "stt_accumulate", rest);
            } else if let Some(rest) = text.strip_prefix("partial:") {
                push_stt_dialogue(&sid_clone, smgr_clone.clone(), "stt_partial", rest);
            } else if text == "barge_in:1" {
                push_stt_dialogue(&sid_clone, smgr_clone.clone(), "barge_in_triggered", "1");
            } else {
                push_stt_dialogue(&sid_clone, smgr_clone.clone(), "stt_dialogue_text", text);
            }
        });

        let dialogue_output = Arc::new(DialogueSttOutput {
            push_fn,
            state: Arc::new(std::sync::Mutex::new(DialogueState {
                buffer: String::new(),
                silence_timeout_secs: silence_timeout,
                reset_flag: false,
            })),
        });

        // Store dialogue output for reset command
        {
            let mut ds = dialogue_state().lock().await;
            *ds = Some(dialogue_output.clone());
        }

        {
            let mut state = stt_state().lock().await;
            *state = Some(SttSession {
                cancel: cancel.clone(),
                dialogue_output: Some(dialogue_output.clone()),
            });
        }

        let output: Box<dyn SttOutput> = Box::new(DialogueSttOutputWrapper {
            inner: dialogue_output,
        });

        // Spawn silence timer that checks accumulated text periodically
        let cancel_clone = cancel.clone();
        let sid_timer = session_id.clone();
        let smgr_timer = session_mgr.clone();
        let dialogue_for_timer = {
            let ds = dialogue_state().lock().await;
            ds.clone()
        };

        tokio::spawn(async move {
            let check_interval = std::time::Duration::from_millis(500);
            let timeout_secs = silence_timeout;
            let mut last_text_len = 0usize;
            let mut silence_start: Option<std::time::Instant> = None;

            loop {
                tokio::time::sleep(check_interval).await;
                if cancel_clone.is_cancelled() {
                    break;
                }

                let Some(ref dlg) = dialogue_for_timer else {
                    break;
                };
                let state = dlg.state.lock().unwrap();
                let current_len = state.buffer.len();
                let reset = state.reset_flag;
                drop(state);

                if reset {
                    last_text_len = 0;
                    silence_start = None;
                    continue;
                }

                if current_len > 0 && current_len != last_text_len {
                    // New text arrived, reset silence timer
                    last_text_len = current_len;
                    silence_start = Some(std::time::Instant::now());
                } else if current_len > 0 && current_len == last_text_len {
                    // No new text, check silence timeout
                    if let Some(start) = silence_start
                        && start.elapsed().as_secs_f64() >= timeout_secs
                    {
                        // Timeout: flush and auto-send
                        let text = dlg.flush();
                        if let Some(text) = text {
                            last_text_len = 0;
                            silence_start = None;
                            push_stt_dialogue(
                                &sid_timer,
                                smgr_timer.clone(),
                                "stt_auto_send",
                                &text,
                            );
                        }
                    }
                }
            }
        });

        // 采集源（W7 注入面）在管线线程内构造：cpal Stream 是 !Send，源从不过
        // 线程边界；失败经 output.send_text 上报（对话模式吞 [ 前缀状态消息 =
        // 与旧管线内失败同表现）。
        tokio::task::spawn_blocking(move || {
            let mut source = match nemesis_voice::MicSource::new(&capture_device, target_sr) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("[STT Pipeline] Audio capture failed: {}", e);
                    output.send_text(&format!("[错误] 麦克风初始化失败: {}", e));
                    return;
                }
            };
            run_stt_pipeline(
                &mut source,
                target_sr,
                &cfg_for_detector,
                &cancel,
                output.as_ref(),
                rt_params.as_ref(),
            );
        });

        Ok(Some(serde_json::json!({ "started": true })))
    }

    async fn cmd_stt_dialogue_stop(&self) -> Result<Option<serde_json::Value>, String> {
        let mut state = stt_state().lock().await;
        match state.take() {
            Some(session) => {
                // Flush remaining text (timer task may have already sent it)
                if let Some(ref output) = session.dialogue_output {
                    let _ = output.flush();
                }
                session.cancel.cancel();
                // 清 relay 注册（G3）：出站广播即刻回前端自播；迟到的 dispatch
                // 因注册槽空而丢弃。
                crate::voice_relay::set_relay_registration(None);
                // 接力播放一并停（关=立刻安静）：清注册只挡住后续入队，已入
                // 队未播的句子会继续合成播完——「关了还在说话」。停播语义与
                // barge-in 一致（当前音频清队列即刻静音 + 撤整个播放会话）。
                // 本函数是 async 上下文：撤队列用 lock().await（trigger_barge_in
                // 在 spawn_blocking 音频线程才用 blocking_lock，两处不同）。
                if let Some(p) = active_playback().lock().unwrap().as_ref() {
                    p.stop();
                }
                let mut mgr = tts_playback_state().lock().await;
                if let Some(m) = mgr.take() {
                    m.cancel.cancel();
                }
                // Clear dialogue state
                let mut ds = dialogue_state().lock().await;
                *ds = None;
                Ok(Some(serde_json::json!({ "stopped": true })))
            }
            None => Err("STT dialogue not running".to_string()),
        }
    }

    async fn cmd_stt_dialogue_reset(&self) -> Result<Option<serde_json::Value>, String> {
        let ds = dialogue_state().lock().await;
        if let Some(ref output) = *ds {
            output.reset();
            Ok(Some(serde_json::json!({ "reset": true })))
        } else {
            Err("No dialogue session active".to_string())
        }
    }

    // -------------------------------------------------------------------
    // TTS playback: frontend-driven queue
    // -------------------------------------------------------------------

    async fn cmd_tts_playback(
        &self,
        voice_dir: &std::path::Path,
        config_dir: &std::path::Path,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let text = data
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("missing field: text")?
            .to_string();
        if text.trim().is_empty() {
            return Err("text cannot be empty".to_string());
        }

        let voice_cfg = read_voice_config(config_dir);
        let default_speaker = voice_cfg
            .get("speaker_id")
            .and_then(|v| v.as_u64())
            .unwrap_or(45) as u32;
        let default_speed = voice_cfg
            .get("speed")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;
        let default_volume = voice_cfg
            .get("volume")
            .and_then(|v| v.as_u64())
            .unwrap_or(50);

        let speaker_id = data
            .get("speaker")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_speaker as u64) as u32;
        let speed = data
            .get("speed")
            .and_then(|v| v.as_f64())
            .unwrap_or(default_speed as f64) as f32;
        let volume = data
            .get("volume")
            .and_then(|v| v.as_u64())
            .unwrap_or(default_volume);

        tts_enqueue_item(voice_dir, text, speaker_id, speed, volume).await?;

        Ok(Some(serde_json::json!({ "queued": true })))
    }

    async fn cmd_tts_playback_stop(&self) -> Result<Option<serde_json::Value>, String> {
        let mut mgr = tts_playback_state().lock().await;
        match mgr.take() {
            Some(m) => {
                m.cancel.cancel();
                // Drop tx to signal the playback loop to exit
                Ok(Some(serde_json::json!({ "stopped": true })))
            }
            None => Ok(Some(
                serde_json::json!({ "stopped": true, "was_running": false }),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// TTS enqueue（前端 tts_playback 与 realtime tts_relay 共用）
// ---------------------------------------------------------------------------

/// 入队一条 TTS 播放（懒建播放通道：首次调用 spawn 播放循环，之后复用）。
#[cfg(target_os = "windows")]
async fn tts_enqueue_item(
    voice_dir: &std::path::Path,
    text: String,
    speaker_id: u32,
    speed: f32,
    volume: u64,
) -> Result<(), String> {
    let mut mgr = tts_playback_state().lock().await;
    if mgr.is_none() {
        let (tx, rx) = std::sync::mpsc::channel::<TtsPlaybackItem>();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let dir = voice_dir.to_path_buf();

        // Spawn background playback task
        tokio::task::spawn_blocking(move || {
            tts_playback_loop(&dir, rx, &cancel_clone);
        });

        *mgr = Some(TtsPlaybackManager { tx, cancel });
    }

    mgr.as_ref()
        .unwrap()
        .tx
        .send(TtsPlaybackItem {
            text,
            speaker_id,
            speed,
            volume,
        })
        .map_err(|_| "TTS playback channel closed".to_string())
}

/// TTS 接力入队（voice_relay::relay_dispatch 落点，G3）：assistant 回复
/// 清洗 → 切句 → 播放队列。spoken_form 开 = 口播话术转换；关 = 仅长句软
/// 切分。注册槽空 = no-op（竞态安全：stop 清注册后迟到的 dispatch 丢弃）。
#[cfg(target_os = "windows")]
pub async fn relay_enqueue(content: &str) {
    let Some(reg) = crate::voice_relay::relay_registration() else {
        return;
    };
    let spoken_form_on = crate::voice_relay::realtime_hot_state()
        .spoken_form
        .load(std::sync::atomic::Ordering::Relaxed);
    let pieces = if spoken_form_on {
        nemesis_voice::spoken_pieces(content)
    } else {
        nemesis_voice::sentence::split_for_tts(
            content,
            nemesis_voice::spoken_form::MAX_SENTENCE_CHARS,
        )
    };
    if pieces.is_empty() {
        return;
    }

    // 播放参数沿用 config.voice.json 默认（与 cmd_tts_playback 同源）
    let voice_cfg = read_voice_config(&reg.config_dir);
    let speaker = voice_cfg
        .get("speaker_id")
        .and_then(|v| v.as_u64())
        .unwrap_or(45) as u32;
    let speed = voice_cfg
        .get("speed")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0) as f32;
    let volume = voice_cfg
        .get("volume")
        .and_then(|v| v.as_u64())
        .unwrap_or(50);

    for piece in pieces {
        if piece.trim().is_empty() {
            continue;
        }
        if tts_enqueue_item(&reg.voice_dir, piece, speaker, speed, volume)
            .await
            .is_err()
        {
            break; // 播放通道没了，剩余条目丢弃
        }
    }
}

// ---------------------------------------------------------------------------
// Barge-in（G4）：停当前播放 + 撤播放队列 + 取消 agent 当前轮 + 前端标注
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn trigger_barge_in(
    output: &dyn SttOutput,
    registration: Option<&crate::voice_relay::RelayRegistration>,
) {
    tracing::info!("[STT Pipeline] barge-in: 用户打断 TTS 播放");
    // 1) 切当前音频（清播放队列，cpal 回调即刻出静音）
    if let Some(p) = active_playback().lock().unwrap().as_ref() {
        p.stop();
    }
    // 2) 撤整个播放会话（未播条目全部丢弃；通道由下次入队懒建重建）。
    //    blocking_lock 而非 try_lock：本函数跑在音频管线 spawn_blocking 线程
    //    （非 runtime worker），持锁方（tts_enqueue_item）临界区无 await 点、
    //    微秒级必释放——等待无死锁风险；try_lock 撞上入队瞬间会跳过撤队列，
    //    症状是「当前句停了，队列下一句又响」（打断不彻底且难复现）。
    let mut mgr = tts_playback_state().blocking_lock();
    if let Some(m) = mgr.take() {
        m.cancel.cancel();
    }
    // 3) 取消 agent 当前轮（窄化版急停：只杀进行中的回复，不动会话状态）
    if let Some(reg) = registration
        && let Some(ref al) = reg.agent_loop
    {
        let cancelled = al.cancel_session(&reg.session_key);
        tracing::info!(
            "[STT Pipeline] barge-in cancel_session({})={}",
            reg.session_key,
            cancelled
        );
    }
    // 4) 前端标注（对应气泡打「已打断」badge；不伪造 chat_log 条目）
    output.notify_barge_in();
}

/// barge-in 声纹门：speaker_enabled 时对 onset 起累积音频做 owner 校验
/// （防旁人说话/电视声误打断）。音频不足 0.5s 不校验直接拒绝（宁可不打断，
/// 也不在短音上误放行非 owner）。引擎未装载 = 校验不可用，放行（与 VAD 段
/// 路径同语义；AEC 装载是另一道前置闸）。
#[cfg(target_os = "windows")]
fn barge_speaker_ok(audio: &[f32], target_sr: u32) -> bool {
    if !*speaker_enabled_state().lock().unwrap() {
        return true;
    }
    if audio.len() < target_sr as usize / 2 {
        return false;
    }
    let engine_guard = speaker_engine_state().lock().unwrap();
    let manager_guard = speaker_manager_state().lock().unwrap();
    match (&*engine_guard, &*manager_guard) {
        (Some(engine), Some(manager)) => {
            let threshold = *speaker_threshold_state().lock().unwrap();
            match engine.embed(audio, target_sr) {
                Ok(emb) => manager.verify("owner", &emb, threshold),
                Err(e) => {
                    tracing::warn!("[STT Pipeline] barge-in speaker embed error: {}", e);
                    false
                }
            }
        }
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// TTS playback loop (blocking background task)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn tts_playback_loop(
    voice_dir: &std::path::Path,
    rx: std::sync::mpsc::Receiver<TtsPlaybackItem>,
    cancel: &CancellationToken,
) {
    let config_path = voice_dir.join("config.toml");
    if !config_path.exists() {
        tracing::error!("[TTS Playback] config.toml not found");
        return;
    }

    let cfg = nemesis_voice::AppConfig::load_or_default(&config_path);

    if !nemesis_voice::sherpa_is_initialized()
        && let Err(e) = nemesis_voice::bootstrap::init_sherpa(voice_dir)
    {
        tracing::error!("[TTS Playback] sherpa init failed: {}", e);
        return;
    }

    let mut consecutive_failures: u32 = 0;
    let max_consecutive = 3;
    let mut restart_attempts: u32 = 0;
    let max_restarts = 3;

    while !cancel.is_cancelled() {
        let item = match rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(item) => item,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        if cancel.is_cancelled() {
            break;
        }

        // Get or create TTS engine
        let (samples, sample_rate) = {
            let guard = tts_engine_state().lock().unwrap();
            if let Some(ref engine) = *guard {
                match engine.generate(&item.text, item.speaker_id, item.speed) {
                    Ok(result) => result,
                    Err(e) => {
                        tracing::warn!("[TTS Playback] Generation failed: {}", e);
                        consecutive_failures += 1;
                        drop(guard);
                        handle_tts_failure(
                            consecutive_failures,
                            max_consecutive,
                            &mut restart_attempts,
                            max_restarts,
                        );
                        continue;
                    }
                }
            } else {
                tracing::error!("[TTS Playback] TTS engine not loaded");
                drop(guard);
                break;
            }
        };

        if samples.is_empty() {
            tracing::warn!("[TTS Playback] Empty audio generated");
            continue;
        }

        // Reset failure counter on success
        consecutive_failures = 0;

        // Play the audio
        let gain = item.volume as f32 / 50.0 * cfg.audio.gain;
        let playback = match nemesis_voice::AudioPlayback::new(
            &cfg.audio.playback_device,
            sample_rate,
            gain,
        ) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("[TTS Playback] Playback init failed: {}", e);
                continue;
            }
        };

        // barge-in 共享态：播放中挂停止句柄（STT 管线线程 stop() 切音频用），
        // 结束记尾窗锚点（300ms 内仍可抢断下一句开播前）。
        let stop_handle = playback.stop_handle();
        *active_playback().lock().unwrap() = Some(stop_handle);
        tts_playing().store(true, std::sync::atomic::Ordering::Relaxed);
        let play_result = playback.play_blocking(&samples, sample_rate);
        tts_playing().store(false, std::sync::atomic::Ordering::Relaxed);
        *tts_last_play_end().lock().unwrap() = Some(std::time::Instant::now());
        *active_playback().lock().unwrap() = None;
        if let Err(e) = play_result {
            tracing::warn!("[TTS Playback] Playback error: {}", e);
        }
    }

    tracing::info!("[TTS Playback] Loop ended");
}

#[cfg(target_os = "windows")]
fn handle_tts_failure(
    consecutive_failures: u32,
    max: u32,
    restart_attempts: &mut u32,
    max_restarts: u32,
) {
    if consecutive_failures >= max {
        if *restart_attempts >= max_restarts {
            tracing::error!("[TTS Playback] Max restart attempts reached, marking engine dead");
            let mut guard = tts_engine_state().lock().unwrap();
            *guard = None;
            // Note: push engine_fault to frontend is complex from this sync context
            // The frontend will detect engine unavailable on next request
        } else {
            tracing::warn!(
                "[TTS Playback] Restarting TTS engine (attempt {})",
                *restart_attempts + 1
            );
            {
                let mut guard = tts_engine_state().lock().unwrap();
                *guard = None;
            }
            *restart_attempts += 1;
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
    let stt_engine = match nemesis_voice::SttEngine::new(
        stt_dir,
        &cfg.stt.model_name,
        language,
        cfg.stt.lang_remedy,
        use_itn,
        num_threads,
    ) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[STT] Engine init failed: {}", e);
            push_stt_result(
                session_id,
                session_mgr.clone(),
                &format!("[错误] STT引擎初始化失败: {}", e),
            );
            return;
        }
    };

    let mut detector = nemesis_voice::create_detector(cfg);

    let capture = match nemesis_voice::AudioCapture::new(capture_device) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[STT] Audio capture failed: {}", e);
            push_stt_result(
                session_id,
                session_mgr.clone(),
                &format!("[错误] 麦克风初始化失败: {}", e),
            );
            return;
        }
    };

    let mut resampler = match nemesis_voice::Resampler::new(capture.sample_rate, target_sample_rate)
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[STT] Resampler init failed: {}", e);
            push_stt_result(
                session_id,
                session_mgr.clone(),
                &format!("[错误] 重采样器初始化失败: {}", e),
            );
            return;
        }
    };

    tracing::info!(
        "[STT] Dictation started (session={}, detector={})",
        session_id,
        detector.name()
    );

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
    if let Some(speech) = detector.flush()
        && !speech.is_empty()
    {
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

    tracing::info!(
        "[STT] Dictation stopped (session={}, chunks={}, speech_segments={})",
        session_id,
        chunk_count,
        speech_count
    );
}

// ---------------------------------------------------------------------------
// STT pipeline (blocking, uses persistent engine)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
fn run_stt_pipeline(
    source: &mut dyn nemesis_voice::AudioChunkSource,
    target_sr: u32,
    cfg: &nemesis_voice::AppConfig,
    cancel: &CancellationToken,
    output: &dyn SttOutput,
    realtime: Option<&RealtimePipelineParams>,
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

    // 流式引擎懒建（W2）：模型缺 = warn + 本轮降级仅离线（不阻塞下载，
    // 安装走 install_model model=stt_stream）。
    let mut stream_engine: Option<nemesis_voice::StreamSttEngine> = None;
    if let Some(rt) = realtime
        && rt.stream_stt
    {
        match nemesis_voice::model::probe_stt_stream_model(cfg) {
            Some(dir) => match nemesis_voice::StreamSttEngine::new_default(
                &dir,
                cfg.stt_stream.num_threads,
                cfg.stt_stream.rule1_min_trailing_silence,
                cfg.stt_stream.rule2_min_trailing_silence,
            ) {
                Ok(e) => stream_engine = Some(e),
                Err(e) => {
                    tracing::warn!("[STT Pipeline] 流式引擎创建失败，本轮降级仅离线: {}", e)
                }
            },
            None => {
                tracing::warn!(
                    "[STT Pipeline] 流式 STT 模型未安装，本轮降级仅离线（安装：语音页 install_model model=stt_stream）"
                );
            }
        }
    }

    // barge-in 状态机 + onset 起累积音频（声纹校验样本）
    let mut barge_watch = nemesis_voice::barge_in::BargeInWatch::new(BARGE_MIN_SUSTAIN);
    let mut barge_audio: Vec<f32> = Vec::new();

    // Far-end 重采样器：播放设备率 → target_sr（采集与播放可能是不同设备、不同采样率，
    // 不能复用 near-end 的 resampler）。AEC 关闭时不使用，但廉价，预先建好。
    let mut far_resampler =
        nemesis_voice::Resampler::new(nemesis_voice::far_end_sample_rate(), target_sr).ok();

    tracing::info!(
        "[STT Pipeline] Started (detector={}, realtime={})",
        detector.name(),
        realtime.is_some()
    );

    let mut chunk_count: u64 = 0;
    let mut speech_count: u64 = 0;
    let mut last_partial = String::new();

    while !cancel.is_cancelled() {
        let Some(chunk) = source.try_chunk() else {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        };
        chunk_count += 1;
        let resampled = chunk;

        // AEC：用 TTS 播放（far-end）抵消麦克风（near-end）里的回声，使 VAD/STT
        // 只听到用户。未启用/未装载时直通 resampled，无额外开销。
        let cleaned: Vec<f32> = {
            let mut guard = aec_state().lock().unwrap();
            match guard.as_mut() {
                Some(aec) => {
                    // 排空 far-end 参考（设备率）并重采样到 target_sr
                    let far_dev: Vec<f32> = nemesis_voice::far_end_buffer()
                        .lock()
                        .unwrap()
                        .drain(..)
                        .collect();
                    let far_16k = match far_resampler.as_mut() {
                        Some(r) => r.resample(&far_dev),
                        None => far_dev,
                    };
                    aec.process(&resampled, &far_16k)
                }
                None => resampled,
            }
        };

        // realtime 流式分支（W2）：喂流式引擎 + partial 草稿上屏（变化才发，
        // 省带宽；早于段尾 final 数秒可见）。
        if let Some(ref se) = stream_engine {
            se.accept_waveform(&cleaned, target_sr);
            se.decode();
            let p = se.partial();
            if !p.is_empty() && p != last_partial {
                last_partial = p;
                output.send_partial(&last_partial);
            }
        }

        // barge-in 判定（G4）：is_speaking 在本块 process 之前读（状态滞后
        // 一块 ~100ms；BARGE_MIN_SUSTAIN 250ms 守卫覆盖该滞后，行为无碍）。
        // 前置闸 = AEC 已装载（无 AEC 时 TTS 回声会被 VAD 听成用户说话，
        // 必自打断）；窗口 = 播放中或尾窗 300ms；声纹开 = onset 起音频 owner 校验。
        if let Some(rt) = realtime
            && rt.barge_in
        {
            let speaking_now = detector.is_speaking();
            if barge_watch.onset(speaking_now) {
                barge_audio.clear();
            }
            if speaking_now {
                barge_audio.extend_from_slice(&cleaned);
                // 滑窗上限：sustained 已满足但播放窗口未开时（bot 没在播
                // 报、用户连续说话），上面两处 clear 都不触发——累积无界
                // 增长（64KB/s）。cap 在最近 30s：声纹校验只用最近音频，
                // 丢旧无损。
                const BARGE_AUDIO_CAP: usize = 30 * 16000; // 30s @16kHz f32
                if barge_audio.len() > BARGE_AUDIO_CAP {
                    let overflow = barge_audio.len() - BARGE_AUDIO_CAP;
                    barge_audio.drain(..overflow);
                }
            }
            if barge_watch.sustained() {
                let playing = tts_playing().load(std::sync::atomic::Ordering::Relaxed);
                let since_end = *tts_last_play_end().lock().unwrap();
                let aec_loaded = aec_state().lock().unwrap().is_some();
                if aec_loaded
                    && nemesis_voice::barge_in::BargeInWatch::playback_window_open(
                        playing, since_end, BARGE_TAIL,
                    )
                    && barge_speaker_ok(&barge_audio, target_sr)
                {
                    trigger_barge_in(output, rt.registration.as_deref());
                    barge_watch.reset();
                    barge_audio.clear();
                }
            }
        }

        let speech_segment = detector.process(&cleaned, target_sr);
        if let Some(speech) = speech_segment {
            speech_count += 1;
            if !speech.is_empty() {
                // Speaker verification: if enabled, verify before STT
                if *speaker_enabled_state().lock().unwrap() {
                    let engine_guard = speaker_engine_state().lock().unwrap();
                    let manager_guard = speaker_manager_state().lock().unwrap();
                    match (&*engine_guard, &*manager_guard) {
                        (Some(engine), Some(manager)) => {
                            let threshold = *speaker_threshold_state().lock().unwrap();
                            match engine.embed(&speech, target_sr) {
                                Ok(embedding) => {
                                    if !manager.verify("owner", &embedding, threshold) {
                                        tracing::info!(
                                            "[STT Pipeline] Speaker rejected (segment #{})",
                                            speech_count
                                        );
                                        continue;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("[STT Pipeline] Speaker embedding error: {}", e);
                                    continue;
                                }
                            }
                        }
                        _ => {
                            // Engine or manager not loaded — skip verification, proceed to STT
                        }
                    }
                }

                // 段尾 final 来源（W2）：two_pass 开（或无流式引擎）= 离线整段
                // 精识（原路径，准确）；关 = 流式 partial 直接作 final（省一次
                // 精识，延迟更低）。两种来源都把流式流复位对齐下一段。
                let stream_final = stream_engine.as_ref().map(|se| {
                    let t = se.partial();
                    se.reset();
                    t
                });
                last_partial.clear();
                let two_pass = realtime.map(|rt| rt.two_pass).unwrap_or(true);

                let final_text: Option<String> = if two_pass || stream_final.is_none() {
                    let guard = stt_engine_state().lock().unwrap();
                    match guard.as_ref() {
                        Some(engine) => match engine.recognize(&speech, target_sr) {
                            Ok(text) => Some(text.trim().to_string()),
                            Err(e) => {
                                tracing::warn!("[STT Pipeline] Recognition error: {}", e);
                                None
                            }
                        },
                        None => {
                            tracing::error!("[STT Pipeline] Engine released during pipeline");
                            output.send_text("[错误] STT引擎已释放");
                            return;
                        }
                    }
                } else {
                    stream_final
                };

                if let Some(text) = final_text
                    && !text.is_empty()
                {
                    output.send_text(&punctuate_if_loaded(&text));
                }
            }
        }
    }

    // Flush remaining audio
    if let Some(speech) = detector.flush()
        && !speech.is_empty()
    {
        let guard = stt_engine_state().lock().unwrap();
        if let Some(ref engine) = *guard {
            match engine.recognize(&speech, target_sr) {
                Ok(text) => {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        tracing::info!("[STT Pipeline] Recognized (final flush): {}", trimmed);
                        output.send_text(&punctuate_if_loaded(trimmed));
                    }
                }
                Err(e) => tracing::warn!("[STT Pipeline] Final recognition error: {}", e),
            }
        }
    }

    tracing::info!(
        "[STT Pipeline] Stopped (chunks={}, speech_segments={})",
        chunk_count,
        speech_count
    );
}

#[cfg(target_os = "windows")]
/// 若标点引擎已加载，给 STT 文本加标点；否则原样返回。
#[cfg(target_os = "windows")]
fn punctuate_if_loaded(text: &str) -> String {
    let guard = punct_engine_state().lock().unwrap();
    if let Some(ref engine) = *guard {
        match engine.add_punctuation(text) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("[STT Pipeline] punctuate failed, using raw text: {}", e);
                text.to_string()
            }
        }
    } else {
        text.to_string()
    }
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

#[cfg(target_os = "windows")]
fn push_stt_to_input(
    session_id: &str,
    session_mgr: Arc<crate::session::SessionManager>,
    text: &str,
) {
    let msg = crate::protocol::ProtocolMessage::push(
        "voice",
        "stt_to_input",
        Some(serde_json::json!({ "text": text })),
    );
    if let Ok(bytes) = msg.to_json() {
        let rt = tokio::runtime::Handle::current();
        let sid = session_id.to_string();
        rt.spawn(async move {
            if let Err(e) = session_mgr.broadcast(&sid, &bytes).await {
                tracing::warn!("[STT InputBox] Failed to push result: {}", e);
            }
        });
    }
}

#[cfg(target_os = "windows")]
fn push_stt_dialogue(
    session_id: &str,
    session_mgr: Arc<crate::session::SessionManager>,
    cmd: &str,
    text: &str,
) {
    let msg = crate::protocol::ProtocolMessage::push(
        "voice",
        cmd,
        Some(serde_json::json!({ "text": text })),
    );
    if let Ok(bytes) = msg.to_json() {
        let rt = tokio::runtime::Handle::current();
        let sid = session_id.to_string();
        rt.spawn(async move {
            if let Err(e) = session_mgr.broadcast(&sid, &bytes).await {
                tracing::warn!("[STT Dialogue] Failed to push result: {}", e);
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
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && has_onnx_file(&entry.path())
            {
                return true;
            }
        }
    }
    false
}

#[cfg(target_os = "windows")]
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

// Phase 3 批次 18（2026-08-25）：voice.rs 纯逻辑子模块测试。子模块（而非
// sibling 文件）是为了访问本文件私有项（DialogueSttOutput 缓冲语义 /
// check_model_subdir_any / read_voice_config 回退 / cmd_status 文件探测 /
// cmd_speaker_* 持久化）。原生音频臂（WASAPI / AEC / kokoro / espeak /
// SenseVoice 真 DLL）不可 mock → 结构性豁免（台账 §9.4 VE-voice-1）。
#[cfg(all(test, target_os = "windows"))]
mod wweb2_tests;

// quality-hardening goal 冲刺 S10a（2026-08-25）：voice.rs 非豁免行覆盖第二子模块。
// 聚焦 VE1 豁免范围外的 WSAPI dispatch 提取臂、start/stop 状态机（注入会话）、
// 配置读写、纯逻辑 helper 与 push 广播路径。测试间全局状态竞态由模块内
// voice_state_lock() 串行化（wweb2_tests::voice_shutdown_idle_is_safe 也持有同一把锁）。
#[cfg(all(test, target_os = "windows"))]
mod s10_tests;

// AGT 覆盖率批次（2026-09-24）：第三子模块。setup/install_runtime/install_aec/
// install_model 的 create_dir 失败与锁/config 门、tts 与 tts_playback 纯校验段、
// init_engines_from_config fail-fast、注入 SpeakerManager 的 remove/status/shutdown
// 臂。同样持有 s10_tests::voice_state_lock 串行化全局状态。
#[cfg(all(test, target_os = "windows"))]
mod agt_tests;

// Wave4 覆盖率批次（2026-09-25）：第四子模块。config_get/set、voice_config
// 全字段合并、chat_config 回环、speakers 表投影、engine_stop 三模型空态、
// pipeline/dictation/dialogue/speaker_test/register stop 的空状态诚实报错、
// tts_playback_stop 与 register_cancel 幂等成功、engine_status 空态投影。
#[cfg(all(test, target_os = "windows"))]
mod w4_tests;
