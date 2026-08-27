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

// ===========================================================================
// wave_b（覆盖率补测 2026-08-27）：run() 分发的元组/臂缺口（98/100/104 +
// 142/143/144/148）+ print_rule_table 空集直调（197-199）。stdin 只能 EOF
// （cargo test 的 stdin 是管道），向导的交互深度分支（kind 2/3、去重循环、
// 预览确认、ask_level）属结构性不可测 → EXEMPT，见任务报告。
// 涉及 NEMESISBOT_HOME 的测试持 GLOBAL_STATE_LOCK，prev-value 按 Option 恢复。
// ===========================================================================

mod wave_b {
    use super::*;

    /// NEMESISBOT_HOME 守卫：prev-value 按 Option 恢复（纪律要求）。调用方必须持
    /// crate::GLOBAL_STATE_LOCK；#[tokio::test] 不要求 Send，守卫可跨 await 存活
    /// （同文件 run_dispatch_list_toggle_and_remove_via_env_home 先例）。
    struct WaveBHomeGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl WaveBHomeGuard {
        fn set(root: &std::path::Path) -> Self {
            let prev = std::env::var_os("NEMESISBOT_HOME");
            unsafe { std::env::set_var("NEMESISBOT_HOME", root) };
            Self { prev }
        }
    }

    impl Drop for WaveBHomeGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => unsafe { std::env::set_var("NEMESISBOT_HOME", v) },
                None => unsafe { std::env::remove_var("NEMESISBOT_HOME") },
            }
        }
    }

    fn wave_b_seeded_home() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        let path = eval_assessor::rules_file_path(&home);
        eval_assessor::load_rules(&path).unwrap(); // 首次调用种子默认集
        let seeded: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let id0 = seeded["rules"][0]["id"].as_str().expect("默认集首条有 id").to_string();
        (tmp, path, id0)
    }

    /// 197-199：空规则集渲染 "(no rules)" 后返回（此前只有非空表格被间接触达）。
    #[test]
    fn wave_b_print_rule_table_empty_set_prints_no_rules() {
        // 用 parse_rules 构造，避免手拼 RulesFile 字段名
        let file = eval_assessor::parse_rules(r#"{"rules":[]}"#).unwrap();
        assert!(file.rules.is_empty());
        print_rule_table(&file);
    }

    /// tuple-98 + dispatch-142：New 子命令经 run() 分发。stdin EOF → 向导 kind
    /// 默认 path 分支 → 关键词读空 → bail「路径为空」，且绝不落盘（S11c 直调
    /// cmd_new 已钉行为；这里补的是 run() 这层分派桥）。
    #[tokio::test]
    async fn wave_b_run_new_wizard_eof_stdin_cancels() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        let _env = WaveBHomeGuard::set(tmp.path());

        let err = run(RulesAction::New { local: false }, false)
            .await
            .expect_err("EOF 取消 → 路径为空 bail");
        assert!(err.to_string().contains("路径为空"), "got: {err:#}");
        assert!(
            !eval_assessor::rules_file_path(&home).exists(),
            "取消路径绝不落盘"
        );
    }

    /// dispatch-144（Edit 臂经 run()）+ dispatch-143（Add 臂经 run()）：
    /// 全链路替换与追加都以落盘后的文件内容断言。
    #[tokio::test]
    async fn wave_b_run_edit_then_add_dispatch_persist_effects() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let (tmp, path, id0) = wave_b_seeded_home();
        let _env = WaveBHomeGuard::set(tmp.path());

        // ── Edit：文件 id 必须等于命令行 id（校验前提），描述改成可辨认标记 ──
        let edit_file = tmp.path().join("wave_b_edit.json");
        std::fs::write(
            &edit_file,
            format!(
                r#"{{"id":"{id0}","description":"wave-b edited","level":"low","source":"subject","conditions":[{{"field":"text","op":"contains","value":"x"}}]}}"#
            ),
        )
        .unwrap();
        run(
            RulesAction::Edit {
                id: id0.clone(),
                file: edit_file,
                local: false,
            },
            false,
        )
        .await
        .expect("run Edit ok");
        let after = eval_assessor::load_rules(&path).unwrap();
        assert_eq!(
            after.rules.iter().find(|r| r.id == id0).unwrap().description,
            "wave-b edited",
            "Edit 经分发替换生效"
        );

        // ── Add：追加新 id，默认集 +1 ──
        let add_file = tmp.path().join("wave_b_add.json");
        std::fs::write(&add_file, rule_json("wave-b-new")).unwrap();
        run(
            RulesAction::Add {
                file: add_file,
                local: false,
            },
            false,
        )
        .await
        .expect("run Add ok");
        let after = eval_assessor::load_rules(&path).unwrap();
        assert!(after.rules.iter().any(|r| r.id == "wave-b-new"), "Add 经分发生效");
        let defaults = eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON).unwrap();
        assert_eq!(after.rules.len(), defaults.rules.len() + 1);
    }

    /// tuple-104 + dispatch-148：Reset --force 经 run() 分发回默认集。
    #[tokio::test]
    async fn wave_b_run_reset_force_dispatch_restores_defaults() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let (tmp, path, _id0) = wave_b_seeded_home();
        let _env = WaveBHomeGuard::set(tmp.path());

        // 先混入自定义规则，让 reset 有东西可清
        let add_file = tmp.path().join("wave_b_custom.json");
        std::fs::write(&add_file, rule_json("wave-b-custom")).unwrap();
        run(
            RulesAction::Add {
                file: add_file,
                local: false,
            },
            false,
        )
        .await
        .expect("seed custom rule");

        run(
            RulesAction::Reset {
                force: true,
                local: false,
            },
            false,
        )
        .await
        .expect("run Reset ok");

        let after = eval_assessor::load_rules(&path).unwrap();
        assert!(!after.rules.iter().any(|r| r.id == "wave-b-custom"));
        let defaults = eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON).unwrap();
        assert_eq!(after.rules.len(), defaults.rules.len(), "回到默认集");
    }
}

// ===========================================================================
// r9（覆盖率补测批 2026-08-27）：eval_rules 向导/CLI 全管道（子进程级）。
//
// wave_b 曾把向导交互深分支判 EXEMPT（理由：进程内 stdin 是管道 EOF，喂不
// 进答案序列）。本组换武器：test_harness::run_cli_with_stdin 起**真二进制**
// 子进程、管道喂完整答案序列 —— 向导 kind=1/2/3 全流、预览确认 Y/n、id 冲突
// -2 后缀循环、reset 确认 y/n、list/show/enable/disable 各臂全部真实点亮。
//
// 夹具纪律：
//   * TestWorkspace 起手只有空 tempdir → BB4 会拦写命令，必须先 create_dir_all(home)；
//   * run_cli* 自动 prepend --local + cwd=tempdir → 每测试独立 home，互不污染；
//   * 父进程环境变量零改动（--local/cwd 已隔离一切）→ 无需 GLOBAL_STATE_LOCK，
//     与同文件 env-home 测试天然正交；
//   * 二进制经 resolve_nemesisbot_bin 解析 target/release/nemesisbot.exe，
//     未构建/非 Windows → 打印 SKIP early-return（跨平台测试文件不能硬 panic）；
//   * 每次子进程调用预算 30s（向导毫秒级结束，预算只防挂死）。
// ===========================================================================
mod r9_wizard_pipeline {
    use super::*;

    /// 解析真二进制；解析不了（未构建 / 非 Windows 平台）→ SKIP。
    fn r9_bin_or_skip() -> Option<std::path::PathBuf> {
        match test_harness::resolve_nemesisbot_bin() {
            Ok(b) => Some(b),
            Err(e) => {
                println!("[r9 SKIP] 未找到 nemesisbot 可执行文件（先构建 release 版）：{e:#}");
                None
            }
        }
    }

    /// 建 home 的隔离工作区 + 二进制；home 不建会被 BB4 写命令 bail。
    /// 二进制解析不了（未构建 / 非 Windows）→ None（调用方 SKIP 早退）。
    fn r9_ws() -> Option<(test_harness::TestWorkspace, std::path::PathBuf, std::path::PathBuf)> {
        let bin = r9_bin_or_skip()?;
        let tw = test_harness::TestWorkspace::new().expect("tempdir");
        let home = tw.home();
        std::fs::create_dir_all(&home).unwrap();
        Some((tw, bin, home))
    }

    /// 从落盘规则 JSON 里找 id 对应条目（JSON Value 直查，不依赖 serde 模型）。
    fn find_rule(home: &std::path::Path, id: &str) -> Option<serde_json::Value> {
        let rules: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(eval_assessor::rules_file_path(home)).ok()?)
                .ok()?;
        rules["rules"]
            .as_array()?
            .iter()
            .find(|r| r["id"] == *id)
            .cloned()
    }

    fn default_count() -> usize {
        eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON)
            .unwrap()
            .rules
            .len()
    }

    // ── B1：kind=3（subject 静态文本层）完整向导流 ────────────────────────
    #[tokio::test]
    async fn r9_wizard_kind3_creates_subject_rule_persisted() {
        let Some((tw, bin, home)) = r9_ws() else { return };
        // 答案序列：kind=3 → 关键词 secret.pem → 描述回车自动生成 → 确认 Y。
        let out = tw
            .run_cli_with_stdin(&bin, &["eval", "rules", "new"], "3\nsecret.pem\n\nY\n", 30)
            .await;
        assert!(
            out.success(),
            "向导全绿流必须退 0\n{}\n{}",
            out.stdout,
            out.stderr
        );
        assert!(out.stdout.contains("新建评估规则向导"), "进的是向导入口");
        assert!(
            out.stdout.contains("已保存") && out.stdout.contains("subject-secret-pem"),
            "保存回执带新 id，got:\n{}",
            out.stdout
        );
        let r = find_rule(&home, "subject-secret-pem").expect("subject 规则落盘");
        assert_eq!(r["level"], "high", "kind=3 固定 high");
        assert_eq!(r["source"], "subject");
        assert_eq!(r["conditions"][0]["field"], "text");
        assert_eq!(r["conditions"][0]["op"], "contains");
        assert_eq!(r["conditions"][0]["value"], "secret.pem", "用户输入按字面量保存");
        assert_eq!(r["enabled"], true);
    }

    // ── B2：kind=2（命令关键词层）+ ask_level 选 medium ───────────────────
    #[tokio::test]
    async fn r9_wizard_kind2_level_choice_maps_to_medium() {
        let Some((tw, bin, home)) = r9_ws() else { return };
        // 序列：kind=2 → 关键词 curl upload → level=3(medium) → 描述回车 → Y。
        let out = tw
            .run_cli_with_stdin(
                &bin,
                &["eval", "rules", "new"],
                "2\ncurl upload\n3\n\nY\n",
                30,
            )
            .await;
        assert!(out.success(), "{}\n{}", out.stdout, out.stderr);
        assert!(
            out.stdout.contains("cmd-curl-upload"),
            "cmd- 前缀 + kebab slug，got:\n{}",
            out.stdout
        );
        let r = find_rule(&home, "cmd-curl-upload").expect("cmd 规则落盘");
        assert_eq!(r["level"], "medium", "ask_level 第 3 项 = medium");
        assert_eq!(r["source"], "tool_trace");
        assert_eq!(r["conditions"][0]["field"], "arguments.command");
        assert_eq!(r["conditions"][0]["op"], "regex");
        // keyword_to_pattern：空格保持字面量 + (?i) 大小写前缀（slug 用连字符，
        // 匹配值保留原始空格——两者本就是不同用途）。
        assert_eq!(r["conditions"][0]["value"], "(?i)curl upload");
    }

    // ── B3：预览确认 n → 取消不写入（只剩 load_rules 在去重步骤种下的默认集）┐
    #[tokio::test]
    async fn r9_wizard_confirm_n_aborts_without_write() {
        let Some((tw, bin, home)) = r9_ws() else { return };
        let path = eval_assessor::rules_file_path(&home);
        let before_exists = path.exists();

        let out = tw
            .run_cli_with_stdin(&bin, &["eval", "rules", "new"], "3\nkeep-out\n\nn\n", 30)
            .await;
        assert!(out.success(), "取消是正常出口退 0，got:\n{}\n{}", out.stdout, out.stderr);
        assert!(
            out.stdout.contains("已取消，未写入"),
            "取消文案，got:\n{}",
            out.stdout
        );
        assert!(
            !before_exists && !out.stdout.contains("已保存"),
            "取消路径绝无保存回执"
        );
        // 注意时序事实：load_rules（去重步）先于确认种子默认集 → 文件存在但
        // 内容必须仍是纯默认集，向导规则绝不混入。
        assert!(path.exists(), "种子发生在确认前（既有行为），文件由默认集占位");
        let file = eval_assessor::load_rules(&path).unwrap();
        assert_eq!(file.rules.len(), default_count(), "取消后维持默认集");
        assert!(!file.rules.iter().any(|r| r.id == "subject-keep-out"));
    }

    // ── B5：同一 id 连开两次向导 → 第二次自动 -2 后缀 ─────────────────────
    #[tokio::test]
    async fn r9_wizard_same_id_second_run_gets_minus_two_suffix() {
        let Some((tw, bin, home)) = r9_ws() else { return };
        for i in 1..=2 {
            let out = tw
                .run_cli_with_stdin(
                    &bin,
                    &["eval", "rules", "new"],
                    "3\ndup-key\n\nY\n",
                    30,
                )
                .await;
            assert!(out.success(), "第 {i} 次向导失败:\n{}\n{}", out.stdout, out.stderr);
        }
        // 第二次的 stdout 必须点名 -2 后缀 id（打印行即去重结果）。
        let file = eval_assessor::load_rules(&eval_assessor::rules_file_path(&home)).unwrap();
        assert!(file.rules.iter().any(|r| r.id == "subject-dup-key"), "第一条原 id");
        assert!(file.rules.iter().any(|r| r.id == "subject-dup-key-2"), "第二条 -2 后缀");
        assert_eq!(
            file.rules.len(),
            default_count() + 2,
            "默认集 + 恰好两条新规则"
        );
    }

    // ── B4：CLI 面各一发——list / show / disable / enable 全链路状态翻转 ──
    #[tokio::test]
    async fn r9_cli_list_show_disable_enable_roundtrip_on_seeded_defaults() {
        let Some((tw, bin, home)) = r9_ws() else { return };
        let path = eval_assessor::rules_file_path(&home);

        let list = tw.run_cli_with_timeout(&bin, &["eval", "rules", "list"], 30).await;
        assert!(list.success(), "list: {}\n{}", list.stdout, list.stderr);
        assert!(list.stdout.contains("rule(s)"), "列表尾部统计行，got:\n{}", list.stdout);

        let show = tw
            .run_cli_with_timeout(&bin, &["eval", "rules", "show", "outbox-deny-ssh"], 30)
            .await;
        assert!(show.success(), "show 存在 id 退 0：\n{}\n{}", show.stdout, show.stderr);
        assert!(
            show.stdout.contains("outbox-deny-ssh"),
            "show 输出完整 JSON 定义"
        );

        let disable = tw
            .run_cli_with_timeout(&bin, &["eval", "rules", "disable", "outbox-deny-ssh"], 30)
            .await;
        assert!(disable.success(), "{}\n{}", disable.stdout, disable.stderr);
        assert!(
            !find_rule(&home, "outbox-deny-ssh").unwrap()["enabled"]
                .as_bool()
                .unwrap(),
            "disable 经 CLI 分发落到磁盘"
        );

        let enable = tw
            .run_cli_with_timeout(&bin, &["eval", "rules", "enable", "outbox-deny-ssh"], 30)
            .await;
        assert!(enable.success(), "{}\n{}", enable.stdout, enable.stderr);
        assert!(
            find_rule(&home, "outbox-deny-ssh").unwrap()["enabled"]
                .as_bool()
                .unwrap(),
            "enable 回写恢复"
        );
        assert!(path.exists());
    }

    // ── B6：reset 确认双答（n 拦停 / y 执行）────────────────────────────────
    #[tokio::test]
    async fn r9_cli_reset_confirm_n_blocks_then_y_restores_defaults() {
        let Some((tw, bin, home)) = r9_ws() else { return };
        let path = eval_assessor::rules_file_path(&home);

        // 种子（首次 list 落默认集）+ 混入一条自定义规则（fixture 直改 JSON）。
        tw.run_cli_with_timeout(&bin, &["eval", "rules", "list"], 30)
            .await;
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        v["rules"].as_array_mut().unwrap().push(serde_json::json!({
            "id": "r9-custom-x",
            "description": "d",
            "level": "low",
            "source": "subject",
            "conditions": [{"field": "text", "op": "contains", "value": "x"}],
            "min_count": 1
        }));
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        // 第一发：不带 --force + 答 n → Aborted，自定义规则原地不动。
        let no = tw
            .run_cli_with_stdin(&bin, &["eval", "rules", "reset"], "n\n", 30)
            .await;
        assert!(no.success(), "拦停是正常出口：\n{}\n{}", no.stdout, no.stderr);
        assert!(no.stdout.contains("Aborted"), "拦停文案，got:\n{}", no.stdout);
        assert!(find_rule(&home, "r9-custom-x").is_some(), "n 之后规则保持原样");

        // 第二发：答 y → 回到默认集，自定义消失。
        let yes = tw
            .run_cli_with_stdin(&bin, &["eval", "rules", "reset"], "y\n", 30)
            .await;
        assert!(yes.success(), "{}\n{}", yes.stdout, yes.stderr);
        assert!(
            yes.stdout.contains("reset ") && yes.stdout.contains("rule(s) from built-in defaults"),
            "reset 统计回执，got:\n{}",
            yes.stdout
        );
        assert!(find_rule(&home, "r9-custom-x").is_none(), "y 之后自定义被清");
        let file = eval_assessor::load_rules(&path).unwrap();
        assert_eq!(file.rules.len(), default_count(), "回到纯默认集");
    }
}

// ===========================================================================
// r10（覆盖率补测批 2026-08-27）：eval_rules 向导 A 类 miss 行收口。
// 目标行（生产 eval_rules.rs）：ask_choice 重试环 361-366、三种空关键词
// bail 臂 408/430/452-453、kind=1 完整向导流 455-486（OR 拆两条
// probe-<slug>/probe-<slug>-path）、ask_level 三臂 532(critical)/534(low)/
// 535(high/_ 兜底)。全部走 r9 同款子进程管道 stdin（交互深度分支的
// 唯一可达通道），夹具/纪律逐条沿用：--local + cwd=tempdir 每测试独立
// home、父进程环境零改动无需 GLOBAL_STATE_LOCK、二进制解析不了则 SKIP。
// ===========================================================================
mod r10_wizard_deep_branches {
    use super::*;

    /// 解析真二进制；解析不了（未构建 / 非 Windows 平台）→ SKIP。
    fn r10_bin_or_skip() -> Option<std::path::PathBuf> {
        match test_harness::resolve_nemesisbot_bin() {
            Ok(b) => Some(b),
            Err(e) => {
                println!("[r10 SKIP] 未找到 nemesisbot 可执行文件（先构建 release 版）：{e:#}");
                None
            }
        }
    }

    /// 建 home 的隔离工作区 + 二进制（home 不建会被 BB4 写命令拦停）。
    fn r10_ws() -> Option<(test_harness::TestWorkspace, std::path::PathBuf, std::path::PathBuf)> {
        let bin = r10_bin_or_skip()?;
        let tw = test_harness::TestWorkspace::new().expect("tempdir");
        let home = tw.home();
        std::fs::create_dir_all(&home).unwrap();
        Some((tw, bin, home))
    }

    fn r10_find_rule(home: &std::path::Path, id: &str) -> Option<serde_json::Value> {
        let rules: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(eval_assessor::rules_file_path(home)).ok()?)
                .ok()?;
        rules["rules"].as_array()?.iter().find(|r| r["id"] == *id).cloned()
    }

    fn r10_default_count() -> usize {
        eval_assessor::parse_rules(eval_assessor::DEFAULT_RULES_JSON)
            .unwrap()
            .rules
            .len()
    }

    // ── ⭐ kind=1（回车取默认 kind）完整向导流：ask_choice 重试环 + OR 拆分
    //    双规则块一次点亮。stdin 时序：
    //      "9"           → 选择越界 → 重试提示（361-366）
    //      ""            → 回车默认 kind=1
    //      "id_ed25519"  → 保护路径关键词（非空 → 推进到 455-486）
    //      "1"           → ask_level 选 critical（532 臂）
    //      ""            → 描述自动生成
    //      "Y"           → 确认保存
    #[tokio::test]
    async fn r10_wizard_kind1_default_enter_full_flow_creates_or_split_probe_pair() {
        let Some((tw, bin, home)) = r10_ws() else { return };
        let out = tw
            .run_cli_with_stdin(
                &bin,
                &["eval", "rules", "new"],
                "9\n\nid_ed25519\n1\n\nY\n",
                30,
            )
            .await;
        assert!(
            out.success(),
            "kind=1 全绿流必须退 0\n{}\n{}",
            out.stdout,
            out.stderr
        );
        assert!(
            out.stdout.contains("请输入 1-3"),
            "越界选项触发重试提示，got:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("probe-id-ed25519") && out.stdout.contains("probe-id-ed25519-path"),
            "预览点名 OR 拆分后的两条 id，got:\n{}",
            out.stdout
        );
        assert!(
            out.stdout.contains("已保存 2 条规则"),
            "保存回执计数=2，got:\n{}",
            out.stdout
        );

        let base = r10_find_rule(&home, "probe-id-ed25519").expect("command 条规则落盘");
        assert_eq!(base["level"], "critical", "ask_level 第 1 项 = critical");
        assert_eq!(base["source"], "tool_trace");
        assert_eq!(base["conditions"][0]["field"], "arguments.command");
        assert_eq!(base["conditions"][0]["op"], "regex");
        assert_eq!(base["conditions"][0]["value"], "(?i)id_ed25519");

        let path_rule = r10_find_rule(&home, "probe-id-ed25519-path").expect("path 条规则落盘");
        assert_eq!(path_rule["level"], "critical", "拆分件继承级别");
        assert_eq!(path_rule["conditions"][0]["field"], "arguments.path");
        assert_eq!(path_rule["conditions"][0]["value"], "(?i)id_ed25519");

        let file = eval_assessor::load_rules(&eval_assessor::rules_file_path(&home)).unwrap();
        assert_eq!(
            file.rules.len(),
            r10_default_count() + 2,
            "默认集 + 恰好一对拆分规则"
        );
    }

    // ── ask_level 越界后合法值映射 high（match `_` 兜底臂 535），顺带复跑
    //    重试环于 level 菜单；关键词带 Windows 反斜杠验证斜杠等价正则。
    #[tokio::test]
    async fn r10_wizard_level_out_of_range_then_two_maps_high_arm() {
        let Some((tw, bin, home)) = r10_ws() else { return };
        // 序列：kind=1 直接合法 → D:\secret → level 越界 9 → 重试后选 2(high) → 描述空 → Y。
        let out = tw
            .run_cli_with_stdin(
                &bin,
                &["eval", "rules", "new"],
                "1\nD:\\secret\n9\n2\n\nY\n",
                30,
            )
            .await;
        assert!(
            out.success(),
            "{}\n{}",
            out.stdout,
            out.stderr
        );
        assert!(
            out.stdout.contains("请输入 1-4"),
            "level 菜单同样有越界重试提示，got:\n{}",
            out.stdout
        );
        let r = r10_find_rule(&home, "probe-d-secret").expect("反斜杠关键词 slug 化落盘");
        assert_eq!(r["level"], "high", "ask_level 合法项 2 = high（`_` 兜底臂）");
        assert_eq!(
            r["conditions"][0]["value"],
            // 字母保持原样（大小写不敏感由 (?i) 前缀承担），只转义分隔符
            r"(?i)D:[\\/]secret",
            "路径分隔符兼容正则按 keyword_to_pattern 生成"
        );
        assert!(r10_find_rule(&home, "probe-d-secret-path").is_some(), "OR 拆分第二件在盘");
    }

    // ── ask_level 第 4 项 = low 弱信号档（534 臂；此前 r9 只喂过 medium）。──
    #[tokio::test]
    async fn r10_wizard_kind2_level_four_maps_low_arm() {
        let Some((tw, bin, home)) = r10_ws() else { return };
        let out = tw
            .run_cli_with_stdin(&bin, &["eval", "rules", "new"], "2\ncurl upload\n4\n\nY\n", 30)
            .await;
        assert!(out.success(), "{}\n{}", out.stdout, out.stderr);
        let r = r10_find_rule(&home, "cmd-curl-upload").expect("cmd 规则落盘");
        assert_eq!(r["level"], "low", "ask_level 第 4 项 = low");
        assert_eq!(r["source"], "tool_trace");
        assert_eq!(r["conditions"][0]["value"], "(?i)curl upload");
    }

    // ── 三种空关键词 bail 臂（bail 结束会话 → 各自独立子进程一发）：
    //    文件必须在 bail 处**未种子化**（load_rules 在确认步之后才首次触达）。
    // ── kind=3（提示词文本层）关键词回车为空 → 「关键词为空」（408）。
    #[tokio::test]
    async fn r10_wizard_kind3_empty_keyword_bails_without_seeding_file() {
        let Some((tw, _bin, home)) = r10_ws() else { return };
        let out = tw
            .run_cli_with_stdin(_bin.as_path(), &["eval", "rules", "new"], "3\n\n", 30)
            .await;
        assert!(!out.success(), "bail 必须非零退码:\n{}\n{}", out.stdout, out.stderr);
        assert!(
            out.stderr.contains("关键词为空"),
            "错误信息落到 stderr，got:\n{}\n{}",
            out.stdout,
            out.stderr
        );
        assert!(
            !eval_assessor::rules_file_path(&home).exists(),
            "bail 先于种子化，规则文件绝不出现"
        );
    }

    // ── kind=2（命令层）关键词回车为空 → 同型 bail（430）。─────────────────
    #[tokio::test]
    async fn r10_wizard_kind2_empty_keyword_bails_without_seeding_file() {
        let Some((tw, _bin, home)) = r10_ws() else { return };
        let out = tw
            .run_cli_with_stdin(_bin.as_path(), &["eval", "rules", "new"], "2\n\n", 30)
            .await;
        assert!(!out.success(), "bail 必须非零退码:\n{}\n{}", out.stdout, out.stderr);
        assert!(
            out.stderr.contains("关键词为空"),
            "got:\n{}\n{}",
            out.stdout,
            out.stderr
        );
        assert!(!eval_assessor::rules_file_path(&home).exists());
    }

    // ── kind=1（回车默认进入路径层）路径回车为空 → 「路径为空」bail
    //    （452-453 区域；进程内 EOF 版已钉行为，这里补 CLI 层同断言）。─────
    #[tokio::test]
    async fn r10_wizard_path_empty_keyword_bails_without_seeding_file() {
        let Some((tw, _bin, home)) = r10_ws() else { return };
        let out = tw
            .run_cli_with_stdin(_bin.as_path(), &["eval", "rules", "new"], "\n\n", 30)
            .await;
        assert!(!out.success(), "bail 必须非零退码:\n{}\n{}", out.stdout, out.stderr);
        assert!(
            out.stderr.contains("路径为空"),
            "got:\n{}\n{}",
            out.stdout,
            out.stderr
        );
        assert!(!eval_assessor::rules_file_path(&home).exists());
    }
}
