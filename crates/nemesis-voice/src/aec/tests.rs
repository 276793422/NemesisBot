use super::*;

#[test]
fn f32_s16_roundtrip_mid() {
    for &v in &[0.0, 0.5, -0.5, 1.0, -1.0, 0.1234, -0.9876] {
        let back = s16_to_f32(f32_to_s16(v));
        assert!((back - v).abs() < 1.0 / 32767.0 + 1e-6, "v={v} back={back}");
    }
}

#[test]
fn f32_to_s16_clamps() {
    assert_eq!(f32_to_s16(2.0), 32767);
    assert_eq!(f32_to_s16(-2.0), -32768);
    assert_eq!(f32_to_s16(0.0), 0);
}
