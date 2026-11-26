//! BUG-2（2026-11-25 远程 home 取证）回归：gateway 日志 writer 写失败曾
//! 全静默且无恢复（tracing-appender 内部 writer 私有）——一次 I/O 故障
//! （11-25 20:46 半行截断现场）后管线永久静默，进程活着但观测全盲。
//! ResilientRollingWriter 的修复语义在此钉死：
//! 1. 文件形态与 RollingFileAppender 一致（`{prefix}.{YYYY-MM-DD}`）；
//! 2. 写失败自动 reopen（占位移除后下一条即恢复落盘）；
//! 3. 半行尾巴（上一会话写失败残留）恢复时补换行，后续行完整；
//! 4. 连续失败转降级（探活节奏可调），恢复后写中断标记行（SseLogEvent
//!    同 schema WARN，报中断时长与丢弃量）；
//! 5. 健康路径零改动（正常写、跨天轮转文件名）。

use nemesis_logger::ResilientRollingWriter;
use std::io::Write;
use std::time::Duration;

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn file_content(dir: &std::path::Path, prefix: &str) -> String {
    let path = dir.join(format!("{}.{}", prefix, today()));
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// 正常路径：写入即落盘，文件名形态 = `{prefix}.{YYYY-MM-DD}`（与
/// RollingFileAppender::new(DAILY, dir, prefix) 完全一致——Dashboard 正则
/// `^{stem}\.\d{4}-\d{2}-\d{2}$` 依赖此形态）。
#[test]
fn test_normal_write_and_filename_shape() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut w = ResilientRollingWriter::with_retry_interval(
        tmp.path(),
        "nemesisbot",
        Duration::from_millis(100),
    );
    // 直接通过 Write trait 写（绕开 tracing subscriber，聚焦 writer 本体）。
    w.write_all(b"{\"level\":\"INFO\",\"message\":\"hello\"}\n")
        .expect("write ok");
    let content = file_content(tmp.path(), "nemesisbot");
    assert!(
        content.contains("\"message\":\"hello\""),
        "content must land: {content:?}"
    );
    assert!(content.ends_with('\n'), "line must end with newline");
}

/// 写失败不固化：文件路径被目录占位（open 必败）期间写入被诚实丢弃
/// （不 panic、不误报成功之外的崩溃），占位移除后下一条写入自动恢复落盘。
#[test]
fn test_write_failure_recovers_after_blocker_removed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut w = ResilientRollingWriter::with_retry_interval(
        tmp.path(),
        "nemesisbot",
        Duration::from_millis(80),
    );
    let blocker = tmp.path().join(format!("nemesisbot.{}", today()));
    std::fs::create_dir(&blocker).expect("placeholder dir");

    // 占位期间：写 5 条（>= DEGRADE_AFTER_CONSECUTIVE=3 → 进降级态），
    // 全部诚实丢弃，不 panic。
    for i in 0..5 {
        w.write_all(format!("{{\"m\":\"lost{}\"}}\n", i).as_bytes())
            .expect("write during outage must not error");
    }
    assert!(
        file_content(tmp.path(), "nemesisbot").is_empty(),
        "nothing must land while blocked"
    );

    // 占位移除 → 因处降级态，按探活节奏（80ms）恢复。写一条 → 可能还在
    // 探活窗口内被丢弃；等一个探活周期再写 → 必落盘 + 恢复标记行在前。
    std::fs::remove_dir(&blocker).expect("blocker removed");
    w.write_all(b"{\"m\":\"probe\"}\n").expect("probe ok");
    std::thread::sleep(Duration::from_millis(150));
    w.write_all(b"{\"m\":\"recovered-line\"}\n")
        .expect("write after recovery");
    let content = file_content(tmp.path(), "nemesisbot");
    assert!(
        content.contains("\"recovered-line\""),
        "must land after recovery: {content:?}"
    );
    assert!(
        content.contains("resilient_writer"),
        "recovery marker line must be written: {content:?}"
    );
    assert!(
        content.contains("日志管线恢复"),
        "marker must report recovery: {content:?}"
    );
}

/// 半行修复：上一会话写失败留下的无换行尾巴，恢复写入时先补 `\n`，
/// 保证新行完整独立（不与残尾粘连——11-25 20:46 半行现场的后遗症防线）。
#[test]
fn test_partial_line_tail_gets_newline_on_reopen() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join(format!("nemesisbot.{}", today()));
    std::fs::write(&path, "{\"m\":\"truncat").expect("partial tail seeded");

    let mut w = ResilientRollingWriter::with_retry_interval(
        tmp.path(),
        "nemesisbot",
        Duration::from_millis(100),
    );
    w.write_all(b"{\"m\":\"next\"}\n").expect("write ok");

    let content = file_content(tmp.path(), "nemesisbot");
    assert_eq!(
        content, "{\"m\":\"truncat\n{\"m\":\"next\"}\n",
        "partial tail must be newline-terminated before the new line: {content:?}"
    );
}

/// 跨天轮转形态：date 前缀直接由 file_path_for 的实现保证（与 today 一致
/// 已在其余用例隐式钉住）；此处补一个「恢复标记行是合法 SseLogEvent 形态」
/// 的 schema 检查——Dashboard 历史加载按行 serde 解析，字段必须齐。
#[test]
fn test_recovery_marker_line_is_valid_json_with_required_fields() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut w = ResilientRollingWriter::with_retry_interval(
        tmp.path(),
        "nemesisbot",
        Duration::from_millis(50),
    );
    let blocker = tmp.path().join(format!("nemesisbot.{}", today()));
    std::fs::create_dir(&blocker).expect("placeholder dir");
    for i in 0..4 {
        w.write_all(format!("{{\"m\":\"lost{}\"}}\n", i).as_bytes())
            .expect("outage writes ok");
    }
    std::fs::remove_dir(&blocker).expect("blocker removed");
    std::thread::sleep(Duration::from_millis(80));
    w.write_all(b"{\"m\":\"recovered-line\"}\n")
        .expect("write ok");

    let content = file_content(tmp.path(), "nemesisbot");
    let marker_line = content
        .lines()
        .find(|l| l.contains("resilient_writer"))
        .expect("marker line present");
    let v: serde_json::Value =
        serde_json::from_str(marker_line).expect("marker must be valid JSON");
    for key in ["seq", "level", "timestamp", "target", "source", "message"] {
        assert!(
            v.get(key).is_some(),
            "marker must carry SseLogEvent field `{key}`: {v}"
        );
    }
    assert_eq!(v["level"], "WARN");
    let msg = v["message"].as_str().unwrap();
    assert!(
        msg.contains("丢弃约 4 条") || msg.contains("丢弃约 5 条"),
        "dropped count must be reported: {msg}"
    );
}
