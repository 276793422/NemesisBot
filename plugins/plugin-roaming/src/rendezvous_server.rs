//! rendezvous 成员名册——服务端半边（axum 注册表）。
//!
//! 搬自宿主 `nemesis-cluster::roaming::rendezvous_server`（QUIC 插件化 M1）。
//! 差异：类型走本插件 `types.rs` 副本；日志走 vtable 转发（插件不落自己的
//! 日志文件）；其余（token 门禁、载荷校验与钳制、TTL 剔除、容量闸、axum
//! 薄壳两端点）逐行同源。

use crate::types::{MAX_MEMBERS, MEMBER_TTL_SECS, RendezvousMember, valid_iroh_id};
use axum::response::IntoResponse;
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// 字段钳制（公网面上的自我防御：超长输入不放大存储）。
const NODE_ID_MAX: usize = 64;
const NAME_MAX: usize = 64;
const BUILD_MAX: usize = 64;
const RELAY_URL_MAX: usize = 256;
const TAG_ITEM_MAX: usize = 32;
const TAG_COUNT_MAX: usize = 16;

/// 注册错误（校验失败；HTTP 语义由 handler 映射）。
#[derive(Debug, PartialEq)]
pub enum RegisterError {
    /// 必填字段缺失或非法（iroh hex / node_id / relay_url 形态）。
    Invalid(String),
    /// 名册已满。
    Full,
}

// ---------------------------------------------------------------------------
// 校验与钳制
// ---------------------------------------------------------------------------

/// 注册载荷校验 + 钳制（非法 loud 拒、超长截断不放大）。
pub fn sanitize_member(mut m: RendezvousMember) -> Result<RendezvousMember, RegisterError> {
    m.node_id = m.node_id.trim().to_string();
    if m.node_id.is_empty() {
        return Err(RegisterError::Invalid("node_id 为空".to_string()));
    }
    if m.node_id.chars().count() > NODE_ID_MAX {
        return Err(RegisterError::Invalid(format!(
            "node_id 超长（> {NODE_ID_MAX} 字符）"
        )));
    }
    m.iroh_id = m.iroh_id.trim().to_lowercase();
    // 64-hex 校验（镜像宿主 endpoint 单一真相源——非法身份对谁都没用）。
    if let Err(e) = valid_iroh_id(&m.iroh_id) {
        return Err(RegisterError::Invalid(format!("iroh_id 非法: {e}")));
    }
    m.name = clamp_str(&m.name, NAME_MAX);
    m.role = clamp_str(&m.role, TAG_ITEM_MAX);
    m.category = clamp_str(&m.category, TAG_ITEM_MAX);
    m.node_type = clamp_str(&m.node_type, TAG_ITEM_MAX);
    m.build = clamp_str(&m.build, BUILD_MAX);
    m.relay_url = m.relay_url.trim().to_string();
    if m.relay_url.chars().count() > RELAY_URL_MAX {
        return Err(RegisterError::Invalid(format!(
            "relay_url 超长（> {RELAY_URL_MAX} 字符）"
        )));
    }
    if !m.relay_url.is_empty()
        && !m.relay_url.starts_with("http://")
        && !m.relay_url.starts_with("https://")
    {
        return Err(RegisterError::Invalid(
            "relay_url 必须为 http(s):// 形态".to_string(),
        ));
    }
    m.tags = clamp_list(&m.tags);
    m.professions = clamp_list(&m.professions);
    if let Some(t) = m.tier.as_mut() {
        *t = clamp_str(t, TAG_ITEM_MAX);
    }
    Ok(m)
}

/// 截断到上限字符数（按字符不按字节——CJK 名字友好）。
fn clamp_str(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

/// 列表钳制：项数 ≤ TAG_COUNT_MAX，单项 ≤ TAG_ITEM_MAX，空项丢弃。
fn clamp_list(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|s| clamp_str(s, TAG_ITEM_MAX))
        .filter(|s| !s.is_empty())
        .take(TAG_COUNT_MAX)
        .collect()
}

// ---------------------------------------------------------------------------
// 内存名册存储
// ---------------------------------------------------------------------------

/// 内存名册（token 门禁 + TTL 剔除 + 容量闸；纯函数核心，axum 薄壳）。
pub struct RendezvousRegistry {
    /// Some(token) = 门禁生效；None = 放行（生产入口 token 非空才拉起
    /// 注册表，fail-closed；None 仅供测试 loopback）。
    token: Option<String>,
    members: parking_lot::RwLock<HashMap<String, (RendezvousMember, Instant)>>,
}

impl RendezvousRegistry {
    pub fn new(token: &str) -> Self {
        let t = token.trim();
        Self {
            token: (!t.is_empty()).then(|| t.to_string()),
            members: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// token 校验（Bearer 头或 ?token= 查询参数；与中继 SharedTokenAccess
    /// 同款语义）。
    pub fn token_ok(&self, bearer: Option<&str>, query_token: Option<&str>) -> bool {
        match &self.token {
            None => true,
            Some(expect) => {
                bearer.map(str::trim) == Some(expect.as_str())
                    || query_token.map(str::trim) == Some(expect.as_str())
            }
        }
    }

    /// 注册（校验 → 容量闸 → upsert；已存在身份 = 原地更新，不受容量闸
    /// 拒绝——TTL 内的回头客永远进得来）。
    pub fn register(&self, m: RendezvousMember) -> Result<RendezvousMember, RegisterError> {
        let m = sanitize_member(m)?;
        let mut members = self.members.write();
        sweep_expired(&mut members);
        let exists = members.contains_key(&m.iroh_id);
        if !exists && members.len() >= MAX_MEMBERS {
            return Err(RegisterError::Full);
        }
        let mut stored = m.clone();
        stored.last_seen_ms = now_ms();
        members.insert(m.iroh_id.clone(), (stored, Instant::now()));
        Ok(m)
    }

    /// TTL 内成员（顺带清扫过期；按 node_id 排序——确定性，测试友好）。
    pub fn live_members(&self) -> Vec<RendezvousMember> {
        let mut members = self.members.write();
        sweep_expired(&mut members);
        let mut out: Vec<RendezvousMember> = members.values().map(|(m, _)| m.clone()).collect();
        out.sort_by(|a, b| a.node_id.cmp(&b.node_id).then(a.iroh_id.cmp(&b.iroh_id)));
        out
    }

    /// TTL 内成员数（测试消费；宿主状态面 M2 接线后走 status 汇总）。
    #[allow(dead_code)]
    pub fn live_count(&self) -> usize {
        let mut members = self.members.write();
        sweep_expired(&mut members);
        members.len()
    }
}

/// 过期剔除（调用方持写锁时顺带执行）。
fn sweep_expired(members: &mut HashMap<String, (RendezvousMember, Instant)>) {
    let now = Instant::now();
    members.retain(|_, (_, seen)| now.duration_since(*seen) < Duration::from_secs(MEMBER_TTL_SECS));
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// axum 薄壳
// ---------------------------------------------------------------------------

/// 运行中的注册表服务句柄（shutdown 幂等——join 槽取空即无操作）。
pub struct RegistryServerHandle {
    /// 实际监听地址（端口 0 = OS 分配时尤其有用）。
    pub addr: SocketAddr,
    registry: Arc<RendezvousRegistry>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl RegistryServerHandle {
    /// TTL 内成员（观测面：status / 名册快照）。
    pub fn live_members(&self) -> Vec<RendezvousMember> {
        self.registry.live_members()
    }

    /// TTL 内成员数（测试消费；宿主状态面 M2 接线后走 status 汇总）。
    #[allow(dead_code)]
    pub fn live_count(&self) -> usize {
        self.registry.live_count()
    }

    /// 优雅停机（幂等；join 超时 5s 兜底——停机卡死不拖垮调用方）。
    pub async fn shutdown(&mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(join) = self.join.take()
            && tokio::time::timeout(Duration::from_secs(5), join)
                .await
                .is_err()
        {
            crate::plugin_log(
                crate::roaming::ROAMING_LOG_WARN,
                "rendezvous",
                "注册表停机超时（best-effort 放弃等待）",
            );
        }
    }
}

/// 启动注册表 HTTP 服务（bind `0.0.0.0:<port>`；端口 0 = OS 分配，测试用）。
pub async fn spawn_registry_server(port: u16, token: &str) -> Result<RegistryServerHandle, String> {
    let registry = Arc::new(RendezvousRegistry::new(token));
    let app = registry_router(registry.clone());
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port)))
        .await
        .map_err(|e| format!("注册表端口 {port} 绑定失败: {e}"))?;
    let addr = listener
        .local_addr()
        .map_err(|e| format!("注册表监听地址探测失败: {e}"))?;
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let join = tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = shutdown_rx.changed().await;
        });
        if let Err(e) = server.await {
            crate::plugin_log(
                crate::roaming::ROAMING_LOG_WARN,
                "rendezvous",
                &format!("注册表服务退出（best-effort 记录）: {e}"),
            );
        }
    });
    crate::plugin_log(
        crate::roaming::ROAMING_LOG_INFO,
        "rendezvous",
        &format!("成员注册表已启动 addr={addr}"),
    );
    Ok(RegistryServerHandle {
        addr,
        registry,
        shutdown_tx,
        join: Some(join),
    })
}

/// 路由装配（独立函数——单测可直接构造共享 registry 断言存储行为）。
fn registry_router(registry: Arc<RendezvousRegistry>) -> axum::Router {
    use axum::routing::{get, post};
    axum::Router::new()
        .route(
            "/v1/roaming/register",
            post(register_handler).with_state(registry.clone()),
        )
        .route(
            "/v1/roaming/members",
            get(members_handler).with_state(registry),
        )
}

/// Bearer 头提取（`Authorization: Bearer <token>`；其他形态忽略）。
fn bearer_of(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

async fn register_handler(
    axum::extract::State(registry): axum::extract::State<Arc<RendezvousRegistry>>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
    axum::Json(member): axum::Json<RendezvousMember>,
) -> axum::response::Response {
    if !registry.token_ok(
        bearer_of(&headers).as_deref(),
        q.get("token").map(String::as_str),
    ) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    match registry.register(member) {
        Ok(m) => (axum::http::StatusCode::OK, axum::Json(m)).into_response(),
        Err(RegisterError::Invalid(msg)) => (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(json!({ "error": msg })),
        )
            .into_response(),
        Err(RegisterError::Full) => (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            axum::Json(json!({ "error": format!("名册已满（{MAX_MEMBERS}）") })),
        )
            .into_response(),
    }
}

async fn members_handler(
    axum::extract::State(registry): axum::extract::State<Arc<RendezvousRegistry>>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> axum::response::Response {
    if !registry.token_ok(
        bearer_of(&headers).as_deref(),
        q.get("token").map(String::as_str),
    ) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    let members = registry.live_members();
    (axum::Json(json!({ "members": members }))).into_response()
}

#[cfg(test)]
mod tests;
