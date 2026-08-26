//! eval_rules 命令组单测（第九轮 AA1：此前只有会话内手测，无自动化）。
//!
//! cmd_* 函数都是 path 参数化的纯文件操作，直接对临时目录测。
//! `run()` 本身（home 解析 + clap 分派）不在这里——那是 CLI 层。

use super::*;

fn temp_rules() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("eval_rules.json");
    (tmp, path)
}

fn rule_json(id: &str) -> String {
    format!(
        r#"{{"id":"{id}","description":"d","level":"low","source":"subject","conditions":[{{"field":"text","op":"contains","value":"x"}}]}}"#
    )
}

#[test]
fn cmd_list_seeds_and_lists_defaults() {
    let (_tmp, path) = temp_rules();
    cmd_list(&path).unwrap();
    assert!(path.exists(), "list must seed the default set on first run");
    // 再次 list 稳定（不重复种子）。
    cmd_list(&path).unwrap();
}

#[test]
fn cmd_add_remove_roundtrip() {
    let (_tmp, path) = temp_rules();
    let in_file = path.parent().unwrap().join("new_rule.json");
    std::fs::write(&in_file, rule_json("test-add")).unwrap();

    cmd_add(&path, &in_file).unwrap();
    let file = eval_assessor::load_rules(&path).unwrap();
    assert!(file.rules.iter().any(|r| r.id == "test-add"));

    // 重复 add 同 id 拒绝。
    assert!(cmd_add(&path, &in_file).is_err());

    cmd_remove(&path, "test-add").unwrap();
    let file = eval_assessor::load_rules(&path).unwrap();
    assert!(!file.rules.iter().any(|r| r.id == "test-add"));

    // 再删报错（不存在）。
    assert!(cmd_remove(&path, "test-add").is_err());
}

#[test]
fn cmd_toggle_enable_disable() {
    let (_tmp, path) = temp_rules();
    eval_assessor::load_rules(&path).unwrap(); // seed

    cmd_toggle(&path, "outbox-deny-ssh", false).unwrap();
    let file = eval_assessor::load_rules(&path).unwrap();
    assert!(!file.rules.iter().find(|r| r.id == "outbox-deny-ssh").unwrap().enabled);

    cmd_toggle(&path, "outbox-deny-ssh", true).unwrap();
    let file = eval_assessor::load_rules(&path).unwrap();
    assert!(file.rules.iter().find(|r| r.id == "outbox-deny-ssh").unwrap().enabled);

    // 不存在的 id 报错。
    assert!(cmd_toggle(&path, "no-such-rule", true).is_err());
}

#[test]
fn cmd_edit_rejects_id_mismatch_and_missing() {
    let (_tmp, path) = temp_rules();
    eval_assessor::load_rules(&path).unwrap(); // seed

    let in_file = path.parent().unwrap().join("edit_rule.json");
    std::fs::write(&in_file, rule_json("some-other-id")).unwrap();
    // 文件 id ≠ 命令行 id → 拒绝。
    assert!(cmd_edit(&path, "outbox-deny-ssh", &in_file).is_err());

    std::fs::write(&in_file, rule_json("no-such-rule")).unwrap();
    // id 匹配但规则不存在 → 拒绝。
    assert!(cmd_edit(&path, "no-such-rule", &in_file).is_err());
}

#[test]
fn cmd_edit_replaces_in_place() {
    let (_tmp, path) = temp_rules();
    eval_assessor::load_rules(&path).unwrap();

    let in_file = path.parent().unwrap().join("edit_rule.json");
    let new_rule = r#"{"id":"outbox-deny-ssh","description":"改过的描述","level":"high","source":"subject","conditions":[{"field":"text","op":"exists"}]}"#;
    std::fs::write(&in_file, new_rule).unwrap();

    cmd_edit(&path, "outbox-deny-ssh", &in_file).unwrap();
    let file = eval_assessor::load_rules(&path).unwrap();
    let r = file.rules.iter().find(|r| r.id == "outbox-deny-ssh").unwrap();
    assert_eq!(r.description, "改过的描述");
    assert_eq!(r.level, "high");
    // 替换而非追加。
    assert_eq!(file.rules.len(), eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON).unwrap().rules.len());
}

#[test]
fn cmd_edit_rejects_multi_rule_file() {
    // R5 回归（自动化）：多条规则的文件走 edit → 明确拒绝。
    let (_tmp, path) = temp_rules();
    eval_assessor::load_rules(&path).unwrap();

    let in_file = path.parent().unwrap().join("multi.json");
    std::fs::write(&in_file, format!("{{\"rules\":[{},{}]}}", rule_json("a"), rule_json("b"))).unwrap();
    assert!(cmd_edit(&path, "a", &in_file).is_err());
}

#[test]
fn cmd_reset_force_restores_defaults() {
    let (_tmp, path) = temp_rules();
    eval_assessor::load_rules(&path).unwrap();

    // 加一条自定义规则再 reset --force → 回到默认集。
    let in_file = path.parent().unwrap().join("new_rule.json");
    std::fs::write(&in_file, rule_json("custom-x")).unwrap();
    cmd_add(&path, &in_file).unwrap();

    cmd_reset(&path, true).unwrap();
    let file = eval_assessor::load_rules(&path).unwrap();
    assert!(!file.rules.iter().any(|r| r.id == "custom-x"));
    let defaults = eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON).unwrap();
    assert_eq!(file.rules.len(), defaults.rules.len());
}

#[test]
fn cmd_reset_non_force_aborts_on_eof_stdin() {
    // BB2b 回归：非 force + stdin EOF（cargo test 的 stdin）→ read_line 得
    // 空串 → 非 y → Aborted，规则文件**保持原样**（不重置）。
    let (_tmp, path) = temp_rules();
    eval_assessor::load_rules(&path).unwrap();
    let in_file = path.parent().unwrap().join("new_rule.json");
    std::fs::write(&in_file, rule_json("keep-me")).unwrap();
    cmd_add(&path, &in_file).unwrap();

    cmd_reset(&path, false).unwrap(); // EOF stdin → abort
    let file = eval_assessor::load_rules(&path).unwrap();
    assert!(file.rules.iter().any(|r| r.id == "keep-me"), "aborted reset must not change rules");
}

// ── M1 补测（quality-hardening goal 2026-08-25）：向导纯函数 + cmd_show +
//    add 空 rules 拒绝 + BB4 run() 双分支 ─────────────────────────────────

#[test]
fn keyword_to_pattern_escapes_metachars_and_matches_literal() {
    // (?i) 前缀 + 元字符全部转义（用户输入按字面量处理）
    assert_eq!(keyword_to_pattern("a.b"), r"(?i)a\.b");
    assert_eq!(keyword_to_pattern(".+*?()[]{}^$|"), r"(?i)\.\+\*\?\(\)\[\]\{\}\^\$\|");
    // 产物必须可编译且语义是字面量：a.b 不匹配 axb（点被转义）。
    let re = regex::Regex::new(&keyword_to_pattern("secret.pem")).unwrap();
    assert!(re.is_match("reading SECRET.PEM file"), "case-insensitive literal match");
    assert!(!re.is_match("secretXpem"), "escaped dot must not match arbitrary char");
}

#[test]
fn keyword_to_pattern_slash_equivalence() {
    // 两种斜杠都编译成 [\ \/]（写哪种都匹配哪种）
    assert_eq!(keyword_to_pattern("d:/x"), r"(?i)d:[\\/]x");
    assert_eq!(keyword_to_pattern("d:\\x"), r"(?i)d:[\\/]x");
    // 行为：正斜杠关键词匹配反斜杠路径，反之亦然（大小写不敏感）。
    let re = regex::Regex::new(&keyword_to_pattern("d:/secret")).unwrap();
    assert!(re.is_match(r"D:\secret"));
    assert!(re.is_match("d:/secret"));
    let re2 = regex::Regex::new(&keyword_to_pattern(r"C:\Users\k")).unwrap();
    assert!(re2.is_match("c:/users/K"));
}

#[test]
fn slugify_normalizes_to_kebab() {
    assert_eq!(slugify("my key.pem"), "my-key-pem");
    assert_eq!(slugify("Reg Add"), "reg-add");
    // 连续分隔压缩、开头/结尾分隔去除
    assert_eq!(slugify("a--b"), "a-b");
    assert_eq!(slugify("-lead-"), "lead");
    // 全非法字符（含纯中文）→ custom 兜底
    assert_eq!(slugify("!!!"), "custom");
    assert_eq!(slugify("密码"), "custom");
    // 40 字符上限
    let long = "a".repeat(50);
    assert_eq!(slugify(&long).len(), 40);
}

fn cond(field: &str, op: &str, value: serde_json::Value) -> eval_assessor::Condition {
    eval_assessor::Condition { field: field.into(), op: op.into(), value }
}

fn cond_rule(conditions: Vec<eval_assessor::Condition>, min_count: usize) -> eval_assessor::Rule {
    eval_assessor::Rule {
        id: "t".into(),
        description: "d".into(),
        level: "low".into(),
        enabled: true,
        source: "subject".into(),
        conditions,
        min_count,
    }
}

#[test]
fn condition_summary_formats_all_ops_and_min_count() {
    // 五种 op 的格式化 + 未知 op 兜底
    let r = cond_rule(
        vec![
            cond("arguments.path", "exists", serde_json::Value::Null),
            cond("count", "equals", serde_json::json!(5)),
            cond("text", "contains", serde_json::json!("越狱")),
            cond("arguments.command", "regex", serde_json::json!("(?i)curl")),
            cond("retries", "gt", serde_json::json!(3)),
            cond("a", "within", serde_json::json!("b")),
        ],
        2,
    );
    let s = condition_summary(&r);
    assert!(s.contains("arguments.path 存在"), "got: {s}");
    assert!(s.contains("count == 5"), "got: {s}");
    assert!(s.contains("text 含 '越狱'"), "got: {s}");
    assert!(s.contains("arguments.command 匹配 /(?i)curl/"), "got: {s}");
    assert!(s.contains("retries > 3"), "got: {s}");
    assert!(s.contains("a within b"), "unknown op falls back to generic, got: {s}");
    assert!(s.contains(" 且 "), "conditions joined by 且, got: {s}");
    assert!(s.ends_with("（≥2 条记录）"), "min_count>1 suffix, got: {s}");
    // min_count=1（默认）无后缀
    let r1 = cond_rule(vec![cond("text", "exists", serde_json::Value::Null)], 1);
    assert!(!condition_summary(&r1).contains("条记录"));
}

#[test]
fn condition_summary_truncates_long_values_on_char_boundary() {
    // 19 个中文（57B）+ "ab"（2B）= 59B；第 60B 落在下一个中文字符中间 →
    // truncation_point 必须回退到 59（char boundary），不 panic。
    let mut v = "密".repeat(19);
    v.push_str("ab");
    v.push_str(&"尾".repeat(10));
    let r = cond_rule(vec![cond("text", "contains", serde_json::json!(v))], 1);
    let s = condition_summary(&r);
    assert!(s.contains('…'), "long value must be truncated with ellipsis, got: {s}");
    let expected_prefix: String = v.chars().take(21).collect(); // 19中文+ab = 21 chars = 59 bytes
    assert!(s.contains(&format!("{expected_prefix}…")), "truncate at 59B boundary, got: {s}");
    // 恰好 60B（20 个中文）不截断
    let v60 = "密".repeat(20);
    let r60 = cond_rule(vec![cond("text", "contains", serde_json::json!(v60.clone()))], 1);
    let s60 = condition_summary(&r60);
    assert!(!s60.contains('…'), "exactly 60 bytes must not truncate, got: {s60}");
    assert!(s60.contains(&v60));
}

#[test]
fn cmd_show_found_and_not_found() {
    let (_tmp, path) = temp_rules();
    eval_assessor::load_rules(&path).unwrap(); // seed 默认集

    cmd_show(&path, "outbox-deny-ssh").unwrap();
    assert!(cmd_show(&path, "no-such-rule").is_err());
}

#[test]
fn cmd_add_rejects_empty_rules_file() {
    let (_tmp, path) = temp_rules();
    eval_assessor::load_rules(&path).unwrap(); // seed
    let in_file = path.parent().unwrap().join("empty.json");
    std::fs::write(&in_file, r#"{"rules":[]}"#).unwrap();
    let err = cmd_add(&path, &in_file).unwrap_err();
    assert!(err.to_string().contains("no rules"), "got: {err}");
    // 规则文件保持默认集不变
    let file = eval_assessor::load_rules(&path).unwrap();
    let defaults = eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON).unwrap();
    assert_eq!(file.rules.len(), defaults.rules.len());
}

#[tokio::test]
async fn bb4_local_missing_write_rejected_readonly_degrades() {
    // BB4 防静默建家回归：--local home 不存在时——写命令 bail（且不创建
    // .nemesisbot）；只读命令降级展示内置默认集（零落盘）。
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let orig = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    // 写命令：bail 发生在读输入文件之前，in.json 无需存在。
    let add = run(RulesAction::Add { file: "in.json".into(), local: true }, false).await;
    // 只读 list / show：降级内置默认集。
    let list = run(RulesAction::List { local: true }, false).await;
    let show_ok = run(RulesAction::Show { id: "outbox-deny-ssh".into(), local: true }, false).await;
    let show_missing = run(RulesAction::Show { id: "zzz-no-such".into(), local: true }, false).await;
    let home_created = tmp.path().join(".nemesisbot").exists();

    std::env::set_current_dir(&orig).unwrap();

    let add_err = add.expect_err("write cmd must bail when --local home missing");
    assert!(add_err.to_string().contains("--local home 不存在"), "got: {add_err}");
    list.expect("readonly list must degrade to built-in defaults");
    show_ok.expect("readonly show (id in defaults) must degrade to built-in defaults");
    assert!(show_missing.is_err(), "show of unknown id in defaults must error");
    assert!(!home_created, "--local home must not be silently created");
}

// ===========================================================================
// S11c（quality-hardening goal 冲刺 S11）：cmd_new 的 EOF 取消臂（stdin 管道
// EOF → ask 返回 "" → kind 默认 1 → 路径空 → bail）+ run() 非 BB4 的正常
// 分发臂（既有测试只跑过 local-missing 分支）。env home 隔离。
// ===========================================================================

#[test]
fn cmd_new_eof_stdin_cancels_with_path_empty_error() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("eval_rules.json");
    let err = cmd_new(&path).expect_err("stdin EOF → kind=1 → kw 空 → bail");
    assert!(err.to_string().contains("路径为空"), "got: {err:#}");
    assert!(!path.exists(), "取消路径绝不落盘");
}

#[tokio::test]
async fn run_dispatch_list_toggle_and_remove_via_env_home() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("NEMESISBOT_HOME", tmp.path());
    }
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();

    // List：无规则文件时种子默认集并列表（run → cmd_list 全链路）。
    run(RulesAction::List { local: false }, false)
        .await
        .expect("run List ok");
    let rules_path = eval_assessor::rules_file_path(&home);
    assert!(rules_path.exists(), "List 种子默认集要落盘");

    // 取一个真实 id 走 Enable/Disable/Remove 分发臂。
    let seeded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&rules_path).unwrap()).unwrap();
    let id = seeded["rules"][0]["id"]
        .as_str()
        .expect("默认集第一条必须有 id")
        .to_string();

    run(
        RulesAction::Disable { id: id.clone(), local: false },
        false,
    )
    .await
    .expect("run Disable ok");
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&rules_path).unwrap()).unwrap();
    let disabled: Vec<&str> = after["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["id"] == *id && r["enabled"] == false)
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(disabled.len(), 1, "disable 经分发生效");

    run(RulesAction::Enable { id: id.clone(), local: false }, false)
        .await
        .expect("run Enable ok");
    run(RulesAction::Remove { id: id.clone(), local: false }, false)
        .await
        .expect("run Remove ok");
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&rules_path).unwrap()).unwrap();
    assert!(
        after["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"] != *id),
        "remove 经分发生效"
    );

    // Show（存在 id）+ cli_local 与 per-command local 取或——这里 home 存在，
    // 直接走正常读路径。
    run(
        RulesAction::Show {
            id: after["rules"][0]["id"].as_str().unwrap().to_string(),
            local: false,
        },
        false,
    )
    .await
    .expect("run Show ok");

    unsafe {
        std::env::remove_var("NEMESISBOT_HOME");
    }
}
