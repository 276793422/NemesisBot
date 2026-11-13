//! ExpertsStore 单元测试（专家子代理资源层）。
//!
//! slug 约定：`create` 的 slug = `sanitize_path_segment(名字)`（保留大小写；
//! 纯中文名无 ASCII 可辨 → `expert-<millis>` 时间戳形态）。测试用 ASCII 名
//! 拿确定性 slug；中文名退化路径独立断言。

use super::experts_store::{ExpertUpsert, ExpertsStore};
use std::sync::Arc;
use tempfile::TempDir;

fn store() -> (TempDir, ExpertsStore) {
    let tmp = tempfile::TempDir::new().unwrap();
    let s = ExpertsStore::new(tmp.path());
    (tmp, s)
}

fn up(name: &str, prompt: &str) -> ExpertUpsert {
    ExpertUpsert {
        name: Some(name.to_string()),
        prompt: Some(prompt.to_string()),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// create → list → get 往返（frontmatter 落盘 + 解析回来字段齐）
// ---------------------------------------------------------------------------

#[test]
fn create_roundtrip_preserves_all_fields() {
    let (_t, s) = store();
    let created = s
        .create(&ExpertUpsert {
            name: Some("code-reviewer".to_string()),
            summary: Some("review diffs".to_string()),
            tags: Some(vec!["review".into(), "rust".into()]),
            model: Some("gpt-6.1-sol".to_string()),
            enabled: Some(true),
            prompt: Some("You are a code review expert.".to_string()),
        })
        .unwrap();
    assert_eq!(created.slug, "code-reviewer");
    assert_eq!(created.source, "manual");

    let all = s.list();
    assert_eq!(all.len(), 1);
    let e = s.get("code-reviewer").unwrap();
    assert_eq!(e.name, "code-reviewer");
    assert_eq!(e.summary, "review diffs");
    assert_eq!(e.tags, vec!["review", "rust"]);
    assert_eq!(e.model, "gpt-6.1-sol");
    assert!(e.enabled);
    assert_eq!(e.prompt, "You are a code review expert.");
}

// ---------------------------------------------------------------------------
// lookup：slug / 名字 / 大小写不敏感 / 未知回灌在册名单
// ---------------------------------------------------------------------------

#[test]
fn lookup_by_slug_and_name_case_insensitive() {
    let (_t, s) = store();
    s.create(&up("Reviewer", "p1")).unwrap();
    assert_eq!(s.lookup("reviewer").unwrap().name, "Reviewer");
    assert_eq!(s.lookup("REVIEWER").unwrap().slug, "Reviewer");
    assert_eq!(s.lookup("  Reviewer  ").unwrap().slug, "Reviewer");
}

#[test]
fn lookup_unknown_carries_roster_for_self_correction() {
    let (_t, s) = store();
    s.create(&up("Alpha", "p")).unwrap();
    s.create(&up("Beta", "p")).unwrap();
    let err = s.lookup("nope").unwrap_err();
    assert!(err.contains("Unknown expert 'nope'"), "got: {err}");
    assert!(err.contains("Alpha"), "roster must list slugs, got: {err}");
    assert!(err.contains("Beta"), "roster must list slugs, got: {err}");
    assert!(
        err.contains("action"),
        "must hint the list action, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// dispatch：happy path / 停用闸 / 空提示词闸
// ---------------------------------------------------------------------------

#[test]
fn dispatch_returns_prompt_and_model() {
    let (_t, s) = store();
    s.create(&ExpertUpsert {
        name: Some("Writer".to_string()),
        model: Some("glm-5.3-flash".to_string()),
        prompt: Some("You write docs.".to_string()),
        ..Default::default()
    })
    .unwrap();
    let d = s.dispatch("writer").unwrap();
    assert_eq!(d.slug, "Writer");
    assert_eq!(d.name, "Writer");
    assert_eq!(d.prompt, "You write docs.");
    assert_eq!(d.model, "glm-5.3-flash");
}

#[test]
fn dispatch_rejects_disabled_and_empty_prompt() {
    let (_t, s) = store();
    s.create(&up("Gate", "p")).unwrap();
    s.set_enabled("gate", false).unwrap();
    let err = s.dispatch("gate").unwrap_err();
    assert!(err.contains("disabled"), "got: {err}");

    s.set_enabled("gate", true).unwrap();
    s.update(
        "gate",
        &ExpertUpsert {
            prompt: Some("   ".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    let err = s.dispatch("gate").unwrap_err();
    assert!(err.contains("empty prompt"), "got: {err}");
}

// ---------------------------------------------------------------------------
// update 部分更新：改名只动 frontmatter（slug 稳定）、其余字段不动
// ---------------------------------------------------------------------------

#[test]
fn update_renames_without_changing_slug() {
    let (_t, s) = store();
    s.create(&ExpertUpsert {
        name: Some("Old".to_string()),
        summary: Some("keep me".to_string()),
        prompt: Some("p".to_string()),
        ..Default::default()
    })
    .unwrap();
    let upd = s
        .update(
            "old",
            &ExpertUpsert {
                name: Some("New".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(upd.slug, "Old", "目录名（slug）稳定");
    assert_eq!(upd.name, "New");
    assert_eq!(upd.summary, "keep me");

    // 旧 slug 仍可读（目录没动）；新名可查；旧名经 slug 大小写不敏感
    // 匹配仍命中（文档化语义：slug 稳定，模型侧旧引用不失效）。
    assert_eq!(s.get("old").unwrap().name, "New");
    assert_eq!(s.lookup("new").unwrap().slug, "Old");
    assert_eq!(s.lookup("old").unwrap().slug, "Old");
}

// ---------------------------------------------------------------------------
// remove + 保留名守卫
// ---------------------------------------------------------------------------

#[test]
fn remove_then_lookup_fails() {
    let (_t, s) = store();
    s.create(&up("Gone", "p")).unwrap();
    s.remove("gone").unwrap();
    assert!(s.list().is_empty());
    assert!(s.lookup("gone").is_err());
    assert!(s.get("gone").is_err());
}

#[test]
fn slug_guard_rejects_reserved_and_path_shapes() {
    let (_t, s) = store();
    assert!(s.get("").is_err());
    assert!(s.get("_default").is_err(), "_default 是保留名");
    assert!(s.get("_default.meta.json").is_err());
    assert!(s.get("..").is_err());
    assert!(s.get("a/b").is_err());
    assert!(s.get("a\\b").is_err());
}

// ---------------------------------------------------------------------------
// hire（超市雇佣形态）：source=shop + hired_from + 幂等重雇
// ---------------------------------------------------------------------------

#[test]
fn hire_writes_shop_source_and_rehire_overwrites() {
    let (_t, s) = store();
    s.hire(
        "code-master",
        "代码大师",
        "writes code",
        vec!["dev".into()],
        "You code.".into(),
        "persona-42",
    )
    .unwrap();
    let e = s.get("code-master").unwrap();
    assert_eq!(e.source, "shop");
    assert_eq!(e.hired_from, "persona-42");
    assert_eq!(e.model, "", "雇佣初始无模型覆盖");
    assert!(e.enabled);

    s.hire(
        "code-master",
        "代码大师",
        "v2",
        vec![],
        "You code better.".into(),
        "persona-42",
    )
    .unwrap();
    let e2 = s.get("code-master").unwrap();
    assert_eq!(e2.summary, "v2");
    assert_eq!(e2.prompt, "You code better.");
    assert_eq!(s.list().len(), 1, "重雇不产生第二个目录");
}

#[test]
fn hire_rejects_empty_prompt() {
    let (_t, s) = store();
    assert!(s.hire("x", "X", "", vec![], "  ".into(), "p1").is_err());
}

// ---------------------------------------------------------------------------
// 默认子代理元数据：回落常量 / 落盘往返 / 损坏不炸
// ---------------------------------------------------------------------------

#[test]
fn default_name_fallback_and_roundtrip() {
    let (_t, s) = store();
    assert_eq!(s.default_name(), ExpertsStore::FALLBACK_DEFAULT_NAME);
    s.set_default_name("总管").unwrap();
    assert_eq!(s.default_name(), "总管");
}

#[test]
fn default_name_corrupt_file_falls_back() {
    let (t, s) = store();
    std::fs::create_dir_all(t.path().join("experts")).unwrap();
    std::fs::write(
        t.path().join("experts").join("_default.meta.json"),
        "{not json",
    )
    .unwrap();
    assert_eq!(s.default_name(), ExpertsStore::FALLBACK_DEFAULT_NAME);
}

// ---------------------------------------------------------------------------
// 中文名 slug 退化：无 ASCII 可辨名 → expert-<millis> 时间戳形态
// ---------------------------------------------------------------------------

#[test]
fn create_chinese_name_gets_timestamp_slug() {
    let (_t, s) = store();
    let e = s.create(&up("紫微斗数大师", "p")).unwrap();
    assert!(
        e.slug.starts_with("expert-"),
        "中文无 ASCII 应退化为 expert-<millis>，got: {}",
        e.slug
    );
    assert!(s.get(&e.slug).is_ok());
    assert_eq!(s.lookup("紫微斗数大师").unwrap().slug, e.slug);
}

// ---------------------------------------------------------------------------
// split_frontmatter：BOM / CRLF / 缺围栏 / 正文两端 trim
// ---------------------------------------------------------------------------

#[test]
fn split_frontmatter_normal_and_crlf() {
    let (fm, body) =
        super::experts_store::split_frontmatter("---\nname = \"A\"\n---\n\nBody line.\n").unwrap();
    assert!(fm.contains("name = \"A\""));
    assert_eq!(body, "Body line.");

    let (_, body2) =
        super::experts_store::split_frontmatter("---\r\nname = \"A\"\r\n---\r\n\r\nCRLF body.\r\n")
            .unwrap();
    assert!(body2.contains("CRLF body."));

    // write 落盘形态（双换行分隔）round-trip 逐字节稳定。
    let (_, body3) =
        super::experts_store::split_frontmatter("---\nname = \"X\"\n---\n\nYou write docs.")
            .unwrap();
    assert_eq!(body3, "You write docs.");

    assert!(super::experts_store::split_frontmatter("no fence").is_err());
    assert!(super::experts_store::split_frontmatter("---\nname = 1\n").is_err());
    let (fm3, _) =
        super::experts_store::split_frontmatter("\u{feff}---\nname = \"B\"\n---\nx\n").unwrap();
    assert!(fm3.contains("name = \"B\""), "BOM 应被容忍");
}

// ---------------------------------------------------------------------------
// handle()：roster 投影（含停用标记 + 默认名）与 resolve（执行面）
// ---------------------------------------------------------------------------

#[test]
fn handle_roster_and_resolve() {
    let (_t, s) = store();
    s.hire(
        "reviewer",
        "Reviewer",
        "reviews",
        vec!["rv".into()],
        "You review.".into(),
        "p9",
    )
    .unwrap();
    s.create(&up("Silent", "p")).unwrap();
    s.set_enabled("Silent", false).unwrap();
    s.set_default_name("总管").unwrap();

    let arc = Arc::new(s);
    let h = arc.handle();
    let roster = (h.roster)();
    assert_eq!(roster.default_name, "总管");
    assert_eq!(roster.experts.len(), 2);
    let dis = roster.experts.iter().find(|e| e.slug == "Silent").unwrap();
    assert!(!dis.enabled, "停用专家在名册中带 DISABLED 标记");

    let d = (h.resolve)("reviewer").unwrap();
    assert_eq!(d.prompt, "You review.");
    let err = (h.resolve)("silent").unwrap_err();
    assert!(err.contains("disabled"), "停用专家 resolve 诚实拒绝: {err}");
    let err2 = (h.resolve)("ghost").unwrap_err();
    assert!(err2.contains("Unknown expert"), "got: {err2}");
}

// ---------------------------------------------------------------------------
// 空/缺席目录：list 空不炸（experts 目录不存在是常态）
// ---------------------------------------------------------------------------

#[test]
fn list_on_missing_dir_is_empty() {
    let (_t, s) = store();
    assert!(s.list().is_empty());
    let roster = (Arc::new(s).handle().roster)();
    assert!(roster.experts.is_empty());
    assert_eq!(roster.default_name, ExpertsStore::FALLBACK_DEFAULT_NAME);
}

// ---------------------------------------------------------------------------
// 模式策略面（2026-11-13 总开关 + 自动降级）：默认投影 / patch 原子写 /
// 校验先行 / 无 config 路径诚实拒绝
// ---------------------------------------------------------------------------

/// 策略面测试用 store：workspace + config.json 双路径注入。
fn store_with_config() -> (TempDir, ExpertsStore, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let s = ExpertsStore::new(tmp.path()).with_config_path(cfg_path.clone());
    (tmp, s, cfg_path)
}

#[test]
fn policy_defaults_and_handle_projection() {
    let (_t, s, _cfg) = store_with_config();
    // config 文件缺席 = 保守默认（模式关 / 降级开 / 通知静默）。
    assert_eq!(
        serde_json::to_value(s.mode_policy_json()).unwrap(),
        serde_json::json!({ "enabled": false, "degrade": true, "notify": false })
    );
    // handle 的 policy 闭包同投影（spawn 侧消费的就是这条臂）。
    let p = (Arc::new(s).handle().policy)();
    assert!(!p.enabled);
    assert!(p.degrade);
    assert!(!p.notify);
    // 手写 Default 与 serde default fn 同源钉子（ ExpertsPolicy↔config 缺键
    // 语义一致：降级缺键=true）。
    let cfg: nemesis_config::Config = serde_json::from_str("{}").unwrap();
    assert_eq!(cfg.agents.experts.enabled, p.enabled);
    assert_eq!(cfg.agents.experts.degrade, p.degrade);
    assert_eq!(cfg.agents.experts.notify, p.notify);
}

#[test]
fn set_mode_policy_minimal_patch_preserves_untyped_keys() {
    let (_t, s, cfg_path) = store_with_config();
    // 预置活文档 config（非 typed 键 + 无 experts 段）。
    std::fs::write(
        &cfg_path,
        r#"{"my_custom":{"keep":1},"channels":{"web":{"port":49000}}}"#,
    )
    .unwrap();

    let out = s
        .set_mode_policy(&serde_json::json!({ "enabled": true }))
        .unwrap();
    assert_eq!(
        out,
        serde_json::json!({ "enabled": true, "degrade": true, "notify": false }),
        "返回值 = 更新后全量投影"
    );

    // 文件面：patch 只写 enabled（degrade/notify 缺键 = serde default fn
    // 兜底）；非 typed 键原样保留（raw patch 不经 typed round-trip）。
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(raw["agents"]["experts"]["enabled"], true);
    assert!(
        raw["agents"]["experts"].get("degrade").is_none(),
        "最小 patch：未点名的键不落盘"
    );
    assert_eq!(raw["my_custom"]["keep"], 1, "非 typed 键必须保留");
    assert_eq!(raw["channels"]["web"]["port"], 49000);

    // 策略闭包即时生效（fresh-read，无需重启）。
    let p = (Arc::new(s).handle().policy)();
    assert!(p.enabled);
    assert!(p.degrade, "缺键 degrade 走 default fn = true");
}

#[test]
fn set_mode_policy_partial_keys_independent() {
    let (_t, s, cfg_path) = store_with_config();
    s.set_mode_policy(&serde_json::json!({ "degrade": false, "notify": true }))
        .unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(raw["agents"]["experts"]["degrade"], false);
    assert_eq!(raw["agents"]["experts"]["notify"], true);
    assert!(
        raw["agents"]["experts"].get("enabled").is_none(),
        "enabled 未点名不落盘"
    );
    assert_eq!(
        s.mode_policy_json(),
        serde_json::json!({ "enabled": false, "degrade": false, "notify": true })
    );

    // null 值 = 跳过该键（部分更新语义）；混合 patch 只收 boolean。
    s.set_mode_policy(&serde_json::json!({ "notify": null, "degrade": true }))
        .unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(raw["agents"]["experts"]["degrade"], true);
    assert_eq!(raw["agents"]["experts"]["notify"], true, "null 不改写");
}

#[test]
fn set_mode_policy_rejects_bad_input() {
    let (_t, s, cfg_path) = store_with_config();
    // 未知键诚实拒绝（防拼写错静默无效）。
    let err = s
        .set_mode_policy(&serde_json::json!({ "enable": true }))
        .unwrap_err();
    assert!(err.contains("未知字段") && err.contains("enable"), "{err}");
    // 非 boolean 拒绝。
    let err = s
        .set_mode_policy(&serde_json::json!({ "enabled": "yes" }))
        .unwrap_err();
    assert!(err.contains("boolean"), "{err}");
    // 空 patch / null 全跳过 = 无可更新。
    let err = s.set_mode_policy(&serde_json::json!({})).unwrap_err();
    assert!(err.contains("无可更新字段"), "{err}");
    let err = s
        .set_mode_policy(&serde_json::json!({ "enabled": null }))
        .unwrap_err();
    assert!(err.contains("无可更新字段"), "{err}");
    // 非对象拒绝。
    let err = s.set_mode_policy(&serde_json::json!(true)).unwrap_err();
    assert!(err.contains("JSON 对象"), "{err}");
    // 全部拒绝后文件不动。
    assert!(!cfg_path.exists(), "校验先行：失败路径零写入");
}

#[test]
fn set_mode_policy_requires_config_path() {
    let (_t, s) = store();
    let err = s
        .set_mode_policy(&serde_json::json!({ "enabled": true }))
        .unwrap_err();
    assert!(err.contains("未装配"), "{err}");
}
