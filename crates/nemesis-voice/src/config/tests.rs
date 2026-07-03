use super::*;
use std::path::Path;

#[test]
fn punct_config_default_matches_documented_values() {
    let p = PunctConfig::default();
    assert_eq!(p.model_name, "ct-transformer-zh-en");
    assert_eq!(p.num_threads, 1);
}

#[test]
fn speaker_config_derived_default_returns_empty_model() {
    // Derived #[derive(Default)] produces empty model_name despite the
    // serde `default = "default_speaker_model"` attribute — that attribute
    // only kicks in during deserialization of *partial* TOML.
    // This test pins the current (slightly surprising) behavior.
    let s = SpeakerConfig::default();
    assert_eq!(s.model_name, "");
    assert_eq!(s.num_threads, 0);
}

#[test]
fn proxy_config_is_set_false_for_empty_or_whitespace() {
    let mut p = ProxyConfig::default();
    assert!(!p.is_set());

    p.url = "   ".to_string();
    assert!(!p.is_set(), "whitespace-only url should be treated as unset");

    p.url = "http://127.0.0.1:7890".to_string();
    assert!(p.is_set());
}

#[test]
fn app_config_default_populates_expected_fields() {
    let cfg = AppConfig::default();

    assert_eq!(cfg.stt.model_name, "sensevoice-small");
    assert_eq!(cfg.stt.language, "zh");
    assert!(!cfg.stt.use_itn);
    assert_eq!(cfg.stt.num_threads, 1);

    assert_eq!(cfg.vad.model_name, "silero_vad");
    assert_eq!(cfg.vad.threshold, 0.5);
    assert_eq!(cfg.vad.window_size, 512);

    assert_eq!(cfg.tts.model_name, "kokoro-multi-lang-v1_0");
    assert_eq!(cfg.tts.speaker_id, 45);
    assert_eq!(cfg.tts.speed, 1.0);

    assert_eq!(cfg.audio.target_sample_rate, 16000);
    assert_eq!(cfg.audio.gain, 3.0);
    assert_eq!(cfg.audio.energy_threshold, 0.015);

    assert_eq!(cfg.models.dir, "./data");
    assert!(cfg.models.auto_download);
    assert_eq!(cfg.models.mirror.base, "https://hf-mirror.com");
    assert!(cfg.models.sources.is_empty());

    assert_eq!(cfg.base_dir, PathBuf::from("."));
}

#[test]
fn app_config_model_dir_absolute_path_returned_as_is() {
    let mut cfg = AppConfig::default();
    cfg.models.dir = "/opt/nemesisbot/models".to_string();
    cfg.base_dir = PathBuf::from("/etc/nemesisbot");

    let dir = cfg.model_dir();
    assert_eq!(dir, PathBuf::from("/opt/nemesisbot/models"));
}

#[test]
fn app_config_model_dir_relative_joined_with_base_dir() {
    let mut cfg = AppConfig::default();
    cfg.models.dir = "./data".to_string();
    cfg.base_dir = PathBuf::from("/home/user/.nemesisbot/workspace/config");

    let dir = cfg.model_dir();
    assert_eq!(
        dir,
        PathBuf::from("/home/user/.nemesisbot/workspace/config/data")
    );
}

#[test]
fn app_config_find_model_source_returns_match_by_name() {
    let mut cfg = AppConfig::default();
    cfg.models.sources = vec![
        ModelSource {
            name: "sensevoice".to_string(),
            category: "stt".to_string(),
            repo: "user/sensevoice".to_string(),
            files: vec![],
        },
        ModelSource {
            name: "silero".to_string(),
            category: "vad".to_string(),
            repo: "user/silero".to_string(),
            files: vec![],
        },
    ];

    let found = cfg.find_model_source("silero").expect("should find silero");
    assert_eq!(found.category, "vad");

    assert!(cfg.find_model_source("nonexistent").is_none());
}

#[test]
fn app_config_find_model_source_empty_sources_returns_none() {
    let cfg = AppConfig::default();
    assert!(cfg.find_model_source("anything").is_none());
}

#[test]
fn app_config_load_or_default_missing_file_uses_default_with_parent_dir() {
    let missing = Path::new("/definitely/does/not/exist/config.toml");
    let cfg = AppConfig::load_or_default(missing);

    // Default values populated
    assert_eq!(cfg.stt.model_name, "sensevoice-small");

    // base_dir resolved to parent of requested path
    assert_eq!(cfg.base_dir, PathBuf::from("/definitely/does/not/exist"));
}

#[test]
fn app_config_load_or_default_existing_parent_dir_used_even_on_parse_error() {
    // /tmp exists but is not a config file — should still produce defaults
    let tmp = Path::new("/tmp");
    let cfg = AppConfig::load_or_default(tmp);

    assert_eq!(cfg.stt.model_name, "sensevoice-small");
    // parent of "/" → "/" (or root)
    assert_eq!(cfg.base_dir, PathBuf::from("/"));
}

#[test]
fn app_config_load_valid_toml_roundtrip() {
    let tmpdir = std::env::temp_dir();
    let path = tmpdir.join(format!(
        "nemesisbot_voice_cfg_test_{}.toml",
        std::process::id()
    ));
    std::fs::write(
        &path,
        r#"
[stt]
model_name = "test-stt"
language = "en"
use_itn = true
num_threads = 2

[vad]
model_name = "test-vad"
threshold = 0.7
min_silence_duration = 0.5
min_speech_duration = 0.4
max_speech_duration = 25.0
window_size = 256

[tts]
model_name = "test-tts"
speaker_id = 1
speed = 1.2
num_threads = 2

[audio]
capture_device = "default"
playback_device = "speakers"
target_sample_rate = 8000
gain = 2.0
energy_threshold = 0.02

[models]
dir = "./alt-data"
auto_download = false

[models.mirror]
base = "https://example.com"
"#,
    )
    .unwrap();

    let cfg = AppConfig::load(&path).expect("parse should succeed");
    assert_eq!(cfg.stt.model_name, "test-stt");
    assert_eq!(cfg.stt.use_itn, true);
    assert_eq!(cfg.vad.window_size, 256);
    assert_eq!(cfg.tts.speed, 1.2);
    assert_eq!(cfg.audio.target_sample_rate, 8000);
    assert_eq!(cfg.models.dir, "./alt-data");
    assert!(!cfg.models.auto_download);
    assert_eq!(cfg.models.mirror.base, "https://example.com");

    // Punct defaulted (no #[serde(default)] on the field, but PunctConfig::default uses campplus-free values)
    assert_eq!(cfg.punct.model_name, "ct-transformer-zh-en");
    // Speaker: missing [speaker] section → SpeakerConfig::default() → empty model_name
    // (serde default fn only triggers when the field is omitted inside an existing section)
    assert_eq!(cfg.speaker.model_name, "");

    // base_dir derived from path parent
    assert_eq!(cfg.base_dir, tmpdir);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn app_config_load_invalid_toml_returns_err() {
    let tmpdir = std::env::temp_dir();
    let path = tmpdir.join(format!(
        "nemesisbot_voice_cfg_invalid_{}.toml",
        std::process::id()
    ));
    std::fs::write(&path, b"this is not valid toml = = =").unwrap();

    let res = AppConfig::load(&path);
    assert!(res.is_err());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn app_config_load_missing_path_returns_err() {
    let res = AppConfig::load(Path::new("/no/such/path.toml"));
    assert!(res.is_err());
}

#[test]
fn model_source_clones_correctly() {
    let src = ModelSource {
        name: "x".to_string(),
        category: "stt".to_string(),
        repo: "user/x".to_string(),
        files: vec![ModelFile {
            local: "model.onnx".to_string(),
            remote: "v1/model.onnx".to_string(),
            url: "https://example.com/model.onnx".to_string(),
        }],
    };
    let cloned = src.clone();
    assert_eq!(cloned.name, src.name);
    assert_eq!(cloned.files.len(), 1);
    assert_eq!(cloned.files[0].local, "model.onnx");
    assert_eq!(cloned.files[0].url, "https://example.com/model.onnx");
}
