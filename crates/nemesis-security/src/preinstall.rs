//! #14 装前扫描（W2 批三，2026-11-20 拍板）：MCP server 定义接入扫描——
//! 注入检测 + eval 规则双信号，命中按模式裁决。
//!
//! 三层结构：
//! - [`scan_text_with`]/[`scan_text`]：双信号扫描。规则面共用
//!   `nemesis-config::eval_rules::scan_record`（与全报告评估同 op 语义，
//!   单一真相源）；注入面复用 Layer ① 的 [`Detector`]（同阈值同词表）。
//! - [`definition_fingerprint`] + [`Allowlist`]：定义指纹与人工确认
//!   allowlist（`<workspace>/config/mcp_scan_allowlist.json`，原子写；
//!   文件损坏按空表处理 + WARN——强制重新确认是安全方向，fail-open 只
//!   多一次确认成本）。
//! - [`decide`]：模式 × allowlist × 扫描结果 × confirm 的**纯函数**裁决
//!   （单测钉死语义矩阵；模式来自 `nemesis-config::resolve_preinstall_scan`：
//!   Off=现形态 / On=命中拒绝可确认放行 / Enforced=命中硬拒 confirm 也
//!   放不了行）。
//!
//! 扫描记录形态：定义对象 + 注入 `text` 字段（定义的 JSON 序列化全文，
//! [`definition_record`]）——subject 规则按 `text` 字段匹配，注入检测
//! 喂同一文本，两信号同源。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::injection::{Detector, InjectionConfig, InjectionResult};
use nemesis_config::eval_rules::{RulesFile, ScanHit, scan_record};

// ---------------------------------------------------------------------------
// 扫描
// ---------------------------------------------------------------------------

/// 装前扫描结果（双信号合一）。
#[derive(Debug, Clone)]
pub struct PreinstallScanResult {
    pub injection: InjectionResult,
    pub rule_hits: Vec<ScanHit>,
}

impl PreinstallScanResult {
    /// 双信号全干净。
    pub fn is_clean(&self) -> bool {
        !self.injection.is_injection && self.rule_hits.is_empty()
    }

    /// 人读命中摘要（确认卡 / 审计留痕 / WSAPI findings 投影用）。
    pub fn findings_summary(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.injection.is_injection {
            out.push(format!(
                "注入检测命中：score={:.2} level={} patterns={}",
                self.injection.score,
                self.injection.level,
                self.injection.matched_patterns.join(", ")
            ));
        }
        for hit in &self.rule_hits {
            out.push(format!(
                "规则命中 [{}] {}: {}",
                hit.level, hit.rule_id, hit.description
            ));
        }
        out
    }
}

/// 用给定 Detector 扫描文本（规则面 `None` = 规则文件缺失/损坏——规则
/// 信号缺席，注入检测照常；裁决是否因「无规则」拒绝由 [`decide`] 的
/// 调用方按 mode 语义决定）。
pub fn scan_text_with(
    detector: &Detector,
    text: &str,
    rules: Option<&RulesFile>,
) -> PreinstallScanResult {
    let injection = detector.analyze(text);
    let rule_hits = match rules {
        Some(r) => scan_record(&definition_record_value(text), Some(r)),
        None => Vec::new(),
    };
    PreinstallScanResult {
        injection,
        rule_hits,
    }
}

/// 便捷形态：默认配置 Detector（threshold 0.7 / enabled）。
pub fn scan_text(text: &str, rules: Option<&RulesFile>) -> PreinstallScanResult {
    scan_text_with(&Detector::new(InjectionConfig::default()), text, rules)
}

/// MCP server 定义的扫描记录：`{"text": <定义的 JSON 序列化全文>}`——
/// subject 规则与注入检测共用同一文本源（规范形态，与
/// `eval_assessor` 的 subject_record 同构）。
pub fn definition_record_value(text: &str) -> serde_json::Value {
    serde_json::json!({ "text": text })
}

/// 定义对象的序列化全文（扫描与指纹的同一文本源）。
pub fn definition_text(definition: &serde_json::Value) -> String {
    serde_json::to_string(definition).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 指纹与 allowlist
// ---------------------------------------------------------------------------

/// 定义指纹：定义对象 JSON 序列化字节的 SHA-256 hex。serde_json 默认
/// BTreeMap（键序稳定），同一配置文件的 parse→serialize 往返字节确定。
/// 指纹只作「同一定义再接入」的判等键（配置未改动 = 免重复确认；改动
/// 任何字节 = 新指纹 = 重新走扫描），不承载跨版本语义。
pub fn definition_fingerprint(definition: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(definition).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// allowlist 条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistEntry {
    pub fingerprint: String,
    /// server 名（展示用，非判等键）。
    pub name: String,
    /// 人工确认时的处置注记（如 `confirmed-risk-accepted` / `clean`）。
    pub verdict: String,
    /// RFC3339 加入时间。
    pub added_at: String,
}

/// allowlist 文件顶层。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AllowlistFile {
    #[serde(default = "default_allowlist_version")]
    version: u32,
    #[serde(default)]
    entries: Vec<AllowlistEntry>,
}

fn default_allowlist_version() -> u32 {
    1
}

/// 当前时间 RFC3339（调用方构造 [`AllowlistEntry::added_at`]/审计时间戳用）。
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 人工确认 allowlist（`<workspace>/config/mcp_scan_allowlist.json`）。
/// 读失败/损坏 → 空表 + WARN（fail-open 只多一次确认成本，方向安全）；
/// 写走 `write_file_atomic`。
pub struct Allowlist {
    path: PathBuf,
}

impl Allowlist {
    pub fn path_in_workspace(workspace: &Path) -> PathBuf {
        workspace.join("config").join("mcp_scan_allowlist.json")
    }

    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read_entries(&self) -> Vec<AllowlistEntry> {
        if !self.path.exists() {
            return Vec::new();
        }
        match nemesis_utils::fsutil::read_config_string(&self.path) {
            Ok(content) => match serde_json::from_str::<AllowlistFile>(&content) {
                Ok(f) => f.entries,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %self.path.display(),
                        "装前扫描 allowlist 文件损坏——按空表处理（需重新确认）"
                    );
                    Vec::new()
                }
            },
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %self.path.display(),
                    "装前扫描 allowlist 文件读取失败——按空表处理（需重新确认）"
                );
                Vec::new()
            }
        }
    }

    /// 指纹是否已确认。
    pub fn contains(&self, fingerprint: &str) -> bool {
        self.read_entries()
            .iter()
            .any(|e| e.fingerprint == fingerprint)
    }

    /// 追加条目（幂等：指纹已存在则 no-op）。原子写。
    pub fn add(&self, entry: AllowlistEntry) -> Result<(), String> {
        let mut file = AllowlistFile {
            version: default_allowlist_version(),
            entries: self.read_entries(),
        };
        if file
            .entries
            .iter()
            .any(|e| e.fingerprint == entry.fingerprint)
        {
            return Ok(());
        }
        file.entries.push(entry);
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let content =
            serde_json::to_string_pretty(&file).map_err(|e| format!("serialize allowlist: {e}"))?;
        nemesis_utils::write_file_atomic(&self.path.to_string_lossy(), content.as_bytes(), 0o600)
    }
}

// ---------------------------------------------------------------------------
// 裁决（纯函数）
// ---------------------------------------------------------------------------

/// 装前扫描裁决结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreinstallDecision {
    /// 双信号干净 → 放行。
    Allow,
    /// 有命中但指纹在 allowlist（此前人工确认过同一定义）→ 放行。
    AllowAllowlisted,
    /// 有命中 + On 模式且未确认 → 拒绝本次接入，要求调用方重发带
    /// `confirm_scan_risk: true`（审计留痕后放行）。
    RequireConfirm,
    /// 有命中 + Enforced 模式 → 硬拒（confirm_scan_risk 也放不了行）。
    Reject,
}

/// 裁决矩阵（模式 × allowlist × 扫描 × confirm）：
/// - 干净 → Allow（任何模式，含 Enforced——强制扫描不等于逢接必拒）；
/// - 命中 + allowlist → AllowAllowlisted；
/// - 命中 + Enforced → Reject（confirm 不可越过——强制不可关）；
/// - 命中 + On + confirm → Allow（审计留痕是调用方责任）；
/// - 命中 + On 无 confirm → RequireConfirm；
/// - Off → 恒 Allow（调用方通常在 Off 时直接跳过扫描，此臂为 totality）。
pub fn decide(
    mode: nemesis_config::PreinstallScanMode,
    allowlisted: bool,
    scan: &PreinstallScanResult,
    confirm_scan_risk: bool,
) -> PreinstallDecision {
    if scan.is_clean() {
        return PreinstallDecision::Allow;
    }
    if allowlisted {
        return PreinstallDecision::AllowAllowlisted;
    }
    match mode {
        nemesis_config::PreinstallScanMode::Enforced => PreinstallDecision::Reject,
        nemesis_config::PreinstallScanMode::On => {
            if confirm_scan_risk {
                PreinstallDecision::Allow
            } else {
                PreinstallDecision::RequireConfirm
            }
        }
        nemesis_config::PreinstallScanMode::Off => PreinstallDecision::Allow,
    }
}

#[cfg(test)]
mod tests;
