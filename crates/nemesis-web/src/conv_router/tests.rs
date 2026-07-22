use super::*;

#[test]
fn bind_and_target() {
    let r = ConvRouter::new();
    assert_eq!(r.target("agent:main:session:abc"), None);
    r.bind("agent:main:session:abc", "web:deadbeef");
    assert_eq!(
        r.target("agent:main:session:abc"),
        Some("web:deadbeef".to_string())
    );
}

#[test]
fn latest_wins_overwrites() {
    let r = ConvRouter::new();
    r.bind("agent:main:session:abc", "web:1111");
    r.bind("agent:main:session:abc", "web:2222");
    assert_eq!(
        r.target("agent:main:session:abc"),
        Some("web:2222".to_string())
    );
    assert_eq!(r.len(), 1);
}

#[test]
fn empty_inputs_ignored() {
    let r = ConvRouter::new();
    r.bind("", "web:x");
    r.bind("agent:main:session:abc", "");
    assert!(r.is_empty());
}

#[test]
fn distinct_conversations_independent() {
    let r = ConvRouter::new();
    r.bind("agent:main:session:a", "web:1");
    r.bind("agent:main:session:b", "web:2");
    assert_eq!(r.target("agent:main:session:a"), Some("web:1".to_string()));
    assert_eq!(r.target("agent:main:session:b"), Some("web:2".to_string()));
    assert_eq!(r.len(), 2);
}
