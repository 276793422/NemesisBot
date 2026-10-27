//! models.copy / models.rename / models.test（真实现）测试——2026-10-27
//! 五项增强批 ②③①。
//!
//! 与 tests.rs 同一形态：声明在 models.rs 内、直接构私有方法（不造
//! AppState；rename 的热切臂用 make_ctx——agent_loop 为 None，热切在
//! create_provider 失败时走 warn 分支安静返回，不触网不碰全局槽）。
//! models.test 的 ok / timeout 臂用本文件内的微型假端点（TcpListener
//! 手写一问一答，零外部依赖）；unsupported / config-fail 臂不触网。

use super::*;

/// tests.rs 的 make_ctx（pub(super)）本地转发——与既有批次同一 AppState 形态。
fn make_ctx(dir: &tempfile::TempDir) -> crate::ws_router::RequestContext {
    super::tests::make_ctx(dir)
}

fn write_config(home: &std::path::Path, body: &str) {
    std::fs::write(home.join("config.json"), body).unwrap();
}

fn read_config_raw(home: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap()
}

fn home_str(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

/// m1 带 raw extras（tier/real_name/custom_note）+ m2 裸条目——复制必须
/// 连 key 与 extras 一起带走；改名不能动兄弟条目。
const SEED: &str = r#"{
  "agents": { "defaults": { "llm": "m1" }, "small_model": "m1" },
  "tools": { "image_gen": { "model": "m1" } },
  "model_list": [
    {
      "model_name": "m1",
      "model": "qwen/qwen3-30b",
      "api_key": "sk-secret",
      "model_tier": "auto",
      "real_name": "Qwen3-30B",
      "custom_note": "keep-me"
    },
    { "model_name": "m2", "model": "test/testai-1.1" }
  ]
}"#;

// --- copy -------------------------------------------------------------------

#[test]
fn copy_deep_copies_entry_with_key_and_extras() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();

    let out = h
        .copy_model(
            &home_str(&dir),
            &serde_json::json!({ "name": "m1", "new_name": "m1-copy" }),
        )
        .unwrap()
        .unwrap();
    assert_eq!(out["copied"], true);
    assert_eq!(out["new_name"], "m1-copy");

    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["model_list"].as_array().unwrap().len(), 3);
    // 复制体在尾部（与 CLI model add 追加同形）。
    let copy = &cfg["model_list"][2];
    assert_eq!(copy["model_name"], "m1-copy");
    assert_eq!(copy["model"], "qwen/qwen3-30b");
    assert_eq!(copy["api_key"], "sk-secret", "复制必须连 key 一起带走");
    assert_eq!(copy["model_tier"], "auto", "raw extras 随体复制");
    assert_eq!(copy["real_name"], "Qwen3-30B");
    assert_eq!(copy["custom_note"], "keep-me");
    // 原条目原封不动。
    assert_eq!(cfg["model_list"][0]["model_name"], "m1");
    // 复制不自动改默认（复制 ≠ 想切换）。
    assert_eq!(cfg["agents"]["defaults"]["llm"], "m1");
}

#[test]
fn copy_rejects_collision_and_missing_source() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();
    let home = home_str(&dir);

    let err = h
        .copy_model(
            &home,
            &serde_json::json!({ "name": "m1", "new_name": "m2" }),
        )
        .unwrap_err();
    assert!(err.contains("already exists"), "重名 loud 拒绝: {err}");
    let err = h
        .copy_model(
            &home,
            &serde_json::json!({ "name": "nope", "new_name": "x" }),
        )
        .unwrap_err();
    assert!(err.contains("not found"), "源不存在 loud 拒绝: {err}");
    let err = h
        .copy_model(
            &home,
            &serde_json::json!({ "name": "m1", "new_name": "  " }),
        )
        .unwrap_err();
    assert!(err.contains("empty"), "空名拒绝: {err}");
    // 失败不落半状态。
    assert_eq!(
        read_config_raw(dir.path())["model_list"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

// --- rename -----------------------------------------------------------------

#[test]
fn rename_updates_entry_and_syncs_all_references() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();

    let out = h
        .rename(
            &home_str(&dir),
            &serde_json::json!({ "name": "m1", "new_name": "m9" }),
            &make_ctx(&dir),
        )
        .unwrap()
        .unwrap();
    assert_eq!(out["renamed"], true);

    let cfg = read_config_raw(dir.path());
    // 条目改名、extras 与 key 原样。
    let m9 = &cfg["model_list"][0];
    assert_eq!(m9["model_name"], "m9");
    assert_eq!(m9["api_key"], "sk-secret");
    assert_eq!(m9["custom_note"], "keep-me");
    // 三处引用同步。
    assert_eq!(cfg["agents"]["defaults"]["llm"], "m9");
    assert_eq!(cfg["agents"]["small_model"], "m9");
    assert_eq!(cfg["tools"]["image_gen"]["model"], "m9");
    // 兄弟条目不动。
    assert_eq!(cfg["model_list"][1]["model_name"], "m2");
}

#[test]
fn rename_matches_model_string_identity_too() {
    // defaults.llm 存的是 model 串（vendor/model）而非别名——身份匹配
    // 必须覆盖（与 delete/set_default 多形态比对同口径）。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let mut cfg = read_config_raw(dir.path());
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("qwen/qwen3-30b");
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
    let h = ModelsHandler::new();

    h.rename(
        &home_str(&dir),
        &serde_json::json!({ "name": "m1", "new_name": "m9" }),
        &make_ctx(&dir),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        read_config_raw(dir.path())["agents"]["defaults"]["llm"],
        "m9"
    );
}

#[test]
fn rename_rejects_collision_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();
    let home = home_str(&dir);

    let err = h
        .rename(
            &home,
            &serde_json::json!({ "name": "m1", "new_name": "m2" }),
            &make_ctx(&dir),
        )
        .unwrap_err();
    assert!(err.contains("already exists"), "{err}");
    let err = h
        .rename(
            &home,
            &serde_json::json!({ "name": "nope", "new_name": "x" }),
            &make_ctx(&dir),
        )
        .unwrap_err();
    assert!(err.contains("not found"), "{err}");
    assert_eq!(
        read_config_raw(dir.path())["agents"]["defaults"]["llm"],
        "m1"
    );
}

#[test]
fn rename_of_default_is_allowed_and_config_hot_follows() {
    // 方案 B：改到当前默认模型 = 允许，defaults.llm 跟着新名走。
    // m1 的 provider（qwen 前缀 + 有 key）可构造 provider 但热切链路里
    // create_provider 只构造不联网；agent_loop 为 None 时 loop/Forge 联动
    // 自然跳过——本测断言的是盘上真相（引用同步 + 条目改名）。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();

    let out = h
        .rename(
            &home_str(&dir),
            &serde_json::json!({ "name": "m1", "new_name": "renamed-default" }),
            &make_ctx(&dir),
        )
        .unwrap()
        .unwrap();
    assert_eq!(out["renamed"], true);
    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["agents"]["defaults"]["llm"], "renamed-default");
    assert_eq!(cfg["model_list"][0]["model_name"], "renamed-default");
    // models.list 的 is_default 判据跟着新名走（默认不丢）。
    let list = h.list(&home_str(&dir)).unwrap().unwrap();
    let rows = list["models"].as_array().unwrap();
    let renamed = rows
        .iter()
        .find(|r| r["model_name"] == "renamed-default")
        .unwrap();
    assert_eq!(renamed["is_default"], true);
}

// --- test（真实现） -----------------------------------------------------------

/// 微型假 OpenAI 端点：收一请求回一响应（手写 HTTP/1.1，零依赖）。
async fn fake_chat_server(mode: FakeMode) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            let Ok(n) = sock.read(&mut tmp).await else {
                return;
            };
            if n == 0 {
                return;
            }
            buf.extend_from_slice(&tmp[..n]);
            let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
            let cl: usize = headers
                .split("\r\n")
                .find_map(|l| {
                    l.strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0);
            if buf.len() >= pos + 4 + cl {
                break;
            }
        }
        match mode {
            FakeMode::Ok => {
                let body = r#"{"id":"t","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
            FakeMode::Silent => {
                // 收下请求永不回——测 1s 硬顶超时臂。
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        }
    });
    format!("http://{}", addr)
}

enum FakeMode {
    Ok,
    Silent,
}

fn test_config_with_base(home: &std::path::Path, base: &str) {
    let body = format!(
        r#"{{
          "model_list": [
            {{
              "model_name": "m1",
              "model": "testai/pingmodel",
              "api_key": "sk-test",
              "api_base": "{base}"
            }}
          ]
        }}"#
    );
    write_config(home, &body);
}

#[tokio::test]
async fn test_ok_against_local_fake_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let base = fake_chat_server(FakeMode::Ok).await;
    test_config_with_base(dir.path(), &base);
    let h = ModelsHandler::new();

    let out = h
        .test(&home_str(&dir), "m1", &serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"], "ok", "detail: {}", out["detail"]);
    assert!(out["latency_ms"].as_u64().is_some());
    assert_eq!(out["lane"], "openai-compat");
}

#[tokio::test]
async fn test_timeout_kind_when_endpoint_silent() {
    let dir = tempfile::tempdir().unwrap();
    let base = fake_chat_server(FakeMode::Silent).await;
    test_config_with_base(dir.path(), &base);
    let h = ModelsHandler::new();

    let out = h
        .test(
            &home_str(&dir),
            "m1",
            &serde_json::json!({ "timeout_secs": 1 }),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"], "fail");
    assert_eq!(out["kind"], "timeout");
}

#[tokio::test]
async fn test_unsupported_for_images_protocol_entry() {
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{
          "model_list": [
            { "model_name": "img", "model": "zhipu/cogview-4", "protocol": "images-openai", "api_key": "k" }
          ]
        }"#,
    );
    let h = ModelsHandler::new();
    let out = h
        .test(&home_str(&dir), "img", &serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    // 图像 lane：诚实 unsupported（不触网、不红不绿）。
    assert_eq!(out["status"], "unsupported");
}

#[tokio::test]
async fn test_config_failure_is_fail_not_command_error() {
    // 缺 key 的未知 provider：resolve_provider_selection loud 拒——一键测试
    // 场景下应染红点（status=fail, kind=config）而非整体报命令错。
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{
          "model_list": [
            { "model_name": "m1", "model": "unknownprov/xyz" }
          ]
        }"#,
    );
    let h = ModelsHandler::new();
    let out = h
        .test(&home_str(&dir), "m1", &serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["status"], "fail");
    assert_eq!(out["kind"], "config");
}

#[tokio::test]
async fn test_missing_model_is_command_error() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();
    let err = h
        .test(&home_str(&dir), "nope", &serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

// --- new_model 扩展（2026-10-27 漏项补救：换实际调用的 LLM ID） ---------------

#[test]
fn rename_with_new_model_changes_model_field_too() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();

    let out = h
        .rename(
            &home_str(&dir),
            &serde_json::json!({ "name": "m1", "new_name": "m9", "new_model": "qwen/qwen3-32b" }),
            &make_ctx(&dir),
        )
        .unwrap()
        .unwrap();
    assert_eq!(out["model_changed"], true);

    let cfg = read_config_raw(dir.path());
    let m9 = &cfg["model_list"][0];
    assert_eq!(m9["model_name"], "m9");
    assert_eq!(m9["model"], "qwen/qwen3-32b", "model 串同趟落盘");
    assert_eq!(m9["api_key"], "sk-secret", "key 不受影响");
}

#[test]
fn rename_model_only_allows_unchanged_name() {
    // 只换模型不动显示名（new_name == name）：重名检查必须排除自身——
    // 首版把这种情况误判成 already exists（换模型唯一路径被堵）。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();

    let out = h
        .rename(
            &home_str(&dir),
            &serde_json::json!({ "name": "m1", "new_name": "m1", "new_model": "qwen/qwen3-32b" }),
            &make_ctx(&dir),
        )
        .unwrap()
        .unwrap();
    assert_eq!(out["renamed"], true);
    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["model_list"][0]["model_name"], "m1");
    assert_eq!(cfg["model_list"][0]["model"], "qwen/qwen3-32b");
    // 引用指针仍指本条目（名字没变，同步是幂等改写）。
    assert_eq!(cfg["agents"]["defaults"]["llm"], "m1");
}

#[test]
fn rename_default_with_new_model_swaps_and_config_follows() {
    // 默认模型改 display 名 + 换 LLM ID：盘上引用跟新名，热切从 config
    // 重解析（新 model 串生效）。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();

    h.rename(
        &home_str(&dir),
        &serde_json::json!({ "name": "m1", "new_name": "renamed", "new_model": "qwen/qwen3-32b" }),
        &make_ctx(&dir),
    )
    .unwrap()
    .unwrap();
    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["agents"]["defaults"]["llm"], "renamed");
    let e = &cfg["model_list"][0];
    assert_eq!(e["model_name"], "renamed");
    assert_eq!(e["model"], "qwen/qwen3-32b");
    let list = h.list(&home_str(&dir)).unwrap().unwrap();
    let rows = list["models"].as_array().unwrap();
    let renamed = rows.iter().find(|r| r["model_name"] == "renamed").unwrap();
    assert_eq!(renamed["is_default"], true);
}

#[test]
fn rename_new_model_empty_string_is_loud_reject() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();
    let err = h
        .rename(
            &home_str(&dir),
            &serde_json::json!({ "name": "m1", "new_name": "m9", "new_model": "  " }),
            &make_ctx(&dir),
        )
        .unwrap_err();
    assert!(err.contains("must not be empty"), "{err}");
    assert_eq!(
        read_config_raw(dir.path())["model_list"][0]["model"],
        "qwen/qwen3-30b"
    );
}

#[test]
fn copy_with_new_model_overrides_copy_model_field() {
    // 复制时直接换 LLM ID：「复制 → 换模型」一步到位。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), SEED);
    let h = ModelsHandler::new();

    h.copy_model(
        &home_str(&dir),
        &serde_json::json!({ "name": "m1", "new_name": "m1-copy", "new_model": "qwen/qwen3-32b" }),
    )
    .unwrap()
    .unwrap();
    let cfg = read_config_raw(dir.path());
    let copy = &cfg["model_list"][2];
    assert_eq!(copy["model_name"], "m1-copy");
    assert_eq!(copy["model"], "qwen/qwen3-32b", "复制体的 model 被覆盖");
    assert_eq!(copy["api_key"], "sk-secret");
    // 源条目 model 原样。
    assert_eq!(cfg["model_list"][0]["model"], "qwen/qwen3-30b");
}

// --- 用户报障复现（2026-10-27）：copy → set_default(copy) → 「弹回原模型」 ----

fn repro_seed(defaults_llm: &str) -> String {
    format!(
        r#"{{
  "agents": {{ "defaults": {{ "llm": "{defaults_llm}" }} }},
  "model_list": [
    {{ "model_name": "glm", "model": "zhipu/glm-4.7", "api_key": "sk-1" }}
  ]
}}"#
    )
}

fn assert_default_is(dir: &tempfile::TempDir, h: &ModelsHandler, want: &str) {
    let cfg = read_config_raw(dir.path());
    assert_eq!(
        cfg["agents"]["defaults"]["llm"], want,
        "盘上 defaults.llm 应为 {want}"
    );
    let list = h.list(&dir.path().to_string_lossy()).unwrap().unwrap();
    let rows = list["models"].as_array().unwrap();
    let defaulted: Vec<&serde_json::Value> =
        rows.iter().filter(|r| r["is_default"] == true).collect();
    assert_eq!(defaulted.len(), 1, "恰好一行默认: {defaulted:?}");
    assert_eq!(defaulted[0]["model_name"], want);
}

#[test]
fn repro_copy_then_set_default_sticks_no_model_change() {
    // 形态 A：复制不改模型串。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &repro_seed("glm"));
    let h = ModelsHandler::new();
    h.copy_model(
        &dir.path().to_string_lossy(),
        &serde_json::json!({ "name": "glm", "new_name": "glm-copy" }),
    )
    .unwrap()
    .unwrap();
    h.set_default(&dir.path().to_string_lossy(), "glm-copy", &make_ctx(&dir))
        .unwrap()
        .unwrap();
    assert_default_is(&dir, &h, "glm-copy");
}

#[test]
fn repro_copy_then_set_default_sticks_with_model_change() {
    // 形态 B：复制时换模型串（用户报障时的实际操作）。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &repro_seed("glm"));
    let h = ModelsHandler::new();
    h.copy_model(
        &dir.path().to_string_lossy(),
        &serde_json::json!({
            "name": "glm",
            "new_name": "glm-copy",
            "new_model": "zhipu/glm-4.8"
        }),
    )
    .unwrap()
    .unwrap();
    h.set_default(&dir.path().to_string_lossy(), "glm-copy", &make_ctx(&dir))
        .unwrap()
        .unwrap();
    assert_default_is(&dir, &h, "glm-copy");
}

#[test]
fn repro_default_llm_was_model_string_then_copy_set_default() {
    // 形态 C：defaults.llm 存的是 model 串（CLI add --default 形态）而非
    // 显示名——复制 + 设默认后仍应落显示名并唯一命中复制体。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), &repro_seed("zhipu/glm-4.7"));
    let h = ModelsHandler::new();
    h.copy_model(
        &dir.path().to_string_lossy(),
        &serde_json::json!({ "name": "glm", "new_name": "glm-copy" }),
    )
    .unwrap()
    .unwrap();
    h.set_default(&dir.path().to_string_lossy(), "glm-copy", &make_ctx(&dir))
        .unwrap()
        .unwrap();
    assert_default_is(&dir, &h, "glm-copy");
}

// --- 引用归属隐患修复（2026-10-27）：共享模型串的复制体不得认领引用 ----------
//
// 此前「身份集三形态任一命中」（model_name / model 串 / 别名）的宽匹配，在
// 复制体未换模型串时与原始条目身份重叠：rename 偷默认指针 + 假热切、
// update_field 假热切（内存/磁盘默认分叉）、list/proxy_overview 双行同标
// 默认、delete 误挡。修法 = ref_owner（resolver 同序归属判定）单一真相源，
// 五个消费点全部换装。

/// 偷默认场景种子：原始条目 glm 是默认，复制体 glm-copy 未换模型串
/// （model 与原始相同）——身份集重叠形态。
const THEFT_SEED: &str = r#"{
  "agents": { "defaults": { "llm": "glm" }, "small_model": "glm" },
  "tools": { "image_gen": { "model": "glm" } },
  "model_list": [
    { "model_name": "glm", "model": "zhipu/glm-4.7", "api_key": "sk-1" },
    { "model_name": "glm-copy", "model": "zhipu/glm-4.7", "api_key": "sk-1" }
  ]
}"#;

#[test]
fn rename_copy_with_shared_model_does_not_steal_default_pointer() {
    // 主锁：改名复制体（含换模型）不得把 defaults.llm 认领走——精确名
    // 命中压倒软匹配，指向原始条目名的引用原样保留、不触发 was_default。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), THEFT_SEED);
    let h = ModelsHandler::new();

    h.rename(
        &home_str(&dir),
        &serde_json::json!({
            "name": "glm-copy",
            "new_name": "glm2",
            "new_model": "zhipu/glm-4.8"
        }),
        &make_ctx(&dir),
    )
    .unwrap()
    .unwrap();

    let cfg = read_config_raw(dir.path());
    assert_eq!(
        cfg["agents"]["defaults"]["llm"], "glm",
        "默认指针必须留在原始条目上，不被改名操作偷走"
    );
    assert_eq!(cfg["agents"]["small_model"], "glm");
    assert_eq!(cfg["tools"]["image_gen"]["model"], "glm");
    let e = &cfg["model_list"][1];
    assert_eq!(e["model_name"], "glm2");
    assert_eq!(e["model"], "zhipu/glm-4.8", "换模型照常生效");
    // 原始条目原封不动。
    assert_eq!(cfg["model_list"][0]["model_name"], "glm");
    assert_eq!(cfg["model_list"][0]["model"], "zhipu/glm-4.7");
    // 默认徽标唯一且仍归原始条目。
    assert_default_is(&dir, &h, "glm");
}

#[test]
fn rename_original_default_with_copy_sibling_still_follows() {
    // 反向锁：精确名命中不受兄弟软匹配干扰——改名真默认时引用照常跟随。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), THEFT_SEED);
    let h = ModelsHandler::new();

    h.rename(
        &home_str(&dir),
        &serde_json::json!({ "name": "glm", "new_name": "glm-renamed" }),
        &make_ctx(&dir),
    )
    .unwrap()
    .unwrap();

    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["agents"]["defaults"]["llm"], "glm-renamed");
    assert_eq!(cfg["agents"]["small_model"], "glm-renamed");
    assert_eq!(cfg["tools"]["image_gen"]["model"], "glm-renamed");
    assert_eq!(cfg["model_list"][0]["model_name"], "glm-renamed");
    assert_eq!(cfg["model_list"][0]["model"], "zhipu/glm-4.7");
    // 复制体兄弟不动。
    assert_eq!(cfg["model_list"][1]["model_name"], "glm-copy");
    assert_eq!(cfg["model_list"][1]["model"], "zhipu/glm-4.7");
    assert_default_is(&dir, &h, "glm-renamed");
}

#[test]
fn list_and_proxy_mark_only_exact_owner_when_copy_shares_model() {
    // 复制体共享模型串时不再双行同标默认（list + proxy_overview 两展示面）。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), THEFT_SEED);
    let h = ModelsHandler::new();
    let home = dir.path().to_string_lossy().to_string();

    let rows = h.list(&home).unwrap().unwrap()["models"]
        .as_array()
        .unwrap()
        .clone();
    let defaulted: Vec<&serde_json::Value> =
        rows.iter().filter(|r| r["is_default"] == true).collect();
    assert_eq!(defaulted.len(), 1, "恰好一行默认: {defaulted:?}");
    assert_eq!(defaulted[0]["model_name"], "glm");
    assert_eq!(
        rows.iter().find(|r| r["model_name"] == "glm-copy").unwrap()["is_default"],
        false
    );

    let rows = h.proxy_overview(&home).unwrap().unwrap()["models"]
        .as_array()
        .unwrap()
        .clone();
    let defaulted: Vec<&serde_json::Value> =
        rows.iter().filter(|r| r["is_default"] == true).collect();
    assert_eq!(defaulted.len(), 1, "代理页同样恰好一行默认: {defaulted:?}");
    assert_eq!(defaulted[0]["model_name"], "glm");
}

#[test]
fn update_field_on_copy_shared_model_no_false_swap() {
    // 改复制体的 stream（热切触发字段）不得假热切：此前误判成默认 → 运行
    // 时槽被切到复制体、内存/磁盘默认分叉。ctx.state.model_name 初值
    // "test-model" 当哨兵；热切成功 = 变为 de-prefixed 模型串。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), THEFT_SEED);
    let h = ModelsHandler::new();
    let ctx = make_ctx(&dir);
    assert_eq!(*ctx.state.model_name.lock(), "test-model");

    h.update_field(
        &home_str(&dir),
        &serde_json::json!({ "name": "glm-copy", "field": "stream", "value": "yes" }),
        &ctx,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        *ctx.state.model_name.lock(),
        "test-model",
        "改非默认条目不得触发运行时热切"
    );

    // 改真默认照常热切。
    h.update_field(
        &home_str(&dir),
        &serde_json::json!({ "name": "glm", "field": "stream", "value": "yes" }),
        &ctx,
    )
    .unwrap()
    .unwrap();
    assert_eq!(*ctx.state.model_name.lock(), "glm-4.7", "改真默认照常热切");
}

#[test]
fn delete_copy_with_shared_model_is_allowed() {
    // 复制体共享模型串时此前被误判「默认模型」删不掉；真默认照样挡。
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), THEFT_SEED);
    let h = ModelsHandler::new();
    let home = home_str(&dir);

    let out = h.delete(&home, "glm-copy").unwrap().unwrap();
    assert_eq!(out["deleted"], true);
    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["model_list"].as_array().unwrap().len(), 1);
    assert_eq!(cfg["agents"]["defaults"]["llm"], "glm");

    let err = h.delete(&home, "glm").unwrap_err();
    assert!(err.contains("cannot delete default"), "{err}");
}

#[test]
fn delete_first_entry_allowed_when_default_elsewhere() {
    // first_name 位置臂只在 defaults.llm 为空时兜底——默认在别处时第一条
    // 可删（此前位置臂无条件生效，第一条恒删不掉）。
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{
          "agents": { "defaults": { "llm": "m2" } },
          "model_list": [
            { "model_name": "m1", "model": "zhipu/a", "api_key": "k" },
            { "model_name": "m2", "model": "zhipu/b", "api_key": "k" }
          ]
        }"#,
    );
    let h = ModelsHandler::new();
    let out = h.delete(&home_str(&dir), "m1").unwrap().unwrap();
    assert_eq!(out["deleted"], true);
    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["model_list"].as_array().unwrap().len(), 1);
    assert_eq!(cfg["model_list"][0]["model_name"], "m2");
}

#[test]
fn delete_position_fallback_still_blocked_when_default_unset() {
    // defaults.llm 为空的老配置：list[0] 位置默认兜底保留（旧行为不丢）。
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        r#"{
          "model_list": [
            { "model_name": "m1", "model": "zhipu/a", "api_key": "k" },
            { "model_name": "m2", "model": "zhipu/b", "api_key": "k" }
          ]
        }"#,
    );
    let h = ModelsHandler::new();
    let err = h.delete(&home_str(&dir), "m1").unwrap_err();
    assert!(err.contains("cannot delete default"), "{err}");
    // 非位置默认可删。
    h.delete(&home_str(&dir), "m2").unwrap().unwrap();
}

#[test]
fn ref_owner_pass_order_exact_beats_soft() {
    let entries: Vec<(String, String)> = vec![
        ("glm".to_string(), "zhipu/glm-4.7".to_string()),
        ("glm-copy".to_string(), "zhipu/glm-4.7".to_string()),
    ];
    // ① 精确名命中压倒软匹配（偷默认闸门的本体）。
    assert_eq!(ref_owner(&entries, "glm").as_deref(), Some("glm"));
    assert_eq!(ref_owner(&entries, "glm-copy").as_deref(), Some("glm-copy"));
    // ② 无精确名命中 → 模型串首命中（与 resolver 第二段同序）。
    assert_eq!(ref_owner(&entries, "zhipu/glm-4.7").as_deref(), Some("glm"));
    // ③ 别名首命中。
    assert_eq!(ref_owner(&entries, "glm-4.7").as_deref(), Some("glm"));
    // ④ 不命中 / 空引用。
    assert_eq!(ref_owner(&entries, "nope"), None);
    assert_eq!(ref_owner(&entries, ""), None);
    // 别名歧义形态：多个条目 model 末段同名 → 首命中（resolver 同序）。
    let alias_entries: Vec<(String, String)> = vec![
        ("a".to_string(), "v/x".to_string()),
        ("b".to_string(), "w/x".to_string()),
    ];
    assert_eq!(ref_owner(&alias_entries, "x").as_deref(), Some("a"));
}
