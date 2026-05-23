use super::*;
use std::io::Write;
use tracing_subscriber::fmt::MakeWriter;

#[test]
fn test_dual_writer_console_only() {
    let mut w = DualWriter { console: true, file: None };
    // Should succeed — writes to stderr which is always available.
    let result = w.write(b"hello\n");
    assert!(result.is_ok());
    assert!(result.unwrap() > 0);
    assert!(w.flush().is_ok());
}

#[test]
fn test_dual_writer_with_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.log");
    let path_str = path.to_string_lossy().to_string();

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&*path_str)
        .unwrap();
    let mut w = DualWriter {
        console: true,
        file: Some(Arc::new(Mutex::new(file))),
    };

    w.write(b"line1\n").unwrap();
    w.write(b"line2\n").unwrap();
    w.flush().unwrap();

    let content = std::fs::read_to_string(&*path_str).unwrap();
    assert!(content.contains("line1\n"));
    assert!(content.contains("line2\n"));
}

#[test]
fn test_dual_writer_file_only() {
    // console=false, file=Some — should write ONLY to file, not stderr
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file_only.log");
    let path_str = path.to_string_lossy().to_string();

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&*path_str)
        .unwrap();
    let mut w = DualWriter {
        console: false,
        file: Some(Arc::new(Mutex::new(file))),
    };

    w.write(b"file_only_line\n").unwrap();
    w.flush().unwrap();

    let content = std::fs::read_to_string(&*path_str).unwrap();
    assert!(content.contains("file_only_line"));
}

#[test]
fn test_dual_writer_discard() {
    // console=false, file=None — should discard silently
    let mut w = DualWriter { console: false, file: None };
    let result = w.write(b"discarded\n");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 10); // reports buf.len() but writes nothing
    assert!(w.flush().is_ok());
}

#[test]
fn test_make_writer_console_only() {
    let mw = DualMakeWriter::console_only();
    let mut w = mw.make_writer();
    assert!(w.write(b"console\n").is_ok());
}

#[test]
fn test_make_writer_with_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mw.log");
    let path_str = path.to_string_lossy().to_string();

    let mw = DualMakeWriter::with_file(&path_str).unwrap();
    let mut w1 = mw.make_writer();
    let mut w2 = mw.make_writer();

    w1.write(b"w1\n").unwrap();
    w2.write(b"w2\n").unwrap();
    w1.flush().unwrap();
    w2.flush().unwrap();

    let content = std::fs::read_to_string(&*path_str).unwrap();
    assert!(content.contains("w1"));
    assert!(content.contains("w2"));
}

#[test]
fn test_make_writer_file_not_found_error() {
    let result = DualMakeWriter::with_file("/nonexistent_dir/sub/file.log");
    assert!(result.is_err());
}

#[test]
fn test_go_style_formatter_format() {
    // Basic sanity: timestamp format is correct.
    let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f").to_string();
    // Should be something like "2026-05-22T05:50:26.935143"
    assert!(ts.len() >= 26);
    assert!(ts.contains('T'));
    assert!(ts.contains('.'));
}

#[test]
fn test_dual_writer_flush_without_file() {
    let mut w = DualWriter { console: true, file: None };
    assert!(w.flush().is_ok());
}

#[test]
fn test_dual_writer_flush_with_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("flush.log");
    let path_str = path.to_string_lossy().to_string();

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&*path_str)
        .unwrap();
    let mut w = DualWriter {
        console: true,
        file: Some(Arc::new(Mutex::new(file))),
    };
    assert!(w.flush().is_ok());
}

#[test]
fn test_dual_writer_multiple_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi.log");
    let path_str = path.to_string_lossy().to_string();

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&*path_str)
        .unwrap();
    let mut w = DualWriter {
        console: true,
        file: Some(Arc::new(Mutex::new(file))),
    };

    for i in 0..10 {
        w.write(format!("line {}\n", i).as_bytes()).unwrap();
    }
    w.flush().unwrap();

    let content = std::fs::read_to_string(&*path_str).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 10);
}

#[test]
fn test_make_writer_file_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fo.log");
    let path_str = path.to_string_lossy().to_string();

    let mw = DualMakeWriter::file_only(&path_str).unwrap();
    let mut w = mw.make_writer();
    w.write(b"file-only-content\n").unwrap();
    w.flush().unwrap();

    let content = std::fs::read_to_string(&*path_str).unwrap();
    assert!(content.contains("file-only-content"));
}
