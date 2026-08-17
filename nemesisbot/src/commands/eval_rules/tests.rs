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
