//! 语音 realtime 热态 + TTS 接力注册（realtime P1 G3/G6）
//!
//! **无条件编译**（不挂 `voice` feature、不挂平台 cfg）：`server.rs` 的出站
//! 广播路径每帧都查热态，热态默认全 false = 逐字节现状。真实现（TTS 入队、
//! 打断）在 `handlers/voice.rs`（`voice` feature + Windows）——本模块只持有
//! 跨进程共享的开关面与注册槽，两侢单一真相源。
//!
//! 生命周期：gateway 启动时 [`init_realtime_hot`] 从 `config.chat.json` 的
//! `realtime` 段装载；运行期 `voice.chat_config_set` 写盘后同步 apply（热生
//! 效，无需重启）。对话模式启动时注册 [`RelayRegistration`]，停止时清除。

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// chat 配置文件名 + 默认模板（`handlers/voice.rs` 与本模块共用，单一来源）。
pub const CHAT_CONFIG_FILENAME: &str = "config.chat.json";
pub const DEFAULT_CHAT_CONFIG: &str =
    include_str!("../../../nemesisbot/config/config.chat.default.json");

/// realtime 热态（G6 机制级开关）。全部 AtomicBool：读方（管线循环、出站
/// 广播）无锁热查；写方只有 config 写盘路径。
#[derive(Debug, Default)]
pub struct RealtimeHot {
    /// 总开关：关 = 全部子能力停用 = 行为与 realtime 落地前逐字节一致。
    pub enabled: AtomicBool,
    /// 流式 STT（边说边出 partial 草稿）。
    pub stream_stt: AtomicBool,
    /// 两遍识别（partial 实时 + VAD 段尾离线精识）。关 = final 直接用流式结果。
    pub two_pass: AtomicBool,
    /// TTS 接力：assistant 回复由后端合成播放（前端跳过自身播放）。
    pub tts_relay: AtomicBool,
    /// 打断（barge-in）：TTS 播放中用户开口 → 停播 + 取消 agent 当前轮。
    pub barge_in: AtomicBool,
    /// 语音化清洗（spoken_form）：接力播放前把 markdown/代码/链接转口播话术。
    pub spoken_form: AtomicBool,
    /// 语音对话提示词（L2）：回复倾向口语短句（tier=mini 不注入）。
    pub voice_prompt: AtomicBool,
}

fn realtime_hot() -> &'static RealtimeHot {
    static INSTANCE: OnceLock<RealtimeHot> = OnceLock::new();
    INSTANCE.get_or_init(RealtimeHot::default)
}

/// 热态实例引用（handlers/voice.rs 管线/入队读子开关；测试断言面同源）。
pub fn realtime_hot_state() -> &'static RealtimeHot {
    realtime_hot()
}

fn load_flag(v: &serde_json::Value, key: &str) -> Option<bool> {
    v.get(key).and_then(|x| x.as_bool())
}

/// Apply a `realtime` config object onto the hot state.
pub fn apply_realtime_config(realtime: &serde_json::Value) {
    let hot = realtime_hot();
    let set = |slot: &AtomicBool, key: &str| {
        if let Some(b) = load_flag(realtime, key) {
            slot.store(b, Ordering::Relaxed);
        }
    };
    set(&hot.enabled, "enabled");
    set(&hot.stream_stt, "stream_stt");
    set(&hot.two_pass, "two_pass");
    set(&hot.tts_relay, "tts_relay");
    set(&hot.barge_in, "barge_in");
    set(&hot.spoken_form, "spoken_form");
    set(&hot.voice_prompt, "voice_prompt");
    // voice_prompt 双存储同步：AgentLoop 启动时只取一次 voice_prompt_flag()
    // Arc，此后 loop 侧读的是镜像——写路径必须把它一起刷，否则热切换不生效。
    if let Some(b) = load_flag(realtime, "voice_prompt") {
        voice_prompt_flag().store(b, Ordering::Relaxed);
    }
}

/// gateway 启动装配点：从 `{workspace}/config/config.chat.json` 装载热态。
/// 文件缺失 = 先落默认模板（与 handlers/voice.rs ensure_chat_config 同语义），
/// 解析失败 = 回退默认模板。幂等，重复调用只重复刷同一份值。
pub fn init_realtime_hot(config_dir: &std::path::Path) {
    let path = config_dir.join(CHAT_CONFIG_FILENAME);
    if !path.exists() {
        let _ = std::fs::create_dir_all(config_dir);
        let _ = std::fs::write(&path, DEFAULT_CHAT_CONFIG);
    }
    let cfg = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::from_str(DEFAULT_CHAT_CONFIG).unwrap_or_default());
    if let Some(rt) = cfg.get("realtime") {
        apply_realtime_config(rt);
    }
}

/// voice_prompt 热态旗：gateway 注入给 `AgentLoop::set_voice_prompt` 的
/// 共享 Arc——`apply_realtime_config` 写热态时同步刷本镜像，loop 侧下一轮
/// build_messages 生效（late-binding，无需重启）。
pub fn voice_prompt_flag() -> Arc<AtomicBool> {
    // 独立 Arc 而非借用 hot.voice_prompt：AgentLoop 持 Arc<AtomicBool> 签名。
    static MIRROR: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    MIRROR
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

// ---------------------------------------------------------------------------
// TTS 接力注册槽（G3）
// ---------------------------------------------------------------------------

/// 对话模式的接力注册：session_key 寻址 + TTS/配置目录 + agent loop 句柄
/// （barge-in 取消当前轮用；装配失败/非 agent 会话 = None，仅降级取消）。
pub struct RelayRegistration {
    pub session_key: String,
    pub voice_dir: std::path::PathBuf,
    pub config_dir: std::path::PathBuf,
    pub agent_loop: Option<Arc<nemesis_agent::r#loop::AgentLoop>>,
}

fn relay_registry() -> &'static std::sync::Mutex<Option<Arc<RelayRegistration>>> {
    static INSTANCE: OnceLock<std::sync::Mutex<Option<Arc<RelayRegistration>>>> = OnceLock::new();
    INSTANCE.get_or_init(|| std::sync::Mutex::new(None))
}

/// 对话模式启动时注册（存在即代表语音对话进行中；同一时刻至多一个管线）。
pub fn set_relay_registration(reg: Option<Arc<RelayRegistration>>) {
    *relay_registry().lock().unwrap() = reg;
}

/// 当前注册快照（handlers/voice.rs 管线与接力入队取用）。
pub fn relay_registration() -> Option<Arc<RelayRegistration>> {
    relay_registry().lock().unwrap().clone()
}

/// 出站帧是否应由后端接管 TTS（send_to_session 调）：
/// 热态开 + assistant 回复非空 + 注册中会话精确匹配。
pub fn relay_should_take_over(session_key: Option<&str>, role: &str, content: &str) -> bool {
    if role != "assistant" || content.trim().is_empty() {
        return false;
    }
    let hot = realtime_hot();
    if !hot.enabled.load(Ordering::Relaxed) || !hot.tts_relay.load(Ordering::Relaxed) {
        return false;
    }
    let Some(key) = session_key else {
        return false;
    };
    relay_registry()
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|reg| reg.session_key == key)
}

/// 接力入队（broadcast 成功后调）：清洗 → 切句 → TTS 播放队列。
/// 真实现在 `handlers/voice::relay_enqueue`（voice feature + Windows）；
/// 其余平台/feature 组合 = 静默 no-op（热态开着也只标记帧，不出声）。
pub async fn relay_dispatch(content: &str) {
    #[cfg(all(feature = "voice", target_os = "windows"))]
    crate::handlers::voice::relay_enqueue(content).await;
    #[cfg(not(all(feature = "voice", target_os = "windows")))]
    let _ = content;
}

#[cfg(test)]
mod voice_relay_tests;
