//! 壳调用结果类型 + JSON 解析（原 WIT bindgen 面的宿主侧替身）。
//!
//! 壳的 `call_export` 返回结果 JSON（status=ok 时）：
//! - ok 臂 → `{"ok": <payload|null>}`
//! - err 臂 → `{"err": {"tag": "<host-error 变体>", "detail": "<string>"}}`
//!
//! get-metadata payload = `{name,title,description,parameters-json,operation-type}`
//!（kebab-case，WIT record 字段名直投影）；execute payload =
//! `{content,is-error}`；observe payload = null。

/// 工具元数据（get-metadata 结果；字段名对齐 WIT tool-metadata record）。
#[derive(Debug, Clone)]
pub struct ToolMetadata {
    /// 工具基础名（guest 自报）。
    pub name: String,
    /// 人读标题。
    pub title: String,
    /// 描述。
    pub description: String,
    /// 参数 JSON Schema（字符串形态）。
    pub parameters_json: String,
    /// 操作类型声明（read/write/exec/network/空）。
    pub operation_type: String,
}

/// host-error 五变体（对齐 WIT `host-error`；tag 词表见壳 `result_val`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostErr {
    /// 无权限（egress 未授予等）。
    NoPermission,
    /// 策略拒绝（带原因）。
    PolicyDenied(String),
    /// 目标不存在（带说明）。
    NotFound(String),
    /// 帧预算耗尽（带说明）。
    BudgetExceeded(String),
    /// 能力不可用（观察者帧收窄/未装配，带说明）。
    Unavailable(String),
}

impl std::fmt::Display for HostErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostErr::NoPermission => write!(f, "no-permission"),
            HostErr::PolicyDenied(s) => write!(f, "policy-denied({s})"),
            HostErr::NotFound(s) => write!(f, "not-found({s})"),
            HostErr::BudgetExceeded(s) => write!(f, "budget-exceeded({s})"),
            HostErr::Unavailable(s) => write!(f, "unavailable({s})"),
        }
    }
}

/// 解析壳结果 JSON：Ok(payload) 或 Err(HostErr)。
fn parse_result_json(out_json: &str) -> Result<serde_json::Value, HostErr> {
    let v: serde_json::Value = serde_json::from_str(out_json)
        .map_err(|e| HostErr::Unavailable(format!("call result decode failed: {e}")))?;
    if let Some(err) = v.get("err") {
        let tag = err
            .get("tag")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unavailable");
        let detail = err
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Err(match tag {
            "no-permission" => HostErr::NoPermission,
            "policy-denied" => HostErr::PolicyDenied(detail),
            "not-found" => HostErr::NotFound(detail),
            "budget-exceeded" => HostErr::BudgetExceeded(detail),
            _ => HostErr::Unavailable(detail),
        });
    }
    Ok(v.get("ok").cloned().unwrap_or(serde_json::Value::Null))
}

/// get-metadata 结果解析。
pub fn parse_get_metadata(out_json: &str) -> Result<ToolMetadata, HostErr> {
    let v = parse_result_json(out_json)?;
    let s = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Ok(ToolMetadata {
        name: s("name"),
        title: s("title"),
        description: s("description"),
        parameters_json: s("parameters-json"),
        operation_type: s("operation-type"),
    })
}

/// execute 结果解析（tool-output record：content + is-error）。
pub fn parse_execute(out_json: &str) -> Result<Result<(String, bool), HostErr>, HostErr> {
    let v = parse_result_json(out_json)?;
    if !v.is_object() || v.get("content").is_none() {
        // guest 显式回 host-error（设计上只发生在非预期形态）。
        return Ok(Err(HostErr::Unavailable(format!(
            "unexpected tool-output shape: {v}"
        ))));
    }
    Ok(Ok((
        v.get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        v.get("is-error")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    )))
}

/// observe 结果解析（payload = null 即成功）。
pub fn parse_observe(out_json: &str) -> Result<Result<(), HostErr>, HostErr> {
    let v = parse_result_json(out_json)?;
    if v.is_null() {
        Ok(Ok(()))
    } else {
        // 宽和面：observe 返回类型是 result<(), host-error>，正常成功
        // payload 恒 null；非 null 视为形态漂移（不失败，warn 由调用方落）。
        Ok(Ok(()))
    }
}
