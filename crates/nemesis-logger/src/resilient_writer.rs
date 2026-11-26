//! BUG-2（2026-11-25 远程 home 取证）：带自恢复的按天滚动日志 writer。
//!
//! 背景：gateway 主日志 writer 原为 tracing-appender [`RollingFileAppender`]，
//! 其内部 writer 私有且**写失败完全静默**——一次 I/O 故障（如 11-25 20:46
//! 一行日志写到半截后物理截断）后管线永久静默，进程活着但日志观测全盲。
//! 本 writer 复刻其文件形态（`{prefix}.{YYYY-MM-DD}`，本地时区按天轮转，
//! append 模式），并补上三件缺失的自愈语义：
//!
//! 1. **失败不固化**：写失败即关闭句柄，下次写入重试打开（reopen 追加）；
//!    连续失败达阈值转入**降级态**（不再每条重试，按 `retry_interval` 探活），
//!    避免持续故障时每条日志都吃一次 I/O 超时拖慢主流程。
//! 2. **半行不粘连**：失败可能发生在行中间（write_all 部分成功后报错），
//!    留下半行；恢复路径 reopen 时检测文件末尾缺换行即补一个 `\n`，
//!    保证后续每条日志都是完整行。
//! 3. **降级可观测**：失败期间限频向 stderr 告警（不能走 tracing——
//!    file layer 自身就在 tracing 管线里，会递归）；恢复后向日志文件写一条
//!    与 [`crate::SseLogEvent`] 同 schema 的 WARN 行，报告中断时长与丢弃量，
//!    Dashboard 历史加载直接可见。
//!
//! 锁纪律：`parking_lot::Mutex` + `unwrap_or_else(|e| e.into_inner())` 兜底
//! （锁毒化不永久死，与 BUG-8 修复同款纪律）。

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate};
use tracing_subscriber::fmt::MakeWriter;

/// 降级态下的恢复探活间隔（默认 60s；测试可调小）。
pub const DEFAULT_RETRY_INTERVAL: Duration = Duration::from_secs(60);
/// 连续写失败达到该次数进入降级态（此前每条都重试 open——短暂抖动零丢失）。
pub const DEGRADE_AFTER_CONSECUTIVE: u32 = 3;
/// 失败 WARN 限频间隔。
const WARN_THROTTLE: Duration = Duration::from_secs(300);

struct State {
    file: Option<std::fs::File>,
    /// 当前打开文件对应的日期（None = 未打开；下次写入必先 open）。
    open_date: Option<NaiveDate>,
    /// 连续写失败计数（成功即清零）。
    consecutive_fails: u32,
    /// 降级起始时刻（None = 正常态）。
    degraded_since: Option<Instant>,
    /// 降级态下下次恢复探活时刻。
    next_retry: Instant,
    /// 上次失败 WARN 时刻（限频）。
    last_warn: Option<Instant>,
    /// 失败/降级期间丢弃的写入调用数（≈事件条数，恢复标记行上报）。
    dropped: u64,
}

/// 带自恢复的按天滚动日志 writer。实现 [`MakeWriter`]，可直接喂给
/// `tracing_subscriber::fmt::layer().with_writer(...)`。
pub struct ResilientRollingWriter {
    dir: PathBuf,
    prefix: String,
    retry_interval: Duration,
    state: Mutex<State>,
}

impl ResilientRollingWriter {
    /// 装配到 `dir` 目录、文件名前缀 `prefix`（与
    /// `RollingFileAppender::new(Rotation::DAILY, dir, prefix)` 的文件形态一致：
    /// `{prefix}.{YYYY-MM-DD}`）。`dir` 的存在性由调用方保证（原装配点已有
    /// create_dir_all + 降级 console-only 分支）。
    pub fn new(dir: impl Into<PathBuf>, prefix: impl Into<String>) -> Self {
        Self::with_retry_interval(dir, prefix, DEFAULT_RETRY_INTERVAL)
    }

    /// 测试/特殊场景可调探活间隔的构造形态。
    pub fn with_retry_interval(
        dir: impl Into<PathBuf>,
        prefix: impl Into<String>,
        retry_interval: Duration,
    ) -> Self {
        Self {
            dir: dir.into(),
            prefix: prefix.into(),
            retry_interval,
            state: Mutex::new(State {
                file: None,
                open_date: None,
                consecutive_fails: 0,
                degraded_since: None,
                next_retry: Instant::now(),
                last_warn: None,
                dropped: 0,
            }),
        }
    }

    fn file_path_for(&self, date: NaiveDate) -> PathBuf {
        self.dir
            .join(format!("{}.{}", self.prefix, date.format("%Y-%m-%d")))
    }

    /// 打开（或轮转到）`date` 对应的文件，并修复可能残留的半行（上一
    /// writer 会话写失败留下的无换行尾巴——补 `\n` 保证本条是完整行）。
    fn open_and_repair(&self, date: NaiveDate) -> io::Result<std::fs::File> {
        let path = self.file_path_for(date);
        // read(true) 与 append 并申请：append-only 句柄在 Windows 上无
        // FILE_READ_DATA 权限，下面的末字节探测会静默失败（半行修复失效）。
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)?;
        // 半行修复：append 模式下文件游标已在末尾；读末字节判断是否缺换行。
        let end = f.seek(SeekFrom::End(0))?;
        if end > 0 {
            f.seek(SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            if f.read_exact(&mut last).is_ok() && last[0] != b'\n' {
                let _ = f.write_all(b"\n");
            }
            f.seek(SeekFrom::End(0))?;
        }
        Ok(f)
    }

    fn warn_throttled(&self, st: &mut State, msg: &str) {
        let now = Instant::now();
        let should = st
            .last_warn
            .is_none_or(|t| now.duration_since(t) >= WARN_THROTTLE);
        if should {
            // stderr 直写：本 writer 就在 tracing 管线内，tracing WARN 会递归。
            eprintln!("[ResilientLogWriter] {msg}");
            st.last_warn = Some(now);
        }
    }

    /// 写入一个完整事件行（fmt layer 每事件一次 `write_all` 调用）。失败
    /// 不上抛（fmt layer 忽略 io 错误，返回 Err 也无人看见）——全部语义
    /// 都在本函数内：丢弃计数 + 限频告警 + 降级探活 + 恢复标记。
    fn write_event(&self, buf: &[u8]) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let today = Local::now().date_naive();

        // 降级态：不到探活时刻直接丢弃（可观测 + 不拖慢主流程）。
        if st.degraded_since.is_some() {
            let now = Instant::now();
            if now < st.next_retry {
                st.dropped += 1;
                return;
            }
            match self.open_and_repair(today) {
                Ok(f) => {
                    let since = st.degraded_since.take().expect("checked above");
                    let dropped = st.dropped;
                    st.file = Some(f);
                    st.open_date = Some(today);
                    st.consecutive_fails = 0;
                    st.dropped = 0;
                    // 恢复标记行：SseLogEvent 同 schema 的 WARN，Dashboard
                    // 历史加载可见「日志管线曾中断 X 秒、丢弃 Y 条」。
                    let marker = serde_json::json!({
                        "seq": crate::sse_layer::next_seq(),
                        "level": "WARN",
                        "timestamp": Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
                        "target": "nemesis_logger::resilient_writer",
                        "source": "general",
                        "component": "nemesis-logger",
                        "file": "",
                        "line": 0,
                        "message": format!(
                            "日志管线恢复：曾中断 {} 秒，期间丢弃约 {} 条日志记录（writer={}/{}）",
                            since.elapsed().as_secs(),
                            dropped,
                            self.dir.display(),
                            self.prefix
                        ),
                        "fields": {},
                    });
                    if let Some(f) = st.file.as_mut() {
                        let _ = f.write_all(format!("{marker}\n").as_bytes());
                    }
                }
                Err(e) => {
                    st.next_retry = Instant::now() + self.retry_interval;
                    st.dropped += 1;
                    let st: &mut State = &mut st;
                    self.warn_throttled(
                        st,
                        &format!("日志文件仍不可用（降级中，已丢弃 {} 条）：{e}", st.dropped),
                    );
                    return;
                }
            }
        }

        // 正常态：日期轮转检查（首次打开 / 跨天）。
        if st.open_date != Some(today) {
            st.file = None;
            match self.open_and_repair(today) {
                Ok(f) => {
                    st.file = Some(f);
                    st.open_date = Some(today);
                }
                Err(e) => {
                    self.record_failure(&mut st, today, &e);
                    return;
                }
            }
        }

        let Some(f) = st.file.as_mut() else {
            return;
        };
        match f.write_all(buf) {
            Ok(()) => {
                st.consecutive_fails = 0;
            }
            Err(e) => {
                // 失败即关句柄：下次写入强制 reopen（自愈入口）。
                st.file = None;
                st.open_date = None;
                self.record_failure(&mut st, today, &e);
            }
        }
    }

    /// 失败记账：计数、限频告警、达阈值进降级态（含摘要）。
    fn record_failure(&self, st: &mut State, today: NaiveDate, e: &io::Error) {
        st.consecutive_fails += 1;
        st.dropped += 1;
        let path = self.file_path_for(today);
        if st.consecutive_fails >= DEGRADE_AFTER_CONSECUTIVE && st.degraded_since.is_none() {
            st.degraded_since = Some(Instant::now());
            st.next_retry = Instant::now() + self.retry_interval;
            // 降级转换立即告警（不走限频——状态翻转必须可见）。
            eprintln!(
                "[ResilientLogWriter] 日志写入连续失败 {} 次，转降级（每 {} 秒探活恢复，期间日志丢弃）：{e}（file={}）",
                st.consecutive_fails,
                self.retry_interval.as_secs(),
                path.display()
            );
            st.last_warn = Some(Instant::now());
            return;
        }
        self.warn_throttled(
            st,
            &format!(
                "日志文件写入失败（连续 {} 次）：{e}（file={}）",
                st.consecutive_fails,
                path.display()
            ),
        );
    }
}

/// [`MakeWriter`] 借用 guard：write 时拿锁走 [`ResilientRollingWriter::write_event`]。
pub struct Guard<'a> {
    w: &'a ResilientRollingWriter,
}

impl<'a> MakeWriter<'a> for ResilientRollingWriter {
    type Writer = Guard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        Guard { w: self }
    }
}

impl Write for Guard<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.w.write_event(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 直接写入入口（writer 本体也是 `io::Write`）——测试与工具脚本用，
/// 不经 MakeWriter guard。
impl Write for ResilientRollingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_event(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
