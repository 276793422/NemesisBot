//! 签名验证启动自验状态（接入计划 §4，2026-09-23）。
//!
//! **刻意不门控**：纯数据透传结构，无 nemesis-security 依赖；AppState /
//! server 字段在 nemesis-web 默认 feature（无 security）下也必须编译。
//! WSAPI 命令 `security.signature_verify_status` 本体仍在（feature 门控的）
//! `handlers::security`；裁决逻辑单一真相源在 nemesisbot `verify_policy`。

/// gateway 启动时从 verify_policy 快照映射注入（只读快照，进程内不变）。
#[derive(Debug, Clone)]
pub struct SignatureVerifyStatus {
    /// 生效模式：off / warn / enforce（三态矩阵裁决后）。
    pub mode: String,
    /// 锁定版构建（verify-enforce-lock feature，config 被忽略恒 enforce）。
    pub locked: bool,
    /// 编译期信任锚指纹（None = 无锚）。
    pub anchor_fp: Option<String>,
    /// 启动自验九态结果（None = 未执行验签：off 跳过 / 无锚降级）。
    pub last_result: Option<String>,
    /// 签名者公钥指纹（Valid 时有值）。
    pub key_fp: Option<String>,
    /// 详情（九态 detail / 降级原因 / 空串）——徽标悬停文案数据源。
    pub detail: String,
}
