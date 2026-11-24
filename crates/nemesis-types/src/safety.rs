//! 安全词表单一真相源（自学习铺路批次一，2026-11-24）。
//!
//! 「常驻注入面」的硬边界：洞察台账（forge insights）与用户习惯画像
//! （user_profile）两类会进 LLM 上下文的自学习条目共用同一份禁词表——
//! 拒绝任何含权限/安全语义的内容进入注入面（入库闸 + 渲染闸防御性
//! 双保险）。审批放行只走 approval pattern 机制，注入面绝不承载。
//!
//! 词表刻意**不可配置**——硬边界不随部署漂移（同 schema 常量纪律）。
//! 原住民是 user_profile.rs 私有 const；洞察台账引入时按单一真相源
//! 纪律上收本模块，两侧共用。

/// 禁词表（子串匹配，大小写不敏感；中英文覆盖）。
pub const FORBIDDEN_WORDS: &[&str] = &[
    "自动批准",
    "自动放行",
    "免审批",
    "跳过审批",
    "自动 approve",
    "auto approve",
    "auto-approve",
    "降档",
    "降低安全",
    "绕过审批",
    "skip approval",
    "bypass approval",
];

/// 入库/渲染层过滤：内容含任一禁词（大小写不敏感子串）即违规。
pub fn violates_security_boundary(content: &str) -> bool {
    let lower = content.to_lowercase();
    FORBIDDEN_WORDS.iter().any(|k| lower.contains(k))
}
