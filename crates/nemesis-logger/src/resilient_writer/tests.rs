//! ResilientRollingWriter 活跃守卫接线回归（BUG-20261126-112 横向面）。
//!
//! gateway 主日志 writer（Gateway 族 14d TTL 清扫对象）的句柄跨写常驻——
//! 守卫必须在册（清扫删除前跳过）、writer 退场注销（恢复可清扫）。日期
//! 轮转 / 写失败自愈两处注销点依赖 [`State::set_file`]/[`State::clear_file`]
//! 单点收敛（结构性防漂移），本文件锁「打开 ⇒ 在册 / 退场 ⇒ 注销」主链。

use super::*;
use std::io::Write as _;
use std::time::Duration;

#[test]
fn rolling_writer_registers_active_guard_while_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = ResilientRollingWriter::with_retry_interval(
        dir.path(),
        "gateway",
        Duration::from_millis(50),
    );
    w.write_all(b"{\"line\":1}\n").unwrap();
    let today = Local::now().date_naive();
    let path = dir
        .path()
        .join(format!("gateway.{}", today.format("%Y-%m-%d")));
    assert!(path.exists(), "首轮写入打开当日文件");
    assert!(
        nemesis_utils::active_files::is_active(&path),
        "句柄开着 = 路径在册（Gateway 族 TTL 清扫删除前跳过的依据）"
    );

    // writer 退场：State drop → 守卫注销 → 恢复可清扫。
    drop(w);
    assert!(
        !nemesis_utils::active_files::is_active(&path),
        "writer 退场守卫必须注销"
    );
}
