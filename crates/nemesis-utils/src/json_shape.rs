//! JSON document shape gates for external input (BUG-20261120-027/029/030
//! 家族统一定义).
//!
//! serde derive 对全字段带 `#[serde(default)]` 的 struct 同时实现
//! `visit_map` 与 `visit_seq`：把 `[]` 喂给这类 struct 走 `visit_seq`
//! 零元素臂 = 「全字段落 default」的**合法值**——`from_value`/`from_str`
//! 不报错。后果：错误形状的外部载荷（WSAPI save 口、hooks.json、
//! config.json 文档）被静默解释成「整份配置全默认」——静默重置还成功
//! 返回。字符串/数字/null 形态走 visit_str/visit_* 诚实报错，唯独数组
//! 形态有洞。
//!
//! 纪律：**反序列化 struct 之前先验形状**——配置文档唯一合法形状是
//! object；数组/标量一律 loud 拒绝。Vec 目标（空数组合法）、下游有版本/
//! 必填字段闸兜底的位点不受本洞影响，不需要闸。
//!
//! 本模块是 `json_type_name` 文案的单一真相源（此前 nemesis-web 用
//! "bool"、nemesis-agent 用 "boolean"，同语义两种拼法——家族第三处
//! 落点时上移收口）。

use serde_json::Value;

/// 人类可读的 JSON 类型名（报错文案用）。
pub fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 形状闸：`v` 必须是 JSON object，否则返回
/// `"{what} must be a JSON object, got {type}"`。
///
/// 调用点 = struct 反序列化**之前**（见模块文档的 visit_seq 洞）。
/// 错误通道是 `String`——crate 自有错误类型（ConfigError::Validation、
/// handler String、解析层 String）各自包一层。
pub fn ensure_object(v: &Value, what: &str) -> Result<(), String> {
    if v.is_object() {
        Ok(())
    } else {
        Err(format!(
            "{what} must be a JSON object, got {}",
            json_type_name(v)
        ))
    }
}
