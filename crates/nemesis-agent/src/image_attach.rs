//! 统一图片附加（goal T5，真相源 §2 P2.2 + P2.3）。
//!
//! 轮次摄取时两条来源统一处理：
//! 1. `InboundMessage.media` 引用（Telegram 照片 / Web 上传落盘路径；URL 场景 T9 接入）
//! 2. T4 文本提取路径（主场景：用户在提示词里写本地图片路径）
//!
//! **附加 = 一次程序化 file_read 走安全管线**：构造 `read_file` invocation 交
//! `SecurityPlugin::execute`（8 层全跑：注入 → ABAC file_rules → 凭据 → DLP →
//! SSRF → 病毒扫描 → 审计链），允许后读文件算 sha256 并向审计链追加一条
//! path+hash 事件。工作区边界语义：聊天用户点名路径 = 主体意志，附加**不走**
//! 文件工具的 restrict_to_workspace 校验（该开关继续管 agent 自主工具调用，
//! 三态见 tests.rs）；收紧走 file_rules（管线 Layer 3 照常拦截）。
//!
//! 字节水合不在附加时做：附加只产出**路径引用**（历史持久化同 T6 存引用），
//! build_messages 每轮按引用重读文件 → base64（[`hydrate_image_refs`]）。
//! 文件已删/不可读 → 降级占位文本，不静默。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::image_path_detector::{self, CandidateStatus, ImagePathCandidate};

/// 水合后的单张图片（build_messages 产出；ProviderAdapter 转 provider
/// ContentPart：文本 part → `[图片: <path>]` 标注 part → 图片 part）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LlmImage {
    /// 模型可见的标注路径（`[图片: <path>]`）。
    pub path: String,
    /// MIME 类型（image/png 等，由扩展名映射）。
    pub media_type: String,
    /// base64 编码的文件字节。
    pub data: String,
}

/// 成功附加的图片（路径引用形态；base64 水合在 build_messages）。
#[derive(Debug, Clone, PartialEq)]
pub struct AttachedImage {
    /// 用户原始字面量 / media 引用原文。
    pub raw: String,
    /// 解析后的绝对路径（水合按此读取；历史持久化同款字符串）。
    pub resolved: PathBuf,
}

/// 轮次摄取统一附加结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AttachOutcome {
    /// 成功附加（去重后；顺序 = 文本出现序 + media 追加序）。
    pub attached: Vec<AttachedImage>,
    /// 失败注记（诚实注明原因；空 = 全部成功）。
    pub notes: Vec<String>,
}

impl AttachOutcome {
    /// 把失败注记合并进轮次文本（诚实注明；全成功时原样返回）。
    pub fn merge_into_text(&self, mut text: String) -> String {
        if self.notes.is_empty() {
            return text;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for note in &self.notes {
            text.push_str(note);
            text.push('\n');
        }
        // 去掉末尾多余换行（逐 note 追加产生的最后一个）。
        while text.ends_with('\n') {
            text.pop();
        }
        text
    }

    /// 附加路径引用（进 ConversationTurn.image_refs / StoredMessage.image_refs）。
    pub fn ref_strings(&self) -> Vec<String> {
        self.attached
            .iter()
            .map(|a| a.resolved.to_string_lossy().into_owned())
            .collect()
    }
}

/// 从候选状态提取诚实注记（复用 T4 的 failure_reason 语义）。
fn candidate_note(c: &ImagePathCandidate) -> Option<String> {
    c.failure_reason()
        .map(|reason| format!("[图片未附加: {}]", reason))
}

/// 单路径安全闸 + hash 审计（附加 = 一次程序化 file_read 走管线）。
///
/// - `security`：生产传 SecurityPlugin（feature=security）；None = 管线未挂
///   （security.enabled=false 或 feature 裁剪），闸门直通（与工具调用同语义）。
/// - 允许 → 读文件算 sha256 并向审计链追加 path+hash 事件，返回 Ok(hash)。
/// - 拒绝 → Err(原因)（调用方诚实注明）。
#[cfg(feature = "security")]
fn gate_and_hash(
    security: Option<&nemesis_security::pipeline::SecurityPlugin>,
    path: &Path,
    channel: &str,
) -> Result<String, String> {
    let path_str = path.to_string_lossy().into_owned();
    if let Some(sec) = security {
        let invocation = nemesis_security::types::ToolInvocation {
            tool_name: "read_file".to_string(),
            args: serde_json::json!({ "path": path_str }),
            user: String::new(),
            source: channel.to_string(),
            metadata: std::collections::HashMap::new(),
        };
        let (allowed, reason) = sec.execute(&invocation);
        if !allowed {
            return Err(reason.unwrap_or_else(|| "operation denied by security policy".to_string()));
        }
    }
    // 读文件 + sha256（审计记 hash 不记像素；P4.2 盲区边界同款语义）。
    let bytes = std::fs::read(path).map_err(|e| format!("文件无法读取: {} ({})", path_str, e))?;
    let hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        format!("{:x}", hasher.finalize())
    };
    if let Some(sec) = security
        && let Some(chain) = sec.audit_chain()
    {
        let _ = chain.append(
            "image_attach",
            "read_file",
            "",
            channel,
            &path_str,
            "allowed",
            &format!("media attached; sha256={}", hash),
        );
    }
    Ok(hash)
}

/// 非 security 构建的同形闸门（管线整体裁剪 = 闸门直通，仍算 hash 保行为一致）。
#[cfg(not(feature = "security"))]
fn gate_and_hash(_security: Option<()>, path: &Path, _channel: &str) -> Result<String, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("文件无法读取: {} ({})", path.display(), e))?;
    let _ = bytes; // 无审计链可记，hash 仅用于统一失败面
    Ok(String::new())
}

/// 不跟随重定向的下载 client（2026-09-03 二次回归 A1）：共享池默认策略跟随
/// 至多 10 跳重定向——SSRF 闸只校验**首跳** URL，`302 → 内网地址` 会绕过闸
/// 直接打内网。图片下载一律用本 client：重定向响应（3xx）走 error_for_status
/// → 诚实注明失败，不静默放行内网。
fn no_redirect_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("nemesisbot-image-fetch")
            .build()
            .unwrap_or_default()
    })
}

/// 按 SSRF 闸验证过的 IP 集**钉死 DNS** 的下载 client（2026-09-04 四轮盲审
/// S2）：闸解析验证过 ≠ reqwest 实际连接用的 IP——两次独立解析之间，rebinding
/// 域名（攻击者控 DNS、TTL 0）可先答公网 IP 过闸、再答内网/元数据 IP 收连接。
/// `resolve(host, addr)` 把该 host 的全部连接钉在已验证 IP 上，reqwest 不再
/// 发起第二次解析，TOCTOU 关闭。TLS SNI/证书校验仍按原 host（仅钉解析）。
///
/// 构建失败返回 None，调用方回退 [`no_redirect_client`]（Policy::none 语义
/// 不丢；**绝不**回退到 reqwest 默认 client——默认会跟随重定向，A1 就回来了）。
// 唯一调用点在 security 布防分支（S2 钉死路径）；feature 裁掉 security 时
// 本函数随之裁掉，不留 dead-code 警告。
#[cfg(feature = "security")]
fn pinned_no_redirect_client(url: &str, ips: &[std::net::IpAddr]) -> Option<reqwest::Client> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_string();
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("nemesisbot-image-fetch");
    for ip in ips {
        builder = builder.resolve(&host, std::net::SocketAddr::new(*ip, port));
    }
    builder.build().ok()
}

/// T9（多模态 goal 2026-09-03）：URL media 预取——把 http(s) 引用改为本地
/// 路径引用，随后统一走同步附加链（验真/安全闸/hash 审计/去重/历史持久化
/// 全复用，URL 项不新增旁路）。
///
/// 每条 URL 依次过：
/// 1. **SSRF 闸**（安全管线 Layer 6 同一真相源 `ssrf::Guard`——DNS 解析后
///    校验回环/内网/链路本地/元数据端点；guard=None（feature 裁剪 / 插件未
///    挂 / ssrf 层配置关闭）直通，与 [`gate_and_hash`] 的三态语义一致）；
///    2026-09-04 四轮盲审 S2：验证过的 IP 集会**钉死**到下载 client（防
///    rebinding 域名在闸解析与实际连接之间换答案）；
/// 2. **下载**（不跟随重定向的专用 client：30s 请求超时 + UA；S1：流式
///    读取 + 25MB 硬上限，不再整包进内存）；
/// 3. **验真**：非空、magic byte（与落盘图片同一张签名表）。
///
/// 通过 → 落 `uploads_dir/url_{hash8}_{millis}.{ext}` 并改写为路径引用；
/// 失败 → 诚实注明 `[图片未附加: ...]` 并从 media 中移除（调用方合并注记）。
/// 同批同 URL 只拉一次（复用同一落盘路径，附加层照常跨来源去重）。
pub async fn fetch_url_media(
    media: &[nemesis_types::channel::MediaAttachment],
    uploads_dir: &Path,
    #[cfg(feature = "security")] ssrf_guard: Option<&nemesis_security::ssrf::Guard>,
    #[cfg(not(feature = "security"))] ssrf_guard: Option<()>,
) -> (Vec<nemesis_types::channel::MediaAttachment>, Vec<String>) {
    use nemesis_types::channel::MediaAttachment;

    let _ = ssrf_guard;
    let mut kept = Vec::new();
    let mut notes = Vec::new();
    // 同批 URL 去重：同 URL 只拉一次，复用同一落盘路径。
    let mut fetched: std::collections::HashMap<String, PathBuf> = std::collections::HashMap::new();
    let mut dir_ready = false;

    // F-K（2026-09-04 四轮盲审）：每消息张数上限必须在**下载前**生效——
    // 先全量下载落盘再被 D6 丢弃 = 磁盘/带宽白烧（API 调用方可发任意长的
    // media 数组，前端 8 张上限只管 UI）。溢出聚合一条诚实注记（与 D6
    // 先到先得同语义）。
    let mut media: Vec<MediaAttachment> = media.to_vec();
    if media.len() > image_path_detector::MAX_IMAGES_PER_MESSAGE {
        let max = image_path_detector::MAX_IMAGES_PER_MESSAGE;
        let dropped = media.len() - max;
        media.truncate(max);
        notes.push(format!(
            "[图片未附加: 媒体引用数量超过每消息上限 {max}，仅处理前 {max} 条（弃 {dropped} 条）]"
        ));
    }

    for m in &media {
        let url = m.url.trim();
        // L4（2026-09-04 四轮盲审）：内联 base64（MediaAttachment.data）与
        // data: URI 不在设计面内（media 形态 = {id}|{path}|http(s) URL），
        // 诚实注明而非静默丢/当路径误报文件不存在。
        if url.is_empty() || url.starts_with("data:") {
            if m.data.is_some() || url.starts_with("data:") {
                notes.push(
                    "[图片未附加: 暂不支持内联 data 图片引用（请改用文件路径、上传或 URL）]"
                        .to_string(),
                );
            }
            continue;
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            // 本地路径引用（T7/T8 落盘产物 / 用户点名路径）原样保留。
            kept.push(m.clone());
            continue;
        }
        if let Some(path) = fetched.get(url) {
            kept.push(MediaAttachment {
                media_type: "image".to_string(),
                url: path.to_string_lossy().into_owned(),
                data: None,
            });
            continue;
        }

        // 1) SSRF 闸（Layer 6 同源 Guard；None = 直通，同管线三态语义）。
        //    S2（2026-09-04 四轮盲审）：验证过的 IP 取回并钉死到下载
        //    client（防 DNS rebinding TOCTOU——见 pinned_no_redirect_client
        //    文档）。Ok(空)=闸关/白名单直通（共享 client 正常解析）。
        #[cfg(feature = "security")]
        let pinned_client: Option<reqwest::Client> = if let Some(guard) = ssrf_guard {
            match guard.resolve_and_validate_collect(url) {
                Ok(ips) if ips.is_empty() => None,
                Ok(ips) => pinned_no_redirect_client(url, &ips),
                Err(e) => {
                    notes.push(format!("[图片未附加: SSRF 拦截 {} ({})]", url, e));
                    continue;
                }
            }
        } else {
            None
        };
        #[cfg(not(feature = "security"))]
        let pinned_client: Option<reqwest::Client> = None;
        let client = match &pinned_client {
            Some(c) => c,
            None => no_redirect_client(),
        };

        // 2) 下载（不跟随重定向的专用 client：30s 超时兜底 + SSRF 首跳语义
        // 不被 302 绕过，见 no_redirect_client 文档）。
        let resp = match client.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                notes.push(format!("[图片未附加: URL 拉取失败 {} ({})]", url, e));
                continue;
            }
        };
        let mut resp = match resp.error_for_status() {
            Ok(r) => r,
            Err(e) => {
                notes.push(format!("[图片未附加: URL 拉取失败 {} ({})]", url, e));
                continue;
            }
        };
        // A1（2026-09-03 二次回归）：重定向不跟随——client 已设 Policy::none()
        //（SSRF 闸只校验首跳 URL，302 → 内网地址会绕过闸直打内网；且
        // error_for_status 只拒 4xx/5xx，3xx 会漏过）。重定向响应在此显式
        // 诚实注明失败，不读其 body、不放行。
        if resp.status().is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("?");
            notes.push(format!(
                "[图片未附加: URL 重定向未跟随（防 SSRF 绕过）{} → {}]",
                url, location
            ));
            continue;
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        // S1（2026-09-04 四轮盲审）：流式读取 + 硬上限——旧实现 `resp.bytes()`
        // 先把整个 body 读进内存再看大小，远程可借大 body（30s 窗口内 GB 级）
        // 撑爆网关内存。现在：Content-Length 预检（超限未下载即拒）→ 分块
        // 累计（累计超限立即中止），内存占用恒 ≤ 25MB + 单块。
        if let Some(len) = resp
            .content_length()
            .filter(|&len| len > image_path_detector::MAX_IMAGE_BYTES)
        {
            notes.push(format!(
                "[图片未附加: 图片超过 25MB 上限 (Content-Length {len} 字节): {}]",
                url
            ));
            continue;
        }
        let mut bytes: Vec<u8> = Vec::new();
        let mut read_err: Option<reqwest::Error> = None;
        let mut overflow = false;
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    if bytes.len() as u64 + chunk.len() as u64
                        > image_path_detector::MAX_IMAGE_BYTES
                    {
                        overflow = true;
                        break;
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(e) => {
                    read_err = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = read_err {
            notes.push(format!("[图片未附加: URL 响应读取失败 {} ({})]", url, e));
            continue;
        }
        if overflow {
            notes.push(format!(
                "[图片未附加: 图片超过 25MB 上限（流式读取中止）: {}]",
                url
            ));
            continue;
        }

        // 3) 验真：非空 + magic（与落盘图片同一张签名表）。
        //    （大小上限已由上方流式读取保证恒 ≤ 25MB，不再重复检查。）
        if bytes.is_empty() {
            notes.push(format!("[图片未附加: URL 响应为空 {}]", url));
            continue;
        }
        if !image_path_detector::sniff_magic(&bytes) {
            notes.push(format!("[图片未附加: URL 内容不是图片 {}]", url));
            continue;
        }

        // 4) 落盘：url_{hash8(url)}_{millis}.{ext}（hash8 防碰撞可读前缀）。
        // 扩展名（2026-09-03 二次回归 C3）：magic 已验真 → 优先按内容签名表
        // 定扩展名（Content-Type 可伪造/缺失、URL 后缀可说谎）；URL 后缀只作
        // 兜底可读性（理论上 magic 过了而签名表没匹配上的形态不存在，
        // ext_from_magic 与 sniff_magic 同表——防御式兜底）。
        let ext = image_path_detector::ext_from_magic(&bytes)
            .map(str::to_string)
            .unwrap_or_else(|| pick_url_ext(url, &content_type));
        let hash8: String = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(url.as_bytes());
            format!("{:x}", h.finalize())[..8].to_string()
        };
        let dest = uploads_dir.join(format!(
            "url_{}_{}.{}",
            hash8,
            chrono::Utc::now().timestamp_millis(),
            ext
        ));
        if !dir_ready {
            if let Err(e) = std::fs::create_dir_all(uploads_dir) {
                notes.push(format!(
                    "[图片未附加: 创建 uploads 目录失败: {} ({})]",
                    e, url
                ));
                continue;
            }
            dir_ready = true;
        }
        if let Err(e) = std::fs::write(&dest, &bytes) {
            notes.push(format!("[图片未附加: 落盘失败 {} ({})]", url, e));
            continue;
        }
        tracing::info!(
            url = %url,
            file = %dest.display(),
            "[ImageAttach] URL image fetched"
        );

        let path = dest;
        fetched.insert(url.to_string(), path.clone());
        kept.push(MediaAttachment {
            media_type: "image".to_string(),
            url: path.to_string_lossy().into_owned(),
            data: None,
        });
    }

    (kept, notes)
}

/// URL 图片的扩展名推断：URL 路径扩展名（白名单内优先）→ Content-Type 映射
/// → 兜底 jpg（magic 嗅探已在前，扩展名只影响落盘可读性）。
fn pick_url_ext(url: &str, content_type: &str) -> String {
    let whitelist = ["png", "jpg", "jpeg", "webp", "gif"];
    if let Some(name) = url.split(['/', '?']).rev().find(|s| s.contains('.')) {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if whitelist.contains(&ext.as_str()) {
            return ext;
        }
    }
    for (ct, ext) in [
        ("image/png", "png"),
        ("image/jpeg", "jpg"),
        ("image/webp", "webp"),
        ("image/gif", "gif"),
    ] {
        if content_type.contains(ct) {
            return ext.to_string();
        }
    }
    "jpg".to_string()
}

/// 轮次摄取统一附加入口（loop.rs process_admitted 调用）。
///
/// 来源 1：文本提取路径（T4 检测器，验真已含存在性/大小/magic/去重）。
/// 来源 2：`media` 引用——本地路径（T7 Telegram / T8 Web 上传 / T9 URL 预取
///         落盘产物；URL 须先经 [`fetch_url_media`] 预取，本同步入口不拉
///         网络，直接收到 URL 属调用方误用，诚实注明）。
/// 两来源统一过安全闸 + 跨来源去重（解析路径大小写不敏感）。
/// 张数上限（D6）：两来源合计 ≤[`image_path_detector::MAX_IMAGES_PER_MESSAGE`]，
/// 超出部分不附加、聚合成一条诚实注记（先到先得：文本出现序 → media 追加序）。
pub fn attach_turn_images(
    text: &str,
    media: &[nemesis_types::channel::MediaAttachment],
    workspace_dir: Option<&Path>,
    channel: &str,
    #[cfg(feature = "security")] security: Option<&nemesis_security::pipeline::SecurityPlugin>,
    #[cfg(not(feature = "security"))] security: Option<()>,
) -> AttachOutcome {
    let mut outcome = AttachOutcome::default();
    let mut seen: HashSet<String> = HashSet::new();
    let mut overflow: Vec<String> = Vec::new();

    // 来源 1：文本提取路径（T4）。非 Ok 候选按 failure_reason 诚实注明。
    for candidate in image_path_detector::detect_image_paths(text, workspace_dir) {
        if !candidate.is_attachable() {
            if let Some(note) = candidate_note(&candidate) {
                outcome.notes.push(note);
            }
            continue;
        }
        let key = image_path_detector::dedup_key(&candidate.resolved);
        if !seen.insert(key) {
            continue;
        }
        if outcome.attached.len() >= image_path_detector::MAX_IMAGES_PER_MESSAGE {
            overflow.push(candidate.raw);
            continue;
        }
        match gate_and_hash(security, &candidate.resolved, channel) {
            Ok(_) => outcome.attached.push(AttachedImage {
                raw: candidate.raw,
                resolved: candidate.resolved,
            }),
            Err(reason) => outcome.notes.push(format!("[图片未附加: {}]", reason)),
        }
    }

    // 来源 2：media 引用。
    for m in media {
        let url = m.url.trim();
        if url.is_empty() {
            continue;
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            // 防御分支：生产路径（loop.rs）已先经 fetch_url_media 预取，
            // 这里收到 URL 属调用方误用（本同步入口不拉网络），诚实注明。
            outcome.notes.push(format!(
                "[图片未附加: URL 引用未经 fetch_url_media 预取 ({})]",
                url
            ));
            continue;
        }
        // 本地路径引用（T7/T8 落盘产物）：与文本候选同一验真 + 闸门链。
        let path = PathBuf::from(url);
        match image_path_detector::verify(&path) {
            CandidateStatus::Ok { .. } => {
                let key = image_path_detector::dedup_key(&path);
                if !seen.insert(key) {
                    continue;
                }
                if outcome.attached.len() >= image_path_detector::MAX_IMAGES_PER_MESSAGE {
                    overflow.push(url.to_string());
                    continue;
                }
                match gate_and_hash(security, &path, channel) {
                    Ok(_) => outcome.attached.push(AttachedImage {
                        raw: url.to_string(),
                        resolved: path,
                    }),
                    Err(reason) => outcome.notes.push(format!("[图片未附加: {}]", reason)),
                }
            }
            other => {
                let reason = match other {
                    CandidateStatus::NotFound => "文件不存在".to_string(),
                    CandidateStatus::TooLarge { size } => {
                        format!("图片超过 25MB 上限 ({} 字节)", size)
                    }
                    CandidateStatus::NotImage { magic_hex } => {
                        format!("文件内容不是图片（文件头 {}）", magic_hex)
                    }
                    _ => "文件无法读取".to_string(),
                };
                outcome
                    .notes
                    .push(format!("[图片未附加: {}: {}]", reason, url));
            }
        }
    }

    // D6 张数上限溢出：聚合成一条诚实注记（列表 = 被忽略的原文，按忽略序）。
    if !overflow.is_empty() {
        outcome.notes.push(format!(
            "[图片未附加: 超过每消息 {} 张上限，已忽略 {} 张: {}]",
            image_path_detector::MAX_IMAGES_PER_MESSAGE,
            overflow.len(),
            overflow.join(", ")
        ));
    }

    outcome
}

/// 水合：路径引用 → base64 图片（build_messages 每轮重建时调用，T6 语义）。
///
/// 每个引用重验真（存在/≤25MB/magic）后读取 → base64；失败引用降级为占位
/// 文本行 `[图片已失效: <path>]`（调用方追加进消息文本），不静默、不炸。
/// 返回 (成功水合的图片, 失效占位行)。
pub fn hydrate_image_refs(refs: &[String]) -> (Vec<LlmImage>, Vec<String>) {
    let mut images = Vec::new();
    let mut placeholders = Vec::new();
    for raw in refs {
        let path = PathBuf::from(raw);
        let fail = || format!("[图片已失效: {}]", raw);
        match image_path_detector::verify(&path) {
            CandidateStatus::Ok { .. } => {
                let Some(media_type) = image_path_detector::media_type_for_path(&path) else {
                    placeholders.push(fail());
                    continue;
                };
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        use base64::Engine as _;
                        images.push(LlmImage {
                            path: raw.clone(),
                            media_type: media_type.to_string(),
                            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                        });
                    }
                    Err(_) => placeholders.push(fail()),
                }
            }
            _ => placeholders.push(fail()),
        }
    }
    (images, placeholders)
}

#[cfg(test)]
mod tests;
