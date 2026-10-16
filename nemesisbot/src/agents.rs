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
    pub tool_policy: serde_json::Value,
    pub persona_refs: serde_json::Value,
    pub credential_refs: serde_json::Value,
}

/// sweep 收敛结果（WSAPI/日志可观测面）。
#[derive(Debug, Clone, PartialEq)]
pub struct SweepReport {
    /// provisioning → active（续建成功）。
    pub activated: Vec<String>,
    /// provisioning → failed（物化失败，WARN 留痕）。
    pub failed: Vec<String>,
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
        let home = resolve_agent_home_in_workspace(&self.workspace, &agent_id);
        let mut record = AgentRecord::new_provisioning(&agent_id, name, agent_kind::SELFBUILT);
        record.exec_root = home.to_string_lossy().to_string();
        record.home_path = home_path_str(&self.workspace, &agent_id);
        record.tier = p.tier;
        record.tool_policy = p.tool_policy;
        record.persona_refs = p.persona_refs;
        // selfbuilt 独占 `agents/<id>` 记忆域。
        record.memory_namespace = format!("agents/{agent_id}");
        record.credential_refs = p.credential_refs;

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
    /// `active`；失败 → `failed` 终态（调用方 WARN 留痕）。
    pub fn sweep(&self) -> Result<SweepReport, String> {
        let mut report = SweepReport {
            activated: Vec::new(),
            failed: Vec::new(),
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
        Ok(report)
    }

    /// 删除记录。main 是对话组身份投影不可删（项目/自建可删；物理 home
    /// 目录保留——删除是解除实体注册，不做文件清除，与 projects 同语义）。
    pub fn remove(&self, agent_id: &str) -> Result<(), String> {
        if let Some(record) = self.store.get(agent_id)?
            && record.kind == agent_kind::MAIN
        {
            return Err("main agent 是对话组身份投影，不可删除".to_string());
        }
        self.store.remove(agent_id)
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
/// `vault set`/轮换落盘后下一次解析即生效）。P3 常驻执行体 env 白名单
/// 注入是这个函数的主要消费方。vault 本体在 nemesis-security（安全
/// 子系统）里——security 裁剪形态无 vault 可解析，本函数随之编译期消失。
#[cfg(feature = "security")]
pub fn resolve_agent_credential(
    workspace: &Path,
    agent_id: &str,
    name: &str,
) -> Result<String, String> {
    let vp = nemesis_path::resolve_vault_path_in_workspace(workspace);
    crate::vault_runtime::resolve_alias_at(&vp, &format!("agent/{agent_id}/{name}"))
}
