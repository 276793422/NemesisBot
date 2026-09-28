//! Minimal WAV codec — realtime P1（W7 测试注入面）
//!
//! RIFF/WAVE 解析（PCM16 / float32，多声道 → mono 平均）与 mono PCM16 写入。
//! 只服务测试夹具与本地采集回放，不做完整格式兼容。

use anyhow::Result;

/// Minimal RIFF/WAVE parser → (mono f32 samples, sample_rate).
pub fn parse_wav(bytes: &[u8]) -> Result<(Vec<f32>, u32)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        anyhow::bail!("Not a RIFF/WAVE file");
    }

    let mut pos = 12usize;
    let mut format_tag: u16 = 0;
    let mut channels: u16 = 0;
    let mut sample_rate: u32 = 0;
    let mut bits: u16 = 0;
    let mut data: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(size).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    anyhow::bail!("WAVE fmt chunk too short");
                }
                format_tag = u16::from_le_bytes([body[0], body[1]]);
                channels = u16::from_le_bytes([body[2], body[3]]);
                sample_rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                bits = u16::from_le_bytes([body[14], body[15]]);
            }
            b"data" => data = Some(body),
            _ => {}
        }
        // Chunks are word-aligned.
        pos = body_start + size.div_ceil(2) * 2;
    }

    let data = data.ok_or_else(|| anyhow::anyhow!("WAVE data chunk missing"))?;
    if channels == 0 || sample_rate == 0 {
        anyhow::bail!("WAVE fmt chunk missing or invalid");
    }

    let mut samples = Vec::with_capacity(data.len() / 2);
    match (format_tag, bits) {
        (1, 16) => {
            for ch in data.chunks_exact(2) {
                samples.push(i16::from_le_bytes([ch[0], ch[1]]) as f32 / 32768.0);
            }
        }
        (3, 32) => {
            for ch in data.chunks_exact(4) {
                samples.push(f32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]));
            }
        }
        _ => anyhow::bail!(
            "Unsupported WAVE format (tag={}, bits={})",
            format_tag,
            bits
        ),
    }

    // Interleave → mono average.
    let mono = if channels > 1 {
        let ch = channels as usize;
        samples
            .chunks_exact(ch)
            .map(|frame| frame.iter().sum::<f32>() / ch as f32)
            .collect()
    } else {
        samples
    };

    Ok((mono, sample_rate))
}

/// Minimal mono f32 → PCM16 WAV writer (test fixture helper, also handy for
/// local captures). 44-byte canonical header + little-endian data.
pub fn write_wav_mono16(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod wav_tests;
