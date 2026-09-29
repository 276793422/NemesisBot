//! 集群专业职能框架 M5：验收链图像工件取回 + vision 评审。
//!
//! worker（ui-design 职能等）交付 PNG 时经 board_asset publish 产出引用
//! bundle 粘进交付评论；本模块在 master 评审侧：
//! 1. 从交付/线程评论**文本**中提取图像 bundle（数据非指令——bundle 只
//!    当取货凭据，内容不进 prompt 指令位）；
//! 2. 程序化取回（复用 [`crate::board_asset_tool::BoardAssetTool::do_fetch`]
//!    单一实现：HTTP 直连 + RPC 分块兜底 + sha256 校验）；
//! 3. base64 image part 进评审裸调用（providers 多模态管线；provider 取
//!    `default_slot::current()`——与主 loop 活跃模型同源，不经 nemesis-agent
//!    改动）→ `parse_review` 三态契约与文本评审完全同源；
//! 4. **诚实降级**（计划 §2.7 预案，按次触发而非一刀切）：无默认模型槽 /
//!    视觉调用失败 → 回落文本评审 + prompt 注记「图片已取回本地 <path>，
//!    转人工查看」——功能不残（图在 master 工作区随时可看），仅验收自动
//!    化打折。
//!
//! 预算上限：每单 ≤ [`MAX_REVIEW_IMAGES`] 张、单张 ≤
//! [`MAX_IMAGE_BYTES`]（超限诚实跳过并注记，不静默截半）。

use crate::board_asset_tool::BoardAssetTool;
use nemesis_board::{AssetTokenBundle, Comment, CommentType};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// 每单评审最多附图张数（超出诚实跳过）。
pub(crate) const MAX_REVIEW_IMAGES: usize = 4;
/// 单张图像字节上限（4 MiB；b64 后 ~5.5MB，在 LLM 请求预算内）。
pub(crate) const MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024;

/// 图像扩展名白名单（bundle ref 的扩展名过滤；不带扩展名的 ref 不当图）。
fn is_image_ref(ref_name: &str) -> bool {
    let lower = ref_name.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".webp", ".gif"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// 从一段评论文本中提取图像资产 bundle。
///
/// 扫描文本里的 `{...}` JSON 对象（括号配对，容忍嵌套引号转义），能反
/// 序列化成 [`AssetTokenBundle`] 且 ref 是图像扩展名的才收——worker 汇报
/// 里的普通 JSON/代码块不受影响。
fn extract_image_bundles_from(text: &str) -> Vec<AssetTokenBundle> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        // 括号配对扫描（引号内忽略括号；反斜杠转义跳过）。
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        let mut end = None;
        for (off, &b) in bytes[i..].iter().enumerate() {
            if escaped {
                escaped = false;
                continue;
            }
            match b {
                b'\\' if in_string => escaped = true,
                b'"' => in_string = !in_string,
                b'{' if !in_string => depth += 1,
                b'}' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i + off + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        let candidate = &text[i..end];
        if let Ok(bundle) = serde_json::from_str::<AssetTokenBundle>(candidate)
            && !bundle.asset_ref.is_empty()
            && is_image_ref(&bundle.asset_ref)
        {
            out.push(bundle);
        }
        i = end;
    }
    out
}

/// 取回并就绪的评审图像。
pub(crate) struct ReviewImage {
    pub ref_name: String,
    /// 原始字节（≤ [`MAX_IMAGE_BYTES`]；b64 在组装请求时做）。
    pub bytes: Vec<u8>,
    /// 落盘路径（master 工作区 board/assets/<ref>；降级注记给人工看图用）。
    pub path: PathBuf,
}

/// 从交付/线程评论收集评审图像（去重 by ref；先本地后网络）。
///
/// `workspace` = master 工作区；`cluster` = 集群句柄（RPC 兜底腿用）。
/// 全部失败 = 空 Vec + warn（不阻塞评审——文本路径照常）。
pub(crate) async fn collect_review_images(
    workspace: &Path,
    cluster: &std::sync::Arc<nemesis_cluster::cluster::Cluster>,
    comments: &[Comment],
) -> Vec<ReviewImage> {
    // 提取 + 去重（同 ref 多次出现只取一次；交付评论在前优先）。
    let mut bundles: Vec<AssetTokenBundle> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in comments {
        if matches!(c.ctype, CommentType::StatusChange | CommentType::System) {
            continue;
        }
        for b in extract_image_bundles_from(&c.content) {
            if seen.insert(b.asset_ref.clone()) {
                bundles.push(b);
            }
            if bundles.len() >= MAX_REVIEW_IMAGES {
                break;
            }
        }
        if bundles.len() >= MAX_REVIEW_IMAGES {
            break;
        }
    }
    if bundles.is_empty() {
        return Vec::new();
    }

    // 与 BoardAssetTool::assets_dir 同源（nemesis-path 单一真相源）。
    let assets_dir = nemesis_path::resolve_board_assets_dir_in_workspace(workspace);
    let tool = BoardAssetTool::new(workspace.to_path_buf()).with_cluster(cluster.clone());
    let mut out = Vec::new();
    for b in bundles {
        let dest = assets_dir.join(&b.asset_ref);
        // 先本地：实体已在（上次取回/本机 worker publish）→ 免网络。
        let ok_local = std::fs::metadata(&dest).is_ok_and(|m| m.is_file())
            && nemesis_board::sha256_file(&dest)
                .map(|sha| sha == b.sha256)
                .unwrap_or(false);
        if !ok_local {
            match tool
                .do_fetch(
                    &b.node_url,
                    &b.asset_ref,
                    &b.asset_token,
                    b.expires_at,
                    &b.sha256,
                    Some(&b.node_id),
                )
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    warn!(
                        "[BoardReviewAssets] 图像 `{}` 取回失败（跳过该图）: {e}",
                        b.asset_ref
                    );
                    continue;
                }
            }
        }
        match std::fs::read(&dest) {
            Ok(bytes) => {
                if bytes.len() as u64 > MAX_IMAGE_BYTES {
                    warn!(
                        "[BoardReviewAssets] 图像 `{}` {} 字节超单张上限（跳过该图）",
                        b.asset_ref,
                        bytes.len()
                    );
                    continue;
                }
                info!(
                    "[BoardReviewAssets] 评审图像就绪: {}（{} 字节）",
                    b.asset_ref,
                    bytes.len()
                );
                out.push(ReviewImage {
                    ref_name: b.asset_ref,
                    bytes,
                    path: dest,
                });
            }
            Err(e) => warn!("[BoardReviewAssets] 图像 `{}` 读取失败: {e}", b.asset_ref),
        }
        if out.len() >= MAX_REVIEW_IMAGES {
            break;
        }
    }
    out
}

/// 视觉评审裸调用：与 [`run_review_llm`]（board_review.rs）同契约——
/// `parse_review` 三态 JSON + 失败回灌重试 ≤2（共 3 轮）；差异只在
/// 底座：直调 `default_slot` provider（多模态 image parts），不走
/// `run_detached`（DetachedOpts 无 images 面，nemesis-agent 禁区不动）。
///
/// 解析失败重试时图 parts 原样保留（回灌只换文本段）。
pub(crate) async fn run_review_llm_vision(
    provider: std::sync::Arc<dyn nemesis_providers::router::LLMProvider>,
    model: &str,
    system_prompt: &str,
    prompt: &mut String,
    images: &[ReviewImage],
) -> Result<nemesis_board::ReviewOutput, String> {
    use nemesis_providers::types::{
        ChatOptions, ContentPart, ImageSource, Message, MessageContent,
    };
    let mut last_err = String::new();
    for _ in 0..=2 {
        let mut parts = Vec::with_capacity(1 + images.len());
        parts.push(ContentPart::Text {
            text: prompt.clone(),
        });
        for img in images {
            let media = if img.ref_name.to_ascii_lowercase().ends_with(".png") {
                "image/png"
            } else if img.ref_name.to_ascii_lowercase().ends_with(".webp") {
                "image/webp"
            } else if img.ref_name.to_ascii_lowercase().ends_with(".gif") {
                "image/gif"
            } else {
                "image/jpeg"
            };
            parts.push(ContentPart::Image {
                image: ImageSource::Base64 {
                    media_type: media.to_string(),
                    data: base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &img.bytes,
                    ),
                },
                detail: None,
            });
        }
        let messages = [
            Message {
                role: "system".to_string(),
                content: MessageContent::Text(system_prompt.to_string()),
                ..Default::default()
            },
            Message {
                role: "user".to_string(),
                content: MessageContent::Parts(parts),
                ..Default::default()
            },
        ];
        let resp = provider
            .chat(&messages, &[], model, &ChatOptions::default())
            .await
            .map_err(|e| format!("LLM 调用失败：{e}"))?;
        match nemesis_board::parse_review(&resp.content) {
            Ok(out) => return Ok(out),
            Err(e) => {
                *prompt = nemesis_board::review::build_retry_prompt(&resp.content, &e);
                last_err = e.message;
            }
        }
    }
    Err(format!("评审输出连续 3 轮无法解析：{last_err}"))
}

#[cfg(test)]
mod tests;
