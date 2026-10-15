//! 隐藏验收：形如十六进制字面量的字符串经 dump 后再加载，必须仍是原字符串，
//! 不得被静默解析为整数（emitter round-trip 保真）。
extern crate yaml_rust;
use yaml_rust::{Yaml, YamlEmitter, YamlLoader};

#[test]
fn hex_like_string_survives_dump_reload() {
    let doc = Yaml::String(String::from("0xFF"));
    let mut out = String::new();
    YamlEmitter::new(&mut out)
        .dump(&doc)
        .expect("dump must succeed");
    let docs = YamlLoader::load_from_str(&out).expect("emitted yaml must reparse");
    assert_eq!(docs.len(), 1, "one document expected, got {}", docs.len());
    assert_eq!(
        docs[0],
        Yaml::String(String::from("0xFF")),
        "round-trip must preserve string type; emitted was: {out}"
    );
}
