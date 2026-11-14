//! 图像生成独立 lane（`images-openai` 协议，集群专业职能框架 M4）。
//!
//! **不走 Provider trait**（chat 形状完全不符：无流式、无工具调用、单 POST
//! + JSON）。单一请求函数 [`generate_image`]：POST `{api_base}/images/
//! generations`，`response_format: "b64_json"` 优先——无 URL 过期/二次下载
//! 面（响应只认 `b64_json`，端点只回 URL = 诚实报错，不做二次下载——
//! 服务端返回 URL 的下载面是 SSRF 供给源，刻意不开）。
//!
//! **gpt-image-1 系兼容**（BUG-20261106-008）：gpt-image-1/1.5/2 拒绝
//! `response_format` 参数（HTTP 400 unknown parameter）且该族只回 b64——
//! 端点 400 且 body 明确抱怨该参数时，剥掉重试一次（重试只发生在立即拒绝
//! 的快路径上，不占生成时长；对接受该参数的端点零影响）。
//!
//! 认证与超时对齐 chat lane 语义：Bearer key + 每请求墙钟超时。

use std::time::Duration;

/// 一次图像生成请求。
#[derive(Debug, Clone)]
pub struct ImageRequest {
    /// 图像模型名（model_list 条目的 wire 名）。
    pub model: String,
    /// 提示词（要画什么）。
    pub prompt: String,
    /// 尺寸（如 "1024x1024"）；None = 端点默认。
    pub size: Option<String>,
}

/// 单张生成结果（base64 编码的图像字节）。
#[derive(Debug, Clone, PartialEq)]
pub struct ImageResult {
    /// 图像字节（base64 编码，`data[]` 元素的 `b64_json` 原文）。
    pub b64_json: String,
}

/// 生成图像（单张；n 固定 1——多张交给调用方多次请求，失败面更可控）。
///
/// `timeout_secs` = 本次请求墙钟预算（`tools.image_gen.timeout_secs`）。
pub async fn generate_image(
    api_base: &str,
    api_key: &str,
    request: &ImageRequest,
    timeout_secs: u64,
) -> Result<ImageResult, String> {
    if api_base.trim().is_empty() {
        return Err("图像端点未配置（api_base 为空）".to_string());
    }
    if api_key.trim().is_empty() {
        return Err("图像端点未配置 API key".to_string());
    }
    if request.prompt.trim().is_empty() {
        return Err("prompt 为空".to_string());
    }

    let url = format!(
        "{}/images/generations",
        api_base.trim().trim_end_matches('/')
    );
    let mut body = serde_json::json!({
        "model": request.model,
        "prompt": request.prompt,
        "n": 1,
        "response_format": "b64_json",
    });
    if let Some(size) = request
        .size
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        body["size"] = serde_json::json!(size);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.max(1)))
        // 凭据随重定向跨主机泄漏防线（nemesis-utils::http_redirect 单一
        // 真相源——同源跟随/跨源终停）。
        .redirect(nemesis_utils::http_redirect::no_cross_host_redirect_policy())
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    let (mut status, mut text) = post_for_text(&client, &url, api_key, &body, timeout_secs).await?;

    // BUG-20261106-008：gpt-image-1 系拒绝 response_format 参数（官方 400
    // 形态 "Unknown parameter: 'response_format'."）且该族恒回 b64——端点
    // 明确抱怨该参数时剥掉重试一次。激活条件从严：400 + body 同时点名参数
    // 名与抱怨措辞（unknown/unrecognized/not supported/invalid 族），其余
    // 400（额度/内容策略）不重试。
    if status == reqwest::StatusCode::BAD_REQUEST && complains_about_response_format(&text) {
        tracing::warn!("图像端点拒绝 response_format 参数（gpt-image-1 系形态），剥掉重试一次");
        let mut retry = body.clone();
        if let Some(obj) = retry.as_object_mut() {
            obj.remove("response_format");
        }
        let (s, t) = post_for_text(&client, &url, api_key, &retry, timeout_secs).await?;
        status = s;
        text = t;
    }

    if !status.is_success() {
        // 4xx 额度/参数、5xx 上游——原文尾部截断透传（契约降级纪律的输入）。
        let chars: Vec<char> = text.chars().collect();
        let start = chars.len().saturating_sub(600);
        let tail: String = chars[start..].iter().collect();
        return Err(format!("图像端点 HTTP {status}: {tail}"));
    }

    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("图像响应非 JSON: {e}；body 头部 {}", head(&text, 200)))?;
    let data = parsed
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| format!("图像响应缺 data 数组；body 头部 {}", head(&text, 200)))?;
    let first = data
        .first()
        .ok_or_else(|| "图像响应 data 为空数组".to_string())?;
    let b64 = first.get("b64_json").and_then(|v| v.as_str()).ok_or_else(|| {
        "端点未返回 b64_json（可能只回 URL）——本通道不做 URL 二次下载，请改用支持 b64_json 的端点"
            .to_string()
    })?;
    if b64.trim().is_empty() {
        return Err("端点返回了空 b64_json".to_string());
    }
    Ok(ImageResult {
        b64_json: b64.to_string(),
    })
}

fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 发请求 + 读 body（错误语义集中一处；两臂共用——首试与剥参重试）。
///
/// body 读传输失败诚实报（BUG-20261029-153 同族）：unwrap_or_default 会把
/// 200 路径断连伪装成「图像响应非 JSON」误导诊断（去查根本不存在的格式
/// 问题）、非 200 路径详情丢失。读失败按连接中断/超时报，状态码非成功
/// 时一并保留（诊断不丢现场）。
async fn post_for_text(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &serde_json::Value,
    timeout_secs: u64,
) -> Result<(reqwest::StatusCode, String), String> {
    let resp = client
        .post(url)
        .bearer_auth(api_key.trim())
        .json(body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                format!("图像生成超时（{timeout_secs}s）：{e}")
            } else {
                format!("图像请求失败: {e}")
            }
        })?;

    let status = resp.status();
    // 双用拆分（BUG-20261113-164）：成功体（b64 图像，合法 MB 级载荷）全量
    // 读；错误体只喂诊断文案——有帽读取（nemesis-utils::http_cap 共享单一
    // 真相源），失控/恶意端点的巨大错误体不再灌满内存。
    let text = if status.is_success() {
        match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                let reason = if e.is_timeout() {
                    "超时"
                } else {
                    "连接中断"
                };
                return Err(format!("图像响应 body 读取失败（{reason}）: {e}"));
            }
        }
    } else {
        nemesis_utils::http_cap::read_error_body(resp).await
    };
    Ok((status, text))
}

/// 400 响应体是否明确抱怨 `response_format` 参数：body 同时点名参数名与
/// 抱怨措辞（unknown/unrecognized/not supported/invalid 族，大小写不敏感，
/// 覆盖 OpenAI `"code": "unknown_parameter"` 与 message 两种承载形态）。
/// 从严激活——只认端点点名该参数的 400，额度/内容策略类不触发重试。
fn complains_about_response_format(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("response_format")
        && [
            "unknown parameter",
            "unknown_parameter",
            "unrecognized",
            "not supported",
            "unsupported",
            "invalid parameter",
        ]
        .iter()
        .any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests;
