//! S10b (quality-hardening goal 冲刺, web 批次 2): ConvRouter::Default impl
//! (delegates to new — empty, no bindings).

use super::*;

#[test]
fn default_impl_yields_empty_router() {
    let r = ConvRouter::default();
    assert!(r.is_empty());
    assert_eq!(r.len(), 0);
    assert!(r.target("agent:main:session:any").is_none());
}
