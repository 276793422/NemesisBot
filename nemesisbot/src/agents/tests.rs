//! AgentManager 行为用例（P2）：两阶段创建 / main 幂等 seed / sweep 收敛 /
//! 项目镜像 / 删除守卫。全部走 tempdir 真文件 + 内存可开的 tasks.db。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{AgentManager, CreateAgentParams};
use nemesis_path::resolve_agent_home_in_workspace;
use nemesis_taskstore::TaskDb;
use nemesis_taskstore::agents::{kind as agent_kind, status as agent_status};

fn setup(dir: &Path) -> AgentManager {
    let ws = dir.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    let db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(&ws)).unwrap();
    AgentManager::from_db(ws, db)
}

fn selfbuilt_params(name: &str) -> CreateAgentParams {
    CreateAgentParams {
        name: name.to_string(),
        tier: String::new(),
        tool_policy: serde_json::json!({}),
        persona_refs: serde_json::Value::Null,
        credential_refs: serde_json::json!([]),
    }
}

#[test]
fn two_phase_create_produces_active_record_and_materialized_home() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let rec = mgr.create_agent(selfbuilt_params("worker-a")).unwrap();
    assert_eq!(rec.status, agent_status::ACTIVE);
    assert_eq!(rec.kind, agent_kind::SELFBUILT);
    // home 物化在盘上（内容断言，不止目录存在性——目录存在性断言在
    // Windows 删除挂起幻影下会假红，此处是创建面直接断言条目）。
    let ws = dir.path().join("workspace");
    let home = resolve_agent_home_in_workspace(&ws, &rec.agent_id);
    assert!(home.is_dir(), "home 未物化: {}", home.display());
    let memory = nemesis_path::resolve_agent_memory_dir_in_workspace(&ws, &rec.agent_id);
    assert!(memory.is_dir(), "记忆目录未物化: {}", memory.display());
    // 命名空间规则：selfbuilt 独占域。
    assert_eq!(rec.memory_namespace, format!("agents/{}", rec.agent_id));
    // 隔离根语义：exec_root=自己 home（跨 agent home 互不可见）。
    assert_eq!(PathBuf::from(&rec.exec_root), home);
    assert_eq!(PathBuf::from(&rec.home_path), home);
}

#[test]
fn create_rejects_bad_inputs_without_leaving_records() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    // 空名拒绝。
    assert!(mgr.create_agent(selfbuilt_params("   ")).is_err());
    // 无记录残留（失败路径不留半创建）。
    assert!(mgr.list().unwrap().is_empty());
    // 正常创建照常可用（拒绝面只针对非法入参）。
    assert!(mgr.create_agent(selfbuilt_params("ok")).is_ok());
}

#[test]
fn main_agent_seed_is_idempotent_across_manager_instances() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    // 两个 manager 实例（模拟两次 boot）共享同一 db。
    let db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(&ws)).unwrap();
    let m1 = AgentManager::from_db(ws.clone(), Arc::clone(&db));
    let m2 = AgentManager::from_db(ws.clone(), Arc::clone(&db));
    let first = m1.ensure_main_agent().unwrap();
    let second = m2.ensure_main_agent().unwrap();
    assert_eq!(first.agent_id, second.agent_id, "main 身份必须跨 boot 稳定");
    assert_eq!(first.kind, agent_kind::MAIN);
    // main 记忆 = workspace 记忆（namespace 留空，行为零变化）。
    assert_eq!(first.memory_namespace, "");
    assert_eq!(first.exec_root, ws.to_string_lossy().to_string());
    // main 不可删。
    assert!(m2.remove(&first.agent_id).is_err());
    assert!(m2.get(&first.agent_id).unwrap().is_some());
}

#[test]
fn sweep_converges_stuck_provisioning_records() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let ok = mgr.create_agent(selfbuilt_params("fine")).unwrap();
    // 手工制造半创建：直接落 provisioning 记录不物化 home（模拟中断）。
    let stuck = nemesis_taskstore::AgentRecord::new_provisioning(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "stuck",
        agent_kind::SELFBUILT,
    );
    let stuck_ws = dir.path().join("workspace");
    // 用一个独立 manager 直写 store 制造「记录在、home 无」。
    let db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(&stuck_ws)).unwrap();
    let raw_store = nemesis_taskstore::SqliteAgentsStore::from_db(db);
    raw_store.upsert(&stuck).unwrap();
    // sweep：stuck 物化成功 → active；已 active 的 fine 不受影响。
    let report = mgr.sweep().unwrap();
    assert_eq!(report.activated, vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]);
    assert!(report.failed.is_empty());
    assert_eq!(
        mgr.get(&stuck.agent_id).unwrap().unwrap().status,
        agent_status::ACTIVE
    );
    assert_eq!(
        mgr.get(&ok.agent_id).unwrap().unwrap().status,
        agent_status::ACTIVE
    );
}

#[test]
fn sweep_marks_failed_when_home_unmaterializable() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let ws = dir.path().join("workspace");
    // 制造「物化必败」：home 父位被一个同名文件占住（create_dir_all 必败）。
    let agent_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    std::fs::create_dir_all(ws.join("agents")).unwrap();
    std::fs::write(ws.join("agents").join(agent_id), "not a dir").unwrap();
    let db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(&ws)).unwrap();
    let raw = nemesis_taskstore::SqliteAgentsStore::from_db(db);
    let mut rec =
        nemesis_taskstore::AgentRecord::new_provisioning(agent_id, "doomed", agent_kind::SELFBUILT);
    rec.exec_root = ws.to_string_lossy().to_string();
    raw.upsert(&rec).unwrap();
    let report = mgr.sweep().unwrap();
    assert_eq!(report.failed, vec![agent_id]);
    assert_eq!(
        mgr.get(agent_id).unwrap().unwrap().status,
        agent_status::FAILED
    );
}

#[test]
fn project_mirror_sync_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let entries = vec![("p1".to_string(), "代码项目".to_string(), proj.clone())];
    let created1 = mgr.sync_project_agents(&entries).unwrap();
    assert_eq!(created1.len(), 1);
    let created2 = mgr.sync_project_agents(&entries).unwrap();
    assert!(created2.is_empty(), "二次同步必须幂等");
    let rec = mgr
        .list()
        .unwrap()
        .into_iter()
        .find(|r| r.kind == agent_kind::PROJECT)
        .unwrap();
    assert_eq!(rec.project_id.as_deref(), Some("p1"));
    assert_eq!(rec.exec_root, proj.to_string_lossy().to_string());
    // 项目 agent 与 workspace 记忆共享（namespace 留空）。
    assert_eq!(rec.memory_namespace, "");
}

#[test]
fn three_kinds_coexist_without_cross_talk() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let main = mgr.ensure_main_agent().unwrap();
    let proj_recs = mgr
        .sync_project_agents(&[("p1".into(), "prj".into(), proj)])
        .unwrap();
    let own = mgr.create_agent(selfbuilt_params("self")).unwrap();
    let all = mgr.list().unwrap();
    assert_eq!(all.len(), 3, "三类组并存");
    // 身份互异、kind 各就各位、命名空间互不串。
    assert_ne!(main.agent_id, own.agent_id);
    assert_eq!(main.kind, agent_kind::MAIN);
    assert_eq!(proj_recs.len(), 1);
    assert_eq!(own.kind, agent_kind::SELFBUILT);
    let ns: Vec<&str> = all.iter().map(|r| r.memory_namespace.as_str()).collect();
    assert_eq!(
        ns.iter().filter(|n| n.is_empty()).count(),
        2,
        "main+project 共享域"
    );
    assert!(
        ns.iter().any(|n| n.starts_with("agents/")),
        "selfbuilt 独占域"
    );
}

// -- 凭据 vault 别名（P2：vault:agent/<id>/<name>）--

#[test]
fn credential_alias_format() {
    assert_eq!(
        super::credential_alias("abc123", "api_key"),
        "vault:agent/abc123/api_key"
    );
}

#[test]
fn validate_credential_refs_accepts_own_refs_and_empty() {
    let id = "self-id";
    assert!(super::validate_credential_refs(id, &serde_json::json!([])).is_ok());
    assert!(
        super::validate_credential_refs(id, &serde_json::json!(["vault:agent/self-id/api_key"]))
            .is_ok()
    );
    assert!(
        super::validate_credential_refs(
            id,
            &serde_json::json!(["vault:agent/self-id/a", "vault:agent/self-id/b"])
        )
        .is_ok()
    );
}

#[test]
fn validate_credential_refs_rejects_foreign_and_malformed() {
    let id = "self-id";
    // 跨 agent 引用拒绝（隔离性质的核心断言）。
    let err =
        super::validate_credential_refs(id, &serde_json::json!(["vault:agent/other-id/api_key"]))
            .unwrap_err();
    assert!(err.contains("非法"), "got: {err}");
    // 非 vault: 前缀 / 缺名段 / 嵌套 vault: 全拒绝。
    assert!(super::validate_credential_refs(id, &serde_json::json!(["env:FOO"])).is_err());
    assert!(
        super::validate_credential_refs(id, &serde_json::json!(["vault:agent/self-id/"])).is_err()
    );
    assert!(
        super::validate_credential_refs(id, &serde_json::json!(["vault:agent/self-id/a:vault:b"]))
            .is_err()
    );
    // 非数组 / 非字符串条目拒绝。
    assert!(super::validate_credential_refs(id, &serde_json::json!({"k": "v"})).is_err());
    assert!(super::validate_credential_refs(id, &serde_json::json!([42])).is_err());
}

#[test]
fn create_agent_rejects_foreign_credential_ref_without_residue() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let mut p = selfbuilt_params("thief");
    p.credential_refs = serde_json::json!(["vault:agent/someone-else/api_key"]);
    assert!(mgr.create_agent(p).is_err());
    assert!(mgr.list().unwrap().is_empty(), "失败不留半创建");
}

/// 解析全链路：真实 VaultStore 落盘 → resolve_agent_credential 出真值；
/// 缺别名 fail loud。直连 resolve_alias_at 不触全局解析器（与
/// vault_runtime 测试的全局槽互不干扰）；argon2 模式的 env 口令与
/// vault_runtime 测试同值（并发互踩时值一致，良性）。
/// security 裁剪形态整个消失（vault 本体在 nemesis-security 里）。
#[cfg(all(test, feature = "security"))]
#[test]
fn agent_credential_resolution_roundtrip_via_real_vault() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    let agent_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let vp = nemesis_path::resolve_vault_path_in_workspace(&ws);
    std::fs::create_dir_all(vp.parent().unwrap()).unwrap();
    let mode = nemesis_security::vault::VaultStore::default_mode();
    let passphrase = "rt-test-pw";
    if mode == nemesis_security::vault::VaultMode::Argon2id {
        unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", passphrase) };
    }
    let mut store = nemesis_security::vault::VaultStore::create(
        &vp,
        mode,
        if mode == nemesis_security::vault::VaultMode::Argon2id {
            Some(passphrase)
        } else {
            None
        },
    )
    .unwrap();
    store
        .set(
            &format!("agent/{agent_id}/api_key"),
            "sk-test-123",
            "test",
            "",
        )
        .unwrap();
    store.save().unwrap();

    let got = super::resolve_agent_credential(&ws, agent_id, "api_key").unwrap();
    assert_eq!(got, "sk-test-123");

    // 缺别名 fail loud（带 vault set 指引）。
    let err = super::resolve_agent_credential(&ws, agent_id, "absent").unwrap_err();
    assert!(err.contains("vault set"), "got: {err}");
}
