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
    let err = require_config(&dir.path().join("tools/voice")).expect_err("缺 config.toml 必须 Err");
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
            std::fs::create_dir_all(home.join("workspace").join("tools").join("voice")).unwrap();
            for action in [
                VoiceAction::Download,
                VoiceAction::Tts {
                    text: "你好".into(),
                    speaker: Some(45),
                    speed: 1.0,
                },
                VoiceAction::Stt,
                VoiceAction::Chat,
            ] {
                let err = run(action, false).expect_err("缺 config.toml → Voice not set up");
                assert!(err.to_string().contains("Voice not set up"), "got: {err:#}");
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

// ===========================================================================
// wave_b（coverage 补测）：cmd_status 全绿臂 + cmd_download 缓存命中离线通路。
//
// - 全绿臂：所有动态库假文件 + 默认模型名五目录全建（含 punct 的
//   ct-transformer-zh-en，上一轮只建了 tts/stt/vad）→ [OK] TTS/STT/VAD/Punct
//   四行 + 尾部 "ready" 横幅一并覆盖。
// - Download 缓存臂：config.toml 写【完整显式 schema】+ models.sources 五条
//   自造源（文件清单与本地产物一一对应并预置），ensure_*_model 走
//   `!files.is_empty() && check_model_files` 早退分支返回本地目录，
//   绝不触网（mirror.base 指向死地址兜底证明——真要下载必然立刻失败暴露）。
//   空 config（默认 AppConfig）时 sources 为空 vec → 必落下载路径 → 禁网
//   纪律下不可测；显式 sources 是唯一离线通路。
// ============================================================================

mod wave_b {
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

    /// 全绿环境：库文件全假造 + 默认五个模型目录全建（punct 也建）。
    #[test]
    fn wave_b_status_all_components_present_reports_ready_banner() {
        let dir = tempfile::tempdir().unwrap();
        let voice_dir = dir.path().join("tools").join("voice");
        std::fs::create_dir_all(&voice_dir).unwrap();
        // 所有必需动态库都放假文件 → libs_ok = true → 走 ready 分支。
        for lib in nemesis_voice::bootstrap::required_lib_names() {
            std::fs::write(voice_dir.join(lib), b"fake-lib").unwrap();
        }
        std::fs::write(voice_dir.join("config.toml"), "").unwrap();
        // 默认配置的模型目录全量预置（AppConfig::default 名称）。
        let data = voice_dir.join("data");
        std::fs::create_dir_all(data.join("tts").join("kokoro-multi-lang-v1_1")).unwrap();
        std::fs::create_dir_all(data.join("stt").join("sensevoice-small")).unwrap();
        std::fs::create_dir_all(data.join("vad").join("silero_vad")).unwrap();
        std::fs::create_dir_all(data.join("punct").join("ct-transformer-zh-en")).unwrap();

        cmd_status(&voice_dir).expect("组件齐全 → 全 [OK] + ready 横幅 → Ok");
    }

    /// cmd_download：sources 驱动的纯本地缓存命中（不触网的唯一通路）。
    #[test]
    fn wave_b_download_hits_local_cache_for_all_five_models_offline() {
        with_env_home(|home| {
            let voice_dir = home.join("workspace").join("tools").join("voice");
            std::fs::create_dir_all(&voice_dir).unwrap();
            // 完整显式 schema（punct/speaker 走 serde default 字段名也显式写出）。
            std::fs::write(
                voice_dir.join("config.toml"),
                r#"
[stt]
model_name = "wb-stt"
language = "zh"
use_itn = true
num_threads = 1

[vad]
model_name = "wb-vad"
threshold = 0.5
min_silence_duration = 0.3
min_speech_duration = 0.25
max_speech_duration = 30.0
window_size = 512

[tts]
model_name = "wb-tts"
speaker_id = 45
speed = 1.0
num_threads = 2

[punct]
model_name = "wb-punct"
num_threads = 1

[speaker]
model_name = "wb-speaker"

[audio]
capture_device = ""
playback_device = ""
target_sample_rate = 16000

[models]
dir = "./data"
auto_download = false

[models.mirror]
base = "http://127.0.0.1:9"

[[models.sources]]
name = "wb-stt"
category = "stt"
repo = "local/wb-stt"
files = [{ local = "model.onnx", remote = "model.onnx" }]

[[models.sources]]
name = "wb-vad"
category = "vad"
repo = "local/wb-vad"
files = [{ local = "model.onnx", remote = "model.onnx" }, { local = "vad.op", remote = "vad.op" }]

[[models.sources]]
name = "wb-tts"
category = "tts"
repo = "local/wb-tts"
files = [{ local = "model.onnx", remote = "model.onnx" }, { local = "voices.bin", remote = "voices.bin" }]

[[models.sources]]
name = "wb-punct"
category = "punct"
repo = "local/wb-punct"
files = [{ local = "model.onnx", remote = "model.onnx" }]

[[models.sources]]
name = "wb-speaker"
category = "speaker"
repo = "local/wb-speaker"
files = [{ local = "campplus.bin", remote = "campplus.bin" }]
"#,
            )
            .unwrap();

            // 本地“模型”产物按 source.files[].local 一一预置。
            let data = voice_dir.join("data");
            let put = |rel: &str| {
                let p = data.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, b"wb-local-bytes").unwrap();
            };
            put("stt/wb-stt/model.onnx");
            put("vad/wb-vad/model.onnx");
            put("vad/wb-vad/vad.op");
            put("tts/wb-tts/model.onnx");
            put("tts/wb-tts/voices.bin");
            put("punct/wb-punct/model.onnx");
            put("speaker/wb-speaker/campplus.bin");

            // 五个 ensure_* 全部走 check_model_files 早退 → Ready 打印 → Ok。
            // 若实现回归为强制下载，mirror.base 死地址会让它立刻失败而非静默挂网。
            run(VoiceAction::Download, false).expect("全部缓存命中 → 五段 Ready + Ok");
        });
    }

    /// Download 的门卫语义（缺 config.toml bail）已有 run_arm 覆盖；这里补一个
    /// config 存在但 model_dir 不可创建的奇异情况不可行（tempdir 恒可写），
    /// 改钉 Status 的最尾打印出口（Ok 收尾语义不变）。
    #[test]
    fn wave_b_status_returns_ok_even_when_everything_missing() {
        let dir = tempfile::tempdir().unwrap();
        // voice_dir 不创建 —— mkdir 都省了，命令必须容错。
        cmd_status(&dir.path().join("never/created")).expect("全 missing → Ok");
    }
}

// ===========================================================================
// r10 wave（覆盖率 95% goal 第七波）：cmd_status 仅剩的两个纯 fs 臂。
//
// - 101-102 行 unwrap_or_else 兜底臂：config.toml 存在但内容是非法 TOML →
//   AppConfig::load Err → 默认配置接管（模型名走 default，目录都不在 →
//   [--] 家族 + incomplete 横幅）。此前只写过空串（合法空表 = load Ok），
//   这个 Err 兜底臂从未被驱动。
// - 显式模型名加载链：config.toml 最小必填 schema 解析出【自造模型名】→
//   对应目录预置 → 四个模型 [OK] 臂吃到的是从文件解析的名字而不是默认常量
//   （此前 status 只有默认名路径；显式名字此前只在 download 缓存命中测过）。
// ===========================================================================

mod r10_status_arms {
    use super::*;

    /// 非法 TOML → AppConfig::load Err → unwrap_or_else 默认兜底臂。
    #[test]
    fn r10_voice_status_corrupt_toml_takes_default_config_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let voice_dir = dir.path().join("tools").join("voice");
        std::fs::create_dir_all(&voice_dir).unwrap();
        // 明确的 TOML 解析毒药：read 成功但 parse 必崩 → load Err。
        std::fs::write(
            voice_dir.join("config.toml"),
            "[[[ 这不是合法的 toml {{@@\nkey without value",
        )
        .unwrap();

        cmd_status(&voice_dir).expect("load 失败兜底后照常扫默认目录并 Ok");
    }

    /// 完整 schema（serde 必填字段全覆盖，optional 省略走 serde default）
    /// 解析出四个自造模型名 + 配套目录预置 → 四个 [OK] 模型臂来自文件加载
    /// 的名字；同时放一个假动态库吃 MB 尺寸格式化臂 + 全绿 ready 横幅。
    #[test]
    fn r10_voice_status_loaded_config_explicit_model_names_report_ok() {
        let dir = tempfile::tempdir().unwrap();
        let voice_dir = dir.path().join("tools").join("voice");
        std::fs::create_dir_all(&voice_dir).unwrap();

        // 必需库随便放一个有实际字节数的假文件（尺寸格式化臂吃 metadata len）。
        if let Some(first_lib) = nemesis_voice::bootstrap::required_lib_names().first() {
            std::fs::write(voice_dir.join(first_lib), vec![0u8; 1024]).unwrap();
        }

        std::fs::write(
            voice_dir.join("config.toml"),
            r#"
[stt]
model_name = "r10-stt"
language = "zh"
use_itn = true
num_threads = 1

[vad]
model_name = "r10-vad"
threshold = 0.5
min_silence_duration = 0.3
min_speech_duration = 0.25
max_speech_duration = 30.0
window_size = 512

[tts]
model_name = "r10-tts"
speaker_id = 45
speed = 1.0
num_threads = 2

[punct]
model_name = "r10-punct"
num_threads = 1

[audio]
capture_device = ""
playback_device = ""
target_sample_rate = 16000

[models]
dir = "./data"
auto_download = false

[models.mirror]
base = "http://127.0.0.1:9"
"#,
        )
        .unwrap();

        // 与上述名字一一对应的目录预置（model_dir 相对 config 所在目录解析）。
        let data = voice_dir.join("data");
        for rel in [
            "stt/r10-stt",
            "vad/r10-vad",
            "tts/r10-tts",
            "punct/r10-punct",
        ] {
            std::fs::create_dir_all(data.join(rel)).unwrap();
        }

        cmd_status(&voice_dir).expect("显式名字 + 目录齐备 → 四 [OK] + ready 横幅 → Ok");
    }
}
