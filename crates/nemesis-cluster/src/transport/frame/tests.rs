use super::*;

#[test]
fn test_validate_frame_size_ok() {
    let data = vec![0u8; 1024];
    assert!(validate_frame_size(&data).is_ok());
}

#[test]
fn test_validate_frame_size_too_large() {
    let data = vec![0u8; MAX_FRAME_SIZE + 1];
    assert!(validate_frame_size(&data).is_err());
}

#[test]
fn test_encode_decode_batch() {
    let frames = vec![
        Frame::new(b"frame-1".to_vec()),
        Frame::new(b"frame-2".to_vec()),
        Frame::new(b"frame-3".to_vec()),
    ];

    let encoded = encode_batch(&frames);
    let (decoded, consumed) = decode_all(&encoded);

    assert_eq!(decoded.len(), 3);
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded[0].data, b"frame-1");
    assert_eq!(decoded[1].data, b"frame-2");
    assert_eq!(decoded[2].data, b"frame-3");
}

#[test]
fn test_decode_partial() {
    let frame = Frame::new(b"partial".to_vec());
    let encoded = frame.encode();

    // Only first half
    let (decoded, _) = decode_all(&encoded[..encoded.len() / 2]);
    assert!(decoded.is_empty());
}

#[test]
fn test_sync_write_read_frame() {
    use std::io::Cursor;

    let data = b"hello world";
    let mut buf = Cursor::new(Vec::new());
    write_frame(&mut buf, data).unwrap();

    buf.set_position(0);
    let read = read_frame(&mut buf).unwrap();
    assert_eq!(read, data);
}

#[tokio::test]
async fn test_async_frame_reader() {
    // Build a framed payload
    let payload = b"async frame data";
    let mut encoded = Vec::new();
    let len = payload.len() as u32;
    encoded.extend_from_slice(&len.to_be_bytes());
    encoded.extend_from_slice(payload);

    let cursor = std::io::Cursor::new(encoded);
    let mut reader = AsyncFrameReader::new(cursor);
    let data = reader.read_frame().await.unwrap();
    assert_eq!(data, payload);
}

#[tokio::test]
async fn test_async_write_frame() {
    let payload = b"async write test";
    let mut buf = Vec::new();
    write_frame_async(&mut buf, payload).await.unwrap();

    // Verify we can read it back
    let cursor = std::io::Cursor::new(buf);
    let mut reader = AsyncFrameReader::new(cursor);
    let data = reader.read_frame().await.unwrap();
    assert_eq!(data, payload);
}

#[tokio::test]
async fn test_async_frame_reader_multiple_frames() {
    let mut encoded = Vec::new();

    for i in 0..5u8 {
        let payload = vec![i; 64];
        let len = payload.len() as u32;
        encoded.extend_from_slice(&len.to_be_bytes());
        encoded.extend_from_slice(&payload);
    }

    let cursor = std::io::Cursor::new(encoded);
    let mut reader = AsyncFrameReader::new(cursor);

    for i in 0..5u8 {
        let data = reader.read_frame().await.unwrap();
        assert_eq!(data, vec![i; 64]);
    }
}
