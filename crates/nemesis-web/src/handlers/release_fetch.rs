//! GitHub Release 资产下载管线（skins 与 WASM 插件开发包共用的单一来源）。
//!
//! 自 handlers/skins.rs 公共化抽出的下载链（skins feature 关闭时
//! plugins-wasm 的 devkit 下载仍可用——此前管线整个锁在
//! `#![cfg(feature = "skins")]` 模块内）：https-only + 每跳独立过 SSRF 闸
//! （拦 = 诚实失败）+ 手动重定向循环（≤[`FETCH_MAX_HOPS`] 跳，相对
//! Location 走 `Url::join`；重定向降级到非 https 拒绝）+ 流式上限。
//!
//! 官方 Release 枚举走列表 API 而非 `releases/latest`：nightly-build 是
//! prerelease，latest 语义不认 prerelease（实测 HTTP 404），列表第一条才是
//! 真实的最新发布。
//!
//! SSRF 闸槽经 [`set_ssrf_guard`] 由 gateway 注入（原 skins 槽位迁移至此，
//! 顺序解耦语义不变：init_web 早于 init_agent 的安全插件构建；None = 直通）。
//!
//! 模块闸 = `any(skins, plugins-wasm)`（随消费方：两者全关不编译）。

// `RwLock` 仅 SSRF 闸槽使用（槽本体 cfg(security)，security 关时无消费）。
#[cfg(feature = "security")]
use parking_lot::RwLock;

/// 重定向跳数上限（GitHub release 资产 → S3 一跳即可，5 跳宽裕）。
const FETCH_MAX_HOPS: usize = 5;
const FETCH_UA: &str = "nemesisbot-release-fetch";
const FETCH_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const FETCH_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Release 附件默认读取上限：25MB（skins 包与 devkit 源码包同量级口径；
/// 调用方可用自有常量覆写——上限语义归属各子系统，本处只是下载器的参数）。
/// 当前唯一消费方 = devkit 下载（skins 传自有常量），闸随消费方。
#[cfg(feature = "plugins-wasm")]
pub(crate) const RELEASE_ASSET_MAX_BYTES: usize = 25 * 1024 * 1024;

/// 渠道下载 SSRF 闸槽：`SecurityPlugin::ssrf_guard()` 的克隆（Guard Clone
/// 共享 inner 状态）。gateway 在 init_agent 后注入；None = 直通
///（security feature 关 / security.enabled=false / ssrf 层关）。
#[cfg(feature = "security")]
static SSRF_GUARD_SLOT: RwLock<Option<nemesis_security::ssrf::Guard>> = RwLock::new(None);

/// gateway 装配点：注入 SSRF 闸（与装配顺序无关）。
#[cfg(feature = "security")]
pub fn set_ssrf_guard(guard: Option<nemesis_security::ssrf::Guard>) {
    *SSRF_GUARD_SLOT.write() = guard;
}

/// 不跟随重定向的下载 client（Policy::none：SSRF 闸只校验发出去的那一跳，
/// 自动跟跳 = `302 → 内网` 绕闸——image_attach A1 同判例，循环在调用方）。
fn fetch_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(FETCH_CONNECT_TIMEOUT)
        .timeout(FETCH_TOTAL_TIMEOUT)
        .user_agent(FETCH_UA)
        .build()
        .unwrap_or_default()
}

/// 按 SSRF 闸验证过的 IP 集钉死 DNS 的下载 client（防 rebinding TOCTOU——
/// 闸解析过 ≠ reqwest 连接用的 IP；TLS SNI/证书校验仍按原 host）。
#[cfg(feature = "security")]
fn pinned_fetch_client(url: &str, ips: &[std::net::IpAddr]) -> Option<reqwest::Client> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_string();
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(FETCH_CONNECT_TIMEOUT)
        .timeout(FETCH_TOTAL_TIMEOUT)
        .user_agent(FETCH_UA);
    for ip in ips {
        builder = builder.resolve(&host, std::net::SocketAddr::new(*ip, port));
    }
    builder.build().ok()
}

/// 渠道下载：https-only + 每跳独立过 SSRF 闸（拦 = 诚实失败）+ 手动重定向
/// 循环（≤[`FETCH_MAX_HOPS`] 跳；重定向降级到非 https 拒绝）+ 流式
/// `max_bytes` 上限。
pub(crate) async fn fetch_bytes(url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err("仅支持 https:// 地址".to_string());
    }
    #[cfg(feature = "security")]
    let guard = SSRF_GUARD_SLOT.read().clone();
    let base = fetch_client();
    let mut current = reqwest::Url::parse(url).map_err(|e| format!("URL 解析失败：{e}"))?;
    for _hop in 0..=FETCH_MAX_HOPS {
        #[cfg(feature = "security")]
        let client = match &guard {
            Some(g) => match g.resolve_and_validate_collect(current.as_str()) {
                // Ok(空集) = 闸放行但未给出可钉 IP（allowlist 域名等）→ 共享池。
                Ok(ips) if ips.is_empty() => base.clone(),
                Ok(ips) => {
                    pinned_fetch_client(current.as_str(), &ips).unwrap_or_else(|| base.clone())
                }
                Err(e) => return Err(format!("SSRF 闸拦截：{e}")),
            },
            None => base.clone(),
        };
        #[cfg(not(feature = "security"))]
        let client = &base;

        let resp = client
            .get(current.clone())
            .send()
            .await
            .map_err(|e| format!("下载失败：{e}"))?;
        let status = resp.status();
        if status.is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| format!("重定向 {status} 缺少 Location 头"))?;
            let next = current
                .join(loc)
                .map_err(|e| format!("Location 解析失败：{e}"))?;
            if next.scheme() != "https" {
                return Err("重定向降级到非 https，已拒绝".to_string());
            }
            current = next;
            continue;
        }
        if !status.is_success() {
            return Err(format!("下载失败：HTTP {status}"));
        }
        return read_capped(resp, max_bytes).await;
    }
    Err(format!("重定向超过 {FETCH_MAX_HOPS} 跳，已放弃"))
}

/// 流式读响应体（Content-Length 预检 + chunk 累计双闸）。
async fn read_capped(resp: reqwest::Response, max_bytes: usize) -> Result<Vec<u8>, String> {
    if let Some(len) = resp.content_length()
        && len as usize > max_bytes
    {
        return Err(format!(
            "包体 {len} 字节超过 {}MB 上限，已拒收",
            max_bytes / (1024 * 1024)
        ));
    }
    use futures::StreamExt as _;
    let mut out = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载中断：{e}"))?;
        if out.len() + chunk.len() > max_bytes {
            return Err(format!(
                "包体超过 {}MB 上限，下载已中止",
                max_bytes / (1024 * 1024)
            ));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// 官方发布源 Release 的列表 API 址。
const RELEASE_LIST_API: &str =
    "https://api.github.com/repos/276793422/NemesisBot_Rust/releases?per_page=10";

/// 官方 Release 指定附件下载：列表 API → 首个含 `asset_name` 附件的
/// Release → SSRF 闸逐跳下载。找不到附件 = 诚实报错（CI 尚未产出）。
pub(crate) async fn fetch_release_asset(
    asset_name: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let api_bytes = fetch_bytes(RELEASE_LIST_API, max_bytes).await?;
    let api: serde_json::Value = serde_json::from_slice(&api_bytes)
        .map_err(|e| format!("Release API 响应不是合法 JSON：{e}"))?;
    let releases = api
        .as_array()
        .ok_or_else(|| "Release API 响应不是列表（仓库不存在或 API 限流）".to_string())?;
    let url = releases
        .iter()
        .filter_map(|r| r.get("assets").and_then(|v| v.as_array()))
        .find_map(|assets| {
            assets.iter().find_map(|a| {
                let name = a.get("name").and_then(|v| v.as_str())?;
                (name == asset_name)
                    .then(|| a.get("browser_download_url").and_then(|v| v.as_str()))
                    .flatten()
            })
        })
        .ok_or_else(|| format!("近期 Release 均未找到 {asset_name} 附件（CI 尚未产出该包）"))?
        .to_string();
    fetch_bytes(&url, max_bytes).await
}

/// 带退避重试的目录树删除（nemesis-cluster::fsutil 同款模式——Windows 上
/// Defender 实时扫描/索引服务会短暂持有句柄，单发 `remove_dir_all` 在
/// CI runner 上实证不成立；nemesis-web 不依赖 nemesis-cluster，本地同型
/// 实现。尽力而为语义：返回是否成功，不向上传播错误）。当前唯一消费方
/// = devkit 覆盖重下的旧目录清除，闸随消费方。
#[cfg(feature = "plugins-wasm")]
pub(crate) fn remove_dir_all_resilient(dir: &std::path::Path) -> bool {
    let mut last_err = None;
    for attempt in 0..10u32 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(e) => {
                tracing::debug!(
                    dir = %dir.display(),
                    attempt,
                    error = %e,
                    "[ReleaseFetch] 目录树删除失败，退避重试"
                );
                last_err = Some(e);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(80 * (attempt as u64 + 1)));
    }
    tracing::warn!(
        dir = %dir.display(),
        last_error = ?last_err,
        "[ReleaseFetch] 目录树删除重试耗尽"
    );
    false
}
