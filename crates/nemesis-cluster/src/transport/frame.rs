//! Transport frame - length-prefixed binary message framing.
//!
//! Provides both synchronous and asynchronous frame encoding/decoding.
//! Frames use a 4-byte big-endian length header followed by the payload.

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
    validate_frame_size(data).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
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
                format!("Frame too large: {} bytes (max {} bytes)", len, MAX_FRAME_SIZE),
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
    validate_frame_size(data).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = data.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(data).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
