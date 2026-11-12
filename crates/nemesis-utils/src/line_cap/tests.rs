use super::read_line_capped;
use std::io::Cursor as StdCursor;

/// 读完整条流，返回全部 (行字节, 是否截断)。
async fn capped_lines(input: &[u8], cap: usize) -> Vec<(Vec<u8>, bool)> {
    let mut reader = tokio::io::BufReader::new(StdCursor::new(input));
    let mut out = Vec::new();
    while let Some(pair) = read_line_capped(&mut reader, cap).await.unwrap() {
        out.push(pair);
    }
    out
}

#[tokio::test]
async fn normal_lines_then_eof() {
    let lines = capped_lines(b"hello\nworld\n", 1024).await;
    assert_eq!(
        lines,
        vec![(b"hello\n".to_vec(), false), (b"world\n".to_vec(), false)]
    );
}

#[tokio::test]
async fn cap_boundary_line_exactly_cap_is_not_truncated() {
    // 行字节恰好等于帽（8 字节含 \n）→ 不截（判定是严格超 > cap）。
    let lines = capped_lines(b"1234567\n", 8).await;
    assert_eq!(lines, vec![(b"1234567\n".to_vec(), false)]);
}

#[tokio::test]
async fn truncates_poison_line_and_cursor_aligns_next_line() {
    // 毒丸行超帽 → 前 cap 字节 + truncated=true，剩余丢弃；
    // 游标落在下一行行首，后续行不受毒丸行影响。
    let mut input = b"first\n".to_vec();
    input.extend_from_slice(b"0123456789ABCDEFGHIJKLMNOP\n"); // 27 字节毒丸
    input.extend_from_slice(b"after\n");
    let lines = capped_lines(&input, 8).await;
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0], (b"first\n".to_vec(), false));
    assert_eq!(lines[1], (b"01234567".to_vec(), true));
    assert_eq!(lines[2], (b"after\n".to_vec(), false));
}

#[tokio::test]
async fn accumulates_across_fill_buf_chunks() {
    // 帽内行跨多个 fill_buf 片段（BufReader 容量 3）拼装——分块不劈语义。
    let mut reader =
        tokio::io::BufReader::with_capacity(3, StdCursor::new(&b"abcdef\nafter\n"[..]));
    let line = read_line_capped(&mut reader, 1024).await.unwrap().unwrap();
    assert_eq!(line, (b"abcdef\n".to_vec(), false));
    let line2 = read_line_capped(&mut reader, 1024).await.unwrap().unwrap();
    assert_eq!(line2, (b"after\n".to_vec(), false));
    assert!(read_line_capped(&mut reader, 1024).await.unwrap().is_none());
}

#[tokio::test]
async fn truncation_across_chunks_drops_rest_of_line() {
    // 毒丸行跨片段：帽触发在无 \n 的片段（None 臂）→ 进丢弃模式，
    // 消费到行尾/EOF 为止，下一行照常。
    let mut reader =
        tokio::io::BufReader::with_capacity(3, StdCursor::new(&b"abcdefghij\nnext\n"[..]));
    let line = read_line_capped(&mut reader, 5).await.unwrap().unwrap();
    // "abc"（≤帽积累）→ "def"（累计 6 > 5 触发截断，keep=2）→
    // "ghi"（丢弃模式）→ "j\n"（消费到行尾，返回截断行）。
    assert_eq!(line, (b"abcde".to_vec(), true));
    let next = read_line_capped(&mut reader, 1024).await.unwrap().unwrap();
    assert_eq!(next, (b"next\n".to_vec(), false));
    assert!(read_line_capped(&mut reader, 1024).await.unwrap().is_none());
}

#[tokio::test]
async fn no_trailing_newline_returns_line_then_eof() {
    // 无换行尾行：read_line 同语义 Ok(n>0)；再读 = 纯 EOF None。
    let mut reader = tokio::io::BufReader::new(StdCursor::new(&b"tail"[..]));
    let line = read_line_capped(&mut reader, 1024).await.unwrap().unwrap();
    assert_eq!(line, (b"tail".to_vec(), false));
    assert!(read_line_capped(&mut reader, 1024).await.unwrap().is_none());
}

#[tokio::test]
async fn truncated_no_trailing_newline() {
    // 毒丸行无换行直到 EOF：帽后全部丢弃，返回截断标记；EOF 后终止。
    let mut reader = tokio::io::BufReader::new(StdCursor::new(&b"0123456789"[..]));
    let line = read_line_capped(&mut reader, 4).await.unwrap().unwrap();
    assert_eq!(line, (b"0123".to_vec(), true));
    assert!(read_line_capped(&mut reader, 1024).await.unwrap().is_none());
}

#[tokio::test]
async fn multibyte_line_byte_exact_roundtrip() {
    // 多字节 UTF-8 行字节精确往返——字节收齐后调用方一次 lossy
    //（BUG-20261108-027 同族纪律：分块 lossy 会劈字符静默损坏 JSON 值）。
    let mut input = "你好\n".as_bytes().to_vec();
    input.extend_from_slice(b"world\n");
    let lines = capped_lines(&input, 1024).await;
    assert_eq!(
        lines,
        vec![
            ("你好\n".as_bytes().to_vec(), false),
            (b"world\n".to_vec(), false)
        ]
    );
}

// ---------- 同步孪生（read_line_capped_sync；语义镜像 async 版） ----------

use super::read_line_capped_sync;

/// 同步版读完整条流。
fn capped_lines_sync(input: &[u8], cap: usize) -> Vec<(Vec<u8>, bool)> {
    let mut reader = StdCursor::new(input);
    let mut out = Vec::new();
    while let Some(pair) = read_line_capped_sync(&mut reader, cap).unwrap() {
        out.push(pair);
    }
    out
}

#[test]
fn sync_normal_lines_then_eof() {
    let lines = capped_lines_sync(b"hello\nworld\n", 1024);
    assert_eq!(
        lines,
        vec![(b"hello\n".to_vec(), false), (b"world\n".to_vec(), false)]
    );
}

#[test]
fn sync_cap_boundary_exactly_cap_not_truncated() {
    let lines = capped_lines_sync(b"1234567\n", 8);
    assert_eq!(lines, vec![(b"1234567\n".to_vec(), false)]);
}

#[test]
fn sync_truncates_poison_line_and_cursor_aligns_next_line() {
    let lines = capped_lines_sync(b"first\n0123456789ABCDEF\nafter\n", 8);
    assert_eq!(
        lines,
        vec![
            (b"first\n".to_vec(), false),
            (b"01234567".to_vec(), true),
            (b"after\n".to_vec(), false)
        ]
    );
}

#[test]
fn sync_accumulates_across_chunks() {
    let mut buf = std::io::BufReader::with_capacity(3, StdCursor::new(&b"abcdef\nafter\n"[..]));
    let line = read_line_capped_sync(&mut buf, 1024).unwrap().unwrap();
    assert_eq!(line, (b"abcdef\n".to_vec(), false));
    let line2 = read_line_capped_sync(&mut buf, 1024).unwrap().unwrap();
    assert_eq!(line2, (b"after\n".to_vec(), false));
    assert!(read_line_capped_sync(&mut buf, 1024).unwrap().is_none());
}

#[test]
fn sync_truncation_across_chunks_drops_rest() {
    let mut buf = std::io::BufReader::with_capacity(3, StdCursor::new(&b"abcdefghij\nnext\n"[..]));
    let line = read_line_capped_sync(&mut buf, 5).unwrap().unwrap();
    assert_eq!(line, (b"abcde".to_vec(), true));
    let next = read_line_capped_sync(&mut buf, 1024).unwrap().unwrap();
    assert_eq!(next, (b"next\n".to_vec(), false));
    assert!(read_line_capped_sync(&mut buf, 1024).unwrap().is_none());
}

#[test]
fn sync_no_trailing_newline_then_eof() {
    let lines = capped_lines_sync(b"tail", 1024);
    assert_eq!(lines, vec![(b"tail".to_vec(), false)]);
}

#[test]
fn sync_non_utf8_bytes_are_not_an_error() {
    // 非 UTF-8 字节不构成错误——原样返回字节，lossy 由调用方执行
    //（BUG-20261113-135：外部对端 locale 输出不应一行杀死消费循环）。
    let lines = capped_lines_sync(b"ok\n\xff\xfe\xfebytes\nafter\n", 1024);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], (b"ok\n".to_vec(), false));
    assert_eq!(lines[1], (b"\xff\xfe\xfebytes\n".to_vec(), false));
    assert_eq!(lines[2], (b"after\n".to_vec(), false));
}
