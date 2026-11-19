//! 冲刺覆盖测试：icons.rs 中现有测试未覆盖的 `pixel()` 数据不足守卫。
//!
//! 生产代码中 Icon 只能通过 `load_from_bytes` 构造（保证 data ==
//! width*height*4），但 Icon 字段是 pub 的，直接构造一个 data 过短的
//! Icon 即可确定性覆盖 `idx + 3 >= self.data.len()` 分支。

use super::*;

#[test]
fn s7_pixel_with_undersized_data_returns_none() {
    // data 只有 3 字节，但 width/height 声称 4x4（需要 64 字节）。
    // 坐标在界内，所以走的是数据长度守卫而不是坐标越界守卫。
    let icon = Icon {
        data: vec![1u8, 2, 3],
        width: 4,
        height: 4,
    };
    // idx = (0*4+0)*4 = 0, idx+3 = 3 >= data.len() = 3 -> None
    assert_eq!(icon.pixel(0, 0), None);
    // idx = (3*4+3)*4 = 60, 60+3 >= 3 -> None
    assert_eq!(icon.pixel(3, 3), None);

    // 对照：正确尺寸的 Icon 正常返回像素。
    let ok_icon = Icon {
        data: vec![10u8, 20, 30, 40],
        width: 1,
        height: 1,
    };
    assert_eq!(ok_icon.pixel(0, 0), Some((10, 20, 30, 40)));
}

/// BUG-20261118-170 回归：`get_scaled` 采样读的 s7 同族守卫。
///
/// 修复前 `get_scaled` 用裸切片 `&data[src_idx..src_idx+4]` 采样，依赖
/// 构造不变量 data == width*height*4；但 Icon 字段 pub 且 derive
/// Deserialize，不变量可绕过——与 `pixel()` 已防御的形态完全同族
/// （见上），畸形 Icon 触发 panic（slice index out of range）。
/// 修复 = 采样点界守卫，越界像素透明填充，输出恒良构。
#[test]
fn s7_get_scaled_with_undersized_data_does_not_panic() {
    // data 只有 3 字节，但 width/height 声称 4x4（需要 64 字节）。
    // 修复前本测试 panic："range end index 4 out of range for slice of length 3"。
    let icon = Icon {
        data: vec![1u8, 2, 3],
        width: 4,
        height: 4,
    };
    let scaled = icon.get_scaled(2, 2);
    // 输出恒良构：target_w*target_h*4 字节、维度随 target。
    assert_eq!(scaled.width, 2);
    assert_eq!(scaled.height, 2);
    assert_eq!(scaled.data.len(), 2 * 2 * 4);
    // 全部采样点（src_idx = 0/8/32/40）都越界（data.len()=3）→ 透明填充。
    assert_eq!(scaled.pixel(0, 0), Some((0, 0, 0, 0)));
    assert_eq!(scaled.pixel(1, 0), Some((0, 0, 0, 0)));
    assert_eq!(scaled.pixel(0, 1), Some((0, 0, 0, 0)));
    assert_eq!(scaled.pixel(1, 1), Some((0, 0, 0, 0)));

    // 对照：正确尺寸 Icon 的缩放采样照常读出真像素（2x2 → 2x2 恒等映射）。
    let ok = Icon {
        data: (0u8..16).collect(),
        width: 2,
        height: 2,
    };
    let s = ok.get_scaled(2, 2);
    assert_eq!(s.pixel(0, 0), Some((0, 1, 2, 3)));
    assert_eq!(s.pixel(1, 1), Some((12, 13, 14, 15)));
}

/// 部分截断形态：data 覆盖前两个像素（8 字节）但声称 2x2（需 16 字节）——
/// 界内采样照常读出，越界采样点透明，其余像素原值保留。
#[test]
fn s7_get_scaled_partial_truncation_keeps_inbounds_pixels() {
    let icon = Icon {
        data: vec![10u8, 11, 12, 13, 20, 21, 22, 23],
        width: 2,
        height: 2,
    };
    let scaled = icon.get_scaled(2, 2);
    assert_eq!(scaled.data.len(), 2 * 2 * 4);
    // (0,0) → src_idx=0、(1,0) → src_idx=4：前两像素字节 0-7 在界内，原值读出。
    assert_eq!(scaled.pixel(0, 0), Some((10, 11, 12, 13)));
    assert_eq!(scaled.pixel(1, 0), Some((20, 21, 22, 23)));
    // (0,1) → src_idx=8、(1,1) → src_idx=12：均越界（len=8）→ 透明。
    assert_eq!(scaled.pixel(0, 1), Some((0, 0, 0, 0)));
    assert_eq!(scaled.pixel(1, 1), Some((0, 0, 0, 0)));
}
