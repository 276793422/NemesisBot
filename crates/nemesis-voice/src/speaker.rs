//! Speaker verification safe wrapper — CAM++ via sherpa-onnx SpeakerEmbedding

use anyhow::Result;
use std::ffi::{CStr, CString};
use std::path::Path;

use crate::sherpa;

pub struct SpeakerEngine {
    extractor: *const sherpa::SherpaOnnxSpeakerEmbeddingExtractor,
    dim: i32,
}

pub struct SpeakerManager {
    manager: *const sherpa::SherpaOnnxSpeakerEmbeddingManager,
    dim: i32,
}

unsafe impl Send for SpeakerEngine {}
unsafe impl Sync for SpeakerEngine {}
unsafe impl Send for SpeakerManager {}
unsafe impl Sync for SpeakerManager {}

impl SpeakerEngine {
    pub fn new(model_dir: &Path, num_threads: u32) -> Result<Self> {
        let model_path =
            model_dir.join("3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx");
        if !model_path.exists() {
            anyhow::bail!("Speaker model not found: {}", model_path.display());
        }

        let model_c = CString::new(model_path.to_str().unwrap_or("")).unwrap();
        let provider_c = CString::new("cpu").unwrap();

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

        let mut embedding = Vec::with_capacity(self.dim as usize);
        if !embedding_ptr.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    embedding_ptr,
                    embedding.as_mut_ptr(),
                    self.dim as usize,
                );
                embedding.set_len(self.dim as usize);
            }
            unsafe { sherpa::SherpaOnnxSpeakerEmbeddingExtractorDestroyEmbedding(embedding_ptr) };
        }

        unsafe { sherpa::SherpaOnnxDestroyOnlineStream(stream) };

        Ok(embedding)
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

impl SpeakerManager {
    pub fn new(dim: i32) -> Self {
        let manager = unsafe { sherpa::SherpaOnnxCreateSpeakerEmbeddingManager(dim) };
        Self { manager, dim }
    }

    pub fn register(&mut self, name: &str, embedding: &[f32]) -> bool {
        let name_c = CString::new(name).unwrap();
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
        let name_c = CString::new(name).unwrap();
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
        let name_c = CString::new(name).unwrap();
        let rc = unsafe {
            sherpa::SherpaOnnxSpeakerEmbeddingManagerRemove(self.manager, name_c.as_ptr())
        };
        rc != 0
    }

    pub fn verify(&self, name: &str, embedding: &[f32], threshold: f32) -> bool {
        let name_c = CString::new(name).unwrap();
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
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests;
