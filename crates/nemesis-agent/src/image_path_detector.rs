//! 图片路径检测器（goal T4，真相源 §2 P2.1）。
//!
//! 从用户消息文本中提取候选图片路径并验真：Windows 绝对（`C:\...`）/
//! POSIX 绝对（`/...`）/ UNC（`\\server\share\a.png`）/ 工作区相对。
//! 含空格的路径必须用引号包裹（`"..."` / `「...」`，shell 语义——空白是
//! 行文与路径的边界，理由见 [`path_regex`] 文档）。
//! 扩展名白名单 png/jpg/jpeg/webp/gif；验真 = 存在性 + 单图 ≤25MB（D6）
//! + magic byte 嗅探（防文本伪装 .png）+ 同消息去重。
//!
//! 误报防线（两层）：
//! 1. **裸文件名（无任何路径分隔符/盘符/UNC 前缀）不是候选**——
//!    「png 是一种图片格式」这类普通文本不会被误判。
//! 2. **相对候选不存在时静默丢弃**——相对路径（`sub/a.png`、`./a.png`）
//!    可能只是行文中的比喻（"and/or.png"），不存在 = 大概率不是真路径，
//!    不产噪；**绝对/UNC 候选不存在则诚实注明**（用户明确点名了路径）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 单图大小上限（D6：25MB；真实上限在 provider 侧，bot 侧不过度设卡）。
pub const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;

/// 每消息图片张数上限（D6：8 张，2026-09-03 用户定值；两来源合计、去重后计）。
/// 超出部分不附加，由附加层聚合成一条诚实注记（[`super::attach_turn_images`]）。
pub const MAX_IMAGES_PER_MESSAGE: usize = 8;

/// 扩展名白名单（小写比较）。
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];

/// 候选验真结果。
#[derive(Debug, Clone, PartialEq)]
pub enum CandidateStatus {
    /// 通过验真（大小已记录）。
    Ok { size: u64 },
    /// 文件不存在。
    NotFound,
    /// 超过单图上限。
    TooLarge { size: u64 },
    /// magic byte 不符（扩展名伪装；读到的前 4 字节十六进制便于诊断）。
    NotImage { magic_hex: String },
    /// 元数据读取失败（权限等 IO 错误）。
    Unreadable,
}

/// 一个候选路径（检测 + 验真结果）。
#[derive(Debug, Clone, PartialEq)]
pub struct ImagePathCandidate {
    /// 文本中的原始字面量（未做任何归一）。
    pub raw: String,
    /// 解析后的路径（绝对候选原样；相对候选基于 workspace_dir；无基准时保留原样）。
    pub resolved: PathBuf,
    /// 是否"明确点名"路径（绝对/UNC = 用户明确指向某文件；相对 = 可能是行文）。
    pub deliberate: bool,
    pub status: CandidateStatus,
}

impl ImagePathCandidate {
    /// 是否附加成功（T5 统一 attach 流程的准入判断）。
    pub fn is_attachable(&self) -> bool {
        matches!(self.status, CandidateStatus::Ok { .. })
    }

    /// 诚实失败原因（T5 文本注明用；Ok / 静默类返回 None）。
    pub fn failure_reason(&self) -> Option<String> {
        match &self.status {
            CandidateStatus::Ok { .. } => None,
            CandidateStatus::NotFound if self.deliberate => {
                Some(format!("文件不存在: {}", self.raw))
            }
            CandidateStatus::NotFound => None, // 相对候选不存在 = 行文，静默
            CandidateStatus::TooLarge { size } => {
                Some(format!("图片超过 25MB 上限 ({} 字节): {}", size, self.raw))
            }
            CandidateStatus::NotImage { magic_hex } => Some(format!(
                "文件内容不是图片（扩展名伪装，文件头 {}）: {}",
                magic_hex, self.raw
            )),
            CandidateStatus::Unreadable => Some(format!("文件无法读取: {}", self.raw)),
        }
    }
}

/// 候选提取正则（OnceLock 缓存）。
///
/// 两形态家族（2026-09-04 三份 CI 日志 28 处红的根修，shell 语义对齐）：
/// - **无引号形态**：段字符类**不含任何空白**——空白是行文与路径的天然
///   边界。此前类含空格 + POSIX/相对分支无左锚点，`/a.png 和 /b.png` 被
///   贪心连成**一个**候选（`find_iter` 只取这一个重叠匹配，真路径全丢）。
///   Windows 靠盘符 `:` 不在类内天然截断所以本地测不出；Linux CI 上是
///   生产 bug：同句两图粘连成假 NotFound 候选。根修 = 空白出类。
/// - **引号形态**（`"..."` / `「...」`）：路径含空格必须包裹（与 shell
///   一致）；引号不在段类内，贪心既逃不出也进不来，引号内段可含空格。
///   匹配后剥离成对包裹字符再解析（[`strip_path_quotes`]）。
///
/// 段字符类其余部分：字母数字/下划线/`~`（**8.3 短名** `RUNNER~1`——CI
/// runner 与开启短名生成的机器上 `std::env::temp_dir()` 真实返回这种路径，
/// `~` 曾不在类内导致文本点名路径全部失明）/CJK/谚文/拉丁扩展/点/括号/
/// 连字符；不含分隔符与 Windows 文件名非法字符（`:*?"<>|`）。
/// 扩展名要求前置点号 + 白名单词（防 "C:\\tempng" 之类吞段误报）。
fn path_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let seg_ns =
            r#"[\w~\u{4e00}-\u{9fff}\u{3400}-\u{4dbf}\u{ac00}-\u{d7af}\u{00c0}-\u{024f}.()\-]"#;
        // 引号内形态：允许空格（引号是边界，贪心跨不出去）。
        let seg_q =
            r#"[\w~\u{4e00}-\u{9fff}\u{3400}-\u{4dbf}\u{ac00}-\u{d7af}\u{00c0}-\u{024f}.()\- ]"#;
        let build = |seg: &str| -> (String, String, String, String) {
            let name = format!("(?:{seg}+)"); // 末段文件名（可含点）
            let ext = r#"\.(?i:png|jpe?g|webp|gif)"#;
            // Windows 盘符绝对：C:\a\b.png（\ / 混用均收）
            let win = format!(r#"[A-Za-z]:[\\/](?:{name}[\\/])*{name}{ext}"#);
            // UNC：\\server\share\a.png
            let unc = format!(r#"\\\\{name}[\\/](?:{name}[\\/])*{name}{ext}"#);
            // POSIX 绝对：/a/b.png（仅 / 分隔）
            let nix_abs = format!(r#"/(?:{name}/)*{name}{ext}"#);
            // 相对（含 ./ ../ 与任意含分隔符的相对；\ / 均收）。段内无空白后，
            // 无锚点分支天然从真实路径起点起搏（行文前缀 "看 " 含空格进不了首段）。
            let rel = format!(r#"{seg}+(?:[\\/]{name}+)*[\\/]{name}{ext}"#);
            (win, unc, nix_abs, rel)
        };
        let (win, unc, nix_abs, rel) = build(seg_ns);
        let core = format!("(?:{win})|(?:{unc})|(?:{nix_abs})|(?:{rel})");
        let (win_q, unc_q, nix_q, rel_q) = build(seg_q);
        let core_q = format!("(?:{win_q})|(?:{unc_q})|(?:{nix_q})|(?:{rel_q})");
        // 引号形态排在无引号之前：同一位置两者都可行时优先整段带引号匹配
        // （leftmost-first），raw 恒定走剥离逻辑，行为单一。
        regex::RegexBuilder::new(format!("\"(?:{core_q})\"|「(?:{core_q})」|(?:{core})").as_str())
            .build()
            .expect("image path regex is static and valid")
    })
}

/// 引号形态匹配的 raw 带包裹字符（`"..."` / `「...」`）——剥成对包裹后再
/// 解析。引号分支产出的 raw 必然成对（闭合引号是分支的一部分），故两侧
/// 独立剥离即可；无引号形态不含引号字符，剥离是 no-op。
fn strip_path_quotes(raw: &str) -> &str {
    let inner = raw
        .strip_prefix('"')
        .or_else(|| raw.strip_prefix('「'))
        .unwrap_or(raw);
    inner
        .strip_suffix('"')
        .or_else(|| inner.strip_suffix('」'))
        .unwrap_or(inner)
}

/// 扩展名是否在白名单（大小写不敏感）。
/// pub：T8 Web 上传端点复用同一白名单（单一真相源）。
pub fn has_image_extension(candidate: &str) -> bool {
    let lower = candidate.to_lowercase();
    IMAGE_EXTENSIONS
        .iter()
        .any(|ext| lower.rsplit('.').next() == Some(*ext) && lower.contains('.'))
}

/// URL span 识别（F-H）：`scheme://` 后到空白/常见终止符为止的整段文本。
/// 覆盖 http/https 及任意 scheme 形态（ftp:// 等同样不该被抓成路径）。
fn url_span_regex() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // 非贪婪到第一个空白或闭合性标点（) > ] } " ' ，。；！？）为止。
        regex::Regex::new(r#"[A-Za-z][A-Za-z0-9+.-]*://[^\s)>\]}"'，。；！？]+"#)
            .expect("url span regex is static and valid")
    })
}

/// magic byte 嗅探（防文本伪装扩展名）。
/// pub：T8 Web 上传端点落盘**前**复用同一嗅探（单一真相源，勿在他处重抄签名表）。
pub fn sniff_magic(bytes: &[u8]) -> bool {
    ext_from_magic(bytes).is_some()
}

/// magic byte → 白名单扩展名（2026-09-03 二次回归 C3：URL 下载落盘按**内容**
/// 定扩展名——Content-Type 可伪造/缺失、URL 后缀可说谎；与 [`sniff_magic`]
/// 同一张签名表 = 单一真相源，勿在他处重抄）。
pub fn ext_from_magic(bytes: &[u8]) -> Option<&'static str> {
    // PNG: 89 50 4E 47
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("png");
    }
    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    // GIF87a / GIF89a
    if bytes.starts_with(b"GIF8") {
        return Some("gif");
    }
    // WebP: RIFF....WEBP
    if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

/// 跨来源去重键（2026-09-03 二次回归 C1/C2）：分隔符 `\` → `/` 归一（检测器
/// 明确接受 `\` `/` 混用路径，raw 小写化下 `C:\x\a.png` 与 `C:/x/a.png` 是
/// 两个键 = 同图重复附加）；小写化**仅 Windows**（NTFS 大小写不敏感）。
/// L5（2026-09-04 四轮盲审）：分隔符归一同样**仅 Windows**——POSIX 上 `\`
/// 是合法文件名字符（`dir\a.png` 与 `dir/a.png` 是两个不同文件），归一会把
/// 同消息里的两个不同文件错误去重掉一个。
/// pub：T4 检测器与 T5 附加层（文本来源 + media 引用来源）共用同一键。
pub fn dedup_key(resolved: &Path) -> String {
    let s = resolved.to_string_lossy();
    if cfg!(windows) {
        s.replace('\\', "/").to_lowercase()
    } else {
        s.into_owned()
    }
}

/// 单候选验真：存在性 + 大小上限 + magic byte。
/// pub(crate)：T5 统一附加流程对 media 引用路径复用同一验真链。
pub fn verify(resolved: &Path) -> CandidateStatus {
    let metadata = match std::fs::metadata(resolved) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return CandidateStatus::NotFound,
        Err(_) => return CandidateStatus::Unreadable,
    };
    if !metadata.is_file() {
        return CandidateStatus::NotFound;
    }
    let size = metadata.len();
    if size > MAX_IMAGE_BYTES {
        return CandidateStatus::TooLarge { size };
    }
    // magic 嗅探：只读前 12 字节（WebP 需要）
    match std::fs::File::open(resolved) {
        Ok(mut f) => {
            use std::io::Read;
            let mut buf = [0u8; 12];
            match f.read(&mut buf) {
                Ok(n) => {
                    if sniff_magic(&buf[..n]) {
                        CandidateStatus::Ok { size }
                    } else {
                        CandidateStatus::NotImage {
                            magic_hex: buf[..n.min(4)]
                                .iter()
                                .map(|b| format!("{:02x}", b))
                                .collect::<Vec<_>>()
                                .join(" "),
                        }
                    }
                }
                Err(_) => CandidateStatus::Unreadable,
            }
        }
        Err(_) => CandidateStatus::Unreadable,
    }
}

/// 扩展名 → OpenAI/Anthropic vision 通用 MIME（T5 附加 + 水合用）。
/// 白名单外返回 None（调用方按不支持处理）。
pub fn media_type_for_path(path: &Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    match ext.as_deref() {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        _ => None,
    }
}

/// 从消息文本检测图片路径并验真（T4 主入口；T5 轮次摄取调用）。
///
/// - `workspace_dir`：工作区相对路径的解析基准；None 时相对候选不产出。
/// - 相对候选不存在 → 静默丢弃（行文误报防线）；绝对/UNC 候选不存在 →
///   保留并诚实注明。
/// - 同消息去重：解析路径归一一致（分隔符统一；Windows 下大小写不敏感）
///   只保留首个。
pub fn detect_image_paths(text: &str, workspace_dir: Option<&Path>) -> Vec<ImagePathCandidate> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();

    // F-H（2026-09-04 四轮盲审）：先收集 URL span——`https://host/img/a.png`
    // 里的 `/host/img/a.png` 会被 nix_abs（无左边界）抓成"POSIX 绝对路径"，
    // 产出假的 deliberate 候选（NotFound → 每次都往历史里灌一条"文件不存在"
    // 假注记；路径碰巧存在时还会错误附上本地文件）。URL 不是文本附加的
    // 支持面（URL 走 media 引用预取，T9），落在 URL span 内的匹配整体跳过。
    let url_spans: Vec<(usize, usize)> = url_span_regex()
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect();
    let in_url_span =
        |start: usize, end: usize| url_spans.iter().any(|&(s, e)| start < e && end > s);

    for m in path_regex().find_iter(text) {
        if in_url_span(m.start(), m.end()) {
            continue;
        }
        // 引号形态的匹配带包裹字符——剥离后再进扩展名判断与路径解析，
        // raw/notes/去重键见到的都是干净路径。
        let raw = strip_path_quotes(m.as_str()).to_string();
        if !has_image_extension(&raw) {
            continue;
        }

        let candidate_path = Path::new(&raw);
        // 明确点名 = 有根路径：盘符绝对 / UNC / POSIX 根（"/x" 在 Windows 上
        // is_absolute=false 但 has_root=true——它是根锚定路径，不是工作区相对）
        let rooted = candidate_path.has_root();
        let resolved: PathBuf = if rooted {
            candidate_path.to_path_buf()
        } else {
            match workspace_dir {
                Some(ws) => ws.join(candidate_path),
                None => continue, // 无基准的相对候选不产出
            }
        };

        // 去重键：解析路径归一（分隔符统一 + Windows 大小写不敏感）。
        let key = dedup_key(&resolved);
        if !seen.insert(key) {
            continue;
        }

        let status = verify(&resolved);
        // 行文误报防线：相对候选且文件不存在 → 静默丢弃
        if !rooted && status == CandidateStatus::NotFound {
            continue;
        }

        out.push(ImagePathCandidate {
            raw,
            resolved,
            deliberate: rooted,
            status,
        });
    }
    out
}

#[cfg(test)]
mod tests;
