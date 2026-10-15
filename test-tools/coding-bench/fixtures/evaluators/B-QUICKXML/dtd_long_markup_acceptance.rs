//! 隐藏验收：DOCTYPE 内部子集中紧随 `<!` 的长未知标记声明，经小缓冲
//! BufReader 逐事件读取时不得 panic（报错是合法结局，崩溃不是）。
use quick_xml::{events::Event, Reader};

#[test]
fn long_unknown_dtd_markup_does_not_panic_with_small_buffer() {
    let data = b"<!DOCTYPE r[<aaaaaaaaaaaaaaaaaaaaaaaaaaaaa>]>";
    let mut reader = Reader::from_reader(std::io::BufReader::with_capacity(24, &data[..]));
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(ev) => {
                if matches!(ev, Event::Eof) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}
