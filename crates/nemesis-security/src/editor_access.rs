//! Full Access 编辑器放行开关(2026-09-20 用户裁决)。
//!
//! 两个运行时开关的内存态本体,**不持久化**(进程重启一律回到双关,
//! 必须用户手动再开——每次重启重新授权):
//!
//! - **开关一 Full Access**:项目目录内文件操作全放行;项目目录外
//!   read/exec/network/system 等放行;项目目录外 write/delete 保留原
//!   判定(仍走审批)。exec 族不分内外整体放行(真沙盒兜底,第九道防护)。
//! - **开关二 外部写删放行**:项目外 write/delete 也放行;依赖开关一
//!   ([`EditorAccessState::set_flags`] 内嵌联动,ext=true ⇒ full=true)。
//!
//! 消费方:`SecurityAuditor::evaluate_request` 在自毁硬拦之后、规则遍之前
//! 短路调用 [`EditorAccessState::evaluate`](Some = 直接采信该判定,None =
//! 回落原判定链);web 层 `handlers/editor.rs` 持同一 Arc 读写 + SSE 广播。
//! 短路放行的操作在外层统一审计落盘(policy_rule=`editor_access:*` 可过滤)。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::RwLock;

use crate::types::{OperationType, SecurityDecision};

/// 写删族:项目目录外的这类操作受开关二管辖(其余族开关一直接放行)。
const WRITE_DELETE_OPS: &[OperationType] = &[
    OperationType::FileWrite,
    OperationType::FileDelete,
    OperationType::DirCreate,
    OperationType::DirDelete,
];

/// Registry 族(用户裁决确认点:当前 `tool_to_operation` 无任何工具映射到
/// 此族,是死臂;按「其余全放」归入开关一直接放行。将来若有工具映射进
/// 该族且语义应视作写删,把对应变体挪进 [`WRITE_DELETE_OPS`] 或在此独立
/// 成臂即可)。
const REGISTRY_OPS: &[OperationType] = &[
    OperationType::RegistryRead,
    OperationType::RegistryWrite,
    OperationType::RegistryDelete,
];

/// Full Access 双开关运行时状态。gateway 装配处创建一个 Arc,同时注入
/// auditor(判定热路径读)与 web editor handler(WSAPI 读写)——单一
/// 真相源,两处 UI(聊天框旁按钮 / 设置页【编辑器】TAB)都经 handler
/// 读写同一实例。
pub struct EditorAccessState {
    /// 开关一 Full Access。
    full_access: AtomicBool,
    /// 开关二 外部写删放行(依赖开关一,单独为 true 无意义)。
    external_write: AtomicBool,
    /// 「项目目录」根列表(workspace roots,归一化形态)。判定 target
    /// 内外用;gateway 启动注入主 workspace + 项目 registry 全部项目根,
    /// 运行期 editor.get/set 每次刷新(新建项目下次触碰开关即纠正)。
    workspace_roots: RwLock<Vec<String>>,
}

impl EditorAccessState {
    /// 新建双关状态(roots 空 = 未注入,写删族一律判 outside → fail-safe
    /// 回落原判定,多走审批)。
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            full_access: AtomicBool::new(false),
            external_write: AtomicBool::new(false),
            workspace_roots: RwLock::new(Vec::new()),
        })
    }

    /// 翻转开关(**服务端联动收口**:ext=true ⇒ full=true;UI 层的禁用
    /// 联动只是体验,不变量在这里保证)。
    ///
    /// 存储顺序纪律:读侧语义是 `if full { … if ext … }`——开 = 先 full 后
    /// ext(在途请求最坏见到 (true, false) = 外部写删暂未放行,回落原判定);
    /// 关 = 先 ext 后 full(中间态 (true, false) 同为 fail-safe,与开路径
    /// 同向——两个方向都不会出现「该拦的一瞬没拦」)。Relaxed 足够:单标志
    /// 独立读,无跨变量序依赖。
    pub fn set_flags(&self, full_access: bool, external_write: bool) {
        let full = full_access || external_write;
        if full {
            self.full_access.store(true, Ordering::Relaxed);
            self.external_write.store(external_write, Ordering::Relaxed);
        } else {
            self.external_write.store(false, Ordering::Relaxed);
            self.full_access.store(false, Ordering::Relaxed);
        }
        tracing::info!(
            full_access = full,
            external_write = external_write,
            "[EditorAccess] 开关已切换(运行时态,进程重启后回到双关)"
        );
    }

    /// 当前双开关快照(web 展示 / WSAPI get)。
    pub fn snapshot(&self) -> (bool, bool) {
        (
            self.full_access.load(Ordering::Relaxed),
            self.external_write.load(Ordering::Relaxed),
        )
    }

    /// 注入 workspace roots(归一化后存储)。空段自动滤除。
    pub fn set_workspace_roots(&self, roots: Vec<String>) {
        let normalized: Vec<String> = roots
            .into_iter()
            .filter_map(|r| {
                let r = r.trim();
                if r.is_empty() {
                    return None;
                }
                Some(normalize_path(r).trim_end_matches('/').to_string())
            })
            .collect();
        *self.workspace_roots.write() = normalized;
    }

    /// ABAC 判定链短路入口(自毁硬拦**之后**调用——自毁形态不被本开关
    /// 绕过)。
    ///
    /// 返回 `Some((decision, reason, policy_rule))` = 直接采信短路判定;
    /// `None` = 开关未开/写删族项目外且开关二未开 → 回落原判定链
    /// (规则遍 → exec_unknown_policy → default_action → 审批)。
    pub fn evaluate(
        &self,
        op_type: OperationType,
        target: &str,
    ) -> Option<(SecurityDecision, String, String)> {
        // 开关一未开:整体不短路(读侧纪律:先读 full——与 set_flags 的
        // 存储顺序配对)。
        if !self.full_access.load(Ordering::Relaxed) {
            return None;
        }

        // 写删族:项目内直接放;项目外仅开关二也开才放,否则回落原判定
        //(仍走规则/审批——正是「项目外写删保留治理」的裁决语义)。
        if WRITE_DELETE_OPS.contains(&op_type) {
            let Some(norm) = normalize_target(target) else {
                // 空目标:fail-safe 判 outside(回落原判定,与今天行为一致)。
                return None;
            };
            if self.is_inside_workspace(&norm) {
                return Some((
                    SecurityDecision::Allowed,
                    format!("editor full access: {} inside workspace roots", op_type),
                    "editor_access:full".to_string(),
                ));
            }
            if self.external_write.load(Ordering::Relaxed) {
                return Some((
                    SecurityDecision::Allowed,
                    format!(
                        "editor full access + external write: {} outside workspace roots",
                        op_type
                    ),
                    "editor_access:full+ext".to_string(),
                ));
            }
            return None;
        }

        // Registry 族死臂注释(见 REGISTRY_OPS 文档)。
        let _ = REGISTRY_OPS.contains(&op_type);

        // 其余族(读/exec/network/system/hardware):开关一直接放行,
        // 不分内外(exec 判不可靠,假精度比不做更危险——裁决)。
        Some((
            SecurityDecision::Allowed,
            format!("editor full access: {} allowed without approval", op_type),
            "editor_access:full".to_string(),
        ))
    }

    /// 归一化 target 是否在任一 workspace root 内。
    ///
    /// 逃逸硬化(对所有路径生效,先于相对/绝对判定):归一化后含 `..` 段
    /// 或 `~` 头 → outside(fail-safe 回落原判定)。
    /// 相对路径(无盘符/前导斜杠/UNC 前缀)= agent 工具链语义相对
    /// workspace 解析 → inside。绝对路径做分隔符感知前缀匹配
    /// (`t == root` 或 `t` 以 `root + "/"` 开头,杜绝 `C:/foo` vs
    /// `C:/foobar` 兄弟前缀误命中)。roots 空 = 未注入 → outside。
    fn is_inside_workspace(&self, norm: &str) -> bool {
        if norm == "~" || norm.starts_with("~/") || norm.split('/').any(|seg| seg == "..") {
            return false;
        }
        if !is_absolute_normalized(norm) {
            return true;
        }
        let roots = self.workspace_roots.read();
        roots
            .iter()
            .any(|r| norm == r || norm.starts_with(&format!("{}/", r)))
    }
}

/// 路径归一化(与 auditor protected_paths 同款基调 + verbatim/UNC 剥离):
/// 小写、反斜杠转正斜杠、剥 `\\?\` / `\\?\UNC\` 前缀。**调用方保证非空**
/// (空串返回无意义,evaluate 已先滤)。
fn normalize_path(p: &str) -> String {
    let mut s = p.to_lowercase().replace('\\', "/");
    // verbatim 前缀://?/c:/... → c:/...;//?/unc/server/share → //server/share
    if let Some(rest) = s.strip_prefix("//?/unc/") {
        s = format!("//{rest}");
    } else if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    s
}

/// 归一化目标入口:trim + 空串拒绝(返回 None = 判 outside)。
fn normalize_target(target: &str) -> Option<String> {
    let t = target.trim();
    if t.is_empty() {
        return None;
    }
    Some(normalize_path(t))
}

/// 归一化形态的绝对判定:盘符(`c:/…`)/ 前导 `/` / UNC(`//server/…`)。
fn is_absolute_normalized(s: &str) -> bool {
    let bytes = s.as_bytes();
    (bytes.len() >= 2 && bytes[1] == b':') || s.starts_with('/')
}

#[cfg(test)]
mod tests;
