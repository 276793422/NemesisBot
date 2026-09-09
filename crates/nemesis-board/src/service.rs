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
use std::path::PathBuf;
use std::sync::Arc;

/// 本地发言入口桥（Swarm M3 批次 E）：把 dashboard 人工发言接进 master
/// 讨论管线（幂等/额度三闸/裁决投递与 worker 上行同源，G3/G12 语义单一
/// 真相）。实现方在 nemesisbot（`board_bus::LocalDiscussionIngress`）——
/// 本 crate 不依赖 cluster/agent，故以 trait 倒置；未注入 = 发言不可用
/// （讨论总线未装配），调用方诚实报错。
pub trait DiscussionIngress: Send + Sync {
    /// 发言。返回缓存/首响 JSON（`{comment_id|message_id, seq}`）；
    /// Err = 额度拒绝（`[code] 文案`）或落库失败（人读文案）。
    #[allow(clippy::too_many_arguments)]
    fn post(
        &self,
        sender: &crate::assignment::Actor,
        thread_kind: &str,
        thread_id: i64,
        client_msg_id: &str,
        content: &str,
        reply_to: Option<i64>,
        kind_tag: &str,
    ) -> Result<serde_json::Value, String>;
}

#[derive(Clone)]
pub struct BoardService {
    store: Arc<BoardStore>,
    role: NodeRole,
    /// 资产下载密钥（Swarm M3 §5.4；gateway 装配时经 [`BoardService::with_asset_secret`]
    /// 注入）。None = 本节点未配资产服务——下载端点诚实 503，签发 helper
    /// 诚实报错。密钥永不出节点：签发/验证都用本节点自己的 secret。
    asset_secret: Option<Arc<[u8]>>,
    /// 资产实体目录（`<workspace>/board/assets`；与 secret 一起装配，
    /// 下载端点的白名单根——端点只服务此目录下、asset 表已登记的 ref）。
    assets_dir: Option<PathBuf>,
    /// 讨论发言桥（Swarm M3 批次 E；`with_discussion` 注入）。None =
    /// board+cluster 讨论总线未装配，`channel.post` 诚实拒绝。
    discussion: Option<Arc<dyn DiscussionIngress>>,
}

impl BoardService {
    pub fn new(store: Arc<BoardStore>, role: NodeRole) -> Self {
        Self {
            store,
            role,
            asset_secret: None,
            assets_dir: None,
            discussion: None,
        }
    }

    /// 注入资产密钥（builder；gateway 装配用）。重复调用以最后一次为准。
    pub fn with_asset_secret(mut self, secret: Vec<u8>) -> Self {
        self.asset_secret = Some(secret.into_boxed_slice().into());
        self
    }

    /// 注入资产实体目录（builder；与 [`BoardService::with_asset_secret`]
    /// 成对——二者齐备下载端点才可用）。
    pub fn with_assets_dir(mut self, dir: PathBuf) -> Self {
        self.assets_dir = Some(dir);
        self
    }

    /// 注入讨论发言桥（builder；gateway 在 board+cluster 装配时接
    /// `board_bus` 管线）。
    pub fn with_discussion(mut self, ingress: Arc<dyn DiscussionIngress>) -> Self {
        self.discussion = Some(ingress);
        self
    }

    /// 资产密钥（None = 未配资产服务）。
    pub fn asset_secret(&self) -> Option<&[u8]> {
        self.asset_secret.as_deref()
    }

    /// 资产实体目录（None = 未配资产服务）。
    pub fn assets_dir(&self) -> Option<&std::path::Path> {
        self.assets_dir.as_deref()
    }

    /// 资产服务是否就绪（secret + 目录齐备）。
    pub fn asset_serving_ready(&self) -> bool {
        self.asset_secret.is_some() && self.assets_dir.is_some()
    }

    /// 讨论发言桥（None = 讨论总线未装配）。
    pub fn discussion(&self) -> Option<Arc<dyn DiscussionIngress>> {
        self.discussion.clone()
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
