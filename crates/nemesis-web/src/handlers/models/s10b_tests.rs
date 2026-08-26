//! S10b (quality-hardening goal 冲刺, web 批次 2): models handler — the only
//! remaining non-structural uncovered lines are the DISABLED typed-save
//! helper `save_config` (kept per the code-change discipline; exercised here
//! so its behavior stays pinned if it is ever revived). The other TSV gaps
//! are structural: `load_config`/`write_raw_config` global-ConfigStore arms
//! (setting the OnceLock store would poison every other test in this
//! binary) and `catalog_update` (spawns `current_exe()` as the real CLI —
//! under `cargo test` that is the test binary, so the arm is
//! untestable without a real nemesisbot.exe).

use super::*;

fn write_config(home: &std::path::Path, body: &str) {
    std::fs::write(home.join("config.json"), body).unwrap();
}

fn home_str(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

#[test]
fn disabled_save_config_still_roundtrips_via_disk() {
    let dir = tempfile::tempdir().unwrap();
    let home = home_str(&dir);
    write_config(
        dir.path(),
        r#"{ "model_list": [ { "model_name": "m", "model": "x/y", "api_key": "sk" } ] }"#,
    );

    let mut cfg = load_config(&home).expect("load seeded config");
    let len_before = cfg.model_list.len();
    cfg.agents.defaults.llm = "m".to_string();
    save_config(&home, &mut cfg).expect("typed save via disk path");

    // Round-trip: the typed write persists and reloads.
    let reloaded = load_config(&home).expect("reload after save");
    assert_eq!(reloaded.model_list.len(), len_before);
    assert_eq!(reloaded.agents.defaults.llm, "m");
    // Raw file still parses.
    assert!(read_raw_config(&home).expect("raw read")["model_list"].is_array());
}
