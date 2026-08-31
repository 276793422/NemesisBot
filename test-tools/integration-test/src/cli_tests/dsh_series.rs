//! DSH-series new-CLI coverage (4b layer-④ gap fill): the series added
//! `session list/show/fork`, `history search/reindex`, `credentials import`,
//! `model set-effort`, `model catalog-update` — none of which had a single
//! integration assertion (verified by full-source grep, 2026-08-24).
//!
//! Ordering constraint: these suites run in Phase 1 BEFORE main.rs restores
//! the clean config, so config mutations here (set-effort write, credentials
//! import rewriting api_key to `yaml:` refs) are safe for gateway phases.

use serde_json::Value;
use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// model set-effort <name> <off|low|medium|high>  (U16effort)
// ---------------------------------------------------------------------------

pub async fn test_cli_model_set_effort(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/model_set_effort";
    let mut results = Vec::new();
    print_suite_header(suite);

    // A model to operate on (plaintext key — also feeds credentials_import).
    let _ = ws
        .run_cli(
            bin,
            &[
                "model",
                "add",
                "--model",
                "test/effort-model",
                "--base",
                "http://127.0.0.1:8080/v1",
                "--key",
                "sk-plain-it123",
            ],
        )
        .await;

    // Set a tier -> exit 0 + echoed value + config carries the field.
    let out = ws
        .run_cli(bin, &["model", "set-effort", "test/effort-model", "low"])
        .await;
    if out.success() && out.stdout_contains("reasoning_effort=low") {
        results.push(pass(&format!("{}/set_low", suite), "exit=0 + echo"));
    } else {
        results.push(fail(
            &format!("{}/set_low", suite),
            format!(
                "exit={} stdout='{}' stderr='{}'",
                out.exit_code,
                out.stdout.trim(),
                out.stderr.trim()
            ),
        ));
    }
    if let Some(v) = read_model_field(ws, "test/effort-model", "reasoning_effort") {
        if v == "low" {
            results.push(pass(
                &format!("{}/config_low", suite),
                "model_list[].reasoning_effort==low",
            ));
        } else {
            results.push(fail(
                &format!("{}/config_low", suite),
                format!("reasoning_effort={v}"),
            ));
        }
    }

    // "off" clears the field (empty string = send nothing).
    let out = ws
        .run_cli(bin, &["model", "set-effort", "test/effort-model", "off"])
        .await;
    if out.success() && out.stdout_contains("cleared") {
        results.push(pass(&format!("{}/set_off", suite), "off clears"));
    } else {
        results.push(fail(
            &format!("{}/set_off", suite),
            format!("exit={} stdout='{}'", out.exit_code, out.stdout.trim()),
        ));
    }

    // Invalid tier -> non-zero + loud refusal.
    let out = ws
        .run_cli(bin, &["model", "set-effort", "test/effort-model", "ultra"])
        .await;
    if !out.success()
        && (out.stderr_contains("Invalid effort") || out.stdout_contains("Invalid effort"))
    {
        results.push(pass(
            &format!("{}/invalid", suite),
            "non-zero + Invalid effort",
        ));
    } else {
        results.push(fail(
            &format!("{}/invalid", suite),
            format!(
                "exit={} stdout='{}' stderr='{}'",
                out.exit_code,
                out.stdout.trim(),
                out.stderr.trim()
            ),
        ));
    }

    // Unknown model -> non-zero (must not silently create / no-op succeed).
    let out = ws
        .run_cli(bin, &["model", "set-effort", "test/no-such-model", "low"])
        .await;
    results.push(if !out.success()
        && (out.stderr_contains("Model not found") || out.stdout_contains("Model not found"))
    {
        pass(
            &format!("{}/unknown_model", suite),
            "non-zero + Model not found",
        )
    } else {
        fail(
            &format!("{}/unknown_model", suite),
            format!(
                "exit={} stdout='{}' stderr='{}'",
                out.exit_code,
                out.stdout.trim(),
                out.stderr.trim()
            ),
        )
    });

    // Horizontal regression (bug-fix #6): every mutating model command must
    // fail loudly on a missing entry — set-tier / set-size / remove previously
    // printed "Model not found" and still exited 0.
    for (label, args) in [
        ("set_tier", vec!["model", "set-tier", "test/no-such-model", "big"]),
        ("set_size", vec!["model", "set-size", "test/no-such-model", "30B"]),
        (
            "set_real_name",
            vec!["model", "set-real-name", "test/no-such-model", "X"],
        ),
        ("remove", vec!["model", "remove", "test/no-such-model", "--force"]),
    ] {
        let out = ws.run_cli(bin, &args).await;
        results.push(if !out.success() {
            pass(&format!("{}/missing_{label}", suite), "non-zero on unknown model")
        } else {
            fail(
                &format!("{}/missing_{label}", suite),
                "exit=0 on unknown model",
            )
        });
    }

    results
}

// ---------------------------------------------------------------------------
// session list / show / fork  (Z1 Phase4-d)
// ---------------------------------------------------------------------------

pub async fn test_cli_session_fork(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/session_fork";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Fresh home: list is a graceful empty, not an error.
    let out = ws.run_cli(bin, &["session", "list"]).await;
    if out.success() {
        results.push(pass(&format!("{}/list_exit", suite), "exit=0"));
    } else {
        results.push(fail(
            &format!("{}/list_exit", suite),
            format!("exit={}", out.exit_code),
        ));
    }

    // show/fork on a nonexistent key: loud non-zero, no panic.
    let out = ws
        .run_cli(bin, &["session", "show", "agent:main:session:nonexistent"])
        .await;
    let show_missing_ok = !out.success()
        && (out.stderr_contains("不存在") || out.stdout_contains("不存在"));
    results.push(if show_missing_ok {
        pass(&format!("{}/show_missing", suite), "non-zero + 不存在")
    } else {
        fail(
            &format!("{}/show_missing", suite),
            format!(
                "exit={} stdout='{}' stderr='{}'",
                out.exit_code,
                out.stdout.trim(),
                out.stderr.trim()
            ),
        )
    });

    let out = ws
        .run_cli(bin, &["session", "fork", "agent:main:session:nonexistent"])
        .await;
    results.push(if !out.success() {
        pass(
            &format!("{}/fork_missing", suite),
            "non-zero on missing source",
        )
    } else {
        fail(&format!("{}/fork_missing", suite), "exit=0 on missing source")
    });

    // Fabricate a real 2-turn session + its chat log. The two stores are
    // DELIBERATELY divergent on timestamps (store = 12:xx, jsonl = 10:xx) —
    // 2026-08-25 round 3: jsonl is the single source of truth for fork
    // content, so the fork must carry the JSONL timestamps verbatim, never
    // the store's.
    let key = "agent:main:session:it1";
    let safe = key.replace(':', "_");
    let sess_path = ws.workspace().join("sessions").join(format!("{safe}.json"));
    std::fs::create_dir_all(sess_path.parent().unwrap()).unwrap();
    let session_json = serde_json::json!({
        "key": key,
        "messages": [
            {"role": "user", "content": "ITFORK turn one question", "tool_calls": [], "tool_call_id": null, "timestamp": "2026-08-24T12:00:00+08:00"},
            {"role": "assistant", "content": "ITFORK turn one answer", "tool_calls": [], "tool_call_id": null, "timestamp": "2026-08-24T12:00:05+08:00"},
            {"role": "user", "content": "ITFORK turn two question", "tool_calls": [], "tool_call_id": null, "timestamp": "2026-08-24T12:01:00+08:00"},
            {"role": "assistant", "content": "ITFORK turn two answer", "tool_calls": [], "tool_call_id": null, "timestamp": "2026-08-24T12:01:05+08:00"}
        ],
        "summary": "",
        "created": "2026-08-24T10:00:00+08:00",
        "updated": "2026-08-24T10:01:05+08:00"
    });
    std::fs::write(&sess_path, session_json.to_string()).unwrap();

    let log_path = ws
        .workspace()
        .join("logs")
        .join("session_logs")
        .join(format!("{safe}.jsonl"));
    std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
    let log_lines: Vec<String> = [
        ("user", "ITFORK turn one question", "2026-08-24T10:00:00+08:00"),
        ("assistant", "ITFORK turn one answer", "2026-08-24T10:00:05+08:00"),
        ("user", "ITFORK turn two question", "2026-08-24T10:01:00+08:00"),
        ("assistant", "ITFORK turn two answer", "2026-08-24T10:01:05+08:00"),
    ]
    .iter()
    .map(|(role, content, ts)| {
        serde_json::json!({"role": role, "content": content, "timestamp": ts}).to_string()
    })
    .collect();
    std::fs::write(&log_path, log_lines.join("\n") + "\n").unwrap();
    let source_before = std::fs::read_to_string(&sess_path).unwrap();

    // list now shows the session.
    let out = ws.run_cli(bin, &["session", "list"]).await;
    results.push(if out.success() && out.stdout_contains("agent:main:session:it1") {
        pass(&format!("{}/list_shows", suite), "key listed")
    } else {
        fail(
            &format!("{}/list_shows", suite),
            format!("stdout='{}'", out.stdout.trim()),
        )
    });

    // show prints the --at boundary table (2 turns + preview).
    let out = ws.run_cli(bin, &["session", "show", key]).await;
    results.push(
        if out.success()
            && out.stdout_contains("--at 2")
            && out.stdout_contains("ITFORK turn two question")
        {
            pass(&format!("{}/show_turns", suite), "boundary table + preview")
        } else {
            fail(
                &format!("{}/show_turns", suite),
                format!("stdout='{}'", out.stdout.trim()),
            )
        },
    );

    // fork --at 1: new key exists, keeps turn 1 only (2 messages).
    let out = ws
        .run_cli(
            bin,
            &["session", "fork", key, "--at", "1", "--new-key", "agent:main:session:it1f"],
        )
        .await;
    if out.success()
        && out.stdout_contains("会话分支完成")
        && out.stdout_contains("agent:main:session:it1f")
    {
        results.push(pass(&format!("{}/fork_exit", suite), "exit=0 + report"));
    } else {
        results.push(fail(
            &format!("{}/fork_exit", suite),
            format!(
                "exit={} stdout='{}' stderr='{}'",
                out.exit_code,
                out.stdout.trim(),
                out.stderr.trim()
            ),
        ));
    }
    let fork_path = ws
        .workspace()
        .join("sessions")
        .join("agent_main_session_it1f.json");
    match std::fs::read_to_string(&fork_path).map(|d| serde_json::from_str::<Value>(&d)) {
        Ok(Ok(s)) => {
            let n = s
                .get("messages")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let first = s
                .pointer("/messages/0/content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if n == 2 && first == "ITFORK turn one question" {
                results.push(pass(
                    &format!("{}/fork_file", suite),
                    "2 msgs, turn-1 prefix",
                ));
            } else {
                results.push(fail(
                    &format!("{}/fork_file", suite),
                    format!("n={n} first={first}"),
                ));
            }
        }
        _ => results.push(fail(
            &format!("{}/fork_file", suite),
            "fork session file missing/unparseable",
        )),
    }

    // Source session untouched (true branch, not a rollback).
    let source_after = std::fs::read_to_string(&sess_path).unwrap_or_default();
    results.push(if source_after == source_before {
        pass(&format!("{}/source_untouched", suite), "byte-identical")
    } else {
        fail(&format!("{}/source_untouched", suite), "source session mutated")
    });

    // Chat-log copied VERBATIM from the source jsonl prefix (2026-08-25
    // 第三轮契约): rows = source jsonl lines [..cut], byte-for-byte — the
    // store is a rebuildable cache and never defines fork content. --at 1
    // keeps [u1, a1]: expect 2 rows with the JSONL timestamps (10:00:00 /
    // 10:00:05), NOT the store's divergent 12:xx ones.
    let fork_log = ws
        .workspace()
        .join("logs")
        .join("session_logs")
        .join("agent_main_session_it1f.jsonl");
    if let Ok(fdata) = std::fs::read_to_string(&fork_log) {
        let rows: Vec<Value> = fdata
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect();
        let ok = rows.len() == 2
            && rows[0]["role"] == "user"
            && rows[0]["content"] == "ITFORK turn one question"
            && rows[0]["timestamp"] == "2026-08-24T10:00:00+08:00"
            && rows[1]["role"] == "assistant"
            && rows[1]["content"] == "ITFORK turn one answer"
            && rows[1]["timestamp"] == "2026-08-24T10:00:05+08:00";
        results.push(if ok {
            pass(&format!("{}/chatlog_prefix", suite), "jsonl verbatim 2-row prefix")
        } else {
            fail(
                &format!("{}/chatlog_prefix", suite),
                format!("fork rows={rows:?}"),
            )
        });
    } else {
        results.push(fail(
            &format!("{}/chatlog_prefix", suite),
            "fork chat log missing",
        ));
    }

    results
}

// ---------------------------------------------------------------------------
// history search / reindex  (U20)
// ---------------------------------------------------------------------------

pub async fn test_cli_history_search(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/history_search";
    let mut results = Vec::new();
    print_suite_header(suite);

    // reindex on a quiet home: exit 0, deterministic summary line.
    let out = ws.run_cli(bin, &["history", "reindex"]).await;
    results.push(
        if out.success() && out.stdout_contains("重建索引完成") {
            pass(&format!("{}/reindex", suite), "exit=0")
        } else {
            fail(
                &format!("{}/reindex", suite),
                format!(
                    "exit={} stdout='{}' stderr='{}'",
                    out.exit_code,
                    out.stdout.trim(),
                    out.stderr.trim()
                ),
            )
        },
    );

    // No-hit query: graceful empty result, not an error.
    let out = ws
        .run_cli(bin, &["history", "search", "zebraunicorn42", "--limit", "5"])
        .await;
    results.push(
        if out.success() && out.stdout_contains("没有找到匹配") {
            pass(&format!("{}/no_hit", suite), "graceful empty")
        } else {
            fail(
                &format!("{}/no_hit", suite),
                format!("stdout='{}'", out.stdout.trim()),
            )
        },
    );

    // A hit: a fabricated chat log must be found and attributed to its session.
    let key = "agent:main:session:hit1";
    let safe = key.replace(':', "_");
    let log_path = ws
        .workspace()
        .join("logs")
        .join("session_logs")
        .join(format!("{safe}.jsonl"));
    std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
    let line = serde_json::json!({
        "role": "user",
        "content": "ITFINDING unique oxstamp 7788",
        "timestamp": "2026-08-24T11:00:00+08:00"
    });
    std::fs::write(&log_path, line.to_string() + "\n").unwrap();

    let out = ws.run_cli(bin, &["history", "search", "oxstamp"]).await;
    results.push(
        if out.success()
            && out.stdout_contains("找到 1 条匹配")
            && out.stdout_contains("agent_main_session_hit1")
            && out.stdout_contains("ITFINDING unique oxstamp 7788")
        {
            pass(&format!("{}/hit", suite), "hit + session key + snippet")
        } else {
            fail(
                &format!("{}/hit", suite),
                format!("stdout='{}'", out.stdout.trim()),
            )
        },
    );

    results
}

// ---------------------------------------------------------------------------
// credentials import  (U15 T1)
// ---------------------------------------------------------------------------

pub async fn test_cli_credentials_import(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/credentials_import";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Pre-seeded by set_effort suite: test/effort-model has plaintext
    // "sk-plain-it123". Import must migrate it. Earlier suites (model add
    // in 1.2a) may contribute further plaintext keys, so we assert the
    // specific entry rather than an exact total count; the migrated list
    // prints the entry's `name` field ("effort-model"), not the `model`
    // path ("test/effort-model").
    let out = ws.run_cli(bin, &["credentials", "import"]).await;
    results.push(
        if out.success()
            && out.stdout_contains("个明文 key")
            && out.stdout_contains("effort-model -> yaml:")
        {
            pass(&format!("{}/migrate", suite), "plaintext -> yaml: alias")
        } else {
            fail(
                &format!("{}/migrate", suite),
                format!(
                    "exit={} stdout='{}' stderr='{}'",
                    out.exit_code,
                    out.stdout.trim(),
                    out.stderr.trim()
                ),
            )
        },
    );

    // Config entry now references the alias instead of the plaintext.
    if let Some(v) = read_model_field(ws, "test/effort-model", "api_key") {
        results.push(if v.starts_with("yaml:") {
            pass(&format!("{}/config_ref", suite), format!("api_key={v}"))
        } else {
            fail(&format!("{}/config_ref", suite), format!("api_key={v}"))
        });
    }

    // The yaml store exists and holds the plaintext value.
    let cred_path = ws.workspace().join("config").join("credentials.yaml");
    match std::fs::read_to_string(&cred_path) {
        Ok(text) => results.push(if text.contains("sk-plain-it123") {
            pass(
                &format!("{}/yaml_store", suite),
                "plaintext landed in credentials.yaml",
            )
        } else {
            fail(&format!("{}/yaml_store", suite), "value missing from yaml")
        }),
        Err(e) => results.push(fail(
            &format!("{}/yaml_store", suite),
            format!("read: {e}"),
        )),
    }

    // Idempotent: a second run is a noop.
    let out = ws.run_cli(bin, &["credentials", "import"]).await;
    results.push(
        if out.success() && out.stdout_contains("没有需要迁移的明文 key") {
            pass(&format!("{}/idempotent", suite), "second run noop")
        } else {
            fail(
                &format!("{}/idempotent", suite),
                format!("stdout='{}'", out.stdout.trim()),
            )
        },
    );

    results
}

// ---------------------------------------------------------------------------
// model catalog-update  (U16)
// ---------------------------------------------------------------------------

pub async fn test_cli_model_catalog_update(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/model_catalog_update";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Online: "目录已更新" + cache file. Offline: non-zero + loud report
    // (kept-cache or no-cache message) — both are the documented contract;
    // a crash/panic is the only failure mode we reject. The fetch can
    // legitimately exceed the default 15s CLI timeout, so allow 90s.
    let out = ws
        .run_cli_with_timeout(bin, &["model", "catalog-update"], 90)
        .await;
    let online = out.success() && out.stdout_contains("目录已更新");
    // 离线大声报告会落在两个流：进度行走 stdout（「正在拉取…」），终态
    // bail! 走 stderr（「Error: 拉取失败…」）。2026-08-24 复检前只查 stdout
    // ——之前每轮网络都好、只走过 online 分支，离线分支首次被真实验证
    // （models.dev + jsDelivr 双端点不通）时暴露检测串找错流 → 假 FAIL。
    let offline = !out.success()
        && (out.stdout_contains("拉取失败")
            || out.stdout_contains("目录不可用")
            || out.stderr_contains("拉取失败")
            || out.stderr_contains("目录不可用"));
    results.push(if online || offline {
        pass(
            &format!("{}/contract", suite),
            if online { "online updated" } else { "offline non-zero" },
        )
    } else {
        fail(
            &format!("{}/contract", suite),
            format!(
                "exit={} stdout='{}' stderr='{}'",
                out.exit_code,
                out.stdout.trim(),
                out.stderr.trim()
            ),
        )
    });
    if online {
        // Cache lands under the workspace data dir (2026-08-28 moved from the
        // home root): `<home>/workspace/data/models_catalog.json` — NOT
        // workspace/config. Path single-sourced in nemesis-path
        // (models_catalog_cache_path); this literal pins the real disk layout.
        let cache = ws
            .home()
            .join("workspace")
            .join("data")
            .join("models_catalog.json");
        results.push(if cache.is_file() {
            pass(&format!("{}/cache_file", suite), "workspace/data/models_catalog.json written")
        } else {
            fail(&format!("{}/cache_file", suite), "cache missing after online update")
        });
    }

    results
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Read one field off a model_list entry by model/name (None -> no assertion).
fn read_model_field(ws: &TestWorkspace, model: &str, field: &str) -> Option<String> {
    let data = std::fs::read_to_string(ws.config_path()).ok()?;
    let cfg: Value = serde_json::from_str(&data).ok()?;
    let arr = cfg.get("model_list")?.as_array()?;
    for m in arr {
        let name = m
            .get("model")
            .and_then(|v| v.as_str())
            .or_else(|| m.get("name").and_then(|v| v.as_str()));
        if name == Some(model) {
            return m.get(field).and_then(|v| v.as_str()).map(|s| s.to_string());
        }
    }
    None
}
