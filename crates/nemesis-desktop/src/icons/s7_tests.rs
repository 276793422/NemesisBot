//! S7 冲刺覆盖测试：icons.rs 中现有测试未覆盖的 `pixel()` 数据不足守卫。
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
