//! nemesis-ziwei-profile —— 紫微系统 profile 数据层（纯数据，无网络/进程副作用）。
//!
//! 职责（总控 §4.1）：
//! - profile schema v1 类型 + serde（`deny_unknown_fields` loud 拒绝）
//! - 目录存储读写（`<workspace>/ziwei/profiles/<id>/`，原子写）
//! - 锚集（anchors.json）独立读写 + 锚投票/失效判定（纯函数）
//! - 保存前结构校验（字段越界 / chain 超限 / 非法路径，全部 loud）
//!
//! 窄门面纪律（决议 8①）：bot 侧只准经 `nemesis-ziwei` 门面间接使用本层，
//! 禁止直依赖。

pub mod anchors;
pub mod model;
pub mod store;

#[cfg(test)]
mod tests;

pub use anchors::{Anchor, AnchorScanResult, AnchorSet, AnchorType, AttachVerdict};
pub use model::{
    ArgSpec, AttachInfo, CHAIN_CAP, CaptureSpec, ChainStep, Conv, DataStructure, EmitSink,
    FieldBase, FieldDecode, FieldDef, FieldType, FunctionDef, LocateKind, LocateSpec, Machine,
    NotifyRule, OutputSpec, PollMode, Primitive, Profile, ProfileStatus, RunPolicy, SCHEMA_VERSION,
    TargetFingerprint, ToolAction, ToolDef, Verification, VerifyPolicy, parse_byte_pattern,
};
pub use store::{ProfileStore, ProfileSummary, StoreError};
