//! 会话作用域凭据占位符（B4，2026-11-08 七轮调研落地批次④）。
//!
//! 与 [`crate::loop::credential_injection`]（声明槽位 `vault:` 别名，配置
//! 驱动、跨会话别名、发卡恢复）互补的第二条凭据通路：**模型在执行中途经
//! question 凭据形态向用户收取的凭据**，落 vault 会话作用域别名
//! `agent/<会话>/<名>`，随后在**工具声明的凭据槽位**里以模板
//! `{{vault:<alias>}}` 引用——dispatch 最内层（安全 8 层之后、execute 之前）
//! 展开替换，值全程不过 LLM 上下文。
//!
//! **槽位收紧（2026-11-11，用户拍板）**：占位符只允许出现在工具声明槽位
//! 的字符串值里（顶层字符串槽 | 对象槽叶子深度恒 1，与声明槽改写
//! [`crate::loop::credential_injection`] 同形）；槽外任何字符串值出现完整
//! 模板 = fail loud 工具不执行。此前「任意字符串参数都展开」的语义下，
//! 模板写进 exec 的 command 等普通参数时真值会进命令行——进程列表全用户
//! 可见，违反「真值不进命令行」不变量。无闭合片段按字面量保留（文档/
//! 示例文本里写「{{vault:…」不算引用，不误伤）。
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
//! ——Layer 4 凭据扫描与 Layer 5 DLP 做**匹配级引用豁免**
//!（[`nemesis_config::vault_ref::strip_reference_forms`]：命中文本剔除全部
//! vault 引用形态——完整模板 `{{vault:...}}` **与裸引用 `vault:<alias>`**
//! ——后重扫，仍命中才算真命中），引用不再被当明文秘密误拦（裸形态 =
//! 配置文件合法写法，2026-11-10 用户拍板豁免；根因/防回退/未来出路见
//! `strip_reference_forms` 文档），明文/混拼/并存秘密照旧拦；审计链落
//! 占位符原文天然 hash-only 干净）→ 本模块展开（模板形态；裸形态引用
//! 归 [`crate::loop::credential_injection`] 声明槽改写，不经此处）→
//! execute → 结果掩码。
//!
//! 结果掩码是尽力而为（明文子串扫描，编码绕过不保证）；空值不掩
//!（空串替换无意义）、按长度降序替换（长值优先，避免短值先替换破坏
//! 长值匹配）。

use nemesis_config::vault_ref::{TEMPLATE_HEAD, TEMPLATE_TAIL};
use nemesis_utils::sanitize::sanitize_path_segment;

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

/// 展开工具 args 中**声明凭据槽位内**字符串值里的 `{{vault:<alias>}}` 占位符。
///
/// `slots` = 工具声明的凭据槽位键（[`crate::loop_tools::tools_trait::Tool::
/// credential_arg_keys`]）；槽位面与声明槽改写同形：顶层字符串槽 | 对象槽
/// 叶子深度恒 1（[`super::credential_injection`] `walk_declared_slots` 契约）。
///
/// 返回 `(展开后 args JSON, 本调用出现的凭据明文清单)`——明文清单供
/// [`mask_secrets_in_result`] 在工具结果上做掩码，不进任何日志/历史。
/// args 非 JSON → 原样透传（与凭据注入层同构：交给工具自身校验响亮失败）。
/// 无占位符 → 原样透传零分配路径。
///
/// 错误（fail loud，工具不执行）：
/// - **槽外模板（placement）**：槽外任何字符串值出现**完整模板**（有头有
///   尾）——报错含字段路径与别名原文（模型自己写的，非机密），**不查库**；
///   placement 检查优先于作用域检查（槽外越界别名报 placement 错）；
/// - **越界别名（scope）**：槽内 alias 不带本会话 `agent/<会话>/` 前缀——
///   同样不查库（无存在性信号）；
/// - **解析失败**：别名不存在 / vault 锁定 / 解析器未装配——vault 层
///   错误透传（诚实报错，不做静默纠偏）。
///
/// `{{vault:` 无 `}}` 闭合 → 按字面量保留（非合法模板形态；槽外同理
/// 不报错——文档/示例文本误伤零化）。
pub fn expand_session_placeholders_in_slots(
    args: &str,
    session_key: &str,
    slots: &[&str],
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
    match &mut val {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                let declared = slots.contains(&key.as_str());
                match v {
                    // 顶层字符串槽：槽面内，展开。
                    serde_json::Value::String(s) if declared => {
                        expand_in_string(s, &scope, &mut secrets)?;
                    }
                    // 对象槽：叶子深度恒 1（与声明槽改写同形）——字符串叶
                    // 展开，更深/非字符串结构一概属槽外检测面（宁缺毋滥）。
                    serde_json::Value::Object(inner) if declared => {
                        for (leaf_key, leaf) in inner.iter_mut() {
                            match leaf {
                                serde_json::Value::String(s) => {
                                    expand_in_string(s, &scope, &mut secrets)?;
                                }
                                other => {
                                    let mut path = format!("{key}.{leaf_key}");
                                    detect_templates_outside_slots(other, &mut path, slots)?;
                                }
                            }
                        }
                    }
                    // 其余一切（未声明键 / 声明键下的数组等）都在槽外 → 检测。
                    other => {
                        let mut path = key.clone();
                        detect_templates_outside_slots(other, &mut path, slots)?;
                    }
                }
            }
        }
        // 顶层非对象（数组/标量）：不存在顶层键 = 无声明槽面，全在槽外。
        other => {
            let mut path = String::new();
            detect_templates_outside_slots(other, &mut path, slots)?;
        }
    }
    // 槽内占位符已全部替换（secrets 只收集不残留）→ 序列化输出。此时键序
    // 经 serde Map 归一（BTreeMap 字典序）——语义等价（工具按 JSON 解析），
    // 该串只进 execute 不回模型上下文，无缓存面影响。
    let out = serde_json::to_string(&val).map_err(|e| format!("占位符展开序列化失败: {e}"))?;
    Ok((out, secrets))
}

/// 槽外**完整模板**检测（递归）。只检测不改写——发现即 Err（fail loud，
/// 工具不执行、无 checkpoint 残留），报错含字段路径与别名原文（模型自写
/// 非机密，可回显）。无闭合片段不算（按字面量保留，见模块文档）。
fn detect_templates_outside_slots(
    val: &serde_json::Value,
    path: &mut String,
    slots: &[&str],
) -> Result<(), String> {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter() {
                let mark = path.len();
                path.push('.');
                path.push_str(k);
                let r = detect_templates_outside_slots(v, path, slots);
                path.truncate(mark);
                r?;
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                let mark = path.len();
                path.push('.');
                path.push_str(&i.to_string());
                let r = detect_templates_outside_slots(v, path, slots);
                path.truncate(mark);
                r?;
            }
            Ok(())
        }
        serde_json::Value::String(s) => {
            if let Some(alias) = first_complete_template(s) {
                let declared = if slots.is_empty() {
                    "本工具无声明凭据槽位".to_string()
                } else {
                    format!("声明槽位：{}", slots.join(", "))
                };
                return Err(format!(
                    "⛔ CREDENTIAL TEMPLATE OUTSIDE SLOT [layer:session_secret|field:{path}] \
                     占位符 {{{{vault:{alias}}}}} 出现在声明凭据槽位之外（{declared}）。\
                     {OUTSIDE_SLOT_GUIDANCE}"
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// 槽外模板报错的处置指引（常量串——花括号字面量免转义）。
const OUTSIDE_SLOT_GUIDANCE: &str = "占位符只允许在工具声明的凭据槽位里引用（顶层字符串槽或\
     对象叶子，如 exec 族 env / web_fetch headers）——写在命令行/URL/普通参数里会把凭据真值\
     暴露到进程列表或外发内容。请改走声明槽位（如 exec 的 env 参数放 \
     {\"KEY\": \"{{vault:agent/<会话>/<名>}}\"}）。工具未执行。";

/// 字符串内首个**完整模板**（有头有尾）的别名；只有无闭合片段 → None。
fn first_complete_template(s: &str) -> Option<String> {
    let mut rest = s;
    while let Some(rel) = rest.find(TEMPLATE_HEAD) {
        let after_head = &rest[rel + TEMPLATE_HEAD.len()..];
        if let Some(tail_rel) = after_head.find(TEMPLATE_TAIL) {
            return Some(after_head[..tail_rel].trim().to_string());
        }
        rest = after_head;
    }
    None
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
