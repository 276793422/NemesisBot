//! Offline punctuation restoration — ct-transformer via sherpa-onnx
//!
//! Adds proper punctuation (commas, question marks, periods) to STT output
//! before sending to LLM. Uses sherpa-onnx-punct-ct-transformer-zh-en model.

use anyhow::Result;
use std::ffi::CStr;
use std::path::Path;

use crate::sherpa;

pub struct PunctEngine {
    punct: *const sherpa::SherpaOnnxOfflinePunctuation,
}

unsafe impl Send for PunctEngine {}
unsafe impl Sync for PunctEngine {}

impl PunctEngine {
    pub fn new(model_path: &Path, num_threads: u32) -> Result<Self> {
        if !model_path.exists() {
            anyhow::bail!("Punctuation model not found: {}", model_path.display());
        }

        let model_c = sherpa::to_cstr(model_path.to_str().unwrap_or(""));
        let provider_c = sherpa::to_cstr("cpu");

        let config = sherpa::SherpaOnnxOfflinePunctuationConfig {
            model: sherpa::SherpaOnnxOfflinePunctuationModelConfig {
                ct_transformer: model_c.as_ptr(),
                num_threads: num_threads as libc::c_int,
                debug: 0,
                provider: provider_c.as_ptr(),
            },
        };

        let punct = unsafe { sherpa::SherpaOnnxCreateOfflinePunctuation(&config) };
        if punct.is_null() {
            anyhow::bail!("Failed to create punctuation engine");
        }

        Ok(Self { punct })
    }

    /// Add punctuation to text. Returns the punctuated text.
    pub fn add_punctuation(&self, text: &str) -> Result<String> {
        if text.is_empty() {
            return Ok(String::new());
        }

        let text_c = sherpa::to_cstr(text);

        let result_ptr = unsafe {
            sherpa::SherpaOfflinePunctuationAddPunct(self.punct, text_c.as_ptr())
        };

        let result = if !result_ptr.is_null() {
            let s = unsafe { CStr::from_ptr(result_ptr) }
                .to_str()
                .unwrap_or("")
                .to_string();
            unsafe { sherpa::SherpaOfflinePunctuationFreeText(result_ptr) };
            s
        } else {
            text.to_string()
        };

        Ok(result)
    }
}

impl Drop for PunctEngine {
    fn drop(&mut self) {
        unsafe { sherpa::SherpaOnnxDestroyOfflinePunctuation(self.punct) };
    }
}
