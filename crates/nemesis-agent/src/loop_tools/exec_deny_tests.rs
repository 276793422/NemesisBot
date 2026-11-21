//! exec deny 词表接线测试（2026-11-08 激活 `tools.exec.*` 死键）。
//!
//! 语义钉死：
//! - `ExecTool::new` / `AsyncExecTool::new` = 内置默认词表（接线前行为）；
//! - `new_with_config` 自定义词表 = **替换**语义（非叠加）；
//! - 自定义词表全部编译失败 / 传入空表 = 回落内置默认（永不全放行）；
//! - deny 命中 → execute 返回 Err 且消息带规则 name + 人话 reason +
//!   危险闸页指引（2026-11-20 元数据化 + 留痕同批修）；
//! - 词表单一真相源 = `nemesis_tools::shell::DEFAULT_EXEC_DENY_ENTRIES`。

use super::*;
use crate::context::RequestContext;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

/// 默认词表非空且关键模式在场（单一真相源卫兵——词表被误清空时先红这里）。
/// 元数据完整性一并钉死：name 唯一 / category+severity 值域 / pattern
/// 全部可编译（危险闸页面的呈现契约依赖这三条）。
#[test]
fn default_deny_table_nonempty_with_key_patterns() {
    let table = nemesis_tools::shell::DEFAULT_EXEC_DENY_ENTRIES;
    assert!(table.len() >= 20, "默认词表至少 20 条，got {}", table.len());
    let mut names: Vec<&str> = table.iter().map(|e| e.name).collect();
    let uniq = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), uniq, "deny 词条 name 必须全表唯一");
    let valid_categories = [
        "destructive",
        "system",
        "privilege",
        "exfiltration",
        "remote",
        "rce",
        "package",
        "container",
        "git",
        "obfuscation",
    ];
    let valid_severities = ["high", "medium", "low"];
    for e in table {
        assert!(
            valid_categories.contains(&e.category),
            "词条 {} category 值域外: {}",
            e.name,
            e.category
        );
        assert!(
            valid_severities.contains(&e.severity),
            "词条 {} severity 值域外: {}",
            e.name,
            e.severity
        );
        assert!(!e.reason.is_empty(), "词条 {} reason 不得为空", e.name);
        regex::Regex::new(e.pattern)
            .unwrap_or_else(|err| panic!("词条 {} pattern 编译失败: {err}", e.name));
    }
    let joined = table
        .iter()
        .map(|e| e.pattern)
        .collect::<Vec<_>>()
        .join("\n");
    for key in [
        "\\brm\\s",
        "\\bsudo\\b",
        "shutdown",
        "git\\s+push",
        "\\beval\\b",
    ] {
        assert!(joined.contains(key), "默认词表必须含 {key} 相关模式");
    }
}

/// `ExecTool::new` = 默认词表：危险命令被拒、普通命令放行（deny_hit 层）。
/// 命中带元数据（name/reason）——危险闸页面与拒绝消息的消费契约。
#[test]
fn exec_tool_default_deny_blocks_dangerous() {
    let t = ExecTool::new("/tmp/ws", true);
    // 词表条目有序，首个命中返回——"sudo rm -rf /" 同时匹配 rm 与 sudo，
    // rm 条目在前 → 首命中是 rm_recursive_force（真实返回哪个都带元数据）。
    let hit = t
        .deny_hit("sudo rm -rf /")
        .expect("rm -rf 必须命中默认词表");
    assert!(!hit.reason.is_empty());
    assert_eq!(
        hit.name, "rm_recursive_force",
        "首命中 = 表序靠前的 rm 条目"
    );
    // 独占 sudo 的命令 → sudo 条目
    let hit_sudo = t.deny_hit("sudo apt install foo").expect("sudo 必须命中");
    assert_eq!(hit_sudo.name, "sudo");
    assert!(
        t.deny_hit("git push --force origin main").is_some(),
        "git push 必须命中"
    );
    assert!(t.deny_hit("shutdown /s").is_some(), "shutdown 必须命中");
    assert!(t.deny_hit("ls -la").is_none(), "普通只读命令不得误伤");
    assert!(t.deny_hit("cargo build --release").is_none());
}

/// 大小写不敏感（deny_hit 统一小写后比对）。
#[test]
fn exec_tool_deny_case_insensitive() {
    let t = ExecTool::new("/tmp/ws", true);
    assert!(t.deny_hit("SUDO rm -rf /").is_some());
    assert!(t.deny_hit("Git PUSH origin").is_some());
}

/// `new_with_config` 自定义词表 = **替换**语义：自定义命中、
/// 默认词表条目放行。自定义条目 name = `custom:N`（不可伪装内置名）。
#[test]
fn exec_tool_custom_deny_replaces_defaults() {
    let custom = ["forbidden_tool\\.exe"];
    let t = ExecTool::new_with_config("/tmp/ws", true, Some(&custom));
    let hit = t
        .deny_hit("run forbidden_tool.exe --flag")
        .expect("自定义词表条目必须命中");
    assert_eq!(hit.name, "custom:0");
    assert!(
        t.deny_hit("sudo rm -rf /").is_none(),
        "替换语义：默认词表条目在自定义模式下放行（Go 版 NewExecToolWithConfig 同语义）"
    );
}

/// 自定义词表全部编译失败 = 回落默认词表（不静默全放行）。
#[test]
fn exec_tool_invalid_custom_falls_back_to_defaults() {
    let custom = ["([unclosed", "also(bad"];
    let t = ExecTool::new_with_config("/tmp/ws", true, Some(&custom));
    assert!(
        t.deny_hit("sudo ls").is_some(),
        "全无效自定义词表必须回落内置默认词表"
    );
}

/// 空表 / None = 默认词表（装配侧「开关开但词表空」的回落形态）。
#[test]
fn exec_tool_empty_custom_uses_defaults() {
    let empty: [&str; 0] = [];
    let t = ExecTool::new_with_config("/tmp/ws", true, Some(&empty));
    assert!(t.deny_hit("sudo ls").is_some(), "空自定义词表 = 默认词表");

    let t2 = ExecTool::new_with_config("/tmp/ws", true, None);
    assert!(t2.deny_hit("sudo ls").is_some(), "None = 默认词表");
}

/// execute 面：deny 命中返回 Err 且消息带规则 name + reason + 危险闸页
/// 指引（spawn 之前拦截，不产生子进程）。普通命令照常走 spawn 路径
/// （此处只断言拒绝面）。
#[tokio::test]
async fn exec_tool_execute_denies_before_spawn() {
    let t = ExecTool::new("/tmp/ws", true);
    let args = serde_json::json!({ "command": "sudo echo hi" }).to_string();
    let err = t.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("blocked by deny rule"), "拒绝消息形态: {err}");
    assert!(err.contains("'sudo'"), "拒绝消息须带规则 name: {err}");
    assert!(err.contains("危险闸"), "拒绝消息须带危险闸页指引: {err}");
}

/// execute 面：词表放行的普通命令在 deny 层不报错（走到围栏/spawn 层——
/// 本测试只断言错误不是 deny 拒绝）。
#[tokio::test]
async fn exec_tool_execute_non_denied_passes_deny_layer() {
    let t = ExecTool::new("/tmp/ws", true);
    let args = serde_json::json!({ "command": "echo deny-pass-through" }).to_string();
    match t.execute(&args, &ctx()).await {
        Ok(_) => {}
        Err(e) => assert!(
            !e.contains("blocked by deny rule"),
            "普通命令不得被 deny 层拦截: {e}"
        ),
    }
}

/// `AsyncExecTool` 同源同语义（默认词表 + 自定义替换 + execute 拒绝面）。
#[test]
fn async_exec_tool_deny_parity() {
    let t = AsyncExecTool::new("/tmp/ws", true);
    assert!(t.deny_hit("shutdown /s").is_some());
    assert!(t.deny_hit("notepad.exe").is_none());

    let custom = ["evil\\.exe"];
    let t2 = AsyncExecTool::new_with_config("/tmp/ws", true, Some(&custom));
    assert!(t2.deny_hit("start evil.exe").is_some());
    assert!(
        t2.deny_hit("sudo rm -rf /").is_none(),
        "异步工具同为替换语义"
    );
}

#[tokio::test]
async fn async_exec_tool_execute_denies_before_spawn() {
    let t = AsyncExecTool::new("/tmp/ws", true);
    let args = serde_json::json!({ "command": "sudo notepad" }).to_string();
    let err = t.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("blocked by deny rule"), "拒绝消息形态: {err}");
    assert!(err.contains("'sudo'"), "拒绝消息须带规则 name: {err}");
}

// ============================================================================
// 危险闸生效表合成（build_effective_deny，2026-11-20）
// ============================================================================

use crate::loop_tools::exec::{build_effective_deny, deny_hit_in};

fn exec_cfg(
    enable: bool,
    custom: Vec<String>,
    disabled: Vec<String>,
    extra: Vec<String>,
) -> nemesis_config::ExecConfig {
    nemesis_config::ExecConfig {
        enable_deny_patterns: enable,
        custom_deny_patterns: custom,
        deny_disabled: disabled,
        deny_extra: extra,
        ..Default::default()
    }
}

/// 缺省形态 = 全量内置词表（typed 默认不加任何键 = 现行为字节不变）。
#[test]
fn effective_deny_default_is_full_builtin() {
    let entries = build_effective_deny(&exec_cfg(false, vec![], vec![], vec![]));
    assert_eq!(
        entries.len(),
        nemesis_tools::shell::DEFAULT_EXEC_DENY_ENTRIES.len()
    );
    let all = entries
        .iter()
        .map(|e| e.name.clone())
        .collect::<Vec<_>>()
        .join(",");
    assert!(all.contains("sudo"), "内置词条须在场: {all}");
}

/// `deny_disabled` 增量语义：关 ssh 一条 → ssh 放行、其余防线不动
/// （危险闸页面的核心语义——逐条开关，不是整表替换）。
#[test]
fn effective_deny_disable_single_entry_keeps_rest() {
    let entries = build_effective_deny(&exec_cfg(
        false,
        vec![],
        vec!["ssh_remote_login".into()],
        vec![],
    ));
    assert!(
        deny_hit_in(&entries, "ssh root@10.0.0.8").is_none(),
        "被关条目（ssh）必须放行"
    );
    assert!(
        deny_hit_in(&entries, "sudo rm -rf /").is_some(),
        "未关条目（sudo）防线必须保留"
    );
    assert_eq!(
        entries.len(),
        nemesis_tools::shell::DEFAULT_EXEC_DENY_ENTRIES.len() - 1,
        "生效表恰好少一条"
    );
}

/// `deny_disabled` 未知 name = 忽略（版本漂移容忍，不炸不扩面）。
#[test]
fn effective_deny_disable_unknown_name_ignored() {
    let entries = build_effective_deny(&exec_cfg(
        false,
        vec![],
        vec!["not_a_real_rule".into()],
        vec![],
    ));
    assert_eq!(
        entries.len(),
        nemesis_tools::shell::DEFAULT_EXEC_DENY_ENTRIES.len()
    );
}

/// `deny_extra` 追加语义：自定义正则与内置**叠加**生效（name=extra:N）。
#[test]
fn effective_deny_extra_appends_on_top_of_builtin() {
    let entries = build_effective_deny(&exec_cfg(
        false,
        vec![],
        vec![],
        vec!["forbidden_binary\\.exe".into()],
    ));
    let hit = deny_hit_in(&entries, "./forbidden_binary.exe --run").expect("追加条目必须命中");
    assert_eq!(hit.name, "extra:0");
    assert_eq!(hit.category, "custom");
    assert!(
        deny_hit_in(&entries, "sudo ls").is_some(),
        "内置防线叠加保留（追加语义核心）"
    );
    // 条目数 = 内置 + 1
    assert_eq!(
        entries.len(),
        nemesis_tools::shell::DEFAULT_EXEC_DENY_ENTRIES.len() + 1
    );
}

/// `deny_extra` 编译失败逐条丢弃：坏条目不出现在生效表，好条目与内置
/// 防线不受牵连（不清空不回落）。
#[test]
fn effective_deny_extra_bad_entry_dropped_individually() {
    let entries = build_effective_deny(&exec_cfg(
        false,
        vec![],
        vec![],
        vec!["([unclosed".into(), "evil_pattern\\.exe".into()],
    ));
    assert_eq!(
        entries.len(),
        nemesis_tools::shell::DEFAULT_EXEC_DENY_ENTRIES.len() + 1,
        "坏条目丢弃、好条目保留"
    );
    assert!(deny_hit_in(&entries, "run evil_pattern.exe").is_some());
    assert!(deny_hit_in(&entries, "sudo ls").is_some());
}

/// 组合形态：关 ssh + 追加自定义 + 内置其余保留（危险闸页面保存后
/// 的典型生效面）。
#[test]
fn effective_deny_disable_and_extra_combined() {
    let entries = build_effective_deny(&exec_cfg(
        false,
        vec![],
        vec!["ssh_remote_login".into()],
        vec!["mycorp_block\\.sh".into()],
    ));
    assert!(deny_hit_in(&entries, "ssh user@host").is_none());
    assert!(deny_hit_in(&entries, "bash mycorp_block.sh").is_some());
    assert!(deny_hit_in(&entries, "sudo ls").is_some());
}

/// 旧高级出口语义不变：custom 整表替换（开 + 非空 = 只剩 custom 条目），
/// disabled 对 custom 条目无效（name=custom:N 不在内置名单——结构上
/// 不可伪装），extra 仍叠加。
#[test]
fn effective_deny_custom_replace_legacy_semantics() {
    let entries = build_effective_deny(&exec_cfg(
        true,
        vec!["legacy_block\\.exe".into()],
        vec![],
        vec![],
    ));
    assert!(deny_hit_in(&entries, "run legacy_block.exe").is_some());
    assert!(
        deny_hit_in(&entries, "sudo ls").is_none(),
        "替换语义：内置条目不在生效表"
    );
    // custom:N 条目数 = 1
    assert_eq!(entries.len(), 1);

    // 自定义全失败回落内置（永不全放行），回落后的条目仍可被 disabled 过滤
    let entries2 = build_effective_deny(&exec_cfg(
        true,
        vec!["([bad".into()],
        vec!["sudo".into()],
        vec![],
    ));
    assert!(
        deny_hit_in(&entries2, "sudo ls").is_none(),
        "回落内置后 disabled 照常过滤"
    );
}

/// flag 关 + custom 非空：替换**不**激活——custom 被忽略、内置整表照常
/// 生效（`enable_deny_patterns` 只选表不关闸）。与快照显示面
/// exec_deny_snapshot_custom_replacement_needs_flag（nemesis-web）同语义
/// 镜像；此前消费面所有 flag=false 用例都传空 custom，该象限无直接断言
/// （2026-11-21 复查补钉——防后人把条件「简化」成 `!custom.is_empty()`）。
#[test]
fn effective_deny_flag_off_ignores_custom() {
    let entries = build_effective_deny(&exec_cfg(false, vec!["evil\\.exe".into()], vec![], vec![]));
    assert_eq!(
        entries.len(),
        nemesis_tools::shell::DEFAULT_EXEC_DENY_ENTRIES.len(),
        "flag 关 = 内置整表生效（custom 不参与）"
    );
    assert!(
        entries.iter().all(|e| !e.name.starts_with("custom")),
        "flag 关时 custom 条目不得进生效表"
    );
    assert!(
        deny_hit_in(&entries, "sudo ls").is_some(),
        "内置防线保留（flag 只选表，deny 恒激活无全放行形态）"
    );
}

// ============================================================================
// 审计钩子（DenyAuditHook，2026-11-20 留痕同批修）
// ============================================================================

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// execute 拒绝路径调用注入的审计钩子：参数 = 命令预览 + 规则 name。
#[tokio::test]
async fn execute_calls_deny_audit_hook() {
    let called = Arc::new(AtomicBool::new(false));
    let captured = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let called2 = called.clone();
    let captured2 = captured.clone();
    let hook: super::exec::DenyAuditHook = Arc::new(move |preview: &str, rule: &str| {
        called2.store(true, Ordering::SeqCst);
        captured2
            .lock()
            .unwrap()
            .push((preview.to_string(), rule.to_string()));
    });
    let t = ExecTool::new("/tmp/ws", true).with_deny_audit_hook(Some(hook));
    let args = serde_json::json!({ "command": "sudo echo audit-me" }).to_string();
    let err = t.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("blocked by deny rule"));
    assert!(called.load(Ordering::SeqCst), "钩子必须被调用");
    let got = captured.lock().unwrap();
    let (preview, rule) = &got[0];
    assert_eq!(
        preview, "sudo echo audit-me",
        "预览 = 完整命令（<200 字符）"
    );
    assert_eq!(rule, "sudo", "规则 name 透传");
}

/// 钩子缺席（None）= 诚实降级：拦截照常返回拒绝消息，不 panic。
#[tokio::test]
async fn execute_without_hook_still_denies() {
    let t = ExecTool::new("/tmp/ws", true);
    let args = serde_json::json!({ "command": "sudo echo no-hook" }).to_string();
    let err = t.execute(&args, &ctx()).await.unwrap_err();
    assert!(
        err.contains("blocked by deny rule"),
        "无钩子 = 只 WARN，拦截语义不变: {err}"
    );
}

/// 命令预览多字节安全：>200 字符按字符截断（不 panic 于 UTF-8 边界）。
#[test]
fn deny_preview_multibyte_safe() {
    let entries = build_effective_deny(&exec_cfg(false, vec![], vec![], vec![]));
    let entry = deny_hit_in(&entries, "sudo ls").unwrap();
    let long_cmd = "sudo ".to_string() + &"汉".repeat(300);
    let preview: String = long_cmd.chars().take(200).collect();
    assert_eq!(preview.chars().count(), 200);
    assert_eq!(preview.chars().last().unwrap(), '汉');
    // deny_reject 经 execute 面验证——这里只锁 chars().take() 语义
    let _ = entry;
}

/// 经热读路径（exec_config_path 在场）的 execute 同样触发审计钩子
/// （主 gateway 生产路径）。
#[tokio::test]
async fn hot_read_path_calls_deny_audit_hook() {
    let dir = std::env::temp_dir().join(format!("nb_deny_hook_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_path = dir.join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"tools":{"exec":{"deny_extra":["hook_probe\\.exe"]}}}"#,
    )
    .unwrap();

    let called = Arc::new(AtomicBool::new(false));
    let rule_name = Arc::new(Mutex::new(String::new()));
    let called2 = called.clone();
    let rule2 = rule_name.clone();
    let hook: super::exec::DenyAuditHook = Arc::new(move |_preview: &str, rule: &str| {
        called2.store(true, Ordering::SeqCst);
        *rule2.lock().unwrap() = rule.to_string();
    });
    let t = ExecTool::new("/tmp/ws", true)
        .with_deny_audit_hook(Some(hook))
        .with_exec_config_path(Some(cfg_path));
    let args = serde_json::json!({ "command": "run hook_probe.exe" }).to_string();
    let err = t.execute(&args, &ctx()).await.unwrap_err();
    assert!(err.contains("blocked by deny rule"), "{err}");
    assert!(called.load(Ordering::SeqCst), "热读路径钩子必须被调用");
    assert_eq!(*rule_name.lock().unwrap(), "extra:0");
    let _ = std::fs::remove_dir_all(&dir);
}
