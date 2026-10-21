//! Agent 持久实体管理器（P2）。
//!
//! 记录真相源 = tasks.db `agents` 表（[`nemesis_taskstore::SqliteAgentsStore`]）；
//! home/记忆目录物理布局单一真相源 = `nemesis-path`（`resolve_agent_*`）。
//! 本模块只做编排纪律：
//!
//! - **两阶段创建**（D6 复合操作纪律）：记录先落 `provisioning` → home
//!   物化（`create_dir_all` 幂等可重跑）→ 条件转移到 `active`。任何一步
//!   中断都不产生假 active 记录。
//! - **启动 sweep**：卡在 `provisioning` 的半创建记录在 gateway 启动时
//!   收敛——重物化成功→`active`；物化失败→`failed`（终态，WARN 留痕，
//!   人工可删可重建）。
//! - **main 幂等 seed**：对话组默认 agent 在启动时按 kind=main 查找，
//!   缺失才创建（身份投影跨重启稳定，绝不每次 boot 生成新 id）。
//! - **项目 agent 镜像**：projects.json 是路径真相源（L6++ 语义不变），
//!   agents 表按项目镜像运行时实体（boot 时同步；`kind=project` 的
//!   exec_root=项目目录）。
//!
//! 命名空间规则（三类组不串台）：main/project 共享 workspace 记忆
//! （namespace 留空——与现有行为零变化），selfbuilt 独占
//! `agents/<id>` 命名空间。

#[cfg(test)]
mod tests;

mod bridge;

/// P3 T4：常驻执行体监督器（per-agent 长驻子进程 + 隔离 + 幂等）。
pub mod runtime;

/// G1（2026-10-18 三方复评整改批三）：自建 agent 常驻 LLM loop 生命周期
/// 管理器（`agents.send` WSAPI 的解析/构建/投递面；R4 remove 联动）。
pub mod loop_manager;

/// P5 第 3 步：VM 持久实体监督器（`vm` feature 闸——收窄/IoT 形态整模块
/// 不进二进制，`nemesis-sandbox/vm` 子模块同闸）。
#[cfg(feature = "vm")]
pub mod vm_runtime;

/// P6-R2：agent 跨节点迁移（`cluster` feature 闸——编排/传输/B 端落地
/// 面全在此；裁剪形态不可迁移，WSAPI 臂诚实报错）。
#[cfg(feature = "cluster")]
pub mod migrate;

/// P5 第 4 步：`vm.*` WSAPI 桥（web 侧 [`nemesis_web::handlers::vm::VmBridge`]
/// trait 的 [`VmSupervisor`](vm_runtime::VmSupervisor) 实现——投影与转发，
/// gateway 装配期装槽）。B3 起 pub(crate)：CLI `vm build-image` 前台臂
/// 复用其输入裁决与任务槽判定（单一真相源，不另写一份预检）。
#[cfg(feature = "vm")]
pub(crate) mod vm_bridge;

/// B7 伴生测试（vm_bridge.rs 的 WSL 自动路由纯函数；零内联纪律）。
#[cfg(all(test, feature = "vm"))]
mod vm_bridge_tests;

/// B9-3 伴生测试（vm_bridge.rs 的同步目标解析；零内联纪律）。
#[cfg(all(test, feature = "vm"))]
mod vm_bridge_sync_tests;

use std::path::{Path, PathBuf};

use nemesis_path::{resolve_agent_home_in_workspace, resolve_agent_memory_dir_in_workspace};
use nemesis_taskstore::agents::{kind as agent_kind, status as agent_status};
use nemesis_taskstore::{AgentRecord, SqliteAgentsStore, TaskDb};

/// 自建 agent 创建入参。exec_root 由本模块派生（= 自己 home，PLAN 钉死
/// 语义「隔离根=exec_root、跨 agent home 互不可见」）；kind 固定
/// selfbuilt（project 镜像走 [`AgentManager::sync_project_agents`]，main
/// 走 [`AgentManager::ensure_main_agent`]）。
#[derive(Debug, Clone)]
pub struct CreateAgentParams {
    pub name: String,
    pub tier: String,
    /// P5 第 4 步：运行时后端档位（`process|vm`；空串同 process）。值域
    /// 校验单一真相源 `nemesis_taskstore::agents::runtime_backend::validate`
    /// ——create 落库前过，非法值 loud 拒绝。
    pub runtime_backend: String,
    pub tool_policy: serde_json::Value,
    pub persona_refs: serde_json::Value,
    pub credential_refs: serde_json::Value,
    /// P6-R4（D4 预留）：主体标识（空 = 未绑定）。只落库不消费——多租户
    /// 闸不做；迁移 `rewrite_record_for_target` 不触碰本字段（全记录
    /// serialize 过 RPC，B 端 upsert 原值保全）。
    pub principal: String,
}

/// sweep 收敛结果（WSAPI/日志可观测面）。
#[derive(Debug, Clone, PartialEq)]
pub struct SweepReport {
    /// provisioning → active（续建成功）。
    pub activated: Vec<String>,
    /// provisioning → failed（物化失败，WARN 留痕）。
    pub failed: Vec<String>,
    /// P6-R2：migrating + 墓碑在场（commit ACK 已达）→ 删记录收敛。
    pub migrated_removed: Vec<String>,
    /// P6-R2：migrating + 无墓碑（commit 未达）→ 回滚 active 照常服务。
    pub reactivated: Vec<String>,
}

/// Agent 记录管理器（clone 廉价；与任务真相源共享同一 `Arc<TaskDb>`）。
#[derive(Clone)]
pub struct AgentManager {
    store: SqliteAgentsStore,
    workspace: PathBuf,
    /// 常驻执行体监督器（T9 生命周期接线；gateway post_agent 装配——
    /// supervisor 构造晚于 manager，经 [`Self::set_runtime`] 后补注入。
    /// `None` = 未装配（headless/测试形态），生命周期面诚实不可用）。
    runtime: std::sync::Arc<
        std::sync::RwLock<Option<std::sync::Arc<crate::agents::runtime::AgentRuntimeSupervisor>>>,
    >,
    /// G1：自建 agent 常驻 LLM loop 管理器（同 runtime 后补注入形态；
    /// `remove` 联动停机摘 loop——R4 协调点）。
    loop_manager: std::sync::Arc<
        std::sync::RwLock<Option<std::sync::Arc<crate::agents::loop_manager::AgentLoopManager>>>,
    >,
    /// P5 第 4 步：VM 常驻后端槽（feature 无关投影 [`VmBackend`]——`vm`
    /// feature 关闭时恒 None，档位路由与管理面诚实降级，off-state 字节
    /// 不变）。gateway post_agent 装配（同 set_runtime 的晚注入纪律）。
    vm: std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<dyn VmBackend>>>>,
    /// B9-1：VM 具体型监督器槽——vm_sync 同步引擎的 `internal_call`（内部
    /// 方法通道）需要具体型 [`vm_runtime::VmSupervisor`]；`vm` trait 槽是
    /// feature 无关投影，不携带内部方法。与 `vm` 槽并存（post_agent 同一
    /// 装配点装同一 Arc 的两投影）。
    #[cfg(feature = "vm")]
    vm_supervisor: std::sync::Arc<
        std::sync::RwLock<Option<std::sync::Arc<crate::agents::vm_runtime::VmSupervisor>>>,
    >,
    /// P6-R2：迁移传输层槽（cluster feature 闸；gateway post_agent 在
    /// cluster Arc 就绪后注入——与 set_runtime 同款晚注入纪律）。
    #[cfg(feature = "cluster")]
    migrate_transport:
        std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<dyn migrate::MigrateTransport>>>>,
}

// ---------------------------------------------------------------------------
// P5 第 4 步：VM 常驻后端面（feature 无关投影）
// ---------------------------------------------------------------------------

/// VM 常驻后端的生命周期 + 执行面投影。trait 常在（不挂 feature）——
/// 管理面（agents WSAPI 切档/睡眠/唤醒）与工具路由面（BackendRoutingExecutor）
/// 对它编程，`vm` feature 关闭时槽恒 None：路由面把 VM 档记作 Unavailable
/// 回落 per-call（与「开关关」同语义），管理面诚实报「未装配」。实现 =
/// [`vm_runtime::VmSupervisor`](vm_runtime::VmSupervisor)（cfg vm）。
///
/// supertrait [`PersistentExecutor`](nemesis_agent::remote_executor_tool::PersistentExecutor)
/// 保证路由面与进程档走同一 trait 面（P5 验收门「同一 RuntimeBackend 接口」
/// 的编译期形态）。
#[async_trait::async_trait]
pub trait VmBackend: nemesis_agent::remote_executor_tool::PersistentExecutor {
    /// 睡眠：`Ok(true)` = 已睡眠（状态落盘）；`Ok(false)` = 无运行中实例
    /// 可睡（与进程档 sleep 的「无槽」同义，不算错误）；`Err` = 真实失败。
    async fn sleep_agent(&self, agent_id: &str) -> Result<bool, String>;

    /// 唤醒/冷启。`exec_root` 与进程档 wake 同形签名（审计语义保留）；
    /// VM 档工作区在 guest 内 `/workspace`，宿主路径不进 guest（共享子树
    /// 接线诚实后置——Step 3 `shared_host_path: None` 注记同源）。
    async fn wake_agent(&self, agent_id: &str, exec_root: &Path) -> Result<(), String>;

    /// 运行中实例的 agent 清单（slots 投影用；睡眠/冷停实例不在清单——
    /// 与进程档「睡着的槽已离开 map」同语义）。
    fn running_agents(&self) -> Vec<String>;

    /// R2（2026-10-20 复评）+ F2（2026-10-21 三轮复评）：删除联动逐出，
    /// **两段式**（prepare / commit / abort）。`AgentManager::remove` **无条件**
    /// 调 prepare（不读 runtime_backend——切过档的旧 VM 实例一并收敛）：
    /// 进临时拒绝态（新启动被拒）+ 运行实例冷停出 map，**可逆**；真相源
    /// （DB）删除成功后 commit（临时态转永久墓碑），失败则 abort（临时态
    /// 解除，记录仍在册 = 身份可再唤醒）。commit/abort 同步不可失败；三者
    /// **均无默认实现**——默认 Ok/空操作会把未来实现者的漏接静默成老 bug。
    async fn evict_agent_prepare(&self, agent_id: &str) -> Result<(), String>;
    fn evict_agent_commit(&self, agent_id: &str);
    fn evict_agent_abort(&self, agent_id: &str);
}

// ---------------------------------------------------------------------------
// 进程级管理器槽（gateway boot 装配，WSAPI/集群宣告消费）
// ---------------------------------------------------------------------------
// `RwLock<Option<_>>` 可替换而非 OnceLock（同 nemesis-web PROJECTS_BRIDGE 的
// 设计偏差理由）：crate 内测试需要「未装配诚实报错」与「装配后各臂」两类
// 形态并存，并行测试进程内共享静态槽，OnceLock 只能 set 一次会互踩。

static MANAGER_SLOT: std::sync::RwLock<Option<std::sync::Arc<AgentManager>>> =
    std::sync::RwLock::new(None);

/// gateway boot 装配（幂等覆盖——agent 重启重建走同一函数重装）。
pub fn install_manager(m: std::sync::Arc<AgentManager>) {
    *MANAGER_SLOT
        .write()
        .expect("agents manager slot lock poisoned") = Some(m);
}

/// 槽读取（未装配 = None，消费方诚实降级——headless/独立测试无 gateway）。
pub fn manager() -> Option<std::sync::Arc<AgentManager>> {
    MANAGER_SLOT
        .read()
        .expect("agents manager slot lock poisoned")
        .clone()
}

impl AgentManager {
    pub fn from_db(workspace: PathBuf, db: std::sync::Arc<TaskDb>) -> Self {
        Self {
            store: SqliteAgentsStore::from_db(db),
            workspace,
            runtime: std::sync::Arc::new(std::sync::RwLock::new(None)),
            loop_manager: std::sync::Arc::new(std::sync::RwLock::new(None)),
            vm: std::sync::Arc::new(std::sync::RwLock::new(None)),
            #[cfg(feature = "vm")]
            vm_supervisor: std::sync::Arc::new(std::sync::RwLock::new(None)),
            #[cfg(feature = "cluster")]
            migrate_transport: std::sync::Arc::new(std::sync::RwLock::new(None)),
        }
    }

    /// T9：常驻监督器后补注入（supervisor 构造晚于 manager——gateway
    /// post_agent 在两件都在场时接线；幂等覆盖）。
    pub fn set_runtime(&self, rt: std::sync::Arc<crate::agents::runtime::AgentRuntimeSupervisor>) {
        *self.runtime.write().expect("agent manager runtime lock") = Some(rt);
    }

    /// 取监督器快照（clone Arc 出来，不持锁返回）。
    pub fn runtime(
        &self,
    ) -> Option<std::sync::Arc<crate::agents::runtime::AgentRuntimeSupervisor>> {
        self.runtime
            .read()
            .expect("agent manager runtime lock")
            .clone()
    }

    /// G1：自建 agent 常驻 loop 管理器后补注入（同 runtime 形态——loop
    /// manager 需要 main loop 的 session_store，构造晚于 manager；幂等覆盖）。
    pub fn set_loop_manager(
        &self,
        lm: std::sync::Arc<crate::agents::loop_manager::AgentLoopManager>,
    ) {
        *self.loop_manager.write().expect("agent manager loop lock") = Some(lm);
    }

    /// 取 loop manager 快照（未装配 = None——headless/测试形态，会话面
    /// 诚实不可用）。
    pub fn loop_manager(
        &self,
    ) -> Option<std::sync::Arc<crate::agents::loop_manager::AgentLoopManager>> {
        self.loop_manager
            .read()
            .expect("agent manager loop lock")
            .clone()
    }

    /// P5 第 4 步：VM 后端后补注入（supervisor 构造晚于 manager——gateway
    /// post_agent 在两件都在场时接线；幂等覆盖）。唯一调用方在
    /// `#[cfg(feature = "vm")]` 装配块内——vm 关态 dead，allow 门随 feature。
    #[cfg_attr(not(feature = "vm"), allow(dead_code))]
    pub fn set_vm_backend(&self, vm: std::sync::Arc<dyn VmBackend>) {
        *self.vm.write().expect("agent manager vm lock") = Some(vm);
    }

    /// 取 VM 后端快照（feature 关 / 未装配 = None）。
    pub fn vm_backend(&self) -> Option<std::sync::Arc<dyn VmBackend>> {
        self.vm.read().expect("agent manager vm lock").clone()
    }

    /// B9-1：VM 具体型监督器注入（与 set_vm_backend 同一装配点、同一 Arc
    /// 的两投影；幂等覆盖）。
    #[cfg(feature = "vm")]
    pub fn set_vm_supervisor(&self, sup: std::sync::Arc<crate::agents::vm_runtime::VmSupervisor>) {
        *self
            .vm_supervisor
            .write()
            .expect("agent manager vm supervisor lock") = Some(sup);
    }

    /// 取具体型监督器快照（vm 关 / 未装配 = None——vm_sync 诚实不可用）。
    #[cfg(feature = "vm")]
    pub fn vm_supervisor(&self) -> Option<std::sync::Arc<crate::agents::vm_runtime::VmSupervisor>> {
        self.vm_supervisor
            .read()
            .expect("agent manager vm supervisor lock")
            .clone()
    }

    /// P6-R2：迁移传输层注入（cluster Arc 就绪后；幂等覆盖）。
    #[cfg(feature = "cluster")]
    pub fn set_migrate_transport(&self, t: std::sync::Arc<dyn migrate::MigrateTransport>) {
        *self
            .migrate_transport
            .write()
            .expect("agent manager migrate transport lock") = Some(t);
    }

    /// 取迁移传输层快照（cluster 关 / 未注入 = None——迁移面诚实报错）。
    #[cfg(feature = "cluster")]
    pub fn migrate_transport(&self) -> Option<std::sync::Arc<dyn migrate::MigrateTransport>> {
        self.migrate_transport
            .read()
            .expect("agent manager migrate transport lock")
            .clone()
    }

    /// P6-R2：迁移一个 selfbuilt agent 到目标节点。cluster 裁剪形态恒
    /// 不可用（诚实报错）；启用形态全权委托
    /// [`migrate::migrate_agent`](crate::agents::migrate::migrate_agent)
    /// 的 12 步编排（validate/转移/回滚/墓碑语义见该模块头）。
    #[cfg(feature = "cluster")]
    pub async fn migrate_agent(
        &self,
        agent_id: &str,
        target_node: &str,
    ) -> Result<serde_json::Value, String> {
        migrate::migrate_agent(self, agent_id, target_node).await
    }

    /// P6-R2（cluster 裁剪形态）：迁移不可用。
    #[cfg(not(feature = "cluster"))]
    pub async fn migrate_agent(
        &self,
        agent_id: &str,
        _target_node: &str,
    ) -> Result<serde_json::Value, String> {
        let _ = agent_id;
        Err("agent 迁移不可用：cluster feature 未编译".to_string())
    }

    /// B9-1：按项目 ID 查项目 agent 记录（kind=project 且 project_id 匹配；
    /// status 不筛——provisioning/failed 形态下 ensure_running 会诚实失败，
    /// 拦截消息比静默放行诚实）。存储读取失败 = warn + None（消息路由热
    /// 路径不因账本故障整体停车，但留痕）。
    pub fn agent_by_project(&self, project_id: &str) -> Option<AgentRecord> {
        match self.store.list_all() {
            Ok(rows) => rows.into_iter().find(|r| {
                r.kind == agent_kind::PROJECT && r.project_id.as_deref() == Some(project_id)
            }),
            Err(e) => {
                tracing::warn!(
                    "[Agents] agent_by_project({project_id}) 存储读取失败，按无 agent 处理: {e}"
                );
                None
            }
        }
    }

    /// 两阶段创建（自建 agent）：`provisioning` 落库 → home 物化（幂等）→
    /// `active`。exec_root=自己 home（隔离根语义：跨 agent home 互不可见）。
    /// 物化失败 = 记录留在 `provisioning`（启动 sweep 会重试收敛），错误
    /// 上抛给调用方（诚实失败，不留假成功）。
    pub fn create_agent(&self, p: CreateAgentParams) -> Result<AgentRecord, String> {
        let name = p.name.trim();
        if name.is_empty() {
            return Err("agent name 不能为空".to_string());
        }

        let agent_id = uuid::Uuid::new_v4().simple().to_string();
        // 凭据引用面创建期封死（vault:agent/<self_id>/<name> 约定，跨
        // agent 引用拒绝）——先验后落库，失败不留半创建。
        validate_credential_refs(&agent_id, &p.credential_refs)?;
        // 档位值域同点封死（P5 第 4 步；空串同 process——旧调用方零迁移）。
        let runtime_backend =
            nemesis_taskstore::agents::runtime_backend::validate(&p.runtime_backend)?.to_string();
        let home = resolve_agent_home_in_workspace(&self.workspace, &agent_id);
        let mut record = AgentRecord::new_provisioning(&agent_id, name, agent_kind::SELFBUILT);
        record.exec_root = home.to_string_lossy().to_string();
        record.home_path = home_path_str(&self.workspace, &agent_id);
        record.tier = p.tier;
        record.runtime_backend = runtime_backend;
        record.tool_policy = p.tool_policy;
        record.persona_refs = p.persona_refs;
        // selfbuilt 独占 `agents/<id>` 记忆域。
        record.memory_namespace = format!("agents/{agent_id}");
        record.credential_refs = p.credential_refs;
        // P6-R4（D4 预留）：主体标识落库（只立字段，无消费方）。
        record.principal = p.principal;

        // 第一阶段：记录先落库（provisioning）——后续任何失败都留有
        // 可收敛的痕迹，不产生「目录在、记录无」的孤儿。
        self.store.upsert(&record)?;
        // 第二阶段：home 物化（幂等）+ 条件转移。转移失败（并发抢先）
        // 不算错误——记录已 active 即达目的。
        materialize_home(&self.workspace, &agent_id)?;
        self.store.transition_status(
            &agent_id,
            agent_status::PROVISIONING,
            agent_status::ACTIVE,
        )?;
        self.store
            .get(&agent_id)?
            .ok_or_else(|| format!("agent {agent_id} 刚创建即读失（存储不一致）"))
    }

    /// P5 第 4 步：运行时后端切档（G4 ⑦「上层无感」的写入口）。真相源 =
    /// tasks.db 记录字段；工具调用侧路由执行器每调用现读——切档即时生效，
    /// 无需重启 loop 或迁移会话（对话/记忆/交付语义不变）。原档常驻实例
    /// 如需释放，走 sleep（跨档生命周期互不自动接管——切档是偏好变更，
    /// 不是停机指令）。
    pub fn update_backend(
        &self,
        agent_id: &str,
        runtime_backend: &str,
    ) -> Result<AgentRecord, String> {
        let backend =
            nemesis_taskstore::agents::runtime_backend::validate(runtime_backend)?.to_string();
        let mut rec = self
            .get(agent_id)?
            .ok_or_else(|| format!("agent 不存在: {agent_id}"))?;
        if rec.runtime_backend == backend {
            return Ok(rec); // 幂等：同档重复切 = 无操作（诚实返回现值）。
        }
        rec.runtime_backend = backend;
        self.store.upsert(&rec)?;
        tracing::info!(
            agent_id = %agent_id,
            backend = %rec.runtime_backend,
            "[agents] runtime_backend 切档已落库（下次工具调用现读生效）"
        );
        Ok(rec)
    }

    /// main agent 幂等 seed（boot-time）。已存在（任意状态）直接返回；
    /// 缺失才创建。main 记录是对话组的持久身份投影：exec_root=workspace、
    /// memory_namespace 留空（主记忆即 workspace 记忆，行为零变化）。
    pub fn ensure_main_agent(&self) -> Result<AgentRecord, String> {
        if let Some(existing) = self.find_by_kind(agent_kind::MAIN)? {
            return Ok(existing);
        }
        let agent_id = uuid::Uuid::new_v4().simple().to_string();
        let mut record = AgentRecord::new_provisioning(&agent_id, "main", agent_kind::MAIN);
        record.exec_root = self.workspace.to_string_lossy().to_string();
        record.home_path = home_path_str(&self.workspace, &agent_id);
        // main 记忆 = workspace 记忆（留空 = 不分域）。
        record.memory_namespace = String::new();
        self.store.upsert(&record)?;
        materialize_home(&self.workspace, &agent_id)?;
        self.store.transition_status(
            &agent_id,
            agent_status::PROVISIONING,
            agent_status::ACTIVE,
        )?;
        self.store
            .get(&agent_id)?
            .ok_or_else(|| format!("agent {agent_id} 刚创建即读失（存储不一致）"))
    }

    /// 项目 agent 镜像同步（boot-time 幂等）：projects.json 条目 → agents
    /// 表。已存在（按 project_id 匹配）跳过；不存在则创建（exec_root=
    /// 项目目录）。项目移除后表记录保留（与 projects.json「注册不拥有」
    /// 同语义；孤儿由 `owner_of` 链路自然降级）。
    pub fn sync_project_agents(
        &self,
        entries: &[(String, String, PathBuf)],
    ) -> Result<Vec<String>, String> {
        let mut created = Vec::new();
        let known: Vec<String> = self
            .store
            .list_all()?
            .into_iter()
            .filter(|r| r.kind == agent_kind::PROJECT)
            .filter_map(|r| r.project_id)
            .collect();
        for (project_id, name, path) in entries {
            if known.contains(project_id) {
                continue;
            }
            let exec_root = validate_exec_root(path).map_err(|e| {
                format!("project {project_id} exec_root 校验失败（项目目录不可用？）: {e}")
            })?;
            let agent_id = uuid::Uuid::new_v4().simple().to_string();
            let mut record = AgentRecord::new_provisioning(&agent_id, name, agent_kind::PROJECT);
            record.exec_root = exec_root.to_string_lossy().to_string();
            record.home_path = home_path_str(&self.workspace, &agent_id);
            record.project_id = Some(project_id.clone());
            // project 共享 workspace 记忆（与 create_agent 的 kind 规则一致）。
            record.memory_namespace = String::new();
            self.store.upsert(&record)?;
            materialize_home(&self.workspace, &agent_id)?;
            self.store.transition_status(
                &agent_id,
                agent_status::PROVISIONING,
                agent_status::ACTIVE,
            )?;
            created.push(agent_id);
        }
        Ok(created)
    }

    /// 启动 sweep：收敛卡在 `provisioning` 的半创建记录。重物化成功 →
    /// `active`；失败 → `failed` 终态（调用方 WARN 留痕）。P6-R2 起同批
    /// 收敛 `migrating` 残留（cluster 启用形态）：墓碑在场 = 迁移成功
    /// 终局删记录；缺席 = commit 未达回滚 active。
    pub fn sweep(&self) -> Result<SweepReport, String> {
        let mut report = SweepReport {
            activated: Vec::new(),
            failed: Vec::new(),
            migrated_removed: Vec::new(),
            reactivated: Vec::new(),
        };
        for record in self.store.list_all()? {
            if record.status != agent_status::PROVISIONING {
                continue;
            }
            let agent_id = record.agent_id.clone();
            match materialize_home(&self.workspace, &agent_id) {
                Ok(()) => {
                    if self.store.transition_status(
                        &agent_id,
                        agent_status::PROVISIONING,
                        agent_status::ACTIVE,
                    )? {
                        report.activated.push(agent_id);
                    }
                }
                Err(e) => {
                    if self.store.transition_status(
                        &agent_id,
                        agent_status::PROVISIONING,
                        agent_status::FAILED,
                    )? {
                        tracing::warn!(
                            "[Agents] sweep: agent {agent_id} home 物化失败，标 failed（人工可删可重建）: {e}"
                        );
                        report.failed.push(agent_id);
                    }
                }
            }
        }
        // P6-R2：migrating 残留收敛（cluster 裁剪形态无迁移态，恒空跳过）。
        #[cfg(feature = "cluster")]
        migrate::sweep_migrating(&self.store, &self.workspace, &mut report)?;
        Ok(report)
    }

    /// 删除记录。main 是对话组身份投影不可删（项目/自建可删；物理 home
    /// 目录保留——删除是解除实体注册，不做文件清除，与 projects 同语义）。
    /// R4（2026-10-18 三方复评）：删除联动常驻执行体停机（尽力而为——监督
    /// 器未装配时跳过）+ 看护循环经 registration_check 不再为已删 agent 重
    /// 拉；evict 拿槽锁等在途调用收尾，删除不与 execute 并发脱管。G1：同批
    /// 联动停机自建 agent 的常驻 LLM loop（记录删了 loop 不留——此后
    /// `agents.send` 走「agent 不存在」诚实报错）。
    ///
    /// 顺序契约（2026-10-18 六批复查）：**真相源先删、loop 后停**（projects
    /// `remove` 同序）——记录在册时先摘 loop，`lm.remove` 与 `store.remove`
    /// 之间并发抵达的 `agents.send`/`loop_for_agent` 会经 `resolve_or_build`
    /// 查到仍在册的记录把 loop 重建出来（孤儿 loop 存活到重启）；先删记录
    /// 则并发解析立即「agent 不存在」诚实报错，随后 `lm.remove` 收尾停既有
    /// loop。「构建在途」（early get 后、insert 前）的交错由 loop_manager
    /// insert 前的真相源复查封死（`resolve_or_build` 同日注）。store 删除
    /// 失败 = 整个删除失败原样上抛（`lm` 未动，状态一致；runtime evict 已
    /// 发生但子进程按需重建，无泄漏）。
    ///
    /// F3（二轮复评 2026-10-18）：runtime 侧**两段式编排**——`evict_begin`
    /// （置删除在途旗标 + 排空旧槽）先行，旗标覆盖「旧槽已摘、排空等待中」
    /// 与「排空完成、记录未删」两个窗口（期间 `get_or_spawn` 锁内复核拒新，
    /// 删除等待期造不出替代槽）；`store.remove`/`lm.remove` 完成后
    /// `evict_end` 清旗标。store.remove 失败也要清旗标（不得把失败删除泄漏
    /// 成永久拒绝），故不用 `?` 中途返回。
    ///
    /// R2（2026-10-20 复评）：删除联动 VM 后端收敛——`evict_begin` 之前
    /// **无条件**走 VM 逐出（不读 runtime_backend：切过档的旧 VM 实例一并
    /// 收敛）。F2（2026-10-21 三轮复评）改**两段式**：prepare（临时拒绝态
    /// 加冷停出槽，可逆）先行；`store.remove` 成功 → commit（永久墓碑）；
    /// 失败 → abort（临时态解除，记录仍在册 = 身份可再唤醒，已冷停实例走
    /// 正常冷启恢复）。旧单段式的事故面（探针实证）：墓碑先于 DB 删除永久
    /// 生效，DB 失败后「记录在册但身份永久不可唤醒」直到 supervisor 重启。
    /// 显式**不采用**「先删库后停机」换序——那会重新产生已删身份的脱管
    /// 运行实例（R2 整改的原始动机）。
    pub async fn remove(&self, agent_id: &str) -> Result<(), String> {
        if let Some(record) = self.store.get(agent_id)?
            && record.kind == agent_kind::MAIN
        {
            return Err("main agent 是对话组身份投影，不可删除".to_string());
        }
        if let Some(vm) = self.vm_backend() {
            vm.evict_agent_prepare(agent_id).await.map_err(|e| {
                format!("VM 后端逐出准备失败，删除中止（记录未动，重试即干净）: {e}")
            })?;
        }
        if let Some(rt) = self.runtime() {
            rt.evict_begin(agent_id).await;
        }
        let result = self.store.remove(agent_id);
        match &result {
            Ok(()) => {
                if let Some(lm) = self.loop_manager() {
                    lm.remove(agent_id);
                }
                if let Some(vm) = self.vm_backend() {
                    vm.evict_agent_commit(agent_id);
                }
            }
            Err(_) => {
                // 删除失败：解除 VM 临时拒绝态（永久墓碑不落——身份保持
                // 可唤醒）；process 侧旗标由下方 evict_end 统一清。
                if let Some(vm) = self.vm_backend() {
                    vm.evict_agent_abort(agent_id);
                }
            }
        }
        if let Some(rt) = self.runtime() {
            rt.evict_end(agent_id);
        }
        result.map(|_| ())
    }

    pub fn find_by_kind(&self, kind: &str) -> Result<Option<AgentRecord>, String> {
        Ok(self.store.list_all()?.into_iter().find(|r| r.kind == kind))
    }

    pub fn list(&self) -> Result<Vec<AgentRecord>, String> {
        self.store.list_all()
    }

    pub fn get(&self, agent_id: &str) -> Result<Option<AgentRecord>, String> {
        self.store.get(agent_id)
    }

    pub fn touch(&self, agent_id: &str) -> Result<(), String> {
        self.store.touch_last_active(agent_id)
    }
}

/// home 物化路径字符串（nemesis-path 单一真相源派生）。
fn home_path_str(workspace: &Path, agent_id: &str) -> String {
    resolve_agent_home_in_workspace(workspace, agent_id)
        .to_string_lossy()
        .to_string()
}

/// home 物化（幂等）：home 目录 + 记忆命名空间目录。`create_dir_all`
/// 天然幂等——重跑零副作用，这是 sweep 续建的安全性基础。
fn materialize_home(workspace: &Path, agent_id: &str) -> Result<(), String> {
    let home = resolve_agent_home_in_workspace(workspace, agent_id);
    std::fs::create_dir_all(&home)
        .map_err(|e| format!("agent home 物化失败 ({}): {e}", home.display()))?;
    let memory = resolve_agent_memory_dir_in_workspace(workspace, agent_id);
    std::fs::create_dir_all(&memory)
        .map_err(|e| format!("agent 记忆目录物化失败 ({}): {e}", memory.display()))?;
    Ok(())
}

/// exec_root 校验：绝对路径 + 已存在（执行根不预创建——目录缺失意味着
/// 调用方给了错误目标）。
fn validate_exec_root(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("exec_root 必须是绝对路径: {}", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("exec_root 目录不存在: {}", path.display()));
    }
    Ok(path.to_path_buf())
}

// ---------------------------------------------------------------------------
// 凭据 vault 别名（P2：`vault:agent/<id>/<name>`，plugin 别名先例
// `vault:plugin/<slug>/<name>` 同构）
// ---------------------------------------------------------------------------

/// agent 凭据别名（`vault:` 全前缀形态，即 vault entry key 加前缀）。
/// 凭据本体只存 vault（AES-256-GCM），agents 表的 credential_refs 只存
/// 引用——web 面投影只出条数不出内容（bridge.rs 同纪律）。
///
/// `#[allow(dead_code)]`：P5 消费面（agent 实体持久化/重启链的凭据装配），
/// 当前无生产调用方；格式由 tests.rs `credential_alias_format` 钉住。
#[allow(dead_code)]
pub fn credential_alias(agent_id: &str, name: &str) -> String {
    format!("vault:agent/{agent_id}/{name}")
}

/// credential_refs 校验：数组形态，每条必须是 `vault:agent/<self_id>/<name>`。
///
/// 跨 agent 引用在创建期就拒绝（隔离性质：A 的记录拿不到 B 的凭据——
/// 引用面封死，不留给运行期解析层兜底）。name 段非空且不含 `vault:` 再套
/// （畸形引用宁可创建期 loud 报错）。
pub fn validate_credential_refs(agent_id: &str, refs: &serde_json::Value) -> Result<(), String> {
    let Some(items) = refs.as_array() else {
        return Err("credential_refs 必须是数组".to_string());
    };
    let prefix = format!("vault:agent/{agent_id}/");
    for item in items {
        let Some(s) = item.as_str() else {
            return Err("credential_refs 条目必须是字符串（vault 别名）".to_string());
        };
        if !s.starts_with(&prefix) {
            return Err(format!(
                "credential_refs 条目 {s:?} 非法：必须是 {prefix:?} 形态（跨 agent 凭据引用拒绝）"
            ));
        }
        let name = s.strip_prefix(&prefix).unwrap();
        if name.is_empty() || name.contains("vault:") {
            return Err(format!("credential_refs 条目 {s:?} 的凭据名段非法"));
        }
    }
    Ok(())
}

/// 解析 agent 凭据（fail loud，vault_runtime 同源每次现开现解锁——
/// `vault set`/轮换落盘后下一次解析即生效）。vault 本体在 nemesis-security
/// （安全子系统）里——security 裁剪形态无 vault 可解析，本函数随之编译期
/// 消失。
///
/// `#[allow(dead_code)]`：P5 消费面（常驻执行体 env 凭据装配，注入契约
/// 随 P5/P6 讨论定形）；当前无生产调用方，全链路行为由 tests.rs
/// `resolve_agent_credential_roundtrip` 钉住（真 vault 落盘 → 解析出真值）。
#[cfg(feature = "security")]
#[allow(dead_code)]
pub fn resolve_agent_credential(
    workspace: &Path,
    agent_id: &str,
    name: &str,
) -> Result<String, String> {
    let vp = nemesis_path::resolve_vault_path_in_workspace(workspace);
    crate::vault_runtime::resolve_alias_at(&vp, &format!("agent/{agent_id}/{name}"))
}
