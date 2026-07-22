//! Transport frame - length-prefixed binary message framing.
//!
//! Provides both synchronous and asynchronous frame encoding/decoding.
//! Frames use a 4-byte big-endian length header followed by the payload.
//!
//! # AEAD encryption
//!
//! When an auth token is configured, frame payloads are encrypted with
//! AES-256-GCM. The auth token is hashed with SHA-256 to derive a 32-byte
//! key. Each encrypted frame carries a 12-byte random nonce prepended to
//! the ciphertext (which includes the 16-byte GCM tag). This replaces the
//! legacy plaintext `token\n` first-line auth and eliminates the transport
//! desync bug caused by BufReader consuming frame bytes during auth read.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::rpc_types::Frame;

/// Re-export of Frame as TransportFrame for the transport layer.
pub type TransportFrame = Frame;

/// Maximum frame payload size (16 MB).
pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Frame header size in bytes (4-byte big-endian length).
pub const FRAME_HEADER_SIZE: usize = 4;

/// Validate that a frame payload does not exceed the maximum size.
pub fn validate_frame_size(data: &[u8]) -> Result<(), String> {
    if data.len() > MAX_FRAME_SIZE {
        return Err(format!(
            "Frame payload too large: {} bytes (max {} bytes)",
            data.len(),
            MAX_FRAME_SIZE
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Synchronous helpers
// ---------------------------------------------------------------------------

/// Encode multiple frames into a single buffer (for batch sending).
pub fn encode_batch(frames: &[Frame]) -> Vec<u8> {
    let mut buf = Vec::new();
    for frame in frames {
        buf.extend_from_slice(&frame.encode());
    }
    buf
}

/// Decode all complete frames from a buffer.
/// Returns a vector of decoded frames and the number of bytes consumed.
pub fn decode_all(buf: &[u8]) -> (Vec<Frame>, usize) {
    let mut frames = Vec::new();
    let mut offset = 0;

    while offset < buf.len() {
        if let Some((frame, consumed)) = Frame::decode(&buf[offset..]) {
            frames.push(frame);
            offset += consumed;
        } else {
            break;
        }
    }

    (frames, offset)
}

/// Write a single frame synchronously to a writer.
pub fn write_frame<W: std::io::Write>(writer: &mut W, data: &[u8]) -> std::io::Result<()> {
    validate_frame_size(data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = data.len() as u32;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(data)?;
    writer.flush()?;
    Ok(())
}

/// Read a single frame synchronously from a reader.
pub fn read_frame<R: std::io::Read>(reader: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; FRAME_HEADER_SIZE];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Frame too large: {} bytes", len),
        ));
    }
    let mut data = vec![0u8; len];
    reader.read_exact(&mut data)?;
    Ok(data)
}

// ---------------------------------------------------------------------------
// Async helpers
// ---------------------------------------------------------------------------

/// Asynchronous frame reader for streaming frame consumption.
///
/// Wraps an async reader with buffering for efficient frame-by-frame reading.
/// Equivalent to Go's `FrameReader`.
pub struct AsyncFrameReader<R> {
    reader: BufReader<R>,
}

impl<R: AsyncRead + Unpin> AsyncFrameReader<R> {
    /// Create a new async frame reader with default buffer capacity (8 KB).
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
        }
    }

    /// Create a new async frame reader with the specified buffer capacity.
    pub fn with_capacity(reader: R, capacity: usize) -> Self {
        Self {
            reader: BufReader::with_capacity(capacity, reader),
        }
    }

    /// Read the next frame from the underlying reader.
    ///
    /// Reads the 4-byte length header, then reads the payload.
    /// Returns an error if the frame exceeds `MAX_FRAME_SIZE`.
    pub async fn read_frame(&mut self) -> std::io::Result<Vec<u8>> {
        let mut len_buf = [0u8; FRAME_HEADER_SIZE];
        self.reader.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > MAX_FRAME_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Frame too large: {} bytes (max {} bytes)",
                    len, MAX_FRAME_SIZE
                ),
            ));
        }
        let mut data = vec![0u8; len];
        self.reader.read_exact(&mut data).await?;
        Ok(data)
    }

    /// Consume the inner reader, returning it.
    pub fn into_inner(self) -> BufReader<R> {
        self.reader
    }
}

/// Write a single frame asynchronously to a writer.
///
/// Equivalent to Go's `WriteFrame`.
pub async fn write_frame_async<W: AsyncWrite + Unpin>(
    writer: &mut W,
    data: &[u8],
) -> std::io::Result<()> {
    validate_frame_size(data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = data.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(data).await?;
    writer.flush().await?;
    Ok(())
}

// ===========================================================================
// AEAD encryption (AES-256-GCM)
// ===========================================================================

/// Size of the AES-256 key in bytes.
pub const AES_KEY_SIZE: usize = 32;
/// Size of the GCM nonce in bytes.
pub const NONCE_SIZE: usize = 12;
/// Size of the GCM authentication tag in bytes.
pub const TAG_SIZE: usize = 16;

/// Derive a 32-byte AES-256 key from an auth token string using SHA-256.
///
/// The token is hashed once; the digest serves as the symmetric key shared
/// by both ends of the connection (pre-shared key model).
pub fn derive_key(auth_token: &str) -> [u8; AES_KEY_SIZE] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(auth_token.as_bytes());
    let mut key = [0u8; AES_KEY_SIZE];
    key.copy_from_slice(&digest);
    key
}

/// Encrypt a plaintext payload with AES-256-GCM.
///
/// Returns a byte vector laid out as `[nonce (12 bytes)][ciphertext + tag]`.
/// A fresh random nonce is generated for each call; the tag is appended to
/// the ciphertext by the `aes-gcm` crate.
pub fn encrypt_frame(payload: &[u8], key: &[u8; AES_KEY_SIZE]) -> Result<Vec<u8>, String> {
    use aes_gcm::aead::{Aead, KeyInit, OsRng};
    use aes_gcm::{AeadCore, Aes256Gcm, Key};

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, payload)
        .map_err(|e| format!("AES-GCM encrypt failed: {}", e))?;

    let mut out = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt data produced by [`encrypt_frame`].
///
/// Input layout: `[nonce (12 bytes)][ciphertext + tag]`. Returns the
/// original plaintext payload. Fails if the data is too short or the GCM
/// authentication tag does not verify (wrong key or tampered data).
pub fn decrypt_frame(data: &[u8], key: &[u8; AES_KEY_SIZE]) -> Result<Vec<u8>, String> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Key, Nonce};

    if data.len() < NONCE_SIZE + TAG_SIZE {
        return Err(format!(
            "encrypted frame too short: {} bytes (need at least {})",
            data.len(),
            NONCE_SIZE + TAG_SIZE
        ));
    }

    let (nonce_bytes, ciphertext) = data.split_at(NONCE_SIZE);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|e| format!("AES-GCM decrypt failed (auth error or corrupted): {}", e))
}

#[cfg(test)]
mod tests;
