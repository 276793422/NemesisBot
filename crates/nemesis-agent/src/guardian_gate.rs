//! Guardian（LLM 安全法官）语义二审闸——决策表单一真相源。
//!
//! BUG-20261025-37：此前 guardian 审查只活在 `loop/tool_dispatch.rs` 的
//! 内联块里；LSP rename 的合成 `write_file` 闸（`loop_tools/lsp_tool.rs`
//! `gate_and_write`）只跑 `security.execute()` 八层规则管线，guardian
//! 缺席——而文档声称「guardian 按既有语义继承」。分叉窗口：guardian_mode
//! =high 时直写 write_file 会因破坏形态内容被词表扫中送审，rename 同
//! 内容却直落磁盘。本模块把 dispatch 的审查决策表抽出来供两处同源消费。
//! 语义与原内联块逐字节一致（⛔ 文案原样迁移），dispatch 行为零变化。

use nemesis_security::auditor::ApprovalContext;
use nemesis_security::guardian::JudgeRequest;
use nemesis_security::pipeline::SecurityPlugin;

/// 审查结论：放行 / 已拦（携带完整 ⛔ 工具结果文本，调用方原样回灌）。
pub(crate) enum GuardianGate {
    Allow,
    Blocked(String),
}

/// P5 guardian（LLM safety judge）语义二审。规则层 allow 之后调用；是否
/// 送审由 guardian_mode 决定（未配置 = 不审零成本；critical = 仅
/// CRITICAL；high = HIGH+CRITICAL 且破坏形态词表命中）。`danger` 由调用
/// 方按连接器 op 投影（slug fail-closed CRITICAL）或静态表算好。
///
/// 请求刻意 CONTEXT-FREE（宪法）：judge 只见操作本身 + 危级元数据。
/// D2：judge Err 不静默落空，按 guardian_failure_policy 分支
/// （allow=显式放行 / ask=审批直通车 fail-closed / deny=拒绝 /
/// 未配置=旧行为放行 / 未知值=fail-closed）。
///
/// 只升格拍板（2026-09-16）：judge ask/deny 不无声硬拦——转人工审批卡
/// （误报留人工通道）；用户拒绝 / 无审批通道 = fail-closed 拒绝。
pub(crate) async fn guardian_gate(
    security: &SecurityPlugin,
    tool_name: &str,
    args_json: &str,
    danger: &str,
    approval_ctx: &ApprovalContext,
) -> GuardianGate {
    if !security.guardian_should_review_with_danger(tool_name, args_json, danger) {
        return GuardianGate::Allow;
    }
    let Some(judge) = security.judge() else {
        return GuardianGate::Allow;
    };
    let req = JudgeRequest {
        action: tool_name.to_string(),
        risk_level: danger.to_string(),
        command: args_json.to_string(),
    };
    match judge.judge(&req).await {
        Ok(v) => {
            let guardian_mode = security.guardian_mode();
            security
                .auditor()
                .log_guardian_verdict(tool_name, &guardian_mode, &v);
            if v.is_allow() {
                tracing::info!(
                    "[GuardianGate] Guardian allowed {} (rec={}, rules_match={})",
                    tool_name,
                    v.recommendation,
                    v.matches_rules
                );
                GuardianGate::Allow
            } else {
                match security.auditor().request_guardian_verdict_approval(
                    tool_name,
                    &v.risk_level,
                    &guardian_mode,
                    &v,
                    Some(approval_ctx),
                ) {
                    Ok(av) if av.approved => {
                        tracing::info!(
                            "[GuardianGate] Guardian flagged {} (rec={}) but user approved",
                            tool_name,
                            v.recommendation
                        );
                        GuardianGate::Allow
                    }
                    Ok(av) => {
                        let note = av
                            .note
                            .as_deref()
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                            .map(|n| format!(": {n}"))
                            .unwrap_or_default();
                        GuardianGate::Blocked(format!(
                            "⛔ GUARDIAN FLAGGED — USER REJECTED [layer:guardian|policy:guardian_mode={}] The safety judge flagged this operation ({}: {}) and the user declined to approve. Do NOT retry. Inform the user.{}",
                            guardian_mode, v.recommendation, v.rationale, note
                        ))
                    }
                    Err(e) => GuardianGate::Blocked(format!(
                        "⛔ GUARDIAN FLAGGED [layer:guardian|policy:guardian_mode={}] The safety judge flagged this operation ({}: {}) and no approval channel is available ({}). Fail-closed: operation denied. Do NOT retry. Inform the user.",
                        guardian_mode, v.recommendation, v.rationale, e
                    )),
                }
            }
        }
        Err(guardian_err) => match security.guardian_failure_policy().as_str() {
            "allow" => {
                tracing::warn!(
                    "[GuardianGate] Guardian failed on {} (guardian_failure_policy=allow, proceeding): {}",
                    tool_name,
                    guardian_err
                );
                GuardianGate::Allow
            }
            "ask" => {
                match security.auditor().request_guardian_failure_approval(
                    tool_name,
                    &format!("Guardian (LLM safety judge) failed: {}", guardian_err),
                    Some(approval_ctx),
                ) {
                    Ok(v) if v.approved => {
                        tracing::info!(
                            "[GuardianGate] Guardian failed on {}: user approved",
                            tool_name
                        );
                        GuardianGate::Allow
                    }
                    Ok(v) => {
                        let note = v
                            .note
                            .as_deref()
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                            .map(|n| format!(": {n}"))
                            .unwrap_or_default();
                        GuardianGate::Blocked(format!(
                            "⛔ GUARDIAN UNAVAILABLE — USER REJECTED [layer:guardian|policy:guardian_failure_policy=ask] Safety judge failed ({}) and the user declined to approve. Do NOT retry. Inform the user.{}",
                            guardian_err, note
                        ))
                    }
                    Err(e) => GuardianGate::Blocked(format!(
                        "⛔ GUARDIAN UNAVAILABLE [layer:guardian|policy:guardian_failure_policy=ask] Safety judge failed ({}) and no approval channel is available ({}). Fail-closed: operation denied. Do NOT retry. Inform the user.",
                        guardian_err, e
                    )),
                }
            }
            "deny" => GuardianGate::Blocked(format!(
                "⛔ GUARDIAN UNAVAILABLE — DENIED BY POLICY [layer:guardian|policy:guardian_failure_policy=deny] Safety judge failed ({}). Fail-closed: operation denied by policy. Do NOT retry. Inform the user.",
                guardian_err
            )),
            other if !other.is_empty() => {
                tracing::warn!(
                    "[GuardianGate] Unknown guardian_failure_policy {:?} on {}, treating as deny (fail-closed)",
                    other,
                    tool_name
                );
                GuardianGate::Blocked(format!(
                    "⛔ GUARDIAN UNAVAILABLE — DENIED BY POLICY [layer:guardian|policy:guardian_failure_policy={}] Safety judge failed ({}). Fail-closed: operation denied. Do NOT retry. Inform the user.",
                    other, guardian_err
                ))
            }
            _ => GuardianGate::Allow,
        },
    }
}
