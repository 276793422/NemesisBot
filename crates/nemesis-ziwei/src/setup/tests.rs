//! setup 单测（M3，T3.2-T3.7 文件面）。
//!
//! 纯文件步骤（annotate/define_struct/define_tool/commit/abort/status）
//! 跨平台直测；select_target/snapshot/diff 依赖真实进程 + payload 管道，
//! 归 UAT（dummy-draw 全链路，`test-tools/resource/ziwei-uat/m3/`）。

use std::path::PathBuf;

use super::{
    SetupInfo, SetupManager, SetupState, draft_dir, global, load_draft, save_draft,
    save_draft_anchors,
};
use nemesis_ziwei_profile::{AnchorSet, Machine, Profile, ProfileStore, SCHEMA_VERSION};

// ── 夹具 ────────────────────────────────────────────────────────────────

fn temp_ws(tag: &str) -> PathBuf {
    let ws = std::env::temp_dir().join(format!(
        "zwsetup-{}-{}-{tag}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    std::fs::create_dir_all(&ws).unwrap();
    ws
}

fn cleanup(ws: &PathBuf) {
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(ws);
}

/// 最小 draft（select_target 在 windows 下的产物形态）。
fn test_draft(id: &str) -> Profile {
    let now = chrono::Utc::now();
    serde_json::from_slice(
        &serde_json::to_vec(&serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "profile_id": id,
            "name": format!("draft: dummy.exe"),
            "created_at": now.to_rfc3339(),
            "last_verified_at": now.to_rfc3339(),
            "status": "active",
            "target": {
                "exe_name": "dummy.exe",
                "machine": "x64",
                "file_sha256": format!("{:064x}", 42u32),
                "verify_policy": "hash_or_version",
            },
            "verification": {
                "on_attach": ["file_sha256", "anchors_vote>=2"],
                "stale_action": "notify_user",
            },
        }))
        .unwrap(),
    )
    .unwrap()
}

/// 建 draft 目录 + 文件（模拟 select_target 的落盘产物）。
fn seed_draft(ws: &std::path::Path, id: &str) {
    let dir = draft_dir(ws, id);
    std::fs::create_dir_all(&dir).unwrap();
    save_draft(ws, &test_draft(id)).unwrap();
    save_draft_anchors(
        ws,
        &AnchorSet {
            profile_id: id.to_string(),
            anchors: Vec::new(),
        },
    )
    .unwrap();
}

fn inject_state(mgr: &SetupManager, id: &str) {
    mgr.state.lock().unwrap().replace(SetupState {
        info: SetupInfo {
            profile_id: id.to_string(),
            pid: 4242,
            module_base: 0x140000000,
            module_size: 0x10000,
            machine: Machine::X64,
            exe_path: PathBuf::from("C:/dummy/dummy.exe"),
            exe_name: "dummy.exe".to_string(),
            file_sha256: "aa".repeat(32),
        },
        analysis: None,
    });
}

fn annotate_body(semantic: &str, with_anchors: bool) -> serde_json::Value {
    serde_json::json!({
        "rva": 0x2A10u64,
        "semantic": semantic,
        "anchors": if with_anchors {
            vec![serde_json::json!({
                "kind": "byte_pattern",
                "pattern": "90 90 90 90 b8 8d 7c 6b 5a",
            })]
        } else {
            vec![]
        },
    })
}

// ── annotate ────────────────────────────────────────────────────────────

#[tokio::test]
async fn annotate_forces_anchor_and_persists() {
    let ws = temp_ws("ann");
    let id = "zw-pro-d111";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    // 强制锚：无锚 = 拒绝
    let err = mgr
        .annotate(&ws, &annotate_body("push_record", false))
        .unwrap_err();
    assert!(err.contains("≥1"), "实际：{err}");

    // 带锚：通过；draft.functions/anchors 落盘
    let out = mgr
        .annotate(&ws, &annotate_body("push_record", true))
        .unwrap();
    assert_eq!(out["annotated"], "push_record");
    assert_eq!(out["anchors"], 1);
    let profile = load_draft(&ws, id).unwrap();
    assert_eq!(profile.functions.len(), 1);
    assert_eq!(profile.functions[0].semantic, "push_record");
    assert_eq!(
        profile.functions[0].locate.kind,
        nemesis_ziwei_profile::LocateKind::BytePattern,
        "byte_pattern 锚应成为函数 locate"
    );
    let anchors = super::load_draft_anchors(&ws, id).unwrap();
    assert_eq!(anchors.anchors.len(), 1);
    assert_eq!(anchors.anchors[0].function, "push_record");
    assert_eq!(anchors.anchors[0].module.as_deref(), Some("dummy.exe"));

    // 重复语义名 = 拒绝；重复 RVA = 拒绝
    let dup = mgr
        .annotate(&ws, &annotate_body("push_record", true))
        .unwrap_err();
    assert!(dup.contains("已标注"), "实际：{dup}");
    let mut other = annotate_body("feed_render", true);
    other["rva"] = serde_json::json!(0x2A10u64); // 同 RVA 不同名
    let dup_rva = mgr.annotate(&ws, &other).unwrap_err();
    assert!(dup_rva.contains("已被"), "实际：{dup_rva}");

    // 坏 pattern = 拒绝（奇数 hex；独立 RVA——RVA 查重先于 pattern 解析）
    let mut bad = annotate_body("other_fn", true);
    bad["rva"] = serde_json::json!(0x2A20u64);
    bad["anchors"] = serde_json::json!([{"kind": "byte_pattern", "pattern": "9 zz"}]);
    let bad_pat = mgr.annotate(&ws, &bad).unwrap_err();
    assert!(bad_pat.contains("pattern 非法"), "实际：{bad_pat}");
    cleanup(&ws);
}

// ── define_struct / define_tool ─────────────────────────────────────────

#[tokio::test]
async fn define_struct_and_tool_roundtrip() {
    let ws = temp_ws("ds");
    let id = "zw-pro-d222";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    // 结构定义
    let ds = serde_json::json!({
        "name": "DataHeader",
        "size": 24,
        "fields": [
            {"offset": 0, "name": "magic", "type": "u32"},
            {"offset": 4, "name": "layout_version", "type": "u32"},
            {"offset": 8, "name": "count", "type": "u32"},
            {"offset": 12, "name": "capacity", "type": "u32"},
            {"offset": 16, "name": "last_tick_ms", "type": "u64"},
        ],
    });
    let out = mgr.define_struct(&ws, &ds).unwrap();
    assert_eq!(out["struct"], "DataHeader");

    // 越界字段 = 拒绝
    let bad = serde_json::json!({
        "name": "Bad", "size": 8,
        "fields": [{"offset": 4, "name": "v", "type": "u64"}],
    });
    let err = mgr.define_struct(&ws, &bad).unwrap_err();
    assert!(err.contains("越界"), "实际：{err}");

    // 未知类型 = 拒绝
    let bad_ty = serde_json::json!({
        "name": "Bad2", "size": 8,
        "fields": [{"offset": 0, "name": "v", "type": "int128"}],
    });
    assert!(mgr.define_struct(&ws, &bad_ty).is_err());

    // poll 工具（struct: 引用已定义 → 校验过）
    let poll = serde_json::json!({
        "tool_name": "dummy_area_read",
        "description": "读 DataHeader+记录",
        "action": {
            "primitive": "poll_report",
            "addr": "struct:DataHeader",
            "mode": "on_change",
        },
        "output": {"dest": "ziwei/runs/latest.json"},
    });
    let out = mgr.define_tool(&ws, &poll).unwrap();
    assert_eq!(out["tools"], 1);

    // call_fn 引用未标注函数 = 拒绝
    let bad_call = serde_json::json!({
        "tool_name": "dummy_call",
        "description": "调 push_record",
        "action": {"primitive": "call_fn", "addr": "fn:push_record"},
    });
    let err = mgr.define_tool(&ws, &bad_call).unwrap_err();
    assert!(err.contains("不在 functions"), "实际：{err}");

    // 标注后 call_fn 可定义（默认 run_policy=confirm，决议 1）
    mgr.annotate(&ws, &annotate_body("push_record", true))
        .unwrap();
    let call = mgr.define_tool(&ws, &bad_call).unwrap();
    assert_eq!(call["tools"], 2);
    let profile = load_draft(&ws, id).unwrap();
    let call_tool = profile
        .tools
        .iter()
        .find(|t| t.tool_name == "dummy_call")
        .unwrap();
    assert_eq!(
        call_tool.run_policy,
        nemesis_ziwei_profile::RunPolicy::Confirm,
        "call_fn 默认 confirm"
    );
    cleanup(&ws);
}

// ── commit / abort / status ─────────────────────────────────────────────

#[tokio::test]
async fn commit_migrates_draft_to_profile() {
    let ws = temp_ws("commit");
    let id = "zw-pro-d333";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    // 无函数 = 拒绝提交
    let err = mgr.commit(&ws).unwrap_err();
    assert!(err.contains("至少 annotate"), "实际：{err}");

    // 补齐：结构 + 函数 + 工具
    mgr.define_struct(
        &ws,
        &serde_json::json!({
            "name": "DataHeader", "size": 24,
            "fields": [{"offset": 0, "name": "magic", "type": "u32"}],
        }),
    )
    .unwrap();
    mgr.annotate(&ws, &annotate_body("push_record", true))
        .unwrap();
    mgr.define_tool(
        &ws,
        &serde_json::json!({
            "tool_name": "dummy_area_read",
            "description": "读",
            "action": {"primitive": "poll_report", "addr": "struct:DataHeader"},
        }),
    )
    .unwrap();

    let out = mgr.commit(&ws).unwrap();
    assert_eq!(out["committed"], id);
    assert_eq!(out["functions"], 1);
    assert_eq!(out["tools"], 1);
    assert_eq!(out["anchors"], 1);

    // profiles/ 有正式 profile 且可读回；draft 目录已清
    let store = ProfileStore::new(&ws);
    let profile = store.load(id).unwrap();
    assert_eq!(profile.functions[0].semantic, "push_record");
    let anchors = store.load_anchors(id).unwrap();
    assert_eq!(anchors.anchors.len(), 1);
    assert!(!draft_dir(&ws, id).exists(), "draft 目录应已删除");
    // 提交后状态清空（再次 commit = 无向导）
    assert!(mgr.commit(&ws).is_err());
    cleanup(&ws);
}

#[tokio::test]
async fn abort_clears_orphan_draft_and_state() {
    let ws = temp_ws("abort");
    let id = "zw-pro-d444";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    let out = mgr.abort(&ws).await.unwrap();
    assert_eq!(out["aborted"], id);
    assert!(!draft_dir(&ws, id).exists(), "draft 目录应已删除");
    assert!(
        !ws.join("ziwei").join("setup").join(id).exists(),
        "setup 目录应已删除"
    );
    // 二次 abort = 无向导
    assert!(mgr.abort(&ws).await.is_err());
    cleanup(&ws);
}

#[tokio::test]
async fn status_falls_back_to_disk_after_restart() {
    let ws = temp_ws("status");
    let id = "zw-pro-d555";
    seed_draft(&ws, id);
    let mgr = SetupManager::new(); // 模拟 gateway 重启：内存态空

    let st = mgr.status(&ws).unwrap();
    assert_eq!(st["profile_id"], id);
    assert_eq!(st["resumed"], true);

    // 文件面步骤照常可用（require_info 重建路径）
    let out = mgr
        .annotate(&ws, &annotate_body("fn_after_restart", true))
        .unwrap();
    assert_eq!(out["annotated"], "fn_after_restart");

    // 快照类步骤诚实拒绝（基址不可信）
    let err = mgr.snapshot(&ws).await.unwrap_err();
    assert!(
        err.contains("ASLR") || err.contains("重新 select_target"),
        "实际：{err}"
    );
    cleanup(&ws);
}

#[tokio::test]
async fn select_target_rejects_when_draft_exists() {
    let ws = temp_ws("sel");
    let id = "zw-pro-d666";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    // 有 draft 未收尾 → 无论平台，select_target 必须拒绝（开新向导前提）
    let err = mgr.select_target(&ws, Some(1), None).await.unwrap_err();
    assert!(
        err.contains("draft") || err.contains("仅 Windows"),
        "实际：{err}"
    );
    let _ = id;
    cleanup(&ws);
}

// ── 全局槽 / 辅助 ───────────────────────────────────────────────────────

#[test]
fn global_slot_stable() {
    let a = global();
    let b = global();
    assert!(std::sync::Arc::ptr_eq(&a, &b), "全局槽应返回同一实例");
}

#[test]
fn normalize_pattern_shapes_hex() {
    // 经 annotate 间接验证（函数私有）：byte_pattern 锚 pattern 归一化大写
    let ws = temp_ws("norm");
    let id = "zw-pro-d777";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);
    mgr.annotate(
        &ws,
        &serde_json::json!({
            "rva": 0x10u64,
            "semantic": "norm_fn",
            "anchors": [{"kind": "byte_pattern", "pattern": "b8 8d 7c 6b 5a"}],
        }),
    )
    .unwrap();
    let anchors = super::load_draft_anchors(&ws, id).unwrap();
    assert_eq!(
        anchors.anchors[0].pattern.as_deref(),
        Some("B8 8D 7C 6B 5A")
    );
    cleanup(&ws);
}
