//! A4（2026-10-21 DeepInfer 思路引入）：安全校准——规则层 × guardian LLM
//! 二审的一致性标定。
//!
//! 借鉴 DeepInfer `rule_vs_llm_agreement` 的形态：同一批样本分别过规则
//! 引擎和 LLM 判断，量化两者的分歧面——规则层是确定性底线，LLM 二审
//! （guardian）管语义，两者长期共处，分歧样本就是「规则该不该改 /
//! judge 提示词该不该调」的工作清单。
//!
//! 数据流（`security calibrate`）：
//! 1. 读样本 JSONL（每行 `{"tool","args","actual","note"?}`）；
//! 2. 每条样本构造 [`nemesis_security::ToolInvocation`] 过规则管线
//!    （[`SecurityPlugin::execute`]）——**纯规则评估，样本永不执行**；
//!    装配与 gateway `build_security_plugin` 同源（层开关 + DLP 配置 +
//!    ABAC 规则读 `<home>/config/config.security.json`，共享
//!    `apply_dlp_config` / `apply_security_layer_switches` /
//!    `load_security_rules`；审批 pattern 记忆表同源接
//!    `<workspace>/config/approval_rules.json` 热载器），但**只读**——不走
//!    F3 自愈写入、审计链/审计日志恒 off（plugin 无文件组件）= 零磁盘副作用；
//! 3. `--judge-model` 给定时，另请 guardian LLM（同 gateway 的
//!    `GatewayLlmJudge` 提示词与解析，固定审计宪法）对同批样本出
//!    三值结论；`deny→block`、`allow→allow`、`ask`/解析失败→**单列
//!    不计入一致性分母**（不猜）;
//! 4. 产出混淆矩阵 + precision/recall + 两两一致率，JSON+MD 落
//!    `<home>/workspace/logs/calibration/<ts>/`。
//!
//! 纪律红线（计划钦定）：校准**永不修改任何配置**——只出报告；样本
//! **永不执行**——管线是评估面不是执行面。

use anyhow::{Context, Result};
use nemesis_security::guardian::{JudgeRequest, JudgeVerdict, LlmJudge};
use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 单条校准样本（JSONL 一行）。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CalibrateSample {
    /// 工具名（`exec` / `write_file` / ...——与管线静态危级表同名）。
    pub tool: String,
    /// 工具入参（JSON 对象；字符串 args 先按 JSON 解析，失败再包成
    /// `{"raw": ...}`——与真实 dispatch 的宽松口径一致）。
    pub args: Value,
    /// 人工标注的期望结果：`block` / `allow`（大小写不敏感；其他值该行
    /// 跳过并计数——脏数据不进混淆矩阵）。
    pub actual: String,
    /// 可选备注（报告里原样回显）。
    #[serde(default)]
    pub note: String,
    /// 样本在 JSONL 文件里的真实行号（解析后回填，不从 JSON 读；跳行
    /// 存在时报告「行」列与文件行号保持一致——上轮回查修正 2026-10-21）。
    #[serde(default)]
    pub src_line: usize,
}

/// ⑲（七轮回查 2026-10-21）：exec 族样本缺 `command` 键时管线 target
/// 静默为空，规则匹配整体失真（真实踩坑：手写样本用 `cmd` 键 → FN 假象，
/// 排查 15 分钟）。报告「备注」列诚实提示，不让失真矩阵被误读成规则盲区。
/// op 判定复用 [`nemesis_security::tool_to_operation`] 单一真相源，不复制名单。
fn annotate_sample_note(tool: &str, args: &serde_json::Value, note: &str) -> String {
    use nemesis_security::types::OperationType;
    let mut out = note.to_string();
    if nemesis_security::tool_to_operation(tool) == Some(OperationType::ProcessExec)
        && args.get("command").and_then(|v| v.as_str()).is_none()
    {
        if !out.is_empty() {
            out.push('；');
        }
        out.push_str("args 无 command 字段：target 为空，规则匹配失真（exec 族参数键为 command）");
    }
    out
}

/// 单条校准样本（JSONL 一行）解析 + 收集（脏行跳过并计数）。
///
/// 返回 (合法样本, 跳过行数)。跳过的行：空行/`#` 注释行（不计数）、
/// JSON 解析失败、`actual` 归一后非 block/allow（不猜）。样本带真实
/// 文件行号（[`CalibrateSample::src_line`]），报告行号与文件行号一致。
fn parse_samples<R: std::io::BufRead>(reader: R) -> anyhow::Result<(Vec<CalibrateSample>, usize)> {
    let mut samples = Vec::new();
    let mut skipped = 0usize;
    for (i, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("第 {} 行读取失败", i + 1))?;
        let mut t = line.trim();
        // 首行 BOM（PowerShell `Out-File -Encoding utf8BOM` 常见产物）不
        // 该吞掉第一份样本——剥掉再解析（六轮回查 2026-10-21）。
        if let Some(rest) = t.strip_prefix('\u{feff}') {
            t = rest.trim();
        }
        if t.is_empty() || t.starts_with('#') {
            continue; // 空行 / 注释行不计数
        }
        match serde_json::from_str::<CalibrateSample>(t) {
            Ok(mut s) => {
                // 真实文件行号随样本走（跳行存在时报告行号不与文件错位）。
                s.src_line = i + 1;
                // actual 归一并前置过滤：非 block/allow 的脏标注不进混淆
                // 矩阵（宁可少算不可错算——不猜「大概是 allow」）。
                s.actual = normalize_actual(&s.actual);
                if s.actual != "block" && s.actual != "allow" {
                    eprintln!(
                        "  [skip] 第 {} 行 actual 不是 block/allow: {}",
                        i + 1,
                        s.actual
                    );
                    skipped += 1;
                    continue;
                }
                samples.push(s);
            }
            Err(e) => {
                eprintln!("  [skip] 第 {} 行不是合法样本: {e}", i + 1);
                skipped += 1;
            }
        }
    }
    Ok((samples, skipped))
}

/// 规则层的判定（二值 + 拒因 + 审批态）。
#[derive(Debug, Clone, Serialize)]
pub struct RuleOutcome {
    pub blocked: bool,
    /// 三轮回查修正（2026-10-21）：deny 实为「需审批」（auditor 判
    /// RequireApproval、calibrate 无审批管理器 → execute 返回 block）。
    /// 矩阵仍按 block 口径计入（对无人值守场景「不自动放行」等价于
    /// block），但明细与 JSON 以此字段明示，不与硬拦混同。
    pub approval_required: bool,
    /// F5 DenyInfo 的 layer/policy/summary（blocked 时有）。
    pub layer: Option<String>,
    pub policy: Option<String>,
    pub summary: Option<String>,
}

/// guardian 二审判定（--judge-model 时有）。
#[derive(Debug, Clone, Serialize)]
pub struct JudgeOutcome {
    /// deny / allow / ask / parse_error / call_error。
    pub raw_recommendation: String,
    /// 归一：block / allow / excluded（ask 与失败单列，不进分母）。
    pub mapped: String,
    /// judge 自己的一句意图描述（报告可读性）。
    pub intent: String,
    pub rationale: String,
    pub error: Option<String>,
}

/// 单样本完整结果。
#[derive(Debug, Clone, Serialize)]
pub struct SampleOutcome {
    pub line: usize,
    pub tool: String,
    pub actual: String,
    pub note: String,
    pub rule: RuleOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgeOutcome>,
}

/// 汇总报告（JSON 形态）。
#[derive(Debug, Serialize)]
pub struct CalibrateReport {
    pub generated_at: String,
    pub total_samples: usize,
    pub skipped_lines: usize,
    /// 规则层 vs 人工标注：混淆矩阵四格。
    pub rule_true_positive: usize,
    pub rule_false_positive: usize,
    pub rule_true_negative: usize,
    pub rule_false_negative: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge: Option<JudgeAgreement>,
    pub samples: Vec<SampleOutcome>,
}

impl CalibrateReport {
    /// 规则层对 block 类的 precision（TP/(TP+FP)；分母 0 = None）。
    pub fn rule_precision(&self) -> Option<f64> {
        let d = self.rule_true_positive + self.rule_false_positive;
        (d > 0).then(|| self.rule_true_positive as f64 / d as f64)
    }

    /// 规则层对 block 类的 recall（TP/(TP+FN)）。
    pub fn rule_recall(&self) -> Option<f64> {
        let d = self.rule_true_positive + self.rule_false_negative;
        (d > 0).then(|| self.rule_true_positive as f64 / d as f64)
    }

    /// 规则层一致率（(TP+TN)/总数）。
    pub fn rule_agreement(&self) -> Option<f64> {
        let d = self.rule_true_positive
            + self.rule_false_positive
            + self.rule_true_negative
            + self.rule_false_negative;
        (d > 0).then(|| (self.rule_true_positive + self.rule_true_negative) as f64 / d as f64)
    }
}

/// judge 参与时的两两一致性（规则 × judge；excluded 不进分母）。
#[derive(Debug, Serialize)]
pub struct JudgeAgreement {
    /// 规则 block 且 judge deny。
    pub both_block: usize,
    /// 规则 allow 且 judge allow。
    pub both_allow: usize,
    /// 规则 block 但 judge allow（规则更严——误拦候选）。
    pub rule_stricter: usize,
    /// 规则 allow 但 judge deny（judge 更严——漏拦候选，**重点关注**）。
    pub judge_stricter: usize,
    /// judge ask / 解析失败 / 调用失败——单列不计分母。
    pub excluded: usize,
}

impl JudgeAgreement {
    pub fn agreement_rate(&self) -> Option<f64> {
        let d = self.both_block + self.both_allow + self.rule_stricter + self.judge_stricter;
        (d > 0).then(|| (self.both_block + self.both_allow) as f64 / d as f64)
    }
}

/// 校准入口（`security calibrate` 的实现）。
///
/// `judge_model` = Some 时额外跑 guardian 二审（模型走配置解析，同
/// `model probe` 的装配路径）。返回 (报告, 报告目录)。
pub async fn run_calibrate(
    home: &Path,
    samples_path: &Path,
    judge_model: Option<&str>,
    judge_interval_ms: u64,
) -> Result<(CalibrateReport, PathBuf)> {
    // ── 1. 读样本 ──
    let file = std::fs::File::open(samples_path)
        .with_context(|| format!("样本文件打开失败: {}", samples_path.display()))?;
    let (samples, skipped) = parse_samples(std::io::BufReader::new(file))?;
    anyhow::ensure!(!samples.is_empty(), "样本为空（或全部非法）——无可校准面");

    // ── 2. 规则层重放（生产同源装配，**只读**）──
    // 层开关 + DLP 配置（构造期，apply_dlp_config / apply_security_layer_switches）
    // 与 ABAC 规则（构造后 load_security_rules）都来自
    // <home>/config/config.security.json——与 gateway `build_security_plugin`
    // 同一批装配函数、同一份文件，校准矩阵描述的是**部署面**而非出厂默认。
    // 红线不变：只读——不触发 F3 自愈写入（不走 ensure_security_config）、
    // 不初始化审计链/审计日志（audit_* 恒 off，plugin 无文件组件），样本
    // 永不执行。文件缺失/解析失败 = 出厂默认规则集（诚实降级）。
    let mut plugin_config = SecurityPluginConfig {
        enabled: true,
        default_action: "allow".to_string(),
        ..Default::default()
    };
    let sec_config_path = crate::common::security_config_path(home);
    let sec_json = std::fs::read_to_string(&sec_config_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    if let Some(ref v) = sec_json {
        crate::security_setup::apply_dlp_config(v, &mut plugin_config);
        crate::security_setup::apply_security_layer_switches(v, &mut plugin_config);
    }
    let plugin = std::sync::Arc::new(SecurityPlugin::new(plugin_config));
    if sec_json.is_some() {
        crate::security_setup::load_security_rules(&plugin, &sec_config_path);
    } else {
        eprintln!(
            "  [note] {} 不存在或不可解析：按出厂默认规则集校准",
            sec_config_path.display()
        );
    }
    // F3 审批 pattern 记忆表（三轮回查修正 2026-10-21）：与 gateway
    // runtime.rs 同源——同一磁盘文件 `<workspace>/config/approval_rules.json`
    // + 同一 loader。「总是允许」pattern 的自动放行是生产判定面的一部分；
    // 不接它，有 pattern 的部署会把真实放行报成误拦（假 FP）。只读：
    // HotReloader 是查询侧磁盘读取，本命令不写该文件。
    let approval_rules_hot = std::sync::Arc::new(nemesis_config::HotReloader::new(
        nemesis_path::resolve_approval_rules_path_in_workspace(&crate::common::workspace_path(
            home,
        )),
        nemesis_security::approval_rules::load_rules,
    ));
    plugin.auditor().set_approval_rules(approval_rules_hot);
    // 四轮回查修正（2026-10-21，④/⑨ 横向扫描补完）：gateway 对判定面的
    // 其余派生接线逐项补齐——
    // ① 自毁硬拦保护路径（home / workspace root / `~`）+ workspace 豁免
    //    （security_setup.rs 同一构造：打保护路径的样本生产必拦，不接则
    //    校准漏报）；
    // ② pre-exec 扫描的相对脚本路径解析根（同 workspace root；未注入 =
    //    生产相对路径不解析，接入后与生产行为一致）；
    // ③ 免打扰开关（config.security.json `approvals.auto_approve`，typed
    //    同源 loader、读失败按缺省关）——开着时生产 RequireApproval 自动
    //    放行，不接则校准把这些操作报成 ask（假 FP）。
    // editor_access 刻意不接：运行时态、进程重启一律回关（security_setup
    // 注释钦定），新启动状态与 calibrate 现状等价；approval_manager /
    // guardian judge 刻意不接（交互面/另测面），见上方 ask 口径与模块文档。
    let workspace_root = crate::common::workspace_path(home);
    plugin.auditor().set_protected_paths(vec![
        home.to_string_lossy().to_string(),
        workspace_root.to_string_lossy().to_string(),
        "~".to_string(),
    ]);
    plugin
        .auditor()
        .set_self_destruct_exempt_path(&workspace_root.to_string_lossy());
    plugin.set_preexec_workspace_root(&workspace_root.to_string_lossy());
    let approvals_auto = match nemesis_config::load_security_config(&sec_config_path) {
        Ok(sec) => sec.approvals.auto_approve,
        Err(_) => false,
    };
    plugin
        .auditor()
        .set_approval_auto_approve(std::sync::Arc::new(
            nemesis_security::approval_auto::ApprovalAutoApproveState::new(approvals_auto),
        ));
    let mut outcomes = Vec::with_capacity(samples.len());
    for s in samples.iter() {
        let args = if s.args.is_object() || s.args.is_array() || s.args.is_string() {
            s.args.clone()
        } else {
            serde_json::json!({ "raw": s.args })
        };
        let note = annotate_sample_note(&s.tool, &args, &s.note);
        let invocation = nemesis_security::ToolInvocation {
            tool_name: s.tool.clone(),
            args,
            user: "calibrate".to_string(),
            source: "cli".to_string(),
            metadata: Default::default(),
        };
        let (allowed, deny) = plugin.execute(&invocation);
        // 「需审批」判定锚点：auditor RequireApproval 无管理器时 execute
        // 返回 deny summary 固定以 "approval required" 开头（auditor.rs
        // pending-request 臂），layer/policy 均为 "abac"。
        let approval_required = deny
            .as_ref()
            .is_some_and(|d| d.summary.starts_with("approval required"));
        outcomes.push(SampleOutcome {
            line: s.src_line,
            tool: s.tool.clone(),
            actual: normalize_actual(&s.actual),
            note,
            rule: RuleOutcome {
                blocked: !allowed,
                approval_required,
                layer: deny.as_ref().map(|d| d.layer.to_string()),
                policy: deny.as_ref().map(|d| d.policy.clone()),
                summary: deny.as_ref().map(|d| d.summary.clone()),
            },
            judge: None,
        });
    }

    // ── 3. guardian 二审（可选）──
    let mut judge_agreement = None;
    if let Some(model_alias) = judge_model {
        let judge = CalibrateJudge::create(home, model_alias).await?;
        let mut both_block = 0;
        let mut both_allow = 0;
        let mut rule_stricter = 0;
        let mut judge_stricter = 0;
        let mut excluded = 0;
        for (o, s) in outcomes.iter_mut().zip(samples.iter()) {
            let req = JudgeRequest {
                action: s.tool.clone(),
                risk_level: plugin.tool_danger_level(&s.tool).to_string(),
                command: serde_json::to_string_pretty(&s.args)
                    .unwrap_or_else(|_| format!("{}", s.args)),
            };
            let outcome = match judge.judge(&req).await {
                Ok(v) => map_verdict(&v),
                Err(e) => JudgeOutcome {
                    raw_recommendation: "call_error".to_string(),
                    mapped: "excluded".to_string(),
                    intent: String::new(),
                    rationale: String::new(),
                    error: Some(e),
                },
            };
            match outcome.mapped.as_str() {
                "block" if o.rule.blocked => both_block += 1,
                "block" => judge_stricter += 1,
                "allow" if o.rule.blocked => rule_stricter += 1,
                "allow" => both_allow += 1,
                _ => excluded += 1,
            }
            o.judge = Some(outcome);
            if judge_interval_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(judge_interval_ms)).await;
            }
        }
        judge_agreement = Some(JudgeAgreement {
            both_block,
            both_allow,
            rule_stricter,
            judge_stricter,
            excluded,
        });
    }

    // ── 4. 汇总 ──
    let mut report = CalibrateReport {
        generated_at: chrono::Local::now().to_rfc3339(),
        total_samples: outcomes.len(),
        skipped_lines: skipped,
        rule_true_positive: 0,
        rule_false_positive: 0,
        rule_true_negative: 0,
        rule_false_negative: 0,
        judge: judge_agreement,
        samples: outcomes,
    };
    for o in &report.samples {
        let predicted_block = o.rule.blocked;
        let actual_block = o.actual == "block";
        match (predicted_block, actual_block) {
            (true, true) => report.rule_true_positive += 1,
            (true, false) => report.rule_false_positive += 1,
            (false, true) => report.rule_false_negative += 1,
            (false, false) => report.rule_true_negative += 1,
        }
    }

    // ── 5. 报告落盘 ──
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let dir = home
        .join("workspace")
        .join("logs")
        .join("calibration")
        .join(ts.to_string());
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("report.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    std::fs::write(dir.join("report.md"), render_markdown(&report))?;
    Ok((report, dir))
}

/// actual 标注归一（大小写不敏感 + 常见同义词；归一后仍非 block/allow 的
/// 行在收集侧被跳过——见 `run_calibrate`，脏标注不进混淆矩阵）。
fn normalize_actual(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "block" | "blocked" | "deny" | "denied" => "block".to_string(),
        "allow" | "allowed" | "pass" => "allow".to_string(),
        other => other.to_string(),
    }
}

/// JudgeVerdict → 二值映射：deny→block / allow→allow / ask 与其他→excluded
/// （不猜——ask 的语义是「交人工」，既非确定拦也非确定放）。
fn map_verdict(v: &JudgeVerdict) -> JudgeOutcome {
    let mapped = match v.recommendation.trim().to_ascii_lowercase().as_str() {
        "deny" => "block",
        "allow" => "allow",
        _ => "excluded",
    };
    JudgeOutcome {
        raw_recommendation: v.recommendation.clone(),
        mapped: mapped.to_string(),
        intent: v.intent.clone(),
        rationale: v.rationale.clone(),
        error: None,
    }
}

/// 校准专用 judge：提示词/解析与 gateway 的 `GatewayLlmJudge` 同源
/// （GUARDIAN_PROMPT + parse_verdict），provider 按 `model probe` 同款
/// 装配路径从 config 构造——CLI 无 gateway 常驻，每次独立建。
struct CalibrateJudge {
    provider: std::sync::Arc<dyn nemesis_providers::router::LLMProvider>,
    model: String,
}

impl CalibrateJudge {
    async fn create(home: &Path, model_alias: &str) -> Result<Self> {
        let cfg_path = crate::common::config_path(home);
        let cfg = nemesis_config::load_config(&cfg_path)
            .map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))?;
        let resolution = nemesis_config::resolve_model_config(&cfg, model_alias)
            .map_err(|e| anyhow::anyhow!("Failed to resolve model '{}': {}", model_alias, e))?;
        let factory_cfg = nemesis_providers::factory::FactoryConfig {
            proxy: resolution.proxy.clone(),
            stream_mode: resolution.stream_mode.clone(),
            cache_breakpoints: resolution.cache_breakpoints,
            llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
            api_key: resolution.api_key.clone(),
            api_base: resolution.api_base.clone(),
            workspace: home.join("workspace").to_string_lossy().to_string(),
            connect_mode: resolution.connect_mode,
            protocol: resolution.protocol.clone(),
            timeout_secs: resolution.timeout_secs,
            account_id: String::new(),
            headers: std::collections::HashMap::new(),
        };
        let provider = nemesis_providers::factory::create_provider(&factory_cfg)
            .map_err(|e| anyhow::anyhow!("Failed to create provider: {}", e))?;
        Ok(Self {
            provider: provider as std::sync::Arc<dyn nemesis_providers::router::LLMProvider>,
            model: resolution.model_name.clone(),
        })
    }
}

#[async_trait::async_trait]
impl LlmJudge for CalibrateJudge {
    async fn judge(&self, req: &JudgeRequest) -> std::result::Result<JudgeVerdict, String> {
        // 与 GatewayLlmJudge 同一请求形态（固定宪法 + `<command>` 包裹的
        // 命令本体）——校准面与生产面的 judge 看到的东西字节一致。
        let user = format!(
            "工具：{}\n管线危险级别：{}\n\n<command>\n{}\n</command>\n\n请审计上述命令，立即输出 JSON 结论。",
            req.action, req.risk_level, req.command
        );
        let messages = vec![
            nemesis_providers::types::Message {
                role: "system".into(),
                content: nemesis_security::guardian::GUARDIAN_PROMPT
                    .to_string()
                    .into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
            },
            nemesis_providers::types::Message {
                role: "user".into(),
                content: user.into(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: std::collections::HashMap::new(),
            },
        ];
        let opts = nemesis_providers::types::ChatOptions {
            temperature: Some(0.0),
            max_tokens: Some(4096),
            top_p: None,
            stop: None,
            reasoning_effort: None,
            extra: std::collections::HashMap::new(),
        };
        let resp = self
            .provider
            .chat(&messages, &[], &self.model, &opts)
            .await
            .map_err(|e| format!("guardian LLM call failed: {}", e))?;
        nemesis_security::guardian::parse_verdict(&resp.content)
    }
}

/// Markdown 报告渲染。
fn render_markdown(r: &CalibrateReport) -> String {
    let mut out = String::new();
    out.push_str("# 安全校准报告（security calibrate）\n\n");
    out.push_str(&format!("- 时间：{}\n", r.generated_at));
    out.push_str(&format!(
        "- 样本：{}（跳过脏行 {}）\n",
        r.total_samples, r.skipped_lines
    ));
    out.push_str("- 纪律：样本**未执行**（纯规则评估 + 可选 LLM 二审）；本报告不修改任何配置。\n");
    out.push_str("- 口径：规则判「需审批」（ask）时矩阵按 block 计入（对无人值守\n");
    out.push_str("  「不自动放行」等价于 block），逐样本明细以 **ask** 标出，不与硬拦混同。\n\n");

    out.push_str("## 规则层 vs 人工标注\n\n");
    out.push_str("| | 实际 block | 实际 allow |\n|---|---|---|\n");
    out.push_str(&format!(
        "| **规则 block** | TP = {} | FP = {} |\n",
        r.rule_true_positive, r.rule_false_positive
    ));
    out.push_str(&format!(
        "| **规则 allow** | FN = {} | TN = {} |\n\n",
        r.rule_false_negative, r.rule_true_negative
    ));
    let pct = |v: Option<f64>| match v {
        Some(x) => format!("{:.1}%", x * 100.0),
        None => "-".to_string(),
    };
    out.push_str(&format!(
        "- precision（block 类）：{}\n- recall（block 类）：{}\n- 一致率：{}\n\n",
        pct(r.rule_precision()),
        pct(r.rule_recall()),
        pct(r.rule_agreement())
    ));

    if let Some(j) = &r.judge {
        out.push_str("## 规则层 × guardian 二审\n\n");
        out.push_str("| 双 block | 双 allow | 规则更严 | judge 更严（漏拦候选） | 单列 |\n|---|---|---|---|---|\n");
        out.push_str(&format!(
            "| {} | {} | {} | **{}** | {} |\n\n",
            j.both_block, j.both_allow, j.rule_stricter, j.judge_stricter, j.excluded
        ));
        out.push_str(&format!(
            "- 一致率（排除单列）：{}\n- judge 更严样本 = 规则层漏拦候选，逐条核对见下表。\n\n",
            pct(j.agreement_rate())
        ));
    }

    out.push_str("## 逐样本明细\n\n");
    out.push_str("| 行 | 工具 | 标注 | 规则 | judge | 备注 |\n|---|---|---|---|---|---|\n");
    for o in &r.samples {
        let rule = if o.rule.blocked {
            if o.rule.approval_required {
                "ask（需审批）".to_string()
            } else {
                format!("block（{}）", o.rule.policy.as_deref().unwrap_or("-"))
            }
        } else {
            "allow".to_string()
        };
        let judge = match &o.judge {
            Some(j) => format!("{} → {}", j.raw_recommendation, j.mapped),
            None => "-".to_string(),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            o.line,
            o.tool,
            o.actual,
            rule,
            judge,
            o.note.replace('|', "\\|")
        ));
    }
    out
}

#[cfg(test)]
mod tests;
