//! setup 单测（.2-.7 文件面）。
//!
//! 纯文件步骤（annotate/define_struct/define_tool/commit/abort/status）
//! 跨平台直测；select_target/snapshot/diff 依赖真实进程 + payload 管道，
//! 归 UAT（dummy-draw 全链路，`test-tools/resource/ziwei-uat/m3/`）。

use std::path::PathBuf;

use super::{
    SetupInfo, SetupManager, SetupState, draft_dir, global, init_draft_files, load_draft,
    save_draft, save_draft_anchors,
};
// rollback_draft_error 的诚实性单测是 windows 门（share_mode）——Linux 测试
// 目标不引用它，跟随生产侧 cfg(windows) 门，避免未使用导入红。
#[cfg(windows)]
use super::rollback_draft_error;
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

/// pid=0 占位形态的 info（select_target 预占态 / analyze 重建态共用形状）。
fn placeholder_info(id: &str) -> SetupInfo {
    SetupInfo {
        profile_id: id.to_string(),
        pid: 0,
        module_base: 0,
        module_size: 0,
        machine: Machine::X64,
        exe_path: PathBuf::new(),
        exe_name: String::new(),
        file_sha256: String::new(),
    }
}

/// 预占占位态（select_target 锁内插入的窗口期形态）。
fn inject_reserved(mgr: &SetupManager, id: &str) {
    mgr.state.lock().unwrap().replace(SetupState {
        info: placeholder_info(id),
        analysis: None,
        reserved: true,
    });
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
        reserved: false,
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

    // 快照类步骤诚实拒绝（基址不可信；Linux 形态诚实报「仅 Windows」——
    // 同 select_target 用例的平台双臂断言先例）
    let err = mgr.snapshot(&ws).await.unwrap_err();
    assert!(
        err.contains("ASLR") || err.contains("重新 select_target") || err.contains("仅 Windows"),
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

// ──：draft 与已提交库的隔离根因钉死 ────────────────────

/// 根因不对称：向导期 profile_id 只存在于 `ziwei/drafts/<id>/`（本向导
/// 新造的毫秒戳 id，已提交库 `ziwei/profiles/<id>/` 必不存在）——旧 hook
/// 臂用 `ProfileStore:load`（只读已提交库）恒 NotFound → hook_notes
/// 「draft profile 不可用」+ fn_addrs 永不设置 = hook=true 臂整条死路。
/// 修复后 hook 臂走 `load_draft`（本测试钉死：同一 id load_draft 成功、
/// ProfileStore:load 诚实 NotFound）。
#[test]
fn draft_only_id_is_invisible_to_committed_store() {
    let ws = temp_ws("draft-only");
    let id = "zw-pro-d1751700000000";
    seed_draft(&ws, id);

    // 新路径：hook 臂同源读法，成功
    let profile = load_draft(&ws, id).expect("load_draft 应命中向导 draft");
    assert_eq!(profile.profile_id, id);

    // 旧路径：已提交库对同一 id 诚实 NotFound（死路根因）
    let committed = ProfileStore::new(&ws).load(id);
    assert!(committed.is_err(), "已提交库不得看到 draft：{committed:?}");

    cleanup(&ws);
}

// ── 预占态竞态修复（2026-11-05 风险报告评审）：reserved 单一真相源 ────────
//
// select_target 预占窗口（BUG-20261105-019）以 pid=0 占位态插入，旧形态无
// 显式标记：并发 abort 跳过 detach 后无条件删 draft + 清态，预占者写回无
// 条件复活 → 僵尸活动态 + 注入会话泄漏；Err 臂无条件清态可误清他人态。
// 修复 = SetupState.reserved 显式字段 + 清态/写回归属校验式 + abort 拒绝
// 预占态。pid==0 不可作占位判据（analyze 重建态合法 pid=0），重建态照常
// 可 abort 由 abort_clears_rebuilt_state 钉死。
// select_target_reserved 本体（windows + 真实进程）归 UAT。

#[test]
fn clear_state_if_owned_only_clears_own_state() {
    let mgr = SetupManager::new();

    // 自己的态：清空 + true
    inject_state(&mgr, "zw-pro-c1");
    assert!(mgr.clear_state_if_owned("zw-pro-c1"));
    assert!(mgr.state.lock().unwrap().is_none());

    // 他人态（id 不匹配）：不清 + false
    inject_state(&mgr, "zw-pro-c1");
    assert!(!mgr.clear_state_if_owned("zw-pro-c2"));
    assert!(mgr.state.lock().unwrap().is_some(), "易主时不得误清他人态");

    // None：false
    *mgr.state.lock().unwrap() = None;
    assert!(!mgr.clear_state_if_owned("zw-pro-c1"));
    assert!(mgr.state.lock().unwrap().is_none());
}

#[test]
fn publish_state_if_owned_closes_reservation_and_refuses_takeover() {
    let mgr = SetupManager::new();
    let info = SetupInfo {
        profile_id: "zw-pro-p1".to_string(),
        pid: 1234,
        module_base: 0x140000000,
        module_size: 0x10000,
        machine: Machine::X64,
        exe_path: PathBuf::from("C:/x/t.exe"),
        exe_name: "t.exe".to_string(),
        file_sha256: "bb".repeat(32),
    };

    // 预占态 + 同 id 写回：替换为完整态且 reserved 落 false（窗口收口）
    inject_reserved(&mgr, "zw-pro-p1");
    assert!(mgr.publish_state_if_owned("zw-pro-p1", info.clone()));
    {
        let st = mgr.state.lock().unwrap();
        let st = st.as_ref().expect("写回后状态应在场");
        assert_eq!(st.info.pid, 1234);
        assert!(!st.reserved, "写回应落 reserved=false（预占收口）");
    }

    // 易主（当前态 id ≠ 写回 id）：拒写且不覆盖
    inject_state(&mgr, "zw-pro-p1");
    let mut foreign = info.clone();
    foreign.profile_id = "zw-pro-other".to_string();
    assert!(!mgr.publish_state_if_owned("zw-pro-other", foreign));
    assert_eq!(
        mgr.state.lock().unwrap().as_ref().unwrap().info.profile_id,
        "zw-pro-p1",
        "易主时不得覆盖他人态"
    );

    // None：拒写且不复活（旧实现无条件写回正是僵尸态来源）
    *mgr.state.lock().unwrap() = None;
    assert!(!mgr.publish_state_if_owned("zw-pro-p1", info));
    assert!(mgr.state.lock().unwrap().is_none(), "None 态不得被写回复活");
}

#[test]
fn init_draft_files_creates_all_three() {
    let ws = temp_ws("init-ok");
    let id = "zw-pro-init1";
    let profile = test_draft(id);
    let draft = init_draft_files(&ws, &profile).unwrap();
    assert_eq!(draft, draft_dir(&ws, id));
    assert!(draft.join("draft.json").exists(), "draft.json 应落盘");
    assert!(draft.join("anchors.json").exists(), "anchors.json 应落盘");
    assert!(load_draft(&ws, id).is_ok(), "draft.json 应可回读");
    cleanup(&ws);
}

#[test]
fn init_draft_files_rolls_back_partial_failure() {
    let ws = temp_ws("init-fail");
    let id = "zw-pro-init2";
    let profile = test_draft(id);
    // 失败注入：draft.json 位置预置为目录 → save_draft 的 fs::write 必败
    let draft = draft_dir(&ws, id);
    std::fs::create_dir_all(draft.join("draft.json")).unwrap();
    let err = init_draft_files(&ws, &profile).unwrap_err();
    assert!(err.contains("draft.json"), "实际：{err}");
    assert!(
        !draft.exists(),
        "失败应回滚整个 draft 目录（不留半成品挡死新向导）"
    );
    cleanup(&ws);
}

#[tokio::test]
async fn abort_refuses_during_reservation() {
    let ws = temp_ws("abort-res");
    let id = "zw-pro-abt1";
    let mgr = SetupManager::new();
    inject_reserved(&mgr, id);

    let err = mgr.abort(&ws).await.unwrap_err();
    assert!(err.contains("预占"), "实际：{err}");
    // 状态保留：select_target 继续持有预占，收尾写回不受干扰
    let st = mgr.status(&ws).unwrap();
    assert_eq!(st["profile_id"], id, "预占态应原样保留");
    cleanup(&ws);
}

#[tokio::test]
async fn abort_clears_rebuilt_state_from_disk() {
    // gateway 重启后 analyze 重建的 pid=0 态（reserved=false）必须照常可
    // abort——预占拒绝只针对 reserved=true 占位态，不误伤重建态合法路径。
    let ws = temp_ws("abort-rebuild");
    let id = "zw-pro-abt2";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    mgr.state.lock().unwrap().replace(SetupState {
        info: placeholder_info(id),
        analysis: None,
        reserved: false,
    });

    let out = mgr.abort(&ws).await.unwrap();
    assert_eq!(out["aborted"], id);
    assert!(mgr.state.lock().unwrap().is_none(), "abort 后状态应清空");
    assert!(!draft_dir(&ws, id).exists(), "draft 目录应已删除");
    cleanup(&ws);
}

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

// ── 清理失败诚实化（横扫族 C，2026-11-06）────────────────────────────────
// Windows 形态测试（编译期消失，Linux 打开句柄不阻碍删除）。失败注入：
// 持有句柄且 share_mode 不含 FILE_SHARE_DELETE → remove_dir_all 报
// sharing violation（ERROR 32），确定性失败。注意裸 File::open 挡不住——
// std 默认 share mode 含 FILE_SHARE_DELETE（实测实证），必须显式 share_mode。
// 失败注入会让 remove_dir_all_retry 走满 ~4.4s 退避窗口，两测各多耗数秒。

#[cfg(windows)]
#[tokio::test]
async fn abort_reports_draft_cleanup_failure_and_keeps_state() {
    let ws = temp_ws("abort-lock");
    let id = "zw-pro-abt3";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    // 失败注入：draft 目录内放一个被持有句柄的文件 → 删除必败
    let locked = draft_dir(&ws, id).join("locked.bin");
    std::fs::write(&locked, b"x").unwrap();
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2) // READ|WRITE，无 DELETE → 删除必撞 sharing violation
        .open(&locked)
        .unwrap();

    let err = mgr.abort(&ws).await.unwrap_err();
    assert!(err.contains("清理失败"), "实际：{err}");
    assert!(err.contains("可重试"), "实际：{err}");
    // 未收尾：状态保留（可重试 abort），draft 目录仍在
    assert!(
        mgr.state.lock().unwrap().is_some(),
        "清理失败不得清态（假成功）"
    );
    assert!(draft_dir(&ws, id).exists(), "draft 目录应保留待重试");

    // 释放句柄后重试 abort = 收敛成功（幂等可重试的实证）
    drop(handle);
    let out = mgr.abort(&ws).await.unwrap();
    assert_eq!(out["aborted"], id);
    assert!(mgr.state.lock().unwrap().is_none());
    assert!(!draft_dir(&ws, id).exists());
    cleanup(&ws);
}

#[cfg(windows)]
#[tokio::test]
async fn commit_reports_draft_cleanup_failure_in_warnings() {
    let ws = temp_ws("commit-lock");
    let id = "zw-pro-cmt4";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);
    // 补齐提交前置（同 commit_migrates_draft_to_profile）
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

    // 失败注入：draft 目录内放一个被持有句柄的文件 → 删除必败
    let locked = draft_dir(&ws, id).join("locked.bin");
    std::fs::write(&locked, b"x").unwrap();
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2) // READ|WRITE，无 DELETE → 删除必撞 sharing violation
        .open(&locked)
        .unwrap();

    // save 已落盘不可回滚 → commit 不假失败，清理失败进返回值注记
    let out = mgr.commit(&ws).unwrap();
    assert_eq!(out["committed"], id);
    let warnings = out["cleanup_warnings"].as_array().unwrap();
    assert!(
        !warnings.is_empty()
            && warnings
                .iter()
                .any(|w| w.as_str().unwrap_or("").contains("draft")),
        "cleanup_warnings 应注记 draft 清理失败：{warnings:?}"
    );
    // 正式 profile 已提交可读回（提交本体不受清理降级影响）
    let store = ProfileStore::new(&ws);
    assert!(store.load(id).is_ok());

    drop(handle);
    cleanup(&ws);
}

#[cfg(windows)]
#[test]
fn rollback_draft_error_is_honest_about_rollback_failure() {
    // 横扫族 C 二轮：select_target_reserved 三处失败臂的「已回滚」断言
    // 不得谎报——回滚失败时错误串必须含「回滚失败」+ abort 收尸指引、
    // 不得含 ok_suffix；回滚成功时 base+ok_suffix 原样拼接。
    let ws = temp_ws("rb-lock");
    let id = "zw-pro-rb5";
    seed_draft(&ws, id);
    let draft = draft_dir(&ws, id);

    // 失败注入：同款 share_mode 剥 DELETE 位 → 删除必撞 sharing violation
    let locked = draft.join("locked.bin");
    std::fs::write(&locked, b"x").unwrap();
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(&locked)
        .unwrap();

    let err = rollback_draft_error(&draft, "主错误原文".to_string(), "（draft 已回滚）");
    assert!(err.starts_with("主错误原文"), "主错误保首位：{err}");
    assert!(err.contains("回滚失败"), "回滚失败必须说话：{err}");
    assert!(err.contains("abort 收尸"), "应带收尸指引：{err}");
    assert!(!err.contains("已回滚"), "不得谎报已回滚：{err}");
    assert!(draft.exists(), "残留目录应保留待收尸");

    // 释放句柄后重试：回滚成功 → base+ok_suffix 拼接，目录删除
    drop(handle);
    let ok = rollback_draft_error(&draft, "主错误原文".to_string(), "（draft 已回滚）");
    assert_eq!(ok, "主错误原文（draft 已回滚）");
    assert!(!draft.exists());
    cleanup(&ws);
}

// ── analyze 全链路（离线夹具）+ 查询面矩阵 ────────────────────────────────
//
// select_target/snapshot 依赖真实进程 + 注入管道（归 UAT），但 analyze 起点是
// 磁盘快照（SnapshotStore 落盘产物）+ 盘上 exe——两者都能用手工构造的最小
// x64 PE 夹具离线喂入。夹具：.text 带调用图/立即数/rip 相对引用，
// .rdata 带导出/导入/字符串（与 nemesis-ziwei-rev 测试夹具同构）。
// iced-x86 解码器是纯 Rust，analyze→查询面全部可离线驱动。

use nemesis_ziwei_rev::snapshot::{SnapshotMeta, SnapshotStore};
use serde_json::json;

/// .text 内容（0x100 字节；基址 rva 0x1000）：
/// 0x1000 call 0x1008；0x1005 nop×3；0x1008 mov eax,imm32；
/// 0x100D mov rax,[rip→0x20F0]；0x1014 call [rip→IAT 0x20B0]；0x101A ret；
/// 0x1020 nop×8 + jmp→0x200F（界外）；其余 0xCC 填充（无主区素材）。
fn fixture_text() -> Vec<u8> {
    let mut b = vec![0xCC; 0x100];
    b[0x00..0x05].copy_from_slice(&[0xE8, 0x03, 0x00, 0x00, 0x00]);
    b[0x05..0x08].copy_from_slice(&[0x90, 0x90, 0x90]);
    b[0x08..0x0D].copy_from_slice(&[0xB8, 0x8D, 0x7C, 0x6B, 0x5A]);
    b[0x0D..0x14].copy_from_slice(&[0x48, 0x8B, 0x05, 0xDC, 0x10, 0x00, 0x00]);
    b[0x14..0x1A].copy_from_slice(&[0xFF, 0x15, 0x96, 0x10, 0x00, 0x00]);
    b[0x1A] = 0xC3;
    b[0x20..0x28].fill(0x90);
    // 0x1028: jmp rel32 → 0x200F（.text 界外——disasm 的 outside 目标形态；
    // rel = 0x200F - 0x102D = 0xFE2）
    b[0x28..0x2D].copy_from_slice(&[0xE9, 0xE2, 0x0F, 0x00, 0x00]);
    b
}

/// .rdata 内容（0x200 字节）：导出目录 + 导入描述符 + IAT/OFT + 字符串。
fn fixture_rdata() -> Vec<u8> {
    let mut b = vec![0u8; 0x200];
    let w32 = |buf: &mut [u8], off: usize, v: u32| {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    };
    // 导出目录（单导出 ziwei_test_fn → 0x1008）
    w32(&mut b, 0x0C, 0x2040); // name rva
    w32(&mut b, 0x14, 1); // num funcs
    w32(&mut b, 0x18, 1); // num names
    w32(&mut b, 0x1C, 0x2060); // addr funcs
    w32(&mut b, 0x20, 0x2068); // addr names
    w32(&mut b, 0x24, 0x2070); // addr ords
    b[0x40..0x49].copy_from_slice(b"test.dll\0");
    b[0x50..0x5E].copy_from_slice(b"ziwei_test_fn\0");
    w32(&mut b, 0x60, 0x1008);
    w32(&mut b, 0x68, 0x2050);
    w32(&mut b, 0x70, 0);
    // 导入描述符（USER32.dll!MessageBoxA，IAT 0x20B0）
    w32(&mut b, 0x80, 0x20CC); // OFT
    w32(&mut b, 0x8C, 0x20C0); // dll name
    w32(&mut b, 0x90, 0x20B0); // IAT
    b[0xB0..0xB8].copy_from_slice(&0x20DCu64.to_le_bytes());
    b[0xCC..0xD4].copy_from_slice(&0x20DCu64.to_le_bytes());
    b[0xC0..0xCA].copy_from_slice(b"USER32.dll");
    b[0xDE..0xDE + 12].copy_from_slice(b"MessageBoxA\0");
    b[0xF0..0x102].copy_from_slice(b"ZIWEI_TEST_STRING\0");
    b
}

/// 完整 PE 文件字节（DOS + PE + 2 节头；.text@0x200 / .rdata@0x400）。
fn fixture_pe_file() -> Vec<u8> {
    let mut f = vec![0u8; 0x600];
    f[0] = b'M';
    f[1] = b'Z';
    f[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    f[0x80..0x84].copy_from_slice(b"PE\0\0");
    f[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
    f[0x86..0x88].copy_from_slice(&2u16.to_le_bytes());
    f[0x94..0x96].copy_from_slice(&0xF0u16.to_le_bytes());
    f[0x96..0x98].copy_from_slice(&0x0022u16.to_le_bytes());
    let opt = 0x98usize;
    f[opt..opt + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
    f[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes());
    f[opt + 24..opt + 32].copy_from_slice(&0x140000000u64.to_le_bytes());
    f[opt + 32..opt + 36].copy_from_slice(&0x1000u32.to_le_bytes());
    f[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes());
    let dd = opt + 112;
    f[dd..dd + 4].copy_from_slice(&0x2000u32.to_le_bytes()); // export dir
    f[dd + 4..dd + 8].copy_from_slice(&0x40u32.to_le_bytes());
    f[dd + 8..dd + 12].copy_from_slice(&0x2080u32.to_le_bytes()); // import dir
    f[dd + 12..dd + 16].copy_from_slice(&0x28u32.to_le_bytes());
    let sect = |f: &mut [u8],
                base: usize,
                name: &[u8; 8],
                vs: u32,
                rva: u32,
                raws: u32,
                ptr: u32,
                ch: u32| {
        f[base..base + 8].copy_from_slice(name);
        f[base + 8..base + 12].copy_from_slice(&vs.to_le_bytes());
        f[base + 12..base + 16].copy_from_slice(&rva.to_le_bytes());
        f[base + 16..base + 20].copy_from_slice(&raws.to_le_bytes());
        f[base + 20..base + 24].copy_from_slice(&ptr.to_le_bytes());
        f[base + 36..base + 40].copy_from_slice(&ch.to_le_bytes());
    };
    let sh = 0x188usize;
    sect(
        &mut f,
        sh,
        b".text\0\0\0",
        0x1000,
        0x1000,
        0x200,
        0x200,
        0x6000_0020,
    );
    sect(
        &mut f,
        sh + 40,
        b".rdata\0\0",
        0x1000,
        0x2000,
        0x200,
        0x400,
        0x4000_0040,
    );
    f[0x200..0x300].copy_from_slice(&fixture_text());
    f[0x400..0x600].copy_from_slice(&fixture_rdata());
    f
}

/// 建带 exe_path_hint 的 draft + 盘上夹具 exe + 最新快照（analyze 起点三件套）。
fn seed_analyze_env(ws: &std::path::Path, id: &str) -> PathBuf {
    let exe = ws.join("dummy.exe");
    let file = fixture_pe_file();
    std::fs::write(&exe, &file).unwrap();

    let now = chrono::Utc::now();
    let profile: Profile = serde_json::from_slice(
        &serde_json::to_vec(&serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "profile_id": id,
            "name": "draft: dummy.exe",
            "created_at": now.to_rfc3339(),
            "last_verified_at": now.to_rfc3339(),
            "status": "active",
            "target": {
                "exe_name": "dummy.exe",
                "exe_path_hint": exe.display().to_string(),
                "machine": "x64",
                "file_sha256": format!("{:064x}", 7u32),
                "verify_policy": "hash_or_version",
            },
            "verification": {
                "on_attach": ["file_sha256"],
                "stale_action": "notify_user",
            },
        }))
        .unwrap(),
    )
    .unwrap();
    init_draft_files(ws, &profile).unwrap();

    let pe = nemesis_ziwei_rev::pe::PeInfo::parse(&file).unwrap();
    let meta = SnapshotMeta {
        profile_id: id.to_string(),
        ts_rfc3339: "2026-11-01T00:00:00Z".to_string(),
        pid: 7,
        module_base: 0x140000000,
        machine: pe.machine,
        exe_path: exe.display().to_string(),
        sections: pe.sections.clone(),
    };
    let store = SnapshotStore::new(ws, id);
    store
        .save(
            &meta,
            &file,
            &[
                (".text".to_string(), fixture_text()),
                (".rdata".to_string(), fixture_rdata()),
            ],
            1_000,
        )
        .unwrap();
    exe
}

/// analyze 夹具上跑完整分析，返回 profile_id（内存缓存已建立）。
fn analyzed(ws: &std::path::Path, id: &str) {
    let mgr = SetupManager::new();
    let out = mgr.analyze(ws).expect("analyze 离线全链路应成功");
    assert!(
        out["functions"].as_u64().unwrap() >= 2,
        "至少两个函数: {out}"
    );
    assert!(out["seeds_by_source"].is_object());
    // analysis.json 落盘
    assert!(draft_dir(ws, id).join("analysis.json").exists());
}

#[test]
fn analyze_error_paths_without_snapshot_and_bad_headers() {
    let ws = temp_ws("ana-err");
    let id = "zw-pro-dana1";
    // 无 draft：require_info 直接失败
    let mgr = SetupManager::new();
    assert!(mgr.analyze(&ws).is_err(), "无 draft = 无向导");
    // 有 draft 无快照：诚实报「无快照」
    seed_draft(&ws, id);
    let err = mgr.analyze(&ws).unwrap_err();
    assert!(err.contains("无快照"), "实际：{err}");
    cleanup(&ws);
}

#[test]
fn analyze_rejects_bad_and_elf_snapshot_headers() {
    let ws = temp_ws("ana-hdr");
    let id = "zw-pro-dana2";
    seed_analyze_env(&ws, id);
    let store = SnapshotStore::new(&ws, id);
    let exe = ws.join("dummy.exe");
    // 垃圾头：BinHeaders::parse 诚实报错
    let meta_bad = SnapshotMeta {
        profile_id: id.to_string(),
        ts_rfc3339: "t".to_string(),
        pid: 1,
        module_base: 0,
        machine: 0x8664,
        exe_path: exe.display().to_string(),
        sections: vec![],
    };
    store
        .save(
            &meta_bad,
            b"garbage-header",
            &[(".text".to_string(), fixture_text())],
            2_000,
        )
        .unwrap();
    let mgr = SetupManager::new();
    let err = mgr.analyze(&ws).unwrap_err();
    assert!(
        err.contains("快照头解析失败") || err.contains("魔数"),
        "实际：{err}"
    );

    // ELF 头：魔数分派识别为 Elf，但 arch 归一先于容器臂拒绝（machine 0）——
    // 诚实报错，容器解析器未启用（顺带钉住 arch 闸先行的现行为）
    let mut elf = vec![0u8; 64];
    elf[0..4].copy_from_slice(b"\x7fELF");
    store
        .save(
            &meta_bad,
            &elf,
            &[(".text".to_string(), fixture_text())],
            3_000,
        )
        .unwrap();
    let err = mgr.analyze(&ws).unwrap_err();
    assert!(err.contains("machine 0x0000"), "实际：{err}");

    // 盘上 exe 无可执行节（.text 特征位改数据节）→ 可执行节界缺席，诚实报错
    //（镜像节字节缺席时 analyze 会以 0 函数「成功」——节界权威在 exe 头表，
    // 此处钉的是 exe 头表无 exec 节的拒绝路径）
    let mut no_exec = fixture_pe_file();
    let sh0 = 0x188usize;
    no_exec[sh0 + 36..sh0 + 40].copy_from_slice(&0x4000_0040u32.to_le_bytes()); // .text → 数据节
    let pe_ne = nemesis_ziwei_rev::pe::PeInfo::parse(&no_exec).unwrap();
    std::fs::write(&exe, &no_exec).unwrap();
    let meta_ne = SnapshotMeta {
        profile_id: id.to_string(),
        ts_rfc3339: "t".to_string(),
        pid: 1,
        module_base: 0,
        machine: 0x8664,
        exe_path: exe.display().to_string(),
        sections: pe_ne.sections.clone(),
    };
    store
        .save(
            &meta_ne,
            &no_exec,
            &[(".text".to_string(), fixture_text())],
            4_000,
        )
        .unwrap();
    let err = mgr.analyze(&ws).unwrap_err();
    assert!(err.contains("可执行节"), "实际：{err}");
    cleanup(&ws);
}

#[test]
fn query_disasm_reports_targets_and_body_note() {
    let ws = temp_ws("q-disasm");
    let id = "zw-pro-dq1";
    seed_analyze_env(&ws, id);
    analyzed(&ws, id);
    let mgr = SetupManager::new();

    // 函数起点：call 目标 0x1008 标 func。0x1008 同时是导出种子 → 独立函数
    // 起点，0x1000 的线性体在其边界收口（call + nop×2 共 3 条）——种子分片
    // 是 analyze 的真实行为，断言按实际恢复结果钉。
    let out = mgr.query_disasm(&ws, 0x1000, 16).unwrap();
    assert_eq!(out["func_rva"], json_str(0x1000));
    assert_eq!(
        out["total_insns"].as_u64().unwrap(),
        3,
        "call+nop×2 在 0x1008 边界收口: {out}"
    );
    let insns = out["instructions"].as_array().unwrap();
    assert_eq!(insns[0]["m"], "call");
    assert!(insns[0]["bytes"].as_str().unwrap().starts_with("e8"));
    let target = &insns[0]["target"];
    assert_eq!(target["rva"], json_str(0x1008));
    assert_eq!(target["kind"], "func", "0x1008 已识别为函数: {target}");
    assert_eq!(insns[1]["m"], "nop", "前导 nop 入列: {insns:#?}");

    // 导出函数体：mov/mov/call/ret 四条（间接 call [rip] 无直接转移目标行）
    let out = mgr.query_disasm(&ws, 0x1008, 16).unwrap();
    assert_eq!(out["func_rva"], json_str(0x1008));
    assert_eq!(out["total_insns"].as_u64().unwrap(), 4, "{out}");
    assert_eq!(out["instructions"][0]["m"], "mov");
    assert_eq!(out["instructions"][2]["m"], "call");
    assert!(
        out["instructions"][2]["target"].is_null(),
        "间接调用 [rip] 无 branch_target 行"
    );

    // nopsled 函数尾 jmp：目标 0x2010 在 .text 界外 → outside 形态
    let out = mgr.query_disasm(&ws, 0x1020, 16).unwrap();
    assert_eq!(out["func_rva"], json_str(0x1020));
    let insns_out = out["instructions"].as_array().unwrap();
    let last = insns_out.last().unwrap();
    assert_eq!(last["m"], "jmp");
    assert_eq!(last["target"]["rva"], json_str(0x200F));
    assert_eq!(last["target"]["kind"], "outside", "界外直接转移: {last}");

    // 体内地址：非起点解析 + note
    let out = mgr.query_disasm(&ws, 0x1005, 4).unwrap();
    assert_eq!(out["func_rva"], json_str(0x1000));
    assert!(
        out["note"].as_str().unwrap_or("").contains("体内"),
        "实际: {out}"
    );

    // max_insns 钳制 + 截断注记
    let out = mgr.query_disasm(&ws, 0x1000, 2).unwrap();
    assert_eq!(out["count"].as_u64().unwrap(), 2);
    assert_eq!(out["truncated"], true);

    // 无主地址：诚实报错
    let err = mgr.query_disasm(&ws, 0x1040, 8).unwrap_err();
    assert!(err.contains("不在任何已识别函数"), "实际：{err}");
    cleanup(&ws);
}

fn json_str(v: u32) -> serde_json::Value {
    serde_json::Value::String(format!("0x{v:X}"))
}

#[test]
fn query_xref_to_three_kinds_and_errors() {
    let ws = temp_ws("q-xto");
    let id = "zw-pro-dq2";
    seed_analyze_env(&ws, id);
    analyzed(&ws, id);
    let mgr = SetupManager::new();

    // str 维度：字符串内容反查引用站点
    let out = mgr.query_xref_to(&ws, "str", "ZIWEI_TEST_STRING").unwrap();
    assert_eq!(out["total"].as_u64().unwrap(), 1, "{out}");
    let hits = out["hits"].as_array().unwrap();
    assert_eq!(hits[0]["site"], json_str(0x100D));

    // imm 维度：立即数值反查
    let out = mgr.query_xref_to(&ws, "imm", "0x5A6B7C8D").unwrap();
    assert_eq!(out["total"].as_u64().unwrap(), 1);
    assert_eq!(out["hits"][0]["site"], json_str(0x1008));

    // data 维度：数据 RVA 反查
    let out = mgr.query_xref_to(&ws, "data", "0x20F0").unwrap();
    assert_eq!(out["total"].as_u64().unwrap(), 1);

    // 无命中 + 非法入参
    let out = mgr.query_xref_to(&ws, "str", "NO_SUCH_STRING").unwrap();
    assert_eq!(out["total"].as_u64().unwrap(), 0);
    assert!(out["note"].as_str().unwrap_or("").contains("无引用站点"));
    assert!(mgr.query_xref_to(&ws, "imm", "zzz").is_err());
    assert!(mgr.query_xref_to(&ws, "data", "nothex").is_err());
    let err = mgr.query_xref_to(&ws, "boom", "x").unwrap_err();
    assert!(err.contains("非法"), "实际：{err}");
    cleanup(&ws);
}

#[test]
fn query_xref_from_callers_functions_and_gaps() {
    let ws = temp_ws("q-xfrom");
    let id = "zw-pro-dq3";
    seed_analyze_env(&ws, id);
    analyzed(&ws, id);
    let mgr = SetupManager::new();

    // xref_from：函数引用的 import/字符串/立即数清单（引用点都在导出函数体）
    let out = mgr.query_xref_from(&ws, 0x1008).unwrap();
    let imports = out["imports"].as_array().unwrap();
    assert!(
        imports
            .iter()
            .any(|v| v.as_str().unwrap_or("").contains("MessageBoxA")),
        "{out}"
    );
    let strings = out["strings"].as_array().unwrap();
    assert!(strings.iter().any(|s| {
        s["text"]
            .as_str()
            .unwrap_or("")
            .starts_with("ZIWEI_TEST_STRING")
    }));
    let imms = out["imm_consts"].as_array().unwrap();
    assert!(imms.iter().any(|v| v.as_str() == Some("0x5A6B7C8D")));

    // callers：0x1000 无人调用 → 空注记；0x1008 被 0x1000 的 call 命中
    let out = mgr.query_callers(&ws, 0x1000).unwrap();
    assert_eq!(out["total"].as_u64().unwrap(), 0);
    assert!(out["note"].as_str().unwrap_or("").contains("无直接调用方"));
    let out = mgr.query_callers(&ws, 0x1008).unwrap();
    assert_eq!(out["total"].as_u64().unwrap(), 1, "{out}");
    assert_eq!(out["callers"][0]["rva"], json_str(0x1000));

    // functions 表：非法 sort 拒绝；size 排序 + name 过滤
    assert!(mgr.query_functions(&ws, "bogus", 10, None).is_err());
    let out = mgr.query_functions(&ws, "size", 10, None).unwrap();
    assert!(out["total"].as_u64().unwrap() >= 2);
    let out = mgr.query_functions(&ws, "refs", 1, Some("0x1000")).unwrap();
    assert_eq!(out["count"].as_u64().unwrap(), 1, "rva 子串过滤: {out}");

    // gaps：.text 未覆盖区间（函数没铺满 0x1100 界）
    let out = mgr.query_gaps(&ws).unwrap();
    assert!(out["total_gaps"].as_u64().unwrap() >= 1, "{out}");
    assert!(out["text_bounds"].as_str().unwrap().contains("0x1000"));
    cleanup(&ws);
}

#[test]
fn query_search_all_five_kinds() {
    let ws = temp_ws("q-search");
    let id = "zw-pro-dq4";
    seed_analyze_env(&ws, id);
    analyzed(&ws, id);
    let mgr = SetupManager::new();

    // bytes：nopsled 命中 + 通配 nibble
    let out = mgr.query_search(&ws, "bytes", "9090909090").unwrap();
    assert!(out["count"].as_u64().unwrap() >= 1, "{out}");
    assert_eq!(out["hits"][0]["section"], ".text");
    let out = mgr.query_search(&ws, "bytes", "b8 ?? 7c 6b 5a").unwrap();
    assert!(out["count"].as_u64().unwrap() >= 1, "半通配命中: {out}");
    // string / imm / name（export+import）/ insn
    let out = mgr
        .query_search(&ws, "string", "ZIWEI_TEST_STRING")
        .unwrap();
    assert_eq!(out["count"].as_u64().unwrap(), 1, "{out}");
    let out = mgr.query_search(&ws, "imm", "0x5A6B7C8D").unwrap();
    assert_eq!(out["total"].as_u64().unwrap(), 1);
    let out = mgr.query_search(&ws, "name", "ziwei_test").unwrap();
    assert!(out["total"].as_u64().unwrap() >= 1, "export 命中: {out}");
    let out = mgr.query_search(&ws, "name", "user32").unwrap();
    assert!(out["total"].as_u64().unwrap() >= 1, "import 命中: {out}");
    let out = mgr.query_search(&ws, "insn", "mov").unwrap();
    assert!(out["count"].as_u64().unwrap() >= 2, "{out}");
    // 错误面：空查询 / 未知 kind / 空名
    assert!(mgr.query_search(&ws, "bytes", "").is_err());
    assert!(mgr.query_search(&ws, "bytes", "abc").is_err());
    assert!(mgr.query_search(&ws, "name", "  ").is_err());
    assert!(mgr.query_search(&ws, "insn", "").is_err());
    let err = mgr.query_search(&ws, "boom", "x").unwrap_err();
    assert!(err.contains("非法"), "实际：{err}");
    cleanup(&ws);
}

#[test]
fn query_read_data_clamps_and_errors() {
    let ws = temp_ws("q-read");
    let id = "zw-pro-dq5";
    seed_analyze_env(&ws, id);
    analyzed(&ws, id);
    let mgr = SetupManager::new();

    let out = mgr.query_read_data(&ws, 0x20F0, 8, false).unwrap();
    assert!(
        out["data"]
            .as_str()
            .unwrap()
            .starts_with(&hex::encode(b"ZIWEI_TE"))
    );
    let out = mgr.query_read_data(&ws, 0x20F0, 17, true).unwrap();
    assert_eq!(out["as"], "utf8");
    assert!(
        out["data"]
            .as_str()
            .unwrap()
            .starts_with("ZIWEI_TEST_STRING")
    );
    // size 钳制：0 → 1
    let out = mgr.query_read_data(&ws, 0x20F0, 0, false).unwrap();
    assert_eq!(out["size"].as_u64().unwrap(), 1);
    // 越界地址：诚实报错
    let err = mgr.query_read_data(&ws, 0x9999, 4, false).unwrap_err();
    assert!(err.contains("不在快照节内"), "实际：{err}");
    cleanup(&ws);
}

#[test]
fn query_pe_projections_all_seven_kinds() {
    let ws = temp_ws("q-pe");
    let id = "zw-pro-dq6";
    seed_analyze_env(&ws, id);
    analyzed(&ws, id);
    let mgr = SetupManager::new();

    let out = mgr.query_pe(&ws, "sections").unwrap();
    assert_eq!(out["count"].as_u64().unwrap(), 2);
    let out = mgr.query_pe(&ws, "exports").unwrap();
    assert_eq!(out["total"].as_u64().unwrap(), 1);
    assert_eq!(out["exports"][0]["name"], "ziwei_test_fn");
    let out = mgr.query_pe(&ws, "imports").unwrap();
    assert_eq!(out["total"].as_u64().unwrap(), 1);
    let out = mgr.query_pe(&ws, "pdata").unwrap();
    assert!(out["pdata"].is_null(), "夹具无异常目录: {out}");
    assert!(out["note"].as_str().unwrap_or("").contains("无 .pdata"));
    let out = mgr.query_pe(&ws, "tls").unwrap();
    assert!(out["note"].as_str().unwrap_or("").contains("无 TLS"));
    let out = mgr.query_pe(&ws, "debug").unwrap();
    assert!(out["note"].as_str().unwrap_or("").contains("无 Debug"));
    let out = mgr.query_pe(&ws, "rich").unwrap();
    assert!(out["note"].as_str().unwrap_or("").contains("无 Rich"));
    let err = mgr.query_pe(&ws, "boom").unwrap_err();
    assert!(err.contains("非法"), "实际：{err}");
    cleanup(&ws);
}

#[test]
fn query_locate_status_matrix_and_candidates() {
    let ws = temp_ws("q-loc");
    let id = "zw-pro-dq7";
    seed_analyze_env(&ws, id);
    analyzed(&ws, id);
    let mgr = SetupManager::new();

    // func / body / unowned / outside 四态
    let out = mgr.query_locate(&ws, 0x1000).unwrap();
    assert_eq!(out["status"], "func");
    let out = mgr.query_locate(&ws, 0x1005).unwrap();
    assert_eq!(out["status"], "body");
    let out = mgr.query_locate(&ws, 0x1060).unwrap();
    assert_eq!(out["status"], "unowned", "0xCC 填充区无主: {out}");
    let out = mgr.query_locate(&ws, 0x20F0).unwrap();
    assert_eq!(out["status"], "outside");

    // candidates：nopsled / imm / str 三类候选（import 序列阈值不达）
    let out = mgr.candidates(&ws).unwrap();
    let list = out["candidates"].as_array().unwrap();
    assert!(list.iter().any(|c| c["kind"] == "byte_pattern"), "{out}");
    assert!(list.iter().any(|c| c["kind"] == "imm_const"));
    assert!(list.iter().any(|c| c["kind"] == "str_ref"));

    // 语义名投影：annotate 后查询面带名字
    mgr.annotate(
        &ws,
        &serde_json::json!({
            "rva": 0x1000u64,
            "semantic": "fixture_entry",
            "note": "夹具入口",
            "anchors": [{"kind": "imm_const", "value": 1518500249u64}],
        }),
    )
    .unwrap();
    let out = mgr.query_locate(&ws, 0x1000).unwrap();
    assert_eq!(out["func"]["name"], "fixture_entry", "语义名投影: {out}");
    let out = mgr
        .query_functions(&ws, "size", 10, Some("fixture"))
        .unwrap();
    assert_eq!(out["count"].as_u64().unwrap(), 1);
    cleanup(&ws);
}

#[test]
fn commit_migrates_snapshots_into_profile_store() {
    let ws = temp_ws("cmt-snap");
    let id = "zw-pro-dq8";
    seed_analyze_env(&ws, id);
    analyzed(&ws, id);
    let mgr = SetupManager::new();
    mgr.annotate(
        &ws,
        &serde_json::json!({
            "rva": 0x1000u64,
            "semantic": "fixture_entry",
            "anchors": [{"kind": "imm_const", "value": 1518500249u64}],
        }),
    )
    .unwrap();

    // setup 快照目录在场（analyze_env 落的快照）→ commit 迁移到 profile 名下
    let setup_snaps = ws.join("ziwei").join("setup").join(id).join("snapshots");
    assert!(setup_snaps.is_dir());
    let out = mgr.commit(&ws).unwrap();
    assert_eq!(out["committed"], id);
    assert!(
        out["cleanup_warnings"].as_array().unwrap().is_empty(),
        "迁移成功无警告: {out}"
    );
    assert!(!setup_snaps.exists(), "setup 快照已迁走");
    let store = ProfileStore::new(&ws);
    assert!(store.snapshots_dir(id).is_dir(), "快照落位 profile 名下");
    cleanup(&ws);
}

#[test]
fn commit_reports_snapshot_migration_failure_in_warnings() {
    let ws = temp_ws("cmt-snapfail");
    let id = "zw-pro-dq9";
    seed_analyze_env(&ws, id);
    analyzed(&ws, id);
    let mgr = SetupManager::new();
    mgr.annotate(
        &ws,
        &serde_json::json!({
            "rva": 0x1000u64,
            "semantic": "fixture_entry",
            "anchors": [{"kind": "imm_const", "value": 1518500249u64}],
        }),
    )
    .unwrap();

    // 失败注入：目标快照位预置非空目录（rename 目录→非空目录必败）→
    // 迁移警告（不假失败）
    let store = ProfileStore::new(&ws);
    let target = store.snapshots_dir(id);
    std::fs::create_dir_all(target.join("occupied")).unwrap();
    std::fs::write(target.join("occupied").join("blocker.bin"), b"x").unwrap();

    let out = mgr.commit(&ws).unwrap();
    assert_eq!(out["committed"], id, "落盘本体不受迁移失败影响");
    let warnings = out["cleanup_warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("快照迁移失败")),
        "{warnings:?}"
    );
    cleanup(&ws);
}

// ── annotate / define_struct / define_tool 校验矩阵 ──────────────────────

fn seeded_mgr(ws: &std::path::Path, id: &str) -> SetupManager {
    seed_draft(ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);
    mgr
}

#[test]
fn annotate_rejects_bad_rva_semantic_conv_and_params() {
    let ws = temp_ws("ann-bad");
    let id = "zw-pro-ann1";
    let mgr = seeded_mgr(&ws, id);

    // rva 缺失 / 超 32 位
    let err = mgr.annotate(&ws, &serde_json::json!({"semantic": "x", "anchors": [{"kind": "imm_const", "value": 1u64}]})).unwrap_err();
    assert!(err.contains("缺 rva"), "{err}");
    let err = mgr.annotate(&ws, &serde_json::json!({"rva": 4294967296u64, "semantic": "x", "anchors": [{"kind": "imm_const", "value": 1u64}]})).unwrap_err();
    assert!(err.contains("超出 32 位"), "{err}");

    // semantic 缺失 / 空 / 过长
    let body =
        serde_json::json!({"rva": 0x10u64, "anchors": [{"kind": "imm_const", "value": 1u64}]});
    assert!(
        mgr.annotate(&ws, &body)
            .unwrap_err()
            .contains("缺 semantic")
    );
    let body = serde_json::json!({"rva": 0x10u64, "semantic": "   ", "anchors": [{"kind": "imm_const", "value": 1u64}]});
    assert!(
        mgr.annotate(&ws, &body)
            .unwrap_err()
            .contains("缺 semantic")
    );
    let long = "a".repeat(65);
    let body = serde_json::json!({"rva": 0x10u64, "semantic": long, "anchors": [{"kind": "imm_const", "value": 1u64}]});
    assert!(mgr.annotate(&ws, &body).unwrap_err().contains("过长"));

    // conv 非法
    let body = serde_json::json!({"rva": 0x10u64, "semantic": "f", "conv": "vectorcall", "anchors": [{"kind": "imm_const", "value": 1u64}]});
    assert!(mgr.annotate(&ws, &body).unwrap_err().contains("conv"));

    // anchors 缺失 / 未知 kind / bb_hash 未实装
    let body = serde_json::json!({"rva": 0x10u64, "semantic": "f"});
    assert!(mgr.annotate(&ws, &body).unwrap_err().contains("缺 anchors"));
    let body = serde_json::json!({"rva": 0x10u64, "semantic": "f", "anchors": [{"kind": "magic"}]});
    let err = mgr.annotate(&ws, &body).unwrap_err();
    assert!(err.contains("未知"), "{err}");
    let body =
        serde_json::json!({"rva": 0x10u64, "semantic": "f", "anchors": [{"kind": "bb_hash"}]});
    assert!(mgr.annotate(&ws, &body).unwrap_err().contains("未实装"));

    // 载荷校验：byte_pattern 缺 pattern / str_ref 缺 text / imm 缺 value / import 缺 imports
    let mk = |anchor: serde_json::Value, rva: u64| serde_json::json!({"rva": rva, "semantic": format!("f_{rva:x}"), "anchors": [anchor]});
    let e = mgr
        .annotate(&ws, &mk(json!({"kind": "byte_pattern"}), 0x20))
        .unwrap_err();
    assert!(e.contains("缺 pattern"), "{e}");
    let e = mgr
        .annotate(&ws, &mk(json!({"kind": "str_ref"}), 0x21))
        .unwrap_err();
    assert!(e.contains("str_ref 缺 text"), "{e}");
    let e = mgr
        .annotate(&ws, &mk(json!({"kind": "imm_const"}), 0x22))
        .unwrap_err();
    assert!(e.contains("imm_const 缺 value"), "{e}");
    let e = mgr
        .annotate(&ws, &mk(json!({"kind": "import_call_seq"}), 0x23))
        .unwrap_err();
    assert!(e.contains("import_call_seq 缺 imports"), "{e}");
    cleanup(&ws);
}

#[test]
fn annotate_params_and_conv_variants_and_anchor_payloads_ok() {
    let ws = temp_ws("ann-ok");
    let id = "zw-pro-ann2";
    let mgr = seeded_mgr(&ws, id);

    // params（合法 reg/ty/desc）+ stdcall 调用约定 + str_ref / import_call_seq 锚
    let body = serde_json::json!({
        "rva": 0x30u64,
        "semantic": "fn_params",
        "conv": "stdcall",
        "note": "函数注记",
        "params": [
            {"reg": "rcx", "ty": "u64", "desc": "玩家指针"},
            {"reg": "edx", "ty": "f32"},
            {"reg": "stack:8", "desc": "栈槽"},
        ],
        "anchors": [
            {"kind": "str_ref", "text": "player"},
            {"kind": "import_call_seq", "imports": "USER32.dll!MessageBoxA"},
        ],
    });
    let out = mgr.annotate(&ws, &body).unwrap();
    assert_eq!(out["params"].as_u64().unwrap(), 3);
    let profile = load_draft(&ws, id).unwrap();
    let f = profile
        .functions
        .iter()
        .find(|f| f.semantic == "fn_params")
        .unwrap();
    assert_eq!(
        f.locate.kind,
        nemesis_ziwei_profile::LocateKind::Rva,
        "无 byte_pattern 锚 → RVA 回落"
    );
    assert_eq!(f.args.len(), 3);
    assert_eq!(f.args[0].ty.as_deref(), Some("u64"));
    // 函数级注记级联到无注记锚
    let anchors = super::load_draft_anchors(&ws, id).unwrap();
    assert!(
        anchors
            .anchors
            .iter()
            .all(|a| a.note.as_deref() == Some("函数注记")),
        "{anchors:?}"
    );

    // params 超上限 / 缺 reg / 坏 ty
    let mk_body = |params: serde_json::Value| {
        serde_json::json!({"rva": 0x40u64, "semantic": "fn_bad_params", "params": params,
            "anchors": [{"kind": "imm_const", "value": 2u64}]})
    };
    let many: Vec<_> = (0..17)
        .map(|i| serde_json::json!({"reg": format!("r{i}")}))
        .collect();
    let e = mgr.annotate(&ws, &mk_body(json!(many))).unwrap_err();
    assert!(e.contains("超上限"), "{e}");
    let e = mgr
        .annotate(&ws, &mk_body(json!([{"ty": "u64"}])))
        .unwrap_err();
    assert!(e.contains("缺 reg"), "{e}");
    let e = mgr
        .annotate(&ws, &mk_body(json!([{"reg": "rcx", "ty": "magic_t"}])))
        .unwrap_err();
    assert!(e.contains("非法"), "{e}");

    // cdecl 约定合法
    let body = serde_json::json!({"rva": 0x50u64, "semantic": "fn_cdecl", "conv": "cdecl",
        "anchors": [{"kind": "byte_pattern", "pattern": "b8 8d 7c 6b 5a"}]});
    mgr.annotate(&ws, &body).unwrap();
    cleanup(&ws);
}

#[test]
fn define_struct_and_tool_validation_matrix() {
    let ws = temp_ws("ds-bad");
    let id = "zw-pro-ds1";
    let mgr = seeded_mgr(&ws, id);

    // 结构：缺 name / size / fields；size 越界；字段缺 offset/name/type；decode 未知
    assert!(
        mgr.define_struct(&ws, &json!({"size": 8, "fields": []}))
            .unwrap_err()
            .contains("缺 name")
    );
    assert!(
        mgr.define_struct(&ws, &json!({"name": "A", "fields": []}))
            .unwrap_err()
            .contains("缺 size")
    );
    assert!(
        mgr.define_struct(&ws, &json!({"name": "A", "size": 8}))
            .unwrap_err()
            .contains("缺 fields")
    );
    assert!(
        mgr.define_struct(&ws, &json!({"name": "A", "size": 0, "fields": []}))
            .unwrap_err()
            .contains("越界")
    );
    assert!(
        mgr.define_struct(&ws, &json!({"name": "A", "size": 1 << 21, "fields": []}))
            .unwrap_err()
            .contains("越界")
    );
    let e = mgr
        .define_struct(
            &ws,
            &json!({"name": "A", "size": 8, "fields": [{"name": "v", "type": "u32"}]}),
        )
        .unwrap_err();
    assert!(e.contains("缺 offset"), "{e}");
    let e = mgr
        .define_struct(
            &ws,
            &json!({"name": "A", "size": 8, "fields": [{"offset": 0, "type": "u32"}]}),
        )
        .unwrap_err();
    assert!(e.contains("缺 name"), "{e}");
    let e = mgr
        .define_struct(
            &ws,
            &json!({"name": "A", "size": 8, "fields": [{"offset": 0, "name": "v"}]}),
        )
        .unwrap_err();
    assert!(e.contains("缺 type"), "{e}");
    let e = mgr.define_struct(&ws, &json!({"name": "A", "size": 8, "fields": [{"offset": 0, "name": "v", "type": "u32", "decode": "rot13"}]})).unwrap_err();
    assert!(e.contains("decode"), "{e}");

    // decode 全枚举合法 + 覆盖式重定义
    let body = serde_json::json!({"name": "Rec", "size": 64, "fields": [
        {"offset": 0, "name": "a", "type": "u32", "decode": "utf8"},
        {"offset": 4, "name": "b", "type": "u32", "decode": "utf16"},
        {"offset": 8, "name": "c", "type": "u32", "decode": "unix_ms"},
        {"offset": 12, "name": "d", "type": "u32", "decode": "unix_sec"},
        {"offset": 16, "name": "e", "type": "u32", "decode": "hex"},
    ]});
    mgr.define_struct(&ws, &body).unwrap();
    mgr.define_struct(
        &ws,
        &serde_json::json!({"name": "Rec", "size": 32, "fields": [
            {"offset": 0, "name": "only", "type": "u64"},
        ]}),
    )
    .unwrap();
    let profile = load_draft(&ws, id).unwrap();
    assert_eq!(profile.data_structures.len(), 1, "重定义覆盖不追加");
    assert_eq!(profile.data_structures[0].size, 32);

    // 工具：缺 tool_name / action / primitive / addr；未知 primitive/mode/run_policy；坏 notify
    let e = mgr
        .define_tool(
            &ws,
            &json!({"action": {"primitive": "poll_report", "addr": "struct:Rec"}}),
        )
        .unwrap_err();
    assert!(e.contains("缺 tool_name"), "{e}");
    let e = mgr
        .define_tool(&ws, &json!({"tool_name": "t"}))
        .unwrap_err();
    assert!(e.contains("缺 action"), "{e}");
    let e = mgr
        .define_tool(
            &ws,
            &json!({"tool_name": "t", "action": {"addr": "struct:Rec"}}),
        )
        .unwrap_err();
    assert!(e.contains("缺 primitive"), "{e}");
    let e = mgr
        .define_tool(
            &ws,
            &json!({"tool_name": "t", "action": {"primitive": "magic", "addr": "struct:Rec"}}),
        )
        .unwrap_err();
    assert!(e.contains("未知"), "{e}");
    let e = mgr
        .define_tool(
            &ws,
            &json!({"tool_name": "t", "action": {"primitive": "poll_report"}}),
        )
        .unwrap_err();
    assert!(e.contains("缺 addr"), "{e}");
    let e = mgr.define_tool(&ws, &json!({"tool_name": "t", "action": {"primitive": "poll_report", "addr": "struct:Rec", "mode": "sometimes"}})).unwrap_err();
    assert!(e.contains("mode"), "{e}");
    let e = mgr.define_tool(&ws, &json!({"tool_name": "t", "action": {"primitive": "poll_report", "addr": "struct:Rec"}, "run_policy": "yolo"})).unwrap_err();
    assert!(e.contains("run_policy"), "{e}");
    let e = mgr.define_tool(&ws, &json!({"tool_name": "t", "action": {"primitive": "poll_report", "addr": "struct:Rec", "notify": {"boom": 1}}})).unwrap_err();
    assert!(e.contains("notify"), "{e}");

    // 显式 run_policy auto + 覆盖式重定义 + output/input_schema 透传
    let tool = serde_json::json!({
        "tool_name": "watcher",
        "description": "轮询",
        "input_schema": {"type": "object"},
        "action": {"primitive": "poll_report", "addr": "struct:Rec", "mode": "every", "interval_secs": 5, "max_records": 100},
        "output": {"dest": "ziwei/runs/watch.json"},
        "run_policy": "auto",
    });
    mgr.define_tool(&ws, &tool).unwrap();
    mgr.define_tool(&ws, &tool).unwrap();
    let profile = load_draft(&ws, id).unwrap();
    assert_eq!(profile.tools.len(), 1, "重定义覆盖不追加");
    cleanup(&ws);
}

// ── abort setup 目录失败臂 + 查询辅助纯函数 ───────────────────────────────

#[cfg(windows)]
#[tokio::test]
async fn abort_reports_setup_dir_cleanup_failure() {
    let ws = temp_ws("abort-setup");
    let id = "zw-pro-abt9";
    seed_draft(&ws, id);
    let mgr = SetupManager::new();
    inject_state(&mgr, id);

    // 失败注入：setup 目录内放持句柄文件（剥 DELETE 位）→ setup 目录清理必败
    //（draft 清理先行且成功 → 走到 setup 目录臂）
    let setup_dir = ws.join("ziwei").join("setup").join(id);
    std::fs::create_dir_all(&setup_dir).unwrap();
    let locked = setup_dir.join("locked.bin");
    std::fs::write(&locked, b"x").unwrap();
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(&locked)
        .unwrap();

    let err = mgr.abort(&ws).await.unwrap_err();
    assert!(err.contains("setup 目录清理失败"), "实际：{err}");
    assert!(setup_dir.exists(), "setup 目录保留待重试");

    drop(handle);
    let out = mgr.abort(&ws).await.unwrap();
    assert_eq!(out["aborted"], id, "句柄释放后重试收敛");
    cleanup(&ws);
}

#[test]
fn parse_helpers_hex_imm_nibble_pattern_and_match() {
    use super::{match_pattern, parse_byte_pattern_query, parse_hex_u32, parse_imm_u64};

    // parse_hex_u32：0x/0X 前缀、裸 hex、非法
    assert_eq!(parse_hex_u32("0x10"), Some(0x10));
    assert_eq!(parse_hex_u32("0Xff"), Some(0xff));
    assert_eq!(parse_hex_u32("10"), Some(0x10));
    assert_eq!(parse_hex_u32("zz"), None);
    assert_eq!(parse_hex_u32(""), None);

    // parse_imm_u64：hex 前缀 / 十进制 / hex 回落 / 非法
    assert_eq!(parse_imm_u64("0x10"), Some(16));
    assert_eq!(parse_imm_u64("16"), Some(16));
    assert_eq!(parse_imm_u64("ff"), Some(255), "十进制失败后 hex 回落");
    assert_eq!(parse_imm_u64("zz"), None);

    // 字节模式解析：全通配 / 半通配 / 非法字符 / 奇数长度 / 空模式
    let pat = parse_byte_pattern_query("AA ?? 4? ?F").unwrap();
    assert_eq!(
        pat,
        vec![(0xAA, 0xFF), (0x00, 0x00), (0x40, 0xF0), (0x0F, 0x0F)]
    );
    assert!(parse_byte_pattern_query("   ").unwrap_err().contains("空"));
    assert!(
        parse_byte_pattern_query("abc")
            .unwrap_err()
            .contains("偶数")
    );
    assert!(
        parse_byte_pattern_query("zz")
            .unwrap_err()
            .contains("非法字符")
    );

    // match_pattern：界内命中 / 半通配语义 / 越界保守 false
    // 4? = 高半字节 4（0x40, mask 0xF0）；?F = 低半字节 F（0x0F, mask 0x0F）
    let bytes = [0xAA, 0x00, 0x45, 0x2F];
    assert!(match_pattern(&bytes, 0, &pat));
    assert!(!match_pattern(&bytes, 1, &pat));
    assert!(
        !match_pattern(&bytes, 2, &pat),
        "off+pat.len 越界必须 false"
    );
    let exact = parse_byte_pattern_query("aa00452f").unwrap();
    assert!(match_pattern(&bytes, 0, &exact));
    assert!(!match_pattern(&bytes, 1, &exact));
}

#[test]
fn text_bounds_resolve_func_and_semantic_names_projection() {
    use super::{resolve_func, semantic_names, text_bounds};
    use nemesis_ziwei_rev::funcs::{self as rev_funcs};

    let ws = temp_ws("tb");
    let id = "zw-pro-dtb1";
    seed_analyze_env(&ws, id);
    analyzed(&ws, id);
    let mgr = SetupManager::new();
    mgr.annotate(
        &ws,
        &serde_json::json!({
            "rva": 0x1000u64,
            "semantic": "named_fn",
            "anchors": [{"kind": "imm_const", "value": 1u64}],
        }),
    )
    .unwrap();

    // text_bounds：首个可执行节界（可从缓存读回——直跑 analyze 重建独立镜像）
    let file = fixture_pe_file();
    let pe = nemesis_ziwei_rev::pe::PeInfo::parse(&file).unwrap();
    let img = nemesis_ziwei_rev::pe::ImageMap::new(
        pe,
        vec![
            (".text".to_string(), fixture_text()),
            (".rdata".to_string(), fixture_rdata()),
        ],
    )
    .unwrap();
    assert_eq!(text_bounds(&img), Some((0x1000, 0x2000)));

    // resolve_func：起点 / 体内（带注记）/ 无主
    let seeds = [0x1000u32, 0x1020];
    let set = rev_funcs::recover(
        nemesis_ziwei_rev::disasm::Arch::X64,
        &img,
        &seeds,
        (0x1000, 0x1100),
    );
    let (f, note) = resolve_func(&set, 0x1000).unwrap();
    assert_eq!(f.rva, 0x1000);
    assert!(note.is_none());
    let (f, note) = resolve_func(&set, 0x1005).unwrap();
    assert_eq!(f.rva, 0x1000);
    assert!(note.as_deref().unwrap_or("").contains("体内"));
    assert!(resolve_func(&set, 0x1060).is_err(), "无主地址诚实报错");

    // semantic_names：draft 标注投影；draft 缺席 = 空表
    let names = semantic_names(&ws, id);
    assert_eq!(names.get(&0x1000).map(String::as_str), Some("named_fn"));
    assert!(semantic_names(&ws, "zw-pro-ghost").is_empty());
    cleanup(&ws);
}

// ── select_target / 进程定位（windows；不注入真实会话，到 attach 前闸止步）──

#[cfg(windows)]
#[tokio::test]
async fn select_target_requires_pid_or_exe_hint_and_recovers_reservation() {
    let ws = temp_ws("sel-args");
    let mgr = SetupManager::new();
    let err = mgr.select_target(&ws, None, None).await.unwrap_err();
    assert!(err.contains("缺 pid 或 exe"), "实际：{err}");
    // 失败回收预占：状态已清、无 draft 残留
    assert!(mgr.state.lock().unwrap().is_none(), "预占已回收");
    assert!(super::latest_draft_id(&ws).is_none(), "无 draft 残留");
    cleanup(&ws);
}

#[cfg(windows)]
#[tokio::test]
async fn select_target_rejects_unknown_process_name() {
    let ws = temp_ws("sel-name");
    let mgr = SetupManager::new();
    let err = mgr
        .select_target(&ws, None, Some("no-such-proc-xyz.exe"))
        .await
        .unwrap_err();
    assert!(err.contains("未找到进程"), "实际：{err}");
    assert!(mgr.state.lock().unwrap().is_none(), "预占已回收");
    cleanup(&ws);
}

#[cfg(windows)]
#[tokio::test]
async fn select_target_rejects_nonexistent_pid_at_module_snapshot() {
    let ws = temp_ws("sel-pid");
    let mgr = SetupManager::new();
    // 不存在的 pid：Toolhelp 模块快照诚实失败（不碰任何真实目标）
    let err = mgr
        .select_target(&ws, Some(0x7FFF_FFFF), None)
        .await
        .unwrap_err();
    assert!(
        err.contains("模块快照失败") || err.contains("快照失败"),
        "实际：{err}"
    );
    assert!(mgr.state.lock().unwrap().is_none(), "预占已回收");
    cleanup(&ws);
}

#[cfg(windows)]
#[tokio::test]
async fn select_target_own_process_end_to_end_offline() {
    let ws = temp_ws("sel-attach");
    let mgr = SetupManager::new();
    // 自身进程：pid 定位 + 模块枚举 + 盘上 PE 指纹 + draft 三件套全链路可走；
    // 会话管理器在 lib 测试进程内已被装配（session 模块测试装过全局槽）时
    // attach 直接成功（attach 对象 = 测试进程自身，无外部目标）；未装配时
    // 诚实报错并回滚。两种环境都收敛：成功 → abort 收尾（覆盖写回/快照面），
    // 失败 → 断言回滚诚实性。
    let self_name = std::env::current_exe()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    match mgr.select_target(&ws, None, Some(&self_name)).await {
        Ok(out) => {
            let draft = out["draft"].as_object().expect("draft 投影");
            assert_eq!(draft["pid"].as_u64(), Some(std::process::id() as u64));
            assert_eq!(draft["exe_name"].as_str(), Some(self_name.as_str()));
            assert!(out["session"].is_object(), "会话快照在场");
            assert_eq!(out["next"], "snapshot");
            // 收尾：abort 卸载会话 + 清 draft/快照目录（幂等成功臂）
            let aborted = mgr.abort(&ws).await.expect("abort 收尾");
            assert_eq!(aborted["aborted"], out["draft"]["profile_id"]);
            assert!(mgr.state.lock().unwrap().is_none(), "abort 后状态清空");
            assert!(super::latest_draft_id(&ws).is_none(), "draft 已清");
        }
        Err(e) => {
            assert!(
                e.contains("会话管理器未装配") || e.contains("已回滚"),
                "实际：{e}"
            );
            assert!(mgr.state.lock().unwrap().is_none(), "失败回收预占");
            assert!(super::latest_draft_id(&ws).is_none(), "draft 已回滚不留残");
        }
    }
    cleanup(&ws);
}

#[cfg(windows)]
#[test]
fn find_pid_by_name_and_main_module_locate_own_process() {
    use super::{find_pid_by_name, main_module};

    let self_name = std::env::current_exe()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    // nextest 并行下同二进制的兄弟测试进程与本进程同名是常态——
    // 「唯一命中即自身」的单实例假设不成立（CI 2026-11-13 实证：兄弟
    // pid 2828 < 自身 3172，实现按契约取最小 pid 被误判红）。
    //
    // 也不做跨快照最小值比较：实现内部快照与本测试的伴随枚举是两个
    // 独立时刻，孪生进程集随时增减（同日二次实证：实现返回 1112，是
    // 枚举之后才出现的孪生）——「返回值 = 枚举最小 pid」天然带竞态。
    // 改验证用户可见契约：返回 pid 是同名在册进程。返回自身即天然
    // 成立；否则重试枚举收敛（孪生存活以秒计，8 次连续脱靶可忽略）。
    fn enumerate_pids_named(name_lower: &str) -> Vec<u32> {
        let mut found: Vec<u32> = Vec::new();
        unsafe {
            use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
            use windows_sys::Win32::System::Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            };
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            assert_ne!(snap, INVALID_HANDLE_VALUE, "进程快照创建失败");
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut entry) != 0 {
                loop {
                    let exe = String::from_utf16_lossy(
                        &entry.szExeFile
                            [..entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)],
                    );
                    if exe.to_lowercase() == name_lower {
                        found.push(entry.th32ProcessID);
                    }
                    if Process32NextW(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        found
    }

    let want = self_name.to_lowercase();
    let pids = enumerate_pids_named(&want);
    assert!(
        pids.contains(&std::process::id()),
        "自身进程必在册（命中集合 {pids:?}）"
    );
    let mut contract_holds = false;
    for _ in 0..8 {
        if let Ok(pid) = find_pid_by_name(&self_name)
            && (pid == std::process::id() || enumerate_pids_named(&want).contains(&pid))
        {
            contract_holds = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        contract_holds,
        "find_pid_by_name 必返回同名在册进程（重试收敛失败）"
    );
    // 不存在的进程名：诚实报错
    let err = find_pid_by_name("no-such-proc-xyz.exe").unwrap_err();
    assert!(err.contains("未找到进程"), "{err}");

    // 主模块枚举：自身 pid 命中（base/size/路径/名字四元组）
    let (base, size, path, name) = main_module(std::process::id()).expect("自身模块枚举");
    assert!(base > 0 && size > 0);
    assert!(path.to_lowercase().ends_with(&self_name.to_lowercase()));
    assert_eq!(name.to_lowercase(), self_name.to_lowercase());
    // 不存在的 pid：快照失败诚实报错
    assert!(main_module(0x7FFF_FFFF).is_err());
}
