//! B39 保存时轻校验 + 启动缺 key question 应答执行体测试。
//!
//! 覆盖面：
//! - [`probe_model_entry`]：条目不存在 Err / SaveCheck×CLI lane 诚实 skip /
//!   images lane unsupported / 死回环 connect fail（密闭、毫秒级）；
//! - `models.add` 回执 `verify` 字段：探测失败**不拒存**（条目照常落盘）；
//! - [`runtime_fill_model_key`]：写盘 + 热切（概览三字段随动）+
//!   条目不存在 Err 且不落盘。
//!
//! 密闭性纪律：所有触网臂一律指向死回环 `http://127.0.0.1:1`（端口 1
//! 无人监听，connect refused 毫秒级返回）——无 api_base 的条目会按
//! provider 前缀推断默认端点发起真实网络请求，测试条目一律带死回环 base。
//! runtime_fill_model_key 的热切臂经 `default_slot::swap` 入进程全局槽
//! （单测环境槽消费者不活跃，无害——与 rename 热切测试同一形态）。

use super::*;

fn write_config(home: &std::path::Path, body: &str) {
    std::fs::write(home.join("config.json"), body).unwrap();
}

fn read_config_raw(home: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap()
}

fn home_str(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

/// 死回环靶：端口 1 connect refused 毫秒级失败（密闭探针）。
const DEAD_LOOPBACK: &str = "http://127.0.0.1:1";

/// 三形态条目：HTTP lane（死回环 base + 占位 key——空 key 会在 provider
/// 构造期被 config 类错误短路，触不到网络面，connect-fail 臂必须带 key）/
/// CLI 型 lane（无 base 也无需触网）/ images lane（探测前短路，触不到网）。
const SEED: &str = r#"{
  "agents": { "defaults": { "llm": "m1" } },
  "model_list": [
    {
      "model_name": "m1",
      "model": "test/testai-1.1",
      "api_key": "sk-x",
      "api_base": "http://127.0.0.1:1"
    },
    { "model_name": "cli1", "model": "claude-cli/sonnet", "api_key": "" },
    {
      "model_name": "img1",
      "model": "img/img-model",
      "api_key": "sk-x",
      "api_base": "http://127.0.0.1:1",
      "protocol": "images-openai"
    }
  ]
}"#;

// probe_model_entry ------------------------------------------------------

#[tokio::test]
async fn probe_entry_not_found_is_err() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    // 条目必须按 model_name 精确存在——防推断臂造出假条目测试。
    let err = probe_model_entry(&home_str(&dir), "no-such", ProbeDepth::SaveCheck)
        .await
        .unwrap_err();
    assert!(err.contains("no-such"), "{err}");
}

#[tokio::test]
async fn probe_savecheck_skips_cli_lane() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    // SaveCheck × CLI 型 lane = 诚实 skip（spawn 成本恒超 5s 轻校验预算），
    // 不触网不 spawn。
    let out = probe_model_entry(&home_str(&dir), "cli1", ProbeDepth::SaveCheck)
        .await
        .unwrap();
    assert_eq!(out["status"], "skipped", "detail: {}", out["detail"]);
    assert!(!out["detail"].as_str().unwrap_or("").is_empty());
}

#[tokio::test]
async fn probe_images_entry_is_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    // images-openai 不能对话——不红不绿保持灰，两条深度同形。
    for depth in [ProbeDepth::SaveCheck, ProbeDepth::Interactive(None)] {
        let out = probe_model_entry(&home_str(&dir), "img1", depth)
            .await
            .unwrap();
        assert_eq!(out["status"], "unsupported");
    }
}

#[tokio::test]
async fn probe_savecheck_fail_connect_hermetic() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    // 死回环 = 毫秒级 connect refused → fail/connect（诚实传输错误映射：
    // connect error 前缀归 Timeout 类 + is_connect_error 细分出 connect）。
    let out = probe_model_entry(&home_str(&dir), "m1", ProbeDepth::SaveCheck)
        .await
        .unwrap();
    assert_eq!(out["status"], "fail", "detail: {}", out["detail"]);
    assert_eq!(out["kind"], "connect", "detail: {}", out["detail"]);
}

// models.add 回执 verify -------------------------------------------------

#[tokio::test]
async fn add_receipt_carries_verify_and_does_not_reject_on_fail() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), r#"{ "model_list": [] }"#);
    let h = ModelsHandler::new();
    let out = h
        .add(
            &home_str(&dir),
            &serde_json::json!({
                "name": "v1",
                "model": "test/testai-1.1",
                "key": "sk-new",
                "base_url": DEAD_LOOPBACK,
            }),
        )
        .await
        .unwrap()
        .unwrap();
    // 探测失败（死回环 connect）如实附在回执 verify 字段……
    assert_eq!(out["added"], true);
    assert_eq!(out["verify"]["status"], "fail", "verify: {}", out["verify"]);
    // ……但条目已落盘 = 校验失败不拒存（保存语义与探测语义分离）。
    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["model_list"][0]["model_name"], "v1");
    assert_eq!(cfg["model_list"][0]["api_key"], "sk-new");
    assert_eq!(cfg["model_list"][0]["api_base"], DEAD_LOOPBACK);
}

// runtime_fill_model_key -------------------------------------------------

#[tokio::test]
async fn runtime_fill_model_key_writes_and_swaps() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    // make_ctx 复用（tests.rs pub(super)）：AppState 全字段 None/空——
    // agent_loop None 时热切仍走概览三字段（apply_runtime_swap 不依赖
    // agent_loop 存在）。
    let ctx = super::tests::make_ctx(&dir);
    let state = ctx.state.clone();
    // 基线：make_ctx 初始三字段与本测试无关，断言的是「写入后」状态。
    runtime_fill_model_key(&home_str(&dir), "m1", "sk-new", state.clone()).unwrap();

    // 盘上真相：key 写进对应条目（raw RMW，兄弟条目原样）。
    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["model_list"][0]["api_key"], "sk-new");
    assert_eq!(cfg["model_list"][1]["model_name"], "cli1");
    // 热切生效：概览页 model 串 = provider 前缀剥离后的模型 ID
    // （test/testai-1.1 → testai-1.1），key 非空 → model_has_key true。
    assert_eq!(state.model_name.lock().as_str(), "testai-1.1");
    assert!(
        state
            .model_has_key
            .load(std::sync::atomic::Ordering::Acquire)
    );
}

#[tokio::test]
async fn runtime_fill_model_key_missing_entry_err_and_no_write() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let ctx = super::tests::make_ctx(&dir);
    let err =
        runtime_fill_model_key(&home_str(&dir), "ghost", "sk-x", ctx.state.clone()).unwrap_err();
    assert!(err.contains("ghost"), "{err}");
    // Err = 事务口回滚，key 不落盘（闭包 Err → config 不写，原值原样）。
    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["model_list"][0]["api_key"], "sk-x");
}
