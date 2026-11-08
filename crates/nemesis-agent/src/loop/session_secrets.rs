//! 会话作用域凭据占位符（B4，2026-11-08 七轮调研落地批次④）。
//!
//! 与 [`crate::loop::credential_injection`]（声明槽位 `vault:` 别名，配置
//! 驱动、跨会话别名、发卡恢复）互补的第二条凭据通路：**模型在执行中途经
//! question 凭据形态向用户收取的凭据**，落 vault 会话作用域别名
//! `agent/<会话>/<名>`，随后在**任意工具的任意字符串参数**里以模板
//! `{{vault:<alias>}}` 引用——dispatch 最内层（安全 8 层之后、execute 之前）
//! 展开替换，值全程不过 LLM 上下文。
//!
//! 安全红线（计划批次④钦定）——**作用域闸是主防线，结果掩码只是纵深**：
//! 占位符只解析**本会话作用域**别名（`agent/<本会话 sanitize>/` 前缀），
//! 引用全局/其他会话别名 = 诚实报错不解析（也不查库——不给别名存在性
//! 裁决）。没有这道闸，占位符就成了「读取 vault 任意凭据」的通用通道：
//! 注入攻击可借 web_fetch 等网络类工具的 URL 把凭据带出站（替换发生在
//! 本地、外发在远端，出站掩码救不回已发出的请求）。会话内凭据是本会话
//! 自己向用户索要的，能被会话内调用使用 = 与既有会话权限同级，不构成提权。
//!
//! 替换时机（顺序钉死）：安全 8 层管线全部跑完（各层看到占位符**原文**
//! ——Layer 4 凭据扫描不会对合法使用误拦，审计链落占位符原文天然
//! hash-only 干净）→ 本模块展开 → execute → 结果掩码。
//!
//! 结果掩码是尽力而为（明文子串扫描，编码绕过不保证）；空值不掩
//!（空串替换无意义）、按长度降序替换（长值优先，避免短值先替换破坏
//! 长值匹配）。

use nemesis_utils::sanitize::sanitize_path_segment;

/// 模板起始标记（`{{vault:<alias>}}`）。闭合标记 `}}`。
const TEMPLATE_HEAD: &str = "{{vault:";
const TEMPLATE_TAIL: &str = "}}";

/// 结果掩码占位文本（历史/日志/工具结果中凭据值的替身）。
pub const MASK_TEXT: &str = "[凭据已掩码]";

/// question 凭据形态结果里的接收占位文本（值本身不进上下文）。
pub fn received_text(field_key: &str) -> String {
    format!("[凭据已接收: {field_key}]")
}

/// 会话作用域别名前缀（收取侧写入与使用侧闸面的**单一真相源**——
/// 两臂共用本函数，防 sanitize 口径漂移）。
pub fn session_scope_prefix(session_key: &str) -> String {
    format!("agent/{}/", sanitize_path_segment(session_key))
}

/// 展开工具 args 中所有字符串值里的 `{{vault:<alias>}}` 占位符。
///
/// 返回 `(展开后 args JSON, 本调用出现的凭据明文清单)`——明文清单供
/// [`mask_secrets_in_result`] 在工具结果上做掩码，不进任何日志/历史。
/// args 非 JSON → 原样透传（与凭据注入层同构：交给工具自身校验响亮失败）。
/// 无占位符 → 原样透传零分配路径。
///
/// 错误（fail loud，工具不执行）：
/// - **越界别名**：alias 不带本会话 `agent/<会话>/` 前缀——报错含别名
///   原文（模型自己写的，非机密），**不查库**（无存在性信号）；
/// - **解析失败**：别名不存在 / vault 锁定 / 解析器未装配——vault 层
///   错误透传（诚实报错，不做静默纠偏）。
///
/// `{{vault:` 无 `}}` 闭合 → 按字面量保留（非合法模板形态）。
pub fn expand_session_placeholders(
    args: &str,
    session_key: &str,
) -> Result<(String, Vec<String>), String> {
    // 快速路径：整串不含模板头 → 原样返回（绝大多数工具调用零成本，
    // 且不经 serde round-trip——Map 重排序/浮点形态等序列化伪影全避开，
    // 无占位符调用对工具侧字节级无感）。
    if !args.contains(TEMPLATE_HEAD) {
        return Ok((args.to_string(), Vec::new()));
    }
    let mut val: serde_json::Value = match serde_json::from_str(args) {
        Ok(v) => v,
        Err(_) => return Ok((args.to_string(), Vec::new())),
    };
    let mut secrets = Vec::new();
    let scope = session_scope_prefix(session_key);
    walk_strings(&mut val, &scope, &mut secrets)?;
    // 占位符已全部替换（ secrets 只收集不残留）→ 序列化输出。此时键序
    // 经 serde Map 归一（BTreeMap 字典序）——语义等价（工具按 JSON 解析），
    // 该串只进 execute 不回模型上下文，无缓存面影响。
    let out = serde_json::to_string(&val).map_err(|e| format!("占位符展开序列化失败: {e}"))?;
    Ok((out, secrets))
}

/// 递归遍历 JSON 值，只改写字符串**值**（键名不动——占位符语义是「值里
/// 引用凭据」，键名改写会让对象结构漂移，宁缺毋滥）。
fn walk_strings(
    val: &mut serde_json::Value,
    scope: &str,
    secrets: &mut Vec<String>,
) -> Result<(), String> {
    match val {
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                walk_strings(v, scope, secrets)?;
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            for v in items.iter_mut() {
                walk_strings(v, scope, secrets)?;
            }
            Ok(())
        }
        serde_json::Value::String(s) => expand_in_string(s, scope, secrets).map(|_| ()),
        _ => Ok(()),
    }
}

/// 单个字符串值内的占位符展开（可多处、可嵌入更长字符串）。
fn expand_in_string(s: &mut String, scope: &str, secrets: &mut Vec<String>) -> Result<(), String> {
    // 快速路径：不含模板头 → 原样（绝大多数调用零成本）。
    if !s.contains(TEMPLATE_HEAD) {
        return Ok(());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    loop {
        let Some(head) = rest.find(TEMPLATE_HEAD) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..head]);
        let after_head = &rest[head + TEMPLATE_HEAD.len()..];
        let Some(tail_rel) = after_head.find(TEMPLATE_TAIL) else {
            // 无闭合：本处按字面量保留，继续扫后续（可能后面有合法模板）。
            out.push_str(TEMPLATE_HEAD);
            rest = after_head;
            continue;
        };
        let alias = after_head[..tail_rel].trim();
        if alias.is_empty() {
            return Err("占位符 {{vault:}} 别名为空——写作 {{vault:agent/<会话>/<名>}}".to_string());
        }
        if !alias.starts_with(scope) {
            return Err(format!(
                "占位符 {{{{vault:{alias}}}}} 越界：占位符只解析本会话凭据 \
                （agent/<本会话>/ 前缀，本会话经凭据形态 question 收取的值）；\
                 其他别名请走工具声明槽位的 vault: 引用（域绑定审批面管辖）"
            ));
        }
        // 作用域闸通过才查库（越界臂不查——无存在性信号）。
        let secret = match nemesis_config::resolve_vault_reference(&format!("vault:{alias}")) {
            Some(Ok(v)) => v,
            Some(Err(e)) => return Err(format!("占位符 {{{{vault:{alias}}}}} 解析失败: {e}")),
            None => {
                return Err(format!(
                    "占位符 {{{{vault:{alias}}}}} 解析失败: 引用形态异常"
                ));
            }
        };
        if !secret.is_empty() {
            secrets.push(secret.clone());
        }
        out.push_str(&secret);
        rest = &after_head[tail_rel + TEMPLATE_TAIL.len()..];
    }
    *s = out;
    Ok(())
}

/// 工具结果掩码（纵深防御，尽力而为）：结果文本中出现本调用任一凭据
/// 明文 → 替换为 [`MASK_TEXT`]。按长度降序替换（长值优先——避免短值
/// 先替换破坏长值匹配）；空清单/空值零成本直通。
pub fn mask_secrets_in_result(result: &str, secrets: &[String]) -> String {
    if secrets.is_empty() {
        return result.to_string();
    }
    let mut sorted: Vec<&String> = secrets.iter().collect();
    sorted.sort_by_key(|b| std::cmp::Reverse(b.len()));
    let mut out = result.to_string();
    for s in sorted {
        if !s.is_empty() {
            out = out.replace(s.as_str(), MASK_TEXT);
        }
    }
    out
}
