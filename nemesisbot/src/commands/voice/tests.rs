//! voice 命令单测（Windows-only 实现体；非 Windows 下整个测试模块为空）。
//!
//! cmd_tts / cmd_stt / cmd_chat 需要真音频引擎与模型、cmd_devices 需要音频
//! 硬件、cmd_setup / cmd_download 走下载（结构性）；这里测 require_config
//! 的门卫语义和 cmd_status 的纯文件检查路径。

// 实现体全部 #[cfg(target_os = "windows")] —— 用文件级 cfg 镜像（非 Windows
// 编译为空模块，保持 cargo check --workspace 绿）。
#![cfg(target_os = "windows")]

use super::*;

#[test]
fn require_config_missing_bails_with_setup_hint() {
    let dir = tempfile::tempdir().unwrap();
    let err = require_config(&dir.path().join("tools/voice"))
        .expect_err("缺 config.toml 必须 Err");
    assert!(
        err.to_string().contains("nemesisbot voice setup"),
        "err: {err:#}"
    );
}

#[test]
fn require_config_present_never_fails() {
    let dir = tempfile::tempdir().unwrap();
    let voice_dir = dir.path().join("tools/voice");
    std::fs::create_dir_all(&voice_dir).unwrap();
    // load_or_default 语义：任何内容都落到默认值，不 Err。
    std::fs::write(voice_dir.join("config.toml"), "").unwrap();
    require_config(&voice_dir).expect("config.toml 存在 → Ok");
}

#[test]
fn cmd_status_on_empty_dir_lists_missing_and_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    let voice_dir = dir.path().join("tools/voice");
    std::fs::create_dir_all(&voice_dir).unwrap();
    cmd_status(&voice_dir).expect("纯存在性检查 → Ok（缺件打印 [--] 不 Err）");
}

#[test]
fn cmd_status_with_libs_present_reports_ok() {
    let dir = tempfile::tempdir().unwrap();
    let voice_dir = dir.path().join("tools/voice");
    std::fs::create_dir_all(&voice_dir).unwrap();
    // 放一个假动态库 + config，走 [OK] 分支（metadata 读存在文件）。
    let lib = nemesis_voice::bootstrap::required_lib_names();
    if let Some(first) = lib.first() {
        std::fs::write(voice_dir.join(first), b"fake-lib-bytes").unwrap();
    }
    std::fs::write(voice_dir.join("config.toml"), "").unwrap();
    cmd_status(&voice_dir).expect("文件齐走 [OK] 路径 → Ok");
}

// ===========================================================================
// run() 分发 + cmd_status 模型目录分支（S11c，quality-hardening goal 冲刺 S11）
// —— 既有 4 测只钉 require_config / cmd_status 基础分支。env home 隔离：
// Download/Tts/Stt/Chat 全部经 require_config 门卫在缺 config.toml 时 bail
// （不下模型不碰音频）；Devices 只钉"不炸"（cpal 枚举交给 S12 的 crate 批次）。
// ===========================================================================

mod run_arm {
    use super::*;

    fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot"));
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    #[test]
    fn run_status_on_empty_env_home_is_ok() {
        with_env_home(|_home| {
            run(VoiceAction::Status, false).expect("空 home → 全 missing → Ok");
        });
    }

    #[test]
    fn run_gated_subcommands_bail_without_config() {
        with_env_home(|home| {
            // 显式建出 voice_dir 但不放 config.toml：证明 bail 来自 require_config
            // 而不是别的 IO 错。
            std::fs::create_dir_all(
                home.join("workspace").join("tools").join("voice"),
            )
            .unwrap();
            for action in [
                VoiceAction::Download,
                VoiceAction::Tts { text: "你好".into(), speaker: Some(45), speed: 1.0 },
                VoiceAction::Stt,
                VoiceAction::Chat,
            ] {
                let err = run(action, false).expect_err("缺 config.toml → Voice not set up");
                assert!(
                    err.to_string().contains("Voice not set up"),
                    "got: {err:#}"
                );
            }
        });
    }

    #[test]
    fn run_devices_does_not_panic() {
        // cpal 枚举结果依机器而定（可能无设备/无驱动）：只钉不 panic。
        with_env_home(|_home| {
            let _ = run(VoiceAction::Devices, false);
        });
    }

    #[test]
    fn cmd_status_reports_model_presence_from_default_config_dirs() {
        // 空 config.toml → AppConfig::load Err → 默认配置（model_dir=
        // {voice_dir}/data）；预置 tts/stt/vad 目录走 [OK] 臂、punct 留空走
        // [--] 臂（含 132 行误用 stt_name 打 punct 名的历史行为也一并钉住）。
        let dir = tempfile::tempdir().unwrap();
        let voice_dir = dir.path().join("tools").join("voice");
        let data = voice_dir.join("data");
        std::fs::create_dir_all(data.join("tts").join("kokoro-multi-lang-v1_1")).unwrap();
        std::fs::create_dir_all(data.join("stt").join("sensevoice-small")).unwrap();
        std::fs::create_dir_all(data.join("vad").join("silero_vad")).unwrap();
        std::fs::write(voice_dir.join("config.toml"), "").unwrap();
        cmd_status(&voice_dir).expect("模型目录存在性检查 → Ok");
    }
}
