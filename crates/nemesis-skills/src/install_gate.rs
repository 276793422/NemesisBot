//! 装前审批卡（P13）：InstallPlan + InstallGate。
//!
//! 安装门漏斗的第六环前置产出：`plan_install` 阶段把「来源 repo / commit、
//! 文件清单+逐文件 sha256、安全扫描摘要、验签四态、版本龄」聚合为一张
//! InstallPlan 卡（文本形态 `summary()` 面向用户/模型可读），交给
//! `InstallGate` 裁决 approve/deny，deny 则全程不落盘。
//!
//! 三条通路各自注入实现：
//! - gateway/WSAPI：复用 WebApprovalManager 审批基建（approval_rules 按
//!   repo 粒度「总是允许」记忆）
//! - CLI：交互确认 + `--yes` 跳过
//! - 缺省 / 测试：`AlwaysAllowGate`（`skills.install_approval=false` 同款语义）

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::trust::TrustState;
use crate::types::SecurityCheckResult;
use nemesis_types::error::Result;

/// 安装计划中的单文件条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanFile {
    /// 相对技能根目录的路径。
    pub path: String,
    /// 内容 sha256（hex）。
    pub sha256: String,
    /// 字节数。
    pub bytes: u64,
}

/// 安装计划（审批卡数据面）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallPlan {
    /// 技能名（目标目录名）。
    pub slug: String,
    /// 来源：`github:<owner>/<repo>` 或 `registry:<name>/<slug>`。
    pub source: String,
    /// 用户指定的 ref（`@sha` / `@tag` 语法里的部分；无则为空）。
    pub requested_ref: String,
    /// 实际锁定的 commit SHA（GitHub 路径解析后；registry 路径可能为空）。
    pub commit: String,
    /// 发布时间（unix 秒；GitHub 用 commit date，registry 未知则为空）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<i64>,
    /// 版本龄（天；未知为空）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_days: Option<i64>,
    /// 版本龄裁决说明（"ok" / warn 文案 / block 文案）。
    #[serde(default)]
    pub age_decision: String,
    /// 文件清单 + 逐文件 sha256。
    pub files: Vec<PlanFile>,
    /// 内容总字节数。
    pub total_bytes: u64,
    /// 验签四态。
    pub trust: TrustState,
    /// 验签细节（错误信息 / 签名者公钥前缀等）。
    pub trust_detail: String,
    /// 安全扫描结果（lint+quality）。
    pub security: SecurityCheckResult,
    /// 非阻断性警告聚合（suspicious 标记 / lint 警告 / 龄 warn）。
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl InstallPlan {
    /// 生成面向用户/模型的审批卡文案。
    pub fn summary(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("技能安装审批：{}", self.slug));
        lines.push(format!("来源：{}{}", self.source, self.ref_suffix()));
        if let Some(age) = self.age_days {
            lines.push(format!("版本龄：{} 天（{}）", age, self.age_decision));
        } else {
            lines.push("版本龄：未知".to_string());
        }
        lines.push(format!(
            "验签：{}{}",
            self.trust,
            if self.trust_detail.is_empty() {
                String::new()
            } else {
                format!("（{}）", self.trust_detail)
            }
        ));
        lines.push(format!(
            "安全扫描：{:.0}/100，{} 项警告，分类计数 {:?}",
            self.security.lint_result.score * 100.0,
            self.security.lint_result.warnings.len(),
            self.security.lint_result.category_counts()
        ));
        lines.push(format!(
            "文件：{} 个 / {} 字节",
            self.files.len(),
            self.total_bytes
        ));
        for w in &self.warnings {
            lines.push(format!("⚠ {}", w));
        }
        lines.join("\n")
    }

    fn ref_suffix(&self) -> String {
        if self.commit.is_empty() {
            String::new()
        } else {
            let short: String = self.commit.chars().take(12).collect();
            format!("@{}", short)
        }
    }
}

/// 审批裁决。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallDecision {
    /// 放行安装。
    Approve,
    /// 拒绝安装（携带面向用户/模型的理由；reason 为空则用默认文案）。
    Deny { reason: String },
}

/// 安装门 trait（P13 装前审批的统一注入点）。
#[async_trait]
pub trait InstallGate: Send + Sync {
    /// 对安装计划做裁决。实现方负责阻塞等待用户/规则回复。
    async fn decide(&self, plan: &InstallPlan) -> InstallDecision;

    /// 审批面来源标记（审计/日志用）。
    fn name(&self) -> &str {
        "gate"
    }
}

/// 永远放行（`skills.install_approval=false` / 测试缺省）。
pub struct AlwaysAllowGate;

#[async_trait]
impl InstallGate for AlwaysAllowGate {
    async fn decide(&self, _plan: &InstallPlan) -> InstallDecision {
        InstallDecision::Approve
    }
}

/// 永远拒绝（测试 / 高安全模式）。
pub struct AlwaysDenyGate;

#[async_trait]
impl InstallGate for AlwaysDenyGate {
    async fn decide(&self, _plan: &InstallPlan) -> InstallDecision {
        InstallDecision::Deny {
            reason: "denied by policy (AlwaysDenyGate)".to_string(),
        }
    }
}

/// gate 的共享句柄形态（installer 持有）。
pub type SharedInstallGate = std::sync::Arc<dyn InstallGate>;

/// 校验 plan 序列化 roundtrip（供测试与 WSAPI 下发保证）。
pub fn plan_to_json(plan: &InstallPlan) -> Result<String> {
    serde_json::to_string_pretty(plan).map_err(nemesis_types::error::NemesisError::Serialization)
}

#[cfg(test)]
mod tests;
