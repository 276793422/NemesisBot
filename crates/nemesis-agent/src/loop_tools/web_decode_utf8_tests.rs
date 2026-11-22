//! BUG-20261122-018 回归锁：`percent_decode` / `url_decode_query_param`
//! 的 multibyte UTF-8 整体重组——旧形态逐字节 `byte as char`（Latin-1
//! 映射）把 `中`（%E4%B8%AD）拆成三个独立码位产出 mojibake，web_search
//! 的 uddg 解包对一切非 ASCII 目标 URL 产出乱码。同族先例：oauth.rs
//! url_decode、wechat.rs percent_decode（均已修，本位点为横扫漏网）。

use crate::loop_tools::web::{percent_decode, url_decode_query_param};

#[test]
fn test_percent_decode_utf8() {
    assert_eq!(percent_decode("%E4%B8%AD%E6%96%87"), "中文");
    assert_eq!(
        percent_decode("https%3A%2F%2Fzh.wikipedia.org%2Fwiki%2F%E4%B8%AD"),
        "https://zh.wikipedia.org/wiki/中"
    );
    // ASCII 直通与 `+`→空格行为不变。
    assert_eq!(percent_decode("hello+world"), "hello world");
    assert_eq!(percent_decode("a%26b"), "a&b");
    // 非法 UTF-8 序列 → U+FFFD 诚实降级（不伪造码位）。
    assert_eq!(percent_decode("%FF%FE"), "\u{FFFD}\u{FFFD}");
    // 尾部残缺 % 序列原样保留（旧形态此处会把 %4 解成控制字符 \x04）。
    assert_eq!(percent_decode("a%4"), "a%4");
    assert_eq!(percent_decode("a%"), "a%");
}

#[test]
fn test_url_decode_query_param_utf8() {
    let url = "https://duckduckgo.com/l/?uddg=https%3A%2F%2Fzh.wikipedia.org%2Fwiki%2F%E4%B8%AD%E8%8F%AF%E6%B0%91%E5%9C%8B&rpt=foo";
    assert_eq!(
        url_decode_query_param(url, "uddg"),
        Some("https://zh.wikipedia.org/wiki/中華民國".to_string())
    );
}
