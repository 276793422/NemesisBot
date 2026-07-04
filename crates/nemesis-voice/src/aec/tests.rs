use super::*;

#[test]
fn f32_s16_roundtrip_mid() {
    // f32_to_s16 encodes with ×32767 (symmetric scaling) and s16_to_f32 decodes
    // with ÷32768; combined with `as i16` truncation, the worst-case round-trip
    // error is bounded by ~2/32767 (truncation step + encode/decode scale asymmetry).
    for &v in &[0.0, 0.5, -0.5, 1.0, -1.0, 0.1234, -0.9876] {
        let back = s16_to_f32(f32_to_s16(v));
        assert!((back - v).abs() < 2.0 / 32767.0 + 1e-6, "v={v} back={back}");
    }
}

#[test]
fn f32_to_s16_clamps() {
    // Symmetric ×32767 scaling: -1.0 maps to -32767 (not -32768), keeping the
    // mapping symmetric around 0 and avoiding the asymmetric i16 negative extreme.
    assert_eq!(f32_to_s16(2.0), 32767);
    assert_eq!(f32_to_s16(-2.0), -32767);
    assert_eq!(f32_to_s16(0.0), 0);
}
