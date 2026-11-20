//! #47 记忆投毒隔离的内容闸桥（W2 批三，2026-11-20 拍板）：可疑记忆
//! 「入库 + quarantined 标记 + 不参与召回 + 可人工放行」。
//!
//! 桥接 `nemesis_memory::quarantine::MemoryContentGate`：记忆写入前双信号
//! 扫描——注入检测（安全 8 层 Layer ① 同源 `Detector`）+ eval 规则
//! （`<workspace>/config/eval_rules.json`，mtime 缓存 load_rules）。任一命中
//! → `Some(QuarantineRecord)`（入库但隔离，不参与召回，留取证现场）。
//!
//! 语义边界：规则文件缺失/损坏 = **规则信号缺席**（WARN 一次 + 注入检测
//! 照常），与 #14 MCP 装前扫描同语义——裁决兜底在扫描结果的消费端，不在
//! 加载端。gate 缺席（未装配）= 写直通，升级零行为突变。
//!
//! 装配点：主 workspace 管理器（gateway ctx）、MCP 装配（mcp_serve）、
//! 命名空间/项目管理器咽喉（agent_factory shared）三处 `set_content_gate`。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nemesis_config::eval_rules::{RulesFile, load_rules, scan_record};
use nemesis_memory::quarantine::{MemoryContentGate, QuarantineRecord};
use nemesis_security::injection::{Detector, InjectionConfig};

/// 注入 + 规则双信号内容闸。
pub(crate) struct GatewayMemoryContentGate {
    detector: Detector,
    rules_path: PathBuf,
    cache: parking_lot::RwLock<RulesCache>,
}

struct RulesCache {
    mtime: Option<std::time::SystemTime>,
    /// `None` = 规则文件缺失/损坏（信号缺席，注入检测照常）。
    rules: Option<RulesFile>,
}

impl GatewayMemoryContentGate {
    pub(crate) fn new(workspace: &Path) -> Self {
        Self {
            detector: Detector::new(InjectionConfig::default()),
            rules_path: nemesis_path::resolve_eval_rules_path_in_workspace(workspace),
            cache: parking_lot::RwLock::new(RulesCache {
                mtime: None,
                rules: None,
            }),
        }
    }

    /// 规则信号（mtime 缓存：文件未变不重读；变化/首访重载）。
    fn rule_hits(&self, content: &str) -> Vec<String> {
        let mtime = std::fs::metadata(&self.rules_path)
            .and_then(|m| m.modified())
            .ok();
        {
            let cache = self.cache.read();
            if cache.mtime == mtime {
                return match &cache.rules {
                    Some(rules) => {
                        scan_record(&serde_json::json!({ "text": content }), Some(rules))
                            .into_iter()
                            .map(|hit| hit.rule_id)
                            .collect()
                    }
                    None => Vec::new(),
                };
            }
        }
        let loaded = match load_rules(&self.rules_path) {
            Ok(rules) => Some(rules),
            Err(e) => {
                tracing::warn!(
                    "[memory-content-gate] eval 规则加载失败，规则信号缺席（注入检测照常）：{e}"
                );
                None
            }
        };
        *self.cache.write() = RulesCache {
            mtime,
            rules: loaded.clone(),
        };
        match &loaded {
            Some(rules) => scan_record(&serde_json::json!({ "text": content }), Some(rules))
                .into_iter()
                .map(|hit| hit.rule_id)
                .collect(),
            None => Vec::new(),
        }
    }
}

impl MemoryContentGate for GatewayMemoryContentGate {
    fn scan(&self, content: &str, source: &str) -> Option<QuarantineRecord> {
        let injection = self.detector.analyze(content);
        let rule_hits = self.rule_hits(content);
        let has_injection = injection.is_injection;
        let has_rule_hits = !rule_hits.is_empty();
        if !has_injection && !has_rule_hits {
            return None;
        }
        let mut matched = Vec::new();
        if has_injection {
            matched.extend(injection.matched_patterns.iter().cloned());
        }
        matched.extend(rule_hits);
        let reason = match (has_injection, has_rule_hits) {
            (true, true) => "injection+rule",
            (true, false) => "injection",
            (false, true) => "rule",
            // 上方已对双零信号提前返回 None，此臂不可达。
            (false, false) => unreachable!("both signals clean but quarantine triggered"),
        };
        Some(QuarantineRecord::new(reason.to_string(), matched, source))
    }
}

/// 装配便捷入口：构造 + 挂到管理器（三处装配点同源调用）。
pub(crate) fn install_content_gate(
    manager: &Arc<nemesis_memory::manager::MemoryManager>,
    workspace: &Path,
) {
    manager.set_content_gate(Arc::new(GatewayMemoryContentGate::new(workspace)));
}
