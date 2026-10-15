//! 隐藏验收：请求行带前导空白的 HTTP 请求必须被拒绝（parse 返回 Err），
//! 而规范形态的请求必须完整解析出方法/路径/版本。
use httparse::{Request, Status, EMPTY_HEADER};

#[test]
fn request_line_leading_whitespace_is_rejected() {
    let mut headers = [EMPTY_HEADER; 8];
    let leading_ws = b" GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    // 前导空白是非法形态：必须是 Err（accepting it is the bug）。
    let err = Request::new(&mut headers).parse(leading_ws).unwrap_err();

    let mut headers2 = [EMPTY_HEADER; 8];
    let well_formed = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    let mut req = Request::new(&mut headers2);
    let status = match req.parse(well_formed) {
        Ok(v) => v,
        Err(e) => panic!("well-formed request must still parse, got {e:?}"),
    };
    let consumed = match status {
        Status::Complete(n) => n,
        Status::Partial => panic!("well-formed request must parse completely, got Partial"),
    };
    assert_eq!(req.method, Some("GET"));
    assert_eq!(req.path, Some("/"));
    assert_eq!(req.version, Some(1));
    assert_eq!(consumed, well_formed.len());
    let _ = err;
}
