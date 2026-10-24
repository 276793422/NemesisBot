//! Agent 跨节点迁移（P6-R2，`cluster` feature 闸——裁剪/IoT 形态整模块
//! 不进二进制）。
//!
//! 语义总纲（决策记录 2026-10-21）：
//!
//! - **仅 selfbuilt 可迁**：main 是对话组身份投影（不可迁移不可删），
//!   project 绑定项目目录（exec_root=项目路径，跨节点无意义）；只有
//!   selfbuilt 的 home/memory/凭据是自包含实体。
//! - **在途任务诚实拒绝**：`loop_manager` 渠道有排队消息 = 拒绝迁移
//!   （等消息消费完）；loop 已停（`lm.remove` abort 在途 turn）后的
//!   「未完成回复」是可接受的既有语义（与 agent 删除同源）。
//! - **VM 档不可迁**：guest 内执行体与 overlay 差量不随迁（跨节点无
//!   保证）——诚实拒绝，先切回 process 档。
//! - **凭据随迁**：`credential_refs` 指向的 vault 别名整包导出（随机
//!   一次性口令包裹）随 commit RPC 上行，B 端 import 进自己的 vault。
//!   一次性口令只在本迁移事务内存活，不落盘不落日志。
//! - **12 步 A 端编排**（validate → target 校验 → active→migrating 条件
//!   转移 → queued 检查 → 停 loop → 睡执行体 → pack staging → vault
//!   export → push_transfer_dir → commit RPC → 本地收敛/回滚）。任何
//!   失败回滚 `migrating→active`；成功写墓碑 + 删记录（顺序不可倒——
//!   崩溃窗口留给 sweep 收敛，见 `status::MIGRATING` 注）。
//! - **诚实窗口**（v1 接受，文档留痕）：commit ACK 后本地删除前崩溃 =
//!   短暂双活（B 真活，A 残记录由 sweep 经墓碑收敛）；B 激活后 ACK
//!   丢失 = A 回滚 active 与 B 同 uuid 双活（B 份更新），人工二选一。
//!   N2 根修（2026-10-24）：ACK 后本地收敛失败（store.remove 报错等）
//!   与崩溃同族处置——错误带 [`POST_ACK_ERR_MARK`] 前缀，外层 ⑫ **不
//!   回滚**、保持 migrating 等墓碑收敛（旧行为一视同仁回滚 active =
//!   ACK 后删除失败即永久双活且 sweep 失明）；墓碑写入重试 ×3，全败
//!   且记录删除也失败时 sweep 按无墓碑回滚 = 上述第二类诚实窗口。
//!   R7 根修（2026-10-24 复核）：上窗「墓碑全败 + 删除全败 → sweep 回滚
//!   active」的自动双活不可接受——commit 发出前落**意图日志**（R7），sweep
//!   对「日志在场、墓碑缺席」一律冻结 migrating 转人工（B 可能已激活，
//!   自动回滚 = 同 uuid 双活且 ACTIVE 让 sweep 永久失明）；两锚皆缺席
//!   （崩溃在日志写入前）才回滚 active。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nemesis_taskstore::agents::{kind as agent_kind, status as agent_status};
use nemesis_taskstore::{AgentRecord, SqliteAgentsStore};

/// commit RPC action 名（A→B；B 端 [`handle_migrate_commit`] 消费）。
pub const ACTION_AGENT_MIGRATE_COMMIT: &str = "agent_migrate_commit";

/// commit RPC 等待上限（B 端落 home + vault import；大 home 目录跨节点
/// 已由 transfer 分块通道先行完成，commit 只做 rename + import，300s
/// 远超需要，取整百防慢盘）。
const COMMIT_RPC_TIMEOUT: Duration = Duration::from_secs(300);

/// 迁移 staging 载荷格式版本（migration.json `format` 字段）。
const MIGRATION_FORMAT: u32 = 1;

/// post-ACK 错误标记（2026-10-24 N2 根修）：⑩ commit ACK 已达（B 端已
/// 激活）之后的本地收敛失败，错误串以本前缀起头——外层 ⑫ 回滚臂据此
/// **跳过回滚**（B 活 + A 回滚 active = 同 uuid 双活，且 ACTIVE 状态让
/// sweep 永久失明，双活从短暂窗口变永久态）；保持 migrating 让墓碑收敛。
/// 前缀匹配是刻意选择：错误串经多层 format! 包裹（外层还要附保持语义
/// 说明），结构化 enum 会强改全部消费面签名，标记前缀零涟漪。
const POST_ACK_ERR_MARK: &str = "commit 已确认（B 端已激活）";

// ---------------------------------------------------------------------------
// 传输抽象（trait 化以便测试注入假 transport；生产实现包 `Arc<Cluster>`）
// ---------------------------------------------------------------------------

/// agent 迁移对集群传输面的最小依赖投影。与 outbox 的
/// [`nemesis_cluster::outbox::TransferTransport`] 刻意不同源：迁移是
/// 破坏性操作，`is_peer_online` 的未知节点（None）必须按 fail-closed
/// 拒绝——不复用 outbox `unwrap_or(true)`（推送兜底语义）。
#[async_trait::async_trait]
pub trait MigrateTransport: Send + Sync {
    /// 本节点 id（self 比对 + commit RPC source 身份——T37 教训：发送方
    /// 身份必须显式填，不依赖下层代填）。
    fn self_node_id(&self) -> String;

    /// 目标名/id 归一化（peer name → node id；None = 未知，调用方保留
    /// 原值让 RPC 层诚实失败）。
    fn canonical_peer(&self, target: &str) -> Option<String>;

    /// 对端在线判定。**未知节点 = false（迁移侧 fail-closed）**。
    fn is_peer_online(&self, peer: &str) -> bool;

    /// 推送 staging 目录到对端 inbox（transfer 分块通道；kind=
    /// [`nemesis_cluster::transfer::TRANSFER_KIND_AGENT_HOME`]，上限 0 =
    /// 不设限——迁移方已过本地 staging 打包，超限在此诚实报错）。
    async fn push_dir(
        &self,
        peer: &str,
        task_id: &str,
        dir: &Path,
    ) -> Result<nemesis_cluster::outbox::PushDirOutcome, String>;

    /// 调用对端 RPC action（commit）。
    async fn call(
        &self,
        peer: &str,
        action: &str,
        payload: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String>;
}

/// 生产实现：包 `Arc<Cluster>`（`nemesis_cluster::cluster::Cluster`）。
pub struct ClusterMigrateTransport {
    pub cluster: Arc<nemesis_cluster::cluster::Cluster>,
}

#[async_trait::async_trait]
impl MigrateTransport for ClusterMigrateTransport {
    fn self_node_id(&self) -> String {
        self.cluster.node_id().to_string()
    }

    fn canonical_peer(&self, target: &str) -> Option<String> {
        self.cluster.canonical_peer_id(target)
    }

    fn is_peer_online(&self, peer: &str) -> bool {
        // 未知节点（None）= false：迁移是破坏性操作，fail-closed
        // （与 outbox 推送侧 unwrap_or(true) 刻意不同源，见 trait 注）。
        self.cluster
            .rpc_client_arc()
            .and_then(|rc| rc.is_peer_online(peer))
            .unwrap_or(false)
    }

    async fn push_dir(
        &self,
        peer: &str,
        task_id: &str,
        dir: &Path,
    ) -> Result<nemesis_cluster::outbox::PushDirOutcome, String> {
        let rpc = self
            .cluster
            .rpc_client_arc()
            .ok_or_else(|| "集群 RPC 客户端未装配（集群未启动？）".to_string())?;
        let transport = nemesis_cluster::outbox::RpcTransferTransport::new(
            rpc,
            self.cluster.node_id().to_string(),
        );
        nemesis_cluster::outbox::push_transfer_dir(
            &transport,
            peer,
            task_id,
            nemesis_cluster::transfer::TRANSFER_KIND_AGENT_HOME,
            self.cluster.node_id(),
            dir,
            0,
        )
        .await
    }

    async fn call(
        &self,
        peer: &str,
        action: &str,
        payload: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        use nemesis_cluster::outbox::TransferTransport as _;
        let rpc = self
            .cluster
            .rpc_client_arc()
            .ok_or_else(|| "集群 RPC 客户端未装配（集群未启动？）".to_string())?;
        let transport = nemesis_cluster::outbox::RpcTransferTransport::new(
            rpc,
            self.cluster.node_id().to_string(),
        );
        transport.call(peer, action, payload, timeout).await
    }
}

// ---------------------------------------------------------------------------
// 纯函数 helpers（路径/校验/改写——单测面）
// ---------------------------------------------------------------------------

/// 迁移 transfer task_id（确定性：B 端用同串算 landing 路径；字母数字
/// 连字符过 `sanitize_path_segment` 恒等，见
/// `validate_migrate_agent_id`）。
pub fn migrate_task_id(agent_id: &str) -> String {
    format!("agent-migrate-{agent_id}")
}

/// agent_id 迁移资格校验（将成为两端目录名）：非空、≤128、仅 ASCII
/// 字母数字连字符、`sanitize_path_segment` 恒等（landing 路径与 task_id
/// 不漂移——生产 id 是 uuid simple，恒过）。
pub fn validate_migrate_agent_id(agent_id: &str) -> Result<(), String> {
    if agent_id.is_empty() {
        return Err("agent_id 不能为空".to_string());
    }
    if agent_id.len() > 128 {
        return Err(format!("agent_id 过长（>128）: {agent_id:?}"));
    }
    if !agent_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(format!(
            "agent_id 含非法字符（仅允许 ASCII 字母数字与连字符）: {agent_id:?}"
        ));
    }
    if nemesis_utils::sanitize::sanitize_path_segment(agent_id) != agent_id {
        return Err(format!(
            "agent_id 未通过目录名归一校验（sanitize 恒等）: {agent_id:?}"
        ));
    }
    Ok(())
}

/// B 端记录改写（纯函数）：home_node 换靶、status 归 active、exec_root/
/// home_path 按靶机 workspace 重派生（nemesis-path 单一真相源）、记忆
/// 命名空间不变（`agents/<id>` 语义跨节点稳定）。字段写前快照用于
/// 审计日志（返回改动摘要）。
pub fn rewrite_record_for_target(
    record: &mut AgentRecord,
    target_node_id: &str,
    target_workspace: &Path,
) -> Vec<&'static str> {
    let mut changed = Vec::new();
    if record.home_node != target_node_id {
        record.home_node = target_node_id.to_string();
        changed.push("home_node");
    }
    if record.status != agent_status::ACTIVE {
        record.status = agent_status::ACTIVE.to_string();
        changed.push("status");
    }
    let home = nemesis_path::resolve_agent_home_in_workspace(target_workspace, &record.agent_id);
    let home_str = home.to_string_lossy().to_string();
    if record.exec_root != home_str {
        record.exec_root = home_str.clone();
        changed.push("exec_root");
    }
    if record.home_path != home_str {
        record.home_path = home_str;
        changed.push("home_path");
    }
    changed
}

// ---------------------------------------------------------------------------
// 墓碑（commit ACK 达成标记；sweep 收敛依据）
// ---------------------------------------------------------------------------

/// 迁移墓碑（`<workspace>/agents/<id>.migrated.json`）。写入时机 =
/// commit ACK 已达、本地记录待删（顺序契约见 [`migrate_agent_local_finalize`]）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MigrationTombstone {
    pub agent_id: String,
    pub target_node: String,
    pub migrated_at: String,
}

pub fn tombstone_path(workspace: &Path, agent_id: &str) -> PathBuf {
    workspace
        .join("agents")
        .join(format!("{agent_id}.migrated.json"))
}

pub fn tombstone_exists(workspace: &Path, agent_id: &str) -> bool {
    tombstone_path(workspace, agent_id).is_file()
}

fn write_tombstone(workspace: &Path, agent_id: &str, target_node: &str) -> Result<PathBuf, String> {
    let path = tombstone_path(workspace, agent_id);
    let t = MigrationTombstone {
        agent_id: agent_id.to_string(),
        target_node: target_node.to_string(),
        migrated_at: chrono::Utc::now().to_rfc3339(),
    };
    let json = serde_json::to_vec_pretty(&t).map_err(|e| format!("墓碑序列化失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("墓碑写入失败 ({}): {e}", path.display()))?;
    Ok(path)
}

fn remove_tombstone(workspace: &Path, agent_id: &str) {
    let _ = std::fs::remove_file(tombstone_path(workspace, agent_id));
}

/// 迁移意图日志（R7，2026-10-24 复核：`<workspace>/agents/<id>.migrating.json`）。
///
/// 写入时机 = ⑩ commit RPC **发出前**——给 sweep 的「无墓碑 = commit 未达」
/// 推断一个下界：日志在场 = commit 已发出（或正在发出），B 端**可能已激活**，
/// sweep 禁止自动回滚 active（同 uuid 双活），冻结 migrating + WARN 转人工。
/// 人工裁决路径：B 端确认已激活 → 落墓碑（sweep 下一轮收敛删记录）；B 端
/// 确认未收到 → 删日志（sweep 下一轮回滚 active 照常服务）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MigrationIntentJournal {
    pub agent_id: String,
    pub target_node: String,
    pub started_at: String,
}

pub fn journal_path(workspace: &Path, agent_id: &str) -> PathBuf {
    workspace
        .join("agents")
        .join(format!("{agent_id}.migrating.json"))
}

pub fn journal_exists(workspace: &Path, agent_id: &str) -> bool {
    journal_path(workspace, agent_id).is_file()
}

/// 写意图日志（commit 发出前的安全锚；写入点在不可逆点**之前**，失败必须
/// 重试争取、全败 = 诚实中止迁移——此时回滚零代价，带锚带病上路不可接受；
/// 与墓碑的「不可逆点之后尽力而为」刻意不对称）。
fn write_journal(workspace: &Path, agent_id: &str, target_node: &str) -> Result<(), String> {
    let path = journal_path(workspace, agent_id);
    let j = MigrationIntentJournal {
        agent_id: agent_id.to_string(),
        target_node: target_node.to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
    };
    let json = serde_json::to_vec_pretty(&j).map_err(|e| format!("意图日志序列化失败: {e}"))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("意图日志写入失败 ({}): {e}", path.display()))?;
    Ok(())
}

fn remove_journal(workspace: &Path, agent_id: &str) {
    let _ = std::fs::remove_file(journal_path(workspace, agent_id));
}

/// 迁移 staging 根（A 端打包目录；B 端 landing 由 transfer sink 布局
/// `<inbox>/<task_id>/files/` 派生，见 [`landing_files_dir`]）。
pub fn migrate_staging_root(workspace: &Path, agent_id: &str) -> PathBuf {
    workspace.join("cluster").join("migrate_out").join(agent_id)
}

/// B 端 landing files 目录（transfer sink 落地布局：
/// `<workspace>/cluster/inbox/<task_id>/files/`；task_id 过 sanitize
/// 恒等——`validate_migrate_agent_id` 保证）。
pub fn landing_files_dir(workspace: &Path, agent_id: &str) -> PathBuf {
    workspace
        .join("cluster")
        .join("inbox")
        .join(migrate_task_id(agent_id))
        .join("files")
}

// ---------------------------------------------------------------------------
// A 端编排（AgentManager 薄壳在 agents.rs —— cfg 分发）
// ---------------------------------------------------------------------------

/// 迁移一个 selfbuilt agent 到目标节点（12 步编排主体；`mgr` 的私有
/// 字段在本子模块内可达）。
pub async fn migrate_agent(
    mgr: &super::AgentManager,
    agent_id: &str,
    target_node: &str,
) -> Result<serde_json::Value, String> {
    let transport = mgr
        .migrate_transport()
        .ok_or_else(|| "agent 迁移不可用：迁移传输层未装配（集群未启用？）".to_string())?;

    // ① validate record：仅 selfbuilt + active。
    let record = mgr
        .store
        .get(agent_id)?
        .ok_or_else(|| format!("agent 不存在: {agent_id}"))?;
    if record.kind != agent_kind::SELFBUILT {
        return Err(format!(
            "agent {agent_id} kind={} 不可迁移（仅 selfbuilt 自建 agent 支持跨节点迁移）",
            record.kind
        ));
    }
    if record.status != agent_status::ACTIVE {
        return Err(format!(
            "agent {agent_id} 当前状态 {} 不可迁移（须 active）",
            record.status
        ));
    }
    if record.runtime_backend == nemesis_taskstore::agents::runtime_backend::VM {
        return Err(format!(
            "agent {agent_id} 是 VM 档（runtime_backend=vm）：guest 内执行体与 overlay \
             差量不随迁——请先切回 process 档再迁移"
        ));
    }
    validate_migrate_agent_id(agent_id)?;

    // ② validate target：非 self + 在线（未知节点 fail-closed 拒绝）。
    let self_id = transport.self_node_id();
    let target = transport
        .canonical_peer(target_node)
        .unwrap_or_else(|| target_node.to_string());
    if target == self_id {
        return Err(format!("迁移目标 {target_node} 是本节点"));
    }
    if !transport.is_peer_online(&target) {
        return Err(format!(
            "迁移目标 {target} 不在线（未知节点同样拒绝——迁移是破坏性操作）"
        ));
    }

    // ③ active→migrating 条件转移（并发迁移拒绝；后续失败统一回滚）。
    if !mgr
        .store
        .transition_status(agent_id, agent_status::ACTIVE, agent_status::MIGRATING)?
    {
        return Err(format!(
            "agent {agent_id} active→migrating 转移未生效（并发迁移或状态已变化）"
        ));
    }

    match migrate_inner(mgr, transport.as_ref(), &record, &target, &self_id).await {
        Ok(v) => Ok(v),
        Err(e) => {
            // **post-ACK 失败绝不回滚**（N2）：commit ACK 已达 = B 端已激活，
            // 回滚 active 会造成同 uuid 双活，且 ACTIVE 状态让 sweep 永久
            // 失明（sweep 只收敛 migrating）。保持 migrating：墓碑在场时
            // sweep 按迁移成功收敛删记录；墓碑也缺席（写墓碑重试后仍失败）
            // 时 sweep 会回滚 active——那是与「ACK 丢失」同族的双活诚实窗口
            // （模块头留痕，人工二选一），finalize 侧已 WARN 提示。
            if e.starts_with(POST_ACK_ERR_MARK) {
                return Err(format!(
                    "{e}；本端保持 migrating（B 端已激活，绝不回滚 active），\
                     重启 sweep 按墓碑收敛"
                ));
            }
            // ⑫ 回滚 migrating→active（回滚失败不吞——诚实并入错误信息；
            // 残留 migrating 由启动 sweep 收敛回 active）。
            // R7：本臂 = commit 确认未完成（call Err / 响应异常 / commit 前
            // 失败）——意图日志使命终结，随回滚清除（残留会让 sweep 误冻结）。
            remove_journal(&mgr.workspace, agent_id);
            match mgr.store.transition_status(
                agent_id,
                agent_status::MIGRATING,
                agent_status::ACTIVE,
            ) {
                Ok(true) => Err(format!("{e}（已回滚 active）")),
                Ok(false) => Err(format!(
                    "{e}；且回滚未生效——agent 卡在 migrating，重启 sweep 会收敛回 active"
                )),
                Err(re) => Err(format!(
                    "{e}；回滚也失败: {re}——agent 卡在 migrating，重启 sweep 会收敛"
                )),
            }
        }
    }
}

/// ④-⑪ 主体（③ 已完成；任何 Err 由调用方回滚）。
async fn migrate_inner(
    mgr: &super::AgentManager,
    transport: &dyn MigrateTransport,
    record: &AgentRecord,
    target: &str,
    self_id: &str,
) -> Result<serde_json::Value, String> {
    let agent_id = record.agent_id.as_str();
    let workspace = mgr.workspace.clone();

    // ④ queued 检查：渠道有排队消息 = 拒绝（在途任务诚实拒绝拍板）。
    if let Some(lm) = mgr.loop_manager()
        && let Some(n) = lm.has_queued(agent_id)
        && n > 0
    {
        return Err(format!(
            "agent {agent_id} 渠道有 {n} 条排队消息未消费——等消息处理完再迁移"
        ));
    }

    // ⑤ 停 loop（在途 turn abort =「未完成回复可接受」既有语义）。
    if let Some(lm) = mgr.loop_manager() {
        let _removed = lm.remove(agent_id);
    }

    // ⑥ 睡执行体（**禁 evict_agent**——墓碑只增不清不可逆；睡眠可逆且
    // 回滚后 agent 可原地复活）。无槽/无实例 = Ok(false)/false，不算错误。
    if let Some(rt) = mgr.runtime() {
        let _ = rt.sleep_agent(agent_id).await;
    }
    if let Some(vm) = mgr.vm_backend() {
        let _ = vm.sleep_agent(agent_id).await;
    }

    // ⑦ pack staging：home/** + memory/** + migration.json。
    let staging = migrate_staging_root(&workspace, agent_id);
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&staging); // 陈旧残留清理（幂等）
    std::fs::create_dir_all(&staging)
        .map_err(|e| format!("staging 目录创建失败 ({}): {e}", staging.display()))?;
    let home_src = nemesis_path::resolve_agent_home_in_workspace(&workspace, agent_id);
    let memory_src = nemesis_path::resolve_agent_memory_dir_in_workspace(&workspace, agent_id);
    copy_into(&home_src, &staging.join("home"))?;
    copy_into(&memory_src, &staging.join("memory"))?;
    let migration_meta = serde_json::json!({
        "format": MIGRATION_FORMAT,
        "agent_id": agent_id,
        "record": record,
        "source_node": self_id,
        "migrated_at": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(
        staging.join("migration.json"),
        serde_json::to_vec_pretty(&migration_meta)
            .map_err(|e| format!("migration.json 序列化失败: {e}"))?,
    )
    .map_err(|e| format!("migration.json 写入失败: {e}"))?;

    // ⑧ vault export：credential_refs 指向的别名整包导出（随机一次性
    // 口令包裹；口令只在本事务内存活）。引用了凭据但 vault 缺失/锁定 =
    // 诚实失败（迁过去的 agent 没凭据是残废）。
    // security 裁剪形态：vault 面整个缺席——无凭据可迁（空包直通），
    // 带凭据引用 = 诚实拒绝（裁剪形态本就无法消费凭据）。
    let aliases = credential_alias_list(record);
    let credentials: Option<(Vec<u8>, String)> = if aliases.is_empty() {
        None
    } else {
        #[cfg(feature = "security")]
        {
            let vp = nemesis_path::resolve_vault_path_in_workspace(&workspace);
            let store = crate::vault_runtime::open_unlocked_at(&vp)?;
            let passphrase = one_time_passphrase();
            let blob = store.export_entries(&aliases, &passphrase).map_err(|e| {
                format!("凭据导出失败（迁移中止）: {e}——检查 vault 别名在场且已解锁")
            })?;
            Some((blob, passphrase))
        }
        #[cfg(not(feature = "security"))]
        {
            return Err(
                "agent 引用了凭据（credential_refs），但 security feature 未编译——\
                 无 vault 面可导出，迁移中止（先移除凭据引用或换全量形态）"
                    .to_string(),
            );
        }
    };

    // ⑨ push staging → B 端 inbox（transfer 分块通道；max_bytes=0 不设限）。
    let task_id = migrate_task_id(agent_id);
    match transport.push_dir(target, &task_id, &staging).await? {
        nemesis_cluster::outbox::PushDirOutcome::Delivered => {}
        nemesis_cluster::outbox::PushDirOutcome::NothingToSend => {
            return Err("staging 目录为空（打包异常）——迁移中止".to_string());
        }
        nemesis_cluster::outbox::PushDirOutcome::Overlimit { total_bytes, limit } => {
            return Err(format!(
                "迁移载荷超限（{total_bytes} bytes > {limit}）——迁移中止"
            ));
        }
    }

    // ⑩ commit RPC：B 端 rename + vault import + 记录落库（同步 handler）。
    // R7：commit 发出前先落意图日志（安全锚，见 MigrationIntentJournal 注）
    // ——sweep 据此区分「commit 未达」（回滚）与「commit 已发出」（冻结转
    // 人工）。重试 ×3 全败 = 诚实中止（此刻尚未 commit，回滚零代价）。
    let mut journal_ok = false;
    for attempt in 1..=3u32 {
        match write_journal(&workspace, agent_id, target) {
            Ok(()) => {
                journal_ok = true;
                break;
            }
            Err(e) => {
                tracing::warn!("[agents] 迁移意图日志写入失败（第 {attempt}/3 次）: {e}");
                if attempt < 3 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
    }
    if !journal_ok {
        return Err(
            "迁移意图日志写入失败（三试全败）——迁移中止（commit 未发出，回滚 active）".to_string(),
        );
    }
    let payload = serde_json::json!({
        "agent_id": agent_id,
        "record": record,
        "credential_blob_b64": credentials.as_ref().map(|(b, _)| b64_encode(b)),
        "credential_passphrase_b64": credentials.as_ref().map(|(_, p)| b64_encode(p.as_bytes())),
    });
    let resp = transport
        .call(
            target,
            ACTION_AGENT_MIGRATE_COMMIT,
            payload,
            COMMIT_RPC_TIMEOUT,
        )
        .await?;
    if resp.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Err(format!("commit 响应异常（ok!=true）: {resp}——迁移中止"));
    }

    // ⑪ 本地收敛（顺序契约：墓碑先写、记录后删——崩溃窗口留给 sweep：
    // migrating+墓碑 = 迁移成功终局，删记录；反之回滚 active 照常服务）。
    // 失败必须带 post-ACK 标记上抛：此刻 B 端已激活，外层 ⑫ 不得回滚
    // （N2：旧实现一视同仁回滚 active = ACK 后删除失败即永久双活）。
    if let Err(e) = migrate_agent_local_finalize(mgr, agent_id, target) {
        return Err(format!("{POST_ACK_ERR_MARK}；本地收敛失败：{e}"));
    }

    tracing::info!(
        agent_id = %agent_id,
        target = %target,
        "[agents] agent 迁移完成：记录已从本节点移除，B 端已激活"
    );
    Ok(serde_json::json!({
        "status": "migrated",
        "agent_id": agent_id,
        "target_node": target,
        "note": "agent 已迁移到目标节点并激活；本节点记录已移除（墓碑留待 sweep 清扫）",
    }))
}

/// ⑪ 本地收敛：墓碑 → store.remove → staging 清理（尽力而为）。
/// 崩溃窗口收敛语义见模块头 + `status::MIGRATING` 注。
///
/// 墓碑是 post-ACK 崩溃窗口的收敛锚（migrating+墓碑 = sweep 删记录），
/// 写失败必须重试争取（Windows 瞬时句柄/索引器是常态失败源）——三试全
/// 败仍继续删记录（记录删掉 = 已收敛，墓碑本就冗余）；若记录删除也失败，
/// 外层保持 migrating 而墓碑缺席，sweep 会按「commit 未达」回滚 active
/// ——那是与 ACK 丢失同族的双活诚实窗口，此处 loud WARN 提示人工核查。
fn migrate_agent_local_finalize(
    mgr: &super::AgentManager,
    agent_id: &str,
    target: &str,
) -> Result<(), String> {
    // 墓碑重试 ×3（200ms 线性退避）：单发失败常是瞬时句柄，重试消除；
    // 全败不再阻断删除（理由见 fn 注），但 WARN 升级留痕。
    let mut tomb_ok = false;
    for attempt in 1..=3u32 {
        match write_tombstone(&mgr.workspace, agent_id, target) {
            Ok(_) => {
                tomb_ok = true;
                break;
            }
            Err(e) => {
                tracing::warn!("[agents] 迁移墓碑写入失败（第 {attempt}/3 次）: {e}");
                if attempt < 3 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
    }
    if !tomb_ok {
        tracing::warn!(
            "[agents] 迁移墓碑三试全败（agent {agent_id}）：若记录删除也失败，重启 sweep \
             将按「commit 未达」回滚 active——B 端已激活，届时同 uuid 双活需人工二选一"
        );
    }
    mgr.store.remove(agent_id)?;
    // staging 残留清理（尽力而为——失败只留磁盘占用，不影响语义）。
    let _ =
        nemesis_utils::fsutil::remove_dir_all_retry(migrate_staging_root(&mgr.workspace, agent_id));
    remove_tombstone(&mgr.workspace, agent_id);
    // 意图日志使命完成（记录已删 = 收敛终局；孤儿日志 sweep 不看会留残骸）。
    remove_journal(&mgr.workspace, agent_id);
    Ok(())
}

/// sweep 收敛扩展（`agents.rs::sweep` 的 migrating 分支，cfg cluster 调用）。
/// R7（2026-10-24 复核）三分支：
/// - migrating + 墓碑在场 = commit ACK 已达 → 删记录 + 删墓碑/日志 + 清 staging；
/// - migrating + 意图日志在场、无墓碑 = commit **已发出**（B 端可能已激活）
///   → **冻结**：保持 migrating + loud WARN 转人工（自动回滚 active 会造
///   同 uuid 双活且 ACTIVE 让 sweep 永久失明——旧形态缺陷）。人工裁决：
///   B 已激活 → 落墓碑；B 未收到 → 删日志（下轮 sweep 各自收敛）。
/// - 两者皆无 = commit 未发出（崩溃在 push/日志前）→ 回滚 active 照常服务。
pub fn sweep_migrating(
    store: &SqliteAgentsStore,
    workspace: &Path,
    report: &mut super::SweepReport,
) -> Result<(), String> {
    for record in store.list_all()? {
        if record.status != agent_status::MIGRATING {
            continue;
        }
        let agent_id = record.agent_id.clone();
        if tombstone_exists(workspace, &agent_id) {
            store.remove(&agent_id)?;
            remove_tombstone(workspace, &agent_id);
            remove_journal(workspace, &agent_id);
            let _ = nemesis_utils::fsutil::remove_dir_all_retry(migrate_staging_root(
                workspace, &agent_id,
            ));
            report.migrated_removed.push(agent_id);
        } else if journal_exists(workspace, &agent_id) {
            tracing::warn!(
                "[Agents] sweep: agent {agent_id} migrating + 意图日志在场（commit 已发出，\
                 B 端可能已激活）——冻结转人工：确认 B 已激活则落墓碑 \
                 (<workspace>/agents/{agent_id}.migrated.json)，确认未收到则删意图日志 \
                 (<workspace>/agents/{agent_id}.migrating.json)，下轮 sweep 自动收敛"
            );
            report.frozen_migrating.push(agent_id);
        } else if store.transition_status(
            &agent_id,
            agent_status::MIGRATING,
            agent_status::ACTIVE,
        )? {
            tracing::info!(
                "[Agents] sweep: agent {agent_id} 残留 migrating（commit 未达），已回滚 active"
            );
            report.reactivated.push(agent_id);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// B 端落地面（commit RPC handler 主体；同步——RpcHandlerFn 是同步闭包）
// ---------------------------------------------------------------------------

/// B 端 commit handler 依赖（注册时捕获；workspace = B 端 workspace 根）。
#[derive(Clone)]
pub struct MigrateCommitDeps {
    pub workspace: PathBuf,
    pub self_node_id: String,
}

/// B 端 `agent_migrate_commit` 主体（同步）：载荷校验 → 同 id 冲突拒绝 →
/// landing 在场检查 → vault import 先行（fail fast）→ rename 落位 →
/// record 改写 → upsert → 清 landing。独立 TaskDb 连接（多连接 WAL
/// 既定形态，与 gateway 主连接互不阻塞）。
pub fn handle_migrate_commit(
    deps: &MigrateCommitDeps,
    payload: serde_json::Value,
) -> Result<serde_json::Value, String> {
    // ① parse + 校验（agent_id 将成为 B 端目录名——严格校验）。
    let agent_id = payload
        .get("agent_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "载荷缺 agent_id".to_string())?;
    validate_migrate_agent_id(agent_id)?;
    let record_json = payload
        .get("record")
        .cloned()
        .ok_or_else(|| "载荷缺 record".to_string())?;
    let mut record: AgentRecord =
        serde_json::from_value(record_json).map_err(|e| format!("record 反序列化失败: {e}"))?;
    if record.agent_id != agent_id {
        return Err(format!(
            "record.agent_id ({}) 与载荷 agent_id ({agent_id}) 不一致——拒绝",
            record.agent_id
        ));
    }

    // ② 本地同 id 已存在 = 拒绝（uuid 冲突 / 重放）。
    let db = nemesis_taskstore::TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        &deps.workspace,
    ))?;
    let store = SqliteAgentsStore::from_db(std::sync::Arc::clone(&db));
    if store.get(agent_id)?.is_some() {
        return Err(format!(
            "本节点已存在同 id agent（{agent_id}）——迁移拒绝（uuid 冲突或重复投递）"
        ));
    }

    // ③ landing 在场检查（transfer 通道先于 commit 到达；未到场 = 拒绝，
    // A 端回滚，B 端 inbox 由 transfer 自身的重试/清理机制兜底）。
    let landing = landing_files_dir(&deps.workspace, agent_id);
    let src_home = landing.join("home");
    if !src_home.is_dir() {
        return Err(format!(
            "迁移载荷未到场（{}）——transfer 尚未完成或已被清理",
            src_home.display()
        ));
    }

    // ④ vault import 先行（fail fast——目录 rename 之前；import 失败 =
    // 目录未动、记录未落，A 端干净回滚）。
    // security 裁剪形态：vault 面缺席——带凭据载荷 = 诚实拒绝；空包
    // （无凭据迁移）直通。
    match payload.get("credential_blob_b64").and_then(|v| v.as_str()) {
        #[cfg(feature = "security")]
        Some(blob_b64) => {
            let pass_b64 = payload
                .get("credential_passphrase_b64")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "载荷有凭据 blob 但缺 passphrase".to_string())?;
            let blob = b64_decode(blob_b64)?;
            let passphrase = String::from_utf8(b64_decode(pass_b64)?)
                .map_err(|_| "凭据口令非 UTF-8".to_string())?;
            import_into_vault(&deps.workspace, &blob, &passphrase)?;
        }
        #[cfg(not(feature = "security"))]
        Some(_) => {
            return Err(
                "迁移载荷带凭据（credential blob），但本节点 security feature 未编译——\
                 无 vault 面可导入，commit 拒绝"
                    .to_string(),
            );
        }
        None => {}
    }

    // ⑤ 清 stale + rename 落位（同卷 rename 优先，跨设备 fallback 拷贝）。
    let dst_home = nemesis_path::resolve_agent_home_in_workspace(&deps.workspace, agent_id);
    let dst_memory = nemesis_path::resolve_agent_memory_dir_in_workspace(&deps.workspace, agent_id);
    // 幂等清理陈旧位（重试语义——此前同 id 迁移半途失败残留）。
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&dst_home);
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&dst_memory);
    move_dir(&src_home, &dst_home)?;
    move_dir(&landing.join("memory"), &dst_memory)?;

    // ⑥ 改写 record（home_node/status/exec_root/home_path；纯函数单一
    // 真相源）+ ⑦ upsert。
    let changed = rewrite_record_for_target(&mut record, &deps.self_node_id, &deps.workspace);
    store.upsert(&record)?;

    // ⑧ 清 landing（files + manifest + landed 标记整目录）。
    let landing_root = landing
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| landing.clone());
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&landing_root);

    tracing::info!(
        agent_id = %agent_id,
        source = %record.home_node,
        changed = ?changed,
        "[agents] 迁移 commit 完成：agent 已在本节点落地激活"
    );
    Ok(serde_json::json!({ "ok": true, "agent_id": agent_id }))
}

/// B 端 vault import（缺 vault 先创建：Windows=DPAPI 零摩擦；非 Windows
/// argon2 需口令创建——一次性迁移口令绝不充当 vault 口令，诚实拒绝让
/// 用户先初始化）。security 裁剪形态下调用点已被 cfg 闸住，本函数不进
/// 二进制（vault 面缺席 = 无可导入）。
#[cfg(feature = "security")]
fn import_into_vault(workspace: &Path, blob: &[u8], passphrase: &str) -> Result<(), String> {
    let vp = nemesis_path::resolve_vault_path_in_workspace(workspace);
    if !vp.exists() {
        #[cfg(target_os = "windows")]
        {
            nemesis_security::vault::VaultStore::create(
                &vp,
                nemesis_security::vault::VaultMode::Dpapi,
                None,
            )
            .map_err(|e| format!("vault 初始化失败 ({}): {e}", vp.display()))?;
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = vp;
            return Err(
                "目标机 vault 不存在（非 Windows 平台创建需口令）——先在本节点 \
                 `nemesisbot vault set <别名>` 初始化 vault 再接收迁移"
                    .to_string(),
            );
        }
    }
    let mut store = crate::vault_runtime::open_unlocked_at(&vp)?;
    let imported = store
        .import_entries(blob, passphrase)
        .map_err(|e| format!("迁移凭据导入失败: {e}"))?;
    store.save().map_err(|e| format!("vault 落盘失败: {e}"))?;
    tracing::info!(count = imported.len(), "[agents] 迁移凭据已导入 vault");
    Ok(())
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// record.credential_refs → vault 别名清单（裸别名——vault 层用裸 key；
/// 引用形态带 `vault:` 前缀）。
fn credential_alias_list(record: &AgentRecord) -> Vec<String> {
    record
        .credential_refs
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.strip_prefix("vault:").unwrap_or(s).to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 一次性迁移口令（两枚 uuid simple 拼 64 hex = 256bit 熵；只在本事务
/// 内存活——A 端 payload 组装后 drop，B 端 import 后 drop）。仅 security
/// 形态消费（vault export 唯一调用点）。
#[cfg(feature = "security")]
fn one_time_passphrase() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn b64_encode(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// 仅 security 形态消费（B 端 vault import 的 blob/口令解码）。
#[cfg(feature = "security")]
fn b64_decode(data: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("base64 解码失败: {e}"))
}

/// A 端 staging 打包拷贝：**只拷贝、绝不 rename**——源是 agent 真 home/
/// memory，rename 会把原件搬进 staging（迁移失败回滚后 agent 失去 home）。
/// 成功路径的源清理由 commit 成功后的 finalize 一次性完成（记录+staging
/// +home 都不再属于本节点——home 物理清除走 remove 联动语义，见下）。
fn copy_into(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    nemesis_cluster::transfer::copy_dir_recursive(src, dst)
}

/// B 端落位移动：rename 优先，跨设备 fallback 拷贝 + 删源。
fn move_dir(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(()); // memory 可缺席（A 端物化保证存在，保守不硬性要求）
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("落位父目录创建失败 ({}): {e}", parent.display()))?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    nemesis_cluster::transfer::copy_dir_recursive(src, dst)?;
    if let Err(e) = nemesis_utils::fsutil::remove_dir_all_retry(src) {
        tracing::warn!("[agents] 迁移落位：源目录删除失败（不影响落位，仅 landing 残留）: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
