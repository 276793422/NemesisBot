//! DraftStore 的行为测试（从生产文件迁出，遵循内联测试纪律）。

use super::*;
use crate::types::{NodeDef, TriggerConfig};

fn sample_workflow(name: &str) -> Workflow {
    Workflow {
        name: name.to_string(),
        description: "generated".into(),
        version: "1.0.0".into(),
        triggers: vec![TriggerConfig {
            trigger_type: "cron".into(),
            config: Default::default(),
        }],
        nodes: vec![NodeDef {
            id: "start".into(),
            node_type: "llm".into(),
            config: {
                let mut m = std::collections::HashMap::new();
                m.insert("prompt".to_string(), serde_json::json!("hello"));
                m
            },
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
            is_terminal: true,
        }],
        edges: vec![],
        variables: Default::default(),
        metadata: Default::default(),
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nmb_wf_drafts_{}_{}_{}",
        tag,
        std::process::id(),
        chrono::Local::now().timestamp_millis()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn save_list_get_roundtrip() {
    let root = temp_dir("roundtrip");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    store.save(&sample_workflow("daily-report")).unwrap();

    let list = store.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "daily-report");
    assert!(list[0].valid, "errors: {:?}", list[0].validation_errors);
    assert_eq!(list[0].node_count, 1);
    assert_eq!(list[0].trigger_types, vec!["cron"]);

    let detail = store.get("daily-report").unwrap();
    assert_eq!(detail.workflow.name, "daily-report");
    assert!(detail.yaml.contains("daily-report"));
    assert!(detail.summary.valid);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn invalid_draft_still_saves_and_lists_with_errors() {
    let root = temp_dir("invalid");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs);

    let mut wf = sample_workflow("broken");
    wf.nodes.clear(); // validate() rejects empty node list
    let summary = store.save(&wf).unwrap();
    assert!(!summary.valid);
    assert!(!summary.validation_errors.is_empty());

    // apply refuses
    let engine = crate::engine::WorkflowEngine::new();
    let err = store.apply(&engine, "broken").unwrap_err();
    assert!(err.contains("failed validation"), "err: {}", err);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn apply_registers_and_deletes_draft() {
    let root = temp_dir("apply");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    let engine = crate::engine::WorkflowEngine::new();
    engine.set_workflow_defs_dir(defs.clone());

    store.save(&sample_workflow("new-flow")).unwrap();
    let applied = store.apply(&engine, "new-flow").unwrap();
    assert!(!applied.replaced_existing);
    assert!(applied.backup_file.is_none());

    assert!(engine.get_workflow("new-flow").is_some());
    assert!(store.list().is_empty(), "draft must be consumed");
    assert!(defs.join("new-flow.yaml").exists(), "definition written");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn apply_over_existing_backs_up_to_history() {
    let root = temp_dir("backup");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    let engine = crate::engine::WorkflowEngine::new();
    engine.set_workflow_defs_dir(defs.clone());

    // Seed an existing definition through the normal path.
    engine
        .persist_workflow(sample_workflow("existing-flow"))
        .unwrap();

    // Draft a modified version (different node count so we can tell them apart).
    let mut wf = sample_workflow("existing-flow");
    wf.nodes.push(NodeDef {
        id: "second".into(),
        node_type: "delay".into(),
        config: {
            let mut m = std::collections::HashMap::new();
            m.insert("seconds".to_string(), serde_json::json!(1));
            m
        },
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    });
    store.save(&wf).unwrap();

    let applied = store.apply(&engine, "existing-flow").unwrap();
    assert!(applied.replaced_existing);
    let backup = applied.backup_file.expect("backup recorded");

    let hist_dir = defs.join(".history");
    let backup_path = hist_dir.join(&backup);
    assert!(backup_path.exists(), "backup file on disk");
    let old: Workflow =
        serde_yaml::from_str(&std::fs::read_to_string(&backup_path).unwrap()).unwrap();
    assert_eq!(old.nodes.len(), 1, "backup holds the OLD definition");
    assert_eq!(engine.get_workflow("existing-flow").unwrap().nodes.len(), 2);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn apply_over_yml_variant_backs_up_and_cleans_stale_file() {
    // BUG-20261119-188 回归锁：既有定义落 .yml（load 接受三扩展名的受支持
    // 盘面状态）时——备份必须覆盖 .yml（旧实现只查 .yaml，旧 .yml 无备份
    // 且会在重启后遮蔽新定义），persist 的同名单名变体清理必须删掉旧 .yml。
    let root = temp_dir("backup-yml");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    let engine = crate::engine::WorkflowEngine::new();
    engine.set_workflow_defs_dir(defs.clone());

    // Seed an existing definition as a .yml file（手工/迁移放置形态）。
    std::fs::write(
        defs.join("yml-flow.yml"),
        serde_yaml::to_string(&sample_workflow("yml-flow")).unwrap(),
    )
    .unwrap();

    // Draft a modified version (different node count so we can tell them apart).
    let mut wf = sample_workflow("yml-flow");
    wf.nodes.push(NodeDef {
        id: "second".into(),
        node_type: "delay".into(),
        config: {
            let mut m = std::collections::HashMap::new();
            m.insert("seconds".to_string(), serde_json::json!(1));
            m
        },
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    });
    store.save(&wf).unwrap();

    let applied = store.apply(&engine, "yml-flow").unwrap();
    assert!(applied.replaced_existing);
    let backup = applied.backup_file.expect("backup recorded");
    assert!(
        backup.ends_with(".yml"),
        "备份必须抓到 .yml 变体，实得 {backup}"
    );

    let hist_dir = defs.join(".history");
    let backup_path = hist_dir.join(&backup);
    assert!(backup_path.exists(), "backup file on disk");
    let old: Workflow =
        serde_yaml::from_str(&std::fs::read_to_string(&backup_path).unwrap()).unwrap();
    assert_eq!(old.nodes.len(), 1, "backup holds the OLD definition");

    // 旧 .yml 已被 persist 的变体清理删除；新定义落 .yaml。
    assert!(
        !defs.join("yml-flow.yml").exists(),
        "旧 .yml 必须被 persist 变体清理删除，防重启后遮蔽新定义"
    );
    assert!(defs.join("yml-flow.yaml").exists(), "新 .yaml 主文件落盘");
    assert_eq!(engine.get_workflow("yml-flow").unwrap().nodes.len(), 2);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rapid_reapplies_keep_distinct_history_backups() {
    // Two applies inside the same second must not overwrite each other's
    // .history/ backup (fs::copy replaces the destination silently) — an
    // agent iterating create→apply without runs produces rapid consecutive
    // applies. The backup timestamp carries milliseconds, so each apply
    // keeps its own backup and the OLDER one survives verbatim.
    let root = temp_dir("backup-ms");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    let engine = crate::engine::WorkflowEngine::new();
    engine.set_workflow_defs_dir(defs.clone());

    // v1 (1 node) registered through the normal path.
    engine.persist_workflow(sample_workflow("hot")).unwrap();

    // v2 (2 nodes) and v3 (3 nodes) applied back-to-back.
    //
    // The 5ms sleep is part of the fixture, not a delay tactic: it
    // guarantees the two backup timestamps land in distinct milliseconds
    // (a sub-millisecond gap could coincidentally straddle zero ms
    // distance and fail the fixed code), while staying far inside the
    // same second — which is exactly the window the old second-resolution
    // timestamp collided in.
    let mut wf2 = sample_workflow("hot");
    wf2.nodes.push(NodeDef {
        id: "n2".into(),
        node_type: "delay".into(),
        config: {
            let mut m = std::collections::HashMap::new();
            m.insert("seconds".to_string(), serde_json::json!(1));
            m
        },
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    });
    store.save(&wf2).unwrap();
    let a1 = store.apply(&engine, "hot").unwrap();
    assert!(a1.replaced_existing);

    std::thread::sleep(std::time::Duration::from_millis(5));

    let mut wf3 = wf2.clone();
    wf3.nodes.push(NodeDef {
        id: "n3".into(),
        node_type: "delay".into(),
        config: {
            let mut m = std::collections::HashMap::new();
            m.insert("seconds".to_string(), serde_json::json!(2));
            m
        },
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    });
    store.save(&wf3).unwrap();
    let a2 = store.apply(&engine, "hot").unwrap();
    assert!(a2.replaced_existing);

    // Both backups survive with distinct names AND distinct contents:
    // a1's backup holds v1 (1 node), a2's holds v2 (2 nodes).
    assert_ne!(
        a1.backup_file.as_ref(),
        a2.backup_file.as_ref(),
        "distinct backup names"
    );
    let hist = defs.join(".history");
    let names: Vec<std::path::PathBuf> = std::fs::read_dir(&hist)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    assert_eq!(names.len(), 2, "both backups must survive: {:?}", names);
    let parse = |name: &str| -> Workflow {
        serde_yaml::from_str(&std::fs::read_to_string(hist.join(name)).unwrap()).unwrap()
    };
    assert_eq!(
        parse(a1.backup_file.as_deref().unwrap()).nodes.len(),
        1,
        "a1's backup must hold v1"
    );
    assert_eq!(
        parse(a2.backup_file.as_deref().unwrap()).nodes.len(),
        2,
        "a2's backup must hold v2"
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn discard_is_idempotent() {
    let root = temp_dir("discard");
    let store = DraftStore::new(root.join("drafts"), root.join("definitions"));
    store.save(&sample_workflow("doomed")).unwrap();
    store.discard("doomed").unwrap();
    store.discard("doomed").unwrap(); // second call: Ok
    assert!(store.list().is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn drafts_dir_is_sibling_of_definitions() {
    let defs = Path::new("/ws/workflow/definitions");
    assert_eq!(drafts_dir_from_defs(defs), Path::new("/ws/workflow/drafts"));
}

#[test]
fn sanitizer_matches_engine_behavior() {
    assert_eq!(sanitize_workflow_filename("a b/c.d"), "a_b_c_d");
    assert_eq!(sanitize_workflow_filename(""), "wf_unnamed");
    // '.' is replaced like any other unsafe char — `.`/`..` 穿越与隐藏文件
    // 首点在白名单映射后结构性不可能出现。engine.rs 经 use 消费同一实现
    //（BUG-20261125-073 单一真相源，双副本已删），draft stems ==
    // definition stems 由编译期同源保证。
    assert_eq!(sanitize_workflow_filename(".hidden"), "_hidden");
}

/// 保留设备名中和回归锁（BUG-20261125-073）：白名单全字母数字的
/// `con`/`nul`/`com1` 等此前逐字节直通——`<defs>/con.yaml` /
/// `<drafts>/nul.yaml` 在旧 Windows（10/Server）设备解析（创建诡误/
/// 写入设备/静默丢数据），与 sanitize_path_segment 出口中和
///（BUG-20261111-083）同威胁面同形态（前缀 `_` 断开设备名解析）。
#[test]
fn sanitizer_neutralizes_windows_reserved_device_names() {
    // 纯保留名（扩展名前的基名命中）。
    assert_eq!(sanitize_workflow_filename("con"), "_con");
    assert_eq!(sanitize_workflow_filename("nul"), "_nul");
    assert_eq!(sanitize_workflow_filename("prn"), "_prn");
    assert_eq!(sanitize_workflow_filename("aux"), "_aux");
    // 大小写不敏感（Windows 设备名解析不分大小写）。
    assert_eq!(sanitize_workflow_filename("COM1"), "_COM1");
    assert_eq!(sanitize_workflow_filename("Lpt4"), "_Lpt4");
    // 输入带点：白名单先映射 `.` → `_`，产出 `con_yaml`（基名非 `con`，
    // 天然安全不命中——`con_yaml.yaml` 不是设备名）。
    assert_eq!(sanitize_workflow_filename("con.yaml"), "con_yaml");
    // 带下划线后缀的长形态不是保留名——直通不变。
    assert_eq!(sanitize_workflow_filename("con_x"), "con_x");
    assert_eq!(sanitize_workflow_filename("conveyor"), "conveyor");
    // 普通名零影响（非保留名的既有映射不变）。
    assert_eq!(sanitize_workflow_filename("my_flow"), "my_flow");
    // 非法字符映射出尾部 `_` 的形态不命中（`con ` → `con_`，基名
    // `con_` ≠ `con`；Win32 只剥离尾点/尾空格不剥下划线——尾 `_` 是
    // 稳定安全形态，无剥离回退风险）。
    assert_eq!(sanitize_workflow_filename("con "), "con_");
    // 空串形态不变。
    assert_eq!(sanitize_workflow_filename(""), "wf_unnamed");
}

#[test]
fn save_surfaces_lint_warnings_in_summary() {
    let root = temp_dir("warnings");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    // llm max_tokens 过小 + 无触发器 → + 两条 warning
    let mut wf = sample_workflow("risky");
    wf.triggers.clear();
    wf.nodes[0]
        .config
        .insert("max_tokens".to_string(), serde_json::json!(200));
    store.save(&wf).unwrap();

    let summary = &store.list()[0];
    assert!(summary.valid, "lint 是 warning 通道，不影响 valid");
    assert_eq!(summary.warnings.len(), 2, "{:?}", summary.warnings);
    assert!(summary.warnings[0].contains("max_tokens=200"));
    assert!(summary.warnings[1].contains("run_now"));

    std::fs::remove_dir_all(&root).ok();
}
