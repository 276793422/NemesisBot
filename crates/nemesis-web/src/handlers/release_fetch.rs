//! GitHub Release 资产下载管线（skins / WASM 插件开发包 / selfupdate 自更新
//! 共用的单一来源）。
//!
//! 自 handlers/skins.rs 公共化抽出的下载链（模块不再整体门控——zip 解包等
//! 重依赖留在各消费方的 feature 闸内）：https-only + 每跳独立过 SSRF 闸
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
//! SSRF 槽本体随 `security` feature（security 关时无消费）。

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

/// 官方发布源 Release 的列表 API 址。per_page=100：发布列表同时是自更新
/// 降级闸的单调序列依据（GitHub created_at 降序），窗口尽量放宽——日更节奏
/// 100 条 ≈ 3 个月，窗口外条目降级判定诚实 Unknown（selfupdate 侧审计）。
const RELEASE_LIST_API: &str =
    "https://api.github.com/repos/276793422/NemesisBot/releases?per_page=100";

/// Release 单个附件（名 + 下载 URL）。
#[cfg_attr(
    not(any(feature = "skins", feature = "plugins-wasm", feature = "selfupdate")),
    allow(dead_code)
)]
#[derive(Debug, Clone)]
pub(crate) struct ReleaseAsset {
    pub name: String,
    pub url: String,
}

/// Release 元信息（L9 check / apply 同源解析：免下主包即可读 tag/名/body
/// commit/附件）。字段集即列表 API 单条的最小投影。
#[cfg_attr(
    not(any(feature = "skins", feature = "plugins-wasm", feature = "selfupdate")),
    allow(dead_code)
)]
#[derive(Debug, Clone)]
pub(crate) struct ReleaseMeta {
    pub tag_name: String,
    pub name: String,
    pub body: String,
    pub assets: Vec<ReleaseAsset>,
}

#[cfg_attr(
    not(any(feature = "skins", feature = "plugins-wasm", feature = "selfupdate")),
    allow(dead_code)
)]
impl ReleaseMeta {
    pub fn has_asset(&self, name: &str) -> bool {
        self.assets.iter().any(|a| a.name == name)
    }

    /// 指定附件的下载 URL（无 = None）。
    pub fn asset_url(&self, name: &str) -> Option<&str> {
        self.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.url.as_str())
    }

    /// body 表行 `` | Commit | `<sha>` | `` 里的全量 sha。
    pub fn body_commit(&self) -> Option<String> {
        crate::selfupdate::extract_commit_from_release_body(&self.body)
    }
}

/// 列表 API 响应解析（纯函数，单测钉住；顺序保留 = GitHub created_at 降序
/// 新→旧，自更新降级闸的单调序列依据）。
#[cfg_attr(
    not(any(feature = "skins", feature = "plugins-wasm", feature = "selfupdate")),
    allow(dead_code)
)]
pub(crate) fn parse_release_list(api: &serde_json::Value) -> Result<Vec<ReleaseMeta>, String> {
    let releases = api
        .as_array()
        .ok_or_else(|| "Release API 响应不是列表（仓库不存在或 API 限流）".to_string())?;
    Ok(releases
        .iter()
        .map(|r| ReleaseMeta {
            tag_name: r
                .get("tag_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            name: r
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            body: r
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            assets: r
                .get("assets")
                .and_then(|v| v.as_array())
                .map(|assets| {
                    assets
                        .iter()
                        .filter_map(|a| {
                            let name = a.get("name").and_then(|v| v.as_str())?;
                            let url = a.get("browser_download_url").and_then(|v| v.as_str())?;
                            Some(ReleaseAsset {
                                name: name.to_string(),
                                url: url.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect())
}

/// 拉取并解析发布列表（新→旧序）。
#[cfg_attr(
    not(any(feature = "skins", feature = "plugins-wasm", feature = "selfupdate")),
    allow(dead_code)
)]
#[cfg_attr(
    all(feature = "selfupdate", not(target_os = "windows")),
    allow(dead_code)
)]
pub(crate) async fn fetch_release_list() -> Result<Vec<ReleaseMeta>, String> {
    // 元信息很小，25MB 上限同口径防异常。
    let api_bytes = fetch_bytes(RELEASE_LIST_API, 25 * 1024 * 1024).await?;
    let api: serde_json::Value = serde_json::from_slice(&api_bytes)
        .map_err(|e| format!("Release API 响应不是合法 JSON：{e}"))?;
    parse_release_list(&api)
}

/// **同源解析**（纯函数）：列表中首个含 `asset_name` 附件的 Release。check
/// 展示的元信息与 apply 实际下载的包必须出自同一对象——修复旧实现「check
/// 读列表第一条、apply 下载首个含资产的」双路径分裂（两者可能指向不同
/// Release：最新条目尚未产出资产时，展示与安装会跨版本错位）。
#[cfg_attr(
    not(any(feature = "skins", feature = "plugins-wasm", feature = "selfupdate")),
    allow(dead_code)
)]
pub(crate) fn resolve_release_for_asset<'a>(
    list: &'a [ReleaseMeta],
    asset_name: &str,
) -> Option<&'a ReleaseMeta> {
    list.iter().find(|r| r.has_asset(asset_name))
}

/// 同源解析网络入口：列表 → 首个含 `asset_name` 的 Release。无任何 release
/// 含该资产 → 回落列表首条（`has_asset`=false，check 诚实报「发布在但包未
/// 产出」；apply 下载步对 URL 缺失给明确错误）。
#[cfg_attr(
    not(any(feature = "skins", feature = "plugins-wasm", feature = "selfupdate")),
    allow(dead_code)
)]
#[cfg_attr(
    all(feature = "selfupdate", not(target_os = "windows")),
    allow(dead_code)
)]
pub(crate) async fn fetch_release_for_asset(asset_name: &str) -> Result<ReleaseMeta, String> {
    let list = fetch_release_list().await?;
    if let Some(resolved) = resolve_release_for_asset(&list, asset_name) {
        return Ok(resolved.clone());
    }
    list.first()
        .cloned()
        .ok_or_else(|| "Release 列表为空（CI 尚未产出任何发布）".to_string())
}

/// 官方 Release 指定附件下载：同源解析 → SSRF 闸逐跳下载。找不到附件 =
/// 诚实报错（CI 尚未产出）。消费方全在 feature 门（skins / plugins-wasm /
/// selfupdate）内——裸构建无人调用，条件放行 dead_code；selfupdate 的
/// apply 主体又是 Windows-only（v1 平台边界），非 Windows+selfupdate 组合
/// 同样无人调用。
#[cfg_attr(
    not(any(feature = "skins", feature = "plugins-wasm", feature = "selfupdate")),
    allow(dead_code)
)]
#[cfg_attr(
    all(feature = "selfupdate", not(target_os = "windows")),
    allow(dead_code)
)]
pub(crate) async fn fetch_release_asset(
    asset_name: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    const NOT_FOUND_SUFFIX: &str = "附件（CI 尚未产出该包）";
    let meta = fetch_release_for_asset(asset_name).await?;
    let url = meta
        .asset_url(asset_name)
        .ok_or_else(|| format!("近期 Release 均未找到 {asset_name}{NOT_FOUND_SUFFIX}"))?;
    fetch_bytes(url, max_bytes).await
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
