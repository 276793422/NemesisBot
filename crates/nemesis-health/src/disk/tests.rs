//! disk 模块测试（M2 批次一）：evaluate 纯函数全臂 + volume_free_bytes
//! 真实卷冒烟（查询自身工作目录所在卷，只验成功与量级合理性）。

use super::*;
use std::path::PathBuf;

#[test]
fn evaluate_three_levels() {
    let g = 1024 * 1024 * 1024;
    assert_eq!(evaluate(10 * g, 5, 1), WatermarkLevel::Ok);
    assert_eq!(
        evaluate(5 * g, 5, 1),
        WatermarkLevel::Ok,
        "恰等于 warn 阈值=Ok（< 判）"
    );
    assert_eq!(evaluate(5 * g - 1, 5, 1), WatermarkLevel::Warn);
    assert_eq!(evaluate(g, 5, 1), WatermarkLevel::Warn);
    assert_eq!(evaluate(g - 1, 5, 1), WatermarkLevel::Critical);
    assert_eq!(evaluate(0, 5, 1), WatermarkLevel::Critical);
}

#[test]
fn evaluate_inverted_config_takes_stricter_side() {
    let g = 1024 * 1024 * 1024;
    // 配置倒挂（warn=1 < critical=5）：critical 先命中，warn 不吞 critical。
    assert_eq!(evaluate(3 * g, 1, 5), WatermarkLevel::Critical);
    assert_eq!(evaluate(6 * g, 1, 5), WatermarkLevel::Ok);
    // 相等阈值：走 critical 臂（更严口径）。
    assert_eq!(evaluate(4 * g, 5, 5), WatermarkLevel::Critical);
    assert_eq!(evaluate(5 * g, 5, 5), WatermarkLevel::Ok);
}

#[test]
fn level_str_stable() {
    assert_eq!(WatermarkLevel::Ok.as_str(), "ok");
    assert_eq!(WatermarkLevel::Warn.as_str(), "warn");
    assert_eq!(WatermarkLevel::Critical.as_str(), "critical");
}

#[test]
fn volume_free_bytes_reports_real_volume() {
    // 冒烟：查当前工作目录所在卷——只验 API 成功与量级（>0 且 < 1PB）。
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let free = volume_free_bytes(&cwd).expect("查询真实卷剩余空间应成功");
    assert!(free > 0, "free={free}");
    assert!(free < (1u64 << 50), "free={} 应小于 1PB", free);
}

#[test]
fn volume_free_bytes_nonexistent_path_is_honest_error() {
    // 路径不存在 = 诚实报错（GetDiskFreeSpaceExW ERROR_PATH_NOT_FOUND /
    // statvfs ENOENT），不做「按卷答」的宽容解析——调用方传存在的
    // workspace 目录，错误面留给真异常。
    let base = std::env::temp_dir();
    let bogus = base.join("nemesis-health-m2-nonexistent-dir-probe");
    let err = volume_free_bytes(&bogus).expect_err("不存在路径应报错");
    assert!(!err.is_empty(), "错误信息不得为空: {err}");
}
