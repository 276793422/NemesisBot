//! 看板服务句柄：`BoardStore` + 本节点角色，gateway 装配时注入 web 层。
//!
//! 角色即集群 [`NodeRole`]（goal 硬约束①：复用集群角色，无平行 role 字段）。
//! **2026-08-31 起 role 仅为元数据**（`role()`/`is_coordinator()` 供诊断展示），
//! 不再门控任何写操作：board.db 是节点本地数据（无集群同步、CLI 直写不走
//! web 层），按 role 拒写保护不了任何一致性，只会把非 coordinator 节点的
//! Dashboard 写路径全部堵死（两 worker 集群里无人可写）。派发（dispatch）
//! 的天然闸门是集群在线（`dispatch_issue_core` 报「集群未运行」），与 role 无关。
//!
//! 单节点部署（cluster 关闭）gateway 按 Coordinator 注入（board 计划 §1.2）。

use crate::store::BoardStore;
use nemesis_types::cluster::NodeRole;
use std::sync::Arc;

#[derive(Clone)]
pub struct BoardService {
    store: Arc<BoardStore>,
    role: NodeRole,
}

impl BoardService {
    pub fn new(store: Arc<BoardStore>, role: NodeRole) -> Self {
        Self { store, role }
    }

    pub fn store(&self) -> &Arc<BoardStore> {
        &self.store
    }

    pub fn role(&self) -> NodeRole {
        self.role
    }

    /// 本节点是否看板权威（coordinator）。**仅诊断/展示用途**——自
    /// 2026-08-31 起写权限与 role 无关（见模块文档）。
    pub fn is_coordinator(&self) -> bool {
        self.role == NodeRole::Coordinator
    }
}

#[cfg(test)]
mod tests;
