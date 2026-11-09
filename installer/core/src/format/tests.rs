use super::*;
use anyhow::Result;

fn sample_footer() -> Footer {
    Footer {
        manifest_offset: 1000,
        manifest_len: 500,
        manifest_sha256: [0xab; 32],
    }
}

#[test]
fn footer_encode_decode_roundtrip() -> Result<()> {
    let f = sample_footer();
    let bytes = f.encode();
    assert_eq!(bytes.len(), FOOTER_SIZE);
    assert_eq!(&bytes[..8], &FOOTER_MAGIC);
    assert_eq!(Footer::decode(&bytes)?, f);
    Ok(())
}

#[test]
fn footer_decode_rejects_bad_magic_and_short_buf() {
    let mut bytes = sample_footer().encode();
    bytes[0] = b'X';
    assert!(Footer::decode(&bytes).is_err());
    assert!(Footer::decode(&bytes[..40]).is_err());
}

#[test]
fn locate_finds_footer_at_eof() -> Result<()> {
    // 骨架 + manifest blob + 尾注。
    let blob = vec![0u8; 500];
    let footer = sample_footer();
    let mut file = vec![0x90u8; 1000];
    file.extend_from_slice(&blob);
    file.extend_from_slice(&footer.encode());
    let file_len = file.len() as u64;
    let (got, pos) = locate_footer(file_len, &file)?;
    assert_eq!(got, footer);
    assert_eq!(pos, file_len - FOOTER_SIZE as u64);
    Ok(())
}

#[test]
fn locate_ignores_magic_collision_in_payload() -> Result<()> {
    // 压缩数据里埋一个 magic（假尾注，位置核验必败）+ 真 magic（有效）。
    // 伪造 magic 携带的 offset+len 指向别处——交叉核验把它排除。
    let mut file = vec![0u8; 200];
    file.extend_from_slice(&FOOTER_MAGIC); // 碰撞 @200
    file.extend_from_slice(&7u64.to_le_bytes()); // offset=7
    file.extend_from_slice(&7u64.to_le_bytes()); // len=7（7+7=14 ≠ 200）
    file.extend_from_slice(&[0x01; 32]);
    // 真 blob + 尾注 @EOF（blob 起点 = 200 头 + 56 碰撞块 = 256）。
    let blob = vec![0x33u8; 300];
    let real = Footer {
        manifest_offset: 256,
        manifest_len: 300,
        manifest_sha256: [0x11; 32],
    };
    file.extend_from_slice(&blob);
    file.extend_from_slice(&real.encode());
    let (got, pos) = locate_footer(file.len() as u64, &file)?;
    assert_eq!(got, real, "必须取最靠近 EOF 且过核验的尾注");
    assert_eq!(pos, file.len() as u64 - FOOTER_SIZE as u64);
    Ok(())
}

#[test]
fn locate_rejects_truncated_and_missing() -> Result<()> {
    // 无 magic。
    let file = vec![0u8; 4096];
    assert!(locate_footer(file.len() as u64, &file).is_err());

    // magic 距 EOF 不足 56B（被截断）。
    let mut file2 = vec![0u8; 100];
    file2.extend_from_slice(&FOOTER_MAGIC);
    assert!(locate_footer(file2.len() as u64, &file2).is_err());

    // 窗口大于文件长。
    assert!(locate_footer(10, &file).is_err());

    // 位置核验失败：offset+len ≠ footer 位置。
    let bad = Footer {
        manifest_offset: 0,
        manifest_len: 7,
        manifest_sha256: [0; 32],
    };
    let mut file3 = vec![0u8; 64];
    file3.extend_from_slice(&bad.encode());
    assert!(locate_footer(file3.len() as u64, &file3).is_err());
    Ok(())
}

#[test]
fn footer_size_arithmetic() {
    // 56 = 8 + 8 + 8 + 32；任何漂移都是格式破坏（对端解码立即失配）。
    assert_eq!(FOOTER_SIZE, 56);
}
