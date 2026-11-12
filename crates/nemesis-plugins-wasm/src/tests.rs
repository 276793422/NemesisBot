#[test]
fn plugin_tool_full_name_prefix_shape() {
    assert_eq!(super::PLUGIN_TOOL_PREFIX, "plugin.");
    assert_eq!(super::CONTRACT_API_VERSION, 1);
    assert_eq!(
        super::plugin_tool_full_name("demo", "translate"),
        "plugin.demo.translate"
    );
}
