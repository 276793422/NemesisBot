//! 配置热发布世代化测试：绑定发布 / 执行钉代（嵌套不跨代）/ CAS
//! retryable / last-known-good / 缺省回落 / 代号单调性 / 重绑换源 / 并发烟测。
//!
//! 独立测试文件（生产文件只保留声明行，仓库 2026-08-25 纪律）；provider 桩
//! 自建（config_watch_tests.rs 同先例）。

use super::*;
use async_trait::async_trait;

struct NoopProvider;

#[async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("not used".to_string())
    }
}

fn fresh_loop() -> Arc<AgentLoop> {
    Arc::new(AgentLoop::new(
        Box::new(NoopProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
            tool_fence_profession: None,
        },
    ))
}

/// 写 config.json（返回路径；TempDir 须与 loop 同寿命，由调用方持有）。
fn write_config(dir: &std::path::Path, name: &str, cfg: serde_json::Value) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_string(&cfg).unwrap()).unwrap();
    path
}

fn cfg_with(downscale: bool, retries: i64) -> serde_json::Value {
    serde_json::json!({
        "agents": {
            "image_downscale": downscale,
            "defaults": {"rate_limit_retries": retries}
        }
    })
}

#[test]
fn l18_bind_publishes_generation_and_readers_see_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "config.json", cfg_with(false, 7));
    let loop_ = fresh_loop();
    loop_.set_config_path(path);
    // 绑定即发布（装配期预热），代号从 1 起。
    assert_eq!(loop_.config_generation_seq(), 1);
    assert!(loop_.config_generation().is_some());
    // 读者从代内快照取值（不再是直读盘）。
    assert!(!loop_.current_image_downscale());
    assert_eq!(loop_.current_rate_limit_retries(), 7);
}

#[test]
fn l18_round_pin_readers_hold_generation_until_publication() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "config.json", cfg_with(false, 7));
    let loop_ = fresh_loop();
    loop_.set_config_path(path.clone());
    // 盘上改值（模拟 dashboard/CLI 保存，内容变化）但**不发布**——round 内
    // 钉代：读者仍持旧代（嵌套不跨代）。
    std::fs::write(&path, serde_json::to_string(&cfg_with(true, 9)).unwrap()).unwrap();
    assert!(!loop_.current_image_downscale(), "round 内读者必须持旧代");
    assert_eq!(loop_.current_rate_limit_retries(), 7);
    // round start（check_config_reload）发布新代后才可见。
    loop_.check_config_reload();
    assert!(
        loop_.current_image_downscale(),
        "round start 发布后读者见新代"
    );
    assert_eq!(loop_.current_rate_limit_retries(), 9);
}

#[test]
fn l18_unchanged_check_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "config.json", cfg_with(false, 7));
    let loop_ = fresh_loop();
    loop_.set_config_path(path);
    assert_eq!(loop_.config_generation_seq(), 1);
    // 无盘上变化：重复 round start 零发布（内容闸短路； 起
    // 闸从 mtime 等值改内容比对——Windows 粗时钟同 tick 写 mtime 不可分辨）。
    loop_.check_config_reload();
    loop_.check_config_reload();
    assert_eq!(loop_.config_generation_seq(), 1);
}

#[test]
fn l18_superseded_prepare_is_retryable() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "config.json", cfg_with(false, 7));
    let loop_ = fresh_loop();
    loop_.set_config_path(path.clone());
    assert_eq!(loop_.config_generation_seq(), 1);
    // 准备基于 base=1；随后并发赢家以**更新的盘上内容**先发布（模拟盘上已
    // 推进到更新的文件状态），输家发布被拒。
    let prepared = loop_
        .prepare_config_generation(std::fs::read(&path).unwrap())
        .expect("prepare succeeds");
    let winner_raw = br#"{"agents":{"image_downscale":true}}"#.to_vec();
    let winner = PreparedConfig {
        base_generation: 1,
        raw: winner_raw.clone(),
        value: std::sync::Arc::new(serde_json::json!({"agents": {"image_downscale": true}})),
    };
    assert!(matches!(loop_.publish_prepared_config(winner), Ok(true)));
    assert_eq!(loop_.config_generation_seq(), 2);
    // 输家基于陈旧基线且内容不同 → Superseded，现行代不被回退。
    assert!(matches!(
        loop_.publish_prepared_config(prepared),
        Err(PublishSuperseded)
    ));
    assert_eq!(loop_.config_generation_seq(), 2);
    assert!(loop_.current_image_downscale(), "赢家内容保留");
    // 收敛形态：同（陈旧）基线、但内容与现行代相同 = 赢家已发布同盘态
    // 内容 → Ok(false) 不再换代。
    let converged = PreparedConfig {
        base_generation: 1,
        raw: winner_raw,
        value: std::sync::Arc::new(serde_json::json!({"agents": {"image_downscale": true}})),
    };
    assert!(matches!(
        loop_.publish_prepared_config(converged),
        Ok(false)
    ));
    assert_eq!(loop_.config_generation_seq(), 2);
}

#[test]
fn l18_broken_config_keeps_last_known_good() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "config.json", cfg_with(false, 7));
    let loop_ = fresh_loop();
    loop_.set_config_path(path.clone());
    assert!(!loop_.current_image_downscale());
    // 盘上写坏（坏 JSON）→ round start 校验失败：保留上一代，代不前进。
    std::fs::write(&path, "{\"agents\": broken").unwrap();
    loop_.check_config_reload();
    assert_eq!(loop_.config_generation_seq(), 1, "坏 JSON 不发布新代");
    assert!(!loop_.current_image_downscale(), "last-known-good");
    assert_eq!(loop_.current_rate_limit_retries(), 7);
    // 修复并改值 → 下个发布点恢复且前进。
    std::fs::write(&path, serde_json::to_string(&cfg_with(true, 9)).unwrap()).unwrap();
    loop_.check_config_reload();
    assert_eq!(loop_.config_generation_seq(), 2);
    assert!(loop_.current_image_downscale());
    assert_eq!(loop_.current_rate_limit_retries(), 9);
}

#[test]
fn l18_missing_file_no_generation_readers_fall_back() {
    let dir = tempfile::tempdir().unwrap();
    let loop_ = fresh_loop();
    // 绑定到不存在的文件：无代可发，读者回落各自缺省（与旧独立直读失败同语义）。
    loop_.set_config_path(dir.path().join("nonexistent.json"));
    assert_eq!(loop_.config_generation_seq(), 0);
    assert!(loop_.config_generation().is_none());
    assert!(loop_.current_image_downscale(), "缺省开");
    assert_eq!(loop_.current_rate_limit_retries(), 10);
}

#[test]
fn l18_generation_monotonic_across_publications() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let loop_ = fresh_loop();
    // mtime 闸按时间戳判变化——同 tick 连写可能同 mtime（Windows 粒度），
    // 用显式 set_modified 钉确定性递增 mtime（生产行为与旧 config_mtime 闸
    // 同款 best-effort，测试不赌文件系统时间戳精度）。
    let write_with_mtime = |n: u64| {
        let mut f = std::fs::File::create(&path).unwrap();
        use std::io::Write;
        f.write_all(
            serde_json::to_string(&cfg_with(false, n as i64))
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(n * 3600))
            .unwrap();
    };
    write_with_mtime(1);
    loop_.set_config_path(path.clone());
    assert_eq!(loop_.config_generation_seq(), 1);
    for n in 2u64..=4 {
        write_with_mtime(n);
        loop_.check_config_reload();
        let cur = loop_.config_generation_seq();
        assert_eq!(cur, n, "代号严格单调递增");
    }
}

#[test]
fn l18_rebind_switches_generation_source() {
    let dir = tempfile::tempdir().unwrap();
    let p1 = write_config(dir.path(), "config1.json", cfg_with(false, 7));
    let loop_ = fresh_loop();
    loop_.set_config_path(p1);
    assert!(!loop_.current_image_downscale());
    // 重绑到另一文件（测试tool_doc_folding 多配置换绑）→ 强制发布换源。
    let p2 = write_config(dir.path(), "config2.json", cfg_with(true, 9));
    loop_.set_config_path(p2);
    assert!(loop_.current_image_downscale(), "重绑后读者见新源");
    assert_eq!(loop_.current_rate_limit_retries(), 9);
    assert!(loop_.config_generation_seq() >= 2);
}

#[test]
fn l18_concurrent_republish_and_readers_smoke() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "config.json", cfg_with(false, 7));
    let loop_ = fresh_loop();
    loop_.set_config_path(path);
    // 并发发布者 + 读者烟测：CAS 竞态下无 panic/无死锁，代号单调不回退。
    let mut handles = Vec::new();
    for _ in 0..2 {
        let l = Arc::clone(&loop_);
        handles.push(std::thread::spawn(move || {
            for _ in 0..20 {
                l.republish_config_generation(true);
            }
        }));
    }
    for _ in 0..2 {
        let l = Arc::clone(&loop_);
        handles.push(std::thread::spawn(move || {
            let mut prev = 0u64;
            for _ in 0..40 {
                let seq = l.config_generation_seq();
                assert!(seq >= prev, "代号观测单调不回退");
                prev = seq;
                let _ = l.current_image_downscale();
            }
        }));
    }
    for h in handles {
        h.join().expect("thread no panic");
    }
    assert!(loop_.config_generation_seq() >= 1);
    assert_eq!(
        loop_.current_rate_limit_retries(),
        7,
        "烟测结束读者见最新代内容"
    );
}
