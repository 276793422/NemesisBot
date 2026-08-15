
use std::io::Write;
use std::ffi::c_void;

const LOG: &str = r"C:\AI\NemesisBot\NemesisBot_Rust\test-tools\eval-inject-test\test_dll\popup_log.txt";

#[link(name="user32")] extern "system" {
    fn EnumWindows(cb: extern "system" fn(*mut c_void, *mut c_void) -> i32, lparam: *mut c_void) -> i32;
    fn EnumChildWindows(parent: *mut c_void, cb: extern "system" fn(*mut c_void, *mut c_void) -> i32, lparam: *mut c_void) -> i32;
    fn GetClassNameA(h: *mut c_void, buf: *mut u8, len: i32) -> i32;
    fn GetWindowTextA(h: *mut c_void, buf: *mut u8, len: i32) -> i32;
    fn IsWindowVisible(h: *mut c_void) -> i32;
    fn GetWindowThreadProcessId(h: *mut c_void, pid: *mut u32) -> u32;
}

static mut FOUND: Option<std::sync::mpsc::Sender<String>> = None;

thread_local! {
    static CHILD_TEXTS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

extern "system" fn child_cb(h: *mut c_void, _: *mut c_void) -> i32 {
    unsafe {
        let mut class = [0u8; 64];
        GetClassNameA(h, class.as_mut_ptr(), 64);
        let cls = String::from_utf8_lossy(&class[..class.iter().position(|&c|c==0).unwrap_or(0)]);
        if cls == "Static" {
            let mut txt = [0u8; 1024];
            GetWindowTextA(h, txt.as_mut_ptr(), 1024);
            let t = String::from_utf8_lossy(&txt[..txt.iter().position(|&c|c==0).unwrap_or(0)]);
            if !t.is_empty() { CHILD_TEXTS.with(|c| c.borrow_mut().push(t.to_string())); }
        }
    }
    1
}

extern "system" fn enum_cb(h: *mut c_void, _: *mut c_void) -> i32 {
    unsafe {
        if IsWindowVisible(h) == 0 { return 1; }
        let mut class = [0u8; 64];
        GetClassNameA(h, class.as_mut_ptr(), 64);
        let cls = String::from_utf8_lossy(&class[..class.iter().position(|&c|c==0).unwrap_or(0)]);
        if cls != "#32770" { return 1; }
        let mut title = [0u8; 256];
        GetWindowTextA(h, title.as_mut_ptr(), 256);
        let t = String::from_utf8_lossy(&title[..title.iter().position(|&c|c==0).unwrap_or(0)]);
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(h, &mut pid);
        CHILD_TEXTS.with(|c| c.borrow_mut().clear());
        EnumChildWindows(h, child_cb, std::ptr::null_mut());
        let texts = CHILD_TEXTS.with(|c| c.borrow().join(" | "));
        if let Some(tx) = &FOUND {
            let _ = tx.send(format!("DIALOG title=\"{}\" pid={} text=[{}]", t, pid, texts));
        }
    }
    1
}

fn main() {
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(30);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    unsafe { FOUND = Some(tx); }
    let mut log = std::fs::File::create(LOG).expect("log");
    let start = std::time::Instant::now();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    while start.elapsed().as_secs() < secs {
        unsafe { EnumWindows(enum_cb, std::ptr::null_mut()); }
        while let Ok(msg) = rx.try_recv() {
            if seen.insert(msg.clone()) {
                let _ = writeln!(log, "[{:>5}s] {}", start.elapsed().as_secs(), msg);
                let _ = log.flush();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    let _ = writeln!(log, "monitor done, {} unique dialogs", seen.len());
}
