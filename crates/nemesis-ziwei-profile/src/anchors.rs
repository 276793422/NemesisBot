//! 锚集（anchors.json）与失效判定（总控 §6.3/§6.4）。
//!
//! 标注确认函数时**必须同时**生成锚（事后补不了）；附加时锚逐条重扫，
//! 投票过阈值 → active；hash 一致直接 active；hash 不一致且票不过 → stale
//! 进找回流程。本模块只管数据形态与纯判定函数；重扫执行在 nemesis-ziwei-rev。

use serde::{Deserialize, Serialize};

/// 重定位锚。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    /// 关联的函数语义名（functions[].semantic）。
    pub function: String,
    pub kind: AnchorType,
    /// 所属模块名。
    #[serde(default)]
    pub module: Option<String>,
    /// byte_pattern：十六进制模式（`??` 通配）。
    #[serde(default)]
    pub pattern: Option<String>,
    /// str_ref：被引用的字符串原文。
    #[serde(default)]
    pub text: Option<String>,
    /// str_ref / import_call_seq：引用距离（指令偏移）。
    #[serde(default)]
    pub distance: Option<u32>,
    /// imm_const：独特立即数（magic/GUID 段/资源 ID）。
    #[serde(default)]
    pub value: Option<u64>,
    /// import_call_seq：导入名序列（如 "GetDC,ReleaseDC"）。
    #[serde(default)]
    pub imports: Option<String>,
    /// bb_hash：基本块归一化形态哈希（hex）。
    #[serde(default)]
    pub bb_hash: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// 锚类型（§6.3 五种；抗更新性见该表）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnchorType {
    BytePattern,
    StrRef,
    ImmConst,
    ImportCallSeq,
    BbHash,
}

/// anchors.json 根对象（独立文件：找回流程专用读写）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnchorSet {
    /// 所属 profile_id（一致性校验）。
    pub profile_id: String,
    #[serde(default)]
    pub anchors: Vec<Anchor>,
}

/// 单锚重扫结果（由 rev 引擎填写；M5 找回流程消费）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorScanResult {
    pub function: String,
    pub kind: AnchorType,
    /// 本锚是否命中（≥1 处且落在合理范围）。
    pub matched: bool,
    /// 命中位置数（投票按「函数有锚命中」计，不按位置数）。
    pub hits: u32,
}

/// 附加验证裁决（§6.4 验证链）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachVerdict {
    /// hash 一致 → 直接 active。
    Active,
    /// hash 不一致但锚投票通过 → active + 降级注记（向用户报告一次版本已变）。
    ActiveDegraded,
    /// hash 不一致且锚投票失败 → stale，进找回流程。
    Stale,
}

/// 纯判定：给定 hash 比对结果与锚扫描结果集，产出裁决。
///
/// - `hash_match = true` → [`AttachVerdict::Active`]（不问锚）。
/// - 否则统计锚投票：每个 function 的锚**分组投票**——只要该函数有任一锚
///   命中即记该函数一票？不对：投票语义是**锚级**——profile.verification
///   的 `anchors_vote>=N` 指「至少 N 条锚命中」。全部锚放一起数命中条数，
///   ≥ 阈值 → [`AttachVerdict::ActiveDegraded`]，否则 [`AttachVerdict::Stale`]。
pub fn decide_verdict(
    hash_match: bool,
    scans: &[AnchorScanResult],
    vote_threshold: u32,
) -> AttachVerdict {
    if hash_match {
        return AttachVerdict::Active;
    }
    let votes = scans.iter().filter(|s| s.matched).count() as u32;
    if votes >= vote_threshold.max(1) {
        AttachVerdict::ActiveDegraded
    } else {
        AttachVerdict::Stale
    }
}

/// 校验锚集内部一致性（保存时调用；loud）：
/// - 每个 anchor.function 必须在 profile.functions 中（保存时由 store 传入
///   合法函数名集合核对——本函数只做形态校验，跨文件一致性在 store）。
pub fn validate_anchor(anchor: &Anchor) -> Result<(), String> {
    if anchor.function.trim().is_empty() {
        return Err("锚的 function 关联不得为空".to_string());
    }
    match anchor.kind {
        AnchorType::BytePattern => {
            if anchor
                .pattern
                .as_deref()
                .ok_or_else(|| "byte_pattern 锚缺 pattern".to_string())?
                .is_empty()
            {
                return Err("byte_pattern 锚 pattern 为空".to_string());
            }
        }
        AnchorType::StrRef => {
            if anchor.text.as_deref().map(str::is_empty).unwrap_or(true) {
                return Err("str_ref 锚缺 text".to_string());
            }
        }
        AnchorType::ImmConst => {
            if anchor.value.is_none() {
                return Err("imm_const 锚缺 value".to_string());
            }
        }
        AnchorType::ImportCallSeq => {
            if anchor.imports.as_deref().map(str::is_empty).unwrap_or(true) {
                return Err("import_call_seq 锚缺 imports 序列".to_string());
            }
        }
        AnchorType::BbHash => {
            if anchor.bb_hash.as_deref().map(str::is_empty).unwrap_or(true) {
                return Err("bb_hash 锚缺 bb_hash".to_string());
            }
        }
    }
    Ok(())
}
