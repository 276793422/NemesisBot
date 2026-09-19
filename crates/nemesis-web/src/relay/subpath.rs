//! 桥子路径支持（goal：反向桥与多设备汇聚，一期批次二）。
//!
//! 经中继远程访问某设备面板时，浏览器看到的 URL 是
//! `/d/<node_id>/...`（中继侧路径），中继把**原样 URI** 封帧转发给设备；
//! 设备本机 web server 的职责是把这段前缀「消化」掉：
//!
//! 1. **剥前缀**：请求路径以 `/d/<自身 node_id>/`（或 `/d/<自身 node_id>`）
//!    开头 → 剥掉前缀再进 router——面板的路由/API/WS 全部按无前缀语义
//!    匹配，「桥不解析内容」原则保持（剥前缀是纯路径操作，与内容无关）；
//! 2. **base 注入**：对 HTML 响应注入 `<base href>`——直连注入
//!    `<base href="/">`、剥前缀请求注入 `<base href="/d/<node_id>/">`。
//!    配合前端构建相对化（vite `base: './'`），页面里的相对资源引用
//!    按 base 解析，两种访问形态都落到正确路径；
//! 3. **他人前缀不动**：`/d/<其他 node_id>/...` 是本机内置中继的转发
//!    请求（多设备汇聚：别的设备桥到本机）——不剥不注入，原样交给
//!    relay 转发路由。远程设备的 HTML 由**远程设备自己**注入 base。
//!
//! base href 不影响 JS 运行时构造的 URL（`/api/...`、`/ws` 等根相对
//! 字符串）——那些由前端 `appBase()` helper 统一处理（web/src/lib）。
//!
//! 本层对所有路由生效（Router::layer 最外层），包括 relay 转发路由——
//! 但注入/剥离的判定都基于请求原始路径，转发请求天然不在注入集合内。

use axum::body::Body;
use axum::extract::Request;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use std::task::{Context, Poll};
use tower::Service;

/// HTML 响应注入 base 前的最大 body 尺寸（防御上限；本项目 HTML 壳
/// 均为 KB 量级，16MB 已远超正常范围）。
const MAX_HTML_BYTES: usize = 16 * 1024 * 1024;

/// 桥子路径 service 外壳（goal 批次二）。
///
/// **为什么是 service 外壳而不是 `Router::layer` 中间件**：axum 0.8 的
/// `Router::layer` 包装在每个 Route endpoint 外，运行于**路由匹配之后**
/// （axum-0.8.9 `routing/mod.rs` `Router::layer` → `path_router.layer`）——
/// 中间件里改 URI 来不及，匹配已经按原路径定局（实测：剥前缀后
/// `/health` 仍落 fallback）。因此用「无路由外壳 Router +
/// fallback_service」形态：外壳收到全部请求 → 同步改写 URI → 内层
/// Router 按剥前缀后的路径匹配 → 响应按需注入 `<base href>`。
#[derive(Clone)]
pub struct BridgeSubpathService {
    /// 自身 node_id（`set_bridge_identity` 注入；None = 无身份，前缀
    /// 剥离永不命中，仅保留直连 base 注入）。
    node_id: Option<String>,
    inner: axum::Router,
}

impl BridgeSubpathService {
    /// 组装外壳（server.rs build_router 尾部调用）。
    pub fn new(node_id: Option<String>, inner: axum::Router) -> Self {
        Self { node_id, inner }
    }
}

impl Service<Request<Body>> for BridgeSubpathService {
    type Response = Response;
    type Error = std::convert::Infallible;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // inner Router 的 poll_ready 恒 Ready（axum 语义），直接就绪。
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        // URI 改写必须在**同步段**完成（内层 Router 匹配前）。
        let inject_base = strip_self_prefix(&self.node_id, req.uri_mut());
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let res = inner.call(req).await?;
            Ok(match inject_base {
                Some(base) => inject_base_into_html(res, &base).await,
                None => res,
            })
        })
    }
}

/// 自身桥前缀剥离（URI 原位改写）+ 注入基准判定。
///
/// 三种形态（详见模块头注释）：
/// - `/d/<自身>/...` → 剥前缀，返回 `Some("/d/<自身>/")`
/// - `/d/<其他>...`  → 内置中继的转发请求，不动，返回 None
/// - 其余（直连）    → 返回 `Some("/")`
pub(crate) fn strip_self_prefix(node_id: &Option<String>, uri: &mut http::Uri) -> Option<String> {
    let path = uri.path().to_string();
    let Some(rest) = path.strip_prefix("/d/") else {
        return Some("/".to_string());
    };
    let (first, tail) = match rest.split_once('/') {
        Some((f, t)) => (f, Some(t)),
        None => (rest, None),
    };
    if first.is_empty() || Some(first) != node_id.as_deref() {
        // 他人前缀（转发请求）或畸形空段——原样透传。
        return None;
    }
    // 剥前缀：`/d/<id>` → `/`；`/d/<id>/x/y?q` → `/x/y?q`
    let new_pq = match tail {
        Some(t) if !t.is_empty() => format!("/{}", t),
        _ => "/".to_string(),
    };
    match rewrite_path_and_query(uri, &new_pq) {
        Ok(new_uri) => {
            *uri = new_uri;
            Some(format!("/d/{}/", first))
        }
        Err(e) => {
            // URI 重写失败（理论不可达：输入本就是合法路径）——原样
            // 透传给内层 Router（含 relay 转发路由），由其诚实处置。
            tracing::warn!("[BridgeSubpath] URI 重写失败 {}: {}", path, e);
            None
        }
    }
}

/// 把 URI 的 path_and_query 整体替换为 `new_path`（+ 原 query）。
fn rewrite_path_and_query(uri: &http::Uri, new_path: &str) -> Result<http::Uri, String> {
    let mut parts = uri.clone().into_parts();
    let pq = match uri.query() {
        Some(q) => format!("{}?{}", new_path, q),
        None => new_path.to_string(),
    };
    parts.path_and_query = Some(
        pq.parse()
            .map_err(|e: http::uri::InvalidUri| format!("path_and_query 解析失败: {e}"))?,
    );
    http::Uri::from_parts(parts).map_err(|e| format!("URI 重组失败: {e}"))
}

/// HTML 响应注入 `<base href>`（goal 批次二）。
///
/// 仅处理 `text/html` 且有 body 的响应；注入点 = `<head>` 标签闭合后
/// （退而 `<html>` 后；再退最前）。HTML 已含 `<base ` 时不重复注入。
async fn inject_base_into_html(res: Response, base: &str) -> Response {
    let is_html = res
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("text/html"))
        .unwrap_or(false);
    if !is_html {
        return res;
    }

    let (mut parts, body) = res.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_HTML_BYTES).await {
        Ok(b) => b,
        Err(e) => {
            // body 已被消费、无法重建原响应——诚实 500（理论不可达：
            // 本地 handler 产出的内存 body 不会超限）。
            tracing::error!("[BridgeSubpath] HTML body 读取失败: {}", e);
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "bridge subpath: failed to buffer html body",
            )
                .into_response();
        }
    };
    if bytes.is_empty() {
        // HEAD 等无 body 响应不注入（注入会产生协议错乱的 body）。
        return Response::from_parts(parts, axum::body::Body::from(bytes));
    }
    let Some(new_body) = inject_base_tag(&bytes, base) else {
        // 已有 <base>——原样返回。
        return Response::from_parts(parts, axum::body::Body::from(bytes));
    };
    if let Ok(v) = HeaderValue::from_str(&new_body.len().to_string()) {
        parts.headers.insert(http::header::CONTENT_LENGTH, v);
    }
    Response::from_parts(parts, axum::body::Body::from(new_body))
}

/// 在 HTML 字节里找注入点并插入 `<base href="...">`。
/// 返回 None = 不需要注入（已含 base 标签）。
pub(crate) fn inject_base_tag(html: &[u8], base: &str) -> Option<Vec<u8>> {
    if find_subslice(html, b"<base ").is_some() {
        return None;
    }
    let tag = format!(r#"<base href="{}">"#, base).into_bytes();
    // 注入点优先级：<head ...> > <html ...> > 最前。vite 产物恒有小写
    // `<head>`；取 marker 后第一个 `>` 即标签闭合（HTML 标签属性不含
    // 裸 `>`，此简化安全）。
    for marker in [&b"<head"[..], &b"<html"[..]] {
        if let Some(pos) = find_subslice(html, marker)
            && let Some(gt) = html[pos..].iter().position(|&b| b == b'>')
        {
            let at = pos + gt + 1;
            let mut out = Vec::with_capacity(html.len() + tag.len());
            out.extend_from_slice(&html[..at]);
            out.extend_from_slice(&tag);
            out.extend_from_slice(&html[at..]);
            return Some(out);
        }
    }
    // 兜底：无 head/html 标签（非正常 HTML）——插最前。
    let mut out = tag;
    out.extend_from_slice(html);
    Some(out)
}

/// 字节子串查找（与 protocol::find_subslice 同款；此处独立小实现避免
/// 为测试面把内部辅助 pub 化）。
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}
