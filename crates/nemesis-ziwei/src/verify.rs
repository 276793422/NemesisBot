//! verify —— 附加验证链（T5.1）+ 失效通知（T5.2）+ 授权找回（T5.3）门面。
//!
//! 链序（总控 §6.4）：hash → version_resource（证据）→ 锚投票 → 状态机。
//! 裁决语义：hash 一致 → Active；不一致 → 逐锚重扫（盘上 exe 文件镜像）
//! 投票，过阈值 → ActiveDegraded（降级注记一次性），不过 → Stale（进找回
//! 流程）。version_resource 差异只进报告证据，不单独改判——锚投票是最终
//! 裁决依据（锚在即语义在）。
//!
//! 找回（T5.3）：`recover_start` 逐锚重扫 +（可选）LLM 语义比对 → 《找回
//! 报告》；`recover_confirm` 用户逐条确认 → 全接受 = 新指纹入库 + Active，
//! 有拒绝 = 保持 Stale（拒绝项诚实不猜）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nemesis_ziwei_profile::anchors::{AnchorScanResult, AttachVerdict, decide_verdict};
use nemesis_ziwei_profile::model::{Profile, ProfileStatus};
use nemesis_ziwei_rev::recover::{AnchorLite, rescan};
use sha2::{Digest, Sha256};

use crate::profile::ProfileStore;

/// 事件钩子（gateway 装配 → Dashboard 横幅/推送；与 package::set_llm_hook
/// 同款静态槽模式）。
type EventHook = Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>;
static EVENT_HOOK: std::sync::OnceLock<EventHook> = std::sync::OnceLock::new();

/// 安装事件钩子（幂等；重复 set 忽略后到者）。事件名：
/// `ziwei.stale` / `ziwei.degraded` / `ziwei.recovered`。
pub fn set_event_hook(hook: EventHook) -> bool {
    EVENT_HOOK.set(hook).is_ok()
}

fn fire_event(name: &str, data: &serde_json::Value) {
    if let Some(h) = EVENT_HOOK.get() {
        h(name, data);
    }
}

/// 验证裁决（profile 层 AttachVerdict 的门面镜像，序列化友好）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Active,
    ActiveDegraded,
    Stale,
}

impl From<AttachVerdict> for Verdict {
    fn from(v: AttachVerdict) -> Self {
        match v {
            AttachVerdict::Active => Self::Active,
            AttachVerdict::ActiveDegraded => Self::ActiveDegraded,
            AttachVerdict::Stale => Self::Stale,
        }
    }
}

/// version_resource 单键比对行。
#[derive(Debug, Clone, serde::Serialize)]
pub struct VersionDiffRow {
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub matches: bool,
}

/// 附加验证报告（一次性报告 = 同一 (profile, 新 hash) 组合只落一次盘）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct VerifyReport {
    pub profile_id: String,
    pub verdict: Verdict,
    /// 盘上目标 exe 路径。
    pub exe_path: String,
    pub hash_expected: Option<String>,
    pub hash_actual: String,
    pub hash_match: bool,
    pub version_diff: BTreeMap<String, VersionDiffRow>,
    pub version_match: Option<bool>,
    /// 锚投票（hash 一致时不重扫，None）。
    pub anchors: Option<AnchorVoteSummary>,
    /// 一次性语义：本次是否新落了报告/通知文件（重复验证不再重复打扰）。
    pub notice_emitted: bool,
    pub notes: Vec<String>,
    pub verified_at: String,
}

/// 锚投票摘要。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnchorVoteSummary {
    pub total: usize,
    pub supported: usize,
    pub hit: usize,
    pub threshold: u32,
    pub passed: bool,
}

/// 验证结果（裁决 + 需要持久化的 profile 更新）。
#[derive(Debug)]
pub struct VerifyOutcome {
    pub report: VerifyReport,
    /// Some = 调用方应 store.save（状态机转移 / 基准锚定 / last_verified_at）。
    pub profile_updated: Option<Profile>,
    /// 报告 markdown 路径（降级/失效时落盘；active = None）。
    pub report_path: Option<PathBuf>,
}

/// attach 前置守卫（T5.1 状态机闸）：非 Active 一律拒绝注入。
pub fn pre_attach_guard(profile: &Profile) -> Result<(), String> {
    match profile.status {
        ProfileStatus::Active => Ok(()),
        ProfileStatus::Stale => Err(format!(
            "profile {} 已失效（目标更新后锚验证未通过）——先跑找回：ziwei recover {}",
            profile.profile_id, profile.profile_id
        )),
        ProfileStatus::Revoked => Err(format!(
            "profile {} 已吊销——重建或删除后重配",
            profile.profile_id
        )),
    }
}

/// SHA-256（文件字节流式读取）。
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("读取 {}: {e}", path.display()))?;
    Ok(sha256_hex(&data))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// 验证链主入口（T5.1）。`exe_path` = 目标当前盘上 exe；裁决 + 状态机 +
/// 一次性报告/通知一次完成；profile 更新由调用方持久化（返回值带回）。
pub fn verify_profile(
    store: &ProfileStore,
    workspace: &Path,
    profile: &Profile,
    exe_path: &Path,
) -> Result<VerifyOutcome, String> {
    if profile.status == ProfileStatus::Revoked {
        return Err(format!(
            "profile {} 已吊销（不可验证/附加）",
            profile.profile_id
        ));
    }
    let actual = sha256_file(exe_path)?;
    let mut notes: Vec<String> = Vec::new();
    let mut updated = profile.clone();
    let now = chrono::Utc::now().to_rfc3339();

    // ── hash 环 ──
    let Some(expected) = profile.target.file_sha256.clone() else {
        // 首次验证：锚定基准（不重扫——没有基准就无所谓「变」）。
        updated.target.file_sha256 = Some(actual.clone());
        updated.last_verified_at = chrono::Utc::now();
        notes.push("首次验证：hash 基准已锚定（后续附加逐次核对）".into());
        let report = VerifyReport {
            profile_id: profile.profile_id.clone(),
            verdict: Verdict::Active,
            exe_path: exe_path.display().to_string(),
            hash_expected: None,
            hash_actual: actual,
            hash_match: true,
            version_diff: BTreeMap::new(),
            version_match: None,
            anchors: None,
            notice_emitted: false,
            notes,
            verified_at: now,
        };
        return Ok(VerifyOutcome {
            report,
            profile_updated: Some(updated),
            report_path: None,
        });
    };
    let hash_match = expected == actual;

    // ── hash 一致 → Active（不问锚）──
    if hash_match {
        updated.last_verified_at = chrono::Utc::now();
        let report = VerifyReport {
            profile_id: profile.profile_id.clone(),
            verdict: Verdict::Active,
            exe_path: exe_path.display().to_string(),
            hash_expected: Some(expected),
            hash_actual: actual,
            hash_match: true,
            version_diff: BTreeMap::new(),
            version_match: None,
            anchors: None,
            notice_emitted: false,
            notes,
            verified_at: now,
        };
        return Ok(VerifyOutcome {
            report,
            profile_updated: Some(updated),
            report_path: None,
        });
    }

    // ── hash 不一致 → version 证据 + 锚投票 ──
    let actual_version = extract_version_resource(exe_path);
    let mut version_diff = BTreeMap::new();
    for (k, expected_v) in &profile.target.version_resource {
        let actual_v = actual_version.get(k).cloned();
        let matches = actual_v.as_deref() == Some(expected_v.as_str());
        version_diff.insert(
            k.clone(),
            VersionDiffRow {
                expected: Some(expected_v.clone()),
                actual: actual_v,
                matches,
            },
        );
    }
    let version_match = if profile.target.version_resource.is_empty() {
        None
    } else {
        Some(version_diff.values().all(|r| r.matches))
    };
    if version_match == Some(false) {
        notes.push("version_resource 存在差异（同键不同值）".into());
    } else if version_match == Some(true) {
        notes.push("version_resource 一致（疑同版本重编）".into());
    }

    let anchors = store
        .load_anchors(&profile.profile_id)
        .map_err(|e| format!("锚集读取失败：{e}"))?;
    let file = std::fs::read(exe_path).map_err(|e| format!("读取目标 exe：{e}"))?;
    let lites: Vec<AnchorLite> = anchors.anchors.iter().map(anchor_to_lite).collect();
    let verdict_raw = rescan(&file, &lites)?;
    let scans: Vec<AnchorScanResult> = verdict_raw
        .per_anchor
        .iter()
        .filter(|r| !r.note.starts_with("bb_hash"))
        .map(|r| AnchorScanResult {
            function: r.function.clone(),
            kind: nemesis_ziwei_profile::anchors::AnchorType::BytePattern, // 投票只按 matched 计数，kind 载体无语义
            matched: r.hit,
            hits: r.hit_count as u32,
        })
        .collect();
    let threshold = profile.verification.anchors_vote_threshold();
    let verdict = decide_verdict(false, &scans, threshold);
    let anchor_summary = AnchorVoteSummary {
        total: verdict_raw.anchors_total,
        supported: verdict_raw.anchors_supported,
        hit: verdict_raw.anchors_hit,
        threshold,
        passed: verdict == AttachVerdict::ActiveDegraded,
    };

    let (final_verdict, report_path, notice_emitted) = match verdict {
        AttachVerdict::ActiveDegraded => {
            let (path, emitted) = write_once_report(
                workspace,
                &profile.profile_id,
                &actual,
                "degraded",
                &serde_json::to_string_pretty(&serde_json::json!({
                    "verdict": "active_degraded",
                    "hash_expected": expected,
                    "hash_actual": actual,
                    "anchors": anchor_summary,
                    "notes": notes,
                }))
                .unwrap_or_default(),
            )?;
            if emitted {
                notes.push(
                    "降级注记（一次性）：目标文件已变但锚投票通过——语义面可用，建议空闲时重配"
                        .into(),
                );
                fire_event(
                    "ziwei.degraded",
                    &serde_json::json!({
                        "profile_id": profile.profile_id,
                        "hash_actual": actual,
                        "anchors": anchor_summary,
                    }),
                );
            }
            (Verdict::ActiveDegraded, Some(path), emitted)
        }
        AttachVerdict::Stale => {
            // 状态机先验（从旧状态）后变。
            profile
                .status
                .can_transition_to(ProfileStatus::Stale)
                .map_err(|e| format!("状态机拒绝：{e}"))?;
            let (path, emitted) = write_once_report(
                workspace,
                &profile.profile_id,
                &actual,
                "stale",
                &serde_json::to_string_pretty(&serde_json::json!({
                    "verdict": "stale",
                    "hash_expected": expected,
                    "hash_actual": actual,
                    "anchors": anchor_summary,
                    "notes": notes,
                }))
                .unwrap_or_default(),
            )?;
            if emitted {
                tracing::warn!(
                    "[ziwei] profile {} 失效：hash 不一致且锚投票未过（{anchor_summary:?}）",
                    profile.profile_id
                );
                fire_event(
                    "ziwei.stale",
                    &serde_json::json!({
                        "profile_id": profile.profile_id,
                        "hash_actual": actual,
                        "anchors": anchor_summary,
                    }),
                );
                emit_stale_notice(workspace, profile, &actual)?;
            }
            updated.status = ProfileStatus::Stale;
            (Verdict::Stale, Some(path), emitted)
        }
        AttachVerdict::Active => unreachable!("hash 不一致分支不可能 Active"),
    };
    updated.last_verified_at = chrono::Utc::now();
    let report = VerifyReport {
        profile_id: profile.profile_id.clone(),
        verdict: final_verdict,
        exe_path: exe_path.display().to_string(),
        hash_expected: Some(expected),
        hash_actual: actual,
        hash_match: false,
        version_diff,
        version_match,
        anchors: Some(anchor_summary),
        notice_emitted,
        notes,
        verified_at: now,
    };
    Ok(VerifyOutcome {
        report,
        profile_updated: Some(updated),
        report_path,
    })
}

/// 找回启动（T5.3 前半）：逐锚重扫 +（可选）LLM 语义比对 → 《找回报告》。
pub fn recover_start(
    store: &ProfileStore,
    workspace: &Path,
    profile_id: &str,
    exe_path: &Path,
) -> Result<serde_json::Value, String> {
    let profile = store.load(profile_id).map_err(|e| e.to_string())?;
    if profile.status != ProfileStatus::Stale {
        return Err(format!(
            "profile {profile_id} 状态为 {:?}（找回只对 stale 有意义）",
            profile.status
        ));
    }
    let anchors = store.load_anchors(profile_id).map_err(|e| e.to_string())?;
    let file = std::fs::read(exe_path).map_err(|e| format!("读取目标 exe：{e}"))?;
    let lites: Vec<AnchorLite> = anchors.anchors.iter().map(anchor_to_lite).collect();
    let verdict = rescan(&file, &lites)?;

    // LLM 语义比对（可选富化；确定性证据恒在——无钩子 = 诚实注记）。
    let llm_review = match crate::package::llm_hook() {
        Some(hook) => {
            let prompt = format!(
                "你是二进制取证复核员。旧 profile 的锚集在目标更新后重扫结果如下。\n\
                 旧锚与重扫命中：{}\n请逐锚给出语义一致性判断（命中位置是否仍承载原语义）\n\
                 与置信度（high/medium/low），一行一锚，格式：`<function>: <判断> (置信 <级别>)`。",
                serde_json::to_string_pretty(&verdict).unwrap_or_default()
            );
            match hook(&prompt) {
                Ok(reply) => reply,
                Err(e) => format!("（LLM 语义比对失败，以确定性证据为准：{e}）"),
            }
        }
        None => "（LLM 语义比对未装配——以确定性证据为准）".to_string(),
    };

    let report = serde_json::json!({
        "profile_id": profile_id,
        "exe_path": exe_path.display().to_string(),
        "verdict": verdict,
        "llm_review": llm_review,
        "next": if verdict.verdict == "unrecoverable" {
            "不可找回（全 miss）——重新配置向导"
        } else {
            "确认：ziwei recover-confirm（接受锚的函数名单；全接受 = 恢复 active）"
        },
    });
    let dir = workspace
        .join("reports")
        .join("ziwei-recover")
        .join(profile_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("报告目录创建失败：{e}"))?;
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let md_path = dir.join(format!("recover_{ts}.md"));
    let md = render_recover_md(&profile, &verdict, &llm_review, exe_path);
    std::fs::write(&md_path, md).map_err(|e| format!("报告写入失败：{e}"))?;
    Ok(serde_json::json!({
        "report": report,
        "report_md": md_path.display().to_string(),
    }))
}

/// 找回确认（T5.3 后半）：用户逐条确认 → 全接受 = 新指纹入库 + Active；
/// 有拒绝 = 保持 Stale。byte_pattern 锚按新镜像命中位置重生载荷（总控：
/// 重新生成锚）；语义型锚（imm/str/import）载荷即语义常量，原样保留。
pub fn recover_confirm(
    store: &ProfileStore,
    _workspace: &Path,
    profile_id: &str,
    exe_path: &Path,
    accepted_functions: &[String],
) -> Result<serde_json::Value, String> {
    let mut profile = store.load(profile_id).map_err(|e| e.to_string())?;
    if profile.status != ProfileStatus::Stale {
        return Err(format!(
            "profile {profile_id} 状态为 {:?}（只对 stale 可确认）",
            profile.status
        ));
    }
    let mut anchors = store.load_anchors(profile_id).map_err(|e| e.to_string())?;
    let file = std::fs::read(exe_path).map_err(|e| format!("读取目标 exe：{e}"))?;
    let img = nemesis_ziwei_rev::recover::load_image(&file)?;
    let lites: Vec<AnchorLite> = anchors.anchors.iter().map(anchor_to_lite).collect();
    let verdict = rescan(&file, &lites)?;

    let fn_names: std::collections::HashSet<&str> = profile
        .functions
        .iter()
        .map(|f| f.semantic.as_str())
        .collect();
    for acc in accepted_functions {
        if !fn_names.contains(acc.as_str()) {
            return Err(format!("接受的函数 {acc:?} 不在 profile.functions 中"));
        }
    }

    // 逐函数判定：接受且其锚全命中 → 重生锚；否则该函数保持 miss 状态。
    let mut per_function: Vec<serde_json::Value> = Vec::new();
    let mut regenerated = 0usize;
    let mut all_recovered = true;
    for f in &profile.functions {
        let accepted = accepted_functions.iter().any(|a| a == &f.semantic);
        let fn_anchors_ok = verdict
            .per_anchor
            .iter()
            .filter(|r| r.function == f.semantic && !r.note.starts_with("bb_hash"))
            .all(|r| r.hit);
        let recovered = accepted && fn_anchors_ok;
        if !recovered {
            all_recovered = false;
        }
        if accepted && fn_anchors_ok {
            // byte_pattern 重生：新镜像命中位置读同长载荷（先算后写，避开
            // 迭代 mut 借用与索引借用的冲突）。
            let mut new_payloads: Vec<(usize, String)> = Vec::new();
            for (ai, a) in anchors.anchors.iter().enumerate() {
                if a.function != f.semantic
                    || !matches!(
                        a.kind,
                        nemesis_ziwei_profile::anchors::AnchorType::BytePattern
                    )
                {
                    continue;
                }
                // 按锚自身下标取重扫结果（per_anchor[].index = 输入锚集下标）。
                // 只按 function 过滤会同函数多 byte_pattern 锚时永远拿到第一
                // 个命中锚的 RVA——第二个锚的 pattern 被错写成第一个锚命中
                // 位置的载荷（BUG-20261028-98①）。
                let Some(found) = verdict.per_anchor.iter().find(|r| r.index == ai && r.hit) else {
                    continue;
                };
                let Some(rva) = found.found_rva else { continue };
                // 载荷字节数与 rev::parse_pattern 同源：去空白后 hex 字符数/2
                // （空格 "AA BB" 与连续 "AABB" 双形态统一；split_whitespace
                // 计数对连续形态恒 1，重生 pattern 被截断成单字节
                // ——BUG-20261028-98②）。
                let old_len = a
                    .pattern
                    .as_deref()
                    .map(|p| p.split_whitespace().collect::<String>().len() / 2)
                    .unwrap_or(0);
                if old_len == 0 {
                    continue;
                }
                if let Some(bytes) = img.read_rva(rva, old_len) {
                    let payload: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
                    // 规范化后比较（pattern 存储形态可能带空格）；字节未变
                    // = 无需重生，否则未变化的锚也会被误记 regenerated。
                    let cur = a.pattern.as_deref().map(|p| p.replace(' ', ""));
                    if cur.as_deref() != Some(payload.as_str()) {
                        new_payloads.push((ai, payload));
                    }
                }
            }
            for (ai, payload) in new_payloads {
                anchors.anchors[ai].pattern = Some(payload);
                regenerated += 1;
            }
        }
        per_function.push(serde_json::json!({
            "function": f.semantic,
            "accepted": accepted,
            "anchors_hit": fn_anchors_ok,
            "recovered": recovered,
        }));
    }

    if all_recovered {
        profile
            .status
            .can_transition_to(ProfileStatus::Active)
            .map_err(|e| format!("状态机拒绝：{e}"))?;
        profile.status = ProfileStatus::Active;
        profile.target.file_sha256 = Some(sha256_hex(&file));
        profile.target.version_resource = extract_version_resource(exe_path);
        profile.last_verified_at = chrono::Utc::now();
    } else {
        notes_still_stale(&mut profile);
    }
    store
        .save(&profile, Some(&anchors))
        .map_err(|e| e.to_string())?;
    if all_recovered {
        fire_event(
            "ziwei.recovered",
            &serde_json::json!({ "profile_id": profile_id, "regenerated_anchors": regenerated }),
        );
    }
    Ok(serde_json::json!({
        "profile_id": profile_id,
        "all_recovered": all_recovered,
        "status": profile.status,
        "regenerated_anchors": regenerated,
        "per_function": per_function,
    }))
}

fn notes_still_stale(_p: &mut Profile) {
    // 保持 Stale（有拒绝/未命中项——诚实不猜，不给部分恢复态）
}

/// 全部锚命中的函数语义名（CLI confirm 缺省 accept 集；窄门面——bot 不摸 rev）。
pub fn hit_functions(
    store: &ProfileStore,
    profile_id: &str,
    exe_path: &Path,
) -> Result<Vec<String>, String> {
    let anchors = store.load_anchors(profile_id).map_err(|e| e.to_string())?;
    let file = std::fs::read(exe_path).map_err(|e| format!("读取目标 exe：{e}"))?;
    let lites: Vec<AnchorLite> = anchors.anchors.iter().map(anchor_to_lite).collect();
    let verdict = rescan(&file, &lites)?;
    let mut fns: Vec<String> = verdict
        .per_anchor
        .iter()
        .filter(|r| r.hit)
        .map(|r| r.function.clone())
        .collect();
    fns.sort();
    fns.dedup();
    Ok(fns)
}

/// profile::Anchor → rev::AnchorLite 中性映射。
fn anchor_to_lite(a: &nemesis_ziwei_profile::anchors::Anchor) -> AnchorLite {
    AnchorLite {
        function: a.function.clone(),
        kind: match a.kind {
            nemesis_ziwei_profile::anchors::AnchorType::BytePattern => "byte_pattern".into(),
            nemesis_ziwei_profile::anchors::AnchorType::StrRef => "str_ref".into(),
            nemesis_ziwei_profile::anchors::AnchorType::ImmConst => "imm_const".into(),
            nemesis_ziwei_profile::anchors::AnchorType::ImportCallSeq => "import_call_seq".into(),
            nemesis_ziwei_profile::anchors::AnchorType::BbHash => "bb_hash".into(),
        },
        pattern: a.pattern.clone(),
        text: a.text.clone(),
        value: a.value,
        imports: a.imports.clone(),
    }
}

/// 一次性报告：`reports/ziwei-verify/<profile_id>/<kind>_<hash8>.json` 已存在
/// = 同一 (profile, 新 hash) 已报告过 → 不再打扰（notice_emitted=false）。
fn write_once_report(
    workspace: &Path,
    profile_id: &str,
    hash_actual: &str,
    kind: &str,
    body: &str,
) -> Result<(PathBuf, bool), String> {
    let dir = workspace
        .join("reports")
        .join("ziwei-verify")
        .join(profile_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("报告目录创建失败：{e}"))?;
    let path = dir.join(format!("{kind}_{}.json", &hash_actual[..8]));
    if path.exists() {
        return Ok((path, false));
    }
    std::fs::write(&path, body).map_err(|e| format!("报告写入失败：{e}"))?;
    Ok((path, true))
}

/// stale 通知落盘（T5.2 出站通道面）：logs/ziwei/notify.jsonl 追加一行。
fn emit_stale_notice(workspace: &Path, profile: &Profile, hash_actual: &str) -> Result<(), String> {
    let dir = workspace.join("logs").join("ziwei");
    std::fs::create_dir_all(&dir).map_err(|e| format!("通知目录创建失败：{e}"))?;
    let line = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "kind": "stale",
        "profile_id": profile.profile_id,
        "profile_name": profile.name,
        "hash_actual": hash_actual,
        "hint": "Dashboard 紫微页横幅：重新配置 / 授权找回",
    });
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("notify.jsonl"))
        .map_err(|e| format!("通知文件打开失败：{e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("通知写入失败：{e}"))
}

/// 《找回报告》markdown 渲染。
fn render_recover_md(
    profile: &Profile,
    verdict: &nemesis_ziwei_rev::recover::RecoverVerdict,
    llm_review: &str,
    exe_path: &Path,
) -> String {
    let mut md = String::new();
    md.push_str(&format!("# 紫微找回报告 — {}\n\n", profile.profile_id));
    md.push_str(&format!(
        "- 生成时间：{}\n",
        chrono::Utc::now().to_rfc3339()
    ));
    md.push_str(&format!("- 目标 exe：{}\n", exe_path.display()));
    md.push_str(&format!(
        "- 总判定：**{}**（受支持锚 {}/{} 命中）\n\n",
        verdict.verdict, verdict.anchors_hit, verdict.anchors_supported
    ));
    md.push_str("| # | 函数 | 种类 | 命中 | 位置 | 处数 | 注记 |\n|---|------|------|------|------|------|------|\n");
    for r in &verdict.per_anchor {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            r.index,
            r.function,
            r.kind,
            if r.hit { "✓" } else { "✗" },
            r.found_rva.map(|v| format!("0x{v:X}")).unwrap_or_default(),
            r.hit_count,
            r.note
        ));
    }
    md.push_str(&format!("\n## LLM 语义比对\n\n{llm_review}\n"));
    md.push_str("\n> 真实三方目标的难度边界：结构重排/编译器差异导致的语义漂移\n> 需人工复核；本报告的证据链以确定性字节证据为准。\n");
    md
}

// ── version_resource 抽取（Win32 VERSIONINFO；非 Windows = 空表） ──────

#[cfg(windows)]
pub fn extract_version_resource(exe_path: &Path) -> BTreeMap<String, String> {
    let _ = exe_path;
    extract_version_resource_win(exe_path).unwrap_or_default()
}

#[cfg(not(windows))]
pub fn extract_version_resource(_exe_path: &Path) -> BTreeMap<String, String> {
    BTreeMap::new()
}

#[cfg(windows)]
fn extract_version_resource_win(exe_path: &Path) -> Option<BTreeMap<String, String>> {
    use std::ffi::c_void;

    #[link(name = "version")]
    unsafe extern "system" {
        fn GetFileVersionInfoSizeW(lptstr: *const u16, lpdwhandle: *mut u32) -> u32;
        fn GetFileVersionInfoW(
            lptstr: *const u16,
            dwhandle: u32,
            dellen: u32,
            lpdata: *mut c_void,
        ) -> i32;
        fn VerQueryValueW(
            pblock: *const c_void,
            lpsubblock: *const u16,
            lplpbuffer: *mut *mut c_void,
            pulen: *mut u32,
        ) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let path_w = wide(&exe_path.display().to_string());
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(path_w.as_ptr(), &mut handle);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(
            path_w.as_ptr(),
            handle,
            size,
            data.as_mut_ptr() as *mut c_void,
        ) == 0
        {
            return None;
        }
        // Translation 表 → 主语言码页
        let mut tr_ptr: *mut c_void = std::ptr::null_mut();
        let mut tr_len = 0u32;
        let tr_query = wide("\\VarFileInfo\\Translation");
        if VerQueryValueW(
            data.as_ptr() as *const c_void,
            tr_query.as_ptr(),
            &mut tr_ptr,
            &mut tr_len,
        ) == 0
            || tr_len < 4
        {
            return None;
        }
        let tr = std::slice::from_raw_parts(tr_ptr as *const u16, 2);
        let lang = std::format!("{:04x}{:04x}", tr[0], tr[1]);

        let mut out = BTreeMap::new();
        for key in [
            "ProductName",
            "FileVersion",
            "ProductVersion",
            "FileDescription",
            "CompanyName",
        ] {
            let q = wide(&format!("\\StringFileInfo\\{lang}\\{key}"));
            let mut s_ptr: *mut c_void = std::ptr::null_mut();
            let mut s_len = 0u32;
            if VerQueryValueW(
                data.as_ptr() as *const c_void,
                q.as_ptr(),
                &mut s_ptr,
                &mut s_len,
            ) != 0
                && s_len > 0
            {
                // Win32 契约：VerQueryValueW 的 pulLen 是**字节**数
                // （BUG-20261029-155：此前直接当 u16 元素数用——from_raw_parts
                // 读 2× 字节数，堆越界读正式 UB；平时靠 NUL 提前截断掩盖）。
                let w = std::slice::from_raw_parts(s_ptr as *const u16, s_len as usize / 2);
                let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
                let value = String::from_utf16_lossy(&w[..end]);
                if !value.is_empty() {
                    out.insert(key.to_string(), value);
                }
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests;
