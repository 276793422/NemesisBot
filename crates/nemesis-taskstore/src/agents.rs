//! `SqliteAgentsStore` —— Agent 持久实体真相源（agents 表，tasks.db 同库）。
//!
//! P2 地基件。Agent 记录 = 持久身份（uuid）+ home/记忆/凭据引用 + 执行根，
//! 跨 gateway 重启存活。三类组并存：`main`（对话组，启动时幂等 seed）、
//! `project`（L6++ 项目 loop 收编形态，exec_root=项目目录）、`selfbuilt`
//! （用户自建，home 钉死 `<workspace>/agents/<id>/home/`）。
//!
//! 两阶段创建契约（D6 复合操作纪律）：记录先落 `provisioning` → home 物化
//! （幂等可重跑）→ `transition_status` 到 `active`；启动 sweep 对卡在
//! `provisioning` 的半创建记录收敛（续建或标 `failed`），杜绝中间态。
//! 状态转移走 `UPDATE ... WHERE status=?` 条件更新（先到先得，与审批
//! 台账同款纪律）。

use std::sync::Arc;

use rusqlite::{OptionalExtension, params};

use crate::db::TaskDb;
use crate::tasks::now_rfc3339;

/// Agent 记录生命周期状态常量（`kind`/`status` 值域单一真相源）。
pub mod status {
    /// 两阶段创建第一阶段：记录已落库、home 未物化完成。
    pub const PROVISIONING: &str = "provisioning";
    /// 可用：home 物化完成。
    pub const ACTIVE: &str = "active";
    /// sweep 判定的失败终态（半创建无法续建时）；人工可删可重建。
    pub const FAILED: &str = "failed";
    /// 跨节点迁移进行中（P6-R2）：A 端迁移编排置入，源端 loop 冻结
    /// （resolve_or_build 的 `status != ACTIVE` 闸拒绝新消息）。收敛由
    /// [`super::store`] 的 sweep 负责——本地墓碑在场 = commit ACK 已达
    /// （迁移成功终局），删记录+删墓碑；墓碑缺席 + 意图日志在场 = commit
    /// 已发出、结果未知，**冻结**转人工（绝不自动回滚——B 端可能已激活，
    /// 回滚 = 同 uuid 双活且 ACTIVE 让 sweep 失明）；两锚皆缺席 = commit
    /// 未发出，回滚 active 照常服务。
    pub const MIGRATING: &str = "migrating";
}

/// Agent 记录类别常量。
pub mod kind {
    /// 对话组默认 agent（主 loop 的持久身份投影）。
    pub const MAIN: &str = "main";
    /// L6++ 项目 loop 收编形态（exec_root=项目目录）。
    pub const PROJECT: &str = "project";
    /// 用户自建 agent。
    pub const SELFBUILT: &str = "selfbuilt";
}

/// 运行时后端档位常量（P5 第 4 步接入面：per-agent 工具执行走哪个常驻
/// 执行体）。字段本体 [`AgentRecord::runtime_backend`]——P2 建表即预留。
pub mod runtime_backend {
    /// 进程档（默认）：常驻子进程/沙盒档，工作区 = 宿主 exec_root。
    pub const PROCESS: &str = "process";
    /// VM 档（P5）：桌面级 VM 常驻执行体，工作区 = guest 内 `/workspace`。
    pub const VM: &str = "vm";

    /// 值域校验（单一真相源——create/update 落库前必须过这里；空串同
    /// process 兼容旧调用方）。非法值 loud 拒绝，不静默纠偏。
    pub fn validate(value: &str) -> Result<&str, String> {
        match value.trim() {
            "" | PROCESS => Ok(PROCESS),
            VM => Ok(VM),
            other => Err(format!(
                "runtime_backend 非法: {other:?}（允许 process|vm，空串同 process）"
            )),
        }
    }
}

/// Agent 持久记录（agents 表行投影；JSON 列以 `serde_json::Value` 承载，
/// 损坏 JSON 读取时诚实降级为 `Null` 并保留原文可查——schema 演进由
/// 增列迁移承接，不在读取路径做静默修补）。
///
/// Serialize/Deserialize（P6-R2）：跨节点迁移载荷承载（A 端打包 / B 端
/// 解析）——纯增量 derive，DB 层仍是手工行投影（`row_record`），互不
/// 影响。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AgentRecord {
    pub agent_id: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    /// 人格文件来源引用（IDENTITY/SOUL/USER/AGENT；JSON）。
    pub persona_refs: serde_json::Value,
    /// 记忆命名空间键（`agents/<agent_id>`；物理位置由 home 派生）。
    pub memory_namespace: String,
    /// 执行根（canonical 绝对路径）：自建=home，项目=项目目录，main=workspace。
    pub exec_root: String,
    /// home 物化路径（`<workspace>/agents/<id>/home/`）。
    pub home_path: String,
    /// 凭据引用（vault 别名数组；JSON）。
    pub credential_refs: serde_json::Value,
    /// 工具策略（allow/deny/hidden；JSON）。
    pub tool_policy: serde_json::Value,
    /// 档位（`auto|mini|normal|big`，空 = auto）。
    pub tier: String,
    /// 运行时后端（P2 恒 `process`；`vm` 预留 P5）。
    pub runtime_backend: String,
    /// 多租户主体（D4 预留，P2 恒空串）。
    pub principal: String,
    /// 归属节点 id（D1：集群可寻址）。
    pub home_node: String,
    /// `kind=project` 时关联的项目 id。
    pub project_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_active_at: Option<String>,
}

impl AgentRecord {
    /// 新记录骨架（`provisioning` 起点；记忆命名空间与 home 由调用方按
    /// workspace 布局填入——本 crate 不依赖 nemesis-path）。
    pub fn new_provisioning(agent_id: &str, name: &str, record_kind: &str) -> Self {
        let now = now_rfc3339();
        Self {
            agent_id: agent_id.to_string(),
            name: name.to_string(),
            kind: record_kind.to_string(),
            status: status::PROVISIONING.to_string(),
            persona_refs: serde_json::Value::Null,
            memory_namespace: format!("agents/{agent_id}"),
            exec_root: String::new(),
            home_path: String::new(),
            credential_refs: serde_json::json!([]),
            tool_policy: serde_json::json!({}),
            tier: String::new(),
            runtime_backend: "process".to_string(),
            principal: String::new(),
            home_node: String::new(),
            project_id: None,
            created_at: now.clone(),
            updated_at: now,
            last_active_at: None,
        }
    }
}

/// Agent 记录存储（clone 廉价，共享同一 `Arc<TaskDb>`——与任务真相源同库）。
#[derive(Clone)]
pub struct SqliteAgentsStore {
    db: Arc<TaskDb>,
}

impl SqliteAgentsStore {
    pub fn from_db(db: Arc<TaskDb>) -> Self {
        Self { db }
    }

    /// Upsert（元数据级全量写；状态转移必须走 [`Self::transition_status`]，
    /// 防止并发两阶段创建相互覆盖状态）。
    pub fn upsert(&self, record: &AgentRecord) -> Result<(), String> {
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO agents (agent_id, name, kind, status, persona_refs, \
                 memory_namespace, exec_root, home_path, credential_refs, tool_policy, \
                 tier, runtime_backend, principal, home_node, project_id, \
                 created_at, updated_at, last_active_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
                 ON CONFLICT(agent_id) DO UPDATE SET
                    name=excluded.name, kind=excluded.kind, status=excluded.status,
                    persona_refs=excluded.persona_refs,
                    memory_namespace=excluded.memory_namespace,
                    exec_root=excluded.exec_root, home_path=excluded.home_path,
                    credential_refs=excluded.credential_refs,
                    tool_policy=excluded.tool_policy, tier=excluded.tier,
                    runtime_backend=excluded.runtime_backend,
                    principal=excluded.principal, home_node=excluded.home_node,
                    project_id=excluded.project_id, updated_at=excluded.updated_at,
                    last_active_at=excluded.last_active_at",
                params![
                    record.agent_id,
                    record.name,
                    record.kind,
                    record.status,
                    record.persona_refs.to_string(),
                    record.memory_namespace,
                    record.exec_root,
                    record.home_path,
                    record.credential_refs.to_string(),
                    record.tool_policy.to_string(),
                    record.tier,
                    record.runtime_backend,
                    record.principal,
                    record.home_node,
                    record.project_id,
                    record.created_at,
                    record.updated_at,
                    record.last_active_at,
                ],
            )
            .map_err(|e| format!("tasks.db upsert agent failed: {e}"))?;
            Ok(())
        })
    }

    pub fn get(&self, agent_id: &str) -> Result<Option<AgentRecord>, String> {
        self.db.with_conn(|conn| {
            conn.query_row(AGENT_SELECT, params![agent_id], row_record)
                .optional()
                .map_err(|e| format!("tasks.db get agent failed: {e}"))
        })
    }

    /// 全量列举（created_at, agent_id 稳定序——跨平台 read_dir 顺序不依赖）。
    pub fn list_all(&self) -> Result<Vec<AgentRecord>, String> {
        self.db.with_conn(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {AGENT_COLUMNS} FROM agents ORDER BY created_at, agent_id"
                ))
                .map_err(|e| format!("tasks.db list agents failed: {e}"))?;
            let rows = stmt
                .query_map([], row_record)
                .map_err(|e| format!("tasks.db list agents failed: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("tasks.db list agents row failed: {e}"))
        })
    }

    /// 状态条件转移：`from` 匹配才落 `to`（先到先得；返回是否本调用生效）。
    pub fn transition_status(&self, agent_id: &str, from: &str, to: &str) -> Result<bool, String> {
        self.db.with_conn(|conn| {
            let n = conn
                .execute(
                    "UPDATE agents SET status=?1, updated_at=?2 \
                     WHERE agent_id=?3 AND status=?4",
                    params![to, now_rfc3339(), agent_id, from],
                )
                .map_err(|e| format!("tasks.db transition agent status failed: {e}"))?;
            Ok(n == 1)
        })
    }

    pub fn remove(&self, agent_id: &str) -> Result<(), String> {
        self.db.with_conn(|conn| {
            conn.execute("DELETE FROM agents WHERE agent_id=?1", params![agent_id])
                .map_err(|e| format!("tasks.db remove agent failed: {e}"))?;
            Ok(())
        })
    }

    pub fn touch_last_active(&self, agent_id: &str) -> Result<(), String> {
        self.db.with_conn(|conn| {
            conn.execute(
                "UPDATE agents SET last_active_at=?1, updated_at=?1 WHERE agent_id=?2",
                params![now_rfc3339(), agent_id],
            )
            .map_err(|e| format!("tasks.db touch agent failed: {e}"))?;
            Ok(())
        })
    }
}

const AGENT_COLUMNS: &str = "agent_id, name, kind, status, persona_refs, memory_namespace, \
     exec_root, home_path, credential_refs, tool_policy, tier, runtime_backend, \
     principal, home_node, project_id, created_at, updated_at, last_active_at";
// 列清单单一真相源：按位置消费于 row_record，增列时同步改 row_record。
const AGENT_SELECT: &str = "SELECT agent_id, name, kind, status, persona_refs, \
     memory_namespace, exec_root, home_path, credential_refs, tool_policy, tier, \
     runtime_backend, principal, home_node, project_id, created_at, updated_at, \
     last_active_at FROM agents WHERE agent_id = ?1";
fn row_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRecord> {
    let persona_refs: String = r.get(4)?;
    let credential_refs: String = r.get(8)?;
    let tool_policy: String = r.get(9)?;
    Ok(AgentRecord {
        agent_id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        status: r.get(3)?,
        persona_refs: serde_json::from_str(&persona_refs).unwrap_or(serde_json::Value::Null),
        memory_namespace: r.get(5)?,
        exec_root: r.get(6)?,
        home_path: r.get(7)?,
        credential_refs: serde_json::from_str(&credential_refs).unwrap_or(serde_json::Value::Null),
        tool_policy: serde_json::from_str(&tool_policy).unwrap_or(serde_json::Value::Null),
        tier: r.get(10)?,
        runtime_backend: r.get(11)?,
        principal: r.get(12)?,
        home_node: r.get(13)?,
        project_id: r.get(14)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
        last_active_at: r.get(17)?,
    })
}
