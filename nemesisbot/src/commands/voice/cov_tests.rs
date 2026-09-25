//! voice wave-5 round-2 补测：既有套件吃剩的离线可达缺口。
//!
//! - cmd_download 的五段 Failed 臂（sources 存在 + 文件缺失 +
//!   auto_download=false → ensure_* 全 bail，绝不触网）。
//! - Tts/Stt/Chat 在 config.toml 存在但语音运行时缺失时的 init_sherpa
//!   干净 bail 行（main lib 不存在 → bail，不 LoadLibrary）。
#![cfg(target_os = "windows")]

use super::*;

mod wave5 {
    use super::*;

    fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
        // 锁中毒恢复：别的测试 panic 留下的 PoisonError 不连坐本测试。
        let _guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot"));
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    /// config 合法但 sources 指向的本地文件全部缺失 + auto_download=false
    /// → 五个 ensure_* 逐段 bail → 五个 Failed eprintln 臂 + "Done." Ok。
    fn seed_config_no_autodownload(voice_dir: &std::path::Path, tag: &str) {
        std::fs::create_dir_all(voice_dir).unwrap();
        std::fs::write(
            voice_dir.join("config.toml"),
            format!(
                r#"
[stt]
model_name = "{tag}-stt"
language = "zh"
use_itn = true
num_threads = 1

[vad]
model_name = "{tag}-vad"
threshold = 0.5
min_silence_duration = 0.3
min_speech_duration = 0.25
max_speech_duration = 30.0
window_size = 512

[tts]
model_name = "{tag}-tts"
speaker_id = 45
speed = 1.0
num_threads = 2

[punct]
model_name = "{tag}-punct"
num_threads = 1

[speaker]
model_name = "{tag}-speaker"

[audio]
capture_device = ""
playback_device = ""
target_sample_rate = 16000

[models]
dir = "./data"
auto_download = false

[models.mirror]
base = "http://127.0.0.1:9"
"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn w5_download_missing_files_with_autodownload_disabled_hits_failed_arms() {
        with_env_home(|home| {
            let voice_dir = home.join("workspace").join("tools").join("voice");
            seed_config_no_autodownload(&voice_dir, "w5miss");
            // 不预置任何模型文件 → ensure_* 全走 auto_download=false bail。
            run(VoiceAction::Download, false).expect("五段 Failed 只打印不失败 → Ok");
        });
    }

    #[test]
    fn w5_tts_with_config_but_missing_runtime_bails_at_init_sherpa() {
        with_env_home(|home| {
            let voice_dir = home.join("workspace").join("tools").join("voice");
            seed_config_no_autodownload(&voice_dir, "w5tts");
            let err = run(
                VoiceAction::Tts {
                    text: "hi".into(),
                    speaker: None,
                    speed: 1.0,
                },
                false,
            )
            .expect_err("运行时缺库必须 bail");
            assert!(
                err.to_string().contains("Voice runtime not found"),
                "got: {err:#}"
            );
        });
    }

    #[test]
    fn w5_stt_with_config_but_missing_runtime_bails_at_init_sherpa() {
        with_env_home(|home| {
            let voice_dir = home.join("workspace").join("tools").join("voice");
            seed_config_no_autodownload(&voice_dir, "w5stt");
            let err = run(VoiceAction::Stt, false).expect_err("运行时缺库必须 bail");
            assert!(
                err.to_string().contains("Voice runtime not found"),
                "got: {err:#}"
            );
        });
    }

    #[test]
    fn w5_chat_with_config_but_missing_runtime_bails_at_init_sherpa() {
        with_env_home(|home| {
            let voice_dir = home.join("workspace").join("tools").join("voice");
            seed_config_no_autodownload(&voice_dir, "w5chat");
            let err = run(VoiceAction::Chat, false).expect_err("运行时缺库必须 bail");
            assert!(
                err.to_string().contains("Voice runtime not found"),
                "got: {err:#}"
            );
        });
    }
}
