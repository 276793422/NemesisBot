//! activity-log 示例插件（guest 侧，world `plugin-observer`）。
//!
//! 三方开发者照这里写观察者插件：实现 [`Guest::observe`] 单入口，
//! `export!` 收尾。本插件把每条事件投影追加进 /data/events.log
//! （宿主 per-plugin preopen 数据目录）并打一条宿主日志——演示观察者
//! 两个合法能力通道（log / data 目录持久化）。
//!
//! 对抗夹具行为：事件 JSON 含 `"trap":true` 时故意 panic——验证宿主对
//! observer trap 的失败隔离（事件丢弃、泵存活、后续事件照常投递）。

wit_bindgen::generate!({
    path: "wit",
    world: "plugin-observer",
});

use exports::nemesis::plugin::observer::Guest;
use nemesis::plugin::host::{self, HostError, LogLevel};

struct ActivityLogPlugin;

impl Guest for ActivityLogPlugin {
    fn observe(event: String) -> Result<(), HostError> {
        if event.contains("\"trap\":true") {
            panic!("deliberate observer trap fixture");
        }
        host::log(LogLevel::Info, &format!("event received: {event}"));
        use std::io::Write;
        let log_path = format!("{}/events.log", host::data_dir_path()?);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = writeln!(f, "{event}");
        }
        Ok(())
    }
}

export!(ActivityLogPlugin);
