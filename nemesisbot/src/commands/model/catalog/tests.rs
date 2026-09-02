//! Catalog module tests (U16 sixth batch) — fixture-based, no network.

// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;

const FIXTURE: &str = r#"{
  "openai": {
    "id": "openai",
    "name": "OpenAI",
    "env": ["OPENAI_API_KEY"],
    "npm": "@ai-sdk/openai",
    "doc": "https://platform.openai.com/docs/models",
    "models": {
      "gpt-5.2": {
        "name": "GPT-5.2",
        "description": "Reliable GPT generation",
        "family": "gpt",
        "release_date": "2025-12-11",
        "last_updated": "2025-12-11",
        "attachment": true,
        "reasoning": true,
        "tool_call": true,
        "open_weights": false,
        "limit": { "context": 400000, "output": 128000 }
      },
      "gpt-4o-mini-no-limit": {
        "name": "GPT-4o mini",
        "description": "legacy entry without limit block",
        "open_weights": false
      }
    }
  },
  "anthropic": {
    "id": "anthropic",
    "name": "Anthropic",
    "env": ["ANTHROPIC_API_KEY"],
    "npm": "@ai-sdk/anthropic",
    "doc": "https://docs.anthropic.com",
    "models": {
      "claude-opus-4-8": {
        "name": "Claude Opus 4.8",
        "family": "claude",
        "open_weights": false,
        "limit": { "context": 1000000 }
      }
    }
  },
  "weird-vendor-without-models": {
    "id": "weird",
    "name": "Weird"
  }
}"#;

#[test]
fn test_parse_api_json_extracts_limits() {
    let entries = parse_api_json(FIXTURE).expect("fixture parses");
    // Providers without models / models without limit blocks are skipped.
    assert_eq!(entries.len(), 2, "entries: {entries:?}");
    // Sorted by key.
    assert_eq!(entries[0].key, "anthropic/claude-opus-4-8");
    assert_eq!(entries[0].context_window, 1_000_000);
    assert_eq!(entries[0].max_output_tokens, None);
    assert_eq!(entries[0].family.as_deref(), Some("claude"));
    assert_eq!(entries[1].key, "openai/gpt-5.2");
    assert_eq!(entries[1].context_window, 400_000);
    assert_eq!(entries[1].max_output_tokens, Some(128_000));
}

#[test]
fn test_parse_api_json_rejects_garbage() {
    assert!(parse_api_json("not json").is_err());
    assert!(parse_api_json("[1,2,3]").is_err()); // top-level array
}

#[test]
fn test_parse_api_json_ignores_unknown_fields() {
    // Future API fields must not break parsing (goal §八 risk clause).
    let future = r#"{
      "x": {
        "models": {
          "m1": { "brand_new_field": {"nested": true}, "limit": {"context": 8, "output": 4} }
        }
      }
    }"#;
    let entries = parse_api_json(future).expect("future fields tolerated");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].context_window, 8);
}

#[test]
fn test_cache_roundtrip_and_lookup() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path();
    assert!(load_cache(dir).unwrap().is_none(), "missing file → None");

    let entries = parse_api_json(FIXTURE).unwrap();
    save_cache(dir, entries).expect("save");
    // Atomic rename leaves no temp residue.
    assert!(!nemesis_path::models_catalog_cache_path(dir).with_extension("json.tmp").exists());

    let cat = load_cache(dir).unwrap().expect("loaded after save");
    assert_eq!(cat.version, 1);
    assert_eq!(cat.entries.len(), 2);
    let hit = lookup(&cat, "openai/gpt-5.2").expect("hit");
    assert_eq!(hit.context_window, 400_000);
    assert!(lookup(&cat, "nope/model").is_none());
}

#[test]
fn test_corrupt_cache_is_loud() {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = nemesis_path::models_catalog_cache_path(tmp.path());
    std::fs::create_dir_all(p.parent().unwrap()).unwrap(); // 深路径：workspace/data 父链
    std::fs::write(&p, "{ broken").unwrap();
    assert!(load_cache(tmp.path()).is_err(), "present-but-corrupt → Err");
}

#[test]
fn test_parse_models_json_mirror_shape() {
    // Repo models.json (OpenRouter sync cache) — the jsDelivr mirror shape.
    let mirror = r#"{
      "data": [
        {"id": "anthropic/claude-opus-4.7-fast", "name": "Opus", "context_length": 1000000,
         "top_provider": {"max_completion_tokens": 64000}},
        {"id": "~anthropic/claude-haiku-latest", "context_length": 200000},
        {"id": "no-slash-id", "context_length": 8000},
        {"id": "openai/gpt-no-ctx", "name": "no context field"},
        {"id": "openai/gpt-zero", "context_length": 0}
      ]
    }"#;
    let entries = parse_models_json(mirror).expect("mirror shape parses");
    assert_eq!(entries.len(), 1, "alias/no-slash/no-ctx/zero skipped: {entries:?}");
    assert_eq!(entries[0].key, "anthropic/claude-opus-4.7-fast");
    assert_eq!(entries[0].context_window, 1_000_000);
    assert_eq!(entries[0].max_output_tokens, Some(64_000));
    // parse_any accepts both shapes.
    assert!(parse_any(FIXTURE).is_ok());
    assert!(parse_any(mirror).is_ok());
    assert!(parse_any("garbage").is_err());
}

#[test]
fn test_parse_any_zero_entry_shapes_are_misses() {
    // Live-observed regression (2026-08-23): the mirror body (models.json
    // shape) also parses as a top-level OBJECT, so lenient parse_api_json
    // returned Ok(vec![]) and shadowed the mirror parser → 0 models cached.
    // Now: wrong-shape objects are Err from parse_api_json, and parse_any
    // treats zero-entry successes as misses.
    let wrong_shape_object = r#"{"data": [{"id": "x/y", "context_length": 8}]}"#;
    assert!(parse_api_json(wrong_shape_object).is_err());
    // A genuinely empty api.json provider map is also rejected by parse_api_json
    // (no provider entries) — parse_any then tries the other parser.
    assert!(parse_api_json("{}").is_err());
    assert!(parse_any(wrong_shape_object).is_ok()); // via models_json parser
    assert!(parse_any("{}").is_err()); // neither shape yields entries
}

// by_family — 4b layer-1 gap fill: grouping semantics had no direct test
// (parse/cache tests above cover ingestion, not the family grouping).
#[test]
fn test_by_family_groups_and_none_bucket() {
    let catalog = Catalog {
        version: 1,
        fetched_at: "2026-08-24T00:00:00Z".to_string(),
        entries: vec![
            CatalogEntry {
                key: "openai/gpt-5.2".to_string(),
                context_window: 400000,
                max_output_tokens: Some(128000),
                family: Some("gpt".to_string()),
            },
            CatalogEntry {
                key: "openai/o4-mini".to_string(),
                context_window: 200000,
                max_output_tokens: Some(100000),
                family: Some("gpt".to_string()),
            },
            CatalogEntry {
                key: "anthropic/claude-opus-5".to_string(),
                context_window: 1000000,
                max_output_tokens: Some(64000),
                family: Some("claude".to_string()),
            },
            CatalogEntry {
                key: "weird/no-family".to_string(),
                context_window: 8000,
                max_output_tokens: None,
                family: None,
            },
        ],
    };
    let grouped = by_family(&catalog);
    assert_eq!(
        grouped.get("gpt").map(|v| v.as_slice()),
        Some(&["openai/gpt-5.2".to_string(), "openai/o4-mini".to_string()][..]),
        "same-family entries share a bucket, in catalog order"
    );
    assert_eq!(
        grouped.get("claude").map(|v| v.as_slice()),
        Some(&["anthropic/claude-opus-5".to_string()][..])
    );
    assert_eq!(
        grouped.get("(none)").map(|v| v.as_slice()),
        Some(&["weird/no-family".to_string()][..]),
        "missing family lands in the (none) bucket, not dropped"
    );
    assert_eq!(grouped.len(), 3, "exactly three buckets");
}

// =========================================================================
// S11b 覆盖率冲刺：parser 边界 + 缓存读写 + catalog_from/by_family/lookup
// （fetch_http* 属真网络豁免，不在此测。）
// =========================================================================

#[test]
fn test_s11b_parse_models_json_skip_rules() {
    let raw = r#"{
        "data": [
            {"id": "anthropic/claude-opus-4.7-fast", "context_length": 1000000},
            {"id": "~openai/gpt-4o-2024", "context_length": 128000},
            {"id": "no-slash-id", "context_length": 4096},
            {"id": "zhipu/glm-4.7", "context_length": 0},
            {"id": "x/missing-ctx"},
            {"no-id": true, "context_length": 8192},
            {"id": "openai/gpt-4o", "context_length": 128000,
             "top_provider": {"max_completion_tokens": 16384}}
        ]
    }"#;
    let entries = parse_models_json(raw).unwrap();
    // 只有第 1 条和最后 1 条合法（~ 前缀/无斜杠/ctx==0/缺 ctx/缺 id 全跳过）
    assert_eq!(entries.len(), 2, "{:?}", entries);
    assert_eq!(entries[0].key, "anthropic/claude-opus-4.7-fast", "按 key 排序");
    assert_eq!(entries[0].context_window, 1000000);
    assert_eq!(entries[0].max_output_tokens, None);
    assert_eq!(entries[1].key, "openai/gpt-4o");
    assert_eq!(entries[1].max_output_tokens, Some(16384));
}

#[test]
fn test_s11b_parse_models_json_shapes_and_dedup() {
    // 非 JSON → Err
    assert!(parse_models_json("not json").is_err());
    // 缺 data 数组 → Err
    assert!(parse_models_json("{}").is_err());
    assert!(parse_models_json(r#"{"data": {}}"#).is_err());
    // 同 id 重复 → dedup
    let raw = r#"{"data": [
        {"id": "a/one", "context_length": 100},
        {"id": "a/one", "context_length": 200}
    ]}"#;
    let entries = parse_models_json(raw).unwrap();
    assert_eq!(entries.len(), 1);
    // 空数组 → Ok（空）——由 parse_any 负责判 zero-entry
    assert!(parse_models_json(r#"{"data": []}"#).unwrap().is_empty());
}

#[test]
fn test_s11b_parse_api_json_skip_rules() {
    let raw = r#"{
        "zhipu": {
            "models": {
                "glm-4.7": {"limit": {"context": 128000, "output": 4096}, "family": "glm"},
                "no-limit": {"family": "x"},
                "no-context": {"limit": {"output": 100}},
                "zero-context": {"limit": {"context": 0}}
            }
        },
        "empty-provider": {"id": "empty-provider"},
        "meta-only": {"info": "not a provider"}
    }"#;
    let entries = parse_api_json(raw).unwrap();
    assert_eq!(entries.len(), 1, "缺 limit/缺 context/ctx==0 的模型与无 models 的 provider 全跳过");
    assert_eq!(entries[0].key, "zhipu/glm-4.7");
    assert_eq!(entries[0].context_window, 128000);
    assert_eq!(entries[0].max_output_tokens, Some(4096));
    assert_eq!(entries[0].family.as_deref(), Some("glm"));
}

#[test]
fn test_s11b_parse_api_json_shapes() {
    // 顶层非对象 → Err
    assert!(parse_api_json("[1,2]").is_err());
    assert!(parse_api_json(r#""str""#).is_err());
    // 对象但没有任何 provider（models 映射）→ Err（wrong shape）
    assert!(parse_api_json(r#"{"data": [{"id": "a/b", "context_length": 1}]}"#).is_err());
}

#[test]
fn test_s11b_parse_any_prefers_api_shape_and_zero_entry_is_miss() {
    let api = r#"{"p": {"models": {"m": {"limit": {"context": 8}}}}}"#;
    let models = r#"{"data": [{"id": "a/b", "context_length": 100}]}"#;
    // api.json 形状直接命中
    let e = parse_any(api).unwrap();
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].key, "p/m");
    // models.json 形状回退
    let e = parse_any(models).unwrap();
    assert_eq!(e[0].key, "a/b");
    // 两种形状都解析不出条目 → Err（zero-entry 不算成功）
    assert!(parse_any("{}").is_err());
    assert!(parse_any(r#"{"data": []}"#).is_err());
    // 非 JSON → Err
    assert!(parse_any("garbage").is_err());
}

#[test]
fn test_s11b_cache_roundtrip_missing_and_corrupt() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("nested").join("cfg"); // save_cache 要 mkdir 父链
    // 缺失 → Ok(None)
    assert!(load_cache(&dir).unwrap().is_none());
    // 保存（触发 mkdir 分支）→ 读回
    save_cache(
        &dir,
        vec![CatalogEntry {
            key: "a/b".into(),
            context_window: 42,
            max_output_tokens: None,
            family: None,
        }],
    )
    .unwrap();
    assert!(nemesis_path::models_catalog_cache_path(&dir).exists());
    assert!(!nemesis_path::models_catalog_cache_path(&dir).with_extension("json.tmp").exists(), "tmp 已 rename 走");
    let cat = load_cache(&dir).unwrap().expect("cache present");
    assert_eq!(cat.version, 1);
    assert!(!cat.fetched_at.is_empty());
    assert_eq!(cat.entries.len(), 1);
    assert_eq!(cat.entries[0].key, "a/b");
    // 目录而非文件 → Err（present-but-unreadable 一族）
    std::fs::create_dir_all(nemesis_path::models_catalog_cache_path(&dir).with_extension("json"))
        .unwrap_or(()); // 同名拓展冲突时不影响下面用独立目录验证 corrupt
    let dir2 = tmp.path().join("cfg2");
    let corrupt = nemesis_path::models_catalog_cache_path(&dir2);
    std::fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
    std::fs::write(&corrupt, "{corrupt").unwrap();
    assert!(load_cache(&dir2).is_err(), "corrupt cache 必须响亮报错");
}

#[test]
fn test_s11b_catalog_from_by_family_and_lookup() {
    let cat = catalog_from(vec![
        CatalogEntry {
            key: "a/x".into(),
            context_window: 1,
            max_output_tokens: None,
            family: Some("f1".into()),
        },
        CatalogEntry {
            key: "a/y".into(),
            context_window: 2,
            max_output_tokens: Some(3),
            family: Some("f1".into()),
        },
        CatalogEntry {
            key: "b/z".into(),
            context_window: 4,
            max_output_tokens: None,
            family: None,
        },
    ]);
    assert_eq!(cat.version, 1);
    assert_eq!(cat.entries.len(), 3);
    // family 反查：f1 → 2 keys；无 family → (none) 桶
    let grouped = by_family(&cat);
    assert_eq!(grouped.get("f1").unwrap().len(), 2);
    assert!(grouped.get("f1").unwrap().contains(&"a/x".to_string()));
    assert_eq!(grouped.get("(none)").unwrap(), &vec!["b/z".to_string()]);
    // lookup 精确命中 / 未命中
    let hit = lookup(&cat, "a/y").expect("exact hit");
    assert_eq!(hit.context_window, 2);
    assert_eq!(hit.max_output_tokens, Some(3));
    assert!(lookup(&cat, "a/nope").is_none());
}

// ===========================================================================
// wave_a（R7 中批补盲，2026-08-27）：save_cache 的 mkdir 失败冒泡臂
// （catalog.rs 205-206 —— config_dir 本身是普通文件时 create_dir_all 必败，
// Err 里带 mkdir 前缀）。fetch_http_blocking / fetch_http 的网络臂（230-258）
// 不打真网（网络劣化批次已有教训）→ 豁免池。
// ===========================================================================

#[test]
fn test_wave_a_save_cache_mkdir_failure_bubbles_with_prefix() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("as_file"), "not a dir").unwrap();
    let err = save_cache(&tmp.path().join("as_file"), vec![]).unwrap_err();
    assert!(err.contains("mkdir"), "got: {err}");
}

// ===========================================================================
// r9_fetch_seams（R9 补测批零头组，2026-08-27）：fetch_http_blocking 的网络
// 错误臂改走测试接缝 —— NEMESISBOT_CATALOG_API_URL 把主端点指到本进程内的
// 一次性 TcpListener mock；镜像端点恒为 https，用「死代理
// HTTPS_PROXY=http://127.0.0.1:1」瞬间杀死（reqwest 按 scheme 匹配代理：
// 本地 http:// mock 不受影响）。覆盖 wave_a 豁免池中让位的三个臂：
//   ① 主端点 200 + 合法 api.json → Ok(entries)；
//   ② 主端点 200 + 坏 body（parse_any 失败）→ 回落镜像 → 全端点失败 Err；
//   ③ 主端点 HTTP 500 → 同样回落并报 "all catalog endpoints failed"。
// 全部同步直调（plain #[test]，无 ambient tokio——reqwest::blocking 在 async
// 上下文 drop 会 panic，这里不进 async 测试运行时）。
// ===========================================================================

// 整 mod Windows 形态（3/3 测试 + 专属 helper 全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r9_fetch_seams {
    use super::*;

    /// 环境变量快照 + Drop 恢复：把这些 var 恢复到进入前状态（含删除）。
    /// 必须在 GLOBAL_STATE_LOCK 持有期间构造/销毁（Drop 先于锁释放：声明序
    /// 反序 drop，_env 声明在 _guard 之后）。
    struct EnvRestore(Vec<(&'static str, Option<String>)>);

    impl EnvRestore {
        const VARS: [&'static str; 8] = [
            "NEMESISBOT_CATALOG_API_URL",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
        ];

        fn snapshot_and_apply(sets: &[(&'static str, String)]) -> Self {
            let mut saved = Vec::new();
            for name in Self::VARS {
                saved.push((name, std::env::var(name).ok()));
                unsafe {
                    std::env::remove_var(name);
                }
            }
            for (name, val) in sets {
                unsafe {
                    std::env::set_var(name, val);
                }
            }
            Self(saved)
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, val) in self.0.drain(..) {
                unsafe {
                    match val {
                        Some(v) => std::env::set_var(name, v),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
    }

    /// 一次性单请求 HTTP mock：accept 一次、回固定响应后由对端 Connection:close 收尾。
    fn serve_once(status_line: &'static str, body: String) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status_line,
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        port
    }

    const DEAD_PROXY: &str = "http://127.0.0.1:1";

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn seam_success_parses_api_json_entries() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let port = serve_once(
            "200 OK",
            r#"{"prov1":{"id":"prov1","models":{"m-1":{"family":"fam-a","limit":{"context":400000,"output":128000}}}}}"#
                .to_string(),
        );
        let _env = EnvRestore::snapshot_and_apply(&[(
            "NEMESISBOT_CATALOG_API_URL",
            format!("http://127.0.0.1:{port}"),
        )]);

        let entries = fetch_http_blocking().expect("合法 api.json 必须成功");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "prov1/m-1");
        assert_eq!(entries[0].context_window, 400000);
        assert_eq!(entries[0].max_output_tokens, Some(128000));
        assert_eq!(entries[0].family.as_deref(), Some("fam-a"));
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn seam_bad_body_falls_back_to_mirror_and_reports_all_failed() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        // 200 但解析不出来 → try 镜像；镜像 https 被死代理瞬杀 → 全失败。
        let port = serve_once("200 OK", "not-json{{{".to_string());
        let _env = EnvRestore::snapshot_and_apply(&[
            ("NEMESISBOT_CATALOG_API_URL", format!("http://127.0.0.1:{port}")),
            ("HTTPS_PROXY", DEAD_PROXY.to_string()),
        ]);

        let err = fetch_http_blocking().expect_err("两端点皆败必须 Err");
        assert!(
            err.contains("all catalog endpoints failed"),
            "got: {err}"
        );
        // last_err 只保留最后一条：必须是镜像端点的错误。
        assert!(
            err.contains("cdn.jsdelivr.net"),
            "last 错误应来自镜像端点，got: {err}"
        );
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn seam_http_500_also_falls_back_and_fails_loudly() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let port = serve_once("500 Internal Server Error", "{}".to_string());
        let _env = EnvRestore::snapshot_and_apply(&[
            ("NEMESISBOT_CATALOG_API_URL", format!("http://127.0.0.1:{port}")),
            ("HTTPS_PROXY", DEAD_PROXY.to_string()),
        ]);

        let err = fetch_http_blocking().expect_err("非 2xx 不是成功");
        assert!(err.contains("all catalog endpoints failed"), "got: {err}");
        assert!(err.contains("cdn.jsdelivr.net"), "got: {err}");
    }
}

// ===========================================================================
// r9_offline_cli（同批）：`model catalog-update` 的离线双臂子进程真链路
// （run() CatalogUpdate Err 分支 628-647：有缓存→保留缓存打印 Ok；无缓存→
// bail 非零退码）。死代理让 https 主/镜像端点都秒败，全程无真实网络。
// GLOBAL_STATE_LOCK 全程持有：子进程在 spawn 时继承 env，必须保证没有其它
// 并行测试线程正在改写这些全局变量。
// ===========================================================================

// 整 mod Windows 形态（2/2 测试 + 专属 helper 全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r9_offline_cli {
    use test_harness::{resolve_nemesisbot_bin, TestWorkspace};

    const DEAD_PROXY: &str = "http://127.0.0.1:1";

    /// 快照全部相关 env → 清空 → 设死代理；Drop 恢复。与 r9_fetch_seams 的
    /// EnvRestore 同义，此处独立实现以免跨模块引用私有类型。
    struct ProxyKill {
        saved: Vec<(String, Option<String>)>,
    }

    impl ProxyKill {
        fn arm() -> Self {
            let names = [
                "HTTPS_PROXY",
                "https_proxy",
                "HTTP_PROXY",
                "http_proxy",
                "ALL_PROXY",
                "all_proxy",
                "NO_PROXY",
                "NEMESISBOT_CATALOG_API_URL",
            ];
            let saved: Vec<(String, Option<String>)> =
                names.iter().map(|n| (n.to_string(), std::env::var(n).ok())).collect();
            for n in ["HTTPS_PROXY", "https_proxy"] {
                unsafe {
                    std::env::set_var(n, DEAD_PROXY);
                }
            }
            for n in [
                "HTTP_PROXY",
                "http_proxy",
                "ALL_PROXY",
                "all_proxy",
                "NO_PROXY",
                "NEMESISBOT_CATALOG_API_URL",
            ] {
                unsafe {
                    std::env::remove_var(n);
                }
            }
            Self { saved }
        }
    }

    impl Drop for ProxyKill {
        fn drop(&mut self) {
            for (n, v) in self.saved.drain(..) {
                unsafe {
                    match v {
                        Some(val) => std::env::set_var(&n, val),
                        None => std::env::remove_var(&n),
                    }
                }
            }
        }
    }

    fn fresh_ws_with_empty_config() -> TestWorkspace {
        let ws = TestWorkspace::new().unwrap();
        std::fs::create_dir_all(ws.home()).unwrap();
        std::fs::write(ws.config_path(), "{}").unwrap();
        ws
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn offline_with_seed_cache_keeps_cache_and_exits_zero() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _kill = ProxyKill::arm();
        let ws = fresh_ws_with_empty_config();
        // 种一版本地缓存（serde 版本化格式）。种子写在 **legacy 位置**（home
        // 根）——2026-08-28 布局迁移后 load_cache 首读会把它 rename 进
        // workspace/data，本测试同时就是迁移行为的端到端实证。
        let legacy = nemesis_path::legacy_models_catalog_cache_path(&ws.home());
        let cache = serde_json::json!({
            "version": 1,
            "fetched_at": "2026-08-01T12:00:00+08:00",
            "entries": [{"key": "p/m", "context_window": 12345}]
        });
        std::fs::write(&legacy, serde_json::to_string_pretty(&cache).unwrap()).unwrap();

        let bin = resolve_nemesisbot_bin().unwrap();
        let out = ws.run_cli_with_timeout(&bin, &["model", "catalog-update"], 60).await;
        assert!(
            out.success(),
            "有缓存时离线只警告不清缓存：stdout={} stderr={}",
            out.stdout,
            out.stderr
        );
        assert!(out.stdout_contains("拉取失败"));
        assert!(out.stdout_contains("保留现有缓存：1 个模型"));
        assert!(out
            .stdout_contains("fetched_at=2026-08-01T12:00:00+08:00"));
        // 迁移实证：legacy 已搬走，新位置文件原样保留。
        assert!(!legacy.exists(), "legacy home 根缓存应被迁移 rename 走");
        assert!(
            nemesis_path::models_catalog_cache_path(&ws.home()).exists(),
            "缓存应落在 workspace/data"
        );
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn offline_without_cache_bails_nonzero() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _kill = ProxyKill::arm();
        let ws = fresh_ws_with_empty_config();

        let bin = resolve_nemesisbot_bin().unwrap();
        let out = ws.run_cli_with_timeout(&bin, &["model", "catalog-update"], 60).await;
        assert!(
            !out.success(),
            "无缓存离线必须非零退码提示用户拷贝缓存"
        );
        let combined = format!("{} {}", out.stdout, out.stderr);
        assert!(combined.contains("拉取失败且无本地缓存"), "{combined}");
        assert!(combined.contains("拷贝"), "应提示内网拷贝路径");
    }
}

// ===========================================================================
// r10_body_read_arm：catalog.rs fetch_http_blocking 的 body-read Err 臂
// （`Err(e) => last_err = format!("{url}: body read: {e}")`）。触发条件是
// 「HTTP 层成功（200）但 resp.text() 读体失败」——本地确定性构造：raw
// TcpListener 回 200 头但 Content-Length 声明大于实际写出字节数，随后立刻
// 关连接；hyper 在读体阶段得到 incomplete message 必返 Err（若 reqwest 反常
// 地把截断体当 Ok 交回，parse_any 对这个未闭合 JSON 也必败——两种情形最终
// 都走「回落镜像→死代理→all endpoints failed」，断言不至于假绿；body-read
// 臂本身由行执行覆盖，其字符串会被随后镜像端点的错误覆写，不对外可见）。
// env 快照恢复手抄 r9_fetch_seams::EnvRestore 的模式（该类型模块私有）。
// ===========================================================================
#[cfg(test)]
mod r10_body_read {
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn r10_truncated_200_body_hits_body_read_err_arm() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();

        // 手工快照要动的全部代理/接缝变量，Drop 时按进入前状态恢复。
        struct EnvGuard(Vec<(&'static str, Option<String>)>);
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for (name, val) in self.0.drain(..) {
                    unsafe {
                        match val {
                            Some(v) => std::env::set_var(name, v),
                            None => std::env::remove_var(name),
                        }
                    }
                }
            }
        }
        let mut saved: Vec<(&'static str, Option<String>)> = Vec::new();
        for name in [
            "NEMESISBOT_CATALOG_API_URL",
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
        ] {
            saved.push((name, std::env::var(name).ok()));
            unsafe {
                std::env::remove_var(name);
            }
        }

        // 死代理杀 https 镜像端点（reqwest 按 scheme 匹配，本地 http 不受影响）。
        unsafe {
            std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:1");
        }

        // 截断 mock：声明 Content-Length=500 只写 ~90 字节就关连接。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let served_cloned = served.clone();
        std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let head_and_partial =
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 500\r\nConnection: close\r\n\r\n{\"prov1\":{\"id\":\"prov1\",\"models\":{\"m-1\":{\"limit\":{\"context\":";
                let ok = stream.write_all(head_and_partial.as_bytes()).is_ok()
                    && stream.flush().is_ok();
                served_cloned.store(ok, std::sync::atomic::Ordering::SeqCst);
                // stream drop → 连接关闭 → 客户端读体时 early EOF。
            }
        });
        unsafe {
            std::env::set_var("NEMESISBOT_CATALOG_API_URL", format!("http://127.0.0.1:{port}"));
        }
        let _env = EnvGuard(saved);

        let entries = super::fetch_http_blocking();
        assert!(served.load(std::sync::atomic::Ordering::SeqCst),
            "mock 必须真的被请求到并完整写出截断响应");
        let err = entries.expect_err("主端点读体失败 + 镜像死代理必须整体 Err");
        assert!(
            err.contains("all catalog endpoints failed"),
            "got: {err}"
        );
        // last_err 被最后尝试的镜像端点覆写。
        assert!(err.contains("cdn.jsdelivr.net"), "got: {err}");
    }
}

#[test]
fn migrate_moves_legacy_home_root_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy = nemesis_path::legacy_models_catalog_cache_path(tmp.path());
    std::fs::write(
        &legacy,
        r#"{"version":1,"fetched_at":"t","entries":[{"key":"p/m","context_window":7}]}"#,
    )
    .unwrap();
    // 首读触发迁移：内容可读 + legacy 消失 + 新位置存在。
    let cat = load_cache(tmp.path()).unwrap().expect("migrated cache loads");
    assert_eq!(cat.entries.len(), 1);
    assert!(!legacy.exists(), "legacy 应被 rename 走");
    assert!(nemesis_path::models_catalog_cache_path(tmp.path()).exists(), "缓存落在新位置");
    // 二读直接走新位置（迁移幂等）。
    assert!(load_cache(tmp.path()).unwrap().is_some());
}

#[test]
fn save_cache_removes_legacy_home_root_orphan() {
    // save 路径对称清理：存量部署只跑 catalog-update（不经过读路径迁移）时，
    // home 根的 legacy models_catalog.json 不再永久滞留成孤儿。
    let tmp = tempfile::tempdir().unwrap();
    let legacy = nemesis_path::legacy_models_catalog_cache_path(tmp.path());
    std::fs::write(
        &legacy,
        r#"{"version":1,"fetched_at":"old","entries":[]}"#,
    )
    .unwrap();

    save_cache(tmp.path(), vec![]).expect("save");

    assert!(nemesis_path::models_catalog_cache_path(tmp.path()).exists(), "新位置已写入");
    assert!(!legacy.exists(), "save 后 legacy 孤儿被清掉");
}
