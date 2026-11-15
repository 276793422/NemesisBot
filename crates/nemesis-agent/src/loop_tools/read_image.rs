// ---------------------------------------------------------------------------
// (Phase 5 图像工具层) read_image：LLM 主动读图工具。
// ---------------------------------------------------------------------------

use super::*;

/// `read_image`：验证并把图片附件给当前模型的 LLM。
///
/// 语义：模型对工作区内（或放行路径上）的图片调本工具 → 工具验真（存在/
/// 扩展名白名单/magic byte/25MB 上限）→ 超限图尝试降采样（image-process
/// feature，4096→2048→1024 阶梯 + JPEG q85，产物落 uploads 内容寻址）→
/// 路径 push 进 `RequestContext.tool_images` 槽 → 串行 dispatch 收口把它随
/// tool 轮 `ConversationTurn.image_refs` 落史 → 下一轮 LLM 请求水合带图，
/// 模型真看见。
///
/// 与 attach 侧（用户上传/提示词路径提取）共用同一验真表
/// （`image_path_detector`）与降采样闸（`image_downscale`）——单一真相源，
/// 本文件不重抄签名表。
#[derive(Default)]
pub struct ReadImageTool {
    boundary: Option<Arc<WorkspaceBoundary>>,
}

impl ReadImageTool {
    /// 带工作区边界的生产形态（相对路径锚定工作区根 + restrict 读界）。
    pub fn with_boundary(boundary: Arc<WorkspaceBoundary>) -> Self {
        Self {
            boundary: Some(boundary),
        }
    }
}

#[async_trait]
impl Tool for ReadImageTool {
    fn description(&self) -> String {
        "Read an image file and attach it to the conversation so you (the model) can see its \
visual content. Accepts png/jpg/jpeg/webp/gif. Images above 25MB are automatically downscaled \
when the image pipeline is compiled in. The image is delivered to the model on the next round — \
call this tool, then describe/analyze what you see in your reply."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the image file (workspace-relative or absolute)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let path = extract_path(args)?;
        let path = resolve_tool_path(&path, self.boundary.as_deref(), FenceOpFamily::Read)?;

        if !path.exists() {
            return Err(format!("Image not found: {}", path.display()));
        }
        if !path.is_file() {
            return Err(format!("Not a file (directory?): {}", path.display()));
        }

        // 扩展名白名单（与 attach 侧同一张表）。
        let ext_ok = crate::image_path_detector::has_image_extension(&path.to_string_lossy());
        if !ext_ok {
            return Err(format!(
                "Unsupported image format: {} (supported: png/jpg/jpeg/webp/gif). \
Use read_file for text files.",
                path.display()
            ));
        }

        // 验真链：存在性 + 大小 + magic byte（attach 侧同源）。
        let status = crate::image_path_detector::verify(&path);
        let final_path = match status {
            crate::image_path_detector::CandidateStatus::Ok { .. } => path.clone(),
            crate::image_path_detector::CandidateStatus::TooLarge { size } => {
                // 超限：降采样闸兜底（image-process 在场且 uploads 目录可得
                // 才产出；feature 关/无 uploads 根 = 诚实报错，不静默吞）。
                let uploads_dir = self.boundary.as_ref().map(|b| b.root.join("uploads"));
                match crate::image_downscale::downscale_gate(&path, uploads_dir.as_deref())? {
                    crate::image_downscale::GateVerdict::Replaced {
                        path: product,
                        from_bytes,
                        to_bytes,
                        longest_side,
                    } => {
                        info!(
                            "[read_image] oversized image downscaled: {} ({} bytes) -> {} ({} bytes, longest side {}px)",
                            path.display(),
                            from_bytes,
                            product.display(),
                            to_bytes,
                            longest_side
                        );
                        product
                    }
                    crate::image_downscale::GateVerdict::Keep => {
                        return Err(format!(
                            "Image too large: {} bytes (cap {}). This build has no downscale \
pipeline — ask the user to provide a smaller image.",
                            size,
                            crate::image_path_detector::MAX_IMAGE_BYTES
                        ));
                    }
                }
            }
            crate::image_path_detector::CandidateStatus::NotFound => {
                return Err(format!("Image not found: {}", path.display()));
            }
            crate::image_path_detector::CandidateStatus::Unreadable => {
                return Err(format!(
                    "Image unreadable (permissions/IO): {}",
                    path.display()
                ));
            }
            crate::image_path_detector::CandidateStatus::NotImage { magic_hex } => {
                return Err(format!(
                    "File content is not a recognized image (magic bytes: {}). Extension may \
be lying — use read_file or exec for non-image files.",
                    magic_hex
                ));
            }
        };

        let media_type =
            crate::image_path_detector::media_type_for_path(&final_path).unwrap_or("image/*");
        let size = std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);
        let path_str = final_path.to_string_lossy().to_string();

        // 路径入槽（串行 dispatch 在 apply_tool_guards 取走随 tool 轮落史）。
        context.push_tool_image(path_str.clone());

        Ok(format!(
            "Image attached for the model: {} ({} , {} bytes). Analyze it in your reply.",
            path_str, media_type, size
        ))
    }
}
