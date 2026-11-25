//! Speaker verification safe wrapper — CAM++ via sherpa-onnx SpeakerEmbedding

use anyhow::Result;
use std::ffi::{CStr, CString};
use std::path::Path;

use crate::sherpa;

/// FFI 字符串边界：CString::new 失败（内嵌 NUL）转 Result 错误臂而非
/// panic（BUG-20261115-014，与 stt.rs 同款 helper 形态一致）。
fn c_string(s: &str) -> Result<CString> {
    CString::new(s).map_err(|e| anyhow::anyhow!("字符串含内嵌 NUL 字节，无法转 C 字符串: {e}"))
}

/// C 侧嵌入缓冲 → 拥有型 Vec。null 缓冲 = 诚实错误臂（BUG-20261125-044：
/// 此前静默返回 Ok(空 Vec)，下游把空切片裸指针交给按 dim 无条件读的
/// C API = 堆越界读）。非 null 时按 dim 拷贝；负 dim（契约外）按 0 处理
/// 不做巨分配。
fn collect_embedding(ptr: *const f32, dim: i32) -> Result<Vec<f32>> {
    if ptr.is_null() {
        anyhow::bail!("speaker embedding extraction returned null buffer (dim={dim})");
    }
    let n = dim.max(0) as usize;
    let mut embedding = Vec::with_capacity(n);
    unsafe {
        std::ptr::copy_nonoverlapping(ptr, embedding.as_mut_ptr(), n);
        embedding.set_len(n);
    }
    Ok(embedding)
}

pub struct SpeakerEngine {
    extractor: *const sherpa::SherpaOnnxSpeakerEmbeddingExtractor,
    dim: i32,
}

pub struct SpeakerManager {
    manager: *const sherpa::SherpaOnnxSpeakerEmbeddingManager,
    dim: i32,
}

// SAFETY（审计表 C-6/C-7）：两个指针均为 owning 裸指针，随实例移动安全；
// 不实现 Sync——上游对 SpeakerEmbeddingExtractor/Manager 均无线程安全承诺
// （沉默 → 按非线程安全对待；Manager 的 register/remove 是内部 C++ 容器的
// 变更操作）。唯一消费路径 = web handlers/voice.rs 的
// `OnceLock<Mutex<Option<E>>>` 静态槽（外层 Mutex 串行化，对类型只要求
// Send）。串行需求由外层 Mutex 类型化表达；若未来需无锁共享，必须先补
// 上游依据而非恢复 Sync。
unsafe impl Send for SpeakerEngine {}
unsafe impl Send for SpeakerManager {}

impl SpeakerEngine {
    pub fn new(model_dir: &Path, num_threads: u32) -> Result<Self> {
        let model_path =
            model_dir.join("3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx");
        if !model_path.exists() {
            anyhow::bail!("Speaker model not found: {}", model_path.display());
        }

        // BUG-20261115-014：同 stt.rs——模型目录源自用户配置，防御内嵌
        // NUL 走错误臂（当前 model_path 有 exists() 闸先拦，本臂属纵深
        // 防御；形态与 stt.rs c_string 一致）。
        let model_c = c_string(model_path.to_str().unwrap_or(""))?;
        let provider_c = c_string("cpu")?;

        let config = sherpa::SherpaOnnxSpeakerEmbeddingExtractorConfig {
            model: model_c.as_ptr(),
            num_threads: num_threads as libc::c_int,
            debug: 0,
            provider: provider_c.as_ptr(),
        };

        let extractor = unsafe { sherpa::SherpaOnnxCreateSpeakerEmbeddingExtractor(&config) };

        if extractor.is_null() {
            anyhow::bail!("Failed to create SpeakerEmbedding extractor");
        }

        let dim = unsafe { sherpa::SherpaOnnxSpeakerEmbeddingExtractorDim(extractor) };

        Ok(Self { extractor, dim })
    }

    pub fn embed(&self, samples: &[f32], sample_rate: u32) -> Result<Vec<f32>> {
        let stream =
            unsafe { sherpa::SherpaOnnxSpeakerEmbeddingExtractorCreateStream(self.extractor) };
        if stream.is_null() {
            anyhow::bail!("Failed to create speaker embedding stream");
        }

        unsafe {
            sherpa::SherpaOnnxOnlineStreamAcceptWaveform(
                stream,
                sample_rate as libc::c_int,
                samples.as_ptr(),
                samples.len() as libc::c_int,
            );
            sherpa::SherpaOnnxOnlineStreamInputFinished(stream);
        }

        let ready =
            unsafe { sherpa::SherpaOnnxSpeakerEmbeddingExtractorIsReady(self.extractor, stream) };
        if ready == 0 {
            unsafe { sherpa::SherpaOnnxDestroyOnlineStream(stream) };
            anyhow::bail!("Speaker embedding extractor not ready");
        }

        let embedding_ptr = unsafe {
            sherpa::SherpaOnnxSpeakerEmbeddingExtractorComputeEmbedding(self.extractor, stream)
        };

        // BUG-20261125-044：ComputeEmbedding 返回 null（防御面）不再静默落
        // Ok(空 Vec)。空切片喂给 SpeakerManager::register/verify/search——
        // C API 不接收长度参数、按构造时 dim 无条件读——是堆越界读；且空
        // 嵌入会被 register_stop 持久化进声纹配置跨重启存活（恢复路径按
        // len==dim 静默跳过 → 声纹消失但当时报过 "registered": true）。对齐
        // 本函数上方 stream null / ready==0 两臂的诚实失败形态。
        let embedding = collect_embedding(embedding_ptr, self.dim);
        if !embedding_ptr.is_null() {
            unsafe { sherpa::SherpaOnnxSpeakerEmbeddingExtractorDestroyEmbedding(embedding_ptr) };
        }

        unsafe { sherpa::SherpaOnnxDestroyOnlineStream(stream) };

        embedding
    }

    pub fn embedding_dim(&self) -> i32 {
        self.dim
    }
}

impl Drop for SpeakerEngine {
    fn drop(&mut self) {
        unsafe { sherpa::SherpaOnnxDestroySpeakerEmbeddingExtractor(self.extractor) };
    }
}

/// 名字 → C 字符串。内嵌 NUL（JSON `"\u0000"` 合法、可从 WSAPI speaker 命令
/// 直达）时 `CString::new` 失败——BUG-20261106-020 之前这里 `.unwrap()` panic，
/// 且调用方（web handler `cmd_speaker_remove`/`register_stop`）在持有
/// `speaker_manager_state` std Mutex 的上下文里调用：panic 毒化锁 → 后续所有
/// voice 命令对该锁的 `.lock().unwrap()` 连锁 panic，语音子系统进程内瘫痪。
/// 现契约：诚实拒绝（None → 调用方返回 false，与 C API rc==0 失败同型），
/// 库 API 对任意 `&str` 不 panic。
fn speaker_name_cstr(name: &str) -> Option<CString> {
    match CString::new(name) {
        Ok(c) => Some(c),
        Err(_) => {
            tracing::warn!("[Speaker] name contains interior NUL, rejected");
            None
        }
    }
}

impl SpeakerManager {
    pub fn new(dim: i32) -> Self {
        let manager = unsafe { sherpa::SherpaOnnxCreateSpeakerEmbeddingManager(dim) };
        Self { manager, dim }
    }

    pub fn register(&mut self, name: &str, embedding: &[f32]) -> bool {
        let Some(name_c) = speaker_name_cstr(name) else {
            return false;
        };
        // BUG-20261125-044：C API Add 不接收长度参数，按构造时 dim 无条件
        // 读——错误长度切片（含空）= 堆越界读。长度闸前置（与 NUL 名闸
        // 同型：FFI 前诚实 false）。
        if embedding.len() != self.dim as usize {
            return false;
        }
        let rc = unsafe {
            sherpa::SherpaOnnxSpeakerEmbeddingManagerAdd(
                self.manager,
                name_c.as_ptr(),
                embedding.as_ptr(),
            )
        };
        rc != 0
    }

    pub fn register_multi(&mut self, name: &str, embeddings: &[Vec<f32>]) -> bool {
        if embeddings.is_empty() {
            return false;
        }
        let Some(name_c) = speaker_name_cstr(name) else {
            return false;
        };
        // BUG-20261125-044：AddListFlattened 读 n×dim 个 float——任一分量
        // 长度错位即越界。分量长度闸前置。
        if embeddings.iter().any(|e| e.len() != self.dim as usize) {
            return false;
        }
        let flattened: Vec<f32> = embeddings.iter().flat_map(|e| e.iter().copied()).collect();
        let n = embeddings.len() as libc::c_int;
        let rc = unsafe {
            sherpa::SherpaOnnxSpeakerEmbeddingManagerAddListFlattened(
                self.manager,
                name_c.as_ptr(),
                flattened.as_ptr(),
                n,
            )
        };
        rc != 0
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let Some(name_c) = speaker_name_cstr(name) else {
            return false;
        };
        let rc = unsafe {
            sherpa::SherpaOnnxSpeakerEmbeddingManagerRemove(self.manager, name_c.as_ptr())
        };
        rc != 0
    }

    pub fn verify(&self, name: &str, embedding: &[f32], threshold: f32) -> bool {
        let Some(name_c) = speaker_name_cstr(name) else {
            return false;
        };
        // BUG-20261125-044：Verify 按 dim 无条件读，长度闸前置（同 register）。
        if embedding.len() != self.dim as usize {
            return false;
        }
        let rc = unsafe {
            sherpa::SherpaOnnxSpeakerEmbeddingManagerVerify(
                self.manager,
                name_c.as_ptr(),
                embedding.as_ptr(),
                threshold,
            )
        };
        rc != 0
    }

    pub fn search(&self, embedding: &[f32], threshold: f32) -> Option<String> {
        // BUG-20261125-044：Search 按 dim 无条件读，长度闸前置（同 register）。
        if embedding.len() != self.dim as usize {
            return None;
        }
        let result_ptr = unsafe {
            sherpa::SherpaOnnxSpeakerEmbeddingManagerSearch(
                self.manager,
                embedding.as_ptr(),
                threshold,
            )
        };
        if result_ptr.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(result_ptr) }
            .to_str()
            .ok()
            .map(|s| s.to_string());
        unsafe { sherpa::SherpaOnnxSpeakerEmbeddingManagerFreeSearch(result_ptr) };
        name
    }

    pub fn list_speakers(&self) -> Vec<String> {
        let names_ptr =
            unsafe { sherpa::SherpaOnnxSpeakerEmbeddingManagerGetAllSpeakers(self.manager) };
        let mut speakers = Vec::new();
        if names_ptr.is_null() {
            return speakers;
        }
        let mut i = 0;
        loop {
            let ptr = unsafe { *names_ptr.add(i) };
            if ptr.is_null() {
                break;
            }
            if let Ok(s) = unsafe { CStr::from_ptr(ptr) }.to_str() {
                speakers.push(s.to_string());
            }
            i += 1;
        }
        unsafe { sherpa::SherpaOnnxSpeakerEmbeddingManagerFreeAllSpeakers(names_ptr) };
        speakers
    }

    pub fn dim(&self) -> i32 {
        self.dim
    }
}

impl Drop for SpeakerManager {
    fn drop(&mut self) {
        unsafe { sherpa::SherpaOnnxDestroySpeakerEmbeddingManager(self.manager) };
    }
}

/// Compute cosine similarity between two embedding vectors.
///
/// Returns 0.0 (not comparable) for length mismatch, empty input, zero
/// norm, or a **non-finite norm**. Non-finite norms are rejected, not
/// trusted to the zero-norm check: `NaN == 0.0` is always false (NaN
/// sails through) and inf passes it too — a NaN score then silently
/// vanishes at the speaker-test `similarity >= 0.0` event gate, and an
/// inf score passes that gate, flips `matched` true and serializes as
/// JSON `null`. See BUG-20261117-086 (same doctrine as the shared
/// implementation `nemesis_types::utils::cosine_similarity_f32`,
/// BUG-20261116-015): model-provided embeddings (sherpa-onnx output,
/// copied raw with no upstream finiteness validation) and f32 square
/// accumulation (which overflows to inf from finite large elements)
/// both make non-finite norms reachable without any adversarial input.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 || !norm_a.is_finite() || !norm_b.is_finite() {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests;
