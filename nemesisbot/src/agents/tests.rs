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
        runtime_backend: String::new(),
        tool_policy: serde_json::json!({}),
        persona_refs: serde_json::Value::Null,
        credential_refs: serde_json::json!([]),
        principal: String::new(),
    }
}

// R2 回归锁（原诊断探针翻转）+ F2 适配（2026-10-21 三轮复评）：删除联动
// VM 后端收敛——remove 无条件走 evict_agent_prepare（运行实例冷停出
// running_agents；本 mock 镜像真实语义：prepare 摘清单、commit 记墓碑、
// abort 无操作），删除成功后旧实例不再在运行清单且身份入墓碑。
#[tokio::test]
async fn remove_converges_attached_vm_backend_instance() {
    use super::VmBackend;
    struct ReviewVm {
        running: std::sync::Mutex<Vec<String>>,
        tombstones: std::sync::Mutex<Vec<String>>,
    }
    #[async_trait::async_trait]
    impl nemesis_agent::remote_executor_tool::PersistentExecutor for ReviewVm {
        async fn execute_persistent(
            &self,
            _: &str,
            _: &Path,
            _: &str,
            _: &str,
            _: &nemesis_agent::context::RequestContext,
        ) -> Result<String, nemesis_agent::remote_executor_tool::PersistentCallError> {
            Ok("unused".into())
        }
    }
    #[async_trait::async_trait]
    impl VmBackend for ReviewVm {
        async fn sleep_agent(&self, id: &str) -> Result<bool, String> {
            self.running.lock().unwrap().retain(|x| x != id);
            Ok(true)
        }
        async fn wake_agent(&self, id: &str, _: &Path) -> Result<(), String> {
            self.running.lock().unwrap().push(id.to_string());
            Ok(())
        }
        fn running_agents(&self) -> Vec<String> {
            self.running.lock().unwrap().clone()
        }
        async fn evict_agent_prepare(&self, id: &str) -> Result<(), String> {
            // 镜像 VmSupervisor::evict_agent_prepare 的可见效果：运行实例冷停出清单。
            self.running.lock().unwrap().retain(|x| x != id);
            Ok(())
        }
        fn evict_agent_commit(&self, id: &str) {
            self.tombstones.lock().unwrap().push(id.to_string());
        }
        fn evict_agent_abort(&self, id: &str) {
            // 临时态解除——本 mock 无独立临时集（prepare 无临时副作用），
            // 空操作即镜像「prepare 未记任何待解除状态」。
            let _ = id;
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let mut params = selfbuilt_params("review vm remove");
    params.runtime_backend = "vm".into();
    let record = mgr.create_agent(params).unwrap();
    let vm = Arc::new(ReviewVm {
        running: std::sync::Mutex::new(Vec::new()),
        tombstones: std::sync::Mutex::new(Vec::new()),
    });
    mgr.set_vm_backend(vm.clone());
    vm.wake_agent(&record.agent_id, Path::new(&record.exec_root))
        .await
        .unwrap();
    assert!(vm.running_agents().contains(&record.agent_id));
    mgr.remove(&record.agent_id).await.unwrap();
    assert!(mgr.get(&record.agent_id).unwrap().is_none());
    assert!(!vm.running_agents().contains(&record.agent_id));
    // F2：删除成功 = 永久墓碑落地（旧身份不可再唤醒的语义锚）。
    assert!(
        vm.tombstones.lock().unwrap().contains(&record.agent_id),
        "删除成功必须提交永久墓碑"
    );
    vm.sleep_agent(&record.agent_id).await.unwrap();
}

// F2 回归锁（原三轮复评诊断探针翻转，2026-10-21）：DB 删除失败 = 临时拒绝
// 态解除、永久墓碑不落——记录仍在册的身份**可再唤醒**。旧行为（探针实证）：
// evict_agent 先落永久墓碑再删库，DELETE 触发器注入失败后 ensure_running
// 报「已删除（VM 身份已逐出）」——记录在册但身份永久不可唤醒直到重启。
// 真实 SQLite + BEFORE DELETE RAISE 触发器 + 真实 VmSupervisor（无须 QEMU：
// 断言面 = 控制状态，唤醒错误形态为 VM 档启动类诚实报错）。
#[cfg(feature = "vm")]
#[tokio::test]
async fn failed_delete_releases_evicting_state_record_still_wakeable() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let mut params = selfbuilt_params("review failed deletion");
    params.runtime_backend = "vm".into();
    let record = mgr.create_agent(params).unwrap();
    let workspace = dir.path().join("workspace");
    let vm = Arc::new(super::vm_runtime::VmSupervisor::new(
        workspace.clone(),
        dir.path().join("config.json"),
    ));
    mgr.set_vm_backend(vm.clone());
    let conn =
        rusqlite::Connection::open(nemesis_path::resolve_tasks_db_path_in_workspace(&workspace))
            .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER review_reject_delete BEFORE DELETE ON agents BEGIN SELECT RAISE(ABORT, 'review injected delete failure'); END;",
    )
    .unwrap();
    let delete_error = mgr.remove(&record.agent_id).await.unwrap_err();
    assert!(
        delete_error.contains("review injected delete failure"),
        "删除失败须原样上抛: {delete_error}"
    );
    assert!(
        mgr.get(&record.agent_id).unwrap().is_some(),
        "DB 删除失败 = 记录仍在册"
    );
    // 翻转断言：唤醒不再被「已删除/删除在途」拒绝——永久墓碑未落、临时态
    // 已解除。错误若发生只能是 VM 档本身（测试环境无 config/镜像）的启动
    // 类诚实报错。
    let wake_error = match vm.ensure_running(&record.agent_id).await {
        Ok(_) => panic!("测试环境无 VM config，不应真能启动"),
        Err(e) => e.to_string(),
    };
    assert!(
        !wake_error.contains("已删除"),
        "删除失败后身份不得被判「已删除」（墓碑未落）: {wake_error}"
    );
    assert!(
        !wake_error.contains("删除在途"),
        "删除失败后临时拒绝态必须解除: {wake_error}"
    );
}

// F2 配套：**成功路径**墓碑语义保持——删除成功后旧身份不可再唤醒
// （「已删除」拒绝仍在；与上例合起来钉死两段式的两臂）。
#[cfg(feature = "vm")]
#[tokio::test]
async fn successful_delete_still_tombstones_vm_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let mut params = selfbuilt_params("review success deletion");
    params.runtime_backend = "vm".into();
    let record = mgr.create_agent(params).unwrap();
    let workspace = dir.path().join("workspace");
    let vm = Arc::new(super::vm_runtime::VmSupervisor::new(
        workspace.clone(),
        dir.path().join("config.json"),
    ));
    mgr.set_vm_backend(vm.clone());
    mgr.remove(&record.agent_id).await.unwrap();
    assert!(mgr.get(&record.agent_id).unwrap().is_none());
    let wake_error = match vm.ensure_running(&record.agent_id).await {
        Ok(_) => panic!("已删除身份不应能启动"),
        Err(e) => e.to_string(),
    };
    assert!(
        wake_error.contains("已删除"),
        "删除成功后旧身份必须被墓碑拒绝: {wake_error}"
    );
}

// -- P5 第 4 步：runtime_backend 档位面 --------------------------------------

#[test]
fn create_rejects_illegal_runtime_backend_and_defaults_process() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    // 非法档位 loud 拒绝，不留半创建记录。
    let mut bad = selfbuilt_params("bad");
    bad.runtime_backend = "qemu".to_string();
    assert!(mgr.create_agent(bad).is_err());
    assert!(mgr.list().unwrap().is_empty());
    // 空串同 process（旧调用方零迁移）。
    let rec = mgr.create_agent(selfbuilt_params("w1")).unwrap();
    assert_eq!(rec.runtime_backend, "process");
    // 显式 vm 合法落库。
    let mut vm = selfbuilt_params("w2");
    vm.runtime_backend = "vm".to_string();
    let rec = mgr.create_agent(vm).unwrap();
    assert_eq!(rec.runtime_backend, "vm");
}

// P6-R4（D4 预留）：principal 落库回归锁——create 持久化主体标识，重开
// manager（同 db）读回一致；缺省空串 = 未绑定。纯预留：无任何闸读取。
#[test]
fn create_persists_principal_for_persistence_source_of_truth() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let mut p = selfbuilt_params("bound");
    p.principal = "web/user-42".to_string();
    let rec = mgr.create_agent(p).unwrap();
    assert_eq!(rec.principal, "web/user-42");
    // 未绑定 = 缺省空串。
    let rec = mgr.create_agent(selfbuilt_params("unbound")).unwrap();
    assert_eq!(rec.principal, "");
    // 落库真相源核验（重开 manager 读同一 db——持久性而非缓存态）。
    let db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        &dir.path().join("workspace"),
    ))
    .unwrap();
    let mgr2 = AgentManager::from_db(dir.path().join("workspace"), db);
    assert_eq!(mgr2.get(&rec.agent_id).unwrap().unwrap().principal, "");
}

#[test]
fn update_backend_switches_persistently_and_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let rec = mgr.create_agent(selfbuilt_params("w")).unwrap();
    assert_eq!(rec.runtime_backend, "process");
    // process → vm。
    let rec = mgr.update_backend(&rec.agent_id, "vm").unwrap();
    assert_eq!(rec.runtime_backend, "vm");
    // 落库真相源核验（重开 manager 读同一 db——持久性而非缓存态）。
    let db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        &dir.path().join("workspace"),
    ))
    .unwrap();
    let mgr2 = AgentManager::from_db(dir.path().join("workspace"), db);
    assert_eq!(
        mgr2.get(&rec.agent_id).unwrap().unwrap().runtime_backend,
        "vm"
    );
    // vm → process 回切。
    let rec = mgr.update_backend(&rec.agent_id, "process").unwrap();
    assert_eq!(rec.runtime_backend, "process");
    // 同档幂等 = 无操作（诚实返回现值）。
    let again = mgr.update_backend(&rec.agent_id, "process").unwrap();
    assert_eq!(again.runtime_backend, "process");
    // 非法档位 loud 拒绝（含不存在 agent——值域校验先于存在性）。
    assert!(mgr.update_backend(&rec.agent_id, "docker").is_err());
    assert!(
        mgr.update_backend("nonexistent-agent", "vm")
            .err()
            .unwrap()
            .contains("不存在")
    );
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

/// R4（2026-10-18 三方复评）管理面集成锁：remove 联动常驻执行体停机——
/// create → wake（真 spawn）→ remove → 槽空 + 记录删。真二进制 + 真 spawn
/// （二进制解析纪律与 runtime/tests.rs sup_real_child 同源：debug 槽专用）。
/// 持 GLOBAL_STATE_LOCK 跨 await 是刻意的测试串行化（同 agents_ws_tests
/// 纪律：真进程用例必须与 vault/全局态用例互斥，槽位覆盖整个 await 窗口）。
#[cfg(windows)]
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn remove_evicts_resident_executor_and_deletes_record() {
    use crate::GLOBAL_STATE_LOCK;
    use crate::agents::runtime::AgentRuntimeSupervisor;

    let _env = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    // persistent.enabled=true 的 config 写在 tempdir 根（supervisor 的
    // config_path 指它；manager 只管 tasks.db，两者互不知晓）。
    let cfg_path = dir.path().join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"executor":{"persistent":{"enabled":true,"max_slots":2,"idle_shutdown_secs":0,"call_timeout_secs":120}}}"#,
    )
    .unwrap();

    let mgr = setup(dir.path());
    let rec = mgr.create_agent(selfbuilt_params("doomed")).unwrap();
    let sup = {
        let bin = match std::env::var("NEMESISBOT_TEST_BIN") {
            Ok(b) => PathBuf::from(b),
            Err(_) => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("target")
                .join("debug")
                .join(format!("nemesisbot{}", std::env::consts::EXE_SUFFIX)),
        };
        assert!(
            bin.exists(),
            "target/debug/nemesisbot.exe 不存在——先 cargo build -p nemesisbot"
        );
        Arc::new(AgentRuntimeSupervisor::new(
            bin,
            dir.path().to_path_buf(),
            ws.clone(),
            cfg_path,
        ))
    };
    mgr.set_runtime(Arc::clone(&sup));

    // wake：真 spawn（exec_root = agent home）。
    let exec_root = PathBuf::from(&rec.exec_root);
    sup.wake_agent(&rec.agent_id, &exec_root)
        .await
        .expect("wake spawn");
    assert!(
        sup.slot_pid(&rec.agent_id).await.is_some(),
        "wake 后槽必须在场"
    );

    // remove：evict 停机 + 记录删。
    mgr.remove(&rec.agent_id).await.expect("remove");
    assert!(
        sup.slot_pid(&rec.agent_id).await.is_none(),
        "remove 必须联动停掉常驻执行体（evict）"
    );
    assert!(sup.slot_states().is_empty());
    assert!(mgr.get(&rec.agent_id).unwrap().is_none(), "记录必须已删");
}

#[tokio::test]
async fn main_agent_seed_is_idempotent_across_manager_instances() {
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
    assert!(m2.remove(&first.agent_id).await.is_err());
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
fn remove_project_mirror_targets_only_matching_project() {
    // projects.remove 联动契约：按 project_id 定向摘镜像（其余 kind 与
    // 其他项目记录不动）；幂等（二次摘=0 条）；main/selfbuilt 不误伤。
    let dir = tempfile::tempdir().unwrap();
    let mgr = setup(dir.path());
    let proj1 = dir.path().join("proj1");
    let proj2 = dir.path().join("proj2");
    std::fs::create_dir_all(&proj1).unwrap();
    std::fs::create_dir_all(&proj2).unwrap();
    mgr.ensure_main_agent().unwrap();
    mgr.sync_project_agents(&[("p1".into(), "甲".into(), proj1)])
        .unwrap();
    mgr.sync_project_agents(&[("p2".into(), "乙".into(), proj2)])
        .unwrap();
    let own = mgr.create_agent(selfbuilt_params("self")).unwrap();

    let removed = mgr.remove_project_mirror("p1").unwrap();
    assert_eq!(removed, 1, "恰好摘 p1 一条镜像");
    let left = mgr.list().unwrap();
    assert!(
        left.iter().all(|r| r.project_id.as_deref() != Some("p1")),
        "p1 镜像必须消失: {left:?}"
    );
    assert!(
        left.iter()
            .any(|r| r.kind == agent_kind::PROJECT && r.project_id.as_deref() == Some("p2")),
        "p2 镜像必须保留"
    );
    assert!(
        left.iter().any(|r| r.kind == agent_kind::MAIN),
        "main 不误伤"
    );
    assert!(
        left.iter().any(|r| r.agent_id == own.agent_id),
        "selfbuilt 不误伤"
    );

    // 幂等：二次摘 = 0 条不报错。
    assert_eq!(
        mgr.remove_project_mirror("p1").unwrap(),
        0,
        "二次摘必须幂等"
    );
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
/// vault_runtime 测试的全局槽互不干扰）；argon2 模式的 env 口令是
/// 进程级全局——持 GLOBAL_STATE_LOCK 与 vault_runtime 系测试互斥
/// （vault 测试纪律四件套之一；无锁时 wrong-pw 用例可在 set/读窗口
/// 内穿插改值导致假红），尾部按平台默认态恢复。
/// security 裁剪形态整个消失（vault 本体在 nemesis-security 里）。
#[cfg(all(test, feature = "security"))]
#[test]
fn agent_credential_resolution_roundtrip_via_real_vault() {
    let _guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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

    // 尾部按平台默认态恢复口令 env（对齐 vault_runtime::restore_passphrase_env：
    // Argon2id 平台留惯例 seed 值、DPAPI 平台清除，不向后续测试泄漏）。
    if mode == nemesis_security::vault::VaultMode::Argon2id {
        unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", passphrase) };
    } else {
        unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
    }
}
