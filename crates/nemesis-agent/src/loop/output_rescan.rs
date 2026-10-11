//! 主线工具输出凭据/DLP 复扫（S2②，2026-10-09 高优差距批次一）。
//!
//! 8 层安全管线的凭据扫描（第④层）与 DLP（第⑤层）只看工具**入参**
//! （`SecurityPlugin::execute` 只收 `ToolInvocation.args`）；出侧结果此前
//! 零检查——`scan_tool_output` 在库里但主线从不调用，唯一调用点是 eval
//! 沙盒 worker（`nemesisbot/src/eval_worker.rs`）。后果：`read_file` 读到
//! config.json 的明文 model key、`exec` 回显环境变量秘钥时，原文直接进
//! 模型上下文 / 会话历史 / 审计台账。
//!
//! 本模块在 dispatch 链尾（hooks 之后、Forge 记录之前）做**脱敏式**复扫：
//! 工具已经执行，此处拦停无意义；命中即 `redact_content` 遮蔽 + 尾注诚实
//! 告知模型，目标是明文秘密不进上下文。与 WASM 插件工具的出站凭据复扫
//! 同思想——主线此前反而没有。
//!
//! action 语义对齐（出侧改写必须由明确配置授权，不比入参侧更激进）：
//! 凭据扫描 action 非 block/redact（如 warn）= 只观测不改写（
//! `scan_tool_output` 自带 WARN 记账）；DLP 逐匹配看 `effective_action`，
//! 全部命中都属观测档（warn/log）时不改写。
//!
//! 已知边界：DLP 检测面按引擎自身语义截前 5KB（`scan_tool_output` 内部
//! 截断）；命中后 `redact_content` 对全文遮蔽（超集）。凭据扫描无截断，
//! 全文进出。

use nemesis_security::credential::Scanner as CredentialScanner;
use nemesis_security::dlp::DlpEngine;
use nemesis_security::pipeline::SecurityPlugin;

/// 输出侧复扫入口：`None`（安全模块未装配/未启用）= 原样返回。
pub(crate) fn rescan_tool_output(
    plugin: Option<&SecurityPlugin>,
    tool_name: &str,
    result: String,
) -> String {
    let Some(plugin) = plugin else {
        return result;
    };
    rescan_pair(
        plugin.credential_scanner(),
        plugin.dlp_engine(),
        tool_name,
        result,
    )
}

/// 组装核：独立成函数让测试直扫双扫描器组合，不必构造整个 SecurityPlugin。
pub(crate) fn rescan_pair(
    credential: Option<&CredentialScanner>,
    dlp: Option<&DlpEngine>,
    tool_name: &str,
    mut result: String,
) -> String {
    if result.is_empty() {
        return result;
    }
    let mut notes: Vec<String> = Vec::new();
    if let Some(scanner) = credential {
        rescan_credential(scanner, tool_name, &mut result, &mut notes);
    }
    if let Some(engine) = dlp {
        rescan_dlp(engine, tool_name, &mut result, &mut notes);
    }
    if notes.is_empty() {
        return result;
    }
    result.push('\n');
    result.push_str(&notes.join("\n"));
    result
}

/// 该 action 是否授权出侧改写（block = 入参侧拦停的出侧对应物；redact =
/// 显式脱敏）。其余（warn/log/未知）= 只观测——扫描器自带的 warn/debug
/// 日志照常记账，但不修改模型可见的输出。
fn is_enforcing(action: &str) -> bool {
    action == "block" || action == "redact"
}

fn rescan_credential(
    scanner: &CredentialScanner,
    tool_name: &str,
    result: &mut String,
    notes: &mut Vec<String>,
) {
    let r = scanner.scan_tool_output(tool_name, result);
    if !r.has_matches || !is_enforcing(scanner.get_action()) {
        return;
    }
    *result = scanner.redact_content(result);
    notes.push(format!(
        "[输出已脱敏: 检测到疑似凭据 {} 处，已遮蔽为 [REDACTED_CREDENTIAL]]",
        r.matches.len()
    ));
}

fn rescan_dlp(engine: &DlpEngine, tool_name: &str, result: &mut String, notes: &mut Vec<String>) {
    let r = engine.scan_tool_output(tool_name, result);
    if !r.has_matches {
        return;
    }
    if !r.matches.iter().any(|m| is_enforcing(&m.effective_action)) {
        tracing::debug!(
            tool = tool_name,
            count = r.matches.len(),
            "[AgentLoop] DLP output re-scan: matches are observe-only, not redacted"
        );
        return;
    }
    tracing::warn!(
        tool = tool_name,
        count = r.matches.len(),
        "[AgentLoop] DLP output re-scan: tool output redacted"
    );
    *result = engine.redact_content(result);
    notes.push(format!(
        "[输出已脱敏: DLP 检测到敏感信息 {} 处，已遮蔽]",
        r.matches.len()
    ));
}
