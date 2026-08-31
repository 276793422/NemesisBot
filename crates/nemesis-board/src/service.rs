//! 看板服务句柄：`BoardStore` + 本节点角色，gateway 装配时注入 web 层。
//!
//! 角色即集群 [`NodeRole`]（goal 硬约束①：复用集群角色，无平行 role 字段）：
//! - `Coordinator`（旧值 master/manager 兼容）：看板权威，读写全开；
//! - `Worker`：只读——写操作在 handler 层拒绝（403 语义），写需求经
//!   集群派发到 coordinator（W2 P2 dispatch 链路）。
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

    /// 本节点是否看板权威（coordinator）。
    pub fn is_coordinator(&self) -> bool {
        self.role == NodeRole::Coordinator
    }
}

#[cfg(test)]
mod tests;
