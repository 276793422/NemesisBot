//! 凭据别名最后一刻注入（P0 vault 计划 C1/C2，2026-09-22 计划 §3）。
//!
//! 通用机制原则：本模块不认识任何业务名词（不知道"webhook"、"api_key"是
//! 什么）——工具通过 [`crate::loop::Tool::credential_arg_keys`] **声明**自己
//! 参数里承载凭据别名的槽位；机制只在 dispatch 瀑布最内层、`execute` 调用
//! 前的一瞬，把槽位里的 `vault:<alias>` 改写成真值（copy 语义）。
//!
//! 为什么是"最后一刻 + copy"：`HookToolCall.arguments` 这一个字符串派生出
//! 全部四个泄漏表面——pre-hooks、ToolEventHook 的 args_preview、会话历史
//! StoredToolCall、observer/request_logger。在最内层闭包里改写**副本**，
//! 上游全部表面天然只见别名；改写后的真值只活在 `execute` 的入参里。
//!
//! 槽位语义：值以 `vault:` 开头 → 经全局解析器现查（与 provider api_key
//! 同链路，见 nemesis_config::vault_ref）；非 vault 值（模型传的字面量）
//! 原样通过（向后兼容，工具自行决定语义）。解析失败 → 返回模型可读的
//! 错误串（fail loud，带补救指引），工具不执行。

use crate::r#loop::Tool;

/// 把 `args`（工具参数 JSON）中声明槽位里的 `vault:` 引用改写为真值。
///
/// 返回 `Ok(改写后的 JSON 串)`；槽位为空或无槽位声明时原样返回。
/// `Err(错误串)` = 某个槽位解析失败——调用方把它直接作为工具结果返回给
/// 模型，本轮不执行工具。
pub fn inject_credential_aliases(tool: &dyn Tool, args: &str) -> Result<String, String> {
    let keys = tool.credential_arg_keys();
    if keys.is_empty() {
        return Ok(args.to_string());
    }
    let mut val: serde_json::Value = match serde_json::from_str(args) {
        Ok(v) => v,
        // 非 JSON 参数：交给工具自身/上游校验去响亮失败，注入层不伪装。
        Err(_) => return Ok(args.to_string()),
    };
    let serde_json::Value::Object(map) = &mut val else {
        return Ok(args.to_string());
    };
    for key in keys {
        let Some(serde_json::Value::String(slot)) = map.get_mut(*key) else {
            // 槽位缺席/非字符串：工具的 schema 校验域，注入层不代管。
            continue;
        };
        if let Some(resolved) = nemesis_config::resolve_vault_reference(slot) {
            match resolved {
                Ok(secret) => *slot = secret,
                Err(e) => {
                    return Err(format!(
                        "⛔ CREDENTIAL REFERENCE FAILED [layer:credential|field:{key}] {e}。\
                         工具未执行。不要重试同一别名；若别名正确，请让用户运行 \
                         `nemesisbot vault set <alias>` 写入后再试。"
                    ));
                }
            }
        }
    }
    serde_json::to_string(&val).map_err(|e| format!("凭据注入序列化失败: {e}"))
}
